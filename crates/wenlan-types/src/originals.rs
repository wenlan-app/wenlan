// SPDX-License-Identifier: Apache-2.0
//! Originals: Claude artifacts and ChatGPT canvases kept from imported chats,
//! and HTML, SVG or Markdown an AI tool keeps from a live session.

use serde::{Deserialize, Serialize};

/// `GET /api/pages/{id}/original`: where a page made from a kept original
/// came from, so the app can open the conversation in the original AI tool
/// and find the kept file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageOriginalResponse {
    pub page_id: String,
    /// `claude` or `chatgpt`.
    pub vendor: String,
    /// The vendor's own conversation id.
    pub conversation_id: String,
    /// Where the conversation opens in the vendor's app.
    pub conversation_url: String,
    /// The vendor's id for the artifact or canvas within the conversation.
    pub artifact_id: String,
    /// The export's own type for it (`text/html`, `code/python`, ...).
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The kept file, relative to the data root (`originals/<vendor>/<conversation>/<file>`).
    pub file_path: String,
    /// When the AI last wrote it in the chat (unix seconds), when the export says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub written_at: Option<i64>,
    /// When Wenlan kept it (unix seconds).
    pub imported_at: i64,
    /// The newest version, when a later one replaced this one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub newer: Option<OriginalVersion>,
    /// The versions this one replaced, newest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub older: Vec<OriginalVersion>,
}

/// One version of a kept original.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OriginalVersion {
    pub page_id: String,
    pub title: String,
    /// When Wenlan kept it (unix seconds).
    pub kept_at: i64,
}

/// `POST /api/originals`: keep something an AI tool made in this session (an
/// HTML page, an SVG, a Markdown document) as it is, as its own page. The
/// session comes from the `x-wenlan-session` header.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeepOriginalRequest {
    /// The page title: what it shows, in the person's words.
    pub title: String,
    /// The file's text, exactly as the AI tool made it.
    pub content: String,
    /// `html`, `svg` or `markdown` (a file name works too, e.g. `map.html`).
    pub format: String,
    /// What it shows, as text: an indented outline or an ASCII drawing of
    /// its parts and how they connect. Kept as a memory of this session, so
    /// the night's tidy and later sessions read the structure, not the HTML.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structure: Option<String>,
    /// The older version this one replaces: its page id, or its title as it
    /// was kept. The older page stays as it was and points to this one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaces: Option<String>,
}

/// The page that keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeepOriginalResponse {
    pub page_id: String,
    pub title: String,
    /// False when this session had already kept the same content, so the
    /// existing page is returned.
    pub created: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_original_round_trips_and_leaves_out_what_is_unknown() {
        let original = PageOriginalResponse {
            page_id: "page_1".into(),
            vendor: "claude".into(),
            conversation_id: "c1".into(),
            conversation_url: "https://claude.ai/chat/c1".into(),
            artifact_id: "landing".into(),
            kind: "text/html".into(),
            title: None,
            file_path: "originals/claude/c1/landing.html".into(),
            written_at: None,
            imported_at: 1,
        };
        let json = serde_json::to_value(&original).unwrap();
        assert!(json.get("title").is_none());
        assert!(json.get("written_at").is_none());
        let back: PageOriginalResponse = serde_json::from_value(json).unwrap();
        assert_eq!(back, original);
    }
}
