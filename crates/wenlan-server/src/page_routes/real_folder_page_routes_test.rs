use super::*;
use crate::{space_header::SpaceHeader, state::LintServerConfig, truth_guard::TruthView};

async fn project_active_page(
    id: &str,
    title: &str,
    folder: &str,
) -> (
    tempfile::TempDir,
    Arc<wenlan_core::db::MemoryDB>,
    std::path::PathBuf,
    wenlan_core::pages::Page,
    String,
) {
    let tmp = tempfile::tempdir().unwrap();
    let db = Arc::new(
        wenlan_core::db::MemoryDB::new(tmp.path(), Arc::new(wenlan_core::events::NoopEmitter))
            .await
            .unwrap(),
    );
    db.create_space("work", None, false).await.unwrap();
    let root = tmp.path().join("pages");
    std::fs::create_dir_all(root.join(folder)).unwrap();
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
    let absolute = wenlan_core::export::knowledge::KnowledgeProjectionWrite::new(root.clone(), &db)
        .write_page_gated(&db, &page)
        .await
        .unwrap()
        .unwrap();
    let relative = std::path::Path::new(&absolute)
        .strip_prefix(&root)
        .unwrap()
        .components()
        .map(|component| component.as_os_str().to_str().unwrap())
        .collect::<Vec<_>>()
        .join("/");
    (tmp, db, root, page, relative)
}

async fn set_stored_kind(tmp: &tempfile::TempDir, id: &str, kind: &str) {
    let fixture = libsql::Builder::new_local(tmp.path().join("origin_memory.db"))
        .build()
        .await
        .unwrap();
    let conn = fixture.connect().unwrap();
    conn.execute(
        "UPDATE pages SET kind = ?1 WHERE id = ?2",
        libsql::params![kind, id],
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn publish_reports_committed_page_with_pending_projection_and_retains_intent() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Arc::new(
        wenlan_core::db::MemoryDB::new(tmp.path(), Arc::new(wenlan_core::events::NoopEmitter))
            .await
            .unwrap(),
    );
    let root = tmp.path().join("pages");
    std::fs::create_dir_all(&root).unwrap();
    let state = Arc::new(RwLock::new(ServerState {
        db: Some(db.clone()),
        lint_config: LintServerConfig::new(vec![], Some(root.clone())),
        ..Default::default()
    }));
    let id = "page_00000000-0000-4000-8000-000000000fc1";
    let mut request =
        CreatePageDraftRequest::new(id.into(), "Durable folder".into(), "Body".into(), None);
    request.folder_path = Some("Missing".into());
    let Json(saved) = handle_create_page_draft(
        State(state.clone()),
        SpaceHeader(None),
        TruthView::automatic(),
        Json(request),
    )
    .await
    .unwrap();
    assert_eq!(saved.folder_path, "Missing");
    let Json(reloaded) = handle_get_page(
        State(state.clone()),
        axum::http::HeaderMap::new(),
        SpaceHeader(None),
        TruthView::automatic(),
        Path(id.into()),
    )
    .await
    .unwrap();
    assert_eq!(reloaded.page.folder_path.as_deref(), Some("Missing"));
    let Json(published) = handle_publish_page_draft(
        State(state.clone()),
        TruthView::automatic(),
        Path(id.into()),
        Json(PageDraftVersionRequest {
            expected_version: saved.page.version,
        }),
    )
    .await
    .unwrap();
    assert_eq!(published.page.status, "active");
    assert_eq!(
        published.projection_status,
        Some(PageProjectionStatus::Pending)
    );
    assert!(published.storage_path.is_none());
    assert!(published.projection_error.is_some());
    assert_eq!(db.get_page(id).await.unwrap().unwrap().status, "active");
    std::fs::create_dir(root.join("Missing")).unwrap();
    let Json(retry) = handle_publish_page_draft(
        State(state),
        TruthView::automatic(),
        Path(id.into()),
        Json(PageDraftVersionRequest {
            expected_version: saved.page.version,
        }),
    )
    .await
    .unwrap();
    assert_eq!(retry.projection_status, Some(PageProjectionStatus::Synced));
    let path = retry.storage_path.unwrap();
    assert!(path.starts_with("Missing/"));
    assert!(!std::path::Path::new(&path).is_absolute());
    assert!(root.join(path).is_file());
}

#[tokio::test]
async fn move_route_changes_projection_path_and_preserves_page_identity() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Arc::new(
        wenlan_core::db::MemoryDB::new(tmp.path(), Arc::new(wenlan_core::events::NoopEmitter))
            .await
            .unwrap(),
    );
    let root = tmp.path().join("pages");
    std::fs::create_dir_all(root.join("Research")).unwrap();
    let id = "page_00000000-0000-4000-8000-000000000fc3";
    let draft = db
        .create_page_draft_with_id_in_registered_space(id, "Moving note", "Body", None)
        .await
        .unwrap();
    let wenlan_core::pages::PageDraftPublishOutcome::Published(page) =
        db.publish_page_draft(id, draft.version).await.unwrap()
    else {
        panic!("publish")
    };
    let projection =
        wenlan_core::export::knowledge::KnowledgeProjectionWrite::new(root.clone(), &db);
    let initial = projection
        .write_page_gated(&db, &page)
        .await
        .unwrap()
        .unwrap();
    let expected = std::path::Path::new(&initial)
        .strip_prefix(&root)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let state = Arc::new(RwLock::new(ServerState {
        db: Some(db.clone()),
        lint_config: LintServerConfig::new(vec![], Some(root.clone())),
        ..Default::default()
    }));
    let Json(moved) = handle_move_page(
        State(state.clone()),
        SpaceHeader(None),
        TruthView::automatic(),
        Path(id.into()),
        Json(MovePageRequest {
            expected_storage_path: expected.clone(),
            folder_path: "Research".into(),
            operation_id: "00000000-0000-4000-8000-000000000fc4".into(),
        }),
    )
    .await
    .unwrap();
    assert_eq!(moved.storage_path, format!("Research/{expected}"));
    assert!(!root.join(&expected).exists());
    assert!(root.join(&moved.storage_path).is_file());
    // Direct/search-open detail carries the same verified nested path over HTTP.
    use tower::ServiceExt;
    let response = crate::router::build_router(state.clone())
        .oneshot(
            axum::http::Request::builder()
                .uri(format!("/api/pages/{id}"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 65536)
        .await
        .unwrap();
    let detail: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(detail["page"]["storage_path"], moved.storage_path);
    let saved = db.get_page(id).await.unwrap().unwrap();
    assert_eq!(saved.version, page.version);
    assert_eq!(saved.space, page.space);
    assert_eq!(saved.content, page.content);
    let rewritten = projection
        .write_page_gated(&db, &saved)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        std::path::Path::new(&rewritten),
        root.join(&moved.storage_path)
    );
    let Json(inventory) = handle_list_pages(
        State(state),
        SpaceHeader(None),
        TruthView::automatic(),
        axum::extract::Query(HashMap::new()),
    )
    .await
    .unwrap();
    assert_eq!(
        inventory.pages[0].storage_path.as_deref(),
        Some(moved.storage_path.as_str())
    );
}

#[tokio::test]
async fn detail_resolves_a_live_file_when_stored_kind_is_stale_overview() {
    let (tmp, db, root, page, expected) = project_active_page(
        "page_00000000-0000-4000-8000-000000000fc5",
        "Ordinary active file",
        "Existing/Nested",
    )
    .await;
    set_stored_kind(&tmp, &page.id, "overview").await;
    let state = Arc::new(RwLock::new(ServerState {
        db: Some(db),
        lint_config: LintServerConfig::new(vec![], Some(root)),
        ..Default::default()
    }));

    let Json(detail) = handle_get_page(
        State(state),
        axum::http::HeaderMap::new(),
        SpaceHeader(Some("work".into())),
        TruthView::automatic(),
        Path(page.id),
    )
    .await
    .unwrap();
    assert_eq!(detail.page.storage_path.as_deref(), Some(expected.as_str()));
}

#[tokio::test]
async fn move_accepts_a_live_file_when_stored_kind_is_stale_overview() {
    let (tmp, db, root, page, expected) = project_active_page(
        "page_00000000-0000-4000-8000-000000000fc6",
        "Ordinary active file",
        "Existing/Nested",
    )
    .await;
    set_stored_kind(&tmp, &page.id, "overview").await;
    std::fs::create_dir_all(root.join("Moved")).unwrap();
    let state = Arc::new(RwLock::new(ServerState {
        db: Some(db.clone()),
        lint_config: LintServerConfig::new(vec![], Some(root.clone())),
        ..Default::default()
    }));

    let Json(moved) = handle_move_page(
        State(state.clone()),
        SpaceHeader(Some("work".into())),
        TruthView::automatic(),
        Path(page.id.clone()),
        Json(MovePageRequest {
            expected_storage_path: expected.clone(),
            folder_path: "Moved".into(),
            operation_id: "00000000-0000-4000-8000-000000000fc7".into(),
        }),
    )
    .await
    .unwrap();
    let filename = std::path::Path::new(&expected)
        .file_name()
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(moved.storage_path, format!("Moved/{filename}"));
    assert!(!root.join(&expected).exists());
    assert!(root.join(&moved.storage_path).is_file());

    let saved = db.get_page(&page.id).await.unwrap().unwrap();
    assert_eq!(saved.id, page.id);
    assert_eq!(saved.title, page.title);
    assert_eq!(saved.space, Some("work".into()));
    assert_eq!(saved.version, page.version);
    let Json(detail) = handle_get_page(
        State(state),
        axum::http::HeaderMap::new(),
        SpaceHeader(Some("work".into())),
        TruthView::automatic(),
        Path(page.id),
    )
    .await
    .unwrap();
    assert_eq!(detail.page.storage_path, Some(moved.storage_path));
}

#[tokio::test]
async fn overview_title_with_stale_concept_kind_has_no_file_path() {
    let (tmp, db, root, page, _expected) = project_active_page(
        "page_00000000-0000-4000-8000-000000000fc8",
        wenlan_core::synthesis::overview::OVERVIEW_PAGE_TITLE,
        "ReservedOverview/Nested",
    )
    .await;
    set_stored_kind(&tmp, &page.id, "concept").await;
    let state = Arc::new(RwLock::new(ServerState {
        db: Some(db),
        lint_config: LintServerConfig::new(vec![], Some(root)),
        ..Default::default()
    }));

    let Json(detail) = handle_get_page(
        State(state),
        axum::http::HeaderMap::new(),
        SpaceHeader(Some("work".into())),
        TruthView::automatic(),
        Path(page.id),
    )
    .await
    .unwrap();
    assert!(detail.page.storage_path.is_none());
}

#[tokio::test]
async fn overview_title_with_stale_concept_kind_cannot_be_moved() {
    let (tmp, db, root, page, expected) = project_active_page(
        "page_00000000-0000-4000-8000-000000000fc9",
        wenlan_core::synthesis::overview::OVERVIEW_PAGE_TITLE,
        "ReservedOverview/Nested",
    )
    .await;
    set_stored_kind(&tmp, &page.id, "concept").await;
    std::fs::create_dir_all(root.join("Moved")).unwrap();
    let state = Arc::new(RwLock::new(ServerState {
        db: Some(db),
        lint_config: LintServerConfig::new(vec![], Some(root.clone())),
        ..Default::default()
    }));

    let result = handle_move_page(
        State(state),
        SpaceHeader(Some("work".into())),
        TruthView::automatic(),
        Path(page.id),
        Json(MovePageRequest {
            expected_storage_path: expected.clone(),
            folder_path: "Moved".into(),
            operation_id: "00000000-0000-4000-8000-000000000fca".into(),
        }),
    )
    .await;
    assert!(matches!(result, Err(ServerError::NotFound(_))));
    assert!(root.join(&expected).is_file());
    assert!(std::fs::read_dir(root.join("Moved"))
        .unwrap()
        .next()
        .is_none());
}

#[tokio::test]
async fn move_checks_scope_before_missing_configuration_or_disk_paths() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Arc::new(
        wenlan_core::db::MemoryDB::new(tmp.path(), Arc::new(wenlan_core::events::NoopEmitter))
            .await
            .unwrap(),
    );
    db.create_space("private", None, false).await.unwrap();
    db.create_space("work", None, false).await.unwrap();
    let id = "page_00000000-0000-4000-8000-000000000fc2";
    let draft = db
        .create_page_draft_with_id_in_registered_space(id, "Secret", "Body", Some("private"))
        .await
        .unwrap();
    db.publish_page_draft(id, draft.version).await.unwrap();
    let state = Arc::new(RwLock::new(ServerState {
        db: Some(db),
        ..Default::default()
    }));
    let result = handle_move_page(
        State(state.clone()),
        SpaceHeader(Some("work".into())),
        TruthView::automatic(),
        Path(id.into()),
        Json(MovePageRequest {
            expected_storage_path: "secret.md".into(),
            folder_path: "../escape".into(),
            operation_id: "bad".into(),
        }),
    )
    .await;
    assert!(matches!(result, Err(ServerError::NotFound(_))));
    let fixture = libsql::Builder::new_local(tmp.path().join("origin_memory.db"))
        .build()
        .await
        .unwrap();
    let conn = fixture.connect().unwrap();
    let db = state.read().await.db.clone().unwrap();
    let page = db.get_page(id).await.unwrap().unwrap();
    conn.execute("INSERT INTO page_truth_state(page_id,page_version,support_status,human_reviewed,updated_at,evaluated_at) VALUES(?1,?2,'provisional',0,1,1)",libsql::params![id,page.version]).await.unwrap();
    conn.execute("INSERT OR REPLACE INTO claim_derivation_markers(page_id,page_version,page_version_digest,extractor_version,inventory_count,created_at) VALUES(?1,?2,?3,?4,1,0)",libsql::params![id,page.version,wenlan_core::provenance::revision_content_digest(&page.content),wenlan_core::db::EXTRACTOR_VERSION]).await.unwrap();
    // Exercise enforcement explicitly; the production default promoter is advisory.
    db.set_app_metadata("claim_promoter_enforcement", "1")
        .await
        .unwrap();
    db.set_truth_cutover_generation(1).await.unwrap();
    let hidden = handle_move_page(
        State(state.clone()),
        SpaceHeader(None),
        TruthView::automatic(),
        Path(id.into()),
        Json(MovePageRequest {
            expected_storage_path: "secret.md".into(),
            folder_path: "../escape".into(),
            operation_id: "bad".into(),
        }),
    )
    .await;
    assert!(matches!(hidden, Err(ServerError::NotFound(_))));
    use tower::ServiceExt;
    let response = crate::router::build_router(state)
        .oneshot(
            axum::http::Request::builder()
                .uri(format!("/api/pages/{id}"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::NOT_FOUND);
    let bytes = axum::body::to_bytes(response.into_body(), 65536)
        .await
        .unwrap();
    let hidden_wire: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(hidden_wire.get("page").is_none());
    assert!(hidden_wire.get("storage_path").is_none());
}
