// SPDX-License-Identifier: Apache-2.0
//! Synthetic HTTP contract: rename metadata without replacing the page.
mod common;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

fn rename(id: &str, title: &str, version: i64, space: Option<&str>) -> Request<Body> {
    let mut request = Request::builder()
        .method("POST")
        .uri(format!("/api/pages/{id}/rename"))
        .header("content-type", "application/json");
    if let Some(space) = space {
        request = request.header("x-wenlan-space", space);
    }
    request
        .body(Body::from(
            serde_json::json!({
                "title": title, "expected_version": version,
            })
            .to_string(),
        ))
        .unwrap()
}

#[tokio::test]
async fn page_rename_keeps_identity_body_and_history_and_rejects_stale_retry() {
    let (app, _tmp, db) = common::test_app().await;
    let id = common::create_page_fixture(
        &db,
        "Origin positioning",
        "Keep this exact body.",
        None,
        &[],
        "authored",
    )
    .await;
    let before = db.get_page(&id).await.unwrap().unwrap();
    let response = app
        .clone()
        .oneshot(rename(&id, " Wenlan positioning ", before.version, None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
    let reply: wenlan_types::responses::RenamePageResponse =
        serde_json::from_slice(&bytes).unwrap();
    assert_eq!(reply.id, id);
    assert_eq!(reply.title, "Wenlan positioning");
    assert_eq!(reply.version, before.version + 1);
    let after = db.get_page(&id).await.unwrap().unwrap();
    assert_eq!(after.content, before.content);
    assert_eq!(after.created_at, before.created_at);
    assert_eq!(after.source_memory_ids, before.source_memory_ids);
    assert!(db
        .list_page_history(&id, 10)
        .await
        .unwrap()
        .iter()
        .any(|v| v.version == reply.version));

    let stale = app
        .clone()
        .oneshot(rename(&id, "Stale title", before.version, None))
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    let unchanged = app
        .oneshot(rename(&id, &reply.title, reply.version, None))
        .await
        .unwrap();
    assert_eq!(unchanged.status(), StatusCode::OK);
    assert_eq!(
        db.get_page(&id).await.unwrap().unwrap().version,
        reply.version
    );
}

#[tokio::test]
async fn page_rename_rejects_blank_collision_and_missing_page_without_changes() {
    let (app, _tmp, db) = common::test_app().await;
    let id = common::create_page_fixture(&db, "Alpha", "Alpha body", None, &[], "authored").await;
    common::create_page_fixture(&db, "Beta", "Beta body", None, &[], "authored").await;
    let before = db.get_page(&id).await.unwrap().unwrap();
    for (title, status) in [
        ("  ", StatusCode::UNPROCESSABLE_ENTITY),
        ("beta", StatusCode::CONFLICT),
    ] {
        let response = app
            .clone()
            .oneshot(rename(&id, title, before.version, None))
            .await
            .unwrap();
        assert_eq!(response.status(), status);
    }
    let missing = app
        .oneshot(rename("page-missing", "Name", 1, None))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    let after = db.get_page(&id).await.unwrap().unwrap();
    assert_eq!(after.title, before.title);
    assert_eq!(after.version, before.version);
}

#[tokio::test]
async fn page_rename_requires_a_read_version_and_rejects_unknown_space() {
    let (app, _tmp, db) = common::test_app().await;
    let id = common::create_page_fixture(&db, "Scoped", "Body", None, &[], "authored").await;
    let request = Request::builder()
        .method("POST")
        .uri(format!("/api/pages/{id}/rename"))
        .header("content-type", "application/json")
        .body(Body::from(r#"{"title":"No version"}"#))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(request).await.unwrap().status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        app.oneshot(rename(&id, "New", 1, Some("unregistered-space")))
            .await
            .unwrap()
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(db.get_page(&id).await.unwrap().unwrap().title, "Scoped");
}

#[tokio::test]
async fn page_rename_cannot_cross_the_selected_space() {
    let (app, _tmp, db) = common::test_app().await;
    db.create_space("work", None, false).await.unwrap();
    db.create_space("private", None, false).await.unwrap();
    let id = common::create_page_fixture(
        &db,
        "Private title",
        "Private body",
        Some("private"),
        &[],
        "authored",
    )
    .await;
    let before = db.get_page(&id).await.unwrap().unwrap();
    let response = app
        .clone()
        .oneshot(rename(&id, "Changed", before.version, Some("work")))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let hidden = to_bytes(response.into_body(), 4096).await.unwrap();
    assert!(!String::from_utf8_lossy(&hidden).contains("Private"));
    assert_eq!(db.get_page(&id).await.unwrap().unwrap().title, before.title);
    let allowed = app
        .oneshot(rename(
            &id,
            "Private updated",
            before.version,
            Some("private"),
        ))
        .await
        .unwrap();
    assert_eq!(allowed.status(), StatusCode::OK);
}

#[tokio::test]
async fn page_rename_updates_links_in_the_same_space_and_reports_the_rest() {
    let (app, _tmp, db) = common::test_app().await;
    db.create_space("work", None, false).await.unwrap();
    db.create_space("home", None, false).await.unwrap();
    let target = common::create_page_fixture(
        &db,
        "Origin positioning",
        "Target body.",
        Some("work"),
        &[],
        "authored",
    )
    .await;
    let linker = common::create_page_fixture(
        &db,
        "Strategy",
        "See [[Origin positioning|the old name]] and `[[Origin positioning]]`.",
        Some("work"),
        &[],
        "authored",
    )
    .await;
    let elsewhere = common::create_page_fixture(
        &db,
        "Home notes",
        "Also [[Origin positioning]].",
        Some("home"),
        &[],
        "authored",
    )
    .await;
    let before = db.get_page(&target).await.unwrap().unwrap();
    let linker_before = db.get_page(&linker).await.unwrap().unwrap();
    let elsewhere_before = db.get_page(&elsewhere).await.unwrap().unwrap();

    let response = app
        .clone()
        .oneshot(rename(&target, "Wenlan positioning", before.version, None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
    let reply: wenlan_types::responses::RenamePageResponse =
        serde_json::from_slice(&bytes).unwrap();
    assert_eq!(reply.links_updated, 1);
    assert_eq!(reply.links_pending_review, 0);
    assert_eq!(reply.links_not_updated, 1);

    let linker_after = db.get_page(&linker).await.unwrap().unwrap();
    assert_eq!(
        linker_after.content,
        "See [[Wenlan positioning|the old name]] and `[[Origin positioning]]`."
    );
    assert_eq!(linker_after.version, linker_before.version + 1);
    let elsewhere_after = db.get_page(&elsewhere).await.unwrap().unwrap();
    assert_eq!(elsewhere_after.content, elsewhere_before.content);
    assert_eq!(elsewhere_after.version, elsewhere_before.version);

    let links = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/pages/{target}/links"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(links.status(), StatusCode::OK);
    let links: wenlan_types::responses::PageLinksResponse =
        serde_json::from_slice(&to_bytes(links.into_body(), 65536).await.unwrap()).unwrap();
    assert!(links
        .inbound
        .iter()
        .any(|link| link.source_page_id == linker && link.label == "Wenlan positioning"));
}

#[tokio::test]
async fn restoring_under_a_new_title_updates_links_and_reports_them() {
    let (app, _tmp, db) = common::test_app().await;
    db.create_space("work", None, false).await.unwrap();
    let target = common::create_page_fixture(
        &db,
        "Origin positioning",
        "Target body.",
        Some("work"),
        &[],
        "authored",
    )
    .await;
    let linker = common::create_page_fixture(
        &db,
        "Strategy",
        "See [[Origin positioning]].",
        Some("work"),
        &[],
        "authored",
    )
    .await;
    db.archive_page(&target).await.unwrap();
    let archived = db.get_page(&target).await.unwrap().unwrap();
    let linker_before = db.get_page(&linker).await.unwrap().unwrap();

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/pages/{target}/restore"))
                .header("content-type", "application/json")
                .header("x-wenlan-truth-contract", "1")
                .header("x-wenlan-reader-intent", "explicit")
                .body(Body::from(
                    serde_json::json!({
                        "title": "Wenlan positioning", "expected_version": archived.version,
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
    let reply: wenlan_types::responses::RestorePageResponse =
        serde_json::from_slice(&bytes).unwrap();
    assert_eq!(reply.title, "Wenlan positioning");
    assert_eq!(reply.links_updated, 1);
    assert_eq!(reply.links_pending_review, 0);
    assert_eq!(reply.links_not_updated, 0);

    let linker_after = db.get_page(&linker).await.unwrap().unwrap();
    assert_eq!(linker_after.content, "See [[Wenlan positioning]].");
    assert_eq!(linker_after.version, linker_before.version + 1);
}

#[tokio::test]
async fn a_second_rename_leaves_one_correct_link_card_on_a_locked_page() {
    let (app, _tmp, db) = common::test_app().await;
    db.create_space("work", None, false).await.unwrap();
    let target = common::create_page_fixture(
        &db,
        "Origin positioning",
        "Target body.",
        Some("work"),
        &[],
        "authored",
    )
    .await;
    let locked = common::create_page_fixture(
        &db,
        "Strategy",
        "See [[Origin positioning]].",
        Some("work"),
        &[],
        "authored",
    )
    .await;
    assert!(db.set_page_review_required(&locked, true).await.unwrap());
    let cards = |db: std::sync::Arc<wenlan_core::db::MemoryDB>, page: String| async move {
        db.list_pending_revisions(50)
            .await
            .unwrap()
            .into_iter()
            .filter(|card| card.target_source_id == page)
            .map(|card| card.revision_content)
            .collect::<Vec<_>>()
    };

    let mut version = db.get_page(&target).await.unwrap().unwrap().version;
    for (title, expected) in [
        ("Wenlan positioning", "See [[Wenlan positioning]]."),
        ("Positioning", "See [[Positioning]]."),
    ] {
        let response = app
            .clone()
            .oneshot(rename(&target, title, version, None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let reply: wenlan_types::responses::RenamePageResponse =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(reply.links_pending_review, 1);
        version = reply.version;
        assert_eq!(
            cards(db.clone(), locked.clone()).await,
            vec![expected.to_string()]
        );
    }

    // Back to the title the page still links by: no card is needed.
    let response = app
        .clone()
        .oneshot(rename(&target, "Origin positioning", version, None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(cards(db.clone(), locked.clone()).await.is_empty());
    assert_eq!(
        db.get_page(&locked).await.unwrap().unwrap().content,
        "See [[Origin positioning]]."
    );
}
