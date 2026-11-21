use super::*;
use crate::space_header::SpaceHeader;
use crate::state::{LintServerConfig, ServerState};
use crate::truth_guard::TruthView;
use axum::extract::{Query, State};
use std::sync::Arc;
use tokio::sync::RwLock;

#[tokio::test]
async fn folder_http_routes_round_trip_and_reject_scoped_creation() {
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;
    let tmp = tempfile::tempdir().unwrap();
    let db = Arc::new(
        wenlan_core::db::MemoryDB::new(tmp.path(), Arc::new(wenlan_core::events::NoopEmitter))
            .await
            .unwrap(),
    );
    db.create_space("work", None, false).await.unwrap();
    let root = tmp.path().join("pages");
    std::fs::create_dir(&root).unwrap();
    let state = Arc::new(RwLock::new(ServerState {
        db: Some(db),
        lint_config: LintServerConfig::new(vec![], Some(root.clone())),
        ..Default::default()
    }));
    let response = crate::router::build_router(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/knowledge/folders")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"parent_path":"","name":"Writing"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = crate::router::build_router(state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/knowledge/folders")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 65536)
        .await
        .unwrap();
    let response: wenlan_types::responses::KnowledgeFoldersResponse =
        serde_json::from_slice(&bytes).unwrap();
    assert_eq!(response.folders[0].path, "Writing");
    assert!(!response.truncated);
    let response = crate::router::build_router(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/knowledge/folders")
                .header("content-type", "application/json")
                .header("x-wenlan-space", "work")
                .body(Body::from(r#"{"parent_path":"","name":"Forbidden"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(!root.join("Forbidden").exists());
}

#[tokio::test]
async fn scoped_folder_listing_returns_only_authorized_live_ancestors() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Arc::new(
        wenlan_core::db::MemoryDB::new(tmp.path(), Arc::new(wenlan_core::events::NoopEmitter))
            .await
            .unwrap(),
    );
    db.create_space("work", None, false).await.unwrap();
    db.create_space("private", None, false).await.unwrap();
    let root = tmp.path().join("pages");
    std::fs::create_dir_all(root.join("Visible/Nested")).unwrap();
    std::fs::create_dir_all(root.join("Secret")).unwrap();
    std::fs::create_dir_all(root.join("Empty")).unwrap();
    for (id, space, folder) in [
        (
            "page_00000000-0000-4000-8000-000000000fb1",
            "work",
            "Visible/Nested",
        ),
        (
            "page_00000000-0000-4000-8000-000000000fb2",
            "private",
            "Secret",
        ),
    ] {
        let draft = db
            .create_page_draft_with_id_in_registered_space_and_folder(
                id,
                id,
                "Body",
                Some(space),
                folder,
            )
            .await
            .unwrap();
        let wenlan_core::pages::PageDraftPublishOutcome::Published(page) =
            db.publish_page_draft(id, draft.version).await.unwrap()
        else {
            panic!("publish")
        };
        wenlan_core::export::knowledge::KnowledgeProjectionWrite::new(root.clone(), &db)
            .write_page_gated(&db, &page)
            .await
            .unwrap();
    }
    // A live old projection must not reveal its folder after a failed verdict.
    let hidden_id = "page_00000000-0000-4000-8000-000000000fb3";
    std::fs::create_dir(root.join("HiddenTruth")).unwrap();
    let draft = db
        .create_page_draft_with_id_in_registered_space_and_folder(
            hidden_id,
            "Hidden",
            "Body",
            Some("work"),
            "HiddenTruth",
        )
        .await
        .unwrap();
    let wenlan_core::pages::PageDraftPublishOutcome::Published(hidden) = db
        .publish_page_draft(hidden_id, draft.version)
        .await
        .unwrap()
    else {
        panic!("publish")
    };
    wenlan_core::export::knowledge::KnowledgeProjectionWrite::new(root.clone(), &db)
        .write_page_gated(&db, &hidden)
        .await
        .unwrap();
    let fixture = libsql::Builder::new_local(tmp.path().join("origin_memory.db"))
        .build()
        .await
        .unwrap();
    let conn = fixture.connect().unwrap();
    conn.execute("INSERT INTO page_truth_state(page_id,page_version,support_status,human_reviewed,updated_at,evaluated_at) VALUES(?1,?2,'provisional',0,1,1)",libsql::params![hidden_id,hidden.version]).await.unwrap();
    conn.execute("INSERT OR REPLACE INTO claim_derivation_markers(page_id,page_version,page_version_digest,extractor_version,inventory_count,created_at) VALUES(?1,?2,?3,?4,1,0)",libsql::params![hidden_id,hidden.version,wenlan_core::provenance::revision_content_digest(&hidden.content),wenlan_core::db::EXTRACTOR_VERSION]).await.unwrap();
    // Exercise enforcement explicitly; the production default promoter is advisory.
    db.set_app_metadata("claim_promoter_enforcement", "1")
        .await
        .unwrap();
    db.set_truth_cutover_generation(1).await.unwrap();
    let state = Arc::new(RwLock::new(ServerState {
        db: Some(db),
        lint_config: LintServerConfig::new(vec![], Some(root)),
        ..Default::default()
    }));
    let crate::member_view::PageResponse::Local(response) = handle_list_knowledge_folders(
        State(state.clone()),
        SpaceHeader(Some("work".into())),
        axum::http::HeaderMap::new(),
        TruthView::automatic(),
        Query(Default::default()),
    )
    .await
    .unwrap() else {
        panic!("local folder response")
    };
    assert_eq!(
        response
            .folders
            .iter()
            .map(|f| f.path.as_str())
            .collect::<Vec<_>>(),
        vec!["Visible", "Visible/Nested"]
    );
    assert!(!response.truncated);
    let crate::member_view::PageResponse::Local(global) = handle_list_knowledge_folders(
        State(state.clone()),
        SpaceHeader(None),
        axum::http::HeaderMap::new(),
        TruthView::automatic(),
        Query(Default::default()),
    )
    .await
    .unwrap() else {
        panic!("local folder response")
    };
    assert!(global.folders.iter().any(|f| f.path == "Empty"));
    assert!(global.folders.iter().any(|f| f.path == "Secret"));
    assert!(handle_create_knowledge_folder(
        State(state.clone()),
        SpaceHeader(Some("work".into())),
        axum::http::HeaderMap::new(),
        Json(wenlan_types::requests::CreateKnowledgeFolderRequest {
            parent_path: "".into(),
            name: "Forbidden".into()
        })
    )
    .await
    .is_err());
    let crate::member_view::PageResponse::Local(created) = handle_create_knowledge_folder(
        State(state),
        SpaceHeader(None),
        axum::http::HeaderMap::new(),
        Json(wenlan_types::requests::CreateKnowledgeFolderRequest {
            parent_path: "Visible".into(),
            name: "New".into(),
        }),
    )
    .await
    .unwrap() else {
        panic!("local folder response")
    };
    assert_eq!(created.path, "Visible/New");
}

#[tokio::test]
async fn scoped_folder_listing_uses_authoritative_page_fields_for_live_files() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Arc::new(
        wenlan_core::db::MemoryDB::new(tmp.path(), Arc::new(wenlan_core::events::NoopEmitter))
            .await
            .unwrap(),
    );
    db.create_space("work", None, false).await.unwrap();
    let root = tmp.path().join("pages");
    std::fs::create_dir_all(root.join("Ordinary/Nested")).unwrap();
    std::fs::create_dir_all(root.join("Overview/Nested")).unwrap();

    for (id, title, folder) in [
        (
            "page_00000000-0000-4000-8000-000000000fb4",
            "Ordinary active file",
            "Ordinary/Nested",
        ),
        (
            "page_00000000-0000-4000-8000-000000000fb5",
            wenlan_core::synthesis::overview::OVERVIEW_PAGE_TITLE,
            "Overview/Nested",
        ),
    ] {
        let draft = db
            .create_page_draft_with_id_in_registered_space_and_folder(
                id,
                title,
                "Body",
                Some("work"),
                folder,
            )
            .await
            .unwrap();
        let wenlan_core::pages::PageDraftPublishOutcome::Published(page) =
            db.publish_page_draft(id, draft.version).await.unwrap()
        else {
            panic!("publish")
        };
        wenlan_core::export::knowledge::KnowledgeProjectionWrite::new(root.clone(), &db)
            .write_page_gated(&db, &page)
            .await
            .unwrap()
            .unwrap();
    }

    let fixture = libsql::Builder::new_local(tmp.path().join("origin_memory.db"))
        .build()
        .await
        .unwrap();
    let conn = fixture.connect().unwrap();
    conn.execute(
        "UPDATE pages SET kind = 'overview' WHERE id = ?1",
        ["page_00000000-0000-4000-8000-000000000fb4"],
    )
    .await
    .unwrap();
    conn.execute(
        "UPDATE pages SET kind = 'concept' WHERE id = ?1",
        ["page_00000000-0000-4000-8000-000000000fb5"],
    )
    .await
    .unwrap();

    let state = Arc::new(RwLock::new(ServerState {
        db: Some(db),
        lint_config: LintServerConfig::new(vec![], Some(root)),
        ..Default::default()
    }));
    let crate::member_view::PageResponse::Local(response) = handle_list_knowledge_folders(
        State(state),
        SpaceHeader(Some("work".into())),
        axum::http::HeaderMap::new(),
        TruthView::automatic(),
        Query(Default::default()),
    )
    .await
    .unwrap() else {
        panic!("local folder response")
    };
    assert_eq!(
        response
            .folders
            .iter()
            .map(|folder| folder.path.as_str())
            .collect::<Vec<_>>(),
        vec!["Ordinary", "Ordinary/Nested"]
    );
}
