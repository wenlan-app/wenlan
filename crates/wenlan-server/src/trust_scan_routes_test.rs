// SPDX-License-Identifier: Apache-2.0
//! Route contracts for `/api/trust/scan` and
//! `/api/pages/{id}/citations/requeue`.

use crate::state::ServerState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use std::sync::Arc;
use tokio::sync::RwLock;
use tower::ServiceExt;
use wenlan_core::db::MemoryDB;
use wenlan_types::trust_scan::{CitationRequeueResponse, TrustScanResponse};

async fn state_with_db() -> (Arc<RwLock<ServerState>>, Arc<MemoryDB>, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let db = Arc::new(
        MemoryDB::new(tmp.path(), Arc::new(wenlan_core::events::NoopEmitter))
            .await
            .unwrap(),
    );
    let state = ServerState {
        db: Some(db.clone()),
        ..Default::default()
    };
    (Arc::new(RwLock::new(state)), db, tmp)
}

async fn call(
    state: &Arc<RwLock<ServerState>>,
    method: &str,
    uri: &str,
) -> (StatusCode, serde_json::Value) {
    let app = crate::router::build_router(state.clone());
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
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
async fn trust_scan_route_is_empty_before_the_first_run_then_returns_the_stored_report() {
    let (state, db, _tmp) = state_with_db().await;
    let (status, json) = call(&state, "GET", "/api/trust/scan").await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json, serde_json::json!({ "report": null }));

    let stored = wenlan_core::trust_scan::run_and_store(&db, None, 1_800_000_000)
        .await
        .unwrap();
    let (status, json) = call(&state, "GET", "/api/trust/scan").await;
    assert_eq!(status, StatusCode::OK, "{json}");
    let body: TrustScanResponse = serde_json::from_value(json).unwrap();
    let report = body.report.expect("the stored report");
    assert_eq!(report.ran_at, 1_800_000_000);
    assert_eq!(report.counts.pages_scanned, stored.counts.pages_scanned);
    assert!(report.outside_edit_false_positives.is_none());
}

#[tokio::test]
async fn ux_who_5_requeue_route_says_when_a_page_had_not_given_up() {
    let (state, _db, _tmp) = state_with_db().await;
    let (status, json) = call(&state, "POST", "/api/pages/page_none/citations/requeue").await;
    assert_eq!(status, StatusCode::OK, "{json}");
    let body: CitationRequeueResponse = serde_json::from_value(json).unwrap();
    assert_eq!(body.page_id, "page_none");
    assert!(!body.requeued);
}
