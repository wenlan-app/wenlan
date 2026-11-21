// SPDX-License-Identifier: Apache-2.0

use super::*;
use serde_json::{json, Value};
use wenlan_core::db::{CodeBasisVerdict, MemoryDB};
use wenlan_core::read_scope::ReadScope;

#[path = "team_member_view_round5_test.rs"]
mod round5;

const REPO: &str = "ROUND4_HOST_REPO";
const CODE_FILE: &str = "ROUND4_HOST_CODE.rs";
const EXPLANATION: &str = "ROUND4_LOCAL_CODE_CHANGE";
const TOOL: &str = "ROUND4_HOST_TOOL";
const MARKER: &str = "[872]";
const BEFORE: &str = "ROUND4_BEFORE_SPACE. A second private sentence.";
const SAVED_RULE: &str = "ROUND4_PERSONAL_RULE";
const PRIVATE_DRAFT: &str = "ROUND4_OTHER_MEMBERS_DRAFT";
const DEVICE: &str = "ROUND4_HOST_DEVICE";
const APPROVED: &str = "Record the public review outcome.";
const PROPOSED: &str = "Review one shared change at a time.";

fn set_daemon(f: &mut Fixture, db: &Arc<MemoryDB>, root: &std::path::Path) {
    let state = crate::state::ServerState {
        db: Some(db.clone()),
        presence_root: Some(f._dir.path().into()),
        ..Default::default()
    }
    .with_page_root(root.to_owned());
    f.state.in_process_daemon = Some(crate::router::build_router(Arc::new(
        tokio::sync::RwLock::new(state),
    )));
}

async fn forwarded(f: &Fixture, method: Method, path: &str, data: Value) -> (StatusCode, String) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json")
        .header(crate::space_header::HEADER_NAME, "Design")
        .header("x-agent-name", "fixture@team.reader")
        .header(
            crate::team_actor::SECRET_HEADER,
            crate::team_actor::secret(),
        )
        .body(Body::from(data.to_string()))
        .unwrap();
    let response = f
        .state
        .in_process_daemon
        .as_ref()
        .unwrap()
        .clone()
        .oneshot(request)
        .await
        .unwrap();
    let status = response.status();
    (status, response_text(response).await)
}

async fn seed_code_and_private_citation(f: &mut Fixture, db: &Arc<MemoryDB>) -> (String, String) {
    let root = f._dir.path().join("wiki");
    std::fs::create_dir_all(root.join("Design")).unwrap();
    wenlan_core::space_folders::turn_on(&root).unwrap();
    set_daemon(f, db, &root);
    let page = create_folder_test_page(db, "Round4 Storage Fixture", "authored").await;
    let id = page.id;
    db.update_page_content(
        &id,
        &format!("Synthetic public prose. {MARKER}"),
        &["private-fixture"],
        "agent_refresh",
    )
    .await
    .unwrap();
    let page = db.get_page(&id).await.unwrap().unwrap();
    let changelog = json!([{
        "version": page.version,
        "at": chrono::Utc::now().timestamp(),
        "edited_by": "agent_refresh",
        "agent": TOOL,
    }]);
    let citation = json!([{
        "occurrence": 1, "marker": 872, "source_kind": "memory",
        "locator": "private-fixture", "score": 1.0, "status": "verified",
    }]);
    db.set_page_citations_with_changelog(&id, Some(&citation.to_string()), &changelog.to_string())
        .await
        .unwrap();
    let repo = f._dir.path().join(REPO);
    std::fs::create_dir(&repo).unwrap();
    std::fs::write(repo.join(CODE_FILE), "fn sample() {}\n").unwrap();
    let span = wenlan_core::code_anchor::LineSpan { start: 1, end: 1 };
    let anchor = wenlan_core::code_anchor::anchor("fn old() {}\n", span).unwrap();
    db.upsert_page_code_basis(
        &id,
        &[wenlan_core::code_basis::PreparedCodeBasis {
            input: wenlan_types::CodeBasisInput {
                repo_root: repo.to_string_lossy().into_owned(),
                path: CODE_FILE.into(),
                line_start: 1,
                line_end: 1,
            },
            anchor,
            stamp: None,
            cited_blob: None,
        }],
        TOOL,
    )
    .await
    .unwrap();
    let row = db
        .get_code_basis_row(
            &id,
            &wenlan_types::CodeBasisLocator {
                repo_root: repo.to_string_lossy().into_owned(),
                path: CODE_FILE.into(),
                line_start: 1,
                line_end: 1,
            },
        )
        .await
        .unwrap()
        .unwrap();
    db.apply_code_basis_verdicts(&[(&row, CodeBasisVerdict::Pending)], 1)
        .await
        .unwrap();
    assert!(db
        .record_code_basis_recheck(
            &row,
            &format!("code@v{}", page.version),
            Some(EXPLANATION),
            Some("needs_human"),
            0,
        )
        .await
        .unwrap());
    // Readable through the host's projection index under a distinct filename.
    let filename = wenlan_core::export::knowledge::KnowledgeProjectionWrite::new(root.clone(), db)
        .write_page_gated(db, &db.get_page(&id).await.unwrap().unwrap())
        .await
        .unwrap()
        .expect("fixture must be in the live projection");
    assert!(root.join(&filename).is_file());
    let basename = std::path::Path::new(&filename).file_name().unwrap();
    (id, basename.to_str().unwrap().into())
}

#[tokio::test]
async fn round4_page_responses_omit_local_code_projection_and_citation_markers() {
    let (mut f, db) = fixture_with_daemon().await;
    let (id, filename) = seed_code_and_private_citation(&mut f, &db).await;
    let mut exposed = Vec::new();
    for path in ["/api/pages".into(), format!("/api/pages/{id}")] {
        let (status, body) = forwarded(&f, Method::GET, &path, Value::Null).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
        assert!(body.contains("Synthetic public prose."));
        for canary in [REPO, CODE_FILE, &filename, EXPLANATION, MARKER, TOOL] {
            if body.contains(canary) {
                exposed.push(format!("{path}: {canary}"));
            }
        }
    }
    assert!(exposed.is_empty(), "member response contained {exposed:?}");
}

async fn seed_moved_page(db: &MemoryDB) -> String {
    db.create_space("Personal", None, false).await.unwrap();
    let draft = db
        .create_page_draft("Migrated page", BEFORE, Some("Personal"), None)
        .await
        .unwrap();
    db.publish_page_draft(&draft.id, draft.version)
        .await
        .unwrap();
    db.update_page_content(&draft.id, "Team prose at the move.", &[], "manual_edit")
        .await
        .unwrap();
    let page = db.get_page(&draft.id).await.unwrap().unwrap();
    db.set_page_space(&page.id, Some("Design"), page.version, &ReadScope::Global)
        .await
        .unwrap();
    let page = db.get_page(&page.id).await.unwrap().unwrap();
    page.id
}

async fn seed_host_refresh(db: &MemoryDB, id: &str) {
    db.update_page_content(id, "Team prose after the move.", &[], "agent_refresh")
        .await
        .unwrap();
    let page = db.get_page(id).await.unwrap().unwrap();
    db.set_page_citations_with_changelog(
        id,
        None,
        &json!([{"version": page.version, "at": 123, "edited_by": "agent_refresh", "agent": TOOL}])
            .to_string(),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn round4_history_recent_changes_and_snapshots_use_shared_attribution_and_diffs() {
    let (f, db) = fixture_with_daemon().await;
    let id = seed_moved_page(&db).await;
    let (status, body) = forwarded(&f, Method::GET, "/api/pages/recent-changes", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!body.contains("changed_sentences"), "{body}");
    seed_host_refresh(&db, &id).await;
    let mut exposed = Vec::new();
    for path in [
        format!("/api/pages/{id}/revisions?include=text"),
        "/api/pages/recent-changes".into(),
    ] {
        let (status, body) = forwarded(&f, Method::GET, &path, Value::Null).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
        for canary in [TOOL, "ROUND4_BEFORE_SPACE"] {
            if body.contains(canary) {
                exposed.push(format!("{path}: {canary}"));
            }
        }
    }
    sync_hosted_test_folder(&f, &db).await;
    let files = listed_files(&f).await;
    let path = files
        .iter()
        .find(|path| path.contains(&id))
        .expect("published page version is listed");
    let (status, bytes) = download(&f, path).await;
    assert_eq!(status, StatusCode::OK);
    let body = String::from_utf8(bytes).unwrap();
    for canary in [TOOL, "ROUND4_BEFORE_SPACE"] {
        if body.contains(canary) {
            exposed.push(format!("snapshot: {canary}"));
        }
    }
    assert!(exposed.is_empty(), "member response contained {exposed:?}");
}

async fn seed_saved_rules(db: &MemoryDB) -> (String, i64) {
    let draft = db
        .create_page_draft("Rules I saved", SAVED_RULE, Some("Design"), None)
        .await
        .unwrap();
    db.publish_page_draft(&draft.id, draft.version)
        .await
        .unwrap();
    let page = db.get_page(&draft.id).await.unwrap().unwrap();
    let space = db.get_space("Design").await.unwrap().unwrap();
    db.edit_saved_rules_page(&space.id, |entry| entry.page_id = page.id.clone())
        .await
        .unwrap();
    (page.id, page.version)
}

#[tokio::test]
async fn round4_hidden_saved_rules_cannot_be_renamed() {
    let (f, db) = fixture_with_daemon().await;
    let (id, version) = seed_saved_rules(&db).await;
    let (status, body) = forwarded(
        &f,
        Method::POST,
        &format!("/api/pages/{id}/rename"),
        json!({"title": "Renamed fixture page", "expected_version": version}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(db.get_page(&id).await.unwrap().unwrap().version, version);
}

async fn listed_files(f: &Fixture) -> Vec<String> {
    let (status, body) = call(
        &f.state,
        Method::GET,
        "/team/v1/spaces/Design/files",
        Some(&f.reader.1),
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    serde_json::from_slice::<TeamFileList>(&body).unwrap().files
}

async fn download(f: &Fixture, path: &str) -> (StatusCode, Vec<u8>) {
    call(
        &f.state,
        Method::GET,
        &format!("/team/v1/spaces/Design/file?path={path}"),
        Some(&f.reader.1),
        vec![],
    )
    .await
}

#[tokio::test]
async fn round4_unchanged_survivor_gets_a_snapshot_after_its_basis_is_deleted() {
    let (f, db) = fixture_with_daemon().await;
    let basis = create_folder_test_page(&db, "ROUND4_DELETED_BASIS", "authored").await;
    let survivor = create_folder_test_page(&db, "Surviving team note", "authored").await;
    let version = db.get_page(&basis.id).await.unwrap().unwrap().version;
    db.mark_page_basis(&survivor.id, &basis.id, version, "fixture")
        .await
        .unwrap();
    sync_hosted_test_folder(&f, &db).await;
    let before: Vec<_> = listed_files(&f)
        .await
        .into_iter()
        .filter(|path| path.contains(&survivor.id))
        .collect();
    assert_eq!(before.len(), 1);
    let (status, body) = download(&f, &before[0]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(String::from_utf8(body).unwrap().contains(&basis.id));
    db.delete_page(&basis.id).await.unwrap();
    sync_hosted_test_folder(&f, &db).await;
    let replacement: Vec<_> = listed_files(&f)
        .await
        .into_iter()
        .filter(|path| path.contains(&survivor.id))
        .collect();
    assert_eq!(
        replacement.len(),
        1,
        "survivor must still be available to fresh syncs"
    );
    assert_ne!(replacement[0], before[0]);
    let (status, body) = download(&f, &replacement[0]).await;
    let body = String::from_utf8(body).unwrap();
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Synthetic meeting notes."));
    assert!(!body.contains(&basis.id) && !body.contains("ROUND4_DELETED_BASIS"));
}

fn member_endpoint_input(route: &str, method: &str, version: i64) -> Value {
    match (method, route) {
        ("GET", _) => Value::Null,
        ("POST", "/api/pages/search" | "/api/memory/search") => {
            json!({"query": "Synthetic public"})
        }
        ("POST", "/api/brief") => json!({"topic": "Synthetic public"}),
        ("POST", "/api/skills/record") => {
            json!({"space": "Design", "rule_id": "missing", "report": "changed_plan"})
        }
        ("POST", "/api/pages") => {
            json!({"space": "Design", "title": "Canary control page", "content": "Public control."})
        }
        ("POST", "/api/pages/drafts") => {
            json!({"space": "Design", "draft_id": "page_00000000-0000-4000-8000-000000004002", "title": "Canary control draft", "content": "Public control."})
        }
        ("POST", "/api/knowledge/folders") => {
            json!({"space": "Design", "parent_path": "Design", "name": "Canary control folder"})
        }
        ("POST", "/api/pages/{id}/rename") => {
            json!({"expected_version": version, "title": "Updated title"})
        }
        ("POST", "/api/pages/{id}/move") => {
            json!({"expected_storage_path": "Design/unused.md", "folder_path": "Design", "operation_id": "canary-move"})
        }
        ("POST", "/api/pages/{id}/archive") | ("DELETE", _) => Value::Null,
        ("POST", "/api/pages/{id}/restore" | "/api/pages/drafts/{id}/publish") => {
            json!({"expected_version": version})
        }
        ("POST", "/api/memory/{id}/update-page")
        | ("PUT", "/api/pages/{id}" | "/api/pages/drafts/{id}") => {
            json!({"expected_version": version, "content": "Member write."})
        }
        ("PUT", "/api/pages/{id}/basis/{basis_id}") => json!({"version": version}),
        _ => panic!("supply a member route probe for {method} {route}"),
    }
}

async fn hosted_rpc(
    router: &axum::Router,
    key: &str,
    session: Option<&str>,
    payload: Value,
) -> Response {
    let mut builder = Request::post("/mcp/Design")
        .header(header::HOST, "team.example.com")
        .header(header::AUTHORIZATION, format!("Bearer {key}"))
        .header(header::ACCEPT, "application/json, text/event-stream")
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(session) = session {
        builder = builder
            .header("mcp-session-id", session)
            .header("mcp-protocol-version", "2025-06-18");
    }
    router
        .clone()
        .oneshot(builder.body(Body::from(payload.to_string())).unwrap())
        .await
        .unwrap()
}

fn rpc_reply(body: &str) -> Value {
    let data = body
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .find(|data| !data.trim().is_empty())
        .unwrap_or(body);
    serde_json::from_str(data).unwrap()
}

#[tokio::test]
async fn round4_canary_covers_member_http_and_hosted_mcp_catalogs() {
    let (mut f, db) = fixture_with_daemon().await;
    let (page, filename) = seed_code_and_private_citation(&mut f, &db).await;
    let root = f._dir.path().join("wiki");
    let moved = seed_moved_page(&db).await;
    seed_host_refresh(&db, &moved).await;
    let (hidden, hid_version) = seed_saved_rules(&db).await;
    team::set_url(&f.state.team_path, Some(ORIGIN)).unwrap();
    team::owner_key(&f.state.team_path, "Team owner").unwrap();
    std::fs::write(
        f.state.settings_path.with_file_name("repo-wiki.json"),
        json!({
            "bindings": [{ "space": "Design", "repo_root": f._dir.path().join(REPO),
                "folder": "wiki", "remote_url": "https://github.com/fixture/team-design.git" }]
        })
        .to_string(),
    )
    .unwrap();
    let initial =
        rulesync_team::update_hosted_rules(&f.state.settings_path, "Design", APPROVED).unwrap();
    rulesync_team::decide_local_space_proposal(&f.state.settings_path, "Design", &initial, true)
        .unwrap();
    let proposal =
        rulesync_team::update_hosted_rules(&f.state.settings_path, "Design", PROPOSED).unwrap();
    let reviewer = f.state.sessions.create_fresh(&f.writer.0.id).unwrap();
    let draft = db
        .create_registered_page_draft(
            "page_00000000-0000-4000-8000-000000004004",
            PRIVATE_DRAFT,
            PRIVATE_DRAFT,
            Some("Design"),
            "",
            Some(&f.reader.0.id),
        )
        .await
        .unwrap();
    let metadata = f.folder.join("space.json");
    let mut space_file: Value = serde_json::from_slice(&std::fs::read(&metadata).unwrap()).unwrap();
    space_file["devices"] = json!([{"id": DEVICE, "name": DEVICE}]);
    space_file["main_device"] = json!(DEVICE);
    std::fs::write(metadata, space_file.to_string()).unwrap();
    sync_hosted_test_folder(&f, &db).await;
    let mut legacy = wenlan_core::store::format::page::PageFile::new(
        "Round4 Storage Fixture",
        "Synthetic public prose.",
    );
    legacy.id = Some(page.clone());
    legacy.other.insert(
        "repo_root".into(),
        f._dir.path().join(REPO).display().to_string().into(),
    );
    let bytes = render_test_version(&legacy, "server");
    let (sha, _) = version::write_version(&f.folder, "server", &page, &bytes).unwrap();
    let legacy_path = format!(
        "members/server/pages/{page}/{}",
        version::version_file_name(&sha)
    );

    let canaries = [
        REPO,
        CODE_FILE,
        &filename,
        root.to_str().unwrap(),
        f._dir.path().to_str().unwrap(),
        EXPLANATION,
        TOOL,
        MARKER,
        "ROUND4_BEFORE_SPACE",
        SAVED_RULE,
        PRIVATE_DRAFT,
        DEVICE,
    ];
    let mut failures = Vec::new();
    let mut scan = |label: &str, body: &str, correct: bool| {
        if !correct {
            failures.push(format!("{label}: unexpected response {body}"));
        }
        for canary in canaries {
            if body.contains(canary) {
                failures.push(format!("{label}: exposed {canary}"));
            }
        }
    };
    let mut app_count = 0;
    for entry in app_gate::CALLS {
        let id_route = entry.path.contains("{id}");
        let public = entry.need == app_gate::Need::Read;
        for target in if public && id_route {
            vec![
                page.as_str(),
                moved.as_str(),
                hidden.as_str(),
                draft.id.as_str(),
            ]
        } else if entry.path.contains("/drafts/{id}") {
            vec![hidden.as_str(), draft.id.as_str()]
        } else if entry.space == app_gate::SpaceFrom::PagePair {
            vec![hidden.as_str(), page.as_str()]
        } else {
            vec![hidden.as_str()]
        } {
            let mut path = format!(
                "{}{}",
                app_gate::PREFIX,
                entry
                    .path
                    .replace("{id}", target)
                    .replace("{basis_id}", if target == page { &hidden } else { &page })
            );
            if entry.path.ends_with("/revisions") {
                path.push_str("?include=text");
            }
            let input = member_endpoint_input(
                entry.path,
                entry.method,
                if target == draft.id {
                    draft.version
                } else {
                    hid_version
                },
            );
            let method = Method::from_bytes(entry.method.as_bytes()).unwrap();
            let (status, body) = call(
                &f.state,
                method,
                &path,
                Some(&f.writer.1),
                if input.is_null() {
                    vec![]
                } else {
                    input.to_string().into()
                },
            )
            .await;
            let body = std::str::from_utf8(&body).unwrap();
            let denied = id_route
                && (target == hidden
                    || target == draft.id
                    || entry.space == app_gate::SpaceFrom::PagePair);
            let positive = entry.path != "/api/pages/{id}"
                || target != page
                || body.contains("Synthetic public prose.");
            scan(
                &format!("HTTP {} {path} {status}", entry.method),
                body,
                if denied {
                    status == StatusCode::NOT_FOUND
                } else {
                    status.is_success() && positive
                },
            );
        }
        app_count += 1;
    }

    // Exercise the extra daemon routes used by hosted tools with the same
    // trusted forwarder identity, including operations which share a tool.
    let mut forward_count = 0;
    for &(method, path) in crate::team_actor::MCP_CALLS {
        let denied = path.contains("{id}");
        let input = member_endpoint_input(path, method, hid_version);
        let path = path.replace("{id}", &hidden);
        let (status, body) = forwarded(
            &f,
            Method::from_bytes(method.as_bytes()).unwrap(),
            &path,
            input,
        )
        .await;
        let correct = if denied {
            status == StatusCode::NOT_FOUND
        } else if path == "/api/skills/record" {
            status == StatusCode::NOT_FOUND || body.contains("\"recorded\":false")
        } else if path == "/api/skills/rules" {
            status == StatusCode::OK && body.contains(APPROVED)
        } else {
            status.is_success()
        };
        scan(
            &format!("forwarded {method} {path} {status}"),
            &body,
            correct,
        );
        forward_count += 1;
    }

    let uploaded = version_text(&hidden, &f.writer.0.id, "Attempted update to hidden page.");
    let name = version::version_file_name(&version::sha256_hex(uploaded.as_bytes()));
    let mut data_count = 0;
    for (_, template) in member_data_route_catalog() {
        let uri = template
            .replace("{space}", "Design")
            .replace("{page}", &hidden)
            .replace("{name}", &name)
            .replace("{proposal}", &proposal);
        match template {
            "/team/v1/spaces/{space}/file" => {
                let files = listed_files(&f).await;
                assert!(
                    files.iter().any(|path| path.contains(&page)),
                    "public page missing from sync"
                );
                assert!(
                    files.iter().all(|path| !path.contains(&hidden)),
                    "hidden page listed"
                );
                for path in files {
                    let (status, body) = download(&f, &path).await;
                    scan(
                        &format!("download {path} {status}"),
                        std::str::from_utf8(&body).unwrap(),
                        status == StatusCode::OK,
                    );
                }
                let (status, bytes) = download(&f, &legacy_path).await;
                scan(
                    &format!("direct legacy download {status}"),
                    std::str::from_utf8(&bytes).unwrap(),
                    status == StatusCode::NOT_FOUND,
                );
            }
            "/team/v1/me" | "/team/v1/spaces/{space}/files" => {
                let (status, body) =
                    call(&f.state, Method::GET, &uri, Some(&f.writer.1), vec![]).await;
                scan(
                    &format!("GET {uri} {status}"),
                    std::str::from_utf8(&body).unwrap(),
                    status == StatusCode::OK,
                );
            }
            "/team/v1/spaces/{space}/rules/proposals" => {
                let response =
                    browser(&f.state, Method::GET, &uri, Some(&reviewer), None, None).await;
                let status = response.status();
                let body = response_text(response).await;
                scan(
                    &format!("GET {uri} {status}"),
                    &body,
                    status == StatusCode::OK && body.contains(PROPOSED),
                );
            }
            "/team/v1/spaces/{space}/versions/{page}/{name}" => {
                let (status, body) = call(
                    &f.state,
                    Method::PUT,
                    &uri,
                    Some(&f.writer.1),
                    uploaded.as_bytes().to_vec(),
                )
                .await;
                scan(
                    &format!("PUT {uri} {status}"),
                    std::str::from_utf8(&body).unwrap(),
                    status == StatusCode::NOT_FOUND,
                );
            }
            "/team/v1/spaces/{space}/rules/proposals/{proposal}/decision" => {
                let request = Request::post(&uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, format!("__Host-wenlan-session={reviewer}"))
                    .header(header::ORIGIN, ORIGIN)
                    .body(Body::from(r#"{"accept":false}"#))
                    .unwrap();
                let response = team_router(f.state.clone()).oneshot(request).await.unwrap();
                let status = response.status();
                scan(
                    &format!("POST {uri} {status}"),
                    &response_text(response).await,
                    status == StatusCode::NO_CONTENT,
                );
            }
            _ => panic!("supply a member data route probe for {template}"),
        }
        data_count += 1;
    }

    let router = team_router(f.state.clone());
    let mut tool_count = 0;
    for key in [&f.reader.1, &f.writer.1] {
        let response = hosted_rpc(&router, key, None, json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "canary-fixture", "version": "1"}
        }})).await;
        assert_eq!(response.status(), StatusCode::OK);
        let session = response.headers()["mcp-session-id"]
            .to_str()
            .unwrap()
            .to_owned();
        scan("MCP initialize", &response_text(response).await, true);
        let response = hosted_rpc(
            &router,
            key,
            Some(&session),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let response = hosted_rpc(
            &router,
            key,
            Some(&session),
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_text(response).await;
        scan("MCP tools/list", &body, true);
        let reply = rpc_reply(&body);
        for (index, tool) in reply["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
        {
            let name = tool["name"].as_str().unwrap();
            let probe = |arguments, denied, public_text| (arguments, denied, public_text);
            let probes = match name {
                "recall" => vec![probe(json!({"query": "Synthetic public"}), false, true)],
                "brief" => vec![probe(json!({"topic": "Synthetic public"}), false, false)],
                "get_page_sources" | "read_page" => vec![
                    probe(json!({"page_id": page}), false, name == "read_page"),
                    probe(json!({"page_id": hidden}), true, false),
                ],
                "list_pages" => vec![probe(json!({}), false, true)],
                "read_space_rules" => vec![probe(json!({}), false, true)],
                "report_space_rule_use" => vec![probe(
                    json!({"rule_id": "missing", "outcome": "changed_plan"}),
                    false,
                    false,
                )],
                "write_page" => vec![
                    probe(
                        json!({"title": "Created through MCP", "content": "Public control."}),
                        false,
                        false,
                    ),
                    probe(
                        json!({"page_id": hidden, "content": "Member write.", "expected_version": hid_version}),
                        true,
                        false,
                    ),
                ],
                "rename_page" => vec![probe(
                    json!({"page_id": hidden, "title": "Updated title", "expected_version": hid_version}),
                    true,
                    false,
                )],
                "delete_page" => vec![probe(json!({"page_id": hidden}), true, false)],
                _ => panic!("supply a hosted tool probe for {name}"),
            };
            for (arguments, denied, public_text) in probes {
                let response = hosted_rpc(&router, key, Some(&session), json!({"jsonrpc": "2.0", "id": index + 3, "method": "tools/call", "params": {"name": name, "arguments": arguments}})).await;
                let status = response.status();
                let body = response_text(response).await;
                let correct = if status == StatusCode::OK {
                    let result = &rpc_reply(&body)["result"];
                    let expected = match name {
                        "list_pages" => "Round4 Storage Fixture",
                        "read_space_rules" => APPROVED,
                        _ => "Synthetic public prose.",
                    };
                    result.is_object()
                        && (!public_text || body.contains(expected))
                        && (name == "report_space_rule_use"
                            || result["isError"].as_bool().unwrap_or(false) == denied)
                } else {
                    false
                };
                scan(&format!("MCP {name} {status}"), &body, correct);
            }
            tool_count += 1;
        }
    }
    let after = db.get_page(&hidden).await.unwrap().unwrap();
    assert_eq!(
        (
            after.title.as_str(),
            after.content.as_str(),
            after.version,
            after.status.as_str()
        ),
        ("Rules I saved", SAVED_RULE, hid_version, "active")
    );
    assert_eq!(
        db.get_page(&draft.id).await.unwrap().unwrap().version,
        draft.version
    );
    eprintln!(
        "canary exercised {app_count} browser routes, {data_count} team data routes, {forward_count} additional forwarded routes, {tool_count} MCP tool/grant pairs"
    );
    assert!(
        failures.is_empty(),
        "member canary failures:\n{}",
        failures.join("\n")
    );
}
