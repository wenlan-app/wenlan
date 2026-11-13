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

/// ChatGPT keeps using a connection after a pause. Once the SDK drops an idle
/// connection (five minutes in production), the next call must still work
/// instead of answering "session not found".
#[tokio::test]
async fn a_connection_idle_past_cleanup_keeps_working() {
    let mut manager = LocalSessionManager::default();
    manager.session_config.keep_alive = Some(Duration::from_millis(300));
    let manager = Arc::new(manager);
    let mut config = StreamableHttpServerConfig::default();
    config.session_store = Some(Arc::new(IdleSessionResume::default()));
    let service = StreamableHttpService::new(
        || {
            Ok(WenlanMcpServer::new(
                WenlanClient::new("http://127.0.0.1:1".into()),
                TransportMode::Http,
                "synthetic-idle-test".into(),
                None,
            ))
        },
        manager.clone(),
        config,
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
    let router = Router::new()
        .nest_service("/mcp", service)
        .layer(middleware::from_fn(validate_initialization));
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let result = tokio::spawn(async move {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        let post = |session: Option<&str>, body: serde_json::Value| {
            let mut request = client
                .post(&endpoint)
                .header("Accept", "application/json, text/event-stream")
                .header("Mcp-Protocol-Version", "2025-06-18")
                .json(&body);
            if let Some(id) = session {
                request = request.header("Mcp-Session-Id", id);
            }
            request.send()
        };
        let opened = post(
            None,
            serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {
                    "protocolVersion": "2025-06-18", "capabilities": {},
                    "clientInfo": { "name": "chatgpt-like", "version": "1" }
                }
            }),
        )
        .await
        .unwrap();
        assert_eq!(opened.status(), 200);
        let id = opened.headers()["mcp-session-id"]
            .to_str()
            .unwrap()
            .to_owned();
        opened.text().await.unwrap();
        let initialized = post(
            Some(&id),
            serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        )
        .await
        .unwrap();
        assert_eq!(initialized.status(), 202);

        tokio::time::timeout(Duration::from_secs(3), async {
            while !manager.sessions.read().await.is_empty() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the idle connection is cleaned up first");

        let listed = post(
            Some(&id),
            serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        )
        .await
        .unwrap();
        assert_eq!(
            listed.status(),
            200,
            "an idle connection must not answer 404"
        );
        let body = listed.text().await.unwrap();
        assert!(body.contains("\"tools\""), "tools/list answered: {body}");
    })
    .await;
    server.abort();
    let _ = server.await;
    result.unwrap();
}

/// One call whose daemon answer is slow, against a connection whose idle limit
/// is `idle`. Returns the HTTP status and body of the call.
async fn slow_call_with_idle_limit(idle: Duration) -> (u16, String) {
    // A daemon that takes 800 ms to answer anything.
    let daemon = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let daemon_url = format!("http://{}", daemon.local_addr().unwrap());
    let daemon_task = tokio::spawn(async move {
        let slow = Router::new().fallback(|| async {
            tokio::time::sleep(Duration::from_millis(800)).await;
            axum::Json(serde_json::json!({"pages": [], "total": 0}))
        });
        axum::serve(daemon, slow).await.unwrap()
    });

    let mut manager = LocalSessionManager::default();
    manager.session_config.keep_alive = Some(idle);
    let service = StreamableHttpService::new(
        move || {
            Ok(WenlanMcpServer::new(
                WenlanClient::new(daemon_url.clone()),
                TransportMode::Http,
                "synthetic-slow-test".into(),
                None,
            ))
        },
        Arc::new(manager),
        StreamableHttpServerConfig::default(),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, Router::new().nest_service("/mcp", service))
            .await
            .unwrap()
    });
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let post = |session: Option<String>, body: serde_json::Value| {
        let mut request = client
            .post(&endpoint)
            .header("Accept", "application/json, text/event-stream")
            .header("Mcp-Protocol-Version", "2025-06-18")
            .json(&body);
        if let Some(id) = session {
            request = request.header("Mcp-Session-Id", id);
        }
        request.send()
    };
    let opened = post(
        None,
        serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18", "capabilities": {},
                "clientInfo": { "name": "slow-call", "version": "1" }
            }
        }),
    )
    .await
    .unwrap();
    let id = opened.headers()["mcp-session-id"]
        .to_str()
        .unwrap()
        .to_owned();
    opened.text().await.unwrap();
    post(
        Some(id.clone()),
        serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    )
    .await
    .unwrap();
    let outcome = match post(
        Some(id),
        serde_json::json!({
            "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": { "name": "list_pages", "arguments": {} }
        }),
    )
    .await
    {
        Ok(response) => {
            let status = response.status().as_u16();
            (status, response.text().await.unwrap_or_default())
        }
        Err(error) => (0, error.to_string()),
    };
    server.abort();
    daemon_task.abort();
    outcome
}

/// A save that took longer than the idle limit failed at that mark: the
/// SDK only restarts the timer on a new message, not while a call runs.
#[tokio::test]
async fn a_call_slower_than_the_idle_limit_is_cut_off_and_a_longer_limit_keeps_it() {
    let (_, cut) = slow_call_with_idle_limit(Duration::from_millis(300)).await;
    assert!(
        !cut.contains("\"id\":2"),
        "a call outliving the idle limit should lose its answer: {cut}"
    );
    let (status, kept) = slow_call_with_idle_limit(Duration::from_secs(3)).await;
    assert_eq!(status, 200);
    assert!(kept.contains("\"id\":2"), "the slow call's answer: {kept}");
}

#[test]
fn production_connections_allow_slow_calls_up_to_an_hour() {
    assert_eq!(
        session_manager().session_config.keep_alive,
        Some(SESSION_IDLE_LIMIT)
    );
    assert!(SESSION_IDLE_LIMIT >= Duration::from_secs(30 * 60));
}
