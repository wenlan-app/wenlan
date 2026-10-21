// SPDX-License-Identifier: AGPL-3.0-only
//! Native startup boundary shared by initial connection and process recovery.
use super::store::{Profile, Store, StoreError};
use super::{ConnectorCandidate, GrantAccess, RelayClient, RelayError};
use serde::Deserialize;

pub const TOKEN_ENV: &str = "WENLAN_REMOTE_MCP_TOKEN";

pub async fn storage<T: Send + 'static>(
    operation: impl FnOnce(Store) -> Result<T, StoreError> + Send + 'static,
) -> Result<T, String> {
    // Resolve the identity path before moving into the blocking pool.
    let store = Store::current();
    tokio::task::spawn_blocking(move || operation(store))
        .await
        .map_err(|_| "Remote access storage task failed".to_string())?
        .map_err(|error| error.to_string())
}

pub async fn enabled_profile() -> Result<Profile, String> {
    storage(|store| require_enabled(store.load()?)).await
}

pub async fn consent_profile(expected_revision: String) -> Result<Profile, String> {
    consent_profile_at(Store::current(), expected_revision).await
}

async fn consent_profile_at(store: Store, expected_revision: String) -> Result<Profile, String> {
    tokio::task::spawn_blocking(move || {
        let profile = require_enabled(store.load()?)?;
        if profile.revision() != expected_revision || profile.device().is_none() {
            return Err(StoreError::Stale);
        }
        Ok(profile)
    })
    .await
    .map_err(|_| "Remote access storage task failed".to_string())?
    .map_err(|error| error.to_string())
}

/// Re-read both local consent and the server intent immediately before approval.
/// A frontend-provided label or a stale inspection cannot authorize another client.
/// The access the person chose goes to the relay with the approval; choosing
/// write also records it in the profile, so the web-write process can start.
pub async fn approve_inspected(
    expected_revision: String,
    inspected: super::PairingView,
    access: GrantAccess,
) -> Result<(), String> {
    let client = RelayClient::new().map_err(|e| e.to_string())?;
    approve_inspected_at(
        Store::current(),
        client,
        expected_revision,
        inspected,
        access,
    )
    .await
}

async fn approve_inspected_at(
    store: Store,
    client: RelayClient,
    expected_revision: String,
    inspected: super::PairingView,
    access: GrantAccess,
) -> Result<(), String> {
    let profile = consent_profile_at(store.clone(), expected_revision.clone()).await?;
    let device = profile
        .device()
        .ok_or_else(|| StoreError::Stale.to_string())?;
    let current = client
        .inspect_pairing(device, &inspected.pairing_id)
        .await
        .map_err(|e| e.to_string())?;
    if current != inspected {
        return Err(StoreError::Stale.to_string());
    }
    consent_profile_at(store.clone(), expected_revision.clone()).await?;
    client
        .approve_pairing(device, &current, profile.space(), access)
        .await
        .map_err(|e| e.to_string())?;
    if access == GrantAccess::Write {
        // The relay already holds the write grant. If this cannot be saved the
        // grant is not lost: it can be removed from the apps list, and writes
        // are refused until the process runs.
        tokio::task::spawn_blocking(move || store.enable_write(&expected_revision))
            .await
            .map_err(|_| "Remote access storage task failed".to_string())?
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Replace the saved management key with a fresh one for the same device.
/// Only a profile that is on, at the revision the person saw, with a device that
/// has not ended, can be renewed. Nothing here enrolls or revokes a device for
/// another profile.
pub async fn rotate_device(expected_revision: String) -> Result<Profile, String> {
    let client = RelayClient::new().map_err(|e| e.to_string())?;
    rotate_device_at(Store::current(), client, expected_revision).await
}

async fn rotate_device_at(
    store: Store,
    client: RelayClient,
    expected_revision: String,
) -> Result<Profile, String> {
    let profile = consent_profile_at(store.clone(), expected_revision).await?;
    let device = profile
        .device()
        .ok_or_else(|| StoreError::Stale.to_string())?;
    // Never retried here: the relay may have switched keys before a lost reply.
    let next = client.rotate(device).await.map_err(|e| e.to_string())?;
    let stored = next.clone();
    let revision = profile.revision().to_string();
    let saved = tokio::task::spawn_blocking(move || store.attach_device(&revision, stored))
        .await
        .map_err(|_| "Remote access storage task failed".to_string())
        .and_then(|result| result.map_err(|e| e.to_string()));
    match saved {
        Ok(profile) => Ok(profile),
        Err(error) => {
            // The new key cannot be kept (a newer profile or a stop won the race):
            // end it rather than leave a live key nothing here can use.
            let _ = client.revoke_device(&next).await;
            Err(error)
        }
    }
}

fn require_enabled(profile: Option<Profile>) -> Result<Profile, StoreError> {
    profile
        .filter(Profile::enabled)
        .ok_or(StoreError::NotConfigured)
}

/// The two local servers a connection can run. Both are protected by the same
/// bearer token and pinned to the same scope; they differ in the tools they
/// expose and the name their requests carry in the daemon's activity history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpKind {
    /// Search and read only. Always runs.
    Query,
    /// Search, read, add, edit and delete. Runs only after an app was allowed to write.
    Write,
}

impl McpKind {
    /// The `--tool-profile` value, which `/connector-info` reports back.
    pub fn tool_profile(self) -> &'static str {
        match self {
            McpKind::Query => "query-only",
            McpKind::Write => "web-write",
        }
    }

    fn agent_name(self) -> &'static str {
        match self {
            McpKind::Query => "remote-mcp",
            McpKind::Write => "web-ai",
        }
    }
}

pub fn mcp_args(origin_url: &str, port: u16, profile: &Profile) -> Vec<String> {
    serve_args(McpKind::Query, origin_url, port, profile)
}

/// Arguments for the web-write process: the same launch as [`mcp_args`] with the
/// write tool profile and its own agent name.
pub fn write_mcp_args(origin_url: &str, port: u16, profile: &Profile) -> Vec<String> {
    serve_args(McpKind::Write, origin_url, port, profile)
}

fn serve_args(kind: McpKind, origin_url: &str, port: u16, profile: &Profile) -> Vec<String> {
    let mut args: Vec<String> = [
        "--origin-url",
        origin_url,
        "serve",
        "--host",
        "127.0.0.1",
        "--port",
        &port.to_string(),
        "--tool-profile",
        kind.tool_profile(),
        "--token-env",
        TOKEN_ENV,
        "--agent-name",
        kind.agent_name(),
    ]
    .map(str::to_string)
    .into();
    if profile.whole_library() {
        args.push("--whole-library".into());
    }
    args
}

/// The scope reaches the sidecar only through its environment. One Space is a
/// strict pin. The whole library clears both Space variables, so nothing the
/// app inherited can narrow it, and `--whole-library` refuses a pin anyway.
pub fn mcp_scope_env(profile: &Profile) -> [(&'static str, String); 2] {
    let pinned = if profile.whole_library() {
        String::new()
    } else {
        profile.space().to_string()
    };
    [
        ("WENLAN_SPACE", pinned),
        ("WENLAN_DEFAULT_SPACE", String::new()),
    ]
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConnectorInfo {
    contract_version: u32,
    server: String,
    tool_profile: String,
    space: String,
    authentication: String,
}

/// Probe the protected contract, not only a public /health response. Never
/// follow a redirect with the backend credential or report raw response text.
pub async fn verify_backend(port: u16, profile: &Profile) -> Result<(), RelayError> {
    verify_backend_as(McpKind::Query, port, profile).await
}

/// The same probe for either local server, matching the tool profile it reports.
pub async fn verify_backend_as(
    kind: McpKind,
    port: u16,
    profile: &Profile,
) -> Result<(), RelayError> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .map_err(|_| RelayError::Unavailable)?;
    let url = format!("http://127.0.0.1:{port}/connector-info");
    let mut response = client
        .get(&url)
        .bearer_auth(profile.backend_token())
        .send()
        .await
        .map_err(|_| RelayError::Unavailable)?;
    if response.status() != reqwest::StatusCode::OK {
        return Err(RelayError::InvalidResponse);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| RelayError::Unavailable)?
    {
        if bytes.len() + chunk.len() > 4096 {
            return Err(RelayError::InvalidResponse);
        }
        bytes.extend(chunk);
    }
    let info: ConnectorInfo =
        serde_json::from_slice(&bytes).map_err(|_| RelayError::InvalidResponse)?;
    if info.contract_version != 1
        || info.server != "wenlan-mcp"
        || info.tool_profile != kind.tool_profile()
        || info.space != profile.space()
        || info.authentication != "bearer"
    {
        return Err(RelayError::InvalidResponse);
    }
    let unauthenticated = client
        .get(url)
        .send()
        .await
        .map_err(|_| RelayError::Unavailable)?;
    if unauthenticated.status() != reqwest::StatusCode::UNAUTHORIZED {
        return Err(RelayError::InvalidResponse);
    }
    Ok(())
}

/// A failed or stale registration never returns a direct-tunnel fallback.
/// Keep this future alive through enrollment persistence/cleanup even if the
/// UI cancels startup. The caller owns stopping the transport on cancellation.
pub async fn register(profile: Profile, tunnel_url: String) -> Result<(), String> {
    register_at(
        Store::current(),
        RelayClient::new().map_err(|e| e.to_string())?,
        profile,
        tunnel_url,
    )
    .await
}

#[derive(Debug, thiserror::Error)]
pub enum RenewalError {
    #[error("{0}")]
    Profile(String),
    #[error(transparent)]
    Relay(#[from] RelayError),
}

/// Renew only an already enrolled device. Recovery must never silently enroll
/// a replacement device or rotate credentials and invalidate existing consent.
pub async fn renew(tunnel_url: String) -> Result<(), RenewalError> {
    let profile = enabled_profile().await.map_err(RenewalError::Profile)?;
    renew_at(Store::current(), RelayClient::new()?, profile, tunnel_url).await
}

async fn renew_at(
    store: Store,
    client: RelayClient,
    profile: Profile,
    tunnel_url: String,
) -> Result<(), RenewalError> {
    same_profile(&store, &profile)
        .await
        .map_err(RenewalError::Profile)?;
    let device = profile
        .device()
        .ok_or_else(|| RenewalError::Profile(StoreError::Stale.to_string()))?;
    client
        .refresh(
            device,
            &ConnectorCandidate {
                tunnel_origin: tunnel_url,
                backend_token: profile.backend_token().into(),
                space: profile.space().into(),
            },
        )
        .await?;
    same_profile(&store, &profile)
        .await
        .map_err(RenewalError::Profile)
}

async fn same_profile(store: &Store, expected: &Profile) -> Result<(), String> {
    let store = store.clone();
    let revision = expected.revision().to_string();
    tokio::task::spawn_blocking(move || {
        let current = require_enabled(store.load()?)?;
        if current.revision() != revision {
            return Err(StoreError::Stale);
        }
        Ok(())
    })
    .await
    .map_err(|_| "Remote access storage task failed".to_string())?
    .map_err(|e| e.to_string())
}

async fn register_at(
    store: Store,
    client: RelayClient,
    profile: Profile,
    tunnel_url: String,
) -> Result<(), String> {
    same_profile(&store, &profile).await?;
    let candidate = ConnectorCandidate {
        tunnel_origin: tunnel_url,
        backend_token: profile.backend_token().into(),
        space: profile.space().into(),
    };
    if let Some(device) = profile.device() {
        client
            .refresh(device, &candidate)
            .await
            .map_err(|e| e.to_string())?;
        same_profile(&store, &profile).await?;
    } else {
        let device = client.enroll(&candidate).await.map_err(|e| e.to_string())?;
        let stored_device = device.clone();
        let revision = profile.revision().to_string();
        let saved =
            tokio::task::spawn_blocking(move || store.attach_device(&revision, stored_device))
                .await
                .map_err(|_| "Remote access storage task failed".to_string())
                .and_then(|result| result.map_err(|e| e.to_string()));
        if let Err(error) = saved {
            // A stale completion must not enable the new profile. The caller
            // closes the protected backend even if the network revoke fails.
            let _ = client.revoke_device(&device).await;
            return Err(error);
        }
    }
    Ok(())
}

/// Prepare durable stop intent without discarding credentials on write failure.
/// The caller must stop transport before finishing, even if preparation fails.
pub(crate) async fn prepare_disconnect(
    expected_revision: Option<String>,
) -> Result<super::disconnect::StopPlan, String> {
    let store = Store::current();
    tokio::task::spawn_blocking(move || {
        super::disconnect::prepare(&store, expected_revision.as_deref())
    })
    .await
    .map_err(|_| {
        "Local transport stop requested, but saved settings and server revocation are unconfirmed. Retry disconnect before restarting the App.".to_string()
    })
}

pub(crate) async fn finish_prepared_disconnect(
    plan: super::disconnect::StopPlan,
) -> Result<(), String> {
    super::disconnect::finish(
        Store::current(),
        RelayClient::new().map_err(|e| e.to_string())?,
        plan,
    )
    .await
}

#[cfg(test)]
mod tests;
