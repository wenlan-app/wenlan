// SPDX-License-Identifier: Apache-2.0
//! Distillation phase — turn memory clusters into structured pages.
//!
//! This module owns the synthesis side of the refinery: clustering memories,
//! merging/splitting clusters via LLM, and recompiling pages from
//! source memories. Re-exported from `crate::refinery` for API stability.

use crate::db::MemoryDB;
use crate::error::WenlanError;
use crate::llm_provider::{LlmBackend, LlmError, LlmProvider, LlmRequest};
use crate::post_write::{page_write, PageWrite};
use crate::prompts::PromptRegistry;
use crate::refinery::helpers::{
    is_all_generic_tokens, looks_like_code, looks_like_commit_message, looks_like_markup_styled,
    looks_like_path, looks_like_short_hash, looks_like_uuid,
};
use crate::sources::StabilityTier;
use crate::synthesis::refinement_queue::{resolve_proposal, ResolveStatus};
use crate::truth_adapter::{filter_page_refs, page_write_permit};
use crate::truth_contract::TruthGrant;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use wenlan_types::requests::{CreateConceptRequest, UpdatePageRequest};

const FORMATION_SWEEP_THRESHOLDS: [f64; 4] = [0.55, 0.60, 0.65, 0.70];
const DISTILL_CLUSTER_DOCUMENT_MAX_SHARE_NUMERATOR: usize = 1;
const DISTILL_CLUSTER_DOCUMENT_MAX_SHARE_DENOMINATOR: usize = 2;
const EXISTING_TITLES_HINT_LIMIT: usize = 100;
const EXISTING_TITLE_CHAR_CAP: usize = 128;
const PAGE_TOPIC_CHAR_CAP: usize = 192;
const NO_SPACE_KEY: &str = "(none)";

/// What a distillation pass is scoped to. Resolved from a free-form string
/// supplied by the user (page id, entity name, or domain value).
#[derive(Debug, Clone)]
pub enum DistillTarget {
    /// Existing page — re-distill from its current sources.
    Page(String),
    /// Scope clustering to memories belonging to one entity.
    Entity { id: String, name: String },
    /// Scope clustering to memories with a given domain.
    Domain(String),
}

/// Resolve a free-form target string into a `DistillTarget`.
///
/// Resolution order:
/// 1. Strings starting with `page_` or `concept_` are treated as page ids.
/// 2. Exact entity name match (via `MemoryDB::resolve_entity_by_name`).
/// 3. Exact registered space match.
/// 4. Otherwise `None` — caller decides whether to fail loudly or fall through.
pub async fn resolve_distill_target(
    db: &MemoryDB,
    raw: &str,
) -> Result<Option<DistillTarget>, WenlanError> {
    let s = raw.trim();
    if s.is_empty() {
        return Ok(None);
    }
    if s.starts_with("page_") || s.starts_with("concept_") {
        return Ok(Some(DistillTarget::Page(s.to_string())));
    }
    if let Some(id) = db.resolve_entity_by_name(s).await? {
        return Ok(Some(DistillTarget::Entity {
            id,
            name: s.to_string(),
        }));
    }
    if db.registered_space_or_none(Some(s)).await?.is_some() {
        return Ok(Some(DistillTarget::Domain(s.to_string())));
    }
    Ok(None)
}

/// Build the "existing page titles" hint prefix that gets prepended to
/// every page compile user prompt so the LLM emits exact-match `[[Title]]`
/// wikilinks instead of inventing labels. Returns an empty string when
/// the page set is empty or the DB call errors (best-effort — the worst
/// case is the LLM invents a label that the orphan-by-count feed will
/// surface later). Capped to keep prompts bounded on large vaults.
pub(crate) async fn build_existing_titles_hint(
    db: &MemoryDB,
    topic: &str,
    workspace: Option<&str>,
) -> String {
    let capped_topic: String = topic.chars().take(PAGE_TOPIC_CHAR_CAP).collect();
    let mut titles = db
        .list_relevant_active_page_titles(&capped_topic, workspace, EXISTING_TITLES_HINT_LIMIT)
        .await
        .unwrap_or_default();
    if titles.is_empty() {
        titles = db
            .list_active_page_titles_scoped(workspace, EXISTING_TITLES_HINT_LIMIT)
            .await
            .unwrap_or_default();
    }
    if titles.is_empty() {
        return String::new();
    }
    // An unsupported page is not a safe wikilink target: the title would ride
    // into another page's prose, where any Full-visibility reader sees it with
    // no gate in front of it. `Automatic` because a distill prompt build has no
    // caller grant to pass, same reasoning as `page_write_permit`.
    let titles = filter_page_refs(db, &TruthGrant::Automatic, titles, |item| item.0.as_str())
        .await
        .unwrap_or_default();
    if titles.is_empty() {
        return String::new();
    }
    let formatted = titles
        .iter()
        .map(|(_, title)| {
            let capped: String = title.chars().take(EXISTING_TITLE_CHAR_CAP).collect();
            format!("[[{capped}]]")
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "Existing pages you may reference with exact-match wikilinks: {formatted}\n\
         Use these labels verbatim when linking; only invent a new label \
         when the topic isn't already covered.\n\n"
    )
}

/// Keep refresh-time wikilink suggestions grounded in the supplied sources or
/// explicitly linked basis notes. Titles are navigation labels only; matching
/// a title does not make that page evidence for the synthesis.
async fn build_refresh_titles_hint(
    db: &MemoryDB,
    topic: &str,
    workspace: Option<&str>,
    memories_block: &str,
) -> String {
    let capped_topic: String = topic.chars().take(PAGE_TOPIC_CHAR_CAP).collect();
    let mut titles = db
        .list_relevant_active_page_titles(&capped_topic, workspace, EXISTING_TITLES_HINT_LIMIT)
        .await
        .unwrap_or_default();
    if titles.is_empty() {
        titles = db
            .list_active_page_titles_scoped(workspace, EXISTING_TITLES_HINT_LIMIT)
            .await
            .unwrap_or_default();
    }
    let context = memories_block.to_lowercase();
    titles.retain(|(_, title)| contains_whole_title(&context, title));
    if titles.is_empty() {
        return String::new();
    }
    let titles = filter_page_refs(db, &TruthGrant::Automatic, titles, |item| item.0.as_str())
        .await
        .unwrap_or_default();
    if titles.is_empty() {
        return String::new();
    }
    let formatted = titles
        .iter()
        .map(|(_, title)| {
            let capped: String = title.chars().take(EXISTING_TITLE_CHAR_CAP).collect();
            format!("[[{capped}]]")
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "Page titles mentioned in this page's sources or linked basis (link labels only, not evidence): {formatted}\n\n"
    )
}

/// Case-insensitive whole-title mention test. Unicode letters and digits are
/// word characters, so a short title such as "AI" does not match inside
/// "paid" while `[[AI]]` remains a valid mention.
fn contains_whole_title(context: &str, title: &str) -> bool {
    let title = title.to_lowercase();
    if title.is_empty() {
        return false;
    }
    let title_chars: Vec<char> = title.chars().collect();
    if title_chars.len() >= 2 && title_chars.iter().all(|ch| is_han(*ch)) {
        // Han titles are commonly written directly next to other Han text,
        // without spaces or punctuation to establish word boundaries.
        return context.contains(&title);
    }
    context.match_indices(&title).any(|(start, matched)| {
        let end = start + matched.len();
        let left_is_word = context[..start]
            .chars()
            .next_back()
            .is_some_and(char::is_alphanumeric);
        let right_is_word = context[end..]
            .chars()
            .next()
            .is_some_and(char::is_alphanumeric);
        !left_is_word && !right_is_word
    })
}

fn is_han(ch: char) -> bool {
    matches!(
        ch as u32,
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x2FA1F
    )
}

fn page_workspace(page: &crate::pages::Page) -> Option<&str> {
    page.workspace.as_deref().or(page.space.as_deref())
}

pub(crate) async fn build_page_compile_user_prompt(
    db: &MemoryDB,
    topic: &str,
    workspace: Option<&str>,
    memories_block: &str,
) -> String {
    let titles_hint = build_existing_titles_hint(db, topic, workspace).await;
    let capped_topic: String = topic.chars().take(PAGE_TOPIC_CHAR_CAP).collect();
    format!("{titles_hint}Topic: {capped_topic}\n\n{memories_block}")
}

/// LLM cluster refinement: for entities with multiple clusters, ask the LLM to merge/rename.
pub(crate) async fn refine_clusters_with_llm(
    llm: &Arc<dyn LlmProvider>,
    prompts: &PromptRegistry,
    clusters: Vec<crate::db::DistillationCluster>,
    token_limit: usize,
) -> Vec<crate::db::DistillationCluster> {
    // Group clusters by entity
    let mut by_entity: std::collections::HashMap<String, Vec<usize>> =
        std::collections::HashMap::new();
    for (i, c) in clusters.iter().enumerate() {
        let key = c
            .entity_name
            .as_deref()
            .or(c.entity_id.as_deref())
            .unwrap_or("unlinked")
            .to_string();
        by_entity.entry(key).or_default().push(i);
    }

    // Only refine entities with 2+ clusters (single clusters = nothing to merge)
    let entities_to_refine: Vec<(String, Vec<usize>)> = by_entity
        .into_iter()
        .filter(|(_, indices)| indices.len() >= 2)
        .collect();

    if entities_to_refine.is_empty() {
        return clusters;
    }

    let mut result = clusters;
    let mut merged_indices: std::collections::HashSet<usize> = std::collections::HashSet::new();

    for (entity, indices) in &entities_to_refine {
        // Build one-line summaries for each cluster
        let summaries: String = indices
            .iter()
            .enumerate()
            .map(|(j, &idx)| {
                let c = &result[idx];
                let preview: String = c
                    .contents
                    .iter()
                    .take(3)
                    .map(|s| {
                        let trimmed: String = s.chars().take(60).collect();
                        format!("\"{}...\"", trimmed)
                    })
                    .collect::<Vec<_>>()
                    .join(" / ");
                format!("{}. [{} memories] {}", j, c.source_ids.len(), preview)
            })
            .collect::<Vec<_>>()
            .join("\n");

        let user_prompt = format!("Entity: {}\n\n{}", entity, summaries);

        let response = llm
            .generate(LlmRequest {
                system_prompt: Some(prompts.refine_clusters.clone()),
                user_prompt,
                max_tokens: 512,
                temperature: 0.2,
                label: None,
                timeout_secs: None,
            })
            .await;

        match response {
            Ok(raw) => {
                let clean = crate::llm_provider::strip_think_tags(&raw);
                if let Some(json_str) = crate::engine::extract_json_array(&clean) {
                    if let Ok(actions) = serde_json::from_str::<Vec<serde_json::Value>>(&json_str) {
                        for action in &actions {
                            let act = action
                                .get("action")
                                .and_then(|v| v.as_str())
                                .unwrap_or("keep");
                            match act {
                                "merge" => {
                                    if let Some(to_merge) =
                                        action.get("clusters").and_then(|v| v.as_array())
                                    {
                                        let mut merge_indices: Vec<usize> = to_merge
                                            .iter()
                                            .filter_map(|v| v.as_u64().map(|n| n as usize))
                                            .filter(|&j| j < indices.len())
                                            .collect();
                                        merge_indices.sort_unstable();
                                        merge_indices.dedup();
                                        if merge_indices.len() >= 2 {
                                            // Guard: don't merge if the result would exceed
                                            // the token limit that sub_cluster_by_tokens split
                                            // on. This prevents the LLM from re-merging
                                            // sub-clusters into a monster that OOMs distillation.
                                            let merged_tokens: usize = merge_indices
                                                .iter()
                                                .map(|&j| result[indices[j]].estimated_tokens)
                                                .sum();
                                            if merged_tokens > token_limit {
                                                log::info!(
                                                    "[refine] skipping merge for '{}' — merged tokens {} > limit {}",
                                                    entity,
                                                    merged_tokens,
                                                    token_limit
                                                );
                                            } else {
                                                // Merge: combine all into the first
                                                let first = indices[merge_indices[0]];
                                                for &j in &merge_indices[1..] {
                                                    let idx = indices[j];
                                                    let extra_ids = result[idx].source_ids.clone();
                                                    let extra_contents =
                                                        result[idx].contents.clone();
                                                    result[first].source_ids.extend(extra_ids);
                                                    result[first].contents.extend(extra_contents);
                                                    result[first].estimated_tokens +=
                                                        result[idx].estimated_tokens;
                                                    merged_indices.insert(idx);
                                                }
                                                if let Some(title) =
                                                    action.get("title").and_then(|v| v.as_str())
                                                {
                                                    result[first].entity_name =
                                                        Some(title.to_string());
                                                }
                                                log::info!(
                                                    "[refine] merged {} clusters for '{}'",
                                                    merge_indices.len(),
                                                    entity
                                                );
                                            } // close else (token guard)
                                        }
                                    }
                                }
                                "rename" => {
                                    if let (Some(j), Some(title)) = (
                                        action
                                            .get("cluster")
                                            .and_then(|v| v.as_u64().map(|n| n as usize)),
                                        action.get("title").and_then(|v| v.as_str()),
                                    ) {
                                        if j < indices.len() {
                                            result[indices[j]].entity_name =
                                                Some(title.to_string());
                                            log::info!(
                                                "[refine] renamed cluster {} to '{}' for '{}'",
                                                j,
                                                title,
                                                entity
                                            );
                                        }
                                    }
                                }
                                // Anything else (including the `split` action
                                // the prompt used to offer) is a keep. `split`
                                // was never implemented and landed here
                                // silently, so it is no longer offered — see
                                // the note above `REFINE_CLUSTERS` in
                                // `prompts/defaults.rs`. An overridden prompt
                                // can still emit it; keeping is the same
                                // behavior it always had.
                                _ => {}
                            }
                        }
                    }
                }
            }
            Err(e) => log::warn!("[refine] LLM refinement failed for '{}': {}", entity, e),
        }
    }

    // Remove merged clusters
    if !merged_indices.is_empty() {
        result = result
            .into_iter()
            .enumerate()
            .filter(|(i, _)| !merged_indices.contains(i))
            .map(|(_, c)| c)
            .collect();
    }

    result
}

async fn cap_document_majority_clusters(
    db: &MemoryDB,
    clusters: Vec<crate::db::DistillationCluster>,
    min_cluster_size: usize,
) -> Result<Vec<crate::db::DistillationCluster>, WenlanError> {
    let hashes = load_cluster_content_hashes(db, &clusters).await?;
    Ok(clusters
        .into_iter()
        .filter_map(|cluster| cap_one_document_majority(cluster, &hashes, min_cluster_size))
        .collect())
}

async fn load_cluster_content_hashes(
    db: &MemoryDB,
    clusters: &[crate::db::DistillationCluster],
) -> Result<HashMap<String, Option<String>>, WenlanError> {
    let mut source_ids: Vec<String> = clusters
        .iter()
        .flat_map(|cluster| cluster.source_ids.iter().cloned())
        .collect();
    source_ids.sort();
    source_ids.dedup();
    if source_ids.is_empty() {
        return Ok(HashMap::new());
    }

    db.memory_content_hashes_for_source_ids(&source_ids).await
}

fn cap_one_document_majority(
    cluster: crate::db::DistillationCluster,
    hashes: &HashMap<String, Option<String>>,
    min_cluster_size: usize,
) -> Option<crate::db::DistillationCluster> {
    let original_len = cluster.source_ids.len();
    let mut retained: Vec<usize> = (0..original_len).collect();

    loop {
        if retained.len() < min_cluster_size {
            log::info!(
                "[distill] dropping document-heavy cluster after cap: {} -> {} memories (< min {})",
                original_len,
                retained.len(),
                min_cluster_size
            );
            return None;
        }

        let Some(offending_hash) = document_majority_hash(&cluster.source_ids, hashes, &retained)
        else {
            break;
        };

        if let Some(pos) = retained.iter().rposition(|&idx| {
            hash_for_source_id(&cluster.source_ids[idx], hashes) == Some(offending_hash.as_str())
        }) {
            retained.remove(pos);
        } else {
            break;
        }
    }

    if retained.len() == original_len {
        return Some(cluster);
    }

    let source_ids: Vec<String> = retained
        .iter()
        .map(|&idx| cluster.source_ids[idx].clone())
        .collect();
    let contents: Vec<String> = retained
        .iter()
        .map(|&idx| cluster.contents.get(idx).cloned().unwrap_or_default())
        .collect();
    let estimated_tokens = estimate_cluster_tokens(&contents);
    log::info!(
        "[distill] capped document-heavy cluster: {} -> {} memories",
        original_len,
        source_ids.len()
    );

    Some(crate::db::DistillationCluster {
        source_ids,
        contents,
        entity_id: cluster.entity_id,
        entity_name: cluster.entity_name,
        space: cluster.space,
        estimated_tokens,
        centroid_embedding: cluster.centroid_embedding,
    })
}

fn document_majority_hash(
    source_ids: &[String],
    hashes: &HashMap<String, Option<String>>,
    retained: &[usize],
) -> Option<String> {
    let total = retained.len();
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for &idx in retained {
        if let Some(hash) = hash_for_source_id(&source_ids[idx], hashes) {
            *counts.entry(hash).or_insert(0) += 1;
        }
    }

    counts
        .into_iter()
        .filter(|(_, count)| {
            count * DISTILL_CLUSTER_DOCUMENT_MAX_SHARE_DENOMINATOR
                >= total * DISTILL_CLUSTER_DOCUMENT_MAX_SHARE_NUMERATOR
        })
        .max_by_key(|(_, count)| *count)
        .map(|(hash, _)| hash.to_string())
}

fn hash_for_source_id<'a>(
    source_id: &str,
    hashes: &'a HashMap<String, Option<String>>,
) -> Option<&'a str> {
    hashes
        .get(source_id)
        .and_then(|hash| hash.as_deref())
        .filter(|hash| !hash.is_empty())
}

fn estimate_cluster_tokens(contents: &[String]) -> usize {
    contents
        .iter()
        .map(|content| content.len() / 4 + 15)
        .sum::<usize>()
        + 100
}

/// Process a single distillation cluster.
///
/// Returns `Ok(true)` if a page was created, `Ok(false)` if the cluster was skipped.
/// Extracted from `distill_pages` to enable parallel cluster processing via
/// `DISTILL_CLUSTER_CONCURRENCY`.
/// The summary of a distilled page: the first sentence of its first prose
/// line, skipping headings, blank lines, and everything under an
/// "Open Questions" heading. Citation markers like `[3]` are removed, and so
/// is a leading "TLDR:" label when the model wrote one: the label is not part
/// of the sentence and reads as noise in the app's pull quote. A page with no
/// prose outside "Open Questions" gets no summary rather than a question
/// standing in for one.
pub(crate) fn extract_page_summary(content: &str) -> Option<String> {
    let mut in_open_questions = false;
    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(heading) = line.strip_prefix('#') {
            let heading = heading.trim_start_matches('#').trim().to_ascii_lowercase();
            in_open_questions = heading.starts_with("open question");
            continue;
        }
        if in_open_questions {
            continue;
        }
        let text = line
            .trim_start_matches("- ")
            .trim_start_matches("* ")
            .trim_start_matches("> ")
            .trim();
        let text = strip_summary_label(text);
        let sentence = match text.find(". ") {
            Some(end) => &text[..=end],
            None => text,
        };
        let cleaned = strip_citation_markers(sentence);
        return (!cleaned.is_empty()).then_some(cleaned);
    }
    None
}

/// Remove a leading "TLDR:" label, with any markdown emphasis around it, so a
/// labelled opener does not put the label in the page summary. The label must
/// be followed by a separator, so prose that merely starts with the letters
/// ("TLDRs are no substitute for the page") is left alone.
fn strip_summary_label(text: &str) -> &str {
    let rest = text.trim_start().trim_start_matches(['*', '_']);
    let lower = rest.to_ascii_lowercase();
    let Some(label_len) = ["tl;dr", "tldr"]
        .into_iter()
        .find(|label| lower.starts_with(label))
        .map(str::len)
    else {
        return text;
    };
    let after = rest[label_len..]
        .trim_start_matches(['*', '_'])
        .trim_start();
    let Some(after) = after.strip_prefix([':', '-', '\u{2013}', '\u{2014}']) else {
        return text;
    };
    let after = after.trim_start_matches(['*', '_']).trim_start();
    if after.is_empty() {
        text
    } else {
        after
    }
}

/// Remove `[N]` citation markers and the space that precedes them.
fn strip_citation_markers(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c == '[' {
            let rest = &text[i + 1..];
            if let Some(close) = rest.find(']') {
                let inner = &rest[..close];
                if !inner.is_empty() && inner.chars().all(|d| d.is_ascii_digit()) {
                    while out.ends_with(' ') {
                        out.pop();
                    }
                    for _ in 0..=close {
                        chars.next();
                    }
                    continue;
                }
            }
        }
        out.push(c);
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub async fn distill_one_cluster(
    db: &MemoryDB,
    llm: &Arc<dyn LlmProvider>,
    prompts: &PromptRegistry,
    cluster: &crate::db::DistillationCluster,
    knowledge_writer: Option<&crate::export::knowledge::KnowledgeWriter>,
) -> Result<Option<String>, WenlanError> {
    let tuning = crate::tuning::DistillationConfig::default();
    distill_one_cluster_with_tuning(db, llm, prompts, cluster, &tuning, knowledge_writer).await
}

async fn distill_one_cluster_with_tuning(
    db: &MemoryDB,
    llm: &Arc<dyn LlmProvider>,
    prompts: &PromptRegistry,
    cluster: &crate::db::DistillationCluster,
    tuning: &crate::tuning::DistillationConfig,
    knowledge_writer: Option<&crate::export::knowledge::KnowledgeWriter>,
) -> Result<Option<String>, WenlanError> {
    let topic = cluster
        .entity_name
        .as_deref()
        .or(cluster.space.as_deref())
        .unwrap_or("general");

    // Find the existing page that overlaps this cluster the most. Memories
    // can appear in multiple pages; the check below only prevents duplicate
    // pages, not duplicate sources.
    let overlap_match = db.find_best_overlapping_page(&cluster.source_ids).await?;
    if let Some(ref m) = overlap_match {
        let cluster_size = cluster.source_ids.len();
        let covered = m.intersection >= cluster_size || m.jaccard >= 0.8;
        if covered {
            log::info!(
                "[emergence] cluster '{}' already covered by page '{}' ({} of {} memories, Jaccard {:.2}), skipping",
                topic,
                m.page_title,
                m.intersection,
                cluster_size,
                m.jaccard
            );
            return Ok(None);
        }
        if m.intersection > 0 {
            // Partial overlap: attach the cluster's explicit evidence before
            // queueing refresh. PageWrite::Attach is idempotent for already
            // linked ids and atomically marks the page source_updated for each
            // new link, so the stale sweep reads the evidence that caused it.
            page_write(
                db,
                PageWrite::Attach {
                    page_id: &m.page_id,
                    source_memory_ids: &cluster.source_ids,
                    link_reason: "distill_partial_overlap",
                    agent: "distill",
                },
            )
            .await?;
            log::info!(
                "[emergence] cluster '{}' partially overlaps page '{}' ({} new memories) — attached evidence and queued refresh, skipping new-page synth",
                topic,
                m.page_title,
                cluster_size - m.intersection
            );
            return Ok(None);
        }
    }

    // Source-id overlap found no duplicate. Try the scoped embedding/entity
    // matcher so a source-less seed (zero source_ids) or a topic-equivalent
    // page is still recognized. On a match we ATTACH this cluster's sources to
    // that page's evidence (content untouched) and skip synth — preventing both
    // a duplicate page and the infinite re-cluster loop (un-attached memories
    // would re-cluster, match again, skip again, forever). link_page_source
    // dual-writes page_evidence (Task 3), so the memories are marked consumed
    // in BOTH the legacy and typed provenance tables.
    if let Some(centroid) = cluster.centroid_embedding.as_deref() {
        if let Some(matched) = db
            .find_matching_page_scoped(
                cluster.entity_id.as_deref(),
                centroid,
                tuning.page_match_threshold,
                cluster.space.as_deref(),
                false, // never rewrite/attach onto hand-edited prose
            )
            .await?
        {
            page_write(
                db,
                PageWrite::Attach {
                    page_id: &matched.id,
                    source_memory_ids: &cluster.source_ids,
                    link_reason: "distill_attach",
                    agent: "distill",
                },
            )
            .await?;
            let attached = cluster.source_ids.clone();
            // P3: the scoped-match attach consumes these memories into an existing
            // page, so demote them too. Stamp ONLY the ids that actually attached —
            // never-attached memories must keep ranking normally. (Resolves P2 TODO.)
            if !attached.is_empty() {
                if let Err(e) = db
                    .stamp_last_distilled_at(&attached, chrono::Utc::now().timestamp())
                    .await
                {
                    log::warn!("[distill] stamp last_distilled_at (scoped attach) failed: {e}");
                }
            }
            log::info!(
                "[distill] cluster '{}' attached {} sources to existing page '{}' (scoped match), skipping new-page synth",
                topic,
                cluster.source_ids.len(),
                matched.title
            );
            return Ok(None);
        }
    }

    // Clean input: strip recap headers, domain prefixes, and structured field noise
    let cleaned_contents: Vec<String> = cluster
        .contents
        .iter()
        .map(|c| {
            let mut s = c.trim().to_string();
            // Strip "Activity burst: ..." header lines
            if let Some(pos) = s.find("\n- ") {
                let prefix: String = s.chars().take(pos).collect();
                if prefix.contains("Activity burst") || prefix.contains("memories across") {
                    s = s.chars().skip(pos + 1).collect();
                }
            }
            // Strip "- [domain] " prefixes from each line
            s = s
                .lines()
                .map(|line| {
                    let trimmed = line.trim_start_matches("- ");
                    if trimmed.starts_with('[') {
                        if let Some(end) = trimmed.find("] ") {
                            trimmed[end + 2..].to_string()
                        } else {
                            line.to_string()
                        }
                    } else {
                        line.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            // Strip "claim: " prefix
            if let Some(rest) = s.strip_prefix("claim: ") {
                s = rest.to_string();
            }
            s
        })
        .collect();

    // Skip thin clusters — not enough substance for meaningful compilation
    let total_content_chars: usize = cleaned_contents.iter().map(|c| c.len()).sum();
    if total_content_chars < 200 {
        log::info!(
            "[compile] cluster too thin ({} chars), skipping topic='{}'",
            total_content_chars,
            topic
        );
        return Ok(None);
    }

    log::info!(
        "[distill] processing cluster: {} memories, ~{} tokens",
        cluster.source_ids.len(),
        cluster.estimated_tokens
    );

    // Build user prompt with memory IDs for source attribution.
    // Cap each memory at 800 chars so the LLM gets meaningful substance
    // without runaway context. The 800-char cap is honest: it matches the
    // amount the model can synthesize well at 2048 output tokens.
    const MEM_SNIPPET_CAP: usize = 800;
    // Resolve each source's typed kind (spec §5.1) so a folder-doc or webpage
    // source cited here carries external_file / external_url, not the
    // hardcoded 'memory' default.
    let kinds = db
        .resolve_source_kinds(&cluster.source_ids)
        .await
        .unwrap_or_default();
    let numbered: Vec<crate::citations::NumberedSource> = cluster
        .source_ids
        .iter()
        .zip(cleaned_contents.iter())
        .enumerate()
        .map(|(i, (id, content))| crate::citations::NumberedSource {
            index: (i + 1) as u32,
            source_kind: kinds
                .get(id)
                .cloned()
                .unwrap_or_else(|| "memory".to_string()),
            locator: id.clone(),
            text: content.chars().take(MEM_SNIPPET_CAP).collect(),
        })
        .collect();
    let memories_block = crate::citations::build_numbered_block(&numbered);
    let user_prompt =
        build_page_compile_user_prompt(db, topic, cluster.space.as_deref(), &memories_block).await;

    let response = llm
        .generate(LlmRequest {
            system_prompt: Some(prompts.distill_page.clone()),
            user_prompt,
            max_tokens: llm.recommended_max_output(),
            temperature: 0.1,
            label: Some("distill_body".into()),
            timeout_secs: None,
        })
        .await;

    match response {
        Ok(raw) if !raw.trim().is_empty() => {
            let cleaned = crate::llm_provider::strip_think_tags(&raw);
            let content = cleaned.trim().to_string();

            if content.is_empty() {
                log::warn!("[distill] empty output for topic='{}', skipping", topic);
                return Ok(None);
            }

            // Hallucination check: output must be semantically similar to input
            let texts = vec![content.clone(), cleaned_contents.join(" ")];
            if let Ok(embeddings) = db.generate_embeddings(&texts) {
                if embeddings.len() == 2 {
                    let sim = crate::db::cosine_similarity(&embeddings[0], &embeddings[1]);
                    if sim < 0.6 {
                        log::warn!(
                            "[compile] hallucination detected (sim={:.2}) for topic='{}', skipping",
                            sim,
                            topic
                        );
                        return Ok(None);
                    }
                    log::info!(
                        "[compile] quality check passed (sim={:.2}) for topic='{}'",
                        sim,
                        topic
                    );
                }
            }

            // Generate title. If LLM returns None and the only fallback is a generic
            // placeholder (e.g. "general"), skip this cluster entirely — a generic title
            // is worse than no page at all.
            let llm_title = crate::refinery::generate_short_title(llm, &content).await;
            let title = match llm_title {
                Some(t) => t,
                None if is_all_generic_tokens(topic)
                    || looks_like_markup_styled(topic)
                    || looks_like_path(topic)
                    || looks_like_code(topic)
                    || looks_like_uuid(topic)
                    || looks_like_short_hash(topic)
                    || looks_like_commit_message(topic) =>
                {
                    log::info!(
                        "[distill] no title and topic='{}' is garbage, skipping cluster",
                        topic
                    );
                    return Ok(None);
                }
                None => topic.to_string(),
            };

            // The page opens with a one-sentence TLDR in plain prose; that
            // sentence is the summary. Bullets only appear in enumerable lists
            // and under "Open Questions", so they are never the summary.
            let summary = extract_page_summary(&content);

            // Verify [N] markers the LLM emitted against the numbered sources:
            // out-of-range markers are stripped, each remaining occurrence gets
            // a verified/unverified status via union-of-cited-sources overlap.
            let (content, cites, stats) =
                crate::citations::process_citation_output(&content, &numbered);
            let citations_json = serde_json::to_string(&cites).unwrap_or_else(|_| "[]".into());

            let projection = knowledge_writer.map(|writer| writer.begin_projection_write());
            let write_result = page_write(
                db,
                PageWrite::Create {
                    page_id: None,
                    req: CreateConceptRequest {
                        title: title.clone(),
                        content: content.clone(),
                        summary: summary.clone(),
                        entity_id: cluster.entity_id.clone(),
                        space: (cluster.space.clone()).into(),
                        source_memory_ids: cluster.source_ids.clone(),
                        creation_kind: Some("distilled".to_string()),
                        workspace: cluster.space.clone(),
                    },
                    agent: "system",
                    knowledge_path: None,
                    page_min_cluster_size: tuning.page_min_cluster_size,
                    page_match_threshold: tuning.page_match_threshold,
                    citations_json: Some(citations_json),
                    basis_marks: Vec::new(),
                    basis_space_guard: None,
                    skip_protected_title_match: true,
                },
            )
            .await?;
            let page_id = write_result.id;
            if !write_result.wrote && write_result.attached_to.is_none() {
                return Ok(None);
            }
            // #708: a distilled page written about an entity cites it, and a
            // cited entity has earned its place in the Wiki whatever its memory
            // count -- the page is the evidence the count was standing in for.
            // Runs before the attach early-return because an attached page
            // cites the entity just as much as a new one. A failure here is
            // logged, never fatal: the page is already written, and the next
            // memory link or citation promotes.
            if let Some(entity_id) = cluster.entity_id.as_deref() {
                match db.establish_entity_by_citation(entity_id).await {
                    Ok(true) => log::info!(
                        "[distill] established entity {entity_id} by citation from page {page_id}"
                    ),
                    Ok(false) => {}
                    Err(e) => {
                        log::warn!("[distill] establish by citation failed for {entity_id}: {e}")
                    }
                }
            }
            if write_result.attached_to.is_some() {
                return Ok(None);
            }
            log::info!("[distill] page {page_id} citations: {}", stats.summary());

            // P3 consolidation-demotion: stamp the source memories so the ranking
            // demotion multiplier in search_memory_cross_rerank ranks them below
            // their page for the topic query (they stay in allowed_memory_ids +
            // deep recall). chrono::Utc::now().timestamp() matches the unix-seconds
            // contract of memories.last_distilled_at.
            if let Err(e) = db
                .stamp_last_distilled_at(&cluster.source_ids, chrono::Utc::now().timestamp())
                .await
            {
                log::warn!("[distill] stamp last_distilled_at failed: {e}");
            }

            log::info!(
                "[distill] distilled {} memories -> page '{}' ('{}')",
                cluster.source_ids.len(),
                title,
                content.chars().take(40).collect::<String>()
            );

            if let Some(ref projection) = projection {
                if let Ok(Some(c)) = db.get_page(&page_id).await {
                    match projection.write_page_gated(db, &c).await {
                        Ok(Some(p)) => log::info!("[distill] wrote page to {p}"),
                        Ok(None) => {}
                        Err(e) => log::warn!("[distill] knowledge write failed: {e}"),
                    }
                }
            }
            drop(projection);

            Ok(Some(page_id))
        }
        Ok(_) => {
            log::warn!("[distill] empty output for topic='{}'", topic);
            Ok(None)
        }
        Err(e) => {
            log::warn!("[distill] LLM error for topic='{}': {}", topic, e);
            Ok(None)
        }
    }
}

/// Outcome of a distillation pass — pages the daemon synthesized itself,
/// plus clusters it could not finish (no LLM available, or the cluster
/// exceeded the LLM's effective context budget). Callers with their own
/// LLM (e.g. the agent-driven `/distill` skill) can pick up the pending
/// clusters and finish them.
#[derive(Debug, Clone, serde::Serialize, Default)]
pub struct DistillResult {
    /// Page ids the daemon synthesized + persisted itself this pass.
    pub created: Vec<String>,
    /// Clusters the daemon clustered but did not synthesize. Each cluster
    /// carries the source memory ids, content snippets, and entity / domain
    /// metadata so the caller has everything it needs to write a page and
    /// POST back to `/api/pages`.
    pub pending: Vec<crate::db::DistillationCluster>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct FormationSweepReport {
    pub thresholds: Vec<FormationSweepPoint>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct FormationSweepPoint {
    pub formation_threshold: f64,
    pub capped: bool,
    pub global: FormationClusterStats,
    pub per_space: BTreeMap<String, FormationClusterStats>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct FormationClusterStats {
    pub cluster_count: usize,
    pub size_distribution: FormationSizeDistribution,
    pub overlapping_member_fraction: f64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct FormationSizeDistribution {
    pub min: usize,
    pub median: f64,
    pub max: usize,
    pub sizes: Vec<usize>,
}

/// Read-only pre-flight gate run before trusting the formation path and again
/// before the retro sweep. It reports the fixed threshold grid with global and
/// per-space cluster statistics; it does not tune or write anything.
pub async fn formation_sweep(
    db: &MemoryDB,
    tuning: &crate::tuning::DistillationConfig,
) -> Result<FormationSweepReport, WenlanError> {
    let mut thresholds = Vec::with_capacity(FORMATION_SWEEP_THRESHOLDS.len());
    for formation_threshold in FORMATION_SWEEP_THRESHOLDS {
        let clusters = db
            .find_distillation_clusters_scoped(
                formation_threshold,
                tuning.page_min_cluster_size,
                tuning.max_clusters_per_steep,
                16_000,
                tuning.max_unlinked_cluster_size,
                tuning.max_grouped_cluster_size,
                None,
                None,
            )
            .await?;
        thresholds.push(FormationSweepPoint {
            formation_threshold,
            capped: clusters.len() == tuning.max_clusters_per_steep,
            global: cluster_stats(&clusters),
            per_space: per_space_cluster_stats(&clusters),
        });
    }
    Ok(FormationSweepReport { thresholds })
}

fn per_space_cluster_stats(
    clusters: &[crate::db::DistillationCluster],
) -> BTreeMap<String, FormationClusterStats> {
    let mut grouped: BTreeMap<String, Vec<&crate::db::DistillationCluster>> = BTreeMap::new();
    for cluster in clusters {
        grouped
            .entry(
                cluster
                    .space
                    .as_deref()
                    .filter(|space| !space.is_empty())
                    .unwrap_or(NO_SPACE_KEY)
                    .to_string(),
            )
            .or_default()
            .push(cluster);
    }
    grouped
        .into_iter()
        .map(|(space, clusters)| (space, cluster_stats(clusters)))
        .collect()
}

fn cluster_stats<'a, I>(clusters: I) -> FormationClusterStats
where
    I: IntoIterator<Item = &'a crate::db::DistillationCluster>,
{
    let mut sizes = Vec::new();
    let mut member_counts: HashMap<&str, usize> = HashMap::new();

    for cluster in clusters {
        sizes.push(cluster.source_ids.len());
        for source_id in &cluster.source_ids {
            *member_counts.entry(source_id.as_str()).or_insert(0) += 1;
        }
    }

    sizes.sort_unstable();
    let size_distribution = size_distribution(sizes);
    let distinct_members = member_counts.len();
    let overlapping_members = member_counts.values().filter(|&&count| count >= 2).count();
    let overlapping_member_fraction = if distinct_members == 0 {
        0.0
    } else {
        overlapping_members as f64 / distinct_members as f64
    };

    FormationClusterStats {
        cluster_count: size_distribution.sizes.len(),
        size_distribution,
        overlapping_member_fraction,
    }
}

fn size_distribution(sizes: Vec<usize>) -> FormationSizeDistribution {
    let min = sizes.first().copied().unwrap_or(0);
    let max = sizes.last().copied().unwrap_or(0);
    let median = match sizes.len() {
        0 => 0.0,
        len if len % 2 == 1 => sizes[len / 2] as f64,
        len => (sizes[len / 2 - 1] + sizes[len / 2]) as f64 / 2.0,
    };
    FormationSizeDistribution {
        min,
        median,
        max,
        sizes,
    }
}

/// Distill memory clusters into structured concepts.
/// Memories can appear in multiple concepts. Jaccard overlap prevents duplicate concepts.
///
/// Returns the count of pages the daemon synthesized itself; refer to
/// `distill_pages_scoped` for the full `DistillResult` with `pending`.
pub async fn distill_pages(
    db: &MemoryDB,
    llm: Option<&Arc<dyn LlmProvider>>,
    prompts: &PromptRegistry,
    tuning: &crate::tuning::DistillationConfig,
    knowledge_path: Option<&std::path::Path>,
) -> Result<usize, WenlanError> {
    let r = distill_pages_scoped(db, llm, prompts, tuning, knowledge_path, None).await?;
    Ok(r.created.len())
}

/// Same as `distill_pages` but restricts clustering to a single entity, a
/// single domain, or (when `target` is `DistillTarget::Page`) re-distills one
/// existing page directly. `None` matches `distill_pages` exactly.
///
/// Returns a `DistillResult`: pages the daemon synthesized itself plus
/// clusters it couldn't finish (no LLM, or cluster too big for the LLM's
/// context). The HTTP `/api/distill` route hands the `pending` list back
/// to the caller so the agent-driven skill can synthesize the rest.
pub async fn distill_pages_scoped(
    db: &MemoryDB,
    llm: Option<&Arc<dyn LlmProvider>>,
    prompts: &PromptRegistry,
    tuning: &crate::tuning::DistillationConfig,
    knowledge_path: Option<&std::path::Path>,
    target: Option<DistillTarget>,
) -> Result<DistillResult, WenlanError> {
    distill_pages_scoped_gated(db, llm, prompts, tuning, knowledge_path, target, true).await
}

/// Same as `distill_pages_scoped`, with an explicit switch for the LLM
/// coherence gate (merge/rename cluster review, spec §3.1). The compile
/// router (`refinery::run_periodic_steep_with_api`) skips the gate for an
/// on-device-only compile — the page's keep-card carries that judgment
/// instead of an LLM review. Every other caller goes through the
/// `distill_pages_scoped` wrapper above, which always runs the gate
/// (unchanged behavior).
pub(crate) async fn distill_pages_scoped_gated(
    db: &MemoryDB,
    llm: Option<&Arc<dyn LlmProvider>>,
    prompts: &PromptRegistry,
    tuning: &crate::tuning::DistillationConfig,
    knowledge_path: Option<&std::path::Path>,
    target: Option<DistillTarget>,
    run_coherence_gate: bool,
) -> Result<DistillResult, WenlanError> {
    if let Some(DistillTarget::Page(ref page_id)) = target {
        let outcome = deep_distill_single(db, llm, prompts, page_id, knowledge_path).await?;
        return Ok(DistillResult {
            created: if outcome.wrote {
                vec![page_id.clone()]
            } else {
                vec![]
            },
            pending: vec![],
        });
    }
    let (entity_id_filter, domain_filter): (Option<String>, Option<String>) = match target {
        Some(DistillTarget::Entity { id, .. }) => (Some(id), None),
        Some(DistillTarget::Domain(d)) => (None, Some(d)),
        Some(DistillTarget::Page(_)) | None => (None, None),
    };
    // No LLM available — discover clusters and hand them back as pending
    // so the caller (typically the /distill skill in Basic Memory mode)
    // can finish them with its own LLM.
    let llm = match llm {
        Some(l) if l.is_available() => l,
        _ => {
            // Use a generous budget so candidate discovery isn't gated by
            // a tiny on-device window we don't have anyway.
            let raw_clusters = db
                .find_distillation_clusters_scoped(
                    tuning.formation_threshold,
                    tuning.page_min_cluster_size,
                    tuning.max_clusters_per_steep,
                    16_000,
                    tuning.max_unlinked_cluster_size,
                    tuning.max_grouped_cluster_size,
                    entity_id_filter.as_deref(),
                    domain_filter.as_deref(),
                )
                .await?;
            let mut raw_clusters =
                cap_document_majority_clusters(db, raw_clusters, tuning.page_min_cluster_size)
                    .await?;
            // Cap each memory's content snippet so the caller-facing payload
            // doesn't balloon past practical HTTP/MCP response sizes. The
            // synthesis path uses the same cap when building prompts; doing
            // it at the boundary keeps both code paths consistent and bounds
            // the worst-case pending payload to ~MEM_SNIPPET_CAP * total
            // memories.
            const MEM_SNIPPET_CAP: usize = 800;
            for c in raw_clusters.iter_mut() {
                for content in c.contents.iter_mut() {
                    if content.chars().count() > MEM_SNIPPET_CAP {
                        let mut truncated: String = content.chars().take(MEM_SNIPPET_CAP).collect();
                        truncated.push('…');
                        *content = truncated;
                    }
                }
            }
            return Ok(DistillResult {
                created: vec![],
                pending: raw_clusters,
            });
        }
    };

    // Each model carries its own effective synthesis limit — the max tokens it
    // can meaningfully synthesize (not just read). Research-calibrated per model
    // in on_device_models.rs and llm_provider.rs. Falls back to tuning config
    // if the provider returns the default (for backward compat).
    let token_limit = llm.synthesis_token_limit();
    let raw_clusters = db
        .find_distillation_clusters_scoped(
            tuning.formation_threshold,
            tuning.page_min_cluster_size,
            tuning.max_clusters_per_steep,
            token_limit,
            tuning.max_unlinked_cluster_size,
            tuning.max_grouped_cluster_size,
            entity_id_filter.as_deref(),
            domain_filter.as_deref(),
        )
        .await?;
    let raw_clusters =
        cap_document_majority_clusters(db, raw_clusters, tuning.page_min_cluster_size).await?;

    // LLM cluster refinement: let LLM merge/rename clusters per entity.
    // Coherence gate (spec §3.1) — skipped for an on-device-only compile.
    let clusters = if run_coherence_gate {
        refine_clusters_with_llm(llm, prompts, raw_clusters, token_limit).await
    } else {
        raw_clusters
    };
    let clusters =
        cap_document_majority_clusters(db, clusters, tuning.page_min_cluster_size).await?;

    let cluster_concurrency: usize = std::env::var("DISTILL_CLUSTER_CONCURRENCY")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1)
        .clamp(1, 4);

    let mut created: Vec<String> = Vec::new();

    // Create the writer once, outside the loop
    let knowledge_writer = knowledge_path
        .map(|kp| crate::export::knowledge::KnowledgeWriter::new(kp.to_path_buf(), db));

    let kw = knowledge_writer.as_ref();
    for chunk in clusters.chunks(cluster_concurrency) {
        let futs: Vec<_> = chunk
            .iter()
            .map(|cluster| distill_one_cluster_with_tuning(db, llm, prompts, cluster, tuning, kw))
            .collect();
        let results = futures::future::join_all(futs).await;
        for r in results {
            if let Some(page_id) = r? {
                created.push(page_id);
            }
        }
    }

    Ok(DistillResult {
        created,
        pending: vec![],
    })
}

/// Why a page is being refreshed. Selects the `edited_by` provenance tag the
/// changelog + ownership gate see. All refresh writes are machine writes (gated
/// to a revision card on human-owned pages) and skip the embedding
/// hallucination guard — `refresh_page` runs its own fail-closed citation
/// verification gate instead.
///
/// Post-merge rebuilds flow through `SourceChanged`: a merge marks the page
/// stale, and the stale-page sweep refreshes it on the same path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshReason {
    /// A source memory changed (or a merge marked the page stale). Written as
    /// `re_distill`.
    SourceChanged,
    /// Explicit human/agent request (`POST /api/distill/{id}`, force rebuild).
    /// Written as `distill`.
    Explicit,
    /// User-forced Re-distill ([`force_redistill_single`]). Written as
    /// `distill`, but the page's own `user_edited` flag does not gate the
    /// write: the new body lands in place and releases that flag and the
    /// page's staleness in the same UPDATE. Nothing is released when no body
    /// lands (provider failure, cancellation, discarded or unavailable
    /// output). Authored pages still get a revision card.
    UserForced,
}

impl RefreshReason {
    /// Provenance tag written to the changelog and read by the ownership gate.
    fn edited_by(self) -> &'static str {
        match self {
            RefreshReason::SourceChanged => "re_distill",
            RefreshReason::Explicit | RefreshReason::UserForced => "distill",
        }
    }
}

/// Outcome of a [`refresh_page`] call.
#[derive(Debug, Default, Clone)]
pub struct RefreshOutcome {
    /// The page prose was rewritten in place.
    pub wrote: bool,
    /// The page is human-owned; a revision card was staged instead of a write.
    pub gated: bool,
    /// Verified prose and sources were identical; only the compile watermark advanced.
    pub acknowledged: bool,
    /// Id of the staged revision card, present iff `gated`.
    pub revision_card_id: Option<String>,
    /// Why the synthesis was discarded without a write (for example, failed
    /// citation verification or source context exceeding the model input
    /// budget). Also persisted on
    /// `pages.refresh_blocked_reason` so page reads can surface it; `None` on
    /// every other outcome, including the other no-op early returns.
    pub discard_reason: Option<String>,
}

pub(crate) const AUTOMATIC_PAGE_REFRESH_SOURCE_CAP: usize = 64;

/// Reject an oversized automatic Page refresh before source contents are
/// materialized. The normalized join table is authoritative when present;
/// legacy Pages with no join rows fall back to their JSON source list.
pub(crate) async fn automatic_refresh_exceeds_source_cap(
    db: &MemoryDB,
    page: &crate::pages::Page,
) -> Result<bool, WenlanError> {
    let joined = db
        .count_page_sources_up_to(&page.id, AUTOMATIC_PAGE_REFRESH_SOURCE_CAP)
        .await?;
    let source_count = if joined == 0 {
        page.source_memory_ids.len()
    } else {
        joined
    };
    Ok(source_count > AUTOMATIC_PAGE_REFRESH_SOURCE_CAP)
}

/// Rebuild a page's prose from its CURRENT sources via the DISTILL_PAGE prompt,
/// verify per-claim `[N]` citations against the numbered sources, and write the
/// result atomically through the canonical PageWrite path.
///
/// This is the ONE re-distill / repair op. `deep_distill_single` and
/// `re_distill_stale_pages` both delegate here so the
/// synthesis, citation verification, atomic content+citations+changelog write,
/// and fail-closed citation gate live in exactly one place.
///
/// - **Sources.** Prefers the `page_sources` join table (kept accurate across
///   topic-key upserts), falling back to the page's `source_memory_ids` JSON
///   column for legacy pages with no join rows.
/// - **Ownership.** A machine write to a human-owned page (`user_edited` or
///   `creation_kind = "authored"`) is gated by `update_page` into a pending
///   revision card — the prose is never overwritten in place. The card id is
///   returned on [`RefreshOutcome::revision_card_id`].
/// - **Fail-closed.** If per-claim citation verification verifies zero claims
///   or leaves a majority unverified, the synthesis is discarded and the page
///   is left unchanged — nothing half-written.
///
/// Never holds a DB lock across the LLM call.
pub async fn refresh_page(
    db: &MemoryDB,
    llm: &Arc<dyn LlmProvider>,
    prompts: &PromptRegistry,
    page_id: &str,
    reason: RefreshReason,
    knowledge_path: Option<&std::path::Path>,
) -> Result<RefreshOutcome, WenlanError> {
    refresh_page_with_prompt(
        db,
        llm,
        &prompts.distill_page,
        page_id,
        reason,
        knowledge_path,
    )
    .await
}

/// A verified replacement body awaiting its canonical atomic write.
/// Keeping generation separate lets Overview propose new evidence without
/// disturbing the saved page when the candidate fails verification.
pub(crate) struct CandidateSynthesis {
    pub content: String,
    pub citations_json: String,
    pub stats_summary: String,
    pub summary: Option<String>,
}

/// Outcome of [`synthesize_candidate_body`].
pub(crate) enum CandidateOutcome {
    /// Per-claim verification passed; ready for one canonical write.
    Verified(CandidateSynthesis),
    /// Per-claim verification failed its gate; nothing was written.
    Discarded { discard_reason: String },
    /// No synthesis attempted: empty source list, all sources orphaned, or
    /// empty LLM output.
    Unavailable,
}

/// Limit for compact secondary source previews. Candidate synthesis and its
/// citation verifier intentionally use full numbered source text.
pub(crate) const CANDIDATE_MEM_SNIPPET_CAP: usize = 800;

/// Cap on one basis note's text embedded in the "notes this note relies on"
/// block of an update-from-sources prompt.
const BASIS_CONTEXT_CHAR_CAP: usize = 2000;

/// `pages.refresh_blocked_reason` value for a note the automatic refresh skips
/// because it relies on other notes. Only "Update from sources" rebuilds it.
pub const REFRESH_BLOCKED_HAS_BASIS: &str = "has_basis";

/// The "notes this note relies on" block of an update-from-sources prompt.
/// Empty when the note has no readable basis, so a note without one gets the
/// same prompt as before.
fn build_basis_context_block(bases: &[crate::db::BasisContext]) -> String {
    if bases.is_empty() {
        return String::new();
    }
    let mut block = String::from(
        "## Notes this note relies on\n\
         These are other notes this note is built on. Use them as background context. \
         When you draw a claim from one of them, write it as a link to that note, \
         like [[Title]]. Never give these notes a numbered citation marker such as [1]; \
         numbered markers are only for the numbered sources above.",
    );
    for basis in bases {
        let text: String = basis.content.chars().take(BASIS_CONTEXT_CHAR_CAP).collect();
        block.push_str(&format!("\n\n### [[{}]]\n{}", basis.title, text));
    }
    block
}

/// Input tokens one page-update request may use. On-device models also keep
/// room for the reply inside their fixed context window.
fn refresh_input_budget(llm: &dyn LlmProvider) -> usize {
    if llm.backend() == LlmBackend::OnDevice {
        llm.synthesis_token_limit().min(
            (llm.context_size() as usize).saturating_sub(llm.recommended_max_output() as usize),
        )
    } else {
        llm.synthesis_token_limit()
    }
}

/// One token per three UTF-8 bytes over the complete system and user prompts.
fn estimate_prompt_tokens(system_prompt: &str, user_prompt: &str) -> usize {
    system_prompt
        .len()
        .saturating_add(user_prompt.len())
        .div_ceil(3)
}

fn refresh_memories_block(
    numbered: &[crate::citations::NumberedSource],
    basis_block: &str,
) -> String {
    [
        crate::citations::build_numbered_block_full(numbered),
        basis_block.to_string(),
    ]
    .into_iter()
    .filter(|block| !block.is_empty())
    .collect::<Vec<_>>()
    .join("\n\n")
}

/// Most model calls one page update may spend reading long sources in parts.
/// A local model takes tens of seconds per call, so past this the update is
/// reported as blocked rather than tying up the model for many minutes.
pub(crate) const REFRESH_SOURCE_PART_CALL_CAP: usize = 24;

/// Reply cap for one part's notes.
const REFRESH_SOURCE_NOTES_MAX_TOKENS: u32 = 512;

const REFRESH_SOURCE_NOTHING_RELEVANT: &str = "(nothing relevant)";

/// Replace the longest sources in `sources` with in-order notes of their facts
/// until the writer's prompt fits `input_budget`. `fixed_bytes` is every prompt
/// byte that is not source text. Returns `Ok(Err(reason))` when the sources
/// cannot be made to fit; model errors other than an oversized input propagate.
async fn read_long_sources_in_parts(
    llm: &dyn LlmProvider,
    topic: &str,
    sources: &mut [crate::citations::NumberedSource],
    fixed_bytes: usize,
    input_budget: usize,
) -> Result<Result<(), String>, WenlanError> {
    let notes_prompt = crate::prompts::defaults::REFRESH_SOURCE_NOTES;
    // A part, its framing and the notes prompt must fit the same budget; keep
    // a quarter spare because the byte estimate is not the model's tokenizer.
    let framing_bytes = notes_prompt.len() + topic.len() + 96;
    let part_bytes = input_budget.saturating_mul(3).saturating_sub(framing_bytes) / 4 * 3;
    if part_bytes < 1_024 {
        return Ok(Err(format!(
            "source context exceeds model input budget (limit {input_budget} tokens is too small to read long sources in parts)"
        )));
    }
    let fits = |sources: &[crate::citations::NumberedSource]| {
        let source_bytes: usize = sources.iter().map(|source| source.text.len()).sum();
        (fixed_bytes + source_bytes).div_ceil(3) <= input_budget
    };
    let mut longest_first: Vec<usize> = (0..sources.len()).collect();
    longest_first.sort_by_key(|&i| std::cmp::Reverse(sources[i].text.len()));
    let mut calls = 0usize;
    for i in longest_first {
        if fits(sources) {
            break;
        }
        let parts = split_source_parts(&sources[i].text, part_bytes);
        if calls + parts.len() > REFRESH_SOURCE_PART_CALL_CAP {
            return Ok(Err(format!(
                "source context exceeds model input budget and is too long to read in parts (more than {REFRESH_SOURCE_PART_CALL_CAP} parts; limit {input_budget} tokens)"
            )));
        }
        let mut notes = Vec::with_capacity(parts.len());
        for (k, part) in parts.iter().enumerate() {
            calls += 1;
            let user_prompt = format!(
                "Topic: {topic}\n\nPart {} of {} of source [{}]:\n{part}",
                k + 1,
                parts.len(),
                sources[i].index
            );
            let raw = match llm
                .generate(LlmRequest {
                    system_prompt: Some(notes_prompt.to_string()),
                    user_prompt,
                    max_tokens: llm
                        .recommended_max_output()
                        .min(REFRESH_SOURCE_NOTES_MAX_TOKENS),
                    temperature: 0.1,
                    label: Some("refresh_source_notes".into()),
                    timeout_secs: None,
                })
                .await
            {
                Ok(raw) => raw,
                Err(error @ LlmError::InputContextTooLarge { .. }) => {
                    return Ok(Err(format!(
                        "source context exceeds model input budget ({error})"
                    )));
                }
                Err(error) => {
                    return Err(WenlanError::Llm(format!(
                        "refresh_source_notes LLM: {error}"
                    )))
                }
            };
            let part_notes = crate::llm_provider::strip_think_tags(&raw)
                .trim()
                .to_string();
            if !part_notes.is_empty() && part_notes != REFRESH_SOURCE_NOTHING_RELEVANT {
                notes.push(part_notes);
            }
            // A small model's notes keep an amendment's new value but can drop
            // the words that make it one ("cancelled", the date), and then the
            // writer keeps the decision it replaced. Carry those lines as written.
            let amendments = amendment_lines(part);
            if !amendments.is_empty() {
                notes.push(amendments.join("\n"));
            }
        }
        log::info!(
            "[refresh] read source [{}] in {} parts for '{topic}'",
            sources[i].index,
            parts.len()
        );
        sources[i].text = if notes.is_empty() {
            "(This long source has nothing on this topic.)".to_string()
        } else {
            format!(
                "(Facts noted from this long source, read in {} parts, in order.)\n{}",
                parts.len(),
                notes.join("\n")
            )
        };
    }
    if fits(sources) {
        Ok(Ok(()))
    } else {
        Ok(Err(format!(
            "source context exceeds model input budget even after reading long sources in parts (limit {input_budget} tokens)"
        )))
    }
}

/// Words that mark a statement as changing an earlier one.
const AMENDMENT_CUES: &[&str] = &[
    "amend",
    "cancel",
    "supersede",
    "rescind",
    "revoke",
    "reverse",
    "withdrawn",
    "no longer",
    "instead of",
    "correction",
    "corrected",
    "overrid",
    "changed to",
    "修訂",
    "修正",
    "更正",
    "取消",
    "撤回",
    "撤銷",
    "撤销",
    "改為",
    "改为",
    "改成",
    "不再",
    "作廢",
    "作废",
    "取代",
];

/// Most bytes of verbatim amendment text carried from one part.
const AMENDMENT_LINES_BYTE_CAP: usize = 2_000;

/// The sentences of `part` that amend, cancel or correct something, verbatim
/// and in order. Lines are split into sentences so one long paragraph does not
/// pull in everything around the amendment.
fn amendment_lines(part: &str) -> Vec<&str> {
    let mut kept = Vec::new();
    let mut bytes = 0usize;
    for line in part.lines() {
        for sentence in sentences(line) {
            let sentence = sentence.trim();
            if sentence.is_empty() {
                continue;
            }
            let lower = sentence.to_lowercase();
            if !AMENDMENT_CUES.iter().any(|cue| lower.contains(cue)) {
                continue;
            }
            if bytes + sentence.len() > AMENDMENT_LINES_BYTE_CAP {
                return kept;
            }
            bytes += sentence.len();
            kept.push(sentence);
        }
    }
    kept
}

/// Split one line into sentences: after `.`, `!` or `?` followed by
/// whitespace (so "$48.5" and "v0.20" stay whole), and after `。！？`.
fn sentences(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut chars = line.char_indices().peekable();
    while let Some((i, ch)) = chars.next() {
        let end = i + ch.len_utf8();
        let ends = match ch {
            '。' | '！' | '？' => true,
            '.' | '!' | '?' => chars.peek().is_none_or(|(_, next)| next.is_whitespace()),
            _ => false,
        };
        if ends {
            out.push(&line[start..end]);
            start = end;
        }
    }
    if start < line.len() {
        out.push(&line[start..]);
    }
    out
}

/// Split `text` into parts of at most `max_bytes`, at line ends where possible
/// and always on a character boundary.
fn split_source_parts(text: &str, max_bytes: usize) -> Vec<&str> {
    let max_bytes = max_bytes.max(4);
    let mut parts = Vec::new();
    let mut rest = text;
    while rest.len() > max_bytes {
        let mut cut = max_bytes;
        while !rest.is_char_boundary(cut) {
            cut -= 1;
        }
        if let Some(line_end) = rest[..cut].rfind('\n') {
            if line_end + 1 >= cut / 2 {
                cut = line_end + 1;
            }
        }
        parts.push(&rest[..cut]);
        rest = &rest[cut..];
    }
    if !rest.trim().is_empty() || parts.is_empty() {
        parts.push(rest);
    }
    parts
}

/// Synthesize one candidate body from `source_ids` under `system_prompt` and
/// verify its per-claim `[N]` citations — the shared synthesis/verification
/// core of [`refresh_page_with_prompt`], factored out so first-time creation
/// can run the exact same gate before any row exists. Never writes.
pub(crate) async fn synthesize_candidate_body(
    db: &MemoryDB,
    llm: &Arc<dyn LlmProvider>,
    system_prompt: &str,
    title: &str,
    workspace: Option<&str>,
    source_ids: &[String],
    bases: &[crate::db::BasisContext],
) -> Result<CandidateOutcome, WenlanError> {
    // A note that relies on other notes can be rebuilt from those notes alone;
    // everyone else still needs at least one memory source.
    if source_ids.is_empty() && bases.is_empty() {
        return Ok(CandidateOutcome::Unavailable);
    }
    let memories = if source_ids.is_empty() {
        Vec::new()
    } else {
        db.get_refresh_source_contents_by_ids(source_ids).await?
    };
    if memories.is_empty() && bases.is_empty() {
        log::warn!("[refresh] page '{title}' sources are all orphaned, skipping");
        return Ok(CandidateOutcome::Unavailable);
    }

    let input_budget = refresh_input_budget(llm.as_ref());

    // Resolve each source's typed kind (spec §5.1) so a folder-doc or webpage
    // source cited here carries external_file / external_url, not the
    // hardcoded 'memory' default.
    let ids: Vec<String> = memories.iter().map(|(id, _)| id.clone()).collect();
    let kinds = db.resolve_source_kinds(&ids).await.unwrap_or_default();
    let numbered: Vec<crate::citations::NumberedSource> = memories
        .iter()
        .enumerate()
        .map(|(i, (id, content))| crate::citations::NumberedSource {
            index: (i + 1) as u32,
            source_kind: kinds
                .get(id)
                .cloned()
                .unwrap_or_else(|| "memory".to_string()),
            locator: id.clone(),
            // Keep full source text here: citation verification reads these
            // entries, so truncating them would hide later amendments.
            text: content.clone(),
        })
        .collect();
    // The notes this note relies on are context, never evidence: they are not
    // numbered, never enter the citation store, and the gate below does not
    // count them.
    let basis_block = build_basis_context_block(bases);
    let memories_block = refresh_memories_block(&numbered, &basis_block);
    // Refresh only from this page's numbered sources and explicitly linked
    // basis notes. Preserve exact wikilink labels when those contexts mention
    // them, but do not expose unrelated active-page titles.
    let capped_topic: String = title.chars().take(PAGE_TOPIC_CHAR_CAP).collect();
    let titles_hint = build_refresh_titles_hint(db, title, workspace, &memories_block).await;
    let mut user_prompt = format!("{titles_hint}Topic: {capped_topic}\n\n{memories_block}");

    // This is deliberately a byte-based estimate, not a tokenizer: count the
    // complete system and user prompts, then budget one token per three UTF-8
    // bytes. The on-device provider adds an exact tokenizer check as a final
    // guard. When the complete sources do not fit, the longest ones are read
    // in parts and the writer gets their facts in order; the citation check
    // still reads every source in full. Never silently drop a source's tail.
    if estimate_prompt_tokens(system_prompt, &user_prompt) > input_budget {
        let source_bytes: usize = numbered.iter().map(|source| source.text.len()).sum();
        let fixed_bytes = system_prompt.len() + user_prompt.len() - source_bytes;
        let mut prompt_sources = numbered.clone();
        if let Err(discard_reason) = read_long_sources_in_parts(
            llm.as_ref(),
            &capped_topic,
            &mut prompt_sources,
            fixed_bytes,
            input_budget,
        )
        .await?
        {
            log::warn!("[refresh] page '{title}': {discard_reason}");
            return Ok(CandidateOutcome::Discarded { discard_reason });
        }
        let memories_block = refresh_memories_block(&prompt_sources, &basis_block);
        user_prompt = format!("{titles_hint}Topic: {capped_topic}\n\n{memories_block}");
        let estimated_input_tokens = estimate_prompt_tokens(system_prompt, &user_prompt);
        if estimated_input_tokens > input_budget {
            return Ok(CandidateOutcome::Discarded {
                discard_reason: format!(
                    "source context exceeds model input budget even after reading long sources in parts (estimated {estimated_input_tokens} tokens from UTF-8 bytes; limit {input_budget})"
                ),
            });
        }
    }

    let raw = match llm
        .generate(LlmRequest {
            system_prompt: Some(system_prompt.to_string()),
            user_prompt,
            max_tokens: llm.recommended_max_output(),
            temperature: 0.1,
            label: Some("refresh_page".into()),
            timeout_secs: None,
        })
        .await
    {
        Ok(raw) => raw,
        Err(error @ LlmError::InputContextTooLarge { .. }) => {
            return Ok(CandidateOutcome::Discarded {
                discard_reason: format!("source context exceeds model input budget ({error})"),
            });
        }
        Err(error) => return Err(WenlanError::Llm(format!("refresh_page LLM: {error}"))),
    };

    let body = crate::llm_provider::strip_think_tags(&raw)
        .trim()
        .to_string();
    if body.is_empty() {
        log::warn!("[refresh] empty LLM output for '{title}', skipping");
        return Ok(CandidateOutcome::Unavailable);
    }

    // Verify [N] markers against the numbered sources; out-of-range markers are
    // stripped from the body before it is saved.
    let (content, cites, stats) = crate::citations::process_citation_output(&body, &numbered);

    // Fail-closed (spec §7): discard when per-claim citation verification
    // verifies zero claims or leaves a majority unverified. The previous
    // whole-body sentence-majority scorer false-rejected thin-source pages:
    // overview summaries and connective prose can contain uncited sentences
    // by design.
    // A note built only from other notes has no memory citations to check, so
    // the body-length and no-change rules downstream are its only gates.
    let basis_only = numbered.is_empty();
    if !basis_only && (stats.verified == 0 || stats.unverified > stats.verified) {
        log::warn!(
            "[refresh] page '{title}' synthesis failed the citation verification gate ({}); discarding",
            stats.summary()
        );
        return Ok(CandidateOutcome::Discarded {
            discard_reason: format!("citation verification failed ({})", stats.summary()),
        });
    }

    log::info!("[refresh] page '{title}' citations: {}", stats.summary());
    let citations_json = serde_json::to_string(&cites).unwrap_or_else(|_| "[]".to_string());
    let summary = extract_page_summary(&content);
    Ok(CandidateOutcome::Verified(CandidateSynthesis {
        content,
        citations_json,
        stats_summary: stats.summary(),
        summary,
    }))
}

/// Same op as [`refresh_page`], parameterized on the system prompt. Lets a
/// caller synthesize with a different prompt than the generic `distill_page`
/// template while still going through the ONE re-distill path (citation
/// verification, fail-closed citation gate, atomic `update_page`,
/// staleness CAS) rather than a bespoke write.
///
/// Used by the reserved Overview page (spec §5.3), which needs its own
/// summary-style prompt (`overview_summary`) instead of the deep-dive
/// `distill_page` prompt every other page refresh uses.
pub(crate) async fn refresh_page_with_prompt(
    db: &MemoryDB,
    llm: &Arc<dyn LlmProvider>,
    system_prompt: &str,
    page_id: &str,
    reason: RefreshReason,
    knowledge_path: Option<&std::path::Path>,
) -> Result<RefreshOutcome, WenlanError> {
    refresh_page_with_candidate_sources(
        db,
        llm,
        system_prompt,
        page_id,
        reason,
        knowledge_path,
        None,
    )
    .await
}

/// Propose evidence without invalidating the saved page before verification.
/// The successful write commits it with the body under the original fences.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn refresh_page_with_candidate_sources(
    db: &MemoryDB,
    llm: &Arc<dyn LlmProvider>,
    system_prompt: &str,
    page_id: &str,
    reason: RefreshReason,
    knowledge_path: Option<&std::path::Path>,
    candidate_sources: Option<&[String]>,
) -> Result<RefreshOutcome, WenlanError> {
    // The ambient sweep (`SourceChanged`) is not an HTTP route, so there is no
    // caller grant to pass -- it asks the automatic-reader permit directly,
    // same reasoning as `write_page_gated`. An `Explicit` request already
    // carries its own wire-level grant and is unaffected.
    if reason == RefreshReason::SourceChanged && page_write_permit(db, page_id).await?.is_none() {
        log::info!(
            "[refresh] not re-distilling page {page_id} — an automatic reader may not see it"
        );
        return Ok(RefreshOutcome::default());
    }

    // Read the fence counter FIRST, then the fields it protects (same
    // reasoning as the citation backfill's read order, citations.rs). A
    // concurrent source change (`link_page_source` / `replace_page_sources`)
    // bumps `source_revision` without necessarily bumping `version`, so
    // threading `source_revision` into the terminal CAS below closes the
    // window where a stale source list computed from a page/sources read
    // taken before that change would otherwise still match on `version`
    // alone and overwrite (or re-attach into) the freshly changed page. A
    // change landing AFTER this read bumps the counter past what we
    // captured, so the CAS below catches it; a change landing BEFORE this
    // read is already reflected in the `get_page`/`get_page_sources` reads
    // that follow. Reading `get_page` first would leave a window where a
    // detach landing between the two reads shows up in `source_revision` but
    // not in the page/sources snapshot -- the CAS would then pass on a write
    // that still carries the stale (pre-detach) source list.
    //
    // The revision is read together with the row's incarnation (one
    // statement). Deleting the page and recreating it under the same id
    // restarts `source_revision` (and `version`), so a revision-only CAS
    // could match the replacement; the incarnation cannot. The user-forced
    // write and every discard marker carry both.
    let page_fence = db
        .try_get_page_fence(page_id)
        .await?
        .ok_or_else(|| WenlanError::Validation(format!("page '{page_id}' does not exist")))?;
    let source_revision = page_fence.source_revision;

    let page = db
        .get_page(page_id)
        .await?
        .ok_or_else(|| WenlanError::VectorDb(format!("page {page_id} not found")))?;

    // A forced rebuild of a draft is refused before any generation runs, with
    // the same error the draft guard gives every other page mutator.
    if reason == RefreshReason::UserForced && page.status == "draft" {
        return Err(WenlanError::Validation(format!(
            "Page {page_id} is not active"
        )));
    }

    // A note that relies on other notes is rebuilt only by an explicit "Update
    // from sources": the automatic refresh would write over text a person
    // confirmed against those notes. It is skipped, and the skip is recorded on
    // the page so it reads as "update blocked" and not as an endless update.
    // A run that lands or previews a new body clears the marker.
    let has_basis = db.page_has_basis(page_id).await?;
    if has_basis && reason == RefreshReason::SourceChanged {
        log::info!(
            "[refresh] page {page_id} relies on other notes; leaving it to Update from sources"
        );
        db.set_page_refresh_blocked_reason_at_fence(
            page_id,
            REFRESH_BLOCKED_HAS_BASIS,
            &page_fence,
        )
        .await?;
        return Ok(RefreshOutcome::default());
    }
    let bases = if has_basis {
        db.list_basis_context(page_id).await?
    } else {
        Vec::new()
    };

    // Rebuild from the page's CURRENT sources: join table first, JSON column
    // fallback for legacy pages.
    let sources = db.get_page_sources(page_id).await?;
    let mut source_ids: Vec<String> = sources.iter().map(|s| s.memory_source_id.clone()).collect();
    if source_ids.is_empty() {
        source_ids = page.source_memory_ids.clone();
    }
    if let Some(candidate_sources) = candidate_sources {
        source_ids = candidate_sources.to_vec();
    }
    if source_ids.is_empty() && bases.is_empty() {
        log::warn!("[refresh] page '{}' has no sources, skipping", page.id);
        return Ok(RefreshOutcome::default());
    }

    // Shared synthesis/verification core (same gate as first-time creation):
    // synthesize the candidate, verify its [N] markers, write nothing yet.
    let candidate = match synthesize_candidate_body(
        db,
        llm,
        system_prompt,
        &page.title,
        page_workspace(&page),
        &source_ids,
        &bases,
    )
    .await?
    {
        CandidateOutcome::Unavailable => return Ok(RefreshOutcome::default()),
        CandidateOutcome::Discarded { discard_reason } => {
            // Surface the discard instead of leaving the page silently
            // "updating..." forever: persist WHY on the page row so reads can
            // show it, and hand it back on the outcome so the re-distill routes
            // can put it in their responses. The marker is cleared by every
            // successful canonical page write and by every mark-stale site (a
            // real source change re-arms the automatic retry).
            //
            // The marker rides the page fence read before generation. An
            // edit or source change that landed meanwhile already cleared it,
            // and a page recreated under the same id is a different row;
            // pinning the discard on either would pause the retry of a state
            // this generation never saw, so it is dropped and not attributed
            // to the current page.
            if !db
                .set_page_refresh_blocked_reason_at_fence(page_id, &discard_reason, &page_fence)
                .await?
            {
                log::info!(
                    "[refresh] page {page_id} changed during generation; dropping its obsolete discard"
                );
                return Ok(RefreshOutcome::default());
            }
            return Ok(RefreshOutcome {
                discard_reason: Some(discard_reason),
                ..RefreshOutcome::default()
            });
        }
        CandidateOutcome::Verified(candidate) => candidate,
    };

    // Atomicity (spec §5.1): pass the freshly verified [N] citation map INTO
    // update_page so content, citations, and changelog commit in ONE
    // transaction (mirrors grow_page). CAS: for source-changed refreshes
    // `require_stale = true` means the write only lands while `stale_reason IS
    // NOT NULL`, so a concurrent agent PUT that cleared staleness wins the race
    // without TOCTOU.
    //
    // A user-forced rebuild does not require staleness. It rides the page
    // fence (source revision AND incarnation), so a human edit that landed
    // during generation (which bumps `source_revision`) or a page recreated
    // under the same id still wins, and the edit protection it overrides is
    // released by the write that lands the body, never before.
    let summary = candidate.summary.clone();
    let user_forced_write = reason == RefreshReason::UserForced && candidate_sources.is_none();

    // "Update from sources" on a note that relies on other notes is a preview:
    // the run stages a revision card, whatever the note's ownership, and the
    // card remembers which version of each basis it read. Accepting it
    // confirms the ones that still match. A note without a basis never takes
    // this branch.
    if user_forced_write && has_basis {
        let old_sources: std::collections::HashSet<&str> =
            page.source_memory_ids.iter().map(String::as_str).collect();
        let new_sources: std::collections::HashSet<&str> =
            source_ids.iter().map(String::as_str).collect();
        if candidate.content == page.content && old_sources == new_sources {
            // Nothing to change: no card, and nothing is confirmed. Only the
            // compile watermark advances.
            db.clear_page_refresh_blocked_reason_at_fence(page_id, page.version, &page_fence)
                .await?;
            db.clear_page_staleness_at_fence(page_id, page.version, &page_fence)
                .await?;
            return Ok(RefreshOutcome {
                acknowledged: true,
                ..RefreshOutcome::default()
            });
        }
        let snapshot: Vec<crate::db::BasisSnapshotEntry> =
            bases.iter().map(|b| b.snapshot.clone()).collect();
        let Some(staged) = crate::post_write::stage_basis_preview_card(
            db,
            &page,
            &candidate.content,
            &source_ids,
            &page_fence,
            reason.edited_by(),
            &snapshot,
        )
        .await?
        else {
            log::info!(
                "[refresh] page {page_id} changed or was replaced during generation; no preview card staged"
            );
            return Ok(RefreshOutcome::default());
        };
        if staged.gated {
            db.clear_page_refresh_blocked_reason_at_fence(page_id, page.version, &page_fence)
                .await?;
        }
        db.clear_page_staleness_at_fence(page_id, page.version, &page_fence)
            .await?;
        return Ok(RefreshOutcome {
            gated: staged.gated,
            revision_card_id: staged.revision_card_id,
            ..RefreshOutcome::default()
        });
    }

    let req = UpdatePageRequest {
        content: candidate.content,
        source_memory_ids: source_ids,
        expected_version: candidate_sources.map(|_| page.version),
        caller_id: None,
        operation_id: None,
    };
    let citations = Some((candidate.citations_json, candidate.stats_summary));
    let result = if user_forced_write {
        crate::post_write::page_write(
            db,
            crate::post_write::PageWrite::UserForcedUpdate {
                page_id,
                req,
                edited_by: reason.edited_by(),
                expected_fence: &page_fence,
                knowledge_path,
                citations,
            },
        )
        .await?
    } else {
        crate::post_write::update_page_at_source_revision(
            db,
            page_id,
            req,
            reason.edited_by(),
            candidate_sources.is_none(),
            source_revision,
            knowledge_path,
            citations,
        )
        .await?
    };

    if user_forced_write && result.gated {
        // A successful proposal supersedes an earlier synthesis failure even
        // when the page was already fresh. Clear only the fenced error marker;
        // the review gate still leaves page content and ownership untouched.
        db.clear_page_refresh_blocked_reason_at_fence(page_id, page.version, &page_fence)
            .await?;
    }

    // The one-line summary is derived from the body, so a rebuilt body gets
    // a rebuilt summary; otherwise the pull quote outlives the prose it
    // summarised. A gated or unchanged outcome leaves the stored page alone.
    // A user-forced write is fenced on the incarnation it just wrote, so a
    // page recreated under the same id right after that write keeps its own
    // summary.
    if result.wrote && user_forced_write {
        db.update_page_summary_at_incarnation(page_id, summary.as_deref(), &page_fence.incarnation)
            .await?;
    } else if result.wrote {
        db.update_page_summary(page_id, summary.as_deref()).await?;
    }

    Ok(RefreshOutcome {
        wrote: result.wrote,
        gated: result.gated,
        acknowledged: result.acknowledged,
        revision_card_id: result.revision_card_id,
        discard_reason: None,
    })
}

/// Re-distill a single page by reloading all source memories and recompiling
/// with the LLM. Returns the full [`RefreshOutcome`] so callers can report
/// honest counts (`wrote`) AND say why a no-op was a no-op
/// (`discard_reason` when the citation gate discarded the synthesis; the
/// other no-ops — no source memories, empty LLM output — stay reason-less
/// defaults). Returns `Err(WenlanError::Llm)` only when the LLM call itself
/// fails.
pub async fn deep_distill_single(
    db: &MemoryDB,
    llm: Option<&Arc<dyn LlmProvider>>,
    prompts: &PromptRegistry,
    page_id: &str,
    knowledge_path: Option<&std::path::Path>,
) -> Result<RefreshOutcome, WenlanError> {
    let llm = match llm {
        Some(l) if l.is_available() => l,
        Some(_) => {
            return Err(WenlanError::Llm(
                "LLM not available for re-distillation".into(),
            ));
        }
        None => {
            return Err(WenlanError::Llm(
                "No LLM available for re-distillation".into(),
            ));
        }
    };

    refresh_page(
        db,
        llm,
        prompts,
        page_id,
        RefreshReason::Explicit,
        knowledge_path,
    )
    .await
}

/// User-forced Re-distill of a single page (the Re-distill routes). Same
/// contract as [`deep_distill_single`], except a `user_edited` page is rebuilt
/// in place instead of being staged as a revision card. Edit protection is
/// never cleared ahead of generation: `user_edited`, `stale_reason`, and any
/// pending revision card survive a provider failure, a dropped future, and
/// discarded or unavailable output. Only the UPDATE that lands the new body
/// releases them, under the source-revision fence read before generation.
pub async fn force_redistill_single(
    db: &MemoryDB,
    llm: Option<&Arc<dyn LlmProvider>>,
    prompts: &PromptRegistry,
    page_id: &str,
    knowledge_path: Option<&std::path::Path>,
) -> Result<RefreshOutcome, WenlanError> {
    let llm = match llm {
        Some(l) if l.is_available() => l,
        Some(_) => {
            return Err(WenlanError::Llm(
                "LLM not available for re-distillation".into(),
            ));
        }
        None => {
            return Err(WenlanError::Llm(
                "No LLM available for re-distillation".into(),
            ));
        }
    };

    refresh_page(
        db,
        llm,
        prompts,
        page_id,
        RefreshReason::UserForced,
        knowledge_path,
    )
    .await
}

/// Apply a merge result based on the stability tier of the involved memories.
pub(crate) async fn apply_merge_by_tier(
    db: &MemoryDB,
    source_ids: &[String],
    merged_content: &str,
    proposal_id: &str,
    tier: &StabilityTier,
) -> Result<(), WenlanError> {
    match tier {
        StabilityTier::Ephemeral => {
            // Auto-apply silently
            db.apply_merge(source_ids, merged_content).await?;
            resolve_proposal(db, proposal_id, ResolveStatus::AutoApplied, "daemon").await?;
            log::info!(
                "[refinery] auto-applied merge (ephemeral) for {}",
                proposal_id
            );
        }
        StabilityTier::Standard => {
            // Auto-apply with notification (toast emitted by caller if app_handle available)
            db.apply_merge(source_ids, merged_content).await?;
            resolve_proposal(db, proposal_id, ResolveStatus::AutoApplied, "daemon").await?;
            log::info!(
                "[refinery] auto-applied merge (standard, notify) for {}",
                proposal_id
            );
        }
        StabilityTier::Protected => {
            // Queue for human review — don't auto-apply
            resolve_proposal(db, proposal_id, ResolveStatus::AwaitingReview, "daemon").await?;
            log::info!(
                "[refinery] queued merge for review (protected) for {}",
                proposal_id
            );
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "distill_truth_test.rs"]
mod distill_truth_test;

#[cfg(test)]
mod tests {

    #[test]
    fn summary_is_the_first_prose_sentence_not_an_open_question() {
        let content = "Tally stores all data in one SQLite file next to the app [2][6]. \
The app process is the only writer [3].\n\n## Backup\n\nNightly copy to iCloud Drive [1].\n\n\
## Open Questions\n- Has an iCloud backup ever failed under network conditions?\n";
        assert_eq!(
            extract_page_summary(content).as_deref(),
            Some("Tally stores all data in one SQLite file next to the app.")
        );
    }

    #[test]
    fn summary_skips_leading_headings_and_blank_lines() {
        let content = "# Tally\n\n\nSingle-user invoicing on SQLite.\nMore prose.\n";
        assert_eq!(
            extract_page_summary(content).as_deref(),
            Some("Single-user invoicing on SQLite.")
        );
    }

    #[test]
    fn summary_uses_a_leading_bullet_when_the_page_opens_with_a_list() {
        let content = "- Stripe over PayPal for EU clients [1]. Fees are lower.\n- Second point.\n";
        assert_eq!(
            extract_page_summary(content).as_deref(),
            Some("Stripe over PayPal for EU clients.")
        );
    }

    #[test]
    fn summary_drops_a_tldr_label_the_model_wrote() {
        let content = "TLDR: Tally stores all data in one SQLite file [2]. More prose.\n";
        assert_eq!(
            extract_page_summary(content).as_deref(),
            Some("Tally stores all data in one SQLite file.")
        );
    }

    #[test]
    fn summary_drops_a_bold_tl_dr_label_with_a_dash() {
        let content = "**TL;DR** \u{2014} Stripe over PayPal for EU clients. Fees are lower.\n";
        assert_eq!(
            extract_page_summary(content).as_deref(),
            Some("Stripe over PayPal for EU clients.")
        );
    }

    #[test]
    fn summary_keeps_prose_that_merely_starts_with_those_letters() {
        let content = "TLDRs are no substitute for the page. More prose.\n";
        assert_eq!(
            extract_page_summary(content).as_deref(),
            Some("TLDRs are no substitute for the page.")
        );
    }

    #[test]
    fn summary_is_none_when_only_open_questions_remain() {
        let content = "## Open Questions\n- Is there evidence backups ever failed?\n- What if the VPS is unreachable?\n";
        assert_eq!(extract_page_summary(content), None);
    }

    use super::*;
    use crate::llm_provider::{LlmBackend, LlmError, MockProvider, SequencedMockProvider};
    use crate::sources::RawDocument;
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;

    async fn upsert_test_sources(
        db: &crate::db::MemoryDB,
        docs: &[(&str, &str)],
        space: Option<&str>,
    ) {
        let rows = docs
            .iter()
            .map(|(source_id, content)| RawDocument {
                source: "memory".to_string(),
                source_id: (*source_id).to_string(),
                title: (*source_id).to_string(),
                content: (*content).to_string(),
                last_modified: chrono::Utc::now().timestamp(),
                memory_type: Some("fact".to_string()),
                space: space.map(str::to_string),
                source_agent: Some("test".to_string()),
                confirmed: Some(true),
                ..Default::default()
            })
            .collect();
        db.upsert_documents(rows).await.unwrap();
    }

    struct PromptRecordingProvider {
        prompts: Mutex<Vec<String>>,
    }

    impl PromptRecordingProvider {
        fn new() -> Self {
            Self {
                prompts: Mutex::new(Vec::new()),
            }
        }

        fn prompts(&self) -> Vec<String> {
            self.prompts.lock().unwrap().clone()
        }
    }

    struct BoundedRecordingProvider {
        response: String,
        input_limit: usize,
        reject_physical_context: bool,
        prompts: Mutex<Vec<String>>,
    }

    impl BoundedRecordingProvider {
        fn new(response: impl Into<String>, input_limit: usize) -> Self {
            Self {
                response: response.into(),
                input_limit,
                reject_physical_context: false,
                prompts: Mutex::new(Vec::new()),
            }
        }

        fn rejecting_physical_context() -> Self {
            Self {
                response: String::new(),
                input_limit: 100_000,
                reject_physical_context: true,
                prompts: Mutex::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<String> {
            self.prompts.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl LlmProvider for BoundedRecordingProvider {
        async fn generate(&self, request: LlmRequest) -> Result<String, LlmError> {
            self.prompts.lock().unwrap().push(request.user_prompt);
            if self.reject_physical_context {
                return Err(LlmError::InputContextTooLarge {
                    input_tokens: 7_500,
                    input_limit: 6_144,
                });
            }
            Ok(self.response.clone())
        }

        fn is_available(&self) -> bool {
            true
        }

        fn name(&self) -> &str {
            "bounded-recording"
        }

        fn backend(&self) -> LlmBackend {
            LlmBackend::OnDevice
        }

        fn kind(&self) -> &'static str {
            "mock"
        }

        fn synthesis_token_limit(&self) -> usize {
            self.input_limit
        }
    }

    #[async_trait::async_trait]
    impl LlmProvider for PromptRecordingProvider {
        async fn generate(&self, request: LlmRequest) -> Result<String, LlmError> {
            let user_prompt = request.user_prompt;
            self.prompts.lock().unwrap().push(user_prompt.clone());
            if user_prompt.contains("[[Related Page]]") {
                Ok("Refreshed page prose keeps the valid [[Related Page]] wikilink from the prior page body. [1]".to_string())
            } else {
                Ok(
                    "Refreshed page prose drops the valid wikilink from the prior page body. [1]"
                        .to_string(),
                )
            }
        }

        fn is_available(&self) -> bool {
            true
        }

        fn name(&self) -> &str {
            "prompt-recording"
        }

        fn backend(&self) -> LlmBackend {
            LlmBackend::OnDevice
        }

        fn kind(&self) -> &'static str {
            "mock"
        }
    }

    /// Records every `(system_prompt, user_prompt)` pair `generate` was
    /// called with, so a test can assert a specific system prompt (e.g. the
    /// refine-clusters coherence gate) was never invoked. Echoes the input
    /// content back (citation-marked) for the distill-body call so the
    /// hallucination/faithfulness check passes trivially, and a short fixed
    /// title otherwise.
    struct GateRecordingProvider {
        calls: Mutex<Vec<(Option<String>, String)>>,
    }

    impl GateRecordingProvider {
        fn new() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
            }
        }

        fn saw_system_prompt(&self, needle: &str) -> bool {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .any(|(sys, _)| sys.as_deref() == Some(needle))
        }
    }

    #[async_trait::async_trait]
    impl LlmProvider for GateRecordingProvider {
        async fn generate(&self, request: LlmRequest) -> Result<String, LlmError> {
            let label = request.label.clone();
            self.calls
                .lock()
                .unwrap()
                .push((request.system_prompt.clone(), request.user_prompt.clone()));
            if label.as_deref() == Some("distill_body") {
                Ok(format!("{} [1]", request.user_prompt))
            } else {
                Ok("Test Topic".to_string())
            }
        }

        fn is_available(&self) -> bool {
            true
        }

        fn name(&self) -> &str {
            "gate-recording"
        }

        fn backend(&self) -> LlmBackend {
            LlmBackend::OnDevice
        }

        fn kind(&self) -> &'static str {
            "mock"
        }
    }

    #[tokio::test]
    async fn on_device_only_compile_skips_coherence_gate() {
        let (db, _db_dir) = crate::db::tests::test_db().await;

        // Two well-separated topics, each a real cluster (space is the ONLY
        // partition, so two distinct spaces guarantee two distinct clusters).
        // Neither sets entity_id/entity_name, so both clusters share the
        // refine-clusters "unlinked" grouping key — with the gate ON, this
        // group has 2+ clusters and `refine_clusters_with_llm` would fire.
        let db_topic: Vec<(&str, &str)> = vec![
            (
                "topic_db",
                "The wenlan daemon persists document chunks in a libSQL table using an F32_BLOB column for the 768-dimension embedding vector, with DiskANN indexing enabling fast approximate nearest-neighbor search across the whole memory store.",
            ),
            (
                "topic_db",
                "libSQL's FTS5 virtual table stays synchronized with the chunks table through SQL triggers, so every insert or update to a chunk automatically refreshes its full-text search index without extra application code.",
            ),
            (
                "topic_db",
                "Hybrid retrieval combines the vector similarity score and the FTS5 rank using reciprocal rank fusion, blending semantic and lexical signals into one ranked result list for each search query.",
            ),
            (
                "topic_bread",
                "A sourdough levain needs to be fed with equal parts flour and water twice a day to stay active enough to leaven a loaf, and it should roughly double in volume within four to six hours after each feeding.",
            ),
            (
                "topic_bread",
                "Increasing the hydration of a bread dough to around eighty percent produces a much more open, irregular crumb structure once it is baked, though it also makes the dough considerably harder to shape by hand.",
            ),
            (
                "topic_bread",
                "Kneading a wheat dough develops long gluten strands that trap the carbon dioxide bubbles produced by yeast fermentation, which is what allows the loaf to rise instead of collapsing in the oven.",
            ),
        ];
        for (i, (space, content)) in db_topic.iter().enumerate() {
            let doc = RawDocument {
                source: "memory".to_string(),
                source_id: format!("gate_test_{}", i),
                title: content.to_string(),
                content: content.to_string(),
                space: Some(space.to_string()),
                ..Default::default()
            };
            db.upsert_documents(vec![doc]).await.unwrap();
        }

        let provider = Arc::new(GateRecordingProvider::new());
        let llm: Arc<dyn LlmProvider> = provider.clone();
        let prompts = PromptRegistry::default();

        let result = distill_pages_scoped_gated(
            &db,
            Some(&llm),
            &prompts,
            &crate::tuning::DistillationConfig::default(),
            None,
            None,
            false, // on-device-only: coherence gate OFF
        )
        .await
        .unwrap();

        assert!(
            !provider.saw_system_prompt(&prompts.refine_clusters),
            "on-device-only compile must not invoke the LLM coherence gate"
        );
        assert!(
            !result.created.is_empty(),
            "compile should still synthesize pages despite skipping the coherence gate"
        );
    }

    #[tokio::test]
    async fn resolve_distill_target_ignores_unregistered_memory_space() {
        let (db, _db_dir) = crate::db::tests::test_db().await;
        let now_ts = chrono::Utc::now().timestamp();

        {
            let conn = db.test_primary_session().await;
            conn.execute(
                "INSERT INTO memories (id, source_id, title, content, chunk_index, chunk_type, memory_type, space, source_agent, created_at, last_modified, confirmed, stability, source) \
                 VALUES (?1, ?1, ?1, 'orphaned unregistered space content', 0, 'text', 'fact', 'ghost', 'claude-code', ?2, ?2, 1, 'confirmed', 'memory')",
                libsql::params!["mem_orphan_space".to_string(), now_ts],
            )
            .await
            .unwrap();
        }

        let target = resolve_distill_target(&db, "ghost").await.unwrap();
        assert!(
            target.is_none(),
            "unregistered legacy memory labels must not resolve as distill space targets: {target:?}"
        );
    }

    #[tokio::test]
    async fn distill_one_cluster_persists_verified_and_unverified_citations() {
        let (db, _db_dir) = crate::db::tests::test_db().await;
        let daemon_content =
            "The Wenlan daemon binds to port 7878 by default on localhost, providing \
             the HTTP API surface used by the CLI and the MCP bridge for all downstream \
             tools that talk to the local memory store.";
        let embed_content =
            "FastEmbed uses the BGE-Base-EN embeddings model with 768 dimensions for \
             vector search across every stored memory and page in the local database, \
             combined with FTS5 for hybrid retrieval.";
        let api_content =
            "The Wenlan server exposes local HTTP endpoints for tools that need to store, \
             search, and refresh memories through the daemon.";
        upsert_test_sources(
            &db,
            &[
                ("mem_daemon", daemon_content),
                ("mem_embed", embed_content),
                ("mem_api", api_content),
            ],
            None,
        )
        .await;

        let cluster = crate::db::DistillationCluster {
            source_ids: vec!["mem_daemon".into(), "mem_embed".into(), "mem_api".into()],
            contents: vec![
                daemon_content.to_string(),
                embed_content.to_string(),
                api_content.to_string(),
            ],
            entity_id: None,
            entity_name: Some("Wenlan daemon".into()),
            space: None,
            estimated_tokens: 120,
            centroid_embedding: None,
        };

        let llm: Arc<dyn LlmProvider> = Arc::new(MockProvider::new(
            "The Wenlan daemon binds to port 7878 by default.[1]\n\n\
             Second claim: an entirely unrelated statement about the flavor of ice cream.[2]",
        ));
        let prompts = PromptRegistry::default();

        let page_id = distill_one_cluster(&db, &llm, &prompts, &cluster, None)
            .await
            .unwrap()
            .expect("cluster should synthesize a page");

        let page = db.get_page(&page_id).await.unwrap().unwrap();
        assert_eq!(page.citations.len(), 2, "citations: {:?}", page.citations);
        assert_eq!(page.citations[0].status, "verified");
        assert_eq!(page.citations[0].locator, "mem_daemon");
        assert!(page.content.contains("[1]"));
    }

    #[tokio::test]
    async fn distill_one_cluster_create_goes_through_page_write_birth_contract() {
        let (db, _db_dir) = crate::db::tests::test_db().await;
        let source_ids = [
            "daemon_pagewrite_a",
            "daemon_pagewrite_b",
            "daemon_pagewrite_c",
        ];
        for sid in source_ids {
            db.upsert_documents(vec![RawDocument {
                source: "memory".to_string(),
                source_id: sid.to_string(),
                title: sid.to_string(),
                content: match sid {
                    "daemon_pagewrite_a" => {
                        "Wenlan daemon synthesis keeps page writes on one canonical path so daemon phases cannot bypass canonical validation when they turn source memories into distilled pages."
                    }
                    "daemon_pagewrite_b" => {
                        "Canonical PageWrite births unconfirmed pages for review, matching the same review contract used by agent-created HTTP pages."
                    }
                    _ => "Daemon-created pages carry the same birth contract as HTTP pages and stay visible to reviewers before confirmation.",
                }
                .to_string(),
                last_modified: chrono::Utc::now().timestamp(),
                memory_type: Some("fact".to_string()),
                source_agent: Some("test".to_string()),
                confirmed: Some(true),
                ..Default::default()
            }])
            .await
            .unwrap();
        }
        let cluster = crate::db::DistillationCluster {
            source_ids: source_ids.iter().map(|sid| sid.to_string()).collect(),
            contents: vec![
                "Wenlan daemon synthesis keeps page writes on one canonical path so daemon phases cannot bypass canonical validation when they turn source memories into distilled pages.".to_string(),
                "Canonical PageWrite births unconfirmed pages for review, matching the same review contract used by agent-created HTTP pages.".to_string(),
                "Daemon-created pages carry the same birth contract as HTTP pages and stay visible to reviewers before confirmation.".to_string(),
            ],
            entity_id: None,
            entity_name: Some("Daemon PageWrite".to_string()),
            space: Some("work".to_string()),
            estimated_tokens: 120,
            centroid_embedding: None,
        };
        let llm: Arc<dyn LlmProvider> = Arc::new(MockProvider::new(
            "Wenlan daemon synthesis keeps page writes on one canonical path so daemon phases cannot bypass canonical validation when they turn source memories into distilled pages. [1]\n\
             Canonical PageWrite births unconfirmed pages for review, matching the same review contract used by agent-created HTTP pages. [2]\n\
             Daemon-created pages carry the same birth contract as HTTP pages and stay visible to reviewers before confirmation. [3]",
        ));
        let prompts = PromptRegistry::default();

        let page_id = distill_one_cluster(&db, &llm, &prompts, &cluster, None)
            .await
            .unwrap()
            .expect("cluster should synthesize a page");

        let page = db.get_page(&page_id).await.unwrap().unwrap();
        assert_eq!(
            page.review_status, "unconfirmed",
            "daemon create must inherit PageWrite page birth semantics"
        );
    }

    #[tokio::test]
    async fn refresh_page_re_projects_md_when_path_passed() {
        let (db, _db_dir) = crate::db::tests::test_db().await;
        let knowledge_dir = TempDir::new().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        let now_ts = chrono::Utc::now().timestamp();

        // Insert a seed memory row directly so get_memory_contents_by_ids
        // returns it. The mock body's [1] marker verifies against this content.
        {
            let conn = db.test_primary_session().await;
            conn.execute(
                "INSERT INTO memories (id, source_id, title, content, chunk_index, chunk_type, memory_type, space, source_agent, created_at, last_modified, confirmed, stability, source) \
                 VALUES (?1, ?1, ?1, 'recompiled body reference material', 0, 'text', 'fact', 'test', 'claude-code', ?2, ?2, 1, 'confirmed', 'memory')",
                libsql::params!["mem_seed".to_string(), now_ts],
            )
            .await
            .unwrap();
        }

        // Insert a page that cites that memory, then mark it stale.
        db.insert_page(
            "page_a",
            "Topic A",
            None,
            "original body",
            None,
            None,
            &["mem_seed"],
            &now,
        )
        .await
        .unwrap();
        db.set_page_stale("page_a", "source_updated").await.unwrap();

        let llm: Arc<dyn LlmProvider> = Arc::new(MockProvider::new("recompiled body [1]"));
        let prompts = PromptRegistry::default();
        let outcome = refresh_page(
            &db,
            &llm,
            &prompts,
            "page_a",
            RefreshReason::SourceChanged,
            Some(knowledge_dir.path()),
        )
        .await
        .unwrap();
        assert!(outcome.wrote, "refresh should write");

        // Verify md file was re-projected into the knowledge directory.
        let entries: Vec<_> = std::fs::read_dir(knowledge_dir.path())
            .unwrap()
            .flatten()
            .filter(|e| e.path().extension().map(|x| x == "md").unwrap_or(false))
            // The OKF root `index.md` is a reserved document, not a page.
            .filter(|e| !e.file_name().eq_ignore_ascii_case("index.md"))
            .collect();
        assert_eq!(entries.len(), 1, "exactly one md file");
        let content = std::fs::read_to_string(entries[0].path()).unwrap();
        assert!(
            content.contains("recompiled body"),
            "md body should reflect LLM output"
        );
    }

    #[tokio::test]
    async fn deep_distill_single_prompt_includes_same_space_existing_titles_hint() {
        let (db, _db_dir) = crate::db::tests::test_db().await;
        let now = chrono::Utc::now().to_rfc3339();
        let now_ts = chrono::Utc::now().timestamp();

        {
            let conn = db.test_primary_session().await;
            conn.execute(
                "INSERT INTO memories (id, source_id, title, content, chunk_index, chunk_type, memory_type, space, source_agent, created_at, last_modified, confirmed, stability, source) \
                 VALUES (?1, ?1, ?1, 'The target topic should retain the valid wikilink to Related Page when refreshed.', 0, 'text', 'fact', 'work', 'claude-code', ?2, ?2, 1, 'confirmed', 'memory')",
                libsql::params!["mem_target".to_string(), now_ts],
            )
            .await
            .unwrap();
        }

        db.insert_page(
            "page_related",
            "Related Page",
            None,
            "A work-space page that should be offered as a real wikilink target.",
            None,
            Some("work"),
            &[],
            &now,
        )
        .await
        .unwrap();
        db.insert_page(
            "page_private",
            "Private Page",
            None,
            "A personal-space page that must not be offered in a work refresh prompt.",
            None,
            Some("personal"),
            &[],
            &now,
        )
        .await
        .unwrap();
        db.insert_page(
            "page_target",
            "Target Page",
            None,
            "Original body links to [[Related Page]].",
            None,
            Some("work"),
            &["mem_target"],
            &now,
        )
        .await
        .unwrap();
        db.set_page_stale("page_target", "source_updated")
            .await
            .unwrap();

        let provider = Arc::new(PromptRecordingProvider::new());
        let llm: Arc<dyn LlmProvider> = provider.clone();
        let prompts = PromptRegistry::default();

        let outcome = deep_distill_single(&db, Some(&llm), &prompts, "page_target", None)
            .await
            .unwrap();
        assert!(
            outcome.wrote,
            "deep refresh should write the provider output"
        );

        let seen = provider.prompts();
        let user_prompt = seen
            .first()
            .expect("deep refresh must send one compile prompt");
        assert!(
            user_prompt.starts_with("Page titles mentioned in this page's sources or linked basis (link labels only, not evidence):"),
            "refresh prompt must label matching titles as navigation hints, got:\n{user_prompt}"
        );
        assert!(
            user_prompt.contains("[[Related Page]]"),
            "same-space existing title must be offered as a valid wikilink target, got:\n{user_prompt}"
        );
        assert!(
            !user_prompt.contains("[[Private Page]]"),
            "cross-space existing title must not be offered in a work refresh prompt, got:\n{user_prompt}"
        );
        assert!(
            user_prompt.contains("not evidence"),
            "title hints must be explicitly separated from factual evidence"
        );

        let page = db.get_page("page_target").await.unwrap().unwrap();
        assert!(
            page.content.contains("[[Related Page]]"),
            "refresh should retain already-valid wikilinks when the prompt exposes the real title, got:\n{}",
            page.content
        );
    }

    #[tokio::test]
    async fn refresh_prompt_excludes_unrelated_titles_but_keeps_sources_and_basis() {
        assert!(contains_whole_title("the source mentions [[ai]]", "AI"));
        assert!(
            !contains_whole_title("a paid plan is available", "AI"),
            "a title must not match inside an unrelated word"
        );
        assert!(
            contains_whole_title("資料提到文蘭功能", "文蘭"),
            "Han titles can appear adjacent to other Han prose"
        );

        let (db, _db_dir) = crate::db::tests::test_db().await;
        let now = chrono::Utc::now().to_rfc3339();
        upsert_test_sources(
            &db,
            &[("mem_cedar_atlas", "Cedar Atlas is based in Seattle.")],
            Some("work"),
        )
        .await;

        db.insert_page(
            "page_unrelated_sentinel",
            "UNRELATED_SENTINEL_TITLE_7F3A",
            None,
            "This page is unrelated to Cedar Atlas.",
            None,
            Some("work"),
            &[],
            &now,
        )
        .await
        .unwrap();
        db.insert_page(
            "page_explicit_basis",
            "Atlas Project Note",
            None,
            "The Atlas project's launch review is scheduled for October.",
            None,
            Some("work"),
            &[],
            &now,
        )
        .await
        .unwrap();
        db.insert_page(
            "page_cedar_atlas",
            "Cedar Atlas",
            None,
            "Original page body.",
            None,
            Some("work"),
            &["mem_cedar_atlas"],
            &now,
        )
        .await
        .unwrap();
        let basis_page = db.get_page("page_explicit_basis").await.unwrap().unwrap();
        db.mark_page_basis(
            "page_cedar_atlas",
            "page_explicit_basis",
            basis_page.version,
            "test",
        )
        .await
        .unwrap();
        let bases = db.list_basis_context("page_cedar_atlas").await.unwrap();
        assert_eq!(bases.len(), 1, "the test target must have one linked basis");

        let provider = Arc::new(BoundedRecordingProvider::new(
            "Cedar Atlas is based in Seattle [1].",
            100_000,
        ));
        let llm: Arc<dyn LlmProvider> = provider.clone();
        let outcome = synthesize_candidate_body(
            &db,
            &llm,
            "Summarize the topic using the supplied context.",
            "Cedar Atlas",
            Some("work"),
            &["mem_cedar_atlas".to_string()],
            &bases,
        )
        .await
        .unwrap();
        assert!(
            matches!(outcome, CandidateOutcome::Verified(_)),
            "the source-backed synthesis should pass citation verification"
        );

        let calls = provider.calls();
        let prompt = calls.first().expect("one refresh synthesis call");
        assert!(
            !prompt.contains("UNRELATED_SENTINEL_TITLE_7F3A"),
            "refresh context must not include unrelated page titles: {prompt}"
        );
        assert!(
            prompt.contains("Topic: Cedar Atlas"),
            "topic is retained: {prompt}"
        );
        assert!(
            prompt.contains(
                "--- BEGIN SOURCE [1] ---\nCedar Atlas is based in Seattle.\n--- END SOURCE [1] ---"
            ),
            "numbered source context is retained: {prompt}"
        );
        assert!(
            prompt.contains("### [[Atlas Project Note]]")
                && prompt.contains("The Atlas project's launch review is scheduled for October."),
            "explicitly linked basis context is retained: {prompt}"
        );
    }

    #[tokio::test]
    async fn page_compile_prompt_caps_topic_and_existing_title_text() {
        let (db, _dir) = crate::db::tests::test_db().await;
        let long_title = "T".repeat(600);
        let long_topic = "Q".repeat(600);
        let now = chrono::Utc::now().to_rfc3339();
        db.insert_page(
            "bounded_prompt_existing_page",
            &long_title,
            None,
            "short body",
            None,
            None,
            &[],
            &now,
        )
        .await
        .unwrap();

        let prompt =
            build_page_compile_user_prompt(&db, &long_topic, None, "[1] bounded source").await;

        assert!(
            prompt.chars().count() < 600,
            "fixed-count title hints still need a hard character bound; got {} chars",
            prompt.chars().count()
        );
        assert!(prompt.ends_with("[1] bounded source"));
    }

    // ── refresh_page: the ONE re-distill/repair op ──────────────────────────

    #[tokio::test]
    async fn refresh_page_rebuilds_machine_page_and_bumps_changelog() {
        let (db, _db_dir) = crate::db::tests::test_db().await;
        let now = chrono::Utc::now().to_rfc3339();
        let now_ts = chrono::Utc::now().timestamp();

        // Seed the cited memory. The synthesized body echoes it so the [1]
        // marker verifies and the citation gate passes.
        {
            let conn = db.test_primary_session().await;
            conn.execute(
                "INSERT INTO memories (id, source_id, title, content, chunk_index, chunk_type, memory_type, space, source_agent, created_at, last_modified, confirmed, stability, source) \
                 VALUES (?1, ?1, ?1, 'Tokio is an async runtime for Rust programs', 0, 'text', 'fact', 'test', 'claude-code', ?2, ?2, 1, 'confirmed', 'memory')",
                libsql::params!["mem_seed".to_string(), now_ts],
            )
            .await
            .unwrap();
        }
        db.insert_page(
            "page_m",
            "Tokio",
            None,
            "original body",
            None,
            None,
            &["mem_seed"],
            &now,
        )
        .await
        .unwrap();
        db.set_page_stale("page_m", "source_updated").await.unwrap();

        let llm: Arc<dyn LlmProvider> = Arc::new(MockProvider::new(
            "Tokio is an async runtime for Rust programs [1]",
        ));
        let prompts = PromptRegistry::default();

        let outcome = refresh_page(
            &db,
            &llm,
            &prompts,
            "page_m",
            RefreshReason::SourceChanged,
            None,
        )
        .await
        .unwrap();
        assert!(outcome.wrote, "machine page should be rewritten in place");
        assert!(!outcome.gated);
        assert!(outcome.revision_card_id.is_none());

        let page = db.get_page("page_m").await.unwrap().unwrap();
        assert!(
            page.content.contains("Tokio is an async runtime"),
            "prose rebuilt from evidence, got: {}",
            page.content
        );
        assert!(
            !page.citations.is_empty(),
            "per-claim citations persisted with the content"
        );

        let changelog_raw = db.get_page_changelog("page_m").await.unwrap();
        let changelog: Vec<serde_json::Value> = serde_json::from_str(&changelog_raw).unwrap();
        let latest = changelog
            .last()
            .expect("refresh must append a changelog entry");
        assert_eq!(
            latest.get("edited_by").and_then(|v| v.as_str()),
            Some("re_distill"),
            "changelog records the refresh provenance"
        );
        assert!(
            latest.get("citations_summary").is_some(),
            "citations committed atomically with the content bump"
        );
    }

    #[tokio::test]
    async fn refresh_page_rebuilds_the_summary_from_the_new_body() {
        let (db, _db_dir) = crate::db::tests::test_db().await;
        let now = chrono::Utc::now().to_rfc3339();
        let now_ts = chrono::Utc::now().timestamp();
        {
            let conn = db.test_primary_session().await;
            conn.execute(
                "INSERT INTO memories (id, source_id, title, content, chunk_index, chunk_type, memory_type, space, source_agent, created_at, last_modified, confirmed, stability, source) \
                 VALUES (?1, ?1, ?1, 'Tokio is an async runtime for Rust programs', 0, 'text', 'fact', 'test', 'claude-code', ?2, ?2, 1, 'confirmed', 'memory')",
                libsql::params!["mem_seed".to_string(), now_ts],
            )
            .await
            .unwrap();
        }
        // The stored summary is the pre-#642 symptom: an open question that
        // used to be the page's first bullet.
        db.insert_page(
            "page_m",
            "Tokio",
            Some("Does Tokio scale past one core?"),
            "original body\n\n## Open Questions\n- Does Tokio scale past one core?",
            None,
            None,
            &["mem_seed"],
            &now,
        )
        .await
        .unwrap();
        db.set_page_stale("page_m", "source_updated").await.unwrap();

        let llm: Arc<dyn LlmProvider> = Arc::new(MockProvider::new(
            "Tokio is an async runtime for Rust programs [1]",
        ));
        let outcome = refresh_page(
            &db,
            &llm,
            &PromptRegistry::default(),
            "page_m",
            RefreshReason::SourceChanged,
            None,
        )
        .await
        .unwrap();
        assert!(outcome.wrote);

        let page = db.get_page("page_m").await.unwrap().unwrap();
        assert_eq!(
            page.summary.as_deref(),
            Some("Tokio is an async runtime for Rust programs"),
            "a rebuilt body gets a rebuilt summary"
        );
    }

    /// Round-4 finding (MEDIUM): `refresh_page_with_prompt` used to read
    /// `get_page` before `get_page_source_revision` -- the same read-order
    /// hazard as the citation backfill (citations.rs), but here it can
    /// re-attach a removed source instead of dropping a new one. A detach
    /// landing between the page read and the (join-table-derived)
    /// `get_page_sources` read leaves `page.source_memory_ids` holding the
    /// pre-detach list while `get_page_sources` already reflects the detach
    /// (empty) -- the fallback (`if source_ids.is_empty() { source_ids =
    /// page.source_memory_ids }`) then picks the STALE list right back up,
    /// and if `source_revision` was captured after the detach too (the old,
    /// broken order), the terminal CAS would pass and write it.
    ///
    /// Seam-level proof mirroring the fixed order exactly (there is no live
    /// interposable await in the real function to race the way
    /// `BlockingCitationProvider` races the LLM call, same reasoning as
    /// `combined_cas_read_order_rejects_write_racing_a_mid_read_attach` in
    /// db.rs): read the fence FIRST (fixed order), then `get_page` (still
    /// pre-detach), then the detach itself, then `get_page_sources` (already
    /// post-detach, empty -- triggering the fallback onto the now-stale
    /// `page` snapshot), then run the exact write the real function runs,
    /// using the counter captured before any of this.
    #[tokio::test]
    async fn refresh_fallback_rejects_write_racing_a_mid_read_detach() {
        let (db, _db_dir) = crate::db::tests::test_db().await;
        let now = chrono::Utc::now().to_rfc3339();
        db.insert_page(
            "page_refresh_race",
            "T",
            None,
            "original body",
            None,
            None,
            &[],
            &now,
        )
        .await
        .unwrap();
        db.link_page_source("page_refresh_race", "mem_old", "test")
            .await
            .unwrap();
        db.set_page_stale("page_refresh_race", "source_updated")
            .await
            .unwrap();

        // Fixed order: read the fence FIRST.
        let source_revision = db
            .get_page_source_revision("page_refresh_race")
            .await
            .unwrap();

        // `get_page`, still pre-detach.
        let page = db.get_page("page_refresh_race").await.unwrap().unwrap();
        assert_eq!(
            page.source_memory_ids,
            vec!["mem_old".to_string()],
            "sanity: get_page (read second) must still see mem_old, before the race below"
        );

        // A concurrent detach lands between the page read and the sources
        // read -- exactly the window the fixed order (fence captured before
        // either) is meant to survive.
        db.replace_page_sources("page_refresh_race", &[], "test")
            .await
            .unwrap();

        // `get_page_sources`, already post-detach.
        let sources = db.get_page_sources("page_refresh_race").await.unwrap();
        let mut source_ids: Vec<String> =
            sources.iter().map(|s| s.memory_source_id.clone()).collect();
        if source_ids.is_empty() {
            source_ids = page.source_memory_ids.clone();
        }
        assert_eq!(
            source_ids,
            vec!["mem_old".to_string()],
            "sanity: the fallback must pick up the stale (pre-detach) list, \
             reproducing the exact hazard this test proves the fence closes"
        );

        let result = crate::post_write::update_page_at_source_revision(
            &db,
            "page_refresh_race",
            wenlan_types::requests::UpdatePageRequest {
                content: "refreshed body".to_string(),
                source_memory_ids: source_ids,
                expected_version: None,
                caller_id: None,
                operation_id: None,
            },
            "re_distill",
            true,
            source_revision,
            None,
            None,
        )
        .await
        .unwrap();
        assert!(
            !result.wrote,
            "the stale (pre-detach) source_revision must reject the write, \
             not re-attach the just-detached source: {:?}",
            result
        );

        let after = db.get_page("page_refresh_race").await.unwrap().unwrap();
        assert!(
            after.source_memory_ids.is_empty(),
            "the rejected write must not have re-attached the detached source: {:?}",
            after.source_memory_ids
        );
    }

    #[tokio::test]
    async fn refresh_page_keeps_cited_synthesis_with_uncited_elaboration() {
        let (db, _db_dir) = crate::db::tests::test_db().await;
        let now = chrono::Utc::now().to_rfc3339();

        let runtime_fact = "Tokio is an async runtime for Rust programs.";
        let storage_fact = "LibSQL persists Wenlan memory chunks with vector embeddings.";
        let bridge_fact = "The MCP bridge talks to the local daemon over HTTP.";
        upsert_test_sources(
            &db,
            &[
                ("mem_1_runtime", runtime_fact),
                ("mem_2_storage", storage_fact),
                ("mem_3_bridge", bridge_fact),
            ],
            Some("test"),
        )
        .await;

        db.insert_page(
            "page_thin_sources",
            "Wenlan architecture",
            None,
            "original body",
            None,
            Some("test"),
            &["mem_1_runtime", "mem_2_storage", "mem_3_bridge"],
            &now,
        )
        .await
        .unwrap();
        db.set_page_stale("page_thin_sources", "source_updated")
            .await
            .unwrap();

        let llm: Arc<dyn LlmProvider> = Arc::new(MockProvider::new(
            "Thin-source page summary frames the topic for later review.\n\n\
             ## Runtime\n\
             Tokio is an async runtime for Rust programs [1]. This interpretation extends the operational lens with durable framing beyond the captured note.\n\n\
             ## Storage\n\
             LibSQL persists Wenlan memory chunks with vector embeddings [2]. The wider implication is an architectural posture rather than a discrete observation.\n\n\
             ## Bridge\n\
             The MCP bridge talks to the local daemon over HTTP [3]. This leaves room for coordination patterns outside the original capture.\n\n\
             ## Open Questions\n\
             - Which review cadence should govern this page?\n\
             - What migration signals would change the priority?",
        ));
        let prompts = PromptRegistry::default();

        let outcome = refresh_page(
            &db,
            &llm,
            &prompts,
            "page_thin_sources",
            RefreshReason::SourceChanged,
            None,
        )
        .await
        .unwrap();
        assert!(
            outcome.wrote,
            "citation-faithful refresh should write despite uncited elaboration"
        );

        let page = db.get_page("page_thin_sources").await.unwrap().unwrap();
        assert!(
            page.content.contains("operational lens"),
            "page content should reflect the refreshed synthesis, got: {}",
            page.content
        );
        assert_eq!(
            page.citations.len(),
            3,
            "all three cited claims should persist"
        );
    }

    /// Spec §5.1 (True source kinds): `refresh_page` is the PageWrite
    /// boundary that builds the per-claim `citations` numbered-source list.
    /// Its `NumberedSource.source_kind` must resolve per-source (folder docs
    /// -> `external_file`) instead of hardcoding `'memory'` for every
    /// citation, else the shipped citation chips (wenlan-app PR #70) always
    /// badge folder-doc-sourced claims as plain memories.
    #[tokio::test]
    async fn refresh_page_citations_resolve_external_file_source_kind() {
        let (db, _db_dir) = crate::db::tests::test_db().await;
        let now = chrono::Utc::now().to_rfc3339();
        let now_ts = chrono::Utc::now().timestamp();

        {
            let conn = db.test_primary_session().await;
            conn.execute(
                "INSERT INTO memories (id, source_id, title, content, chunk_index, chunk_type, memory_type, space, source_agent, created_at, last_modified, confirmed, stability, source) \
                 VALUES (?1, ?1, ?1, 'Tokio is an async runtime for Rust programs', 0, 'text', 'fact', 'test', 'claude-code', ?2, ?2, 1, 'confirmed', 'memory')",
                libsql::params!["mem_seed".to_string(), now_ts],
            )
            .await
            .unwrap();
            // Folder-doc row: source_agent='folder' + the `{source_id}::{provenance}`
            // id shape stamped by `sources::directory::document_source_id`.
            conn.execute(
                "INSERT INTO memories (id, source_id, title, content, chunk_index, chunk_type, memory_type, space, source_agent, created_at, last_modified, confirmed, stability, source) \
                 VALUES (?1, ?1, ?1, 'Folder documents describe async runtimes in Rust projects', 0, 'text', 'fact', 'test', 'folder', ?2, ?2, 1, 'confirmed', 'memory')",
                libsql::params!["folder-notes::rust/tokio.md".to_string(), now_ts],
            )
            .await
            .unwrap();
        }
        db.insert_page(
            "page_fk",
            "Tokio",
            None,
            "original body",
            None,
            None,
            &["mem_seed", "folder-notes::rust/tokio.md"],
            &now,
        )
        .await
        .unwrap();
        db.set_page_stale("page_fk", "source_updated")
            .await
            .unwrap();

        let llm: Arc<dyn LlmProvider> = Arc::new(MockProvider::new(
            "Tokio is an async runtime for Rust programs [1]. Folder documents describe async runtimes in Rust projects [2].",
        ));
        let prompts = PromptRegistry::default();

        let outcome = refresh_page(
            &db,
            &llm,
            &prompts,
            "page_fk",
            RefreshReason::SourceChanged,
            None,
        )
        .await
        .unwrap();
        assert!(outcome.wrote, "machine page should be rewritten in place");

        let page = db.get_page("page_fk").await.unwrap().unwrap();
        assert!(
            !page.citations.is_empty(),
            "per-claim citations persisted with the content"
        );
        let kind_of = |locator: &str| -> Option<String> {
            page.citations
                .iter()
                .find(|c| c.locator == locator)
                .map(|c| c.source_kind.clone())
        };
        assert_eq!(
            kind_of("mem_seed").as_deref(),
            Some("memory"),
            "a plain agent capture must carry source_kind='memory' in its citation"
        );
        assert_eq!(
            kind_of("folder-notes::rust/tokio.md").as_deref(),
            Some("external_file"),
            "a folder-doc source cited by refresh_page must carry source_kind='external_file', not 'memory'"
        );
    }

    /// Spec §5.1 (True source kinds): the initial page-creation compile path
    /// (`distill_one_cluster_with_tuning`) must resolve a folder-doc source's
    /// typed kind rather than hardcoding `'memory'` on the new page's citations.
    #[tokio::test]
    async fn distill_one_cluster_resolves_external_file_source_kind() {
        let (db, _dir) = crate::db::tests::test_db().await;
        let now_ts = chrono::Utc::now().timestamp();

        let mem_plain = "mem_cluster_kind_plain";
        let mem_doc = "folder-notes::rust/cluster.md";
        let mem_extra = "mem_cluster_kind_extra";
        {
            let conn = db.test_primary_session().await;
            conn.execute(
                "INSERT INTO memories (id, source_id, title, content, chunk_index, chunk_type, memory_type, space, source_agent, created_at, last_modified, confirmed, stability, source) \
                 VALUES (?1, ?1, ?1, 'Rust ownership rules prevent data races and dangling pointers at compile time without a garbage collector', 0, 'text', 'fact', 'test', 'claude-code', ?2, ?2, 1, 'confirmed', 'memory')",
                libsql::params![mem_plain.to_string(), now_ts],
            )
            .await
            .unwrap();
            // Folder-doc row: source_agent='folder' + the `{source_id}::{provenance}`
            // id shape stamped by `sources::directory::document_source_id`.
            conn.execute(
                "INSERT INTO memories (id, source_id, title, content, chunk_index, chunk_type, memory_type, space, source_agent, created_at, last_modified, confirmed, stability, source) \
                 VALUES (?1, ?1, ?1, 'The borrow checker enforces these ownership rules statically at compile time by tracking lifetimes across function boundaries', 0, 'text', 'fact', 'test', 'folder', ?2, ?2, 1, 'confirmed', 'memory')",
                libsql::params![mem_doc.to_string(), now_ts],
            )
            .await
            .unwrap();
            conn.execute(
                "INSERT INTO memories (id, source_id, title, content, chunk_index, chunk_type, memory_type, space, source_agent, created_at, last_modified, confirmed, stability, source) \
                 VALUES (?1, ?1, ?1, 'Rust lifetimes document how borrowed references remain valid while ownership moves between functions', 0, 'text', 'fact', 'test', 'claude-code', ?2, ?2, 1, 'confirmed', 'memory')",
                libsql::params![mem_extra.to_string(), now_ts],
            )
            .await
            .unwrap();
        }

        let plain_content =
            "Rust ownership rules prevent data races and dangling pointers at compile time without a garbage collector".to_string();
        let doc_content =
            "The borrow checker enforces these ownership rules statically at compile time by tracking lifetimes across function boundaries".to_string();
        let extra_content =
            "Rust lifetimes document how borrowed references remain valid while ownership moves between functions".to_string();
        let cluster = crate::db::DistillationCluster {
            source_ids: vec![
                mem_plain.to_string(),
                mem_doc.to_string(),
                mem_extra.to_string(),
            ],
            contents: vec![
                plain_content.clone(),
                doc_content.clone(),
                extra_content.clone(),
            ],
            entity_id: None,
            entity_name: Some("Rust ownership".into()),
            space: Some("test".into()),
            estimated_tokens: 60,
            centroid_embedding: None,
        };

        let tuning = crate::tuning::DistillationConfig::default();
        let llm: Arc<dyn LlmProvider> = Arc::new(MockProvider::new(&format!(
            "{plain_content}.[1] {doc_content}.[2]"
        )));
        let prompts = PromptRegistry::default();

        let page_id = distill_one_cluster_with_tuning(&db, &llm, &prompts, &cluster, &tuning, None)
            .await
            .unwrap()
            .expect("cluster should synthesize a new page");

        let page = db.get_page(&page_id).await.unwrap().unwrap();
        let kind_of = |locator: &str| -> Option<String> {
            page.citations
                .iter()
                .find(|c| c.locator == locator)
                .map(|c| c.source_kind.clone())
        };
        assert_eq!(
            kind_of(mem_plain).as_deref(),
            Some("memory"),
            "a plain agent capture must carry source_kind='memory' in its citation"
        );
        assert_eq!(
            kind_of(mem_doc).as_deref(),
            Some("external_file"),
            "a folder-doc source cited during initial page synth must carry source_kind='external_file', not 'memory'"
        );
    }

    #[tokio::test]
    async fn refresh_page_includes_and_cites_amendments_after_long_source_context() {
        let (db, _db_dir) = crate::db::tests::test_db().await;
        let repeated_context = "Background notes retain the project setup record. ".repeat(80);
        let amended_fact =
            "The latest migration review on October 22 confirms Cedar Atlas remains in Seattle.";
        let source = format!("{repeated_context}{amended_fact}");
        assert!(source.len() > 1_500);
        db.upsert_documents(vec![RawDocument {
            source: "memory".to_string(),
            source_id: "folder-docs::cedar-atlas.md".to_string(),
            title: "Cedar Atlas project notes".to_string(),
            content: source,
            last_modified: chrono::Utc::now().timestamp(),
            memory_type: Some("fact".to_string()),
            space: Some("test".to_string()),
            source_agent: Some("folder".to_string()),
            confirmed: Some(true),
            ..Default::default()
        }])
        .await
        .unwrap();
        let stored = db
            .get_memory_contents_by_ids(&["folder-docs::cedar-atlas.md".to_string()])
            .await
            .unwrap();
        assert_eq!(
            stored.len(),
            1,
            "the legacy read returns one canonical chunk"
        );
        assert!(
            !stored[0].1.contains(amended_fact),
            "the old chunk-zero reader should demonstrate that the final amendment is later"
        );
        let complete_source = db
            .get_refresh_source_contents_by_ids(&["folder-docs::cedar-atlas.md".to_string()])
            .await
            .unwrap();
        assert_eq!(complete_source.len(), 1);
        assert!(
            complete_source[0].1.contains(amended_fact),
            "the refresh reader must include the amendment stored in a later chunk"
        );

        let now = chrono::Utc::now().to_rfc3339();
        db.insert_page(
            "page_long_tail",
            "Cedar Atlas migration review",
            None,
            "The earlier review is still current.",
            None,
            None,
            &["folder-docs::cedar-atlas.md"],
            &now,
        )
        .await
        .unwrap();
        db.set_page_stale("page_long_tail", "source_updated")
            .await
            .unwrap();

        let provider = Arc::new(BoundedRecordingProvider::new(
            format!("{amended_fact} [1]"),
            100_000,
        ));
        let llm: Arc<dyn LlmProvider> = provider.clone();
        let prompts = PromptRegistry::default();
        let outcome = refresh_page(
            &db,
            &llm,
            &prompts,
            "page_long_tail",
            RefreshReason::SourceChanged,
            None,
        )
        .await
        .unwrap();

        let calls = provider.calls();
        assert_eq!(calls.len(), 1, "one model request should be made");
        assert!(
            calls[0].contains(amended_fact),
            "the full source tail must reach the synthesis prompt"
        );
        assert!(
            outcome.wrote,
            "the verified source amendment should update the page: {outcome:?}"
        );
        let page = db.get_page("page_long_tail").await.unwrap().unwrap();
        assert!(page.content.contains(amended_fact));
        assert!(
            page.citations.iter().any(|citation| {
                citation.locator == "folder-docs::cedar-atlas.md" && citation.status == "verified"
            }),
            "the tail claim's citation must be verified and persisted against the full source"
        );
    }

    /// Answers the long-source notes step from each part's own text and the
    /// final page write with a fixed body, recording every call by label.
    struct PartsReadingProvider {
        fact: String,
        page_body: String,
        input_limit: usize,
        calls: Mutex<Vec<(String, String)>>,
    }

    impl PartsReadingProvider {
        fn new(fact: &str, page_body: impl Into<String>, input_limit: usize) -> Self {
            Self {
                fact: fact.to_string(),
                page_body: page_body.into(),
                input_limit,
                calls: Mutex::new(Vec::new()),
            }
        }

        fn calls_labelled(&self, label: &str) -> Vec<String> {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|(l, _)| l == label)
                .map(|(_, prompt)| prompt.clone())
                .collect()
        }
    }

    #[async_trait::async_trait]
    impl LlmProvider for PartsReadingProvider {
        async fn generate(&self, request: LlmRequest) -> Result<String, LlmError> {
            let label = request.label.clone().unwrap_or_default();
            let estimated = request
                .system_prompt
                .as_deref()
                .unwrap_or_default()
                .len()
                .saturating_add(request.user_prompt.len())
                .div_ceil(3);
            assert!(
                estimated <= self.input_limit,
                "{label} request of ~{estimated} tokens exceeds the {} token budget",
                self.input_limit
            );
            let answer = if label == "refresh_source_notes" {
                if request.user_prompt.contains(&self.fact) {
                    self.fact.clone()
                } else {
                    "(nothing relevant)".to_string()
                }
            } else {
                self.page_body.clone()
            };
            self.calls
                .lock()
                .unwrap()
                .push((label, request.user_prompt));
            Ok(answer)
        }

        fn is_available(&self) -> bool {
            true
        }

        fn name(&self) -> &str {
            "parts-reading"
        }

        fn backend(&self) -> LlmBackend {
            LlmBackend::OnDevice
        }

        fn kind(&self) -> &'static str {
            "mock"
        }

        fn synthesis_token_limit(&self) -> usize {
            self.input_limit
        }
    }

    async fn insert_long_folder_source_page(
        db: &MemoryDB,
        page_id: &str,
        source_id: &str,
        content: String,
    ) {
        db.upsert_documents(vec![RawDocument {
            source: "memory".to_string(),
            source_id: source_id.to_string(),
            title: "Cedar Atlas project notes".to_string(),
            content,
            last_modified: chrono::Utc::now().timestamp(),
            memory_type: Some("fact".to_string()),
            space: Some("test".to_string()),
            source_agent: Some("folder".to_string()),
            confirmed: Some(true),
            ..Default::default()
        }])
        .await
        .unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        db.insert_page(
            page_id,
            "Cedar Atlas migration review",
            None,
            "The earlier review is still current.",
            None,
            None,
            &[source_id],
            &now,
        )
        .await
        .unwrap();
        db.set_page_stale(page_id, "source_updated").await.unwrap();
    }

    #[tokio::test]
    async fn refresh_page_reads_a_source_over_budget_in_parts_and_cites_it_in_full() {
        let (db, _db_dir) = crate::db::tests::test_db().await;
        let amended_fact =
            "The latest migration review on October 22 confirms Cedar Atlas remains in Seattle.";
        let background = "Background notes retain the project setup record.\n".repeat(240);
        let source = format!("{background}{amended_fact}\n");
        insert_long_folder_source_page(
            &db,
            "page_parts",
            "folder-docs::cedar-parts.md",
            source.clone(),
        )
        .await;

        let provider = Arc::new(PartsReadingProvider::new(
            amended_fact,
            format!("{amended_fact} [1]"),
            1_500,
        ));
        assert!(
            estimate_prompt_tokens(&PromptRegistry::default().distill_page, &source) > 1_500,
            "the complete source must not fit the budget"
        );
        let llm: Arc<dyn LlmProvider> = provider.clone();
        let outcome = refresh_page(
            &db,
            &llm,
            &PromptRegistry::default(),
            "page_parts",
            RefreshReason::SourceChanged,
            None,
        )
        .await
        .unwrap();

        let notes_calls = provider.calls_labelled("refresh_source_notes");
        assert!(
            notes_calls.len() > 1,
            "the long source is read in several parts"
        );
        assert!(
            notes_calls
                .iter()
                .any(|prompt| prompt.contains(amended_fact)),
            "the part holding the amendment is read"
        );
        let page_calls = provider.calls_labelled("refresh_page");
        assert_eq!(page_calls.len(), 1);
        assert!(
            page_calls[0].contains(amended_fact),
            "the writer sees the noted amendment"
        );
        assert!(
            !page_calls[0].contains(&background),
            "the writer gets notes, not the complete long source"
        );
        assert!(
            outcome.wrote,
            "the noted amendment should update the page: {outcome:?}"
        );
        let page = db.get_page("page_parts").await.unwrap().unwrap();
        assert!(page.content.contains(amended_fact));
        assert_eq!(page.refresh_blocked_reason, None);
        assert!(
            page.citations.iter().any(|citation| {
                citation.locator == "folder-docs::cedar-parts.md" && citation.status == "verified"
            }),
            "the citation is still checked against the complete source"
        );
    }

    #[tokio::test]
    async fn refresh_page_carries_an_amendment_verbatim_when_notes_drop_its_wording() {
        // Mac e2e with Qwen3-4B (2026-10-23): the notes for the last part kept
        // "stays in Seattle" but dropped "cancelled" and the date, so the
        // writer kept the earlier "Portland is decided".
        let (db, _db_dir) = crate::db::tests::test_db().await;
        let decided = "The office move to Portland is decided.";
        let amendment = "Amendment, October 22: the Portland move is cancelled; Cedar Atlas stays in Seattle, with a budget cap of $48,000.";
        let background = "Background notes retain the project setup record.\n".repeat(240);
        let source = format!("{decided}\n{background}{amendment}\n");
        insert_long_folder_source_page(&db, "page_amend", "folder-docs::cedar-amend.md", source)
            .await;

        let provider = Arc::new(PartsReadingProvider::new(
            "Cedar Atlas stays in Seattle",
            "Cedar Atlas stays in Seattle. [1]",
            1_500,
        ));
        let llm: Arc<dyn LlmProvider> = provider.clone();
        refresh_page(
            &db,
            &llm,
            &PromptRegistry::default(),
            "page_amend",
            RefreshReason::SourceChanged,
            None,
        )
        .await
        .unwrap();

        let page_calls = provider.calls_labelled("refresh_page");
        assert_eq!(page_calls.len(), 1);
        assert!(
            page_calls[0].contains(amendment),
            "the writer must see the amendment as written: {}",
            page_calls[0]
        );
    }

    #[test]
    fn amendment_lines_keep_changing_sentences_verbatim() {
        let part = "Setup is done. The budget is $48.5k for v0.20.\n\
                    ## Amendment, October 22\n\
                    The Portland move is cancelled. Weekly sync stays on Monday.\n\
                    十月二十二日更正：搬遷取消。其他照舊。";
        assert_eq!(
            amendment_lines(part),
            vec![
                "## Amendment, October 22",
                "The Portland move is cancelled.",
                "十月二十二日更正：搬遷取消。",
            ]
        );
        assert!(amendment_lines("Nothing changes here. Budget $48.5k.").is_empty());
    }

    #[tokio::test]
    async fn refresh_page_blocks_a_source_needing_too_many_parts_without_writing() {
        let (db, _db_dir) = crate::db::tests::test_db().await;
        let source = "Background notes retain the project setup record.\n".repeat(2_000);
        insert_long_folder_source_page(&db, "page_too_long", "folder-docs::cedar-long.md", source)
            .await;
        let provider = Arc::new(PartsReadingProvider::new(
            "never present",
            "unexpected generation [1]",
            1_500,
        ));
        let llm: Arc<dyn LlmProvider> = provider.clone();
        let before = db.get_page("page_too_long").await.unwrap().unwrap();
        let outcome = refresh_page(
            &db,
            &llm,
            &PromptRegistry::default(),
            "page_too_long",
            RefreshReason::SourceChanged,
            None,
        )
        .await
        .unwrap();

        assert!(!outcome.wrote);
        assert!(
            provider.calls.lock().unwrap().is_empty(),
            "no model time is spent on a source past the part cap"
        );
        let reason = outcome
            .discard_reason
            .expect("the block reason is reported");
        assert!(reason.contains("too long to read in parts"), "{reason}");
        let after = db.get_page("page_too_long").await.unwrap().unwrap();
        assert_eq!(after.content, before.content);
        assert_eq!(after.version, before.version);
        assert_eq!(
            after.refresh_blocked_reason.as_deref(),
            Some(reason.as_str())
        );
    }

    #[test]
    fn split_source_parts_prefers_line_ends_and_keeps_characters_whole() {
        let text = "第一行說明。\n".repeat(50) + &"長".repeat(400);
        let parts = split_source_parts(&text, 200);
        assert!(parts.len() > 1);
        assert_eq!(parts.concat(), text, "parts rejoin to the exact source");
        for part in &parts {
            assert!(
                part.len() <= 200,
                "part of {} bytes exceeds the cap",
                part.len()
            );
        }
        assert!(
            parts[0].ends_with('\n'),
            "a part ends at a line end when one is near"
        );
        assert_eq!(split_source_parts("short", 200), vec!["short"]);
    }

    #[tokio::test]
    async fn refresh_page_blocks_source_context_over_model_budget_without_calling_provider() {
        let (db, _db_dir) = crate::db::tests::test_db().await;
        let source = format!(
            "{}The amended fact is Cedar Atlas in Seattle.",
            "Project history. ".repeat(120)
        );
        upsert_test_sources(&db, &[("mem_over_budget", &source)], Some("test")).await;

        let now = chrono::Utc::now().to_rfc3339();
        db.insert_page(
            "page_over_budget",
            "Cedar Atlas",
            None,
            "Saved page body must remain intact.",
            None,
            None,
            &["mem_over_budget"],
            &now,
        )
        .await
        .unwrap();
        db.set_page_stale("page_over_budget", "source_updated")
            .await
            .unwrap();

        let provider = Arc::new(BoundedRecordingProvider::new(
            "unexpected generation [1]",
            32,
        ));
        let llm: Arc<dyn LlmProvider> = provider.clone();
        let prompts = PromptRegistry::default();
        let before = db.get_page("page_over_budget").await.unwrap().unwrap();
        let outcome = refresh_page(
            &db,
            &llm,
            &prompts,
            "page_over_budget",
            RefreshReason::SourceChanged,
            None,
        )
        .await
        .unwrap();

        assert!(
            !outcome.wrote,
            "an over-budget candidate must not overwrite the page"
        );
        assert!(
            provider.calls().is_empty(),
            "provider must not be called over its input budget"
        );
        let reason = outcome
            .discard_reason
            .expect("the caller should receive the budget block reason");
        assert!(reason.contains("source context exceeds model input budget"));

        let after = db.get_page("page_over_budget").await.unwrap().unwrap();
        assert_eq!(after.content, before.content);
        assert_eq!(after.version, before.version);
        assert_eq!(after.stale_reason.as_deref(), Some("source_updated"));
        assert_eq!(
            after.refresh_blocked_reason.as_deref(),
            Some(reason.as_str())
        );
    }

    #[tokio::test]
    async fn refresh_page_persists_provider_context_rejection_without_overwrite() {
        let (db, _db_dir) = crate::db::tests::test_db().await;
        upsert_test_sources(
            &db,
            &[("mem_physical_limit", "Cedar Atlas is based in Seattle.")],
            Some("test"),
        )
        .await;
        let now = chrono::Utc::now().to_rfc3339();
        db.insert_page(
            "page_physical_limit",
            "Cedar Atlas",
            None,
            "This saved body must survive provider rejection.",
            None,
            None,
            &["mem_physical_limit"],
            &now,
        )
        .await
        .unwrap();
        db.set_page_stale("page_physical_limit", "source_updated")
            .await
            .unwrap();

        let provider = Arc::new(BoundedRecordingProvider::rejecting_physical_context());
        let llm: Arc<dyn LlmProvider> = provider.clone();
        let prompts = PromptRegistry::default();
        let before = db.get_page("page_physical_limit").await.unwrap().unwrap();
        let outcome = refresh_page(
            &db,
            &llm,
            &prompts,
            "page_physical_limit",
            RefreshReason::SourceChanged,
            None,
        )
        .await
        .unwrap();

        assert!(
            !outcome.wrote,
            "exact provider rejection must not overwrite the page"
        );
        assert_eq!(
            provider.calls().len(),
            1,
            "the final tokenizer guard was reached"
        );
        let reason = outcome
            .discard_reason
            .expect("provider context rejection should be visible as a discard");
        assert!(reason.contains("source context exceeds model input budget"));
        assert!(reason.contains("physical input window"));

        let after = db.get_page("page_physical_limit").await.unwrap().unwrap();
        assert_eq!(after.content, before.content);
        assert_eq!(after.version, before.version);
        assert_eq!(after.stale_reason.as_deref(), Some("source_updated"));
        assert_eq!(
            after.refresh_blocked_reason.as_deref(),
            Some(reason.as_str())
        );
    }

    #[tokio::test]
    async fn refresh_page_on_human_owned_page_stages_card_without_overwrite() {
        let (db, _db_dir) = crate::db::tests::test_db().await;
        let now = chrono::Utc::now().to_rfc3339();
        let now_ts = chrono::Utc::now().timestamp();

        {
            let conn = db.test_primary_session().await;
            conn.execute(
                "INSERT INTO memories (id, source_id, title, content, chunk_index, chunk_type, memory_type, space, source_agent, created_at, last_modified, confirmed, stability, source) \
                 VALUES (?1, ?1, ?1, 'Tokio is an async runtime for Rust programs', 0, 'text', 'fact', 'test', 'claude-code', ?2, ?2, 1, 'confirmed', 'memory')",
                libsql::params!["mem_seed".to_string(), now_ts],
            )
            .await
            .unwrap();
        }
        db.insert_page(
            "page_h",
            "Tokio",
            None,
            "human-authored body",
            None,
            None,
            &["mem_seed"],
            &now,
        )
        .await
        .unwrap();
        db.set_page_stale("page_h", "source_updated").await.unwrap();
        // Mark the page human-owned so a machine write is gated to a card.
        {
            let conn = db.test_primary_session().await;
            conn.execute(
                "UPDATE pages SET user_edited = 1 WHERE id = ?1",
                libsql::params!["page_h".to_string()],
            )
            .await
            .unwrap();
        }

        let llm: Arc<dyn LlmProvider> = Arc::new(MockProvider::new(
            "Tokio is an async runtime for Rust programs [1]",
        ));
        let prompts = PromptRegistry::default();

        let before = db.get_page("page_h").await.unwrap().unwrap();
        let outcome = refresh_page(
            &db,
            &llm,
            &prompts,
            "page_h",
            RefreshReason::SourceChanged,
            None,
        )
        .await
        .unwrap();
        assert!(
            !outcome.wrote,
            "human-owned page must not be overwritten in place"
        );
        assert!(outcome.gated, "a machine write to a human page is gated");
        let card_id = outcome
            .revision_card_id
            .expect("a revision card id is returned");

        let after = db.get_page("page_h").await.unwrap().unwrap();
        assert_eq!(after.content, before.content, "human prose left intact");
        assert_eq!(
            after.version, before.version,
            "no version bump on a gated write"
        );

        // The staged card is a pending revision that supersedes the page.
        let conn = db.test_primary_session().await;
        let mut rows = conn
            .query(
                "SELECT supersedes FROM memories WHERE source_id = ?1 AND pending_revision = 1",
                libsql::params![card_id.clone()],
            )
            .await
            .unwrap();
        let row = rows
            .next()
            .await
            .unwrap()
            .expect("the staged revision card row exists");
        let supersedes: String = row.get(0).unwrap();
        assert_eq!(supersedes, "page_h", "card supersedes the target page");
    }

    #[tokio::test]
    async fn refresh_page_discards_unfaithful_synthesis_leaving_page_unchanged() {
        let (db, _db_dir) = crate::db::tests::test_db().await;
        let now = chrono::Utc::now().to_rfc3339();
        let now_ts = chrono::Utc::now().timestamp();

        {
            let conn = db.test_primary_session().await;
            conn.execute(
                "INSERT INTO memories (id, source_id, title, content, chunk_index, chunk_type, memory_type, space, source_agent, created_at, last_modified, confirmed, stability, source) \
                 VALUES (?1, ?1, ?1, 'Tokio is an async runtime for Rust programs', 0, 'text', 'fact', 'test', 'claude-code', ?2, ?2, 1, 'confirmed', 'memory')",
                libsql::params!["mem_seed".to_string(), now_ts],
            )
            .await
            .unwrap();
        }
        db.insert_page(
            "page_c",
            "Tokio",
            None,
            "original body",
            None,
            None,
            &["mem_seed"],
            &now,
        )
        .await
        .unwrap();
        db.set_page_stale("page_c", "source_updated").await.unwrap();

        // A wholesale-hallucinated body sharing no content tokens with the source.
        let llm: Arc<dyn LlmProvider> = Arc::new(MockProvider::new(
            "Penguins migrate across Antarctic ice shelves during polar winter months [1]",
        ));
        let prompts = PromptRegistry::default();

        let before = db.get_page("page_c").await.unwrap().unwrap();
        let outcome = refresh_page(
            &db,
            &llm,
            &prompts,
            "page_c",
            RefreshReason::SourceChanged,
            None,
        )
        .await
        .unwrap();
        assert!(!outcome.wrote, "unfaithful synthesis must be discarded");
        assert!(!outcome.gated);
        let discard_reason = outcome
            .discard_reason
            .expect("gate discard must carry a reason");
        assert!(
            discard_reason.starts_with("citation verification failed ("),
            "reason names the gate with its stats, got: {discard_reason}"
        );

        let after = db.get_page("page_c").await.unwrap().unwrap();
        assert_eq!(
            after.content, before.content,
            "page content unchanged after fail-closed discard"
        );
        assert_eq!(
            after.version, before.version,
            "no version bump on a discarded synthesis"
        );
        assert_eq!(
            after.refresh_blocked_reason.as_deref(),
            Some(discard_reason.as_str()),
            "discard reason persisted on the page row"
        );
        assert_eq!(
            after.stale_reason.as_deref(),
            Some("source_updated"),
            "the page stays stale — only the retry loop pauses"
        );

        // A later successful write through the canonical update_page path
        // clears the marker, so it can never describe a page that has since
        // compiled.
        let faithful: Arc<dyn LlmProvider> = Arc::new(MockProvider::new(
            "Tokio is an async runtime for Rust programs [1]",
        ));
        let outcome = refresh_page(
            &db,
            &faithful,
            &prompts,
            "page_c",
            RefreshReason::SourceChanged,
            None,
        )
        .await
        .unwrap();
        assert!(outcome.wrote, "faithful synthesis must land");
        assert_eq!(outcome.discard_reason, None);
        let recompiled = db.get_page("page_c").await.unwrap().unwrap();
        assert_eq!(
            recompiled.refresh_blocked_reason, None,
            "successful write clears the blocked-reason marker"
        );
        assert_eq!(recompiled.stale_reason, None);
    }

    #[tokio::test]
    async fn pending_distill_clusters_cap_one_document_below_majority() {
        let (db, _db_dir) = crate::db::tests::test_db().await;
        let now = chrono::Utc::now().timestamp_millis();
        let topic = "Wenlan folder ingest distillation cluster cap";

        for i in 0..4 {
            db.upsert_documents(vec![RawDocument {
                source: "memory".to_string(),
                source_id: format!("book_chunk_{i}"),
                title: format!("Book chunk {i}"),
                content: format!("{topic} repeated book excerpt section {i}"),
                last_modified: now + i,
                memory_type: Some("fact".to_string()),
                space: Some("work".to_string()),
                entity_id: Some("ent_folder_ingest".to_string()),
                content_hash: Some("book-content-hash".to_string()),
                ..Default::default()
            }])
            .await
            .unwrap();
        }

        for i in 0..3 {
            db.upsert_documents(vec![RawDocument {
                source: "memory".to_string(),
                source_id: format!("capture_note_{i}"),
                title: format!("Capture note {i}"),
                content: format!("{topic} supporting capture note {i}"),
                last_modified: now + 10 + i,
                memory_type: Some("fact".to_string()),
                space: Some("work".to_string()),
                entity_id: Some("ent_folder_ingest".to_string()),
                content_hash: None,
                ..Default::default()
            }])
            .await
            .unwrap();
        }

        let prompts = PromptRegistry::default();
        let tuning = crate::tuning::DistillationConfig {
            similarity_threshold: 0.2,
            page_min_cluster_size: 3,
            max_grouped_cluster_size: 20,
            max_unlinked_cluster_size: 20,
            ..Default::default()
        };

        let result = distill_pages_scoped(&db, None, &prompts, &tuning, None, None)
            .await
            .unwrap();
        let cluster = result
            .pending
            .iter()
            .find(|cluster| {
                cluster
                    .source_ids
                    .iter()
                    .any(|id| id.starts_with("book_chunk_"))
            })
            .expect("expected one pending cluster containing book chunks");

        let book_count = cluster
            .source_ids
            .iter()
            .filter(|id| id.starts_with("book_chunk_"))
            .count();
        assert!(
            book_count * 2 < cluster.source_ids.len(),
            "one document must be capped below half of the assembled cluster: book_count={book_count}, cluster={:?}",
            cluster.source_ids
        );
        assert!(
            cluster.source_ids.len() >= tuning.page_min_cluster_size,
            "cluster should still form when enough non-document members remain: {:?}",
            cluster.source_ids
        );
    }

    #[tokio::test]
    async fn formation_sweep_reports_global_and_per_space_grid_without_writes() {
        let (db, _db_dir) = crate::db::tests::test_db().await;
        let topic = "formation sweep dry run reports scoped cluster stats";

        upsert_test_sources(
            &db,
            &[
                ("sweep_work_1", topic),
                ("sweep_work_2", topic),
                ("sweep_work_3", topic),
            ],
            Some("work"),
        )
        .await;
        upsert_test_sources(
            &db,
            &[
                ("sweep_personal_1", topic),
                ("sweep_personal_2", topic),
                ("sweep_personal_3", topic),
            ],
            Some("personal"),
        )
        .await;
        upsert_test_sources(
            &db,
            &[
                ("sweep_none_1", topic),
                ("sweep_none_2", topic),
                ("sweep_none_3", topic),
            ],
            None,
        )
        .await;

        let before = (
            db.count_active_pages().await.unwrap(),
            db.count().await.unwrap(),
            db.count_refinement_queue_rows().await.unwrap(),
        );
        let tuning = crate::tuning::DistillationConfig {
            formation_threshold: 0.42,
            page_min_cluster_size: 3,
            max_clusters_per_steep: 20,
            max_grouped_cluster_size: 20,
            max_unlinked_cluster_size: 20,
            ..Default::default()
        };

        let report = formation_sweep(&db, &tuning).await.unwrap();

        let after = (
            db.count_active_pages().await.unwrap(),
            db.count().await.unwrap(),
            db.count_refinement_queue_rows().await.unwrap(),
        );
        assert_eq!(after, before, "formation sweep must not write rows");

        let thresholds: Vec<f64> = report
            .thresholds
            .iter()
            .map(|point| point.formation_threshold)
            .collect();
        assert_eq!(thresholds, vec![0.55, 0.60, 0.65, 0.70]);

        for point in &report.thresholds {
            assert!(!point.capped, "fixture should not hit the cluster cap");
            assert_eq!(point.global.cluster_count, 3);
            assert_eq!(point.global.size_distribution.sizes, vec![3, 3, 3]);
            assert_eq!(point.global.size_distribution.min, 3);
            assert_eq!(point.global.size_distribution.median, 3.0);
            assert_eq!(point.global.size_distribution.max, 3);
            assert_eq!(point.global.overlapping_member_fraction, 0.0);

            for space in ["(none)", "personal", "work"] {
                let stats = point
                    .per_space
                    .get(space)
                    .unwrap_or_else(|| panic!("missing per-space stats for {space}"));
                assert_eq!(stats.cluster_count, 1);
                assert_eq!(stats.size_distribution.sizes, vec![3]);
                assert_eq!(stats.overlapping_member_fraction, 0.0);
            }
        }
    }

    #[tokio::test]
    async fn overlapping_cluster_attaches_to_source_less_seed_no_duplicate() {
        let (db, _dir) = crate::db::tests::test_db().await;
        let now = chrono::Utc::now().to_rfc3339();
        // 1. Source-less CONFIRMED distilled seed on topic "Tokio", in space "work".
        //    insert_page_with_kind embeds title+summary; make summary ~= the cluster
        //    centroid text so cosine >= 0.85. Long body just to be realistic.
        db.insert_page_with_kind(
            "seed_tokio",
            "Tokio async runtime",
            Some("Tokio asynchronous runtime Rust tasks scheduler reactor"),
            "Tokio is an asynchronous runtime for the Rust programming language providing a work-stealing scheduler and a reactor backed by the OS event queue.",
            None,
            Some("work"),
            &[],
            &now,
            "distilled",
            "confirmed",
            None, // workspace
            None, // citations
        ).await.unwrap();

        // 2. A cluster on the same topic with its own memory + a centroid embedding
        //    built from near-identical text (so it clears the 0.85 threshold) and
        //    content > 200 chars (so WITHOUT the attach, synth would fire + duplicate).
        let centroid = db
            .generate_embeddings(&[
                "Tokio asynchronous runtime Rust tasks scheduler reactor".to_string()
            ])
            .unwrap()
            .remove(0);
        let long_content = "Tokio is an asynchronous runtime for the Rust programming language. \
            It provides a multi-threaded work-stealing scheduler, an async TCP and UDP socket API, \
            and a reactor backed by the operating system event queue for scalable network services."
            .to_string();
        let cluster = crate::db::DistillationCluster {
            source_ids: vec!["mem_x".into()],
            contents: vec![long_content],
            entity_id: None,
            entity_name: Some("Tokio".into()),
            space: Some("work".into()),
            estimated_tokens: 80,
            centroid_embedding: Some(centroid),
        };

        // SANITY: the scoped matcher must actually find the seed (else the test
        // proves nothing — the attach never fires). If THIS fails, the seed
        // summary / centroid text aren't similar enough; make them more identical.
        {
            let probe = db
                .find_matching_page_scoped(
                    None,
                    cluster.centroid_embedding.as_deref().unwrap(),
                    0.85,
                    Some("work"),
                    false,
                )
                .await
                .unwrap();
            assert_eq!(
                probe.as_ref().map(|p| p.id.as_str()),
                Some("seed_tokio"),
                "precondition: scoped matcher must find the seed at 0.85 (tune seed summary vs centroid text if this fails)"
            );
        }

        let pages_before = {
            let conn = db.test_primary_session().await;
            let mut r = conn
                .query("SELECT COUNT(*) FROM pages WHERE status='active'", ())
                .await
                .unwrap();
            let row = r.next().await.unwrap().unwrap();
            row.get::<i64>(0).unwrap()
        };

        // Faithful, Tokio-topical, guard-passing synth output: nearly a verbatim
        // subset of long_content (>0.6 cosine → clears the hallucination guard)
        // and starts with "- " (summary extraction works). WITHOUT the attach,
        // this would emit a REAL duplicate page — making the no-duplicate
        // assertions load-bearing, not masked by the guard rejecting junk text.
        let llm: Arc<dyn LlmProvider> = Arc::new(MockProvider::new(
            "- Tokio is an asynchronous runtime for the Rust programming language.\n\
             - It provides a multi-threaded work-stealing scheduler, an async TCP and UDP \
             socket API, and a reactor backed by the operating system event queue.",
        ));
        let prompts = PromptRegistry::default();
        let r = distill_one_cluster(&db, &llm, &prompts, &cluster, None)
            .await
            .unwrap();
        assert!(
            r.is_none(),
            "overlapping cluster must NOT emit a new page (attach, don't synth)"
        );

        let pages_after = {
            let conn = db.test_primary_session().await;
            let mut r = conn
                .query("SELECT COUNT(*) FROM pages WHERE status='active'", ())
                .await
                .unwrap();
            let row = r.next().await.unwrap().unwrap();
            row.get::<i64>(0).unwrap()
        };
        assert_eq!(pages_before, pages_after, "no duplicate page created");

        // mem_x must now be evidence on the seed (consumed → not re-clustered).
        let ev: Vec<String> = db
            .get_page_evidence("seed_tokio")
            .await
            .unwrap()
            .into_iter()
            .filter_map(|e| e.locator)
            .collect();
        assert!(
            ev.contains(&"mem_x".to_string()),
            "cluster source attached to seed evidence"
        );
        let activity = db
            .list_agent_activity(20, Some("distill"), None)
            .await
            .unwrap();
        assert!(
            activity.iter().any(|entry| entry.action == "page_attach"
                && entry.memory_ids.as_deref() == Some("seed_tokio")),
            "daemon attach must route through PageWrite and log page_attach, got {activity:?}"
        );
    }

    #[tokio::test]
    async fn distill_one_cluster_uses_configured_page_match_threshold() {
        let (db, _dir) = crate::db::tests::test_db().await;
        let now = chrono::Utc::now().to_rfc3339();

        db.insert_page_with_kind(
            "seed_tokio_high_threshold",
            "Tokio async runtime",
            Some("Tokio asynchronous runtime Rust tasks scheduler reactor"),
            "Tokio is an asynchronous runtime for Rust with a scheduler and reactor.",
            None,
            Some("work"),
            &[],
            &now,
            "distilled",
            "confirmed",
            None,
            None,
        )
        .await
        .unwrap();

        let centroid = db
            .generate_embeddings(&[
                "Tokio asynchronous runtime Rust tasks scheduler reactor".to_string()
            ])
            .unwrap()
            .remove(0);
        let long_content = "Tokio is an asynchronous runtime for the Rust programming language. \
            It provides a multi-threaded work-stealing scheduler, an async TCP and UDP socket API, \
            and a reactor backed by the operating system event queue for scalable network services."
            .to_string();
        upsert_test_sources(&db, &[("mem_high_threshold", &long_content)], Some("work")).await;
        let cluster = crate::db::DistillationCluster {
            source_ids: vec!["mem_high_threshold".into()],
            contents: vec![long_content],
            entity_id: None,
            entity_name: Some("Tokio".into()),
            space: Some("work".into()),
            estimated_tokens: 80,
            centroid_embedding: Some(centroid),
        };

        let tuning = crate::tuning::DistillationConfig {
            page_match_threshold: 1.1,
            page_min_cluster_size: 1,
            ..Default::default()
        };
        let llm: Arc<dyn LlmProvider> = Arc::new(MockProvider::new(
            "- Tokio is an asynchronous runtime for the Rust programming language.\n\
             - It provides a multi-threaded work-stealing scheduler, an async TCP and UDP \
             socket API, and a reactor backed by the operating system event queue.",
        ));
        let prompts = PromptRegistry::default();

        let r = distill_one_cluster_with_tuning(&db, &llm, &prompts, &cluster, &tuning, None)
            .await
            .unwrap();

        assert!(
            r.is_some(),
            "threshold above cosine max should prevent attach and allow synthesis"
        );
    }

    #[tokio::test]
    async fn distill_cluster_attaches_to_same_title_even_without_vector_match() {
        let (db, _dir) = crate::db::tests::test_db().await;
        let now = chrono::Utc::now().to_rfc3339();
        db.insert_page_with_kind(
            "seed_tokio_title",
            "Tokio Runtime Notes",
            None,
            "Tokio runtime notes already exist.",
            None,
            Some("work"),
            &[],
            &now,
            "distilled",
            "confirmed",
            Some("work"),
            None,
        )
        .await
        .unwrap();
        let source = "Tokio is an asynchronous runtime for Rust programs with a scheduler, reactor, and async network APIs. It coordinates tasks across worker threads, exposes timers and channels, and lets services process many connections without assigning one operating-system thread to every request.";
        upsert_test_sources(&db, &[("mem_title_guard", source)], Some("work")).await;
        let cluster = crate::db::DistillationCluster {
            source_ids: vec!["mem_title_guard".into()],
            contents: vec![source.into()],
            entity_id: None,
            entity_name: Some("Tokio".into()),
            space: Some("work".into()),
            estimated_tokens: 40,
            centroid_embedding: None,
        };
        let tuning = crate::tuning::DistillationConfig {
            page_match_threshold: 1.1,
            page_min_cluster_size: 1,
            ..Default::default()
        };
        let llm: Arc<dyn LlmProvider> = Arc::new(SequencedMockProvider::new(vec![
            "Tokio is an asynchronous runtime for Rust programs with a scheduler, reactor, and async network APIs. It coordinates tasks across worker threads, exposes timers and channels, and lets services process many connections without assigning one operating-system thread to every request [1].",
            "tokio runtime notes",
        ]));

        let result = distill_one_cluster_with_tuning(
            &db,
            &llm,
            &PromptRegistry::default(),
            &cluster,
            &tuning,
            None,
        )
        .await
        .unwrap();

        assert!(
            result.is_none(),
            "same-title cluster must attach, not create"
        );
        assert_eq!(db.list_pages("active", 10, 0).await.unwrap().len(), 1);
        let evidence: Vec<String> = db
            .get_page_evidence("seed_tokio_title")
            .await
            .unwrap()
            .into_iter()
            .filter_map(|row| row.locator)
            .collect();
        assert!(evidence.contains(&"mem_title_guard".to_string()));
    }

    const FORCED_SOURCE_TEXT: &str = "Tokio is an async runtime for Rust programs";
    const FORCED_REBUILT_BODY: &str = "Tokio is an async runtime for Rust programs [1]";
    const FORCED_USER_PROSE: &str = "My own notes about the Tokio runtime.";

    /// A distilled page with a real source memory, hand edited (`user_edited`)
    /// and stale, the state a user's Re-distill starts from.
    async fn seed_protected_page(db: &crate::db::MemoryDB, page_id: &str) {
        let now = chrono::Utc::now().to_rfc3339();
        let now_ts = chrono::Utc::now().timestamp();
        {
            let conn = db.test_primary_session().await;
            conn.execute(
                "INSERT INTO memories (id, source_id, title, content, chunk_index, chunk_type, memory_type, space, source_agent, created_at, last_modified, confirmed, stability, source) \
                 VALUES (?1, ?1, ?1, ?2, 0, 'text', 'fact', 'test', 'claude-code', ?3, ?3, 1, 'confirmed', 'memory')",
                libsql::params!["mem_forced_seed".to_string(), FORCED_SOURCE_TEXT, now_ts],
            )
            .await
            .unwrap();
        }
        db.insert_page(
            page_id,
            "Tokio",
            None,
            FORCED_SOURCE_TEXT,
            None,
            None,
            &["mem_forced_seed"],
            &now,
        )
        .await
        .unwrap();
        db.update_page_content(page_id, FORCED_USER_PROSE, &["mem_forced_seed"], "fs_edit")
            .await
            .unwrap();
        db.set_page_stale(page_id, "source_updated").await.unwrap();
        let page = db.get_page(page_id).await.unwrap().unwrap();
        assert!(page.user_edited, "precondition: the page is hand edited");
        assert_eq!(page.stale_reason.as_deref(), Some("source_updated"));
    }

    enum MidGeneration {
        Observe,
        HumanEdit,
        /// Return a body the citation gate discards.
        Discard,
        /// Land a human edit, then return a body the citation gate discards.
        HumanEditThenDiscard,
        /// Delete the page and recreate it under the same id at the same
        /// version and source revision (a new incarnation), then return the
        /// faithful rebuilt body.
        RecreateThenRebuild {
            replacement_body: &'static str,
            creation_kind: &'static str,
        },
        /// Recreate the page as above, then return a body the citation gate
        /// discards.
        RecreateThenDiscard,
    }

    const REPLACEMENT_MARKER: &str = "the replacement page's own refresh marker";

    /// Delete `page_id` and recreate it under the same id with the same
    /// `version` and `source_revision` the refresh captured, so only the row
    /// incarnation tells the two apart. The replacement is hand edited, stale,
    /// and carries its own refresh marker, which a stale terminal write would
    /// overwrite or clear.
    async fn recreate_page_under_same_id(
        db: &crate::db::MemoryDB,
        page_id: &str,
        replacement_body: &str,
        creation_kind: &str,
    ) {
        let before = db.get_page(page_id).await.unwrap().unwrap();
        let fence_before = db.try_get_page_fence(page_id).await.unwrap().unwrap();
        db.delete_page(page_id).await.unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        db.insert_page(
            page_id,
            "Tokio",
            None,
            replacement_body,
            None,
            None,
            &["mem_forced_seed"],
            &now,
        )
        .await
        .unwrap();
        {
            let conn = db.test_primary_session().await;
            conn.execute(
                "UPDATE pages SET version = ?1, source_revision = ?2, creation_kind = ?3, \
                   user_edited = 1, stale_reason = 'source_updated', refresh_blocked_reason = ?4 \
                 WHERE id = ?5",
                libsql::params![
                    before.version,
                    fence_before.source_revision,
                    creation_kind.to_string(),
                    REPLACEMENT_MARKER,
                    page_id.to_string()
                ],
            )
            .await
            .unwrap();
        }
        let replacement = db.get_page(page_id).await.unwrap().unwrap();
        let fence_after = db.try_get_page_fence(page_id).await.unwrap().unwrap();
        assert_eq!(
            replacement.version, before.version,
            "precondition: same version"
        );
        assert_eq!(
            fence_after.source_revision, fence_before.source_revision,
            "precondition: the recreated page restarts at the captured source revision"
        );
        assert_ne!(
            fence_after.incarnation, fence_before.incarnation,
            "precondition: only the incarnation tells the replacement apart"
        );
    }

    async fn pending_revision_card_ids(db: &crate::db::MemoryDB, page_id: &str) -> Vec<String> {
        let conn = db.test_primary_session().await;
        let mut rows = conn
            .query(
                "SELECT source_id FROM memories WHERE pending_revision = 1 AND supersedes = ?1",
                libsql::params![page_id.to_string()],
            )
            .await
            .unwrap();
        let mut ids = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            ids.push(row.get::<String>(0).unwrap());
        }
        ids
    }

    const FORCED_UNFAITHFUL_BODY: &str =
        "Penguins migrate across Antarctic ice shelves during polar winter months [1]";

    /// Records the page's `(user_edited, stale_reason)` while generation is in
    /// flight, optionally lands a human edit at that moment, then returns a
    /// body whose single claim verifies against the seeded source.
    struct ForcedRebuildProvider {
        db: Arc<crate::db::MemoryDB>,
        page_id: &'static str,
        mid_generation: MidGeneration,
        observed: tokio::sync::Mutex<Vec<(bool, Option<String>)>>,
    }

    #[async_trait::async_trait]
    impl LlmProvider for ForcedRebuildProvider {
        async fn generate(&self, _request: LlmRequest) -> Result<String, LlmError> {
            let page = self.db.get_page(self.page_id).await.unwrap().unwrap();
            self.observed
                .lock()
                .await
                .push((page.user_edited, page.stale_reason.clone()));
            // A retried generation must not recreate the page a second time.
            let first_call = self.observed.lock().await.len() == 1;
            if let MidGeneration::RecreateThenRebuild {
                replacement_body,
                creation_kind,
            } = self.mid_generation
            {
                if first_call {
                    recreate_page_under_same_id(
                        &self.db,
                        self.page_id,
                        replacement_body,
                        creation_kind,
                    )
                    .await;
                }
            }
            if let (MidGeneration::RecreateThenDiscard, true) = (&self.mid_generation, first_call) {
                recreate_page_under_same_id(
                    &self.db,
                    self.page_id,
                    "A replacement page recreated during generation.",
                    "distilled",
                )
                .await;
            }
            if let MidGeneration::HumanEdit | MidGeneration::HumanEditThenDiscard =
                self.mid_generation
            {
                self.db
                    .update_page_content(
                        self.page_id,
                        "A human edit that landed during generation.",
                        &["mem_forced_seed"],
                        "manual_edit",
                    )
                    .await
                    .unwrap();
            }
            Ok(match self.mid_generation {
                MidGeneration::Discard
                | MidGeneration::HumanEditThenDiscard
                | MidGeneration::RecreateThenDiscard => FORCED_UNFAITHFUL_BODY,
                MidGeneration::Observe
                | MidGeneration::HumanEdit
                | MidGeneration::RecreateThenRebuild { .. } => FORCED_REBUILT_BODY,
            }
            .to_string())
        }

        fn is_available(&self) -> bool {
            true
        }

        fn name(&self) -> &str {
            "forced-rebuild"
        }

        fn backend(&self) -> LlmBackend {
            LlmBackend::OnDevice
        }

        fn kind(&self) -> &'static str {
            "mock"
        }
    }

    fn forced_rebuild_provider(
        db: &Arc<crate::db::MemoryDB>,
        page_id: &'static str,
        mid_generation: MidGeneration,
    ) -> Arc<ForcedRebuildProvider> {
        Arc::new(ForcedRebuildProvider {
            db: Arc::clone(db),
            page_id,
            mid_generation,
            observed: tokio::sync::Mutex::new(Vec::new()),
        })
    }

    #[tokio::test]
    async fn force_redistill_lands_body_and_releases_edit_protection_in_one_write() {
        let (db, _db_dir) = crate::db::tests::test_db().await;
        let db = Arc::new(db);
        seed_protected_page(&db, "page_forced").await;
        let before = db.get_page("page_forced").await.unwrap().unwrap();
        let provider = forced_rebuild_provider(&db, "page_forced", MidGeneration::Observe);
        let llm: Arc<dyn LlmProvider> = provider.clone();

        let outcome = force_redistill_single(
            &db,
            Some(&llm),
            &PromptRegistry::default(),
            "page_forced",
            None,
        )
        .await
        .unwrap();

        assert!(outcome.wrote, "a verified forced rebuild lands in place");
        assert!(!outcome.gated, "user_edited alone must not stage a card");
        assert_eq!(
            *provider.observed.lock().await,
            vec![(true, Some("source_updated".to_string()))],
            "edit protection must hold while generation is in flight"
        );
        let after = db.get_page("page_forced").await.unwrap().unwrap();
        assert_eq!(after.content, FORCED_REBUILT_BODY);
        assert_eq!(after.version, before.version + 1, "exactly one write");
        assert!(!after.user_edited, "the landing write releases user_edited");
        assert_eq!(
            after.stale_reason, None,
            "the landing write consumes staleness"
        );
        assert_eq!(after.refresh_blocked_reason, None);
        let changelog: Vec<serde_json::Value> =
            serde_json::from_str(&db.get_page_changelog("page_forced").await.unwrap()).unwrap();
        let last_entry = changelog
            .last()
            .expect("the write appends a changelog entry");
        assert_eq!(last_entry["version"], after.version);
        assert_eq!(last_entry["edited_by"], "distill");
    }

    #[tokio::test]
    async fn force_redistill_identical_body_acknowledges_without_releasing_edit_protection() {
        let (db, _db_dir) = crate::db::tests::test_db().await;
        let db = Arc::new(db);
        seed_protected_page(&db, "page_forced_same").await;
        // The hand edit already says exactly what the rebuild will produce,
        // and an earlier automatic refresh of it was discarded.
        db.update_page_content(
            "page_forced_same",
            FORCED_REBUILT_BODY,
            &["mem_forced_seed"],
            "fs_edit",
        )
        .await
        .unwrap();
        db.set_page_stale("page_forced_same", "source_updated")
            .await
            .unwrap();
        db.set_page_refresh_blocked_reason("page_forced_same", "an earlier refresh was discarded")
            .await
            .unwrap();
        let before = db.get_page("page_forced_same").await.unwrap().unwrap();
        assert!(before.user_edited, "precondition: the page is hand edited");
        assert_eq!(before.content, FORCED_REBUILT_BODY);
        assert_eq!(before.stale_reason.as_deref(), Some("source_updated"));
        assert_eq!(
            before.refresh_blocked_reason.as_deref(),
            Some("an earlier refresh was discarded")
        );
        let provider = forced_rebuild_provider(&db, "page_forced_same", MidGeneration::Observe);
        let llm: Arc<dyn LlmProvider> = provider.clone();

        let outcome = force_redistill_single(
            &db,
            Some(&llm),
            &PromptRegistry::default(),
            "page_forced_same",
            None,
        )
        .await
        .unwrap();

        assert!(!outcome.wrote, "an identical body is not a write");
        assert!(!outcome.gated, "an identical body stages nothing");
        assert!(
            outcome.acknowledged,
            "the completed compile is acknowledged"
        );
        let after = db.get_page("page_forced_same").await.unwrap().unwrap();
        assert_eq!(after.content, FORCED_REBUILT_BODY);
        assert_eq!(after.version, before.version, "no version bump");
        assert!(
            after.user_edited,
            "nothing landed, so user_edited is not released"
        );
        assert_eq!(after.stale_reason, None, "the compile consumed staleness");
        assert_eq!(
            after.refresh_blocked_reason, None,
            "the acknowledged compile clears the blocked marker"
        );
    }

    #[tokio::test]
    async fn force_redistill_discard_after_a_mid_generation_edit_leaves_the_new_state_unblocked() {
        let (db, _db_dir) = crate::db::tests::test_db().await;
        let db = Arc::new(db);
        seed_protected_page(&db, "page_forced_discard_race").await;
        db.set_page_refresh_blocked_reason("page_forced_discard_race", "an earlier discard")
            .await
            .unwrap();
        let provider = forced_rebuild_provider(
            &db,
            "page_forced_discard_race",
            MidGeneration::HumanEditThenDiscard,
        );
        let llm: Arc<dyn LlmProvider> = provider.clone();

        let outcome = force_redistill_single(
            &db,
            Some(&llm),
            &PromptRegistry::default(),
            "page_forced_discard_race",
            None,
        )
        .await
        .unwrap();

        assert!(!outcome.wrote);
        assert!(!outcome.gated);
        assert_eq!(
            outcome.discard_reason, None,
            "an obsolete discard is not attributed to the edited page"
        );
        let after = db
            .get_page("page_forced_discard_race")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after.content, "A human edit that landed during generation.");
        assert!(after.user_edited, "the human edit keeps its protection");
        assert_eq!(
            after.refresh_blocked_reason, None,
            "the edit cleared the marker and the obsolete discard must not re-add it"
        );
    }

    #[tokio::test]
    async fn force_redistill_discard_on_an_unchanged_page_still_records_the_marker() {
        let (db, _db_dir) = crate::db::tests::test_db().await;
        let db = Arc::new(db);
        seed_protected_page(&db, "page_forced_discard").await;
        let before = db.get_page("page_forced_discard").await.unwrap().unwrap();
        let provider = forced_rebuild_provider(&db, "page_forced_discard", MidGeneration::Discard);
        let llm: Arc<dyn LlmProvider> = provider.clone();

        let outcome = force_redistill_single(
            &db,
            Some(&llm),
            &PromptRegistry::default(),
            "page_forced_discard",
            None,
        )
        .await
        .unwrap();

        assert!(!outcome.wrote);
        let discard_reason = outcome
            .discard_reason
            .expect("a discard of the unchanged page carries its reason");
        let after = db.get_page("page_forced_discard").await.unwrap().unwrap();
        assert_eq!(
            after.refresh_blocked_reason.as_deref(),
            Some(discard_reason.as_str())
        );
        assert_eq!(after.content, before.content);
        assert_eq!(after.version, before.version);
        assert!(after.user_edited, "a discard releases no edit protection");
        assert_eq!(after.stale_reason.as_deref(), Some("source_updated"));
    }

    /// Runs a forced rebuild whose provider deletes and recreates the page
    /// under the same id mid-generation, and asserts the replacement row is
    /// exactly as the recreation left it. Returns the outcome for
    /// path-specific assertions.
    async fn force_redistill_across_a_same_id_recreation(
        page_id: &'static str,
        mid_generation: MidGeneration,
        replacement_body: &str,
    ) -> RefreshOutcome {
        let (db, _db_dir) = crate::db::tests::test_db().await;
        let db = Arc::new(db);
        seed_protected_page(&db, page_id).await;
        let provider = forced_rebuild_provider(&db, page_id, mid_generation);
        let llm: Arc<dyn LlmProvider> = provider.clone();

        let outcome =
            force_redistill_single(&db, Some(&llm), &PromptRegistry::default(), page_id, None)
                .await
                .unwrap();
        let generations = provider.observed.lock().await.len();
        assert!(generations >= 1, "the provider ran and recreated the page");

        let after = db.get_page(page_id).await.unwrap().unwrap();
        assert!(!outcome.wrote, "nothing lands on the replacement page");
        assert!(!outcome.gated, "nothing is staged for the replacement page");
        assert!(!outcome.acknowledged, "no compile is acknowledged on it");
        assert_eq!(outcome.revision_card_id, None);
        assert_eq!(
            after.content, replacement_body,
            "the replacement body stays"
        );
        assert!(
            after.user_edited,
            "the replacement keeps its edit protection"
        );
        assert_eq!(
            after.stale_reason.as_deref(),
            Some("source_updated"),
            "the replacement's staleness is not consumed"
        );
        assert_eq!(
            after.refresh_blocked_reason.as_deref(),
            Some(REPLACEMENT_MARKER),
            "the replacement's own marker is neither cleared nor overwritten"
        );
        assert!(
            pending_revision_card_ids(&db, page_id).await.is_empty(),
            "no revision card targets the replacement page"
        );
        outcome
    }

    #[tokio::test]
    async fn force_redistill_verified_body_does_not_overwrite_a_page_recreated_under_the_same_id() {
        force_redistill_across_a_same_id_recreation(
            "page_forced_recreated_write",
            MidGeneration::RecreateThenRebuild {
                replacement_body: "A replacement page recreated during generation.",
                creation_kind: "distilled",
            },
            "A replacement page recreated during generation.",
        )
        .await;
    }

    #[tokio::test]
    async fn force_redistill_identical_body_does_not_acknowledge_a_page_recreated_under_the_same_id(
    ) {
        // The replacement already holds the exact rebuilt body and source set,
        // so the rebuild reaches the no-op acknowledgement, not the write.
        force_redistill_across_a_same_id_recreation(
            "page_forced_recreated_ack",
            MidGeneration::RecreateThenRebuild {
                replacement_body: FORCED_REBUILT_BODY,
                creation_kind: "distilled",
            },
            FORCED_REBUILT_BODY,
        )
        .await;
    }

    #[tokio::test]
    async fn force_redistill_discard_does_not_mark_a_page_recreated_under_the_same_id() {
        let outcome = force_redistill_across_a_same_id_recreation(
            "page_forced_recreated_discard",
            MidGeneration::RecreateThenDiscard,
            "A replacement page recreated during generation.",
        )
        .await;
        assert_eq!(
            outcome.discard_reason, None,
            "an obsolete discard is not attributed to the replacement page"
        );
    }

    #[tokio::test]
    async fn force_redistill_does_not_stage_a_card_on_an_authored_page_recreated_under_the_same_id()
    {
        // An authored replacement routes the rebuild to revision-card staging
        // and the staleness clear, both of which must miss the new row.
        force_redistill_across_a_same_id_recreation(
            "page_forced_recreated_authored",
            MidGeneration::RecreateThenRebuild {
                replacement_body: "An authored replacement recreated during generation.",
                creation_kind: "authored",
            },
            "An authored replacement recreated during generation.",
        )
        .await;
    }

    #[tokio::test]
    async fn force_redistill_yields_to_a_human_edit_that_lands_mid_generation() {
        let (db, _db_dir) = crate::db::tests::test_db().await;
        let db = Arc::new(db);
        seed_protected_page(&db, "page_forced_race").await;
        let provider = forced_rebuild_provider(&db, "page_forced_race", MidGeneration::HumanEdit);
        let llm: Arc<dyn LlmProvider> = provider.clone();

        let outcome = force_redistill_single(
            &db,
            Some(&llm),
            &PromptRegistry::default(),
            "page_forced_race",
            None,
        )
        .await
        .unwrap();

        assert!(
            !outcome.wrote,
            "the source-revision fence must refuse the stale rebuild"
        );
        assert!(!outcome.gated, "a fenced-out rebuild stages nothing");
        let after = db.get_page("page_forced_race").await.unwrap().unwrap();
        assert_eq!(after.content, "A human edit that landed during generation.");
        assert!(after.user_edited, "the human edit keeps its protection");
        assert_eq!(after.stale_reason.as_deref(), Some("source_updated"));
    }

    #[tokio::test]
    async fn force_redistill_refuses_a_draft_before_generation() {
        let (db, _db_dir) = crate::db::tests::test_db().await;
        let db = Arc::new(db);
        seed_protected_page(&db, "page_forced_draft").await;
        {
            let conn = db.test_primary_session().await;
            conn.execute(
                "UPDATE pages SET status = 'draft' WHERE id = ?1",
                libsql::params!["page_forced_draft"],
            )
            .await
            .unwrap();
        }
        let before = db.get_page("page_forced_draft").await.unwrap().unwrap();
        let provider = forced_rebuild_provider(&db, "page_forced_draft", MidGeneration::Observe);
        let llm: Arc<dyn LlmProvider> = provider.clone();

        let err = force_redistill_single(
            &db,
            Some(&llm),
            &PromptRegistry::default(),
            "page_forced_draft",
            None,
        )
        .await
        .unwrap_err();

        assert!(
            matches!(&err, WenlanError::Validation(msg) if msg == "Page page_forced_draft is not active"),
            "unexpected error: {err:?}"
        );
        assert!(
            provider.observed.lock().await.is_empty(),
            "no generation for a draft"
        );
        let after = db.get_page("page_forced_draft").await.unwrap().unwrap();
        assert_eq!(
            serde_json::to_value(&after).unwrap(),
            serde_json::to_value(&before).unwrap()
        );
    }

    #[tokio::test]
    async fn force_redistill_on_authored_page_stages_card_and_keeps_protection() {
        let (db, _db_dir) = crate::db::tests::test_db().await;
        let db = Arc::new(db);
        seed_protected_page(&db, "page_forced_authored").await;
        {
            let conn = db.test_primary_session().await;
            conn.execute(
                "UPDATE pages SET creation_kind = 'authored' WHERE id = ?1",
                libsql::params!["page_forced_authored"],
            )
            .await
            .unwrap();
        }
        let before = db.get_page("page_forced_authored").await.unwrap().unwrap();
        let provider = forced_rebuild_provider(&db, "page_forced_authored", MidGeneration::Observe);
        let llm: Arc<dyn LlmProvider> = provider.clone();

        let outcome = force_redistill_single(
            &db,
            Some(&llm),
            &PromptRegistry::default(),
            "page_forced_authored",
            None,
        )
        .await
        .unwrap();

        assert!(
            !outcome.wrote,
            "an authored page is never rewritten in place"
        );
        assert!(outcome.gated, "the forced rebuild lands as a revision card");
        assert!(outcome.revision_card_id.is_some());
        let after = db.get_page("page_forced_authored").await.unwrap().unwrap();
        assert_eq!(after.content, before.content);
        assert_eq!(after.version, before.version);
        assert!(
            after.user_edited,
            "a staged card must not release user_edited"
        );
    }

    #[tokio::test]
    async fn successful_gated_refresh_clears_old_block_on_fresh_page() {
        let (db, _db_dir) = crate::db::tests::test_db().await;
        let db = Arc::new(db);
        seed_protected_page(&db, "page_fresh_blocked_authored").await;
        {
            let conn = db.test_primary_session().await;
            conn.execute(
                "UPDATE pages SET creation_kind = 'authored', stale_reason = NULL, \
                    refresh_blocked_reason = 'earlier citation failure' WHERE id = ?1",
                libsql::params!["page_fresh_blocked_authored"],
            )
            .await
            .unwrap();
        }
        let before = db
            .get_page("page_fresh_blocked_authored")
            .await
            .unwrap()
            .unwrap();
        assert!(before.user_edited);
        assert_eq!(before.stale_reason, None);
        assert_eq!(
            before.refresh_blocked_reason.as_deref(),
            Some("earlier citation failure")
        );

        let provider =
            forced_rebuild_provider(&db, "page_fresh_blocked_authored", MidGeneration::Observe);
        let llm: Arc<dyn LlmProvider> = provider.clone();
        let outcome = force_redistill_single(
            &db,
            Some(&llm),
            &PromptRegistry::default(),
            "page_fresh_blocked_authored",
            None,
        )
        .await
        .unwrap();

        assert!(!outcome.wrote, "authored content remains gated");
        assert!(
            outcome.gated,
            "the successful synthesis stages a review card"
        );
        assert!(outcome.revision_card_id.is_some());
        let after = db
            .get_page("page_fresh_blocked_authored")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after.content, before.content);
        assert_eq!(after.version, before.version);
        assert!(after.user_edited);
        assert_eq!(after.stale_reason, None);
        assert_eq!(
            after.refresh_blocked_reason, None,
            "the staged successful proposal replaces the old error state"
        );
    }

    #[tokio::test]
    async fn successful_basis_preview_clears_old_block_on_fresh_page() {
        let (db, _db_dir) = crate::db::tests::test_db().await;
        let db = Arc::new(db);
        seed_protected_page(&db, "page_fresh_blocked_basis").await;
        let now = chrono::Utc::now().to_rfc3339();
        db.insert_page(
            "page_fresh_basis_note",
            "Tokio release note",
            None,
            "Tokio release context.",
            None,
            None,
            &[],
            &now,
        )
        .await
        .unwrap();
        let basis_page = db.get_page("page_fresh_basis_note").await.unwrap().unwrap();
        db.mark_page_basis(
            "page_fresh_blocked_basis",
            "page_fresh_basis_note",
            basis_page.version,
            "test",
        )
        .await
        .unwrap();
        {
            let conn = db.test_primary_session().await;
            conn.execute(
                "UPDATE pages SET stale_reason = NULL, refresh_blocked_reason = 'earlier citation failure' WHERE id = ?1",
                libsql::params!["page_fresh_blocked_basis"],
            )
            .await
            .unwrap();
        }
        let before = db
            .get_page("page_fresh_blocked_basis")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(before.stale_reason, None);
        assert_eq!(
            before.refresh_blocked_reason.as_deref(),
            Some("earlier citation failure")
        );
        assert_eq!(
            db.list_basis_context("page_fresh_blocked_basis")
                .await
                .unwrap()
                .len(),
            1,
            "precondition: this refresh exercises the basis-preview branch"
        );

        let provider =
            forced_rebuild_provider(&db, "page_fresh_blocked_basis", MidGeneration::Observe);
        let llm: Arc<dyn LlmProvider> = provider.clone();
        let outcome = force_redistill_single(
            &db,
            Some(&llm),
            &PromptRegistry::default(),
            "page_fresh_blocked_basis",
            None,
        )
        .await
        .unwrap();

        assert!(!outcome.wrote, "basis-backed content remains behind review");
        assert!(outcome.gated, "a successful basis refresh stages a preview");
        assert!(outcome.revision_card_id.is_some());
        let after = db
            .get_page("page_fresh_blocked_basis")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after.content, before.content);
        assert_eq!(after.version, before.version);
        assert_eq!(after.stale_reason, None);
        assert_eq!(
            after.refresh_blocked_reason, None,
            "the successful basis preview clears only the stale error marker"
        );
    }
}
