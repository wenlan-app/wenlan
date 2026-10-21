// SPDX-License-Identifier: AGPL-3.0-only
use crate::remote_relay::{
    reverse_peer::WriteRoute,
    reverse_runtime::{self, ActiveReverse},
    runtime as relay_runtime,
    runtime::{McpKind, RenewalError},
    store::Profile,
    RelayError, RELAY_ORIGIN,
};
use serde::{Deserialize, Serialize};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use tauri::Manager;
use tauri_plugin_shell::ShellExt;
use tokio::time::{sleep, timeout, Duration};

const MCP_SIDECAR_NAME: &str = "wenlan-mcp";

/// Port range for wenlan-mcp serve (high ports to avoid collisions).
pub const PORT_RANGE_START: u16 = 18080;
const PORT_RANGE_LEN: u16 = 4;

fn port_range_start() -> u16 {
    #[cfg(debug_assertions)]
    if let Some(port) = std::env::var("WENLAN_DEV_REMOTE_PORT_START")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .filter(|port| *port <= u16::MAX - (PORT_RANGE_LEN - 1))
    {
        return port;
    }
    PORT_RANGE_START
}

#[derive(Debug, Clone, Deserialize, Eq, PartialEq, Serialize)]
struct CloudflaredOwner {
    pid: u32,
    port: u16,
    identity: String,
}

fn cloudflared_owner_authorizes_signal(
    owner: &CloudflaredOwner,
    expected: Option<&CloudflaredOwner>,
    range_start: u16,
    live_identity: Option<&str>,
) -> bool {
    expected.is_none_or(|expected| expected == owner)
        && (range_start..=range_start + (PORT_RANGE_LEN - 1)).contains(&owner.port)
        && live_identity == Some(owner.identity.as_str())
}

/// Status of the authenticated remote connection.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RemoteAccessStatus {
    Off,
    Starting,
    Connected {
        /// Legacy wire field; reverse transport has no public tunnel URL.
        tunnel_url: Option<String>,
        /// Stable relay URL (if relay registration succeeded).
        relay_url: Option<String>,
    },
    Error {
        error: String,
    },
}

/// Web access stopped without the person asking. The webview turns this into a
/// localized system notification: only it knows the display language, so no
/// user-facing text lives in native code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RemoteAccessNotice {
    /// Reconnecting gave up, or the relay no longer accepts this computer.
    Stopped,
    /// The 90-day device key ended.
    Expired,
}

/// Whether giving up deserves a notification, and which one. `retrying` means a
/// later attempt is already scheduled, so nothing has stopped yet.
fn give_up_notice(
    error: &RenewalError,
    retrying: bool,
    device_expires_at: Option<u64>,
    now: u64,
) -> Option<RemoteAccessNotice> {
    if retrying {
        return None;
    }
    let key_ended = matches!(error, RenewalError::Relay(RelayError::Unauthorized))
        && device_expires_at.is_some_and(|expires_at| expires_at <= now);
    Some(if key_ended {
        RemoteAccessNotice::Expired
    } else {
        RemoteAccessNotice::Stopped
    })
}

fn generation_is_current(generation: u64, expected: u64) -> bool {
    generation == expected
}

fn try_begin_start(
    status: &mut RemoteAccessStatus,
    generation: &mut u64,
    expected_generation: Option<u64>,
    shutdown_pending: bool,
) -> Option<u64> {
    if shutdown_pending
        || matches!(
            status,
            RemoteAccessStatus::Starting | RemoteAccessStatus::Connected { .. }
        )
    {
        return None;
    }
    if let Some(expected) = expected_generation {
        if !generation_is_current(*generation, expected) {
            return None;
        }
    } else {
        *generation = generation.wrapping_add(1);
    }
    *status = RemoteAccessStatus::Starting;
    Some(*generation)
}

fn mark_off(status: &mut RemoteAccessStatus, generation: &mut u64) -> u64 {
    *generation = generation.wrapping_add(1);
    *status = RemoteAccessStatus::Off;
    *generation
}

async fn remote_generation_is_current(
    app_handle: &tauri::AppHandle,
    expected_generation: u64,
) -> bool {
    let state = app_handle.state::<std::sync::Arc<tokio::sync::RwLock<crate::state::AppState>>>();
    let app_state = { state.read().await.remote_access.clone() };
    let ra = app_state.lock().await;
    generation_is_current(ra.generation, expected_generation)
}

async fn wait_for_generation_change(app_handle: &tauri::AppHandle, expected_generation: u64) {
    while remote_generation_is_current(app_handle, expected_generation).await {
        sleep(Duration::from_millis(50)).await;
    }
}

/// Runtime state owns the local MCP process and outbound relay connection.
pub struct RemoteAccessState {
    pub status: RemoteAccessStatus,
    pub mcp_child: Option<tauri_plugin_shell::process::CommandChild>,
    reverse: Option<ActiveReverse>,
    pub port: Option<u16>,
    /// The web-write server, which runs only after an app was allowed to write.
    /// The search-only server above never depends on it.
    pub write_child: Option<tauri_plugin_shell::process::CommandChild>,
    pub write_port: Option<u16>,
    /// Where the relay connection sends requests for a write grant. Set while
    /// the write server is ready; empty otherwise, which sends them to the
    /// search-only server, and it refuses them.
    write_route: WriteRoute,
    /// One start of the write server at a time, so a failed start can discard
    /// what it spawned without touching another start's process.
    write_starting: bool,
    pending_stops: Vec<crate::remote_relay::shutdown::PendingProcess>,
    orphan_cleanup_failed: bool,
    /// Invalidates stale start/reconnect tasks when the user turns access off.
    pub generation: u64,
    /// Set when access stops on its own; the webview takes it once. Holding it
    /// here (not only in an event) means a notice raised before the window's
    /// listener exists, such as a key that ended while the app was closed, is
    /// delivered on the next load instead of lost.
    pub pending_notice: Option<RemoteAccessNotice>,
}

impl Default for RemoteAccessState {
    fn default() -> Self {
        Self {
            status: RemoteAccessStatus::Off,
            mcp_child: None,
            reverse: None,
            port: None,
            write_child: None,
            write_port: None,
            write_route: WriteRoute::default(),
            write_starting: false,
            pending_stops: Vec::new(),
            orphan_cleanup_failed: false,
            generation: 0,
            pending_notice: None,
        }
    }
}

async fn remote_access_mutex<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
) -> std::sync::Arc<tokio::sync::Mutex<RemoteAccessState>> {
    let state = app_handle.state::<std::sync::Arc<tokio::sync::RwLock<crate::state::AppState>>>();
    // Do not hold the AppState RwLock while waiting for the controller mutex.
    let remote = state.read().await.remote_access.clone();
    remote
}

/// What the connection test needs: the local protected port and the id of the
/// live relay connection this app opened. `None` unless fully connected.
pub(crate) fn connection_target(state: &RemoteAccessState) -> Option<(u16, String)> {
    live_target(
        &state.status,
        state.port,
        state
            .reverse
            .as_ref()
            .map(|active| active.connection.connection_id()),
    )
}

/// The decision behind `connection_target`, on plain values so every
/// combination can be tested without opening a relay connection.
fn live_target(
    status: &RemoteAccessStatus,
    port: Option<u16>,
    connection_id: Option<&str>,
) -> Option<(u16, String)> {
    if !matches!(status, RemoteAccessStatus::Connected { .. }) {
        return None;
    }
    Some((port?, connection_id?.to_owned()))
}

/// Record the notice and wake the webview. The webview takes it with
/// `take_remote_access_notice`, which is the only consumer, so a notice is shown
/// once however many times it listens or reloads.
async fn publish_notice<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    notice: RemoteAccessNotice,
) {
    use tauri::Emitter;
    let remote = remote_access_mutex(app_handle).await;
    remote.lock().await.pending_notice = Some(notice);
    let _ = app_handle.emit("remote-access-notice", notice);
}

fn spawn_and_store_child<T, P>(
    validation: Result<(), String>,
    slot: &mut Option<T>,
    port_slot: &mut Option<u16>,
    port: u16,
    spawn: impl FnOnce() -> Result<(P, T), String>,
    post_spawn: impl FnOnce(&T) -> Result<(), String>,
) -> Result<P, String> {
    validation?;
    if slot.is_some() {
        return Err("Remote access child slot is already occupied.".into());
    }
    // The slot check and spawn call are synchronous. There is no child to
    // drop if validation rejects the request or spawning fails.
    let (payload, child) = spawn()?;
    *slot = Some(child);
    *port_slot = Some(port);
    // Deliberately do not roll back on failure: the caller must coordinate
    // cleanup through the controller state and bounded stop path.
    post_spawn(slot.as_ref().expect("stored child"))?;
    Ok(payload)
}

fn validate_mcp_spawn(ra: &RemoteAccessState, generation: u64) -> Result<(), String> {
    if !generation_is_current(ra.generation, generation) {
        return Err("Remote access start cancelled.".into());
    }
    if !matches!(
        ra.status,
        RemoteAccessStatus::Starting | RemoteAccessStatus::Connected { .. }
    ) {
        return Err("Remote access is not active.".into());
    }
    if !ra.pending_stops.is_empty() || ra.orphan_cleanup_failed {
        return Err(SHUTDOWN_UNCONFIRMED.into());
    }
    if ra.mcp_child.is_some() {
        return Err(format!("{} is already running.", MCP_SIDECAR_NAME));
    }
    Ok(())
}

fn validate_write_spawn(ra: &RemoteAccessState, generation: u64) -> Result<(), String> {
    if !generation_is_current(ra.generation, generation) {
        return Err("Remote access start cancelled.".into());
    }
    if !matches!(
        ra.status,
        RemoteAccessStatus::Starting | RemoteAccessStatus::Connected { .. }
    ) {
        return Err("Remote access is not active.".into());
    }
    if !ra.pending_stops.is_empty() || ra.orphan_cleanup_failed {
        return Err(SHUTDOWN_UNCONFIRMED.into());
    }
    if ra.mcp_child.is_none() {
        return Err(format!(
            "{} search server is not running.",
            MCP_SIDECAR_NAME
        ));
    }
    if ra.write_child.is_some() {
        return Err(format!(
            "{} web-write is already running.",
            MCP_SIDECAR_NAME
        ));
    }
    Ok(())
}

pub(crate) struct StartupResume {
    generation: u64,
    revision: String,
}

fn validate_reconnect_profile(
    profile: Option<Profile>,
    expected_revision: &str,
) -> Result<Profile, String> {
    let profile = profile.ok_or_else(|| "Remote access is not configured.".to_string())?;
    if profile.revision() != expected_revision {
        return Err("Remote access settings changed; refresh before reconnecting.".into());
    }
    if profile.view().disconnect_pending || !profile.enabled() {
        return Err("Remote access is disabled or disconnect is pending.".into());
    }
    Ok(profile)
}

/// A reconnect ticket binds transport cleanup and restart to the same saved intent.
/// It never enables, rotates, or revokes the persisted profile.
/// Whether a saved scope can be shared now. The whole library names no Space,
/// so it never asks the daemon; one Space must still exist.
pub(crate) async fn confirm_saved_scope(
    client: &crate::api::WenlanClient,
    saved_space: &str,
) -> Result<(), String> {
    if saved_space == crate::remote_relay::WHOLE_LIBRARY_SPACE {
        return Ok(());
    }
    let spaces: Vec<wenlan_types::Space> = client.get_json("/api/spaces").await?;
    if !spaces.iter().any(|space| space.name == saved_space) {
        return Err("The selected Space no longer exists".into());
    }
    Ok(())
}

pub(crate) async fn prepare_reconnect(
    app_handle: &tauri::AppHandle,
    expected_revision: String,
    client: crate::api::WenlanClient,
) -> Result<StartupResume, String> {
    let remote = remote_access_mutex(app_handle).await;
    let generation = remote.lock().await.generation;
    let profile = validate_reconnect_profile(
        relay_runtime::storage(|store| store.load()).await?,
        &expected_revision,
    )?;
    confirm_saved_scope(&client, profile.space()).await?;
    // Space lookup may race with Stop or a profile edit. Reject before touching
    // transport, and retain the generation guard through cleanup and restart.
    validate_reconnect_profile(
        relay_runtime::storage(|store| store.load()).await?,
        &expected_revision,
    )?;
    let generation = transition_off(app_handle, Some(generation))
        .await
        .ok_or_else(|| "Remote access reconnect cancelled.".to_string())?;
    confirm_reconnect_cleanup(&remote, generation).await?;
    validate_reconnect_profile(
        relay_runtime::storage(|store| store.load()).await?,
        &expected_revision,
    )?;
    Ok(StartupResume {
        generation,
        revision: expected_revision,
    })
}

async fn confirm_reconnect_cleanup(
    remote: &tokio::sync::Mutex<RemoteAccessState>,
    generation: u64,
) -> Result<(), String> {
    let ra = remote.lock().await;
    if !generation_is_current(ra.generation, generation) {
        return Err("Remote access reconnect cancelled.".into());
    }
    disconnect_result_for(&ra, Ok(()))
}

pub(crate) async fn resume_reconnect(app_handle: tauri::AppHandle, ticket: StartupResume) {
    toggle_on_inner(
        app_handle,
        0,
        Some(ticket.generation),
        Some(ticket.revision),
    )
    .await;
}

/// Renew Web access: stop the connection, give this computer a fresh 90-day key,
/// and bring the connection back on it. Apps connected before this need to
/// connect again, because the relay ends their access when the key changes.
///
/// When the relay does not hand back a new key the old one is kept and the
/// connection resumes on it, so a failed renew never leaves the person with
/// Web access silently switched off. An ambiguous failure (reply lost after the
/// relay already switched keys) shows up as the resumed connection being
/// refused; the person then turns Web access on again.
pub(crate) async fn renew_access(
    app_handle: &tauri::AppHandle,
    expected_revision: String,
    client: crate::api::WenlanClient,
) -> Result<(), String> {
    let ticket = prepare_reconnect(app_handle, expected_revision.clone(), client).await?;
    let (revision, outcome) = match relay_runtime::rotate_device(expected_revision).await {
        Ok(profile) => (profile.revision().to_string(), Ok(())),
        Err(error) => (ticket.revision.clone(), Err(error)),
    };
    tauri::async_runtime::spawn(resume_reconnect(
        app_handle.clone(),
        StartupResume {
            generation: ticket.generation,
            revision,
        },
    ));
    outcome
}

/// Cleanup does not require a healthy daemon, local indexing or a file watcher.
/// A deferred resume ticket cannot override a later user action or profile edit.
pub(crate) async fn prepare_startup(app_handle: tauri::AppHandle) -> Option<StartupResume> {
    use crate::remote_relay::startup::{action, StartupAction};
    let generation = {
        let state =
            app_handle.state::<std::sync::Arc<tokio::sync::RwLock<crate::state::AppState>>>();
        let remote = { state.read().await.remote_access.clone() };
        let ra = remote.lock().await;
        if !matches!(ra.status, RemoteAccessStatus::Off) {
            return None;
        }
        ra.generation
    };
    let profile = match relay_runtime::storage(|store| store.load()).await {
        Ok(profile) => profile,
        Err(error) => {
            if let Some(off_generation) = transition_off(&app_handle, Some(generation)).await {
                let _ = publish_disconnect_result(&app_handle, off_generation, Err(format!(
                    "Remote access settings could not be read ({error}); access was not resumed. Retry disconnect before restarting the App."
                ))).await;
            }
            log::error!(
                "[remote-access] Startup profile unavailable; access not resumed: {}",
                error
            );
            return None;
        }
    };
    match action(profile.as_ref(), crate::remote_relay::now_ms()) {
        StartupAction::Resume => Some(StartupResume {
            generation,
            revision: profile?.revision().to_string(),
        }),
        StartupAction::Disconnect => {
            if let Some(profile) = profile {
                disconnect_startup(&app_handle, generation, profile).await;
            }
            None
        }
        StartupAction::StayOff => {
            transition_off(&app_handle, Some(generation)).await;
            None
        }
    }
}

async fn disconnect_startup(app_handle: &tauri::AppHandle, generation: u64, profile: Profile) {
    let key_ended = profile.enabled()
        && profile
            .device()
            .is_some_and(|device| device.expires_at <= crate::remote_relay::now_ms());
    let plan = relay_runtime::prepare_disconnect(Some(profile.revision().to_string())).await;
    let Some(off_generation) = transition_off(app_handle, Some(generation)).await else {
        return;
    };
    let result = match plan {
        Ok(plan) => relay_runtime::finish_prepared_disconnect(plan).await,
        Err(error) => Err(error),
    };
    if let Err(error) = &result {
        log::warn!(
            "[remote-access] Startup disconnect remains pending: {}",
            error
        );
    }
    let _ = publish_disconnect_result(app_handle, off_generation, result).await;
    if key_ended {
        publish_notice(app_handle, RemoteAccessNotice::Expired).await;
    }
}

async fn publish_disconnect_result(
    app_handle: &tauri::AppHandle,
    off_generation: u64,
    result: Result<(), String>,
) -> Result<(), String> {
    use tauri::Emitter;
    // Notify after durable cleanup, not just after transport shutdown, so an
    // already open settings panel refreshes its pending-disconnect profile.
    let state = app_handle.state::<std::sync::Arc<tokio::sync::RwLock<crate::state::AppState>>>();
    let remote = { state.read().await.remote_access.clone() };
    let mut ra = remote.lock().await;
    if let Some(status) = apply_disconnect_result(&mut ra, off_generation, result) {
        let _ = app_handle.emit("remote-access-status", &status);
        return match status {
            RemoteAccessStatus::Error { error } => Err(error),
            _ => Ok(()),
        };
    }
    Err(
        "Remote access changed while disconnect was finishing; check the current connection state."
            .into(),
    )
}

const SHUTDOWN_UNCONFIRMED: &str = "Local remote-access processes have not been confirmed stopped. New connections are blocked; retry Stop access.";
const EXIT_CLEANUP_LIMIT: Duration = Duration::from_secs(8);

/// Stop only the transport and local processes owned by this app before it
/// exits. Unlike [`toggle_off`], this deliberately does not change the saved
/// profile or revoke its device, so a later launch may reconnect.
pub(crate) async fn shutdown_for_exit<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
) -> Result<(), String> {
    timeout(EXIT_CLEANUP_LIMIT, async {
        transition_off(app_handle, None)
            .await
            .ok_or_else(|| "Remote access exit cleanup became stale".to_string())?;
        ensure_shutdown_confirmed(app_handle).await
    })
    .await
    .map_err(|_| {
        format!("Remote access exit cleanup did not finish within {EXIT_CLEANUP_LIMIT:?}")
    })?
}

pub(crate) async fn ensure_shutdown_confirmed<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
) -> Result<(), String> {
    let state = app_handle.state::<std::sync::Arc<tokio::sync::RwLock<crate::state::AppState>>>();
    let remote = { state.read().await.remote_access.clone() };
    let ra = remote.lock().await;
    disconnect_result_for(&ra, Ok(()))
}

fn disconnect_result_for(
    state: &RemoteAccessState,
    result: Result<(), String>,
) -> Result<(), String> {
    if state.pending_stops.is_empty() && !state.orphan_cleanup_failed {
        result
    } else {
        Err(match result {
            Ok(()) => SHUTDOWN_UNCONFIRMED.to_string(),
            Err(error) => format!("{SHUTDOWN_UNCONFIRMED} {error}"),
        })
    }
}

fn apply_disconnect_result(
    state: &mut RemoteAccessState,
    off_generation: u64,
    result: Result<(), String>,
) -> Option<RemoteAccessStatus> {
    if !generation_is_current(state.generation, off_generation) {
        return None;
    }
    let status = match disconnect_result_for(state, result) {
        Ok(()) => RemoteAccessStatus::Off,
        Err(error) => RemoteAccessStatus::Error { error },
    };
    state.status = status.clone();
    Some(status)
}

pub(crate) async fn resume_startup(app_handle: tauri::AppHandle, ticket: StartupResume) {
    use crate::remote_relay::startup::{action, StartupAction};
    if !remote_generation_is_current(&app_handle, ticket.generation).await {
        return;
    }
    let profile = match relay_runtime::storage(|store| store.load()).await {
        Ok(Some(profile)) if profile.revision() == ticket.revision => profile,
        _ => {
            transition_off(&app_handle, Some(ticket.generation)).await;
            return;
        }
    };
    match action(Some(&profile), crate::remote_relay::now_ms()) {
        StartupAction::Resume => toggle_on_with_retries(app_handle, 0, ticket.generation).await,
        StartupAction::Disconnect => {
            disconnect_startup(&app_handle, ticket.generation, profile).await
        }
        StartupAction::StayOff => {}
    }
}

/// Kill any orphaned wenlan-mcp processes on the remote access port range.
/// These accumulate when the Wenlan app restarts without cleanly shutting down
/// its child processes (the in-memory handles are lost on restart).
pub fn cleanup_orphaned_mcp() -> Result<(), String> {
    use crate::remote_relay::orphan::{cleanup, Observation};
    let my_pid = std::process::id();
    let range_start = port_range_start();
    let mut unconfirmed = false;
    for port in range_start..=range_start + (PORT_RANGE_LEN - 1) {
        let Ok(listeners) = crate::remote_access_platform::listener_pids_for_port(port) else {
            unconfirmed = true;
            continue;
        };
        for pid in listeners {
            if pid == my_pid {
                continue;
            }
            match measured_process_identity(pid) {
                Observation::Gone => {}
                Observation::Unknown => unconfirmed = true,
                Observation::Identity(identity) => {
                    if !identity_command(&identity)
                        .is_some_and(|command| is_expected_remote_mcp_command(command, port))
                    {
                        continue;
                    }
                    let outcome = cleanup(
                        true,
                        &identity,
                        || measured_process_identity(pid),
                        |force| signal_owned_process(pid, force, &identity),
                        cleanup_pause,
                    );
                    unconfirmed |= !outcome.confirmed();
                }
            }
        }
    }
    if unconfirmed {
        Err(SHUTDOWN_UNCONFIRMED.into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
fn parse_listener_pids(
    success: bool,
    code: Option<i32>,
    stdout: &[u8],
    stderr: &[u8],
) -> Option<Vec<u32>> {
    crate::remote_access_platform::parse_lsof_listener_pids(success, code, stdout, stderr).ok()
}

fn cleanup_pause() {
    std::thread::sleep(Duration::from_millis(100));
}

fn cleanup_remote_orphans() -> Result<(), String> {
    let mcp = cleanup_orphaned_mcp();
    let tunnel = cleanup_owned_cloudflared(None);
    mcp.and(tunnel)
}

#[cfg(unix)]
fn signal_owned_process(pid: u32, force: bool, _identity: &str) -> bool {
    std::process::Command::new("/bin/kill")
        .args([if force { "-KILL" } else { "-TERM" }, &pid.to_string()])
        .output()
        .is_ok_and(|output| output.status.success())
}

#[cfg(windows)]
fn signal_owned_process(pid: u32, _force: bool, identity: &str) -> bool {
    crate::remote_access_platform::signal_process(pid, identity)
}

#[cfg(not(any(unix, windows)))]
fn signal_owned_process(_pid: u32, _force: bool, _identity: &str) -> bool {
    false
}

fn identity_command(identity: &str) -> Option<&str> {
    identity.split_once('\n').map(|(_, command)| command)
}

fn cloudflared_owner_path() -> PathBuf {
    crate::identity_paths::mcp_config_dir().join("cloudflared-owner.json")
}

fn cloudflared_owner_lock_path(path: &Path) -> PathBuf {
    let mut lock_path = path.as_os_str().to_os_string();
    lock_path.push(".lock");
    PathBuf::from(lock_path)
}

struct CloudflaredOwnerLock<'a> {
    path: &'a Path,
    _file: std::fs::File,
}

impl CloudflaredOwnerLock<'_> {
    fn read(&self) -> Result<Option<CloudflaredOwner>, String> {
        let contents = match std::fs::read(self.path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(format!(
                    "Failed to read cloudflared ownership receipt at {}: {error}",
                    self.path.display()
                ));
            }
        };
        restrict_private_file(self.path, "cloudflared ownership receipt")?;
        serde_json::from_slice(&contents)
            .map(Some)
            .map_err(|error| {
                format!(
                    "Invalid cloudflared ownership receipt at {}: {error}",
                    self.path.display()
                )
            })
    }

    #[cfg(test)]
    fn write(&self, owner: &CloudflaredOwner) -> Result<(), String> {
        use std::io::Write;

        let contents = serde_json::to_vec(owner).map_err(|error| {
            format!("Failed to serialize cloudflared ownership receipt: {error}")
        })?;
        let parent = self.path.parent().ok_or_else(|| {
            format!(
                "Cloudflared ownership receipt has no parent: {}",
                self.path.display()
            )
        })?;
        let mut staged = tempfile::NamedTempFile::new_in(parent).map_err(|error| {
            format!(
                "Failed to stage cloudflared ownership receipt beside {}: {error}",
                self.path.display()
            )
        })?;
        staged.write_all(&contents).map_err(|error| {
            format!(
                "Failed to write staged cloudflared ownership receipt for {}: {error}",
                self.path.display()
            )
        })?;
        staged.as_file().sync_all().map_err(|error| {
            format!(
                "Failed to sync staged cloudflared ownership receipt for {}: {error}",
                self.path.display()
            )
        })?;
        staged.persist(self.path).map_err(|error| {
            format!(
                "Failed to atomically replace cloudflared ownership receipt at {}: {}",
                self.path.display(),
                error.error
            )
        })?;
        restrict_private_file(self.path, "cloudflared ownership receipt")
    }

    fn remove_if_matches(&self, expected: &CloudflaredOwner) -> Result<(), String> {
        if self.read()?.as_ref() != Some(expected) {
            return Ok(());
        }
        match std::fs::remove_file(self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!(
                "Failed to remove cloudflared ownership receipt at {}: {error}",
                self.path.display()
            )),
        }
    }
}

fn with_cloudflared_owner_lock<T>(
    path: &Path,
    operation: impl FnOnce(&CloudflaredOwnerLock<'_>) -> Result<T, String>,
) -> Result<T, String> {
    let lock_path = cloudflared_owner_lock_path(path);
    prepare_private_parent(&lock_path, "cloudflared ownership lock")?;
    #[cfg(unix)]
    let lock_file = {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&lock_path)
    };
    #[cfg(not(unix))]
    let lock_file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path);
    let lock_file = lock_file.map_err(|error| {
        format!(
            "Failed to open cloudflared ownership lock at {}: {error}",
            lock_path.display()
        )
    })?;
    restrict_private_file(&lock_path, "cloudflared ownership lock")?;
    lock_file.lock().map_err(|error| {
        format!(
            "Failed to lock cloudflared ownership receipt at {}: {error}",
            lock_path.display()
        )
    })?;
    operation(&CloudflaredOwnerLock {
        path,
        _file: lock_file,
    })
}

#[cfg(test)]
fn read_cloudflared_owner_at(path: &Path) -> Result<Option<CloudflaredOwner>, String> {
    with_cloudflared_owner_lock(path, |receipt| receipt.read())
}

#[cfg(test)]
fn write_cloudflared_owner_at(path: &Path, owner: &CloudflaredOwner) -> Result<(), String> {
    with_cloudflared_owner_lock(path, |receipt| receipt.write(owner))
}

fn cleanup_owned_cloudflared(expected: Option<&CloudflaredOwner>) -> Result<(), String> {
    cleanup_owned_cloudflared_at(
        &cloudflared_owner_path(),
        expected,
        port_range_start(),
        measured_process_identity,
        signal_owned_process,
        cleanup_pause,
    )
}

fn cleanup_owned_cloudflared_at(
    path: &Path,
    expected: Option<&CloudflaredOwner>,
    range_start: u16,
    mut probe: impl FnMut(u32) -> crate::remote_relay::orphan::Observation,
    mut signal: impl FnMut(u32, bool, &str) -> bool,
    pause: impl FnMut(),
) -> Result<(), String> {
    use crate::remote_relay::orphan::{cleanup, CleanupOutcome};
    with_cloudflared_owner_lock(path, |receipt| {
        let owner = match receipt.read()? {
            Some(owner) => owner,
            None => return Ok(()),
        };
        let authorized = cloudflared_owner_authorizes_signal(
            &owner,
            expected,
            range_start,
            Some(&owner.identity),
        );
        if authorized
            && (owner.pid == 0
                || owner.pid == std::process::id()
                || !identity_command(&owner.identity)
                    .is_some_and(|command| is_expected_remote_tunnel_command(command, owner.port)))
        {
            return Err(SHUTDOWN_UNCONFIRMED.into());
        }
        let outcome = cleanup(
            authorized,
            &owner.identity,
            || probe(owner.pid),
            |force| signal(owner.pid, force, &owner.identity),
            pause,
        );
        if outcome.confirmed() {
            receipt.remove_if_matches(&owner)
        } else if outcome == CleanupOutcome::NotOwned {
            Ok(())
        } else {
            Err(SHUTDOWN_UNCONFIRMED.into())
        }
    })
    .map_err(|_| SHUTDOWN_UNCONFIRMED.to_string())
}

fn process_command_args(command: &str) -> Option<Vec<String>> {
    match command.strip_prefix("argv:") {
        Some(json) => serde_json::from_str(json).ok(),
        None => Some(command.split_whitespace().map(str::to_owned).collect()),
    }
}

fn process_executable_name<'a>(command: &str, executable: &'a str) -> &'a str {
    if command.starts_with("argv:") {
        executable.rsplit(['/', '\\']).next().unwrap_or_default()
    } else {
        Path::new(executable)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
    }
}

fn is_expected_remote_mcp_command(command: &str, port: u16) -> bool {
    let Some(args) = process_command_args(command) else {
        return false;
    };
    let Some(executable) = args.first() else {
        return false;
    };
    let file_name = process_executable_name(command, executable);
    let expected_executable = matches!(
        file_name,
        "wenlan-mcp"
            | "wenlan-mcp.exe"
            | "wenlan-mcp-aarch64-apple-darwin"
            | "wenlan-mcp-x86_64-unknown-linux-gnu"
            | "wenlan-mcp-aarch64-unknown-linux-gnu"
            | "wenlan-mcp-x86_64-pc-windows-msvc.exe"
    );
    let has_pair = |flag: &str, value: &str| {
        args.windows(2)
            .any(|pair| pair[0] == flag && pair[1] == value)
    };
    // The search-only server, or the web-write one an allowed app starts.
    let ours = has_pair("--agent-name", "remote-mcp")
        || (has_pair("--agent-name", "web-ai") && has_pair("--tool-profile", "web-write"));
    expected_executable
        && args.iter().any(|arg| arg == "serve")
        && has_pair("--port", &port.to_string())
        && ours
}

#[cfg(windows)]
fn measured_process_identity(pid: u32) -> crate::remote_relay::orphan::Observation {
    use crate::remote_relay::orphan::Observation;
    match crate::remote_access_platform::process_identity(pid) {
        Ok(Some((started, args))) => match serde_json::to_string(&args) {
            Ok(args) => Observation::Identity(format!("{started}\nargv:{args}")),
            Err(_) => Observation::Unknown,
        },
        Ok(None) => Observation::Gone,
        Err(()) => Observation::Unknown,
    }
}

#[cfg(not(windows))]
fn measured_process_identity(pid: u32) -> crate::remote_relay::orphan::Observation {
    use crate::remote_relay::orphan::Observation;
    let started = match read_process_field(pid, "lstart=") {
        Ok(Some(started)) => started,
        Ok(None) => return Observation::Gone,
        Err(()) => return Observation::Unknown,
    };
    let command = match read_process_field(pid, "command=") {
        Ok(Some(command)) => command,
        _ => return Observation::Unknown,
    };
    match read_process_field(pid, "lstart=") {
        Ok(Some(current)) if current == started => {
            Observation::Identity(format!("{started}\n{command}"))
        }
        _ => Observation::Unknown,
    }
}

#[cfg(unix)]
fn read_process_field(pid: u32, field: &str) -> Result<Option<String>, ()> {
    if pid == 0 {
        return Err(());
    }
    let output = std::process::Command::new("/bin/ps")
        .args(["-ww", "-p", &pid.to_string(), "-o", field])
        .output()
        .map_err(|_| ())?;
    parse_process_field(
        output.status.success(),
        output.status.code(),
        &output.stdout,
        &output.stderr,
    )
}

#[cfg(not(any(unix, windows)))]
fn read_process_field(_pid: u32, _field: &str) -> Result<Option<String>, ()> {
    Err(())
}

#[cfg(any(unix, test))]
fn parse_process_field(
    success: bool,
    code: Option<i32>,
    stdout: &[u8],
    stderr: &[u8],
) -> Result<Option<String>, ()> {
    if !success {
        return if code == Some(1) && stdout.is_empty() && stderr.is_empty() {
            Ok(None)
        } else {
            Err(())
        };
    }
    if !stderr.is_empty() {
        return Err(());
    }
    let text = std::str::from_utf8(stdout).map_err(|_| ())?.trim();
    if text.is_empty() || text.lines().count() != 1 || text.chars().any(char::is_control) {
        return Err(());
    }
    Ok(Some(text.to_string()))
}

fn is_expected_remote_tunnel_command(command: &str, port: u16) -> bool {
    let Some(args) = process_command_args(command) else {
        return false;
    };
    let Some(executable) = args.first() else {
        return false;
    };
    let file_name = process_executable_name(command, executable);
    let expected_executable = matches!(
        file_name,
        "cloudflared"
            | "cloudflared.exe"
            | "cloudflared-aarch64-apple-darwin"
            | "cloudflared-x86_64-apple-darwin"
            | "cloudflared-x86_64-unknown-linux-gnu"
            | "cloudflared-aarch64-unknown-linux-gnu"
            | "cloudflared-x86_64-pc-windows-msvc.exe"
    );
    let expected_url = format!("http://localhost:{port}");
    expected_executable
        && args.get(1).map(String::as_str) == Some("tunnel")
        && args
            .windows(2)
            .any(|pair| pair[0] == "--url" && pair[1] == expected_url)
}

fn listener_pid_for_port(port: u16) -> Option<u32> {
    let pids = crate::remote_access_platform::listener_pids_for_port(port).ok()?;
    let first = *pids.first()?;
    pids.iter().all(|pid| *pid == first).then_some(first)
}

/// Find an available port in the selected four-port range.
pub fn find_available_port() -> Option<u16> {
    let range_start = port_range_start();
    (range_start..=range_start + (PORT_RANGE_LEN - 1))
        .find(|&port| TcpListener::bind(("127.0.0.1", port)).is_ok())
}

fn create_private_dir(path: &Path, file_name: &str) -> Result<(), String> {
    std::fs::create_dir_all(path).map_err(|e| {
        format!(
            "Failed to create current {} directory at {}: {}",
            file_name,
            path.display(),
            e
        )
    })?;
    restrict_private_dir(path, file_name)
}

fn restrict_private_dir(path: &Path, file_name: &str) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).map_err(|e| {
            format!(
                "Failed to restrict current {} directory at {}: {}",
                file_name,
                path.display(),
                e
            )
        })?;
    }
    #[cfg(not(unix))]
    {
        let _ = (path, file_name);
    }
    Ok(())
}

fn restrict_private_file(path: &Path, file_name: &str) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    if !path.is_file() {
        return Err(format!(
            "Current {} path is not a file: {}",
            file_name,
            path.display()
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|e| {
            format!(
                "Failed to restrict current {} file at {}: {}",
                file_name,
                path.display(),
                e
            )
        })?;
    }
    #[cfg(not(unix))]
    {
        let _ = file_name;
    }
    Ok(())
}

fn prepare_private_parent(path: &Path, file_name: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        create_private_dir(parent, file_name)?;
    }
    Ok(())
}

/// Start remote access through local wenlan-mcp and native reverse transport.
/// Async — emits `remote-access-status` events as state changes.
/// Called from Tauri command handler — the command returns `Starting`
/// immediately and this runs in a background task.
pub fn toggle_on(
    app_handle: tauri::AppHandle,
    is_retry: bool,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
    Box::pin(toggle_on_inner(
        app_handle,
        if is_retry { 1 } else { 0 },
        None,
        None,
    ))
}

/// Start with explicit retry count (used by monitor auto-restart).
fn toggle_on_with_retries(
    app_handle: tauri::AppHandle,
    retry_count: u32,
    expected_generation: u64,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
    Box::pin(toggle_on_inner(
        app_handle,
        retry_count,
        Some(expected_generation),
        None,
    ))
}

async fn toggle_on_inner(
    app_handle: tauri::AppHandle,
    retry_count: u32,
    expected_generation: Option<u64>,
    expected_revision: Option<String>,
) {
    use tauri::Emitter;
    let operation_generation = {
        let remote = remote_access_mutex(&app_handle).await;
        let mut ra = remote.lock().await;
        if expected_generation
            .is_some_and(|expected| !generation_is_current(ra.generation, expected))
        {
            return;
        }
        let shutdown_pending = !ra.pending_stops.is_empty() || ra.orphan_cleanup_failed;
        if shutdown_pending {
            let status = RemoteAccessStatus::Error {
                error: SHUTDOWN_UNCONFIRMED.into(),
            };
            ra.status = status.clone();
            let _ = app_handle.emit("remote-access-status", &status);
            return;
        }
        let RemoteAccessState {
            status, generation, ..
        } = &mut *ra;
        let Some(generation) =
            try_begin_start(status, generation, expected_generation, shutdown_pending)
        else {
            return;
        };
        let _ = app_handle.emit("remote-access-status", &RemoteAccessStatus::Starting);
        generation
    };
    let result = async {
        let profile = relay_runtime::enabled_profile()
            .await
            .map_err(RenewalError::Profile)?;
        let profile = match expected_revision {
            Some(revision) => validate_reconnect_profile(Some(profile), &revision)
                .map_err(RenewalError::Profile)?,
            None => profile,
        };
        start_reverse(&app_handle, operation_generation, profile).await
    }
    .await;
    match result {
        Ok((port, mcp_rx, active)) => {
            let remote = remote_access_mutex(&app_handle).await;
            let mut ra = remote.lock().await;
            if !can_adopt_reverse(&ra, operation_generation, port, None) {
                drop(ra);
                let _ = active.connection.shutdown().await;
                return;
            }
            ra.reverse = Some(active);
            let status = RemoteAccessStatus::Connected {
                tunnel_url: None,
                relay_url: Some(format!("{RELAY_ORIGIN}/mcp")),
            };
            ra.status = status.clone();
            let _ = app_handle.emit("remote-access-status", &status);
            drop(ra);
            tauri::async_runtime::spawn(monitor_reverse(
                app_handle,
                mcp_rx,
                retry_count,
                operation_generation,
            ));
        }
        Err(error) => {
            recover_remote(app_handle, operation_generation, retry_count, error).await;
        }
    }
}

const MAX_RECONNECT_RETRIES: u32 = 3;

/// Spawn a protected local wenlan-mcp listener on a given port: the search-only
/// one, or the web-write one beside it. Reusable for both initial start and
/// MCP-only restarts.
async fn spawn_mcp(
    app_handle: &tauri::AppHandle,
    kind: McpKind,
    port: u16,
    generation: u64,
    profile: &Profile,
) -> Result<
    (
        tokio::sync::mpsc::Receiver<tauri_plugin_shell::process::CommandEvent>,
        u32,
    ),
    String,
> {
    log::warn!(
        "[remote-access] spawning {} serve ({}) on port {}",
        MCP_SIDECAR_NAME,
        kind.tool_profile(),
        port
    );
    let origin_url = crate::api::WenlanClient::new().base_url().to_string();
    let (mcp_rx, child_pid) = {
        let remote = remote_access_mutex(app_handle).await;
        let mut ra = remote.lock().await;
        let validation = match kind {
            McpKind::Query => validate_mcp_spawn(&ra, generation),
            McpKind::Write => validate_write_spawn(&ra, generation),
        };
        let RemoteAccessState {
            mcp_child,
            port: query_port,
            write_child,
            write_port,
            ..
        } = &mut *ra;
        let (mcp_slot, port_slot) = match kind {
            McpKind::Query => (mcp_child, query_port),
            McpKind::Write => (write_child, write_port),
        };
        spawn_and_store_child(
            validation,
            mcp_slot,
            port_slot,
            port,
            || {
                let args = match kind {
                    McpKind::Query => relay_runtime::mcp_args(&origin_url, port, profile),
                    McpKind::Write => relay_runtime::write_mcp_args(&origin_url, port, profile),
                };
                let (mcp_rx, mcp_child) = app_handle
                    .shell()
                    .sidecar(MCP_SIDECAR_NAME)
                    .map_err(|e| format!("{} sidecar not found: {}", MCP_SIDECAR_NAME, e))?
                    .args(args)
                    .env(relay_runtime::TOKEN_ENV, profile.backend_token())
                    .envs(relay_runtime::mcp_scope_env(profile))
                    .env("WENLAN_NO_AUTOSTART", "1")
                    .spawn()
                    .map_err(|e| format!("Failed to spawn {} serve: {}", MCP_SIDECAR_NAME, e))?;
                let child_pid = mcp_child.pid();
                Ok(((mcp_rx, child_pid), mcp_child))
            },
            |_| Ok(()),
        )?
    };
    let readiness = timeout(Duration::from_secs(5), async {
        loop {
            if !remote_generation_is_current(app_handle, generation).await {
                return Err("Remote access start cancelled.".to_string());
            }
            if relay_runtime::verify_backend_as(kind, port, profile)
                .await
                .is_ok()
            {
                match listener_pid_for_port(port) {
                    Some(listener_pid) if listener_pid == child_pid => return Ok(()),
                    Some(listener_pid) => {
                        return Err(format!(
                            "remote access port {port} is owned by PID {listener_pid}, not spawned {} PID {child_pid}",
                            MCP_SIDECAR_NAME
                        ));
                    }
                    None => {}
                }
            }
            sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        Err(format!(
            "{} serve failed to start (health check timeout).",
            MCP_SIDECAR_NAME
        ))
    });

    readiness.map(|()| (mcp_rx, child_pid))
}

fn can_adopt_reverse(
    state: &RemoteAccessState,
    generation: u64,
    port: u16,
    expected_connection: Option<&str>,
) -> bool {
    generation_is_current(state.generation, generation)
        && state.port == Some(port)
        && state.mcp_child.is_some()
        && state.pending_stops.is_empty()
        && !state.orphan_cleanup_failed
        && match expected_connection {
            None => matches!(state.status, RemoteAccessStatus::Starting) && state.reverse.is_none(),
            Some(id) => {
                matches!(state.status, RemoteAccessStatus::Connected { .. })
                    && state
                        .reverse
                        .as_ref()
                        .is_some_and(|active| active.connection.connection_id() == id)
            }
        }
}

fn recovery_delay(error: &RenewalError, attempts: u32) -> Option<Duration> {
    if attempts >= MAX_RECONNECT_RETRIES {
        return None;
    }
    let backoff = Duration::from_secs(30u64 << attempts.min(2));
    match error {
        RenewalError::Relay(RelayError::Unavailable) => Some(backoff),
        RenewalError::Relay(RelayError::RateLimited {
            retry_after_seconds,
        }) => Some(backoff.max(Duration::from_secs(
            retry_after_seconds.unwrap_or(900).min(3600),
        ))),
        _ => None,
    }
}

async fn recover_remote(
    app: tauri::AppHandle,
    generation: u64,
    attempts: u32,
    error: RenewalError,
) {
    use tauri::Emitter;
    let delay = recovery_delay(&error, attempts);
    let Some(next_generation) = transition_off(&app, Some(generation)).await else {
        return;
    };
    match delay {
        Some(delay) => log::warn!(
            "[remote-access] reconnecting in {}s (attempt {}) after: {error}",
            delay.as_secs(),
            attempts + 1
        ),
        None => log::warn!("[remote-access] stopping after: {error}"),
    }
    let cleanup_ok = {
        let remote = remote_access_mutex(&app).await;
        let mut ra = remote.lock().await;
        if !generation_is_current(ra.generation, next_generation) {
            return;
        }
        let cleanup = disconnect_result_for(&ra, Ok(()));
        let status = RemoteAccessStatus::Error {
            error: cleanup
                .as_ref()
                .err()
                .cloned()
                .unwrap_or_else(|| error.to_string()),
        };
        ra.status = status.clone();
        let _ = app.emit("remote-access-status", &status);
        cleanup.is_ok()
    };
    let retrying = cleanup_ok && delay.is_some();
    if !retrying {
        let expires_at = relay_runtime::storage(|store| {
            Ok(store
                .load()?
                .and_then(|profile| profile.device().map(|device| device.expires_at)))
        })
        .await
        .ok()
        .flatten();
        if let Some(notice) =
            give_up_notice(&error, retrying, expires_at, crate::remote_relay::now_ms())
        {
            publish_notice(&app, notice).await;
        }
    }
    if let (true, Some(delay)) = (cleanup_ok, delay) {
        tokio::select! {
            _ = sleep(delay) => {
                toggle_on_with_retries(app.clone(), attempts + 1, next_generation).await;
            }
            _ = wait_for_generation_change(&app, next_generation) => {}
        }
    }
}

/// One owner monitors local process, reverse transport and authenticated status.
/// Reconnection reuses the saved device; authorization failures never re-enroll.
async fn monitor_reverse(
    app: tauri::AppHandle,
    mut mcp_rx: tokio::sync::mpsc::Receiver<tauri_plugin_shell::process::CommandEvent>,
    mut attempts: u32,
    generation: u64,
) {
    let mut renewal = crate::remote_relay::renewal::RenewalSchedule::new(
        std::time::SystemTime::now(),
        std::time::Instant::now(),
    );
    let stable_since = std::time::Instant::now();
    let mut health_tick = 0u8;
    let mut failures = 0u8;
    let failure = loop {
        tokio::select! {
            reason = wait_for_exit(&mut mcp_rx) => {
                log::warn!("[remote-access] {MCP_SIDECAR_NAME} exited: {reason}");
                break RenewalError::Relay(RelayError::Unavailable);
            }
            _ = wait_for_generation_change(&app, generation) => return,
            _ = sleep(Duration::from_secs(5)) => {}
        }
        let (profile, id, port, write_route, finished) = {
            let remote = remote_access_mutex(&app).await;
            let ra = remote.lock().await;
            if !generation_is_current(ra.generation, generation)
                || !matches!(ra.status, RemoteAccessStatus::Connected { .. })
            {
                return;
            }
            let (Some(active), Some(port)) = (ra.reverse.as_ref(), ra.port) else {
                log::warn!("[remote-access] relay connection state missing");
                break RenewalError::Relay(RelayError::Unavailable);
            };
            (
                active.profile.clone(),
                active.connection.connection_id().to_owned(),
                port,
                ra.write_route.clone(),
                active.connection.is_finished(),
            )
        };
        if stable_since.elapsed() >= Duration::from_secs(300) {
            attempts = 0;
        }
        if finished {
            // The transport logs why it ended.
            log::warn!("[remote-access] relay connection finished");
            break RenewalError::Relay(RelayError::Unavailable);
        }
        health_tick += 1;
        if health_tick < 6 {
            continue;
        }
        health_tick = 0;
        if renewal.due(std::time::SystemTime::now(), std::time::Instant::now()) {
            // Opening a verified same-device replacement renews its route lease.
            // This cannot enroll because the active profile has a device.
            let refreshed = reverse_runtime::connect(profile.clone(), port, write_route).await;
            match refreshed {
                Ok(active) => {
                    let previous = {
                        let remote = remote_access_mutex(&app).await;
                        let mut ra = remote.lock().await;
                        if !can_adopt_reverse(&ra, generation, port, Some(&id)) {
                            drop(ra);
                            let _ = active.connection.shutdown().await;
                            return;
                        }
                        ra.reverse.replace(active)
                    };
                    if let Some(previous) = previous {
                        let _ = previous.connection.shutdown().await;
                    }
                    renewal.succeeded(std::time::SystemTime::now(), std::time::Instant::now());
                    failures = 0;
                    continue;
                }
                Err(
                    error @ RenewalError::Relay(
                        RelayError::Unavailable | RelayError::RateLimited { .. },
                    ),
                ) => {
                    log::warn!("[remote-access] route renewal deferred: {error}");
                    let retry_after = match error {
                        RenewalError::Relay(RelayError::RateLimited {
                            retry_after_seconds,
                        }) => retry_after_seconds.map(Duration::from_secs),
                        _ => None,
                    };
                    renewal.failed(
                        std::time::SystemTime::now(),
                        std::time::Instant::now(),
                        retry_after,
                    );
                }
                Err(error) => {
                    log::warn!("[remote-access] route renewal failed: {error}");
                    break error;
                }
            }
        }
        let health = match reverse_runtime::check(&profile, &id).await {
            Ok(()) => relay_runtime::verify_backend(port, &profile)
                .await
                .map_err(|error| ("local server check", RenewalError::Relay(error))),
            Err(error) => Err(("relay status check", error)),
        };
        match health {
            Ok(()) => failures = 0,
            Err((
                check,
                error @ RenewalError::Relay(
                    RelayError::Unavailable | RelayError::RateLimited { .. },
                ),
            )) => {
                failures += 1;
                log::warn!("[remote-access] {check} failed ({failures}/3): {error}");
                if failures >= 3 {
                    break error;
                }
            }
            Err((check, error)) => {
                log::warn!("[remote-access] {check} failed: {error}");
                break error;
            }
        }
        // Start the write server for an app allowed since the last check, or
        // bring it back if it stopped. A task of its own, so a slow start never
        // delays noticing that the search server or the relay connection ended;
        // started after the checks so its settings read does not race theirs.
        tauri::async_runtime::spawn(reconcile_write_process(app.clone(), generation));
    };
    recover_remote(app, generation, attempts, failure).await;
}

async fn wait_for_exit(
    rx: &mut tokio::sync::mpsc::Receiver<tauri_plugin_shell::process::CommandEvent>,
) -> String {
    while let Some(event) = rx.recv().await {
        match event {
            tauri_plugin_shell::process::CommandEvent::Terminated(payload) => {
                return format!(
                    "terminated (code: {:?}, signal: {:?})",
                    payload.code, payload.signal
                );
            }
            tauri_plugin_shell::process::CommandEvent::Error(_) => return "process error".into(),
            _ => {}
        }
    }
    "channel closed".into()
}

async fn start_reverse(
    app: &tauri::AppHandle,
    generation: u64,
    profile: Profile,
) -> Result<
    (
        u16,
        tokio::sync::mpsc::Receiver<tauri_plugin_shell::process::CommandEvent>,
        ActiveReverse,
    ),
    RenewalError,
> {
    let remote = remote_access_mutex(app).await;
    preflight_cleanup(&remote, generation, async {
        tokio::task::spawn_blocking(cleanup_remote_orphans)
            .await
            .map_err(|_| SHUTDOWN_UNCONFIRMED.to_string())?
    })
    .await
    .map_err(RenewalError::Profile)?;
    if !remote_generation_is_current(app, generation).await {
        return Err(RenewalError::Profile(
            "Remote access start cancelled.".into(),
        ));
    }
    let port = find_available_port()
        .ok_or_else(|| RenewalError::Profile("All remote access ports are in use.".into()))?;
    let (mcp_rx, _) = spawn_mcp(app, McpKind::Query, port, generation, &profile)
        .await
        .map_err(RenewalError::Profile)?;
    // An app allowed to write before this start (an app restart, a reconnect)
    // still needs its write server. Search does not wait on it: if it cannot
    // start now, the monitor keeps trying and writes are refused meanwhile.
    if profile.write_enabled() {
        if let Err(error) = start_write_process(app, generation, &profile).await {
            log::warn!("[remote-access] web-write server not started: {error}");
        }
    }
    let write_route = {
        let ra = remote.lock().await;
        ra.write_route.clone()
    };
    // Once enrollment begins, retain its future through durable credential
    // storage. User cancellation still stops the owned local process promptly.
    let connection = reverse_runtime::connect(profile, port, write_route);
    tokio::pin!(connection);
    let active = tokio::select! {
        result = &mut connection => result?,
        _ = wait_for_generation_change(app, generation) => {
            let _ = transition_off(app, Some(generation)).await;
            if let Ok(active) = connection.await {
                let _ = active.connection.shutdown().await;
            }
            return Err(RenewalError::Profile("Remote access start cancelled.".into()));
        }
    };
    Ok((port, mcp_rx, active))
}

/// Start the web-write server on its own port and open the route to it.
/// Only the search-only server and the connection are needed first; the route
/// opens once this server answers its protected probe as the write profile.
async fn start_write_process(
    app: &tauri::AppHandle,
    generation: u64,
    profile: &Profile,
) -> Result<(), String> {
    let port =
        find_available_port().ok_or_else(|| "All remote access ports are in use.".to_string())?;
    match spawn_mcp(app, McpKind::Write, port, generation, profile).await {
        Ok((mut events, pid)) => {
            let remote = remote_access_mutex(app).await;
            let ra = remote.lock().await;
            let ours = generation_is_current(ra.generation, generation)
                && ra
                    .write_child
                    .as_ref()
                    .is_some_and(|child| child.pid() == pid);
            if !ours {
                // A stop took this process, or a newer start owns the slot.
                return Err("Remote access changed while the web-write server started.".into());
            }
            ra.write_route.set(Some(port));
            drop(ra);
            // Something must read its events or the process can stall; the
            // same task notices it ending and takes the route down.
            let watcher = app.clone();
            tauri::async_runtime::spawn(async move {
                let reason = wait_for_exit(&mut events).await;
                log::warn!("[remote-access] {MCP_SIDECAR_NAME} web-write exited: {reason}");
                let remote = remote_access_mutex(&watcher).await;
                let mut ra = remote.lock().await;
                if generation_is_current(ra.generation, generation)
                    && ra
                        .write_child
                        .as_ref()
                        .is_some_and(|child| child.pid() == pid)
                {
                    stop_write_process(&mut ra).await;
                }
            });
            Ok(())
        }
        Err(error) => {
            let remote = remote_access_mutex(app).await;
            let mut ra = remote.lock().await;
            // Only this start's own process can be in the slot, and only while
            // its generation is still current.
            if generation_is_current(ra.generation, generation) {
                stop_write_process(&mut ra).await;
            }
            Err(error)
        }
    }
}

/// Take the route down and stop the write server alone, leaving search and the
/// relay connection running.
async fn stop_write_process(ra: &mut RemoteAccessState) {
    ra.write_route.set(None);
    ra.write_port = None;
    let children: Vec<_> = ra.write_child.take().into_iter().collect();
    request_stops_into_state(ra, children).await;
    ra.pending_stops =
        crate::remote_relay::shutdown::confirm_exits(std::mem::take(&mut ra.pending_stops)).await;
}

/// Make the write server match the saved profile: running once an app was
/// allowed to write, absent otherwise. Idempotent, and safe to call from the
/// approval command and from the monitor at the same time.
pub(crate) async fn reconcile_write_process(app: tauri::AppHandle, generation: u64) {
    let profile = match relay_runtime::enabled_profile().await {
        Ok(profile) => profile,
        Err(_) => return,
    };
    let remote = remote_access_mutex(&app).await;
    {
        let mut ra = remote.lock().await;
        if !generation_is_current(ra.generation, generation)
            || !matches!(ra.status, RemoteAccessStatus::Connected { .. })
            || ra.write_starting
        {
            return;
        }
        if !profile.write_enabled() {
            if ra.write_child.is_some() {
                stop_write_process(&mut ra).await;
            }
            return;
        }
        if ra.write_child.is_some() {
            return;
        }
        ra.write_starting = true;
    }
    let result = start_write_process(&app, generation, &profile).await;
    remote.lock().await.write_starting = false;
    if let Err(error) = result {
        log::warn!("[remote-access] web-write server not started: {error}");
    }
}

/// After an app is allowed to write: start the write server now, so the app can
/// write without Web access being turned off and on. Does nothing unless
/// connected; the monitor and the next start pick it up from the saved profile.
pub(crate) async fn ensure_write_process(app: &tauri::AppHandle) {
    let remote = remote_access_mutex(app).await;
    let generation = {
        let ra = remote.lock().await;
        if !matches!(ra.status, RemoteAccessStatus::Connected { .. }) {
            return;
        }
        ra.generation
    };
    reconcile_write_process(app.clone(), generation).await;
}

async fn preflight_cleanup(
    state: &tokio::sync::Mutex<RemoteAccessState>,
    generation: u64,
    cleanup: impl std::future::Future<Output = Result<(), String>>,
) -> Result<(), String> {
    let mut ra = state.lock().await;
    validate_mcp_spawn(&ra, generation)?;
    if !matches!(ra.status, RemoteAccessStatus::Starting) || ra.reverse.is_some() {
        return Err("Remote access start is no longer active.".into());
    }
    // A preceding scan must finish before stop or a new generation can spawn.
    // Only the dedicated controller mutex is held, never AppState's RwLock.
    let result = cleanup.await;
    ra.orphan_cleanup_failed = result.is_err();
    result
}

async fn transition_off<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    expected_generation: Option<u64>,
) -> Option<u64> {
    use tauri::Emitter;

    let state = app_handle.state::<std::sync::Arc<tokio::sync::RwLock<crate::state::AppState>>>();
    let app_state = { state.read().await.remote_access.clone() };
    stop_with_cleanup(
        &app_state,
        expected_generation,
        |status| {
            let _ = app_handle.emit("remote-access-status", status);
        },
        async {
            tokio::task::spawn_blocking(cleanup_remote_orphans)
                .await
                .map_err(|_| SHUTDOWN_UNCONFIRMED.to_string())?
        },
    )
    .await
}

async fn request_stops_into_state(
    ra: &mut RemoteAccessState,
    children: Vec<tauri_plugin_shell::process::CommandChild>,
) {
    if !children.is_empty() {
        use crate::remote_relay::shutdown::{request_stops, PendingProcess};
        let fallback: Vec<_> = children
            .iter()
            .map(|child| PendingProcess::unmeasured(child.pid()))
            .collect();
        let stopped = match timeout(
            Duration::from_secs(2),
            tokio::task::spawn_blocking(move || request_stops(children, Vec::new())),
        )
        .await
        {
            Ok(Ok(stopped)) => stopped,
            _ => fallback,
        };
        ra.pending_stops.extend(stopped);
    }
}

async fn stop_with_cleanup(
    state: &tokio::sync::Mutex<RemoteAccessState>,
    expected_generation: Option<u64>,
    emit_status: impl FnOnce(&RemoteAccessStatus),
    cleanup: impl std::future::Future<Output = Result<(), String>>,
) -> Option<u64> {
    let mut ra = state.lock().await;
    if expected_generation.is_some_and(|expected| !generation_is_current(ra.generation, expected)) {
        return None;
    }

    let reverse = ra.reverse.take();
    // Requests for a write grant stop reaching the write server before it is
    // told to stop, and every grant ends with this generation.
    ra.write_route.set(None);
    let children: Vec<_> = [ra.mcp_child.take(), ra.write_child.take()]
        .into_iter()
        .flatten()
        .collect();
    let generation = {
        let RemoteAccessState {
            status, generation, ..
        } = &mut *ra;
        mark_off(status, generation)
    };
    if let Some(reverse) = reverse {
        // shutdown always waits for terminal ownership, including an already
        // failed connection; its result is not a server revocation receipt.
        let _ = reverse.connection.shutdown().await;
    }
    request_stops_into_state(&mut ra, children).await;
    // Retain the dedicated controller mutex until old-process cleanup finishes,
    // or a new start can reuse a port and be killed by the preceding stop.
    // No AppState RwLock guard is held; blocking process work runs in its pool.
    ra.orphan_cleanup_failed = cleanup.await.is_err();
    ra.pending_stops =
        crate::remote_relay::shutdown::confirm_exits(std::mem::take(&mut ra.pending_stops)).await;
    if ra.pending_stops.is_empty() && !ra.orphan_cleanup_failed {
        ra.port = None;
        ra.write_port = None;
        ra.status = RemoteAccessStatus::Off;
    } else {
        ra.status = RemoteAccessStatus::Error {
            error: SHUTDOWN_UNCONFIRMED.into(),
        };
    }
    emit_status(&ra.status);
    Some(generation)
}

/// Invalidate stale tasks, stop the reverse socket, and confirm local process exit.
pub async fn toggle_off(app_handle: &tauri::AppHandle) -> Result<(), String> {
    let plan = relay_runtime::prepare_disconnect(None).await;
    let off_generation = transition_off(app_handle, None).await;
    let result = match plan {
        Ok(plan) => relay_runtime::finish_prepared_disconnect(plan).await,
        Err(error) => Err(error),
    };
    if let Some(off_generation) = off_generation {
        return publish_disconnect_result(app_handle, off_generation, result).await;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reconnect_profile(enabled: bool, device: bool) -> Profile {
        serde_json::from_value(serde_json::json!({
            "version": 1,
            "revision": "r".repeat(64),
            "relay_origin": RELAY_ORIGIN,
            "space": "reconnect-test",
            "backend_token": "b".repeat(64),
            "enabled": enabled,
            "device": device.then(|| crate::remote_relay::DeviceCredential {
                id: "d".repeat(64),
                management_token: "m".repeat(64),
                expires_at: 1,
            }),
        }))
        .unwrap()
    }

    /// Serves `/api/spaces` with one Space named `work` and counts requests.
    async fn serve_one_space() -> (
        crate::api::WenlanClient,
        std::sync::Arc<std::sync::atomic::AtomicUsize>,
    ) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = hits.clone();
        let body = serde_json::json!([{
            "id": "s1", "name": "work", "description": null, "suggested": false,
            "starred": false, "sort_order": 0, "memory_count": 0, "entity_count": 0,
            "created_at": 0.0, "updated_at": 0.0,
        }])
        .to_string();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let mut buf = [0_u8; 2048];
                let _ = stream.read(&mut buf).await;
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes()).await;
            }
        });
        (
            crate::api::WenlanClient::with_base_url(format!("http://{addr}")),
            hits,
        )
    }

    #[tokio::test]
    async fn saved_scope_checks_one_space_and_never_asks_about_the_whole_library() {
        let (client, hits) = serve_one_space().await;
        assert_eq!(confirm_saved_scope(&client, "*").await, Ok(()));
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(confirm_saved_scope(&client, "work").await, Ok(()));
        assert_eq!(
            confirm_saved_scope(&client, "gone").await,
            Err("The selected Space no longer exists".to_string())
        );
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[test]
    fn reconnect_rejects_stale_disabled_and_pending_profiles_without_mutation() {
        let profile = reconnect_profile(true, true);
        let before = serde_json::to_vec(&profile).unwrap();
        assert!(validate_reconnect_profile(Some(profile.clone()), "stale").is_err());
        let accepted =
            validate_reconnect_profile(Some(profile.clone()), profile.revision()).unwrap();
        assert_eq!(serde_json::to_vec(&accepted).unwrap(), before);
        // Expired credentials are preserved for the existing reverse runtime
        // to reject; reconnect must not clear them and silently re-enroll.
        assert_eq!(accepted.device().unwrap().expires_at, 1);
        for device in [false, true] {
            let disabled = reconnect_profile(false, device);
            assert!(
                validate_reconnect_profile(Some(disabled.clone()), disabled.revision()).is_err()
            );
        }
        assert!(validate_reconnect_profile(None, profile.revision()).is_err());
    }

    #[tokio::test]
    async fn reconnect_failed_cleanup_and_stale_generation_block_restart() {
        let state = tokio::sync::Mutex::new(RemoteAccessState::default());
        let generation = stop_with_cleanup(&state, Some(0), |_| {}, async {
            Err("unconfirmed cleanup".into())
        })
        .await
        .unwrap();
        assert_eq!(
            confirm_reconnect_cleanup(&state, generation)
                .await
                .unwrap_err(),
            SHUTDOWN_UNCONFIRMED
        );
        let mut ra = state.lock().await;
        let pending = ra.orphan_cleanup_failed;
        let RemoteAccessState {
            status,
            generation: current,
            ..
        } = &mut *ra;
        assert!(try_begin_start(status, current, Some(generation), pending).is_none());
        drop(ra);
        stop_with_cleanup(&state, None, |_| {}, async { Ok(()) }).await;
        assert!(confirm_reconnect_cleanup(&state, generation).await.is_err());
        assert!(
            stop_with_cleanup(&state, Some(generation), |_| panic!("stale event"), async {
                panic!("stale cleanup")
            })
            .await
            .is_none()
        );
    }

    #[tokio::test]
    async fn reconnect_concurrent_stop_cancels_restart_after_serialized_cleanup() {
        let state = std::sync::Arc::new(tokio::sync::Mutex::new(RemoteAccessState::default()));
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let reconnect_state = state.clone();
        let reconnect = tokio::spawn(async move {
            stop_with_cleanup(&reconnect_state, Some(0), |_| {}, async {
                entered_tx.send(()).unwrap();
                release_rx.await.unwrap();
                Ok(())
            })
            .await
            .unwrap()
        });
        entered_rx.await.unwrap();
        let stop_state = state.clone();
        let stop = tokio::spawn(async move {
            stop_with_cleanup(&stop_state, None, |_| {}, async { Ok(()) })
                .await
                .unwrap()
        });
        release_tx.send(()).unwrap();
        let reconnect_generation = reconnect.await.unwrap();
        assert_eq!(stop.await.unwrap(), reconnect_generation + 1);
        assert!(confirm_reconnect_cleanup(&state, reconnect_generation)
            .await
            .is_err());
        let mut ra = state.lock().await;
        let RemoteAccessState {
            status, generation, ..
        } = &mut *ra;
        assert!(try_begin_start(status, generation, Some(reconnect_generation), false).is_none());
        assert!(matches!(ra.status, RemoteAccessStatus::Off));
        assert!(!can_adopt_reverse(
            &ra,
            reconnect_generation,
            PORT_RANGE_START,
            None
        ));
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn reconnect_transport_preserves_persisted_device_and_profile_revision() {
        let tmp = tempfile::tempdir().unwrap();
        let _home = HomeGuard::set(tmp.path());
        let store = crate::remote_relay::store::Store::current();
        let configured = store.configure(None, "reconnect-test").unwrap();
        let enabled = store.enable(configured.revision()).unwrap();
        let profile = store
            .attach_device(
                enabled.revision(),
                crate::remote_relay::DeviceCredential {
                    id: "d".repeat(64),
                    management_token: "m".repeat(64),
                    expires_at: crate::remote_relay::now_ms() + 60_000,
                },
            )
            .unwrap();
        let before = serde_json::to_vec(&profile).unwrap();
        validate_reconnect_profile(store.load().unwrap(), profile.revision()).unwrap();
        let state = tokio::sync::Mutex::new(RemoteAccessState::default());
        let generation = stop_with_cleanup(&state, Some(0), |_| {}, async { Ok(()) })
            .await
            .unwrap();
        confirm_reconnect_cleanup(&state, generation).await.unwrap();
        validate_reconnect_profile(store.load().unwrap(), profile.revision()).unwrap();
        let mut ra = state.lock().await;
        let RemoteAccessState {
            status,
            generation: current,
            ..
        } = &mut *ra;
        assert_eq!(
            try_begin_start(status, current, Some(generation), false),
            Some(generation)
        );
        assert_eq!(
            serde_json::to_vec(&store.load().unwrap().unwrap()).unwrap(),
            before
        );
        let disabled = store.disable(profile.revision()).unwrap();
        assert!(disabled.view().disconnect_pending);
        assert!(validate_reconnect_profile(Some(disabled), profile.revision()).is_err());
    }

    #[test]
    fn reverse_recovery_is_bounded_and_does_not_retry_authorization_failures() {
        let unavailable = RenewalError::Relay(RelayError::Unavailable);
        assert_eq!(
            recovery_delay(&unavailable, 0),
            Some(Duration::from_secs(30))
        );
        assert_eq!(
            recovery_delay(&unavailable, 1),
            Some(Duration::from_secs(60))
        );
        assert_eq!(
            recovery_delay(&unavailable, 2),
            Some(Duration::from_secs(120))
        );
        assert_eq!(recovery_delay(&unavailable, 3), None);
        assert_eq!(
            recovery_delay(&RenewalError::Relay(RelayError::Unauthorized), 0),
            None
        );
        assert_eq!(
            recovery_delay(&RenewalError::Profile("stale".into()), 0),
            None
        );
        assert_eq!(
            recovery_delay(
                &RenewalError::Relay(RelayError::RateLimited {
                    retry_after_seconds: Some(600),
                }),
                0
            ),
            Some(Duration::from_secs(600))
        );
        assert_eq!(
            recovery_delay(
                &RenewalError::Relay(RelayError::RateLimited {
                    retry_after_seconds: None,
                }),
                0
            ),
            Some(Duration::from_secs(900))
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reverse_adoption_requires_current_generation_and_owned_mcp() {
        let app = shell_test_app();
        let mut state = RemoteAccessState {
            status: RemoteAccessStatus::Starting,
            generation: 7,
            port: Some(PORT_RANGE_START),
            ..Default::default()
        };
        assert!(!can_adopt_reverse(&state, 7, PORT_RANGE_START, None));
        let (_rx, child) = app.shell().command("/bin/sleep").arg("5").spawn().unwrap();
        state.mcp_child = Some(child);
        let current = can_adopt_reverse(&state, 7, PORT_RANGE_START, None);
        let stale = can_adopt_reverse(&state, 6, PORT_RANGE_START, None);
        let wrong_port = can_adopt_reverse(&state, 7, PORT_RANGE_START + 1, None);
        let no_previous = can_adopt_reverse(&state, 7, PORT_RANGE_START, Some("old"));
        state.orphan_cleanup_failed = true;
        let unconfirmed = can_adopt_reverse(&state, 7, PORT_RANGE_START, None);
        state.orphan_cleanup_failed = false;
        let state = tokio::sync::Mutex::new(state);
        stop_with_cleanup(&state, Some(7), |_| {}, async { Ok(()) }).await;
        assert!(current);
        assert!(!stale && !wrong_port && !no_previous && !unconfirmed);
        assert!(state.lock().await.pending_stops.is_empty());
    }
    use std::ffi::OsString;

    #[cfg(unix)]
    fn shell_test_app() -> tauri::App<tauri::test::MockRuntime> {
        tauri::test::mock_builder()
            .plugin(tauri_plugin_shell::init())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build a windowless shell test runtime")
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn real_command_child_survives_post_spawn_error_until_coordinated_stop() {
        let app = shell_test_app();
        let mut state = RemoteAccessState {
            status: RemoteAccessStatus::Starting,
            generation: 7,
            ..Default::default()
        };
        let validation = validate_mcp_spawn(&state, 7);
        let mut observed_pid = None;
        let spawned = spawn_and_store_child(
            validation,
            &mut state.mcp_child,
            &mut state.port,
            PORT_RANGE_START,
            || {
                app.shell()
                    .command("/bin/sleep")
                    .arg("5")
                    .spawn()
                    .map_err(|_| "owned fixture spawn failed".to_string())
            },
            |child| {
                observed_pid = Some(child.pid());
                Err("injected receipt failure".into())
            },
        );
        let stored_pid = state.mcp_child.as_ref().map(|child| child.pid());
        let before = stored_pid.map(measured_process_identity);
        let state = tokio::sync::Mutex::new(state);
        // Stop before asserting, even when the probe or injected path failed.
        let generation = stop_with_cleanup(&state, Some(7), |_| {}, async { Ok(()) }).await;
        assert_eq!(spawned.unwrap_err(), "injected receipt failure");
        assert_eq!(stored_pid, observed_pid);
        assert!(matches!(
            before,
            Some(crate::remote_relay::orphan::Observation::Identity(_))
        ));
        assert_eq!(generation, Some(8));
        let stopped = state.lock().await;
        assert!(stopped.mcp_child.is_none());
        assert!(stopped.pending_stops.is_empty());
        assert!(matches!(stopped.status, RemoteAccessStatus::Off));
        assert_eq!(
            measured_process_identity(stored_pid.unwrap()),
            crate::remote_relay::orphan::Observation::Gone
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    #[serial_test::serial]
    async fn exit_shutdown_stops_transport_without_disabling_saved_profile() {
        let tmp = tempfile::tempdir().unwrap();
        let _home = HomeGuard::set(tmp.path());
        let store = crate::remote_relay::store::Store::current();
        let configured = store.configure(None, "exit-test").unwrap();
        let enabled = store.enable(configured.revision()).unwrap();
        let enabled = store
            .attach_device(
                enabled.revision(),
                crate::remote_relay::DeviceCredential {
                    id: "d".repeat(64),
                    management_token: "m".repeat(64),
                    expires_at: crate::remote_relay::now_ms() + 60 * 60 * 1000,
                },
            )
            .unwrap();
        let expected_profile = serde_json::to_vec(&enabled).unwrap();

        let app = shell_test_app();
        let (_events, child) = app.shell().command("/bin/sleep").arg("5").spawn().unwrap();
        let child_pid = child.pid();
        let app_state = std::sync::Arc::new(tokio::sync::RwLock::new(crate::state::AppState {
            remote_access: std::sync::Arc::new(tokio::sync::Mutex::new(RemoteAccessState {
                status: RemoteAccessStatus::Connected {
                    tunnel_url: None,
                    relay_url: Some(format!("{RELAY_ORIGIN}/mcp")),
                },
                mcp_child: Some(child),
                ..Default::default()
            })),
            ..crate::state::AppState::new()
        }));
        app.manage(app_state.clone());

        shutdown_for_exit(app.handle()).await.unwrap();

        let after = store.load().unwrap().unwrap();
        assert_eq!(serde_json::to_vec(&after).unwrap(), expected_profile);
        assert_eq!(
            measured_process_identity(child_pid),
            crate::remote_relay::orphan::Observation::Gone
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stale_stop_cannot_terminate_a_real_new_generation_child() {
        let app = shell_test_app();
        let mut current = RemoteAccessState {
            status: RemoteAccessStatus::Starting,
            generation: 8,
            ..Default::default()
        };
        let validation = validate_mcp_spawn(&current, 8);
        let (mut events, pid) = spawn_and_store_child(
            validation,
            &mut current.mcp_child,
            &mut current.port,
            PORT_RANGE_START,
            || {
                let (events, child) = app
                    .shell()
                    .command("/bin/sleep")
                    .arg("5")
                    .spawn()
                    .map_err(|_| "owned fixture spawn failed".to_string())?;
                Ok(((events, child.pid()), child))
            },
            |_| Ok(()),
        )
        .unwrap();
        let state = tokio::sync::Mutex::new(current);
        let mut stale_cleanup_ran = false;
        let stale = stop_with_cleanup(&state, Some(7), |_| {}, async {
            stale_cleanup_ran = true;
            Ok(())
        })
        .await;
        let retained_pid = state
            .lock()
            .await
            .mcp_child
            .as_ref()
            .map(|child| child.pid());
        let after_stale = measured_process_identity(pid);
        let stopped = stop_with_cleanup(&state, Some(8), |_| {}, async { Ok(()) }).await;
        let exit = timeout(Duration::from_secs(2), wait_for_exit(&mut events)).await;
        assert_eq!(stale, None);
        assert!(!stale_cleanup_ran);
        assert_eq!(retained_pid, Some(pid));
        assert!(matches!(
            after_stale,
            crate::remote_relay::orphan::Observation::Identity(_)
        ));
        assert_eq!(stopped, Some(9));
        assert!(state.lock().await.pending_stops.is_empty());
        assert!(exit.unwrap().starts_with("terminated ("));
        assert_eq!(
            measured_process_identity(pid),
            crate::remote_relay::orphan::Observation::Gone
        );
    }

    #[tokio::test]
    async fn preflight_scan_serializes_stop_and_rejects_stale_cleanup() {
        let state = std::sync::Arc::new(tokio::sync::Mutex::new(RemoteAccessState {
            status: RemoteAccessStatus::Starting,
            generation: 7,
            ..Default::default()
        }));
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let scanning = state.clone();
        let scan = tokio::spawn(async move {
            preflight_cleanup(&scanning, 7, async {
                entered_tx.send(()).unwrap();
                release_rx.await.unwrap();
                Ok(())
            })
            .await
        });
        entered_rx.await.unwrap();
        assert!(state.try_lock().is_err());
        let stopping = state.clone();
        let stop = tokio::spawn(async move {
            stop_with_cleanup(&stopping, Some(7), |_| {}, async { Ok(()) }).await
        });
        release_tx.send(()).unwrap();
        scan.await.unwrap().unwrap();
        assert_eq!(stop.await.unwrap(), Some(8));
        assert!(matches!(state.lock().await.status, RemoteAccessStatus::Off));
        let mut invoked = false;
        assert!(preflight_cleanup(&state, 7, async {
            invoked = true;
            Ok(())
        })
        .await
        .is_err());
        assert!(!invoked);
    }

    #[tokio::test]
    async fn preflight_failure_is_retained_until_coordinated_cleanup() {
        let state = tokio::sync::Mutex::new(RemoteAccessState {
            status: RemoteAccessStatus::Starting,
            generation: 3,
            ..Default::default()
        });
        assert!(
            preflight_cleanup(&state, 3, async { Err("scan failed".into()) })
                .await
                .is_err()
        );
        {
            let current = state.lock().await;
            assert!(current.orphan_cleanup_failed);
            assert!(validate_mcp_spawn(&current, 3).is_err());
        }
        assert_eq!(
            stop_with_cleanup(&state, Some(3), |_| {}, async { Ok(()) }).await,
            Some(4)
        );
        assert!(!state.lock().await.orphan_cleanup_failed);
    }

    #[test]
    fn pending_shutdown_prevents_the_actual_spawn_helper_from_running() {
        let state = RemoteAccessState {
            status: RemoteAccessStatus::Starting,
            generation: 1,
            pending_stops: vec![crate::remote_relay::shutdown::PendingProcess::unmeasured(
                42,
            )],
            ..Default::default()
        };
        let mut slot: Option<()> = None;
        let mut port = None;
        let result: Result<(), String> = spawn_and_store_child(
            validate_mcp_spawn(&state, 1),
            &mut slot,
            &mut port,
            PORT_RANGE_START,
            || panic!("pending stop must prevent spawning"),
            |_| panic!("post-spawn must not run"),
        );
        assert_eq!(result.unwrap_err(), SHUTDOWN_UNCONFIRMED);
        assert!(slot.is_none());
        assert!(port.is_none());
    }

    #[test]
    fn failed_spawn_leaves_slots_unchanged() {
        let mut slot: Option<()> = None;
        let mut port = Some(PORT_RANGE_START);
        let result: Result<(), String> = spawn_and_store_child(
            Ok(()),
            &mut slot,
            &mut port,
            PORT_RANGE_START + 1,
            || Err("spawn failed".into()),
            |_| panic!("post-spawn must not run"),
        );
        assert!(result.is_err());
        assert!(slot.is_none());
        assert_eq!(port, Some(PORT_RANGE_START));
    }

    #[test]
    fn invalid_spawn_validation_does_not_invoke_spawn() {
        let mut slot = None;
        let mut port = None;
        let mut spawned = false;
        let result = spawn_and_store_child(
            Err("stale generation".into()),
            &mut slot,
            &mut port,
            PORT_RANGE_START,
            || {
                spawned = true;
                Ok(((), "fake child"))
            },
            |_| Ok(()),
        );
        assert!(result.is_err());
        assert!(!spawned);
        assert!(slot.is_none());
        assert!(port.is_none());
    }

    #[test]
    fn occupied_spawn_slot_does_not_invoke_spawn() {
        let mut slot = Some("existing child");
        let mut port = Some(PORT_RANGE_START);
        let mut spawned = false;
        let result = spawn_and_store_child(
            Ok(()),
            &mut slot,
            &mut port,
            PORT_RANGE_START + 1,
            || {
                spawned = true;
                Ok(((), "new child"))
            },
            |_| Ok(()),
        );
        assert!(result.is_err());
        assert!(!spawned);
        assert_eq!(slot, Some("existing child"));
        assert_eq!(port, Some(PORT_RANGE_START));
    }

    #[test]
    fn post_spawn_failure_keeps_synchronously_stored_child() {
        let mut slot = None;
        let mut port = None;
        let result = spawn_and_store_child(
            Ok(()),
            &mut slot,
            &mut port,
            PORT_RANGE_START,
            || Ok((123_u32, "fake child")),
            |child| {
                assert_eq!(*child, "fake child");
                Err("ownership receipt failed".into())
            },
        );
        assert!(result.is_err());
        assert_eq!(slot, Some("fake child"));
        assert_eq!(port, Some(PORT_RANGE_START));
    }

    #[cfg(unix)]
    #[test]
    fn measured_identity_confirms_an_owned_child_that_has_exited() {
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("0.01")
            .spawn()
            .unwrap();
        let pid = child.id();
        child.wait().unwrap();
        assert_eq!(
            measured_process_identity(pid),
            crate::remote_relay::orphan::Observation::Gone
        );
    }

    #[test]
    fn malformed_owned_receipt_is_preserved_without_probing_or_signaling() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("owner.json");
        let owner = CloudflaredOwner {
            pid: 424242,
            port: PORT_RANGE_START,
            identity: "invalid".into(),
        };
        write_cloudflared_owner_at(&path, &owner).unwrap();
        assert!(cleanup_owned_cloudflared_at(
            &path,
            None,
            PORT_RANGE_START,
            |_| panic!("invalid probe"),
            |_, _, _| panic!("invalid signal"),
            || panic!("invalid wait")
        )
        .is_err());
        assert_eq!(read_cloudflared_owner_at(&path).unwrap(), Some(owner));
    }

    #[test]
    fn process_and_listener_parsers_do_not_turn_tool_errors_into_absence() {
        assert_eq!(parse_process_field(false, Some(1), b"", b""), Ok(None));
        for (success, code, out, err) in [
            (false, Some(2), &b""[..], &b""[..]),
            (false, Some(1), &b""[..], &b"denied"[..]),
            (false, Some(1), &b"plausible process"[..], &b""[..]),
            (true, Some(0), &b""[..], &b""[..]),
            (true, Some(0), &b"one\ntwo"[..], &b""[..]),
            (true, Some(0), &[255][..], &b""[..]),
        ] {
            assert!(parse_process_field(success, code, out, err).is_err());
        }
        assert_eq!(
            parse_process_field(true, Some(0), b" cloudflared tunnel \n", b""),
            Ok(Some("cloudflared tunnel".into()))
        );
        assert_eq!(parse_listener_pids(false, Some(1), b"", b""), Some(vec![]));
        assert_eq!(
            parse_listener_pids(true, Some(0), b"42\n43\n", b""),
            Some(vec![42, 43])
        );
        assert!(parse_listener_pids(true, Some(0), b"42\nbad\n", b"").is_none());
        assert!(parse_listener_pids(false, Some(1), b"", b"denied").is_none());
        assert!(parse_listener_pids(true, Some(0), b"0\n", b"").is_none());
    }

    #[test]
    fn orphan_receipt_survives_unknown_and_failed_signals() {
        use crate::remote_relay::orphan::Observation;
        for unknown in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("owner.json");
            let owner = CloudflaredOwner { pid: 424242, port: PORT_RANGE_START,
                identity: format!("started\ncloudflared tunnel --no-autoupdate --url http://localhost:{PORT_RANGE_START}") };
            write_cloudflared_owner_at(&path, &owner).unwrap();
            let mut signals = vec![];
            let result = cleanup_owned_cloudflared_at(
                &path,
                None,
                PORT_RANGE_START,
                |_| {
                    if unknown {
                        Observation::Unknown
                    } else {
                        Observation::Identity(owner.identity.clone())
                    }
                },
                |pid, force, identity| {
                    assert_eq!(pid, owner.pid);
                    assert_eq!(identity, owner.identity);
                    signals.push(force);
                    false
                },
                || {},
            );
            assert!(result.is_err());
            assert_eq!(read_cloudflared_owner_at(&path).unwrap(), Some(owner));
            assert_eq!(signals, if unknown { vec![] } else { vec![false, true] });
        }
    }

    #[test]
    fn orphan_receipt_removal_requires_confirmed_exit_or_replacement() {
        use crate::remote_relay::orphan::Observation;
        for replacement in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("owner.json");
            let owner = CloudflaredOwner { pid: 424242, port: PORT_RANGE_START,
                identity: format!("started\ncloudflared tunnel --no-autoupdate --url http://localhost:{PORT_RANGE_START}") };
            write_cloudflared_owner_at(&path, &owner).unwrap();
            cleanup_owned_cloudflared_at(
                &path,
                Some(&owner),
                PORT_RANGE_START,
                |_| {
                    if replacement {
                        Observation::Identity("different start\nother process".into())
                    } else {
                        Observation::Gone
                    }
                },
                |_, _, _| panic!("must not signal exited/replaced process"),
                || panic!("must not wait"),
            )
            .unwrap();
            assert_eq!(read_cloudflared_owner_at(&path).unwrap(), None);
        }
    }

    #[test]
    fn stale_cleanup_never_probes_or_removes_a_replacement_receipt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("owner.json");
        let current = CloudflaredOwner { pid: 424242, port: PORT_RANGE_START,
            identity: format!("started\ncloudflared tunnel --no-autoupdate --url http://localhost:{PORT_RANGE_START}") };
        let old = CloudflaredOwner {
            pid: 424241,
            ..current.clone()
        };
        write_cloudflared_owner_at(&path, &current).unwrap();
        cleanup_owned_cloudflared_at(
            &path,
            Some(&old),
            PORT_RANGE_START,
            |_| panic!("stale probe"),
            |_, _, _| panic!("stale signal"),
            || panic!("stale wait"),
        )
        .unwrap();
        assert_eq!(read_cloudflared_owner_at(&path).unwrap(), Some(current));
    }

    #[tokio::test]
    async fn orphan_failure_blocks_reconnect_until_a_measured_retry_succeeds() {
        let state = tokio::sync::Mutex::new(RemoteAccessState::default());
        stop_with_cleanup(
            &state,
            Some(0),
            |status| assert!(matches!(status, RemoteAccessStatus::Error { .. })),
            async { Err("probe unavailable".into()) },
        )
        .await
        .unwrap();
        {
            let mut current = state.lock().await;
            assert!(current.orphan_cleanup_failed);
            assert!(disconnect_result_for(&current, Ok(())).is_err());
            assert!(matches!(
                apply_disconnect_result(&mut current, 1, Ok(())),
                Some(RemoteAccessStatus::Error { .. })
            ));
        }
        stop_with_cleanup(
            &state,
            Some(1),
            |status| assert!(matches!(status, RemoteAccessStatus::Off)),
            async { Ok(()) },
        )
        .await
        .unwrap();
        assert!(!state.lock().await.orphan_cleanup_failed);
    }

    #[test]
    fn unconfirmed_shutdown_blocks_explicit_and_automatic_starts() {
        for expected in [None, Some(5)] {
            let mut status = RemoteAccessStatus::Error {
                error: SHUTDOWN_UNCONFIRMED.into(),
            };
            let mut generation = 5;
            assert!(try_begin_start(&mut status, &mut generation, expected, true).is_none());
            assert_eq!(generation, 5);
            assert!(matches!(status, RemoteAccessStatus::Error { .. }));
        }
    }

    #[tokio::test]
    async fn incomplete_shutdown_is_retained_and_remote_success_cannot_hide_it() {
        let state = tokio::sync::Mutex::new(RemoteAccessState {
            pending_stops: vec![crate::remote_relay::shutdown::PendingProcess::unmeasured(
                std::process::id(),
            )],
            port: Some(17899),
            ..Default::default()
        });
        let mut emitted = None;
        let generation = stop_with_cleanup(
            &state,
            Some(0),
            |status| emitted = Some(status.clone()),
            async { Ok(()) },
        )
        .await
        .unwrap();
        assert!(
            matches!(emitted, Some(RemoteAccessStatus::Error { error }) if error == SHUTDOWN_UNCONFIRMED)
        );
        let mut current = state.lock().await;
        assert_eq!(current.port, Some(17899));
        assert_eq!(current.pending_stops.len(), 1);
        assert!(
            matches!(apply_disconnect_result(&mut current, generation, Ok(())),
            Some(RemoteAccessStatus::Error { error }) if error == SHUTDOWN_UNCONFIRMED)
        );
        let combined = disconnect_result_for(&current, Err("Server revocation unconfirmed".into()))
            .unwrap_err();
        assert!(combined.contains(SHUTDOWN_UNCONFIRMED));
        assert!(combined.contains("Server revocation unconfirmed"));
    }

    #[test]
    fn unconfirmed_disconnect_is_visible_but_cannot_overwrite_a_later_start() {
        let mut state = RemoteAccessState::default();
        let warning = "Saved settings are unconfirmed; retry before restarting the App";
        assert!(matches!(
            apply_disconnect_result(&mut state, 0, Err(warning.into())),
            Some(RemoteAccessStatus::Error { error }) if error == warning
        ));
        assert!(matches!(state.status, RemoteAccessStatus::Error { .. }));
        state.generation = 1;
        state.status = RemoteAccessStatus::Starting;
        assert!(apply_disconnect_result(&mut state, 0, Ok(())).is_none());
        assert!(apply_disconnect_result(&mut state, 0, Err(warning.into())).is_none());
        assert!(matches!(state.status, RemoteAccessStatus::Starting));
    }

    #[test]
    fn confirmed_disconnect_clears_the_previous_warning() {
        let mut state = RemoteAccessState {
            status: RemoteAccessStatus::Error {
                error: "pending".into(),
            },
            ..Default::default()
        };
        assert!(matches!(
            apply_disconnect_result(&mut state, 0, Ok(())),
            Some(RemoteAccessStatus::Off)
        ));
        assert!(matches!(state.status, RemoteAccessStatus::Off));
    }

    #[tokio::test]
    async fn stop_cleanup_finishes_before_a_new_start_can_reuse_its_ports() {
        let state = std::sync::Arc::new(tokio::sync::Mutex::new(RemoteAccessState::default()));
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let stopping_state = state.clone();
        let stopping = tokio::spawn(async move {
            stop_with_cleanup(&stopping_state, Some(0), |_| {}, async {
                entered_tx.send(()).unwrap();
                release_rx.await.unwrap();
                Ok(())
            })
            .await
        });
        entered_rx.await.unwrap();
        assert!(state.try_lock().is_err());
        let starting_state = state.clone();
        let mut starting = tokio::spawn(async move {
            let mut state = starting_state.lock().await;
            let RemoteAccessState {
                status, generation, ..
            } = &mut *state;
            try_begin_start(status, generation, None, false)
        });
        assert!(timeout(Duration::from_millis(50), &mut starting)
            .await
            .is_err());
        release_tx.send(()).unwrap();
        assert_eq!(stopping.await.unwrap(), Some(1));
        assert_eq!(starting.await.unwrap(), Some(2));
        assert!(matches!(
            state.lock().await.status,
            RemoteAccessStatus::Starting
        ));
    }

    #[tokio::test]
    async fn stale_stop_cannot_emit_or_run_cleanup_against_a_new_generation() {
        let state = tokio::sync::Mutex::new(RemoteAccessState {
            status: RemoteAccessStatus::Starting,
            generation: 2,
            ..RemoteAccessState::default()
        });
        assert_eq!(
            stop_with_cleanup(&state, Some(1), |_| panic!("stale event"), async {
                panic!("stale cleanup");
            })
            .await,
            None
        );
        assert!(matches!(
            state.lock().await.status,
            RemoteAccessStatus::Starting
        ));
    }

    /// Points every root these tests write at a tempdir.
    ///
    /// `HOME` alone did not do that. `mcp_config_dir()` goes through
    /// `dirs::home_dir()`, which on Windows resolves `FOLDERID_Profile` and
    /// ignores `HOME` — so every test here operated on the developer's real
    /// `%USERPROFILE%\.config\wenlan-mcp\relay_id`.
    /// `relay_id_generation_errors_when_relay_id_path_is_directory` then
    /// created that path as a DIRECTORY, which is why the three relay-id tests
    /// were failing on this host: not an environment quirk, this suite's own
    /// leftovers. `isolate_app_roots` is kept alive by the guard so the
    /// relocation lasts exactly as long as `HOME` does.
    struct HomeGuard {
        home: Option<OsString>,
        dev_state: Option<OsString>,
        dev_remote_port_start: Option<OsString>,
        _roots: crate::test_env::EnvGuard,
    }

    impl HomeGuard {
        fn set(path: &Path) -> Self {
            let home = std::env::var_os("HOME");
            let dev_state = std::env::var_os("WENLAN_DEV_STATE_DIR");
            let dev_remote_port_start = std::env::var_os("WENLAN_DEV_REMOTE_PORT_START");
            std::env::set_var("HOME", path);
            std::env::remove_var("WENLAN_DEV_STATE_DIR");
            std::env::remove_var("WENLAN_DEV_REMOTE_PORT_START");
            let _roots = crate::test_env::isolate_app_roots(path);
            Self {
                home,
                dev_state,
                dev_remote_port_start,
                _roots,
            }
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match &self.home {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
            match &self.dev_state {
                Some(value) => std::env::set_var("WENLAN_DEV_STATE_DIR", value),
                None => std::env::remove_var("WENLAN_DEV_STATE_DIR"),
            }
            match &self.dev_remote_port_start {
                Some(value) => std::env::set_var("WENLAN_DEV_REMOTE_PORT_START", value),
                None => std::env::remove_var("WENLAN_DEV_REMOTE_PORT_START"),
            }
        }
    }

    #[test]
    fn test_status_default_is_off() {
        let state = RemoteAccessState::default();
        assert!(matches!(state.status, RemoteAccessStatus::Off));
        assert!(state.port.is_none());
    }

    #[test]
    fn starting_transition_is_atomic_against_duplicate_attempts() {
        let mut status = RemoteAccessStatus::Off;
        let mut generation = 0;

        let start_generation = try_begin_start(&mut status, &mut generation, None, false).unwrap();
        assert!(matches!(status, RemoteAccessStatus::Starting));
        assert!(
            try_begin_start(&mut status, &mut generation, Some(start_generation), false).is_none()
        );
    }

    #[test]
    fn explicit_off_invalidates_in_flight_start_and_delayed_retry() {
        let mut status = RemoteAccessStatus::Off;
        let mut generation = 0;

        let in_flight = try_begin_start(&mut status, &mut generation, None, false).unwrap();
        let recovery = mark_off(&mut status, &mut generation);
        assert_ne!(in_flight, recovery);

        let explicit_off = mark_off(&mut status, &mut generation);
        assert_ne!(recovery, explicit_off);
        assert!(try_begin_start(&mut status, &mut generation, Some(recovery), false).is_none());
        assert!(!generation_is_current(generation, in_flight));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn listener_identity_resolves_the_process_that_owns_the_port() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();

        assert_eq!(listener_pid_for_port(port), Some(std::process::id()));
    }

    #[test]
    fn test_status_serializes_correctly() {
        let status = RemoteAccessStatus::Off;
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("\"off\""));

        let status = RemoteAccessStatus::Starting;
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("\"starting\""));

        let status = RemoteAccessStatus::Connected {
            tunnel_url: None,
            relay_url: Some(format!("{RELAY_ORIGIN}/mcp")),
        };
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["status"], "connected");
        assert!(json["tunnel_url"].is_null());
        assert_eq!(json["relay_url"], format!("{RELAY_ORIGIN}/mcp"));

        let status = RemoteAccessStatus::Error {
            error: "something broke".to_string(),
        };
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("\"error\""));
        assert!(json.contains("something broke"));
    }

    #[test]
    #[cfg(debug_assertions)]
    #[serial_test::serial]
    fn test_find_available_port_returns_first_available() {
        let tmp = tempfile::tempdir().unwrap();
        let _home = HomeGuard::set(tmp.path());

        // HomeGuard::set already cleared WENLAN_DEV_REMOTE_PORT_START, so this
        // is the no-override fallback -- the one case the ephemeral-port scan
        // below can't exercise, since it always sets the override.
        assert_eq!(port_range_start(), PORT_RANGE_START);

        // Scan from an OS-assigned held port instead of the fixed default range,
        // which flakes under machine load. Re-bind if the port is above
        // u16::MAX - 3: the 4-port scan window would otherwise overflow.
        let (_held, held_port) = loop {
            let held = TcpListener::bind("127.0.0.1:0").unwrap();
            let held_port = held.local_addr().unwrap().port();
            if held_port <= u16::MAX - 3 {
                break (held, held_port);
            }
        };
        std::env::set_var("WENLAN_DEV_REMOTE_PORT_START", held_port.to_string());

        let port = find_available_port();
        assert!(port.is_some());
        let p = port.unwrap();
        assert_ne!(p, held_port, "must skip the port already held");
        assert!((held_port..=held_port + 3).contains(&p));
    }

    #[test]
    fn orphan_identity_preserves_windows_paths_with_spaces() {
        let mcp = format!(
            "argv:{}",
            serde_json::json!([
                "C:\\Users\\Test User\\Wenlan\\wenlan-mcp.exe",
                "serve",
                "--port",
                "22000",
                "--agent-name",
                "remote-mcp"
            ])
        );
        assert!(is_expected_remote_mcp_command(&mcp, 22000));
        assert!(!is_expected_remote_mcp_command(&mcp, 22001));
        let tunnel = format!(
            "argv:{}",
            serde_json::json!([
                "C:\\Program Files\\Wenlan\\cloudflared.exe",
                "tunnel",
                "--no-autoupdate",
                "--url",
                "http://localhost:22000"
            ])
        );
        assert!(is_expected_remote_tunnel_command(&tunnel, 22000));
        assert!(!is_expected_remote_tunnel_command(&tunnel, 22001));
        assert!(!is_expected_remote_mcp_command("argv:{broken", 22000));
        assert!(!is_expected_remote_tunnel_command("argv:[]", 22000));
    }

    #[cfg(unix)]
    #[test]
    fn orphan_identity_does_not_treat_unix_backslash_as_path_separator() {
        assert!(!is_expected_remote_mcp_command(
            "/tmp/not-wenlan\\wenlan-mcp serve --port 22000 --agent-name remote-mcp",
            22000
        ));
    }

    #[test]
    fn orphan_cleanup_requires_the_exact_remote_mcp_process_identity() {
        assert!(is_expected_remote_mcp_command(
            "/tmp/wenlan-mcp-aarch64-apple-darwin --origin-url http://127.0.0.1:17777 serve --port 22000 --no-auth --agent-name remote-mcp",
            22000,
        ));
        assert!(!is_expected_remote_mcp_command(
            "/tmp/wenlan-mcp-malicious serve --port 22000 --agent-name remote-mcp",
            22000,
        ));
        assert!(!is_expected_remote_mcp_command(
            "/tmp/wenlan-mcp-aarch64-apple-darwin serve --port 22001 --agent-name remote-mcp",
            22000,
        ));
        assert!(!is_expected_remote_mcp_command(
            "/tmp/wenlan-mcp-aarch64-apple-darwin serve --port 22000 --agent-name another-agent",
            22000,
        ));
    }

    #[test]
    fn orphan_cleanup_also_recognizes_only_the_exact_web_write_process() {
        let write = "/tmp/wenlan-mcp-aarch64-apple-darwin --origin-url http://127.0.0.1:17777 serve --host 127.0.0.1 --port 22001 --tool-profile web-write --token-env WENLAN_REMOTE_MCP_TOKEN --agent-name web-ai";
        assert!(is_expected_remote_mcp_command(write, 22001));
        assert!(!is_expected_remote_mcp_command(write, 22000));
        // The agent name alone, or the profile alone, is not our process.
        assert!(!is_expected_remote_mcp_command(
            "/tmp/wenlan-mcp serve --port 22001 --agent-name web-ai",
            22001
        ));
        assert!(!is_expected_remote_mcp_command(
            "/tmp/wenlan-mcp serve --port 22001 --tool-profile web-write --agent-name another-agent",
            22001
        ));
        assert!(!is_expected_remote_mcp_command(
            "/tmp/wenlan-mcp-malicious serve --port 22001 --tool-profile web-write --agent-name web-ai",
            22001
        ));
    }

    #[test]
    fn a_fresh_state_has_no_write_server_and_an_empty_route() {
        let state = RemoteAccessState::default();
        assert!(state.write_child.is_none());
        assert!(state.write_port.is_none());
        assert_eq!(state.write_route.get(), None);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stop_ends_the_write_server_and_empties_the_route_with_the_search_one() {
        let app = shell_test_app();
        let mut state = RemoteAccessState {
            status: RemoteAccessStatus::Starting,
            generation: 7,
            ..Default::default()
        };
        let validation = validate_mcp_spawn(&state, 7);
        let (_query_events, query_pid) = spawn_and_store_child(
            validation,
            &mut state.mcp_child,
            &mut state.port,
            PORT_RANGE_START,
            || {
                let (events, child) = app
                    .shell()
                    .command("/bin/sleep")
                    .arg("5")
                    .spawn()
                    .map_err(|_| "owned fixture spawn failed".to_string())?;
                Ok(((events, child.pid()), child))
            },
            |_| Ok(()),
        )
        .unwrap();
        // The write server needs the search one to exist first, and only once.
        let validation = validate_write_spawn(&state, 7);
        assert!(validation.is_ok());
        let (_write_events, write_pid) = spawn_and_store_child(
            validation,
            &mut state.write_child,
            &mut state.write_port,
            PORT_RANGE_START + 1,
            || {
                let (events, child) = app
                    .shell()
                    .command("/bin/sleep")
                    .arg("5")
                    .spawn()
                    .map_err(|_| "owned fixture spawn failed".to_string())?;
                Ok(((events, child.pid()), child))
            },
            |_| Ok(()),
        )
        .unwrap();
        state.write_route.set(state.write_port);
        assert!(validate_write_spawn(&state, 7).is_err());
        assert!(validate_write_spawn(&state, 6).is_err());
        let route = state.write_route.clone();
        let state = tokio::sync::Mutex::new(state);

        let generation = stop_with_cleanup(&state, Some(7), |_| {}, async { Ok(()) }).await;

        assert_eq!(generation, Some(8));
        assert_eq!(route.get(), None);
        let stopped = state.lock().await;
        assert!(stopped.mcp_child.is_none() && stopped.write_child.is_none());
        assert!(stopped.write_port.is_none() && stopped.pending_stops.is_empty());
        for pid in [query_pid, write_pid] {
            assert_eq!(
                measured_process_identity(pid),
                crate::remote_relay::orphan::Observation::Gone
            );
        }
    }

    #[test]
    fn the_write_server_is_refused_without_the_search_one_or_when_inactive() {
        let mut state = RemoteAccessState {
            status: RemoteAccessStatus::Connected {
                tunnel_url: None,
                relay_url: None,
            },
            generation: 3,
            ..Default::default()
        };
        assert!(validate_write_spawn(&state, 3).is_err(), "no search server");
        state.status = RemoteAccessStatus::Off;
        assert!(validate_write_spawn(&state, 3).is_err(), "not active");
    }

    #[test]
    fn orphan_cleanup_requires_the_exact_remote_tunnel_process_identity() {
        assert!(is_expected_remote_tunnel_command(
            "/tmp/cloudflared-aarch64-apple-darwin tunnel --url http://localhost:22000",
            22000,
        ));
        assert!(is_expected_remote_tunnel_command(
            "/tmp/cloudflared tunnel --url http://localhost:22000",
            22000,
        ));
        assert!(!is_expected_remote_tunnel_command(
            "/tmp/cloudflared tunnel --url http://localhost:22001",
            22000,
        ));
        assert!(!is_expected_remote_tunnel_command(
            "/tmp/not-cloudflared tunnel --url http://localhost:22000",
            22000,
        ));
        assert!(!is_expected_remote_tunnel_command(
            "/tmp/cloudflared access tcp --url http://localhost:22000",
            22000,
        ));
    }

    #[test]
    fn cloudflared_signal_requires_the_persisted_owner_and_live_identity() {
        let owner = CloudflaredOwner {
            pid: 42,
            port: 22000,
            identity: "started\ncloudflared tunnel".to_string(),
        };
        let other = CloudflaredOwner {
            pid: 43,
            ..owner.clone()
        };

        assert!(cloudflared_owner_authorizes_signal(
            &owner,
            Some(&owner),
            22000,
            Some(owner.identity.as_str()),
        ));
        assert!(cloudflared_owner_authorizes_signal(
            &owner,
            None,
            22000,
            Some(owner.identity.as_str()),
        ));
        assert!(!cloudflared_owner_authorizes_signal(
            &owner,
            Some(&other),
            22000,
            Some(owner.identity.as_str()),
        ));
        assert!(!cloudflared_owner_authorizes_signal(
            &owner,
            Some(&owner),
            22000,
            Some("different process start"),
        ));
        assert!(!cloudflared_owner_authorizes_signal(
            &owner,
            Some(&owner),
            23000,
            Some(owner.identity.as_str()),
        ));
    }

    #[test]
    fn owner_lock_serializes_receipt_replacement_after_compare_remove() {
        use std::sync::{mpsc, Arc};

        let dir = tempfile::tempdir().unwrap();
        let path = Arc::new(dir.path().join("cloudflared-owner.json"));
        let old_owner = CloudflaredOwner {
            pid: 42,
            port: 22000,
            identity: "old".to_string(),
        };
        let new_owner = CloudflaredOwner {
            pid: 43,
            port: 22001,
            identity: "new".to_string(),
        };
        write_cloudflared_owner_at(&path, &old_owner).unwrap();

        let (validated_tx, validated_rx) = mpsc::channel();
        let (continue_tx, continue_rx) = mpsc::channel();
        let cleanup_path = Arc::clone(&path);
        let cleanup_owner = old_owner.clone();
        let cleanup = std::thread::spawn(move || {
            with_cloudflared_owner_lock(&cleanup_path, |receipt| {
                let current = receipt.read()?.unwrap();
                assert_eq!(current, cleanup_owner);
                validated_tx.send(()).unwrap();
                continue_rx.recv().unwrap();
                receipt.remove_if_matches(&cleanup_owner)
            })
            .unwrap();
        });

        validated_rx.recv().unwrap();
        let (replacement_done_tx, replacement_done_rx) = mpsc::channel();
        let replacement_path = Arc::clone(&path);
        let replacement_owner = new_owner.clone();
        let replacement = std::thread::spawn(move || {
            write_cloudflared_owner_at(&replacement_path, &replacement_owner).unwrap();
            replacement_done_tx.send(()).unwrap();
        });

        assert!(replacement_done_rx
            .recv_timeout(std::time::Duration::from_millis(50))
            .is_err());
        continue_tx.send(()).unwrap();
        cleanup.join().unwrap();
        replacement.join().unwrap();

        assert_eq!(read_cloudflared_owner_at(&path).unwrap(), Some(new_owner));
    }

    #[test]
    #[cfg(unix)]
    fn owner_replacement_swaps_the_receipt_inode_instead_of_truncating_in_place() {
        use std::os::unix::fs::MetadataExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cloudflared-owner.json");
        let old_owner = CloudflaredOwner {
            pid: 42,
            port: 22000,
            identity: "old".to_string(),
        };
        let new_owner = CloudflaredOwner {
            pid: 43,
            port: 22001,
            identity: "new".to_string(),
        };

        write_cloudflared_owner_at(&path, &old_owner).unwrap();
        let old_inode = std::fs::metadata(&path).unwrap().ino();
        write_cloudflared_owner_at(&path, &new_owner).unwrap();
        let new_inode = std::fs::metadata(&path).unwrap().ino();

        assert_ne!(old_inode, new_inode);
        assert_eq!(read_cloudflared_owner_at(&path).unwrap(), Some(new_owner));
    }

    #[test]
    #[cfg(debug_assertions)]
    #[serial_test::serial]
    fn dev_remote_access_uses_its_worktree_port_range() {
        let tmp = tempfile::tempdir().unwrap();
        let _home = HomeGuard::set(tmp.path());

        // Scan from an OS-assigned held port instead of the fixed default range,
        // which flakes under machine load. Re-bind if the port is above
        // u16::MAX - 3: the 4-port scan window would otherwise overflow.
        let (_held, held_port) = loop {
            let held = TcpListener::bind("127.0.0.1:0").unwrap();
            let held_port = held.local_addr().unwrap().port();
            if held_port <= u16::MAX - 3 {
                break (held, held_port);
            }
        };
        std::env::set_var("WENLAN_DEV_REMOTE_PORT_START", held_port.to_string());

        let port = find_available_port().expect("dev remote access port");

        assert_ne!(port, held_port, "must skip the port already held");
        assert!((held_port..=held_port + 3).contains(&port));
    }
}

#[cfg(test)]
mod notice_tests {
    use super::*;

    const NOW: u64 = 1_000_000;

    fn offline() -> RenewalError {
        RenewalError::Relay(RelayError::Unavailable)
    }
    fn refused() -> RenewalError {
        RenewalError::Relay(RelayError::Unauthorized)
    }

    #[test]
    fn nothing_is_announced_while_a_retry_is_still_scheduled() {
        assert_eq!(give_up_notice(&offline(), true, None, NOW), None);
        assert_eq!(give_up_notice(&refused(), true, Some(NOW - 1), NOW), None);
    }

    #[test]
    fn giving_up_announces_a_stop_unless_the_key_itself_ended() {
        // Out of attempts while the relay is unreachable.
        assert_eq!(
            give_up_notice(&offline(), false, Some(NOW + 1), NOW),
            Some(RemoteAccessNotice::Stopped)
        );
        // Refused while the key is still valid: revoked or reset elsewhere.
        assert_eq!(
            give_up_notice(&refused(), false, Some(NOW + 1), NOW),
            Some(RemoteAccessNotice::Stopped)
        );
        assert_eq!(
            give_up_notice(&refused(), false, None, NOW),
            Some(RemoteAccessNotice::Stopped)
        );
        // Refused because the key's date has passed (the boundary counts as ended).
        assert_eq!(
            give_up_notice(&refused(), false, Some(NOW), NOW),
            Some(RemoteAccessNotice::Expired)
        );
        assert_eq!(
            give_up_notice(&refused(), false, Some(NOW - 1), NOW),
            Some(RemoteAccessNotice::Expired)
        );
        // A past date does not turn an unrelated failure into "expired".
        assert_eq!(
            give_up_notice(&offline(), false, Some(NOW - 1), NOW),
            Some(RemoteAccessNotice::Stopped)
        );
    }

    #[test]
    fn a_retryable_failure_with_attempts_left_is_not_a_give_up_but_the_last_attempt_is() {
        let retrying = |attempts| recovery_delay(&offline(), attempts).is_some();
        assert_eq!(give_up_notice(&offline(), retrying(0), None, NOW), None);
        assert_eq!(
            give_up_notice(&offline(), retrying(MAX_RECONNECT_RETRIES), None, NOW),
            Some(RemoteAccessNotice::Stopped)
        );
        // Authorization failures are never retried, so they announce at once.
        assert_eq!(
            give_up_notice(
                &refused(),
                recovery_delay(&refused(), 0).is_some(),
                None,
                NOW
            ),
            Some(RemoteAccessNotice::Stopped)
        );
    }

    #[test]
    fn the_wire_shape_is_a_tagged_kind_the_window_can_switch_on() {
        assert_eq!(
            serde_json::to_value(RemoteAccessNotice::Stopped).unwrap(),
            serde_json::json!({ "kind": "stopped" })
        );
        assert_eq!(
            serde_json::to_value(RemoteAccessNotice::Expired).unwrap(),
            serde_json::json!({ "kind": "expired" })
        );
    }

    #[test]
    fn a_connection_is_offered_for_testing_only_when_connected_with_a_port_and_a_live_id() {
        let connected = || RemoteAccessStatus::Connected {
            tunnel_url: None,
            relay_url: None,
        };
        assert_eq!(
            live_target(&connected(), Some(18080), Some("conn-1")),
            Some((18080, "conn-1".to_owned()))
        );
        assert_eq!(live_target(&connected(), None, Some("conn-1")), None);
        assert_eq!(live_target(&connected(), Some(18080), None), None);
        // A live connection id is not enough while the controller says otherwise.
        for status in [
            RemoteAccessStatus::Off,
            RemoteAccessStatus::Starting,
            RemoteAccessStatus::Error {
                error: "x".to_owned(),
            },
        ] {
            assert_eq!(live_target(&status, Some(18080), Some("conn-1")), None);
        }
    }

    #[test]
    fn only_a_fully_connected_state_offers_a_connection_to_test() {
        let mut state = RemoteAccessState::default();
        assert_eq!(connection_target(&state), None);
        state.port = Some(18080);
        state.status = RemoteAccessStatus::Connected {
            tunnel_url: None,
            relay_url: None,
        };
        // Connected in name only: no live relay connection to check.
        assert_eq!(connection_target(&state), None);
        assert_eq!(state.pending_notice.take(), None);
        state.pending_notice = Some(RemoteAccessNotice::Expired);
        assert_eq!(
            state.pending_notice.take(),
            Some(RemoteAccessNotice::Expired)
        );
        assert_eq!(state.pending_notice.take(), None);
    }

    #[tokio::test]
    async fn a_published_notice_wakes_the_window_and_is_taken_exactly_once() {
        use tauri::{Listener, Manager};
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build a windowless test runtime");
        app.manage(std::sync::Arc::new(tokio::sync::RwLock::new(
            crate::state::AppState::new(),
        )));
        let woken = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let heard = woken.clone();
        app.listen("remote-access-notice", move |event| {
            heard.lock().unwrap().push(event.payload().to_string());
        });

        // Nothing is waiting before anything stopped.
        assert_eq!(
            crate::search::take_remote_access_notice(app.state())
                .await
                .unwrap(),
            None
        );

        publish_notice(app.handle(), RemoteAccessNotice::Expired).await;

        assert_eq!(
            woken.lock().unwrap().as_slice(),
            [r#"{"kind":"expired"}"#.to_string()]
        );
        // A window that was not listening yet still finds it, once.
        assert_eq!(
            crate::search::take_remote_access_notice(app.state())
                .await
                .unwrap(),
            Some(RemoteAccessNotice::Expired)
        );
        assert_eq!(
            crate::search::take_remote_access_notice(app.state())
                .await
                .unwrap(),
            None
        );
    }
}
