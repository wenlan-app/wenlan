//! The frozen, minimized response contract for the query-only MCP profile.

use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use wenlan_types::{
    responses::SearchMemoryResponse, BriefReadResponse, BriefReadState, PageSourceWithMemory,
    SearchResult,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QueryHit {
    /// Opaque tool identifier for follow-up calls. Preserve it verbatim; it
    /// may contain a local path, so never use it as a citation label and
    /// never invent a URL for it. Cite `title` instead.
    pub source_id: String,
    /// Human-readable source title. Cite this value when referencing the hit.
    pub title: String,
    pub content: String,
    pub is_archived: bool,
    pub pending_revision: bool,
}

impl From<&SearchResult> for QueryHit {
    fn from(hit: &SearchResult) -> Self {
        Self {
            source_id: hit.source_id.clone(),
            title: hit.title.clone(),
            content: hit.content.clone(),
            is_archived: hit.is_archived,
            pending_revision: hit.pending_revision,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QueryRecallOutput {
    pub results: Vec<QueryHit>,
    pub supplemental_pages: Vec<QueryHit>,
}

pub fn project_recall(response: &SearchMemoryResponse) -> QueryRecallOutput {
    QueryRecallOutput {
        results: response.results.iter().map(QueryHit::from).collect(),
        supplemental_pages: response
            .supplemental_pages
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(QueryHit::from)
            .collect(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum QueryBriefState {
    SpaceNotResolved,
    BriefNotCreated,
    Ready,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QueryBriefItem {
    pub text: String,
    pub gate: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QueryBrief {
    pub last_session_summary: String,
    pub active: Vec<QueryBriefItem>,
    pub backlog: Vec<QueryBriefItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QueryRelatedContext {
    pub query: String,
    pub results: Vec<QueryHit>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QueryBriefOutput {
    pub state: QueryBriefState,
    pub space: Option<String>,
    pub brief: Option<QueryBrief>,
    pub related_context: Option<QueryRelatedContext>,
    /// Notes in the Space that rely on code that changed or went missing.
    /// `None` when no note in the Space relies on code.
    pub code_basis_pending: Option<u32>,
}

pub fn project_brief(response: &BriefReadResponse) -> Result<QueryBriefOutput, &'static str> {
    let brief = match response.state {
        BriefReadState::SpaceNotResolved | BriefReadState::BriefNotCreated => None,
        BriefReadState::Ready => {
            let brief = response
                .brief
                .as_ref()
                .ok_or("ready Brief response omitted brief")?;
            Some(QueryBrief {
                last_session_summary: brief.last_session_summary.clone(),
                active: brief
                    .active
                    .iter()
                    .map(|item| QueryBriefItem {
                        text: item.text.clone(),
                        gate: item.gate.clone(),
                    })
                    .collect(),
                backlog: brief
                    .backlog
                    .iter()
                    .map(|item| QueryBriefItem {
                        text: item.text.clone(),
                        gate: item.gate.clone(),
                    })
                    .collect(),
            })
        }
    };

    let related_context = matches!(response.state, BriefReadState::Ready)
        .then(|| response.related_context.as_ref())
        .flatten()
        .map(|related| QueryRelatedContext {
            query: related.query.clone(),
            results: related.results.iter().map(QueryHit::from).collect(),
        });

    Ok(QueryBriefOutput {
        state: match response.state {
            BriefReadState::SpaceNotResolved => QueryBriefState::SpaceNotResolved,
            BriefReadState::BriefNotCreated => QueryBriefState::BriefNotCreated,
            BriefReadState::Ready => QueryBriefState::Ready,
        },
        space: response.space.clone(),
        brief,
        related_context,
        // A Space with no Brief yet can still have notes that rely on code.
        code_basis_pending: matches!(
            response.state,
            BriefReadState::Ready | BriefReadState::BriefNotCreated
        )
        .then_some(response.code_basis_pending)
        .flatten(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QuerySourceMemory {
    /// Human-readable source title. Cite this value when referencing the source.
    pub title: String,
    pub content: String,
    pub is_archived: bool,
    pub pending_revision: bool,
}

impl From<&wenlan_types::MemoryItem> for QuerySourceMemory {
    fn from(memory: &wenlan_types::MemoryItem) -> Self {
        Self {
            title: memory.title.clone(),
            content: memory.content.clone(),
            is_archived: memory.is_archived,
            pending_revision: memory.pending_revision,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QueryPageSource {
    /// Opaque tool identifier for follow-up calls. Preserve it verbatim; it
    /// may contain a local path, so never use it as a citation label and
    /// never invent a URL for it. Cite the memory `title` instead.
    pub source_id: String,
    pub memory: Option<QuerySourceMemory>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QueryPageSourcesOutput {
    pub page_id: String,
    pub sources: Vec<QueryPageSource>,
}

pub fn project_page_sources(
    page_id: &str,
    sources: &[PageSourceWithMemory],
) -> QueryPageSourcesOutput {
    QueryPageSourcesOutput {
        page_id: page_id.to_string(),
        sources: sources
            .iter()
            // A missing memory may be outside the granted Space, not deleted.
            // Do not reveal its identifier through the public projection.
            .filter(|source| source.memory.is_some())
            .map(|source| QueryPageSource {
                source_id: source.source.memory_source_id.clone(),
                memory: source.memory.as_ref().map(QuerySourceMemory::from),
            })
            .collect(),
    }
}

fn schema_for<T: JsonSchema>() -> Arc<serde_json::Map<String, serde_json::Value>> {
    let value = serde_json::to_value(schemars::schema_for!(T))
        .expect("query-only DTO schema must serialize");
    Arc::new(
        value
            .as_object()
            .expect("query-only DTO schema must be an object")
            .clone(),
    )
}

pub fn output_schema(tool_name: &str) -> Option<Arc<serde_json::Map<String, serde_json::Value>>> {
    match tool_name {
        "brief" => Some(schema_for::<QueryBriefOutput>()),
        "recall" => Some(schema_for::<QueryRecallOutput>()),
        "get_page_sources" => Some(schema_for::<QueryPageSourcesOutput>()),
        _ => None,
    }
}
