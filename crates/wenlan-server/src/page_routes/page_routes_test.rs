use super::*;
use crate::state::LintServerConfig;
use crate::truth_guard::TruthView;
use wenlan_core::db::MemoryDB;
use wenlan_core::pages::PageDraftPublishOutcome;

#[tokio::test]
async fn page_inventory_reports_live_files_after_scope_and_keeps_drafts_null() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Arc::new(
        MemoryDB::new(tmp.path(), Arc::new(wenlan_core::events::NoopEmitter))
            .await
            .unwrap(),
    );
    let root = tmp.path().join("pages");
    let visible_id = "page_00000000-0000-4000-8000-0000000000a1";
    let other_id = "page_00000000-0000-4000-8000-0000000000a2";
    let draft_id = "page_00000000-0000-4000-8000-0000000000a3";
    let mut visible_filename = String::new();
    for (id, space) in [(visible_id, None), (other_id, Some("private"))] {
        let draft = db
            .create_page_draft_with_id(id, id, "Body", space, space)
            .await
            .unwrap();
        let PageDraftPublishOutcome::Published(page) =
            db.publish_page_draft(id, draft.version).await.unwrap()
        else {
            panic!("expected publish");
        };
        let filename =
            wenlan_core::export::knowledge::KnowledgeProjectionWrite::new(root.clone(), &db)
                .write_page(&page)
                .unwrap();
        if id == visible_id {
            visible_filename = std::path::Path::new(&filename)
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .to_string();
        }
    }
    db.create_page_draft_with_id(draft_id, "Draft", "Body", None, None)
        .await
        .unwrap();
    // Deliberately stale state claims a draft has a file. Inventory must not
    // turn that into a draft projection promise, even if the bytes match.
    let state_path = root.join(".wenlan/state.json");
    let mut projection_state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&state_path).unwrap()).unwrap();
    let mut draft_entry = projection_state["pages"][visible_id].clone();
    draft_entry["file"] = serde_json::json!("draft.md");
    projection_state["pages"][draft_id] = draft_entry;
    std::fs::write(&state_path, serde_json::to_vec(&projection_state).unwrap()).unwrap();
    std::fs::write(
        root.join("draft.md"),
        format!("---\norigin_id: {draft_id}\n---\nBody\n"),
    )
    .unwrap();
    let state = Arc::new(RwLock::new(ServerState {
        db: Some(db),
        lint_config: LintServerConfig::new(vec![], Some(root.clone())),
        ..Default::default()
    }));
    let params = HashMap::from([("space".into(), "uncategorized".into())]);
    let Json(inventory) = handle_list_pages(
        State(state.clone()),
        axum::http::HeaderMap::new(),
        crate::space_header::SpaceHeader(None),
        TruthView::automatic(),
        axum::extract::Query(params.clone()),
    )
    .await
    .unwrap();
    assert_eq!(inventory.pages.len(), 1);
    assert_eq!(inventory.pages[0].page.id, visible_id);
    assert_eq!(
        inventory.pages[0].storage_path.as_deref(),
        Some(visible_filename.as_str())
    );
    std::fs::remove_file(root.join(&visible_filename)).unwrap();
    let Json(inventory) = handle_list_pages(
        State(state.clone()),
        axum::http::HeaderMap::new(),
        crate::space_header::SpaceHeader(None),
        TruthView::automatic(),
        axum::extract::Query(params.clone()),
    )
    .await
    .unwrap();
    assert_eq!(inventory.pages[0].storage_path, None);
    let mut params = params;
    params.insert("status".into(), "draft".into());
    let Json(inventory) = handle_list_pages(
        State(state),
        axum::http::HeaderMap::new(),
        crate::space_header::SpaceHeader(None),
        TruthView::automatic(),
        axum::extract::Query(params),
    )
    .await
    .unwrap();
    assert_eq!(inventory.pages.len(), 1);
    assert_eq!(inventory.pages[0].page.id, draft_id);
    assert_eq!(inventory.pages[0].storage_path, None);
}
