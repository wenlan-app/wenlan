use std::time::Duration;

#[tokio::test]
async fn test_health_endpoint_no_auth() {
    let port = portpicker::pick_unused_port().expect("no free port");
    let config = test_config(port, None);

    let handle = tokio::spawn(async move {
        wenlan_mcp::serve::run_serve(config).await.unwrap();
    });

    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = reqwest::get(format!("http://127.0.0.1:{}/health", port))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");
    assert_eq!(body["server"], "wenlan-mcp");

    handle.abort();
}

#[tokio::test]
async fn test_auth_rejects_missing_token() {
    let port = portpicker::pick_unused_port().expect("no free port");
    let config = test_config(port, Some("secret-token".into()));

    let handle = tokio::spawn(async move {
        wenlan_mcp::serve::run_serve(config).await.unwrap();
    });

    tokio::time::sleep(Duration::from_millis(500)).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://127.0.0.1:{}/mcp", port))
        .header("Content-Type", "application/json")
        .body("{}")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401);

    handle.abort();
}

#[tokio::test]
async fn test_auth_rejects_wrong_token() {
    let port = portpicker::pick_unused_port().expect("no free port");
    let config = test_config(port, Some("correct-token".into()));

    let handle = tokio::spawn(async move {
        wenlan_mcp::serve::run_serve(config).await.unwrap();
    });

    tokio::time::sleep(Duration::from_millis(500)).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://127.0.0.1:{}/mcp", port))
        .header("Content-Type", "application/json")
        .header("Authorization", "Bearer wrong-token")
        .body("{}")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401);

    handle.abort();
}

#[tokio::test]
async fn test_health_bypasses_auth() {
    let port = portpicker::pick_unused_port().expect("no free port");
    let config = test_config(port, Some("secret-token".into()));

    let handle = tokio::spawn(async move {
        wenlan_mcp::serve::run_serve(config).await.unwrap();
    });

    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = reqwest::get(format!("http://127.0.0.1:{}/health", port))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    handle.abort();
}

/// Regression: claude.ai MCP context error (Origin Kanban, 2026-04-23).
///
/// rmcp 1.5 added DNS-rebinding protection via
/// `StreamableHttpServerConfig.allowed_hosts`, defaulting to
/// `[localhost, 127.0.0.1, ::1]`. Production deployments expose this
/// server through a public tunnel (Cloudflare, ngrok) that forwards the
/// tunnel hostname in `Host`, so rmcp rejected every tunneled request
/// with a plain-text 403 "Forbidden: Host header is not allowed" before
/// any MCP handling. The Anthropic MCP proxy cannot parse that body as
/// JSON-RPC and surfaces it to users as "-32600 Invalid Request".
///
/// This test exercises the production shape: bearer auth on, foreign
/// Host header, and the full two-leg handshake (`initialize` to mint a
/// session, then `tools/call` for `brief` using that session). Both
/// legs must return 200; the old default fails the first leg with 403.
///
/// The downstream daemon is intentionally unreachable (dead port in
/// `test_config`) — the HTTP/transport layer is what this test
/// validates. A failing daemon surfaces as a tool-level error in the
/// SSE stream, which is orthogonal to the DNS-rebinding bug.
#[tokio::test]
async fn test_tunneled_host_passes_full_mcp_handshake_with_auth() {
    let port = portpicker::pick_unused_port().expect("no free port");
    let config = test_config(port, Some("test-token".into()));

    let handle = tokio::spawn(async move {
        wenlan_mcp::serve::run_serve(config).await.unwrap();
    });

    tokio::time::sleep(Duration::from_millis(500)).await;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();

    // Leg 1: initialize. Foreign Host + valid bearer token.
    let init_resp = client
        .post(format!("http://127.0.0.1:{}/mcp", port))
        .header("Host", "origin-mcp.example.com")
        .header("Authorization", "Bearer test-token")
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}"#,
        )
        .send()
        .await
        .expect("initialize request must complete");

    let init_status = init_resp.status();
    let session_id = init_resp
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    assert_eq!(
        init_status, 200,
        "initialize with tunneled Host + auth must return 200 (was 403 with default allowed_hosts)",
    );
    let session_id = session_id.expect("Mcp-Session-Id header must be present after initialize");

    // Leg 2: tools/call brief, reusing the session from leg 1.
    let call_resp = client
        .post(format!("http://127.0.0.1:{}/mcp", port))
        .header("Host", "origin-mcp.example.com")
        .header("Authorization", "Bearer test-token")
        .header("Mcp-Session-Id", &session_id)
        .header("Mcp-Protocol-Version", "2025-06-18")
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"brief","arguments":{}}}"#,
        )
        .send()
        .await
        .expect("tools/call request must complete");

    assert_eq!(
        call_resp.status(),
        200,
        "tools/call brief with tunneled Host + auth must return 200 (rmcp HTTP layer success)",
    );

    handle.abort();
}

/// Loopback binding alone does not prevent DNS rebinding. Without a bearer
/// token, foreign Host headers must be rejected before MCP session allocation.
#[tokio::test]
async fn test_no_auth_mode_rejects_foreign_host_and_accepts_loopback() {
    let port = portpicker::pick_unused_port().expect("no free port");
    let config = test_config(port, None);
    let handle = tokio::spawn(async move {
        wenlan_mcp::serve::run_serve(config).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(500)).await;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    let mut responses = Vec::new();
    for host in ["my-tunnel.trycloudflare.com", "127.0.0.1", "localhost"] {
        let response = client
            .post(format!("http://127.0.0.1:{port}/mcp"))
            .header("Host", format!("{host}:{port}"))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .body(
                r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}"#,
            )
            .send()
            .await
            .unwrap();
        let status = response.status();
        let has_session = response.headers().contains_key("mcp-session-id");
        let body = response.text().await.unwrap();
        responses.push((host, status, has_session, body));
    }
    handle.abort();
    for (host, status, has_session, body) in responses {
        if host == "my-tunnel.trycloudflare.com" {
            assert_eq!(status, 403, "foreign Host must be rejected: {body}");
            assert!(!has_session, "rejected Host must not allocate a session");
        } else {
            assert_eq!(status, 200, "loopback Host {host} must work: {body}");
            assert!(has_session, "loopback handshake must create a session");
        }
    }
}

#[tokio::test]
async fn test_rejects_disallowed_origin() {
    let port = portpicker::pick_unused_port().expect("no free port");
    let config = wenlan_mcp::serve::ServeConfig {
        port,
        host: "127.0.0.1".into(),
        origin_url: "http://127.0.0.1:19999".into(),
        token: Some("test-token".into()),
        agent_name: "test-agent".into(),
        user_id: None,
        allowed_origins: vec!["https://claude.ai".into()],
        whole_library: false,
    };

    let handle = tokio::spawn(async move {
        wenlan_mcp::serve::run_serve(config).await.unwrap();
    });

    tokio::time::sleep(Duration::from_millis(500)).await;

    let client = reqwest::Client::new();

    // Request with disallowed Origin should get 403
    let resp = client
        .post(format!("http://127.0.0.1:{}/mcp", port))
        .header("Content-Type", "application/json")
        .header("Authorization", "Bearer test-token")
        .header("Origin", "https://evil.com")
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);

    // Request with allowed Origin should pass auth (may fail downstream, but not 403)
    let resp = client
        .post(format!("http://127.0.0.1:{}/mcp", port))
        .header("Content-Type", "application/json")
        .header("Authorization", "Bearer test-token")
        .header("Origin", "https://claude.ai")
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_ne!(resp.status(), 403);

    handle.abort();
}

#[tokio::test]
async fn query_only_requires_a_non_empty_bearer_token() {
    for token in [None, Some(String::new()), Some(" \t\n".into())] {
        let config = test_config(0, token);

        let error = wenlan_mcp::serve::run_serve_with_profile(
            config,
            wenlan_mcp::tools::ToolProfile::QueryOnly,
        )
        .await
        .expect_err("query-only must reject missing or blank auth tokens");
        assert_eq!(error.to_string(), wenlan_mcp::serve::QUERY_ONLY_AUTH_ERROR);
    }
}

#[tokio::test]
async fn query_only_requires_a_strict_space_pin() {
    // No test in this binary calls `lock_state::init_from_env()`, so
    // `locked_space()` stays at its process-default `None` here — this
    // exercises the same "no pin" state without touching env vars.
    let config = test_config(0, Some("secret-token".into()));

    let error = wenlan_mcp::serve::run_serve_with_profile(
        config,
        wenlan_mcp::tools::ToolProfile::QueryOnly,
    )
    .await
    .expect_err("query-only must reject when no strict WENLAN_SPACE pin is active");
    assert_eq!(error.to_string(), wenlan_mcp::serve::QUERY_ONLY_SPACE_ERROR);
}

#[tokio::test]
async fn web_write_requires_a_non_empty_bearer_token() {
    for token in [None, Some(String::new()), Some(" \t\n".into())] {
        let config = test_config(0, token);

        let error = wenlan_mcp::serve::run_serve_with_profile(
            config,
            wenlan_mcp::tools::ToolProfile::WebWrite,
        )
        .await
        .expect_err("web-write must reject missing or blank auth tokens");
        assert_eq!(error.to_string(), wenlan_mcp::serve::WEB_WRITE_AUTH_ERROR);
    }
}

#[tokio::test]
async fn web_write_requires_a_strict_space_pin() {
    let config = test_config(0, Some("secret-token".into()));

    let error =
        wenlan_mcp::serve::run_serve_with_profile(config, wenlan_mcp::tools::ToolProfile::WebWrite)
            .await
            .expect_err("web-write must reject when no strict WENLAN_SPACE pin is active");
    assert_eq!(error.to_string(), wenlan_mcp::serve::WEB_WRITE_SPACE_ERROR);
}

#[test]
fn web_write_shares_the_connector_scope_rules_with_query_only() {
    use wenlan_mcp::serve::connector_scope;
    use wenlan_mcp::tools::ToolProfile;

    for profile in [ToolProfile::QueryOnly, ToolProfile::WebWrite] {
        assert_eq!(
            connector_scope(profile, Some("work".into()), false).unwrap(),
            "work"
        );
        assert_eq!(connector_scope(profile, None, true).unwrap(), "*");
        assert_eq!(
            connector_scope(profile, Some("work".into()), true)
                .unwrap_err()
                .to_string(),
            wenlan_mcp::serve::WHOLE_LIBRARY_PIN_CONFLICT_ERROR
        );
        assert_eq!(
            connector_scope(profile, Some("*".into()), false)
                .unwrap_err()
                .to_string(),
            wenlan_mcp::serve::RESERVED_SPACE_PIN_ERROR
        );
    }
    assert_eq!(
        connector_scope(ToolProfile::WebWrite, None, false)
            .unwrap_err()
            .to_string(),
        wenlan_mcp::serve::WEB_WRITE_SPACE_ERROR
    );
    assert_eq!(
        connector_scope(ToolProfile::QueryOnly, None, false)
            .unwrap_err()
            .to_string(),
        wenlan_mcp::serve::QUERY_ONLY_SPACE_ERROR
    );
    assert!(connector_scope(ToolProfile::Standard, Some("work".into()), true).is_err());
}

/// The web-write profile over a real HTTP listener: bearer-gated, pinned to a
/// Space, exposing exactly the ten tools, with deletes reaching the daemon for
/// an item in the pinned Space and refused for one outside it.
#[tokio::test]
async fn web_write_serves_the_write_tools_inside_the_pinned_space_only() {
    struct ChildGuard(std::process::Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    use wiremock::{
        matchers::{method, path},
        Mock, MockServer, ResponseTemplate,
    };

    let daemon = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/memory/mem_in/detail"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"memory": {
                "source_id": "mem_in", "title": "Fact", "content": "A fact", "summary": null,
                "memory_type": "fact", "space": "synthetic-review", "source_agent": "web-ai",
                "confidence": null, "confirmed": true, "pinned": false, "supersedes": null,
                "last_modified": 100, "chunk_count": 1, "enrichment_status": "done",
                "supersede_mode": "replace", "access_count": 0
            }})),
        )
        .mount(&daemon)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/memory/mem_out/detail"))
        .respond_with(ResponseTemplate::new(404).set_body_json(serde_json::json!({"error": "nf"})))
        .mount(&daemon)
        .await;
    Mock::given(method("DELETE"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"deleted": true})),
        )
        .mount(&daemon)
        .await;

    let port = portpicker::pick_unused_port().expect("no free port");
    let child = std::process::Command::new(env!("CARGO_BIN_EXE_wenlan-mcp"))
        .env("WENLAN_NO_AUTOSTART", "1")
        .env("WENLAN_SPACE", "synthetic-review")
        .env_remove("WENLAN_DEFAULT_SPACE")
        .args([
            "--origin-url",
            &daemon.uri(),
            "--agent-name",
            "web-ai",
            "serve",
            "--tool-profile",
            "web-write",
            "--token",
            "synthetic-connector-token",
            "--port",
            &port.to_string(),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("binary must start");
    let _guard = ChildGuard(child);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let base = format!("http://127.0.0.1:{port}");
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            if client
                .get(format!("{base}/health"))
                .send()
                .await
                .is_ok_and(|response| response.status().is_success())
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("server must become ready");

    let info: serde_json::Value = client
        .get(format!("{base}/connector-info"))
        .bearer_auth("synthetic-connector-token")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(info["tool_profile"], "web-write");
    assert_eq!(info["space"], "synthetic-review");

    let anonymous = client
        .post(format!("{base}/mcp"))
        .header("Accept", "application/json, text/event-stream")
        .json(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}))
        .send()
        .await
        .unwrap();
    assert_eq!(anonymous.status(), 401);

    let initialized = client
        .post(format!("{base}/mcp"))
        .header("Accept", "application/json, text/event-stream")
        .bearer_auth("synthetic-connector-token")
        .json(&serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                "clientInfo": {"name": "synthetic", "version": "1"}}
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(initialized.status(), 200);
    let session_id = initialized.headers()["mcp-session-id"]
        .to_str()
        .unwrap()
        .to_owned();
    initialized.text().await.unwrap();
    let notified = client
        .post(format!("{base}/mcp"))
        .header("Accept", "application/json, text/event-stream")
        .header("Mcp-Session-Id", &session_id)
        .bearer_auth("synthetic-connector-token")
        .json(&serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
        .send()
        .await
        .unwrap();
    assert_eq!(notified.status(), 202);

    let rpc = |id: u64, method: &'static str, params: serde_json::Value| {
        let client = client.clone();
        let base = base.clone();
        let session_id = session_id.clone();
        async move {
            let response = client
                .post(format!("{base}/mcp"))
                .header("Accept", "application/json, text/event-stream")
                .header("Mcp-Session-Id", &session_id)
                .bearer_auth("synthetic-connector-token")
                .json(&serde_json::json!({
                    "jsonrpc": "2.0", "id": id, "method": method, "params": params
                }))
                .send()
                .await
                .unwrap();
            rpc_result(response).await
        }
    };

    let listed = rpc(2, "tools/list", serde_json::json!({})).await;
    let mut names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "brief",
            "capture",
            "delete_page",
            "forget",
            "get_page_sources",
            "list_pages",
            "read_page",
            "recall",
            "rename_page",
            "write_page"
        ]
    );
    for tool in listed["result"]["tools"].as_array().unwrap() {
        assert!(
            tool["inputSchema"]["properties"].get("space").is_none(),
            "a pinned connection hides the space argument: {}",
            tool["name"]
        );
    }
    let hint = |name: &str, field: &str| {
        listed["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == name)
            .unwrap()["annotations"][field]
            .clone()
    };
    assert_eq!(hint("forget", "destructiveHint"), true);
    assert_eq!(hint("delete_page", "destructiveHint"), true);
    assert_eq!(hint("write_page", "readOnlyHint"), false);

    let refused = rpc(
        3,
        "tools/call",
        serde_json::json!({"name": "lint", "arguments": {}}),
    )
    .await;
    assert_eq!(refused["result"]["isError"], true, "{refused}");
    assert!(refused["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("not available in the web-write profile"));

    let outside = rpc(
        4,
        "tools/call",
        serde_json::json!({"name": "forget", "arguments": {"memory_id": "mem_out"}}),
    )
    .await;
    assert_eq!(outside["result"]["isError"], true, "{outside}");
    assert!(outside["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("not found in the Space this connection is limited to"));
    assert!(
        !daemon
            .received_requests()
            .await
            .unwrap()
            .iter()
            .any(|request| request.method == "DELETE"),
        "a memory outside the Space must never be deleted"
    );

    let inside = rpc(
        5,
        "tools/call",
        serde_json::json!({"name": "forget", "arguments": {"memory_id": "mem_in"}}),
    )
    .await;
    assert_ne!(inside["result"]["isError"], true, "{inside}");
    let deletes: Vec<_> = daemon
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|request| request.method == "DELETE")
        .collect();
    assert_eq!(deletes.len(), 1);
    assert_eq!(deletes[0].url.path(), "/api/memory/delete/mem_in");
    assert_eq!(
        deletes[0].headers.get("x-wenlan-space").unwrap(),
        "synthetic-review"
    );
    assert_eq!(deletes[0].headers.get("x-agent-name").unwrap(), "web-ai");
}

/// Run a `serve` invocation that must refuse to start. A regression that lets
/// it start would otherwise block `output()` forever, so it is killed and
/// reported after a bounded wait.
fn output_of_rejected_serve(mut command: std::process::Command) -> std::process::Output {
    let mut child = command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("wenlan-mcp binary must run");
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while child.try_wait().expect("child status").is_none() {
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            let output = child.wait_with_output().expect("killed child output");
            panic!(
                "serve started instead of refusing; stderr={}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    child.wait_with_output().expect("child output")
}

/// Isolated child-process regression: each scenario spawns the real
/// `wenlan-mcp` binary with its own environment, so `WENLAN_SPACE` /
/// `WENLAN_DEFAULT_SPACE` mutation here can never race the env-reading
/// statics used by other tests in this process.
#[test]
fn query_only_without_strict_space_pin_rejects_before_bind() {
    let scenarios: [(&str, Option<&str>, Option<&str>); 3] = [
        ("space unset, no default", None, None),
        ("space unset, default only", None, Some("fallback")),
        ("space whitespace-only", Some("   "), None),
    ];

    for (label, space, default_space) in scenarios {
        let port = portpicker::pick_unused_port().expect("no free port");
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_wenlan-mcp"));
        command.env("WENLAN_NO_AUTOSTART", "1");
        command.args([
            "serve",
            "--tool-profile",
            "query-only",
            "--token",
            "test-token",
            "--port",
            &port.to_string(),
        ]);
        match space {
            Some(value) => {
                command.env("WENLAN_SPACE", value);
            }
            None => {
                command.env_remove("WENLAN_SPACE");
            }
        }
        match default_space {
            Some(value) => {
                command.env("WENLAN_DEFAULT_SPACE", value);
            }
            None => {
                command.env_remove("WENLAN_DEFAULT_SPACE");
            }
        }

        let output = output_of_rejected_serve(command);
        assert!(
            !output.status.success(),
            "{label}: must reject without a strict Space pin"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(wenlan_mcp::serve::QUERY_ONLY_SPACE_ERROR),
            "{label}: stderr={stderr}"
        );
        assert!(
            std::net::TcpListener::bind(("127.0.0.1", port)).is_ok(),
            "{label}: port must remain free — process must exit before binding"
        );
    }
}

/// Each invalid scope combination must exit with its own error before the
/// server binds, in a child process so env mutation cannot race other tests.
#[test]
fn whole_library_scope_conflicts_reject_before_bind() {
    let scenarios: [(&str, &str, Option<&str>, bool, &str); 5] = [
        (
            "web-write whole library plus a Space pin",
            "web-write",
            Some("work"),
            true,
            wenlan_mcp::serve::WHOLE_LIBRARY_PIN_CONFLICT_ERROR,
        ),
        (
            "web-write pin spelled as the reserved value",
            "web-write",
            Some("*"),
            false,
            wenlan_mcp::serve::RESERVED_SPACE_PIN_ERROR,
        ),
        (
            "whole library plus a Space pin",
            "query-only",
            Some("work"),
            true,
            wenlan_mcp::serve::WHOLE_LIBRARY_PIN_CONFLICT_ERROR,
        ),
        (
            "pin spelled as the reserved value",
            "query-only",
            Some("*"),
            false,
            wenlan_mcp::serve::RESERVED_SPACE_PIN_ERROR,
        ),
        (
            "whole library on the standard profile",
            "standard",
            None,
            true,
            wenlan_mcp::serve::WHOLE_LIBRARY_PROFILE_ERROR,
        ),
    ];

    for (label, profile, space, whole_library, expected) in scenarios {
        let port = portpicker::pick_unused_port().expect("no free port");
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_wenlan-mcp"));
        command
            .env("WENLAN_NO_AUTOSTART", "1")
            .env_remove("WENLAN_DEFAULT_SPACE");
        command.args([
            "serve",
            "--tool-profile",
            profile,
            "--token",
            "test-token",
            "--port",
            &port.to_string(),
        ]);
        if whole_library {
            command.arg("--whole-library");
        }
        match space {
            Some(value) => command.env("WENLAN_SPACE", value),
            None => command.env_remove("WENLAN_SPACE"),
        };

        let output = output_of_rejected_serve(command);
        assert!(!output.status.success(), "{label}: must be rejected");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(expected), "{label}: stderr={stderr}");
        assert!(
            std::net::TcpListener::bind(("127.0.0.1", port)).is_ok(),
            "{label}: port must remain free; process must exit before binding"
        );
    }
}

/// Read the JSON-RPC payload from a Streamable HTTP response, which arrives
/// either as plain JSON or as an SSE `data:` event after an empty priming event.
async fn rpc_result(response: reqwest::Response) -> serde_json::Value {
    let text = response.text().await.unwrap();
    let body = text
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .find(|data| !data.trim().is_empty())
        .unwrap_or(&text);
    serde_json::from_str(body.trim()).unwrap_or_else(|error| panic!("{error}: {text}"))
}

#[tokio::test]
async fn whole_library_reports_star_and_keeps_the_space_argument() {
    struct ChildGuard(std::process::Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    use wiremock::{matchers::path, Mock, MockServer, ResponseTemplate};

    // (scope flag, WENLAN_SPACE, connector-info space, recall keeps `space`,
    //  daemon-side space for a recall without and with `space: "work"`)
    for (whole_library, pin, expected_space, keeps_space, sent) in [
        (true, None, "*", true, [None, Some("work")]),
        (
            false,
            Some("synthetic-review"),
            "synthetic-review",
            false,
            [Some("synthetic-review"), Some("synthetic-review")],
        ),
    ] {
        let daemon = MockServer::start().await;
        Mock::given(path("/api/memory/search"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"results": [], "took_ms": 1.0})),
            )
            .mount(&daemon)
            .await;
        let port = portpicker::pick_unused_port().expect("no free port");
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_wenlan-mcp"));
        command
            .env("WENLAN_NO_AUTOSTART", "1")
            // An inherited fallback must never narrow a whole-library connector.
            .env("WENLAN_DEFAULT_SPACE", "fallback")
            .args([
                "--origin-url",
                &daemon.uri(),
                "serve",
                "--tool-profile",
                "query-only",
                "--token",
                "synthetic-connector-token",
                "--port",
                &port.to_string(),
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        if whole_library {
            command.arg("--whole-library");
        }
        match pin {
            Some(value) => command.env("WENLAN_SPACE", value),
            None => command.env_remove("WENLAN_SPACE"),
        };
        let _guard = ChildGuard(command.spawn().expect("binary must start"));
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        let base = format!("http://127.0.0.1:{port}");
        tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                if client
                    .get(format!("{base}/health"))
                    .send()
                    .await
                    .is_ok_and(|response| response.status().is_success())
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("server must become ready");

        let info: serde_json::Value = client
            .get(format!("{base}/connector-info"))
            .bearer_auth("synthetic-connector-token")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(info["space"], expected_space);

        let initialized = client
            .post(format!("{base}/mcp"))
            .header("Accept", "application/json, text/event-stream")
            .bearer_auth("synthetic-connector-token")
            .json(&serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                    "clientInfo": {"name": "synthetic", "version": "1"}}
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(initialized.status(), 200);
        let session_id = initialized.headers()["mcp-session-id"]
            .to_str()
            .unwrap()
            .to_owned();
        initialized.text().await.unwrap();
        let notified = client
            .post(format!("{base}/mcp"))
            .header("Accept", "application/json, text/event-stream")
            .header("Mcp-Session-Id", &session_id)
            .bearer_auth("synthetic-connector-token")
            .json(&serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .send()
            .await
            .unwrap();
        assert_eq!(notified.status(), 202);
        let listed = client
            .post(format!("{base}/mcp"))
            .header("Accept", "application/json, text/event-stream")
            .header("Mcp-Session-Id", &session_id)
            .bearer_auth("synthetic-connector-token")
            .json(&serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}))
            .send()
            .await
            .unwrap();
        let listed = rpc_result(listed).await;
        let recall = listed["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "recall")
            .expect("query-only lists recall");
        assert_eq!(
            recall["inputSchema"]["properties"].get("space").is_some(),
            keeps_space,
            "whole library lets the model narrow; a pin hides the argument"
        );

        for (id, arguments) in [
            (3, serde_json::json!({"query": "synthetic"})),
            (
                4,
                serde_json::json!({"query": "synthetic", "space": "work"}),
            ),
        ] {
            let called = client
                .post(format!("{base}/mcp"))
                .header("Accept", "application/json, text/event-stream")
                .header("Mcp-Session-Id", &session_id)
                .bearer_auth("synthetic-connector-token")
                .json(
                    &serde_json::json!({"jsonrpc":"2.0","id":id,"method":"tools/call",
                    "params": {"name": "recall", "arguments": arguments}}),
                )
                .send()
                .await
                .unwrap();
            let called = rpc_result(called).await;
            assert!(called["error"].is_null(), "recall failed: {called}");
        }
        let searches: Vec<_> = daemon
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .filter(|request| request.url.path() == "/api/memory/search")
            .collect();
        assert_eq!(searches.len(), 2);
        for (request, expected) in searches.iter().zip(sent) {
            let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            assert_eq!(body["space"].as_str(), expected, "search body {body}");
            assert_eq!(
                request
                    .headers
                    .get("x-wenlan-space")
                    .map(|value| value.to_str().unwrap()),
                pin,
                "only a pin sends the Space header"
            );
        }
    }
}

fn test_config(port: u16, token: Option<String>) -> wenlan_mcp::serve::ServeConfig {
    wenlan_mcp::serve::ServeConfig {
        port,
        host: "127.0.0.1".into(),
        origin_url: "http://127.0.0.1:19999".into(), // non-existent, OK for these tests
        token,
        agent_name: "test-agent".into(),
        user_id: None,
        allowed_origins: vec!["*".into()],
        whole_library: false,
    }
}

#[tokio::test]
async fn connector_info_is_authenticated_and_query_only() {
    struct ChildGuard(std::process::Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    for profile in ["query-only", "standard"] {
        let port = portpicker::pick_unused_port().expect("no free port");
        let child = std::process::Command::new(env!("CARGO_BIN_EXE_wenlan-mcp"))
            .env("WENLAN_NO_AUTOSTART", "1")
            .env("WENLAN_SPACE", "synthetic-review")
            .env_remove("WENLAN_DEFAULT_SPACE")
            .args([
                "--origin-url",
                "http://127.0.0.1:19999",
                "serve",
                "--tool-profile",
                profile,
                "--token",
                "synthetic-connector-token",
                "--port",
                &port.to_string(),
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("binary must start");
        let _guard = ChildGuard(child);
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        let base = format!("http://127.0.0.1:{port}");
        tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                if client
                    .get(format!("{base}/health"))
                    .send()
                    .await
                    .is_ok_and(|response| response.status().is_success())
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("server must become ready");

        let invalid_init = serde_json::json!({
            "jsonrpc": "2.0", "method": "initialize",
            "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                "clientInfo": {"name": "synthetic", "version": "1"}}
        });
        let anonymous_init = client
            .post(format!("{base}/mcp"))
            .header("Accept", "application/json, text/event-stream")
            .json(&invalid_init)
            .send()
            .await
            .unwrap();
        assert_eq!(
            anonymous_init.status(),
            401,
            "auth must run before initialization parsing"
        );
        let rejected_init = client
            .post(format!("{base}/mcp"))
            .header("Accept", "application/json, text/event-stream")
            .bearer_auth("synthetic-connector-token")
            .json(&invalid_init)
            .send()
            .await
            .unwrap();
        assert_eq!(
            rejected_init.status(),
            400,
            "every HTTP tool profile applies the pre-allocation initialization guard"
        );
        assert!(rejected_init.headers().get("mcp-session-id").is_none());
        let mut valid_init = invalid_init.clone();
        valid_init["id"] = serde_json::json!(1);
        let initialized = client
            .post(format!("{base}/mcp"))
            .header("Accept", "application/json, text/event-stream")
            .bearer_auth("synthetic-connector-token")
            .json(&valid_init)
            .send()
            .await
            .unwrap();
        assert_eq!(initialized.status(), 200);
        let session_id = initialized.headers()["mcp-session-id"]
            .to_str()
            .unwrap()
            .to_owned();
        initialized.text().await.unwrap();
        if profile == "query-only" {
            let notified = client
                .post(format!("{base}/mcp"))
                .header("Accept", "application/json, text/event-stream")
                .header("Mcp-Session-Id", &session_id)
                .bearer_auth("synthetic-connector-token")
                .json(&serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
                .send()
                .await
                .unwrap();
            assert_eq!(notified.status(), 202);
            let mut events = client
                .get(format!("{base}/mcp"))
                .header("Accept", "text/event-stream")
                .header("Mcp-Session-Id", &session_id)
                .bearer_auth("synthetic-connector-token")
                .send()
                .await
                .unwrap();
            assert_eq!(events.status(), 200);
            let heartbeat = tokio::time::timeout(Duration::from_millis(1800), async {
                let mut text = String::new();
                while let Some(bytes) = events.chunk().await.unwrap() {
                    text.push_str(&String::from_utf8_lossy(&bytes));
                    assert!(text.len() < 4096);
                    if text.lines().any(|line| line.starts_with(':')) {
                        return;
                    }
                }
                panic!("SDK SSE stream ended before its heartbeat");
            })
            .await;
            drop(events);
            assert!(
                heartbeat.is_ok(),
                "query-only SDK heartbeat must arrive within 1.8s"
            );
        }
        let deleted = client
            .delete(format!("{base}/mcp"))
            .header("Mcp-Session-Id", session_id)
            .bearer_auth("synthetic-connector-token")
            .send()
            .await
            .unwrap();
        assert_eq!(deleted.status(), 202);

        for token in [None, Some("wrong-token")] {
            let mut request = client.get(format!("{base}/connector-info"));
            if let Some(token) = token {
                request = request.bearer_auth(token);
            }
            let response = request.send().await.unwrap();
            assert_eq!(response.status(), 401);
            assert!(!response.text().await.unwrap().contains("synthetic-review"));
        }
        let response = client
            .get(format!("{base}/connector-info"))
            .bearer_auth("synthetic-connector-token")
            .send()
            .await
            .unwrap();
        if profile == "standard" {
            assert_eq!(response.status(), 404);
            continue;
        }
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(
            response.json::<serde_json::Value>().await.unwrap(),
            serde_json::json!({
                "contract_version": 1,
                "server": "wenlan-mcp",
                "tool_profile": "query-only",
                "space": "synthetic-review",
                "authentication": "bearer",
            })
        );
        let health = client
            .get(format!("{base}/health"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(!health.contains("synthetic-review"));
    }
}

#[tokio::test]
async fn version_handshake_warns_when_daemon_minor_ahead() {
    use wiremock::{matchers::path, Mock, MockServer, ResponseTemplate};

    let mock_daemon = MockServer::start().await;
    Mock::given(path("/api/health"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "status": "ok",
            "db_initialized": true,
            "version": "9.9.9"
        })))
        .mount(&mock_daemon)
        .await;

    let client = wenlan_mcp::client::WenlanClient::new(mock_daemon.uri());
    let warning = client.version_handshake().await;
    assert!(warning.is_some(), "expected a warning when daemon ahead");
    let msg = warning.unwrap();
    assert!(msg.contains("wenlan-mcp"), "msg={msg}");
    assert!(msg.contains("brew upgrade wenlan-mcp"), "msg={msg}");
}

#[tokio::test]
async fn version_handshake_silent_when_compatible() {
    use wiremock::{matchers::path, Mock, MockServer, ResponseTemplate};

    let mock_daemon = MockServer::start().await;
    Mock::given(path("/api/health"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "status": "ok",
            "db_initialized": true,
            "version": env!("CARGO_PKG_VERSION")
        })))
        .mount(&mock_daemon)
        .await;

    let client = wenlan_mcp::client::WenlanClient::new(mock_daemon.uri());
    assert_eq!(client.version_handshake().await, None);
}

#[tokio::test]
async fn version_handshake_silent_when_daemon_unreachable() {
    let port = portpicker::pick_unused_port().expect("no free port");
    let client = wenlan_mcp::client::WenlanClient::new(format!("http://127.0.0.1:{port}"));
    assert_eq!(client.version_handshake().await, None);
}
