// SPDX-License-Identifier: Apache-2.0
mod common;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use tower::ServiceExt;
use wenlan_core::truth_contract::{CONTRACT_HEADER, INTENT_HEADER};
use wenlan_server::{router::build_router, state::ServerState};
use wenlan_types::requests::{UpdateAgentRequest, UpdateProfileRequest};
use wenlan_types::responses::{AgentResponse, ProfileResponse};

#[derive(Debug, Deserialize)]
struct ErrorEnvelope {
    error: String,
}

#[derive(Debug, Deserialize)]
struct DeleteAgentResponse {
    deleted: String,
}

fn json_body<T: Serialize>(value: &T) -> Body {
    Body::from(serde_json::to_vec(value).unwrap())
}

async fn request_bytes(
    router: &common::AppRouter,
    method: Method,
    uri: &str,
    body: Body,
    marked: bool,
) -> (StatusCode, Vec<u8>) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    if marked {
        request = request
            .header(INTENT_HEADER, "explicit")
            .header(CONTRACT_HEADER, "1")
            .header("x-agent-name", "profile-agent-route-contract");
    }
    let response = router
        .clone()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 256 * 1024)
        .await
        .unwrap()
        .to_vec();
    (status, bytes)
}

async fn request_typed<T>(
    router: &common::AppRouter,
    method: Method,
    uri: &str,
    body: Body,
) -> (StatusCode, T)
where
    T: DeserializeOwned,
{
    let (status, bytes) = request_bytes(router, method.clone(), uri, body, false).await;
    let decoded = serde_json::from_slice::<T>(&bytes).unwrap_or_else(|error| {
        panic!(
            "{method} {uri} returned a non-contract response ({error}): {}",
            String::from_utf8_lossy(&bytes)
        )
    });
    (status, decoded)
}

#[tokio::test]
async fn moved_profile_and_agent_handlers_preserve_typed_contracts() {
    let (router, _tmp, db) = common::test_app_no_gate().await;

    let (status, profile): (StatusCode, ProfileResponse) =
        request_typed(&router, Method::GET, "/api/profile", Body::empty()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(profile.name, "User");

    let profile_update = UpdateProfileRequest {
        name: Some("Route Contract User".to_string()),
        display_name: Some("Contract User".to_string()),
        email: Some("contract@example.test".to_string()),
        bio: Some("Typed profile route evidence.".to_string()),
        avatar_path: Some("/tmp/profile-avatar.png".to_string()),
    };
    let (status, profile): (StatusCode, ProfileResponse) = request_typed(
        &router,
        Method::PUT,
        "/api/profile",
        json_body(&profile_update),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(profile.name, "Route Contract User");
    assert_eq!(profile.display_name.as_deref(), Some("Contract User"));
    assert_eq!(profile.email.as_deref(), Some("contract@example.test"));

    let seeded = db.register_agent("Route Contract Agent").await.unwrap();
    let agent_uri = format!("/api/agents/{}", seeded.name);

    let (status, agents): (StatusCode, Vec<AgentResponse>) =
        request_typed(&router, Method::GET, "/api/agents", Body::empty()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].name, seeded.name);

    let (status, agent): (StatusCode, AgentResponse) =
        request_typed(&router, Method::GET, &agent_uri, Body::empty()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(agent.name, seeded.name);

    let agent_update = UpdateAgentRequest {
        agent_type: Some("mcp".to_string()),
        description: Some("Updated through the typed route.".to_string()),
        enabled: Some(false),
        trust_level: Some("review".to_string()),
        display_name: Some("Route Contract".to_string()),
    };
    let (status, agent): (StatusCode, AgentResponse) =
        request_typed(&router, Method::PUT, &agent_uri, json_body(&agent_update)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(agent.agent_type, "mcp");
    assert_eq!(
        agent.description.as_deref(),
        Some("Updated through the typed route.")
    );
    assert!(!agent.enabled);
    assert_eq!(agent.trust_level, "review");
    assert_eq!(agent.display_name.as_deref(), Some("Route Contract"));

    let (status, body) =
        request_bytes(&router, Method::DELETE, &agent_uri, Body::empty(), true).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "the opaque delete response must retain its None marker shape"
    );
    let refusal: ErrorEnvelope = serde_json::from_slice(&body).unwrap();
    assert!(refusal.error.contains("reader-intent marker"));

    let (status, deleted): (StatusCode, DeleteAgentResponse) =
        request_typed(&router, Method::DELETE, &agent_uri, Body::empty()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(deleted.deleted, seeded.name);

    let (status, error): (StatusCode, ErrorEnvelope) =
        request_typed(&router, Method::GET, &agent_uri, Body::empty()).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(error.error.contains("not found"));

    let (status, error): (StatusCode, ErrorEnvelope) =
        request_typed(&router, Method::PUT, &agent_uri, json_body(&agent_update)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(error.error.contains("not found"));

    let no_db = build_router(Arc::new(RwLock::new(ServerState::default())));
    let no_db_cases = vec![
        (Method::GET, "/api/profile", Vec::new()),
        (
            Method::PUT,
            "/api/profile",
            serde_json::to_vec(&profile_update).unwrap(),
        ),
        (Method::GET, "/api/agents", Vec::new()),
        (Method::GET, "/api/agents/missing-agent", Vec::new()),
        (
            Method::PUT,
            "/api/agents/missing-agent",
            serde_json::to_vec(&agent_update).unwrap(),
        ),
        (Method::DELETE, "/api/agents/missing-agent", Vec::new()),
    ];
    for (method, uri, body) in no_db_cases {
        let (status, error): (StatusCode, ErrorEnvelope) =
            request_typed(&no_db, method.clone(), uri, Body::from(body)).await;
        assert_eq!(
            status,
            StatusCode::SERVICE_UNAVAILABLE,
            "{method} {uri} did not preserve its deterministic no-DB error"
        );
        assert_eq!(error.error, "Database not initialized");
    }
}

fn register_body(name: &str) -> Body {
    json_body(&wenlan_types::requests::RegisterAgentRequest {
        name: name.to_string(),
    })
}

#[tokio::test]
async fn post_agents_registers_a_connected_tool_as_trusted() {
    let (router, _tmp, db) = common::test_app_no_gate().await;

    let (status, agent): (StatusCode, AgentResponse) = request_typed(
        &router,
        Method::POST,
        "/api/agents",
        register_body("Claude Code"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(agent.name, "claude-code");
    assert_eq!(agent.trust_level, "full");
    assert!(agent.enabled);
    assert_eq!(
        agent.last_seen_at, None,
        "registering is not a sighting: the app counts a tool as connected only once it is seen"
    );
    assert_eq!(agent.memory_count, 0);

    // Idempotent: the same call again returns the same row.
    let (status, again): (StatusCode, AgentResponse) = request_typed(
        &router,
        Method::POST,
        "/api/agents",
        register_body("claude-code"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(again.id, agent.id);
    assert_eq!(db.list_agents().await.unwrap().len(), 1);
}

#[tokio::test]
async fn post_agents_promotes_an_untouched_read_created_row() {
    let (router, _tmp, db) = common::test_app_no_gate().await;
    db.record_agent_presence("cursor").await.unwrap();
    assert_eq!(
        db.get_agent("cursor").await.unwrap().unwrap().trust_level,
        "unknown"
    );

    let (status, agent): (StatusCode, AgentResponse) = request_typed(
        &router,
        Method::POST,
        "/api/agents",
        register_body("cursor"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(agent.trust_level, "full");
    assert_eq!(
        db.get_agent("cursor").await.unwrap().unwrap().trust_level,
        "full"
    );
}

#[tokio::test]
async fn post_agents_leaves_a_user_edited_row_as_it_is() {
    let (router, _tmp, db) = common::test_app_no_gate().await;
    db.record_agent_presence("codex").await.unwrap();
    db.update_agent("codex", None, Some("hands off"), None, None, None)
        .await
        .unwrap();
    let before = db.get_agent("codex").await.unwrap().unwrap();
    assert_eq!(before.trust_level, "unknown");
    assert_ne!(before.updated_at, before.created_at);

    let (status, agent): (StatusCode, AgentResponse) =
        request_typed(&router, Method::POST, "/api/agents", register_body("codex")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(agent.trust_level, "unknown");
    let after = db.get_agent("codex").await.unwrap().unwrap();
    assert_eq!(after.trust_level, "unknown");
    assert_eq!(after.updated_at, before.updated_at);
}

#[tokio::test]
async fn post_agents_rejects_invalid_names_with_400_and_creates_nothing() {
    let (router, _tmp, db) = common::test_app_no_gate().await;
    let too_long = "x".repeat(65);
    for bad in [
        "",
        "   ",
        "unknown",
        "wenlan-setup",
        "bad\u{0}name",
        too_long.as_str(),
    ] {
        let (status, error): (StatusCode, ErrorEnvelope) =
            request_typed(&router, Method::POST, "/api/agents", register_body(bad)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad:?}");
        assert!(error.error.contains("name"), "{bad:?}: {}", error.error);
    }
    assert!(db.list_agents().await.unwrap().is_empty());

    let no_db = build_router(Arc::new(RwLock::new(ServerState::default())));
    let (status, error): (StatusCode, ErrorEnvelope) =
        request_typed(&no_db, Method::POST, "/api/agents", register_body("cursor")).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error.error, "Database not initialized");
}
