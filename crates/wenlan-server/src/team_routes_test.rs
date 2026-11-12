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
    assert_eq!(
        me.spaces,
        vec![TeamSpaceAccess {
            space: "Design".into(),
            access: TeamAccess::Write
        }]
    );
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
