// SPDX-License-Identifier: Apache-2.0
//! The nightly trust scan: what is already wrong in the pages and their
//! sources, found by reading only. Nothing here changes a page; the person
//! reads the report and approves any repair.
//!
//! `GET /api/trust/scan` returns the latest report as [`TrustScanResponse`].

use serde::{Deserialize, Serialize};

/// The latest report, or `None` before the first nightly run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustScanResponse {
    pub report: Option<TrustScanReport>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustScanReport {
    /// Unix seconds when the scan ran.
    pub ran_at: i64,
    pub counts: TrustScanCounts,
    /// Live pages that are cut off, hold half a `[[` link, or hold U+FFFD.
    pub damaged_pages: Vec<DamagedPage>,
    /// Live pages whose latest version lost most of its CJK text, or gained
    /// U+FFFD, against the version before it.
    pub cjk_losses: Vec<CjkLoss>,
    /// Live pages whose own source list and live source links disagree.
    pub source_mismatches: Vec<SourceMismatch>,
    /// Live pages that should rest on sources and have none at all.
    pub sourceless_pages: Vec<PageRef>,
    /// What the daily orphaned-source cleanup will remove on its next run.
    pub dangling_sources: DanglingSources,
    /// The older `page_sources` table, compared with the page's own list.
    pub legacy_page_sources: LegacyPageSources,
    /// Files Wenlan would wrongly read as edited outside it. `None` when the
    /// scan ran without the pages folder.
    pub outside_edit_false_positives: Option<Vec<OutsideEditFlag>>,
    /// Pages whose citation backfill gave up and cleared their citations.
    pub backfill_give_ups: Vec<BackfillGiveUp>,
    /// Steps that failed; the rest of the report still stands.
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustScanCounts {
    pub pages_scanned: usize,
    pub cut_off: usize,
    pub half_links: usize,
    pub broken_characters: usize,
    pub cjk_losses: usize,
    pub source_mismatches: usize,
    pub sourceless_pages: usize,
    pub dangling_links: usize,
    pub dangling_listed: usize,
    pub outside_edit_false_positives: Option<usize>,
    pub backfill_give_ups: usize,
}

/// A page named in a finding.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageRef {
    pub page_id: String,
    pub title: String,
    pub kind: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PageDamage {
    /// The last line stops mid-sentence.
    CutOff,
    /// A `[[` link opened and never closed.
    HalfLink,
    /// U+FFFD, a character lost in a bad encoding.
    BrokenCharacters,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DamagedPage {
    pub page: PageRef,
    pub version: i64,
    pub damage: Vec<PageDamage>,
    /// The last characters of the page, or of the half link.
    pub excerpt: String,
    /// The damage sits in sentences the AI may not change (the person's, or
    /// unknown on a guarded page): a repair must be the person's call.
    pub in_person_sentences: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CjkLoss {
    pub page: PageRef,
    pub previous_version: i64,
    pub version: i64,
    /// Who wrote the latest version, as page history records it.
    pub edited_by: String,
    pub cjk_before: usize,
    pub cjk_after: usize,
    pub chars_before: usize,
    pub chars_after: usize,
    pub broken_before: usize,
    pub broken_after: usize,
    /// For gained broken characters: they sit in sentences the AI may not
    /// change. For lost text: the page holds any such sentence, so putting
    /// the text back is the person's call.
    pub in_person_sentences: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceMismatch {
    pub page: PageRef,
    /// Distinct ids in the page's own `source_memory_ids`.
    pub listed: usize,
    /// Distinct live `cites` links, which is what the app shows as sources.
    pub linked: usize,
    /// Up to five ids listed but not linked, and linked but not listed.
    pub listed_only: Vec<String>,
    pub linked_only: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DanglingSources {
    /// Live `cites` links to memories that no longer exist.
    pub links: usize,
    /// Ids in pages' own source lists for memories that no longer exist.
    pub listed: usize,
    /// Up to fifty pages with what would be removed from each.
    pub pages: Vec<DanglingSourcePage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DanglingSourcePage {
    pub page_id: String,
    pub missing: Vec<String>,
}

/// The older source table. Nothing writes it any more; lint's provenance
/// checks still read it, and the daily cleanup does not touch it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacyPageSources {
    pub rows: usize,
    /// Live pages whose row count differs from their own list.
    pub count_mismatches: usize,
    /// Live non-entity pages with no row.
    pub pages_without_rows: usize,
    /// Rows whose memory is gone, matched by memory id or source id.
    pub dangling_rows: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutsideEditReason {
    /// The file is byte for byte what Wenlan last wrote (its stored
    /// checksum matches), yet it is not today's render of the page.
    StoredChecksumMatches,
    /// No checksum was ever stored and the file is behind the page; the
    /// writer refuses it unless page history still holds that version.
    NoChecksumFileBehind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutsideEditFlag {
    pub page: PageRef,
    pub file: String,
    pub file_version: i64,
    pub page_version: i64,
    pub reason: OutsideEditReason,
    /// For `NoChecksumFileBehind`: page history still holds the file's
    /// version, so the startup adoption can recover it.
    pub history_has_file_version: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackfillGiveUp {
    pub page: PageRef,
    /// The give-up note, e.g. "citation backfill gave up: zero markers after
    /// 3 attempts".
    pub reason: String,
    /// Unix seconds of the give-up entry.
    pub gave_up_at: Option<i64>,
}

/// `POST /api/pages/{id}/citations/requeue`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CitationRequeueResponse {
    pub page_id: String,
    /// False when the page had not given up (nothing to re-queue).
    pub requeued: bool,
}
