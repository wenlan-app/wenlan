// SPDX-License-Identifier: AGPL-3.0-only
//! The five read tools of the team library. The host supplies a bound reader;
//! dispatch here has no path to the caller's or host's personal library.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use rmcp::model::{CallToolResult, ContentBlock, ErrorData as McpError, Tool};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value};
use wenlan_types::team_library::{TeamBrief, TeamItem, TeamPageSources, TeamPages, TeamRecall};

pub type ReadFuture = Pin<Box<dyn Future<Output = Result<Output, &'static str>> + Send>>;
pub type Lookup = Arc<dyn Fn(Read) -> ReadFuture + Send + Sync>;

pub enum Read {
    Page(String),
    Sources(String),
    Pages {
        space: Option<String>,
        limit: usize,
        offset: usize,
    },
    Recall {
        space: Option<String>,
        query: String,
        limit: usize,
    },
    Brief {
        space: Option<String>,
        topic: Option<String>,
    },
}

pub enum Output {
    Page(Box<TeamItem>),
    Sources(TeamPageSources),
    Pages(TeamPages),
    Recall(TeamRecall),
    Brief(Box<TeamBrief>),
}

#[derive(Deserialize, JsonSchema)]
struct Page {
    /// The page ID from this team's library.
    page_id: String,
}

#[derive(Deserialize, JsonSchema)]
struct List {
    /// Narrow to a Space by name. Omit for all pages the connection can read.
    space: Option<String>,
    /// Maximum pages, 1 to 200. Default 50.
    limit: Option<usize>,
    offset: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
struct Recall {
    /// Keywords to match in the permitted team pages and memories.
    #[serde(alias = "topic")]
    query: String,
    /// Narrow to a Space by name.
    space: Option<String>,
    limit: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
struct Brief {
    /// Narrow to a Space by name. Omit for the team Brief.
    space: Option<String>,
    /// Optional keywords for related team items.
    topic: Option<String>,
}

fn tool<T: JsonSchema>(name: &'static str, description: &'static str, pinned: bool) -> Tool {
    let schema = serde_json::to_value(schemars::schema_for!(T)).expect("team tool schema");
    let mut schema = schema
        .as_object()
        .expect("team tool schema is an object")
        .clone();
    if pinned {
        if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) {
            properties.remove("space");
        }
    }
    Tool::new(name, description, Arc::new(schema))
}

pub(super) fn definitions(pinned: bool) -> Vec<Tool> {
    vec![
        tool::<Recall>(
            "recall",
            "Find pages and enabled memory snapshots in this team's library.",
            pinned,
        ),
        tool::<Brief>(
            "brief",
            "Read the published team or Space Brief and any matching team items.",
            pinned,
        ),
        tool::<List>(
            "list_pages",
            "List pages that this connection can read in the team library.",
            pinned,
        ),
        tool::<Page>(
            "read_page",
            "Read a team page and its publication attribution.",
            pinned,
        ),
        tool::<Page>(
            "get_page_sources",
            "Read approved team sources and private-source markers for a team page.",
            pinned,
        ),
    ]
}

fn parse<T: serde::de::DeserializeOwned>(args: Value) -> Result<T, McpError> {
    serde_json::from_value(args)
        .map_err(|_| McpError::invalid_params("Invalid team tool arguments.", None))
}

pub(super) async fn dispatch(
    lookup: &Lookup,
    name: &str,
    args: Option<Map<String, Value>>,
) -> Result<CallToolResult, McpError> {
    let args = Value::Object(args.unwrap_or_default());
    let request = match name {
        "recall" => {
            let p: Recall = parse(args)?;
            if p.query.len() > 4_096 {
                return Err(McpError::invalid_params(
                    "The search text is too long.",
                    None,
                ));
            }
            Read::Recall {
                space: p.space,
                query: p.query,
                limit: p.limit.unwrap_or(10),
            }
        }
        "brief" => {
            let p: Brief = parse(args)?;
            if p.topic.as_ref().is_some_and(|topic| topic.len() > 4_096) {
                return Err(McpError::invalid_params(
                    "The search text is too long.",
                    None,
                ));
            }
            Read::Brief {
                space: p.space,
                topic: p.topic,
            }
        }
        "list_pages" => {
            let p: List = parse(args)?;
            Read::Pages {
                space: p.space,
                limit: p.limit.unwrap_or(50),
                offset: p.offset.unwrap_or(0),
            }
        }
        "read_page" => Read::Page(parse::<Page>(args)?.page_id),
        "get_page_sources" => Read::Sources(parse::<Page>(args)?.page_id),
        _ => {
            return Ok(CallToolResult::error(vec![ContentBlock::text(
                "This connection reads the team library.",
            )]))
        }
    };
    match lookup(request).await {
        Ok(Output::Page(page)) => super::hosted_team::structured_result(page),
        Ok(Output::Sources(sources)) => super::hosted_team::structured_result(sources),
        Ok(Output::Pages(pages)) => super::hosted_team::structured_result(pages),
        Ok(Output::Recall(recall)) => super::hosted_team::structured_result(recall),
        Ok(Output::Brief(brief)) => super::hosted_team::structured_result(brief),
        Err(error) => Ok(CallToolResult::error(vec![ContentBlock::text(error)])),
    }
}
