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
        ("GET", "/api/spaces") => {
            let spaces = [("1", "Alpha"), ("2", "Beta"), ("3", "Private")];
            json!(spaces
                .into_iter()
                .filter(|(_, name)| pinned.as_deref() == Some(*name))
                .map(|(id, name)| json!({"id": id, "name": name}))
                .collect::<Vec<_>>())
        }
        ("GET", "/api/pages") | ("POST", "/api/pages/search") => json!({
            "pages": [
                {"id": "a1", "space": pinned, "title": "Alpha one"},
                // A row of another Space, as a daemon bug could return.
                {"id": "b9", "space": "Beta", "title": "Beta secret"},
            ],
        }),
        ("GET", "/api/memory/graph") => {
            let page = |id, title, space| {
                json!({
                    "id": id, "title": title, "space": space, "entity_id": "e1",
                    "creation_kind": "authored", "last_modified": "2026-01-01T00:00:00Z",
                    "storage_path": "/srv/a1.md", "local_note": "round4-private-graph-note"
                })
            };
            json!({
                "entities": [{"id": "e1", "name": "Beta secret"}],
                "relations": [], "memories": [], "memory_links": [],
                "pages": [page("a1", "Alpha one", "Alpha"), page("a2", "Alpha two", "Alpha"), page("b1", "Beta secret", "Beta")],
                "page_links": [
                    {"from": {"kind": "page", "id": "a1"}, "to": {"kind": "page", "id": "a2", "filename": "round4-private-graph-note"}, "link_type": "wikilink"},
                    {"from": {"kind": "page", "id": "a1"}, "to": {"kind": "page", "id": "b1", "title": "Beta secret"}, "link_type": "wikilink"},
                    {"from": {"kind": "page", "id": "a2"}, "to": {"kind": "entity", "id": "e1"}, "link_type": "about"},
                ],
            })
        }
        ("GET", "/api/knowledge/folders") => {
            let space = pinned.clone().unwrap_or_default();
            json!({"folders": [
                {"path": space, "parent_path": "", "name": space},
                {"path": format!("{space}/Q3"), "parent_path": space, "name": "Q3"},
                // A folder of another Space, as a daemon bug could return.
                {"path": "Beta/Beta secret", "parent_path": "Beta", "name": "Beta secret"},
            ], "truncated": false})
        }
        ("GET", "/api/pages/recent") => json!([
            {"id": format!("r-{}", pinned.clone().unwrap_or_default()), "timestamp_ms": 5},
        ]),
        ("GET", "/api/pages/recent-changes") => json!([
            {"page_id": format!("c-{}", pinned.clone().unwrap_or_default()), "changed_at_ms": 5},
        ]),
        // A route that answers for some other page than the one named, as a
        // list route under `/api/pages/` would.
        ("GET", "/api/pages/alias") => json!({"page": {"id": "a2", "space": "Alpha"}}),
        ("GET", _) if path.starts_with("/api/pages/") && path.matches('/').count() == 3 => {
            let id = path.trim_start_matches("/api/pages/");
            match space_of(id) {
                Some(space) if pinned.is_none() || pinned.as_deref() == Some(space) => {
                    json!({"page": {"id": id, "space": space}})
                }
                _ => return StatusCode::NOT_FOUND.into_response(),
            }
        }
        _ => json!({"ok": true, "space": pinned}),
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
            db: None,
            daemon_url: "http://127.0.0.1:1".into(),
            in_process_daemon: Some(crate::router::AppRouter::from_test_router(daemon)),
            mcp: Arc::default(),
            sessions: Arc::default(),
            auth: Arc::default(),
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
        case(
            "recent changes of one own Space",
            AlphaWriter,
            "GET",
            "/api/pages/recent-changes?space=Alpha",
            None,
            ok,
            &["Alpha"],
        ),
        case(
            "recent changes of another Space",
            AlphaWriter,
            "GET",
            "/api/pages/recent-changes?space=Beta",
            None,
            not_found,
            &[],
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
        // Putting back an earlier text is the same hand edit, with the text
        // from before a change; the Space still comes from the page.
        case(
            "put back another Space's page by id",
            BetaWriter,
            "POST",
            "/api/memory/a1/update-page",
            Some(
                json!({"content": "before", "source_memory_ids": [], "expected_version": 3, "caller_id": "wenlan-app", "operation_id": "op-put-back"}),
            ),
            not_found,
            &[],
        ),
        case(
            "reader puts back",
            AlphaReader,
            "POST",
            "/api/memory/a1/update-page",
            Some(
                json!({"content": "before", "source_memory_ids": [], "expected_version": 3, "caller_id": "wenlan-app", "operation_id": "op-put-back"}),
            ),
            forbidden,
            &[],
        ),
        case(
            "put back own page naming another Space in the body",
            AlphaWriter,
            "POST",
            "/api/memory/a1/update-page",
            Some(
                json!({"content": "before", "source_memory_ids": [], "expected_version": 3, "caller_id": "wenlan-app", "operation_id": "op-put-back", "space": "Beta"}),
            ),
            not_found,
            &[],
        ),
        case(
            "put back own page",
            AlphaWriter,
            "POST",
            "/api/memory/a1/update-page",
            Some(
                json!({"content": "before", "source_memory_ids": [], "expected_version": 3, "caller_id": "wenlan-app", "operation_id": "op-put-back"}),
            ),
            ok,
            &["Alpha"],
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
        // Archive, restore, basis, graph (team web controls).
        case(
            "archive own page",
            AlphaWriter,
            "POST",
            "/api/pages/a1/archive",
            None,
            ok,
            &["Alpha"],
        ),
        case(
            "archive another Space's page by id",
            AlphaWriter,
            "POST",
            "/api/pages/b1/archive",
            None,
            not_found,
            &[],
        ),
        case(
            "archive own page: a query Space is dropped, the page's own is pinned",
            AlphaWriter,
            "POST",
            "/api/pages/a1/archive?space=Beta",
            None,
            ok,
            &["Alpha"],
        ),
        case(
            "archive own page naming another Space in the body",
            AlphaWriter,
            "POST",
            "/api/pages/a1/archive",
            Some(json!({"space": "Beta"})),
            not_found,
            &[],
        ),
        case(
            "archive as a reader",
            AlphaReader,
            "POST",
            "/api/pages/a1/archive",
            None,
            forbidden,
            &[],
        ),
        case(
            "restore own page",
            AlphaWriter,
            "POST",
            "/api/pages/a1/restore",
            None,
            ok,
            &["Alpha"],
        ),
        case(
            "restore another Space's page by id",
            AlphaWriter,
            "POST",
            "/api/pages/b1/restore",
            None,
            not_found,
            &[],
        ),
        case(
            "restore own page: a query Space is dropped, the page's own is pinned",
            AlphaWriter,
            "POST",
            "/api/pages/a1/restore?domain=Beta",
            None,
            ok,
            &["Alpha"],
        ),
        case(
            "restore own page naming another Space in the body",
            AlphaWriter,
            "POST",
            "/api/pages/a1/restore",
            Some(json!({"domain": "Beta"})),
            not_found,
            &[],
        ),
        case(
            "restore as a reader",
            AlphaReader,
            "POST",
            "/api/pages/a1/restore",
            None,
            forbidden,
            &[],
        ),
        case(
            "restore a page that is not there",
            AlphaWriter,
            "POST",
            "/api/pages/zz/restore",
            None,
            not_found,
            &[],
        ),
        case(
            "delete for good stays the owner's",
            AlphaWriter,
            "DELETE",
            "/api/pages/a1?permanent=true",
            None,
            not_found,
            &[],
        ),
        case(
            "add a basis in the same Space",
            AlphaWriter,
            "PUT",
            "/api/pages/a1/basis/a2",
            Some(json!({"version": 1})),
            ok,
            &["Alpha"],
        ),
        case(
            "remove a basis in the same Space",
            AlphaWriter,
            "DELETE",
            "/api/pages/a1/basis/a2",
            None,
            ok,
            &["Alpha"],
        ),
        case(
            "add a basis from another Space",
            AlphaWriter,
            "PUT",
            "/api/pages/a1/basis/b1",
            Some(json!({"version": 1})),
            not_found,
            &[],
        ),
        case(
            "remove a basis from another Space",
            AlphaWriter,
            "DELETE",
            "/api/pages/a1/basis/b1",
            None,
            not_found,
            &[],
        ),
        case(
            "add a basis to another Space's page",
            AlphaWriter,
            "PUT",
            "/api/pages/b1/basis/a1",
            Some(json!({"version": 1})),
            not_found,
            &[],
        ),
        case(
            "add a basis that is not there",
            AlphaWriter,
            "PUT",
            "/api/pages/a1/basis/zz",
            Some(json!({"version": 1})),
            not_found,
            &[],
        ),
        case(
            "add a basis whose id answers for another page",
            AlphaWriter,
            "PUT",
            "/api/pages/a1/basis/alias",
            Some(json!({"version": 1})),
            not_found,
            &[],
        ),
        case(
            "add a basis naming another Space in the body",
            AlphaWriter,
            "PUT",
            "/api/pages/a1/basis/a2",
            Some(json!({"version": 1, "space": "Beta"})),
            not_found,
            &[],
        ),
        case(
            "add a basis: a query Space is dropped, the page's own is pinned",
            AlphaWriter,
            "PUT",
            "/api/pages/a1/basis/a2?space=Beta",
            Some(json!({"version": 1})),
            ok,
            &["Alpha"],
        ),
        case(
            "add a basis as a reader",
            AlphaReader,
            "PUT",
            "/api/pages/a1/basis/a2",
            Some(json!({"version": 1})),
            not_found,
            &[],
        ),
        case(
            "remove a basis as a reader",
            AlphaReader,
            "DELETE",
            "/api/pages/a1/basis/a2",
            None,
            not_found,
            &[],
        ),
        case(
            "the graph of a reader's Spaces",
            AlphaReader,
            "GET",
            "/api/memory/graph",
            None,
            ok,
            &["Alpha"],
        ),
        case(
            "the graph: a query Space is dropped, only the member's are read",
            AlphaWriter,
            "GET",
            "/api/memory/graph?space=Beta",
            None,
            ok,
            &["Alpha"],
        ),
        case(
            "lock has no row",
            AlphaWriter,
            "POST",
            "/api/pages/a1/lock",
            Some(json!({"locked": true})),
            not_found,
            &[],
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
        // Folders: a writer makes and moves inside their Space; a reader
        // only sees them; a page of another Space is not there.
        case(
            "folder list",
            AlphaReader,
            "GET",
            "/api/knowledge/folders",
            None,
            ok,
            &["Alpha"],
        ),
        case(
            "folder create by a writer",
            AlphaWriter,
            "POST",
            "/api/knowledge/folders",
            Some(json!({"space": "Alpha", "parent_path": "Alpha", "name": "Q3"})),
            ok,
            &["Alpha"],
        ),
        case(
            "folder create by a reader",
            AlphaReader,
            "POST",
            "/api/knowledge/folders",
            Some(json!({"space": "Alpha", "parent_path": "Alpha", "name": "Q3"})),
            forbidden,
            &[],
        ),
        case(
            "folder create in another Space",
            AlphaWriter,
            "POST",
            "/api/knowledge/folders",
            Some(json!({"space": "Beta", "parent_path": "Beta", "name": "Q3"})),
            not_found,
            &[],
        ),
        case(
            "page move by a writer",
            AlphaWriter,
            "POST",
            "/api/pages/a1/move",
            Some(
                json!({"expected_storage_path": "Alpha/a1.md", "folder_path": "Alpha/Q3", "operation_id": "op-1"}),
            ),
            ok,
            &["Alpha"],
        ),
        case(
            "page move by a reader",
            AlphaReader,
            "POST",
            "/api/pages/a1/move",
            Some(
                json!({"expected_storage_path": "Alpha/a1.md", "folder_path": "Alpha/Q3", "operation_id": "op-1"}),
            ),
            forbidden,
            &[],
        ),
        case(
            "page move of another Space's page",
            AlphaWriter,
            "POST",
            "/api/pages/b1/move",
            Some(
                json!({"expected_storage_path": "Beta/b1.md", "folder_path": "Beta", "operation_id": "op-1"}),
            ),
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

/// A member's folders come from each of their Spaces, each Space a top
/// folder, and nothing of a Space they were not given.
#[tokio::test]
async fn a_members_folders_are_their_spaces_folders_only() {
    let f = fixture().await;
    let (status, answer) = call(
        &f.state,
        "GET",
        &format!("{PREFIX}/api/knowledge/folders"),
        &f.alpha_writer.1,
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let paths: Vec<&str> = answer["folders"]
        .as_array()
        .unwrap()
        .iter()
        .map(|folder| folder["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, ["Alpha", "Alpha/Q3"]);
    assert!(!answer.to_string().contains("Beta"), "{answer}");
}

/// The graph keeps the member's pages and only the links between them: no
/// page, title, entity or link end of another Space reaches the browser.
#[tokio::test]
async fn the_graph_shows_only_the_members_pages_and_their_links() {
    let f = fixture().await;
    let (status, answer) = call(
        &f.state,
        "GET",
        &format!("{PREFIX}/api/memory/graph"),
        &f.alpha_writer.1,
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let ids: Vec<&str> = answer["pages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|page| page["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["a1", "a2"]);
    assert_eq!(
        answer["page_links"],
        json!([{"from": {"kind": "page", "id": "a1"}, "to": {"kind": "page", "id": "a2"}, "link_type": "wikilink"}])
    );
    for key in ["entities", "relations", "memories", "memory_links"] {
        assert_eq!(answer[key], json!([]), "{key}");
    }
    // No entity id: two pages sharing a subject is not told.
    for page in answer["pages"].as_array().unwrap() {
        assert_eq!(page["entity_id"], Value::Null, "{page}");
    }
    let text = answer.to_string();
    assert!(
        !text.contains("b1")
            && !text.contains("Beta")
            && !text.contains("/srv/")
            && !text.contains("round4-private-graph-note"),
        "{text}"
    );
    assert_eq!(
        f.seen()
            .iter()
            .map(|seen| seen.space().unwrap_or(""))
            .collect::<Vec<_>>(),
        ["Alpha"]
    );
}

/// The browser's copy of the table (`src/web/team.ts` `GATE_ROWS`) names
/// exactly these rows, so a control is never offered for a call the gate
/// refuses, nor hidden for one it allows.
#[test]
fn the_browser_mirror_names_exactly_the_rows() {
    const TEAM_TS: &str = include_str!("../../../src/web/team.ts");
    let start = TEAM_TS
        .find("const GATE_ROWS")
        .expect("GATE_ROWS in team.ts");
    let end = start + TEAM_TS[start..].find("];").unwrap();
    let mut mirror: Vec<(String, String)> = TEAM_TS[start..end]
        .lines()
        .filter_map(|line| {
            let mut parts = line.trim().trim_start_matches('[').split('"');
            let method = parts.nth(1)?.to_string();
            let path = parts.nth(1)?.to_string();
            Some((method, path))
        })
        .collect();
    let mut rows: Vec<(String, String)> = CALLS
        .iter()
        .map(|call| (call.method.to_string(), call.path.to_string()))
        .collect();
    mirror.sort();
    rows.sort();
    assert_eq!(mirror, rows);
}

/// A member's entry for a Space the server does not host for the team
/// opens nothing, even when the daemon has a Space of that name (the
/// owner's own `Private`).
#[tokio::test]
async fn a_space_that_is_not_hosted_is_not_opened() {
    let f = fixture().await;
    let (_, key) = team::add_member(
        &f.state.team_path,
        "Dee",
        &[TeamSpaceAccess {
            space: "Private".into(),
            access: TeamAccess::Write,
            id: None,
        }],
    )
    .unwrap();
    for (method, uri, body) in [
        ("GET", "/team/v1/app/api/pages?space=Private", None),
        ("GET", "/team/v1/app/api/pages", None),
        ("GET", "/team/v1/app/api/pages/recent", None),
        (
            "POST",
            "/team/v1/app/api/pages/search",
            Some(json!({"query": "x", "space": "Private"})),
        ),
        (
            "POST",
            "/team/v1/app/api/pages/drafts",
            Some(json!({"space": "Private", "title": "t", "content": "c"})),
        ),
    ] {
        f.forget();
        let (status, answer) = call(&f.state, method, uri, &key, body, &[]).await;
        assert!(
            f.seen().iter().all(|seen| seen.space() != Some("Private")),
            "{method} {uri}: {status} {answer}"
        );
        assert!(
            !answer.to_string().contains("Private"),
            "{method} {uri}: {answer}"
        );
    }
    f.forget();
    let (status, answer) = call(&f.state, "GET", "/team/v1/app/api/spaces", &key, None, &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(answer, json!([]));
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
        json!({"pages": [{"id": "a1", "space": "Alpha", "title": "Alpha one"}]})
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
                        .all(|(a, b)| a == b || (a.starts_with('{') && b.starts_with('{')))
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
    // Reached at an address the team has not saved (a LAN address): the
    // page's origin is the address the request was sent to, so it works;
    // another site's page against the same Host does not.
    for (value, host, status) in [
        (
            "http://192.168.1.20:7879",
            "192.168.1.20:7879",
            StatusCode::OK,
        ),
        (
            "http://192.168.1.20:7879",
            "192.168.1.20:7880",
            StatusCode::FORBIDDEN,
        ),
        (
            "https://evil.example",
            "192.168.1.20:7879",
            StatusCode::FORBIDDEN,
        ),
        (
            "http://evil.example",
            "team.example.com",
            StatusCode::FORBIDDEN,
        ),
        ("null", "null", StatusCode::FORBIDDEN),
    ] {
        let extra = [
            (header::ORIGIN.as_str(), value),
            (header::HOST.as_str(), host),
        ];
        let (got, _) = call(&f.state, "GET", &gate, key, None, &extra).await;
        assert_eq!(got, status, "gate from {value} to {host}");
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

/// A member's numbers reach the daemon held to a size it answers cheaply:
/// no allocation sized by a huge `limit`, no SQLite `LIMIT -1`.
#[tokio::test]
async fn a_members_limits_reach_the_daemon_bounded() {
    let f = fixture().await;
    let key = &f.alpha_writer.1;
    let seen_uri = |f: &Fixture| f.seen().last().unwrap().uri.clone();

    let (status, _) = call(
        &f.state,
        "GET",
        &format!("{PREFIX}/api/pages?space=Alpha&limit=1000000000000&offset=-5"),
        key,
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let uri = seen_uri(&f);
    assert!(uri.contains("limit=1000"), "{uri}");
    assert!(!uri.contains("limit=1000000000000"), "{uri}");
    assert!(uri.contains("offset=0"), "{uri}");

    f.forget();
    let (status, _) = call(
        &f.state,
        "GET",
        &format!("{PREFIX}/api/pages/recent?limit=-1&since_ms=-9"),
        key,
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let uri = seen_uri(&f);
    assert!(
        uri.contains("limit=1") && !uri.contains("limit=-1"),
        "{uri}"
    );
    assert!(uri.contains("since_ms=0"), "{uri}");

    f.forget();
    let (status, _) = call(
        &f.state,
        "GET",
        &format!("{PREFIX}/api/pages?space=Alpha&limit=99999999999999999999999&offset=x"),
        key,
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let uri = seen_uri(&f);
    assert!(!uri.contains("limit") && !uri.contains("offset"), "{uri}");

    f.forget();
    let (status, _) = call(
        &f.state,
        "POST",
        &format!("{PREFIX}/api/pages/search"),
        key,
        Some(json!({"query": "bridges", "space": "Alpha", "limit": 1_000_000_000_000_i64, "offset": -3})),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let body = f.seen().last().unwrap().body.clone();
    assert_eq!(body["limit"], json!(1000));
    assert_eq!(body["offset"], json!(0));
}

#[test]
fn bounded_numbers() {
    assert_eq!(bounded("limit", "0").as_deref(), Some("1"));
    assert_eq!(bounded("limit", "25").as_deref(), Some("25"));
    assert_eq!(bounded("limit", "5000").as_deref(), Some("1000"));
    assert_eq!(bounded("offset", "200000").as_deref(), Some("100000"));
    assert_eq!(bounded("since_ms", "-1").as_deref(), Some("0"));
    assert_eq!(bounded("limit", "ten"), None);
    assert_eq!(bounded("status", "active").as_deref(), Some("active"));
}

/// Every route the daemon's router binds (the handler manifest, not only
/// the page readers) is closed to a member unless it is a row, whatever its
/// method: a route added later is closed until someone writes its row.
#[tokio::test]
async fn every_bound_daemon_route_without_a_row_is_not_found() {
    const MANIFEST: &str = include_str!("route_registry/handler_manifest.txt");
    let f = fixture().await;
    let key = &f.alpha_writer.1;
    let routes: Vec<(String, &str)> = MANIFEST
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('|');
            (parts.next()? == "main")
                .then(|| Some((parts.next()?.to_ascii_uppercase(), parts.next()?)))
                .flatten()
        })
        .collect();
    for wanted in ["GET", "POST", "DELETE"] {
        assert!(
            routes
                .iter()
                .any(|(m, p)| m == wanted && *p == "/api/team/membership"),
            "{wanted} /api/team/membership is in the manifest"
        );
    }
    let mut checked = 0;
    let mut allowed = 0;
    for (method, path) in &routes {
        if CALLS
            .iter()
            .any(|call| call.method == method && call.path == *path)
        {
            allowed += 1;
            continue;
        }
        let concrete: String = path
            .split('/')
            .map(|s| if s.starts_with('{') { "a1" } else { s })
            .collect::<Vec<_>>()
            .join("/");
        f.forget();
        let body = (method != "GET").then(|| json!({}));
        let (status, _) = call(
            &f.state,
            method,
            &format!("{PREFIX}{concrete}"),
            key,
            body,
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {path}");
        assert!(f.seen().is_empty(), "{method} {path} reached the daemon");
        checked += 1;
    }
    assert_eq!(allowed, CALLS.len(), "every row is a bound daemon route");
    assert!(checked > 300, "the daemon's routes were read ({checked})");
}

/// A folder answer keeps the folder's `path` (already the member's) and
/// still loses every other private key.
#[test]
fn a_folder_answer_keeps_only_the_folder_path() {
    let list = json!({"folders": [
        {"path": "Alpha/Q3", "parent_path": "Alpha", "name": "Q3", "absolute_path": "/srv/pages/a/Q3"},
    ], "knowledge_path": "/srv/pages", "truncated": false});
    assert_eq!(
        project_answer::<KnowledgeFoldersResponse>(list),
        json!({"folders": [{"path": "Alpha/Q3", "parent_path": "Alpha", "name": "Q3"}], "truncated": false})
    );
    let made = json!({"path": "Alpha/Q3", "nested": {"path": "/srv/pages/a/Q3"}});
    assert_eq!(
        project_answer::<CreateKnowledgeFolderResponse>(made),
        json!({"path": "Alpha/Q3"})
    );
}

/// End to end: the real gate in front of a real daemon with real files. A
/// writer sees their Space as a top folder, makes a folder in it and moves
/// a page there by the paths they see; no server path reaches them, and
/// another member's folder appears for them too (folders are the Space's).
#[tokio::test]
async fn a_member_uses_folders_through_the_gate_and_a_real_daemon() {
    const SPACE: &str = "產品:A";
    const FOLDER: &str = "產品-A";
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(
        wenlan_core::db::MemoryDB::new(dir.path(), Arc::new(wenlan_core::events::NoopEmitter))
            .await
            .unwrap(),
    );
    db.create_space(SPACE, None, false).await.unwrap();
    db.create_space("Private", None, false).await.unwrap();
    let root = dir.path().join("pages");
    std::fs::create_dir_all(root.join(FOLDER)).unwrap();
    std::fs::create_dir_all(root.join("Private").join("Secret plans")).unwrap();
    wenlan_core::space_folders::turn_on(&root).unwrap();
    let id = "page_00000000-0000-4000-8000-00000000e2e1";
    let draft = db
        .create_page_draft_with_id_in_registered_space_and_folder(
            id,
            "路線",
            "Body",
            Some(SPACE),
            FOLDER,
        )
        .await
        .unwrap();
    let wenlan_core::pages::PageDraftPublishOutcome::Published(page) =
        db.publish_page_draft(id, draft.version).await.unwrap()
    else {
        panic!("publish")
    };
    let absolute = wenlan_core::export::knowledge::KnowledgeProjectionWrite::new(root.clone(), &db)
        .write_page_gated(&db, &page)
        .await
        .unwrap()
        .unwrap();
    let file = std::path::Path::new(&absolute)
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    let daemon_state = Arc::new(tokio::sync::RwLock::new(crate::state::ServerState {
        db: Some(db.clone()),
        lint_config: crate::state::LintServerConfig::new(vec![], Some(root.clone())),
        ..Default::default()
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let daemon = crate::router::build_router(daemon_state);
    tokio::spawn(async move { axum::serve(listener, daemon.into_make_service()).await });

    let team_dir = dir.path().join("team");
    std::fs::create_dir_all(&team_dir).unwrap();
    let team_path = team_dir.join(team::TEAM_FILE);
    let settings_path = team_dir.join(shared_space::SETTINGS_FILE);
    let folder = team_dir.join("team-spaces").join("a");
    let scope = Scope::Space(SPACE.into());
    shared_space::ensure_space_file(
        &folder,
        &scope,
        SPACE,
        &SpaceMember {
            id: "server".into(),
            name: "Server".into(),
        },
    )
    .unwrap();
    shared_space::join_at(&settings_path, &scope, &folder, "Server").unwrap();
    shared_space::update_share(&settings_path, &scope, |share| share.hosted = true).unwrap();
    let grant = |access| {
        vec![TeamSpaceAccess {
            space: SPACE.into(),
            access,
            id: None,
        }]
    };
    let (_, writer) = team::add_member(&team_path, "王小明", &grant(TeamAccess::Write)).unwrap();
    let (_, reader) = team::add_member(&team_path, "Bo", &grant(TeamAccess::Read)).unwrap();
    team::set_url(&team_path, Some("https://team.example.com/")).unwrap();
    let state = TeamState {
        team_path,
        settings_path,
        db: None,
        daemon_url: format!("http://127.0.0.1:{port}"),
        in_process_daemon: None,
        mcp: Arc::default(),
        sessions: Arc::default(),
        auth: Arc::default(),
    };
    let folders = |answer: &Value| -> Vec<String> {
        answer["folders"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["path"].as_str().unwrap().to_string())
            .collect()
    };
    let no_server_path = |answer: &Value| {
        let text = answer.to_string();
        assert!(!text.contains(FOLDER), "{text}");
        assert!(!text.contains("Private"), "{text}");
        assert!(!text.contains(&dir.path().display().to_string()), "{text}");
    };

    let (status, answer) = call(
        &state,
        "GET",
        &format!("{PREFIX}/api/knowledge/folders"),
        &reader,
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(folders(&answer), [SPACE]);
    no_server_path(&answer);

    let (status, answer) = call(
        &state,
        "POST",
        &format!("{PREFIX}/api/knowledge/folders"),
        &reader,
        Some(json!({"space": SPACE, "parent_path": SPACE, "name": "Q3"})),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{answer}");

    let (status, answer) = call(
        &state,
        "POST",
        &format!("{PREFIX}/api/knowledge/folders"),
        &writer,
        Some(json!({"space": SPACE, "parent_path": SPACE, "name": "Q3"})),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["path"], format!("{SPACE}/Q3"));
    no_server_path(&answer);
    assert!(root.join(FOLDER).join("Q3").is_dir());

    // The reader sees the folder the writer made.
    let (_, answer) = call(
        &state,
        "GET",
        &format!("{PREFIX}/api/knowledge/folders"),
        &reader,
        None,
        &[],
    )
    .await;
    assert_eq!(folders(&answer), [SPACE.to_string(), format!("{SPACE}/Q3")]);

    // The page list gives the member the page's place as they see it.
    let (status, answer) = call(
        &state,
        "GET",
        &format!("{PREFIX}/api/pages?space={}", urlencoding(SPACE)),
        &writer,
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["pages"][0]["space_path"], format!("{SPACE}/{id}.md"));
    assert!(answer["pages"][0].get("storage_path").is_none());
    no_server_path(&answer);

    let (status, answer) = call(
        &state,
        "POST",
        &format!("{PREFIX}/api/pages/{id}/move"),
        &writer,
        Some(json!({
            "expected_storage_path": format!("{SPACE}/{id}.md"),
            "folder_path": format!("{SPACE}/Q3"),
            "operation_id": "00000000-0000-4000-8000-00000000e2e2",
        })),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["space_path"], format!("{SPACE}/Q3/{id}.md"));
    no_server_path(&answer);
    assert!(root.join(FOLDER).join("Q3").join(&file).is_file());

    // A target outside the Space is refused and nothing moves.
    let (status, _) = call(
        &state,
        "POST",
        &format!("{PREFIX}/api/pages/{id}/move"),
        &writer,
        Some(json!({
            "expected_storage_path": format!("{SPACE}/Q3/{id}.md"),
            "folder_path": "Private/Secret plans",
            "operation_id": "00000000-0000-4000-8000-00000000e2e3",
        })),
        &[],
    )
    .await;
    assert!(status.is_client_error(), "{status}");
    assert!(root.join(FOLDER).join("Q3").join(&file).is_file());
    assert_eq!(
        std::fs::read_dir(root.join("Private").join("Secret plans"))
            .unwrap()
            .count(),
        0
    );
}

fn urlencoding(text: &str) -> String {
    text.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
