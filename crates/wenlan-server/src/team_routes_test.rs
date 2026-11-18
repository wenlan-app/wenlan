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
            daemon_url: "http://127.0.0.1:9".into(),
            mcp: Arc::default(),
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

fn version_text(page: &str, member: &str, body: &str) -> String {
    let mut file = wenlan_core::store::format::page::PageFile::new("Plan", body);
    file.id = Some(page.into());
    version::render(
        &file,
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
async fn a_writer_sends_only_their_own_whole_versions() {
    let f = fixture();
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
    let f = fixture();
    let page = "page_plan";
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
