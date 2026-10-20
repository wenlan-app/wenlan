// SPDX-License-Identifier: Apache-2.0
//! HTTP integration tests for POST /api/memory/{id}/update-page (manual edit).
//!
//! The route used to call the DB directly, bypassing the page-write gate: no
//! changelog, no version CAS, no history row, and `ok: true` even when the
//! page had moved underneath the edit. These pin the routed behaviour.

mod common;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use tower::ServiceExt;
use wenlan_core::export::knowledge::{KnowledgeProjectionWrite, KnowledgeWriter};
use wenlan_core::export::provenance::{
    canonicalize_page_body, SOURCES_BLOCK_END, SOURCES_BLOCK_START,
};
use wenlan_core::sources::obsidian::extract_frontmatter;

fn post_page(id: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(format!("/api/memory/{id}/update-page"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn get_page(id: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(format!("/api/pages/{id}"))
        .body(Body::empty())
        .unwrap()
}

async fn response_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn projected_page_path(
    root: &std::path::Path,
    db: &wenlan_core::db::MemoryDB,
    page_id: &str,
) -> std::path::PathBuf {
    let filename = KnowledgeWriter::new(root.to_path_buf(), db)
        .page_filename(page_id)
        .expect("a synced page must have a recorded Markdown filename");
    root.join(filename)
}

fn assert_projection_matches_page(
    root: &std::path::Path,
    db: &wenlan_core::db::MemoryDB,
    page: &wenlan_types::Page,
) -> std::path::PathBuf {
    let path = projected_page_path(root, db, &page.id);
    let markdown = std::fs::read_to_string(&path).unwrap();
    let (frontmatter, body) = extract_frontmatter(&markdown);
    assert_eq!(
        frontmatter.fields.get("origin_id").and_then(|v| v.as_str()),
        Some(page.id.as_str())
    );
    assert_eq!(
        frontmatter
            .fields
            .get("origin_version")
            .and_then(|v| v.as_i64()),
        Some(page.version),
        "Markdown must identify the committed DB version"
    );
    assert_eq!(canonicalize_page_body(body), page.content);
    path
}

/// A manual edit must leave a changelog entry and a history row — the trail a
/// user needs to see what a page said before. The direct DB call wrote neither.
#[tokio::test]
async fn manual_edit_goes_through_the_write_gate() {
    let (app, _tmp, db) = common::test_app().await;
    let page_id = common::create_page_fixture(
        &db,
        "Ferns",
        "Ferns reproduce by spores.",
        None,
        &[],
        "authored",
    )
    .await;

    let before = db.get_page(&page_id).await.unwrap().unwrap();
    assert!(
        !before
            .changelog
            .as_deref()
            .unwrap_or("")
            .contains("manual_edit"),
        "precondition: a freshly created page has no manual edit recorded"
    );

    let response = app
        .oneshot(post_page(
            &page_id,
            serde_json::json!({ "content": "Ferns reproduce by spores, not seeds." }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let after = db.get_page(&page_id).await.unwrap().unwrap();
    assert_eq!(after.content, "Ferns reproduce by spores, not seeds.");
    assert_eq!(after.version, before.version + 1);
    assert!(
        after.user_edited,
        "a manual edit takes human ownership of the page"
    );
    let changelog = after.changelog.as_deref().unwrap_or("");
    assert!(
        changelog.contains("manual_edit"),
        "the gate records the edit in the changelog; got {changelog}"
    );

    let history = db.list_page_history(&page_id, 10).await.unwrap();
    let versions: Vec<i64> = history.iter().map(|h| h.version).collect();
    assert!(
        versions.contains(&after.version),
        "the gate records a history row for the new version; got {versions:?}"
    );
}

/// A stale `expected_version` is a precondition failure: the page moved while
/// the editor was open, so the edit is refused rather than silently winning.
/// The old route had no version guard at all and would overwrite.
#[tokio::test]
async fn manual_edit_with_stale_expected_version_is_refused() {
    let (app, _tmp, db) = common::test_app().await;
    let page_id = common::create_page_fixture(
        &db,
        "Lichen",
        "Lichen is a fungus and an alga together.",
        None,
        &[],
        "authored",
    )
    .await;
    let original = db.get_page(&page_id).await.unwrap().unwrap();

    // Somebody else's edit lands first, moving the page past `original.version`.
    db.update_page_content(
        &page_id,
        "Lichen is a fungus and an alga in partnership.",
        &[],
        "manual_edit",
    )
    .await
    .unwrap();
    let landed = db.get_page(&page_id).await.unwrap().unwrap();
    assert_eq!(landed.version, original.version + 1, "precondition");

    let response = app
        .oneshot(post_page(
            &page_id,
            serde_json::json!({
                "content": "Lichen is a plant.",
                "expected_version": original.version,
            }),
        ))
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::CONFLICT,
        "a refused write must not report success"
    );
    let unchanged = db.get_page(&page_id).await.unwrap().unwrap();
    assert_eq!(
        unchanged.content, landed.content,
        "the refused edit must not have clobbered the write that got there first"
    );
}

/// Saving a page without having changed anything is a no-op, not a conflict.
///
/// The route sends the page's own sources back, so an unedited save reaches the
/// "content is already what you asked for" path and returns `wrote: false`.
/// Branching on that alone told the user "page changed while this edit was
/// open; reload and reapply" for a routine save — a false conflict reporting an
/// edit that never happened, with instructions that would discard their work.
#[tokio::test]
async fn manual_edit_with_no_changes_is_not_a_conflict() {
    let (app, _tmp, db) = common::test_app().await;
    let body = "Mosses have no vascular tissue.";
    let page_id = common::create_page_fixture(&db, "Mosses", body, None, &[], "authored").await;
    let before = db.get_page(&page_id).await.unwrap().unwrap();

    // Byte-identical to what the page already holds — the save button on an
    // untouched editor.
    let response = app
        .oneshot(post_page(&page_id, serde_json::json!({ "content": body })))
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "an unchanged save must succeed; a conflict here tells the user someone \
         edited their page when nobody did"
    );
    let after = db.get_page(&page_id).await.unwrap().unwrap();
    assert_eq!(
        after.version, before.version,
        "a no-op save must not burn a version"
    );
}

/// Editing a page that does not exist used to return `ok: true` — the DB
/// update matched zero rows and the route reported success anyway.
#[tokio::test]
async fn manual_edit_of_missing_page_is_not_found() {
    let (app, _tmp, _db) = common::test_app().await;
    let response = app
        .oneshot(post_page(
            "page-that-does-not-exist",
            serde_json::json!({ "content": "anything" }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// An authored page is born with zero sources, and the gate's "must cite at
/// least one source" rule applied to every writer — so routing manual edits
/// through it would have rejected every edit of a page the user wrote
/// themselves. The rule is machine-writes-only for exactly this reason.
#[tokio::test]
async fn manual_edit_of_zero_source_authored_page_is_allowed() {
    let (app, _tmp, db) = common::test_app().await;
    let page_id = common::create_page_fixture(
        &db,
        "My own notes",
        "Things I am thinking about.",
        None,
        &[],
        "authored",
    )
    .await;
    let before = db.get_page(&page_id).await.unwrap().unwrap();
    assert!(
        before.source_memory_ids.is_empty(),
        "precondition: an authored page can have no sources"
    );

    let response = app
        .oneshot(post_page(
            &page_id,
            serde_json::json!({ "content": "Things I am still thinking about." }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let after = db.get_page(&page_id).await.unwrap().unwrap();
    assert_eq!(after.content, "Things I am still thinking about.");
}

#[tokio::test]
async fn manual_edit_round_trips_exact_source_through_post_then_get() {
    let (app, _tmp, db) = common::test_app().await;
    let page_id =
        common::create_page_fixture(&db, "Exact source", "old body", None, &[], "authored").await;
    let before = db.get_page(&page_id).await.unwrap().unwrap();
    let exact = "\u{feff}\r\n  # Exact source  \r\n\r\nBody with terminal bytes\t \r\n\r\n";
    assert_ne!(
        exact.trim_end(),
        exact,
        "positive control: trimming must change this fixture"
    );

    let response = app
        .clone()
        .oneshot(post_page(
            &page_id,
            serde_json::json!({
                "content": exact,
                "expected_version": before.version,
                "caller_id": "wenlan-app",
                "operation_id": "exact-source-round-trip",
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app.oneshot(get_page(&page_id)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(
        body["page"]["content"].as_str(),
        Some(exact),
        "GET must return the exact source accepted by POST"
    );
}

#[tokio::test]
async fn identical_operation_replays_without_a_second_version_or_history_row() {
    let (app, _tmp, db) = common::test_app().await;
    let page_id =
        common::create_page_fixture(&db, "Retry receipt", "old body", None, &[], "authored").await;
    let before = db.get_page(&page_id).await.unwrap().unwrap();
    let history_before = db.list_page_history(&page_id, 10).await.unwrap();
    let request = serde_json::json!({
        "content": "one exact operation",
        "expected_version": before.version,
        "caller_id": "wenlan-app",
        "operation_id": "receipt-replay",
    });

    let first = app
        .clone()
        .oneshot(post_page(&page_id, request.clone()))
        .await
        .unwrap();
    let second = app.oneshot(post_page(&page_id, request)).await.unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(second.status(), StatusCode::OK);

    let after = db.get_page(&page_id).await.unwrap().unwrap();
    let history_after = db.list_page_history(&page_id, 10).await.unwrap();
    assert_eq!(after.version, before.version + 1);
    assert_eq!(history_after.len(), history_before.len() + 1);
    assert!(
        db.get_operation_receipt("wenlan-app", "receipt-replay")
            .await
            .unwrap()
            .is_some(),
        "the successful mutation and its receipt must commit together"
    );
}

#[tokio::test]
async fn reused_operation_with_one_byte_change_conflicts_without_mutation() {
    let (app, _tmp, db) = common::test_app().await;
    let page_id =
        common::create_page_fixture(&db, "Receipt conflict", "old body", None, &[], "authored")
            .await;
    let before = db.get_page(&page_id).await.unwrap().unwrap();
    let first = serde_json::json!({
        "content": "landed body",
        "expected_version": before.version,
        "caller_id": "wenlan-app",
        "operation_id": "receipt-conflict",
    });
    let changed = serde_json::json!({
        "content": "landed body ",
        "expected_version": before.version,
        "caller_id": "wenlan-app",
        "operation_id": "receipt-conflict",
    });

    let response = app
        .clone()
        .oneshot(post_page(&page_id, first))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let landed = db.get_page(&page_id).await.unwrap().unwrap();
    let history_landed = db.list_page_history(&page_id, 10).await.unwrap();

    let response = app.oneshot(post_page(&page_id, changed)).await.unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let unchanged = db.get_page(&page_id).await.unwrap().unwrap();
    let history_unchanged = db.list_page_history(&page_id, 10).await.unwrap();
    assert_eq!(unchanged.content, landed.content);
    assert_eq!(unchanged.version, landed.version);
    assert_eq!(history_unchanged.len(), history_landed.len());
}

#[tokio::test]
async fn canonical_page_write_rejects_every_reserved_sources_marker_shape() {
    let cases = [
        ("start-only", format!("before {SOURCES_BLOCK_START} after")),
        ("end-only", format!("before {SOURCES_BLOCK_END} after")),
        (
            "paired",
            format!("{SOURCES_BLOCK_START}\nowned\n{SOURCES_BLOCK_END}"),
        ),
        (
            "duplicate",
            format!(
                "{SOURCES_BLOCK_START}\none\n{SOURCES_BLOCK_END}\n\
                 {SOURCES_BLOCK_START}\ntwo\n{SOURCES_BLOCK_END}"
            ),
        ),
        (
            "inside-fence",
            format!("```md\n{SOURCES_BLOCK_START}\n```\nkept prose"),
        ),
    ];

    for (case, content) in cases {
        let (app, _tmp, db) = common::test_app().await;
        let page_id = common::create_page_fixture(
            &db,
            &format!("Reserved marker {case}"),
            "unchanged body",
            None,
            &[],
            "authored",
        )
        .await;
        let before = db.get_page(&page_id).await.unwrap().unwrap();
        let history_before = db.list_page_history(&page_id, 10).await.unwrap();
        let operation_id = format!("reserved-marker-{case}");

        let response = app
            .oneshot(post_page(
                &page_id,
                serde_json::json!({
                    "content": content,
                    "expected_version": before.version,
                    "caller_id": "wenlan-app",
                    "operation_id": operation_id,
                }),
            ))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{case} must fail closed"
        );

        let after = db.get_page(&page_id).await.unwrap().unwrap();
        let history_after = db.list_page_history(&page_id, 10).await.unwrap();
        assert_eq!(after.content, before.content, "{case}");
        assert_eq!(after.version, before.version, "{case}");
        assert_eq!(history_after.len(), history_before.len(), "{case}");
        assert!(
            db.get_operation_receipt("wenlan-app", &operation_id)
                .await
                .unwrap()
                .is_none(),
            "{case} must not record a receipt for a rejected write"
        );
    }
}

#[tokio::test]
async fn manual_edit_projects_committed_page_and_preserves_sources_and_history() {
    let (app, tmp, db) = common::test_app().await;
    common::insert_memory(
        &db,
        "manual-projection-source",
        "Ferns reproduce through spores.",
        "memory",
        None,
        None,
        false,
        1,
    )
    .await;
    let page_id = common::create_page_fixture(
        &db,
        "Projected ferns",
        "Ferns have spores.",
        None,
        &["manual-projection-source"],
        "distilled",
    )
    .await;
    let before = db.get_page(&page_id).await.unwrap().unwrap();
    let history_before = db.list_page_history(&page_id, 10).await.unwrap();
    let root = tmp.path().join("pages");
    assert!(!root.exists(), "DB fixture creation has no file projection");

    let response = app
        .oneshot(post_page(
            &page_id,
            serde_json::json!({
                "content": "Ferns reproduce through spores, rather than seeds.",
                "expected_version": before.version,
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let result = response_json(response).await;
    assert_eq!(result["ok"], true);
    assert_eq!(result["projection_status"], "synced");

    let after = db.get_page(&page_id).await.unwrap().unwrap();
    assert_eq!(after.version, before.version + 1);
    assert_eq!(after.source_memory_ids, before.source_memory_ids);
    assert!(
        !after.source_memory_ids.is_empty(),
        "positive source control"
    );
    let history = db.list_page_history(&page_id, 10).await.unwrap();
    assert_eq!(history.len(), history_before.len() + 1);
    let saved = history
        .iter()
        .find(|entry| entry.version == after.version)
        .expect("manual save must append its committed version to history");
    assert_eq!(saved.content, after.content);
    assert_eq!(saved.source_memory_ids, after.source_memory_ids);
    for prior in history_before {
        assert!(
            history.iter().any(|entry| {
                entry.version == prior.version
                    && entry.content == prior.content
                    && entry.source_memory_ids == prior.source_memory_ids
            }),
            "saving must retain earlier content and provenance history"
        );
    }
    let path = assert_projection_matches_page(&root, &db, &after);
    let markdown = std::fs::read_to_string(path).unwrap();
    assert!(markdown.contains("manual-projection-source"));
    assert!(markdown.contains(SOURCES_BLOCK_START));
    assert!(markdown.contains(SOURCES_BLOCK_END));
}

#[tokio::test]
async fn projection_io_failure_commits_edit_and_receipt_then_replay_repairs_file() {
    let (app, tmp, db) = common::test_app().await;
    let page_id = common::create_page_fixture(
        &db,
        "Projection retry",
        "Before the save.",
        None,
        &[],
        "authored",
    )
    .await;
    let before = db.get_page(&page_id).await.unwrap().unwrap();
    let history_before = db.list_page_history(&page_id, 10).await.unwrap();
    let root = tmp.path().join("pages");
    std::fs::write(&root, "a file obstructs the projection directory").unwrap();
    let request = serde_json::json!({
        "content": "The save survives a disk failure.",
        "expected_version": before.version,
        "caller_id": "wenlan-app",
        "operation_id": "projection-io-retry",
    });

    let response = app
        .clone()
        .oneshot(post_page(&page_id, request.clone()))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let result = response_json(response).await;
    assert_eq!(result["ok"], true);
    assert_eq!(result["projection_status"], "pending");
    assert!(
        result["projection_error"]
            .as_str()
            .is_some_and(|error| !error.is_empty()),
        "a pending projection exposes the reason it could not be written"
    );
    let landed = db.get_page(&page_id).await.unwrap().unwrap();
    let history_landed = db.list_page_history(&page_id, 10).await.unwrap();
    assert_eq!(landed.content, "The save survives a disk failure.");
    assert_eq!(landed.version, before.version + 1);
    assert_eq!(history_landed.len(), history_before.len() + 1);
    assert!(db
        .get_operation_receipt("wenlan-app", "projection-io-retry")
        .await
        .unwrap()
        .is_some());

    std::fs::remove_file(&root).unwrap();
    let response = app.oneshot(post_page(&page_id, request)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let result = response_json(response).await;
    assert_eq!(result["ok"], true);
    assert_eq!(result["projection_status"], "synced");
    let after = db.get_page(&page_id).await.unwrap().unwrap();
    assert_eq!(after.version, landed.version);
    assert_eq!(after.content, landed.content);
    assert_eq!(
        db.list_page_history(&page_id, 10).await.unwrap().len(),
        history_landed.len()
    );
    assert_projection_matches_page(&root, &db, &after);
}

#[tokio::test]
async fn old_operation_replay_projects_latest_page_without_reverting_later_edit() {
    let (app, tmp, db) = common::test_app().await;
    let page_id = common::create_page_fixture(
        &db,
        "Latest projection",
        "Initial body.",
        None,
        &[],
        "authored",
    )
    .await;
    let before = db.get_page(&page_id).await.unwrap().unwrap();
    let old_request = serde_json::json!({
        "content": "First saved body.",
        "expected_version": before.version,
        "caller_id": "wenlan-app",
        "operation_id": "old-projection-operation",
    });
    let response = app
        .clone()
        .oneshot(post_page(&page_id, old_request.clone()))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_json(response).await["projection_status"], "synced");
    let first = db.get_page(&page_id).await.unwrap().unwrap();
    let response = app
        .clone()
        .oneshot(post_page(
            &page_id,
            serde_json::json!({
                "content": "A newer body from a later save.",
                "expected_version": first.version,
                "caller_id": "wenlan-app",
                "operation_id": "new-projection-operation",
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_json(response).await["projection_status"], "synced");
    let latest = db.get_page(&page_id).await.unwrap().unwrap();
    let history_latest = db.list_page_history(&page_id, 10).await.unwrap();
    assert_eq!(latest.version, first.version + 1);
    let root = tmp.path().join("pages");
    let path = assert_projection_matches_page(&root, &db, &latest);
    std::fs::remove_file(path).unwrap();

    let response = app.oneshot(post_page(&page_id, old_request)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let result = response_json(response).await;
    assert_eq!(result["ok"], true);
    assert_eq!(result["projection_status"], "synced");
    let after = db.get_page(&page_id).await.unwrap().unwrap();
    assert_eq!(after.content, latest.content);
    assert_eq!(after.version, latest.version);
    assert_eq!(
        db.list_page_history(&page_id, 10).await.unwrap().len(),
        history_latest.len()
    );
    assert_projection_matches_page(&root, &db, &after);
}

#[tokio::test]
async fn unchanged_save_recreates_missing_projection_without_new_history_or_version() {
    let (app, tmp, db) = common::test_app().await;
    let page_id = common::create_page_fixture(
        &db,
        "Unchanged projection",
        "The unchanged body.",
        None,
        &[],
        "authored",
    )
    .await;
    let before = db.get_page(&page_id).await.unwrap().unwrap();
    let root = tmp.path().join("pages");
    KnowledgeProjectionWrite::new(root.clone(), &db)
        .write_page(&before)
        .unwrap();
    let path = assert_projection_matches_page(&root, &db, &before);
    std::fs::remove_file(path).unwrap();
    let history_before = db.list_page_history(&page_id, 10).await.unwrap();

    let response = app
        .oneshot(post_page(
            &page_id,
            serde_json::json!({
                "content": before.content,
                "expected_version": before.version,
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let result = response_json(response).await;
    assert_eq!(result["ok"], true);
    assert_eq!(result["projection_status"], "synced");
    let after = db.get_page(&page_id).await.unwrap().unwrap();
    assert_eq!(after.content, before.content);
    assert_eq!(after.version, before.version);
    assert_eq!(
        db.list_page_history(&page_id, 10).await.unwrap().len(),
        history_before.len()
    );
    assert_projection_matches_page(&root, &db, &after);
}

#[tokio::test]
async fn external_markdown_edit_survives_manual_save_replay_and_startup_reconcile() {
    let (app, tmp, db) = common::test_app().await;
    let page_id = common::create_page_fixture(
        &db,
        "External edit preservation",
        "The original projected prose.",
        None,
        &[],
        "authored",
    )
    .await;
    let before = db.get_page(&page_id).await.unwrap().unwrap();
    let root = tmp.path().join("pages");
    KnowledgeProjectionWrite::new(root.clone(), &db)
        .write_page(&before)
        .unwrap();
    let path = assert_projection_matches_page(&root, &db, &before);
    let projected = std::fs::read_to_string(&path).unwrap();
    let external = projected.replace(
        "The original projected prose.",
        "Unimported prose edited directly in the vault.",
    );
    assert_ne!(external, projected, "positive external edit control");
    std::fs::write(&path, &external).unwrap();
    let history_before = db.list_page_history(&page_id, 10).await.unwrap();
    let request = serde_json::json!({
        "content": "A separately saved edit in the app.",
        "expected_version": before.version,
        "caller_id": "wenlan-app",
        "operation_id": "preserve-external-projection",
    });

    for attempt in 0..2 {
        let response = app
            .clone()
            .oneshot(post_page(&page_id, request.clone()))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let result = response_json(response).await;
        assert_eq!(result["ok"], true);
        assert_eq!(result["projection_status"], "pending", "attempt {attempt}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            external,
            "unimported external prose must remain recoverable after save/replay"
        );
    }
    let after = db.get_page(&page_id).await.unwrap().unwrap();
    assert_eq!(after.content, "A separately saved edit in the app.");
    assert_eq!(after.version, before.version + 1);
    assert_eq!(
        db.list_page_history(&page_id, 10).await.unwrap().len(),
        history_before.len() + 1
    );
    assert!(db
        .get_operation_receipt("wenlan-app", "preserve-external-projection")
        .await
        .unwrap()
        .is_some());
    let (frontmatter, _) = extract_frontmatter(&external);
    assert_eq!(
        frontmatter
            .fields
            .get("origin_version")
            .and_then(|value| value.as_i64()),
        Some(before.version),
        "external edit now carries a stale stamp relative to the saved DB page"
    );

    // Exercise the same projection recovery entrypoint used at daemon startup.
    KnowledgeProjectionWrite::new(root, &db)
        .reconcile(std::slice::from_ref(&after))
        .unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), external);
    let reconciled = db.get_page(&page_id).await.unwrap().unwrap();
    assert_eq!(reconciled.content, after.content);
    assert_eq!(reconciled.version, after.version);
}
