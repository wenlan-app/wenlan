// SPDX-License-Identifier: Apache-2.0
//! Wire types for **steep's new pages**: material no page uses yet (imported
//! notes, chats, files, an AI's captures) becomes pages, written by the
//! caller's AI tool through the same page birth the automatic run uses.
//!
//! The flow is list, claim, commit, like steep's rewrites. The list folds the
//! material into items (one note, one chat, one file, one capture). The caller
//! groups items by topic, claims a group, and gets its sources numbered; the
//! commit writes one page that cites them as `[n]`.

use serde::{Deserialize, Serialize};

/// `kind` of a new-material item: something an AI tool saved.
pub const STEEP_NEW_KIND_CAPTURE: &str = "capture";
/// `kind` of a new-material item: a note from an Obsidian vault or Logseq graph.
pub const STEEP_NEW_KIND_NOTE: &str = "note";
/// `kind` of a new-material item: an imported AI conversation.
pub const STEEP_NEW_KIND_CHAT: &str = "chat";
/// `kind` of a new-material item: a file from a folder or an upload.
pub const STEEP_NEW_KIND_FILE: &str = "file";

/// One piece of material no page uses yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteepNewItem {
    /// Pass back in a claim's `item_ids`.
    pub item_id: String,
    /// `capture`, `note`, `chat` or `file`.
    pub kind: String,
    pub title: String,
    /// The start of the text, enough to tell what it is about.
    pub snippet: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space: Option<String>,
    /// Length of the whole text in characters.
    pub chars: u32,
    /// Unix seconds of the newest change.
    pub last_modified: i64,
    /// A claim from another run holds it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub held: bool,
}

/// `GET /api/steep/new`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SteepNewListResponse {
    pub items: Vec<SteepNewItem>,
    /// Every item in the caller's scope, not just this page of items.
    pub total: u32,
    /// Pass back as `cursor` for the next page; `None` on the last page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    /// How many of `total` are of each `kind`.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub kinds: std::collections::BTreeMap<String, u32>,
    /// How many of `total` a claim holds right now.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub held: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// `POST /api/steep/new/claim`: one topic's items, for one page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteepNewClaimRequest {
    pub item_ids: Vec<String>,
}

/// What a new-page claim hands the caller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteepNewClaimResponse {
    /// Opaque; pass it to commit.
    pub token: String,
    /// Unix seconds. Until then no one else may take these items.
    pub held_until: i64,
    /// The items' text, numbered. The page cites them as `[n]`.
    pub sources: Vec<crate::steep::SteepSource>,
    /// Items left out: already in a page, held by another run, or gone.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<String>,
    /// Plain words on what to write.
    pub note: String,
}

/// `POST /api/steep/new/commit`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteepNewCommitRequest {
    pub token: String,
    /// `write` or `release`.
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The page body, citing the claim's sources as `[n]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

/// `action` of a new-page commit: write the page.
pub const STEEP_NEW_ACTION_WRITE: &str = "write";
/// `action` of a new-page commit: give the items back without a page.
pub const STEEP_NEW_ACTION_RELEASE: &str = "release";

/// What a new-page commit did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteepNewCommitResponse {
    /// `created`, `merged` (a page on the same topic already existed and took
    /// the sources) or `released`.
    pub outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// How many cited sentences the source text backs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub citations: Option<String>,
    /// A problem in a ```mermaid block that would stop it drawing, one line
    /// per diagram, so the agent fixes or drops it. The write still landed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_request_parses_a_release() {
        let req: SteepNewCommitRequest =
            serde_json::from_str(r#"{"token":"t","action":"release"}"#).unwrap();
        assert_eq!(req.action, STEEP_NEW_ACTION_RELEASE);
        assert_eq!(req.title, None);
        assert_eq!(req.content, None);
    }

    #[test]
    fn an_unheld_item_omits_held() {
        let item = SteepNewItem {
            item_id: "i".into(),
            kind: STEEP_NEW_KIND_NOTE.into(),
            title: "t".into(),
            snippet: "s".into(),
            space: None,
            chars: 1,
            last_modified: 0,
            held: false,
        };
        let json = serde_json::to_value(&item).unwrap();
        assert!(json.get("held").is_none());
        assert!(json.get("space").is_none());
    }
}
