// SPDX-License-Identifier: AGPL-3.0-only
//! Personal cloud: the entrance (`wenlan-server gateway`).
//!
//! Everyone reaches the personal cloud here first. The entrance signs people
//! in with Google or GitHub (the team server's sign-in code), finds their
//! account in its account table, asks the [`Locator`] where their Wenlan is,
//! and forwards the request there with a pass ([`crate::cloud_pass`]). It
//! keeps nothing of anyone's pages; each person's Wenlan never signs anyone
//! in.
//!
//! Accounts are made ahead of time (`wenlan-server cloud-add-account`), each
//! with the email of the person it is for. The first sign-in whose provider
//! vouches for that email attaches the provider's account to it; from then
//! on that account alone, `(issuer, subject)`, finds it.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{ConnectInfo, Path as UrlPath, Query, Request, State};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use crate::cloud_locator::{AccountId, LocateError, Locator};
use crate::cloud_pass::{PassKey, Via, PASS_HEADER};
use crate::team_routes::auth::{self, AuthState, Failure, Purpose};
use crate::team_routes::device::{self as device_flow, DeviceStore, Poll, DEVICE_PAGE};
use crate::team_session::{self, SessionStore};

/// The account table, in the entrance's folder.
pub const ACCOUNTS_FILE: &str = "accounts.json";
/// The entrance's signing key, in its folder.
pub const KEY_FILE: &str = "gateway.key";
/// The largest request body forwarded (the largest page version a person's
/// Wenlan takes).
const MAX_BODY: usize = 32 * 1024 * 1024;

// ===== The account table =====

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Accounts {
    #[serde(default)]
    pub accounts: Vec<CloudAccount>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudAccount {
    pub id: String,
    /// The address of the person it is for, as it was made.
    pub email: String,
    /// The provider accounts that sign in to it.
    #[serde(default)]
    pub identities: Vec<CloudIdentity>,
    /// The person's computers, each with its own key.
    #[serde(default)]
    pub devices: Vec<CloudDevice>,
}

/// One of a person's computers. Only the key's hash is kept.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudDevice {
    pub id: String,
    pub name: String,
    pub key_sha256: String,
    /// RFC 3339.
    pub added_at: String,
}

/// A computer as the person sees it in their list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub added_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudIdentity {
    pub issuer: String,
    pub subject: String,
}

/// What a provider proved about who is signing in.
pub struct Proof<'a> {
    pub issuer: &'a str,
    pub subject: &'a str,
    /// The addresses the provider vouches this person owns now.
    pub trusted_emails: &'a [String],
}

fn lock(root: &Path) -> std::io::Result<std::fs::File> {
    use fs2::FileExt;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(root.join("accounts.lock"))?;
    file.lock_exclusive()?;
    Ok(file)
}

fn read_accounts(root: &Path) -> std::io::Result<Accounts> {
    match std::fs::read(root.join(ACCOUNTS_FILE)) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Accounts::default()),
        Err(error) => Err(error),
    }
}

fn write_accounts(root: &Path, accounts: &Accounts) -> std::io::Result<()> {
    let mut file = tempfile::NamedTempFile::new_in(root)?;
    serde_json::to_writer_pretty(&mut file, accounts)?;
    file.as_file().sync_all()?;
    file.persist(root.join(ACCOUNTS_FILE))
        .map_err(|error| error.error)?;
    Ok(())
}

fn same_email(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

/// Makes an account for the person with `email`. One email has one
/// account.
pub fn add_account(root: &Path, email: &str) -> std::io::Result<AccountId> {
    let email = email.trim();
    if !email.contains('@') {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "an account needs the email of the person it is for",
        ));
    }
    std::fs::create_dir_all(root)?;
    let _held = lock(root)?;
    let mut accounts = read_accounts(root)?;
    if accounts
        .accounts
        .iter()
        .any(|a| same_email(&a.email, email))
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "this email already has an account",
        ));
    }
    let id = AccountId::new();
    accounts.accounts.push(CloudAccount {
        id: id.to_string(),
        email: email.to_string(),
        identities: Vec::new(),
        devices: Vec::new(),
    });
    write_accounts(root, &accounts)?;
    Ok(id)
}

/// The account this sign-in is for: the one its provider account is
/// attached to, else the one made for an email the provider vouches for,
/// which it is then attached to. `None` when there is none.
pub fn account_for(root: &Path, proof: &Proof<'_>) -> std::io::Result<Option<AccountId>> {
    let _held = lock(root)?;
    let mut accounts = read_accounts(root)?;
    let attached = accounts.accounts.iter().find(|account| {
        account
            .identities
            .iter()
            .any(|i| i.issuer == proof.issuer && i.subject == proof.subject)
    });
    if let Some(account) = attached {
        return Ok(AccountId::parse(&account.id));
    }
    let Some(account) = accounts.accounts.iter_mut().find(|account| {
        proof
            .trusted_emails
            .iter()
            .any(|email| same_email(email, &account.email))
            // One provider account each: a second Google account with the
            // same address does not take it over.
            && !account.identities.iter().any(|i| i.issuer == proof.issuer)
    }) else {
        return Ok(None);
    };
    account.identities.push(CloudIdentity {
        issuer: proof.issuer.to_string(),
        subject: proof.subject.to_string(),
    });
    let id = AccountId::parse(&account.id);
    write_accounts(root, &accounts)?;
    Ok(id)
}

/// What every computer key starts with.
pub const DEVICE_KEY_PREFIX: &str = "wlc_";
/// Computers one account may have at once.
const DEVICES_PER_ACCOUNT: usize = 20;

fn key_hash(key: &str) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(key.as_bytes()))
}

fn random_hex(bytes: usize) -> std::io::Result<String> {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).map_err(|_| std::io::Error::other("no randomness"))?;
    Ok(hex::encode(buf))
}

fn not_found(what: &'static str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::NotFound, what)
}

/// Gives `account` a new computer called `name`: its id and its key, which
/// is not kept.
pub fn add_device(
    root: &Path,
    account: &AccountId,
    name: &str,
) -> std::io::Result<(String, String)> {
    let _held = lock(root)?;
    let mut accounts = read_accounts(root)?;
    let entry = accounts
        .accounts
        .iter_mut()
        .find(|a| a.id == account.as_str())
        .ok_or_else(|| not_found("no such account"))?;
    if entry.devices.len() >= DEVICES_PER_ACCOUNT {
        return Err(std::io::Error::new(
            std::io::ErrorKind::QuotaExceeded,
            "too many computers; remove one first",
        ));
    }
    let id = format!("dev_{}", random_hex(8)?);
    let key = format!("{DEVICE_KEY_PREFIX}{}", random_hex(32)?);
    entry.devices.push(CloudDevice {
        id: id.clone(),
        name: name.to_string(),
        key_sha256: key_hash(&key),
        added_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    });
    write_accounts(root, &accounts)?;
    Ok((id, key))
}

/// Removes one of `account`'s computers; its key stops working at once.
/// False when it has no such computer.
pub fn remove_device(root: &Path, account: &AccountId, id: &str) -> std::io::Result<bool> {
    let _held = lock(root)?;
    let mut accounts = read_accounts(root)?;
    let Some(entry) = accounts
        .accounts
        .iter_mut()
        .find(|a| a.id == account.as_str())
    else {
        return Ok(false);
    };
    let before = entry.devices.len();
    entry.devices.retain(|d| d.id != id);
    if entry.devices.len() == before {
        return Ok(false);
    }
    write_accounts(root, &accounts)?;
    Ok(true)
}

/// `account`'s computers, oldest first.
pub fn devices(root: &Path, account: &AccountId) -> std::io::Result<Vec<DeviceInfo>> {
    let accounts = read_accounts(root)?;
    Ok(accounts
        .accounts
        .iter()
        .find(|a| a.id == account.as_str())
        .map(|a| {
            a.devices
                .iter()
                .map(|d| DeviceInfo {
                    id: d.id.clone(),
                    name: d.name.clone(),
                    added_at: d.added_at.clone(),
                })
                .collect()
        })
        .unwrap_or_default())
}

/// The account a computer key belongs to. Read from the table each time,
/// so a removed computer is refused at its next request.
pub fn account_for_key(root: &Path, key: &str) -> std::io::Result<Option<AccountId>> {
    if !key.starts_with(DEVICE_KEY_PREFIX) || key.len() != DEVICE_KEY_PREFIX.len() + 64 {
        return Ok(None);
    }
    let hashed = key_hash(key);
    let accounts = read_accounts(root)?;
    Ok(accounts
        .accounts
        .iter()
        .find(|a| a.devices.iter().any(|d| d.key_sha256 == hashed))
        .and_then(|a| AccountId::parse(&a.id)))
}

/// Whether the provider account `(issuer, subject)` signs in to `account`.
fn has_identity(root: &Path, account: &AccountId, issuer: &str, subject: &str) -> bool {
    read_accounts(root).is_ok_and(|accounts| {
        accounts.accounts.iter().any(|a| {
            a.id == account.as_str()
                && a.identities
                    .iter()
                    .any(|i| i.issuer == issuer && i.subject == subject)
        })
    })
}

// ===== The entrance =====

/// How the entrance runs.
pub struct GatewaySettings {
    /// The `https://` address people reach the personal cloud at.
    pub origin: String,
    /// Where the account table and the key are kept.
    pub root: PathBuf,
}

pub struct Gateway<L: Locator> {
    locator: Arc<L>,
    auth: AuthState,
    origin: String,
    root: PathBuf,
    sessions: SessionStore,
    pass_key: PassKey,
    http: reqwest::Client,
    device: DeviceStore,
}

impl<L: Locator> Gateway<L> {
    pub(crate) fn new(
        locator: Arc<L>,
        auth: AuthState,
        settings: GatewaySettings,
        pass_key: PassKey,
    ) -> Self {
        Self {
            locator,
            auth,
            origin: settings.origin.trim_end_matches('/').to_string(),
            root: settings.root,
            sessions: SessionStore::default(),
            pass_key,
            http: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(5))
                .build()
                .unwrap_or_default(),
            device: DeviceStore::default(),
        }
    }

    /// The account of this browser's live session.
    fn session_account(&self, headers: &HeaderMap) -> Option<AccountId> {
        team_session::cookie_token(headers)
            .and_then(|token| self.sessions.lookup(&token))
            .and_then(|id| AccountId::parse(&id))
    }

    /// Whether a page call comes from the entrance's own pages.
    fn same_origin(&self, headers: &HeaderMap) -> bool {
        headers
            .get(header::ORIGIN)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|origin| origin == self.origin)
    }

    /// Whether this browser's session signed in again just now.
    fn fresh(&self, headers: &HeaderMap) -> bool {
        team_session::cookie_token(headers)
            .is_some_and(|token| self.sessions.fresh(&token, team_session::REAUTH_WINDOW))
    }
}

/// The entrance's routes.
pub fn router<L: Locator>(gateway: Arc<Gateway<L>>) -> axum::Router {
    use axum::routing::{any, get, post};
    let auth_routes = axum::Router::new()
        .route("/auth/providers", get(providers))
        .route("/auth/start/{provider}", get(start))
        .route("/auth/callback/{provider}", get(callback))
        .route("/auth/signout", post(sign_out))
        .route("/auth/reauth/{provider}", post(reauth))
        .route("/auth/device", post(device_start))
        .route("/auth/device/token", post(device_token))
        .route("/auth/device/lookup", post(device_lookup))
        .route("/auth/device/approve", post(device_approve))
        .route("/auth/device/deny", post(device_deny))
        .route("/auth/devices", get(device_list))
        .route("/auth/devices/{id}", axum::routing::delete(device_remove))
        // Nothing else under `/auth` is a person's Wenlan's to answer.
        .route("/auth/{*rest}", any(|| async { StatusCode::NOT_FOUND }))
        .layer(axum::middleware::from_fn_with_state(
            gateway.clone(),
            limit::<L>,
        ))
        .layer(axum::middleware::from_fn(no_store));
    axum::Router::new()
        .route("/health", get(|| async { "ok" }))
        .merge(auth_routes)
        .fallback(forward::<L>)
        .with_state(gateway)
}

async fn limit<L: Locator>(
    State(gateway): State<Arc<Gateway<L>>>,
    req: Request,
    next: Next,
) -> Response {
    let peer = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|info| info.0.ip());
    if !gateway.auth.allow(peer) {
        return (StatusCode::TOO_MANY_REQUESTS, "too many requests").into_response();
    }
    next.run(req).await
}

async fn no_store(req: Request, next: Next) -> Response {
    let mut response = next.run(req).await;
    let h = response.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    response
}

fn redirect(to: &str, cookies: &[String]) -> Response {
    let mut response = StatusCode::FOUND.into_response();
    if let Ok(to) = HeaderValue::from_str(to) {
        response.headers_mut().insert(header::LOCATION, to);
    }
    for cookie in cookies {
        if let Ok(cookie) = HeaderValue::from_str(cookie) {
            response.headers_mut().append(header::SET_COOKIE, cookie);
        }
    }
    response
}

fn failed(failure: Failure) -> Response {
    let (status, text) = match failure {
        Failure::Off => (StatusCode::NOT_FOUND, "not found"),
        Failure::NotInTeam => (
            StatusCode::FORBIDDEN,
            "There is no Wenlan account for this sign-in yet.",
        ),
        Failure::Refused(reason) => {
            tracing::warn!("[cloud] sign-in refused: {reason}");
            (
                StatusCode::FORBIDDEN,
                "Sign-in did not complete. Try again.",
            )
        }
        Failure::Unavailable(reason) => {
            tracing::warn!("[cloud] sign-in unavailable: {reason}");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "Sign-in is not available right now. Try again soon.",
            )
        }
    };
    let mut response = (status, text).into_response();
    if let Ok(cookie) = HeaderValue::from_str(&auth::clear_login_cookie()) {
        response.headers_mut().append(header::SET_COOKIE, cookie);
    }
    response
}

/// `GET /auth/providers`: `{"providers":["google","github"]}`.
async fn providers<L: Locator>(State(gateway): State<Arc<Gateway<L>>>) -> Response {
    axum::Json(serde_json::json!({ "providers": gateway.auth.names() })).into_response()
}

/// `GET /auth/start/{provider}`: sends the browser to the provider.
async fn start<L: Locator>(
    State(gateway): State<Arc<Gateway<L>>>,
    UrlPath(provider): UrlPath<String>,
    headers: HeaderMap,
) -> Response {
    match auth::authorize(
        &gateway.auth,
        &gateway.origin,
        &provider,
        Purpose::SignIn,
        &headers,
    )
    .await
    {
        Ok((url, cookie)) => redirect(&url, &[cookie]),
        Err(failure) => failed(failure),
    }
}

/// `GET /auth/callback/{provider}`: the provider sends the browser back.
async fn callback<L: Locator>(
    State(gateway): State<Arc<Gateway<L>>>,
    UrlPath(provider): UrlPath<String>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let proven =
        match auth::prove(&gateway.auth, &gateway.origin, &provider, &query, &headers).await {
            Ok(proven) => proven,
            Err(failure) => return failed(failure),
        };
    match &proven.purpose {
        Purpose::SignIn => {}
        Purpose::Reauth { member_id } => {
            return match reauthed(&gateway, member_id, &proven, &headers).await {
                Ok(response) => response,
                Err(failure) => failed(failure),
            }
        }
        _ => return failed(Failure::Refused("not a sign-in")),
    }
    let account = &proven.account;
    let trusted: Vec<String> = match (&proven.provider.kind, account.email.as_deref()) {
        // GitHub verifies an address by mailing it.
        (auth::Kind::GitHub(_), _) => account.verified_emails.clone(),
        (auth::Kind::Oidc, Some(email))
            if auth::google_email_trust(email, account.hosted_domain)
                == wenlan_core::team::EmailTrust::Trusted =>
        {
            vec![email.to_string()]
        }
        (auth::Kind::Oidc, _) => Vec::new(),
    };
    let root = gateway.root.clone();
    let (issuer, subject) = (account.issuer.clone(), account.subject.clone());
    let found = tokio::task::spawn_blocking(move || {
        account_for(
            &root,
            &Proof {
                issuer: &issuer,
                subject: &subject,
                trusted_emails: &trusted,
            },
        )
    })
    .await;
    let account_id = match found {
        Ok(Ok(Some(id))) => id,
        Ok(Ok(None)) => return failed(Failure::NotInTeam),
        _ => return failed(Failure::Unavailable("the account table cannot be read")),
    };
    if let Some(old) = team_session::cookie_token(&headers) {
        gateway.sessions.delete(&old);
    }
    let Ok(token) = gateway.sessions.create_fresh(account_id.as_str()) else {
        return failed(Failure::Unavailable("no randomness"));
    };
    redirect(
        "/app/",
        &[team_session::set_cookie(&token), auth::clear_login_cookie()],
    )
}

/// The callback of a sign-in again: only for the session that asked, with a
/// provider account already attached to its account, and (Google) one that
/// authenticated just now. The browser gets a new, fresh session.
async fn reauthed<L: Locator>(
    gateway: &Gateway<L>,
    account_id: &str,
    proven: &auth::Proven,
    headers: &HeaderMap,
) -> Result<Response, Failure> {
    let account = AccountId::parse(account_id).ok_or(Failure::Refused("bad account"))?;
    if gateway.session_account(headers).as_ref() != Some(&account) {
        return Err(Failure::Refused("no session of this account"));
    }
    let root = gateway.root.clone();
    let (issuer, subject) = (
        proven.account.issuer.clone(),
        proven.account.subject.clone(),
    );
    let who = account.clone();
    let theirs = tokio::task::spawn_blocking(move || has_identity(&root, &who, &issuer, &subject))
        .await
        .unwrap_or(false);
    if !theirs {
        return Err(Failure::Refused("not an account of this person"));
    }
    if matches!(proven.provider.kind, auth::Kind::Oidc) {
        let at = proven
            .account
            .auth_time
            .ok_or(Failure::Refused("no auth_time"))?;
        let now = chrono::Utc::now();
        let window = chrono::Duration::from_std(team_session::REAUTH_WINDOW).unwrap_or_default();
        if at > now + auth::CLOCK_SKEW
            || now - at > window
            || at < proven.created_wall - auth::CLOCK_SKEW
        {
            return Err(Failure::Refused("not a fresh authentication"));
        }
    }
    if let Some(old) = team_session::cookie_token(headers) {
        gateway.sessions.delete(&old);
    }
    let token = gateway
        .sessions
        .create_fresh(account.as_str())
        .map_err(|_| Failure::Unavailable("no randomness"))?;
    Ok(redirect(
        "/app/",
        &[team_session::set_cookie(&token), auth::clear_login_cookie()],
    ))
}

/// `POST /auth/reauth/{provider}`: a signed-in page asks to sign in again
/// (before allowing a computer). Answers `{"url": …}` to navigate to.
async fn reauth<L: Locator>(
    State(gateway): State<Arc<Gateway<L>>>,
    UrlPath(provider): UrlPath<String>,
    headers: HeaderMap,
) -> Response {
    if !gateway.same_origin(&headers) {
        return (StatusCode::FORBIDDEN, "origin not allowed").into_response();
    }
    let Some(account) = gateway.session_account(&headers) else {
        return (StatusCode::UNAUTHORIZED, "sign in first").into_response();
    };
    let purpose = Purpose::Reauth {
        member_id: account.to_string(),
    };
    match auth::authorize(&gateway.auth, &gateway.origin, &provider, purpose, &headers).await {
        Ok((url, cookie)) => {
            let mut response = axum::Json(serde_json::json!({ "url": url })).into_response();
            if let Ok(cookie) = HeaderValue::from_str(&cookie) {
                response.headers_mut().append(header::SET_COOKIE, cookie);
            }
            response
        }
        Err(failure) => failed(failure),
    }
}

// ===== Connecting a computer =====
//
// The team server's short-code flow (`team_device.rs`), the same calls and
// answers: the computer asks, the person types the code at `/app/device`
// while signed in, and the computer collects a key of its own.

fn device_error(status: StatusCode, code: &'static str) -> Response {
    (status, axum::Json(serde_json::json!({ "error": code }))).into_response()
}

/// `POST /auth/device` with `{"tool": "Qi's MacBook"}`: a computer asks.
async fn device_start<L: Locator>(
    State(gateway): State<Arc<Gateway<L>>>,
    req: Request,
) -> Response {
    let Some(wenlan_types::team::TeamDeviceRequest { tool }) = device_flow::read_json(req).await
    else {
        return device_error(StatusCode::BAD_REQUEST, "invalid_request");
    };
    let Some(name) = device_flow::clean_tool(&tool) else {
        return device_error(StatusCode::BAD_REQUEST, "invalid_request");
    };
    let Some((device_code, user_code)) = gateway.device.start(&name) else {
        return device_error(StatusCode::SERVICE_UNAVAILABLE, "busy");
    };
    axum::Json(wenlan_types::team::TeamDeviceStart {
        device_code,
        user_code: device_flow::shown_code(&user_code),
        verification_uri: format!("{}{DEVICE_PAGE}", gateway.origin),
        expires_in: gateway.device.ttl().as_secs(),
        interval: device_flow::POLL_INTERVAL,
    })
    .into_response()
}

/// `POST /auth/device/token` with `{"device_code": "…"}`: the computer polls.
async fn device_token<L: Locator>(
    State(gateway): State<Arc<Gateway<L>>>,
    req: Request,
) -> Response {
    let Some(wenlan_types::team::TeamDevicePoll { device_code }) =
        device_flow::read_json(req).await
    else {
        return device_error(StatusCode::BAD_REQUEST, "invalid_request");
    };
    match gateway.device.poll(device_code.trim()) {
        Poll::Pending => device_error(StatusCode::BAD_REQUEST, "authorization_pending"),
        Poll::SlowDown => device_error(StatusCode::BAD_REQUEST, "slow_down"),
        Poll::Denied => device_error(StatusCode::BAD_REQUEST, "access_denied"),
        Poll::Expired => device_error(StatusCode::BAD_REQUEST, "expired_token"),
        Poll::Ready(grant) => axum::Json(grant).into_response(),
    }
}

/// The signed-in account of a page call, and the short code it typed,
/// checked; or the answer that refuses it.
async fn typed_code<L: Locator>(
    gateway: &Gateway<L>,
    req: Request,
) -> Result<(AccountId, String), Response> {
    if !gateway.same_origin(req.headers()) {
        return Err((StatusCode::FORBIDDEN, "origin not allowed").into_response());
    }
    let Some(account) = gateway.session_account(req.headers()) else {
        return Err((StatusCode::UNAUTHORIZED, "sign in first").into_response());
    };
    if !gateway.device.may_try(account.as_str()) {
        return Err(device_error(
            StatusCode::TOO_MANY_REQUESTS,
            "too_many_tries",
        ));
    }
    let typed = device_flow::read_json::<wenlan_types::team::TeamDeviceDecision>(req)
        .await
        .and_then(|body| wenlan_types::team::normalize_user_code(&body.user_code));
    match typed {
        Some(code) if gateway.device.tool_for(&code).is_some() => Ok((account, code)),
        _ => {
            gateway.device.missed(account.as_str());
            Err(device_error(StatusCode::NOT_FOUND, "unknown_code"))
        }
    }
}

/// `POST /auth/device/lookup` with `{"user_code": "ABCD-EFGH"}`: which
/// computer asks, for the page that allows it.
async fn device_lookup<L: Locator>(
    State(gateway): State<Arc<Gateway<L>>>,
    req: Request,
) -> Response {
    let code = match typed_code(&gateway, req).await {
        Ok((_, code)) => code,
        Err(refused) => return refused,
    };
    let Some(tool) = gateway.device.tool_for(&code) else {
        return device_error(StatusCode::NOT_FOUND, "unknown_code");
    };
    axum::Json(wenlan_types::team::TeamDeviceLookup {
        tool,
        spaces: Vec::new(),
    })
    .into_response()
}

/// `POST /auth/device/approve` with the short code: makes the computer's
/// key for it to collect. Only for a session that signed in again just now.
async fn device_approve<L: Locator>(
    State(gateway): State<Arc<Gateway<L>>>,
    req: Request,
) -> Response {
    let fresh = gateway.fresh(req.headers());
    let (account, code) = match typed_code(&gateway, req).await {
        Ok(found) => found,
        Err(refused) => return refused,
    };
    if !fresh {
        return device_error(StatusCode::FORBIDDEN, "reauth_required");
    }
    let Some((id, name)) = gateway.device.begin_issue(&code) else {
        return device_error(StatusCode::NOT_FOUND, "unknown_code");
    };
    let root = gateway.root.clone();
    let who = account.clone();
    let added = tokio::task::spawn_blocking(move || add_device(&root, &who, &name)).await;
    match added {
        Ok(Ok((key_id, key))) => {
            gateway.device.ready(
                &id,
                Some(wenlan_types::team::TeamDeviceGrant {
                    key,
                    key_id,
                    spaces: Vec::new(),
                    team: None,
                }),
            );
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(Err(error)) if error.kind() == std::io::ErrorKind::QuotaExceeded => {
            gateway.device.ready(&id, None);
            device_error(StatusCode::CONFLICT, "too_many_devices")
        }
        _ => {
            gateway.device.ready(&id, None);
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
    }
}

/// `POST /auth/device/deny` with the short code: the computer is told no.
async fn device_deny<L: Locator>(State(gateway): State<Arc<Gateway<L>>>, req: Request) -> Response {
    let code = match typed_code(&gateway, req).await {
        Ok((_, code)) => code,
        Err(refused) => return refused,
    };
    if gateway.device.deny(&code) {
        StatusCode::NO_CONTENT.into_response()
    } else {
        device_error(StatusCode::NOT_FOUND, "unknown_code")
    }
}

/// `GET /auth/devices`: the signed-in person's computers.
async fn device_list<L: Locator>(
    State(gateway): State<Arc<Gateway<L>>>,
    headers: HeaderMap,
) -> Response {
    let Some(account) = gateway.session_account(&headers) else {
        return (StatusCode::UNAUTHORIZED, "sign in first").into_response();
    };
    let root = gateway.root.clone();
    match tokio::task::spawn_blocking(move || devices(&root, &account)).await {
        Ok(Ok(list)) => axum::Json(list).into_response(),
        _ => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

/// `DELETE /auth/devices/{id}`: removes one of the person's computers; its
/// key is refused from its next request. Answers the computers left.
async fn device_remove<L: Locator>(
    State(gateway): State<Arc<Gateway<L>>>,
    UrlPath(id): UrlPath<String>,
    headers: HeaderMap,
) -> Response {
    if !gateway.same_origin(&headers) {
        return (StatusCode::FORBIDDEN, "origin not allowed").into_response();
    }
    let Some(account) = gateway.session_account(&headers) else {
        return (StatusCode::UNAUTHORIZED, "sign in first").into_response();
    };
    let root = gateway.root.clone();
    let removed = tokio::task::spawn_blocking(move || {
        remove_device(&root, &account, &id).and_then(|gone| Ok((gone, devices(&root, &account)?)))
    })
    .await;
    match removed {
        Ok(Ok((true, left))) => axum::Json(left).into_response(),
        Ok(Ok((false, _))) => device_error(StatusCode::NOT_FOUND, "unknown_device"),
        _ => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

/// `POST /auth/signout`: ends this browser's session.
async fn sign_out<L: Locator>(
    State(gateway): State<Arc<Gateway<L>>>,
    headers: HeaderMap,
) -> Response {
    if let Some(token) = team_session::cookie_token(&headers) {
        gateway.sessions.delete(&token);
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    if let Ok(cookie) = HeaderValue::from_str(&team_session::clear_cookie()) {
        response.headers_mut().insert(header::SET_COOKIE, cookie);
    }
    response
}

/// The page someone who has not signed in gets.
fn sign_in_page(providers: &[String]) -> Response {
    let mut links = String::new();
    for name in providers {
        let label = match name.as_str() {
            "google" => "Continue with Google",
            "github" => "Continue with GitHub",
            _ => continue,
        };
        links.push_str(&format!(
            "<p><a href=\"/auth/start/{name}\">{label}</a></p>"
        ));
    }
    let page = format!(
        "<!doctype html><meta charset=utf-8><meta name=viewport content=\"width=device-width\"><title>Wenlan</title><h1>Wenlan</h1>{links}"
    );
    let mut response = (StatusCode::UNAUTHORIZED, axum::response::Html(page)).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn wants_page(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|accept| accept.contains("text/html"))
}

/// Headers that belong to one connection, never forwarded either way.
const HOP_BY_HOP: [&str; 8] = [
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// Everything else: the signed-in person's request (a browser's session
/// cookie, or one of their computers' keys), forwarded to their Wenlan with
/// a pass that says which. The entrance's own cookie, any credential the
/// request carries and any pass it claims to have are not forwarded; the
/// Wenlan's own cookies are not sent back.
async fn forward<L: Locator>(State(gateway): State<Arc<Gateway<L>>>, req: Request) -> Response {
    let mut account = gateway.session_account(req.headers());
    let via = if account.is_some() {
        Via::Browser
    } else {
        Via::Computer
    };
    if account.is_none() {
        let key = req
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .filter(|key| key.starts_with(DEVICE_KEY_PREFIX))
            .map(str::to_string);
        if let Some(key) = key {
            let root = gateway.root.clone();
            match tokio::task::spawn_blocking(move || account_for_key(&root, &key)).await {
                Ok(Ok(found)) => account = found,
                _ => return (StatusCode::SERVICE_UNAVAILABLE, "try again soon").into_response(),
            }
        }
    }
    let Some(account) = account else {
        if wants_page(req.headers()) {
            return sign_in_page(&gateway.auth.names());
        }
        return (StatusCode::UNAUTHORIZED, "sign in first").into_response();
    };
    let endpoint = match gateway.locator.locate(&account).await {
        Ok(endpoint) => endpoint,
        Err(LocateError::Busy) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "Wenlan is busy right now. Try again in a minute.",
            )
                .into_response()
        }
        Err(error) => {
            tracing::warn!("[cloud] a person's Wenlan is unavailable: {error}");
            return (
                StatusCode::BAD_GATEWAY,
                "Your Wenlan is not answering right now. Try again soon.",
            )
                .into_response();
        }
    };
    let (parts, body) = req.into_parts();
    let Ok(body) = axum::body::to_bytes(body, MAX_BODY).await else {
        return (StatusCode::PAYLOAD_TOO_LARGE, "the request is too large").into_response();
    };
    let target = format!(
        "http://{}{}",
        endpoint.addr,
        parts.uri.path_and_query().map_or("/", |pq| pq.as_str())
    );
    let mut headers = parts.headers;
    for name in HOP_BY_HOP {
        headers.remove(name);
    }
    for name in [
        header::HOST,
        header::COOKIE,
        header::AUTHORIZATION,
        header::CONTENT_LENGTH,
    ] {
        headers.remove(name);
    }
    headers.remove(PASS_HEADER);
    if via == Via::Computer {
        // A computer is no page: nothing it sends speaks for a page.
        headers.remove(header::ORIGIN);
    }
    let pass = gateway
        .pass_key
        .for_account(&account)
        .sign_via(&account, via);
    if let Ok(pass) = HeaderValue::from_str(&pass) {
        headers.insert(HeaderName::from_static(PASS_HEADER), pass);
    }
    let answer = gateway
        .http
        .request(parts.method, target)
        .headers(headers)
        .body(body)
        .send()
        .await;
    let answer = match answer {
        Ok(answer) => answer,
        Err(_) => {
            return (
                StatusCode::BAD_GATEWAY,
                "Your Wenlan is not answering right now. Try again soon.",
            )
                .into_response()
        }
    };
    let status = answer.status();
    let mut back = answer.headers().clone();
    for name in HOP_BY_HOP {
        back.remove(name);
    }
    back.remove(header::SET_COOKIE);
    let mut response = Response::new(Body::from_stream(answer.bytes_stream()));
    *response.status_mut() = status;
    *response.headers_mut() = back;
    response
}

/// Runs the entrance at `listen` until Ctrl-C, stopping idle people every
/// minute and everyone at the end.
pub async fn run(
    origin: String,
    listen: SocketAddr,
    root: PathBuf,
    max_running: usize,
    idle: Duration,
) -> anyhow::Result<()> {
    use crate::cloud_locator::{PoolSettings, ProcessLauncher, ProcessPool};
    anyhow::ensure!(
        origin.starts_with("https://"),
        "the personal cloud's address must be https://"
    );
    std::fs::create_dir_all(&root)?;
    let pass_key = PassKey::read_or_create(&root.join(KEY_FILE))?;
    let providers = auth::load_providers(&root.join(auth::AUTH_FILE), |name| {
        std::env::var(name)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    });
    anyhow::ensure!(
        !providers.is_empty(),
        "no sign-in provider is configured ({} and {}, or {} and {})",
        auth::GOOGLE_CLIENT_ID_ENV,
        auth::GOOGLE_CLIENT_SECRET_ENV,
        auth::GITHUB_CLIENT_ID_ENV,
        auth::GITHUB_CLIENT_SECRET_ENV,
    );
    let pool = Arc::new(ProcessPool::new(
        ProcessLauncher {
            program: std::env::current_exe()?,
            public_url: origin.clone(),
            pass_key: pass_key.clone(),
        },
        PoolSettings {
            root: root.join("people"),
            idle,
            ready_within: Duration::from_secs(30),
            max_running,
        },
    ));
    let gateway = Arc::new(Gateway::new(
        pool.clone(),
        AuthState::new(providers),
        GatewaySettings {
            origin,
            root: root.clone(),
        },
        pass_key,
    ));
    let reaper = {
        let pool = pool.clone();
        tokio::spawn(async move {
            let mut every = tokio::time::interval(Duration::from_secs(60));
            loop {
                every.tick().await;
                let stopped = pool.stop_idle().await;
                if stopped > 0 {
                    tracing::info!("[cloud] stopped {stopped} idle");
                }
            }
        })
    };
    let listener = tokio::net::TcpListener::bind(listen).await?;
    tracing::info!("[cloud] entrance listening on {listen}");
    axum::serve(
        listener,
        router(gateway).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await?;
    reaper.abort();
    pool.stop_all().await;
    Ok(())
}

#[cfg(test)]
#[path = "cloud_gateway_test.rs"]
mod tests;
