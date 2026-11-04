// SPDX-License-Identifier: Apache-2.0
//! Exercise the organization tools against the real daemon router and scratch
//! Wiki projection. This proves the MCP wrappers, daemon routes, and projection
//! agree on folder paths and scoped page visibility.

use std::{
    ffi::OsString,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};

use rmcp::model::CallToolResult;
use tokio::{sync::RwLock, task::JoinHandle};
use wenlan_core::{db::MemoryDB, events::NoopEmitter};
use wenlan_mcp::{
    client::WenlanClient,
    tools::{
        CreateFolderParams, CreateRelationParams, ListEntitiesParams, ListFoldersParams,
        ListPagesParams, MovePageParams, TransportMode, WenlanMcpServer,
    },
};
use wenlan_types::{requests::CreateConceptRequest, RawDocument, WriteSpaceTarget};

struct EnvRestore(Vec<(&'static str, Option<OsString>)>);

impl EnvRestore {
    fn scratch(data_dir: &Path) -> Self {
        let keys = [
            "WENLAN_DATA_DIR",
            "WENLAN_NO_AUTOSTART",
            "WENLAN_SPACE",
            "WENLAN_DEFAULT_SPACE",
        ];
        let saved = keys
            .into_iter()
            .map(|key| (key, std::env::var_os(key)))
            .collect();
        std::env::set_var("WENLAN_DATA_DIR", data_dir);
        std::env::set_var("WENLAN_NO_AUTOSTART", "1");
        std::env::remove_var("WENLAN_SPACE");
        std::env::remove_var("WENLAN_DEFAULT_SPACE");
        wenlan_mcp::lock_state::init_from_env();
        Self(saved)
    }

    fn lock_to(&self, space: &str) {
        std::env::set_var("WENLAN_SPACE", space);
        wenlan_mcp::lock_state::init_from_env();
    }
}

impl Drop for EnvRestore {
    fn drop(&mut self) {
        for (key, value) in &self.0 {
            if let Some(value) = value {
                std::env::set_var(key, value);
            } else {
                std::env::remove_var(key);
            }
        }
        wenlan_mcp::lock_state::init_from_env();
    }
}

struct TestDaemon {
    _tmp: tempfile::TempDir,
    db: Arc<MemoryDB>,
    page_root: PathBuf,
    base_url: String,
    server_task: Option<JoinHandle<()>>,
}

impl TestDaemon {
    async fn start() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let db = Arc::new(
            MemoryDB::new(tmp.path(), Arc::new(NoopEmitter))
                .await
                .unwrap(),
        );
        db.create_space("work", None, false).await.unwrap();
        db.create_space("private", None, false).await.unwrap();

        let page_root = tmp.path().join("pages");
        std::fs::create_dir_all(&page_root).unwrap();
        let state = Arc::new(RwLock::new(
            wenlan_server::state::ServerState {
                db: Some(db.clone()),
                ..wenlan_server::state::ServerState::default()
            }
            .with_page_root(page_root.clone()),
        ));
        let router = wenlan_server::router::build_router(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        let server_task = tokio::spawn(async move {
            axum::serve(listener, router.into_make_service())
                .await
                .unwrap();
        });

        Self {
            _tmp: tmp,
            db,
            page_root,
            base_url: format!("http://{addr}"),
            server_task: Some(server_task),
        }
    }

    async fn stop(mut self) {
        if let Some(task) = self.server_task.take() {
            task.abort();
            let _ = task.await;
        }
    }
}

impl Drop for TestDaemon {
    fn drop(&mut self) {
        if let Some(task) = self.server_task.take() {
            task.abort();
        }
    }
}

fn make_server(base_url: &str) -> WenlanMcpServer {
    let client = WenlanClient::new(base_url.to_string()).with_agent_name("organize-test".into());
    WenlanMcpServer::new(client, TransportMode::Stdio, "organize-test".into(), None)
}

fn structured(result: &CallToolResult) -> serde_json::Value {
    assert_ne!(result.is_error, Some(true), "tool error: {result:?}");
    result
        .structured_content
        .clone()
        .unwrap_or_else(|| panic!("expected structured result: {result:?}"))
}

fn text_of(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .find_map(|content| match content {
            rmcp::model::ContentBlock::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("expected text result: {result:?}"))
}

fn active_page<'a>(pages: &'a serde_json::Value, id: &str) -> &'a serde_json::Value {
    pages["pages"]
        .as_array()
        .expect("pages array")
        .iter()
        .find(|page| page["page_id"] == id)
        .unwrap_or_else(|| panic!("page {id} missing from {pages}"))
}

async fn create_active_page(
    db: &MemoryDB,
    page_root: &Path,
    title: &str,
    content: &str,
    space: &str,
    source_memory_ids: Vec<String>,
) -> wenlan_types::pages::Page {
    let result = wenlan_core::post_write::create_page(
        db,
        CreateConceptRequest {
            title: title.into(),
            content: content.into(),
            summary: None,
            entity_id: None,
            space: WriteSpaceTarget::Named(space.into()),
            source_memory_ids,
            creation_kind: Some("authored".into()),
            workspace: None,
        },
        "organize-test",
        Some(page_root),
    )
    .await
    .unwrap();
    db.get_page(&result.id).await.unwrap().unwrap()
}

async fn seed_private_page_in_folder(
    db: &MemoryDB,
    page_root: &Path,
) -> (wenlan_types::pages::Page, String) {
    let id = "page_00000000-0000-4000-8000-00000000a901";
    let draft = db
        .create_page_draft_with_id_in_registered_space_and_folder(
            id,
            "Private Archive Note",
            "Confidential body",
            Some("private"),
            "Private Cabinet",
        )
        .await
        .unwrap();
    let wenlan_core::pages::PageDraftPublishOutcome::Published(page) =
        db.publish_page_draft(id, draft.version).await.unwrap()
    else {
        panic!("private page should publish")
    };
    let absolute =
        wenlan_core::export::knowledge::KnowledgeProjectionWrite::new(page_root.to_path_buf(), db)
            .write_page_gated(db, &page)
            .await
            .unwrap()
            .unwrap();
    let relative = Path::new(&absolute)
        .strip_prefix(page_root)
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/");
    (page, relative)
}

fn page_snapshot(page: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "id": page["id"],
        "content": page["content"],
        "source_memory_ids": page["source_memory_ids"],
        "version": page["version"],
        "space": page["space"],
    })
}

#[tokio::test]
async fn organization_tools_use_real_router_and_preserve_scoped_page_state() {
    let scratch = tempfile::tempdir().unwrap();
    let _env = EnvRestore::scratch(scratch.path());
    let daemon = TestDaemon::start().await;
    let server = make_server(&daemon.base_url);

    // These public MCP calls build all folders under the test-owned projection.
    for (parent_path, name, expected) in [
        ("", "Projects", "Projects"),
        ("Projects", "Research", "Projects/Research"),
        ("", "Private Cabinet", "Private Cabinet"),
        ("", "Collision", "Collision"),
    ] {
        let created = structured(
            &server
                .create_folder_impl(CreateFolderParams {
                    parent_path: parent_path.into(),
                    name: name.into(),
                })
                .await
                .unwrap(),
        );
        assert_eq!(created["path"], expected);
        let repeated = structured(
            &server
                .create_folder_impl(CreateFolderParams {
                    parent_path: parent_path.into(),
                    name: name.into(),
                })
                .await
                .unwrap(),
        );
        assert_eq!(repeated["path"], expected, "folder creation is idempotent");
    }

    let (private_page, private_path) =
        seed_private_page_in_folder(&daemon.db, &daemon.page_root).await;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let source_id = "organize-source-moving-brief";
    daemon
        .db
        .upsert_documents(vec![
            RawDocument {
                source: "memory".into(),
                source_id: source_id.into(),
                title: "Moving brief evidence".into(),
                content: "The verified research brief tracks the project decision.".into(),
                last_modified: now,
                space: Some("work".into()),
                source_agent: Some("organize-test".into()),
                ..RawDocument::default()
            },
            RawDocument {
                source: "memory".into(),
                source_id: "organize-private-source".into(),
                title: "Private relation evidence".into(),
                content: "The private initiative has a private relationship.".into(),
                last_modified: now,
                space: Some("private".into()),
                source_agent: Some("organize-test".into()),
                ..RawDocument::default()
            },
        ])
        .await
        .unwrap();

    let target = create_active_page(
        &daemon.db,
        &daemon.page_root,
        "Link Target",
        "The destination page remains in the Wiki root.",
        "work",
        Vec::new(),
    )
    .await;
    let moving = create_active_page(
        &daemon.db,
        &daemon.page_root,
        "Moving Brief",
        "A source-backed brief with an outbound link to [[Link Target]].",
        "work",
        vec![source_id.into()],
    )
    .await;
    let incoming = create_active_page(
        &daemon.db,
        &daemon.page_root,
        "Incoming Reference",
        "This page points back to [[Moving Brief]].",
        "work",
        Vec::new(),
    )
    .await;
    let unrelated = create_active_page(
        &daemon.db,
        &daemon.page_root,
        "Unconnected Note",
        "This note has no wiki links and must not change.",
        "work",
        Vec::new(),
    )
    .await;
    let work_entity = daemon
        .db
        .store_entity(
            "Organize Project",
            "project",
            Some("work"),
            Some("organize-test"),
            None,
        )
        .await
        .unwrap();
    let work_target_entity = daemon
        .db
        .store_entity(
            "Organize Target",
            "project",
            Some("work"),
            Some("organize-test"),
            None,
        )
        .await
        .unwrap();
    let work_third_entity = daemon
        .db
        .store_entity(
            "Organize Third",
            "project",
            Some("work"),
            Some("organize-test"),
            None,
        )
        .await
        .unwrap();
    let private_entity = daemon
        .db
        .store_entity(
            "Private Initiative",
            "project",
            Some("private"),
            Some("organize-test"),
            None,
        )
        .await
        .unwrap();

    let first_relation = server
        .create_relation_impl(CreateRelationParams {
            from_entity_id: work_entity.clone(),
            to_entity_id: work_target_entity.clone(),
            relation_type: "uses".into(),
            source_memory_id: source_id.into(),
        })
        .await
        .unwrap();
    assert_ne!(
        first_relation.is_error,
        Some(true),
        "{}",
        text_of(&first_relation)
    );
    let first_text = text_of(&first_relation);
    assert!(
        first_text.contains("write_outcome: created"),
        "{first_text}"
    );
    let relation_id = first_text
        .split_whitespace()
        .nth(1)
        .expect("relation id follows Relation")
        .to_string();
    let retry_relation = server
        .create_relation_impl(CreateRelationParams {
            from_entity_id: work_entity.clone(),
            to_entity_id: work_target_entity.clone(),
            relation_type: "uses".into(),
            source_memory_id: source_id.into(),
        })
        .await
        .unwrap();
    let retry_text = text_of(&retry_relation);
    assert_ne!(retry_relation.is_error, Some(true), "{retry_text}");
    assert!(
        retry_text.contains("write_outcome: unchanged"),
        "{retry_text}"
    );
    assert_eq!(
        retry_text.split_whitespace().nth(1),
        Some(relation_id.as_str()),
        "idempotent relation retry returns the original edge"
    );
    let relations_before_conflict = daemon
        .db
        .list_relations_between(&work_entity, &work_target_entity)
        .await
        .unwrap();
    assert_eq!(
        relations_before_conflict,
        vec![(relation_id.clone(), "uses".into())]
    );
    let conflicting_relation = server
        .create_relation_impl(CreateRelationParams {
            from_entity_id: work_entity.clone(),
            to_entity_id: work_target_entity.clone(),
            relation_type: "depends_on".into(),
            source_memory_id: source_id.into(),
        })
        .await
        .unwrap();
    assert_eq!(conflicting_relation.is_error, Some(true));
    assert_eq!(
        daemon
            .db
            .list_relations_between(&work_entity, &work_target_entity)
            .await
            .unwrap(),
        relations_before_conflict,
        "a different relation type cannot replace the existing edge"
    );

    let listed = structured(
        &server
            .list_pages_impl(ListPagesParams {
                status: None,
                limit: None,
                offset: None,
                space: Some("work".into()),
            })
            .await
            .unwrap(),
    );
    let initial_path = active_page(&listed, &moving.id)["storage_path"]
        .as_str()
        .expect("source-backed page storage path")
        .to_string();
    let unrelated_path = active_page(&listed, &unrelated.id)["storage_path"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(initial_path.ends_with("moving-brief.md"), "{initial_path}");

    let folders = structured(
        &server
            .list_folders_impl(ListFoldersParams { space: None })
            .await
            .unwrap(),
    );
    let folder_paths: Vec<&str> = folders["folders"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|folder| folder["path"].as_str())
        .collect();
    for expected in ["Projects", "Projects/Research", "Private Cabinet"] {
        assert!(
            folder_paths.contains(&expected),
            "missing {expected}: {folder_paths:?}"
        );
    }

    let before_read = structured(&server.read_page_impl(&moving.id).await.unwrap());
    let before_snapshot = page_snapshot(&before_read["page"]);
    assert_eq!(before_snapshot["id"], moving.id);
    assert_eq!(before_snapshot["content"], moving.content);
    assert_eq!(before_snapshot["source_memory_ids"][0], source_id);
    assert_eq!(before_snapshot["version"], moving.version);
    assert_eq!(before_snapshot["space"], "work");
    let unrelated_before = structured(&server.read_page_impl(&unrelated.id).await.unwrap());
    let unrelated_snapshot = page_snapshot(&unrelated_before["page"]);

    let before_links = structured(&server.get_page_links_impl(&moving.id).await.unwrap());
    assert!(before_links["outbound"]
        .as_array()
        .unwrap()
        .iter()
        .any(|link| link["target_page_id"] == target.id));
    assert!(before_links["inbound"]
        .as_array()
        .unwrap()
        .iter()
        .any(|link| link["source_page_id"] == incoming.id));

    let entities = structured(
        &server
            .list_entities_impl(ListEntitiesParams {
                query: Some("Organize Project".into()),
                space: Some("work".into()),
                status: None,
                limit: None,
                offset: None,
            })
            .await
            .unwrap(),
    );
    assert!(entities["entities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entity| entity["id"] == work_entity));
    let entity_detail = structured(&server.read_entity_impl(&work_entity).await.unwrap());
    assert_eq!(entity_detail["entity"]["id"], work_entity);
    assert_eq!(entity_detail["entity"]["space"], "work");

    let moved = structured(
        &server
            .move_page_impl(MovePageParams {
                page_id: moving.id.clone(),
                expected_storage_path: initial_path.clone(),
                folder_path: "Projects/Research".into(),
                operation_id: "organize-move-moving-brief-001".into(),
            })
            .await
            .unwrap(),
    );
    let moved_path = moved["storage_path"].as_str().unwrap().to_string();
    assert_eq!(moved_path, format!("Projects/Research/{initial_path}"));
    let repeated_move = structured(
        &server
            .move_page_impl(MovePageParams {
                page_id: moving.id.clone(),
                expected_storage_path: initial_path.clone(),
                folder_path: "Projects/Research".into(),
                operation_id: "organize-move-moving-brief-001".into(),
            })
            .await
            .unwrap(),
    );
    assert_eq!(
        repeated_move["storage_path"], moved_path,
        "same move retry is idempotent"
    );

    let after_list = structured(
        &server
            .list_pages_impl(ListPagesParams {
                status: None,
                limit: None,
                offset: None,
                space: Some("work".into()),
            })
            .await
            .unwrap(),
    );
    assert_eq!(
        active_page(&after_list, &moving.id)["storage_path"],
        moved_path
    );
    assert_eq!(
        active_page(&after_list, &unrelated.id)["storage_path"],
        unrelated_path
    );
    let after_read = structured(&server.read_page_impl(&moving.id).await.unwrap());
    assert_eq!(page_snapshot(&after_read["page"]), before_snapshot);
    let unrelated_after = structured(&server.read_page_impl(&unrelated.id).await.unwrap());
    assert_eq!(page_snapshot(&unrelated_after["page"]), unrelated_snapshot);
    let after_links = structured(&server.get_page_links_impl(&moving.id).await.unwrap());
    assert_eq!(after_links["outbound"], before_links["outbound"]);
    assert_eq!(after_links["inbound"], before_links["inbound"]);

    let stale = server
        .move_page_impl(MovePageParams {
            page_id: moving.id.clone(),
            expected_storage_path: initial_path.clone(),
            folder_path: "Projects".into(),
            operation_id: "organize-move-stale-expected-002".into(),
        })
        .await
        .unwrap();
    assert_eq!(
        stale.is_error,
        Some(true),
        "stale expected path must conflict"
    );
    assert_eq!(
        active_page(
            &structured(
                &server
                    .list_pages_impl(ListPagesParams {
                        status: None,
                        limit: None,
                        offset: None,
                        space: Some("work".into()),
                    })
                    .await
                    .unwrap(),
            ),
            &moving.id,
        )["storage_path"],
        moved_path
    );

    // A pre-existing destination file must survive, and the source projection
    // must remain at its current location when the destination cannot be used.
    let moved_bytes = std::fs::read(daemon.page_root.join(&moved_path)).unwrap();
    let filename = Path::new(&moved_path).file_name().unwrap();
    let collision_path = daemon.page_root.join("Collision").join(filename);
    let collision_marker = b"pre-existing destination file";
    std::fs::write(&collision_path, collision_marker).unwrap();
    let collision = server
        .move_page_impl(MovePageParams {
            page_id: moving.id.clone(),
            expected_storage_path: moved_path.clone(),
            folder_path: "Collision".into(),
            operation_id: "organize-move-collision-003".into(),
        })
        .await
        .unwrap();
    assert_eq!(
        collision.is_error,
        Some(true),
        "occupied destination must refuse"
    );
    assert!(daemon.page_root.join(&moved_path).is_file());
    assert_eq!(
        std::fs::read(daemon.page_root.join(&moved_path)).unwrap(),
        moved_bytes
    );
    assert_eq!(std::fs::read(&collision_path).unwrap(), collision_marker);

    // A strict Space lock overrides explicit tool arguments and the daemon
    // independently refuses cross-Space reads and moves.
    _env.lock_to("work");
    let hidden_folders = structured(
        &server
            .list_folders_impl(ListFoldersParams {
                space: Some("private".into()),
            })
            .await
            .unwrap(),
    );
    assert!(!hidden_folders["folders"]
        .as_array()
        .unwrap()
        .iter()
        .any(|folder| folder["path"] == "Private Cabinet"));
    let hidden_page_read = server.read_page_impl(&private_page.id).await.unwrap();
    assert_eq!(hidden_page_read.is_error, Some(true));
    let hidden_move = server
        .move_page_impl(MovePageParams {
            page_id: private_page.id.clone(),
            expected_storage_path: private_path.clone(),
            folder_path: "Projects".into(),
            operation_id: "organize-move-cross-space-004".into(),
        })
        .await
        .unwrap();
    assert_eq!(hidden_move.is_error, Some(true));
    assert!(daemon.page_root.join(&private_path).is_file());
    let hidden_entity = server.read_entity_impl(&private_entity).await.unwrap();
    assert_eq!(hidden_entity.is_error, Some(true));

    let foreign_endpoint_relation = server
        .create_relation_impl(CreateRelationParams {
            from_entity_id: work_entity.clone(),
            to_entity_id: private_entity.clone(),
            relation_type: "uses".into(),
            source_memory_id: source_id.into(),
        })
        .await
        .unwrap();
    assert_eq!(foreign_endpoint_relation.is_error, Some(true));
    assert!(daemon
        .db
        .list_relations_between(&work_entity, &private_entity)
        .await
        .unwrap()
        .is_empty());

    let foreign_source_relation = server
        .create_relation_impl(CreateRelationParams {
            from_entity_id: work_entity.clone(),
            to_entity_id: work_third_entity.clone(),
            relation_type: "uses".into(),
            source_memory_id: "organize-private-source".into(),
        })
        .await
        .unwrap();
    assert_eq!(foreign_source_relation.is_error, Some(true));
    assert_eq!(
        daemon
            .db
            .list_relations_between(&work_entity, &work_third_entity)
            .await
            .unwrap(),
        Vec::<(String, String)>::new()
    );

    daemon.stop().await;
}
