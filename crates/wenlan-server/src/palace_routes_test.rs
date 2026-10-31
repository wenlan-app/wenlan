// SPDX-License-Identifier: Apache-2.0
//! Route contracts for `/api/palace/layout` (UX-GRAPH-1..3).

use crate::state::ServerState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use std::sync::Arc;
use tokio::sync::RwLock;
use tower::ServiceExt;
use wenlan_types::palace::{PalaceAuthor, PalaceLayoutResponse};

fn state_at(root: &std::path::Path) -> Arc<RwLock<ServerState>> {
    Arc::new(RwLock::new(ServerState {
        palace_root: Some(root.to_path_buf()),
        ..Default::default()
    }))
}

async fn call(
    state: &Arc<RwLock<ServerState>>,
    method: &str,
    uri: &str,
    agent: Option<&str>,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let app = crate::router::build_router(state.clone());
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(agent) = agent {
        request = request.header("x-agent-name", agent);
    }
    let body = match body {
        Some(json) => {
            request = request.header("content-type", "application/json");
            Body::from(json.to_string())
        }
        None => Body::empty(),
    };
    let response = app.oneshot(request.body(body).unwrap()).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

#[tokio::test]
async fn the_layout_is_stored_version_checked_and_survives_a_restart() {
    let tmp = tempfile::tempdir().unwrap();
    let state = state_at(tmp.path());
    let (status, json) = call(&state, "GET", "/api/palace/layout", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json,
        serde_json::json!({"version": 0, "layout": {"nodes": {}}})
    );

    let (status, json) = call(
        &state,
        "PUT",
        "/api/palace/layout",
        None,
        Some(serde_json::json!({
            "expected_version": 0,
            "layout": {
                "nodes": {"page_a": {"x": 10.0, "y": 20.0}},
                "lines": [{"id": "l1", "from": "page_a", "to": "page_b", "added_by": "person"}],
                "circles": [{"id": "c1", "members": ["page_a", "page_b"], "label": "Storage"}],
                "notes": [{"id": "n1", "text": "ask about this", "x": 1.0, "y": 2.0}],
                "removed": ["ent_old"]
            }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    let written: PalaceLayoutResponse = serde_json::from_value(json).unwrap();
    assert_eq!(written.version, 1);

    // A new server over the same folder (a restart) reads the same layout.
    let (_, json) = call(
        &state_at(tmp.path()),
        "GET",
        "/api/palace/layout",
        None,
        None,
    )
    .await;
    let reread: PalaceLayoutResponse = serde_json::from_value(json).unwrap();
    assert_eq!(reread.layout, written.layout);
    assert!(tmp.path().join("palace.json").exists());

    // A write based on version 0 is refused with the stored version.
    let (status, json) = call(
        &state,
        "PUT",
        "/api/palace/layout",
        None,
        Some(serde_json::json!({"expected_version": 0, "layout": {"nodes": {}}})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(json["error"], "palace_version_conflict");
    assert_eq!(json["current_version"], 1);
}

#[tokio::test]
async fn a_relayout_is_reverted_and_ai_additions_stay_suggestions() {
    let tmp = tempfile::tempdir().unwrap();
    let state = state_at(tmp.path());
    call(
        &state,
        "PUT",
        "/api/palace/layout",
        None,
        Some(serde_json::json!({"expected_version": 0, "layout": {"nodes": {"a": {"x": 1.0, "y": 1.0}}}})),
    )
    .await;
    let (status, json) = call(
        &state,
        "PUT",
        "/api/palace/layout",
        None,
        Some(serde_json::json!({
            "expected_version": 1,
            "reason": "relayout",
            "layout": {"nodes": {"a": {"x": 99.0, "y": 99.0}}}
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["reason"], "relayout");

    let (status, json) = call(
        &state,
        "POST",
        "/api/palace/layout/revert",
        None,
        Some(serde_json::json!({"expected_version": 2})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["version"], 3);
    assert_eq!(
        json["layout"]["nodes"]["a"],
        serde_json::json!({"x": 1.0, "y": 1.0})
    );

    // An AI tool adds a point it calls accepted; it is stored as a suggestion.
    let (status, json) = call(
        &state,
        "PUT",
        "/api/palace/layout",
        Some("claude-code"),
        Some(serde_json::json!({
            "expected_version": 3,
            "layout": {"nodes": {
                "a": {"x": 1.0, "y": 1.0},
                "idea": {"x": 5.0, "y": 5.0, "label": "Cache", "accepted": true}
            }}
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    let after: PalaceLayoutResponse = serde_json::from_value(json).unwrap();
    assert_eq!(after.layout.nodes["idea"].added_by, Some(PalaceAuthor::Ai));
    assert_eq!(after.layout.nodes["idea"].accepted, Some(false));

    // An AI tool can neither relayout nor revert.
    let (status, _) = call(
        &state,
        "PUT",
        "/api/palace/layout",
        Some("claude-code"),
        Some(serde_json::json!({"expected_version": 4, "reason": "relayout", "layout": {"nodes": {}}})),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = call(
        &state,
        "POST",
        "/api/palace/layout/revert",
        Some("claude-code"),
        Some(serde_json::json!({"expected_version": 4})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // A bad position is refused.
    let (status, _) = call(
        &state,
        "PUT",
        "/api/palace/layout",
        None,
        Some(serde_json::json!({"expected_version": 4, "layout": {"nodes": {"": {"x": 0.0, "y": 0.0}}}})),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}
