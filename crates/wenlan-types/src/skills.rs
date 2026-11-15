// SPDX-License-Identifier: Apache-2.0
//! Wire types for Space-scoped working rules suggested from a Wenlan library.

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

/// One approved line. The quote and immutable revision are the authority for
/// model-authored text; `person_edited` records when the person later made the
/// line their own on the rules page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillLine {
    pub id: String,
    pub kind: SkillLineKind,
    pub text: String,
    pub source_page_id: String,
    pub source_page_title: String,
    pub source_revision: i64,
    pub source_revision_at: i64,
    pub source_quote: String,
    #[serde(default)]
    pub person_edited: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpaceRulesSuggestion {
    pub space_id: String,
    pub space_name: String,
    pub lines: Vec<SkillLine>,
}

/// Suggestion-only edit for a skill the person already owns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillSuggestion {
    pub id: String,
    pub skill: String,
    pub old_line: String,
    pub new_line: String,
    pub reason: String,
    pub source_rule_id: String,
    pub source_page: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillReportKind {
    ChangedPlan,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillBorrowRecord {
    pub at: i64,
    pub tool: String,
    pub space_id: String,
    pub page_title: String,
    pub rule_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub self_report: Option<SkillReportKind>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillsRefreshResponse {
    pub spaces_considered: usize,
    pub spaces_unchanged: usize,
    pub spaces_updated: usize,
    pub suggestions: usize,
    pub failures: usize,
    pub model_calls: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillSuggestionActionResponse {
    pub suggestion: SkillSuggestion,
    pub applied: bool,
    pub undo_available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillReportRequest {
    pub space: String,
    pub rule_id: String,
    pub report: SkillReportKind,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpaceRulesResponse {
    pub space_id: String,
    pub space_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules_page_id: Option<String>,
    pub lines: Vec<SkillLine>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpaceRulesActionResponse {
    pub space_id: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules_page_id: Option<String>,
    pub undo_available: bool,
}
