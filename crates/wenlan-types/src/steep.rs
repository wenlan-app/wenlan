// SPDX-License-Identifier: Apache-2.0
//! Wire types for **steep on demand**: the caller's own AI tool rewrites the
//! notes that are waiting for steep, through the same commit path the
//! automatic background steep uses.
//!
//! The flow is list, claim, commit. A claim hands the caller a snapshot of the
//! note and its numbered sources plus an opaque token; the commit carries the
//! token back and is refused when the note or its sources moved since the
//! claim. A claim also holds the note for a while so the automatic steep and
//! another on-demand caller leave it alone.

use serde::{Deserialize, Serialize};

/// `plan` of a claim: rewrite the note from the numbered sources.
pub const STEEP_PLAN_REWRITE: &str = "rewrite";
/// `plan` of a claim: the note is the user's own writing, so the rewrite is
/// staged as a revision card the user accepts or dismisses.
pub const STEEP_PLAN_REVIEW_CARD: &str = "review_card";
/// `plan` of a claim: an AI cannot settle this note alone (no sources left,
/// or it relies on other notes). Nothing is held; the note waits for the user.
pub const STEEP_PLAN_NEEDS_YOU: &str = "needs_you";

/// `action` of a commit: write (or stage) the rewritten body.
pub const STEEP_ACTION_REWRITE: &str = "rewrite";
/// `action` of a commit: the sources changed but the note still says the same
/// thing; nothing is rewritten and the note stops waiting.
pub const STEEP_ACTION_STILL_ACCURATE: &str = "still_accurate";
/// `action` of a commit: give the note back without doing anything.
pub const STEEP_ACTION_RELEASE: &str = "release";

/// One note waiting for steep.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteepWorkItem {
    pub page_id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space: Option<String>,
    /// `source_updated` or `source_removed`.
    pub stale_reason: String,
    /// The user wrote or edited this note: a rewrite becomes a revision card.
    pub human_owned: bool,
    /// An AI cannot settle it alone (see [`STEEP_PLAN_NEEDS_YOU`]).
    pub needs_you: bool,
    /// Why an earlier rewrite did not land, when one did not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<String>,
    /// Who holds the note right now: `automatic` or `on_demand`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leased_by: Option<String>,
    pub source_count: u32,
}

/// `GET /api/steep/worklist`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SteepWorklistResponse {
    pub items: Vec<SteepWorkItem>,
    /// Every note waiting for steep in the caller's scope, not just this page
    /// of items.
    pub total: u32,
    /// Pass back as `cursor` for the next page; `None` on the last page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// One numbered source in a claim. The rewrite cites it as `[n]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteepSource {
    pub n: u32,
    pub memory_id: String,
    pub text: String,
}

/// `POST /api/steep/claim`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteepClaimRequest {
    pub page_id: String,
}

/// What a claim hands the caller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteepClaimResponse {
    pub page_id: String,
    pub title: String,
    /// `rewrite`, `review_card` or `needs_you`.
    pub plan: String,
    /// Opaque; pass it to commit. Absent when `plan` is `needs_you`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// Unix seconds. Until then no one else may take the note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held_until: Option<i64>,
    pub version: i64,
    pub stale_reason: String,
    pub current_content: String,
    /// The note's surviving sources, numbered. A removed source is not here.
    pub sources: Vec<SteepSource>,
    /// Plain words on what to do, or why the note needs the user.
    pub note: String,
}

/// `POST /api/steep/commit`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteepCommitRequest {
    pub token: String,
    /// `rewrite`, `still_accurate` or `release`.
    pub action: String,
    /// The new body for `rewrite`, citing the claim's sources as `[n]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

/// What a commit did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteepCommitResponse {
    pub page_id: String,
    /// `written`, `review_card`, `still_accurate`, `unchanged` or `released`.
    pub outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision_card_id: Option<String>,
    /// How many cited sentences the source text backs, e.g. `4/5 verified`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub citations: Option<String>,
    /// The same commit was already applied; this is its recorded result.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub replayed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_request_parses_without_content() {
        let req: SteepCommitRequest =
            serde_json::from_str(r#"{"token":"t","action":"release"}"#).unwrap();
        assert_eq!(req.action, STEEP_ACTION_RELEASE);
        assert_eq!(req.content, None);
    }

    #[test]
    fn worklist_omits_absent_cursor() {
        let json = serde_json::to_value(SteepWorklistResponse::default()).unwrap();
        assert!(json.get("next_cursor").is_none());
        assert_eq!(json["total"], 0);
    }
}
