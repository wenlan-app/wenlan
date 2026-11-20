// SPDX-License-Identifier: Apache-2.0
//! Personal rules and skills setup shared by the CLI and daemon.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdoptionPlan {
    /// Binds approval to the exact files seen by the preview.
    pub token: String,
    pub files: Vec<AdoptionChange>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdoptionChange {
    pub path: String,
    pub added: usize,
    pub removed: usize,
    pub diff: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdoptRequest {
    pub token: String,
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
