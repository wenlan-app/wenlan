//! Session attribution: the `session` tool argument (or the environment)
//! reaches the daemon as `x-wenlan-session`, after one `POST /api/sessions`.

use rmcp::model::CallToolResult;
use wenlan_mcp::client::WenlanClient;
use wenlan_mcp::tools::{
    CaptureParams, SteepAction, SteepParams, TransportMode, UpdateBriefParams, WenlanMcpServer,
    WritePageParams,
};
use wenlan_types::responses::StoreMemoryResponse;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Tests here read and write process environment variables.
static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn clear_env() {
    std::env::remove_var("WENLAN_SESSION");
    std::env::remove_var("CODEX_THREAD_ID");
}

fn server(mock: &MockServer, transport: TransportMode) -> WenlanMcpServer {
    WenlanMcpServer::new(
        WenlanClient::new(mock.uri()),
        transport,
        "test-agent".into(),
        None,
    )
}

fn store_response() -> StoreMemoryResponse {
    serde_json::from_value(serde_json::json!({
        "source_id": "mem_s1",
        "chunks_created": 1,
        "memory_type": "fact",
        "entity_id": null,
        "quality": null,
        "warnings": [],
        "near_duplicate": null,
        "gated": false,
        "extraction_method": "none",
        "enrichment": "",
        "hint": ""
    }))
    .unwrap()
}

async fn mount_daemon(mock: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/api/memory/store"))
        .respond_with(ResponseTemplate::new(200).set_body_json(store_response()))
        .mount(mock)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/sessions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "ses_x", "tool": "claude-code", "external_id": "abc-123",
            "first_write_at": 1, "last_write_at": 1
        })))
        .mount(mock)
        .await;
}

fn capture(session: Option<&str>) -> CaptureParams {
    CaptureParams {
        content: "session stamping".into(),
        memory_type: None,
        space: None,
        entity: None,
        entity_type: None,
        entity_id: None,
        confidence: None,
        supersedes: None,
        structured_fields: None,
        retrieval_cue: None,
        session: session.map(str::to_string),
    }
}

fn session_header(request: &wiremock::Request) -> Option<String> {
    request
        .headers
        .get("x-wenlan-session")
        .map(|value| value.to_str().unwrap().to_string())
}

/// The requests the tool sent, leaving out the best-effort steep count each
/// reply reads on the side; it is not what these tests stamp or announce.
async fn sent(mock: &MockServer) -> Vec<wiremock::Request> {
    mock.received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|request| request.url.path() != "/api/steep/worklist")
        .collect()
}

fn is_ok(result: &CallToolResult) -> bool {
    result.is_error != Some(true)
}

#[tokio::test]
async fn capture_announces_the_session_once_then_stamps_each_write() {
    let _guard = ENV_LOCK.lock().await;
    clear_env();
    let mock = MockServer::start().await;
    mount_daemon(&mock).await;
    let server = server(&mock, TransportMode::Stdio);

    for _ in 0..2 {
        let result = server
            .capture_impl(capture(Some("claude-code:abc-123")))
            .await
            .unwrap();
        assert!(is_ok(&result));
    }

    let requests = sent(&mock).await;
    let paths: Vec<_> = requests.iter().map(|r| r.url.path().to_string()).collect();
    assert_eq!(
        paths,
        ["/api/sessions", "/api/memory/store", "/api/memory/store"],
        "metadata is posted once, before the first write"
    );
    let announced: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(announced["session"], "claude-code:abc-123");
    assert!(announced["cwd"].is_string());
    assert_eq!(
        session_header(&requests[1]).as_deref(),
        Some("claude-code:abc-123")
    );
    assert_eq!(
        session_header(&requests[2]).as_deref(),
        Some("claude-code:abc-123")
    );
}

#[tokio::test]
async fn no_session_means_no_header_and_no_announcement() {
    let _guard = ENV_LOCK.lock().await;
    clear_env();
    let mock = MockServer::start().await;
    mount_daemon(&mock).await;

    server(&mock, TransportMode::Stdio)
        .capture_impl(capture(None))
        .await
        .unwrap();

    let requests = sent(&mock).await;
    assert_eq!(requests.len(), 1);
    assert_eq!(session_header(&requests[0]), None);
}

#[tokio::test]
async fn argument_beats_environment_and_invalid_values_are_ignored() {
    let _guard = ENV_LOCK.lock().await;
    clear_env();
    std::env::set_var("WENLAN_SESSION", "cursor:from-env");
    let mock = MockServer::start().await;
    mount_daemon(&mock).await;
    let server = server(&mock, TransportMode::Stdio);

    server
        .capture_impl(capture(Some("claude-code:from-arg")))
        .await
        .unwrap();
    // A malformed argument is ignored, and the environment applies instead.
    let result = server
        .capture_impl(capture(Some("not a session")))
        .await
        .unwrap();
    assert!(is_ok(&result), "a bad session never fails the write");
    clear_env();

    let requests = sent(&mock).await;
    let stamped: Vec<_> = requests
        .iter()
        .filter(|r| r.url.path() == "/api/memory/store")
        .map(session_header)
        .collect();
    assert_eq!(
        stamped,
        [
            Some("claude-code:from-arg".to_string()),
            Some("cursor:from-env".to_string())
        ]
    );
}

#[tokio::test]
async fn codex_thread_id_is_the_last_resort() {
    let _guard = ENV_LOCK.lock().await;
    clear_env();
    std::env::set_var("CODEX_THREAD_ID", "019a-thread");
    let mock = MockServer::start().await;
    mount_daemon(&mock).await;

    server(&mock, TransportMode::Stdio)
        .capture_impl(capture(None))
        .await
        .unwrap();
    clear_env();

    let requests = sent(&mock).await;
    let store = requests
        .iter()
        .find(|r| r.url.path() == "/api/memory/store")
        .unwrap();
    assert_eq!(session_header(store).as_deref(), Some("codex:019a-thread"));
}

#[tokio::test]
async fn a_remote_facing_server_ignores_its_own_environment() {
    let _guard = ENV_LOCK.lock().await;
    clear_env();
    std::env::set_var("WENLAN_SESSION", "cursor:server-env");
    let mock = MockServer::start().await;
    mount_daemon(&mock).await;
    let server = server(&mock, TransportMode::Http);

    server.capture_impl(capture(None)).await.unwrap();
    server
        .capture_impl(capture(Some("claude-code:remote-caller")))
        .await
        .unwrap();
    clear_env();

    let requests = sent(&mock).await;
    let stores: Vec<_> = requests
        .iter()
        .filter(|r| r.url.path() == "/api/memory/store")
        .map(session_header)
        .collect();
    assert_eq!(
        stores,
        [None, Some("claude-code:remote-caller".to_string())]
    );
    // The server's own cwd and repository do not describe a remote caller.
    let announced = requests
        .iter()
        .find(|r| r.url.path() == "/api/sessions")
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&announced.body).unwrap();
    assert_eq!(body["session"], "claude-code:remote-caller");
    assert!(body.get("cwd").is_none());
}

#[tokio::test]
async fn write_page_and_update_brief_carry_the_session() {
    let _guard = ENV_LOCK.lock().await;
    clear_env();
    let mock = MockServer::start().await;
    mount_daemon(&mock).await;
    Mock::given(method("POST"))
        .and(path("/api/pages"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({ "id": "page_1" })),
        )
        .mount(&mock)
        .await;
    Mock::given(method("PATCH"))
        .and(path("/api/brief"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "space": "work", "brief_version": 1, "applied": [], "conflicts": [],
            "projection_path": null, "warnings": []
        })))
        .mount(&mock)
        .await;
    let server = server(&mock, TransportMode::Stdio);

    server
        .write_page_impl(WritePageParams {
            page_id: None,
            title: Some("T".into()),
            content: "body".into(),
            summary: None,
            entity_id: None,
            space: None,
            source_memory_ids: vec![],
            expected_version: None,
            session: Some("claude-code:abc-123".into()),
            basis: Vec::new(),
            code_basis: Vec::new(),
            minor: None,
        })
        .await
        .unwrap();
    server
        .update_brief_impl(UpdateBriefParams {
            space: Some("work".into()),
            operation_id: Some("op-1".into()),
            summary: None,
            mutations: vec![serde_json::from_value(serde_json::json!({
                "kind": "add", "text": "x", "state": "active"
            }))
            .unwrap()],
            session: Some("claude-code:abc-123".into()),
        })
        .await
        .unwrap();

    let requests = sent(&mock).await;
    for wanted in ["/api/pages", "/api/brief"] {
        let request = requests.iter().find(|r| r.url.path() == wanted).unwrap();
        assert_eq!(
            session_header(request).as_deref(),
            Some("claude-code:abc-123"),
            "{wanted}"
        );
    }
    let announcements = requests
        .iter()
        .filter(|r| r.url.path() == "/api/sessions")
        .count();
    assert_eq!(announcements, 1, "one announcement per session per process");
}

#[tokio::test]
async fn a_steep_rewrite_carries_the_session() {
    let _guard = ENV_LOCK.lock().await;
    clear_env();
    let mock = MockServer::start().await;
    mount_daemon(&mock).await;
    Mock::given(method("POST"))
        .and(path("/api/steep/commit"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "page_id": "page_1", "outcome": "written", "replayed": false
        })))
        .mount(&mock)
        .await;
    let server = server(&mock, TransportMode::Stdio);

    let result = server
        .steep_impl(SteepParams {
            action: SteepAction::Commit,
            page_id: None,
            token: Some("stp_x".into()),
            content: Some("body [1]".into()),
            answers: None,
            limit: None,
            cursor: None,
            space: None,
            session: Some("claude-code:abc-123".into()),
        })
        .await
        .unwrap();
    assert!(is_ok(&result), "{result:?}");

    let requests = sent(&mock).await;
    let commit = requests
        .iter()
        .find(|r| r.url.path() == "/api/steep/commit")
        .unwrap();
    assert_eq!(
        session_header(commit).as_deref(),
        Some("claude-code:abc-123")
    );
}
