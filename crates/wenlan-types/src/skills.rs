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
    /// The person asked for this rule to be saved, in an AI tool or with
    /// `wenlan rules add`: where the save came from, the tool's name or
    /// [`SAVED_FROM_TERMINAL`]. It counts at once like a hand-written rule,
    /// and is never shown as written by the person: Wenlan cannot prove who
    /// spoke, only where the save came from (UX-SKILL-14).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved_from: Option<String>,
}

/// `SkillLine::saved_from` for a save that named no AI tool.
pub const SAVED_FROM_TERMINAL: &str = "terminal";

/// The longest rule one save may carry, in characters.
pub const SAVED_RULE_MAX_CHARS: usize = 300;

/// How many rules one tool may save in a day.
pub const SAVED_RULES_PER_TOOL_PER_DAY: usize = 10;

/// How many rules all callers together may save in a day.
pub const SAVED_RULES_PER_DAY: usize = 30;

impl SkillLine {
    /// "saved from Claude Code" / "saved from the terminal", for a rule the
    /// person asked to be saved; `None` for every other line.
    pub fn saved_from_label(&self) -> Option<String> {
        self.saved_from.as_deref().map(saved_from_label)
    }
}

/// How a `saved_from` value reads in a sentence.
pub fn saved_from_label(saved_from: &str) -> String {
    if saved_from == SAVED_FROM_TERMINAL {
        "saved from the terminal".to_string()
    } else {
        format!("saved from {saved_from}")
    }
}

/// `POST /api/skills/rules`: save one rule the person explicitly asked for.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveRuleRequest {
    /// The rule, one line, at most [`SAVED_RULE_MAX_CHARS`] characters.
    pub text: String,
    /// `rule` when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<SkillLineKind>,
    /// The Space the rule is for. The `x-wenlan-space` header says the same.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space: Option<String>,
    /// Save it for every Space. Never assumed: with neither this nor a Space
    /// the save is refused.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub every_space: bool,
    /// The person's own words asking for the save. Kept in the page's history.
    pub person_asked: String,
}

/// What a save or a removal did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SavedRuleStatus {
    /// The rule counts now; AI tools are told it from now on.
    Saved,
    /// The rule is no longer told to AI tools.
    Removed,
    /// The page is locked: the change waits as a card for the person to
    /// accept in the Wenlan app, and nothing changed yet.
    Waiting,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveRuleResponse {
    pub status: SavedRuleStatus,
    /// The saved rule's id, as rules lists carry it and removing takes it;
    /// absent while waiting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_id: Option<String>,
    /// The Space the rule is for; empty for a rule used in every Space.
    pub space_name: String,
    /// The "Rules I saved" page that holds it.
    pub page_id: String,
    pub page_title: String,
    /// Where the save came from, as rules lists show it.
    pub saved_from: String,
    /// The card to accept in the Wenlan app, while waiting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision_card_id: Option<String>,
    /// One sentence for the person: what was saved and where.
    pub message: String,
}

/// `POST /api/skills/rules/{id}/remove`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoveRuleRequest {
    /// The person's own words asking for the removal.
    pub person_asked: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoveRuleResponse {
    pub status: SavedRuleStatus,
    pub id: String,
    pub page_id: String,
    pub page_title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision_card_id: Option<String>,
    pub message: String,
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

#[cfg(test)]
mod saved_rule_tests {
    use super::*;

    #[test]
    fn ux_skill_14_a_line_without_saved_from_still_reads_and_stays_small() {
        // Lines written before the field existed carry none.
        let old = r#"{"id":"a","kind":"rule","text":"t","source_page_id":"p",
            "source_page_title":"T","source_revision":1,"source_revision_at":0,
            "source_quote":"t"}"#;
        let line: SkillLine = serde_json::from_str(old).unwrap();
        assert_eq!(line.saved_from, None);
        assert_eq!(line.saved_from_label(), None);
        assert!(!serde_json::to_string(&line).unwrap().contains("saved_from"));
    }

    #[test]
    fn ux_skill_14_the_label_names_the_tool_or_the_terminal_never_the_person() {
        assert_eq!(saved_from_label("Claude Code"), "saved from Claude Code");
        assert_eq!(
            saved_from_label(SAVED_FROM_TERMINAL),
            "saved from the terminal"
        );
        for label in [
            saved_from_label("Codex"),
            saved_from_label(SAVED_FROM_TERMINAL),
        ] {
            assert!(!label.contains("written by you"), "{label}");
        }
    }

    #[test]
    fn ux_skill_14_a_save_request_never_means_every_space_by_default() {
        let request: SaveRuleRequest =
            serde_json::from_str(r#"{"text":"We release on Tuesdays","person_asked":"save it"}"#)
                .unwrap();
        assert!(!request.every_space);
        assert_eq!(request.space, None);
        assert_eq!(request.kind, None);
    }
}
