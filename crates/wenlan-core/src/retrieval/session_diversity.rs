// SPDX-License-Identifier: Apache-2.0
//! Per-session result diversification cap (T20).
//!
//! Caps how many results from the same conversation session appear in the
//! top-`limit` output of `search_memory_cross_rerank`, preventing a single
//! LME session from monopolising the ranking on multi-session questions.
//!
//! **Production safety**: session keys are only derivable for eval source_ids
//! (`lme_*`).  Real daemon source_ids (`mem_<uuid>`) return `None` from
//! `session_key` and are never counted against the cap — the cap is a pure
//! no-op for production traffic until a real session column ships.
//!
//! **LoCoMo exempt**: `locomo_*` ids return `None`.  LoCoMo has one
//! conversation per sample; collapsing all its observations to one session key
//! would brutally suppress recall (recommendation (c) in the T20 spec).
//!
//! Enabled by `WENLAN_ENABLE_SESSION_DIVERSITY` (truthy: `1`/`true`/`yes`).
//! Cap size set by `WENLAN_SESSION_DIVERSITY_MAX` (default 3).

use std::collections::HashMap;

use wenlan_types::SearchResult;

/// Derive a session key from a `source_id`, or `None` if the id does not
/// encode a session structure we recognise.
///
/// ### Rules
/// * `lme_*` ids: strip the LAST `_t<digits>` suffix; return the prefix.
///   Back-parsing (right-to-left) avoids the front-split trap that silently
///   mis-keys ids with underscores in the question_id segment
///   (e.g. `lme_gpt4_2655b836_4_t12`).
/// * `locomo_*` ids: always return `None` (exempt — single conversation per
///   sample; keying would collapse the whole pool to one session).
/// * All other ids (e.g. `mem_<uuid>`): return `None` (production no-op).
///
/// Char-safe: uses `rsplit_once` / string slices at validated ASCII boundaries;
/// no raw byte indexing on non-ASCII input.
pub(crate) fn session_key(source_id: &str) -> Option<String> {
    // LoCoMo: exempt — single conversation per sample, no session structure.
    if source_id.starts_with("locomo_") {
        return None;
    }

    // LME ids: `lme_<qid>_<session_idx>_t<turn>`.
    // Back-strip the LAST `_t<digits>` suffix only.
    if source_id.starts_with("lme_") {
        // Find the last occurrence of `_t` followed by all-ASCII-digits.
        if let Some((prefix, suffix)) = source_id.rsplit_once("_t") {
            // `suffix` must be entirely ASCII digits (non-empty).
            if !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()) {
                return Some(prefix.to_string());
            }
        }
        return None;
    }

    // Everything else (mem_*, unrecognised) -> None.
    None
}

/// Apply a per-session diversification cap to an already-score-sorted result
/// list, then truncate to `limit`.
///
/// ### Algorithm
/// 1. Single forward pass over `results` (already descending by score).
/// 2. For each result:
///    - If `session_key` returns `None` → unconditionally move to `kept`
///      (never counts against the cap; production rows always go here).
///    - If `session_key` returns `Some(k)` and `count[k] < max_per_session`
///      → move to `kept`, increment `count[k]`.
///    - Otherwise → move to `backfill`.
/// 3. If `kept.len() < limit`, pull from the **front** of `backfill`
///    (preserving descending-score order) until `kept.len() == limit` or
///    backfill is exhausted.
/// 4. Replace `*results` with `kept`.
///
/// ### Edge cases
/// * `max_per_session == 0` → treated as "no cap" (identity transform).
///   Documented behaviour; avoids divide-by-zero and foot-guns.
/// * `limit > results.len()` → all results kept (backfill may also be
///   exhausted before `limit` is reached).
///
/// ### Bite vs no-bite
/// The cap only *bites* (actually drops items) when there are enough
/// other-session results to fill `limit` without the demoted ones.  When the
/// result pool is thin (fewer than `limit` total results), demoted items
/// re-enter via backfill and the final length equals `min(limit, input.len())`.
pub(crate) fn cap_per_session(
    results: &mut Vec<SearchResult>,
    max_per_session: usize,
    limit: usize,
) {
    // max == 0 -> no-op (identity).
    if max_per_session == 0 {
        return;
    }

    let mut kept: Vec<SearchResult> = Vec::with_capacity(limit.min(results.len()));
    let mut backfill: Vec<SearchResult> = Vec::new();
    let mut counts: HashMap<String, usize> = HashMap::new();

    for r in results.drain(..) {
        match session_key(&r.source_id) {
            None => {
                // Unkeyed (production ids, LoCoMo, etc.) — always keep.
                kept.push(r);
            }
            Some(key) => {
                let count = counts.entry(key).or_insert(0);
                if *count < max_per_session {
                    *count += 1;
                    kept.push(r);
                } else {
                    backfill.push(r);
                }
            }
        }
    }

    // Refill from backfill (front-to-back preserves descending-score order).
    if kept.len() < limit {
        let need = limit - kept.len();
        let take = need.min(backfill.len());
        kept.extend(backfill.drain(..take));
    }

    // Truncate to limit (handles the case where kept grew beyond limit
    // because unkeyed rows were interleaved and no demotion occurred).
    kept.truncate(limit);
    *results = kept;
}

// -- Unit tests ----------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal constructor for test SearchResult — only the fields cap logic
    /// inspects (source_id, score, source, id) need real values.
    fn sr(id: &str, source_id: &str, score: f32) -> SearchResult {
        SearchResult {
            basis: Vec::new(),
            code_basis: Vec::new(),
            id: id.to_string(),
            content: String::new(),
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
            importance: None,
            event_date: None,
            entity_name: None,
            quality: None,
            is_archived: false,
            is_recap: false,
            structured_fields: None,
            retrieval_cue: None,
            source_text: None,
            content_hash: None,
            raw_score: 0.0,
            version: 0,
            pending_revision: false,
            merged_from: None,
            last_delta_summary: None,
        }
    }

    // -- session_key tests -----------------------------------------------------

    /// Basic LME id without underscores in the qid segment.
    #[test]
    fn session_key_lme_simple() {
        assert_eq!(
            session_key("lme_2c63a862_0_t3"),
            Some("lme_2c63a862_0".to_string())
        );
    }

    /// THE underscore-in-qid trap: back-strip `_t12` only, do NOT front-split.
    #[test]
    fn session_key_lme_underscore_in_qid() {
        assert_eq!(
            session_key("lme_gpt4_2655b836_4_t12"),
            Some("lme_gpt4_2655b836_4".to_string())
        );
    }

    /// LoCoMo ids are exempt: single conversation per sample.
    #[test]
    fn session_key_locomo_exempt() {
        assert_eq!(session_key("locomo_42_obs_7"), None);
    }

    /// Production mem_ ids are always None.
    #[test]
    fn session_key_mem_uuid_none() {
        assert_eq!(session_key("mem_8f3a-uuid"), None);
    }

    /// Unrecognised prefix -> None.
    #[test]
    fn session_key_unrecognised_none() {
        assert_eq!(session_key("anything_unrecognized"), None);
    }

    /// `_t` present but suffix is NOT all-digits -> reject (don't mis-key).
    #[test]
    fn session_key_lme_non_digit_suffix_none() {
        assert_eq!(session_key("lme_x_0_tABC"), None);
    }

    /// UTF-8 input must not panic regardless of content.
    #[test]
    fn session_key_utf8_no_panic() {
        let _ = session_key("lme_caf\u{00e9}_t3");
        let _ = session_key("\u{1f600}");
        let _ = session_key("");
    }

    // -- cap_per_session tests -------------------------------------------------

    /// Production mem_* rows: none are keyed -> identity transform regardless
    /// of max/limit.
    #[test]
    fn cap_production_ids_identity() {
        let mut results: Vec<SearchResult> = (0..6)
            .map(|i| sr(&format!("r{i}"), &format!("mem_{i}"), 1.0 - i as f32 * 0.1))
            .collect();
        let original_ids: Vec<String> = results.iter().map(|r| r.id.clone()).collect();
        cap_per_session(&mut results, 3, 10);
        let ids: Vec<String> = results.iter().map(|r| r.id.clone()).collect();
        assert_eq!(
            ids, original_ids,
            "mem_* rows must not be reordered or dropped"
        );
    }

    /// Cap + backfill: 5 hits from session lme_q_0, 2 from lme_q_1, max=3,
    /// limit=6.
    ///
    /// Pass ordering: r0->kept(s0#1), r1->kept(s0#2), r2->kept(s0#3),
    /// r3->backfill(s0 full), r4->backfill(s0 full),
    /// r5->kept(s1#1), r6->kept(s1#2).
    /// kept=[r0,r1,r2,r5,r6] len=5 < limit=6 -> pull r3 from backfill.
    /// Final: [r0,r1,r2,r5,r6,r3]. r4 dropped.
    #[test]
    fn cap_and_backfill_exact_order() {
        let mut results = vec![
            sr("r0", "lme_q_0_t0", 0.9),
            sr("r1", "lme_q_0_t1", 0.8),
            sr("r2", "lme_q_0_t2", 0.7),
            sr("r3", "lme_q_0_t3", 0.6),
            sr("r4", "lme_q_0_t4", 0.5),
            sr("r5", "lme_q_1_t0", 0.4),
            sr("r6", "lme_q_1_t1", 0.3),
        ];
        cap_per_session(&mut results, 3, 6);
        let ids: Vec<&str> = results.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["r0", "r1", "r2", "r5", "r6", "r3"]);
    }

    /// Backfill preserves score order: demoted hits re-enter front-to-back
    /// (highest score first among demoted).
    #[test]
    fn cap_backfill_preserves_score_order() {
        let mut results = vec![
            sr("r0", "lme_q_0_t0", 0.9),
            sr("r1", "lme_q_0_t1", 0.8),
            sr("r2", "lme_q_0_t2", 0.7),
            sr("r3", "lme_q_0_t3", 0.6),
        ];
        cap_per_session(&mut results, 2, 4);
        let ids: Vec<&str> = results.iter().map(|r| r.id.as_str()).collect();
        // kept: r0, r1; backfill: r2, r3 -> refill to limit=4
        assert_eq!(ids, ["r0", "r1", "r2", "r3"]);
    }

    /// Output length == min(limit, input.len()).
    #[test]
    fn cap_limit_respected() {
        let mut results: Vec<SearchResult> = (0..5)
            .map(|i| {
                sr(
                    &format!("r{i}"),
                    &format!("lme_q_0_t{i}"),
                    1.0 - i as f32 * 0.1,
                )
            })
            .collect();
        cap_per_session(&mut results, 10, 3);
        assert_eq!(results.len(), 3, "must not exceed limit");
    }

    /// Limit larger than results -> all kept.
    #[test]
    fn cap_limit_larger_than_results() {
        let mut results: Vec<SearchResult> = (0..3)
            .map(|i| {
                sr(
                    &format!("r{i}"),
                    &format!("lme_q_0_t{i}"),
                    1.0 - i as f32 * 0.1,
                )
            })
            .collect();
        cap_per_session(&mut results, 2, 10);
        assert_eq!(results.len(), 3, "limit > input.len() -> keep all");
    }

    /// Mixed keyed+unkeyed: mem_* rows interleaved with lme_* rows are never
    /// demoted and never consume cap budget.
    #[test]
    fn cap_mixed_keyed_unkeyed() {
        // r0(lme s0 #1), r1(mem_ unkeyed), r2(lme s0 #2), r3(mem_ unkeyed),
        // r4(lme s0 -> demoted, count=2 at max).
        // kept=[r0,r1,r2,r3] backfill=[r4].
        // kept.len()=4 < limit=5 -> pull r4 from backfill.
        // Final: [r0,r1,r2,r3,r4].
        let mut results = vec![
            sr("r0", "lme_q_0_t0", 0.9),
            sr("r1", "mem_abc", 0.85),
            sr("r2", "lme_q_0_t1", 0.8),
            sr("r3", "mem_def", 0.75),
            sr("r4", "lme_q_0_t2", 0.7),
        ];
        cap_per_session(&mut results, 2, 5);
        let ids: Vec<&str> = results.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["r0", "r1", "r2", "r3", "r4"]);
    }

    /// max=0 -> identity (no cap applied, no panic).
    #[test]
    fn cap_max_zero_identity() {
        let mut results = vec![
            sr("r0", "lme_q_0_t0", 0.9),
            sr("r1", "lme_q_0_t1", 0.8),
            sr("r2", "lme_q_0_t2", 0.7),
        ];
        let original_ids: Vec<String> = results.iter().map(|r| r.id.clone()).collect();
        cap_per_session(&mut results, 0, 10);
        let ids: Vec<String> = results.iter().map(|r| r.id.clone()).collect();
        assert_eq!(ids, original_ids, "max=0 must be identity");
    }

    /// Distinct sessions all under cap -> unchanged (no bite).
    #[test]
    fn cap_distinct_sessions_under_cap_unchanged() {
        let mut results = vec![
            sr("r0", "lme_q_0_t0", 0.9),
            sr("r1", "lme_q_1_t0", 0.8),
            sr("r2", "lme_q_2_t0", 0.7),
        ];
        let original_ids: Vec<String> = results.iter().map(|r| r.id.clone()).collect();
        cap_per_session(&mut results, 3, 10);
        let ids: Vec<String> = results.iter().map(|r| r.id.clone()).collect();
        assert_eq!(ids, original_ids);
    }

    /// Cap BITES: enough other-session results to fill limit without the
    /// demoted items -> demoted items truly drop.
    #[test]
    fn cap_bites_when_other_sessions_fill_limit() {
        // 5 from session-0, 5 from session-1, max=3, limit=6.
        // kept fills to 6 on the first pass; backfill never consulted.
        let mut results = vec![
            sr("r0", "lme_q_0_t0", 0.90),
            sr("r1", "lme_q_0_t1", 0.88),
            sr("r2", "lme_q_0_t2", 0.86),
            sr("r3", "lme_q_1_t0", 0.84),
            sr("r4", "lme_q_1_t1", 0.82),
            sr("r5", "lme_q_1_t2", 0.80),
            sr("r6", "lme_q_0_t3", 0.78), // demoted: s0 count already 3
            sr("r7", "lme_q_0_t4", 0.76), // demoted
            sr("r8", "lme_q_1_t3", 0.74), // demoted: s1 count already 3
            sr("r9", "lme_q_1_t4", 0.72), // demoted
        ];
        cap_per_session(&mut results, 3, 6);
        let ids: Vec<&str> = results.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["r0", "r1", "r2", "r3", "r4", "r5"]);
        assert_eq!(results.len(), 6);
    }
}
