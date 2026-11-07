// SPDX-License-Identifier: Apache-2.0
//! Source-backed comparison proposals and explicit human decisions.
use crate::{pages::PdfCitationTarget, source_lifecycle::SourceLifecycleSummary};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResearchDisputeKind {
    OpposingClaims,
    IncomparableConditions,
    DifferentDefinitions,
    InsufficientEvidence,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ResearchDisputeEvidence {
    pub source: String,
    pub source_id: String,
    pub source_digest: String,
    pub quote: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pdf_target: Option<PdfCitationTarget>,
    /// Authoritative immutable generation captured by the daemon at proposal time.
    /// Missing legacy generations require a new proposal before confirming evidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_lifecycle: Option<SourceLifecycleSummary>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResearchDispute {
    pub space_id: String,
    pub kind: ResearchDisputeKind,
    pub conditions: String,
    pub reason: String,
    pub evidence: Vec<ResearchDisputeEvidence>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposeResearchDisputeRequest {
    pub space: String,
    pub kind: ResearchDisputeKind,
    pub conditions: String,
    pub reason: String,
    pub evidence: Vec<ResearchDisputeEvidence>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResearchDisputeProposalResult {
    pub id: String,
    pub created: bool,
    /// The original stored payload digest, even when a rerun supplied another reason.
    pub payload_digest: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResearchDisputeDecision {
    ConfirmDifference,
    NotComparable,
    CorrectInterpretation,
    Defer,
    Close,
}
impl ResearchDisputeDecision {
    pub fn confirms_evidence(&self) -> bool {
        !matches!(self, Self::Defer | Self::Close)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecideResearchDisputeRequest {
    pub space: String,
    pub expected_payload_digest: String,
    pub decision: ResearchDisputeDecision,
    pub reason: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResearchEvidenceState {
    Unchanged,
    Changed,
    Unavailable,
    Historical,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResearchDisputeSourceState {
    pub source: String,
    pub source_id: String,
    pub expected_digest: String,
    pub current_digest: Option<String>,
    pub expected_pdf_digest: Option<String>,
    pub current_pdf_digest: Option<String>,
    #[serde(default)]
    pub expected_lifecycle: Option<SourceLifecycleSummary>,
    #[serde(default)]
    pub current_lifecycle: Option<SourceLifecycleSummary>,
    pub state: ResearchEvidenceState,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResearchDisputeDecisionEvent {
    pub decision: ResearchDisputeDecision,
    pub reason: String,
    pub actor: String,
    pub decided_at: i64,
    pub source_states: Vec<ResearchDisputeSourceState>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResearchDisputeView {
    /// Current registered name, resolved from immutable space_id.
    pub space: String,
    pub id: String,
    pub status: String,
    pub dispute: ResearchDispute,
    pub payload_digest: String,
    pub source_states: Vec<ResearchDisputeSourceState>,
    pub decisions: Vec<ResearchDisputeDecisionEvent>,
}
