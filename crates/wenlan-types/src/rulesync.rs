// SPDX-License-Identifier: Apache-2.0
//! Personal rules and skills setup shared by the CLI and daemon.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::{rulesync_settings::SettingsPlan, rulesync_skill::SkillProposal};

pub const BRING_IN_COMMAND: &str = "wenlan rules adopt";
pub const STATUS_COMMAND: &str = "wenlan rules status";
pub const SYNC_COMMAND: &str = "wenlan rules status --sync";
pub const TO_REVIEW_COMMAND: &str = "wenlan rules review";
pub const RESTORE_COMMAND: &str = "wenlan rules restore";

/// Localizable setup diagnostics. Legacy terminal copy is carried separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RulesSetupCode {
    ScopeEdit,
    CompetingRules,
    UnverifiedRule,
    PersonalFileEdit,
    CompetingPersonalFiles,
    RestoreReview,
    ShareRule,
    KeepRule,
    PersonalOnly,
    PersonalShare,
    KeepEdit,
    KeepRemoval,
    CreateCopy,
    RemoveCopy,
    ReplaceCopy,
    KeepCopy,
    SkipToolOnly,
    SkipIncompatible,
    SkipDestination,
    SkipNoCopy,
    WiderRead,
    GitDelivery,
    PermissionsChanged,
    LineEndingsChanged,
    CreateEmpty,
    RemoveEmpty,
    NoFinalNewline,
    ToolMissing,
    PersonalName,
    PersonalUnreadable,
    LegacyWorkflow,
    RuleUnavailable,
    LinkedRule,
    SkillUnreadable,
    LinkedSkillsRoot,
    SkillNameEncoding,
    LinkedSkill,
    SkillEntryMissing,
    SharedSkill,
    SkillPreviewFailed,
    SkillByteCopy,
    PersonalByteCopy,
    PersonalImport,
    EntryLink,
    EntryBinary,
    EntryDirectory,
    OfflineInventory,
    AddTools,
    CodexScript,
    SharedUntouched,
    PersonalImportNotes,
    IgnoreUnsupported,
    GitRule,
    GitSkills,
    GitPersonalFile,
    GitSharedSkills,
    CheckoutNeedsReview,
    OriginalMissing,
    OriginalChanged,
    OriginalUnverified,
    RecoveryNeeded,
    NameConflict,
    SharedReadOnly,
    RestoreBlocked,
    ChangedSinceDelivery,
    ChangedDuringRestore,
    ChangedDuringSync,
    GitUnreviewed,
    RuleConflict,
    RenderUnavailable,
    RenderMissing,
    ReviewNeeded,
    ReadFailed,
    PersonalPreviewFailed,
    RuleEncoding,
    SharedRuleChanged,
    RuleCompareFailed,
    MetadataChanged,
    SkillUnmanaged,
    SkillDuplicate,
    SkillDestinationExists,
    DestinationPermissions,
    SkillMasterMissing,
    SkillDismissed,
    SkillNameInvalid,
    SkillReviewInterrupted,
    SkillProposalFailed,
    SkillQueueFull,
    AdoptRequired,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RulesSetupNotice {
    pub code: RulesSetupCode,
    /// Tool display names from the catalog, in display order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,
}

impl From<RulesSetupCode> for RulesSetupNotice {
    fn from(code: RulesSetupCode) -> Self {
        Self {
            code,
            tools: Vec::new(),
        }
    }
}

impl RulesSetupNotice {
    pub fn for_tool(code: RulesSetupCode, tool: &str) -> Self {
        Self {
            code,
            tools: vec![tool.into()],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdoptionPlan {
    #[serde(default)]
    pub plan_id: String,
    /// Binds approval to the exact files seen by the preview.
    pub token: String,
    /// Nonempty lines in the shared rules once, plus each tool's own overlay.
    /// Absent when the daemon predates this count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_lines: Option<usize>,
    pub files: Vec<AdoptionChange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<SettingsPlan>,
    #[serde(default)]
    pub skills: Vec<AdoptionSkill>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    /// Settings intentionally left in their native tool during adoption.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kept_in_place: Vec<KeptInPlace>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub note_codes: Vec<RulesSetupNotice>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<AdoptionTool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped_tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped_tool_codes: Vec<RulesSetupNotice>,
    /// Tools the first sync will write files for that do not exist yet.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub new_files: Vec<AdoptionNewFiles>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdoptionNewFiles {
    pub tool: String,
    pub files: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdoptionTool {
    pub name: String,
    pub rule_files: usize,
    pub skills: usize,
    pub other_files: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdoptionSkill {
    pub path: String,
    #[serde(flatten)]
    pub outcome: AdoptionSkillOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub byte_copy_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<RulesSetupTool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<RulesSetupCode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub byte_copy_reason_code: Option<RulesSetupCode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum AdoptionSkillOutcome {
    TrackedUpstream { upstream: SkillUpstream },
    PlainCopy,
    Skipped { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SkillUpstream {
    GitHub {
        repository: String,
        /// Empty for a skill in the repository root.
        directory: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        revision: Option<String>,
        /// Home-relative checkout for local tracking if the remote requires authentication.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        checkout: Option<std::path::PathBuf>,
    },
    Npm {
        package: String,
        version: String,
        directory: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdoptedSkillUpdate {
    pub name: String,
    pub upstream: SkillUpstream,
    pub status: SkillUpdateStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SkillUpdateStatus {
    Current,
    Available { revision: String },
    Unavailable { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdoptionChange {
    pub path: String,
    pub added: usize,
    pub removed: usize,
    pub diff: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<RulesSetupTool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notice_codes: Vec<RulesSetupCode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_diff: Option<AdoptionTextDiff>,
}

/// Plain file changes can reuse the terminal diff. Entries with synthetic
/// descriptions provide a separate diff of the underlying text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AdoptionTextDiff {
    FileText,
    Extracted { diff: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdoptPreviewRequest {
    /// The CLI's expected local tool home.
    pub home: std::path::PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdoptRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmed_plan_id: Option<String>,
    pub token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<std::path::PathBuf>,
    /// The person also said yes to sharing the servers that may hold a key.
    /// The server ignores it on the AI confirm route and for agent sessions.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub include_keys: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharePreviewRequest {
    pub home: std::path::PathBuf,
    /// An empty list includes rule lines with no opt-outs. An omitted field
    /// keeps the item-only preview for clients that cannot display rule lines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_rule: Option<Vec<usize>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareRequest {
    #[serde(flatten)]
    pub approval: AdoptRequest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_rule: Option<Vec<usize>>,
}

/// What `wenlan rules share` would copy into the other managed tools.
/// Approval binds to `token`; an AI session confirms with `plan_id`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharingPlan {
    pub plan_id: String,
    pub token: String,
    /// Tools that would receive something, with the names they get.
    #[serde(default)]
    pub tools: Vec<SharingTool>,
    /// Items that stay where they are, one plain reason each.
    #[serde(default)]
    pub stays: Vec<SharingStay>,
    /// Numbered rule lines, including any the person chose to keep in place.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<SharingRule>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rule_stays: Vec<SharingRuleStay>,
    /// Copies another tool already has with the same content.
    #[serde(default)]
    pub already_there: usize,
    /// Servers that may hold a key. Only a person can share them, with a
    /// second yes. Names only; no value is ever part of a plan.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub key_servers: Vec<String>,
    /// Tools those servers would be copied into.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub key_tools: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharingTool {
    pub tool: String,
    #[serde(default)]
    pub rules: usize,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub commands: Vec<String>,
    #[serde(default)]
    pub subagents: Vec<String>,
    #[serde(default)]
    pub mcp_servers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharingRule {
    pub number: usize,
    pub line: String,
    pub tools: Vec<String>,
    pub kept: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharingRuleStay {
    pub tool: String,
    /// Omitted for a possible credential.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<String>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharingStay {
    /// The tool the item stays in.
    pub tool: String,
    pub item: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RulesSyncReport {
    pub written: Vec<String>,
    pub blocked: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked_codes: Vec<RulesSetupNotice>,
    /// Settings intentionally left in their native tool during sync.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kept_in_place: Vec<KeptInPlace>,
    pub proposals: Vec<PullbackCard>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeptInPlace {
    pub tool: String,
    pub count: usize,
}

impl RulesSyncReport {
    pub fn block(&mut self, code: impl Into<RulesSetupNotice>, message: impl Into<String>) {
        self.blocked.push(message.into());
        self.blocked_codes.push(code.into());
    }

    pub fn extend_blocks(
        &mut self,
        code: RulesSetupCode,
        messages: impl IntoIterator<Item = String>,
    ) {
        for message in messages {
            self.block(code, message);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullbackCard {
    pub id: String,
    pub files: Vec<String>,
    pub diff: String,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<RulesSetupCode>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diff_codes: Vec<RulesSetupCode>,
    #[serde(default)]
    pub kind: PullbackKind,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PullbackKind {
    Scope,
    #[default]
    Conflict,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RulesSetupStatus {
    pub enabled: bool,
    pub adopted: bool,
    pub can_adopt: bool,
    pub can_restore: bool,
    pub in_sync: bool,
    pub proposals: Vec<PullbackCard>,
    /// Source files for these cards, with their actual owning tool.
    pub review_tools: BTreeMap<String, RulesSetupTool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_checked_at: Option<i64>,
    #[serde(default)]
    pub tools: Vec<RulesToolStatus>,
    /// Plain lines about shared servers Wenlan left as they are.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RulesSetupTool {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RulesToolStatus {
    #[serde(flatten)]
    pub tool: RulesSetupTool,
    pub state: RulesToolState,
    pub counts: RulesToolCounts,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RulesToolState {
    UpToDate,
    NeedsReview,
    #[default]
    NotManaged,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RulesToolCounts {
    pub rules: usize,
    pub skills: usize,
    pub commands: usize,
    pub subagents: usize,
    pub mcp: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RulesReviewList {
    pub items: Vec<RulesReviewItem>,
    pub review_tools: BTreeMap<String, RulesSetupTool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RulesReviewItem {
    pub id: String,
    pub answers: Vec<PullbackAction>,
    #[serde(flatten)]
    pub content: RulesReviewContent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RulesReviewContent {
    ToolEdit { card: PullbackCard },
    SkillProposal { proposal: SkillProposal },
    SkillUpdate { update: AdoptedSkillUpdate },
    Settings { plan: SettingsPlan },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DismissSkillUpdate {
    pub id: String,
}

/// The selected action and source as shown to the person before approval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullbackPreview {
    pub diff: String,
    pub plan: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plan_codes: Vec<RulesSetupNotice>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diff_codes: Vec<RulesSetupCode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullbackDecision {
    pub id: String,
    pub action: PullbackAction,
    /// The chosen alternative when several files changed the same rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PullbackAction {
    ToolOnly,
    AllTools,
    Dismiss,
    Revert,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CheckoutRulesRequest {
    pub repo: std::path::PathBuf,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CheckoutRulesReceipt {
    pub directory: std::path::PathBuf,
    pub report: RulesSyncReport,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProjectTeamRulesRequest {
    pub team: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedRulesProposalRequest {
    pub markdown: String,
}
