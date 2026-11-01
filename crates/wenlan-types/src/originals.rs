// SPDX-License-Identifier: Apache-2.0
//! Originals kept from imported chats: Claude artifacts and ChatGPT canvases.

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
