// SPDX-License-Identifier: Apache-2.0
//! Agents may propose sourced comparisons; only human Review decides them.
use super::{
    query_success, tool_error, url_encode_simple, ToolProfile, TransportMode, WenlanMcpServer,
};
use rmcp::{model::CallToolResult, ErrorData as McpError};
use serde::Deserialize;
use wenlan_types::{pages::PdfCitationTarget, research_disputes::*};
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DisputeKindParam {
    OpposingClaims,
    IncomparableConditions,
    DifferentDefinitions,
    InsufficientEvidence,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DisputePdfTargetParam {
    pub attachment_key: String,
    pub pdf_digest: String,
    pub page: u32,
    pub quote: String,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DisputeEvidenceParam {
    pub source: String,
    pub source_id: String,
    pub source_digest: String,
    pub quote: String,
    pub pdf_target: Option<DisputePdfTargetParam>,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProposeResearchDisputeParams {
    pub space: String,
    pub kind: DisputeKindParam,
    pub conditions: String,
    pub reason: String,
    pub evidence: Vec<DisputeEvidenceParam>,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadResearchDisputeParams {
    pub space: String,
    pub id: String,
}
impl WenlanMcpServer {
    fn dispute_guard(&self, space: &str) -> Result<(), McpError> {
        if self.transport != TransportMode::Stdio
            || self.tool_profile != ToolProfile::Standard
            || space.trim().is_empty()
            || space == "*"
            || crate::lock_state::locked_space().is_some_and(|locked| locked != space)
        {
            return Err(McpError::invalid_params("disputes require an explicit registered Space matching the local standard stdio connection lock",None));
        }
        Ok(())
    }
    pub async fn propose_research_dispute_impl(
        &self,
        params: ProposeResearchDisputeParams,
    ) -> Result<CallToolResult, McpError> {
        self.dispute_guard(&params.space)?;
        let kind = match params.kind {
            DisputeKindParam::OpposingClaims => ResearchDisputeKind::OpposingClaims,
            DisputeKindParam::IncomparableConditions => ResearchDisputeKind::IncomparableConditions,
            DisputeKindParam::DifferentDefinitions => ResearchDisputeKind::DifferentDefinitions,
            DisputeKindParam::InsufficientEvidence => ResearchDisputeKind::InsufficientEvidence,
        };
        let request = ProposeResearchDisputeRequest {
            space: params.space,
            kind,
            conditions: params.conditions,
            reason: params.reason,
            evidence: params
                .evidence
                .into_iter()
                .map(|e| ResearchDisputeEvidence {
                    source: e.source,
                    source_id: e.source_id,
                    source_digest: e.source_digest,
                    source_lifecycle: None,
                    quote: e.quote,
                    pdf_target: e.pdf_target.map(|p| PdfCitationTarget {
                        attachment_key: p.attachment_key,
                        pdf_digest: p.pdf_digest,
                        page: p.page,
                        quote: p.quote,
                    }),
                })
                .collect(),
        };
        match self
            .client
            .post::<_, ResearchDisputeProposalResult>("/api/research/disputes", &request)
            .await
        {
            Ok(value) => query_success(value),
            Err(error) => Ok(tool_error(error, "research dispute proposal")),
        }
    }
    pub async fn read_research_dispute_impl(
        &self,
        params: ReadResearchDisputeParams,
    ) -> Result<CallToolResult, McpError> {
        self.dispute_guard(&params.space)?;
        let path = format!(
            "/api/research/disputes/{}?space={}",
            url_encode_simple(&params.id),
            url_encode_simple(&params.space)
        );
        match self.client.get::<ResearchDisputeView>(&path).await {
            Ok(value) => query_success(value),
            Err(error) => Ok(tool_error(error, "research dispute")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::WenlanClient;
    use wiremock::{
        matchers::{body_partial_json, header, method, path},
        Mock, MockServer, ResponseTemplate,
    };
    fn server(url: &str, transport: TransportMode) -> WenlanMcpServer {
        WenlanMcpServer::new(
            WenlanClient::new(url.into()),
            transport,
            "dispute-test".into(),
            None,
        )
    }
    fn request(space: &str) -> ProposeResearchDisputeParams {
        ProposeResearchDisputeParams {
            space: space.into(),
            kind: DisputeKindParam::IncomparableConditions,
            conditions: "Different population".into(),
            reason: "Compare original passages".into(),
            evidence: vec![],
        }
    }
    #[tokio::test]
    async fn dispute_tools_require_local_profile_and_matching_explicit_space() {
        let _guard = crate::lock_state::ENV_LOCK.lock().await;
        std::env::set_var("WENLAN_SPACE", "Research");
        crate::lock_state::init_from_env();
        let mock = MockServer::start().await;
        assert!(server(&mock.uri(), TransportMode::Http)
            .propose_research_dispute_impl(request("Research"))
            .await
            .is_err());
        let mut query = server(&mock.uri(), TransportMode::Stdio);
        query.tool_profile = ToolProfile::QueryOnly;
        assert!(query
            .propose_research_dispute_impl(request("Research"))
            .await
            .is_err());
        assert!(server(&mock.uri(), TransportMode::Stdio)
            .propose_research_dispute_impl(request("Other"))
            .await
            .is_err());
        assert!(server(&mock.uri(), TransportMode::Stdio)
            .read_research_dispute_impl(ReadResearchDisputeParams {
                space: String::new(),
                id: "case".into()
            })
            .await
            .is_err());
        assert!(mock.received_requests().await.unwrap().is_empty());
        std::env::remove_var("WENLAN_SPACE");
        crate::lock_state::init_from_env();
    }
    #[tokio::test]
    async fn dispute_proposal_uses_typed_payload_and_original_stored_digest() {
        let _guard = crate::lock_state::ENV_LOCK.lock().await;
        std::env::set_var("WENLAN_SPACE", "Research");
        crate::lock_state::init_from_env();
        let mock = MockServer::start().await;
        Mock::given(method("POST")).and(path("/api/research/disputes"))
            .and(header("x-wenlan-space", "Research")).and(body_partial_json(serde_json::json!({"space":"Research","kind":"incomparable_conditions","conditions":"Different population"}))).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"id":"case","created":false,"payload_digest":"original-stored-digest"}))).mount(&mock).await;
        let output = server(&mock.uri(), TransportMode::Stdio)
            .propose_research_dispute_impl(request("Research"))
            .await
            .unwrap();
        let actual: ResearchDisputeProposalResult =
            serde_json::from_value(output.structured_content.unwrap()).unwrap();
        assert!(!actual.created);
        assert_eq!(actual.payload_digest, "original-stored-digest");
        std::env::remove_var("WENLAN_SPACE");
        crate::lock_state::init_from_env();
    }
}
