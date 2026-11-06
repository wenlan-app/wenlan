// SPDX-License-Identifier: Apache-2.0
use super::{
    query_success, tool_error, url_encode_simple, ToolProfile, TransportMode, WenlanMcpServer,
};
use rmcp::{model::CallToolResult, ErrorData as McpError};
use serde::Deserialize;
use wenlan_types::research::*;
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadOrganizeRunParams {
    pub space: String,
    pub id: String,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateOrganizeRunParams {
    pub space: String,
    pub id: String,
    pub expected_revision: u64,
    pub source: Option<String>,
    pub source_id: Option<String>,
    /// pending, reading, complete, failed or unavailable. Complete means all indexed text was read.
    pub coverage: Option<String>,
    pub content_digest: Option<String>,
    pub next_offset: Option<usize>,
    pub error: Option<String>,
    #[serde(default)]
    pub cancel: bool,
    /// Only after the person explicitly asks to continue a cancelled run.
    #[serde(default)]
    pub resume: bool,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClassificationChangeParam {
    pub source: String,
    pub source_id: String,
    pub source_content_digest: String,
    pub expected_decision_revision: u64,
    pub add_tags: Vec<String>,
    pub remove_tags: Vec<String>,
    pub reason: String,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProposeClassificationsParams {
    pub space: String,
    pub run_id: String,
    pub changes: Vec<ClassificationChangeParam>,
}
impl WenlanMcpServer {
    fn research_guard(&self, space: &str) -> Result<(), McpError> {
        if self.transport != TransportMode::Stdio || self.tool_profile != ToolProfile::Standard {
            return Err(McpError::invalid_params(
                "organizing tools require local standard stdio",
                None,
            ));
        }
        if space.trim().is_empty()
            || space == "*"
            || crate::lock_state::locked_space().is_some_and(|locked| locked != space)
        {
            return Err(McpError::invalid_params(
                "explicit organizing Space conflicts with lock or is missing",
                None,
            ));
        }
        Ok(())
    }
    pub async fn read_organize_run_impl(
        &self,
        p: ReadOrganizeRunParams,
    ) -> Result<CallToolResult, McpError> {
        self.research_guard(&p.space)?;
        let q = format!("space={}", url_encode_simple(&p.space));
        match self
            .client
            .get::<OrganizeRun>(&format!(
                "/api/research/runs/{}?{q}",
                url_encode_simple(&p.id)
            ))
            .await
        {
            Ok(v) => query_success(v),
            Err(e) => Ok(tool_error(e, "organizing run")),
        }
    }
    pub async fn update_organize_run_impl(
        &self,
        p: UpdateOrganizeRunParams,
    ) -> Result<CallToolResult, McpError> {
        self.research_guard(&p.space)?;
        let coverage = p
            .coverage
            .map(|v| serde_json::from_value(serde_json::Value::String(v)))
            .transpose()
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
        let request = UpdateOrganizeRunRequest {
            space: p.space,
            expected_revision: p.expected_revision,
            source: p.source,
            source_id: p.source_id,
            coverage,
            content_digest: p.content_digest,
            next_offset: p.next_offset,
            error: p.error,
            cancel: p.cancel,
            resume: p.resume,
        };
        match self
            .client
            .post::<_, OrganizeRun>(
                &format!("/api/research/runs/{}", url_encode_simple(&p.id)),
                &request,
            )
            .await
        {
            Ok(v) => query_success(v),
            Err(e) => Ok(tool_error(e, "organizing run update")),
        }
    }
    pub async fn propose_source_classifications_impl(
        &self,
        p: ProposeClassificationsParams,
    ) -> Result<CallToolResult, McpError> {
        self.research_guard(&p.space)?;
        let request = ProposeSourceClassificationsRequest {
            space: p.space,
            run_id: p.run_id,
            changes: p
                .changes
                .into_iter()
                .map(|c| SourceClassificationChange {
                    title: String::new(),
                    before_tags: Vec::new(),
                    source: c.source,
                    source_id: c.source_id,
                    source_content_digest: c.source_content_digest,
                    expected_decision_revision: c.expected_decision_revision,
                    add_tags: c.add_tags,
                    remove_tags: c.remove_tags,
                    reason: c.reason,
                })
                .collect(),
        };
        match self
            .client
            .post::<_, ClassificationProposalResult>("/api/research/classifications", &request)
            .await
        {
            Ok(v) => query_success(v),
            Err(e) => Ok(tool_error(e, "classification proposal")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::WenlanClient;
    use wiremock::{
        matchers::{method, path, query_param},
        Mock, MockServer, ResponseTemplate,
    };
    fn server(url: &str, transport: TransportMode) -> WenlanMcpServer {
        WenlanMcpServer::new(
            WenlanClient::new(url.into()),
            transport,
            "research-test".into(),
            None,
        )
    }
    #[tokio::test]
    async fn research_tools_refuse_remote_and_query_profiles_before_http() {
        let mock = MockServer::start().await;
        let remote = server(&mock.uri(), TransportMode::Http);
        assert!(remote
            .read_organize_run_impl(ReadOrganizeRunParams {
                space: "Research".into(),
                id: "run".into()
            })
            .await
            .is_err());
        let mut query = server(&mock.uri(), TransportMode::Stdio);
        query.tool_profile = ToolProfile::QueryOnly;
        assert!(query
            .propose_source_classifications_impl(ProposeClassificationsParams {
                space: "Research".into(),
                run_id: "run".into(),
                changes: vec![]
            })
            .await
            .is_err());
        assert!(mock.received_requests().await.unwrap().is_empty());
    }
    #[tokio::test]
    async fn research_read_run_preserves_scope_revision_and_target() {
        let _guard = crate::lock_state::ENV_LOCK.lock().await;
        std::env::remove_var("WENLAN_SPACE");
        crate::lock_state::init_from_env();
        let mock = MockServer::start().await;
        let value = OrganizeRun {
            id: "run".into(),
            space_id: "stable-space".into(),
            revision: 7,
            rules: OrganizeRules {
                space_id: "stable-space".into(),
                revision: 3,
                name: "Methods".into(),
                instructions: "Compare methods".into(),
                content_digest: "digest".into(),
            },
            research_description: Some("Compare retrieval methods".into()),
            items: vec![],
            target_page_id: Some("existing-topic".into()),
            target_page_version: Some(4),
            target_page_incarnation: Some("incarnation".into()),
            result_page_version: None,
            pending_revision_id: Some("revision".into()),
            state: OrganizeRunState::AwaitingReview,
            created_at: 1,
            updated_at: 2,
        };
        Mock::given(method("GET"))
            .and(path("/api/research/runs/run"))
            .and(query_param("space", "Research"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&value))
            .mount(&mock)
            .await;
        let output = server(&mock.uri(), TransportMode::Stdio)
            .read_organize_run_impl(ReadOrganizeRunParams {
                space: "Research".into(),
                id: "run".into(),
            })
            .await
            .unwrap();
        let actual: OrganizeRun =
            serde_json::from_value(output.structured_content.unwrap()).unwrap();
        assert_eq!(actual, value);
    }
}
