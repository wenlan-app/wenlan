// SPDX-License-Identifier: Apache-2.0
//! Requests and proposals for skills in a person's tool setup.

use serde::{Deserialize, Serialize};

use crate::{rulesync::RulesSyncReport, skill_registry::SkillPreviewFile};

pub const MAX_SAVED_SKILL_FILES: usize = 100;
pub const MAX_SAVED_SKILL_FILE_BYTES: usize = 256 * 1024;
pub const MAX_SAVED_SKILL_BYTES: usize = 512 * 1024;
pub const PENDING_SKILLS_COMMAND: &str = "wenlan skills pending";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveSkillRequest {
    pub name: String,
    pub description: String,
    /// The Markdown after the skill's front matter.
    pub body: String,
    #[serde(default)]
    pub files: Vec<SavedSkillFile>,
    #[serde(default)]
    pub scope: SavedSkillScope,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedSkillFile {
    pub path: String,
    pub text: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SavedSkillScope {
    #[default]
    Shared,
    Tool {
        tool: SavedSkillTool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SavedSkillTool {
    ClaudeCode,
    Codex,
    GeminiCli,
    Antigravity,
    CopilotCli,
    Cursor,
    OpenCode,
    MuseCode,
}

impl SavedSkillTool {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::ClaudeCode => "Claude Code",
            Self::Codex => "Codex",
            Self::GeminiCli => "Gemini CLI",
            Self::Antigravity => "Antigravity",
            Self::CopilotCli => "GitHub Copilot CLI",
            Self::Cursor => "Cursor",
            Self::OpenCode => "OpenCode",
            Self::MuseCode => "Muse Code",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillProposal {
    /// Identifies the exact text, scope, and origin the person reviewed.
    pub id: String,
    pub name: String,
    pub origin: SkillProposalOrigin,
    pub scope: SavedSkillScope,
    pub files: Vec<SkillPreviewFile>,
    /// Present when `files` contains only changed paths from a managed skill.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changes: Option<SkillProposalChanges>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillProposalChanges {
    /// Paths removed from the managed skill; added and changed paths are in `files`.
    pub removed: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub oversized: Vec<OversizedSkillFile>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub permissions: Vec<SkillPermissionChange>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillPermissionChange {
    pub path: String,
    pub before: String,
    pub after: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OversizedSkillFile {
    pub path: String,
    pub before: Option<SkillFileDigest>,
    pub after: SkillFileDigest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillFileDigest {
    pub bytes: usize,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillDeliveryPreview {
    pub token: String,
    pub files: Vec<SkillDeliveryChange>,
    pub blocked: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cleared_overrides: Vec<SavedSkillTool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillDeliveryChange {
    /// Home-relative destination, including the tool's folder.
    pub path: String,
    /// The previous spelling of a path on a case-insensitive volume.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub renamed_from: Option<String>,
    pub before: Option<SkillDeliveryFile>,
    pub after: Option<SkillDeliveryFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillDeliveryFile {
    pub digest: SkillFileDigest,
    pub permissions: String,
    /// Directory entries also appear in the existing destination list.
    #[serde(default, skip_serializing_if = "is_false")]
    pub directory: bool,
}

fn is_false(value: &bool) -> bool {
    !value
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SkillProposalOrigin {
    Agent { agent: String },
    Tool { tool: SavedSkillTool },
    Delivery { tool: SavedSkillTool },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SaveSkillResult {
    Saved {
        name: String,
        delivery: Option<RulesSyncReport>,
    },
    Proposed {
        proposal: SkillProposal,
    },
    ReviewPreview {
        proposal: SkillProposal,
        delivery: SkillDeliveryPreview,
    },
    Dismissed {
        name: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillProposalsResponse {
    pub proposals: Vec<SkillProposal>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillReviewRequest {
    pub id: String,
    pub action: SkillReviewAction,
    /// Choose the original tool or shared scope when accepting a tool's change.
    #[serde(default)]
    pub scope: Option<SavedSkillScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillReviewAction {
    Preview,
    Accept,
    Dismiss,
}
