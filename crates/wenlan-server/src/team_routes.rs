// SPDX-License-Identifier: Apache-2.0
//! The team server (`wenlan_core::team`, `wenlan_types::team`).
//!
//! Two parts:
//!
//! - **On this computer only** (the daemon's own loopback router): the
//!   member list (`/api/team/*`), hosting a Space for the team, and, on a
//!   member's computer, reading a join code (`/api/team/join`).
//! - **The team listener** ([`serve_team_listener`]), only when
//!   `WENLAN_TEAM_ADDR` is set: a separate listener that serves nothing but
//!   `/health`, `/team/v1/*` (desktops sync a Space's page versions) and
//!   `/mcp` (AI tools), each request checked against a member's key first.
//!   None of the daemon's own routes are reachable through it; MCP reaches
//!   the daemon over loopback, pinned to the member's Space.
//!
//! The team listener speaks plain HTTP. Put it behind HTTPS (a reverse
//! proxy) or reach it over a private network such as Tailscale; a member's
//! Wenlan refuses plain `http://` to a public address.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Path, Query, Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use tower::ServiceExt;
use wenlan_core::shared_space::{self, remote, Scope};
use wenlan_core::team::{self, Member};
use wenlan_types::team::{
    TeamAccess, TeamAddMemberRequest, TeamFileList, TeamHostSpaceRequest, TeamJoinRequest,
    TeamJoinResponse, TeamMember, TeamMemberKey, TeamRenameRequest, TeamStatus,
    TeamUpdateMemberRequest,
};

use crate::error::ServerError;
use crate::lifecycle::ShutdownHandle;
use crate::route_registry::{get, post, put, TrackedRouter};
use crate::state::SharedState;

/// The environment variable that turns the team listener on: the address
/// to listen on, such as `0.0.0.0:7879`.
pub const TEAM_ADDR_ENV: &str = "WENLAN_TEAM_ADDR";

fn listening() -> Option<String> {
    std::env::var(TEAM_ADDR_ENV)
        .ok()
        .map(|addr| addr.trim().to_string())
        .filter(|addr| !addr.is_empty())
}

// ===== On this computer =====

pub(crate) fn register(router: TrackedRouter<SharedState>) -> TrackedRouter<SharedState> {
    router
        .route("/api/team", get(handle_team_status).put(handle_rename_team))
        .route("/api/team/members", post(handle_add_member))
        .route(
            "/api/team/members/{id}",
            put(handle_update_member).delete(handle_remove_member),
        )
        .route("/api/team/members/{id}/key", post(handle_new_member_key))
        .route("/api/team/spaces", post(handle_host_space))
        .route("/api/team/join", post(handle_join))
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, wenlan_core::error::WenlanError> + Send + 'static,
) -> Result<T, ServerError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| ServerError::Internal(error.to_string()))?
        .map_err(ServerError::from)
}

/// GET /api/team — the member list (never a key) and the Spaces this
/// Wenlan hosts for the team.
pub async fn handle_team_status() -> Result<Json<TeamStatus>, ServerError> {
    let status = blocking(|| {
        let path = team::team_path();
        let file = team::read_team(&path)?;
        let spaces = team::hosted_spaces(&shared_space::settings_path())?;
        // A Wenlan that hosts nothing and has no members is on no team of
        // its own: no team.json is made for it.
        let info = if spaces.is_empty() && file.members.is_empty() {
            file.info()
        } else {
            Some(team::team_info(&path)?)
        };
        Ok(TeamStatus {
            team: info,
            members: file.members.iter().map(Member::to_wire).collect(),
            everyone: file
                .everyone
                .iter()
                .map(|(space, access)| wenlan_types::team::TeamSpaceAccess {
                    space: space.clone(),
                    access: *access,
                })
                .collect(),
            spaces: spaces.into_iter().map(|(name, _)| name).collect(),
            listening: listening(),
        })
    })
    .await?;
    Ok(Json(status))
}

/// PUT /api/team — names the team; members' Wenlans show the name on their
/// next sync pass.
pub async fn handle_rename_team(
    Json(req): Json<TeamRenameRequest>,
) -> Result<Json<wenlan_types::team::TeamInfo>, ServerError> {
    let info = blocking(move || team::rename_team(&team::team_path(), &req.name)).await?;
    Ok(Json(info))
}

fn member_key(
    member: Member,
    key: String,
    url: Option<&str>,
) -> Result<TeamMemberKey, ServerError> {
    let join_code = match url.map(str::trim).filter(|url| !url.is_empty()) {
        Some(url) => {
            let url = remote::check_url(url).map_err(ServerError::ValidationError)?;
            Some(wenlan_types::team::invite_link(&url, &key))
        }
        None => None,
    };
    Ok(TeamMemberKey {
        member: member.to_wire(),
        key,
        join_code,
    })
}

/// POST /api/team/members — adds a member. Their key is in the answer this
/// once; only its hash is kept.
pub async fn handle_add_member(
    Json(req): Json<TeamAddMemberRequest>,
) -> Result<Json<TeamMemberKey>, ServerError> {
    if let Some(url) = req.url.as_deref().filter(|url| !url.trim().is_empty()) {
        remote::check_url(url).map_err(ServerError::ValidationError)?;
    }
    let (member, key) = {
        let spaces = req.spaces.clone();
        let name = req.name.clone();
        blocking(move || team::add_member(&team::team_path(), &name, &spaces)).await?
    };
    Ok(Json(member_key(member, key, req.url.as_deref())?))
}

/// PUT /api/team/members/{id} — renames a member or changes their Spaces.
pub async fn handle_update_member(
    Path(id): Path<String>,
    Json(req): Json<TeamUpdateMemberRequest>,
) -> Result<Json<TeamMember>, ServerError> {
    let member = blocking(move || {
        team::update_member(
            &team::team_path(),
            &id,
            req.name.as_deref(),
            req.spaces.as_deref(),
        )
    })
    .await?;
    Ok(Json(member.to_wire()))
}

/// DELETE /api/team/members/{id} — removes a member: their key stops
/// working at once. What their computer already synced stays there.
pub async fn handle_remove_member(Path(id): Path<String>) -> Result<Json<TeamMember>, ServerError> {
    let member = blocking(move || team::remove_member(&team::team_path(), &id)).await?;
    Ok(Json(member.to_wire()))
}

#[derive(Debug, Default, Deserialize)]
pub struct NewKeyQuery {
    #[serde(default)]
    pub url: Option<String>,
}

/// POST /api/team/members/{id}/key — a new key for a member; the old one
/// stops working. The key is in the answer this once.
pub async fn handle_new_member_key(
    Path(id): Path<String>,
    Query(query): Query<NewKeyQuery>,
) -> Result<Json<TeamMemberKey>, ServerError> {
    let (member, key) = blocking(move || team::new_member_key(&team::team_path(), &id)).await?;
    Ok(Json(member_key(member, key, query.url.as_deref())?))
}

/// POST /api/team/spaces — hosts a Space of this Wenlan for the team: it is
/// shared through a folder of this Wenlan's own, which members reach only
/// through the team listener. Runs the first pass and returns its counts.
pub async fn handle_host_space(
    State(state): State<SharedState>,
    Json(req): Json<TeamHostSpaceRequest>,
) -> Result<Json<wenlan_types::shared_space::SharedSyncCounts>, ServerError> {
    let (db, page_root) = {
        let s = state.read().await;
        (
            s.db.clone().ok_or(ServerError::DbNotInitialized)?,
            s.lint_config.page_root().map(std::path::Path::to_path_buf),
        )
    };
    let space = db
        .registered_space_or_none(Some(req.space.trim()))
        .await
        .map_err(|e| ServerError::Internal(e.to_string()))?
        .ok_or_else(|| ServerError::NotFound(format!("no Space named {:?}", req.space)))?;
    let settings = shared_space::settings_path();
    let scope = Scope::Space(space.clone());
    if let Some(everyone) = req.everyone {
        if !shared_space::share_settings_at(&settings, &scope)?.is_some_and(|share| share.hosted) {
            return Err(ServerError::ValidationError(format!(
                "{space} is not hosted for the team"
            )));
        }
        let access = match everyone {
            wenlan_types::team::TeamEveryone::Nobody => None,
            wenlan_types::team::TeamEveryone::Read => Some(wenlan_types::team::TeamAccess::Read),
            wenlan_types::team::TeamEveryone::Write => Some(wenlan_types::team::TeamAccess::Write),
        };
        blocking(move || team::set_everyone(&team::team_path(), &space, access)).await?;
        return Ok(Json(Default::default()));
    }
    let folder = match shared_space::share_settings_at(&settings, &scope)? {
        Some(existing) if existing.hosted => PathBuf::from(existing.folder),
        Some(_) => {
            return Err(ServerError::ValidationError(format!(
                "{space} is shared through a folder or another team server already; stop sharing it first"
            )))
        }
        None => team::new_hosted_folder(&settings),
    };
    let name = server_name(&db).await?;
    // Marked hosted before the first pass, so the pass lists the members.
    shared_space::join_at(&settings, &scope, &folder, &name)?;
    shared_space::update_share(&settings, &scope, |share| share.hosted = true)?;
    let counts = shared_space::sync_space(
        &db,
        shared_space::SyncTarget {
            scope: &scope,
            folder: &folder,
            member_name: &name,
            settings_path: &settings,
            knowledge_path: page_root.as_deref(),
            member_id: None,
            host: None,
        },
    )
    .await?;
    Ok(Json(counts))
}

/// The name the team sees for edits made on the server itself.
async fn server_name(db: &wenlan_core::db::MemoryDB) -> Result<String, ServerError> {
    Ok(shared_space::person_name(db)
        .await?
        .unwrap_or_else(|| "Team server".to_string()))
}

/// POST /api/team/join — on a member's computer: reads a join code, asks
/// the server who the key belongs to, and returns the Spaces they can use.
/// Nothing is joined yet.
pub async fn handle_join(
    Json(req): Json<TeamJoinRequest>,
) -> Result<Json<TeamJoinResponse>, ServerError> {
    let (url, key) = join_parts(&req.code, req.key.as_deref())?;
    let me = remote::me(&url, &key)
        .await
        .map_err(ServerError::ValidationError)?;
    Ok(Json(TeamJoinResponse { url, me }))
}

/// The server's url and the key, from a join code or a url and a key.
pub(crate) fn join_parts(code: &str, key: Option<&str>) -> Result<(String, String), ServerError> {
    let (url, key) = match key.map(str::trim).filter(|key| !key.is_empty()) {
        Some(key) => (code.trim().to_string(), key.to_string()),
        None => wenlan_types::team::split_join_code(code).ok_or_else(|| {
            ServerError::ValidationError(
                "this is not an invite link: it looks like https://…/join#wlt_…".into(),
            )
        })?,
    };
    if !wenlan_types::team::is_team_key(&key) {
        return Err(ServerError::ValidationError(
            "this is not a member key".into(),
        ));
    }
    let url = remote::check_url(&url).map_err(ServerError::ValidationError)?;
    Ok((url, key))
}

// ===== The team listener =====

type McpService = rmcp::transport::streamable_http_server::StreamableHttpService<
    wenlan_mcp::tools::WenlanMcpServer,
    rmcp::transport::streamable_http_server::session::local::LocalSessionManager,
>;

#[derive(Clone)]
struct TeamState {
    team_path: PathBuf,
    settings_path: PathBuf,
    /// The daemon's own loopback address, which MCP reaches it at.
    daemon_url: String,
    /// One MCP service per member, Space and access, so a connection's
    /// session stays with the service that made it.
    mcp: McpCache,
}

/// MCP services by (member, Space, access).
type McpCache = Arc<std::sync::Mutex<HashMap<(String, String, TeamAccess), McpService>>>;

/// Starts the team listener when `WENLAN_TEAM_ADDR` is set, until the
/// daemon shuts down. A listener that cannot start is logged; the daemon
/// goes on without it.
pub async fn serve_team_listener(shared: SharedState, shutdown: ShutdownHandle) {
    let Some(addr) = listening() else {
        return;
    };
    let port = { shared.read().await.bound_port };
    let state = TeamState {
        team_path: team::team_path(),
        settings_path: shared_space::settings_path(),
        daemon_url: format!("http://127.0.0.1:{port}"),
        mcp: Arc::default(),
    };
    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(listener) => listener,
        Err(error) => {
            tracing::error!("[team] cannot listen on {addr}: {error}");
            return;
        }
    };
    tracing::info!(
        "[team] team listener on http://{addr}: only /health, /team/v1 and /mcp, each checked against a member key. Serve it over HTTPS or a private network (Tailscale)."
    );
    let app = team_router(state);
    if let Err(error) = axum::serve(listener, app.into_make_service())
        .with_graceful_shutdown(crate::lifecycle::wait_for_shutdown(shutdown.subscribe()))
        .await
    {
        tracing::error!("[team] team listener stopped: {error}");
    }
}

fn team_router(state: TeamState) -> axum::Router {
    use axum::routing::{any, get, put};
    let checked = axum::Router::new()
        .route("/team/v1/me", get(team_me))
        .route("/team/v1/spaces/{space}/files", get(team_files))
        .route("/team/v1/spaces/{space}/file", get(team_file))
        .route(
            "/team/v1/spaces/{space}/versions/{page}/{name}",
            put(team_put_version),
        )
        .route("/mcp", any(team_mcp))
        .route("/mcp/{*space}", any(team_mcp_in))
        .layer(middleware::from_fn(wenlan_mcp::serve::guard_initialization))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_member,
        ));
    axum::Router::new()
        .route("/health", get(team_health))
        .route(wenlan_types::team::JOIN_PATH, get(team_join_page))
        .merge(checked)
        .layer(DefaultBodyLimit::max(remote::MAX_VERSION_BYTES))
        .with_state(state)
}

async fn team_health() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok", "server": "wenlan-team" }))
}

/// GET /join — where an invite link opens in a browser. The key after `#`
/// never reaches the server; the page only says what to do with the link.
/// It shows the team's name and nothing else of the team.
async fn team_join_page(State(state): State<TeamState>) -> impl IntoResponse {
    let name = team::read_team(&state.team_path)
        .ok()
        .and_then(|file| file.name)
        .unwrap_or_else(|| "a team".to_string());
    let name = name
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;");
    let page = format!(
        "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
<title>Join {name} on Wenlan</title>\
<style>body{{font:16px/1.6 system-ui,sans-serif;max-width:32rem;margin:15vh auto;padding:0 16px;color:#1c1d22}}\
@media(prefers-color-scheme:dark){{body{{background:#16171b;color:#ececf1}}a{{color:#a29dff}}}}</style>\
<h1>Join {name} on Wenlan</h1>\
<p>Copy this page's address. In Wenlan, open Spaces, choose Add shared, and paste it.</p>\
<p>Don't have Wenlan yet? <a href=\"https://wenlan.app\">Download it</a>, then come back to this link.</p>"
    );
    (
        [
            (axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (axum::http::header::CACHE_CONTROL, "no-store"),
        ],
        page,
    )
}

/// Origins of web AI apps whose connectors may call `/mcp` from a browser.
const ALLOWED_ORIGINS: &[&str] = &["https://claude.ai", "https://chatgpt.com"];

/// Every request but `/health`: the member key first (401), then a browser
/// origin (403). The member is handed to the handler.
async fn require_member(State(state): State<TeamState>, mut req: Request, next: Next) -> Response {
    let key = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .unwrap_or_default()
        .to_string();
    let path = state.team_path.clone();
    let member = tokio::task::spawn_blocking(move || team::authenticate(&path, &key)).await;
    let member = match member {
        Ok(Ok(Some(member))) => member,
        Ok(Ok(None)) => return (StatusCode::UNAUTHORIZED, "member key required").into_response(),
        Ok(Err(error)) => {
            tracing::warn!("[team] member list cannot be read: {error}");
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    if let Some(origin) = req
        .headers()
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    {
        if !ALLOWED_ORIGINS.contains(&origin) {
            return (StatusCode::FORBIDDEN, "origin not allowed").into_response();
        }
    }
    req.extensions_mut().insert(member);
    next.run(req).await
}

async fn team_me(Extension(member): Extension<Member>) -> impl IntoResponse {
    Json(member.me())
}

/// The hosted Space's folder, when `member` may use it.
#[allow(clippy::result_large_err)] // the refusal is the handler's response
fn space_folder(
    state: &TeamState,
    member: &Member,
    space: &str,
) -> Result<(PathBuf, TeamAccess), Response> {
    let Some(access) = member.access(space) else {
        return Err((StatusCode::FORBIDDEN, "not a Space of this member").into_response());
    };
    match team::hosted_folder(&state.settings_path, space) {
        Ok(Some(folder)) => Ok((folder, access)),
        Ok(None) => Err((
            StatusCode::NOT_FOUND,
            "this server does not host that Space",
        )
            .into_response()),
        Err(error) => {
            tracing::warn!("[team] shared Spaces cannot be read: {error}");
            Err(StatusCode::SERVICE_UNAVAILABLE.into_response())
        }
    }
}

async fn team_files(
    State(state): State<TeamState>,
    Extension(member): Extension<Member>,
    Path(space): Path<String>,
) -> Response {
    let (folder, access) = match space_folder(&state, &member, &space) {
        Ok(found) => found,
        Err(refused) => return refused,
    };
    match tokio::task::spawn_blocking(move || team::list_folder_files(&folder)).await {
        Ok(files) => Json(TeamFileList { access, files }).into_response(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct FileQuery {
    path: String,
}

async fn team_file(
    State(state): State<TeamState>,
    Extension(member): Extension<Member>,
    Path(space): Path<String>,
    Query(query): Query<FileQuery>,
) -> Response {
    let (folder, _) = match space_folder(&state, &member, &space) {
        Ok(found) => found,
        Err(refused) => return refused,
    };
    let Some(file) = team::served_path(&folder, &query.path) else {
        return (StatusCode::BAD_REQUEST, "not a file of the Space").into_response();
    };
    match tokio::fs::read(&file).await {
        Ok(bytes) => (
            [(header::CONTENT_TYPE, "text/markdown; charset=utf-8")],
            Body::from(bytes),
        )
            .into_response(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            StatusCode::NOT_FOUND.into_response()
        }
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

/// PUT one of the member's own page versions into the hosted folder. It
/// must be whole, written by them, and filed under its page; a member who
/// may only read sends none. The server's next pass takes it.
async fn team_put_version(
    State(state): State<TeamState>,
    Extension(member): Extension<Member>,
    Path((space, page, name)): Path<(String, String, String)>,
    body: axum::body::Bytes,
) -> Response {
    let (folder, access) = match space_folder(&state, &member, &space) {
        Ok(found) => found,
        Err(refused) => return refused,
    };
    if access != TeamAccess::Write {
        return (
            StatusCode::FORBIDDEN,
            "this member may only read this Space",
        )
            .into_response();
    }
    let id = member.id.clone();
    let stored = tokio::task::spawn_blocking(move || {
        let kind = team::check_member_version(&id, &page, &name, &body)?;
        let text = std::str::from_utf8(&body)
            .map_err(|_| wenlan_core::error::WenlanError::Validation("not text".into()))?;
        // A version, a proposal or a decision, each in its own folder.
        shared_space::version::write_file(&folder, &id, kind, &page, text)
    })
    .await;
    match stored {
        Ok(Ok((_, wrote))) => {
            if wrote {
                StatusCode::CREATED.into_response()
            } else {
                StatusCode::OK.into_response()
            }
        }
        Ok(Err(error)) => (StatusCode::UNPROCESSABLE_ENTITY, error.to_string()).into_response(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

/// `/mcp`: the member's only Space. A member with more than one uses
/// `/mcp/<Space>`.
async fn team_mcp(
    State(state): State<TeamState>,
    Extension(member): Extension<Member>,
    req: Request,
) -> Response {
    let spaces: Vec<&String> = member
        .spaces
        .keys()
        .filter(|space| member.access(space).is_some())
        .collect();
    match spaces.as_slice() {
        [space] => {
            let space = (*space).clone();
            serve_mcp(state, member, space, req).await
        }
        [] => (StatusCode::FORBIDDEN, "this member has no Space").into_response(),
        _ => (
            StatusCode::NOT_FOUND,
            "this member has more than one Space: connect to /mcp/<Space name>",
        )
            .into_response(),
    }
}

async fn team_mcp_in(
    State(state): State<TeamState>,
    Extension(member): Extension<Member>,
    Path(space): Path<String>,
    req: Request,
) -> Response {
    serve_mcp(state, member, space, req).await
}

async fn serve_mcp(state: TeamState, member: Member, space: String, req: Request) -> Response {
    let access = match space_folder(&state, &member, &space) {
        Ok((_, access)) => access,
        Err(refused) => return refused,
    };
    let service = {
        let mut services = state
            .mcp
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        services
            .entry((member.id.clone(), space.clone(), access))
            .or_insert_with(|| {
                wenlan_mcp::serve::team_member_service(
                    state.daemon_url.clone(),
                    member.id.clone(),
                    space.clone(),
                    match access {
                        TeamAccess::Write => wenlan_mcp::tools::ToolProfile::WebWrite,
                        TeamAccess::Read => wenlan_mcp::tools::ToolProfile::QueryOnly,
                    },
                )
            })
            .clone()
    };
    match service.oneshot(req).await {
        Ok(response) => response.into_response(),
        Err(never) => match never {},
    }
}

#[cfg(test)]
#[path = "team_routes_test.rs"]
mod tests;
