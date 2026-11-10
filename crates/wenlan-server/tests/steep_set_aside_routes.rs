// SPDX-License-Identifier: Apache-2.0
//! `GET /api/steep/new/set-aside` lists the groups Automatic set aside and
//! `POST /api/steep/new/set-aside/release` gives one back. The record is
//! seeded in its first format (a bare `item_id@last_modified` list), which
//! the daemon reads and migrates.

mod common;

use axum::{
    body::{to_bytes, Body},
    http::{Method, Request, StatusCode},
};
use serde_json::{json, Value};
use tower::ServiceExt;
use wenlan_server::router::AppRouter;
use wenlan_types::{
    SteepNewListResponse, SteepSetAsideListResponse, SteepSetAsideReason,
    SteepSetAsideReleaseResponse,
};

async fn send(
    app: &AppRouter,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let builder = Request::builder().method(method).uri(uri);
    let request = match body {
        Some(body) => builder
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn set_aside_groups_are_listed_and_given_back() {
    let (app, _dir, db) = common::test_app().await;
    let now = chrono::Utc::now().timestamp();
    for (id, text) in [
        ("sa_drift", "Driftwood washes up after the autumn storms."),
        (
            "sa_book",
            "The book's second chapter argues for slow reading.",
        ),
    ] {
        db.upsert_documents_without_embeddings(vec![wenlan_types::RawDocument {
            source: "memory".to_string(),
            source_id: id.to_string(),
            title: id.to_string(),
            content: text.to_string(),
            source_agent: Some("claude-code".to_string()),
            last_modified: now,
            ..Default::default()
        }])
        .await
        .unwrap();
    }
    let (status, body) = send(&app, Method::GET, "/api/steep/new", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let listed: SteepNewListResponse = serde_json::from_value(body).unwrap();
    let at = |id: &str| {
        listed
            .items
            .iter()
            .find(|item| item.item_id == id)
            .unwrap_or_else(|| panic!("{id} waits"))
            .last_modified
    };
    let legacy = json!([
        format!("sa_drift@{}", at("sa_drift")),
        format!("sa_book@{}", at("sa_book")),
    ]);
    db.set_app_metadata("steep_new_automatic_failed_items_v1", &legacy.to_string())
        .await
        .unwrap();

    let (status, body) = send(&app, Method::GET, "/api/steep/new/set-aside", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let list: SteepSetAsideListResponse = serde_json::from_value(body).unwrap();
    assert_eq!(list.groups.len(), 2);
    assert!(list
        .groups
        .iter()
        .all(|group| group.reason == SteepSetAsideReason::Other && group.items.len() == 1));
    let drift = list
        .groups
        .iter()
        .find(|group| group.items[0].item_id == "sa_drift")
        .expect("the driftwood item is set aside");
    assert_eq!(drift.name, drift.items[0].title);

    let (status, body) = send(
        &app,
        Method::POST,
        "/api/steep/new/set-aside/release",
        Some(json!({"item_ids": ["sa_drift", "not_set_aside"]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let released: SteepSetAsideReleaseResponse = serde_json::from_value(body).unwrap();
    assert_eq!(released.released, 1);

    let (_, body) = send(&app, Method::GET, "/api/steep/new/set-aside", None).await;
    let list: SteepSetAsideListResponse = serde_json::from_value(body).unwrap();
    let left: Vec<&str> = list
        .groups
        .iter()
        .map(|group| group.items[0].item_id.as_str())
        .collect();
    assert_eq!(left, vec!["sa_book"]);
}
