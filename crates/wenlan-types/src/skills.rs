// SPDX-License-Identifier: Apache-2.0
//! Wire types for skills grown from a person's Wenlan library.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillLineKind {
    Rule,
    Decision,
    Pitfall,
}

impl SkillLineKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rule => "rule",
            Self::Decision => "decision",
            Self::Pitfall => "pitfall",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillLine {
    pub kind: SkillLineKind,
    pub text: String,
    pub source_page: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillSuggestion {
    pub id: String,
    pub skill: String,
    pub old_line: String,
    pub new_line: String,
    pub reason: String,
    pub source_page: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillBorrowRecord {
    pub at: i64,
    pub tool: String,
    pub page_id: String,
    pub page_title: String,
    #[serde(default)]
    pub ai_reported: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillsRefreshResponse {
    pub spaces_read: usize,
    pub source_pages_written: usize,
    pub skills_written: usize,
    pub skills_unchanged: usize,
    pub skills_skipped: usize,
    pub suggestions: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillSuggestionActionResponse {
    pub suggestion: SkillSuggestion,
    pub applied: bool,
    pub undo_available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillReportRequest {
    pub page_id: String,
    /// A short statement of what changed in the plan. Conversation text is
    /// neither requested nor stored.
    pub report: String,
}
