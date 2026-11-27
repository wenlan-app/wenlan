// SPDX-License-Identifier: Apache-2.0

use super::*;
use axum::body::Body;
use axum::extract::Request;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tower::ServiceExt;
use wenlan_types::requests::{CALLER_HEADER, PERSON_CALLER_VALUE, PERSON_KEY_HEADER};
use wenlan_types::team::{TeamAccess, TeamInfo, TeamMe, TeamPerson, TeamRole, TeamSpaceAccess};
use wenlan_types::team_library::{
    JoinedTeamPage, TeamAuthorship, TeamCitation, TeamGroup, TeamItem, TeamItemKind,
    TeamLibraryInfo, TeamPageSources, TeamPages,
};

const MEMBER_KEY: &str = "member-key-for-scratch-team";
const TEAM_ID: &str = "team-acme";
const PAGE_ID: &str = "page-acme-1";

struct DataDirGuard {
    previous: Option<std::ffi::OsString>,
    _dir: tempfile::TempDir,
}

impl DataDirGuard {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let previous = std::env::var_os("WENLAN_DATA_DIR");
        std::env::set_var("WENLAN_DATA_DIR", dir.path());
        std::env::set_var("WENLAN_NO_AUTOSTART", "1");
        Self {
            previous,
            _dir: dir,
        }
    }
}

impl Drop for DataDirGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => std::env::set_var("WENLAN_DATA_DIR", value),
            None => std::env::remove_var("WENLAN_DATA_DIR"),
        }
    }
}

#[derive(Clone)]
struct FakeTeam {
    team: TeamInfo,
    space: TeamGroup,
    page: TeamItem,
    extra_pages: Vec<TeamItem>,
    many_pages: bool,
    slow_library: bool,
    requests: Arc<Mutex<Vec<(String, String)>>>,
}

impl FakeTeam {
    fn new() -> Self {
        let team = TeamInfo {
            id: TEAM_ID.into(),
            name: Some("Acme".into()),
        };
        let person = TeamPerson {
            id: "member-owner".into(),
            name: "Qi-Xuan".into(),
            owner: true,
        };
        Self {
            team: team.clone(),
            space: TeamGroup {
                id: "space-planning".into(),
                name: "Planning".into(),
                restricted: false,
            },
            page: TeamItem {
                team_id: team.id,
                item_id: PAGE_ID.into(),
                kind: TeamItemKind::Page,
                revision: 1,
                space_id: Some("space-planning".into()),
                title: "Launch plan".into(),
                body: "The launch is Friday.".into(),
                contributor: Some(person.clone()),
                authorship: Some(TeamAuthorship::Human),
                publisher: Some(person),
                last_editor: None,
                published_at: Some("2026-11-26T12:00:00Z".into()),
            },
            extra_pages: Vec::new(),
            many_pages: false,
            slow_library: false,
            requests: Arc::default(),
        }
    }

    fn record(&self, request: &Request) {
        self.requests.lock().unwrap().push((
            request.uri().path().to_string(),
            request
                .headers()
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string(),
        ));
    }
}

async fn fake_team(
    axum::extract::State(fake): axum::extract::State<FakeTeam>,
    request: Request,
) -> Response {
    fake.record(&request);
    if request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        != Some(format!("Bearer {MEMBER_KEY}").as_str())
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let path = request.uri().path();
    if fake.slow_library && path.starts_with("/team/v1/library") {
        tokio::time::sleep(std::time::Duration::from_secs(4)).await;
    }
    let response = if path == "/team/v1/me" {
        Json(TeamMe {
            member_id: "member-local".into(),
            name: "Member".into(),
            spaces: Vec::new(),
            library_spaces: vec![TeamSpaceAccess {
                space: fake.space.name.clone(),
                access: TeamAccess::Read,
                id: Some(fake.space.id.clone()),
            }],
            team: Some(fake.team.clone()),
            role: TeamRole::Member,
            signed_in_with: None,
            accounts: Vec::new(),
        })
        .into_response()
    } else if path == "/team/v1/library/pages" {
        let offset = request
            .uri()
            .query()
            .and_then(|query| {
                query
                    .split('&')
                    .find_map(|part| part.strip_prefix("offset="))
            })
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0);
        let pages = if fake.many_pages {
            (offset..offset.saturating_add(200).min(201))
                .map(|index| {
                    let mut page = fake.page.clone();
                    page.item_id = format!("page-{index}");
                    page.title = format!("Page {index}");
                    page
                })
                .collect()
        } else {
            let mut pages = vec![fake.page.clone()];
            pages.extend(fake.extra_pages.clone());
            pages
        };
        Json(TeamPages { pages }).into_response()
    } else if path == "/team/v1/library" {
        Json(TeamLibraryInfo {
            team: fake.team.clone(),
            spaces: vec![fake.space.clone()],
            team_memories: true,
        })
        .into_response()
    } else if path.ends_with(&format!("/{PAGE_ID}")) {
        Json(fake.page.clone()).into_response()
    } else if path.ends_with(&format!("/{PAGE_ID}/sources")) {
        Json(TeamPageSources {
            page_id: PAGE_ID.into(),
            sources: vec![
                TeamCitation::Memory {
                    memory: Box::new(TeamItem {
                        team_id: TEAM_ID.into(),
                        item_id: "shared-memory".into(),
                        kind: TeamItemKind::Memory,
                        revision: 1,
                        space_id: Some("space-planning".into()),
                        title: "Shared source".into(),
                        body: "Shared source text.".into(),
                        contributor: None,
                        authorship: Some(TeamAuthorship::Human),
                        publisher: None,
                        last_editor: None,
                        published_at: Some("2026-11-26T12:00:00Z".into()),
                    }),
                },
                TeamCitation::PrivateSource {
                    label: "SENTINEL-SECRET-7731".into(),
                },
            ],
        })
        .into_response()
    } else {
        StatusCode::NOT_FOUND.into_response()
    };
    response
}

async fn fake_server_with(
    configure: impl FnOnce(&mut FakeTeam),
) -> (String, FakeTeam, tokio::task::JoinHandle<()>) {
    let mut fake = FakeTeam::new();
    configure(&mut fake);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new().fallback(fake_team).with_state(fake.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (url, fake, task)
}

async fn fake_server() -> (String, FakeTeam, tokio::task::JoinHandle<()>) {
    fake_server_with(|_| {}).await
}

fn remember(url: &str, team: TeamInfo) {
    shared_space::remember_membership(
        &shared_space::settings_path(),
        url,
        MEMBER_KEY,
        &TeamMe {
            member_id: "member-local".into(),
            name: "Member".into(),
            spaces: Vec::new(),
            library_spaces: Vec::new(),
            team: Some(team),
            role: TeamRole::Member,
            signed_in_with: None,
            accounts: Vec::new(),
        },
    )
    .unwrap();
}

async fn daemon_get(uri: &str, headers: HeaderMap) -> (StatusCode, HeaderMap, Value) {
    let state = Arc::new(tokio::sync::RwLock::new(
        crate::state::ServerState::default(),
    ));
    let app = crate::router::build_router(state);
    let mut request = Request::builder().method("GET").uri(uri);
    for (name, value) in &headers {
        request = request.header(name, value);
    }
    let response = app
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let response_headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), 4 << 20)
        .await
        .unwrap();
    (
        status,
        response_headers,
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into())),
    )
}

fn person_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(CALLER_HEADER, PERSON_CALLER_VALUE.parse().unwrap());
    headers.insert(
        PERSON_KEY_HEADER,
        crate::person_key::test_key().parse().unwrap(),
    );
    headers
}

async fn test_lock() -> tokio::sync::MutexGuard<'static, ()> {
    crate::TEST_DATA_DIR_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await
}

#[tokio::test]
async fn joined_team_library_lists_pages_and_forwards_the_key_only_to_that_team() {
    let _lock = test_lock().await;
    let _env = DataDirGuard::new();
    let (url, fake, task) = fake_server().await;
    remember(&url, fake.team.clone());

    let (status, response_headers, body) =
        daemon_get("/api/team/joined/library", person_headers()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(response_headers[header::CACHE_CONTROL], "no-store");
    assert_eq!(body["teams"][0]["team"]["id"], TEAM_ID);
    assert_eq!(body["teams"][0]["team"]["name"], "Acme");
    assert_eq!(body["teams"][0]["library"]["state"], "ready", "{body}");
    assert_eq!(
        body["teams"][0]["library"]["spaces"][0]["space"]["name"],
        "Planning"
    );
    assert_eq!(
        body["teams"][0]["library"]["spaces"][0]["pages"][0]["id"],
        PAGE_ID
    );
    assert_eq!(
        body["teams"][0]["library"]["spaces"][0]["pages"][0]["published_by"]["name"],
        "Qi-Xuan"
    );
    assert_eq!(
        body["teams"][0]["library"]["spaces"][0]["pages"][0]["published_at"],
        "2026-11-26T12:00:00Z"
    );

    let requests = fake.requests.lock().unwrap().clone();
    assert!(requests
        .iter()
        .all(|(_, authorization)| authorization == &format!("Bearer {MEMBER_KEY}")));
    assert!(requests
        .iter()
        .all(|(path, _)| path.starts_with("/team/v1/library")));
    task.abort();
}

#[tokio::test]
async fn joined_team_library_lists_whole_team_pages_before_spaces() {
    let _lock = test_lock().await;
    let _env = DataDirGuard::new();
    let (url, fake, task) = fake_server_with(|fake| {
        let mut whole_team = fake.page.clone();
        whole_team.item_id = "page-whole-team".into();
        whole_team.title = "Team-wide announcement".into();
        whole_team.space_id = None;
        fake.extra_pages.push(whole_team);
    })
    .await;
    remember(&url, fake.team.clone());

    let (status, _, body) = daemon_get("/api/team/joined/library", person_headers()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["teams"][0]["library"]["team_pages"][0]["id"],
        "page-whole-team"
    );
    assert_eq!(
        body["teams"][0]["library"]["team_pages"][0]["space"],
        Value::Null
    );
    task.abort();
}

#[tokio::test]
async fn joined_team_page_reads_shared_source_and_redacts_private_source_text() {
    let _lock = test_lock().await;
    let _env = DataDirGuard::new();
    let (url, fake, task) = fake_server().await;
    remember(&url, fake.team.clone());

    let (status, response_headers, body) = daemon_get(
        &format!("/api/team/joined/{TEAM_ID}/pages/{PAGE_ID}"),
        person_headers(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(response_headers[header::CACHE_CONTROL], "no-store");
    assert!(!body.to_string().contains("SENTINEL-SECRET-7731"));
    let page: JoinedTeamPage = serde_json::from_value(body).unwrap();
    let JoinedTeamPage::Ready { page, sources } = page else {
        panic!("expected a ready page");
    };
    assert!(page.read_only);
    assert_eq!(page.body, "The launch is Friday.");
    assert_eq!(page.published_at.as_deref(), Some("2026-11-26T12:00:00Z"));
    assert_eq!(sources.sources.len(), 2);
    let json = serde_json::to_value(sources).unwrap();
    assert_eq!(json["sources"][0]["memory"]["body"], "Shared source text.");
    assert_eq!(json["sources"][1]["kind"], "private_source");
    assert!(json["sources"][1].get("memory").is_none());
    task.abort();
}

#[tokio::test]
async fn joined_team_library_marks_an_unreachable_team_without_waiting_for_the_long_timeout() {
    let _lock = test_lock().await;
    let _env = DataDirGuard::new();
    let unreachable = "http://127.0.0.1:9";
    remember(
        unreachable,
        TeamInfo {
            id: "team-offline".into(),
            name: Some("Offline team".into()),
        },
    );
    let started = Instant::now();
    let (status, _, body) = daemon_get("/api/team/joined/library", person_headers()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    assert!(body["teams"].as_array().unwrap().iter().any(|team| {
        team["team"]["id"] == "team-offline" && team["library"]["state"] == "unreachable"
    }));
}

#[tokio::test]
async fn joined_team_library_marks_a_nonresponsive_team_unreachable_at_the_deadline() {
    let _lock = test_lock().await;
    let _env = DataDirGuard::new();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let _socket = socket;
                std::future::pending::<()>().await;
            });
        }
    });
    remember(
        &url,
        TeamInfo {
            id: "team-hanging".into(),
            name: Some("Hanging team".into()),
        },
    );

    let started = Instant::now();
    let (status, _, body) = daemon_get("/api/team/joined/library", person_headers()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(8),
        "route exceeded its deadline: {:?}",
        started.elapsed()
    );
    assert!(body["teams"].as_array().unwrap().iter().any(|team| {
        team["team"]["id"] == "team-hanging" && team["library"]["state"] == "unreachable"
    }));
    task.abort();
}

#[tokio::test]
async fn joined_team_library_fetches_slow_independent_calls_concurrently() {
    let _lock = test_lock().await;
    let _env = DataDirGuard::new();
    let (url, fake, task) = fake_server_with(|fake| fake.slow_library = true).await;
    remember(&url, fake.team.clone());

    let started = Instant::now();
    let (status, _, body) = daemon_get("/api/team/joined/library", person_headers()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(6),
        "took {:?}",
        started.elapsed()
    );
    assert_eq!(body["teams"][0]["library"]["state"], "ready");
    task.abort();
}

#[tokio::test]
async fn joined_team_library_paginates_past_two_hundred_pages() {
    let _lock = test_lock().await;
    let _env = DataDirGuard::new();
    let (url, fake, task) = fake_server_with(|fake| fake.many_pages = true).await;
    remember(&url, fake.team.clone());

    let (status, _, body) = daemon_get("/api/team/joined/library", person_headers()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["teams"][0]["library"]["spaces"][0]["pages"]
            .as_array()
            .unwrap()
            .len(),
        201
    );
    assert!(body.to_string().contains("page-200"), "{body}");
    assert_eq!(body["teams"][0]["library"]["truncated"], false);
    task.abort();
}

#[tokio::test]
async fn joined_team_library_requires_the_person_marker_and_a_valid_person_key() {
    let _lock = test_lock().await;
    let _env = DataDirGuard::new();
    let (url, fake, task) = fake_server().await;
    remember(&url, fake.team.clone());

    let mut ai = person_headers();
    ai.insert("x-agent-name", "claude-code".parse().unwrap());
    let (status, response_headers, _) = daemon_get("/api/team/joined/library", ai).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(response_headers[header::CACHE_CONTROL], "no-store");

    let mut missing_key = HeaderMap::new();
    missing_key.insert(CALLER_HEADER, PERSON_CALLER_VALUE.parse().unwrap());
    let (status, _, _) = daemon_get("/api/team/joined/library", missing_key).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let previous = std::env::var_os("WENLAN_BIND_ADDR");
    for bind in ["0.0.0.0:7878", "not-a-bind"] {
        std::env::set_var("WENLAN_BIND_ADDR", bind);
        let (status, _, _) = daemon_get("/api/team/joined/library", person_headers()).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "bind={bind}");
        let (status, _, _) = daemon_get(
            &format!("/api/team/joined/{TEAM_ID}/pages/{PAGE_ID}"),
            person_headers(),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "bind={bind}");
    }
    match previous {
        Some(value) => std::env::set_var("WENLAN_BIND_ADDR", value),
        None => std::env::remove_var("WENLAN_BIND_ADDR"),
    }
    task.abort();
}
