// SPDX-License-Identifier: Apache-2.0
//! Read and propose edits to the one personal-rules store. Applying a plan is
//! deliberately left to the person in the terminal.

use super::{query_success, WenlanMcpServer};
use crate::client::WenlanError;
use rmcp::{model::CallToolResult, ErrorData as McpError};
use serde::Deserialize;
use wenlan_types::personal_rules::{
    PersonalRuleEdit, PersonalRuleLayer, PersonalRulePlan, PersonalRuleProposeRequest,
    PersonalRuleShow, PersonalRuleShowRequest,
};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PersonalRuleLayerParam {
    Shared,
    Tool {
        /// The exact tool whose personal overlay is being read or proposed.
        tool: String,
    },
}

impl From<PersonalRuleLayerParam> for PersonalRuleLayer {
    fn from(value: PersonalRuleLayerParam) -> Self {
        match value {
            PersonalRuleLayerParam::Shared => Self::Shared,
            PersonalRuleLayerParam::Tool { tool } => Self::Tool { tool },
        }
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadPersonalRulesParams {
    /// Required. Choose `shared` or `tool`; do not infer a layer.
    pub layer: PersonalRuleLayerParam,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PersonalRuleEditParam {
    /// Required. Choose `shared` or `tool`; do not infer a layer.
    pub layer: PersonalRuleLayerParam,
    /// Complete replacement text for this layer. It is staged for review and
    /// is not written by this tool.
    pub text: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProposePersonalRulesParams {
    /// All edits from this request, grouped into one plan and one diff.
    #[serde(default)]
    pub edits: Vec<PersonalRuleEditParam>,
    /// Preview moving approved every-Space rules from pages. Supply no edits.
    #[serde(default)]
    pub merge_pages: bool,
}

fn invalid_layer(layer: &PersonalRuleLayer) -> bool {
    match layer {
        PersonalRuleLayer::Shared => false,
        PersonalRuleLayer::Tool { tool } => tool.trim().is_empty(),
    }
}

fn credential_message(reason: &str) -> Option<String> {
    let detail = reason.strip_prefix("personal_rule_credential: layer=")?;
    let (layer, line) = detail.split_once(" line=")?;
    if !matches!(
        layer,
        "shared"
            | "claudecode"
            | "codexcli"
            | "geminicli"
            | "antigravity-ide"
            | "copilotcli"
            | "cursor"
            | "opencode"
            | "musecode"
    ) {
        return None;
    }
    let line = line.parse::<usize>().ok().filter(|line| *line > 0)?;
    Some(format!(
        "Possible credential in the {layer} rule layer on line {line}. No rule change was applied."
    ))
}

fn safe_daemon_error(error: WenlanError, fallback: &str) -> String {
    match error {
        WenlanError::Api {
            reason: Some(reason),
            ..
        } => credential_message(&reason),
        _ => None,
    }
    .unwrap_or_else(|| fallback.to_owned())
}

impl WenlanMcpServer {
    pub async fn read_personal_rules_impl(
        &self,
        params: ReadPersonalRulesParams,
    ) -> Result<CallToolResult, McpError> {
        if self.transport != super::TransportMode::Stdio {
            return Err(McpError::invalid_params(
                "Personal rules are available only through local stdio MCP.",
                None,
            ));
        }
        let request = PersonalRuleShowRequest {
            layer: params.layer.into(),
        };
        if invalid_layer(&request.layer) {
            return Err(McpError::invalid_params(
                "Choose a named tool for the tool layer.",
                None,
            ));
        }
        match self
            .client
            .post::<_, PersonalRuleShow>("/api/rules/personal/show", &request)
            .await
        {
            Ok(value) => query_success(value),
            // Avoid relaying daemon response text, which could contain rule
            // contents or credential-shaped text.
            Err(error) => Ok(CallToolResult::error(vec![
                rmcp::model::ContentBlock::text(safe_daemon_error(
                    error,
                    "Wenlan could not read that personal-rules layer. No rule text was returned.",
                )),
            ])),
        }
    }

    pub async fn propose_personal_rules_impl(
        &self,
        params: ProposePersonalRulesParams,
    ) -> Result<CallToolResult, McpError> {
        if self.transport != super::TransportMode::Stdio {
            return Err(McpError::invalid_params(
                "Personal rules are available only through local stdio MCP.",
                None,
            ));
        }
        if params.merge_pages {
            if !params.edits.is_empty() {
                return Err(McpError::invalid_params(
                    "Preview the page-rule move on its own, without layer edits.",
                    None,
                ));
            }
            return match self
                .client
                .get::<Option<PersonalRulePlan>>("/api/rules/personal/merge-pages")
                .await
            {
                Ok(Some(plan)) => query_success(plan),
                Ok(None) => query_success(serde_json::json!({"page_merge_pending": false})),
                Err(error) => Ok(CallToolResult::error(vec![
                    rmcp::model::ContentBlock::text(safe_daemon_error(
                        error,
                        "Wenlan could not preview the move. No rules changed.",
                    )),
                ])),
            };
        }
        if params.edits.is_empty() {
            return Err(McpError::invalid_params(
                "Provide at least one layer edit; no plan was created.",
                None,
            ));
        }
        let edits: Vec<PersonalRuleEdit> = params
            .edits
            .into_iter()
            .map(|edit| PersonalRuleEdit {
                layer: edit.layer.into(),
                text: edit.text,
            })
            .collect();
        if edits.iter().any(|edit| invalid_layer(&edit.layer)) {
            return Err(McpError::invalid_params(
                "Choose a named tool for every tool-layer edit.",
                None,
            ));
        }
        let request = PersonalRuleProposeRequest { edits };
        match self
            .client
            .post::<_, PersonalRulePlan>("/api/rules/personal/propose", &request)
            .await
        {
            Ok(value) => query_success(value),
            // Never echo the submitted text or arbitrary daemon errors. The
            // daemon validates proposals, including credential-shaped lines.
            Err(error) => Ok(CallToolResult::error(vec![rmcp::model::ContentBlock::text(
                safe_daemon_error(error, "Wenlan did not return a personal-rules plan. No rule change was applied; retry after checking the daemon.")
            )])),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{client::WenlanClient, tools::TransportMode};
    use wiremock::{
        matchers::{body_json, header, method, path},
        Mock, MockServer, ResponseTemplate,
    };

    #[tokio::test]
    async fn personal_rules_page_merge_preview_only_calls_the_read_endpoint() {
        let mock = MockServer::start().await;
        Mock::given(method("GET")).and(path("/api/rules/personal/merge-pages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id":"fixture", "expires_at":1, "proposer":"Wenlan pages", "changes":[], "blocked_tools":[],
                "page_merge":{"rule_count":1, "excluded":[], "excludes_space_only":true}
            })))
            .expect(1).mount(&mock).await;
        let result = server(&mock.uri(), TransportMode::Stdio)
            .propose_personal_rules_impl(ProposePersonalRulesParams {
                edits: vec![],
                merge_pages: true,
            })
            .await
            .unwrap();
        let plan: PersonalRulePlan =
            serde_json::from_value(result.structured_content.unwrap()).unwrap();
        assert_eq!(plan.page_merge.unwrap().rule_count, 1);
        assert_eq!(mock.received_requests().await.unwrap().len(), 1);
    }

    fn server(url: &str, transport: TransportMode) -> WenlanMcpServer {
        WenlanMcpServer::new(
            WenlanClient::new(url.into()),
            transport,
            "codex".into(),
            None,
        )
    }

    #[tokio::test]
    async fn read_uses_explicit_layer_and_typed_response() {
        let mock = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/rules/personal/show"))
            .and(header("x-agent-name", "codex"))
            .and(body_json(
                serde_json::json!({"layer":{"kind":"tool","tool":"claude"}}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "layer":{"kind":"tool","tool":"claude"},
                "text":"Keep changes small.",
                "origins":[null],
                "affected_tools":["claude"]
            })))
            .mount(&mock)
            .await;

        let result = server(&mock.uri(), TransportMode::Stdio)
            .read_personal_rules_impl(ReadPersonalRulesParams {
                layer: PersonalRuleLayerParam::Tool {
                    tool: "claude".into(),
                },
            })
            .await
            .unwrap();
        let value: PersonalRuleShow =
            serde_json::from_value(result.structured_content.unwrap()).unwrap();
        assert_eq!(value.text, "Keep changes small.");
        assert_eq!(value.affected_tools, ["claude"]);
    }

    #[tokio::test]
    async fn proposal_sends_multiple_layers_as_one_typed_plan_and_never_applies() {
        let mock = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/rules/personal/propose"))
            .and(body_json(serde_json::json!({"edits":[
                {"layer":{"kind":"shared"},"text":"Shared rule."},
                {"layer":{"kind":"tool","tool":"codex"},"text":"Tool rule."}
            ]})))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id":"plan_1","expires_at":2000000000,"proposer":"codex",
                "changes":[
                    {"layer":{"kind":"shared"},"before":"","after":"Shared rule.","diff":"+Shared rule.","affected_tools":["codex","claude"]},
                    {"layer":{"kind":"tool","tool":"codex"},"before":"","after":"Tool rule.","diff":"+Tool rule.","affected_tools":["codex"]}
                ],"blocked_tools":[]
            })))
            .mount(&mock)
            .await;

        let result = server(&mock.uri(), TransportMode::Stdio)
            .propose_personal_rules_impl(ProposePersonalRulesParams {
                merge_pages: false,
                edits: vec![
                    PersonalRuleEditParam {
                        layer: PersonalRuleLayerParam::Shared,
                        text: "Shared rule.".into(),
                    },
                    PersonalRuleEditParam {
                        layer: PersonalRuleLayerParam::Tool {
                            tool: "codex".into(),
                        },
                        text: "Tool rule.".into(),
                    },
                ],
            })
            .await
            .unwrap();
        let plan: PersonalRulePlan =
            serde_json::from_value(result.structured_content.unwrap()).unwrap();
        assert_eq!(plan.id, "plan_1");
        assert_eq!(plan.changes.len(), 2);
        assert_eq!(mock.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn proposal_errors_do_not_echo_unvalidated_text_or_daemon_body() {
        let mock = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/rules/personal/propose"))
            .respond_with(
                ResponseTemplate::new(400)
                    .set_body_string("credential rejected: sk-live-this-must-not-escape"),
            )
            .mount(&mock)
            .await;
        let result = server(&mock.uri(), TransportMode::Stdio)
            .propose_personal_rules_impl(ProposePersonalRulesParams {
                merge_pages: false,
                edits: vec![PersonalRuleEditParam {
                    layer: PersonalRuleLayerParam::Shared,
                    text: "sk-live-this-must-not-escape".into(),
                }],
            })
            .await
            .unwrap();
        let output = result
            .content
            .iter()
            .filter_map(|c| match c {
                rmcp::model::ContentBlock::Text(t) => Some(t.text.as_str()),
                _ => None,
            })
            .collect::<String>();
        assert!(!output.contains("sk-live-this-must-not-escape"));
        assert!(output.contains("No rule change was applied"));
    }

    #[tokio::test]
    async fn credential_refusal_reports_only_the_validated_layer_and_line() {
        let mock = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/rules/personal/propose"))
            .respond_with(ResponseTemplate::new(409).set_body_json(serde_json::json!({
                "error": "personal_rule_credential: layer=shared line=2"
            })))
            .mount(&mock)
            .await;
        let result = server(&mock.uri(), TransportMode::Stdio)
            .propose_personal_rules_impl(ProposePersonalRulesParams {
                merge_pages: false,
                edits: vec![PersonalRuleEditParam {
                    layer: PersonalRuleLayerParam::Shared,
                    text: "Unreturned submitted text".into(),
                }],
            })
            .await
            .unwrap();
        let output = result
            .content
            .iter()
            .filter_map(|content| match content {
                rmcp::model::ContentBlock::Text(text) => Some(text.text.as_str()),
                _ => None,
            })
            .collect::<String>();
        assert!(output.contains("shared rule layer on line 2"));
        assert!(!output.contains("Unreturned submitted text"));
        assert!(credential_message("personal_rule_credential: layer=secret line=2").is_none());
    }

    #[tokio::test]
    async fn rules_tools_refuse_remote_transport_before_request() {
        let mock = MockServer::start().await;
        let result = server(&mock.uri(), TransportMode::Http)
            .read_personal_rules_impl(ReadPersonalRulesParams {
                layer: PersonalRuleLayerParam::Shared,
            })
            .await;
        assert!(result.is_err());
        let result = server(&mock.uri(), TransportMode::Http)
            .propose_personal_rules_impl(ProposePersonalRulesParams {
                edits: vec![],
                merge_pages: false,
            })
            .await;
        assert!(result.is_err());
        assert!(mock.received_requests().await.unwrap().is_empty());
    }

    #[test]
    fn schemas_require_an_explicit_layer_and_tool_name() {
        let read = serde_json::to_value(schemars::schema_for!(ReadPersonalRulesParams)).unwrap();
        let edit = serde_json::to_value(schemars::schema_for!(PersonalRuleEditParam)).unwrap();
        assert_eq!(read["required"], serde_json::json!(["layer"]));
        assert!(read["properties"]["layer"].to_string().contains("shared"));
        assert!(read["properties"]["layer"].to_string().contains("tool"));
        assert!(edit["properties"]["layer"].to_string().contains("tool"));
        assert!(edit.to_string().contains("exact tool"));
        assert!(serde_json::from_value::<ReadPersonalRulesParams>(serde_json::json!({})).is_err());
        assert!(
            serde_json::from_value::<PersonalRuleEditParam>(serde_json::json!({
                "layer":{"kind":"tool"},"text":"rule"
            }))
            .is_err()
        );
    }
}
