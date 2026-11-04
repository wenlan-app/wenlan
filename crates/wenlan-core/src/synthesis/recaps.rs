// SPDX-License-Identifier: Apache-2.0
//! Recap generation — chronological digests of recent memories.

use crate::db::MemoryDB;
use crate::error::WenlanError;
use crate::llm_provider::{LlmProvider, LlmRequest};
use crate::prompts::PromptRegistry;
use crate::refinery::Nudge;
use std::sync::Arc;

/// Recaps phase — Ambient when any are generated.
pub(crate) fn classify_recaps(generated: usize) -> (Nudge, Option<String>) {
    match generated {
        0 => (Nudge::Silent, None),
        1 => (
            Nudge::Ambient,
            Some("Wenlan steeped a recent activity burst into a recap".to_string()),
        ),
        n => (
            Nudge::Ambient,
            Some(format!(
                "Wenlan steeped {} recent activity bursts into recaps",
                n
            )),
        ),
    }
}

/// The id of the recap (or decision log, by `kind`) for one burst. It is
/// derived from the burst's memories, not drawn at random, so two passes that
/// both find the burst uncovered -- the coverage check, the model call and the
/// write are not one transaction -- write the same row instead of two.
pub(crate) fn burst_recap_id<'a>(
    kind: &str,
    source_ids: impl IntoIterator<Item = &'a str>,
) -> String {
    use sha2::Digest as _;
    let mut ids: Vec<&str> = source_ids.into_iter().collect();
    ids.sort_unstable();
    ids.dedup();
    let mut hasher = sha2::Sha256::new();
    hasher.update(kind.as_bytes());
    for id in ids {
        hasher.update([0u8]);
        hasher.update(id.as_bytes());
    }
    let digest = format!("{:x}", hasher.finalize());
    format!("recap_{}", &digest[..32])
}

/// Public wrapper for generate_recaps, callable from post_ingest module.
pub async fn generate_recaps_public(
    db: &MemoryDB,
    llm: Option<&Arc<dyn LlmProvider>>,
    prompts: &PromptRegistry,
    tuning: &crate::tuning::RefineryConfig,
) -> Result<u32, WenlanError> {
    generate_recaps(db, llm, prompts, tuning).await
}

/// Generate recaps from recent non-recap memories.
/// Groups memories into 30-min activity bursts; generates a recap for each burst
/// with 3+ memories that isn't already covered by an existing recap.
pub(crate) async fn generate_recaps(
    db: &MemoryDB,
    llm: Option<&Arc<dyn LlmProvider>>,
    prompts: &PromptRegistry,
    tuning: &crate::tuning::RefineryConfig,
) -> Result<u32, WenlanError> {
    let now = chrono::Utc::now().timestamp();
    let lookback_start = now - tuning.recap_lookback_secs;

    // Get all non-recap memories in the lookback window
    let recent = db
        .get_recent_memories_for_recap(lookback_start, 100)
        .await?;
    if recent.len() < tuning.min_memories_for_recap {
        return Ok(0);
    }

    // Group into bursts by 30-min gaps
    let bursts = crate::refinery::group_into_bursts(&recent);
    let mut recaps_generated = 0u32;

    for burst in &bursts {
        if burst.len() < tuning.min_memories_for_recap {
            continue;
        }

        // Determine burst time range
        let burst_start = burst.iter().map(|m| m.3).min().unwrap_or(0);
        let burst_end = burst.iter().map(|m| m.3).max().unwrap_or(0);

        // Skip if a recap already covers this burst
        if db.has_recap_covering_range(burst_start, burst_end).await? {
            continue;
        }

        // Group content for the LLM
        let contents: Vec<String> = burst
            .iter()
            .map(|(_, content, space, _)| match space {
                Some(d) => format!("[{}] {}", d, content),
                None => content.clone(),
            })
            .collect();

        // Determine the dominant space
        let mut domain_counts: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for (_, _, space, _) in burst.iter() {
            if let Some(d) = space {
                *domain_counts.entry(d.clone()).or_insert(0) += 1;
            }
        }
        let dominant_domain = domain_counts
            .into_iter()
            .max_by_key(|(_, count)| *count)
            .map(|(d, _)| d);

        // Build raw context from burst
        let burst_slice: Vec<(String, String, Option<String>, i64)> =
            burst.iter().map(|m| (*m).clone()).collect();
        let raw_context =
            crate::refinery::build_burst_context(&burst_slice, burst_start, burst_end);

        // Send to LLM for synthesis (or generate without LLM)
        let (recap_summary, recap_content) = if let Some(llm) = llm {
            let domain_hint = dominant_domain.as_deref().unwrap_or("general");
            let combined = contents
                .iter()
                .enumerate()
                .map(|(i, c)| format!("{}. {}", i + 1, c))
                .collect::<Vec<_>>()
                .join("\n");

            let response = llm
                .generate(LlmRequest {
                    system_prompt: Some(
                        prompts.detect_pattern.replace("{domain_hint}", domain_hint),
                    ),
                    user_prompt: combined,
                    max_tokens: 128,
                    temperature: 0.1,
                    label: None,
                    timeout_secs: None,
                })
                .await;

            match response {
                Ok(output)
                    if output.trim().to_lowercase() != "null" && !output.trim().is_empty() =>
                {
                    let cleaned = crate::llm_provider::strip_think_tags(&output);
                    (cleaned.trim().to_string(), raw_context)
                }
                _ => generate_simple_recap(&burst_slice, burst_start, burst_end),
            }
        } else {
            generate_simple_recap(&burst_slice, burst_start, burst_end)
        };

        // Store the recap — set last_modified = burst_end so it sorts next to its source memories
        // Track source memory IDs so the detail view can show exact sources
        let source_ids: Vec<&str> = burst.iter().map(|(id, _, _, _)| id.as_str()).collect();
        let recap_id = burst_recap_id("recap", source_ids.iter().copied());
        let structured = serde_json::json!({ "source_ids": source_ids }).to_string();
        // Generate a short topic title via LLM; fall back to burst header
        let burst_header: String = recap_content
            .lines()
            .take(2)
            .collect::<Vec<_>>()
            .join(" · ");
        let title = if let Some(llm) = llm {
            crate::refinery::generate_short_title(llm, &recap_content)
                .await
                .unwrap_or_else(|| burst_header.chars().take(80).collect())
        } else {
            burst_header.chars().take(80).collect()
        };
        let doc = crate::sources::RawDocument {
            source: "memory".to_string(),
            source_id: recap_id,
            title,
            summary: Some(recap_summary),
            content: recap_content,
            url: None,
            last_modified: burst_end,
            metadata: std::collections::HashMap::new(),
            memory_type: Some("fact".to_string()),
            space: dominant_domain,
            source_agent: Some("refinery".to_string()),
            confidence: Some(0.5),
            confirmed: None,
            supersedes: None,
            pending_revision: false,
            is_recap: true,
            structured_fields: Some(structured),
            ..Default::default()
        };
        db.upsert_documents(vec![doc]).await?;
        recaps_generated += 1;
    }

    Ok(recaps_generated)
}

/// Generate a simple recap without LLM (fallback).
/// Returns (summary, raw_context).
pub(crate) fn generate_simple_recap(
    memories: &[(String, String, Option<String>, i64)],
    burst_start: i64,
    burst_end: i64,
) -> (String, String) {
    let count = memories.len();
    let domains: Vec<String> = memories
        .iter()
        .filter_map(|(_, _, d, _)| d.clone())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();

    let summary = format!(
        "Recap: {} memories stored{}",
        count,
        if domains.is_empty() {
            String::new()
        } else {
            format!(" ({})", domains.join(", "))
        },
    );

    let raw_context = crate::refinery::build_burst_context(memories, burst_start, burst_end);

    (summary, raw_context)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::tests::test_db;
    use crate::read_scope::ReadScope;
    use crate::sources::RawDocument;

    fn make_memory(source_id: &str, content: &str, memory_type: &str, space: &str) -> RawDocument {
        RawDocument {
            source_id: source_id.to_string(),
            content: content.to_string(),
            source: "memory".to_string(),
            title: content.chars().take(40).collect(),
            memory_type: Some(memory_type.to_string()),
            space: Some(space.to_string()),
            confidence: Some(0.7),
            last_modified: chrono::Utc::now().timestamp(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn test_generate_recaps_from_recent_memories() {
        let (db, _dir) = test_db().await;

        // Insert 5 recent memories (enough to trigger recap)
        let now = chrono::Utc::now().timestamp();
        for i in 0..5 {
            let mut doc = make_memory(
                &format!("recent_{}", i),
                &format!("Working on feature {} for the Wenlan project", i),
                "fact",
                "engineering",
            );
            doc.last_modified = now - 60 * i as i64; // spread across last 5 minutes
            db.upsert_documents(vec![doc]).await.unwrap();
        }

        // No recap should exist yet
        assert!(!db.has_recap_since(now - 86400_i64).await.unwrap());

        // Generate recaps (no LLM — uses simple fallback)
        let generated = generate_recaps(
            &db,
            None,
            &PromptRegistry::default(),
            &crate::tuning::RefineryConfig::default(),
        )
        .await
        .unwrap();
        assert_eq!(generated, 1, "should generate 1 recap");

        // Recap should now exist
        assert!(db.has_recap_since(now - 86400_i64).await.unwrap());

        // Running again should skip (recap already exists)
        let generated2 = generate_recaps(
            &db,
            None,
            &PromptRegistry::default(),
            &crate::tuning::RefineryConfig::default(),
        )
        .await
        .unwrap();
        assert_eq!(generated2, 0, "should not generate duplicate recap");

        // The recap should be searchable (stored as memory_type=fact, is_recap=1)
        let results = db
            .search_memory(
                "Wenlan project feature",
                10,
                Some("fact"),
                &ReadScope::Global,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();
        assert!(!results.is_empty(), "recap should be searchable as fact");
    }

    /// A burst from earlier in the day gets one recap, however many times the
    /// recap pass runs over it. The recap is stamped at the burst's end, not at
    /// the time it was written, so the coverage check must look there.
    #[tokio::test]
    async fn an_earlier_burst_is_recapped_once() {
        let (db, _dir) = test_db().await;
        let burst_end = chrono::Utc::now().timestamp() - 3 * 3600;
        for i in 0..5 {
            let mut doc = make_memory(
                &format!("earlier_{i}"),
                &format!("Earlier work on feature {i} for the Wenlan project"),
                "fact",
                "engineering",
            );
            doc.last_modified = burst_end - 60 * i64::from(i);
            db.upsert_documents(vec![doc]).await.unwrap();
        }
        let tuning = crate::tuning::RefineryConfig::default();
        let prompts = PromptRegistry::default();

        assert_eq!(
            generate_recaps(&db, None, &prompts, &tuning).await.unwrap(),
            1
        );
        assert_eq!(
            generate_recaps(&db, None, &prompts, &tuning).await.unwrap(),
            0,
            "the second pass must see the first recap"
        );

        // A racing pass that missed the first recap writes the same id, so
        // the burst still has one recap.
        let ids: Vec<String> = (0..5).map(|i| format!("earlier_{i}")).collect();
        let recap_id = burst_recap_id("recap", ids.iter().map(String::as_str));
        let racing = crate::sources::RawDocument {
            source: "memory".to_string(),
            source_id: recap_id.clone(),
            title: "racing recap".to_string(),
            content: "racing recap body".to_string(),
            last_modified: burst_end,
            is_recap: true,
            ..Default::default()
        };
        db.upsert_documents(vec![racing]).await.unwrap();
        let conn = db.test_primary_session().await;
        let mut rows = conn
            .query(
                "SELECT COUNT(DISTINCT source_id), MAX(source_id) FROM memories WHERE is_recap = 1",
                (),
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<i64>(0).unwrap(), 1);
        assert_eq!(row.get::<String>(1).unwrap(), recap_id);
    }

    #[test]
    fn a_burst_has_one_recap_id_whatever_the_order() {
        let a = burst_recap_id("recap", ["m2", "m1", "m3"]);
        assert_eq!(a, burst_recap_id("recap", ["m3", "m1", "m2"]));
        assert!(a.starts_with("recap_"));
        assert_ne!(a, burst_recap_id("recap", ["m1", "m2"]));
        assert_ne!(a, burst_recap_id("decision", ["m1", "m2", "m3"]));
    }

    #[tokio::test]
    async fn test_recap_separates_content_and_summary() {
        let (db, _dir) = test_db().await;

        let now = chrono::Utc::now().timestamp();
        for i in 0..4 {
            let mut doc = make_memory(
                &format!("sep_{}", i),
                &format!("Debugging issue {} in the auth module", i),
                "fact",
                "engineering",
            );
            doc.last_modified = now - 60 * i as i64;
            db.upsert_documents(vec![doc]).await.unwrap();
        }

        let generated = generate_recaps(
            &db,
            None,
            &PromptRegistry::default(),
            &crate::tuning::RefineryConfig::default(),
        )
        .await
        .unwrap();
        assert_eq!(generated, 1);

        // Find the recap via search (stored as memory_type=fact, is_recap=1)
        let results = db
            .search_memory(
                "auth module debugging",
                10,
                Some("fact"),
                &ReadScope::Global,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();
        assert!(!results.is_empty(), "recap should be searchable as fact");

        // Find the recap result (source_id starts with "recap_")
        let recap_result = results
            .iter()
            .find(|r| r.source_id.starts_with("recap_"))
            .expect("should find a recap entry in results");

        // Fetch full detail to verify content vs summary separation
        let detail = db
            .get_memory_detail(&recap_result.source_id)
            .await
            .unwrap()
            .unwrap();
        assert!(detail.summary.is_some(), "recap should have a summary");
        let summary = detail.summary.unwrap();
        assert!(
            summary.starts_with("Recap:"),
            "summary should be the concise line"
        );
        assert!(
            detail.content.contains("Activity burst:"),
            "content should be the structured context"
        );
        assert_ne!(detail.content, summary, "content and summary must differ");
    }

    #[tokio::test]
    async fn test_no_recap_when_too_few_memories() {
        let (db, _dir) = test_db().await;

        // Only 2 memories — below threshold
        let now = chrono::Utc::now().timestamp();
        for i in 0..2 {
            let mut doc = make_memory(&format!("few_{}", i), "Some content", "fact", "eng");
            doc.last_modified = now - 60;
            db.upsert_documents(vec![doc]).await.unwrap();
        }

        let generated = generate_recaps(
            &db,
            None,
            &PromptRegistry::default(),
            &crate::tuning::RefineryConfig::default(),
        )
        .await
        .unwrap();
        assert_eq!(
            generated, 0,
            "should not generate recap with too few memories"
        );
    }

    #[test]
    fn test_classify_recaps_silent_when_zero() {
        assert_eq!(classify_recaps(0), (Nudge::Silent, None));
    }

    #[test]
    fn test_classify_recaps_ambient_when_generated() {
        let (nudge, headline) = classify_recaps(1);
        assert_eq!(nudge, Nudge::Ambient);
        let h = headline.expect("headline should be set");
        assert!(
            h.contains("steeped"),
            "headline should use steep vocabulary: {}",
            h
        );
    }

    #[test]
    fn test_classify_recaps_plural_form() {
        let (_, headline) = classify_recaps(3);
        let h = headline.expect("headline should be set");
        assert!(
            h.contains('3'),
            "multi-recap headline should mention the count: {}",
            h
        );
    }
}
