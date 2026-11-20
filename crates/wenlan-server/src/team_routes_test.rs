// SPDX-License-Identifier: Apache-2.0
//! The team listener on its own, with a member list and one hosted Space in
//! a scratch folder: what a key opens, and what it never does.

use super::*;
use axum::body::to_bytes;
use axum::http::Method;
use wenlan_core::shared_space::{version, SpaceMember};
use wenlan_types::team::TeamSpaceAccess;

struct Fixture {
    _dir: tempfile::TempDir,
    state: TeamState,
    folder: PathBuf,
    writer: (Member, String),
    reader: (Member, String),
    outsider: (Member, String),
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let team_path = dir.path().join(team::TEAM_FILE);
    let settings_path = dir.path().join(shared_space::SETTINGS_FILE);
    let folder = dir.path().join("team-spaces").join("design");
    let scope = Scope::Space("Design".into());
    shared_space::ensure_space_file(
        &folder,
        &scope,
        "Design",
        &SpaceMember {
            id: "server".into(),
            name: "Server".into(),
        },
    )
    .unwrap();
    shared_space::join_at(&settings_path, &scope, &folder, "Server").unwrap();
    shared_space::update_share(&settings_path, &scope, |share| share.hosted = true).unwrap();
    let access = |access| {
        vec![TeamSpaceAccess {
            space: "Design".into(),
            access,
            id: None,
        }]
    };
    let writer = team::add_member(&team_path, "Amy", &access(TeamAccess::Write)).unwrap();
    let reader = team::add_member(&team_path, "Bo", &access(TeamAccess::Read)).unwrap();
    let outsider = team::add_member(
        &team_path,
        "Cy",
        &[TeamSpaceAccess {
            space: "Other".into(),
            access: TeamAccess::Write,
            id: None,
        }],
    )
    .unwrap();
    Fixture {
        state: TeamState {
            team_path,
            settings_path,
            db: None,
            daemon_url: "http://127.0.0.1:9".into(),
            in_process_daemon: None,
            mcp: Arc::default(),
            sessions: Arc::default(),
            auth: Arc::default(),
        },
        _dir: dir,
        folder,
        writer,
        reader,
        outsider,
    }
}

async fn call(
    state: &TeamState,
    method: Method,
    uri: &str,
    key: Option<&str>,
    body: Vec<u8>,
) -> (StatusCode, Vec<u8>) {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(key) = key {
        req = req.header(header::AUTHORIZATION, format!("Bearer {key}"));
    }
    let response = team_router(state.clone())
        .oneshot(req.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 16 * 1024 * 1024)
        .await
        .unwrap();
    (status, bytes.to_vec())
}

async fn join_page_response(state: &TeamState, language: Option<&str>) -> Response {
    let mut request = Request::builder().method(Method::GET).uri("/join");
    if let Some(language) = language {
        request = request.header(header::ACCEPT_LANGUAGE, language);
    }
    team_router(state.clone())
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn response_text(response: Response) -> String {
    let bytes = to_bytes(response.into_body(), 16 * 1024 * 1024)
        .await
        .unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

async fn fixture_with_daemon() -> (Fixture, Arc<wenlan_core::db::MemoryDB>) {
    let mut fixture = fixture();
    let db = Arc::new(
        wenlan_core::db::MemoryDB::new(
            &fixture._dir.path().join("daemon"),
            Arc::new(wenlan_core::events::NoopEmitter),
        )
        .await
        .unwrap(),
    );
    db.create_space("Design", None, false).await.unwrap();
    fixture.state.db = Some(db.clone());
    let state = crate::state::ServerState {
        db: Some(db.clone()),
        ..Default::default()
    };
    fixture.state.in_process_daemon = Some(crate::router::build_router(Arc::new(
        tokio::sync::RwLock::new(state),
    )));
    (fixture, db)
}

async fn create_folder_test_page(
    db: &wenlan_core::db::MemoryDB,
    title: &str,
    kind: &str,
) -> wenlan_core::post_write::WriteResult {
    wenlan_core::post_write::create_page_with_floor(
        db,
        wenlan_types::requests::CreateConceptRequest {
            title: title.into(),
            content: "Synthetic meeting notes.".into(),
            summary: None,
            entity_id: None,
            space: Some("Design".into()).into(),
            source_memory_ids: Vec::new(),
            creation_kind: Some(kind.into()),
            workspace: Some("Design".into()),
        },
        "fixture",
        None,
        1,
    )
    .await
    .unwrap()
}

async fn sync_hosted_test_folder(
    f: &Fixture,
    db: &wenlan_core::db::MemoryDB,
) -> wenlan_types::shared_space::SharedSyncCounts {
    shared_space::sync_space(
        db,
        shared_space::SyncTarget {
            scope: &Scope::Space("Design".into()),
            folder: &f.folder,
            member_name: "Server",
            settings_path: &f.state.settings_path,
            knowledge_path: None,
            member_id: None,
            host: None,
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn authenticated_team_reader_sees_shared_page_without_seeded_host_memory() {
    const PRIVATE_ID: &str = "fixture-authenticated-evidence";
    const PRIVATE_TEXT: &str = "fictional amber envelope for the design";
    const SHARED: &str = "The design meeting starts on Tuesday. [1]";
    let (f, db) = fixture_with_daemon().await;
    db.upsert_documents(vec![wenlan_core::sources::RawDocument {
        source: "memory".into(),
        source_id: PRIVATE_ID.into(),
        title: "Fixture reminder".into(),
        content: PRIVATE_TEXT.into(),
        space: Some("Design".into()),
        confirmed: Some(true),
        ..Default::default()
    }])
    .await
    .unwrap();
    let page = wenlan_core::post_write::create_page_with_floor(
        &db,
        wenlan_types::requests::CreateConceptRequest {
            title: "Fixture page".into(),
            content: SHARED.into(),
            summary: None,
            entity_id: None,
            space: Some("Design".to_string()).into(),
            source_memory_ids: vec![PRIVATE_ID.into()],
            creation_kind: Some("distilled".into()),
            workspace: Some("Design".into()),
        },
        "fixture",
        None,
        1,
    )
    .await
    .unwrap();
    let citations = serde_json::json!([{
        "occurrence": 1, "marker": 1, "source_kind": "memory",
        "locator": PRIVATE_ID, "score": 1.0, "status": "verified",
    }]);
    db.set_page_citations(&page.id, Some(&citations.to_string()))
        .await
        .unwrap();

    let source_path = format!("/api/pages/{}/sources", page.id);
    let host = f
        .state
        .in_process_daemon
        .as_ref()
        .unwrap()
        .clone()
        .oneshot(
            Request::builder()
                .uri(&source_path)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(host.status(), StatusCode::OK);
    let host = response_text(host).await;
    assert!(host.contains(PRIVATE_ID) && host.contains(PRIVATE_TEXT));

    let page_path = format!("/team/v1/app/api/pages/{}", page.id);
    let (status, body) = call(&f.state, Method::GET, &page_path, Some(&f.reader.1), vec![]).await;
    assert_eq!(status, StatusCode::OK);
    let body = String::from_utf8(body).unwrap();
    assert!(body.contains(SHARED));
    assert!(!body.contains(PRIVATE_ID) && !body.contains(PRIVATE_TEXT));
    let sources = format!("/team/v1/app{source_path}");
    let (status, body) = call(&f.state, Method::GET, &sources, Some(&f.reader.1), vec![]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
        serde_json::json!([])
    );
    for (member, expected) in [
        (None, StatusCode::UNAUTHORIZED),
        (Some(f.outsider.1.as_str()), StatusCode::NOT_FOUND),
    ] {
        let (status, _) = call(&f.state, Method::GET, &sources, member, vec![]).await;
        assert_eq!(status, expected);
    }
}

#[tokio::test]
async fn authenticated_team_writer_can_save_and_publish_a_draft_kept_off_read_routes() {
    use serde_json::{json, Value};

    let (f, _) = fixture_with_daemon().await;
    let (status, body) = call(
        &f.state,
        Method::POST,
        "/team/v1/app/api/pages/drafts",
        Some(&f.writer.1),
        serde_json::to_vec(&json!({
            "draft_id": "page_00000000-0000-4000-8000-00000000e241",
            "title": "Draft schedule",
            "content": "First draft.",
        }))
        .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "draft creation");
    let draft: Value = serde_json::from_slice(&body).unwrap();
    let id = draft["page"]["id"].as_str().unwrap();
    let page_path = format!("/team/v1/app/api/pages/{id}");
    let (status, _) = call(&f.state, Method::GET, &page_path, Some(&f.reader.1), vec![]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let draft_path = format!("/team/v1/app/api/pages/drafts/{id}");
    let (status, body) = call(
        &f.state,
        Method::PUT,
        &draft_path,
        Some(&f.writer.1),
        serde_json::to_vec(&json!({
            "title": "Team schedule",
            "content": "Published schedule.",
            "expected_version": draft["page"]["version"],
        }))
        .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "draft save");
    let saved: Value = serde_json::from_slice(&body).unwrap();
    let publish =
        serde_json::to_vec(&json!({"expected_version": saved["page"]["version"]})).unwrap();
    let publish_path = format!("{draft_path}/publish");
    for _ in 0..2 {
        let (status, body) = call(
            &f.state,
            Method::POST,
            &publish_path,
            Some(&f.writer.1),
            publish.clone(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "draft publication");
        let published: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(published["page"]["status"], "active");
        assert_eq!(
            published["page"]["version"].as_i64(),
            saved["page"]["version"].as_i64().map(|v| v + 1)
        );
    }
    let (status, body) = call(&f.state, Method::GET, &page_path, Some(&f.reader.1), vec![]).await;
    assert_eq!(status, StatusCode::OK);
    let published: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(published["page"]["content"], "Published schedule.");
}

fn version_text(page: &str, member: &str, body: &str) -> String {
    let mut file = wenlan_core::store::format::page::PageFile::new("Plan", body);
    file.id = Some(page.into());
    render_test_version(&file, member)
}

fn render_test_version(file: &wenlan_core::store::format::page::PageFile, member: &str) -> String {
    version::render(
        file,
        &version::VersionMeta {
            parent: None,
            merged: Vec::new(),
            body_digest: version::body_digest(&file.body),
            author: version::Author {
                member: member.into(),
                kind: version::AuthorKind::Person,
                agent: None,
            },
            ts: "2026-11-11T05:00:00Z".into(),
            archived: false,
            locked: false,
            proposal: false,
            decides: None,
            device: None,
        },
    )
}

#[tokio::test]
async fn only_health_and_the_join_page_answer_without_a_member_key() {
    let f = fixture();
    let (status, _) = call(&f.state, Method::GET, "/health", None, vec![]).await;
    assert_eq!(status, StatusCode::OK);
    // An invite link opened in a browser: the team's name and what to do,
    // never anything else of the team.
    team::rename_team(&f.state.team_path, "Acme <b>").unwrap();
    let (status, body) = call(&f.state, Method::GET, "/join", None, vec![]).await;
    assert_eq!(status, StatusCode::OK);
    let page = String::from_utf8(body.to_vec()).unwrap();
    assert!(page.contains("Join Acme &lt;b&gt; on Wenlan"), "{page}");
    assert!(!page.contains("Amy") && !page.contains("Design"));
    for uri in [
        "/team/v1/me",
        "/team/v1/spaces/Design/files",
        "/mcp",
        // The daemon's own routes are not served here at all.
        "/api/memory/search",
        "/api/shutdown",
        // The bundle's index is served only at /app, never at the root or
        // under its own file name.
        "/",
        "/index.html",
        "/assets/index-deadbeef.js",
    ] {
        let (status, _) = call(&f.state, Method::GET, uri, None, vec![]).await;
        assert!(
            status == StatusCode::UNAUTHORIZED || status == StatusCode::NOT_FOUND,
            "{uri}: {status}"
        );
    }
    let (status, _) = call(
        &f.state,
        Method::GET,
        "/team/v1/me",
        Some(&team::new_key()),
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = call(
        &f.state,
        Method::POST,
        "/api/shutdown",
        Some(&f.writer.1),
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn team_app_is_the_static_bundle_with_no_key_and_nothing_of_the_team() {
    let f = fixture();
    team::rename_team(&f.state.team_path, "Acme").unwrap();
    for uri in ["/app", "/app/", "/app/anything"] {
        let response = team_router(f.state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        // 503 when this binary was built without the web bundle.
        assert!(
            status == StatusCode::OK || status == StatusCode::SERVICE_UNAVAILABLE,
            "{uri}: {status}"
        );
        let csp = response.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap()
            .to_string();
        assert!(csp.contains("default-src 'self'"), "{csp}");
        assert!(csp.contains("connect-src 'self'"), "{csp}");
        assert!(csp.contains("frame-ancestors 'none'"), "{csp}");
        assert!(
            csp.contains("script-src 'self';"),
            "no inline script: {csp}"
        );
        assert_eq!(response.headers()[header::REFERRER_POLICY], "no-referrer");
        let page = response_text(response).await;
        for private in ["Acme", "Amy", "Design", &f.writer.1] {
            assert!(!page.contains(private), "{uri} leaked {private}");
        }
    }
    let (status, _) = call(&f.state, Method::POST, "/app", None, vec![]).await;
    assert!(status == StatusCode::METHOD_NOT_ALLOWED || status == StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn team_join_page_accept_keeps_the_checked_key_and_opens_app() {
    let f = fixture();
    for (language, accept) in [
        ("en", ">Accept</button>"),
        ("zh-CN", ">接受</button>"),
        ("zh-TW", ">接受</button>"),
    ] {
        let page = response_text(join_page_response(&f.state, Some(language)).await).await;
        assert!(
            page.contains(&format!("<button id=\"accept\" type=\"button\"{accept}")),
            "{language}"
        );
        // Accept comes first; connecting an AI tool and the download stay below.
        let accept_at = page.find("id=\"accept\"").unwrap();
        assert!(accept_at < page.find("id=\"connect\"").unwrap());
        assert!(accept_at < page.find("https://wenlan.app").unwrap());
    }
    let page = response_text(join_page_response(&f.state, None).await).await;
    let script = &page[page.find("accept.addEventListener").unwrap()..];
    let check = script.find("fetch('/team/v1/me'").unwrap();
    let keep = script
        .find("localStorage.setItem('wenlan.team.key', key)")
        .unwrap();
    let open = script.find("window.location.assign('/app')").unwrap();
    assert!(
        check < keep && keep < open,
        "the key is kept only after it was checked"
    );
    assert!(script[..script.find("connect.addEventListener").unwrap()].contains("fail();"));
}

#[tokio::test]
async fn team_join_page_takes_an_invite_out_of_the_address_before_sending_it_in_a_body() {
    let f = fixture();
    let page = response_text(join_page_response(&f.state, None).await).await;
    let read = page.find("startsWith('inv_')").unwrap();
    let clear = page.find("window.history.replaceState(null, ''").unwrap();
    let send = page.find("fetch('/auth/invite/' + provider").unwrap();
    assert!(read < clear && clear < send, "cleared right after reading");
    let sent = &page[send..];
    let sent = &sent[..sent.find("window.location.assign(started.url)").unwrap()];
    assert!(
        sent.contains("method: 'POST'") && sent.contains("body: JSON.stringify({ token: invite })")
    );
    assert!(!sent.contains("localStorage"));
    for (language, label) in [
        ("en", "Sign in with Google"),
        ("zh-CN", "用 Google 登录"),
        ("zh-TW", "用 Google 登入"),
    ] {
        let page = response_text(join_page_response(&f.state, Some(language)).await).await;
        assert!(
            page.contains(&format!("\"sign_in_google\":\"{label}\"")),
            "{language}"
        );
    }
    // The key flow is still there, untouched.
    assert!(page.contains("localStorage.setItem('wenlan.team.key', key)"));
}

#[tokio::test]
async fn team_join_page_has_private_security_headers_and_no_member_key() {
    let f = fixture();
    let response = join_page_response(&f.state, None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::CACHE_CONTROL).unwrap(),
        "no-store"
    );
    assert_eq!(
        response.headers().get(header::REFERRER_POLICY).unwrap(),
        "no-referrer"
    );
    let csp = response
        .headers()
        .get(header::CONTENT_SECURITY_POLICY)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(csp.contains("default-src 'none'"), "{csp}");
    assert!(csp.contains("connect-src 'self'"), "{csp}");
    assert!(csp.contains("script-src 'nonce-"), "{csp}");
    assert!(csp.contains("style-src 'nonce-"), "{csp}");
    let page = response_text(response).await;
    assert!(page.contains("Don't want to install? Connect your AI tool"));
    assert!(page
        .contains("<p class=\"tool-question\" id=\"tool-question\">Which AI tool do you use?</p>"));
    for id in [
        "connect",
        "setup",
        "chooser",
        "spaces",
        "readonly",
        "error",
        "tool-claude",
        "tool-codex",
        "tool-cursor",
        "snippet",
        "copy",
    ] {
        assert!(page.contains(&format!("id=\"{id}\"")), "missing #{id}");
    }
    assert!(!page.contains("id=\"address\""));
    assert!(!page.contains("class=\"address\""));
    assert!(!page.contains("<strong>Address:</strong>"));
    assert!(page.contains("id=\"snippet\" hidden"));
    let snippet_start = page
        .find("<section class=\"snippet\" id=\"snippet\" hidden>")
        .unwrap();
    let snippet_end = snippet_start + page[snippet_start..].find("</section>").unwrap();
    let snippet_markup = &page[snippet_start..snippet_end];
    assert!(snippet_markup.contains(
        "<p class=\"readonly\" id=\"readonly\" hidden>Your AI tool can search this Space, but it cannot change pages.</p>"
    ));
    assert!(snippet_markup
        .contains("<p class=\"private\">Keep this private. It lets an AI tool act as you.</p>"));
    assert!(page.contains("chooser.hidden = spaces.length === 1;"));
    assert!(page.contains("button.classList.toggle('is-selected', selected)"));
    assert!(
        page.contains(".tool.is-selected { background:#5751c9; color:white; font-weight:600; }")
    );
    assert!(page.contains(".tools { display:flex; max-width:100%; gap:8px; }"));
    assert!(page.contains(".tool { flex:1 1 0; min-width:0; padding:8px; border-radius:8px; overflow:hidden; background:#e8e7e2;"));
    assert!(page
        .contains("font-size:.9rem; line-height:1.4; text-overflow:ellipsis; white-space:nowrap;"));
    assert!(page.contains(
        ".tool:focus-visible,.copy:focus-visible { outline:3px solid #5146b8; outline-offset:2px; }"
    ));
    assert!(page.contains(".tool { background:#303137; color:#ececf1; }"));
    assert!(page.contains(".tool:focus-visible,.copy:focus-visible { outline-color:#aaa5ff; }"));
    assert!(page.contains("snippet.hidden = true;\n      readonlyNode.hidden = true;"));
    for (id, tool) in [
        ("tool-claude", "claude"),
        ("tool-codex", "codex"),
        ("tool-cursor", "cursor"),
    ] {
        assert!(
            page.contains(&format!(
                "id=\"{id}\" type=\"button\" data-tool=\"{tool}\" aria-pressed=\"false\""
            )),
            "#{id} must start unchosen"
        );
    }
    assert_eq!(page.matches("id=\"copy\"").count(), 1);
    assert!(!page.contains(&f.writer.1), "member key leaked into /join");
    assert!(
        !page.contains("wlt_"),
        "a key-like value was emitted by /join"
    );
}

#[tokio::test]
async fn team_join_page_selects_english_simplified_and_traditional_chinese() {
    let f = fixture();
    let cases = [
        (
            "en-US,en;q=0.9",
            "lang=\"en\"",
            "<h1>Join a team on Wenlan</h1>",
            "Don't want to install? Connect your AI tool",
            "<p class=\"tool-question\" id=\"tool-question\">Which AI tool do you use?</p>",
        ),
        (
            "zh-CN",
            "lang=\"zh-Hans\"",
            "<h1>加入团队</h1>",
            "不想安装？直接连接你的 AI 工具",
            "<p class=\"tool-question\" id=\"tool-question\">你用哪个 AI 工具？</p>",
        ),
        (
            "zh-SG",
            "lang=\"zh-Hans\"",
            "<h1>加入团队</h1>",
            "不想安装？直接连接你的 AI 工具",
            "<p class=\"tool-question\" id=\"tool-question\">你用哪个 AI 工具？</p>",
        ),
        (
            "zh-TW",
            "lang=\"zh-Hant\"",
            "<h1>加入團隊</h1>",
            "不想安裝？直接連接你的 AI 工具",
            "<p class=\"tool-question\" id=\"tool-question\">你用哪個 AI 工具？</p>",
        ),
        (
            "zh-HK",
            "lang=\"zh-Hant\"",
            "<h1>加入團隊</h1>",
            "不想安裝？直接連接你的 AI 工具",
            "<p class=\"tool-question\" id=\"tool-question\">你用哪個 AI 工具？</p>",
        ),
        (
            "zh-MO",
            "lang=\"zh-Hant\"",
            "<h1>加入團隊</h1>",
            "不想安裝？直接連接你的 AI 工具",
            "<p class=\"tool-question\" id=\"tool-question\">你用哪個 AI 工具？</p>",
        ),
        (
            "zh-Hant",
            "lang=\"zh-Hant\"",
            "<h1>加入團隊</h1>",
            "不想安裝？直接連接你的 AI 工具",
            "<p class=\"tool-question\" id=\"tool-question\">你用哪個 AI 工具？</p>",
        ),
        (
            "en;q=0.2,zh-TW;q=0.9",
            "lang=\"zh-Hant\"",
            "<h1>加入團隊</h1>",
            "不想安裝？直接連接你的 AI 工具",
            "<p class=\"tool-question\" id=\"tool-question\">你用哪個 AI 工具？</p>",
        ),
    ];
    for (accept_language, lang, heading, button, tool_question) in cases {
        let page = response_text(join_page_response(&f.state, Some(accept_language)).await).await;
        assert!(page.contains(lang), "{accept_language}: {page}");
        assert!(page.contains(heading), "{accept_language}: {page}");
        assert!(page.contains(button), "{accept_language}: {page}");
        assert!(page.contains(tool_question), "{accept_language}: {page}");
    }
}

#[tokio::test]
async fn team_join_page_escapes_a_script_shaped_team_name() {
    let f = fixture();
    team::rename_team(
        &f.state.team_path,
        "Acme </script><script>alert(\"x\")</script> \"",
    )
    .unwrap();
    let page = response_text(join_page_response(&f.state, Some("en")).await).await;
    assert!(page
        .contains("Acme &lt;/script&gt;&lt;script&gt;alert(&quot;x&quot;)&lt;/script&gt; &quot;"));
    assert_eq!(page.matches("<script").count(), 1, "{page}");
    assert_eq!(page.matches("</script>").count(), 1, "{page}");
}

#[tokio::test]
async fn team_join_page_mcp_route_decodes_encoded_space_names() {
    async fn captured_space(Path(space): Path<String>) -> String {
        space
    }
    let response = axum::Router::new()
        .route("/mcp/{*space}", axum::routing::get(captured_space))
        .oneshot(
            Request::builder()
                .uri("/mcp/Roadmap%20%26%20Ideas")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_text(response).await, "Roadmap & Ideas");
}

#[tokio::test]
async fn bare_mcp_counts_a_space_open_to_everyone() {
    // The app's default invite names no Space: the member gets only what is
    // open to everyone. The invite page then shows the bare `/mcp` address,
    // so that address must open that one Space.
    let f = fixture();
    team::set_everyone(&f.state.team_path, "Design", Some(TeamAccess::Read)).unwrap();
    let (_, key) = team::add_member(&f.state.team_path, "Dee", &[]).unwrap();
    let (status, body) = call(&f.state, Method::GET, "/team/v1/me", Some(&key), vec![]).await;
    assert_eq!(status, StatusCode::OK);
    let me: wenlan_types::team::TeamMe = serde_json::from_slice(&body).unwrap();
    assert_eq!(me.spaces.len(), 1);
    let (status, body) = call(&f.state, Method::GET, "/mcp", Some(&key), vec![]).await;
    let text = String::from_utf8_lossy(&body).to_string();
    assert!(
        status != StatusCode::FORBIDDEN && !text.contains("has no Space"),
        "{status}: {text}"
    );
    // One of their own plus one open to everyone is two: the page shows a
    // chooser and `/mcp/<Space>`, and the bare address says so.
    let (status, body) = call(&f.state, Method::GET, "/mcp", Some(&f.outsider.1), vec![]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(String::from_utf8_lossy(&body).contains("more than one Space"));
}

#[tokio::test]
async fn hosted_mcp_lists_tools_for_the_authenticated_space_grant() {
    let f = fixture();
    let router = team_router(f.state);
    for (key, access) in [
        (&f.reader.1, TeamAccess::Read),
        (&f.writer.1, TeamAccess::Write),
    ] {
        let request = |body: serde_json::Value, session: Option<&str>| {
            let mut builder = Request::post("/mcp")
                .header(header::HOST, "team.example.com")
                .header(header::AUTHORIZATION, format!("Bearer {key}"))
                .header(header::ACCEPT, "application/json, text/event-stream")
                .header(header::CONTENT_TYPE, "application/json");
            if let Some(session) = session {
                builder = builder
                    .header("mcp-session-id", session)
                    .header("mcp-protocol-version", "2025-06-18");
            }
            builder.body(Body::from(body.to_string())).unwrap()
        };
        let response = router
            .clone()
            .oneshot(request(
                serde_json::json!({
                    "jsonrpc": "2.0", "id": 1, "method": "initialize",
                    "params": {
                        "protocolVersion": "2025-06-18", "capabilities": {},
                        "clientInfo": {"name": "team-test", "version": "1"}
                    }
                }),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let session = response.headers()["mcp-session-id"]
            .to_str()
            .unwrap()
            .to_owned();
        let _ = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let response = router
            .clone()
            .oneshot(request(
                serde_json::json!({
                    "jsonrpc": "2.0", "method": "notifications/initialized"
                }),
                Some(&session),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let response = router
            .clone()
            .oneshot(request(
                serde_json::json!({
                    "jsonrpc": "2.0", "id": 2, "method": "tools/list"
                }),
                Some(&session),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_text(response).await;
        let data = body
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .find(|data| !data.trim().is_empty())
            .unwrap_or(&body);
        let reply: serde_json::Value = serde_json::from_str(data).unwrap();
        let tools = reply["result"]["tools"].as_array().unwrap();
        let names: std::collections::HashSet<_> = tools
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect();
        for tool in ["recall", "brief", "get_page_sources", "read_space_rules"] {
            assert!(names.contains(tool));
        }
        assert!(!names.contains("capture"));
        assert!(!names.contains("forget"));
        assert_eq!(names.contains("write_page"), access == TeamAccess::Write);
    }
}

#[tokio::test]
async fn a_key_reads_only_its_members_spaces() {
    let f = fixture();
    let acme = team::rename_team(&f.state.team_path, "Acme").unwrap();
    let (status, body) = call(
        &f.state,
        Method::GET,
        "/team/v1/me",
        Some(&f.writer.1),
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let me: wenlan_types::team::TeamMe = serde_json::from_slice(&body).unwrap();
    assert_eq!(me.member_id, f.writer.0.id);
    assert_eq!(
        me.team,
        Some(acme.clone()),
        "the member learns the team's name"
    );
    // The server's own computer shows the hosted Space under the same team.
    let hosted = shared_space::share_statuses_at(&f.state.settings_path).unwrap();
    assert_eq!(hosted[0].team, Some(acme));

    let (status, body) = call(
        &f.state,
        Method::GET,
        "/team/v1/spaces/Design/files",
        Some(&f.reader.1),
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let list: TeamFileList = serde_json::from_slice(&body).unwrap();
    assert_eq!(list.access, TeamAccess::Read);
    assert_eq!(list.files, ["space.json"]);

    let (status, _) = call(
        &f.state,
        Method::GET,
        "/team/v1/spaces/Design/files",
        Some(&f.outsider.1),
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(
        &f.state,
        Method::GET,
        "/team/v1/spaces/Design/file?path=..%2Fteam.json",
        Some(&f.writer.1),
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn hosted_files_withhold_old_snapshots_with_private_quotes() {
    use wenlan_core::store::format::page::{Citation, Locator, PageFile};
    let (f, db) = fixture_with_daemon().await;
    let legacy_page = create_folder_test_page(&db, "Team notes", "authored").await;
    let id = legacy_page.id.as_str();
    let mut legacy = PageFile::new("Team notes", "The meeting starts on Tuesday. [1]");
    legacy.id = Some(id.into());
    legacy.citations.push(Citation {
        marker: 1,
        label: "“Fixture excerpt” (from Server's conversation)".into(),
        locator: Locator::Other("shared:quote".into()),
    });
    let old = render_test_version(&legacy, "server");
    let (old_sha, _) = version::write_version(&f.folder, "server", id, &old).unwrap();
    let old_path = format!(
        "members/server/pages/{id}/{}",
        version::version_file_name(&old_sha)
    );
    let safe_page = create_folder_test_page(&db, "Published note", "authored").await;
    let safe = version_text(&safe_page.id, "server", "A published note.");
    let (safe_sha, _) = version::write_version(&f.folder, "server", &safe_page.id, &safe).unwrap();
    let safe_path = format!(
        "members/server/pages/{}/{}",
        safe_page.id,
        version::version_file_name(&safe_sha)
    );
    let (status, files) = call(
        &f.state,
        Method::GET,
        "/team/v1/spaces/Design/files",
        Some(&f.reader.1),
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let list: TeamFileList = serde_json::from_slice(&files).unwrap();
    let (download, _) = call(
        &f.state,
        Method::GET,
        &format!("/team/v1/spaces/Design/file?path={old_path}"),
        Some(&f.reader.1),
        vec![],
    )
    .await;
    assert_eq!(
        (list.files.contains(&old_path), download),
        (false, StatusCode::NOT_FOUND),
        "an old private-source snapshot must be absent from the inventory and downloads"
    );
    assert!(list.files.contains(&safe_path));
    let (status, body) = call(
        &f.state,
        Method::GET,
        &format!("/team/v1/spaces/Design/file?path={safe_path}"),
        Some(&f.reader.1),
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, safe.into_bytes());
}

#[tokio::test]
async fn hosted_files_withhold_old_snapshots_of_generated_source_pages() {
    let f = fixture();
    let id = wenlan_core::document_enrichment::source_page_id("fixture", "/import.md");
    let legacy = version_text(&id, "server", "Private import fixture.");
    let (sha, _) = version::write_version(&f.folder, "server", &id, &legacy).unwrap();
    let path = format!(
        "members/server/pages/{id}/{}",
        version::version_file_name(&sha)
    );
    let (status, files) = call(
        &f.state,
        Method::GET,
        "/team/v1/spaces/Design/files",
        Some(&f.reader.1),
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let list: TeamFileList = serde_json::from_slice(&files).unwrap();
    let (download, _) = call(
        &f.state,
        Method::GET,
        &format!("/team/v1/spaces/Design/file?path={path}"),
        Some(&f.reader.1),
        vec![],
    )
    .await;
    assert_eq!(
        (list.files.contains(&path), download),
        (false, StatusCode::NOT_FOUND),
        "import snapshots must be absent from the inventory and downloads"
    );
}

#[tokio::test]
async fn hosted_files_withhold_ordinary_snapshots_that_name_private_imports() {
    use wenlan_core::store::format::page::{Basis, Citation, Locator, PageFile};

    let (f, db) = fixture_with_daemon().await;
    let imported_page = create_folder_test_page(&db, "Imported private title", "imported").await;
    let visible_page = create_folder_test_page(&db, "Published target", "authored").await;
    let safe_page = create_folder_test_page(&db, "Safe ordinary page", "authored").await;
    let ordinary_page = create_folder_test_page(&db, "Ordinary page", "authored").await;
    let shared_ids = db
        .list_shared_pages_scoped(
            "active",
            100,
            0,
            &wenlan_core::read_scope::ReadScope::Space("Design".into()),
        )
        .await
        .unwrap();
    assert!(shared_ids.iter().any(|page| page.id == visible_page.id));
    assert!(!shared_ids.iter().any(|page| page.id == imported_page.id));

    let import_snapshot = version_text(&imported_page.id, "server", "Legacy imported body.");
    let (import_sha, _) =
        version::write_version(&f.folder, "server", &imported_page.id, &import_snapshot).unwrap();
    let import_path = format!(
        "members/server/pages/{}/{}",
        imported_page.id,
        version::version_file_name(&import_sha)
    );

    let id = ordinary_page.id.as_str();
    let mut legacy = PageFile::new("Ordinary page", "A shared paragraph.");
    legacy.id = Some(id.to_string());
    legacy.citations.push(Citation {
        marker: 1,
        label: "[[Imported private title]]".into(),
        locator: Locator::Wenlan {
            id: imported_page.id.clone(),
            version: 1,
        },
    });
    legacy.basis.push(Basis {
        label: "[[Imported private title]]".into(),
        locator: Locator::Wenlan {
            id: imported_page.id,
            version: 1,
        },
    });
    let old = render_test_version(&legacy, "server");
    let (sha, _) = version::write_version(&f.folder, "server", id, &old).unwrap();
    let path = format!(
        "members/server/pages/{id}/{}",
        version::version_file_name(&sha)
    );

    let (status, files) = call(
        &f.state,
        Method::GET,
        "/team/v1/spaces/Design/files",
        Some(&f.reader.1),
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let list: TeamFileList = serde_json::from_slice(&files).unwrap();
    assert!(
        !list.files.contains(&import_path),
        "an ordinary-ID imported page must stay out of the inventory"
    );
    let (download, body) = call(
        &f.state,
        Method::GET,
        &format!("/team/v1/spaces/Design/file?path={path}"),
        Some(&f.reader.1),
        vec![],
    )
    .await;
    assert_eq!(
        (list.files.contains(&path), download),
        (false, StatusCode::NOT_FOUND),
        "an ordinary snapshot must not disclose a private import citation or basis"
    );
    assert!(!String::from_utf8_lossy(&body).contains("Imported private title"));
    let (download, _) = call(
        &f.state,
        Method::GET,
        &format!("/team/v1/spaces/Design/file?path={import_path}"),
        Some(&f.reader.1),
        vec![],
    )
    .await;
    assert_eq!(
        download,
        StatusCode::NOT_FOUND,
        "ordinary-ID imports stay hidden as pages"
    );

    let mut safe = PageFile::new("Safe ordinary page", "A shared paragraph.");
    safe.id = Some(safe_page.id.clone());
    safe.citations.push(Citation {
        marker: 1,
        label: "[[Published target]]".into(),
        locator: Locator::Wenlan {
            id: visible_page.id.clone(),
            version: 1,
        },
    });
    let safe_text = render_test_version(&safe, "server");
    let (safe_sha, _) =
        version::write_version(&f.folder, "server", &safe_page.id, &safe_text).unwrap();
    let safe_path = format!(
        "members/server/pages/{}/{}",
        safe_page.id,
        version::version_file_name(&safe_sha)
    );
    let (status, files) = call(
        &f.state,
        Method::GET,
        "/team/v1/spaces/Design/files",
        Some(&f.reader.1),
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let list: TeamFileList = serde_json::from_slice(&files).unwrap();
    assert!(
        list.files.contains(&safe_path),
        "visible shared targets remain servable"
    );
    let (status, body) = call(
        &f.state,
        Method::GET,
        &format!("/team/v1/spaces/Design/file?path={safe_path}"),
        Some(&f.reader.1),
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let body = String::from_utf8(body).unwrap();
    assert!(body.contains("[[Published target]]"));
    assert!(body.contains(&visible_page.id));
}

#[tokio::test]
async fn hosted_files_withhold_old_per_private_source_placeholders() {
    use wenlan_core::store::format::page::{Citation, Locator, PageFile};

    let (f, db) = fixture_with_daemon().await;
    let page = create_folder_test_page(&db, "Old private placeholder", "authored").await;
    let id = page.id.as_str();
    let mut legacy = PageFile::new("Old private placeholder", "A shared paragraph. [1]");
    legacy.id = Some(id.into());
    legacy.citations.push(Citation {
        marker: 1,
        label: "Unavailable source".into(),
        locator: Locator::Other("shared:private-source".into()),
    });
    let old = render_test_version(&legacy, "server");
    let (sha, _) = version::write_version(&f.folder, "server", id, &old).unwrap();
    let path = format!(
        "members/server/pages/{id}/{}",
        version::version_file_name(&sha)
    );
    let (status, files) = call(
        &f.state,
        Method::GET,
        "/team/v1/spaces/Design/files",
        Some(&f.reader.1),
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let list: TeamFileList = serde_json::from_slice(&files).unwrap();
    let (download, _) = call(
        &f.state,
        Method::GET,
        &format!("/team/v1/spaces/Design/file?path={path}"),
        Some(&f.reader.1),
        vec![],
    )
    .await;
    assert_eq!(
        (list.files.contains(&path), download),
        (false, StatusCode::NOT_FOUND)
    );
}

#[tokio::test]
async fn hosted_files_deliver_archives_after_a_shared_page_is_deleted() {
    let (f, db) = fixture_with_daemon().await;
    let public = create_folder_test_page(&db, "Shared meeting", "authored").await;
    let imported = create_folder_test_page(&db, "Local import", "imported").await;
    assert_eq!(sync_hosted_test_folder(&f, &db).await.exported, 1);
    db.delete_page(&public.id).await.unwrap();
    db.delete_page(&imported.id).await.unwrap();
    assert_eq!(sync_hosted_test_folder(&f, &db).await.exported, 1);

    let (status, bytes) = call(
        &f.state,
        Method::GET,
        "/team/v1/spaces/Design/files",
        Some(&f.reader.1),
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let list: TeamFileList = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(list.files.len(), 2, "the archive should remain visible");
    let path = &list.files[1];
    assert!(path.contains(&public.id));
    let (status, archived) = call(
        &f.state,
        Method::GET,
        &format!("/team/v1/spaces/Design/file?path={path}"),
        Some(&f.reader.1),
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let version = version::parse(path.rsplit('/').next().unwrap(), &archived).unwrap();
    assert!(version.meta.archived);

    // An archive-shaped legacy file alone is no proof the import was shared.
    let mut legacy = version.page;
    legacy.id = Some(imported.id.clone());
    let old = version::render(&legacy, &version.meta);
    let (sha, _) = version::write_version(&f.folder, "server", &imported.id, &old).unwrap();
    let (status, _) = call(
        &f.state,
        Method::GET,
        &format!(
            "/team/v1/spaces/Design/file?path=members/server/pages/{}/{}",
            imported.id,
            version::version_file_name(&sha)
        ),
        Some(&f.reader.1),
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_writer_sends_only_their_own_whole_versions() {
    let (f, db) = fixture_with_daemon().await;
    let page = "page_plan";
    let mine = version_text(page, &f.writer.0.id, "We marry in May.");
    let name = version::version_file_name(&version::sha256_hex(mine.as_bytes()));
    let uri = format!("/team/v1/spaces/Design/versions/{page}/{name}");

    let (status, _) = call(
        &f.state,
        Method::PUT,
        &uri,
        Some(&f.writer.1),
        mine.clone().into(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(f
        .folder
        .join("members")
        .join(&f.writer.0.id)
        .join("pages")
        .join(page)
        .join(&name)
        .is_file());
    // The same file again changes nothing.
    let (status, _) = call(
        &f.state,
        Method::PUT,
        &uri,
        Some(&f.writer.1),
        mine.clone().into(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // A reader sends nothing.
    let theirs = version_text(page, &f.reader.0.id, "We marry in June.");
    let their_name = format!("{}.md", version::sha256_hex(theirs.as_bytes()));
    let (status, _) = call(
        &f.state,
        Method::PUT,
        &format!("/team/v1/spaces/Design/versions/{page}/{their_name}"),
        Some(&f.reader.1),
        theirs.into(),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Another member's version, under the writer's key, is refused.
    let forged = version_text(page, &f.reader.0.id, "We marry in July.");
    let forged_name = format!("{}.md", version::sha256_hex(forged.as_bytes()));
    let (status, _) = call(
        &f.state,
        Method::PUT,
        &format!("/team/v1/spaces/Design/versions/{page}/{forged_name}"),
        Some(&f.writer.1),
        forged.into(),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    // Bytes that do not match the name, or a version of another page.
    let (status, _) = call(
        &f.state,
        Method::PUT,
        &uri,
        Some(&f.writer.1),
        b"changed".to_vec(),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = call(
        &f.state,
        Method::PUT,
        &format!("/team/v1/spaces/Design/versions/page_other/{name}"),
        Some(&f.writer.1),
        mine.into(),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    let (status, body) = call(
        &f.state,
        Method::GET,
        "/team/v1/spaces/Design/files",
        Some(&f.reader.1),
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let pending: TeamFileList = serde_json::from_slice(&body).unwrap();
    assert_eq!(pending.files, ["space.json"]);
    assert_eq!(sync_hosted_test_folder(&f, &db).await.applied, 1);

    let (status, body) = call(
        &f.state,
        Method::GET,
        "/team/v1/spaces/Design/files",
        Some(&f.reader.1),
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let list: TeamFileList = serde_json::from_slice(&body).unwrap();
    assert_eq!(list.files.len(), 2);
    let path = &list.files[1];
    let (status, body) = call(
        &f.state,
        Method::GET,
        &format!(
            "/team/v1/spaces/Design/file?path={}",
            path.replace('/', "%2F")
        ),
        Some(&f.reader.1),
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        version::sha256_hex(&body),
        version::version_sha(&name).unwrap()
    );
}

#[tokio::test]
async fn a_writers_proposals_and_decisions_land_in_their_own_folders() {
    let (f, db) = fixture_with_daemon().await;
    let published = create_folder_test_page(&db, "Plan", "authored").await;
    assert_eq!(sync_hosted_test_folder(&f, &db).await.exported, 1);
    let page = published.id.as_str();
    let put = |meta_change: fn(&mut version::VersionMeta), body: &'static str| {
        let mut file = wenlan_core::store::format::page::PageFile::new("Plan", body);
        file.id = Some(page.into());
        let mut meta = version::VersionMeta {
            parent: None,
            merged: Vec::new(),
            body_digest: version::body_digest(body),
            author: version::Author {
                member: f.writer.0.id.clone(),
                kind: version::AuthorKind::Ai,
                agent: Some("claude-code".into()),
            },
            ts: "2026-11-11T05:00:00Z".into(),
            archived: false,
            locked: true,
            proposal: false,
            decides: None,
            device: None,
        };
        meta_change(&mut meta);
        version::render(&file, &meta)
    };
    let proposal = put(|meta| meta.proposal = true, "We marry in May.");
    let decision = put(
        |meta| {
            meta.author.kind = version::AuthorKind::Person;
            meta.author.agent = None;
            meta.decides = Some(version::Decides {
                proposal: "a".repeat(64),
                accepted: false,
            });
        },
        "",
    );
    for (text, kind) in [(proposal, "proposals"), (decision, "decisions")] {
        let name = version::version_file_name(&version::sha256_hex(text.as_bytes()));
        let (status, _) = call(
            &f.state,
            Method::PUT,
            &format!("/team/v1/spaces/Design/versions/{page}/{name}"),
            Some(&f.writer.1),
            text.into(),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{kind}");
        let dir = f.folder.join("members").join(&f.writer.0.id);
        assert!(dir.join(kind).join(page).join(&name).is_file(), "{kind}");
        assert!(!dir.join("pages").join(page).join(&name).exists(), "{kind}");
    }
    let (status, body) = call(
        &f.state,
        Method::GET,
        "/team/v1/spaces/Design/files",
        Some(&f.reader.1),
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let list: TeamFileList = serde_json::from_slice(&body).unwrap();
    assert!(list.files.iter().any(|path| path.contains("/proposals/")));
    assert!(list.files.iter().any(|path| path.contains("/decisions/")));
}

#[tokio::test]
async fn a_removed_member_is_refused_at_once() {
    let f = fixture();
    team::remove_member(&f.state.team_path, &f.writer.0.id).unwrap();
    let (status, _) = call(
        &f.state,
        Method::GET,
        "/team/v1/me",
        Some(&f.writer.1),
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_browser_from_another_site_is_refused() {
    let f = fixture();
    let response = team_router(f.state.clone())
        .oneshot(
            Request::builder()
                .uri("/team/v1/me")
                .header(header::AUTHORIZATION, format!("Bearer {}", f.writer.1))
                .header(header::ORIGIN, "https://evil.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[test]
fn join_codes_need_a_key_and_a_safe_address() {
    let key = team::new_key();
    assert_eq!(
        join_parts(&format!("https://team.example.com/#{key}"), None).unwrap(),
        ("https://team.example.com".to_string(), key.clone())
    );
    assert!(join_parts(&format!("http://team.example.com#{key}"), None).is_err());
    assert!(join_parts("https://team.example.com", None).is_err());
    assert_eq!(
        join_parts("http://100.70.1.2:7879", Some(&key)).unwrap().0,
        "http://100.70.1.2:7879"
    );
}

#[tokio::test]
async fn only_the_owner_reaches_the_team_s_management_calls() {
    let f = fixture();
    for (method, uri) in [
        (Method::GET, "/team/v1/admin"),
        (Method::POST, "/team/v1/admin/members"),
        (Method::DELETE, "/team/v1/admin/members/x"),
    ] {
        let (status, _) = call(&f.state, method, uri, Some(&f.writer.1), vec![]).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}");
    }
    let (status, _) = call(&f.state, Method::GET, "/team/v1/admin", None, vec![]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (_, owner_key) = team::owner_key(&f.state.team_path, "Owner").unwrap();
    // Nothing but the team's own calls is passed on to this Wenlan.
    for uri in [
        "/team/v1/admin/../pages",
        "/team/v1/admin/join",
        "/team/v1/admin/members/x/y",
    ] {
        let (status, _) = call(&f.state, Method::GET, uri, Some(&owner_key), vec![]).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
    }
    // A member id that a URL parser further on would read as another path
    // (`\` as `/`, then `..`), dots, or escapes: never passed on.
    for (method, uri) in [
        (Method::DELETE, "/team/v1/admin/members/..\\..\\pages\\p1"),
        (Method::PUT, "/team/v1/admin/members/..\\..\\config"),
        (
            Method::POST,
            "/team/v1/admin/members/..\\..\\..\\anything/key",
        ),
        (Method::PUT, "/team/v1/admin/members/..%5C..%5Cconfig"),
        (Method::PUT, "/team/v1/admin/members/.."),
        (Method::PUT, "/team/v1/admin/members/a.b"),
    ] {
        let (status, _) = call(&f.state, method, uri, Some(&owner_key), vec![]).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
    }
    // An allowed call goes to this Wenlan over loopback (not running here).
    let (status, _) = call(
        &f.state,
        Method::GET,
        "/team/v1/admin",
        Some(&owner_key),
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn the_owner_is_told_every_hosted_space_and_can_read_it() {
    let f = fixture();
    let (_, owner_key) = team::owner_key(&f.state.team_path, "Owner").unwrap();
    let (status, body) = call(
        &f.state,
        Method::GET,
        "/team/v1/me",
        Some(&owner_key),
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let me: wenlan_types::team::TeamMe = serde_json::from_slice(&body).unwrap();
    assert_eq!(me.role, TeamRole::Owner);
    assert_eq!(me.spaces.len(), 1);
    assert_eq!(
        (me.spaces[0].space.as_str(), me.spaces[0].access),
        ("Design", TeamAccess::Write)
    );
    // With its id, which members' computers follow it by when renamed.
    assert!(me.spaces[0].id.is_some());
    let (status, _) = call(
        &f.state,
        Method::GET,
        "/team/v1/spaces/Design/files",
        Some(&owner_key),
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

/// `people` names whoever may have changed a page the member can open, and
/// nobody else: id and name only, never a key, a key hash or an invite.
#[tokio::test]
async fn a_member_learns_only_the_names_of_people_in_their_spaces() {
    let f = fixture();
    let (owner, _) = team::owner_key(&f.state.team_path, "Lin").unwrap();
    let gone = team::add_member(
        &f.state.team_path,
        "Dee",
        &[TeamSpaceAccess {
            space: "Design".into(),
            access: TeamAccess::Write,
            id: None,
        }],
    )
    .unwrap();
    team::remove_member(&f.state.team_path, &gone.0.id).unwrap();
    let (status, body) = call(
        &f.state,
        Method::GET,
        "/team/v1/me",
        Some(&f.reader.1),
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let me: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let people: Vec<wenlan_types::team::TeamPerson> =
        serde_json::from_value(me["people"].clone()).unwrap();
    let mut named: Vec<(&str, &str, bool)> = people
        .iter()
        .map(|person| (person.id.as_str(), person.name.as_str(), person.owner))
        .collect();
    named.sort();
    let mut want = vec![
        (f.writer.0.id.as_str(), "Amy", false),
        (f.reader.0.id.as_str(), "Bo", false),
        // A removed member's past changes keep their name.
        (gone.0.id.as_str(), "Dee", false),
        (owner.id.as_str(), "Lin", true),
    ];
    want.sort();
    assert_eq!(named, want, "Cy shares no Space with Bo");
    // Each person is an id, a name and the owner mark: nothing else.
    for person in me["people"].as_array().unwrap() {
        let keys: Vec<&String> = person.as_object().unwrap().keys().collect();
        assert!(
            keys.iter()
                .all(|key| ["id", "name", "owner"].contains(&key.as_str())),
            "{keys:?}"
        );
    }
    let text = String::from_utf8(body).unwrap();
    for secret in [
        &f.writer.1,
        &f.outsider.1,
        f.writer.0.key_sha256.as_ref().unwrap(),
    ] {
        assert!(!text.contains(secret.as_str()));
    }
    assert!(!text.contains("Cy") && !text.contains("Other") && !text.contains("join"));

    // Cy's only Space, "Other", is not hosted here: Cy is told of nobody.
    let (_, body) = call(
        &f.state,
        Method::GET,
        "/team/v1/me",
        Some(&f.outsider.1),
        vec![],
    )
    .await;
    let me: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(me["people"], serde_json::json!([]));
}

// ===== Tool keys and the route table =====

#[test]
fn the_route_table_gives_a_browser_session_the_web_only() {
    use team::CredentialKind::{BrowserSession, LegacyKey, ToolKey};
    for path in [
        "/team/v1/me",
        "/team/v1/app/api/pages",
        "/auth/logout",
        "/auth/logout-all",
    ] {
        assert!(credential_may_reach(BrowserSession, path), "{path}");
    }
    for path in [
        "/mcp",
        "/mcp/Design",
        "/team/v1/admin",
        "/team/v1/admin/members/x/compromised",
        "/team/v1/spaces/design/files",
        "/team/v1/spaces/design/versions/p/n",
        "/team/v1/account/keys",
        "/team/v1/mex",
        "/",
    ] {
        assert!(!credential_may_reach(BrowserSession, path), "{path}");
    }
    assert!(!credential_may_reach(LegacyKey, "/auth/logout"));
    assert!(!credential_may_reach(ToolKey, "/auth/logout"));
    assert!(!credential_may_reach(LegacyKey, "/auth/logout-all"));
    assert!(!credential_may_reach(ToolKey, "/auth/logout-all"));
}

#[test]
fn the_route_table_gives_a_tool_key_only_mcp() {
    use team::CredentialKind::{LegacyKey, ToolKey};
    for path in ["/mcp", "/mcp/Design", "/mcp/Design/x"] {
        assert!(credential_may_reach(ToolKey, path), "{path}");
        assert!(credential_may_reach(LegacyKey, path), "{path}");
    }
    for path in [
        "/team/v1/me",
        "/team/v1/admin",
        "/team/v1/admin/members/x/compromised",
        "/team/v1/app/anything",
        "/team/v1/spaces/design/files",
        "/mcpx",
        "/",
    ] {
        assert!(!credential_may_reach(ToolKey, path), "{path}");
        assert!(credential_may_reach(LegacyKey, path), "{path}");
    }
}

#[tokio::test]
async fn a_tool_key_is_refused_on_every_route_but_mcp() {
    let f = fixture();
    let (_, tool) = team::issue_tool_key(&f.state.team_path, &f.writer.0.id, "Cursor").unwrap();
    let page = "p1";
    let name = format!("{}.md", "a".repeat(64));
    let put = format!("/team/v1/spaces/design/versions/{page}/{name}");
    let routes: Vec<(Method, String)> = vec![
        (Method::GET, "/team/v1/me".into()),
        (Method::POST, "/team/v1/me".into()),
        (Method::GET, "/team/v1/admin".into()),
        (Method::PUT, "/team/v1/admin".into()),
        (Method::POST, "/team/v1/admin/members".into()),
        (Method::POST, "/team/v1/admin/spaces".into()),
        (Method::DELETE, "/team/v1/admin/members/x".into()),
        (Method::POST, "/team/v1/admin/members/x/key".into()),
        (Method::POST, "/team/v1/admin/members/x/compromised".into()),
        (Method::GET, "/team/v1/spaces/Design/files".into()),
        (
            Method::GET,
            "/team/v1/spaces/Design/file?path=space.json".into(),
        ),
        (Method::PUT, put),
        (Method::GET, "/team/v1/app/api/pages".into()),
        (Method::POST, "/team/v1/app/api/pages".into()),
        (Method::GET, "/team/v1/account/keys".into()),
        (Method::GET, "/nothing-here".into()),
    ];
    for (method, uri) in routes {
        let (status, _) = call(&f.state, method.clone(), &uri, Some(&tool), b"x".to_vec()).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}");
    }
    // The same key passes the table on /mcp (whatever the MCP service then
    // answers, it is not the table's 401 or 403).
    for uri in ["/mcp", "/mcp/Design"] {
        let (status, _) = call(&f.state, Method::GET, uri, Some(&tool), vec![]).await;
        assert_ne!(status, StatusCode::FORBIDDEN, "{uri}");
        assert_ne!(status, StatusCode::UNAUTHORIZED, "{uri}");
    }
    // The member's legacy key still reaches what it did.
    let (status, _) = call(
        &f.state,
        Method::GET,
        "/team/v1/me",
        Some(&f.writer.1),
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // A revoked or unknown tool key is 401 everywhere.
    let (id, revoked) = team::issue_tool_key(&f.state.team_path, &f.writer.0.id, "Old").unwrap();
    team::revoke_tool_key(&f.state.team_path, &f.writer.0.id, &id).unwrap();
    for uri in ["/mcp", "/team/v1/me"] {
        let (status, _) = call(&f.state, Method::GET, uri, Some(&revoked), vec![]).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri}");
    }
}

#[tokio::test]
async fn an_owner_tool_key_does_not_open_the_management_calls() {
    let f = fixture();
    let (owner, _) = team::owner_key(&f.state.team_path, "Owner").unwrap();
    let (_, tool) = team::issue_tool_key(&f.state.team_path, &owner.id, "Cursor").unwrap();
    let uri = format!("/team/v1/admin/members/{}/compromised", f.writer.0.id);
    let (status, _) = call(&f.state, Method::POST, &uri, Some(&tool), vec![]).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // Nothing happened to Amy.
    assert!(team::authenticate(&f.state.team_path, &f.writer.1)
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn the_owner_resets_a_compromised_account() {
    let f = fixture();
    let (_, tool) = team::issue_tool_key(&f.state.team_path, &f.writer.0.id, "Cursor").unwrap();
    let uri = format!("/team/v1/admin/members/{}/compromised", f.writer.0.id);

    // Not for a member, or without a key.
    let (status, _) = call(&f.state, Method::POST, &uri, Some(&f.reader.1), vec![]).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(&f.state, Method::POST, &uri, None, vec![]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (_, owner_key) = team::owner_key(&f.state.team_path, "Owner").unwrap();
    let (status, _) = call(&f.state, Method::GET, &uri, Some(&owner_key), vec![]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(
        &f.state,
        Method::POST,
        "/team/v1/admin/members/nobody/compromised",
        Some(&owner_key),
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, body) = call(&f.state, Method::POST, &uri, Some(&owner_key), vec![]).await;
    assert_eq!(status, StatusCode::OK);
    let answer: wenlan_types::team::TeamMemberKey = serde_json::from_slice(&body).unwrap();
    assert_eq!(answer.member.id, f.writer.0.id);
    assert!(wenlan_types::team::is_team_key(&answer.key));
    for dead in [&f.writer.1, &tool] {
        let (status, _) = call(&f.state, Method::GET, "/mcp", Some(dead), vec![]).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    let (status, _) = call(
        &f.state,
        Method::GET,
        "/team/v1/me",
        Some(&answer.key),
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

// ===== Sign-in invites: owner admin =====

fn with_url(f: &Fixture) {
    team::set_url(&f.state.team_path, Some("https://team.example.com")).unwrap();
}

#[tokio::test]
async fn the_owner_makes_an_invite_with_a_link_and_nobody_else_can() {
    let f = fixture();
    with_url(&f);
    let (owner, owner_key) = team::owner_key(&f.state.team_path, "Owner").unwrap();
    let (_, tool) = team::issue_tool_key(&f.state.team_path, &owner.id, "Cursor").unwrap();
    let request = serde_json::json!({
        "name": "Dee", "email": "dee@example.com",
        "spaces": [{"space": "Design", "access": "write"}]
    })
    .to_string()
    .into_bytes();
    let uri = "/team/v1/admin/invites";
    // A member, a tool key (the owner's too), and no key: refused.
    for key in [Some(f.writer.1.as_str()), Some(tool.as_str()), None] {
        let (status, _) = call(&f.state, Method::POST, uri, key, request.clone()).await;
        assert!(
            matches!(status, StatusCode::FORBIDDEN | StatusCode::UNAUTHORIZED),
            "{status}"
        );
    }
    assert_eq!(
        team::read_team(&f.state.team_path)
            .unwrap()
            .members
            .iter()
            .filter(|m| m.name == "Dee")
            .count(),
        0
    );

    let (status, body) = call(
        &f.state,
        Method::POST,
        uri,
        Some(&owner_key),
        request.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let answer: wenlan_types::team::TeamInviteLink = serde_json::from_slice(&body).unwrap();
    assert_eq!(answer.member.name, "Dee");
    let token = answer
        .link
        .strip_prefix("https://team.example.com/join#")
        .unwrap();
    assert!(wenlan_types::team::is_invite_token(token));
    let dee = team::read_team(&f.state.team_path)
        .unwrap()
        .members
        .into_iter()
        .find(|m| m.id == answer.member.id)
        .unwrap();
    assert!(dee.key_sha256.is_none());
    assert_eq!(
        dee.invite.as_ref().unwrap().email.as_deref(),
        Some("dee@example.com")
    );
    assert_eq!(dee.access("Design"), Some(TeamAccess::Write));
    // The link is not a key: it opens nothing.
    let (status, _) = call(&f.state, Method::GET, "/team/v1/me", Some(token), vec![]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // A bad email or name is a 400 and makes nobody.
    let bad = serde_json::json!({"name": "Eve", "email": "nope", "spaces": []})
        .to_string()
        .into_bytes();
    let (status, _) = call(&f.state, Method::POST, uri, Some(&owner_key), bad).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Reissue replaces the token; approve with nobody waiting is a 400.
    let reissue = format!("/team/v1/admin/members/{}/invite", dee.id);
    let (status, body) = call(&f.state, Method::POST, &reissue, Some(&owner_key), vec![]).await;
    assert_eq!(status, StatusCode::OK);
    let again: wenlan_types::team::TeamInviteLink = serde_json::from_slice(&body).unwrap();
    assert_ne!(again.link, answer.link);
    let approve = format!("/team/v1/admin/members/{}/approve", dee.id);
    let (status, _) = call(&f.state, Method::POST, &approve, Some(&owner_key), vec![]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = call(
        &f.state,
        Method::POST,
        "/team/v1/admin/members/nobody/approve",
        Some(&owner_key),
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_invite_needs_the_teams_address_for_its_link() {
    let f = fixture();
    let (_, owner_key) = team::owner_key(&f.state.team_path, "Owner").unwrap();
    let body = serde_json::json!({"name": "Dee", "spaces": []})
        .to_string()
        .into_bytes();
    let (status, _) = call(
        &f.state,
        Method::POST,
        "/team/v1/admin/invites",
        Some(&owner_key),
        body,
    )
    .await;
    // (The test process may set WENLAN_TEAM_URL; without it this is a 409.)
    if env_value(TEAM_URL_ENV).is_none() {
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(team::read_team(&f.state.team_path)
            .unwrap()
            .members
            .iter()
            .all(|m| m.name != "Dee"));
    }
}

#[tokio::test]
async fn a_browser_session_cannot_reach_the_invite_admin_calls() {
    let f = fixture();
    with_url(&f);
    let (owner, _) = team::owner_key(&f.state.team_path, "Owner").unwrap();
    let token = f.state.sessions.create_fresh(&owner.id).unwrap();
    for (method, uri) in [
        (Method::POST, "/team/v1/admin/invites".to_string()),
        (
            Method::POST,
            format!("/team/v1/admin/members/{}/invite", f.writer.0.id),
        ),
        (
            Method::POST,
            format!("/team/v1/admin/members/{}/approve", f.writer.0.id),
        ),
    ] {
        let req = Request::builder()
            .method(method.clone())
            .uri(&uri)
            .header(header::ORIGIN, "https://team.example.com")
            .header(
                header::COOKIE,
                format!("{}={token}", crate::team_session::COOKIE_NAME),
            )
            .body(Body::from("{}"))
            .unwrap();
        let response = team_router(f.state.clone()).oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{method} {uri}");
    }
    assert!(!credential_may_reach(
        team::CredentialKind::BrowserSession,
        "/team/v1/admin/invites"
    ));
    // The sign-in start is public, like the other /auth entry points.
    assert!(credential_may_reach(
        team::CredentialKind::BrowserSession,
        "/auth/invite/google"
    ));
}

#[tokio::test]
async fn a_tool_key_is_refused_on_the_invite_admin_calls() {
    let f = fixture();
    with_url(&f);
    let (owner, _) = team::owner_key(&f.state.team_path, "Owner").unwrap();
    let (_, tool) = team::issue_tool_key(&f.state.team_path, &owner.id, "Cursor").unwrap();
    for uri in [
        "/team/v1/admin/invites".to_string(),
        format!("/team/v1/admin/members/{}/invite", f.writer.0.id),
        format!("/team/v1/admin/members/{}/approve", f.writer.0.id),
    ] {
        let (status, _) = call(&f.state, Method::POST, &uri, Some(&tool), b"{}".to_vec()).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}");
    }
}

// ===== Browser sessions =====

const ORIGIN: &str = "https://team.example.com";
const DAY: u64 = 24 * 60 * 60;

/// A fixture whose session clock the test moves, and whose own address is
/// [`ORIGIN`].
fn session_fixture() -> (Fixture, Arc<std::sync::atomic::AtomicU64>) {
    use std::sync::atomic::{AtomicU64, Ordering};
    let mut f = fixture();
    team::set_url(&f.state.team_path, Some(ORIGIN)).unwrap();
    let base = std::time::Instant::now();
    let offset = Arc::new(AtomicU64::new(0));
    let seen = offset.clone();
    f.state.sessions = Arc::new(crate::team_session::SessionStore::with_clock(Arc::new(
        move || base + std::time::Duration::from_secs(seen.load(Ordering::SeqCst)),
    )));
    (f, offset)
}

async fn browser(
    state: &TeamState,
    method: Method,
    uri: &str,
    cookie: Option<&str>,
    origin: Option<&str>,
    bearer: Option<&str>,
) -> Response {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(token) = cookie {
        req = req.header(header::COOKIE, format!("__Host-wenlan-session={token}"));
    }
    if let Some(origin) = origin {
        req = req.header(header::ORIGIN, origin);
    }
    if let Some(key) = bearer {
        req = req.header(header::AUTHORIZATION, format!("Bearer {key}"));
    }
    team_router(state.clone())
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

fn no_store(response: &Response) -> bool {
    response
        .headers()
        .get(header::CACHE_CONTROL)
        .is_some_and(|v| v == "no-store")
}

#[tokio::test]
async fn rulesync_team_project_review_requires_a_current_write_session() {
    let (f, _) = session_fixture();
    team::owner_key(&f.state.team_path, "Team owner").unwrap();
    std::fs::write(
        f.state.settings_path.with_file_name("repo-wiki.json"),
        serde_json::to_vec(&serde_json::json!({
            "bindings": [{
                "space": "Design",
                "repo_root": f._dir.path().join("repo"),
                "folder": "wiki",
                "remote_url": "https://github.com/acme/design.git",
            }]
        }))
        .unwrap(),
    )
    .unwrap();
    let seed = rulesync_team::update_hosted_rules(&f.state.settings_path, "Design", "Initial rule")
        .unwrap();
    rulesync_team::decide_local_space_proposal(&f.state.settings_path, "Design", &seed, true)
        .unwrap();
    let files = "/team/v1/spaces/Design/files";
    let (status, body) = call(&f.state, Method::GET, files, Some(&f.reader.1), vec![]).await;
    assert_eq!(status, StatusCode::OK);
    let list: TeamFileList = serde_json::from_slice(&body).unwrap();
    let rules = list.rules.unwrap();
    assert_eq!(rules.project.as_ref().unwrap().space, "Design");
    let revision = rules
        .file
        .rsplit('/')
        .next()
        .and_then(version::version_sha)
        .unwrap();

    let mut page = wenlan_core::store::format::page::PageFile::new("Rules", "Review every edit");
    page.id = Some(rules.page_id.clone());
    let text = version::render(
        &page,
        &version::VersionMeta {
            parent: Some(revision.into()),
            merged: Vec::new(),
            body_digest: version::body_digest(&page.body),
            author: version::Author {
                member: f.reader.0.id.clone(),
                kind: version::AuthorKind::Ai,
                agent: None,
            },
            ts: "2026-11-18T05:00:00Z".into(),
            archived: false,
            locked: true,
            proposal: true,
            decides: None,
            device: None,
        },
    );
    let proposal = version::sha256_hex(text.as_bytes());
    let name = version::version_file_name(&proposal);
    let upload = format!("/team/v1/spaces/Design/versions/{}/{name}", rules.page_id);
    let (status, _) = call(
        &f.state,
        Method::PUT,
        &upload,
        Some(&f.reader.1),
        text.into_bytes(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let forged = version_text(&rules.page_id, &f.writer.0.id, "Unapproved team rules");
    let forged_name = version::version_file_name(&version::sha256_hex(forged.as_bytes()));
    let forged_path = format!(
        "/team/v1/spaces/Design/versions/{}/{forged_name}",
        rules.page_id
    );
    let (status, _) = call(
        &f.state,
        Method::PUT,
        &forged_path,
        Some(&f.writer.1),
        forged.into_bytes(),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    let review = "/team/v1/spaces/Design/rules/proposals";
    let (status, _) = call(&f.state, Method::GET, review, Some(&f.writer.1), vec![]).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let writer = f.state.sessions.create(&f.writer.0.id).unwrap();
    let reader = f.state.sessions.create(&f.reader.0.id).unwrap();
    let response = browser(&f.state, Method::GET, review, Some(&writer), None, None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response_text(response).await.contains(&proposal));
    let decision = format!("{review}/{proposal}/decision");
    let accept = |token: &str| {
        team_router(f.state.clone()).oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(&decision)
                .header(header::COOKIE, format!("__Host-wenlan-session={token}"))
                .header(header::ORIGIN, ORIGIN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"accept":true}"#))
                .unwrap(),
        )
    };
    assert_eq!(
        accept(&reader).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        accept(&writer).await.unwrap().status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        accept(&writer).await.unwrap().status(),
        StatusCode::CONFLICT
    );
    let approved = rulesync_team::hosted_project_layer(&f.state.settings_path, "Design")
        .unwrap()
        .unwrap();
    assert_eq!(approved.markdown, "Review every edit");

    team::update_member(&f.state.team_path, &f.reader.0.id, None, Some(&[])).unwrap();
    let (status, _) = call(&f.state, Method::GET, files, Some(&f.reader.1), vec![]).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_session_cookie_reaches_me_and_the_app_gate_and_nothing_else() {
    let (f, _) = session_fixture();
    let token = f.state.sessions.create(&f.writer.0.id).unwrap();
    let get = |uri: &'static str| browser(&f.state, Method::GET, uri, Some(&token), None, None);

    let me = get("/team/v1/me").await;
    assert_eq!(me.status(), StatusCode::OK);
    assert!(no_store(&me));
    let bytes = to_bytes(me.into_body(), 1 << 20).await.unwrap();
    assert!(String::from_utf8_lossy(&bytes).contains(&f.writer.0.id));

    let gate = get("/team/v1/app/api/pages").await;
    assert_ne!(gate.status(), StatusCode::UNAUTHORIZED);
    assert_ne!(gate.status(), StatusCode::FORBIDDEN);
    assert!(no_store(&gate));
    // A write on the gate from the server's own page passes the table too.
    let post = browser(
        &f.state,
        Method::POST,
        "/team/v1/app/api/pages",
        Some(&token),
        Some(ORIGIN),
        None,
    )
    .await;
    assert_ne!(post.status(), StatusCode::UNAUTHORIZED);
    assert_ne!(post.status(), StatusCode::FORBIDDEN);

    let page = "p1";
    let name = format!("{}.md", "a".repeat(64));
    let put = format!("/team/v1/spaces/design/versions/{page}/{name}");
    for (method, uri) in [
        (Method::GET, "/mcp".to_string()),
        (Method::POST, "/mcp/Design".to_string()),
        (Method::GET, "/team/v1/spaces/Design/files".to_string()),
        (
            Method::GET,
            "/team/v1/spaces/Design/file?path=x".to_string(),
        ),
        (Method::PUT, put),
        (Method::GET, "/team/v1/admin".to_string()),
        (Method::POST, "/team/v1/admin/members".to_string()),
        (
            Method::POST,
            "/team/v1/admin/members/x/compromised".to_string(),
        ),
        (Method::GET, "/team/v1/account/keys".to_string()),
    ] {
        let response = browser(
            &f.state,
            method.clone(),
            &uri,
            Some(&token),
            Some(ORIGIN),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{method} {uri}");
        assert!(no_store(&response), "{method} {uri}");
    }
    // An owner's cookie does not open the owner's calls either, yet.
    let (owner, _) = team::owner_key(&f.state.team_path, "Owner").unwrap();
    let owner_token = f.state.sessions.create(&owner.id).unwrap();
    let response = browser(
        &f.state,
        Method::GET,
        "/team/v1/admin",
        Some(&owner_token),
        None,
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_wrong_bearer_is_refused_even_with_a_good_cookie() {
    let (f, _) = session_fixture();
    let token = f.state.sessions.create(&f.writer.0.id).unwrap();
    let wrong = format!("wlt_{}", "0".repeat(64));
    for bearer in [wrong.as_str(), "", "garbage"] {
        let response = browser(
            &f.state,
            Method::GET,
            "/team/v1/me",
            Some(&token),
            None,
            Some(bearer),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{bearer:?}");
    }
    // A good key with the cookie is the key's request, as before.
    let response = browser(
        &f.state,
        Method::GET,
        "/mcp",
        Some(&token),
        None,
        Some(&f.writer.1),
    )
    .await;
    assert_ne!(response.status(), StatusCode::UNAUTHORIZED);
    assert_ne!(response.status(), StatusCode::FORBIDDEN);
    assert!(!no_store(&response) || response.status() != StatusCode::OK);
}

#[tokio::test]
async fn a_cookie_change_needs_this_servers_exact_origin() {
    let (f, _) = session_fixture();
    let token = f.state.sessions.create(&f.writer.0.id).unwrap();
    let uri = "/team/v1/app/api/pages";
    for origin in [
        None,
        Some("null"),
        Some("http://team.example.com"),
        Some("https://team.example.com:8443"),
        Some("https://team.example.com."),
        Some("https://evil.example"),
        Some("https://team.example.com/"),
    ] {
        let response = browser(&f.state, Method::POST, uri, Some(&token), origin, None).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{origin:?}");
    }
    // No Host-based fallback: a page at the very Host it called still fails.
    let response = team_router(f.state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(uri)
                .header(header::COOKIE, format!("__Host-wenlan-session={token}"))
                .header(header::HOST, "other.example")
                .header(header::ORIGIN, "https://other.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let response = browser(
        &f.state,
        Method::POST,
        uri,
        Some(&token),
        Some(ORIGIN),
        None,
    )
    .await;
    assert_ne!(response.status(), StatusCode::FORBIDDEN);
    // A read needs no Origin, but one that is present must be the server's.
    let response = browser(
        &f.state,
        Method::GET,
        "/team/v1/me",
        Some(&token),
        None,
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let response = browser(
        &f.state,
        Method::GET,
        "/team/v1/me",
        Some(&token),
        Some("https://evil.example"),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    // Without a configured address no change passes.
    team::set_url(&f.state.team_path, None).unwrap();
    let response = browser(
        &f.state,
        Method::POST,
        uri,
        Some(&token),
        Some(ORIGIN),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_bearer_request_keeps_the_origin_rules_it_had() {
    let (f, _) = session_fixture();
    // The same-Host fallback still serves the app gate for a key.
    let response = team_router(f.state.clone())
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/team/v1/app/api/pages")
                .header(header::AUTHORIZATION, format!("Bearer {}", f.writer.1))
                .header(header::HOST, "other.example")
                .header(header::ORIGIN, "https://other.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(response.status(), StatusCode::FORBIDDEN);
    assert!(!no_store(&response) || response.status() != StatusCode::OK);
}

#[tokio::test]
async fn a_removed_members_cookie_is_refused_and_forgotten() {
    let (f, _) = session_fixture();
    let token = f.state.sessions.create(&f.writer.0.id).unwrap();
    let uri = "/team/v1/me";
    let ok = browser(&f.state, Method::GET, uri, Some(&token), None, None).await;
    assert_eq!(ok.status(), StatusCode::OK);
    team::remove_member(&f.state.team_path, &f.writer.0.id).unwrap();
    let response = browser(&f.state, Method::GET, uri, Some(&token), None, None).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(no_store(&response));
    assert!(f.state.sessions.lookup(&token).is_none());
}

#[tokio::test]
async fn idle_and_absolute_expiry_refuse_and_remove_the_session() {
    use std::sync::atomic::Ordering;
    let (f, clock) = session_fixture();
    let uri = "/team/v1/me";
    let idle = f.state.sessions.create(&f.writer.0.id).unwrap();
    clock.fetch_add(30 * DAY, Ordering::SeqCst);
    let response = browser(&f.state, Method::GET, uri, Some(&idle), None, None).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(f.state.sessions.len(), 0);

    let long = f.state.sessions.create(&f.writer.0.id).unwrap();
    for _ in 0..3 {
        clock.fetch_add(29 * DAY, Ordering::SeqCst);
        let response = browser(&f.state, Method::GET, uri, Some(&long), None, None).await;
        assert_eq!(response.status(), StatusCode::OK);
    }
    clock.fetch_add(3 * DAY, Ordering::SeqCst);
    let response = browser(&f.state, Method::GET, uri, Some(&long), None, None).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(f.state.sessions.len(), 0);
}

#[tokio::test]
async fn logout_ends_the_session_and_clears_the_cookie() {
    let (f, _) = session_fixture();
    let token = f.state.sessions.create(&f.writer.0.id).unwrap();
    let other = f.state.sessions.create(&f.writer.0.id).unwrap();
    let uri = "/auth/logout";
    // Without the server's Origin it does nothing.
    let response = browser(&f.state, Method::POST, uri, Some(&token), None, None).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(f.state.sessions.lookup(&token).is_some());
    // A key has no session to end.
    let response = browser(&f.state, Method::POST, uri, None, None, Some(&f.writer.1)).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let response = browser(
        &f.state,
        Method::POST,
        uri,
        Some(&token),
        Some(ORIGIN),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let headers = response.headers();
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    assert_eq!(headers[header::REFERRER_POLICY], "no-referrer");
    assert_eq!(
        headers[header::SET_COOKIE],
        "__Host-wenlan-session=; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=0"
    );
    let response = browser(
        &f.state,
        Method::GET,
        "/team/v1/me",
        Some(&token),
        None,
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    // Only that browser's session ended.
    let response = browser(
        &f.state,
        Method::GET,
        "/team/v1/me",
        Some(&other),
        None,
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn compromise_reset_and_removal_through_the_owner_end_the_sessions() {
    let (f, _) = session_fixture();
    let (_, owner_key) = team::owner_key(&f.state.team_path, "Owner").unwrap();
    let token = f.state.sessions.create(&f.writer.0.id).unwrap();
    let uri = format!("/team/v1/admin/members/{}/compromised", f.writer.0.id);
    let (status, _) = call(&f.state, Method::POST, &uri, Some(&owner_key), vec![]).await;
    assert_eq!(status, StatusCode::OK);
    let response = browser(
        &f.state,
        Method::GET,
        "/team/v1/me",
        Some(&token),
        None,
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(f.state.sessions.len(), 0);
}

#[tokio::test]
async fn an_unknown_or_malformed_cookie_is_401() {
    let (f, _) = session_fixture();
    for token in ["x", &"0".repeat(64), ""] {
        let response = browser(
            &f.state,
            Method::GET,
            "/team/v1/me",
            Some(token),
            None,
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{token:?}");
    }
    let response = browser(&f.state, Method::GET, "/team/v1/me", None, None, None).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn me_says_how_the_request_signed_in_and_which_accounts_are_linked() {
    let (f, _) = session_fixture();
    let json = |response: Response| async move {
        let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()
    };
    // A key: no account yet.
    let by_key = browser(
        &f.state,
        Method::GET,
        "/team/v1/me",
        None,
        None,
        Some(&f.writer.1),
    )
    .await;
    assert_eq!(by_key.status(), StatusCode::OK);
    let by_key = json(by_key).await;
    assert_eq!(by_key["signed_in_with"], "key");
    assert!(by_key.get("accounts").is_none());

    // The same member with a GitHub account, by cookie: the provider and
    // the email, never the subject id.
    team::link_identity(
        &f.state.team_path,
        &f.writer.0.id,
        team::Identity {
            issuer: "https://github.com".into(),
            subject: "subject-4242".into(),
            email: Some("amy@example.com".into()),
            linked_at: String::new(),
        },
    )
    .unwrap();
    let token = f.state.sessions.create(&f.writer.0.id).unwrap();
    let by_cookie = browser(
        &f.state,
        Method::GET,
        "/team/v1/me",
        Some(&token),
        None,
        None,
    )
    .await;
    let text = {
        let bytes = to_bytes(by_cookie.into_body(), 1 << 20).await.unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    };
    assert!(!text.contains("subject-4242"));
    let me: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(me["signed_in_with"], "browser");
    assert_eq!(me["accounts"][0]["provider"], "github");
    assert_eq!(me["accounts"][0]["email"], "amy@example.com");
}

#[tokio::test]
async fn logout_all_ends_every_session_of_the_member_and_nobody_elses() {
    let (f, _) = session_fixture();
    let uri = "/auth/logout-all";
    let mine = f.state.sessions.create(&f.writer.0.id).unwrap();
    let mine_elsewhere = f.state.sessions.create(&f.writer.0.id).unwrap();
    let theirs = f.state.sessions.create(&f.reader.0.id).unwrap();

    // The server's exact origin only, and only from a browser.
    for origin in [None, Some("https://evil.example")] {
        let response = browser(&f.state, Method::POST, uri, Some(&mine), origin, None).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{origin:?}");
    }
    let response = browser(&f.state, Method::POST, uri, None, None, Some(&f.writer.1)).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(f.state.sessions.lookup(&mine_elsewhere).is_some());

    let response = browser(&f.state, Method::POST, uri, Some(&mine), Some(ORIGIN), None).await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert!(response.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .contains("Max-Age=0"));
    for token in [&mine, &mine_elsewhere] {
        let me = browser(
            &f.state,
            Method::GET,
            "/team/v1/me",
            Some(token),
            None,
            None,
        )
        .await;
        assert_eq!(me.status(), StatusCode::UNAUTHORIZED);
    }
    let me = browser(
        &f.state,
        Method::GET,
        "/team/v1/me",
        Some(&theirs),
        None,
        None,
    )
    .await;
    assert_eq!(me.status(), StatusCode::OK);
    // Their AI tool keys are untouched.
    let (status, _) = call(
        &f.state,
        Method::GET,
        "/team/v1/me",
        Some(&f.writer.1),
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

/// A team started at a loopback invite address (`--url http://127.0.0.1:…`)
/// listens on this computer only, never on the network; any other address
/// still listens on every network so teammates can reach it.
#[test]
fn local_only_team_listens_on_loopback_only() {
    for url in [
        "http://127.0.0.1:17944",
        "http://localhost:17944",
        "http://LOCALHOST:17944",
        "http://[::1]:17944",
        "http://127.0.0.2:17944",
    ] {
        let addr: std::net::SocketAddr = listen_addr(Some(url), 17944)
            .parse()
            .unwrap_or_else(|_| panic!("{url}: not a socket address"));
        assert!(addr.ip().is_loopback(), "{url} listened on {addr}");
        assert_eq!(addr.port(), 17944);
    }
    for url in [
        Some("http://192.168.1.20:7879"),
        Some("http://100.101.102.103:7879"),
        Some("https://wenlan.example.com"),
        None,
    ] {
        assert_eq!(listen_addr(url, 7879), "0.0.0.0:7879", "{url:?}");
    }
}

/// The loopback address really binds loopback only: the socket the team
/// listener would open is not on an unspecified (every network) address.
#[tokio::test]
async fn local_only_team_binds_loopback_socket() {
    let listener = tokio::net::TcpListener::bind(listen_addr(Some("http://127.0.0.1:0"), 0))
        .await
        .unwrap();
    let local = listener.local_addr().unwrap();
    assert!(local.ip().is_loopback(), "bound {local}");
    assert!(!local.ip().is_unspecified());
}
