use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};
use wenlan_mcp::client::WenlanClient;
use wenlan_mcp::tools::{TransportMode, WenlanMcpServer};

#[tokio::test]
async fn idle_and_abandoned_http_sessions_are_removed_by_the_pinned_sdk() {
    let mut manager = LocalSessionManager::default();
    assert_eq!(
        manager.session_config.keep_alive,
        Some(Duration::from_secs(300)),
        "the SDK default; production raises it (serve::SESSION_IDLE_LIMIT) so a slow call is not cut off",
    );
    // Exercise the real timeout/cleanup path without a five-minute wall-clock test.
    manager.session_config.keep_alive = Some(Duration::from_millis(500));
    let manager = Arc::new(manager);
    let service = StreamableHttpService::new(
        || {
            Ok(WenlanMcpServer::new(
                WenlanClient::new("http://127.0.0.1:1".into()),
                TransportMode::Http,
                "synthetic-retention-test".into(),
                None,
            ))
        },
        manager.clone(),
        StreamableHttpServerConfig::default(),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, Router::new().nest_service("/mcp", service))
            .await
            .unwrap();
    });
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();

    // Always stop the owned listener before propagating an assertion failure.
    let result = tokio::spawn(async move {
        for complete_initialization in [false, true] {
            let response = client
                .post(&endpoint)
                .header("Accept", "application/json, text/event-stream")
                .json(&serde_json::json!({
                    "jsonrpc": "2.0", "id": 1, "method": "initialize",
                    "params": {
                        "protocolVersion": "2025-06-18", "capabilities": {},
                        "clientInfo": { "name": "synthetic-retention-test", "version": "1" }
                    }
                }))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            let id = response.headers()["mcp-session-id"]
                .to_str()
                .unwrap()
                .to_owned();
            assert!(manager.sessions.read().await.contains_key(id.as_str()));
            if complete_initialization {
                response.text().await.unwrap();
                let initialized = client
                    .post(&endpoint)
                    .header("Accept", "application/json, text/event-stream")
                    .header("Mcp-Session-Id", &id)
                    .header("Mcp-Protocol-Version", "2025-06-18")
                    .json(&serde_json::json!({
                        "jsonrpc": "2.0", "method": "notifications/initialized"
                    }))
                    .send()
                    .await
                    .unwrap();
                assert_eq!(initialized.status(), 202);
            } else {
                // No body consumption, initialized notification or DELETE:
                // the caller has abandoned the successful initialization.
                drop(response);
            }
        }

        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if manager.sessions.read().await.is_empty() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("idle session tasks must terminate and remove their manager entries");
    })
    .await;
    server.abort();
    let stopped = server.await;
    assert!(stopped.is_err_and(|error| error.is_cancelled()));
    result.unwrap();
}
