// SPDX-License-Identifier: AGPL-3.0-only
//! Signing in to the team server's web with Google (OpenID Connect; the code
//! is written for any OIDC issuer, so another one is configuration) or with
//! GitHub (plain OAuth 2.0, which has no id token).
//!
//! Sign-in is on only when the team's address (`WENLAN_TEAM_URL`, else the
//! saved one) is `https://` and at least one provider has a client id and
//! secret. Either provider alone is enough. Then
//! the callback address is built from that address alone, never from a
//! request's `Host`. Otherwise every route here answers 404.
//!
//! A sign-in is a *login transaction*: a random state, nonce and PKCE
//! (S256) verifier kept in memory with the provider and what the sign-in is
//! for ([`Purpose`]). The browser that started it carries the transaction's
//! id in a short-lived `__Host-wenlan-login` cookie, so the callback only
//! completes in that browser. A transaction lives ten minutes and is used
//! once: the first callback that presents it consumes it, right or wrong.
//!
//! The Google callback checks the id token's signature, issuer, audience,
//! expiry and nonce (the `openidconnect` crate), and that the provider calls
//! the email verified. Who someone is stays `(issuer, subject)`; the email is
//! shown and never looked up. A callback never reads `code`, `state` or a
//! cookie into a log line, and a failure names no more than a fixed label.
//!
//! GitHub is the authorization code flow with PKCE (S256, never downgraded)
//! and the client secret. It has no id token and no nonce, so the binding is
//! the transaction, its `state` and the PKCE verifier. The callback trades
//! the code for an access token, calls `GET /user` and `GET /user/emails`
//! with it, and drops it; the token is never stored or logged. The account is
//! `("https://github.com", numeric id)`: the id never changes, the `login`
//! can, so the login is never used. The verified emails are every address on
//! `/user/emails` marked verified; GitHub verifies an address by mailing it,
//! so they count as [`team::EmailTrust::Trusted`].
//!
//! What a sign-in may do:
//!
//! - **SignIn**: the member whose linked accounts hold `(issuer, subject)`
//!   gets a new browser session. Anyone else is "not in this team".
//! - **Link**: attaches the account to the member who started it (with a
//!   legacy key, or a browser session that signed in again within
//!   [`REAUTH_WINDOW`] with an account already theirs). An account on
//!   another member is refused. An email that happens to match changes
//!   nothing.
//! - **Invite**: the browser's invite token (sent in a POST body, never in a
//!   URL) is bound to the transaction by its hash. The callback hands the
//!   account to core's `redeem_invite_hash`, which does every check and the
//!   change in one lock: the member gets the account and a new session
//!   (Google accounts with a Gmail address or a company domain, `hd`, and any
//!   GitHub account with the invited address among its verified emails, are
//!   trusted to own the invited email), or waits for the owner's approval
//!   when the email matches but the provider does not vouch for it. A wrong
//!   email names the invited one masked (`a***@example.com`).
//! - **Reauth**: succeeds only when the account is already one of this
//!   member's. With Google the provider must also say the member
//!   authenticated just now (`max_age=0` and the id token's `auth_time`).
//!   GitHub has no such parameter, so a GitHub reauth only guarantees that a
//!   fresh authorization round trip came back with an account already
//!   linked to this member, not that the person typed a password again.
//!   Either way it sets the new session's freshness for the routes that make
//!   long-lived credentials.
//!
//! Every success makes a new session token and ends the one the browser
//! presented. Every answer on these routes is `Cache-Control: no-store`,
//! `Referrer-Policy: no-referrer` and not frameable.
//!
//! Requests are limited per direct peer IP with a token bucket. Behind a
//! reverse proxy every client has the proxy's address, so they share one
//! bucket; a trusted-proxy list is not built. There is no counter of
//! failures across everyone, which anyone could fill to lock the team out.
//!
//! The stores hold a `std::sync::Mutex`, never held across an `.await`.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, Path as UrlPath, Query, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use chrono::{DateTime, Utc};
use openidconnect::core::{CoreAuthenticationFlow, CoreClient, CoreProviderMetadata};
use openidconnect::{
    AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointMaybeSet, EndpointNotSet,
    EndpointSet, IssuerUrl, Nonce, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope,
    TokenResponse,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use wenlan_core::team::{self, Credential, Identity, Member};

use super::{own_origin, TeamState};
use crate::team_session::{self, REAUTH_WINDOW};

/// The Google client id, from the environment.
pub const GOOGLE_CLIENT_ID_ENV: &str = "WENLAN_TEAM_GOOGLE_CLIENT_ID";
/// The Google client secret, from the environment.
pub const GOOGLE_CLIENT_SECRET_ENV: &str = "WENLAN_TEAM_GOOGLE_CLIENT_SECRET";
/// The file (in the data root, next to `team.json`) that can hold the same.
pub const AUTH_FILE: &str = "team-auth.json";
const GOOGLE_ISSUER: &str = "https://accounts.google.com";
/// The GitHub client id, from the environment.
pub const GITHUB_CLIENT_ID_ENV: &str = "WENLAN_TEAM_GITHUB_CLIENT_ID";
/// The GitHub client secret, from the environment.
pub const GITHUB_CLIENT_SECRET_ENV: &str = "WENLAN_TEAM_GITHUB_CLIENT_SECRET";
/// The issuer string a GitHub account is kept under.
pub const GITHUB_ISSUER: &str = "https://github.com";
/// GitHub refuses API calls without a `User-Agent`.
const GITHUB_USER_AGENT: &str = "wenlan-team-server";
/// What the GitHub sign-in asks to read: the profile and the email list.
const GITHUB_SCOPE: &str = "read:user user:email";
/// The most a GitHub answer may weigh before it is refused.
const GITHUB_BODY_CAP: usize = 1 << 20;

/// The login cookie's name (`__Host-`: Secure, `Path=/`, no `Domain`).
const LOGIN_COOKIE: &str = "__Host-wenlan-login";
/// A login transaction lives this long.
const TRANSACTION_TTL: Duration = Duration::from_secs(10 * 60);
/// Login transactions in all; the oldest is dropped at the cap.
const TRANSACTION_CAP: usize = 5_000;
/// A provider's metadata (its signing keys included) is read again after
/// this long.
const METADATA_TTL: Duration = Duration::from_secs(60 * 60);
/// A refused sign-in may read the metadata again (a provider rotates its
/// keys), but not more often than this.
const METADATA_REFRESH_MIN: Duration = Duration::from_secs(5 * 60);
/// How far the provider's clock and this one may differ.
const CLOCK_SKEW: chrono::Duration = chrono::Duration::seconds(60);
/// Peers remembered by the rate limiter.
const LIMITER_CAP: usize = 10_000;
/// Requests a peer may make at once, and per second after that.
const BURST: f64 = 30.0;
const REFILL_PER_SEC: f64 = 0.5;

// ===== Provider configuration =====

/// The three GitHub addresses a sign-in talks to. Production uses
/// [`GithubUrls::default`]; a test points them at a local fake.
#[derive(Clone, Debug)]
pub(crate) struct GithubUrls {
    pub(crate) authorize: String,
    pub(crate) token: String,
    /// The API base, without a trailing slash (`/user` is added).
    pub(crate) api: String,
}

impl Default for GithubUrls {
    fn default() -> Self {
        Self {
            authorize: "https://github.com/login/oauth/authorize".into(),
            token: "https://github.com/login/oauth/access_token".into(),
            api: "https://api.github.com".into(),
        }
    }
}

/// How a provider proves who someone is.
#[derive(Clone)]
pub(crate) enum Kind {
    /// OpenID Connect: discovery, an id token, a nonce.
    Oidc,
    /// GitHub's OAuth 2.0: no id token; `/user` and `/user/emails`.
    GitHub(GithubUrls),
}

/// One provider this server signs people in with.
#[derive(Clone)]
pub(crate) struct Provider {
    /// The name in the routes (`google`, `github`).
    pub(crate) name: String,
    pub(crate) issuer: String,
    pub(crate) client_id: String,
    client_secret: String,
    pub(crate) kind: Kind,
}

impl std::fmt::Debug for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Provider")
            .field("name", &self.name)
            .field("issuer", &self.issuer)
            .field("client_id", &self.client_id)
            .field("client_secret", &"<hidden>")
            .finish()
    }
}

impl Provider {
    pub(crate) fn new(name: &str, issuer: &str, client_id: &str, client_secret: &str) -> Self {
        Self {
            name: name.into(),
            issuer: issuer.into(),
            client_id: client_id.into(),
            client_secret: client_secret.into(),
            kind: Kind::Oidc,
        }
    }

    /// A GitHub provider at GitHub's own addresses.
    pub(crate) fn github(client_id: &str, client_secret: &str) -> Self {
        Self::github_with_urls(client_id, client_secret, GithubUrls::default())
    }

    /// A GitHub provider at the given addresses (a test's fake server).
    pub(crate) fn github_with_urls(client_id: &str, client_secret: &str, urls: GithubUrls) -> Self {
        Self {
            kind: Kind::GitHub(urls),
            ..Self::new("github", GITHUB_ISSUER, client_id, client_secret)
        }
    }
}

#[derive(Deserialize)]
struct FileProvider {
    client_id: Option<String>,
    client_secret: Option<String>,
    /// Only `https://` is taken.
    issuer: Option<String>,
}

#[derive(Deserialize)]
struct AuthFile {
    google: Option<FileProvider>,
    github: Option<FileProvider>,
}

fn read_auth_file(path: &Path) -> Option<AuthFile> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            tracing::warn!("[team] {AUTH_FILE} cannot be read: {error}");
            return None;
        }
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            if meta.permissions().mode() & 0o077 != 0 {
                tracing::warn!(
                    "[team] {AUTH_FILE} holds a secret and other users can read it; make it yours alone (chmod 600)."
                );
            }
        }
    }
    match serde_json::from_str(&text) {
        Ok(file) => Some(file),
        // The parser's message can quote the text, which holds the secret.
        Err(_) => {
            tracing::warn!("[team] {AUTH_FILE} is not valid JSON of the expected shape");
            None
        }
    }
}

/// A client id and secret, the environment first, then the file, each on
/// its own. `None` when either is missing (a half configuration is warned
/// about, never used).
fn credentials(
    label: &str,
    id_env: &str,
    secret_env: &str,
    file: Option<&FileProvider>,
    env: &impl Fn(&str) -> Option<String>,
) -> Option<(String, String)> {
    let pick = |env_name: &str, from_file: Option<&String>| {
        env(env_name)
            .or_else(|| from_file.map(|v| v.trim().to_string()))
            .filter(|v| !v.is_empty())
    };
    let id = pick(id_env, file.and_then(|f| f.client_id.as_ref()));
    let secret = pick(secret_env, file.and_then(|f| f.client_secret.as_ref()));
    match (id, secret) {
        (Some(id), Some(secret)) => Some((id, secret)),
        (None, None) => None,
        _ => {
            tracing::warn!(
                "[team] {label} sign-in needs both {id_env} and {secret_env} (or both in {AUTH_FILE}); one is missing."
            );
            None
        }
    }
}

/// The providers with a complete configuration: the environment first, then
/// `team-auth.json` at `file`, for each of the client id and secret.
pub(crate) fn load_providers(file: &Path, env: impl Fn(&str) -> Option<String>) -> Vec<Provider> {
    let parsed = read_auth_file(file);
    let (google, github) = match parsed {
        Some(f) => (f.google, f.github),
        None => (None, None),
    };
    let mut providers = Vec::new();
    if let Some((id, secret)) = credentials(
        "Google",
        GOOGLE_CLIENT_ID_ENV,
        GOOGLE_CLIENT_SECRET_ENV,
        google.as_ref(),
        &env,
    ) {
        let issuer = google
            .as_ref()
            .and_then(|g| g.issuer.as_deref())
            .map(str::trim)
            .filter(|i| i.starts_with("https://"))
            .unwrap_or(GOOGLE_ISSUER);
        providers.push(Provider::new("google", issuer, &id, &secret));
    }
    // GitHub's addresses are fixed; a file's `issuer` is not read for it.
    if let Some((id, secret)) = credentials(
        "GitHub",
        GITHUB_CLIENT_ID_ENV,
        GITHUB_CLIENT_SECRET_ENV,
        github.as_ref(),
        &env,
    ) {
        providers.push(Provider::github(&id, &secret));
    }
    providers
}

// ===== State =====

/// What a login is for.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Purpose {
    /// Anyone signing in: the member is found by the account.
    SignIn,
    /// Attach the account to this member.
    Link { member_id: String },
    /// This member proves themselves again with an account already theirs.
    Reauth { member_id: String },
    /// Someone with an invite link signs in to become the member it is for.
    /// Only the token's hash is kept here.
    Invite { token_hash: String },
}

struct Transaction {
    state: String,
    nonce: String,
    pkce_verifier: String,
    provider: String,
    purpose: Purpose,
    created_at: Instant,
    created_wall: DateTime<Utc>,
}

type Hash = [u8; 32];

fn hash(text: &str) -> Hash {
    Sha256::digest(text.as_bytes()).into()
}

fn random_hex() -> Result<String, getrandom::Error> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)?;
    Ok(hex::encode(bytes))
}

/// Constant-time equality of two strings.
fn same(a: &str, b: &str) -> bool {
    let (a, b) = (hash(a), hash(b));
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

struct Bucket {
    tokens: f64,
    last: Instant,
}

type Metadata = Arc<CoreProviderMetadata>;
/// An OpenID Connect client made from discovered metadata.
type OidcClient = CoreClient<
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointMaybeSet,
    EndpointMaybeSet,
>;

/// Sign-in's state on the team listener.
pub(crate) struct AuthState {
    providers: Vec<Provider>,
    transactions: Mutex<HashMap<Hash, Transaction>>,
    buckets: Mutex<HashMap<IpAddr, Bucket>>,
    metadata: tokio::sync::Mutex<HashMap<String, (Instant, Metadata)>>,
    http: OnceLock<openidconnect::reqwest::Client>,
    ttl: Duration,
    burst: f64,
    refill: f64,
}

impl Default for AuthState {
    /// Sign-in off: no provider.
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

impl AuthState {
    pub(crate) fn new(providers: Vec<Provider>) -> Self {
        Self {
            providers,
            transactions: Mutex::default(),
            buckets: Mutex::default(),
            metadata: tokio::sync::Mutex::default(),
            http: OnceLock::new(),
            ttl: TRANSACTION_TTL,
            burst: BURST,
            refill: REFILL_PER_SEC,
        }
    }

    /// The providers `team-auth.json` (next to `team_path`) and the
    /// environment configure.
    pub(crate) fn from_config(team_path: &Path) -> Self {
        let file = team_path.with_file_name(AUTH_FILE);
        Self::new(load_providers(&file, super::env_value))
    }

    fn provider(&self, name: &str) -> Option<&Provider> {
        self.providers.iter().find(|p| p.name == name)
    }

    /// The HTTP client for the provider: it never follows a redirect (the
    /// crate's advice against SSRF) and gives up after 10 seconds.
    fn http(&self) -> Result<&openidconnect::reqwest::Client, Failure> {
        if let Some(client) = self.http.get() {
            return Ok(client);
        }
        let client = openidconnect::reqwest::ClientBuilder::new()
            .redirect(openidconnect::reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|_| Failure::Unavailable("no HTTP client"))?;
        Ok(self.http.get_or_init(|| client))
    }

    fn transactions(&self) -> std::sync::MutexGuard<'_, HashMap<Hash, Transaction>> {
        // A panic while holding the lock leaves a map that is still valid.
        self.transactions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Keeps a transaction and returns the id for the browser's cookie.
    /// `replacing` is the id of a transaction this browser still had: it is
    /// dropped, so one browser holds at most one.
    fn put(&self, tx: Transaction, replacing: Option<&str>) -> Result<String, Failure> {
        let id = random_hex().map_err(|_| Failure::Unavailable("no randomness"))?;
        let mut map = self.transactions();
        let ttl = self.ttl;
        map.retain(|_, t| t.created_at.elapsed() < ttl);
        if let Some(old) = replacing {
            map.remove(&hash(old));
        }
        while map.len() >= TRANSACTION_CAP {
            let Some(oldest) = map
                .iter()
                .min_by_key(|(_, t)| t.created_at)
                .map(|(key, _)| *key)
            else {
                break;
            };
            map.remove(&oldest);
        }
        map.insert(hash(&id), tx);
        Ok(id)
    }

    /// Takes the transaction this id names, if it is still live. It cannot
    /// be taken twice.
    fn take(&self, id: &str) -> Option<Transaction> {
        let tx = self.transactions().remove(&hash(id))?;
        (tx.created_at.elapsed() < self.ttl).then_some(tx)
    }

    #[cfg(test)]
    pub(crate) fn transactions_len(&self) -> usize {
        self.transactions().len()
    }

    /// Whether this peer may make a request now. A peer without an address
    /// (never on a real connection) shares one bucket.
    fn allow(&self, peer: Option<IpAddr>) -> bool {
        let key = limiter_key(peer.unwrap_or(IpAddr::from([0, 0, 0, 0])));
        let now = Instant::now();
        let mut buckets = self
            .buckets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if buckets.len() >= LIMITER_CAP && !buckets.contains_key(&key) {
            // Forget the peers that have their full allowance back.
            let (burst, refill) = (self.burst, self.refill);
            buckets.retain(|_, b| {
                b.tokens + now.saturating_duration_since(b.last).as_secs_f64() * refill < burst
            });
            if buckets.len() >= LIMITER_CAP {
                buckets.clear();
            }
        }
        let bucket = buckets.entry(key).or_insert(Bucket {
            tokens: self.burst,
            last: now,
        });
        let gained = now.saturating_duration_since(bucket.last).as_secs_f64() * self.refill;
        bucket.tokens = (bucket.tokens + gained).min(self.burst);
        bucket.last = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Test knobs.
    #[cfg(test)]
    pub(crate) fn tuned(mut self, ttl: Duration, burst: f64, refill: f64) -> Self {
        self.ttl = ttl;
        self.burst = burst;
        self.refill = refill;
        self
    }

    /// Test client (proxy settings of the machine do not reach a local
    /// fake provider).
    #[cfg(test)]
    pub(crate) fn with_http(self, client: openidconnect::reqwest::Client) -> Self {
        let _ = self.http.set(client);
        self
    }

    /// The provider's metadata, read through discovery and kept for an
    /// hour. `refresh` reads it again when what is kept is older than a few
    /// minutes (a provider rotates its signing keys).
    async fn metadata(&self, provider: &Provider, refresh: bool) -> Result<Metadata, Failure> {
        let http = self.http()?;
        let mut kept = self.metadata.lock().await;
        if let Some((at, found)) = kept.get(&provider.name) {
            let age = at.elapsed();
            if age < METADATA_TTL && !(refresh && age > METADATA_REFRESH_MIN) {
                return Ok(found.clone());
            }
        }
        let issuer = IssuerUrl::new(provider.issuer.clone())
            .map_err(|_| Failure::Unavailable("bad issuer"))?;
        let found = CoreProviderMetadata::discover_async(issuer, http)
            .await
            .map_err(|_| Failure::Unavailable("provider discovery failed"))?;
        let found = Arc::new(found);
        kept.insert(provider.name.clone(), (Instant::now(), found.clone()));
        Ok(found)
    }
}

/// The key a peer is limited by: an IPv6 peer by its /64 (a host has a
/// whole network of addresses), an IPv4-mapped one as its IPv4.
fn limiter_key(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(_) => ip,
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => {
                let mut octets = v6.octets();
                octets[8..].fill(0);
                IpAddr::V6(octets.into())
            }
        },
    }
}

/// Where the provider sends the browser back: built from the configured
/// address alone, never from a request's `Host`.
fn callback_url(origin: &str, provider: &Provider) -> String {
    format!("{origin}/auth/callback/{}", provider.name)
}

fn client_for(
    meta: &CoreProviderMetadata,
    provider: &Provider,
    origin: &str,
) -> Result<OidcClient, Failure> {
    let redirect = RedirectUrl::new(callback_url(origin, provider))
        .map_err(|_| Failure::Unavailable("bad callback address"))?;
    Ok(CoreClient::from_provider_metadata(
        meta.clone(),
        ClientId::new(provider.client_id.clone()),
        Some(ClientSecret::new(provider.client_secret.clone())),
    )
    .set_redirect_uri(redirect))
}

/// The account an id token proved.
struct Account {
    issuer: String,
    subject: String,
    /// The address shown with the account (Google's, or GitHub's primary
    /// verified one).
    email: Option<String>,
    /// Every address the provider vouches for as this account's: Google's
    /// one, GitHub's every verified one.
    verified_emails: Vec<String>,
    /// Only Google says when the person authenticated.
    auth_time: Option<DateTime<Utc>>,
    /// The id token carries a `hd` (hosted domain) claim: a company account.
    hosted_domain: bool,
}

/// How far to believe a Google account owns its email now: a Gmail address
/// or an account of a company domain (`hd`) does; Google's "verified" on
/// any other address does not show the person still owns the mailbox.
fn google_email_trust(email: &str, hosted_domain: bool) -> team::EmailTrust {
    let lower = email.trim().to_ascii_lowercase();
    let gmail = lower.ends_with("@gmail.com") || lower.ends_with("@googlemail.com");
    if hosted_domain || gmail {
        team::EmailTrust::Trusted
    } else {
        team::EmailTrust::Untrusted
    }
}

/// Whether the (already verified) id token has a non-empty `hd` claim.
fn has_hosted_domain(id_token: &str) -> bool {
    use base64::Engine;
    id_token
        .split('.')
        .nth(1)
        .and_then(|payload| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(payload)
                .ok()
        })
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|claims| {
            claims
                .get("hd")
                .and_then(|hd| hd.as_str().map(str::to_owned))
        })
        .is_some_and(|hd| !hd.trim().is_empty())
}

// ===== Answers =====

/// Why a sign-in did not complete. A label is logged, never a value.
#[derive(Debug)]
enum Failure {
    /// Sign-in is off or the provider is unknown.
    Off,
    /// The request, or the provider's answer, was not acceptable.
    Refused(&'static str),
    /// The account is not a member of this team.
    NotInTeam,
    /// The provider could not be reached, or this server could not work.
    Unavailable(&'static str),
}

fn text(status: StatusCode, body: &'static str) -> Response {
    let mut response = (status, body).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

fn cookie_attributes() -> &'static str {
    "HttpOnly; Secure; SameSite=Lax; Path=/"
}

fn login_cookie(id: &str) -> String {
    format!(
        "{LOGIN_COOKIE}={id}; {}; Max-Age={}",
        cookie_attributes(),
        TRANSACTION_TTL.as_secs()
    )
}

fn clear_login_cookie() -> String {
    format!("{LOGIN_COOKIE}=; {}; Max-Age=0", cookie_attributes())
}

fn set_cookie(response: &mut Response, value: &str) {
    response.headers_mut().append(
        header::SET_COOKIE,
        HeaderValue::from_str(value).expect("cookie values are visible ASCII"),
    );
}

/// A plain-text page that ends this browser's login too.
fn plain_page(status: StatusCode, message: String) -> Response {
    let mut response = (status, message).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    set_cookie(&mut response, &clear_login_cookie());
    response
}

fn refusal_message(reason: &team::InviteRefusal) -> String {
    use team::InviteRefusal::*;
    match reason {
        Unknown => "This invite does not work. Ask the owner of the team for a new link.".into(),
        Expired => "This invite has expired. Ask the owner of the team for a new link.".into(),
        Used => {
            "This invite has already been used. Ask the owner of the team for a new link.".into()
        }
        IdentityTaken => "This account already belongs to another member of this team.".into(),
        EmailMismatch { invited } => format!(
            "This invite is for {invited}. Go back to the link and sign in with that account."
        ),
    }
}

impl Failure {
    fn into_response(self) -> Response {
        let mut response = match self {
            Failure::Off => return StatusCode::NOT_FOUND.into_response(),
            Failure::Refused(reason) => {
                tracing::warn!("[team] sign-in refused: {reason}");
                text(
                    StatusCode::BAD_REQUEST,
                    "Sign-in did not work. Go back and try again.",
                )
            }
            Failure::NotInTeam => {
                tracing::info!("[team] sign-in refused: the account is not in this team");
                text(StatusCode::FORBIDDEN, "This account is not in this team.")
            }
            Failure::Unavailable(reason) => {
                tracing::warn!("[team] sign-in unavailable: {reason}");
                text(
                    StatusCode::BAD_GATEWAY,
                    "Sign-in is not available right now. Try again later.",
                )
            }
        };
        // Whatever went wrong, this browser's transaction is over.
        set_cookie(&mut response, &clear_login_cookie());
        response
    }
}

// ===== Middleware =====

/// Headers on every answer of an `/auth` sign-in route: nothing is cached,
/// no referrer leaks the callback's query, and no page may frame it.
pub(super) async fn headers(req: Request, next: Next) -> Response {
    let mut response = next.run(req).await;
    let h = response.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; frame-ancestors 'none'"),
    );
    response
}

/// First the rate limit (before anything costly), then whether sign-in is
/// on at all: the address people reach this server at is `https://` and a
/// provider is configured. Off is 404, whoever asks.
pub(super) async fn guard(State(state): State<TeamState>, req: Request, next: Next) -> Response {
    let peer = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|info| info.0.ip());
    if !state.auth.allow(peer) {
        let mut response = text(StatusCode::TOO_MANY_REQUESTS, "Too many requests.");
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from_static("5"));
        return response;
    }
    if site(&state).await.is_none() {
        return StatusCode::NOT_FOUND.into_response();
    }
    next.run(req).await
}

/// This server's own `https://` origin, when sign-in is on.
async fn site(state: &TeamState) -> Option<String> {
    if state.auth.providers.is_empty() {
        return None;
    }
    own_origin(state)
        .await
        .filter(|origin| origin.starts_with("https://"))
}

/// The names of the providers a page may offer: none when sign-in is off.
pub(super) async fn offered(state: &TeamState) -> Vec<String> {
    if site(state).await.is_none() {
        return Vec::new();
    }
    state
        .auth
        .providers
        .iter()
        .map(|p| p.name.clone())
        .collect()
}

/// Logs once, at start, whether sign-in is on and, if not, why.
pub(super) async fn announce(state: &TeamState) {
    if state.auth.providers.is_empty() {
        tracing::info!(
            "[team] sign-in is off: no provider is configured ({GOOGLE_CLIENT_ID_ENV} and {GOOGLE_CLIENT_SECRET_ENV}, {GITHUB_CLIENT_ID_ENV} and {GITHUB_CLIENT_SECRET_ENV}, or {AUTH_FILE})."
        );
    } else if site(state).await.is_none() {
        tracing::info!(
            "[team] sign-in is off: the team's address is not https:// (set WENLAN_TEAM_URL to the https address people reach this server at)."
        );
    } else {
        tracing::info!("[team] sign-in is on");
    }
}

// ===== Starting a login =====

/// The same-origin check for the calls a page makes with `fetch`: an
/// `Origin` exactly the configured origin, scheme and port included.
async fn same_origin(state: &TeamState, headers: &HeaderMap) -> bool {
    let Some(origin) = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    own_origin(state).await.as_deref() == Some(origin)
}

/// Makes the transaction and returns the provider's address to send the
/// browser to, and the login cookie that binds it to this browser.
async fn begin(
    state: &TeamState,
    provider: &str,
    purpose: Purpose,
    headers: &HeaderMap,
) -> Result<(String, String), Failure> {
    let origin = site(state).await.ok_or(Failure::Off)?;
    let provider = state.auth.provider(provider).ok_or(Failure::Off)?;
    let (url, csrf_state, nonce, verifier) = match &provider.kind {
        Kind::Oidc => {
            let meta = state.auth.metadata(provider, false).await?;
            let client = client_for(&meta, provider, &origin)?;
            let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
            let mut request = client
                .authorize_url(
                    CoreAuthenticationFlow::AuthorizationCode,
                    CsrfToken::new_random,
                    Nonce::new_random,
                )
                .add_scope(Scope::new("email".into()))
                .set_pkce_challenge(challenge);
            if matches!(purpose, Purpose::Reauth { .. }) {
                // The provider must authenticate the person again now; the
                // callback checks `auth_time` against it.
                request = request.set_max_age(Duration::ZERO);
            }
            let (url, csrf, nonce) = request.url();
            (
                url.to_string(),
                csrf.secret().clone(),
                nonce.secret().clone(),
                verifier.secret().clone(),
            )
        }
        Kind::GitHub(urls) => {
            // GitHub has no `max_age` and no nonce: a reauth is the same
            // round trip as any other, and the binding is the transaction.
            let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
            let csrf = CsrfToken::new_random();
            let mut url = url::Url::parse(&urls.authorize)
                .map_err(|_| Failure::Unavailable("bad authorize address"))?;
            url.query_pairs_mut()
                .append_pair("client_id", &provider.client_id)
                .append_pair("redirect_uri", &callback_url(&origin, provider))
                .append_pair("scope", GITHUB_SCOPE)
                .append_pair("state", csrf.secret())
                .append_pair("code_challenge", challenge.as_str())
                .append_pair("code_challenge_method", "S256");
            (
                url.to_string(),
                csrf.secret().clone(),
                String::new(),
                verifier.secret().clone(),
            )
        }
    };
    let replacing = team_session::cookie_value(headers, LOGIN_COOKIE);
    let id = state.auth.put(
        Transaction {
            state: csrf_state,
            nonce,
            pkce_verifier: verifier,
            provider: provider.name.clone(),
            purpose,
            created_at: Instant::now(),
            created_wall: Utc::now(),
        },
        replacing.as_deref(),
    )?;
    Ok((url, login_cookie(&id)))
}

/// `GET /auth/start/{provider}?purpose=signin`: sends the browser to the
/// provider.
pub(super) async fn start(
    State(state): State<TeamState>,
    UrlPath(provider): UrlPath<String>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    if query.get("purpose").map(String::as_str) != Some("signin") {
        return text(StatusCode::BAD_REQUEST, "purpose must be signin");
    }
    match begin(&state, &provider, Purpose::SignIn, &headers).await {
        Ok((url, cookie)) => {
            let mut response = StatusCode::FOUND.into_response();
            response.headers_mut().insert(
                header::LOCATION,
                HeaderValue::from_str(&url).expect("an authorization URL is ASCII"),
            );
            set_cookie(&mut response, &cookie);
            response
        }
        Err(failure) => failure.into_response(),
    }
}

/// What a page gets to navigate to.
async fn started(
    state: &TeamState,
    provider: &str,
    purpose: Purpose,
    headers: &HeaderMap,
) -> Response {
    match begin(state, provider, purpose, headers).await {
        Ok((url, cookie)) => {
            let mut response = Json(serde_json::json!({ "url": url })).into_response();
            set_cookie(&mut response, &cookie);
            response
        }
        Err(failure) => failure.into_response(),
    }
}

/// `POST /auth/link/{provider}`: start linking the provider's account to
/// the member who is calling. A legacy key may; a browser session must have
/// signed in again within [`REAUTH_WINDOW`] with an account already linked.
pub(super) async fn link(
    State(state): State<TeamState>,
    UrlPath(provider): UrlPath<String>,
    Extension(member): Extension<Member>,
    Extension(credential): Extension<Credential>,
    headers: HeaderMap,
) -> Response {
    if !same_origin(&state, &headers).await {
        return text(StatusCode::FORBIDDEN, "origin not allowed");
    }
    if matches!(credential, Credential::BrowserSession) {
        let fresh = team_session::cookie_token(&headers)
            .is_some_and(|token| state.sessions.fresh(&token, REAUTH_WINDOW));
        if !fresh {
            return (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({ "error": "reauth_required" })),
            )
                .into_response();
        }
    }
    started(
        &state,
        &provider,
        Purpose::Link {
            member_id: member.id,
        },
        &headers,
    )
    .await
}

/// `POST /auth/reauth/{provider}`: a browser session signs in again with an
/// account already linked to its member.
pub(super) async fn reauth(
    State(state): State<TeamState>,
    UrlPath(provider): UrlPath<String>,
    Extension(member): Extension<Member>,
    headers: HeaderMap,
) -> Response {
    if !same_origin(&state, &headers).await {
        return text(StatusCode::FORBIDDEN, "origin not allowed");
    }
    if member.identities.is_empty() {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({ "error": "no_linked_account" })),
        )
            .into_response();
    }
    started(
        &state,
        &provider,
        Purpose::Reauth {
            member_id: member.id,
        },
        &headers,
    )
    .await
}

/// `POST /auth/invite/{provider}` with `{"token": "inv_…"}`: someone with an
/// invite link starts signing in as the member it is for. The token travels
/// in the body only. A token that cannot work (unknown, expired, used) is
/// told so here, before the provider is bothered; the callback checks it
/// all again in one lock.
pub(super) async fn invite(
    State(state): State<TeamState>,
    UrlPath(provider): UrlPath<String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    if !same_origin(&state, &headers).await {
        return text(StatusCode::FORBIDDEN, "origin not allowed");
    }
    #[derive(Deserialize)]
    struct Body {
        token: String,
    }
    let token = match serde_json::from_slice::<Body>(&body) {
        Ok(Body { token }) if wenlan_types::team::is_invite_token(token.trim()) => {
            token.trim().to_string()
        }
        _ => return text(StatusCode::BAD_REQUEST, "this is not an invite"),
    };
    let token_hash = team::invite_token_sha256(&token);
    let path = state.team_path.clone();
    let hash = token_hash.clone();
    match blocking(move || team::invite_usable(&path, &hash)).await {
        Ok(true) => {}
        Ok(false) => {
            return (
                StatusCode::GONE,
                Json(serde_json::json!({ "error": "invite_unusable" })),
            )
                .into_response()
        }
        Err(_) => return Failure::Unavailable("the member list cannot be read").into_response(),
    }
    started(&state, &provider, Purpose::Invite { token_hash }, &headers).await
}

// ===== The callback =====

/// `GET /auth/callback/{provider}`. The query holds the one-time `code`; it
/// is read here and nowhere logged.
pub(super) async fn callback(
    State(state): State<TeamState>,
    UrlPath(provider): UrlPath<String>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    match finish(&state, &provider, &query, &headers).await {
        Ok(response) => response,
        Err(failure) => failure.into_response(),
    }
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, wenlan_core::error::WenlanError> + Send + 'static,
) -> Result<T, wenlan_core::error::WenlanError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| wenlan_core::error::WenlanError::Validation("the task ended".into()))?
}

async fn finish(
    state: &TeamState,
    provider_name: &str,
    query: &HashMap<String, String>,
    headers: &HeaderMap,
) -> Result<Response, Failure> {
    let origin = site(state).await.ok_or(Failure::Off)?;
    let provider = state.auth.provider(provider_name).ok_or(Failure::Off)?;

    // The browser's login cookie names the transaction; it is used up here
    // whatever follows.
    let login = team_session::cookie_value(headers, LOGIN_COOKIE)
        .ok_or(Failure::Refused("no login cookie"))?;
    let tx = state
        .auth
        .take(&login)
        .ok_or(Failure::Refused("unknown, used or expired login"))?;
    if tx.provider != provider.name {
        return Err(Failure::Refused("login is for another provider"));
    }
    let returned_state = query
        .get("state")
        .ok_or(Failure::Refused("no state returned"))?;
    if !same(returned_state, &tx.state) {
        return Err(Failure::Refused("state does not match"));
    }
    if query.contains_key("error") {
        return Err(Failure::Refused("the provider refused"));
    }
    let code = query
        .get("code")
        .filter(|code| !code.is_empty())
        .ok_or(Failure::Refused("no code returned"))?;

    let Account {
        issuer,
        subject,
        email,
        verified_emails,
        auth_time,
        hosted_domain,
    } = verified_account(state, provider, &origin, &tx, code).await?;

    let member_id = match &tx.purpose {
        Purpose::SignIn => {
            let (i, s) = (issuer.clone(), subject.clone());
            let path = state.team_path.clone();
            let found = blocking(move || team::member_by_identity(&path, &i, &s))
                .await
                .map_err(|_| Failure::Unavailable("the member list cannot be read"))?;
            found.ok_or(Failure::NotInTeam)?.id
        }
        Purpose::Link { member_id } => {
            let path = state.team_path.clone();
            let id = member_id.clone();
            let identity = Identity {
                issuer: issuer.clone(),
                subject: subject.clone(),
                email,
                linked_at: String::new(),
            };
            blocking(move || team::link_identity(&path, &id, identity))
                .await
                .map_err(|error| match error {
                    wenlan_core::error::WenlanError::Conflict(_) => {
                        Failure::Refused("the account is linked to another member")
                    }
                    _ => Failure::Refused("the account cannot be linked"),
                })?
                .id
        }
        Purpose::Invite { token_hash } => {
            let trust = match (&provider.kind, email.as_deref()) {
                (Kind::GitHub(_), _) => team::EmailTrust::Trusted,
                (Kind::Oidc, Some(email)) => google_email_trust(email, hosted_domain),
                (Kind::Oidc, None) => return Err(Failure::Refused("no email")),
            };
            let identity = Identity {
                issuer: issuer.clone(),
                subject: subject.clone(),
                email,
                linked_at: String::new(),
            };
            let path = state.team_path.clone();
            let hash = token_hash.clone();
            let outcome = blocking(move || {
                team::redeem_invite_hash(&path, &hash, identity, &verified_emails, trust)
            })
            .await
            .map_err(|_| Failure::Unavailable("the invite cannot be used"))?;
            match outcome {
                team::InviteOutcome::Linked(member) => member.id,
                team::InviteOutcome::PendingApproval(_) => {
                    tracing::info!("[team] an invited account waits for the owner's approval");
                    return Ok(plain_page(
                        StatusCode::OK,
                        "Thanks. The owner of this team needs to approve you before you can sign in. Ask them, then come back and sign in.".into(),
                    ));
                }
                team::InviteOutcome::Refused(reason) => {
                    tracing::info!("[team] an invite was refused");
                    return Ok(plain_page(StatusCode::FORBIDDEN, refusal_message(&reason)));
                }
            }
        }
        Purpose::Reauth { member_id } => {
            // Only the session of the member who asked can sign in again.
            let presented =
                team_session::cookie_token(headers).and_then(|token| state.sessions.lookup(&token));
            if presented.as_deref() != Some(member_id.as_str()) {
                return Err(Failure::Refused("no session of this member"));
            }
            let path = state.team_path.clone();
            let id = member_id.clone();
            let member = blocking(move || team::member_by_id(&path, &id))
                .await
                .map_err(|_| Failure::Unavailable("the member list cannot be read"))?
                .filter(Member::is_active)
                .ok_or(Failure::Refused("the member is gone"))?;
            let theirs = member
                .identities
                .iter()
                .any(|i| i.issuer == issuer && i.subject == subject);
            if !theirs {
                return Err(Failure::Refused("not an account of this member"));
            }
            // Only an OIDC provider can say when the person authenticated;
            // GitHub's reauth is the round trip and the linked account.
            if matches!(provider.kind, Kind::Oidc) {
                let at = auth_time.ok_or(Failure::Refused("no auth_time"))?;
                let now = Utc::now();
                let window = chrono::Duration::from_std(REAUTH_WINDOW).unwrap_or_default();
                if at > now + CLOCK_SKEW || now - at > window || at < tx.created_wall - CLOCK_SKEW {
                    return Err(Failure::Refused("not a fresh authentication"));
                }
            }
            member.id
        }
    };

    // A new session; the one this browser presented ends.
    if let Some(old) = team_session::cookie_token(headers) {
        state.sessions.delete(&old);
    }
    let token = state
        .sessions
        .create_fresh(&member_id)
        .map_err(|_| Failure::Unavailable("no randomness"))?;
    let mut response = StatusCode::FOUND.into_response();
    response
        .headers_mut()
        .insert(header::LOCATION, HeaderValue::from_static("/app"));
    set_cookie(&mut response, &team_session::set_cookie(&token));
    set_cookie(&mut response, &clear_login_cookie());
    Ok(response)
}

/// Exchanges the code and checks the id token: signature, issuer, audience,
/// expiry and nonce, then a verified email. Returns the account
/// `(issuer, subject)`, its email and `auth_time`.
async fn verified_account(
    state: &TeamState,
    provider: &Provider,
    origin: &str,
    tx: &Transaction,
    code: &str,
) -> Result<Account, Failure> {
    let http = state.auth.http()?;
    if let Kind::GitHub(urls) = &provider.kind {
        return github_account(http, urls, provider, origin, tx, code).await;
    }
    let meta = state.auth.metadata(provider, false).await?;
    match exchange(http, &meta, provider, origin, tx, code).await {
        Err(Failure::Refused(reason)) if reason == "id token not accepted" => {
            // The provider may have rotated its keys: read them again
            // (at most every few minutes) and try this once more is not
            // possible, the code is spent; the next sign-in uses them.
            let _ = state.auth.metadata(provider, true).await;
            Err(Failure::Refused(reason))
        }
        other => other,
    }
}

async fn exchange(
    http: &openidconnect::reqwest::Client,
    meta: &CoreProviderMetadata,
    provider: &Provider,
    origin: &str,
    tx: &Transaction,
    code: &str,
) -> Result<Account, Failure> {
    let client = client_for(meta, provider, origin)?;
    let token = client
        .exchange_code(AuthorizationCode::new(code.to_string()))
        .map_err(|_| Failure::Unavailable("no token endpoint"))?
        .set_pkce_verifier(PkceCodeVerifier::new(tx.pkce_verifier.clone()))
        .request_async(http)
        .await
        .map_err(|_| Failure::Refused("code exchange failed"))?;
    let id_token = token.id_token().ok_or(Failure::Refused("no id token"))?;
    let claims = id_token
        .claims(&client.id_token_verifier(), &Nonce::new(tx.nonce.clone()))
        .map_err(|_| Failure::Refused("id token not accepted"))?;
    if claims.email_verified() != Some(true) {
        return Err(Failure::Refused("email not verified"));
    }
    let email = claims.email().map(|e| e.as_str().to_string());
    if email.is_none() {
        return Err(Failure::Refused("no email"));
    }
    Ok(Account {
        issuer: claims.issuer().as_str().to_string(),
        subject: claims.subject().as_str().to_string(),
        verified_emails: email.iter().cloned().collect(),
        email,
        auth_time: claims.auth_time(),
        // The signature was checked above, so the payload is the provider's.
        hosted_domain: has_hosted_domain(&id_token.to_string()),
    })
}

// ===== GitHub =====

/// Reads a GitHub answer as JSON: a success status, a bounded size, and
/// nothing of it logged.
async fn github_json(
    response: openidconnect::reqwest::Response,
    what: &'static str,
) -> Result<serde_json::Value, Failure> {
    if !response.status().is_success() {
        return Err(Failure::Refused(what));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| Failure::Unavailable("GitHub could not be read"))?;
    if bytes.len() > GITHUB_BODY_CAP {
        return Err(Failure::Refused(what));
    }
    serde_json::from_slice(&bytes).map_err(|_| Failure::Refused(what))
}

/// The account GitHub proved: trades the code (with the PKCE verifier and
/// the client secret) for an access token, reads `/user` and `/user/emails`
/// with it, and drops it. Nothing here logs the token or a response body.
async fn github_account(
    http: &openidconnect::reqwest::Client,
    urls: &GithubUrls,
    provider: &Provider,
    origin: &str,
    tx: &Transaction,
    code: &str,
) -> Result<Account, Failure> {
    let form = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("client_id", &provider.client_id)
        .append_pair("client_secret", &provider.client_secret)
        .append_pair("code", code)
        .append_pair("redirect_uri", &callback_url(origin, provider))
        .append_pair("code_verifier", &tx.pkce_verifier)
        .finish();
    let response = http
        .post(&urls.token)
        .header(header::ACCEPT.as_str(), "application/json")
        .header(header::USER_AGENT.as_str(), GITHUB_USER_AGENT)
        .header(
            header::CONTENT_TYPE.as_str(),
            "application/x-www-form-urlencoded",
        )
        .body(form)
        .send()
        .await
        .map_err(|_| Failure::Unavailable("GitHub could not be reached"))?;
    let answer = github_json(response, "code exchange failed").await?;
    // GitHub answers a bad code with 200 and an `error` field.
    let access_token = answer
        .get("access_token")
        .and_then(|v| v.as_str())
        .filter(|t| !t.is_empty())
        .ok_or(Failure::Refused("code exchange failed"))?;
    let bearer = answer
        .get("token_type")
        .and_then(|v| v.as_str())
        .is_some_and(|t| t.eq_ignore_ascii_case("bearer"));
    if !bearer {
        return Err(Failure::Refused("unexpected token type"));
    }

    let get = |path: &str| {
        http.get(format!("{}{path}", urls.api.trim_end_matches('/')))
            .bearer_auth(access_token)
            .header(header::ACCEPT.as_str(), "application/vnd.github+json")
            .header(header::USER_AGENT.as_str(), GITHUB_USER_AGENT)
    };
    let user = get("/user")
        .send()
        .await
        .map_err(|_| Failure::Unavailable("GitHub could not be reached"))?;
    let user = github_json(user, "GitHub profile refused").await?;
    // The numeric id is the subject; the login can be renamed.
    let id = user
        .get("id")
        .and_then(|v| v.as_u64())
        .filter(|id| *id > 0)
        .ok_or(Failure::Refused("no numeric id"))?;

    let emails = get("/user/emails")
        .send()
        .await
        .map_err(|_| Failure::Unavailable("GitHub could not be reached"))?;
    let emails = github_json(emails, "GitHub emails refused").await?;
    let list = emails
        .as_array()
        .ok_or(Failure::Refused("GitHub emails not a list"))?;
    let mut verified = Vec::new();
    let mut primary = None;
    for entry in list {
        let address = entry
            .get("email")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|e| !e.is_empty());
        let is_verified = entry.get("verified").and_then(|v| v.as_bool()) == Some(true);
        if let (Some(address), true) = (address, is_verified) {
            if entry.get("primary").and_then(|v| v.as_bool()) == Some(true) && primary.is_none() {
                primary = Some(address.to_string());
            }
            verified.push(address.to_string());
        }
    }
    let email = primary.or_else(|| verified.first().cloned());
    Ok(Account {
        issuer: GITHUB_ISSUER.to_string(),
        subject: id.to_string(),
        email,
        verified_emails: verified,
        auth_time: None,
        hosted_domain: false,
    })
}

#[cfg(test)]
#[path = "team_auth_test.rs"]
mod tests;
