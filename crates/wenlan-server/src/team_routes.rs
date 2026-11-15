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
use std::sync::{Arc, LazyLock};

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Path, Query, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use tower::ServiceExt;
use wenlan_core::shared_space::{self, remote, Scope};
use wenlan_core::team::{self, Member};
use wenlan_types::team::{
    TeamAccess, TeamAddMemberRequest, TeamFileList, TeamHostSpaceRequest, TeamJoinRequest,
    TeamJoinResponse, TeamMember, TeamMemberKey, TeamMembershipRequest, TeamRenameRequest,
    TeamRole, TeamStartRequest, TeamStatus, TeamUpdateMemberRequest, DEFAULT_TEAM_PORT,
};

use crate::error::ServerError;
use crate::lifecycle::ShutdownHandle;
use crate::route_registry::{get, post, put, TrackedRouter};
use crate::state::SharedState;

/// The environment variable that turns the team listener on: the address
/// to listen on, such as `0.0.0.0:7879`.
pub const TEAM_ADDR_ENV: &str = "WENLAN_TEAM_ADDR";
/// The address put in invite links, for a server started with
/// `WENLAN_TEAM_ADDR` (Docker), such as `https://wenlan.example.com`.
pub const TEAM_URL_ENV: &str = "WENLAN_TEAM_URL";
/// Set (to anything) on a server started with `WENLAN_TEAM_ADDR`: the
/// owner's invite link is made again and logged; the old one stops.
pub const NEW_OWNER_LINK_ENV: &str = "WENLAN_TEAM_NEW_OWNER_LINK";
/// The SHA-256 of the owner's key, which the owner's own Wenlan made and
/// keeps (`wenlan-server team <address> --owner <sha256>`): the server takes
/// it as the owner's on its first start instead of printing a new link.
/// Only the hash is ever on this machine, never the key.
pub const OWNER_SHA256_ENV: &str = "WENLAN_TEAM_OWNER_SHA256";

/// The name a server nobody sits at gives its owner until the owner's own
/// Wenlan takes the owner link and says who they are.
const OWNER_STAND_IN: &str = "Owner";

fn env_value(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Where the owner turned the team listener on from the app (`team.json`
/// `listen`), as the running listener follows it. `WENLAN_TEAM_ADDR`, when
/// set, wins and cannot be turned off from the app.
static LISTEN: LazyLock<tokio::sync::watch::Sender<Option<String>>> =
    LazyLock::new(|| tokio::sync::watch::channel(None).0);

fn listening() -> Option<String> {
    env_value(TEAM_ADDR_ENV).or_else(|| LISTEN.borrow().clone())
}

/// This Wenlan is a team's server, or is set up to be one: it has the
/// team listener on, people or hosted Spaces.
fn hosts_a_team(file: &team::TeamFile, hosted: usize) -> bool {
    listening().is_some() || file.listen.is_some() || !file.members.is_empty() || hosted > 0
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
        .route("/api/team/start", post(handle_start_team))
        .route("/api/team/stop", post(handle_stop_team))
        .route("/api/team/firewall", get(handle_firewall))
        .route(
            "/api/team/membership",
            post(handle_remember_membership).delete(handle_forget_membership),
        )
}

/// GET /api/team/firewall — on Windows, whether other computers get
/// through Windows Firewall to this computer's team listener; `null`
/// elsewhere, or when the rules cannot be read.
pub async fn handle_firewall() -> Json<Option<wenlan_types::team::TeamFirewall>> {
    Json(crate::team_firewall::check().await)
}

/// When this computer is no team's server but its person owns a team
/// whose server is elsewhere (Docker): that server's url and their key.
/// The `/api/team` routes then manage that team, on its server.
async fn managed_elsewhere() -> Result<Option<(String, String)>, ServerError> {
    blocking(|| {
        let file = team::read_team(&team::team_path())?;
        let settings = shared_space::settings_path();
        if hosts_a_team(&file, team::hosted_spaces(&settings)?.len()) {
            return Ok(None);
        }
        shared_space::owned_team(&settings)
    })
    .await
}

/// One management call on the owned team's server, its answer read as `T`.
async fn on_server<T: serde::de::DeserializeOwned>(
    (url, key): &(String, String),
    method: reqwest::Method,
    path: &str,
    body: Option<serde_json::Value>,
) -> Result<T, ServerError> {
    let value = remote::admin(url, key, method, path, body.as_ref())
        .await
        .map_err(ServerError::ValidationError)?;
    serde_json::from_value(value)
        .map_err(|_| ServerError::Internal(format!("{url} answered something unexpected")))
}

/// Renames the Space `name` here (or only changes its description), and
/// what it is shared through goes with it:
/// - a Space synced from the team this person owns on another server is
///   renamed there first, for the whole team;
/// - a Space shared through a folder or a team keeps its folder and member;
/// - a Space hosted here keeps its team access, and members' computers
///   follow it by its id.
pub(crate) async fn rename_space(
    db: &wenlan_core::db::MemoryDB,
    name: &str,
    new_name: &str,
    description: Option<&str>,
) -> Result<wenlan_core::db::Space, ServerError> {
    let new_name = new_name.trim();
    let settings = shared_space::settings_path();
    let share = shared_space::share_settings_at(&settings, &Scope::Space(name.to_string()))?;
    let renaming = new_name != name;
    if renaming && share.is_some() {
        if shared_space::share_settings_at(&settings, &Scope::Space(new_name.to_string()))?
            .is_some()
        {
            return Err(ServerError::ValidationError(format!(
                "{new_name} is shared from here already"
            )));
        }
        let owned = blocking({
            let settings = settings.clone();
            move || shared_space::owned_team(&settings)
        })
        .await?;
        let host = share.as_ref().and_then(|share| share.host.as_ref());
        if let (Some(owned), Some(host)) = (owned, host) {
            if owned.0 == host.url {
                let request = TeamHostSpaceRequest {
                    space: host.space.clone(),
                    everyone: None,
                    rename_to: Some(new_name.to_string()),
                };
                let _: wenlan_types::shared_space::SharedSyncCounts =
                    on_server(&owned, reqwest::Method::POST, "/spaces", json_of(&request)).await?;
            }
        }
    }
    let space = db
        .update_space(name, new_name, description)
        .await
        .map_err(ServerError::from)?;
    if renaming {
        if let Some(share) = share {
            let (from, to) = (name.to_string(), new_name.to_string());
            blocking(move || {
                shared_space::rename_share(&settings, &from, &to)?;
                if share.hosted {
                    team::rename_space_access(&team::team_path(), &from, &to)?;
                }
                // Renamed on the team server just now: this computer
                // follows it under its new name there.
                if let Some(host) = share.host.filter(|host| host.space == from) {
                    let owner = shared_space::owned_team(&settings)?
                        .is_some_and(|(url, _)| url == host.url);
                    if owner {
                        shared_space::update_share(
                            &settings,
                            &Scope::Space(to.clone()),
                            |share| {
                                if let Some(host) = share.host.as_mut() {
                                    host.space = to.clone();
                                }
                            },
                        )?;
                    }
                }
                Ok(())
            })
            .await?;
        }
    }
    Ok(space)
}

/// This computer's person's name, as their profile has it.
async fn own_name(state: &SharedState) -> Option<String> {
    let db = { state.read().await.db.clone() }?;
    let profile = db.get_profile().await.ok().flatten()?;
    let name = profile.display_name?.trim().to_string();
    (!name.is_empty()).then_some(name)
}

fn json_of<T: serde::Serialize>(value: &T) -> Option<serde_json::Value> {
    serde_json::to_value(value).ok()
}

/// Percent-encodes one path segment or query value.
fn encoded(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
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
    if let Some(owned) = managed_elsewhere().await? {
        // Read for its owner. When the server cannot be reached, this
        // computer says it is on no team of its own.
        match on_server::<TeamStatus>(&owned, reqwest::Method::GET, "", None).await {
            Ok(mut status) => {
                status.remote = Some(owned.0);
                status.listening = None;
                status.suggested_urls = Vec::new();
                return Ok(Json(status));
            }
            Err(error) => tracing::debug!("[team] the owned team was not read: {error}"),
        }
    }
    let status = blocking(|| {
        let path = team::team_path();
        let file = team::read_team(&path)?;
        let spaces = team::hosted_spaces(&shared_space::settings_path())?;
        // A Wenlan that hosts nothing and has no members is on no team of
        // its own: no team.json is made for it.
        let info = if spaces.is_empty() && file.members.is_empty() && file.listen.is_none() {
            file.info()
        } else {
            Some(team::team_info(&path)?)
        };
        let port = listening()
            .and_then(|addr| addr.parse::<std::net::SocketAddr>().ok())
            .map(|addr| addr.port())
            .unwrap_or(DEFAULT_TEAM_PORT);
        Ok(TeamStatus {
            team: info,
            members: file.members.iter().map(Member::to_wire).collect(),
            everyone: file
                .everyone
                .iter()
                .map(|(space, access)| wenlan_types::team::TeamSpaceAccess {
                    space: space.clone(),
                    access: *access,
                    id: None,
                })
                .collect(),
            spaces: spaces.into_iter().map(|(name, _)| name).collect(),
            listening: listening(),
            url: env_value(TEAM_URL_ENV).or(file.url.clone()),
            suggested_urls: suggested_urls(port),
            remote: None,
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
    if let Some(owned) = managed_elsewhere().await? {
        let info: wenlan_types::team::TeamInfo =
            on_server(&owned, reqwest::Method::PUT, "", json_of(&req)).await?;
        // The new name shows here at once, not after the next sync pass.
        let (url, remembered) = (owned.0.clone(), info.clone());
        blocking(move || {
            shared_space::remember_team(&shared_space::settings_path(), &url, &remembered)
        })
        .await?;
        return Ok(Json(info));
    }
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
    Json(mut req): Json<TeamAddMemberRequest>,
) -> Result<Json<TeamMemberKey>, ServerError> {
    if let Some(owned) = managed_elsewhere().await? {
        return Ok(Json(
            on_server(&owned, reqwest::Method::POST, "/members", json_of(&req)).await?,
        ));
    }
    if req.url.as_deref().is_none_or(|url| url.trim().is_empty()) {
        req.url = link_address().await?;
    }
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
    if let Some(owned) = managed_elsewhere().await? {
        let path = format!("/members/{}", encoded(&id));
        return Ok(Json(
            on_server(&owned, reqwest::Method::PUT, &path, json_of(&req)).await?,
        ));
    }
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
    if let Some(owned) = managed_elsewhere().await? {
        let path = format!("/members/{}", encoded(&id));
        return Ok(Json(
            on_server(&owned, reqwest::Method::DELETE, &path, None).await?,
        ));
    }
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
    if let Some(owned) = managed_elsewhere().await? {
        let mut path = format!("/members/{}/key", encoded(&id));
        if let Some(url) = query.url.as_deref().filter(|url| !url.trim().is_empty()) {
            path.push_str(&format!("?url={}", encoded(url)));
        }
        return Ok(Json(
            on_server(&owned, reqwest::Method::POST, &path, None).await?,
        ));
    }
    let url = match query.url.filter(|url| !url.trim().is_empty()) {
        Some(url) => Some(url),
        None => link_address().await?,
    };
    let (member, key) = blocking(move || team::new_member_key(&team::team_path(), &id)).await?;
    Ok(Json(member_key(member, key, url.as_deref())?))
}

/// The address put in invite links when the caller gives none: the one
/// the owner set (or `WENLAN_TEAM_URL`).
async fn link_address() -> Result<Option<String>, ServerError> {
    if let Some(url) = env_value(TEAM_URL_ENV) {
        return Ok(Some(url));
    }
    blocking(|| Ok(team::read_team(&team::team_path())?.url)).await
}

/// POST /api/team/spaces — hosts a Space of this Wenlan for the team: it is
/// shared through a folder of this Wenlan's own, which members reach only
/// through the team listener. Runs the first pass and returns its counts.
pub async fn handle_host_space(
    State(state): State<SharedState>,
    Json(req): Json<TeamHostSpaceRequest>,
) -> Result<Json<wenlan_types::shared_space::SharedSyncCounts>, ServerError> {
    if let Some(owned) = managed_elsewhere().await? {
        // The server makes the Space; this computer follows it at once.
        let counts = on_server(&owned, reqwest::Method::POST, "/spaces", json_of(&req)).await?;
        let db = { state.read().await.db.clone() };
        if let Some(db) = db {
            shared_space::follow_teams_now(&db, &shared_space::settings_path()).await;
        }
        return Ok(Json(counts));
    }
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
    if let Some(new_name) = req.rename_to.as_deref() {
        let description = db
            .get_space(&space)
            .await
            .map_err(|e| ServerError::Internal(e.to_string()))?
            .and_then(|space| space.description);
        rename_space(&db, &space, new_name, description.as_deref()).await?;
        return Ok(Json(Default::default()));
    }
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
        // Newly hosted. Access given to it beforehand stays: whatever
        // ended an earlier hosting of this name (stop hosting, delete) took
        // that one's access with it, and a hosted Space cannot be renamed.
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
    State(state): State<SharedState>,
    Json(req): Json<TeamJoinRequest>,
) -> Result<Json<TeamJoinResponse>, ServerError> {
    let (url, key) = join_parts(&req.code, req.key.as_deref())?;
    let mut me = remote::me(&url, &key)
        .await
        .map_err(ServerError::ValidationError)?;
    // The name joining takes on (see handle_remember_membership), not the
    // stand-in a server nobody sits at starts its owner with.
    if me.role == TeamRole::Owner && me.name == OWNER_STAND_IN {
        if let Some(name) = own_name(&state).await {
            me.name = name;
        }
    }
    Ok(Json(TeamJoinResponse { url, me }))
}

/// POST /api/team/start — makes this computer a team's server: names the
/// team, names its owner (the person here, as the team sees what they
/// write), and turns the team listener on, now and after every restart.
pub async fn handle_start_team(
    State(state): State<SharedState>,
    Json(req): Json<TeamStartRequest>,
) -> Result<Json<TeamStatus>, ServerError> {
    let person = req.person.trim().to_string();
    if person.is_empty() || person.chars().count() > 80 {
        return Err(ServerError::ValidationError(
            "your name needs 1 to 80 characters".into(),
        ));
    }
    let port = req.port.unwrap_or(DEFAULT_TEAM_PORT);
    let url = match req
        .url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
    {
        Some(url) => Some(remote::check_url(url).map_err(ServerError::ValidationError)?),
        None => suggested_urls(port).into_iter().next(),
    };
    let db = { state.read().await.db.clone() }.ok_or(ServerError::DbNotInitialized)?;
    if let Some(profile) = db
        .get_profile()
        .await
        .map_err(|e| ServerError::Internal(e.to_string()))?
    {
        db.update_profile(&profile.id, None, Some(&person), None, None, None)
            .await
            .map_err(|e| ServerError::Internal(e.to_string()))?;
    }
    let listen = format!("0.0.0.0:{port}");
    let name = req.name.clone();
    {
        let listen = listen.clone();
        blocking(move || {
            let path = team::team_path();
            team::rename_team(&path, &name)?;
            team::set_listen(&path, Some(&listen), url.as_deref())
        })
        .await?;
    }
    if env_value(TEAM_ADDR_ENV).is_none() {
        LISTEN.send_replace(Some(listen));
    }
    handle_team_status().await
}

/// POST /api/team/stop — turns the team listener off: members' computers
/// stop syncing with this one until it is on again. The team, its people
/// and Spaces stay. (A listener set by `WENLAN_TEAM_ADDR` stays on.)
pub async fn handle_stop_team() -> Result<Json<TeamStatus>, ServerError> {
    blocking(|| team::set_listen(&team::team_path(), None, None)).await?;
    LISTEN.send_replace(None);
    handle_team_status().await
}

/// POST /api/team/membership — keeps that this computer is on the team an
/// invite link is for, even before any Space is open to this person; their
/// Spaces follow as the team opens them, and an owner manages it from here.
pub async fn handle_remember_membership(
    State(state): State<SharedState>,
    Json(req): Json<TeamMembershipRequest>,
) -> Result<Json<wenlan_types::team::TeamMe>, ServerError> {
    let (url, key) = join_parts(&req.code, None)?;
    let mut me = remote::me(&url, &key)
        .await
        .map_err(ServerError::ValidationError)?;
    // The owner of a server nobody sits at starts with a stand-in name; the
    // team sees their own name from the moment they take it over.
    if me.role == TeamRole::Owner && me.name == OWNER_STAND_IN {
        if let Some(name) = own_name(&state).await {
            let path = format!("/members/{}", encoded(&me.member_id));
            let body = serde_json::json!({ "name": name });
            match remote::admin(&url, &key, reqwest::Method::PUT, &path, Some(&body)).await {
                Ok(_) => me.name = name,
                Err(error) => tracing::warn!("[team] the owner's name was not set: {error}"),
            }
        }
    }
    {
        let me = me.clone();
        blocking(move || {
            shared_space::remember_membership(&shared_space::settings_path(), &url, &key, &me)
        })
        .await?;
    }
    Ok(Json(me))
}

#[derive(Debug, Default, Deserialize)]
pub struct MembershipQuery {
    pub url: String,
}

/// DELETE /api/team/membership?url= — this person left the team at `url`.
pub async fn handle_forget_membership(
    Query(query): Query<MembershipQuery>,
) -> Result<Json<serde_json::Value>, ServerError> {
    let url = remote::check_url(&query.url).map_err(ServerError::ValidationError)?;
    blocking(move || shared_space::forget_membership(&shared_space::settings_path(), &url)).await?;
    Ok(Json(serde_json::json!({ "left": true })))
}

/// Addresses others can likely reach this computer at on `port`: its
/// Tailscale address first, then its address on the local network. Read
/// from the routes the system would use (no packet is sent).
fn suggested_urls(port: u16) -> Vec<String> {
    fn local_ip(probe: &str) -> Option<std::net::IpAddr> {
        let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
        socket.connect(probe).ok()?;
        Some(socket.local_addr().ok()?.ip())
    }
    let mut out = Vec::new();
    // Tailscale's own address; only a computer on a tailnet routes it.
    if let Some(std::net::IpAddr::V4(ip)) = local_ip("100.100.100.100:53") {
        let [a, b, ..] = ip.octets();
        if a == 100 && (64..128).contains(&b) {
            out.push(format!("http://{ip}:{port}"));
        }
    }
    if let Some(std::net::IpAddr::V4(ip)) = local_ip("192.0.2.1:80") {
        let url = format!("http://{ip}:{port}");
        if ip.is_private() && !out.contains(&url) {
            out.push(url);
        }
    }
    out
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

/// Runs the team listener while it is on: `WENLAN_TEAM_ADDR`, or the
/// address the owner turned it on at from the app (kept in `team.json`, so
/// it is on again after a restart). Turning it off or on from the app
/// stops or starts it without a restart. A listener that cannot start is
/// logged; the daemon goes on without it.
pub async fn serve_team_listener(shared: SharedState, shutdown: ShutdownHandle) {
    let port = { shared.read().await.bound_port };
    let state = TeamState {
        team_path: team::team_path(),
        settings_path: shared_space::settings_path(),
        daemon_url: format!("http://127.0.0.1:{port}"),
        mcp: Arc::default(),
    };
    let kept = {
        let path = state.team_path.clone();
        tokio::task::spawn_blocking(move || team::read_team(&path).map(|file| file.listen))
            .await
            .ok()
            .and_then(Result::ok)
            .flatten()
    };
    LISTEN.send_replace(kept);
    if env_value(TEAM_ADDR_ENV).is_some() {
        owner_link_for_headless_server(&state).await;
    }
    let mut changes = LISTEN.subscribe();
    let stop = shutdown.subscribe();
    loop {
        changes.borrow_and_update();
        if crate::lifecycle::shutdown_requested(&stop) {
            return;
        }
        let Some(addr) = listening() else {
            tokio::select! {
                _ = crate::lifecycle::wait_for_shutdown(stop.clone()) => return,
                changed = changes.changed() => if changed.is_err() { return },
            }
            continue;
        };
        let listener = match tokio::net::TcpListener::bind(&addr).await {
            Ok(listener) => listener,
            Err(error) => {
                tracing::error!("[team] cannot listen on {addr}: {error}");
                tokio::select! {
                    _ = crate::lifecycle::wait_for_shutdown(stop.clone()) => return,
                    changed = changes.changed() => if changed.is_err() { return },
                }
                continue;
            }
        };
        tracing::info!(
            "[team] team listener on http://{addr}: only /health, /join, /team/v1 and /mcp, each checked against a member key. Serve it over HTTPS or a private network (Tailscale)."
        );
        let app = team_router(state.clone());
        let mut until = changes.clone();
        let stop_for = stop.clone();
        let ended = axum::serve(listener, app.into_make_service())
            .with_graceful_shutdown(async move {
                tokio::select! {
                    _ = crate::lifecycle::wait_for_shutdown(stop_for) => {}
                    _ = until.changed() => {}
                }
            })
            .await;
        if let Err(error) = ended {
            tracing::error!("[team] team listener stopped: {error}");
            return;
        }
        if crate::lifecycle::shutdown_requested(&stop) {
            return;
        }
        // Turned off, or moved: the loop listens again as it says now.
    }
}

/// A server nobody sits at (started with `WENLAN_TEAM_ADDR`, as in
/// Docker): the first start makes the team's owner and logs their invite
/// link once. The owner opens it in their own Wenlan and manages the team
/// from there. `WENLAN_TEAM_NEW_OWNER_LINK` makes a new link (the old one
/// stops working).
async fn owner_link_for_headless_server(state: &TeamState) {
    let path = state.team_path.clone();
    let renew = env_value(NEW_OWNER_LINK_ENV).is_some();
    let given = env_value(OWNER_SHA256_ENV);
    let made =
        tokio::task::spawn_blocking(move || -> Result<_, wenlan_core::error::WenlanError> {
            if let Some(hash) = given {
                // The owner's Wenlan made this key and joins with it by
                // itself; a restart with the same command changes nothing.
                if team::is_owner_hash(&path, &hash)? {
                    return Ok(None);
                }
                if team::has_owner(&path)? && !renew {
                    tracing::warn!(
                        "[team] the team already has an owner, so the owner's key in the start command is not used. To make it the owner's, start the server once with {NEW_OWNER_LINK_ENV}=1."
                    );
                    return Ok(None);
                }
                team::set_owner_hash(&path, OWNER_STAND_IN, &hash)?;
                println!(
                    "\nWenlan team server is running. The Wenlan that made the start command joins it by itself as the team's owner.\n"
                );
                tracing::info!("[team] the owner's key (its SHA-256) from the start command was taken");
                return Ok(None);
            }
            if team::has_owner(&path)? && !renew {
                return Ok(None);
            }
            let file = team::read_team(&path)?;
            let (_, key) = team::owner_key(&path, OWNER_STAND_IN)?;
            Ok(Some((key, file.url)))
        })
        .await;
    let (key, kept_url) = match made {
        Ok(Ok(Some(made))) => made,
        Ok(Ok(None)) => return,
        Ok(Err(error)) => {
            tracing::error!("[team] the owner's invite link was not made: {error}");
            return;
        }
        Err(_) => return,
    };
    let port = listening()
        .and_then(|addr| addr.parse::<std::net::SocketAddr>().ok())
        .map(|addr| addr.port())
        .unwrap_or(DEFAULT_TEAM_PORT);
    let url = env_value(TEAM_URL_ENV)
        .or(kept_url)
        .or_else(|| suggested_urls(port).into_iter().next())
        .unwrap_or_else(|| format!("http://localhost:{port}"));
    if env_value(TEAM_URL_ENV).is_none() {
        tracing::warn!(
            "[team] {TEAM_URL_ENV} is not set: the invite links use {url}, a guess. Set it to the address people reach this server at."
        );
    }
    let link = wenlan_types::team::invite_link(url.trim_end_matches('/'), &key);
    // Shown once, on the server's own output: whoever reads it first is
    // the owner, so it is not written anywhere else.
    println!(
        "\nWenlan team server: the owner's invite link (open it in your own Wenlan: Settings › Team › Join with an invite link):\n\n  {link}\n\nIt is shown only now. To make a new one, start the server once with {NEW_OWNER_LINK_ENV}=1.\n"
    );
    tracing::info!("[team] the owner's invite link was printed to the server's output");
}

fn team_router(state: TeamState) -> axum::Router {
    use axum::routing::{any, get, put};
    let checked = axum::Router::new()
        .route("/team/v1/me", get(team_me))
        .route("/team/v1/admin", any(team_admin))
        .route("/team/v1/admin/{*rest}", any(team_admin))
        .route("/team/v1/spaces/{space}/files", get(team_files))
        .route("/team/v1/spaces/{space}/file", get(team_file))
        .route(
            "/team/v1/spaces/{space}/versions/{page}/{name}",
            put(team_put_version),
        )
        .route("/mcp", any(team_mcp))
        .route("/mcp/{*space}", any(team_mcp_in))
        .layer(middleware::from_fn(wenlan_mcp::serve::guard_initialization))
        .layer(middleware::from_fn(capped_body))
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

/// Reads a member's request body up to the size of the largest version,
/// whoever handles it next: MCP hands the raw request on, and nothing
/// after it would otherwise stop a body of any size.
async fn capped_body(req: Request, next: Next) -> Response {
    let (parts, body) = req.into_parts();
    match axum::body::to_bytes(body, remote::MAX_VERSION_BYTES).await {
        Ok(bytes) => {
            next.run(Request::from_parts(parts, Body::from(bytes)))
                .await
        }
        Err(_) => (StatusCode::PAYLOAD_TOO_LARGE, "the request is too large").into_response(),
    }
}

async fn team_health() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok", "server": "wenlan-team" }))
}

#[derive(Clone, Copy, Serialize)]
struct JoinPageStrings {
    lang: &'static str,
    heading: &'static str,
    unnamed_heading: &'static str,
    unnamed_team: &'static str,
    paste: &'static str,
    download_before: &'static str,
    download: &'static str,
    download_after: &'static str,
    connect: &'static str,
    space: &'static str,
    read_only: &'static str,
    tool_question: &'static str,
    claude_command: &'static str,
    codex_config: &'static str,
    cursor_config: &'static str,
    copy: &'static str,
    copied: &'static str,
    copy_failed: &'static str,
    private: &'static str,
    expired: &'static str,
}

const JOIN_PAGE_EN: JoinPageStrings = JoinPageStrings {
    lang: "en",
    heading: "Join {team} on Wenlan",
    unnamed_heading: "Join {team} on Wenlan",
    unnamed_team: "a team",
    paste: "Copy this page's address. In Wenlan, open Settings, then Team, choose Join with an invite link, and paste it.",
    download_before: "Don't have Wenlan yet?",
    download: "Download it",
    download_after: ", then come back to this link.",
    connect: "Don't want to install? Connect your AI tool",
    space: "Space",
    read_only: "Your AI tool can search this Space, but it cannot change pages.",
    tool_question: "Which AI tool do you use?",
    claude_command: "Run this command:",
    codex_config: "Add to ~/.codex/config.toml:",
    cursor_config: "Add to ~/.cursor/mcp.json:",
    copy: "Copy",
    copied: "Copied",
    copy_failed: "Couldn't copy. Select the text above and copy it yourself.",
    private: "Keep this private. It lets an AI tool act as you.",
    expired: "This link no longer works. Ask whoever invited you for a new one.",
};

const JOIN_PAGE_ZH_HANS: JoinPageStrings = JoinPageStrings {
    lang: "zh-Hans",
    heading: "加入 {team}",
    unnamed_heading: "加入{team}",
    unnamed_team: "团队",
    paste:
        "复制这个页面的地址。在文澜中打开“设置”，然后打开“团队”，选择“用邀请链接加入团队”并粘贴。",
    download_before: "还没安装文澜？",
    download: "下载文澜",
    download_after: "，然后再打开这个链接。",
    connect: "不想安装？直接连接你的 AI 工具",
    space: "Space",
    read_only: "你的 AI 工具可以搜索这个 Space，但不能更改页面。",
    tool_question: "你用哪个 AI 工具？",
    claude_command: "运行这条命令：",
    codex_config: "添加到 ~/.codex/config.toml：",
    cursor_config: "添加到 ~/.cursor/mcp.json：",
    copy: "复制",
    copied: "已复制",
    copy_failed: "未能复制。请选中上面的文字并手动复制。",
    private: "请妥善保管。AI 工具可以用它以你的身份操作。",
    expired: "这个链接已经失效。请让邀请你的人重新发一个。",
};

const JOIN_PAGE_ZH_HANT: JoinPageStrings = JoinPageStrings {
    lang: "zh-Hant",
    heading: "加入 {team}",
    unnamed_heading: "加入{team}",
    unnamed_team: "團隊",
    paste: "複製這個頁面的位址。在文瀾中打開「設定」，再打開「團隊」，選擇「用邀請連結加入團隊」並貼上。",
    download_before: "還沒安裝文瀾？",
    download: "下載文瀾",
    download_after: "，然後再打開這個連結。",
    connect: "不想安裝？直接連接你的 AI 工具",
    space: "Space",
    read_only: "你的 AI 工具可以搜尋這個 Space，但不能更改頁面。",
    tool_question: "你用哪個 AI 工具？",
    claude_command: "執行這行指令：",
    codex_config: "加入 ~/.codex/config.toml：",
    cursor_config: "加入 ~/.cursor/mcp.json：",
    copy: "複製",
    copied: "已複製",
    copy_failed: "未能複製。請選取上面的文字並手動複製。",
    private: "請妥善保管。AI 工具可以用它以你的身分操作。",
    expired: "這個連結已經失效。請讓邀請你的人重新傳一個。",
};

fn join_page_strings(headers: &HeaderMap) -> &'static JoinPageStrings {
    let languages = headers
        .get(header::ACCEPT_LANGUAGE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let mut preferred = None;
    let mut preferred_quality = -1.0_f32;
    for language in languages.split(',') {
        let mut parts = language.split(';');
        let tag = parts.next().unwrap_or_default().trim().to_ascii_lowercase();
        let quality = parts
            .find_map(|part| part.trim().strip_prefix("q="))
            .map(|quality| quality.parse::<f32>().unwrap_or(0.0))
            .unwrap_or(1.0)
            .clamp(0.0, 1.0);
        if quality == 0.0 {
            continue;
        }
        let candidate = if matches!(tag.as_str(), "zh-hant" | "zh-tw" | "zh-hk" | "zh-mo")
            || tag.starts_with("zh-hant-")
        {
            Some(&JOIN_PAGE_ZH_HANT)
        } else if tag == "zh" || tag.starts_with("zh-") {
            Some(&JOIN_PAGE_ZH_HANS)
        } else if tag == "en" || tag.starts_with("en-") || tag == "*" {
            Some(&JOIN_PAGE_EN)
        } else {
            None
        };
        if candidate.is_some() && quality > preferred_quality {
            preferred = candidate;
            preferred_quality = quality;
        }
    }
    preferred.unwrap_or(&JOIN_PAGE_EN)
}

fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn join_page_script_strings(strings: &JoinPageStrings) -> String {
    // JSON strings are valid JavaScript literals, but `<` must still be
    // escaped because the HTML parser, not JavaScript, recognizes `</script>`.
    serde_json::to_string(strings)
        .expect("join-page strings serialize")
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

const JOIN_PAGE_TEMPLATE: &str = r#"<!doctype html>
<html lang="%%LANG%%">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>%%HEADING%%</title>
<style nonce="%%NONCE%%">
:root { color-scheme: light dark; }
* { box-sizing: border-box; }
body { margin:0; background:#f7f7f5; color:#202124; font:16px/1.55 system-ui,-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif; }
main { width:min(38rem,100%); margin:10vh auto; padding:0 20px 48px; }
h1 { margin:0 0 12px; font-size:clamp(1.7rem,7vw,2.3rem); line-height:1.15; letter-spacing:-.025em; }
p { margin:8px 0; }
a { color:#5146b8; }
button,select { font:inherit; }
button { border:0; border-radius:8px; cursor:pointer; }
#connect { margin-top:18px; padding:10px 14px; background:#5751c9; color:white; font-weight:600; }
#connect:disabled { opacity:.55; cursor:default; }
[hidden] { display:none !important; }
#error { margin-top:18px; }
#setup { margin-top:24px; }
.chooser { display:flex; align-items:center; gap:10px; margin:12px 0; }
.chooser select { min-width:0; max-width:100%; padding:6px 8px; }
.readonly { color:#6a5a18; }
.tool-question { margin:20px 0 8px; font-weight:600; }
.tools { display:flex; max-width:100%; gap:8px; }
.tool { flex:1 1 0; min-width:0; padding:8px; border-radius:8px; overflow:hidden; background:#e8e7e2; color:#202124; font-size:.9rem; line-height:1.4; text-overflow:ellipsis; white-space:nowrap; }
.tool.is-selected { background:#5751c9; color:white; font-weight:600; }
.tool:focus-visible,.copy:focus-visible { outline:3px solid #5146b8; outline-offset:2px; }
.snippet { min-width:0; margin-top:16px; }
.snippet-head { display:flex; flex-wrap:wrap; align-items:center; justify-content:space-between; gap:8px 12px; }
.instruction { margin:0; color:#66676b; font-size:.88rem; }
.copy { padding:5px 10px; background:#e8e7e2; color:#202124; }
pre { max-width:100%; margin:0; padding:12px; overflow:auto; border-radius:8px; background:#ecece8; font:13px/1.45 ui-monospace,SFMono-Regular,Consolas,monospace; white-space:pre-wrap; overflow-wrap:anywhere; }
.copy-status { min-height:1.25em; margin:4px 0 0; color:#66676b; font-size:.82rem; }
.private { margin-top:20px; font-size:.9rem; font-weight:600; }
@media (prefers-color-scheme:dark) {
  body { background:#16171b; color:#ececf1; }
  a { color:#aaa5ff; }
  .instruction,.copy-status { color:#aaaab2; }
  .tool { background:#303137; color:#ececf1; }
  .copy { background:#303137; color:#ececf1; }
  .tool.is-selected { background:#7770e7; color:white; }
  .tool:focus-visible,.copy:focus-visible { outline-color:#aaa5ff; }
  pre { background:#25262b; }
  .readonly { color:#e1c86b; }
}
@media (max-width:480px) {
  main { margin:7vh auto; padding-inline:16px; }
}
</style>
</head>
<body>
<main>
<h1>%%HEADING%%</h1>
<p>%%PASTE%%</p>
<p>%%DOWNLOAD_BEFORE%% <a href="https://wenlan.app">%%DOWNLOAD%%</a>%%DOWNLOAD_AFTER%%</p>
<button id="connect" type="button">%%CONNECT%%</button>
<p id="error" hidden>%%EXPIRED%%</p>
<section id="setup" hidden>
  <label class="chooser" id="chooser" hidden><span>%%SPACE%%</span><select id="spaces"></select></label>
  <p class="tool-question" id="tool-question">%%TOOL_QUESTION%%</p>
  <div class="tools" role="group" aria-labelledby="tool-question">
    <button class="tool" id="tool-claude" type="button" data-tool="claude" aria-pressed="false">Claude Code</button>
    <button class="tool" id="tool-codex" type="button" data-tool="codex" aria-pressed="false">Codex</button>
    <button class="tool" id="tool-cursor" type="button" data-tool="cursor" aria-pressed="false">Cursor</button>
  </div>
  <section class="snippet" id="snippet" hidden>
    <div class="snippet-head">
      <p class="instruction" id="instruction"></p>
      <button class="copy" id="copy" type="button">%%COPY%%</button>
    </div>
    <pre tabindex="0"><code id="snippet-text"></code></pre>
    <p class="copy-status" id="copy-status" aria-live="polite"></p>
    <p class="readonly" id="readonly" hidden>%%READ_ONLY%%</p>
    <p class="private">%%PRIVATE%%</p>
  </section>
</section>
</main>
<script nonce="%%NONCE%%">
(() => {
  const text = %%SCRIPT_STRINGS%%;
  const connect = document.getElementById('connect');
  const error = document.getElementById('error');
  const setup = document.getElementById('setup');
  const chooser = document.getElementById('chooser');
  const spacesNode = document.getElementById('spaces');
  const readonlyNode = document.getElementById('readonly');
  const snippet = document.getElementById('snippet');
  const instruction = document.getElementById('instruction');
  const snippetText = document.getElementById('snippet-text');
  const copy = document.getElementById('copy');
  const copyStatus = document.getElementById('copy-status');
  const toolButtons = {
    claude: document.getElementById('tool-claude'),
    codex: document.getElementById('tool-codex'),
    cursor: document.getElementById('tool-cursor')
  };
  const instructions = {
    claude: text.claude_command,
    codex: text.codex_config,
    cursor: text.cursor_config
  };
  let selectedTool = null;
  let snippets = null;
  let readOnly = false;

  const fail = () => {
    setup.hidden = true;
    error.hidden = false;
    connect.disabled = false;
  };

  const selectText = (node) => {
    const selection = window.getSelection();
    if (!selection) return;
    const range = document.createRange();
    range.selectNodeContents(node);
    selection.removeAllRanges();
    selection.addRange(range);
    node.parentElement.focus();
  };

  const renderTool = () => {
    Object.entries(toolButtons).forEach(([tool, button]) => {
      const selected = tool === selectedTool;
      button.setAttribute('aria-pressed', String(selected));
      button.classList.toggle('is-selected', selected);
    });
    if (!selectedTool || !snippets) {
      snippet.hidden = true;
      readonlyNode.hidden = true;
      return;
    }
    instruction.textContent = instructions[selectedTool];
    snippetText.textContent = snippets[selectedTool];
    copy.textContent = text.copy;
    copyStatus.textContent = '';
    readonlyNode.hidden = !readOnly;
    snippet.hidden = false;
  };

  Object.entries(toolButtons).forEach(([tool, button]) => {
    button.addEventListener('click', () => {
      selectedTool = tool;
      renderTool();
    });
  });

  copy.addEventListener('click', async () => {
    try {
      await navigator.clipboard.writeText(snippetText.textContent);
      copy.textContent = text.copied;
      copyStatus.textContent = '';
    } catch (_) {
      selectText(snippetText);
      copyStatus.textContent = text.copy_failed;
    }
  });

  connect.addEventListener('click', async () => {
    const key = window.location.hash.slice(1);
    if (!key) {
      fail();
      return;
    }
    connect.disabled = true;
    error.hidden = true;
    try {
      const response = await fetch('/team/v1/me', {
        method: 'GET',
        headers: { Authorization: 'Bearer ' + key },
        credentials: 'omit',
        cache: 'no-store',
        referrerPolicy: 'no-referrer'
      });
      if (!response.ok) throw new Error('join refused');
      const me = await response.json();
      const spaces = Array.isArray(me.spaces) ? me.spaces : [];
      if (spaces.length === 0) throw new Error('no Space');

      spacesNode.replaceChildren();
      spaces.forEach((entry, index) => {
        const option = document.createElement('option');
        option.value = String(index);
        option.textContent = entry.space;
        spacesNode.append(option);
      });
      chooser.hidden = spaces.length === 1;

      const update = () => {
        const entry = spaces[Number(spacesNode.value || 0)];
        const address = spaces.length === 1
          ? window.location.origin + '/mcp'
          : window.location.origin + '/mcp/' + encodeURIComponent(entry.space);
        readOnly = entry.access === 'read';
        snippets = {
          claude: 'claude mcp add --transport http wenlan-team ' + address
            + ' --header "Authorization: Bearer ' + key + '"',
          codex: '[mcp_servers.wenlan-team]\nurl = "' + address
            + '"\nhttp_headers = { Authorization = "Bearer ' + key + '" }',
          cursor: JSON.stringify({
            mcpServers: { 'wenlan-team': { url: address, headers: { Authorization: 'Bearer ' + key } } }
          })
        };
        renderTool();
      };
      spacesNode.addEventListener('change', update, { once: false });
      update();
      setup.hidden = false;
      connect.hidden = true;
    } catch (_) {
      fail();
    }
  });
})();
</script>
</body>
</html>
"#;

fn render_join_page(name: Option<&str>, strings: &JoinPageStrings, nonce: &str) -> String {
    let (heading, name) = match name {
        Some(name) => (strings.heading, name),
        None => (strings.unnamed_heading, strings.unnamed_team),
    };
    let heading = html_escape(&heading.replace("{team}", name));
    let mut page = JOIN_PAGE_TEMPLATE.to_string();
    let escaped = [
        ("%%LANG%%", html_escape(strings.lang)),
        ("%%HEADING%%", heading),
        ("%%NONCE%%", html_escape(nonce)),
        ("%%PASTE%%", html_escape(strings.paste)),
        ("%%DOWNLOAD_BEFORE%%", html_escape(strings.download_before)),
        ("%%DOWNLOAD%%", html_escape(strings.download)),
        ("%%DOWNLOAD_AFTER%%", html_escape(strings.download_after)),
        ("%%CONNECT%%", html_escape(strings.connect)),
        ("%%SPACE%%", html_escape(strings.space)),
        ("%%READ_ONLY%%", html_escape(strings.read_only)),
        ("%%TOOL_QUESTION%%", html_escape(strings.tool_question)),
        ("%%COPY%%", html_escape(strings.copy)),
        ("%%PRIVATE%%", html_escape(strings.private)),
        ("%%EXPIRED%%", html_escape(strings.expired)),
    ];
    for (marker, value) in escaped {
        page = page.replace(marker, &value);
    }
    page.replace("%%SCRIPT_STRINGS%%", &join_page_script_strings(strings))
}

/// GET /join — where an invite link opens in a browser. The key after `#`
/// never reaches the server. The browser can use it only to read `/team/v1/me`
/// and build the member's AI-tool configuration locally.
async fn team_join_page(State(state): State<TeamState>, headers: HeaderMap) -> Response {
    let name = team::read_team(&state.team_path)
        .ok()
        .and_then(|file| file.name);
    let strings = join_page_strings(&headers);
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let page = render_join_page(name.as_deref(), strings, &nonce);
    let csp = format!(
        "default-src 'none'; script-src 'nonce-{nonce}'; style-src 'nonce-{nonce}'; connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'"
    );
    let mut response = page.into_response();
    let response_headers = response.headers_mut();
    response_headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    response_headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response_headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response_headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_str(&csp).expect("join-page CSP is a valid header"),
    );
    response
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

async fn team_me(State(state): State<TeamState>, Extension(member): Extension<Member>) -> Response {
    let settings = state.settings_path.clone();
    let hosted = tokio::task::spawn_blocking(move || team::hosted_spaces(&settings)).await;
    match hosted {
        Ok(Ok(hosted)) => Json(member.me(&hosted)).into_response(),
        _ => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "the Spaces cannot be read",
        )
            .into_response(),
    }
}

/// `/team/v1/admin…` — the team's owner manages it from their own
/// computer: the same calls as this Wenlan's own `/api/team…`, made here
/// over loopback for them. Anyone else is refused (403).
async fn team_admin(
    State(state): State<TeamState>,
    Extension(member): Extension<Member>,
    req: Request,
) -> Response {
    if member.role != TeamRole::Owner {
        return (StatusCode::FORBIDDEN, "only the team's owner").into_response();
    }
    let method = req.method().clone();
    let rest = req
        .uri()
        .path()
        .strip_prefix("/team/v1/admin")
        .unwrap_or("")
        .to_string();
    let query = req
        .uri()
        .query()
        .map(|query| format!("?{query}"))
        .unwrap_or_default();
    // Only plain segments: no `.`, `\`, `%` or anything else a URL parser
    // further on could read as another path on the daemon's own API.
    let plain = rest
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'_' | b'-'));
    let allowed = plain
        && matches!(
            (
                method.as_str(),
                rest.split('/').collect::<Vec<_>>().as_slice(),
            ),
            ("GET" | "PUT", [""])
                | ("POST", ["", "members" | "spaces"])
                | ("PUT" | "DELETE", ["", "members", _])
                | ("POST", ["", "members", _, "key"])
        );
    if !allowed {
        return (StatusCode::NOT_FOUND, "no such team call").into_response();
    }
    let body = match axum::body::to_bytes(req.into_body(), remote::MAX_VERSION_BYTES).await {
        Ok(body) => body,
        Err(_) => return (StatusCode::BAD_REQUEST, "the request is too large").into_response(),
    };
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
    {
        Ok(client) => client,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    // A Space the owner makes from their computer is made here first: the
    // server has nobody at it to make one.
    if rest == "/spaces" {
        if let Ok(host) = serde_json::from_slice::<TeamHostSpaceRequest>(&body) {
            if host.everyone.is_none() && host.rename_to.is_none() {
                let made = client
                    .post(format!("{}/api/spaces", state.daemon_url))
                    .json(&serde_json::json!({ "name": host.space.trim() }))
                    .send()
                    .await;
                if let Err(error) = made {
                    tracing::warn!("[team] the owner's Space was not made: {error}");
                }
            }
        }
    }
    let method = match reqwest::Method::from_bytes(method.as_str().as_bytes()) {
        Ok(method) => method,
        Err(_) => return StatusCode::METHOD_NOT_ALLOWED.into_response(),
    };
    let mut call = client.request(
        method,
        format!("{}/api/team{rest}{query}", state.daemon_url),
    );
    if !body.is_empty() {
        call = call
            .header(header::CONTENT_TYPE, "application/json")
            .body(body.to_vec());
    }
    match call.send().await {
        Ok(answer) => {
            let status = StatusCode::from_u16(answer.status().as_u16())
                .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            let bytes = answer.bytes().await.unwrap_or_default();
            (
                status,
                [(header::CONTENT_TYPE, "application/json")],
                Body::from(bytes),
            )
                .into_response()
        }
        Err(_) => (StatusCode::BAD_GATEWAY, "the team server did not answer").into_response(),
    }
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
    // The same list `/team/v1/me` gives, so the address the invite page
    // shows for "one Space" always opens: a Space open to everyone counts
    // exactly like one given to this member by name.
    let settings = state.settings_path.clone();
    let hosted = match tokio::task::spawn_blocking(move || team::hosted_spaces(&settings)).await {
        Ok(Ok(hosted)) => hosted,
        _ => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "the Spaces cannot be read",
            )
                .into_response()
        }
    };
    let mut spaces: Vec<String> = member
        .me(&hosted)
        .spaces
        .into_iter()
        .map(|entry| entry.space)
        .filter(|space| member.access(space).is_some())
        .collect();
    match spaces.len() {
        1 => {
            let space = spaces.remove(0);
            serve_mcp(state, member, space, req).await
        }
        0 => (StatusCode::FORBIDDEN, "this member has no Space").into_response(),
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
