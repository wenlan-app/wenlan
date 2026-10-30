// SPDX-License-Identifier: Apache-2.0
//! Real in-process HTTP restore contracts; all DB and projection paths are scratch.
mod common;

use axum::body::{to_bytes, Body};
use axum::http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;
use wenlan_core::db::MemoryDB;
use wenlan_types::responses::{PageProjectionStatus, RestorePageResponse};

const BODY: &str = "\n\n    indented code\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n\n```text\na | b\n```\nHard break  \n\n";

async fn send(
    app: &common::AppRouter,
    method: Method,
    path: &str,
    body: Option<Value>,
    space: Option<&str>,
    explicit: bool,
) -> (StatusCode, Value) {
    let mut request = Request::builder().method(method).uri(path);
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    if let Some(space) = space {
        request = request.header("x-wenlan-space", space);
    }
    if explicit {
        request = request
            .header("x-wenlan-truth-contract", "1")
            .header("x-wenlan-reader-intent", "explicit");
    }
    let response = app
        .clone()
        .oneshot(
            request
                .body(
                    body.map(|value| Body::from(value.to_string()))
                        .unwrap_or_else(Body::empty),
                )
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1_000_000).await.unwrap();
    let value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    (status, value)
}

async fn archived(db: &Arc<MemoryDB>, title: &str, space: Option<&str>) -> (String, i64) {
    let draft = db
        .create_page_draft(title, BODY, space, space)
        .await
        .unwrap();
    let published = db
        .publish_page_draft(&draft.id, draft.version)
        .await
        .unwrap();
    assert!(matches!(
        published,
        wenlan_core::pages::PageDraftPublishOutcome::Published(_)
    ));
    db.archive_page(&draft.id).await.unwrap();
    let page = db.get_page(&draft.id).await.unwrap().unwrap();
    assert_eq!(
        page.content, BODY,
        "fixture must retain the exact authored bytes"
    );
    (page.id, page.version)
}

// Same authoritative stale-verdict fixture seam as page_draft_routes.rs.
// extractor_version=1 mirrors the current core extractor; if it changes,
// the automatic 404 control must fail instead of silently testing no gate.
async fn unsupported(tmp: &tempfile::TempDir, db: &Arc<MemoryDB>, id: &str) {
    let page = db.get_page(id).await.unwrap().unwrap();
    let raw = libsql::Builder::new_local(tmp.path().join("origin_memory.db").to_str().unwrap())
        .build()
        .await
        .unwrap();
    let conn = raw.connect().unwrap();
    conn.execute(
        "INSERT INTO page_truth_state
         (page_id,page_version,support_status,provisional_reason,evaluated_at,human_reviewed,updated_at)
         VALUES (?1,?2,'provisional','refuted by fixture',1,0,1)
         ON CONFLICT(page_id) DO UPDATE SET page_version=?2,support_status='provisional',
             provisional_reason='refuted by fixture',evaluated_at=1,human_reviewed=0,updated_at=1",
        libsql::params![id, page.version],
    ).await.unwrap();
    conn.execute(
        "INSERT OR REPLACE INTO claim_derivation_markers
         (page_id,page_version,page_version_digest,extractor_version,inventory_count,created_at)
         VALUES (?1,?2,?3,1,1,0)",
        libsql::params![
            id,
            page.version,
            wenlan_core::provenance::revision_content_digest(&page.content)
        ],
    )
    .await
    .unwrap();
    db.set_app_metadata("claim_promoter_enforcement", "1")
        .await
        .unwrap();
    db.set_truth_cutover_generation(1).await.unwrap();
}

#[tokio::test]
async fn restore_route_projects_exact_body_and_replays_without_duplicate_versions() {
    let (app, tmp, db) = common::test_app().await;
    let (id, version) = archived(&db, "Archived original", None).await;
    let path = format!("/api/pages/{id}/restore");
    let (status, value) = send(
        &app,
        Method::POST,
        &path,
        Some(json!({"expected_version":version})),
        None,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    let result: RestorePageResponse = serde_json::from_value(value).unwrap();
    assert_eq!(result.id, id);
    assert_eq!(result.status, "active");
    assert_eq!(result.version, version + 1);
    assert_eq!(result.projection_status, PageProjectionStatus::Synced);
    assert!(tmp.path().join("pages/.wenlan/state.json").is_file());
    for _ in 0..2 {
        let (status, read) = send(
            &app,
            Method::GET,
            &format!("/api/pages/{id}"),
            None,
            None,
            true,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{read}");
        assert_eq!(read["page"]["content"], BODY);
        assert_eq!(read["page"]["version"], result.version);
    }
    let history = db.list_page_history(&id, 20).await.unwrap();
    let (status, _) = send(
        &app,
        Method::POST,
        &path,
        Some(json!({"expected_version":version})),
        None,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, replay) = send(
        &app,
        Method::POST,
        &path,
        Some(json!({"expected_version":result.version})),
        None,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    assert_eq!(replay["version"], result.version);
    assert_eq!(db.list_page_history(&id, 20).await.unwrap(), history);
    assert_eq!(db.get_page(&id).await.unwrap().unwrap().content, BODY);
}

#[tokio::test]
async fn restore_route_checks_scope_and_requires_typed_version() {
    let (app, _tmp, db) = common::test_app().await;
    db.create_space("work", None, false).await.unwrap();
    db.create_space("private", None, false).await.unwrap();
    let (id, version) = archived(&db, "Private title", Some("private")).await;
    let path = format!("/api/pages/{id}/restore");
    for (body, space, expected) in [
        (json!({}), None, StatusCode::UNPROCESSABLE_ENTITY),
        (
            json!({"expected_version":"wrong"}),
            None,
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            json!({"expected_version":version}),
            Some("unknown-space"),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            json!({"expected_version":version}),
            Some("work"),
            StatusCode::NOT_FOUND,
        ),
    ] {
        let (status, value) = send(&app, Method::POST, &path, Some(body), space, true).await;
        assert_eq!(status, expected, "{value}");
        assert!(!value.to_string().contains("Private title"));
        let page = db.get_page(&id).await.unwrap().unwrap();
        assert_eq!(page.status, "archived");
        assert_eq!(page.version, version);
    }
    let (status, result) = send(
        &app,
        Method::POST,
        &path,
        Some(json!({"expected_version":version})),
        Some("private"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
}

#[tokio::test]
async fn restore_route_title_conflict_does_not_disclose_other_identity_and_accepts_alternate_title()
{
    let (app, _tmp, db) = common::test_app().await;
    let (id, version) = archived(&db, "ÄBC", None).await;
    let other = common::create_page_fixture(
        &db,
        "äbc",
        "Private conflicting body",
        None,
        &[],
        "authored",
    )
    .await;
    let path = format!("/api/pages/{id}/restore");
    let (status, error) = send(
        &app,
        Method::POST,
        &path,
        Some(json!({"expected_version":version})),
        None,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{error}");
    assert!(!error.to_string().contains(&other));
    assert!(!error.to_string().contains("äbc"));
    assert!(!error.to_string().contains("Private conflicting body"));
    let (status, value) = send(
        &app,
        Method::POST,
        &path,
        Some(json!({"expected_version":version,"title":"  Recovered title  "})),
        None,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    let response: RestorePageResponse = serde_json::from_value(value).unwrap();
    assert_eq!(response.title, "Recovered title");
    assert_eq!(response.id, id);
    assert_eq!(db.get_page(&id).await.unwrap().unwrap().content, BODY);
    assert_eq!(db.get_page(&other).await.unwrap().unwrap().title, "äbc");
}

#[tokio::test]
async fn restore_named_intent_can_restore_old_hidden_metadata_without_copying_its_verdict() {
    let (app, tmp, db) = common::test_app().await;
    let (id, version) = archived(&db, "Hidden archive", None).await;
    unsupported(&tmp, &db, &id).await;
    let path = format!("/api/pages/{id}/restore");
    let (status, denied) = send(
        &app,
        Method::POST,
        &path,
        Some(json!({"expected_version":version})),
        None,
        false,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "the unsupported fixture must actually hide: {denied}"
    );
    assert!(!denied.to_string().contains("Hidden archive"));
    assert_eq!(db.get_page(&id).await.unwrap().unwrap().status, "archived");
    let (status, allowed) = send(
        &app,
        Method::POST,
        &path,
        Some(json!({"expected_version":version})),
        None,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{allowed}");
    let restored: RestorePageResponse = serde_json::from_value(allowed).unwrap();
    assert_eq!(restored.version, version + 1);
    // A new version makes the OLD verdict stale/Unevaluated under the existing
    // truth contract. It is not a manufactured approval or a copied judgment.
    assert_eq!(restored.projection_status, PageProjectionStatus::Synced);
    let truth = db.page_truth_states(&[id.clone()]).await.unwrap();
    assert_eq!(
        truth[&id].support,
        wenlan_core::truth_contract::Support::Unevaluated
    );
    assert!(!truth[&id].human_reviewed);
}

#[tokio::test]
async fn restore_current_version_hidden_replay_does_not_project_or_duplicate_history() {
    let (app, tmp, db) = common::test_app().await;
    let id = common::create_page_fixture(&db, "Hidden current", BODY, None, &[], "authored").await;
    let version = db.get_page(&id).await.unwrap().unwrap().version;
    unsupported(&tmp, &db, &id).await;
    let path = format!("/api/pages/{id}/restore");
    assert!(
        !tmp.path().join("pages").exists(),
        "DB-only fixture has no projection"
    );
    let history = db.list_page_history(&id, 20).await.unwrap();
    let (status, _) = send(
        &app,
        Method::POST,
        &path,
        Some(json!({"expected_version":version})),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, result) = send(
        &app,
        Method::POST,
        &path,
        Some(json!({"expected_version":version})),
        None,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    let response: RestorePageResponse = serde_json::from_value(result).unwrap();
    assert_eq!(response.projection_status, PageProjectionStatus::Hidden);
    assert_eq!(response.version, version);
    assert!(
        !tmp.path().join("pages").exists(),
        "explicit read intent must not grant projection"
    );
    assert_eq!(db.list_page_history(&id, 20).await.unwrap(), history);
}

#[tokio::test]
async fn restore_projection_failure_reports_pending_after_durable_success_and_retry_is_safe() {
    let (app, tmp, db) = common::test_app().await;
    let (id, version) = archived(&db, "Pending file", None).await;
    let root = tmp.path().join("pages");
    assert!(!root.exists());
    // A file at the configured root deterministically rejects directory writes
    // on all platforms, without permission tricks or touching a real vault.
    std::fs::write(&root, "scratch obstruction").unwrap();
    let path = format!("/api/pages/{id}/restore");
    let (status, value) = send(
        &app,
        Method::POST,
        &path,
        Some(json!({"expected_version":version})),
        None,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    let response: RestorePageResponse = serde_json::from_value(value).unwrap();
    assert_eq!(response.projection_status, PageProjectionStatus::Pending);
    assert!(response.projection_error.is_some());
    let saved = db.get_page(&id).await.unwrap().unwrap();
    assert_eq!(saved.status, "active");
    assert_eq!(saved.content, BODY);
    let history = db.list_page_history(&id, 20).await.unwrap();
    std::fs::remove_file(&root).unwrap();
    let (status, value) = send(
        &app,
        Method::POST,
        &path,
        Some(json!({"expected_version":response.version})),
        None,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    let retried: RestorePageResponse = serde_json::from_value(value).unwrap();
    assert_eq!(retried.projection_status, PageProjectionStatus::Synced);
    assert_eq!(retried.version, response.version);
    assert_eq!(db.list_page_history(&id, 20).await.unwrap(), history);
}

#[tokio::test]
async fn restore_preserves_external_whitespace_edits_in_the_existing_file() {
    let (app, tmp, db) = common::test_app().await;
    let draft = db
        .create_page_draft("External whitespace", BODY, None, None)
        .await
        .unwrap();
    let wenlan_core::pages::PageDraftPublishOutcome::Published(page) = db
        .publish_page_draft(&draft.id, draft.version)
        .await
        .unwrap()
    else {
        panic!("fixture must publish")
    };
    let projection = wenlan_core::export::knowledge::KnowledgeProjectionWrite::new(
        tmp.path().join("pages"),
        &db,
    );
    let filename = projection
        .write_page_gated(&db, &page)
        .await
        .unwrap()
        .unwrap();
    db.archive_page(&page.id).await.unwrap();
    let archived = db.get_page(&page.id).await.unwrap().unwrap();
    let original = std::fs::read_to_string(&filename).unwrap();
    assert!(original.contains(BODY));
    let edited = original.replacen(BODY, &format!("\n{BODY}\n  "), 1);
    std::fs::write(&filename, &edited).unwrap();
    let (status, response) = send(
        &app,
        Method::POST,
        &format!("/api/pages/{}/restore", page.id),
        Some(json!({"expected_version":archived.version})),
        None,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(
        response["projection_status"], "pending",
        "unsynced external authored whitespace must not be overwritten"
    );
    assert_eq!(std::fs::read_to_string(&filename).unwrap(), edited);
    let restored = db.get_page(&page.id).await.unwrap().unwrap();
    assert_eq!(restored.status, "active");
    assert_eq!(
        restored.content, BODY,
        "the DB snapshot is restored without guessing the external edit"
    );
}
