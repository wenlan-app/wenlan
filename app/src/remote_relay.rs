// SPDX-License-Identifier: AGPL-3.0-only
//! Typed desktop control-plane client for the standalone relay. Credentials
//! stay native: these types must not be returned by a Tauri command or logged.
use reqwest::{Client, Method, RequestBuilder, Response, StatusCode, Url};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub(crate) mod disconnect;
pub(crate) mod orphan;
pub(crate) mod renewal;
pub(crate) mod reverse_client;
pub(crate) mod reverse_peer;
pub(crate) mod reverse_protocol;
pub(crate) mod reverse_runtime;
pub(crate) mod reverse_socket;
pub mod runtime;
pub(crate) mod shutdown;
pub(crate) mod startup;
pub mod store;

pub const RELAY_ORIGIN: &str = "https://relay.wenlan.app";
/// The saved scope for "every Space, plus everything not in a Space". The daemon
/// refuses it as a Space name, so a one-Space choice can never mean it.
pub const WHOLE_LIBRARY_SPACE: &str = "*";
const QUERY_SCOPE: &str = "wenlan:query";
const RESPONSE_LIMIT: usize = 64 * 1024;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RelayError {
    #[error("Invalid remote connection request")]
    InvalidInput,
    #[error("Remote connection requires device authorization")]
    Unauthorized,
    #[error("Remote connection unavailable; retry later")]
    Unavailable,
    #[error("Remote connection rate limited")]
    RateLimited { retry_after_seconds: Option<u64> },
    #[error("Remote connection rejected (HTTP {0})")]
    Rejected(u16),
    #[error("Unexpected remote connection response")]
    InvalidResponse,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceCredential {
    pub id: String,
    pub management_token: String,
    pub expires_at: u64,
}

impl std::fmt::Debug for DeviceCredential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("DeviceCredential([redacted])")
    }
}

impl DeviceCredential {
    fn validate_shape(&self) -> Result<(), RelayError> {
        if valid_id(&self.id, 32) && valid_id(&self.management_token, 32) && self.expires_at > 0 {
            Ok(())
        } else {
            Err(RelayError::Unauthorized)
        }
    }

    fn validate(&self) -> Result<(), RelayError> {
        self.validate_shape()?;
        if self.expires_at <= now_ms() {
            return Err(RelayError::Unauthorized);
        }
        Ok(())
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorCandidate {
    pub tunnel_origin: String,
    pub backend_token: String,
    pub space: String,
}

impl std::fmt::Debug for ConnectorCandidate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ConnectorCandidate([redacted])")
    }
}

impl ConnectorCandidate {
    fn validate(&self) -> Result<(), RelayError> {
        let url = Url::parse(&self.tunnel_origin).map_err(|_| RelayError::InvalidInput)?;
        let label = url
            .host_str()
            .and_then(|host| host.strip_suffix(".trycloudflare.com"));
        let valid_label = label.is_some_and(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        });
        if url.scheme() != "https"
            || url.port().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
            || !valid_label
            || !valid_id(&self.backend_token, 32)
            || !valid_space(&self.space)
        {
            return Err(RelayError::InvalidInput);
        }
        Ok(())
    }
}

/// What the relay says about the app asking for access. Every identity field is
/// optional so the app keeps working against a relay that predates them; absent
/// means "not recognized". The display name is never taken from `client_name`
/// (the app registers it itself): the UI derives it from `redirect_host`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PairingView {
    pub pairing_id: String,
    pub client_id: String,
    pub resource: String,
    pub scopes: Vec<String>,
    pub expires_at: u64,
    #[serde(default)]
    pub client_name: Option<String>,
    #[serde(default)]
    pub redirect_host: Option<String>,
    #[serde(default)]
    pub known_client: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GrantStatus {
    Active,
    Inactive,
}

/// Why a connection stopped working. A value this app does not know yet reads
/// as `Other` instead of failing the whole page.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GrantEndReason {
    Revoked,
    Reset,
    Expired,
    Replaced,
    #[serde(other)]
    Other,
}

/// What an app connected through the relay may do. `Query` searches and reads;
/// `Write` also adds, edits and deletes. A relay that predates the choice sends
/// nothing, and a value this app does not know reads as `Query`, the narrower one.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GrantAccess {
    Write,
    #[default]
    #[serde(other)]
    Query,
}

impl GrantAccess {
    fn as_str(self) -> &'static str {
        match self {
            GrantAccess::Query => "query",
            GrantAccess::Write => "write",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrantView {
    pub id: String,
    pub client_id: String,
    pub space: String,
    pub created_at: u64,
    pub expires_at: u64,
    pub status: GrantStatus,
    pub cleanup_pending: bool,
    #[serde(default)]
    pub client_name: Option<String>,
    #[serde(default)]
    pub redirect_host: Option<String>,
    #[serde(default)]
    pub known_client: bool,
    #[serde(default)]
    pub last_used_at: Option<u64>,
    #[serde(default)]
    pub end_reason: Option<GrantEndReason>,
    #[serde(default)]
    pub access: GrantAccess,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GrantPage {
    pub items: Vec<GrantView>,
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GrantRevocation {
    pub revoked: bool,
    pub cleanup_pending: bool,
}

#[derive(Deserialize)]
struct Success {
    success: bool,
}
#[derive(Deserialize)]
struct Approval {
    approved: bool,
}

#[derive(Clone)]
pub struct RelayClient {
    http: Client,
    origin: Url,
}

impl RelayClient {
    pub fn new() -> Result<Self, RelayError> {
        Self::build(
            Url::parse(RELAY_ORIGIN).map_err(|_| RelayError::InvalidInput)?,
            true,
        )
    }

    fn build(origin: Url, https_only: bool) -> Result<Self, RelayError> {
        let http = Client::builder()
            .https_only(https_only)
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|_| RelayError::Unavailable)?;
        Ok(Self { http, origin })
    }

    pub fn mcp_url(&self) -> String {
        format!("{RELAY_ORIGIN}/mcp")
    }

    fn endpoint(&self, path: &str) -> Url {
        // Paths are static or assembled from already validated opaque IDs.
        let mut url = self.origin.clone();
        url.set_path(path);
        url.set_query(None);
        url.set_fragment(None);
        url
    }

    fn request(
        &self,
        method: Method,
        path: &str,
        credential: Option<&DeviceCredential>,
    ) -> Result<RequestBuilder, RelayError> {
        let mut request = self
            .http
            .request(method, self.endpoint(path))
            .header("accept", "application/json");
        if let Some(credential) = credential {
            credential.validate()?;
            request = request
                .bearer_auth(&credential.management_token)
                .header("x-wenlan-device-id", &credential.id);
        }
        Ok(request)
    }

    async fn send(request: RequestBuilder) -> Result<Response, RelayError> {
        let response = request.send().await.map_err(|_| RelayError::Unavailable)?;
        if response.status().is_success() {
            Ok(response)
        } else {
            Err(response_error(&response))
        }
    }

    pub async fn enroll(
        &self,
        candidate: &ConnectorCandidate,
    ) -> Result<DeviceCredential, RelayError> {
        candidate.validate()?;
        let response = Self::send(
            self.request(Method::POST, "/devices", None)?
                .json(candidate),
        )
        .await?;
        if response.status() != StatusCode::CREATED {
            return Err(RelayError::InvalidResponse);
        }
        let credential: DeviceCredential = decode(response).await?;
        credential
            .validate()
            .map_err(|_| RelayError::InvalidResponse)?;
        Ok(credential)
    }

    pub async fn refresh(
        &self,
        credential: &DeviceCredential,
        candidate: &ConnectorCandidate,
    ) -> Result<(), RelayError> {
        candidate.validate()?;
        let response = Self::send(
            self.request(Method::POST, "/devices/refresh", Some(credential))?
                .json(candidate),
        )
        .await?;
        require_success(response).await
    }

    /// Do not blindly retry rotation after an ambiguous transport failure: the
    /// server may have invalidated the old credential before its reply was lost.
    pub async fn rotate(
        &self,
        credential: &DeviceCredential,
    ) -> Result<DeviceCredential, RelayError> {
        let response = Self::send(
            self.request(Method::POST, "/devices/rotate", Some(credential))?
                .json(&serde_json::json!({})),
        )
        .await?;
        let next: DeviceCredential = decode(response).await?;
        next.validate().map_err(|_| RelayError::InvalidResponse)?;
        if next.id != credential.id || next.management_token == credential.management_token {
            return Err(RelayError::InvalidResponse);
        }
        Ok(next)
    }

    pub async fn revoke_device(&self, credential: &DeviceCredential) -> Result<(), RelayError> {
        // Only this attenuation endpoint may use an expired credential. Do not
        // interpret local expiry or a remote 401 as confirmation of revocation.
        credential.validate_shape()?;
        let response = Self::send(
            self.request(Method::POST, "/devices/revoke", None)?
                .bearer_auth(&credential.management_token)
                .header("x-wenlan-device-id", &credential.id)
                .json(&serde_json::json!({})),
        )
        .await?;
        require_success(response).await
    }

    pub async fn inspect_pairing(
        &self,
        credential: &DeviceCredential,
        pairing_id: &str,
    ) -> Result<PairingView, RelayError> {
        if !valid_id(pairing_id, 32) {
            return Err(RelayError::InvalidInput);
        }
        let response = Self::send(self.request(
            Method::GET,
            &format!("/pairings/{pairing_id}"),
            Some(credential),
        )?)
        .await?;
        self.accept_view(decode(response).await?, Some(pairing_id))
    }

    /// Find a pending request by the short code the browser page shows
    /// (`XXXX-XXXX`). The code travels in the body, never in a URL. A shape the
    /// relay could never have issued fails here, before it spends one of the
    /// relay's limited misses.
    pub async fn lookup_pairing(
        &self,
        credential: &DeviceCredential,
        code: &str,
    ) -> Result<PairingView, RelayError> {
        let code = normalize_user_code(code).ok_or(RelayError::InvalidInput)?;
        let response = Self::send(
            self.request(Method::POST, "/pairings/lookup", Some(credential))?
                .json(&serde_json::json!({ "code": code })),
        )
        .await?;
        self.accept_view(decode(response).await?, None)
    }

    /// Tell the browser the person declined. Only the explicit 204 counts: any
    /// other success is not the contract.
    pub async fn deny_pairing(
        &self,
        credential: &DeviceCredential,
        pairing_id: &str,
    ) -> Result<(), RelayError> {
        if !valid_id(pairing_id, 32) {
            return Err(RelayError::InvalidInput);
        }
        let response = Self::send(
            self.request(
                Method::POST,
                &format!("/pairings/{pairing_id}/deny"),
                Some(credential),
            )?
            .json(&serde_json::json!({})),
        )
        .await?;
        if response.status() == StatusCode::NO_CONTENT {
            Ok(())
        } else {
            Err(RelayError::InvalidResponse)
        }
    }

    /// The same checks for a request found by id or by code: it must be for this
    /// relay's fixed resource and query-only scope, and not already expired.
    fn accept_view(
        &self,
        mut view: PairingView,
        expected_id: Option<&str>,
    ) -> Result<PairingView, RelayError> {
        if expected_id.map_or(!valid_id(&view.pairing_id, 32), |id| view.pairing_id != id)
            || view.client_id.is_empty()
            || view.client_id.len() > 2048
            || view.resource != self.mcp_url()
            || view.scopes != [QUERY_SCOPE]
            || view.expires_at <= now_ms()
        {
            return Err(RelayError::InvalidResponse);
        }
        view.client_name = clean_client_name(view.client_name.take());
        view.redirect_host = clean_redirect_host(view.redirect_host.take());
        Ok(view)
    }

    /// Only call after the person explicitly approves the inspected client and
    /// the selected data scope. No enrollment/start/reconnect path calls this.
    pub async fn approve_pairing(
        &self,
        credential: &DeviceCredential,
        view: &PairingView,
        space: &str,
        access: GrantAccess,
    ) -> Result<(), RelayError> {
        if !valid_id(&view.pairing_id, 32)
            || view.client_id.is_empty()
            || view.client_id.len() > 2048
            || view.resource != self.mcp_url()
            || view.scopes != [QUERY_SCOPE]
            || view.expires_at <= now_ms()
            || !valid_space(space)
        {
            return Err(RelayError::InvalidInput);
        }
        let body = serde_json::json!({ "approved": true, "clientId": view.client_id, "resource": view.resource, "space": space, "access": access.as_str() });
        let response = Self::send(
            self.request(
                Method::POST,
                &format!("/pairings/{}/approve", view.pairing_id),
                Some(credential),
            )?
            .json(&body),
        )
        .await?;
        if decode::<Approval>(response).await?.approved {
            Ok(())
        } else {
            Err(RelayError::InvalidResponse)
        }
    }

    pub async fn grants(
        &self,
        credential: &DeviceCredential,
        cursor: Option<&str>,
    ) -> Result<GrantPage, RelayError> {
        if cursor.is_some_and(|cursor| !valid_id(cursor, 16)) {
            return Err(RelayError::InvalidInput);
        }
        let mut request = self.request(Method::GET, "/grants", Some(credential))?;
        if let Some(cursor) = cursor {
            request = request.query(&[("cursor", cursor)]);
        }
        let mut page: GrantPage = decode(Self::send(request).await?).await?;
        for item in &mut page.items {
            item.client_name = clean_client_name(item.client_name.take());
            item.redirect_host = clean_redirect_host(item.redirect_host.take());
        }
        let mut ids = std::collections::HashSet::new();
        if page.items.len() > 25
            || page
                .cursor
                .as_deref()
                .is_some_and(|next| !valid_id(next, 16) || Some(next) == cursor)
            || page.items.iter().any(|item| {
                !valid_id(&item.id, 16)
                    || !ids.insert(&item.id)
                    || item.client_id.is_empty()
                    || item.client_id.len() > 2048
                    || !valid_space(&item.space)
                    || item.created_at > item.expires_at
            })
        {
            return Err(RelayError::InvalidResponse);
        }
        Ok(page)
    }

    pub async fn revoke_grant(
        &self,
        credential: &DeviceCredential,
        grant_id: &str,
    ) -> Result<GrantRevocation, RelayError> {
        if !valid_id(grant_id, 16) {
            return Err(RelayError::InvalidInput);
        }
        let response = self
            .request(
                Method::POST,
                &format!("/grants/{grant_id}/revoke"),
                Some(credential),
            )?
            .json(&serde_json::json!({}))
            .send()
            .await
            .map_err(|_| RelayError::Unavailable)?;
        let status = response.status();
        if status != StatusCode::OK && status != StatusCode::SERVICE_UNAVAILABLE {
            return Err(response_error(&response));
        }
        let result: GrantRevocation = decode(response).await?;
        if !result.revoked || (status == StatusCode::SERVICE_UNAVAILABLE) != result.cleanup_pending
        {
            return Err(RelayError::InvalidResponse);
        }
        Ok(result)
    }
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn valid_id(value: &str, minimum: usize) -> bool {
    (minimum..=128).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
/// Short pairing codes use this alphabet (no 0/O/1/I/L/U look-alikes).
const USER_CODE_ALPHABET: &str = "23456789ABCDEFGHJKMNPQRSTVWXYZ";
const USER_CODE_LEN: usize = 8;

/// Accept what a person types: any case, with spaces or dashes between groups.
/// `None` when the result could not be a code the relay issued.
pub(crate) fn normalize_user_code(input: &str) -> Option<String> {
    let code: String = input
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| c.to_ascii_uppercase())
        .collect();
    (code.chars().count() == USER_CODE_LEN && code.chars().all(|c| USER_CODE_ALPHABET.contains(c)))
        .then_some(code)
}

const MAX_CLIENT_NAME_CHARS: usize = 80;

/// Direction overrides and invisible marks can make one name read as another.
fn is_display_spoofing(c: char) -> bool {
    matches!(
        c,
        '\u{061C}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}' | '\u{FEFF}'
    )
}

/// The name an app registered for itself is untrusted text. Keep it short and
/// printable; an empty result is treated as no name.
fn clean_client_name(name: Option<String>) -> Option<String> {
    let cleaned: String = name?
        .chars()
        .filter(|c| !c.is_control() && !is_display_spoofing(*c))
        .collect();
    let cleaned: String = cleaned.trim().chars().take(MAX_CLIENT_NAME_CHARS).collect();
    (!cleaned.is_empty()).then_some(cleaned)
}

/// A redirect host is a lowercase DNS-style name or IPv4 address with an
/// optional port. Anything else is dropped and the request is shown as unknown.
fn clean_redirect_host(host: Option<String>) -> Option<String> {
    let host = host?.trim().to_ascii_lowercase();
    let (name, port) = match host.split_once(':') {
        Some((name, port)) => (name, Some(port)),
        None => (host.as_str(), None),
    };
    let port_ok = port.is_none_or(|port| {
        (1..=5).contains(&port.len()) && port.bytes().all(|b| b.is_ascii_digit())
    });
    let name_ok = !name.is_empty()
        && name.len() <= 253
        && name.split('.').all(|label| {
            (1..=63).contains(&label.len())
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        });
    (port_ok && name_ok).then_some(host)
}

/// What a person chose to share with web apps. One Space can never name the
/// whole-library value: that choice is refused, never widened.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum ScopeChoice {
    // A struct variant, not a unit one: serde ignores extra fields on an
    // internally tagged unit variant even with deny_unknown_fields.
    WholeLibrary {},
    Space { name: String },
}

impl ScopeChoice {
    /// The value saved in the profile and sent to the relay.
    pub fn saved_space(&self) -> Result<&str, &'static str> {
        match self {
            ScopeChoice::WholeLibrary {} => Ok(WHOLE_LIBRARY_SPACE),
            ScopeChoice::Space { name } if name.trim() == WHOLE_LIBRARY_SPACE => {
                Err("That Space name is reserved; choose another Space")
            }
            ScopeChoice::Space { name } => Ok(name),
        }
    }
}

fn valid_space(value: &str) -> bool {
    !value.is_empty()
        && value.trim() == value
        && value.encode_utf16().count() <= 256
        && !value.chars().any(char::is_control)
}

fn response_error(response: &Response) -> RelayError {
    match response.status() {
        StatusCode::UNAUTHORIZED => RelayError::Unauthorized,
        StatusCode::TOO_MANY_REQUESTS => RelayError::RateLimited {
            retry_after_seconds: response
                .headers()
                .get("retry-after")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok())
                .filter(|value| *value <= 3600),
        },
        status if status.is_server_error() => RelayError::Unavailable,
        status => RelayError::Rejected(status.as_u16()),
    }
}

async fn decode<T: DeserializeOwned>(mut response: Response) -> Result<T, RelayError> {
    if response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        != Some("application/json")
        || response
            .content_length()
            .is_some_and(|length| length > RESPONSE_LIMIT as u64)
    {
        return Err(RelayError::InvalidResponse);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| RelayError::Unavailable)?
    {
        if bytes.len().saturating_add(chunk.len()) > RESPONSE_LIMIT {
            return Err(RelayError::InvalidResponse);
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| RelayError::InvalidResponse)
}

async fn require_success(response: Response) -> Result<(), RelayError> {
    if decode::<Success>(response).await?.success {
        Ok(())
    } else {
        Err(RelayError::InvalidResponse)
    }
}

#[cfg(test)]
#[path = "remote_relay_tests.rs"]
mod tests;
