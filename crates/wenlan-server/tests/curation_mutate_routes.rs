// SPDX-License-Identifier: Apache-2.0
//! HTTP integration tests for Spec C-2 curation mutate routes.
//! Boots the real Axum router with a temp-dir MemoryDB and asserts:
//!   - typed responses match the wire-type structs
//!   - missing ids return HTTP 404 (Shape A/B/C)
//!   - re-call returns HTTP 404 (row consumed after first approve/dismiss)
//!   - x-agent-name header threads into agent_activity rows

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;
use wenlan_types::{ContradictionDismissResponse, RevisionAcceptResponse, RevisionDismissResponse};

async fn body_as_json<T: serde::de::DeserializeOwned>(response: axum::http::Response<Body>) -> T {
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).expect("response body is valid JSON of expected type")
}

// ── accept_pending_revision ──────────────────────────────────────────────────

async fn seed_pending_revision_in_test_app(
    db: &std::sync::Arc<wenlan_core::db::MemoryDB>,
    target: &str,
    revision: &str,
) {
    let now = chrono::Utc::now().timestamp();
    use wenlan_types::RawDocument;
    // Seed the original (target) memory as confirmed
    db.upsert_documents(vec![RawDocument {
        source_id: target.to_string(),
        title: format!("title-{target}"),
        content: "original content".to_string(),
        source: "memory".to_string(),
        source_agent: Some("claude-code".to_string()),
        last_modified: now,
        ..Default::default()
    }])
    .await
    .unwrap();
    // Seed the revision row pointing at the target with pending_revision=true
    db.upsert_documents(vec![RawDocument {
        source_id: revision.to_string(),
        title: format!("title-{revision}"),
        content: "revised content".to_string(),
        source: "memory".to_string(),
        source_agent: Some("claude-code".to_string()),
        last_modified: now,
        supersedes: Some(target.to_string()),
        pending_revision: true,
        ..Default::default()
    }])
    .await
    .unwrap();
}

#[tokio::test]
async fn accept_revision_succeeds_and_returns_typed_response() {
    let (router, _tmp, db) = common::test_app().await;
    seed_pending_revision_in_test_app(&db, "mem_ar1_target", "mem_ar1_rev").await;

    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/memory/revision/mem_ar1_target/accept")
                .header("x-agent-name", "test-agent")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let parsed: RevisionAcceptResponse = body_as_json(response).await;
    assert_eq!(parsed.target_source_id, "mem_ar1_target");
    assert_eq!(parsed.revision_source_id, "mem_ar1_rev");
    assert!(parsed.wrote);
}

#[tokio::test]
async fn accept_revision_accepts_revision_source_id() {
    let (router, _tmp, db) = common::test_app().await;
    seed_pending_revision_in_test_app(&db, "mem_ar_rev_id_target", "mem_ar_rev_id_rev").await;

    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/memory/revision/mem_ar_rev_id_rev/accept")
                .header("x-agent-name", "test-agent")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let parsed: RevisionAcceptResponse = body_as_json(response).await;
    assert_eq!(parsed.target_source_id, "mem_ar_rev_id_target");
    assert_eq!(parsed.revision_source_id, "mem_ar_rev_id_rev");
    assert!(parsed.wrote);
}

#[tokio::test]
async fn accept_revision_returns_404_on_missing_target() {
    let (router, _tmp, _db) = common::test_app().await;
    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/memory/revision/mem_missing/accept")
                .header("x-agent-name", "test-agent")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn accept_revision_returns_404_on_re_call() {
    let (router, _tmp, db) = common::test_app().await;
    seed_pending_revision_in_test_app(&db, "mem_ar2_target", "mem_ar2_rev").await;

    let first = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/memory/revision/mem_ar2_target/accept")
                .header("x-agent-name", "test-agent")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);

    let second = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/memory/revision/mem_ar2_target/accept")
                .header("x-agent-name", "test-agent")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn accept_revision_threads_x_agent_name() {
    let (router, _tmp, db) = common::test_app().await;
    seed_pending_revision_in_test_app(&db, "mem_ar3_target", "mem_ar3_rev").await;
    router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/memory/revision/mem_ar3_target/accept")
                .header("x-agent-name", "openai-test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let count =
        common::count_activity_for_action_and_agent(&db, "revision_accept", "openai-test").await;
    assert_eq!(count, 1);
}

// ── dismiss_pending_revision HTTP tests ─────────────────────────────────────

#[tokio::test]
async fn dismiss_revision_succeeds_and_returns_typed_response() {
    let (router, _tmp, db) = common::test_app().await;
    seed_pending_revision_in_test_app(&db, "mem_dr1_target", "mem_dr1_rev").await;
    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/memory/revision/mem_dr1_rev/dismiss")
                .header("x-agent-name", "test-agent")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let parsed: RevisionDismissResponse = body_as_json(response).await;
    assert_eq!(parsed.target_source_id, "mem_dr1_target");
    assert!(parsed.dismissed);
    assert!(!parsed.wrote, "rejecting a card writes nothing");
    let revision = db
        .get_memory_detail("mem_dr1_rev")
        .await
        .unwrap()
        .expect("dismissed revision must survive as an independent memory");
    assert!(
        revision.supersedes.is_none(),
        "dismiss must clear the false supersedes link"
    );
    assert!(
        db.get_memory_detail("mem_dr1_target")
            .await
            .unwrap()
            .is_some(),
        "the original target must remain after dismiss"
    );
}

#[tokio::test]
async fn dismiss_revision_returns_404_on_missing() {
    let (router, _tmp, _db) = common::test_app().await;
    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/memory/revision/mem_missing/dismiss")
                .header("x-agent-name", "test-agent")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn dismiss_revision_returns_404_on_re_call() {
    let (router, _tmp, db) = common::test_app().await;
    seed_pending_revision_in_test_app(&db, "mem_dr2_target", "mem_dr2_rev").await;
    let first = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/memory/revision/mem_dr2_target/dismiss")
                .header("x-agent-name", "test-agent")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let second = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/memory/revision/mem_dr2_target/dismiss")
                .header("x-agent-name", "test-agent")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn dismiss_revision_threads_x_agent_name() {
    let (router, _tmp, db) = common::test_app().await;
    seed_pending_revision_in_test_app(&db, "mem_dr3_target", "mem_dr3_rev").await;
    router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/memory/revision/mem_dr3_target/dismiss")
                .header("x-agent-name", "zed-test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let count =
        common::count_activity_for_action_and_agent(&db, "revision_dismiss", "zed-test").await;
    assert_eq!(count, 1);
}

// ── dismiss_contradiction ─────────────────────────────────────────────────────

#[tokio::test]
async fn dismiss_contradiction_returns_200_with_typed_response_even_for_unknown_id() {
    let (router, _tmp, _db) = common::test_app().await;
    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/memory/contradiction/mem_unknown/dismiss")
                .header("x-agent-name", "test-agent")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let parsed: ContradictionDismissResponse = body_as_json(response).await;
    assert_eq!(parsed.source_id, "mem_unknown");
    assert!(parsed.wrote);
}

#[tokio::test]
async fn dismiss_contradiction_threads_x_agent_name() {
    let (router, _tmp, db) = common::test_app().await;
    router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/memory/contradiction/mem_attr/dismiss")
                .header("x-agent-name", "mcp-test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let count =
        common::count_activity_for_action_and_agent(&db, "contradiction_dismiss", "mcp-test").await;
    assert_eq!(count, 1);
}

#[tokio::test]
async fn dismiss_contradiction_idempotent_re_call_remains_wrote_true() {
    let (router, _tmp, _db) = common::test_app().await;
    for _ in 0..3 {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/memory/contradiction/mem_idem/dismiss")
                    .header("x-agent-name", "test-agent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let parsed: ContradictionDismissResponse = body_as_json(response).await;
        assert!(parsed.wrote);
    }
}

#[tokio::test]
async fn dismiss_contradiction_flips_awaiting_review_row_to_dismissed() {
    let (router, _tmp, db) = common::test_app().await;
    // Seed a detect_contradiction proposal (status=pending), then promote to awaiting_review.
    db.insert_refinement_proposal(
        "ref_contra_1",
        "detect_contradiction",
        &["mem_target".to_string()],
        None,
        0.9,
    )
    .await
    .unwrap();
    db.resolve_refinement_if_open("ref_contra_1", "awaiting_review")
        .await
        .unwrap();

    router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/memory/contradiction/mem_target/dismiss")
                .header("x-agent-name", "test-agent")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let proposal = db
        .get_refinement_proposal("ref_contra_1")
        .await
        .unwrap()
        .expect("ref_contra_1 must exist after dismiss");
    assert_eq!(proposal.status, "dismissed");
}
