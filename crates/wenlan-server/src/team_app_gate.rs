// SPDX-License-Identifier: AGPL-3.0-only
//! `/team/v1/app/<call>` — the calls a team member's browser makes to use
//! Wenlan on the web, behind the member key.
//!
//! Each call the browser may make is one row of [`CALLS`]: its method and
//! path, where its Space comes from, the access it needs, and what is taken
//! out of the answer. Anything else answers 404. An allowed call goes on to
//! this daemon over loopback (as `/team/v1/admin` does), pinned to the one
//! Space the gate checked, and carrying who is acting
//! ([`crate::team_actor`]), set here from the member and never from what
//! the browser sent.

use std::collections::BTreeMap;

#[cfg(test)]
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::Extension;
use serde_json::{Map, Value};
use wenlan_core::member_view::{self as view, MemberGraph};
use wenlan_core::shared_space::remote;
use wenlan_core::team::{self, Member};
use wenlan_types::team::TeamAccess;

use super::TeamState;
use crate::member_view::{self, MemberBody, MemberResponse};

/// Where the gate's calls start.
pub(super) const PREFIX: &str = "/team/v1/app";

/// Where a call's Space comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SpaceFrom {
    /// The call reads nothing of a Space.
    NoSpace,
    /// The query's `space` (or `domain`); without one, each of the member's
    /// Spaces in turn, the answers put together.
    QueryOrMember,
    /// The JSON body's `space` (or `domain`); without one, as
    /// [`SpaceFrom::QueryOrMember`].
    BodyOrMember,
    /// The JSON body's `space` (or `domain`); without one, the member's one
    /// Space, else refused.
    Body,
    /// The page (or draft) named in the path, its Space looked up here.
    Page,
    /// The two pages named in the path, both looked up here: the call goes
    /// on only when both are in the same Space. Any other pair answers as
    /// a missing page, so it says nothing about the second page.
    PagePair,
    /// Each of the member's Spaces in turn; a client filter is not read.
    MemberSpaces,
}

/// The access a call needs in its Space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Need {
    Read,
    Write,
}

/// The only wire shapes a projected response may take.
#[derive(Debug, Clone, Copy)]
enum View {
    Health,
    Spaces,
    Inventory,
    Activity,
    Changes,
    Search,
    Page,
    Revisions,
    Sources,
    Graph,
    ManualWrite,
    Draft,
    Removal,
    Restore,
    Basis,
    BasisRemoval,
    Folders,
    Folder,
    Move,
}

impl View {
    fn project(self, value: Value) -> MemberResponse {
        match self {
            Self::Health => project::<view::MemberHealth>(value),
            Self::Spaces => project::<Vec<view::MemberSpace>>(value),
            Self::Inventory => project::<view::MemberPages<view::MemberPageEntry>>(value),
            Self::Activity => project::<Vec<view::MemberActivity>>(value),
            Self::Changes => project::<Vec<view::MemberChange>>(value),
            Self::Search => project::<view::MemberPages<view::MemberPage>>(value),
            Self::Page => project::<view::MemberPageRead>(value),
            Self::Revisions => project::<view::MemberRevisions>(value),
            Self::Sources => project::<member_view::MemberSources>(value),
            Self::Graph => project::<view::MemberGraph>(value),
            Self::ManualWrite => project::<view::MemberManualWrite>(value),
            Self::Draft => project::<view::MemberDraft>(value),
            Self::Removal => project::<view::MemberRemoval>(value),
            Self::Restore => project::<view::MemberRestore>(value),
            Self::Basis => project::<view::MemberBasis>(value),
            Self::BasisRemoval => project::<member_view::MemberBasisRemoval>(value),
            Self::Folders => project::<member_view::MemberFolders>(value),
            Self::Folder => project::<member_view::MemberFolder>(value),
            Self::Move => project::<view::MemberMove>(value),
        }
    }
}

fn project<T: serde::de::DeserializeOwned + MemberBody>(value: Value) -> MemberResponse {
    match serde_json::from_value::<T>(value) {
        Ok(view) => MemberResponse::json(StatusCode::OK, view),
        Err(err) => {
            tracing::warn!("member gateway received a response outside its contract: {err}");
            MemberResponse::status(StatusCode::BAD_GATEWAY)
        }
    }
}

/// One call a member's browser may make.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Call {
    pub method: &'static str,
    /// The daemon path, a `{…}` segment for one plain segment (the page id
    /// first, then a second page's).
    pub path: &'static str,
    pub space: SpaceFrom,
    pub need: Need,
    output: View,
    /// The query parameters passed on; every other one is dropped.
    pub query: &'static [&'static str],
    /// The answer's page list, put together across Spaces, and the field of
    /// a row that names its Space (rows of another Space are dropped).
    pub rows: Rows,
}

/// How an answer holding many rows is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Rows {
    /// One thing, not a list.
    One,
    /// A list of the member's authorized Spaces.
    Spaces,
    /// `{ "pages": [...] }`, each page naming its `space`.
    Pages,
    /// A bare list, newest first by `field`.
    Newest(&'static str),
    /// The knowledge graph: its pages and the links between them, put
    /// together across Spaces. Nothing else of the graph is sent on.
    Graph,
    /// `{ "folders": [...] }`, each folder's path starting with its Space's
    /// name (as a member sees folders), put together across Spaces.
    Folders,
}

/// The calls. Owner-only calls (config, team admin, shared folders,
/// onboarding, telemetry, agents, models) have no row.
pub(crate) const CALLS: &[Call] = &[
    Call {
        method: "GET",
        path: "/api/health",
        space: SpaceFrom::NoSpace,
        need: Need::Read,
        output: View::Health,
        query: &[],
        rows: Rows::One,
    },
    Call {
        method: "GET",
        path: "/api/spaces",
        space: SpaceFrom::MemberSpaces,
        need: Need::Read,
        output: View::Spaces,
        query: &[],
        rows: Rows::Spaces,
    },
    // Home and a Space: the page list.
    Call {
        method: "GET",
        path: "/api/pages",
        space: SpaceFrom::QueryOrMember,
        need: Need::Read,
        output: View::Inventory,
        query: &["status", "limit", "offset"],
        rows: Rows::Pages,
    },
    Call {
        method: "GET",
        path: "/api/pages/recent",
        space: SpaceFrom::MemberSpaces,
        need: Need::Read,
        output: View::Activity,
        query: &["limit", "since_ms"],
        rows: Rows::Newest("timestamp_ms"),
    },
    Call {
        method: "GET",
        path: "/api/pages/recent-changes",
        // One Space's changes when the page asks for one (a Space's page,
        // and each Space on the home page, labels them with it).
        space: SpaceFrom::QueryOrMember,
        need: Need::Read,
        output: View::Changes,
        query: &["limit"],
        rows: Rows::Newest("changed_at_ms"),
    },
    // Search.
    Call {
        method: "POST",
        path: "/api/pages/search",
        space: SpaceFrom::BodyOrMember,
        need: Need::Read,
        output: View::Search,
        query: &[],
        rows: Rows::Pages,
    },
    // Reading a page, its history and its sources.
    Call {
        method: "GET",
        path: "/api/pages/{id}",
        space: SpaceFrom::Page,
        need: Need::Read,
        output: View::Page,
        query: &[],
        rows: Rows::One,
    },
    Call {
        method: "GET",
        path: "/api/pages/{id}/revisions",
        space: SpaceFrom::Page,
        need: Need::Read,
        output: View::Revisions,
        query: &["include"],
        rows: Rows::One,
    },
    Call {
        method: "GET",
        path: "/api/pages/{id}/sources",
        space: SpaceFrom::Page,
        need: Need::Read,
        output: View::Sources,
        query: &[],
        rows: Rows::One,
    },
    // The graph beside a page: pages and their links, of the member's
    // Spaces only.
    Call {
        method: "GET",
        path: "/api/memory/graph",
        space: SpaceFrom::MemberSpaces,
        need: Need::Read,
        output: View::Graph,
        query: &[],
        rows: Rows::Graph,
    },
    // Editing a page by hand.
    Call {
        method: "POST",
        path: "/api/memory/{id}/update-page",
        space: SpaceFrom::Page,
        need: Need::Write,
        output: View::ManualWrite,
        query: &[],
        rows: Rows::One,
    },
    // A new page: a draft, saved as it is typed, then published.
    Call {
        method: "POST",
        path: "/api/pages/drafts",
        space: SpaceFrom::Body,
        need: Need::Write,
        output: View::Draft,
        query: &[],
        rows: Rows::One,
    },
    Call {
        method: "PUT",
        path: "/api/pages/drafts/{id}",
        space: SpaceFrom::Page,
        need: Need::Write,
        output: View::Draft,
        query: &[],
        rows: Rows::One,
    },
    Call {
        method: "POST",
        path: "/api/pages/drafts/{id}/publish",
        space: SpaceFrom::Page,
        need: Need::Write,
        output: View::Draft,
        query: &[],
        rows: Rows::One,
    },
    // Archiving a page and putting it back. Deleting for good has no row.
    Call {
        method: "POST",
        path: "/api/pages/{id}/archive",
        space: SpaceFrom::Page,
        need: Need::Write,
        output: View::Removal,
        query: &[],
        rows: Rows::One,
    },
    Call {
        method: "POST",
        path: "/api/pages/{id}/restore",
        space: SpaceFrom::Page,
        need: Need::Write,
        output: View::Restore,
        query: &[],
        rows: Rows::One,
    },
    // A page another relies on, in the same Space: added and removed.
    Call {
        method: "PUT",
        path: "/api/pages/{id}/basis/{basis_id}",
        space: SpaceFrom::PagePair,
        need: Need::Write,
        output: View::Basis,
        query: &[],
        rows: Rows::One,
    },
    Call {
        method: "DELETE",
        path: "/api/pages/{id}/basis/{basis_id}",
        space: SpaceFrom::PagePair,
        need: Need::Write,
        output: View::BasisRemoval,
        query: &[],
        rows: Rows::One,
    },
    // Folders, while the owner keeps each Space in a folder of its own: a
    // member sees each of their Spaces as a top folder (paths as they see
    // them, never the server's), makes folders inside a Space they can
    // edit, and moves its pages between them.
    Call {
        method: "GET",
        path: "/api/knowledge/folders",
        space: SpaceFrom::MemberSpaces,
        need: Need::Read,
        output: View::Folders,
        query: &[],
        rows: Rows::Folders,
    },
    Call {
        method: "POST",
        path: "/api/knowledge/folders",
        space: SpaceFrom::Body,
        need: Need::Write,
        output: View::Folder,
        query: &[],
        rows: Rows::One,
    },
    Call {
        method: "POST",
        path: "/api/pages/{id}/move",
        space: SpaceFrom::Page,
        need: Need::Write,
        output: View::Move,
        query: &[],
        rows: Rows::One,
    },
];

/// The row of `method` and `path` (a daemon path), with the page ids it
/// names, in order. Only plain segments match.
pub(crate) fn find(method: &str, path: &str) -> Option<(&'static Call, Vec<String>)> {
    let plain = path
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'_' | b'-'));
    if !plain {
        return None;
    }
    let given: Vec<&str> = path.split('/').collect();
    CALLS.iter().find_map(|call| {
        if call.method != method {
            return None;
        }
        let pattern: Vec<&str> = call.path.split('/').collect();
        if pattern.len() != given.len() {
            return None;
        }
        let mut ids = Vec::new();
        for (want, got) in pattern.iter().zip(&given) {
            if want.starts_with('{') {
                if got.is_empty() {
                    return None;
                }
                ids.push((*got).to_string());
            } else if want != got {
                return None;
            }
        }
        if !ids.is_empty() && names_another_route(&given) {
            return None;
        }
        Some((call, ids))
    })
}

/// Whether a path the gate would read as a row's `{id}` is in fact another
/// route the daemon registers, such as `/api/pages/orphan-links` for the
/// `/api/pages/{id}` row: the daemon routes that path to its own handler,
/// so the gate must not forward it, not even for the page lookup.
fn names_another_route(given: &[&str]) -> bool {
    wenlan_core::truth_manifest::runtime_entries().any(|(_, _, route)| {
        let route: Vec<&str> = route.split('/').collect();
        route.len() == given.len()
            && route
                .iter()
                .zip(given)
                .any(|(want, got)| !want.starts_with('{') && want == got)
            && route
                .iter()
                .zip(given)
                .all(|(want, got)| want.starts_with('{') || want == got)
            && route.iter().any(|segment| !segment.starts_with('{'))
            && !CALLS.iter().any(|call| {
                call.path.split('/').count() == route.len()
                    && call
                        .path
                        .split('/')
                        .zip(&route)
                        .all(|(a, b)| a == *b || (a.starts_with('{') && b.starts_with('{')))
            })
    })
}

fn refused(status: StatusCode, why: &'static str) -> MemberResponse {
    MemberResponse::text(status, why)
}

fn not_found() -> MemberResponse {
    refused(StatusCode::NOT_FOUND, "no such team call")
}

/// The member's Spaces this server hosts, with their access: the Spaces
/// `/team/v1/me` lists.
async fn member_spaces(
    state: &TeamState,
    member: &Member,
) -> Result<BTreeMap<String, TeamAccess>, MemberResponse> {
    let settings = state.settings_path.clone();
    let hosted = tokio::task::spawn_blocking(move || team::hosted_spaces(&settings)).await;
    match hosted {
        // Only Spaces this server hosts for the team: a member's entry for
        // a name that is not hosted (not hosted yet, or no longer) must not
        // open a Space of the same name that is the owner's own.
        // The access in each is the rules' answer (`team_policy`).
        Ok(Ok(hosted)) => Ok(member
            .me(&hosted)
            .spaces
            .into_iter()
            .filter_map(|space| {
                let is_hosted = hosted.iter().any(|(name, _)| *name == space.space);
                crate::team_policy::access(member, &space.space, is_hosted)
                    .map(|access| (space.space, access))
            })
            .collect()),
        _ => Err(MemberResponse::status(StatusCode::SERVICE_UNAVAILABLE)),
    }
}

/// Whether the access the rules gave a Space covers `need`.
fn allows(access: TeamAccess, need: Need) -> bool {
    need == Need::Read || access == TeamAccess::Write
}

/// The Space named by `space` and `domain` (either, or both the same).
/// `Err` when they differ or are not text.
fn named_space(space: Option<&Value>, domain: Option<&Value>) -> Result<Option<String>, ()> {
    let read = |value: Option<&Value>| match value {
        None => Ok(None),
        Some(Value::String(name)) if !name.trim().is_empty() => Ok(Some(name.trim().to_string())),
        Some(_) => Err(()),
    };
    match (read(space)?, read(domain)?) {
        (Some(a), Some(b)) if a != b => Err(()),
        (a, b) => Ok(a.or(b)),
    }
}

/// One Space the member may use as `need` wants, or the refusal.
#[allow(clippy::result_large_err)] // the refusal is the handler's response
fn checked(
    spaces: &BTreeMap<String, TeamAccess>,
    space: &str,
    need: Need,
) -> Result<String, MemberResponse> {
    match spaces.get(space) {
        None => Err(not_found()),
        Some(access) if !allows(*access, need) => Err(refused(
            StatusCode::FORBIDDEN,
            "this member may only read this Space",
        )),
        Some(_) => Ok(space.to_string()),
    }
}

/// The member's Spaces a fan-out reads, in a fixed order.
fn every(spaces: &BTreeMap<String, TeamAccess>, need: Need) -> Vec<String> {
    spaces
        .iter()
        .filter(|(_, access)| allows(**access, need))
        .map(|(space, _)| space.clone())
        .collect()
}

fn client() -> Option<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .ok()
}

/// One call to this daemon as `member`, pinned to `space`.
async fn forward(
    state: &TeamState,
    member: &Member,
    method: &str,
    path_and_query: &str,
    space: Option<&str>,
    body: Option<&[u8]>,
    draft_lookup: bool,
) -> Result<(StatusCode, Vec<u8>), MemberResponse> {
    let client =
        client().ok_or_else(|| MemberResponse::status(StatusCode::INTERNAL_SERVER_ERROR))?;
    let method = reqwest::Method::from_bytes(method.as_bytes())
        .map_err(|_| MemberResponse::status(StatusCode::METHOD_NOT_ALLOWED))?;
    let mut call = client
        .request(method, format!("{}{path_and_query}", state.daemon_url))
        .header(
            crate::team_actor::SECRET_HEADER,
            crate::team_actor::secret(),
        )
        .header(crate::team_actor::KIND_HEADER, "person")
        .header(crate::team_actor::MEMBER_HEADER, member.id.as_str());
    if draft_lookup {
        call = call.header(crate::team_actor::DRAFT_SPACE_LOOKUP_HEADER, "1");
    }
    if let Ok(name) = reqwest::header::HeaderValue::from_str(member.name.trim()) {
        call = call.header(crate::team_actor::MEMBER_NAME_HEADER, name);
    }
    if let Some(space) = space {
        let value = reqwest::header::HeaderValue::from_str(space).map_err(|_| not_found())?;
        call = call.header(crate::space_header::HEADER_NAME, value);
    }
    if let Some(body) = body {
        call = call
            .header(header::CONTENT_TYPE, "application/json")
            .body(body.to_vec());
    }
    #[cfg(test)]
    if let Some(daemon) = &state.in_process_daemon {
        use tower::ServiceExt;

        let built = call.build().expect("valid daemon forwarding request");
        let bytes = built
            .body()
            .and_then(reqwest::Body::as_bytes)
            .unwrap_or_default();
        let mut request = axum::http::Request::builder()
            .method(built.method().clone())
            .uri(path_and_query)
            .body(Body::from(bytes.to_vec()))
            .expect("valid in-process forwarding request");
        *request.headers_mut() = built.headers().clone();
        let answer = daemon.clone().oneshot(request).await.unwrap();
        let status = answer.status();
        let bytes = axum::body::to_bytes(answer.into_body(), usize::MAX)
            .await
            .unwrap_or_default();
        return Ok((status, bytes.to_vec()));
    }
    match call.send().await {
        Ok(answer) => {
            let status = StatusCode::from_u16(answer.status().as_u16())
                .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            let bytes = answer.bytes().await.unwrap_or_default();
            Ok((status, bytes.to_vec()))
        }
        Err(_) => Err(refused(
            StatusCode::BAD_GATEWAY,
            "the team server did not answer",
        )),
    }
}

/// The Space of page (or draft) `id`, read on this daemon.
async fn page_space(
    state: &TeamState,
    member: &Member,
    id: &str,
    draft_lookup: bool,
) -> Result<Option<String>, MemberResponse> {
    let (status, bytes) = forward(
        state,
        member,
        "GET",
        &format!("/api/pages/{id}"),
        None,
        None,
        draft_lookup,
    )
    .await?;
    if !status.is_success() {
        return Ok(None);
    }
    #[derive(serde::Deserialize)]
    struct Lookup {
        page: view::MemberDraftSpace,
    }
    let Ok(Lookup { page }) = serde_json::from_slice(&bytes) else {
        return Ok(None);
    };
    // Only the page asked for counts: reserved segments can reach list routes.
    Ok((page.id == id).then_some(page.space).flatten())
}

fn row_space(row: &Value) -> Option<&str> {
    row.get("space")
        .or_else(|| row.get("domain"))
        .and_then(Value::as_str)
}

fn query_pairs(query: Option<&str>) -> Vec<(String, String)> {
    url::form_urlencoded::parse(query.unwrap_or_default().as_bytes())
        .into_owned()
        .collect()
}

fn encode(pairs: &[(String, String)]) -> String {
    if pairs.is_empty() {
        return String::new();
    }
    let mut out = url::form_urlencoded::Serializer::new(String::new());
    for (key, value) in pairs {
        out.append_pair(key, value);
    }
    format!("?{}", out.finish())
}

/// The gate: `/team/v1/app/<daemon path>`.
pub(super) async fn handle(
    State(state): State<TeamState>,
    Extension(member): Extension<Member>,
    req: Request,
) -> MemberResponse {
    let method = req.method().as_str().to_string();
    let path = req
        .uri()
        .path()
        .strip_prefix(PREFIX)
        .unwrap_or("")
        .to_string();
    let Some((call, ids)) = find(&method, &path) else {
        return not_found();
    };
    let given = query_pairs(req.uri().query());
    let body = match axum::body::to_bytes(req.into_body(), remote::MAX_VERSION_BYTES).await {
        Ok(body) => body,
        Err(_) => return refused(StatusCode::BAD_REQUEST, "the request is too large"),
    };
    let spaces = match member_spaces(&state, &member).await {
        Ok(spaces) => spaces,
        Err(response) => return response,
    };
    // Only the query parameters the row names go on, each number held to
    // a size the daemon can answer cheaply.
    let query: Vec<(String, String)> = given
        .iter()
        .filter(|(key, _)| call.query.contains(&key.as_str()))
        .filter_map(|(key, value)| bounded(key, value).map(|value| (key.clone(), value)))
        .collect();
    let mut object: Option<Map<String, Value>> = if body.is_empty() {
        None
    } else {
        match serde_json::from_slice::<Value>(&body) {
            Ok(Value::Object(map)) => Some(map),
            _ => return refused(StatusCode::BAD_REQUEST, "the request is not a JSON object"),
        }
    };
    if let Some(map) = object.as_mut() {
        for key in ["limit", "offset"] {
            let Some(given) = map.get(key) else { continue };
            let text = match given {
                Value::Number(n) => n.to_string(),
                Value::String(s) => s.clone(),
                _ => String::new(),
            };
            match bounded(key, &text).and_then(|v| v.parse::<i64>().ok()) {
                Some(n) => map.insert(key.into(), Value::from(n)),
                None => map.remove(key),
            };
        }
    }

    // Which Spaces the call runs in: one, or each of the member's.
    let targets: Vec<Option<String>> = match call.space {
        SpaceFrom::NoSpace => {
            if call.need == Need::Write && every(&spaces, Need::Write).is_empty() {
                return refused(StatusCode::FORBIDDEN, "this member may only read");
            }
            vec![None]
        }
        SpaceFrom::MemberSpaces => every(&spaces, call.need).into_iter().map(Some).collect(),
        SpaceFrom::QueryOrMember => {
            let find = |name: &str| {
                given
                    .iter()
                    .find(|(key, _)| key == name)
                    .map(|(_, value)| Value::String(value.clone()))
            };
            match named_space(find("space").as_ref(), find("domain").as_ref()) {
                Err(()) => return not_found(),
                Ok(Some(space)) => match checked(&spaces, &space, call.need) {
                    Ok(space) => vec![Some(space)],
                    Err(response) => return response,
                },
                Ok(None) => every(&spaces, call.need).into_iter().map(Some).collect(),
            }
        }
        SpaceFrom::Body | SpaceFrom::BodyOrMember => {
            let map = object.get_or_insert_with(Map::new);
            if map.get("space").is_some_and(Value::is_null)
                || map.get("domain").is_some_and(Value::is_null)
            {
                return not_found();
            }
            match named_space(map.get("space"), map.get("domain")) {
                Err(()) => return not_found(),
                Ok(Some(space)) => match checked(&spaces, &space, call.need) {
                    Ok(space) => vec![Some(space)],
                    Err(response) => return response,
                },
                Ok(None) if call.space == SpaceFrom::BodyOrMember => {
                    every(&spaces, call.need).into_iter().map(Some).collect()
                }
                Ok(None) => {
                    let mine = every(&spaces, call.need);
                    match mine.as_slice() {
                        [one] => vec![Some(one.clone())],
                        [] => return refused(StatusCode::FORBIDDEN, "this member may only read"),
                        _ => return refused(StatusCode::BAD_REQUEST, "say which Space"),
                    }
                }
            }
        }
        SpaceFrom::Page | SpaceFrom::PagePair => {
            let id = ids.first().map(String::as_str).unwrap_or_default();
            let draft_lookup = call.path.starts_with("/api/pages/drafts/");
            let space = match page_space(&state, &member, id, draft_lookup).await {
                Ok(Some(space)) => space,
                Ok(None) => return not_found(),
                Err(response) => return response,
            };
            if call.space == SpaceFrom::PagePair {
                // The first page must be the member's before the second is
                // looked up at all; then both must share its Space.
                if checked(&spaces, &space, call.need).is_err() {
                    return not_found();
                }
                let other = match ids.get(1) {
                    Some(other) => page_space(&state, &member, other, false).await,
                    None => Ok(None),
                };
                match other {
                    Ok(Some(other)) if other == space => {}
                    Ok(_) => return not_found(),
                    Err(response) => return response,
                }
            }
            if let Some(map) = object.as_ref() {
                match named_space(map.get("space"), map.get("domain")) {
                    Ok(None) => {}
                    Ok(Some(named)) if named == space => {}
                    // Not moved to another Space from the web.
                    _ => return not_found(),
                }
            }
            match checked(&spaces, &space, call.need) {
                Ok(space) => vec![Some(space)],
                Err(response) => return response,
            }
        }
    };

    // Answers, one per Space. A list over several Spaces is read in full up
    // to the window asked for, then put together and cut to that window.
    let window = (call.rows == Rows::Pages && targets.len() > 1).then(|| {
        let number = |name: &str, default: usize| {
            query
                .iter()
                .find(|(key, _)| key == name)
                .and_then(|(_, value)| value.parse::<usize>().ok())
                .unwrap_or(default)
        };
        let limit = number("limit", if method == "GET" { 50 } else { 20 });
        (number("offset", 0), limit)
    });
    let mut answers: Vec<(StatusCode, Value)> = Vec::new();
    for space in &targets {
        let mut pairs = query.clone();
        let mut body = object.clone();
        if let Some(space) = space {
            if let Some(map) = body.as_mut() {
                map.remove("domain");
                // A draft's save names its Space (the daemon checks it is
                // the draft's); other page calls carry none.
                if !matches!(call.space, SpaceFrom::Page | SpaceFrom::PagePair)
                    || map.contains_key("space")
                    || call.path == "/api/pages/drafts/{id}"
                {
                    map.insert("space".into(), Value::String(space.clone()));
                }
            }
        }
        if let Some((offset, limit)) = window {
            pairs.retain(|(key, _)| key != "offset" && key != "limit");
            let wanted = (offset + limit).to_string();
            if method == "GET" {
                pairs.push(("limit".into(), wanted));
            } else if let Some(map) = body.as_mut() {
                map.insert("limit".into(), Value::from(offset + limit));
            }
        }
        let bytes = body.map(|map| serde_json::to_vec(&Value::Object(map)).unwrap_or_default());
        let (status, answer) = match forward(
            &state,
            &member,
            &method,
            &format!("{path}{}", encode(&pairs)),
            space.as_deref(),
            bytes.as_deref(),
            false,
        )
        .await
        {
            Ok(answer) => answer,
            Err(response) => return response,
        };
        let value = serde_json::from_slice::<Value>(&answer).unwrap_or(Value::Null);
        answers.push((status, value));
    }

    let failed = answers
        .iter()
        .find(|(status, _)| !status.is_success())
        .cloned();
    let mut value = match failed {
        Some((status, value)) => return MemberResponse::project_problem(status, value),
        None => merge(call, &query, window, answers, &spaces),
    };
    // Every folder starts with its Space's name: folders of a Space not the
    // member's go.
    if call.rows == Rows::Folders {
        if let Some(Value::Array(folders)) = value.get_mut("folders") {
            folders.retain(|row| {
                row.get("path")
                    .and_then(Value::as_str)
                    .and_then(|path| path.split('/').next())
                    .is_some_and(|space| spaces.contains_key(space))
            });
        }
    }
    // Every page row names its Space: rows of a Space not the member's go.
    if call.rows == Rows::Pages {
        if let Some(Value::Array(pages)) = value.get_mut("pages") {
            pages.retain(|row| row_space(row).is_some_and(|space| spaces.contains_key(space)));
        }
    }
    if call.rows == Rows::Spaces {
        if let Some(list) = value.as_array_mut() {
            list.retain(|space| {
                space
                    .get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|name| spaces.contains_key(name))
            });
        }
    }
    call.output.project(value)
}

/// The most rows one call may ask for: the app's largest page list.
pub(crate) const MAX_LIMIT: i64 = 1_000;
/// The furthest a member may page into a list.
pub(crate) const MAX_OFFSET: i64 = 100_000;

/// A number a member sends, held to what the daemon answers cheaply: a
/// `limit` of 1 to [`MAX_LIMIT`], an `offset` of 0 to [`MAX_OFFSET`], a time
/// of 0 or later. Anything that is not a whole number is dropped, so the
/// daemon's own default applies.
pub(crate) fn bounded(key: &str, value: &str) -> Option<String> {
    let range = match key {
        "limit" => 1..=MAX_LIMIT,
        "offset" => 0..=MAX_OFFSET,
        "since_ms" => 0..=i64::MAX,
        _ => return Some(value.to_string()),
    };
    let n = value.trim().parse::<i64>().ok()?;
    Some(n.clamp(*range.start(), *range.end()).to_string())
}

/// The answers of a call made in several Spaces, as one.
fn merge(
    call: &Call,
    query: &[(String, String)],
    window: Option<(usize, usize)>,
    answers: Vec<(StatusCode, Value)>,
    spaces: &BTreeMap<String, TeamAccess>,
) -> Value {
    let mut answers = answers.into_iter().map(|(_, value)| value);
    match call.rows {
        Rows::One => answers.next().unwrap_or(Value::Null),
        Rows::Spaces => Value::Array(
            answers
                .flat_map(|answer| match answer {
                    Value::Array(spaces) => spaces,
                    _ => Vec::new(),
                })
                .collect(),
        ),
        Rows::Pages => {
            let mut first = Value::Null;
            let mut pages = Vec::new();
            for (n, mut answer) in answers.enumerate() {
                if let Some(Value::Array(list)) = answer.get_mut("pages").map(Value::take) {
                    pages.extend(list);
                }
                if n == 0 {
                    first = answer;
                }
            }
            if let Some((offset, limit)) = window {
                pages = pages.into_iter().skip(offset).take(limit).collect();
            }
            if !first.is_object() {
                first = Value::Object(Map::new());
            }
            first["pages"] = Value::Array(pages);
            first
        }
        Rows::Folders => {
            let mut folders = Vec::new();
            for mut answer in answers {
                if let Some(Value::Array(list)) = answer.get_mut("folders").map(Value::take) {
                    folders.extend(list);
                }
            }
            serde_json::json!({ "folders": folders, "truncated": false })
        }
        Rows::Graph => {
            let graphs = answers.filter_map(|answer| serde_json::from_value(answer).ok());
            serde_json::to_value(MemberGraph::merge(graphs, |space| {
                spaces.contains_key(space)
            }))
            .unwrap_or(Value::Null)
        }
        Rows::Newest(field) => {
            let mut rows: Vec<Value> = answers
                .flat_map(|answer| match answer {
                    Value::Array(list) => list,
                    _ => Vec::new(),
                })
                .collect();
            rows.sort_by_key(|row| {
                std::cmp::Reverse(row.get(field).and_then(Value::as_i64).unwrap_or(0))
            });
            let limit = query
                .iter()
                .find(|(key, _)| key == "limit")
                .and_then(|(_, value)| value.parse::<usize>().ok())
                .unwrap_or(10);
            rows.truncate(limit.max(1));
            Value::Array(rows)
        }
    }
}

#[cfg(test)]
#[path = "team_app_gate_test.rs"]
mod tests;
