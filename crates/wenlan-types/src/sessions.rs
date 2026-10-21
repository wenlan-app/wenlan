// SPDX-License-Identifier: Apache-2.0
//! Agent sessions: the conversation a write came from.
//!
//! A session is named on the wire by `<tool>:<external_id>` (for example
//! `claude-code:00893aaf-19fa-41d2-8238-13269b9b3ca0`). The daemon derives a
//! stable row id from that name, so replays and backfills converge on one row.

use serde::{Deserialize, Serialize};

/// Header a client sends on a write to name the session it came from.
pub const SESSION_HEADER: &str = "x-wenlan-session";

const MAX_TOOL_LEN: usize = 32;
const MAX_EXTERNAL_ID_LEN: usize = 128;

/// A parsed `<tool>:<external_id>` session name.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionRef {
    pub tool: String,
    pub external_id: String,
}

impl SessionRef {
    /// Parse `<tool>:<external_id>`, splitting on the first `:`.
    ///
    /// `tool` is `^[a-z0-9][a-z0-9-]{0,31}$`; `external_id` is 1 to 128
    /// characters of `[A-Za-z0-9._-]`.
    pub fn parse(raw: &str) -> Result<Self, String> {
        let raw = raw.trim();
        let (tool, external_id) = raw
            .split_once(':')
            .ok_or_else(|| "session must be <tool>:<external_id>".to_string())?;
        Self::new(tool, external_id)
    }

    /// Validate and build a session name from its two parts.
    pub fn new(tool: &str, external_id: &str) -> Result<Self, String> {
        let mut tool_chars = tool.chars();
        let tool_ok = tool.len() <= MAX_TOOL_LEN
            && tool_chars
                .next()
                .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
            && tool_chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if !tool_ok {
            return Err(format!(
                "session tool must match ^[a-z0-9][a-z0-9-]{{0,{}}}$",
                MAX_TOOL_LEN - 1
            ));
        }
        let id_ok = !external_id.is_empty()
            && external_id.len() <= MAX_EXTERNAL_ID_LEN
            && external_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
        if !id_ok {
            return Err(format!(
                "session external_id must be 1 to {MAX_EXTERNAL_ID_LEN} characters of [A-Za-z0-9._-]"
            ));
        }
        Ok(Self {
            tool: tool.to_string(),
            external_id: external_id.to_string(),
        })
    }
}

impl std::fmt::Display for SessionRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.tool, self.external_id)
    }
}

/// One conversation in an AI tool, with where it ran. Every field after
/// `external_id` is optional context a client reports once per process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSession {
    pub id: String,
    pub tool: String,
    pub external_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_remote: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_commit: Option<String>,
    /// Where the transcript lives on the user's machine. A location only; the
    /// daemon never reads it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_path: Option<String>,
    /// Space of the first write, when it had one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space: Option<String>,
    pub first_write_at: i64,
    pub last_write_at: i64,
}

/// `POST /api/sessions`. Non-null fields overwrite; absent fields keep what
/// is stored. Recording metadata is not a write, so `last_write_at` does not
/// move.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpsertSessionRequest {
    /// `<tool>:<external_id>`.
    pub session: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_remote: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_path: Option<String>,
}

/// A memory a session wrote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMemoryEntry {
    pub source_id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space: Option<String>,
    pub created_at: i64,
}

/// A note version a session wrote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionPageEntry {
    pub id: String,
    pub title: String,
    pub version: i64,
    pub edited_at: i64,
}

/// A Brief update a session made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionHandoffEntry {
    pub space: String,
    pub brief_version: i64,
    pub summary: String,
    pub created_at: i64,
}

/// What a session read from Wenlan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionReadKind {
    /// A memory search or recall; the label is the query.
    Recall,
    /// A Space Brief read; the label is the Space name.
    Brief,
    /// A note read in full; the label is the note id.
    ReadPage,
}

impl SessionReadKind {
    /// The value stored in `session_reads.kind`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Recall => "recall",
            Self::Brief => "brief",
            Self::ReadPage => "read_page",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "recall" => Some(Self::Recall),
            "brief" => Some(Self::Brief),
            "read_page" => Some(Self::ReadPage),
            _ => None,
        }
    }
}

/// One thing a session looked up, newest first in the detail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionReadEntry {
    pub kind: SessionReadKind,
    /// The query (`recall`), the Space name (`brief`), or the note id
    /// (`read_page`). Queries are cut short.
    pub label: String,
    /// The note's title, for `read_page` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub created_at: i64,
}

/// A note that cites a memory the session wrote, without the session having
/// written a version of the note itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionCitedByEntry {
    pub id: String,
    pub title: String,
    /// When the note most recently linked one of the session's memories.
    pub cited_at: i64,
}

/// `GET /api/sessions/{id}`: the session and what it wrote, newest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionDetailResponse {
    pub session: AgentSession,
    pub memories: Vec<SessionMemoryEntry>,
    pub pages: Vec<SessionPageEntry>,
    pub handoffs: Vec<SessionHandoffEntry>,
    /// What the session read from Wenlan, newest first, at most 10. Only
    /// reads that named the session (`x-wenlan-session`) are recorded.
    #[serde(default)]
    pub reads: Vec<SessionReadEntry>,
    /// How many recorded reads this caller can see, so a client can say how
    /// many more there are than `reads` lists.
    #[serde(default)]
    pub reads_total: i64,
    /// Notes that later cited a memory this session wrote, newest first, at
    /// most 10. Notes the session wrote a version of are in `pages` instead.
    #[serde(default)]
    pub cited_by: Vec<SessionCitedByEntry>,
    /// Whether the transcript file named by `transcript_path` is still on
    /// this machine. Tools prune old transcripts, so the path alone does not
    /// say. Computed by the detail route only; false when no path is stored.
    #[serde(default)]
    pub transcript_available: bool,
}

/// How a session relates to a note.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PageSessionVia {
    /// The session wrote a version of the note.
    Wrote,
    /// The session wrote a memory the note cites.
    Source,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageSessionEntry {
    pub session: AgentSession,
    pub via: PageSessionVia,
}

/// `GET /api/pages/{id}/sessions`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageSessionsResponse {
    pub sessions: Vec<PageSessionEntry>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tool_and_external_id() {
        let parsed = SessionRef::parse("claude-code:00893aaf-19fa-41d2-8238-13269b9b3ca0").unwrap();
        assert_eq!(parsed.tool, "claude-code");
        assert_eq!(parsed.external_id, "00893aaf-19fa-41d2-8238-13269b9b3ca0");
        assert_eq!(
            parsed.to_string(),
            "claude-code:00893aaf-19fa-41d2-8238-13269b9b3ca0"
        );
    }

    #[test]
    fn read_kinds_round_trip_their_stored_names() {
        for kind in [
            SessionReadKind::Recall,
            SessionReadKind::Brief,
            SessionReadKind::ReadPage,
        ] {
            assert_eq!(SessionReadKind::parse(kind.as_str()), Some(kind));
            assert_eq!(
                serde_json::to_value(kind).unwrap(),
                serde_json::json!(kind.as_str())
            );
        }
        assert_eq!(SessionReadKind::parse("write"), None);
    }

    #[test]
    fn an_older_detail_without_the_new_fields_still_parses() {
        let detail: SessionDetailResponse = serde_json::from_value(serde_json::json!({
            "session": {
                "id": "ses_a", "tool": "codex", "external_id": "x",
                "first_write_at": 1, "last_write_at": 2
            },
            "memories": [], "pages": [], "handoffs": []
        }))
        .unwrap();
        assert!(detail.reads.is_empty() && detail.cited_by.is_empty());
        assert_eq!(detail.reads_total, 0);
    }

    #[test]
    fn splits_on_the_first_colon_only() {
        // The second colon lands in the external id, which does not allow it.
        assert!(SessionRef::parse("codex:abc:def").is_err());
        let parsed = SessionRef::parse(" codex:019a.b_c-d ").unwrap();
        assert_eq!(parsed.external_id, "019a.b_c-d");
    }

    #[test]
    fn rejects_malformed_names() {
        for bad in [
            "",
            "nocolon",
            ":id",
            "tool:",
            "Tool:id",
            "-tool:id",
            "tool_x:id",
            "tool:has space",
            "tool:slash/id",
        ] {
            assert!(
                SessionRef::parse(bad).is_err(),
                "{bad:?} should be rejected"
            );
        }
        let long_id = "a".repeat(129);
        assert!(SessionRef::parse(&format!("tool:{long_id}")).is_err());
        assert!(SessionRef::parse(&format!("tool:{}", "a".repeat(128))).is_ok());
        let long_tool = "a".repeat(33);
        assert!(SessionRef::parse(&format!("{long_tool}:id")).is_err());
        assert!(SessionRef::parse(&format!("{}:id", "a".repeat(32))).is_ok());
    }
}
