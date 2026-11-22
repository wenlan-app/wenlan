// SPDX-License-Identifier: Apache-2.0
//! Step 4 acceptance against the authenticated team router and real MCP service.

use super::*;
use serde_json::{json, Value};

const ORIGIN: &str = "https://team.example.com";
const LIBRARY: &str = "/team/v1/library";
const PRIVATE: &str = "orchid-local-only";
const PUBLIC: &str = "orchid-team-note";
const RESTRICTED: &str = "orchid-restricted-note";

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

async fn publish(f: &Fixture, cookie: &str, draft: Value) -> Value {
    let p = preview(f, cookie, draft).await;
    let (status, body) = confirm(f, cookie, &p).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

fn page(body: &str, space_id: Option<&str>) -> Value {
    json!({"kind":"page","title":"Release decision","body":body,"space_id":space_id})
}

async fn read(f: &Fixture, key: &str, path: &str) -> (StatusCode, String) {
    let (status, bytes) = call(&f.state, Method::GET, path, Some(key), vec![]).await;
    (status, String::from_utf8(bytes).unwrap())
}

async fn rpc(
    f: &Fixture,
    key: &str,
    path: &str,
    session: Option<&str>,
    message: Value,
) -> Response {
    let mut request = Request::post(path)
        .header(header::HOST, "team.example.com")
        .header(header::AUTHORIZATION, format!("Bearer {key}"))
        .header(header::ACCEPT, "application/json, text/event-stream")
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(session) = session {
        request = request
            .header("mcp-session-id", session)
            .header("mcp-protocol-version", "2025-06-18");
    }
    team_router(f.state.clone())
        .oneshot(request.body(Body::from(message.to_string())).unwrap())
        .await
        .unwrap()
}

async fn initialize(f: &Fixture, key: &str, path: &str) -> String {
    let response = rpc(
        f,
        key,
        path,
        None,
        json!({
            "jsonrpc":"2.0", "id":1, "method":"initialize",
            "params":{"protocolVersion":"2025-06-18","capabilities":{},
                      "clientInfo":{"name":"step4-acceptance","version":"1"}}
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let session = response.headers()["mcp-session-id"]
        .to_str()
        .unwrap()
        .to_owned();
    drop(response_text(response).await);
    let response = rpc(
        f,
        key,
        path,
        Some(&session),
        json!({"jsonrpc":"2.0", "method":"notifications/initialized"}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    session
}

async fn tool(f: &Fixture, key: &str, path: &str, session: &str, name: &str, args: Value) -> Value {
    let response = rpc(
        f,
        key,
        path,
        Some(session),
        json!({
            "jsonrpc":"2.0", "id":2, "method":"tools/call",
            "params":{"name":name,"arguments":args}
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response_text(response).await;
    let data = body
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .find(|data| !data.trim().is_empty())
        .unwrap_or(&body);
    serde_json::from_str(data).unwrap()
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
async fn step4_mcp_reads_apply_grants_before_retrieval() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let (mut f, db) = profiles().await;
    let personal_requests = Arc::new(AtomicUsize::new(0));
    let counter = personal_requests.clone();
    f.state.in_process_daemon = Some(crate::router::AppRouter::from_test_router(
        axum::Router::new().fallback(move || async move {
            counter.fetch_add(1, Ordering::SeqCst);
            StatusCode::INTERNAL_SERVER_ERROR
        }),
    ));
    seed(&db, "host-personal-id", PRIVATE).await;
    let writer = f.state.sessions.create_fresh(&f.writer.0.id).unwrap();
    let mut draft = page(PUBLIC, None);
    draft["brief"] = json!(true);
    draft["private_source"] = json!(true);
    let published = publish(&f, &writer, draft).await;
    let id = published["item_id"].as_str().unwrap();
    publish(&f, &writer, page(RESTRICTED, Some("design"))).await;
    let (owner, _) = team::owner_key(&f.state.team_path, "Owner").unwrap();
    let owner_session = f.state.sessions.create_fresh(&owner.id).unwrap();
    let (status, group) = browser(
        &f,
        &owner_session,
        Method::POST,
        &format!("{LIBRARY}/spaces"),
        json!({"name":"Roadmap"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let ordinary = publish(
        &f,
        &writer,
        page("A team-wide decision", group["id"].as_str()),
    )
    .await;
    let ordinary_url = format!("{LIBRARY}/pages/{}", ordinary["item_id"].as_str().unwrap());
    assert_eq!(read(&f, &f.reader.1, &ordinary_url).await.0, StatusCode::OK);

    let session = initialize(&f, &f.reader.1, "/mcp").await;
    for (name, args, marker) in [
        ("recall", json!({"topic":"orchid"}), PUBLIC),
        ("brief", json!({"topic":"orchid"}), PUBLIC),
        ("list_pages", json!({}), "Release decision"),
        ("read_page", json!({"page_id":id}), PUBLIC),
        ("get_page_sources", json!({"page_id":id}), "private source"),
    ] {
        let value = tool(&f, &f.reader.1, "/mcp", &session, name, args).await;
        let result = &value["result"];
        assert_ne!(result["isError"], true, "{name}: {value}");
        let blob = result["structuredContent"].to_string();
        assert!(blob.contains(marker), "{name}: {value}");
        assert!(
            !blob.contains(PRIVATE) && !blob.contains(RESTRICTED),
            "{blob}"
        );
        if name == "read_page" {
            assert!(
                blob.contains("publisher") && blob.contains(&f.writer.0.id),
                "{blob}"
            );
            assert!(
                blob.contains("revision") && blob.contains("human"),
                "{blob}"
            );
        }
    }
    let miss = tool(
        &f,
        &f.reader.1,
        "/mcp",
        &session,
        "recall",
        json!({"topic":PRIVATE}),
    )
    .await;
    assert_eq!(miss["result"]["structuredContent"]["memories"], json!([]));
    assert_eq!(miss["result"]["structuredContent"]["pages"], json!([]));
    assert_eq!(personal_requests.load(Ordering::SeqCst), 0);

    let (guest, key) = team::invite_with_key(
        &f.state.team_path,
        "Guest",
        &[TeamSpaceAccess {
            space: "Design".into(),
            access: TeamAccess::Read,
            id: None,
        }],
    )
    .unwrap();
    assert_ne!(guest.id, f.reader.0.id);
    let session = initialize(&f, &key, "/mcp/Design").await;
    let reply = tool(
        &f,
        &key,
        "/mcp/Design",
        &session,
        "recall",
        json!({"topic":"orchid"}),
    )
    .await;
    assert!(reply.to_string().contains(RESTRICTED) && !reply.to_string().contains(PUBLIC));
    let (status, issued) = browser(
        &f,
        &writer,
        Method::POST,
        &format!("{LIBRARY}/tool-keys"),
        json!({"name":"Narrow connector","space_id":"design"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let pinned_key = issued["key"].as_str().unwrap();
    let pinned = initialize(&f, pinned_key, "/mcp").await;
    let result = tool(
        &f,
        pinned_key,
        "/mcp",
        &pinned,
        "recall",
        json!({"query":"orchid"}),
    )
    .await;
    assert!(result.to_string().contains(RESTRICTED) && !result.to_string().contains(PUBLIC));
    assert_eq!(
        read(&f, pinned_key, &format!("{LIBRARY}/pages/{id}"))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        read(&f, &key, &format!("{LIBRARY}/pages/{id}")).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(read(&f, &key, &ordinary_url).await.0, StatusCode::NOT_FOUND);
    assert_eq!(
        read(&f, &f.outsider.1, &format!("{LIBRARY}/pages/{id}"))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        read(&f, "invalid-outsider-key", &format!("{LIBRARY}/pages/{id}"))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
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
    let mcp = initialize(&f, &f.reader.1, "/mcp").await;
    let result = tool(
        &f,
        &f.reader.1,
        "/mcp",
        &mcp,
        "recall",
        json!({"query":"orchid"}),
    )
    .await;
    let memories = result["result"]["structuredContent"]["memories"]
        .as_array()
        .unwrap();
    assert_eq!(memories.len(), 1);
    assert_eq!(memories[0]["body"], "approved redacted orchid");
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
async fn step4_human_write_recheck_forget_and_withdraw() {
    let (mut f, _) = profiles().await;
    let cookie = f.state.sessions.create_fresh(&f.writer.0.id).unwrap();
    let read_cookie = f.state.sessions.create_fresh(&f.reader.0.id).unwrap();
    let mut draft = page(PUBLIC, Some("design"));
    draft["memories"] = json!([{"title":"No partial write","body":"partially committed source"}]);
    let p = preview(&f, &cookie, draft.clone()).await;
    let uri = format!("{LIBRARY}/publish");
    let (status, _) = call(
        &f.state,
        Method::POST,
        &uri,
        Some(&f.writer.1),
        json!({"preview_id":p["id"]}).to_string().into_bytes(),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(
        browser(
            &f,
            &read_cookie,
            Method::POST,
            &format!("{LIBRARY}/preview"),
            page(PUBLIC, None)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    team::update_member(&f.state.team_path, &f.writer.0.id, None, Some(&[])).unwrap();
    assert_eq!(confirm(&f, &cookie, &p).await.0, StatusCode::FORBIDDEN);
    let (status, pages) = read(&f, &f.writer.1, &format!("{LIBRARY}/pages")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        serde_json::from_str::<Value>(&pages).unwrap()["pages"],
        json!([])
    );
    assert!(!read(
        &f,
        &f.writer.1,
        &format!("{LIBRARY}/recall?topic=partially")
    )
    .await
    .1
    .contains("partially committed source"));

    let local = wenlan_core::db::MemoryDB::new(
        &f._dir.path().join("writer"),
        Arc::new(wenlan_core::events::NoopEmitter),
    )
    .await
    .unwrap();
    seed(
        &local,
        "private-original",
        "private original remains personal",
    )
    .await;
    seed(&local, "another-original", "another private original").await;
    let mut draft = page(PUBLIC, None);
    draft["memories"] = json!([{"title":"Reviewed snapshot","body":"reviewed orchid"},
                               {"title":"Other snapshot","body":"other review"}]);
    let receipt = publish(&f, &cookie, draft).await;
    let id = receipt["item_id"].as_str().unwrap();
    let memory = receipt["memory_ids"][0].as_str().unwrap();
    local
        .forget_memory("private-original", "person", "acceptance")
        .await
        .unwrap();
    let sources = format!("{LIBRARY}/pages/{id}/sources");
    assert!(read(&f, &f.reader.1, &sources)
        .await
        .1
        .contains("reviewed orchid"));
    f.state.library = Some(library::Host::new(&f.state.team_path, false));
    assert_eq!(
        read(&f, &f.reader.1, &format!("{LIBRARY}/memories/{memory}"))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert!(!read(&f, &f.reader.1, &sources)
        .await
        .1
        .contains("reviewed orchid"));
    assert_eq!(
        browser(
            &f,
            &cookie,
            Method::POST,
            &format!("{LIBRARY}/withdraw/{memory}"),
            Value::Null
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    f.state.library = Some(library::Host::new(&f.state.team_path, true));
    let blob = read(&f, &f.reader.1, &sources).await.1;
    assert!(blob.contains("private source") && !blob.contains("reviewed orchid"));
    assert!(local
        .get_memory_detail("another-original")
        .await
        .unwrap()
        .is_some());

    let edit = preview(
        &f,
        &cookie,
        json!({
            "kind":"page","title":"Release decision","body":"offline edit",
            "item_id":id,"expected_revision":1
        }),
    )
    .await;
    assert_eq!(
        browser(
            &f,
            &cookie,
            Method::POST,
            &format!("{LIBRARY}/withdraw/{id}"),
            Value::Null
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(confirm(&f, &cookie, &edit).await.0, StatusCode::CONFLICT);
    f.state.library = Some(library::Host::new(&f.state.team_path, true));
    let replay = browser(
        &f,
        &cookie,
        Method::POST,
        &format!("{LIBRARY}/preview"),
        edit["draft"].clone(),
    )
    .await;
    assert_eq!(replay.0, StatusCode::CONFLICT);
    assert!(
        replay.1.to_string().contains("The item is unavailable"),
        "{replay:?}"
    );
    for suffix in ["", "/file", "/history", "/sources"] {
        assert_eq!(
            read(&f, &f.reader.1, &format!("{LIBRARY}/pages/{id}{suffix}"))
                .await
                .0,
            StatusCode::NOT_FOUND
        );
    }

    let session = initialize(&f, &f.reader.1, "/mcp").await;
    team::remove_member(&f.state.team_path, &f.reader.0.id).unwrap();
    assert_eq!(
        browser(&f, &read_cookie, Method::GET, &sources, Value::Null)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let reply = rpc(
        &f,
        &f.reader.1,
        "/mcp",
        Some(&session),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/list"}),
    )
    .await;
    assert_eq!(reply.status(), StatusCode::UNAUTHORIZED);
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
    f.state.library = Some(library::Host::new(&f.state.team_path, true));
    f.state.in_process_daemon = Some(crate::router::AppRouter::from_test_router(
        axum::Router::new().route(
            "/api/skills/rules",
            axum::routing::get(|| async {
                Json(json!({"space_id":"design","space_name":"Design","lines":[]}))
            }),
        ),
    ));
    let session = initialize(&f, &future, "/mcp/Design").await;
    let reply = tool(
        &f,
        &future,
        "/mcp/Design",
        &session,
        "recall",
        json!({"topic":PRIVATE}),
    )
    .await;
    assert_eq!(reply["result"]["structuredContent"]["memories"], json!([]));
    let rules = tool(
        &f,
        &future,
        "/mcp/Design",
        &session,
        "read_space_rules",
        json!({}),
    )
    .await;
    assert_eq!(rules["result"]["structuredContent"]["space_name"], "Design");
    let refused = tool(
        &f,
        &future,
        "/mcp/Design",
        &session,
        "write_page",
        json!({}),
    )
    .await;
    assert_eq!(refused["result"]["isError"], true);
    let (owner, _) = team::owner_key(&f.state.team_path, "Owner").unwrap();
    let cookie = f.state.sessions.create_fresh(&owner.id).unwrap();
    let withdrawal = browser(
        &f,
        &cookie,
        Method::POST,
        &format!("{LIBRARY}/withdraw/{}", public.id),
        Value::Null,
    )
    .await;
    assert_eq!(withdrawal.0, StatusCode::NO_CONTENT);
    assert_eq!(read(&f, &future, &url).await.0, StatusCode::NOT_FOUND);
    assert_eq!(
        read(
            &f,
            &future,
            &format!("/team/v1/app/api/pages/{}", public.id)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
}
