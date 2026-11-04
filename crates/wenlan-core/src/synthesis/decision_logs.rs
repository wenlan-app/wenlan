// SPDX-License-Identifier: Apache-2.0
//! Decision-log generation — traceback compilation for decisions.

use crate::db::MemoryDB;
use crate::error::WenlanError;
use crate::llm_provider::{LlmProvider, LlmRequest};
use crate::prompts::PromptRegistry;
use crate::refinery::{build_burst_context, group_into_bursts, Nudge};
use std::sync::Arc;

/// Decision logs phase — Ambient when any logs are generated.
pub(crate) fn classify_decision_logs(generated: usize) -> (Nudge, Option<String>) {
    match generated {
        0 => (Nudge::Silent, None),
        1 => (
            Nudge::Ambient,
            Some("Wenlan narrated a week's worth of decisions into a log".to_string()),
        ),
        n => (
            Nudge::Ambient,
            Some(format!("Wenlan narrated decisions into {} logs", n)),
        ),
    }
}

/// Generate decision logs: recap-like summaries for clusters of decision memories.
/// Groups recent decision memories and generates a summary recap if not already covered.
pub(crate) async fn generate_decision_logs(
    db: &MemoryDB,
    llm: Option<&Arc<dyn LlmProvider>>,
    prompts: &PromptRegistry,
    tuning: &crate::tuning::RefineryConfig,
) -> Result<usize, WenlanError> {
    let now = chrono::Utc::now().timestamp();
    let lookback_start = now - tuning.recap_lookback_secs;

    // Get recent decision memories
    let recent_decisions = db
        .get_recent_decisions_for_recap(lookback_start, 50)
        .await?;
    if recent_decisions.len() < tuning.min_memories_for_recap {
        return Ok(0);
    }

    let bursts = group_into_bursts(&recent_decisions);
    let mut generated = 0usize;

    for burst in &bursts {
        if burst.len() < tuning.min_memories_for_recap {
            continue;
        }

        let burst_start = burst.iter().map(|m| m.3).min().unwrap_or(0);
        let burst_end = burst.iter().map(|m| m.3).max().unwrap_or(0);

        if db.has_recap_covering_range(burst_start, burst_end).await? {
            continue;
        }

        let burst_slice: Vec<(String, String, Option<String>, i64)> =
            burst.iter().map(|m| (*m).clone()).collect();
        let raw_context = build_burst_context(&burst_slice, burst_start, burst_end);

        let (summary, content) = if let Some(llm) = llm {
            let combined = burst
                .iter()
                .enumerate()
                .map(|(i, (_, content, space, _))| {
                    let d = space.as_deref().unwrap_or("general");
                    format!("{}. [{}] {}", i + 1, d, content)
                })
                .collect::<Vec<_>>()
                .join("\n");

            let response = llm
                .generate(LlmRequest {
                    system_prompt: Some(prompts.summarize_decisions.clone()),
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
                _ => {
                    let summary = format!("Decision log: {} decisions recorded", burst.len());
                    (summary, raw_context)
                }
            }
        } else {
            let summary = format!("Decision log: {} decisions recorded", burst.len());
            (summary, raw_context)
        };

        let recap_id = crate::synthesis::recaps::burst_recap_id(
            "decision",
            burst.iter().map(|(id, _, _, _)| id.as_str()),
        );
        let doc = crate::sources::RawDocument {
            source: "memory".to_string(),
            source_id: recap_id,
            title: summary.chars().take(80).collect(),
            summary: Some(summary),
            content,
            url: None,
            last_modified: burst_end,
            metadata: std::collections::HashMap::new(),
            memory_type: Some("decision".to_string()),
            space: None,
            source_agent: Some("refinery".to_string()),
            confidence: Some(0.5),
            confirmed: None,
            supersedes: None,
            pending_revision: false,
            is_recap: true,
            ..Default::default()
        };
        db.upsert_documents(vec![doc]).await?;
        generated += 1;
    }

    if generated > 0 {
        log::info!("[refinery] generated {} decision logs", generated);
    }
    Ok(generated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::tests::test_db;
    use crate::prompts::PromptRegistry;
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
    async fn test_decision_logs_no_decisions() {
        let (db, _dir) = test_db().await;

        // No decision memories → should generate 0 logs
        let generated = generate_decision_logs(
            &db,
            None,
            &PromptRegistry::default(),
            &crate::tuning::RefineryConfig::default(),
        )
        .await
        .unwrap();
        assert_eq!(generated, 0);
    }

    #[tokio::test]
    async fn test_decision_logs_with_decisions() {
        let (db, _dir) = test_db().await;

        let now = chrono::Utc::now().timestamp();
        for i in 0..4 {
            let mut doc = make_memory(
                &format!("dec_{}", i),
                &format!("Decided to use approach {} for the backend refactor", i),
                "decision",
                "engineering",
            );
            doc.last_modified = now - 60 * i as i64;
            db.upsert_documents(vec![doc]).await.unwrap();
        }

        let generated = generate_decision_logs(
            &db,
            None,
            &PromptRegistry::default(),
            &crate::tuning::RefineryConfig::default(),
        )
        .await
        .unwrap();
        assert_eq!(generated, 1, "should generate 1 decision log");

        // Verify the decision log is stored as decision type with is_recap=true
        let results = db
            .search_memory(
                "backend refactor approach",
                10,
                Some("decision"),
                &ReadScope::Global,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();
        let recap = results.iter().find(|r| r.source_id.starts_with("recap_"));
        assert!(recap.is_some(), "should find a decision log recap");
    }

    #[test]
    fn test_classify_decision_logs_silent_when_zero() {
        assert_eq!(classify_decision_logs(0), (Nudge::Silent, None));
    }

    #[test]
    fn test_classify_decision_logs_ambient_when_generated() {
        let (nudge, headline) = classify_decision_logs(1);
        assert_eq!(nudge, Nudge::Ambient);
        let h = headline.expect("headline should be set");
        assert!(
            h.contains("decision"),
            "headline should mention decisions: {}",
            h
        );
    }

    #[test]
    fn test_classify_decision_logs_plural_form() {
        let (_, headline) = classify_decision_logs(3);
        let h = headline.expect("headline should be set");
        assert!(
            h.contains('3'),
            "plural headline should mention count: {}",
            h
        );
    }
}
