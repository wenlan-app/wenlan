// SPDX-License-Identifier: Apache-2.0
//! Explicit retained-source status and the historical references affected by it.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SourceLifecycleState {
    #[default]
    Active,
    Excluded,
    Retracted,
    Removed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceLifecycleRecord {
    pub space_id: String,
    pub source: String,
    pub source_id: String,
    pub revision: u64,
    pub state: SourceLifecycleState,
    pub reason: String,
    pub actor: String,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetSourceLifecycleRequest {
    pub space: String,
    pub source: String,
    pub source_id: String,
    pub expected_revision: u64,
    pub state: SourceLifecycleState,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceLifecycleImpact {
    pub page_id: String,
    pub page_version: i64,
    pub source_revision: i64,
    pub source_ids: Vec<String>,
    pub occurrences: Vec<u32>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceLifecycleResponse {
    pub record: SourceLifecycleRecord,
    /// Append-only decisions, including active restorations; a restoration alone
    /// does not certify any old claim or resolve its pending review.
    pub history: Vec<SourceLifecycleRecord>,
    /// Recorded effects of decisions that were actually applied.
    pub impacts: Vec<SourceLifecycleImpact>,
    /// Read-only prospective effects, populated by GET before a decision.
    #[serde(default)]
    pub preview_impacts: Vec<SourceLifecycleImpact>,
}

/// Scoped resolution of a retained document/chunk locator; never reattaches it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceReferenceIdentity {
    pub space_id: String,
    pub source: String,
    pub source_id: String,
    pub source_agent: Option<String>,
    pub title: String,
}

/// Current eligibility metadata, independent of content SHA256 and historical readability.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceLifecycleSummary {
    pub state: SourceLifecycleState,
    pub revision: u64,
    pub reason: String,
}
