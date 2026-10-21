//! The public query profile must never forward the daemon's full wire objects.
use rmcp::model::{CallToolResult, ContentBlock};
use serde_json::{json, Value};
use wenlan_mcp::client::WenlanClient;
use wenlan_mcp::tools::{BriefParams, RecallParams, ToolProfile, TransportMode, WenlanMcpServer};
use wenlan_types::{BriefReadResponse, PageSourceWithMemory, SearchResult};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn server(mock: &MockServer, transport: TransportMode, profile: ToolProfile) -> WenlanMcpServer {
    WenlanMcpServer::new(
        WenlanClient::new(mock.uri()),
        transport,
        "test".into(),
        None,
    )
    .with_tool_profile(profile)
}

fn hit() -> SearchResult {
    serde_json::from_value(json!({
        "id":"SECRET_SENTINEL_chunk", "source_id":"page_fixture", "source":"page",
        "title":"Fixture", "content":"A grounded decision", "chunk_index":123,
        "last_modified":100, "score":0.9, "is_archived":true, "pending_revision":true,
        "source_text":"SECRET_SENTINEL_raw", "structured_fields":"SECRET_SENTINEL_structured",
        "content_hash":"SECRET_SENTINEL_hash", "source_agent":"SECRET_SENTINEL_agent",
        "retrieval_cue":"SECRET_SENTINEL_cue", "merged_from":["SECRET_SENTINEL_merge"],
        "last_delta_summary":"SECRET_SENTINEL_delta"
    }))
    .unwrap()
}

fn expected_hit() -> Value {
    json!({"source_id":"page_fixture", "title":"Fixture", "content":"A grounded decision",
        "is_archived":true, "pending_revision":true})
}

fn sources() -> Vec<PageSourceWithMemory> {
    serde_json::from_value(json!([
        {"source":{"page_id":"page_fixture", "memory_source_id":"mem_fixture",
            "linked_at":100, "link_reason":"SECRET_SENTINEL_link"},
         "memory":{"source_id":"mem_fixture", "title":"Evidence", "content":"Supporting fact",
            "confirmed":true, "pinned":false, "last_modified":100, "chunk_count":1,
            "enrichment_status":"SECRET_SENTINEL_enrichment", "supersede_mode":"replace",
            "access_count":999, "source_text":"SECRET_SENTINEL_raw",
            "structured_fields":"SECRET_SENTINEL_structured", "changelog":"SECRET_SENTINEL_change",
            "is_archived":true, "pending_revision":true}},
        {"source":{"page_id":"page_fixture", "memory_source_id":"mem_missing", "linked_at":101},
         "memory":null}
    ]))
    .unwrap()
}

fn brief() -> BriefReadResponse {
    serde_json::from_value(json!({
        "state":"ready", "space":"fixture",
        "brief":{"space_id":"SECRET_SENTINEL_space", "space":"fixture", "version":5,
            "last_session_summary":"Current work", "last_handoff_at":100,
            "active":[{"id":"SECRET_SENTINEL_item", "text":"Review evidence", "state":"active",
                "added_at":100, "version":4, "gate":"Approval required"}], "backlog":[]},
        "related_context":{"query":"evidence", "results":[hit()]}
    }))
    .unwrap()
}

fn recall_params() -> RecallParams {
    RecallParams {
        session: None,
        query: "decision".into(),
        limit: Some(3),
        memory_type: None,
        space: Some("fixture".into()),
        rerank: Some(false),
    }
}

fn brief_params() -> BriefParams {
    BriefParams {
        session: None,
        topic: Some("evidence".into()),
        space: Some("fixture".into()),
    }
}

fn assert_output(result: &CallToolResult, expected: Value) {
    assert_eq!(result.is_error, Some(false));
    assert_eq!(result.structured_content.as_ref(), Some(&expected));
    assert_eq!(result.content.len(), 1);
    let ContentBlock::Text(text) = &result.content[0] else {
        panic!("expected JSON text")
    };
    assert_eq!(serde_json::from_str::<Value>(&text.text).unwrap(), expected);
    assert!(!serde_json::to_string(result)
        .unwrap()
        .contains("SECRET_SENTINEL"));
}

#[tokio::test]
async fn query_recall_projects_hits_and_supplemental_pages() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/memory/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results":[hit()], "supplemental_pages":[hit()], "took_ms":2.5
        })))
        .mount(&mock)
        .await;
    for transport in [TransportMode::Stdio, TransportMode::Http] {
        let result = server(&mock, transport.clone(), ToolProfile::QueryOnly)
            .recall_impl(recall_params())
            .await
            .unwrap();
        assert_output(
            &result,
            json!({"results":[expected_hit()], "supplemental_pages":[expected_hit()]}),
        );
        let standard = server(&mock, transport, ToolProfile::Standard)
            .recall_impl(recall_params())
            .await
            .unwrap();
        assert!(standard.structured_content.is_none());
        let ContentBlock::Text(text) = &standard.content[0] else {
            panic!()
        };
        assert!(text.text.starts_with("1 results (2.5ms)\n"));
        assert!(text.text.contains("Compiled pages:"));
        assert!(text.text.contains("SECRET_SENTINEL_hash"));
    }
}

#[tokio::test]
async fn query_brief_projects_ready_and_related_context() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/brief"))
        .respond_with(ResponseTemplate::new(200).set_body_json(brief()))
        .mount(&mock)
        .await;
    for transport in [TransportMode::Stdio, TransportMode::Http] {
        let result = server(&mock, transport.clone(), ToolProfile::QueryOnly)
            .brief_impl(brief_params())
            .await
            .unwrap();
        assert_output(
            &result,
            json!({"state":"ready", "space":"fixture",
            "brief":{"last_session_summary":"Current work", "active":[{"text":"Review evidence",
                "gate":"Approval required"}], "backlog":[]},
            "related_context":{"query":"evidence", "results":[expected_hit()]},
            "code_basis_pending":null}),
        );
        let standard = server(&mock, transport, ToolProfile::Standard)
            .brief_impl(brief_params())
            .await
            .unwrap();
        assert!(standard.structured_content.is_none());
        let ContentBlock::Text(text) = &standard.content[0] else {
            panic!()
        };
        assert!(text.text.contains("SECRET_SENTINEL_item (v4)"));
    }
}

#[tokio::test]
async fn query_brief_preserves_empty_states() {
    for (state, space) in [
        ("space_not_resolved", Value::Null),
        ("brief_not_created", json!("fixture")),
    ] {
        let mock = MockServer::start().await;
        let mut payload = serde_json::to_value(brief()).unwrap();
        payload["state"] = json!(state);
        payload["space"] = space.clone();
        Mock::given(method("POST"))
            .and(path("/api/brief"))
            .respond_with(ResponseTemplate::new(200).set_body_json(payload))
            .mount(&mock)
            .await;
        let result = server(&mock, TransportMode::Http, ToolProfile::QueryOnly)
            .brief_impl(brief_params())
            .await
            .unwrap();
        assert_output(
            &result,
            json!({"state":state, "space":space, "brief":null, "related_context":null,
                "code_basis_pending":null}),
        );
    }
}

#[tokio::test]
async fn query_brief_does_not_turn_malformed_ready_into_success() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/brief"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"state":"ready","space":"fixture"})),
        )
        .mount(&mock)
        .await;
    assert!(server(&mock, TransportMode::Http, ToolProfile::QueryOnly)
        .brief_impl(brief_params())
        .await
        .is_err());
}

#[tokio::test]
async fn query_sources_omit_unavailable_evidence_without_changing_standard() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/pages/page_fixture/sources"))
        .respond_with(ResponseTemplate::new(200).set_body_json(sources()))
        .mount(&mock)
        .await;
    for transport in [TransportMode::Stdio, TransportMode::Http] {
        let result = server(&mock, transport.clone(), ToolProfile::QueryOnly)
            .get_page_sources_impl("page_fixture")
            .await
            .unwrap();
        assert_output(
            &result,
            json!({"page_id":"page_fixture", "sources":[
            {"source_id":"mem_fixture", "memory":{"title":"Evidence", "content":"Supporting fact",
                "is_archived":true, "pending_revision":true}}]}),
        );
        assert!(!serde_json::to_string(&result)
            .unwrap()
            .contains("mem_missing"));
        let standard = server(&mock, transport, ToolProfile::Standard)
            .get_page_sources_impl("page_fixture")
            .await
            .unwrap();
        assert!(standard.structured_content.is_none());
        let ContentBlock::Text(text) = &standard.content[0] else {
            panic!()
        };
        assert_eq!(
            text.text,
            format!(
                "2 sources\n{}",
                serde_json::to_string_pretty(&sources()).unwrap()
            )
        );
    }
}

#[tokio::test]
async fn query_sources_do_not_disclose_any_ids_when_all_evidence_is_unavailable() {
    let mock = MockServer::start().await;
    let hidden = vec![sources().pop().unwrap()];
    Mock::given(method("GET"))
        .and(path("/api/pages/page_fixture/sources"))
        .respond_with(ResponseTemplate::new(200).set_body_json(hidden))
        .mount(&mock)
        .await;
    let result = server(&mock, TransportMode::Http, ToolProfile::QueryOnly)
        .get_page_sources_impl("page_fixture")
        .await
        .unwrap();
    assert_output(&result, json!({"page_id":"page_fixture", "sources":[]}));
}

#[tokio::test]
async fn query_errors_stay_errors_without_raw_backend_details() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500).set_body_string("SECRET_SENTINEL_backend_error"))
        .mount(&mock)
        .await;
    let result = server(&mock, TransportMode::Http, ToolProfile::QueryOnly)
        .get_page_sources_impl("page_fixture")
        .await
        .unwrap();
    assert_eq!(result.is_error, Some(true));
    assert!(result.structured_content.is_none());
    assert!(!serde_json::to_string(&result)
        .unwrap()
        .contains("SECRET_SENTINEL"));
}
