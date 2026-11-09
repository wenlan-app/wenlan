// SPDX-License-Identifier: Apache-2.0
//! The words of a conversation, as search shows them (UX-FIND-1, UX-FIND-2).
//!
//! A conversation's transcript stays where its AI tool keeps it. The daemon
//! reads it to find the sentence someone searched for and the lines around
//! it. The only lines Wenlan stores are the few an AI tool sends with a
//! memory it saves ([`MAX_KEPT_SAID_LINES`]), for chats with no transcript
//! Wenlan can read.

use serde::{Deserialize, Serialize};

/// Most lines an AI tool can send with one memory (`StoreMemoryRequest::said`).
/// The lines nearest the save are kept.
pub const MAX_KEPT_SAID_LINES: usize = 3;
/// Longest kept line, in characters; a longer one is cut.
pub const MAX_KEPT_SAID_CHARS: usize = 2_000;

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
    /// Where `lines` starts in the conversation, so a hit's `index` finds its
    /// line when only the newest lines of a long conversation are returned.
    #[serde(default)]
    pub first_index: usize,
    pub lines: Vec<ConversationLine>,
}

/// One line a memory came from, with the line before and after it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaidLine {
    /// Where the line sits in the conversation, for opening it there.
    pub index: usize,
    pub line: ConversationLine,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<ConversationLine>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<ConversationLine>,
}

/// `GET /api/memories/{source_id}/said`: the lines of its conversation a
/// memory came from (UX-WHO-6). `lines` is empty when the conversation can
/// be read but no line says the same thing.
///
/// `kept` is true when the lines are the copy the AI tool sent with the
/// memory (`StoreMemoryRequest::said`), because the conversation itself can't
/// be read: a chat in ChatGPT or Claude on the web, or a transcript that is
/// gone. Such lines are only what was kept, so they open nothing further.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemorySaidResponse {
    /// The conversation, when Wenlan knows it. A memory saved from a web
    /// chat has kept lines but no conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub tool: String,
    #[serde(default)]
    pub kept: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thing_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thing_title: Option<String>,
    pub lines: Vec<SaidLine>,
}
