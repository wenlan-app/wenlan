// SPDX-License-Identifier: Apache-2.0
//! Wire types for the in-chat check (UX-CHAT-1..6) and the procedural page
//! every AI tool reads before starting work (UX-HOW-1).
//!
//! The rules live in the daemon. Hooks and MCP only carry text in and print
//! what comes back, so every AI tool gets the same answer.

use serde::{Deserialize, Serialize};

/// The most reminders the daemon hands out per local day, across sessions.
pub const CHAT_REMINDER_DAILY_CAP: u32 = 5;
/// History op recorded for every reminder handed out.
pub const CHAT_REMINDER_OP: &str = "chat.reminder";
/// `role` of the page that says how the person wants AI tools to work.
pub const INSTRUCTIONS_PAGE_ROLE: &str = "instructions";

/// `POST /api/check`: the AI's latest reply, checked against what was
/// decided earlier.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatCheckRequest {
    pub reply: String,
    /// `<tool>:<id>`, e.g. `claude-code:<session id>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
}

/// Why a check returned no reminders without looking, when it did not look.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatCheckSkipped {
    /// The daemon could not answer within the budget (1 s); the caller goes on.
    Timeout,
    /// Today's reminders are used up.
    DailyCap,
    /// Nothing to check (empty text).
    Empty,
    /// The person has not turned the in-chat check on (it is opt-in).
    Off,
}

/// `POST /api/check` response. `reminders` is empty when nothing conflicts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatCheckResponse {
    pub reminders: Vec<ChatReminder>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped: Option<ChatCheckSkipped>,
    /// Reminders still available today after this answer.
    #[serde(default)]
    pub remaining_today: u32,
}

/// One reminder: the reply goes against something decided earlier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatReminder {
    /// History sequence number of the `chat.reminder` entry.
    pub seq: i64,
    /// One line for the person, with the date and the link.
    pub text: String,
    /// The decided statement, as recorded.
    pub decided: String,
    /// Local date the decision was made, `YYYY-MM-DD`.
    pub decided_on: String,
    /// The reply sentence that goes against it.
    pub reply_excerpt: String,
    /// The option the reply adopted or dropped.
    pub option: String,
    pub source: DecisionSource,
}

/// An openable source for a decided statement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionSource {
    pub memory_id: String,
    pub memory_version: i64,
    /// The newest visible page citing the memory, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<SourcePageRef>,
    /// `wenlan://page/{id}` when a page cites it, else `wenlan://memory/{id}`.
    pub link: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourcePageRef {
    pub page_id: String,
    pub version: i64,
    pub title: String,
}

/// `POST /api/check/context`: the person's prompt, to hand the AI the past
/// decisions it touches before it answers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatContextRequest {
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatContextResponse {
    /// Most relevant first, at most three. Empty when none applies.
    pub decisions: Vec<PastDecision>,
    /// `decisions` as the text block a hook hands the AI, or empty (absent
    /// on the wire) when there are none.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub context: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped: Option<ChatCheckSkipped>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PastDecision {
    pub decided: String,
    pub decided_on: String,
    pub source: DecisionSource,
}

/// The page that says how the person wants AI tools to work with them
/// (`type: page`, `role: instructions`). Found by role, never by title.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstructionsPage {
    pub page_id: String,
    pub title: String,
    pub version: i64,
    /// The whole body.
    pub content: String,
    pub role: String,
    /// `wenlan://page/{id}`.
    pub link: String,
}

/// `GET /api/pages/instructions`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstructionsPageResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<InstructionsPage>,
    /// True when this call created the page from the person's memories.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub seeded: bool,
}

/// `GET /api/pages/{id}` `body`: the page body in its three layers
/// (UX-VIEW-2/3). `page.content` keeps the whole Markdown.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageBodyParts {
    /// First paragraph of the human part.
    pub short: String,
    /// Everything before the details marker.
    pub human: String,
    /// Everything after the marker heading; absent without a marker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details_heading: Option<String>,
}

/// `wenlan://page/{id}`: opens the page in the app.
pub fn page_link(page_id: &str) -> String {
    format!("wenlan://page/{page_id}")
}

/// `wenlan://memory/{id}`: opens the memory in the app.
pub fn memory_link(memory_id: &str) -> String {
    format!("wenlan://memory/{memory_id}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn requests_accept_the_minimal_body() {
        let req: ChatCheckRequest = serde_json::from_value(json!({"reply": "hi"})).unwrap();
        assert_eq!(req.reply, "hi");
        assert!(req.session.is_none() && req.space.is_none() && req.workspace.is_none());
        let req: ChatContextRequest = serde_json::from_value(json!({"prompt": "p"})).unwrap();
        assert_eq!(req.prompt, "p");
    }

    #[test]
    fn an_empty_answer_is_small_on_the_wire() {
        assert_eq!(
            serde_json::to_value(ChatCheckResponse::default()).unwrap(),
            json!({"reminders": [], "remaining_today": 0})
        );
        assert_eq!(
            serde_json::to_value(ChatCheckResponse {
                skipped: Some(ChatCheckSkipped::Timeout),
                ..Default::default()
            })
            .unwrap()["skipped"],
            json!("timeout")
        );
        assert_eq!(
            serde_json::to_value(InstructionsPageResponse::default()).unwrap(),
            json!({})
        );
    }

    #[test]
    fn links_use_the_wenlan_scheme() {
        assert_eq!(page_link("page_1"), "wenlan://page/page_1");
        assert_eq!(memory_link("mem_1"), "wenlan://memory/mem_1");
    }
}
