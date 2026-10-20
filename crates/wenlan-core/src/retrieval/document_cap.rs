// SPDX-License-Identifier: Apache-2.0
//! Per-document flooding guard for ranked retrieval results.
//!
//! Runs after RRF fusion has produced a ranked pool and before top-k truncation.
//! Rows with a `content_hash` share a document budget; rows without a hash are
//! capture/legacy memories and pass through without consuming any document cap.

use std::collections::HashMap;

use wenlan_types::SearchResult;

pub(crate) const DEFAULT_PER_DOCUMENT_CAP: usize = 2;

pub(crate) fn cap_per_document(
    results: Vec<SearchResult>,
    max_per_document: usize,
) -> Vec<SearchResult> {
    if max_per_document == 0 {
        return results;
    }

    let mut counts: HashMap<String, usize> = HashMap::new();
    results
        .into_iter()
        .filter(|result| match result.content_hash.as_deref() {
            Some(hash) if !hash.is_empty() => {
                let count = counts.entry(hash.to_string()).or_insert(0);
                if *count < max_per_document {
                    *count += 1;
                    true
                } else {
                    false
                }
            }
            _ => true,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sr(id: &str, content_hash: Option<&str>) -> SearchResult {
        SearchResult {
            basis: Vec::new(),
            id: id.to_string(),
            content: format!("content for {id}"),
            source: "memory".to_string(),
            source_id: id.to_string(),
            title: String::new(),
            url: None,
            chunk_index: 0,
            last_modified: 0,
            score: 1.0,
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
            is_archived: false,
            is_recap: false,
            structured_fields: None,
            retrieval_cue: None,
            source_text: None,
            content_hash: content_hash.map(str::to_string),
            raw_score: 0.0,
            version: 1,
            pending_revision: false,
            merged_from: None,
            last_delta_summary: None,
        }
    }

    #[test]
    fn cap_per_document_limits_ranked_chunks_by_content_hash() {
        let ranked = vec![
            sr("doc_a_0", Some("hash-a")),
            sr("doc_a_1", Some("hash-a")),
            sr("capture_0", None),
            sr("doc_a_2", Some("hash-a")),
            sr("doc_b_0", Some("hash-b")),
            sr("doc_a_3", Some("hash-a")),
            sr("capture_1", None),
            sr("doc_a_4", Some("hash-a")),
            sr("doc_a_5", Some("hash-a")),
            sr("doc_a_6", Some("hash-a")),
            sr("doc_a_7", Some("hash-a")),
            sr("doc_a_8", Some("hash-a")),
            sr("doc_a_9", Some("hash-a")),
            sr("doc_b_1", Some("hash-b")),
        ];

        let capped = cap_per_document(ranked, 2);

        let ids: Vec<&str> = capped.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "doc_a_0",
                "doc_a_1",
                "capture_0",
                "doc_b_0",
                "capture_1",
                "doc_b_1",
            ],
            "keeps the first two chunks per document, keeps uncapped captures, and preserves ranked order"
        );
        assert!(
            capped
                .iter()
                .filter(|r| r.content_hash.as_deref() == Some("hash-a"))
                .count()
                <= 2,
            "hash-a must not contribute more than the cap"
        );
    }
}
