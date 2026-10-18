// SPDX-License-Identifier: Apache-2.0

mod common;

use axum::{
    body::{to_bytes, Body},
    http::{Method, Request, StatusCode},
};
use std::sync::Arc;
use tokio::sync::RwLock;
use tower::ServiceExt;
use wenlan_core::{db::MemoryDB, events::NoopEmitter, sources::RawDocument};
use wenlan_server::{router::build_router, state::ServerState};
use wenlan_types::{BriefItemState, BriefReadResponse, BriefReadState, BriefUpdateReceipt};

async fn fixture() -> (
    wenlan_server::router::AppRouter,
    tempfile::TempDir,
    Arc<MemoryDB>,
) {
    let temp = tempfile::tempdir().unwrap();
    let db = Arc::new(
        MemoryDB::new(&temp.path().join("db"), Arc::new(NoopEmitter))
            .await
            .unwrap(),
    );
    let state = ServerState {
        db: Some(Arc::clone(&db)),
        ..ServerState::default()
    }
    .with_brief_status_root(temp.path().join("status"));
    (build_router(Arc::new(RwLock::new(state))), temp, db)
}

async fn json_request(
    app: wenlan_server::router::AppRouter,
    method: Method,
    body: serde_json::Value,
    space_header: Option<&str>,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri("/api/brief")
        .header("content-type", "application/json");
    if let Some(space) = space_header {
        builder = builder.header("x-wenlan-space", space);
    }
    let response = app
        .oneshot(builder.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

fn update(space: &str, operation_id: &str, text: &str) -> serde_json::Value {
    serde_json::json!({
        "space": space,
        "caller_id": "brief-route-test",
        "operation_id": operation_id,
        "mutations": [{
            "kind": "add",
            "text": text,
            "state": "active"
        }]
    })
}

#[tokio::test]
async fn unresolved_read_is_typed_and_write_free() {
    let (app, _temp, db) = fixture().await;

    let (status, body) = json_request(app, Method::POST, serde_json::json!({}), None).await;

    assert_eq!(status, StatusCode::OK);
    let response: BriefReadResponse = serde_json::from_value(body).unwrap();
    assert_eq!(response.state, BriefReadState::SpaceNotResolved);
    assert!(db.list_spaces().await.unwrap().is_empty());
}

#[tokio::test]
async fn registered_space_without_brief_is_typed_and_write_free() {
    let (app, _temp, db) = fixture().await;
    db.create_space("work", None, false).await.unwrap();

    let (status, body) = json_request(
        app,
        Method::POST,
        serde_json::json!({"space": "work"}),
        None,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    let response: BriefReadResponse = serde_json::from_value(body).unwrap();
    assert_eq!(response.state, BriefReadState::BriefNotCreated);
    assert_eq!(response.space.as_deref(), Some("work"));
    assert!(db.get_brief_by_space_name("work").await.unwrap().is_none());
}

#[tokio::test]
async fn no_topic_returns_the_complete_brief_without_related_context() {
    let (app, _temp, db) = fixture().await;
    db.apply_brief_update(
        &serde_json::from_value(update("work", "create-work", "ship release")).unwrap(),
    )
    .await
    .unwrap();

    let (_, body) = json_request(
        app,
        Method::POST,
        serde_json::json!({"space": "work"}),
        None,
    )
    .await;
    let response: BriefReadResponse = serde_json::from_value(body).unwrap();

    assert_eq!(response.state, BriefReadState::Ready);
    assert_eq!(response.brief.unwrap().active[0].text, "ship release");
    assert!(response.related_context.is_none());
}

#[tokio::test]
async fn topic_keeps_the_complete_brief_and_recalls_only_inside_the_same_space() {
    let (app, _temp, db) = fixture().await;
    db.apply_brief_update(
        &serde_json::from_value(update("work", "create-work", "ship release")).unwrap(),
    )
    .await
    .unwrap();
    db.create_space("personal", None, false).await.unwrap();
    for (source_id, content, space) in [
        (
            "work-release",
            "release gate work-only marker",
            Some("work".to_string()),
        ),
        (
            "personal-release",
            "release gate personal-only marker",
            Some("personal".to_string()),
        ),
    ] {
        db.upsert_documents(vec![RawDocument {
            source: "memory".into(),
            source_id: source_id.into(),
            title: source_id.into(),
            content: content.into(),
            last_modified: 1,
            confirmed: Some(true),
            space,
            ..Default::default()
        }])
        .await
        .unwrap();
    }

    let (_, body) = json_request(
        app,
        Method::POST,
        serde_json::json!({"space": "work", "topic": "release gate"}),
        None,
    )
    .await;
    let response: BriefReadResponse = serde_json::from_value(body).unwrap();

    assert_eq!(response.brief.unwrap().active[0].text, "ship release");
    let related = response.related_context.unwrap();
    assert_eq!(related.query, "release gate");
    assert!(related
        .results
        .iter()
        .any(|result| result.content.contains("work-only marker")));
    assert!(!related
        .results
        .iter()
        .any(|result| result.content.contains("personal-only marker")));
}

#[tokio::test]
async fn explicit_then_header_then_default_choose_the_brief_space() {
    let (app, _temp, db) = fixture().await;
    for (index, space) in ["default", "header", "explicit"].iter().enumerate() {
        db.apply_brief_update(
            &serde_json::from_value(update(
                space,
                &format!("create-{index}"),
                &format!("{space} item"),
            ))
            .unwrap(),
        )
        .await
        .unwrap();
    }
    let default = db.get_space("default").await.unwrap().unwrap();
    db.set_default_space(&default.id).await.unwrap();

    for (body, header, expected) in [
        (serde_json::json!({}), None, "default"),
        (serde_json::json!({}), Some("header"), "header"),
        (
            serde_json::json!({"space": "explicit"}),
            Some("header"),
            "explicit",
        ),
    ] {
        let (_, body) = json_request(app.clone(), Method::POST, body, header).await;
        let response: BriefReadResponse = serde_json::from_value(body).unwrap();
        assert_eq!(response.space.as_deref(), Some(expected));
    }
}

#[tokio::test]
async fn patch_commits_safe_changes_reports_stale_conflicts_and_projects_receipt() {
    let (app, temp, _db) = fixture().await;
    let (_, created) = json_request(
        app.clone(),
        Method::PATCH,
        update("work", "create", "ship release"),
        None,
    )
    .await;
    let created: BriefUpdateReceipt = serde_json::from_value(created).unwrap();
    let item_id = created.applied[0].item_id.clone().unwrap();
    assert_eq!(
        created.projection_path.as_deref(),
        Some(temp.path().join("status").join("work.md").to_str().unwrap())
    );
    assert!(temp.path().join("status").join("work.md").is_file());

    let (_, stale) = json_request(
        app,
        Method::PATCH,
        serde_json::json!({
            "space": "work",
            "caller_id": "brief-route-test",
            "operation_id": "stale-and-safe",
            "mutations": [
                {
                    "kind": "edit",
                    "item_id": item_id,
                    "expected_version": 9,
                    "text": "must not overwrite"
                },
                {
                    "kind": "add",
                    "text": "safe parallel item",
                    "state": BriefItemState::Backlog
                }
            ]
        }),
        None,
    )
    .await;
    let stale: BriefUpdateReceipt = serde_json::from_value(stale).unwrap();

    assert_eq!(stale.conflicts.len(), 1);
    assert_eq!(stale.applied.len(), 1);
    assert!(temp.path().join("status").join("work.md").is_file());
}

async fn get_request(
    app: wenlan_server::router::AppRouter,
    uri: &str,
    space_header: Option<&str>,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder().method(Method::GET).uri(uri);
    if let Some(space) = space_header {
        builder = builder.header("x-wenlan-space", space);
    }
    let response = app
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

#[tokio::test]
async fn history_lists_updates_newest_first_and_show_returns_the_old_brief() {
    let (app, _temp, _db) = fixture().await;
    for (op, text) in [("op-1", "first item"), ("op-2", "second item")] {
        let (status, _) =
            json_request(app.clone(), Method::PATCH, update("work", op, text), None).await;
        assert_eq!(status, StatusCode::OK);
    }

    let (status, body) = get_request(app.clone(), "/api/brief/history?space=work", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let response: wenlan_types::BriefHistoryResponse = serde_json::from_value(body).unwrap();
    assert_eq!(response.space.as_deref(), Some("work"));
    let versions: Vec<i64> = response.entries.iter().map(|e| e.version).collect();
    assert_eq!(versions, vec![2, 1]);
    assert_eq!(response.entries[0].caller_id, "brief-route-test");
    assert_eq!(response.entries[0].operation_id, "op-2");
    assert_eq!(response.entries[0].mutation_count, 1);

    let (_, body) = get_request(app.clone(), "/api/brief/history?space=work&limit=1", None).await;
    let limited: wenlan_types::BriefHistoryResponse = serde_json::from_value(body).unwrap();
    assert_eq!(limited.entries.len(), 1);
    assert_eq!(limited.entries[0].version, 2);

    // the header selects the Space just like the Brief read route
    let (_, body) = get_request(app.clone(), "/api/brief/history", Some("work")).await;
    let by_header: wenlan_types::BriefHistoryResponse = serde_json::from_value(body).unwrap();
    assert_eq!(by_header.entries.len(), 2);

    let (status, body) = get_request(app.clone(), "/api/brief/history/1?space=work", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let old: wenlan_types::BriefVersionResponse = serde_json::from_value(body).unwrap();
    assert_eq!(old.entry.version, 1);
    assert_eq!(old.brief.version, 1);
    assert_eq!(old.brief.active.len(), 1);
    assert_eq!(old.brief.active[0].text, "first item");

    let (status, _) = get_request(app, "/api/brief/history/9?space=work", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn history_is_scoped_to_the_resolved_space() {
    let (app, _temp, db) = fixture().await;
    json_request(
        app.clone(),
        Method::PATCH,
        update("work", "work-op", "work only item"),
        None,
    )
    .await;
    db.create_space("personal", None, false).await.unwrap();

    // another Space sees nothing and cannot read the version
    let (status, body) = get_request(app.clone(), "/api/brief/history", Some("personal")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["entries"].as_array().unwrap().len(), 0);
    let (status, body) = get_request(app.clone(), "/api/brief/history/1", Some("personal")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert!(!body.to_string().contains("work only item"));

    // explicit query Space wins over the header
    let (_, body) = get_request(
        app.clone(),
        "/api/brief/history?space=work",
        Some("personal"),
    )
    .await;
    assert_eq!(body["entries"].as_array().unwrap().len(), 1);

    // an unknown Space is rejected, not silently widened
    let (status, _) = get_request(app.clone(), "/api/brief/history", Some("missing")).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = get_request(app, "/api/brief/history/1", Some("missing")).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}
