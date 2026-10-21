use super::*;

#[tokio::test]
async fn initialization_guard_rejects_before_sdk_session_allocation() {
    for profile in [
        ToolProfile::Standard,
        ToolProfile::QueryOnly,
        ToolProfile::WebWrite,
    ] {
        initialization_guard_case(profile).await;
    }
}

async fn initialization_guard_case(profile: ToolProfile) {
    let manager = Arc::new(LocalSessionManager::default());
    let service = StreamableHttpService::new(
        move || {
            Ok(WenlanMcpServer::new(
                WenlanClient::new("http://127.0.0.1:1".into()),
                TransportMode::Http,
                "synthetic-initialization-test".into(),
                None,
            )
            .with_tool_profile(profile))
        },
        manager.clone(),
        StreamableHttpServerConfig::default(),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let endpoint = format!("http://{address}/mcp");
    let router = Router::new()
        .nest_service("/mcp", service)
        .layer(middleware::from_fn(validate_initialization));
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let result = tokio::spawn(async move {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap();
        let initialize = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18", "capabilities": {},
                "clientInfo": { "name": "synthetic", "version": "1" }
            }
        });
        let mut missing_id = initialize.clone();
        missing_id.as_object_mut().unwrap().remove("id");
        let mut wrong_params = initialize.clone();
        wrong_params["params"] = serde_json::json!({});
        let invalid = [
            missing_id,
            wrong_params,
            serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            serde_json::json!({"jsonrpc":"2.0","id":1,"method":"ping"}),
            serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
            serde_json::json!({"jsonrpc":"2.0","id":1,"result":{}}),
            serde_json::json!([initialize.clone()]),
        ];
        for path in [&endpoint, &format!("{endpoint}/")] {
            for value in &invalid {
                let response = client
                    .post(path)
                    .header("Accept", "application/json, text/event-stream")
                    .json(value)
                    .send()
                    .await
                    .unwrap();
                assert!(
                    manager.sessions.read().await.is_empty(),
                    "invalid request allocated an SDK session"
                );
                assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            }
        }
        let invalid_header = client.post(&endpoint)
            .header("Accept", "application/json, text/event-stream")
            .header("Mcp-Session-Id", http::HeaderValue::from_bytes(&[0xff]).unwrap())
            .json(&invalid[0]).send().await.unwrap();
        assert_eq!(invalid_header.status(), StatusCode::BAD_REQUEST);
        assert!(manager.sessions.read().await.is_empty(), "opaque header must not bypass pre-allocation validation");
        let oversized = client
            .post(&endpoint)
            .header("Accept", "application/json, text/event-stream")
            .body("x".repeat(64 * 1024 + 1))
            .send()
            .await
            .unwrap();
        assert_eq!(oversized.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert!(manager.sessions.read().await.is_empty());

        let response = client
            .post(&endpoint)
            .header("Accept", "application/json, text/event-stream")
            .json(&initialize)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let id = response.headers()["mcp-session-id"]
            .to_str()
            .unwrap()
            .to_owned();
        response.text().await.unwrap();
        assert_eq!(manager.sessions.read().await.len(), 1);
        let notification = client
            .post(&endpoint)
            .header("Accept", "application/json, text/event-stream")
            .header("Mcp-Session-Id", &id)
            .json(&serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .send()
            .await
            .unwrap();
        assert_eq!(notification.status(), StatusCode::ACCEPTED);
        let deleted = client
            .delete(&endpoint)
            .header("Mcp-Session-Id", &id)
            .send()
            .await
            .unwrap();
        assert_eq!(deleted.status(), StatusCode::ACCEPTED);
        assert!(manager.sessions.read().await.is_empty());

        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut stalled = tokio::net::TcpStream::connect(address).await.unwrap();
        stalled.write_all(format!(
            "POST /mcp HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n1\r\n{{\r\n"
        ).as_bytes()).await.unwrap();
        let mut response = Vec::new();
        tokio::time::timeout(Duration::from_secs(7), stalled.read_to_end(&mut response))
            .await.unwrap().unwrap();
        assert!(String::from_utf8(response).unwrap().starts_with("HTTP/1.1 408"));
        assert!(manager.sessions.read().await.is_empty());
    })
    .await;
    server.abort();
    assert!(server.await.is_err_and(|error| error.is_cancelled()));
    result.unwrap();
}

#[test]
fn query_only_scope_requires_exactly_one_scope() {
    assert_eq!(
        query_only_scope(Some("work".into()), false).unwrap(),
        "work"
    );
    assert_eq!(query_only_scope(None, true).unwrap(), WHOLE_LIBRARY_SPACE);
    for (locked, whole_library, expected) in [
        (Some("work"), true, WHOLE_LIBRARY_PIN_CONFLICT_ERROR),
        (Some("*"), false, RESERVED_SPACE_PIN_ERROR),
        (None, false, QUERY_ONLY_SPACE_ERROR),
    ] {
        let error = query_only_scope(locked.map(str::to_string), whole_library)
            .expect_err("invalid scope must be rejected");
        assert_eq!(error.to_string(), expected);
    }
}
