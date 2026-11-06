// SPDX-License-Identifier: Apache-2.0
//! Durable, exact-scope external-AI organizing contracts.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OrganizeRules {
    pub space_id: String,
    pub revision: u64,
    pub name: String,
    pub instructions: String,
    pub content_digest: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveOrganizeRulesRequest {
    pub space: String,
    pub expected_revision: u64,
    pub name: String,
    pub instructions: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResearchSourceRef {
    pub source: String,
    pub source_id: String,
    pub title: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResearchCoverage {
    Pending,
    Reading,
    Complete,
    Failed,
    Unavailable,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OrganizeRunItem {
    #[serde(flatten)]
    pub source_ref: ResearchSourceRef,
    pub coverage: ResearchCoverage,
    pub content_digest: String,
    pub decision_revision: u64,
    /// Frozen eligibility generation; initial/legacy sources are generation zero.
    #[serde(default)]
    pub lifecycle_revision: u64,
    /// Server-recorded contiguous Unicode character prefix actually returned by run-bound reads.
    #[serde(default)]
    pub served_chars: usize,
    #[serde(default)]
    pub served_total_chars: Option<usize>,
    pub next_offset: Option<usize>,
    pub error: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OrganizeRunState {
    Prepared,
    InProgress,
    Partial,
    AwaitingReview,
    Completed,
    Cancelled,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OrganizeRun {
    pub id: String,
    pub space_id: String,
    pub revision: u64,
    pub rules: OrganizeRules,
    pub items: Vec<OrganizeRunItem>,
    pub target_page_id: Option<String>,
    pub target_page_version: Option<i64>,
    #[serde(default)]
    pub target_page_incarnation: Option<String>,
    pub result_page_version: Option<i64>,
    pub pending_revision_id: Option<String>,
    pub state: OrganizeRunState,
    pub created_at: i64,
    pub updated_at: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateOrganizeRunRequest {
    pub space: String,
    pub sources: Vec<ResearchSourceRef>,
    pub instructions: Option<String>,
    pub target_page_id: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateOrganizeRunRequest {
    pub space: String,
    pub expected_revision: u64,
    pub source: Option<String>,
    pub source_id: Option<String>,
    pub coverage: Option<ResearchCoverage>,
    pub content_digest: Option<String>,
    pub next_offset: Option<usize>,
    pub error: Option<String>,
    #[serde(default)]
    pub cancel: bool,
    #[serde(default)]
    pub resume: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResearchRunsResponse {
    pub runs: Vec<OrganizeRun>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TagDecision {
    pub tag: String,
    pub present: bool,
    pub actor: String,
    pub decision_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SourceClassificationDecisions {
    pub revision: u64,
    pub decisions: Vec<TagDecision>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SourceClassificationChange {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub before_tags: Vec<String>,
    pub source: String,
    pub source_id: String,
    pub source_content_digest: String,
    pub expected_decision_revision: u64,
    pub add_tags: Vec<String>,
    pub remove_tags: Vec<String>,
    pub reason: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposeSourceClassificationsRequest {
    pub space: String,
    pub run_id: String,
    pub changes: Vec<SourceClassificationChange>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassificationProposalResult {
    pub id: String,
    pub created: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MergeResearchTagsRequest {
    pub space: String,
    pub sources: Vec<ResearchSourceRef>,
    pub from_tag: String,
    pub into_tag: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeResearchTagsResponse {
    pub updated: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadOrganizeSourceRequest {
    pub space: String,
    pub source: String,
    pub source_id: String,
    #[serde(default)]
    pub offset: usize,
    #[serde(default = "default_read_limit")]
    pub limit: usize,
}
fn default_read_limit() -> usize {
    8000
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadOrganizeSourceResponse {
    pub run_id: String,
    pub run_revision: u64,
    pub source: String,
    pub source_id: String,
    pub title: String,
    pub space: String,
    pub text: String,
    pub content_digest: String,
    pub source_lifecycle: crate::source_lifecycle::SourceLifecycleSummary,
    pub offset: usize,
    pub total_chars: usize,
    pub next_offset: Option<usize>,
    /// This page reaches the end; served_chars separately records contiguous coverage.
    pub complete: bool,
    pub served_chars: usize,
}
