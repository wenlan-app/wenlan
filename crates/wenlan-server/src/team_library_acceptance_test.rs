// SPDX-License-Identifier: Apache-2.0
//! Step 4 acceptance against the authenticated team router.

use super::*;
use serde_json::{json, Value};

const ORIGIN: &str = "https://team.example.com";
const LIBRARY: &str = "/team/v1/library";
const PRIVATE: &str = "orchid-local-only";
const PUBLIC: &str = "orchid-team-note";

async fn profiles() -> (Fixture, Arc<wenlan_core::db::MemoryDB>) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.scratch/step4/acceptance");
    std::fs::create_dir_all(&root).unwrap();
    let run = tempfile::tempdir_in(root).unwrap();
    for profile in ["host", "writer", "reader", "outsider"] {
        let profile = run.path().join(profile);
        let pages = profile.join("pages");
        std::fs::create_dir_all(&pages).unwrap();
        std::fs::write(
            profile.join("config.json"),
            json!({"knowledge_path":pages}).to_string(),
        )
        .unwrap();
    }
    let mut f = fixture_in_profile(run, "host");
    f.state.library = Some(library::Host::new(&f.state.team_path, true));
    team::set_url(&f.state.team_path, Some(ORIGIN)).unwrap();
    let mut settings: Value =
        serde_json::from_slice(&std::fs::read(&f.state.team_path).unwrap()).unwrap();
    for m in settings["members"].as_array_mut().unwrap() {
        if m["id"] == f.writer.0.id {
            m["library_access"] = json!("write");
        }
        if m["id"] == f.reader.0.id {
            m["library_access"] = json!("read");
            m["spaces"] = json!({});
        }
    }
    std::fs::write(&f.state.team_path, serde_json::to_vec(&settings).unwrap()).unwrap();
    with_daemon(f).await
}

async fn browser(
    f: &Fixture,
    cookie: &str,
    method: Method,
    path: &str,
    data: Value,
) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(path)
        .header(header::COOKIE, format!("__Host-wenlan-session={cookie}"))
        .header(header::ORIGIN, ORIGIN)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(data.to_string()))
        .unwrap();
    let response = team_router(f.state.clone()).oneshot(req).await.unwrap();
    let status = response.status();
    let body = response_text(response).await;
    (
        status,
        serde_json::from_str(&body).unwrap_or(Value::String(body)),
    )
}

async fn preview(f: &Fixture, cookie: &str, draft: Value) -> Value {
    let (status, body) = browser(
        f,
        cookie,
        Method::POST,
        &format!("{LIBRARY}/preview"),
        draft,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

async fn confirm(f: &Fixture, cookie: &str, preview: &Value) -> (StatusCode, Value) {
    browser(
        f,
        cookie,
        Method::POST,
        &format!("{LIBRARY}/publish"),
        json!({"preview_id": preview["id"]}),
    )
    .await
}

fn page(body: &str, space_id: Option<&str>) -> Value {
    json!({"kind":"page","title":"Release decision","body":body,"space_id":space_id})
}

async fn read(f: &Fixture, key: &str, path: &str) -> (StatusCode, String) {
    let (status, bytes) = call(&f.state, Method::GET, path, Some(key), vec![]).await;
    (status, String::from_utf8(bytes).unwrap())
}

async fn seed(db: &wenlan_core::db::MemoryDB, id: &str, body: &str) {
    db.upsert_documents(vec![wenlan_core::sources::RawDocument {
        source: "memory".into(),
        source_id: id.into(),
        title: "Private title".into(),
        content: body.into(),
        confirmed: Some(true),
        ..Default::default()
    }])
    .await
    .unwrap();
}

#[tokio::test]
async fn step4_review_publishes_only_approved_snapshot_and_retries_once() {
    let (f, _) = profiles().await;
    let local = wenlan_core::db::MemoryDB::new(
        &f._dir.path().join("writer"),
        Arc::new(wenlan_core::events::NoopEmitter),
    )
    .await
    .unwrap();
    seed(&local, "writer-approved-id", "raw orchid budget 173").await;
    seed(&local, "writer-declined-id", "declined private quote").await;
    let cookie = f.state.sessions.create_fresh(&f.writer.0.id).unwrap();
    let mut draft = page(PUBLIC, None);
    draft["memories"] = json!([{"title":"Approved note","body":"approved redacted orchid"}]);
    draft["private_source"] = json!(true);
    let p = preview(&f, &cookie, draft).await;
    assert_eq!(p["draft"]["body"], PUBLIC);
    assert!(p["audience"]["now"].to_string().contains(&f.reader.0.id));
    assert!(!p["audience"]["later"].as_str().unwrap().is_empty());
    let (status, receipt) = confirm(&f, &cookie, &p).await;
    assert_eq!(status, StatusCode::OK, "{receipt}");
    assert_eq!(confirm(&f, &cookie, &p).await, (status, receipt.clone()));
    assert_eq!(receipt["memory_ids"].as_array().unwrap().len(), 1);
    let id = receipt["item_id"].as_str().unwrap();
    local
        .update_memory("writer-approved-id", "edited original")
        .await
        .unwrap();

    let (status, sources) = read(&f, &f.reader.1, &format!("{LIBRARY}/pages/{id}/sources")).await;
    assert_eq!(status, StatusCode::OK);
    let list: Value = serde_json::from_str(&sources).unwrap();
    assert_eq!(list["sources"].as_array().unwrap().len(), 2);
    assert!(sources.contains("approved redacted orchid") && sources.contains("private source"));
    for suffix in ["", "/sources", "/history", "/file"] {
        let (status, body) = read(&f, &f.reader.1, &format!("{LIBRARY}/pages/{id}{suffix}")).await;
        assert_eq!(status, StatusCode::OK, "{suffix}: {body}");
        for forbidden in [
            "raw orchid budget",
            "declined private",
            "writer-approved-id",
            "writer-declined-id",
            "edited original",
        ] {
            assert!(
                !body.contains(forbidden),
                "{suffix} exposed {forbidden}: {body}"
            );
        }
    }

    // A host's terminal review also checks the personal original again,
    // before it can create any team copy or save a personal receipt.
    use wenlan_types::team_library::{TeamOriginal, TeamTerminalAction, TeamTerminalReply};
    team::owner_key(&f.state.team_path, "Owner").unwrap();
    let original = local
        .get_memory_detail("writer-approved-id")
        .await
        .unwrap()
        .unwrap();
    let draft = serde_json::from_value(json!({
        "kind":"memory","title":"Terminal snap","body":"approved terminal text"
    }))
    .unwrap();
    let host = f.state.library.as_ref().unwrap();
    let reply = library::terminal_action(
        host,
        &f.state,
        &local,
        TeamTerminalAction::Preview {
            draft,
            originals: vec![TeamOriginal {
                kind: wenlan_types::team_library::TeamItemKind::Memory,
                id: original.source_id,
                revision: original.version,
            }],
        },
    )
    .await
    .unwrap();
    let TeamTerminalReply::Preview { preview } = reply else {
        panic!("expected terminal preview");
    };
    local
        .update_memory("writer-approved-id", "changed during review")
        .await
        .unwrap();
    assert!(matches!(
        library::terminal_action(
            host,
            &f.state,
            &local,
            TeamTerminalAction::Publish {
                preview_id: preview.id
            },
        )
        .await,
        Err(wenlan_core::team_library::LibraryError::Conflict(_))
    ));
    assert!(
        !read(&f, &f.reader.1, &format!("{LIBRARY}/recall?topic=terminal"))
            .await
            .1
            .contains("approved terminal text")
    );
}

#[tokio::test]
async fn step4_legacy_migration_keeps_only_member_heads_and_refuses_uploads() {
    let (mut f, db) = profiles().await;
    f.state.library = Some(library::Host::new(&f.state.team_path, false));
    let public = create_folder_test_page(&db, "Legacy shared page", "authored").await;
    let hidden = create_folder_test_page(&db, "Legacy private page", "imported").await;
    seed(&db, "legacy-personal-memory", PRIVATE).await;
    sync_hosted_test_folder(&f, &db).await;
    let old = team::list_folder_files(&f.folder, &db.shared_folder_pages("Design").await.unwrap())
        .into_iter()
        .find(|path| path.contains(&public.id))
        .unwrap();
    db.update_page_content(&public.id, "Second authorized edition.", &[], "manual_edit")
        .await
        .unwrap();
    sync_hosted_test_folder(&f, &db).await;
    let file = format!("{LIBRARY}/pages/{}", public.id);
    assert_eq!(read(&f, &f.writer.1, &file).await.0, StatusCode::OK);
    assert_eq!(
        read(&f, &f.writer.1, &format!("{LIBRARY}/pages/{}", hidden.id))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(read(&f, &f.reader.1, &file).await.0, StatusCode::NOT_FOUND);

    let (status, body) = read(&f, &f.writer.1, "/team/v1/spaces/Design/files").await;
    assert_eq!(status, StatusCode::OK);
    let files: TeamFileList = serde_json::from_str(&body).unwrap();
    let path = files
        .files
        .iter()
        .find(|path| path.contains(&public.id))
        .unwrap();
    let url = format!("/team/v1/spaces/Design/file?path={path}");
    assert_eq!(
        read(&f, &f.writer.1, &url).await,
        (
            StatusCode::OK,
            std::fs::read_to_string(f.folder.join(path)).unwrap()
        )
    );
    assert_eq!(
        read(
            &f,
            &f.writer.1,
            &format!("/team/v1/spaces/Design/file?path={old}")
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let upload = format!(
        "/team/v1/spaces/Design/versions/{}/{}",
        public.id,
        path.rsplit('/').next().unwrap()
    );
    assert_eq!(
        call(&f.state, Method::PUT, &upload, Some(&f.writer.1), vec![])
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    team::set_everyone(&f.state.team_path, "Design", Some(TeamAccess::Read)).unwrap();
    let (_, future) = team::add_member(&f.state.team_path, "Future", &[]).unwrap();
    assert_eq!(read(&f, &future, &file).await.0, StatusCode::OK);
}
