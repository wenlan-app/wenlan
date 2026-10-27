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

/// The short id a compact list shows for an item: ten hex digits of the
/// FNV-1a hash of its `item_id`. A claim's `item_ids` take it in place of the
/// full id, which for a file can run to a hundred characters.
pub fn steep_short_id(item_id: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in item_id.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{:010x}", hash >> 24)
}

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
    /// Titles of the newest existing pages in the caller's scope (at most
    /// 50), so grouping can name a group after the page it belongs to. On
    /// the first page of the list only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub page_titles: Vec<String>,
    /// The next protocol action, in plain words for hosts with no Wenlan skill.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub next_step: String,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// `POST /api/steep/new/claim`: one topic's items, for one page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteepNewClaimRequest {
    pub item_ids: Vec<String>,
    /// The group's topic. When it names an existing page in the claim's
    /// Space, the claim offers that page as `existing_page`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
}

/// An existing page on a claim's subject, in the claim's Space: the claim's
/// sources go into it (commit action `attach`) instead of a second page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteepExistingPage {
    pub page_id: String,
    pub title: String,
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
    /// Organizing the claimed sources still need before the page is written
    /// (the same steps Wenlan's own model runs in Automatic mode). Present
    /// only when the caller's AI tool does that work: hand it to a small
    /// model and pass the answers to `POST /api/steep/new/organize`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organize: Option<SteepOrganizeWork>,
    /// Titles of existing pages these sources relate to (through the people,
    /// projects and tools they mention). The page may link them as
    /// `[[Title]]` where it talks about them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub link_targets: Vec<String>,
    /// A page on this subject already exists: attach the sources to it
    /// (commit action `attach`) instead of writing another page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub existing_page: Option<SteepExistingPage>,
    /// Non-fatal grouping concerns. The claim remains valid.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    /// The next protocol action, in plain words for hosts with no Wenlan skill.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub next_step: String,
}

/// `organize.tasks[].stage`: memory type, quality, importance and tags.
pub const STEEP_ORGANIZE_STAGE_CLASSIFY: &str = "classify";
/// `organize.tasks[].stage`: the people, projects and tools a memory mentions.
pub const STEEP_ORGANIZE_STAGE_ENTITIES: &str = "entities";

/// The organizing a new-page claim hands out, one small-model call per task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteepOrganizeWork {
    /// The system prompt of each stage present in `tasks`, once.
    pub stages: Vec<SteepOrganizeStage>,
    pub tasks: Vec<SteepOrganizeTask>,
    /// Plain words on how to answer.
    pub note: String,
}

/// One stage's instructions: the system prompt for every task of `stage`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteepOrganizeStage {
    pub stage: String,
    pub instructions: String,
}

/// One small-model call: answer `prompt` under the instructions of `stage`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteepOrganizeTask {
    pub n: u32,
    pub stage: String,
    pub memory_id: String,
    pub prompt: String,
}

/// `POST /api/steep/new/organize`: the small model's replies to a claim's
/// organize tasks, each passed back unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteepOrganizeCommitRequest {
    pub token: String,
    pub answers: Vec<crate::steep::SteepCodeAnswer>,
}

/// What an organize commit did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SteepOrganizeCommitResponse {
    /// Memories whose type, quality and tags were saved.
    pub classified: u32,
    /// Memories whose entity detection was saved.
    pub detected: u32,
    /// Of the claim's sources, how many are now linked to an entity.
    pub linked: u32,
    /// Entities that earned their Wiki page from these answers.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub confirmed: u32,
    /// Answers that could not be read; Wenlan retries the step later.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub failed: u32,
    /// Answers for a memory that changed since the claim; nothing was saved.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub stale: u32,
    /// Task numbers this claim has no open task for (unknown or answered).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unknown: Vec<u32>,
    /// Tasks of the claim still without an answer.
    #[serde(default)]
    pub remaining: u32,
    /// As in the claim, now that the sources are organized.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub link_targets: Vec<String>,
    /// As in the claim, now that the sources are organized.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub existing_page: Option<SteepExistingPage>,
    /// The next protocol action, in plain words for hosts with no Wenlan skill.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub next_step: String,
}

/// `POST /api/steep/new/links`: a claim's `link_targets` and
/// `existing_page` as they are now, read just before the page is written
/// (pages written since the claim count). Needs no model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteepNewLinksRequest {
    pub token: String,
}

/// `POST /api/steep/new/tasks`: a claim's organize tasks still without an
/// answer, fetched by the helper that answers them with only the token, so
/// the main model never passes source text on. Needs no model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteepNewTasksRequest {
    pub token: String,
}

/// What `POST /api/steep/new/tasks` returns: `None` when nothing is left.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SteepNewTasksResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organize: Option<SteepOrganizeWork>,
    /// The next protocol action, in plain words for hosts with no Wenlan skill.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub next_step: String,
}

/// What `POST /api/steep/new/links` returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SteepNewLinksResponse {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub link_targets: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub existing_page: Option<SteepExistingPage>,
    /// The next protocol action, in plain words for hosts with no Wenlan skill.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub next_step: String,
}

/// `POST /api/steep/new/commit`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteepNewCommitRequest {
    pub token: String,
    /// `write`, `attach` or `release`.
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The page body, citing the claim's sources as `[n]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

/// `action` of a new-page commit: write the page.
pub const STEEP_NEW_ACTION_WRITE: &str = "write";
/// `action` of a new-page commit: add the claim's sources to its
/// `existing_page`, which then waits to be rewritten with all its sources.
pub const STEEP_NEW_ACTION_ATTACH: &str = "attach";
/// `action` of a new-page commit: give the items back without a page.
pub const STEEP_NEW_ACTION_RELEASE: &str = "release";

/// What a new-page commit did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteepNewCommitResponse {
    /// `created`, `merged` (a page on the same topic already existed and took
    /// the sources), `attached` (the claim's `existing_page` took them) or
    /// `released`.
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
    /// Link targets the saved page could still use but did not.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub link_targets: Vec<String>,
    /// The next protocol action, in plain words for hosts with no Wenlan skill.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub next_step: String,
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

    #[test]
    fn organize_receipt_keeps_zero_remaining() {
        let json = serde_json::to_value(SteepOrganizeCommitResponse::default()).unwrap();
        assert_eq!(json["remaining"], 0);
    }
}

#[cfg(test)]
mod short_id_tests {
    use super::steep_short_id;

    #[test]
    fn a_short_id_is_ten_stable_hex_digits() {
        let id = steep_short_id("vault1::Projects/Home/Kitchen renovation notes.md");
        assert_eq!(id.len(), 10);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(
            id,
            steep_short_id("vault1::Projects/Home/Kitchen renovation notes.md")
        );
        assert_ne!(
            id,
            steep_short_id("vault1::Projects/Home/Kitchen renovation notes 2.md")
        );
        // FNV-1a 64 of "" is cbf29ce484222325; its top 40 bits.
        assert_eq!(steep_short_id(""), "cbf29ce484");
    }
}
