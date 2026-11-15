// SPDX-License-Identifier: Apache-2.0
//! The app gate against a stand-in daemon that records what reaches it:
//! a member's key opens only their own Spaces, by every way a call names
//! one, and nothing outside the table gets through.

use std::sync::Mutex;

use axum::body::to_bytes;
use axum::http::{HeaderMap, Method};
use axum::Json;
use serde_json::json;
use tower::ServiceExt;
use wenlan_core::shared_space::{self, Scope, SpaceMember};
use wenlan_types::team::TeamSpaceAccess;

use super::super::{team_router, TeamState};
use super::*;

/// One call the stand-in daemon received.
#[derive(Debug, Clone)]
struct Seen {
    method: String,
    uri: String,
    headers: HeaderMap,
    body: Value,
}

impl Seen {
    fn space(&self) -> Option<&str> {
        self.headers
            .get(crate::space_header::HEADER_NAME)
            .and_then(|value| value.to_str().ok())
    }
}

type Log = Arc<Mutex<Vec<Seen>>>;
use std::sync::Arc;

/// Which Space a stand-in page id is in: `a…` Alpha, `b…` Beta.
fn space_of(id: &str) -> Option<&'static str> {
    match id.chars().next() {
        Some('a') => Some("Alpha"),
        Some('b') => Some("Beta"),
        _ => None,
    }
}

async fn stand_in(log: Log, req: Request) -> Response {
    let method = req.method().to_string();
    let uri = req.uri().to_string();
    let path = req.uri().path().to_string();
    let headers = req.headers().clone();
    let bytes = to_bytes(req.into_body(), 1 << 20).await.unwrap();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let pinned = headers
        .get(crate::space_header::HEADER_NAME)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    log.lock().unwrap().push(Seen {
        method: method.clone(),
        uri,
        headers,
        body,
    });
    let answer = match (method.as_str(), path.as_str()) {
        ("GET", "/api/health") => json!({
            "status": "ok", "db_initialized": true, "version": "1.0.0",
            "data_root": "/Users/server/.wenlan", "data_root_identity": "dev-1",
        }),
        ("GET", "/api/spaces") => json!([
            {"id": "1", "name": "Alpha"}, {"id": "2", "name": "Beta"}, {"id": "3", "name": "Private"},
        ]),
        ("GET", "/api/pages") | ("POST", "/api/pages/search") => json!({
            "pages": [
                {"id": "a1", "space": pinned, "storage_path": "/srv/pages/a1.md",
                 "nested": {"device_id": "dev-1", "title": "kept"}},
                // A row of another Space, as a daemon bug could return.
                {"id": "b9", "space": "Beta", "title": "Beta secret"},
            ],
        }),
        ("GET", "/api/pages/recent") => json!([
            {"id": format!("r-{}", pinned.clone().unwrap_or_default()), "timestamp_ms": 5},
        ]),
        ("GET", "/api/pages/recent-changes") => json!([
            {"page_id": format!("c-{}", pinned.clone().unwrap_or_default()), "changed_at_ms": 5},
        ]),
        ("GET", _) if path.starts_with("/api/pages/") && path.matches('/').count() == 3 => {
            let id = path.trim_start_matches("/api/pages/");
            match space_of(id) {
                Some(space) if pinned.is_none() || pinned.as_deref() == Some(space) => {
                    json!({"page": {"id": id, "space": space, "storage_path": "/srv/x.md"}})
                }
                _ => return StatusCode::NOT_FOUND.into_response(),
            }
        }
        _ => json!({"ok": true, "space": pinned, "source_path": "/srv/file.txt"}),
    };
    Json(answer).into_response()
}

struct Fixture {
    _dir: tempfile::TempDir,
    state: TeamState,
    log: Log,
    alpha_writer: (Member, String),
    alpha_reader: (Member, String),
    beta_writer: (Member, String),
}

impl Fixture {
    fn seen(&self) -> Vec<Seen> {
        self.log.lock().unwrap().clone()
    }

    fn forget(&self) {
        self.log.lock().unwrap().clear();
    }
}

async fn fixture() -> Fixture {
    let log: Log = Arc::default();
    let daemon = axum::Router::new().fallback({
        let log = log.clone();
        move |req: Request| stand_in(log.clone(), req)
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, daemon).await });

    let dir = tempfile::tempdir().unwrap();
    let team_path = dir.path().join(team::TEAM_FILE);
    let settings_path = dir.path().join(shared_space::SETTINGS_FILE);
    for name in ["Alpha", "Beta"] {
        let folder = dir.path().join("team-spaces").join(name.to_lowercase());
        let scope = Scope::Space(name.into());
        shared_space::ensure_space_file(
            &folder,
            &scope,
            name,
            &SpaceMember {
                id: "server".into(),
                name: "Server".into(),
            },
        )
        .unwrap();
        shared_space::join_at(&settings_path, &scope, &folder, "Server").unwrap();
        shared_space::update_share(&settings_path, &scope, |share| share.hosted = true).unwrap();
    }
    let access = |space: &str, access| {
        vec![TeamSpaceAccess {
            space: space.into(),
            access,
            id: None,
        }]
    };
    let alpha_writer =
        team::add_member(&team_path, "Amy", &access("Alpha", TeamAccess::Write)).unwrap();
    let alpha_reader =
        team::add_member(&team_path, "Bo", &access("Alpha", TeamAccess::Read)).unwrap();
    let beta_writer =
        team::add_member(&team_path, "Cy", &access("Beta", TeamAccess::Write)).unwrap();
    team::set_url(&team_path, Some("https://team.example.com/")).unwrap();
    Fixture {
        state: TeamState {
            team_path,
            settings_path,
            daemon_url: format!("http://127.0.0.1:{port}"),
            mcp: Arc::default(),
        },
        _dir: dir,
        log,
        alpha_writer,
        alpha_reader,
        beta_writer,
    }
}

async fn call(
    state: &TeamState,
    method: &str,
    uri: &str,
    key: &str,
    body: Option<Value>,
    extra: &[(&str, &str)],
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(Method::from_bytes(method.as_bytes()).unwrap())
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {key}"));
    for (name, value) in extra {
        req = req.header(*name, *value);
    }
    let body = match body {
        Some(body) => {
            req = req.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(&body).unwrap())
        }
        None => Body::empty(),
    };
    let response = team_router(state.clone())
        .oneshot(req.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[derive(Clone, Copy)]
enum Who {
    AlphaWriter,
    AlphaReader,
    BetaWriter,
}

/// What a case expects: the status, and the Spaces the daemon was called
/// pinned to (none at all when refused).
struct Case {
    why: &'static str,
    who: Who,
    method: &'static str,
    path: &'static str,
    body: Option<Value>,
    status: StatusCode,
    pinned: &'static [&'static str],
}

fn case(
    why: &'static str,
    who: Who,
    method: &'static str,
    path: &'static str,
    body: Option<Value>,
    status: StatusCode,
    pinned: &'static [&'static str],
) -> Case {
    Case {
        why,
        who,
        method,
        path,
        body,
        status,
        pinned,
    }
}

/// Test 1: across Spaces, by path, by query field, by body field and by
/// page id, and a member who may only read cannot write.
#[tokio::test]
async fn a_member_reaches_only_their_own_spaces() {
    use Who::*;
    let not_found = StatusCode::NOT_FOUND;
    let forbidden = StatusCode::FORBIDDEN;
    let ok = StatusCode::OK;
    let f = fixture().await;
    let cases = vec![
        // By query field.
        case(
            "query space",
            AlphaWriter,
            "GET",
            "/api/pages?space=Beta",
            None,
            not_found,
            &[],
        ),
        case(
            "query domain",
            AlphaWriter,
            "GET",
            "/api/pages?domain=Beta",
            None,
            not_found,
            &[],
        ),
        case(
            "query both, differing",
            AlphaWriter,
            "GET",
            "/api/pages?space=Alpha&domain=Beta",
            None,
            not_found,
            &[],
        ),
        case(
            "query of a Space not hosted",
            AlphaWriter,
            "GET",
            "/api/pages?space=Private",
            None,
            not_found,
            &[],
        ),
        case(
            "query own Space",
            AlphaWriter,
            "GET",
            "/api/pages?space=Alpha",
            None,
            ok,
            &["Alpha"],
        ),
        case(
            "no query: the member's Spaces",
            AlphaWriter,
            "GET",
            "/api/pages",
            None,
            ok,
            &["Alpha"],
        ),
        case(
            "recent: the member's Spaces only",
            BetaWriter,
            "GET",
            "/api/pages/recent?space=Alpha",
            None,
            ok,
            &["Beta"],
        ),
        case(
            "recent changes: the member's Spaces only",
            BetaWriter,
            "GET",
            "/api/pages/recent-changes",
            None,
            ok,
            &["Beta"],
        ),
        // By body field.
        case(
            "search body space",
            AlphaWriter,
            "POST",
            "/api/pages/search",
            Some(json!({"query": "x", "space": "Beta"})),
            not_found,
            &[],
        ),
        case(
            "search body domain",
            AlphaWriter,
            "POST",
            "/api/pages/search",
            Some(json!({"query": "x", "domain": "Beta"})),
            not_found,
            &[],
        ),
        case(
            "search body, no Space",
            AlphaWriter,
            "POST",
            "/api/pages/search",
            Some(json!({"query": "x"})),
            ok,
            &["Alpha"],
        ),
        case(
            "draft in another Space",
            AlphaWriter,
            "POST",
            "/api/pages/drafts",
            Some(json!({"draft_id": "a-new", "title": "t", "content": "c", "space": "Beta"})),
            not_found,
            &[],
        ),
        case(
            "draft with no Space",
            AlphaWriter,
            "POST",
            "/api/pages/drafts",
            Some(json!({"draft_id": "a-new", "title": "t", "content": "c"})),
            ok,
            &["Alpha"],
        ),
        case(
            "draft with a null Space",
            AlphaWriter,
            "POST",
            "/api/pages/drafts",
            Some(json!({"draft_id": "a-new", "title": "t", "content": "c", "space": null})),
            not_found,
            &[],
        ),
        case(
            "no row: pasted text into another Space",
            AlphaWriter,
            "POST",
            "/api/import/memories",
            Some(json!({"source": "other", "content": "x", "space": "Beta"})),
            not_found,
            &[],
        ),
        case(
            "no row: pasted text, Space as a number",
            AlphaWriter,
            "POST",
            "/api/import/memories",
            Some(json!({"source": "other", "content": "x", "space": 7})),
            not_found,
            &[],
        ),
        case(
            "no row: pasted text into own Space",
            AlphaWriter,
            "POST",
            "/api/import/memories",
            Some(json!({"source": "other", "content": "x", "domain": "Alpha"})),
            not_found,
            &[],
        ),
        case(
            "no row: link into another Space",
            AlphaWriter,
            "POST",
            "/api/ingest/webpage",
            Some(json!({"url": "https://example.com", "domain": "Beta"})),
            not_found,
            &[],
        ),
        // By page id (the page's Space looked up; one lookup reaches the daemon).
        case(
            "read a page of another Space",
            BetaWriter,
            "GET",
            "/api/pages/a1",
            None,
            not_found,
            &[],
        ),
        case(
            "history of another Space's page",
            BetaWriter,
            "GET",
            "/api/pages/a1/revisions",
            None,
            not_found,
            &[],
        ),
        case(
            "sources of another Space's page",
            BetaWriter,
            "GET",
            "/api/pages/a1/sources",
            None,
            not_found,
            &[],
        ),
        case(
            "edit another Space's page",
            BetaWriter,
            "POST",
            "/api/memory/a1/update-page",
            Some(json!({"content": "x", "source_memory_ids": []})),
            not_found,
            &[],
        ),
        case(
            "publish another Space's draft",
            BetaWriter,
            "POST",
            "/api/pages/drafts/a1/publish",
            Some(json!({"expected_version": 1})),
            not_found,
            &[],
        ),
        case(
            "move a draft to another Space",
            AlphaWriter,
            "PUT",
            "/api/pages/drafts/a1",
            Some(json!({"expected_version": 1, "title": "t", "content": "c", "space": "Beta"})),
            not_found,
            &[],
        ),
        case(
            "a page with no Space",
            AlphaWriter,
            "GET",
            "/api/pages/x1",
            None,
            not_found,
            &[],
        ),
        case(
            "read own page",
            AlphaReader,
            "GET",
            "/api/pages/a1",
            None,
            ok,
            &["Alpha"],
        ),
        case(
            "edit own page",
            AlphaWriter,
            "POST",
            "/api/memory/a1/update-page",
            Some(json!({"content": "x", "source_memory_ids": []})),
            ok,
            &["Alpha"],
        ),
        case(
            "save own draft",
            AlphaWriter,
            "PUT",
            "/api/pages/drafts/a1",
            Some(json!({"expected_version": 1, "title": "t", "content": "c"})),
            ok,
            &["Alpha"],
        ),
        // A member who may only read.
        case(
            "reader edits",
            AlphaReader,
            "POST",
            "/api/memory/a1/update-page",
            Some(json!({"content": "x", "source_memory_ids": []})),
            forbidden,
            &[],
        ),
        case(
            "reader makes a draft",
            AlphaReader,
            "POST",
            "/api/pages/drafts",
            Some(json!({"draft_id": "a-new", "title": "t", "content": "c", "space": "Alpha"})),
            forbidden,
            &[],
        ),
        case(
            "reader saves a draft",
            AlphaReader,
            "PUT",
            "/api/pages/drafts/a1",
            Some(json!({"expected_version": 1, "title": "t", "content": "c"})),
            forbidden,
            &[],
        ),
        case(
            "reader publishes",
            AlphaReader,
            "POST",
            "/api/pages/drafts/a1/publish",
            Some(json!({"expected_version": 1})),
            forbidden,
            &[],
        ),
        case(
            "no row: reader pastes text",
            AlphaReader,
            "POST",
            "/api/import/memories",
            Some(json!({"source": "other", "content": "x"})),
            not_found,
            &[],
        ),
        case(
            "no row: reader saves a link",
            AlphaReader,
            "POST",
            "/api/ingest/webpage",
            Some(json!({"url": "https://example.com"})),
            not_found,
            &[],
        ),
        case(
            "no row: reader fetches a link",
            AlphaReader,
            "POST",
            "/api/webpage/fetch",
            Some(json!({"url": "https://example.com"})),
            not_found,
            &[],
        ),
        case(
            "reader searches",
            AlphaReader,
            "POST",
            "/api/pages/search",
            Some(json!({"query": "x"})),
            ok,
            &["Alpha"],
        ),
        // By path: not in the table, or not plain.
        case(
            "a call with no row",
            AlphaWriter,
            "GET",
            "/api/config",
            None,
            not_found,
            &[],
        ),
        case(
            "a dotted path",
            AlphaWriter,
            "GET",
            "/api/pages/..%2F..%2Fapi%2Fconfig",
            None,
            not_found,
            &[],
        ),
        case(
            "another method",
            AlphaWriter,
            "DELETE",
            "/api/pages/a1",
            None,
            not_found,
            &[],
        ),
    ];
    for case in cases {
        f.forget();
        let key = match case.who {
            Who::AlphaWriter => &f.alpha_writer.1,
            Who::AlphaReader => &f.alpha_reader.1,
            Who::BetaWriter => &f.beta_writer.1,
        };
        let uri = format!("{PREFIX}{}", case.path);
        let (status, answer) = call(&f.state, case.method, &uri, key, case.body.clone(), &[]).await;
        assert_eq!(status, case.status, "{}: {answer}", case.why);
        // The lookup of a page's Space is unpinned; every other call the
        // daemon saw is pinned to one of the expected Spaces.
        let pinned: Vec<String> = f
            .seen()
            .iter()
            .filter(|seen| {
                !(seen.method == "GET"
                    && seen.space().is_none()
                    && seen.uri.starts_with("/api/pages/"))
            })
            .map(|seen| seen.space().unwrap_or("(none)").to_string())
            .collect();
        assert_eq!(pinned, case.pinned, "{}", case.why);
        for seen in f.seen() {
            // Only the gate's own query parameters, never the client's Space.
            assert!(!seen.uri.contains("space="), "{}: {}", case.why, seen.uri);
            assert!(!seen.uri.contains("domain="), "{}: {}", case.why, seen.uri);
            if let Value::Object(body) = &seen.body {
                assert!(!body.contains_key("domain"), "{}", case.why);
                if let Some(space) = body.get("space") {
                    assert_eq!(space.as_str(), seen.space(), "{}", case.why);
                }
            }
        }
        let text = answer.to_string();
        for private in ["/srv/", "dev-1", "/Users/server", "Beta secret"] {
            assert!(!text.contains(private), "{}: {private} in {text}", case.why);
        }
    }
}

#[tokio::test]
async fn answers_keep_only_the_members_spaces() {
    let f = fixture().await;
    let key = &f.alpha_writer.1;
    let (_, spaces) = call(
        &f.state,
        "GET",
        &format!("{PREFIX}/api/spaces"),
        key,
        None,
        &[],
    )
    .await;
    assert_eq!(spaces, json!([{"id": "1", "name": "Alpha"}]));
    let (_, health) = call(
        &f.state,
        "GET",
        &format!("{PREFIX}/api/health"),
        key,
        None,
        &[],
    )
    .await;
    assert_eq!(
        health,
        json!({"status": "ok", "db_initialized": true, "version": "1.0.0"})
    );
    let (_, pages) = call(
        &f.state,
        "GET",
        &format!("{PREFIX}/api/pages"),
        key,
        None,
        &[],
    )
    .await;
    assert_eq!(
        pages,
        json!({"pages": [{"id": "a1", "space": "Alpha", "nested": {"title": "kept"}}]})
    );
}

/// The identity the daemon is told is the member the key belongs to, set by
/// the gate; a copy the browser sends is never what reaches the daemon.
#[tokio::test]
async fn a_forged_identity_is_replaced_by_the_real_member() {
    let f = fixture().await;
    let forged = [
        (crate::team_actor::SECRET_HEADER, "guessed"),
        (
            crate::team_actor::MEMBER_HEADER,
            f.beta_writer.0.id.as_str(),
        ),
        (crate::team_actor::MEMBER_NAME_HEADER, "Cy"),
        (crate::team_actor::KIND_HEADER, "ai"),
        ("x-agent-name", "claude@team.someone"),
        (crate::space_header::HEADER_NAME, "Beta"),
    ];
    let (status, _) = call(
        &f.state,
        "POST",
        &format!("{PREFIX}/api/memory/a1/update-page"),
        &f.alpha_writer.1,
        Some(json!({"content": "x", "source_memory_ids": []})),
        &forged,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let seen = f.seen();
    assert!(!seen.is_empty());
    for seen in seen {
        let named =
            crate::team_actor::from_headers(&seen.headers).expect("the gate names the member");
        assert_eq!(named.id, f.alpha_writer.0.id);
        assert_eq!(named.name, "Amy");
        assert!(seen.headers.get("x-agent-name").is_none());
        assert_ne!(seen.space(), Some("Beta"));
    }
}

/// Test 2: every route this daemon registers that has no row answers 404
/// through the gate, and nothing reaches the daemon.
#[tokio::test]
async fn every_daemon_route_without_a_row_is_not_found() {
    use wenlan_core::truth_manifest::{runtime_entries, Builder, ReaderMethod};
    let f = fixture().await;
    let owner_like = &f.alpha_writer.1;
    let mut checked = 0;
    let mut allowed = 0;
    for (builder, method, path) in runtime_entries() {
        if builder != Builder::Main {
            continue;
        }
        let method = match method {
            ReaderMethod::Get => "GET",
            ReaderMethod::Post => "POST",
            ReaderMethod::Put => "PUT",
            ReaderMethod::Delete => "DELETE",
            ReaderMethod::Patch => "PATCH",
        };
        let concrete: String = path
            .split('/')
            .map(|segment| {
                if segment.starts_with('{') {
                    "a1"
                } else {
                    segment
                }
            })
            .collect::<Vec<_>>()
            .join("/");
        let in_table = CALLS
            .iter()
            .any(|call| call.method == method && call.path == path)
            || find(method, &concrete).is_some_and(|(call, _)| {
                // Same row only when the registered pattern is the row's.
                call.path.split('/').count() == path.split('/').count()
                    && call
                        .path
                        .split('/')
                        .zip(path.split('/'))
                        .all(|(a, b)| a == b || (a == "{id}" && b.starts_with('{')))
            });
        if in_table {
            allowed += 1;
            continue;
        }
        f.forget();
        let body = (method != "GET").then(|| json!({}));
        let (status, _) = call(
            &f.state,
            method,
            &format!("{PREFIX}{concrete}"),
            owner_like,
            body,
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {path}");
        assert!(f.seen().is_empty(), "{method} {path} reached the daemon");
        checked += 1;
    }
    assert_eq!(
        allowed,
        CALLS.len(),
        "every row is a registered daemon route"
    );
    assert!(checked > 100, "the daemon's routes were read ({checked})");
}

/// Test 3: the gate answers a browser only from the team server's own
/// origin; no other origin works, even with a valid key. The connectors'
/// origins keep working where they did, and not on the gate.
#[tokio::test]
async fn only_the_team_servers_own_origin_calls_the_gate() {
    let f = fixture().await;
    let key = &f.alpha_writer.1;
    let gate = format!("{PREFIX}/api/pages/a1");
    let origin = |value| [(header::ORIGIN.as_str(), value)];
    for (value, status) in [
        ("https://team.example.com", StatusCode::OK),
        ("https://claude.ai", StatusCode::FORBIDDEN),
        ("https://chatgpt.com", StatusCode::FORBIDDEN),
        ("https://evil.example", StatusCode::FORBIDDEN),
        ("http://team.example.com", StatusCode::FORBIDDEN),
        ("https://team.example.com:8443", StatusCode::FORBIDDEN),
        ("null", StatusCode::FORBIDDEN),
    ] {
        let (got, _) = call(&f.state, "GET", &gate, key, None, &origin(value)).await;
        assert_eq!(got, status, "gate from {value}");
    }
    // No key: refused before the origin is read, as everywhere.
    let (got, _) = call(
        &f.state,
        "GET",
        &gate,
        "nope",
        None,
        &origin("https://team.example.com"),
    )
    .await;
    assert_eq!(got, StatusCode::UNAUTHORIZED);
    // Elsewhere: the team's own origin is not one the connectors' routes take.
    let (got, _) = call(
        &f.state,
        "GET",
        "/team/v1/me",
        key,
        None,
        &origin("https://team.example.com"),
    )
    .await;
    assert_eq!(got, StatusCode::FORBIDDEN);
    let (got, _) = call(
        &f.state,
        "GET",
        "/team/v1/me",
        key,
        None,
        &origin("https://claude.ai"),
    )
    .await;
    assert_eq!(got, StatusCode::OK);
    // A path that only starts like the gate's is not the gate.
    let (got, _) = call(
        &f.state,
        "GET",
        "/team/v1/apps",
        key,
        None,
        &origin("https://team.example.com"),
    )
    .await;
    assert_eq!(got, StatusCode::FORBIDDEN);
}
