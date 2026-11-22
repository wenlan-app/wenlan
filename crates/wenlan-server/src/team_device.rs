// SPDX-License-Identifier: Apache-2.0
//! Connecting an AI tool with a short code (`wenlan login`), and the
//! member's own account calls: their AI tools and a desktop link.
//!
//! ```text
//! CLI                      browser (any device)            this server
//!  |-- POST /auth/device {tool} ----------------------------------->|
//!  |<-- device_code (long, secret), user_code ABCD-EFGH, address ---|
//!  |                        | /app/device: type the short code      |
//!  |                        |-- POST /auth/device/lookup ---------->|
//!  |                        |-- POST /auth/device/approve --------->| tool key made
//!  |-- POST /auth/device/token {device_code} every 5 s ------------>|
//!  |<-- the key, this once; the request is gone after --------------|
//! ```
//!
//! Only the long `device_code` collects the key, so a short code that leaks
//! (it is typed, read aloud, seen) gives nobody a key. A short code lives 10
//! minutes; a member who types five wrong ones is refused until those 10
//! minutes pass, and every call is behind the per-address rate limit.
//! Allowing makes a credential that lasts, so a browser session must have
//! signed in again within [`REAUTH_WINDOW`]; a member key may allow it.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::extract::{Path as UrlPath, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use sha2::{Digest, Sha256};
use wenlan_core::team::{self, Credential, Member};
use wenlan_types::team::{
    TeamDesktopLink, TeamDeviceDecision, TeamDeviceGrant, TeamDeviceLookup, TeamDevicePoll,
    TeamDeviceRequest, TeamDeviceStart, DEVICE_CODE_ALPHABET,
};

use super::{drop_member_mcp, own_origin, TeamState};
use crate::team_session::{self, REAUTH_WINDOW};

/// How long both codes work.
pub(crate) const DEVICE_TTL: Duration = Duration::from_secs(10 * 60);
/// Seconds the CLI waits between polls; a faster poll is told to slow down.
pub(crate) const POLL_INTERVAL: u64 = 5;
/// Requests waiting at once, across the whole server.
const DEVICE_CAP: usize = 1_000;
/// Wrong short codes a member may type within [`DEVICE_TTL`].
pub(crate) const MISSES_ALLOWED: u32 = 5;
/// The longest tool name kept.
const TOOL_NAME_MAX: usize = 60;
/// Where the person types the short code.
pub(crate) const DEVICE_PAGE: &str = "/app/device";

type Hash = [u8; 32];

fn hash(text: &str) -> Hash {
    Sha256::digest(text.as_bytes()).into()
}

enum Status {
    Pending,
    /// Allowed, and the key is being made.
    Issuing,
    Denied,
    Ready(TeamDeviceGrant),
}

struct Grant {
    user_code: String,
    tool: String,
    created: Instant,
    last_poll: Option<Instant>,
    status: Status,
}

/// What a poll learns.
pub(crate) enum Poll {
    Pending,
    SlowDown,
    Denied,
    Expired,
    Ready(TeamDeviceGrant),
}

/// The requests waiting for a person, in memory only: a restart ends them,
/// and the CLI is told to start again.
pub(crate) struct DeviceStore {
    grants: Mutex<HashMap<Hash, Grant>>,
    misses: Mutex<HashMap<String, (u32, Instant)>>,
    ttl: Duration,
    interval: Duration,
}

impl Default for DeviceStore {
    fn default() -> Self {
        Self {
            grants: Mutex::default(),
            misses: Mutex::default(),
            ttl: DEVICE_TTL,
            interval: Duration::from_secs(POLL_INTERVAL),
        }
    }
}

fn random_code() -> Option<String> {
    // Rejection sampling: 248 = 31 * 8, so every character is equally likely.
    let mut code = String::with_capacity(8);
    let mut bytes = [0u8; 32];
    while code.len() < 8 {
        getrandom::fill(&mut bytes).ok()?;
        for byte in bytes {
            if code.len() == 8 {
                break;
            }
            if usize::from(byte) < DEVICE_CODE_ALPHABET.len() * 8 {
                code.push(char::from(
                    DEVICE_CODE_ALPHABET[usize::from(byte) % DEVICE_CODE_ALPHABET.len()],
                ));
            }
        }
    }
    Some(code)
}

fn random_hex() -> Option<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).ok()?;
    Some(hex::encode(bytes))
}

/// `ABCDEFGH` as people read it: `ABCD-EFGH`.
pub(crate) fn shown_code(code: &str) -> String {
    format!("{}-{}", &code[..4], &code[4..])
}

impl DeviceStore {
    fn grants(&self) -> std::sync::MutexGuard<'_, HashMap<Hash, Grant>> {
        self.grants
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// How long both codes work.
    pub(crate) fn ttl(&self) -> Duration {
        self.ttl
    }

    /// Test knobs.
    #[cfg(test)]
    pub(crate) fn tuned(mut self, ttl: Duration, interval: Duration) -> Self {
        self.ttl = ttl;
        self.interval = interval;
        self
    }

    /// A new request for `tool`: the device code and the short code. None
    /// when the server has no randomness or holds too many already.
    pub(crate) fn start(&self, tool: &str) -> Option<(String, String)> {
        let device_code = random_hex()?;
        let mut grants = self.grants();
        let ttl = self.ttl;
        grants.retain(|_, grant| grant.created.elapsed() < ttl);
        if grants.len() >= DEVICE_CAP {
            return None;
        }
        let user_code = loop {
            let code = random_code()?;
            if !grants.values().any(|grant| grant.user_code == code) {
                break code;
            }
        };
        grants.insert(
            hash(&device_code),
            Grant {
                user_code: user_code.clone(),
                tool: tool.to_string(),
                created: Instant::now(),
                last_poll: None,
                status: Status::Pending,
            },
        );
        Some((device_code, user_code))
    }

    /// The tool a live, undecided short code is for.
    pub(crate) fn tool_for(&self, user_code: &str) -> Option<String> {
        let grants = self.grants();
        grants
            .values()
            .find(|grant| {
                grant.user_code == user_code
                    && grant.created.elapsed() < self.ttl
                    && matches!(grant.status, Status::Pending)
            })
            .map(|grant| grant.tool.clone())
    }

    /// Marks a live, undecided short code as being allowed, so nobody else
    /// allows it too. Returns the tool and the grant's id for [`Self::ready`].
    pub(crate) fn begin_issue(&self, user_code: &str) -> Option<(Hash, String)> {
        let mut grants = self.grants();
        let ttl = self.ttl;
        let (id, grant) = grants.iter_mut().find(|(_, grant)| {
            grant.user_code == user_code
                && grant.created.elapsed() < ttl
                && matches!(grant.status, Status::Pending)
        })?;
        grant.status = Status::Issuing;
        Some((*id, grant.tool.clone()))
    }

    pub(crate) fn ready(&self, id: &Hash, outcome: Option<TeamDeviceGrant>) {
        if let Some(grant) = self.grants().get_mut(id) {
            grant.status = match outcome {
                Some(ready) => Status::Ready(ready),
                None => Status::Pending,
            };
        }
    }

    /// Refuses a live, undecided short code. False when there is none.
    pub(crate) fn deny(&self, user_code: &str) -> bool {
        let mut grants = self.grants();
        let ttl = self.ttl;
        match grants.values_mut().find(|grant| {
            grant.user_code == user_code
                && grant.created.elapsed() < ttl
                && matches!(grant.status, Status::Pending)
        }) {
            Some(grant) => {
                grant.status = Status::Denied;
                true
            }
            None => false,
        }
    }

    /// The CLI asks with its device code. A key is handed over once: the
    /// request is gone after, as it is once refused or expired.
    pub(crate) fn poll(&self, device_code: &str) -> Poll {
        let id = hash(device_code);
        let mut grants = self.grants();
        let Some(grant) = grants.get_mut(&id) else {
            return Poll::Expired;
        };
        if grant.created.elapsed() >= self.ttl {
            grants.remove(&id);
            return Poll::Expired;
        }
        let now = Instant::now();
        let too_soon = grant
            .last_poll
            .is_some_and(|last| now.saturating_duration_since(last) < self.interval);
        grant.last_poll = Some(now);
        match grant.status {
            Status::Pending | Status::Issuing if too_soon => Poll::SlowDown,
            Status::Pending | Status::Issuing => Poll::Pending,
            Status::Denied => {
                grants.remove(&id);
                Poll::Denied
            }
            Status::Ready(_) => match grants.remove(&id).map(|grant| grant.status) {
                Some(Status::Ready(ready)) => Poll::Ready(ready),
                _ => Poll::Expired,
            },
        }
    }

    /// Whether this member may still type a short code.
    pub(crate) fn may_try(&self, member_id: &str) -> bool {
        let mut misses = self
            .misses
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let ttl = self.ttl;
        misses.retain(|_, (_, first)| first.elapsed() < ttl);
        misses
            .get(member_id)
            .is_none_or(|(count, _)| *count < MISSES_ALLOWED)
    }

    pub(crate) fn missed(&self, member_id: &str) {
        let mut misses = self
            .misses
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entry = misses
            .entry(member_id.to_string())
            .or_insert((0, Instant::now()));
        entry.0 += 1;
    }
}

fn error(status: StatusCode, code: &'static str) -> Response {
    (status, Json(serde_json::json!({ "error": code }))).into_response()
}

/// A tool name worth showing: trimmed, no control characters, not empty,
/// at most [`TOOL_NAME_MAX`] characters.
pub(crate) fn clean_tool(name: &str) -> Option<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().any(char::is_control) {
        return None;
    }
    Some(name.chars().take(TOOL_NAME_MAX).collect())
}

pub(crate) async fn read_json<T: serde::de::DeserializeOwned>(req: Request) -> Option<T> {
    let bytes = axum::body::to_bytes(req.into_body(), 4 * 1024).await.ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// `POST /auth/device` with `{"tool": "Claude Code"}`: anyone may ask; only
/// a member who types the short code can allow it.
pub(super) async fn start(State(state): State<TeamState>, req: Request) -> Response {
    if let Some(limited) = super::auth::limited(&state, &req) {
        return limited;
    }
    let Some(origin) = own_origin(&state).await else {
        return error(StatusCode::NOT_FOUND, "no_address");
    };
    let Some(TeamDeviceRequest { tool }) = read_json(req).await else {
        return error(StatusCode::BAD_REQUEST, "invalid_request");
    };
    let Some(tool) = clean_tool(&tool) else {
        return error(StatusCode::BAD_REQUEST, "invalid_request");
    };
    let Some((device_code, user_code)) = state.device.start(&tool) else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "busy");
    };
    Json(TeamDeviceStart {
        device_code,
        user_code: shown_code(&user_code),
        verification_uri: format!("{origin}{DEVICE_PAGE}"),
        expires_in: state.device.ttl.as_secs(),
        interval: POLL_INTERVAL,
    })
    .into_response()
}

/// `POST /auth/device/token` with `{"device_code": "…"}`: the CLI polls.
/// Answers as RFC 8628 does: `authorization_pending`, `slow_down`,
/// `access_denied`, `expired_token`, or the key.
pub(super) async fn token(State(state): State<TeamState>, req: Request) -> Response {
    if let Some(limited) = super::auth::limited(&state, &req) {
        return limited;
    }
    let Some(TeamDevicePoll { device_code }) = read_json(req).await else {
        return error(StatusCode::BAD_REQUEST, "invalid_request");
    };
    match state.device.poll(device_code.trim()) {
        Poll::Pending => error(StatusCode::BAD_REQUEST, "authorization_pending"),
        Poll::SlowDown => error(StatusCode::BAD_REQUEST, "slow_down"),
        Poll::Denied => error(StatusCode::BAD_REQUEST, "access_denied"),
        Poll::Expired => error(StatusCode::BAD_REQUEST, "expired_token"),
        Poll::Ready(grant) => Json(grant).into_response(),
    }
}

/// The short code in a decision body, checked: its shape, and the member's
/// count of wrong ones.
async fn typed_code(state: &TeamState, member: &Member, req: Request) -> Result<String, Response> {
    if !state.device.may_try(&member.id) {
        return Err(error(StatusCode::TOO_MANY_REQUESTS, "too_many_tries"));
    }
    let typed = read_json::<TeamDeviceDecision>(req)
        .await
        .and_then(|body| wenlan_types::team::normalize_user_code(&body.user_code));
    match typed {
        Some(code) if state.device.tool_for(&code).is_some() => Ok(code),
        _ => {
            state.device.missed(&member.id);
            Err(error(StatusCode::NOT_FOUND, "unknown_code"))
        }
    }
}

/// The Spaces this member can use now, as `/team/v1/me` lists them.
async fn member_spaces(
    state: &TeamState,
    member: &Member,
) -> Option<Vec<wenlan_types::team::TeamSpaceAccess>> {
    let settings = state.settings_path.clone();
    let hosted = tokio::task::spawn_blocking(move || team::hosted_spaces(&settings))
        .await
        .ok()?
        .ok()?;
    Some(member.me(&hosted).spaces)
}

/// `POST /auth/device/lookup` with `{"user_code": "ABCD-EFGH"}`: which tool
/// asks, for the consent screen.
pub(super) async fn lookup(
    State(state): State<TeamState>,
    Extension(member): Extension<Member>,
    req: Request,
) -> Response {
    let code = match typed_code(&state, &member, req).await {
        Ok(code) => code,
        Err(refused) => return refused,
    };
    let Some(tool) = state.device.tool_for(&code) else {
        return error(StatusCode::NOT_FOUND, "unknown_code");
    };
    let Some(spaces) = member_spaces(&state, &member).await else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    Json(TeamDeviceLookup { tool, spaces }).into_response()
}

/// Whether a browser session signed in again within [`REAUTH_WINDOW`]. A
/// member key always may: it is the member's own strongest credential.
fn fresh(state: &TeamState, credential: &Credential, headers: &HeaderMap) -> bool {
    match credential {
        Credential::BrowserSession => team_session::cookie_token(headers)
            .is_some_and(|token| state.sessions.fresh(&token, REAUTH_WINDOW)),
        Credential::LegacyKey => true,
        Credential::ToolKey { .. } => false,
    }
}

fn reauth_required() -> Response {
    error(StatusCode::FORBIDDEN, "reauth_required")
}

/// `POST /auth/device/approve` with the short code: makes the tool key for
/// the CLI to collect.
pub(super) async fn approve(
    State(state): State<TeamState>,
    Extension(member): Extension<Member>,
    Extension(credential): Extension<Credential>,
    req: Request,
) -> Response {
    if !fresh(&state, &credential, req.headers()) {
        return reauth_required();
    }
    let code = match typed_code(&state, &member, req).await {
        Ok(code) => code,
        Err(refused) => return refused,
    };
    let Some((id, tool)) = state.device.begin_issue(&code) else {
        return error(StatusCode::NOT_FOUND, "unknown_code");
    };
    let Some(spaces) = member_spaces(&state, &member).await else {
        state.device.ready(&id, None);
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let path = state.team_path.clone();
    let member_id = member.id.clone();
    let issued =
        tokio::task::spawn_blocking(move || team::issue_tool_key(&path, &member_id, &tool)).await;
    match issued {
        Ok(Ok((key_id, key))) => {
            state.device.ready(
                &id,
                Some(TeamDeviceGrant {
                    key,
                    key_id,
                    spaces,
                    team: member.team.clone(),
                }),
            );
            StatusCode::NO_CONTENT.into_response()
        }
        _ => {
            state.device.ready(&id, None);
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
    }
}

/// `POST /auth/device/deny` with the short code: the CLI is told no.
pub(super) async fn deny(
    State(state): State<TeamState>,
    Extension(member): Extension<Member>,
    req: Request,
) -> Response {
    let code = match typed_code(&state, &member, req).await {
        Ok(code) => code,
        Err(refused) => return refused,
    };
    if state.device.deny(&code) {
        StatusCode::NO_CONTENT.into_response()
    } else {
        error(StatusCode::NOT_FOUND, "unknown_code")
    }
}

// ===== The member's own account =====

/// `GET /team/v1/account/keys`: the caller's AI tools.
pub(super) async fn keys(Extension(member): Extension<Member>) -> Response {
    Json(member.tool_keys()).into_response()
}

/// `DELETE /team/v1/account/keys/{id}`: removes one of the caller's AI
/// tools; it is refused at its next request. Answers the tools left.
pub(super) async fn revoke(
    State(state): State<TeamState>,
    Extension(member): Extension<Member>,
    UrlPath(key_id): UrlPath<String>,
) -> Response {
    if !member.tool_keys().iter().any(|key| key.id == key_id) {
        return error(StatusCode::NOT_FOUND, "unknown_key");
    }
    let path = state.team_path.clone();
    let member_id = member.id.clone();
    let gone = key_id.clone();
    let revoked =
        tokio::task::spawn_blocking(move || team::revoke_tool_key(&path, &member_id, &key_id))
            .await;
    match revoked {
        Ok(Ok(())) => {
            drop_member_mcp(&state.mcp, &member.id);
            // The tools left, for the list shown next.
            let left: Vec<_> = member
                .tool_keys()
                .into_iter()
                .filter(|key| key.id != gone)
                .collect();
            Json(left).into_response()
        }
        _ => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

/// `DELETE /team/v1/account/accounts/{provider}`: unlinks the caller's
/// Google or GitHub account. Refused (409 `last_sign_in`) when it is the
/// only way they sign in. Their browser sessions keep going; the account no
/// longer signs anyone in. Answers the accounts left.
pub(super) async fn unlink(
    State(state): State<TeamState>,
    Extension(member): Extension<Member>,
    UrlPath(provider): UrlPath<String>,
) -> Response {
    if !matches!(provider.as_str(), "google" | "github") {
        return error(StatusCode::NOT_FOUND, "unknown_provider");
    }
    let path = state.team_path.clone();
    let member_id = member.id.clone();
    match tokio::task::spawn_blocking(move || team::unlink_provider(&path, &member_id, &provider))
        .await
    {
        Ok(Ok(left)) => Json(left).into_response(),
        Ok(Err(wenlan_core::error::WenlanError::NotFound(_))) => {
            error(StatusCode::NOT_FOUND, "not_linked")
        }
        Ok(Err(wenlan_core::error::WenlanError::Validation(_))) => {
            error(StatusCode::CONFLICT, "last_sign_in")
        }
        _ => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

/// `POST /team/v1/account/desktop-link`: a one-time link with a new member
/// key for the desktop app (`<address>/join#wlt_…`). Only for a browser
/// session that signed in again just now: a member who has a key already
/// uses that one, and a new key would stop it.
pub(super) async fn desktop_link(
    State(state): State<TeamState>,
    Extension(member): Extension<Member>,
    Extension(credential): Extension<Credential>,
    headers: HeaderMap,
) -> Response {
    if !matches!(credential, Credential::BrowserSession) {
        return error(StatusCode::CONFLICT, "has_key");
    }
    if !fresh(&state, &credential, &headers) {
        return reauth_required();
    }
    let Some(origin) = own_origin(&state).await else {
        return error(StatusCode::NOT_FOUND, "no_address");
    };
    let path = state.team_path.clone();
    let member_id = member.id.clone();
    match tokio::task::spawn_blocking(move || team::new_member_key(&path, &member_id)).await {
        Ok(Ok((_, key))) => Json(TeamDesktopLink {
            link: wenlan_types::team::invite_link(&origin, &key),
        })
        .into_response(),
        _ => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}
