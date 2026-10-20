// SPDX-License-Identifier: Apache-2.0
//! Fact-channel (T15a): per-fact "child" vectors that rehydrate to their
//! PARENT memory.
//!
//! Each memory is exploded into per-fact child vectors (a narrative child =
//! the chunk content, plus one child per structured field). Children are
//! indexed in their own `child_vectors_vec_idx`. A child hit does NOT surface
//! a new id -- it REHYDRATES the parent memory (the parent's own `source_id`),
//! so retrieval metrics stay honest (source="memory", no provenance map).
//!
//! This module holds the pure, unit-testable helpers: the opt-in flag/limit
//! gates and the RRF-mass-only merge + max-pool-by-parent reducers. The DB
//! wiring (producer + `search_facts_channel`) lives in `db.rs` and mirrors the
//! merged page-channel architecture.

use std::collections::HashMap;

use crate::db::SearchResult;

/// Default fact-channel pool size (best-child parents pulled into the merge).
/// Mirrors `page_channel_limit`'s env-override shape so the eval harness can
/// sweep the value without a recompile.
const FACT_CHANNEL_LIMIT_DEFAULT: usize = 20;

/// True iff `WENLAN_ENABLE_FACT_CHANNEL` is set to a truthy value
/// (`1`, `true`, or `yes`, case-insensitive). The fact-channel is OPT-IN
/// (master write+read switch): unset or a falsey value (`0`/`false`/`no`/"")
/// leaves it disabled, so behaviour is byte-identical to pre-T15a.
///
/// The WRITE side (child-vector co-write in `upsert_documents`) and the READ
/// side (`search_facts_channel` inside `search_memory_cross_rerank`) both gate
/// on this single helper, and the eval harness reads it too -- so an
/// `WENLAN_ENABLE_FACT_CHANNEL` setting can't disagree between production and
/// eval (which would make baseline filenames lie). Truthy-only parse, mirrors
/// `page_channel_enabled`.
///
/// Design note: write-gating means a DB ingested with the flag OFF has an
/// EMPTY `child_vectors` table, so flipping the flag ON at read time alone is a
/// silent no-op. `MemoryDB::rebuild_child_vectors_for_missing` is the supported
/// turn-it-on-later backfill (mirrors how page-channel pages exist independent
/// of the read flag).
pub fn fact_channel_enabled() -> bool {
    std::env::var("WENLAN_ENABLE_FACT_CHANNEL")
        .ok()
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

/// Resolve the fact-channel pool size at call time. Env override
/// `WENLAN_FACT_CHANNEL_LIMIT` lets the eval harness sweep tuning values
/// without recompile (mirrors `page_channel_limit`). Default 20.
pub fn fact_channel_limit() -> usize {
    std::env::var("WENLAN_FACT_CHANNEL_LIMIT")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(FACT_CHANNEL_LIMIT_DEFAULT)
}

/// A child hit: the parent memory's `source_id` plus the child's rank in the
/// fact-channel result list (0-based, best first).
#[derive(Debug, Clone)]
pub struct ChildHit {
    /// Parent memory `source_id` (what rehydrates).
    pub parent_id: String,
    /// 0-based rank of this child in the raw fact-channel result list.
    pub rank: usize,
}

/// Max-pool child hits by parent: collapse multiple children of the same parent
/// down to the single BEST (lowest-rank) child, and return the parent ids in
/// best-rank order. This keeps a parent with many weakly-matching fields from
/// accumulating runaway RRF mass (it contributes ONLY its best child's mass,
/// not the sum).
///
/// Deterministic: ties on rank break by parent id so output is stable.
pub fn max_pool_by_parent(child_hits: &[ChildHit]) -> Vec<String> {
    // Best (lowest) rank seen per parent.
    let mut best: HashMap<String, usize> = HashMap::new();
    for h in child_hits {
        best.entry(h.parent_id.clone())
            .and_modify(|r| {
                if h.rank < *r {
                    *r = h.rank;
                }
            })
            .or_insert(h.rank);
    }
    let mut parents: Vec<(String, usize)> = best.into_iter().collect();
    parents.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    parents.into_iter().map(|(id, _)| id).collect()
}

/// Merge rehydrated fact-channel parents into the running merge maps as
/// RRF-mass ONLY (`1/(60+rank)`), exactly like the page/episode channels.
///
/// `parents` are the rehydrated parent `SearchResult`s in best-child-rank order
/// (max-pooled -- each parent appears at most once). A parent already present in
/// `result_map` (because the base memory channel also returned it) gains the
/// RRF mass on top of its existing score; a parent new to the pool is inserted
/// with mass only and its raw cosine zeroed so the scales never mix.
///
/// This NEVER inserts a raw cosine into `score_map`, guarding the
/// score-scale-mixing trap.
pub fn merge_fact_channel(
    score_map: &mut HashMap<String, f32>,
    result_map: &mut HashMap<String, SearchResult>,
    parents: Vec<SearchResult>,
) {
    for (rank, parent) in parents.into_iter().enumerate() {
        let rrf_score = 1.0 / (60.0 + rank as f32);
        let mut r = parent;
        // Zero the rehydrated raw cosine: it only contributes RRF mass, never a
        // cosine at an incompatible scale (mirrors the episode merge).
        r.score = 0.0;
        *score_map.entry(r.id.clone()).or_default() += rrf_score;
        result_map.entry(r.id.clone()).or_insert(r);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_result(id: &str, source_id: &str, score: f32) -> SearchResult {
        SearchResult {
            basis: Vec::new(),
            code_basis: Vec::new(),
            id: id.to_string(),
            content: format!("content for {id}"),
            source: "memory".to_string(),
            source_id: source_id.to_string(),
            title: String::new(),
            url: None,
            chunk_index: 0,
            last_modified: 0,
            score,
            chunk_type: None,
            language: None,
            semantic_unit: None,
            memory_type: None,
            space: None,
            source_agent: None,
            confidence: None,
            confirmed: None,
            stability: None,
            supersedes: None,
            summary: None,
            entity_id: None,
            entity_name: None,
            quality: None,
            importance: None,
            event_date: None,
            is_recap: false,
            is_archived: false,
            structured_fields: None,
            retrieval_cue: None,
            source_text: None,
            content_hash: None,
            raw_score: 0.0,
            version: 1,
            pending_revision: false,
            merged_from: None,
            last_delta_summary: None,
        }
    }

    #[test]
    fn fact_channel_enabled_treats_zero_and_unset_as_disabled() {
        for v in ["0", "false", "no", "", "  "] {
            temp_env::with_var("WENLAN_ENABLE_FACT_CHANNEL", Some(v), || {
                assert!(!fact_channel_enabled(), "value {v:?} must disable");
            });
        }
        temp_env::with_var("WENLAN_ENABLE_FACT_CHANNEL", None::<&str>, || {
            assert!(!fact_channel_enabled(), "unset must disable");
        });
    }

    #[test]
    fn fact_channel_enabled_accepts_truthy_synonyms() {
        for v in ["1", "true", "TRUE", "yes", "Yes", " 1 "] {
            temp_env::with_var("WENLAN_ENABLE_FACT_CHANNEL", Some(v), || {
                assert!(fact_channel_enabled(), "value {v:?} must enable");
            });
        }
    }

    #[test]
    fn fact_channel_limit_default_and_override() {
        temp_env::with_var("WENLAN_FACT_CHANNEL_LIMIT", None::<&str>, || {
            assert_eq!(fact_channel_limit(), 20, "default is 20");
        });
        temp_env::with_var("WENLAN_FACT_CHANNEL_LIMIT", Some("7"), || {
            assert_eq!(fact_channel_limit(), 7, "env override honoured");
        });
        temp_env::with_var("WENLAN_FACT_CHANNEL_LIMIT", Some("garbage"), || {
            assert_eq!(
                fact_channel_limit(),
                20,
                "unparseable falls back to default"
            );
        });
    }

    #[test]
    fn max_pool_by_parent_keeps_best_rank_and_dedups() {
        // P_a has children at ranks 0 and 3; P_b at ranks 1 and 2.
        let hits = vec![
            ChildHit {
                parent_id: "P_a".into(),
                rank: 0,
            },
            ChildHit {
                parent_id: "P_b".into(),
                rank: 1,
            },
            ChildHit {
                parent_id: "P_b".into(),
                rank: 2,
            },
            ChildHit {
                parent_id: "P_a".into(),
                rank: 3,
            },
        ];
        let parents = max_pool_by_parent(&hits);
        assert_eq!(
            parents,
            vec!["P_a".to_string(), "P_b".to_string()],
            "each parent once, ordered by best (lowest) child rank"
        );
    }

    #[test]
    fn max_pool_by_parent_ties_break_by_id() {
        let hits = vec![
            ChildHit {
                parent_id: "P_z".into(),
                rank: 0,
            },
            ChildHit {
                parent_id: "P_a".into(),
                rank: 0,
            },
        ];
        let parents = max_pool_by_parent(&hits);
        assert_eq!(parents, vec!["P_a".to_string(), "P_z".to_string()]);
    }

    #[test]
    fn merge_fact_channel_adds_rrf_mass_only() {
        // Seed a memory-channel parent P_a with a raw score; P_b is brand new.
        let mut score_map: HashMap<String, f32> = HashMap::new();
        let mut result_map: HashMap<String, SearchResult> = HashMap::new();
        let seeded = mk_result("id_a", "P_a", 0.42);
        score_map.insert("id_a".into(), 0.42);
        result_map.insert("id_a".into(), seeded);

        // Fact channel returns rehydrated parents [P_a (rank 0), P_b (rank 1)].
        let parents = vec![mk_result("id_a", "P_a", 0.9), mk_result("id_b", "P_b", 0.9)];
        merge_fact_channel(&mut score_map, &mut result_map, parents);

        // P_a: pre-existing 0.42 + 1/(60+0).
        let expected_a = 0.42 + 1.0 / 60.0;
        assert!(
            (score_map["id_a"] - expected_a).abs() < 1e-6,
            "P_a gains exactly 1/(60+0) on top of its base score, got {}",
            score_map["id_a"]
        );
        // P_b: new, mass only = 1/(60+1).
        let expected_b = 1.0 / 61.0;
        assert!(
            (score_map["id_b"] - expected_b).abs() < 1e-6,
            "P_b enters with mass only 1/(60+1), got {}",
            score_map["id_b"]
        );
        // The new parent's raw cosine (0.9) must NEVER leak into score_map.
        assert_eq!(
            result_map["id_b"].score, 0.0,
            "rehydrated raw cosine is zeroed; only RRF mass counts"
        );
    }

    #[test]
    fn merge_fact_channel_never_inserts_raw_cosine() {
        // Guard: even a single child entering an empty map carries only RRF mass.
        let mut score_map: HashMap<String, f32> = HashMap::new();
        let mut result_map: HashMap<String, SearchResult> = HashMap::new();
        let parents = vec![mk_result("id_x", "P_x", 0.99)];
        merge_fact_channel(&mut score_map, &mut result_map, parents);
        assert!(
            (score_map["id_x"] - 1.0 / 60.0).abs() < 1e-6,
            "score is RRF mass, never the 0.99 raw cosine"
        );
        assert_ne!(score_map["id_x"], 0.99, "raw cosine must not be the score");
    }
}
