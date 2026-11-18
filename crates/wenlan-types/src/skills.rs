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
    /// The line applies to any work, not only its own Space's project. AI
    /// tools read general lines in every Space and with no Space at all; the
    /// line still lives on, and is checked against, its own Space's page.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub general: bool,
    /// The person wrote this line themselves on a page of their own. It
    /// counts at once, with no model and no accept.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hand_written: bool,
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

/// How the rules used in every Space reach one AI tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RulesDeliveryMethod {
    /// Wenlan keeps them in a file the tool reads in every conversation.
    File,
    /// The tool gets them only when it asks Wenlan through MCP.
    Mcp,
}

/// Where one AI tool stands, worked out when asked from the file as it is.
/// It says what Wenlan wrote, never whether the AI follows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RulesDeliveryState {
    /// The file holds exactly what Wenlan would write now.
    Delivered,
    /// Another file the tool reads holds them on disk right now: one the
    /// tool's file brings in, or the file the tool falls back to.
    CoveredByAnotherFile,
    /// The tool's own folder is not on this computer; nothing was written.
    FolderMissing,
    /// The person's text plus Wenlan's part is more than the tool reads.
    OverLimit,
    /// The setting is off and the person wrote no rule for every Space.
    SettingOff,
    /// A dev, test or demo run on other data never writes the tools' files.
    IsolatedRun,
    /// The tool is not connected, or was turned off.
    NotConnected,
    /// There is no rule used in every Space to write.
    NoRules,
    /// The file does not hold what Wenlan would write now and the next sync
    /// has not run yet.
    OutOfDate,
    /// Wenlan cannot safely change the file and leaves it exactly as it is;
    /// `blocked_reason` says why.
    Blocked,
    /// No file is written for this tool; it gets the rules when it asks.
    OnRequest,
}

/// One AI tool's row of `GET /api/skills/delivery`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolRulesDelivery {
    pub id: String,
    pub display_name: String,
    /// Connected and not turned off.
    pub connected: bool,
    pub method: RulesDeliveryMethod,
    /// The tool's always-read file, when the method is `file`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub state: RulesDeliveryState,
    /// Why the file is left alone, as a short machine word, only when
    /// `blocked`: `start_marker_without_end`, `end_marker_without_start`,
    /// `end_marker_before_start`, `more_than_one_part`,
    /// `part_would_not_read_back`, `broken_link`, `folder_not_usable`,
    /// `not_a_file`, `not_text`, `not_readable`, `read_only`,
    /// `not_wenlans_file`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<String>,
    /// When Wenlan last wrote its part (Unix seconds), from Wenlan's own
    /// record, only when `delivered`. Left out when Wenlan has no record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivered_at: Option<i64>,
    /// How many rules used in every Space Wenlan writes into a tool's file.
    pub rule_count: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RulesDeliveryResponse {
    pub tools: Vec<ToolRulesDelivery>,
}
