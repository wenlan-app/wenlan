// SPDX-License-Identifier: AGPL-3.0-only
use super::MemberView;

fn product() -> MemberView {
    MemberView {
        space: "產品".into(),
        folder: "Acme/產品".into(),
    }
}

#[test]
fn a_server_path_is_shown_under_the_space_name() {
    let view = product();
    assert_eq!(
        view.shown("Acme/產品/路線.md").as_deref(),
        Some("產品/路線.md")
    );
    assert_eq!(
        view.shown("Acme/產品/Q3/路線.md").as_deref(),
        Some("產品/Q3/路線.md")
    );
    assert_eq!(view.shown("Acme/產品").as_deref(), Some("產品"));
    assert_eq!(view.shown("Acme/設計/x.md"), None);
    assert_eq!(view.shown("Acme/產品 2/x.md"), None);
    assert_eq!(view.shown("私人/x.md"), None);
}

#[test]
fn a_folder_the_member_names_maps_inside_their_space_only() {
    let view = product();
    assert_eq!(view.folder_path("產品").unwrap(), "Acme/產品");
    assert_eq!(view.folder_path("產品/Q3").unwrap(), "Acme/產品/Q3");
    for bad in [
        "",
        "設計",
        "設計/Q3",
        "產品 2",
        "產品/..",
        "產品/../設計",
        "產品/Q3/../../x",
        "/產品",
        "產品\\Q3",
        "產品/.wenlan",
        "產品/archive",
    ] {
        assert!(view.folder_path(bad).is_err(), "{bad:?} accepted");
    }
}

#[test]
fn a_member_page_location_uses_its_id_and_shared_folder() {
    let view = product();
    let live = "Acme/產品/路線.md";
    assert_eq!(
        view.page_location("page-a", live).as_deref(),
        Some("產品/page-a.md")
    );
    assert_eq!(view.page_location("page-a", "Acme/設計/私.md"), None);
    assert_eq!(
        view.file_path("page-a", "產品/Q3/page-a.md", live).unwrap(),
        "Acme/產品/Q3/路線.md"
    );
    for bad in [
        "產品/路線.md",
        "設計/page-a.md",
        "產品/../設計/page-a.md",
        "產品/page-b.md",
    ] {
        assert!(
            view.file_path("page-a", bad, live).is_err(),
            "{bad:?} accepted"
        );
    }
}

mod routes {
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use serde_json::Value;
    use tokio::sync::RwLock;
    use tower::ServiceExt;

    use crate::state::{LintServerConfig, ServerState, SharedState};

    /// A Space whose folder is spelled differently from its name, so a
    /// path the member sees can never pass for a server path.
    const SPACE: &str = "產品:A";
    const FOLDER: &str = "產品-A";

    struct Setup {
        _tmp: tempfile::TempDir,
        db: Arc<wenlan_core::db::MemoryDB>,
        root: std::path::PathBuf,
        state: SharedState,
    }

    async fn setup(layout_on: bool) -> Setup {
        let tmp = tempfile::tempdir().unwrap();
        let db = Arc::new(
            wenlan_core::db::MemoryDB::new(tmp.path(), Arc::new(wenlan_core::events::NoopEmitter))
                .await
                .unwrap(),
        );
        db.create_space(SPACE, None, false).await.unwrap();
        db.create_space("設計", None, false).await.unwrap();
        let root = tmp.path().join("pages");
        std::fs::create_dir_all(root.join(FOLDER)).unwrap();
        std::fs::create_dir_all(root.join("設計").join("私")).unwrap();
        if layout_on {
            wenlan_core::space_folders::turn_on(&root).unwrap();
        }
        let state = Arc::new(RwLock::new(ServerState {
            db: Some(db.clone()),
            lint_config: LintServerConfig::new(vec![], Some(root.clone())),
            ..Default::default()
        }));
        Setup {
            _tmp: tmp,
            db,
            root,
            state,
        }
    }

    fn member_call(method: &str, uri: &str, body: Option<Value>) -> Request<Body> {
        member_call_in(SPACE, method, uri, body)
    }

    fn member_call_in(space: &str, method: &str, uri: &str, body: Option<Value>) -> Request<Body> {
        let builder = Request::builder()
            .method(method)
            .uri(uri)
            .header(
                crate::team_actor::SECRET_HEADER,
                crate::team_actor::secret(),
            )
            .header(crate::team_actor::KIND_HEADER, "person")
            .header(crate::team_actor::MEMBER_HEADER, "m-1")
            .header(crate::team_actor::MEMBER_NAME_HEADER, "Amy")
            .header(crate::space_header::HEADER_NAME, space)
            .header("content-type", "application/json");
        builder
            .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
            .unwrap()
    }

    async fn send(state: &SharedState, req: Request<Body>) -> (StatusCode, Value) {
        let response = crate::router::build_router(state.clone())
            .oneshot(req)
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    fn paths(answer: &Value) -> Vec<String> {
        answer["folders"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["path"].as_str().unwrap().to_string())
            .collect()
    }

    #[tokio::test]
    async fn a_member_lists_and_makes_folders_only_inside_their_space() {
        let s = setup(true).await;
        let (status, answer) =
            send(&s.state, member_call("GET", "/api/knowledge/folders", None)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(paths(&answer), vec![SPACE.to_string()]);

        let (status, answer) = send(
            &s.state,
            member_call(
                "POST",
                "/api/knowledge/folders",
                Some(serde_json::json!({"parent_path": SPACE, "name": "Q3"})),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{answer}");
        assert!(s.root.join(FOLDER).join("Q3").is_dir());
        assert!(!answer.to_string().contains(FOLDER), "{answer}");

        let (_, answer) = send(&s.state, member_call("GET", "/api/knowledge/folders", None)).await;
        assert_eq!(
            paths(&answer),
            vec![SPACE.to_string(), format!("{SPACE}/Q3")]
        );

        for parent in ["設計", "", "產品:A/..", "產品:A/../設計", FOLDER] {
            let (status, _) = send(
                &s.state,
                member_call(
                    "POST",
                    "/api/knowledge/folders",
                    Some(serde_json::json!({"parent_path": parent, "name": "x"})),
                ),
            )
            .await;
            assert!(status.is_client_error(), "{parent:?} gave {status}");
        }
        assert!(!s.root.join("設計").join("x").exists());
        assert!(!s.root.join("x").exists());
        assert!(!s.root.join(FOLDER).join("x").exists());
    }

    #[tokio::test]
    async fn with_the_layout_off_a_member_has_no_folders() {
        let s = setup(false).await;
        let (status, answer) =
            send(&s.state, member_call("GET", "/api/knowledge/folders", None)).await;
        assert_eq!(status, StatusCode::OK);
        assert!(paths(&answer).is_empty());
        let (status, _) = send(
            &s.state,
            member_call(
                "POST",
                "/api/knowledge/folders",
                Some(serde_json::json!({"parent_path": SPACE, "name": "Q3"})),
            ),
        )
        .await;
        assert!(status.is_client_error(), "{status}");
        assert!(!s.root.join(FOLDER).join("Q3").exists());
    }

    #[tokio::test]
    async fn a_member_moves_a_page_by_the_paths_they_see_and_only_inside_their_space() {
        let s = setup(true).await;
        std::fs::create_dir_all(s.root.join(FOLDER).join("Q3")).unwrap();
        let id = "page_00000000-0000-4000-8000-00000000f01d";
        let draft =
            s.db.create_page_draft_with_id_in_registered_space_and_folder(
                id,
                "路線",
                "Body",
                Some(SPACE),
                FOLDER,
            )
            .await
            .unwrap();
        let wenlan_core::pages::PageDraftPublishOutcome::Published(page) =
            s.db.publish_page_draft(id, draft.version).await.unwrap()
        else {
            panic!("publish")
        };
        let absolute =
            wenlan_core::export::knowledge::KnowledgeProjectionWrite::new(s.root.clone(), &s.db)
                .write_page_gated(&s.db, &page)
                .await
                .unwrap()
                .unwrap();
        let file = std::path::Path::new(&absolute)
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        let shown = format!("{SPACE}/{id}.md");
        let uri = format!("/api/pages/{id}/move");

        for (n, folder) in ["設計", "設計/私", "產品:A/..", FOLDER]
            .into_iter()
            .enumerate()
        {
            let (status, _) = send(
                &s.state,
                member_call(
                    "POST",
                    &uri,
                    Some(serde_json::json!({
                        "expected_storage_path": shown,
                        "folder_path": folder,
                        "operation_id": format!("00000000-0000-4000-8000-00000000f0a{n}"),
                    })),
                ),
            )
            .await;
            assert!(status.is_client_error(), "{folder:?} gave {status}");
        }
        assert!(s.root.join(FOLDER).join(&file).is_file());

        // Pinned to another Space, the page is not there at all.
        let (status, _) = send(
            &s.state,
            member_call_in(
                "設計",
                "POST",
                &uri,
                Some(serde_json::json!({
                    "expected_storage_path": format!("設計/{id}.md"),
                    "folder_path": "設計/私",
                    "operation_id": "00000000-0000-4000-8000-00000000f0b0",
                })),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(s.root.join(FOLDER).join(&file).is_file());

        let (status, answer) = send(
            &s.state,
            member_call(
                "POST",
                &uri,
                Some(serde_json::json!({
                    "expected_storage_path": shown,
                    "folder_path": format!("{SPACE}/Q3"),
                    "operation_id": "00000000-0000-4000-8000-00000000f0b1",
                })),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{answer}");
        assert!(answer.get("storage_path").is_none());
        assert_eq!(answer["space_path"], format!("{SPACE}/Q3/{id}.md"));
        assert!(s.root.join(FOLDER).join("Q3").join(&file).is_file());
    }
}
