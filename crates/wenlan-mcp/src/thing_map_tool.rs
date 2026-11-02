// SPDX-License-Identifier: Apache-2.0
//! The `thing_map` tool: the AI tool keeps the map of what a conversation is
//! working through (UX-VIEW-7), which the thing's recap reads too
//! (UX-VIEW-6). Points the person wrote change only as proposals
//! (UX-WHO-2, enforced by the daemon). The tool never adds a line for the
//! person: a map update is not one of the UX-CHAT-7 moments.

use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::ErrorData as McpError;
use serde::Deserialize;
use wenlan_types::{
    ThingMapOp, ThingMapOpsRequest, ThingMapOpsResponse, ThingMapResponse, ThingMapSource,
    ThingMapState,
};

use super::{tool_error, url_encode_simple, WenlanMcpServer};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ThingMapAction {
    /// Read the map: points with their ids, links, and the revision.
    #[default]
    Read,
    /// Apply `ops` in order, all or none, against `expected_revision`.
    Change,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ThingMapStateParam {
    Open,
    Decided,
    Dropped,
}

impl From<ThingMapStateParam> for ThingMapState {
    fn from(value: ThingMapStateParam) -> Self {
        match value {
            ThingMapStateParam::Open => Self::Open,
            ThingMapStateParam::Decided => Self::Decided,
            ThingMapStateParam::Dropped => Self::Dropped,
        }
    }
}

/// One change. Ids are the ones `read` shows; an `add` may name its own
/// `id` so later ops in the same call can use it.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum ThingMapOpParam {
    /// A new point. `parent_id` is left out only for the first point of an
    /// empty map. `memory_id` cites a memory you captured for it.
    Add {
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        parent_id: Option<String>,
        kind: String,
        #[serde(default)]
        state: Option<ThingMapStateParam>,
        label: String,
        #[serde(default)]
        memory_id: Option<String>,
    },
    /// Change a point's kind, state or label; a field left out stays.
    Edit {
        id: String,
        #[serde(default)]
        kind: Option<String>,
        #[serde(default)]
        state: Option<ThingMapStateParam>,
        #[serde(default)]
        label: Option<String>,
    },
    /// Put a point under another one.
    Move { id: String, parent_id: String },
    /// Remove a point and everything under it.
    Remove { id: String },
    /// Link two points beyond parent and child, e.g. kind `answers`.
    Link {
        from: String,
        to: String,
        kind: String,
    },
    Unlink {
        from: String,
        to: String,
        kind: String,
    },
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ThingMapParams {
    #[serde(default)]
    pub action: ThingMapAction,
    #[schemars(
        description = "The thing whose map to use. Leave out to use the thing this conversation is in."
    )]
    #[serde(default)]
    pub thing_id: Option<String>,
    #[schemars(description = "For change: the revision your last read showed.")]
    #[serde(default)]
    pub expected_revision: Option<i64>,
    #[schemars(description = "For change: the ops, applied in order, all or none.")]
    #[serde(default)]
    pub ops: Vec<ThingMapOpParam>,
    #[schemars(
        description = "Your AI session as tool:id, e.g. claude-code:<session id>; names the conversation the points come from."
    )]
    #[serde(default)]
    pub session: Option<String>,
}

/// The daemon's op for `param`. A new point's source is this conversation,
/// with the memory it cites when given.
pub(super) fn to_op(param: ThingMapOpParam, session: Option<&str>) -> ThingMapOp {
    match param {
        ThingMapOpParam::Add {
            id,
            parent_id,
            kind,
            state,
            label,
            memory_id,
        } => ThingMapOp::Add {
            id,
            parent_id,
            rank: None,
            kind,
            state: state.map(Into::into),
            label,
            sources: session
                .map(|session| ThingMapSource {
                    session_id: session.to_string(),
                    from: None,
                    to: None,
                    memory_id,
                })
                .into_iter()
                .collect(),
        },
        ThingMapOpParam::Edit {
            id,
            kind,
            state,
            label,
        } => ThingMapOp::Edit {
            id,
            kind,
            state: state.map(Into::into),
            label,
            sources: None,
        },
        ThingMapOpParam::Move { id, parent_id } => ThingMapOp::Move {
            id,
            parent_id,
            rank: None,
        },
        ThingMapOpParam::Remove { id } => ThingMapOp::Remove { id },
        ThingMapOpParam::Link { from, to, kind } => ThingMapOp::Link { from, to, kind },
        ThingMapOpParam::Unlink { from, to, kind } => ThingMapOp::Unlink { from, to, kind },
    }
}

fn state_word(state: Option<ThingMapState>) -> &'static str {
    match state {
        Some(ThingMapState::Open) | None => "",
        Some(ThingMapState::Decided) => ", decided",
        Some(ThingMapState::Dropped) => ", dropped",
    }
}

/// The map as an indented list the AI can act on: ids, kinds, states and
/// who wrote each point.
pub(super) fn format_map(map: &ThingMapResponse, proposed: Option<usize>) -> String {
    let mut out = format!(
        "thing {} · revision {} · {} points\n",
        map.thing_id,
        map.revision,
        map.nodes.len()
    );
    if map.nodes.is_empty() {
        out.push_str("(empty: start with one root point, parent_id left out)\n");
    }
    fn walk(map: &ThingMapResponse, parent: Option<&str>, depth: usize, out: &mut String) {
        for node in map
            .nodes
            .iter()
            .filter(|n| n.parent_id.as_deref() == parent)
        {
            let by = match node.by {
                wenlan_types::ThingMapAuthor::You => ", the person's",
                _ => "",
            };
            out.push_str(&format!(
                "{}- [{}] {}{}{}: {}\n",
                "  ".repeat(depth),
                node.id,
                node.kind,
                state_word(node.state),
                by,
                node.label
            ));
            walk(map, Some(&node.id), depth + 1, out);
        }
    }
    walk(map, None, 0, &mut out);
    for edge in &map.edges {
        out.push_str(&format!(
            "link: {} -{}-> {}\n",
            edge.from, edge.kind, edge.to
        ));
    }
    if let Some(proposed) = proposed.filter(|n| *n > 0) {
        out.push_str(&format!(
            "{proposed} of these changes touch the person's points and wait for them as proposals.\n"
        ));
    }
    if !map.proposals.is_empty() {
        out.push_str(&format!(
            "{} proposals wait for the person.\n",
            map.proposals.len()
        ));
    }
    out
}

impl WenlanMcpServer {
    pub async fn thing_map_impl(&self, params: ThingMapParams) -> Result<CallToolResult, McpError> {
        let client = self.session_client(params.session.as_deref()).await;
        let session = client.session().map(|session| session.to_string());
        let thing_id = match params.thing_id.filter(|id| !id.trim().is_empty()) {
            Some(id) => id,
            None => {
                let Some(session) = session.as_deref() else {
                    return Ok(CallToolResult::error(vec![ContentBlock::text(
                        "Pass thing_id, or session so the map of this conversation's thing is used."
                            .to_string(),
                    )]));
                };
                let detail: wenlan_types::sessions::SessionDetailResponse = match client
                    .get(&format!("/api/sessions/{}", url_encode_simple(session)))
                    .await
                {
                    Ok(detail) => detail,
                    Err(error) => return Ok(tool_error(error, "thing_map")),
                };
                match detail.things.as_slice() {
                    [] => {
                        return Ok(CallToolResult::error(vec![ContentBlock::text(
                            "This conversation is not in a thing yet; Wenlan sorts it within a minute of its first save. Try again then."
                                .to_string(),
                        )]))
                    }
                    [one] => one.id.clone(),
                    many => {
                        let names: Vec<String> = many
                            .iter()
                            .map(|thing| match &thing.title {
                                Some(title) => format!("{} ({title})", thing.id),
                                None => thing.id.clone(),
                            })
                            .collect();
                        return Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                            "This conversation is in several things; pass thing_id: {}",
                            names.join(", ")
                        ))]));
                    }
                }
            }
        };
        let path = format!("/api/things/{}/map", url_encode_simple(&thing_id));
        match params.action {
            ThingMapAction::Read => {
                let map: ThingMapResponse = match client.get(&path).await {
                    Ok(map) => map,
                    Err(error) => return Ok(tool_error(error, "thing_map")),
                };
                Ok(CallToolResult::success(vec![ContentBlock::text(
                    format_map(&map, None),
                )]))
            }
            ThingMapAction::Change => {
                let Some(expected_revision) = params.expected_revision else {
                    return Err(McpError::invalid_params(
                        "change needs expected_revision from your last read",
                        None,
                    ));
                };
                if params.ops.is_empty() {
                    return Err(McpError::invalid_params(
                        "change needs at least one op",
                        None,
                    ));
                }
                let request = ThingMapOpsRequest {
                    expected_revision,
                    ops: params
                        .ops
                        .into_iter()
                        .map(|op| to_op(op, session.as_deref()))
                        .collect(),
                };
                let response: ThingMapOpsResponse =
                    match client.post(&format!("{path}/ops"), &request).await {
                        Ok(response) => response,
                        Err(error) => return Ok(tool_error(error, "thing_map")),
                    };
                Ok(CallToolResult::success(vec![ContentBlock::text(
                    format_map(&response.map, Some(response.proposed)),
                )]))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::WenlanClient;
    use crate::tools::TransportMode;
    use wenlan_types::{ThingMapAuthor, ThingMapNode};

    fn node(id: &str, parent: Option<&str>, kind: &str, by: ThingMapAuthor) -> ThingMapNode {
        ThingMapNode {
            id: id.into(),
            parent_id: parent.map(str::to_string),
            rank: 1.0,
            kind: kind.into(),
            state: None,
            label: format!("{id} label"),
            by,
            sources: Vec::new(),
        }
    }

    fn text(result: &CallToolResult) -> String {
        match &result.content[0] {
            ContentBlock::Text(text) => text.text.clone(),
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[test]
    fn ux_view_7_the_map_reads_as_a_tree_with_ids_and_whose_points() {
        let mut decided = node("o1", Some("q"), "option", ThingMapAuthor::You);
        decided.state = Some(ThingMapState::Decided);
        let map = ThingMapResponse {
            thing_id: "thg_1".into(),
            revision: 4,
            nodes: vec![
                node("q", None, "question", ThingMapAuthor::Ai),
                decided,
                node("r", Some("o1"), "reason", ThingMapAuthor::Ai),
            ],
            edges: vec![wenlan_types::ThingMapEdge {
                from: "r".into(),
                to: "q".into(),
                kind: "answers".into(),
            }],
            proposals: Vec::new(),
        };
        assert_eq!(
            format_map(&map, Some(1)),
            "thing thg_1 · revision 4 · 3 points\n\
             - [q] question: q label\n\
             \x20 - [o1] option, decided, the person's: o1 label\n\
             \x20   - [r] reason: r label\n\
             link: r -answers-> q\n\
             1 of these changes touch the person's points and wait for them as proposals.\n"
        );
    }

    #[test]
    fn a_new_point_comes_from_this_conversation() {
        let op = to_op(
            ThingMapOpParam::Add {
                id: Some("n".into()),
                parent_id: Some("q".into()),
                kind: "option".into(),
                state: Some(ThingMapStateParam::Dropped),
                label: "Fly".into(),
                memory_id: Some("mem_1".into()),
            },
            Some("claude-code:abc"),
        );
        let ThingMapOp::Add { sources, state, .. } = op else {
            panic!("an add stays an add");
        };
        assert_eq!(state, Some(ThingMapState::Dropped));
        assert_eq!(
            sources,
            [ThingMapSource {
                session_id: "claude-code:abc".into(),
                from: None,
                to: None,
                memory_id: Some("mem_1".into()),
            }]
        );
    }

    /// UX-WHO-2 travels through the daemon: the tool posts as the AI (its
    /// agent name) and reports what waits for the person.
    #[tokio::test]
    async fn a_change_posts_the_ops_as_the_ai_against_the_revision_read() {
        use wiremock::matchers::{body_partial_json, header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/things/thg_1/map/ops"))
            .and(header("x-agent-name", "claude-code"))
            .and(body_partial_json(serde_json::json!({
                "expected_revision": 2,
                "ops": [{"op": "edit", "id": "o1", "state": "dropped"}]
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "thing_id": "thg_1",
                "revision": 2,
                "nodes": [],
                "edges": [],
                "proposals": [{
                    "id": "tmp_1",
                    "by": "ai",
                    "change": {"op": "edit", "id": "o1", "state": "dropped"},
                    "created_at": 1
                }],
                "proposed": 1
            })))
            .mount(&mock)
            .await;
        let server = WenlanMcpServer::new(
            WenlanClient::new(mock.uri()),
            TransportMode::Stdio,
            "claude-code".into(),
            None,
        );
        let result = server
            .thing_map_impl(ThingMapParams {
                action: ThingMapAction::Change,
                thing_id: Some("thg_1".into()),
                expected_revision: Some(2),
                ops: vec![ThingMapOpParam::Edit {
                    id: "o1".into(),
                    kind: None,
                    state: Some(ThingMapStateParam::Dropped),
                    label: None,
                }],
                session: None,
            })
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(false), "{result:?}");
        let reply = text(&result);
        assert!(reply.contains("wait for them as proposals"), "{reply}");
        assert!(
            !reply.contains("saved_in"),
            "the tool says nothing to the person"
        );

        let missing = server
            .thing_map_impl(ThingMapParams {
                action: ThingMapAction::Change,
                thing_id: Some("thg_1".into()),
                expected_revision: None,
                ops: Vec::new(),
                session: None,
            })
            .await;
        assert!(missing.is_err(), "a change needs the revision read");
    }
}
