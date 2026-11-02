// SPDX-License-Identifier: Apache-2.0
//! The words of a conversation, as search shows them (UX-FIND-1, UX-FIND-2).
//!
//! A conversation's transcript stays where its AI tool keeps it. The daemon
//! reads it to find the sentence someone searched for and the lines around
//! it; nothing here is stored.

use serde::{Deserialize, Serialize};

/// Who said a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationSpeaker {
    /// The person.
    You,
    /// The AI tool.
    Ai,
}

/// One thing someone said in a conversation. `at` is Unix seconds, when the
/// transcript records it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationLine {
    pub speaker: ConversationSpeaker,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<i64>,
}

/// One sentence search found, with a little of what came before and after.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationHit {
    /// The conversation (`ses_` row id).
    pub session_id: String,
    /// The AI tool it ran in, such as `claude-code`.
    pub tool: String,
    /// The conversation's own title, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// What you're working on that this conversation belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thing_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thing_title: Option<String>,
    /// Where the line sits in the conversation, for opening it there.
    pub index: usize,
    pub line: ConversationLine,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<ConversationLine>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<ConversationLine>,
}

/// `GET /api/conversations/search?q=`: newest conversation first.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ConversationSearchResponse {
    pub hits: Vec<ConversationHit>,
}

/// `GET /api/sessions/{id}/lines`: the whole conversation, for "show all".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationLinesResponse {
    pub session_id: String,
    pub tool: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thing_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thing_title: Option<String>,
    pub lines: Vec<ConversationLine>,
}
