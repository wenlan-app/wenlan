// SPDX-License-Identifier: Apache-2.0
//! Personal rules and skills setup shared by the CLI and daemon.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdoptionPlan {
    /// Binds approval to the exact files seen by the preview.
    pub token: String,
    pub files: Vec<AdoptionChange>,
    #[serde(default)]
    pub skills: Vec<AdoptionSkill>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdoptionSkill {
    pub path: String,
    #[serde(flatten)]
    pub outcome: AdoptionSkillOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub byte_copy_reason: Option<String>,
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdoptPreviewRequest {
    /// The CLI's expected local tool home.
    pub home: std::path::PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdoptRequest {
    pub token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<std::path::PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RulesSyncReport {
    pub written: Vec<String>,
    pub blocked: Vec<String>,
    pub proposals: Vec<PullbackCard>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullbackCard {
    pub id: String,
    pub files: Vec<String>,
    pub diff: String,
    pub reason: String,
}

/// The selected action and source as shown to the person before approval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullbackPreview {
    pub diff: String,
    pub plan: String,
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
