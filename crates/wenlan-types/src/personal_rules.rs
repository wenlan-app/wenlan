// SPDX-License-Identifier: Apache-2.0
//! Personal rule plans and their recorded origins.

use serde::{Deserialize, Serialize};

/// The caller must choose a layer; an absent layer is not a default.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PersonalRuleLayer {
    Shared,
    Tool { tool: String },
}

/// A line's origin, when the evidence for that line is still available.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersonalRuleOrigin {
    pub author: Option<String>,
    pub proposer: Option<String>,
    pub confirmer: Option<String>,
    pub layer: PersonalRuleLayer,
    /// Unix time in seconds, or null for an unknown earlier version.
    pub time: Option<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pages: Vec<PersonalRulePageOrigin>,
    /// A page also supplied an existing shared line. Removing the page's
    /// contribution must leave the original shared line in place.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub kept_shared: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersonalRulePageOrigin {
    pub location: PageRuleLocation,
    pub rule_id: String,
    pub author: Option<String>,
    pub proposer: String,
    pub confirmer: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PageRuleLocation {
    /// Conversation preferences have a line position but no page.
    pub page_id: Option<String>,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageRuleMergePreview {
    pub rule_count: usize,
    pub excluded: Vec<PageRuleLocation>,
    pub excludes_space_only: bool,
}

/// Origin records are bound to a whole layer's exact content. If another
/// writer changes that content, stale records must not be used as attribution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersonalRuleLayerOrigins {
    pub layer: PersonalRuleLayer,
    pub content_digest: String,
    /// One entry per line, in order; null means unknown, not AI-authored.
    pub lines: Vec<Option<PersonalRuleOrigin>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersonalRuleShowRequest {
    pub layer: PersonalRuleLayer,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonalRuleShow {
    pub layer: PersonalRuleLayer,
    pub text: String,
    pub origins: Vec<Option<PersonalRuleOrigin>>,
    pub affected_tools: Vec<String>,
}

/// The rules every tool gets, and each tool's own lines, as they stand now.
/// Read-only: changing them is a plan the person applies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersonalRulesOverview {
    pub shared: String,
    pub tools: Vec<PersonalRuleToolText>,
}

/// One tool's own lines. `tool` is the tool's agent id (for example
/// `claude-code`); tools with no lines of their own are left out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersonalRuleToolText {
    pub tool: String,
    pub text: String,
}

/// Replacement text for one named layer. A proposal may contain several.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersonalRuleEdit {
    pub layer: PersonalRuleLayer,
    pub text: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersonalRuleProposeRequest {
    pub edits: Vec<PersonalRuleEdit>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersonalRulePlanChange {
    pub layer: PersonalRuleLayer,
    pub before: String,
    pub after: String,
    pub diff: String,
    pub affected_tools: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersonalRulePlan {
    pub id: String,
    /// Unix time in seconds; the daemon enforces expiry when loading a plan.
    pub expires_at: i64,
    pub proposer: Option<String>,
    pub changes: Vec<PersonalRulePlanChange>,
    pub blocked_tools: Vec<String>,
    /// Named or previously enrolled tools that will not receive this change.
    #[serde(default)]
    pub unmanaged_tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_merge: Option<PageRuleMergePreview>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersonalRuleApplyRequest {
    pub plan_id: String,
    /// The plan exactly as the person saw it. When given, the apply is
    /// refused unless the waiting plan is still that one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_plan: Option<PersonalRulePlan>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonalRuleVersionSummary {
    pub id: String,
    pub time: Option<i64>,
    pub cause: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub author: Option<String>,
    pub proposer: Option<String>,
    pub confirmer: Option<String>,
    pub layers: Vec<PersonalRuleLayer>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonalRuleVersion {
    pub summary: PersonalRuleVersionSummary,
    pub rules: Vec<PersonalRuleShow>,
}
