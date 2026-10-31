// SPDX-License-Identifier: Apache-2.0
//! `GET /api/pages/{id}/revisions`: `?include=text` and the merged history
//! (UX-HIST-1/2).

use crate::state::ServerState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use std::sync::Arc;
use tokio::sync::RwLock;
use tower::ServiceExt;
use wenlan_core::db::MemoryDB;
use wenlan_types::responses::{ListPageRevisionsResponse, PageHistoryKind};

async fn page(db: &MemoryDB, title: &str, content: &str) -> String {
    let request = wenlan_types::requests::CreateConceptRequest {
        title: title.to_string(),
        content: content.to_string(),
        summary: None,
        entity_id: None,
        space: wenlan_types::WriteSpaceTarget::Uncategorized,
        source_memory_ids: Vec::new(),
        creation_kind: Some("authored".to_string()),
        workspace: None,
    };
    let created = wenlan_core::post_write::create_page(db, request, "test", None)
        .await
        .unwrap();
    created.attached_to.unwrap_or(created.id)
}

async fn call(
    state: &Arc<RwLock<ServerState>>,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let app = crate::router::build_router(state.clone());
    let mut request = Request::builder().method(method).uri(uri);
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
async fn revisions_carry_texts_on_request_and_one_merged_history() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Arc::new(
        MemoryDB::new(tmp.path(), Arc::new(wenlan_core::events::NoopEmitter))
            .await
            .unwrap(),
    );
    let state = Arc::new(RwLock::new(ServerState {
        db: Some(db.clone()),
        ..Default::default()
    }));
    let source = page(&db, "Alpha", "The launch is on 2026-03-01.").await;
    let citer = page(&db, "Beta", "Plans built on Alpha.").await;
    let source_version = db.get_page(&source).await.unwrap().unwrap().version;
    db.mark_page_basis(&citer, &source, source_version, "human")
        .await
        .unwrap();

    // An AI refresh of the page, then a new version of the page it relies on.
    let citer_version = db.get_page(&citer).await.unwrap().unwrap().version;
    let (status, json) = call(
        &state,
        "PUT",
        &format!("/api/pages/{citer}"),
        Some(serde_json::json!({
            "content": "Plans built on Alpha, refreshed.",
            "source_memory_ids": [],
            "expected_version": citer_version,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    db.update_page_content(
        &source,
        "The launch moved to 2026-04-01.",
        &[],
        "agent_refresh",
    )
    .await
    .unwrap();

    // Without include=text: the old fields as before, the history without texts.
    let (status, json) = call(
        &state,
        "GET",
        &format!("/api/pages/{citer}/revisions"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    let plain: ListPageRevisionsResponse = serde_json::from_value(json).unwrap();
    assert!(!plain.entries.is_empty());
    assert!(plain.entries.iter().all(|e| e.text.is_none()));
    assert!(plain.history.iter().all(|h| h.text.is_none()));

    let (status, json) = call(
        &state,
        "GET",
        &format!("/api/pages/{citer}/revisions?include=text"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    let full: ListPageRevisionsResponse = serde_json::from_value(json).unwrap();
    assert!(full
        .entries
        .iter()
        .any(|e| e.text.as_deref() == Some("Plans built on Alpha, refreshed.")));
    let ai = full
        .history
        .iter()
        .find(|h| h.kind == PageHistoryKind::AiEdit)
        .expect("the AI refresh is in the history");
    assert_eq!(ai.text.as_deref(), Some("Plans built on Alpha, refreshed."));
    let update = full
        .history
        .iter()
        .find(|h| h.kind == PageHistoryKind::SourceUpdate)
        .expect("the source's new version is in the same list");
    let s = update.source.as_ref().unwrap();
    assert_eq!(s.page_id, source);
    assert_eq!(s.version, source_version + 1);
    assert!(s.pending);
    assert_eq!(s.text.as_deref(), Some("The launch moved to 2026-04-01."));
}
