use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Listener, Manager};
use tauri_plugin_updater::UpdaterExt;

const SUPPRESS_TTL: Duration = Duration::from_secs(24 * 3600);
const STARTUP_DELAY: Duration = Duration::from_secs(3);
const CHECK_INTERVAL: Duration = Duration::from_secs(30 * 60);
// Manifest checks use a small budget; the package download gets a larger one
// because the two requests go through separate clients.
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const READ_TIMEOUT: Duration = Duration::from_secs(30);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const DAEMON_RECOVERY_TIMEOUT: Duration = Duration::from_secs(10);
const DAEMON_RECOVERY_POLL: Duration = Duration::from_millis(250);

static UPDATER_STARTED: AtomicBool = AtomicBool::new(false);

fn configure_network(
    builder: tauri_plugin_updater::UpdaterBuilder,
) -> tauri_plugin_updater::UpdaterBuilder {
    builder.timeout(CHECK_TIMEOUT).configure_client(|client| {
        client
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
    })
}

fn configure_download(update: &mut tauri_plugin_updater::Update) {
    update.timeout = Some(DOWNLOAD_TIMEOUT);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LifecyclePhase {
    Starting,
    Idle,
    Checking,
    Prompting,
    Installing,
}

impl LifecyclePhase {
    fn accepts_manual_check(self) -> bool {
        self == Self::Idle
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StatusState {
    Checking,
    Current,
    Available,
    Error,
}

impl StatusState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Checking => "checking",
            Self::Current => "current",
            Self::Available => "available",
            Self::Error => "error",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct KnownStatus {
    state: StatusState,
    version: Option<String>,
    error: Option<String>,
}

impl KnownStatus {
    fn checking() -> Self {
        Self {
            state: StatusState::Checking,
            version: None,
            error: None,
        }
    }

    fn current() -> Self {
        Self {
            state: StatusState::Current,
            version: None,
            error: None,
        }
    }

    fn error(message: impl Into<String>) -> Self {
        Self {
            state: StatusState::Error,
            version: None,
            error: Some(message.into()),
        }
    }
}

#[derive(Debug)]
struct LifecycleState {
    phase: LifecyclePhase,
    status: KnownStatus,
}

type SharedLifecycleState = Arc<Mutex<LifecycleState>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CheckTrigger {
    Startup,
    Scheduled,
    Manual,
}

fn parse_action(payload: &str) -> Option<bool> {
    let action =
        serde_json::from_str::<String>(payload).unwrap_or_else(|_| payload.trim().to_string());
    match action.as_str() {
        "install" => Some(true),
        "later" => Some(false),
        _ => None,
    }
}

fn should_prompt(trigger: CheckTrigger, recently_dismissed: bool) -> bool {
    trigger == CheckTrigger::Manual || !recently_dismissed
}

fn selected_data_dir_override() -> Option<PathBuf> {
    std::env::var_os("WENLAN_DATA_DIR")
        .or_else(|| std::env::var_os("ORIGIN_DATA_DIR"))
        .map(PathBuf::from)
}

fn canonical_absolute_path(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    // First launch may precede creation of the data directory. Resolve the
    // existing ancestor, retaining only genuinely absent path components.
    // Broken symlinks and inaccessible paths remain unknown, not equivalent.
    let mut ancestor = path;
    let mut missing = Vec::new();
    loop {
        match std::fs::canonicalize(ancestor) {
            Ok(mut resolved) => {
                for name in missing.iter().rev() {
                    resolved.push(name);
                }
                return Some(resolved);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match std::fs::symlink_metadata(ancestor) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    _ => return None,
                }
                missing.push(ancestor.file_name()?);
                ancestor = ancestor.parent()?;
            }
            Err(_) => return None,
        }
    }
}

fn updater_enabled_for_paths(
    debug_build: bool,
    production_root: Option<&Path>,
    override_root: Option<&Path>,
) -> bool {
    if debug_build {
        return false;
    }

    let Some(override_root) = override_root else {
        // The default release root is safe to use even on first launch, when
        // the daemon has not created its directory yet. An unresolved default
        // root only matters if an override needs to be compared with it.
        return true;
    };

    let Some(production_root) = production_root.and_then(canonical_absolute_path) else {
        return false;
    };
    canonical_absolute_path(override_root)
        .is_some_and(|override_root| override_root == production_root)
}

fn updater_enabled_for_environment(debug_build: bool, production_root: Option<&Path>) -> bool {
    let override_root = selected_data_dir_override();
    updater_enabled_for_paths(debug_build, production_root, override_root.as_deref())
}

#[derive(Serialize, Deserialize, Debug)]
struct DismissedUpdate {
    version: String,
    dismissed_at_secs: u64,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn dismissal_file_path(dir: &Path) -> PathBuf {
    dir.join("updater-dismissed.json")
}

fn dismissal_path(app: &AppHandle) -> Option<PathBuf> {
    let dir = app.path().app_data_dir().ok()?;
    std::fs::create_dir_all(&dir).ok()?;
    Some(dismissal_file_path(&dir))
}

fn was_recently_dismissed_at(path: &Path, version: &str, now: u64) -> bool {
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    let Ok(entry) = serde_json::from_slice::<DismissedUpdate>(&bytes) else {
        return false;
    };
    entry.version == version && now.saturating_sub(entry.dismissed_at_secs) < SUPPRESS_TTL.as_secs()
}

fn record_dismissal_at(path: &Path, version: &str, now: u64) {
    let entry = DismissedUpdate {
        version: version.to_string(),
        dismissed_at_secs: now,
    };
    if let Ok(bytes) = serde_json::to_vec(&entry) {
        let _ = std::fs::write(path, bytes);
    }
}

fn was_recently_dismissed(app: &AppHandle, version: &str) -> bool {
    let Some(path) = dismissal_path(app) else {
        return false;
    };
    was_recently_dismissed_at(&path, version, now_secs())
}

fn record_dismissal(app: &AppHandle, version: &str) {
    if let Some(path) = dismissal_path(app) {
        record_dismissal_at(&path, version, now_secs());
    }
}

fn emit_available(app: &AppHandle, version: &str) {
    let _ = app.emit(
        "updater://available",
        serde_json::json!({ "version": version }),
    );
}

fn status_payload(status: &KnownStatus) -> serde_json::Value {
    let mut payload = serde_json::Map::new();
    payload.insert(
        "state".to_string(),
        serde_json::Value::String(status.state.as_str().to_string()),
    );
    if let Some(version) = &status.version {
        payload.insert(
            "version".to_string(),
            serde_json::Value::String(version.clone()),
        );
    }
    if let Some(error) = &status.error {
        payload.insert(
            "error".to_string(),
            serde_json::Value::String(error.clone()),
        );
    }
    serde_json::Value::Object(payload)
}

fn emit_status(app: &AppHandle, status: &KnownStatus) {
    let _ = app.emit("updater://status", status_payload(status));
}

fn replay_status(app: &AppHandle, state: &SharedLifecycleState) {
    let (phase, status) = snapshot_lifecycle(state);
    emit_status(app, &status);
    if phase == LifecyclePhase::Prompting && status.state == StatusState::Available {
        if let Some(version) = status.version.as_deref() {
            emit_available(app, version);
        }
    }
}

fn snapshot_lifecycle(state: &SharedLifecycleState) -> (LifecyclePhase, KnownStatus) {
    state
        .lock()
        .map(|guard| (guard.phase, guard.status.clone()))
        .unwrap_or((
            LifecyclePhase::Starting,
            KnownStatus::error("Updater state unavailable"),
        ))
}

fn set_status(app: &AppHandle, state: &SharedLifecycleState, status: KnownStatus) {
    if let Ok(mut guard) = state.lock() {
        guard.status = status.clone();
    }
    emit_status(app, &status);
}

fn set_phase(state: &SharedLifecycleState, phase: LifecyclePhase) {
    if let Ok(mut guard) = state.lock() {
        guard.phase = phase;
    }
}

fn begin_check(state: &SharedLifecycleState) -> bool {
    let Ok(mut guard) = state.lock() else {
        return false;
    };
    if guard.phase != LifecyclePhase::Idle {
        return false;
    }
    guard.phase = LifecyclePhase::Checking;
    true
}

fn accept_manual_check(state: &SharedLifecycleState) -> bool {
    state
        .lock()
        .map(|guard| guard.phase.accepts_manual_check())
        .unwrap_or(false)
}

/// Emit `updater://available` to the main webview and wait for the user's
/// choice via the `updater://action` event. RuntimeOverlays mounts once as a
/// stable sibling of App's branch body and is reconciled in place, rather than
/// remounted as App moves between branches. The `updater://ui-ready` handshake
/// covers webview-load timing: UpdateWatcher emits one ready event after its
/// listeners register, and the backend re-emits availability if the prompt
/// predates it. The offer is shown as a row in the sidebar's service status
/// (see `src/components/UpdateWatcher.tsx` and `src/lib/appUpdate.ts`).
async fn prompt_via_overlay(app: &AppHandle, version: &str) -> bool {
    let (tx, rx) = tokio::sync::oneshot::channel::<bool>();
    let tx = Arc::new(Mutex::new(Some(tx)));

    let tx_action = Arc::clone(&tx);
    let action_id = app.listen("updater://action", move |event| {
        let payload = event.payload();
        let Some(install) = parse_action(payload) else {
            return;
        };
        if let Ok(mut g) = tx_action.lock() {
            if let Some(sender) = g.take() {
                let _ = sender.send(install);
            }
        }
    });

    emit_available(app, version);

    let accepted = rx.await.unwrap_or(false);
    app.unlisten(action_id);
    accepted
}

/// Resolve the release again after consent, immediately before downloading.
/// The manifest points at one immutable release, so an offer held open while a
/// newer release is published must not keep the older asset alive.
async fn refresh_update(
    updater: &tauri_plugin_updater::Updater,
) -> Result<Option<tauri_plugin_updater::Update>, String> {
    updater.check().await.map_err(|error| error.to_string())
}

fn update_restart_block_reason(
    outcome: &crate::daemon_start::SidecarStopOutcome,
) -> Option<String> {
    use crate::daemon_start::SidecarStopOutcome;

    match outcome {
        SidecarStopOutcome::NoSidecar | SidecarStopOutcome::Ended => None,
        SidecarStopOutcome::StillRunning { reason } => Some(format!(
            "update is ready, but the app sidecar is still running: {reason}"
        )),
        SidecarStopOutcome::CouldNotMeasure { reason } => Some(format!(
            "update is ready, but the app could not verify that its sidecar stopped: {reason}"
        )),
    }
}

fn report_install_failure(
    app: &AppHandle,
    state: &SharedLifecycleState,
    version: &str,
    message: String,
) {
    log::error!("update failed: {message}");
    let _ = app.emit(
        "updater://progress",
        serde_json::json!({ "error": message }),
    );
    set_status(
        app,
        state,
        KnownStatus {
            state: StatusState::Error,
            version: Some(version.to_string()),
            error: Some(message),
        },
    );
    set_phase(state, LifecyclePhase::Idle);
}

fn check_recovered_daemon_version(actual: &str) -> Result<(), String> {
    let expected = env!("CARGO_PKG_VERSION");
    if crate::release_part(actual) == crate::release_part(expected) {
        Ok(())
    } else {
        Err(format!(
            "daemon reports version {actual}; expected {expected}"
        ))
    }
}

fn validate_recovered_daemon_health(
    health: &wenlan_types::responses::HealthResponse,
) -> Result<(), String> {
    if health.status != "ok" {
        return Err(format!(
            "daemon health status is {}; expected ok",
            health.status
        ));
    }
    if !health.db_initialized {
        return Err("daemon health reports that its database is not initialized".to_string());
    }
    check_recovered_daemon_version(&health.version)
}

async fn restore_stopped_daemon(
    app: &AppHandle,
    client: &crate::api::WenlanClient,
) -> Result<(), String> {
    let deadline = tokio::time::Instant::now() + DAEMON_RECOVERY_TIMEOUT;
    let mut last_start_error = None;
    let mut last_health_error = None;
    let mut last_version_error = None;

    loop {
        match tokio::time::timeout_at(
            deadline,
            crate::daemon_start::start_daemon_if_unowned(app, client),
        )
        .await
        {
            Ok(crate::daemon_start::DaemonStartResult::Failed { message }) => {
                last_start_error = Some(message);
            }
            Ok(crate::daemon_start::DaemonStartResult::Started)
            | Ok(crate::daemon_start::DaemonStartResult::AlreadyRunning)
            | Ok(crate::daemon_start::DaemonStartResult::LaunchdManaged) => {}
            Err(_) => break,
        }

        match tokio::time::timeout_at(deadline, client.health()).await {
            Ok(Ok(health)) => match validate_recovered_daemon_health(&health) {
                Ok(()) => return Ok(()),
                Err(error) => last_version_error = Some(error),
            },
            Ok(Err(error)) => last_health_error = Some(error),
            Err(_) => break,
        }

        let now = tokio::time::Instant::now();
        if now >= deadline {
            break;
        }
        tokio::time::sleep(DAEMON_RECOVERY_POLL.min(deadline - now)).await;
    }

    let mut details = vec![format!(
        "daemon recovery did not confirm version {} within {DAEMON_RECOVERY_TIMEOUT:?}",
        env!("CARGO_PKG_VERSION")
    )];
    if let Some(error) = last_start_error {
        details.push(format!("last start attempt: {error}"));
    }
    if let Some(error) = last_health_error {
        details.push(format!("last health check: {error}"));
    }
    if let Some(error) = last_version_error {
        details.push(error);
    }
    Err(details.join("; "))
}

async fn recover_after_original_exit(
    wait_for_exit: impl std::future::Future<Output = Result<(), String>>,
    restore: impl std::future::Future<Output = Result<(), String>>,
) -> Result<(), String> {
    wait_for_exit.await?;
    restore.await
}

async fn resume_saved_remote_access(
    app: &AppHandle,
    state: &SharedLifecycleState,
) -> Option<String> {
    let ticket = crate::remote_access::prepare_startup(app.clone()).await;
    if let Some(ticket) = ticket {
        // A later update may already have stopped remote access. Obtain the
        // generation ticket first, then check the updater phase: a subsequent
        // stop invalidates the ticket before it can start a new process.
        loop {
            let installing = state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .phase
                == LifecyclePhase::Installing;
            if !installing {
                break;
            }
            // A successful install ends this process. If it fails, let this
            // recovery continue; the ticket still rejects any later stop.
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        crate::remote_access::resume_startup(app.clone(), ticket).await;
    }
    let app_state = app.state::<Arc<tokio::sync::RwLock<crate::state::AppState>>>();
    let remote = { app_state.read().await.remote_access.clone() };
    let status = remote.lock().await.status.clone();
    match status {
        crate::remote_access::RemoteAccessStatus::Error { error } => {
            Some(format!("saved remote access could not resume: {error}"))
        }
        crate::remote_access::RemoteAccessStatus::Off
        | crate::remote_access::RemoteAccessStatus::Starting
        | crate::remote_access::RemoteAccessStatus::Connected { .. } => None,
    }
}

async fn report_failure_after_cleanup(
    app: &AppHandle,
    state: &SharedLifecycleState,
    version: &str,
    message: String,
    restart_stopped_sidecar: bool,
) {
    let mut recovery_errors = Vec::new();
    if restart_stopped_sidecar {
        let client = crate::api::WenlanClient::new();
        if let Err(error) = restore_stopped_daemon(app, &client).await {
            recovery_errors.push(format!("daemon recovery failed: {error}"));
        }
    }
    let message = if recovery_errors.is_empty() {
        message
    } else {
        format!("{message} ({})", recovery_errors.join("; "))
    };
    // Report the installer failure before reconnecting: relay backoff may take
    // minutes, and a timed-out cleanup may still own the controller lock.
    let recovery_app = app.clone();
    let recovery_state = state.clone();
    report_then_recover(
        || report_install_failure(app, state, version, message),
        async move {
            if let Some(error) = resume_saved_remote_access(&recovery_app, &recovery_state).await {
                log::error!("[updater] remote access recovery failed: {error}");
            }
        },
    );
}

fn report_then_recover(
    report: impl FnOnce(),
    recovery: impl std::future::Future<Output = ()> + Send + 'static,
) {
    report();
    tauri::async_runtime::spawn(recovery);
}

async fn check_once_after_begin(
    app: &AppHandle,
    state: &SharedLifecycleState,
    trigger: CheckTrigger,
) {
    set_status(app, state, KnownStatus::checking());

    let updater = match configure_network(app.updater_builder()).build() {
        Ok(updater) => updater,
        Err(error) => {
            let message = error.to_string();
            log::warn!("updater unavailable: {message}");
            set_status(app, state, KnownStatus::error(message));
            set_phase(state, LifecyclePhase::Idle);
            return;
        }
    };

    let update = match updater.check().await {
        Ok(Some(update)) => update,
        Ok(None) => {
            set_status(app, state, KnownStatus::current());
            set_phase(state, LifecyclePhase::Idle);
            return;
        }
        Err(error) => {
            let message = error.to_string();
            log::warn!("update check failed: {message}");
            set_status(app, state, KnownStatus::error(message));
            set_phase(state, LifecyclePhase::Idle);
            return;
        }
    };

    let version = update.version.clone();
    // Retain only the version while the user considers the offer. The Update
    // itself carries a release-specific URL that may age while the prompt is
    // open; the accepted path resolves a fresh Update before installing.
    drop(update);
    let suppressed = !should_prompt(trigger, was_recently_dismissed(app, &version));
    if !suppressed {
        // Mark the prompt pending before publishing availability so a
        // ready-handshake arriving in this tiny window can still replay it.
        set_phase(state, LifecyclePhase::Prompting);
    }
    set_status(
        app,
        state,
        KnownStatus {
            state: StatusState::Available,
            version: Some(version.clone()),
            error: None,
        },
    );

    if suppressed {
        log::info!("update v{version} suppressed (dismissed within 24h)");
        set_phase(state, LifecyclePhase::Idle);
        return;
    }

    let accepted = prompt_via_overlay(app, &version).await;

    if !accepted {
        record_dismissal(app, &version);
        set_phase(state, LifecyclePhase::Idle);
        return;
    }

    set_phase(state, LifecyclePhase::Checking);
    let mut update = match refresh_update(&updater).await {
        Ok(Some(update)) => update,
        Ok(None) => {
            set_status(app, state, KnownStatus::current());
            set_phase(state, LifecyclePhase::Idle);
            return;
        }
        Err(error) => {
            log::warn!("update recheck failed: {error}");
            set_status(app, state, KnownStatus::error(error));
            set_phase(state, LifecyclePhase::Idle);
            return;
        }
    };
    let version = update.version.clone();
    set_status(
        app,
        state,
        KnownStatus {
            state: StatusState::Available,
            version: Some(version.clone()),
            error: None,
        },
    );
    set_phase(state, LifecyclePhase::Installing);
    let app_chunk = app.clone();
    let on_chunk = move |chunk_len: usize, total: Option<u64>| {
        let _ = app_chunk.emit(
            "updater://progress",
            serde_json::json!({
                "chunk": chunk_len,
                "total": total,
            }),
        );
    };
    let app_done = app.clone();
    let on_done = move || {
        let _ = app_done.emit("updater://progress", serde_json::json!({ "done": true }));
    };

    // Update starts with timeout None even when the builder timeout is set, so
    // bind the package budget here; configure_client carries over for body reads.
    configure_download(&mut update);
    let bytes = match update.download(on_chunk, on_done).await {
        Ok(bytes) => bytes,
        Err(error) => {
            report_install_failure(app, state, &version, error.to_string());
            return;
        }
    };

    // Once acquired, actual remote and daemon spawn decisions are inhibited
    // through installer handoff, including commands already in flight.
    let install_lease = match tokio::time::timeout(
        Duration::from_secs(8),
        crate::remote_access::acquire_update_install_lease(app),
    )
    .await
    {
        Ok(Ok(lease)) => lease,
        Ok(Err(error)) => {
            report_install_failure(app, state, &version, error);
            return;
        }
        Err(_) => {
            report_install_failure(
                app,
                state,
                &version,
                "Remote access is still busy; try updating again.".into(),
            );
            return;
        }
    };

    if let Err(error) = crate::remote_access::shutdown_for_exit(app).await {
        drop(install_lease);
        report_failure_after_cleanup(app, state, &version, error, false).await;
        return;
    }

    let (outcome, stop_ticket) = crate::daemon_start::stop_sidecar_with_ticket().await;
    if let Some(mut message) = update_restart_block_reason(&outcome) {
        drop(install_lease);
        if let Some(ticket) = stop_ticket {
            let client = crate::api::WenlanClient::new();
            if let Err(error) = recover_after_original_exit(
                crate::daemon_start::wait_for_stopped_sidecar(ticket, DAEMON_RECOVERY_TIMEOUT),
                restore_stopped_daemon(app, &client),
            )
            .await
            {
                message = format!("{message} (daemon recovery failed: {error})");
            }
        }
        report_failure_after_cleanup(app, state, &version, message, false).await;
        return;
    }

    if let Err(error) = update.install(bytes) {
        drop(install_lease);
        let sidecar_was_stopped = matches!(outcome, crate::daemon_start::SidecarStopOutcome::Ended);
        report_failure_after_cleanup(app, state, &version, error.to_string(), sidecar_was_stopped)
            .await;
        return;
    }

    tokio::time::sleep(Duration::from_millis(800)).await;
    app.restart();
}

async fn check_once(app: &AppHandle, state: &SharedLifecycleState, trigger: CheckTrigger) {
    if !begin_check(state) {
        return;
    }
    check_once_after_begin(app, state, trigger).await;
}

async fn next_check_trigger(
    interval: &mut tokio::time::Interval,
    check_rx: &mut tokio::sync::mpsc::Receiver<CheckTrigger>,
    state: &SharedLifecycleState,
) -> CheckTrigger {
    loop {
        tokio::select! {
            _ = interval.tick() => {
                // Claim the lifecycle before draining a request that raced
                // with the scheduled tick. This guarantees one check, with a
                // manual request winning over the scheduled trigger.
                if begin_check(state) {
                    let mut trigger = CheckTrigger::Scheduled;
                    if check_rx.try_recv().is_ok() {
                        trigger = CheckTrigger::Manual;
                        while check_rx.try_recv().is_ok() {}
                    }
                    return trigger;
                }
            }
            result = check_rx.recv() => {
                match result {
                    Some(trigger) => {
                        if begin_check(state) {
                            while check_rx.try_recv().is_ok() {}
                            return trigger;
                        }
                    }
                    None => {
                        // Sender dropped (tests only); wait for the schedule
                        // instead of spinning on a closed channel. The outer
                        // loop retries until begin_check succeeds.
                        interval.tick().await;
                        if begin_check(state) {
                            return CheckTrigger::Scheduled;
                        }
                    }
                }
            }
        }
    }
}

/// Run the serialized updater lifecycle. The first check waits briefly for the
/// main webview, then checks every 30 minutes. The frontend can request an
/// immediate check with `updater://check-now`; requests received while a check,
/// prompt, or install is active are dropped instead of queued.
pub async fn check_and_prompt(app: AppHandle) {
    if !updater_enabled_for_environment(
        cfg!(debug_assertions),
        crate::identity_paths::production_app_data_dir().as_deref(),
    ) {
        let status = KnownStatus {
            state: StatusState::Error,
            version: None,
            error: Some(
                "Update checks are disabled for development or custom data directories".to_string(),
            ),
        };
        log::warn!("[updater] skipping update check: development or custom data-dir path");
        emit_status(&app, &status);

        // Keep the frontend's manual check control from remaining in a
        // checking state when an isolated development run is active. These
        // listeners intentionally never call the real updater.
        let app_for_ready = app.clone();
        let status_for_ready = status.clone();
        app.listen("updater://ui-ready", move |_| {
            emit_status(&app_for_ready, &status_for_ready);
        });
        let app_for_check = app.clone();
        app.listen("updater://check-now", move |_| {
            emit_status(&app_for_check, &status);
        });
        return;
    }

    if UPDATER_STARTED.swap(true, Ordering::AcqRel) {
        log::debug!("[updater] lifecycle already running; ignoring duplicate start");
        return;
    }

    let state = Arc::new(Mutex::new(LifecycleState {
        phase: LifecyclePhase::Starting,
        status: KnownStatus::checking(),
    }));

    // Keep the latest status replayable for the webview. This listener is set
    // up before the startup delay so a webview loaded early still receives the
    // known state through its ready handshake.
    let app_for_ready = app.clone();
    let state_for_ready = Arc::clone(&state);
    app.listen("updater://ui-ready", move |_| {
        replay_status(&app_for_ready, &state_for_ready);
    });

    let (check_tx, mut check_rx) = tokio::sync::mpsc::channel::<CheckTrigger>(1);
    let app_for_check = app.clone();
    let state_for_check = Arc::clone(&state);
    app.listen("updater://check-now", move |_| {
        if accept_manual_check(&state_for_check) && check_tx.try_send(CheckTrigger::Manual).is_ok()
        {
            return;
        }
        replay_status(&app_for_check, &state_for_check);
    });

    // Emit the initial known state for any already-mounted consumers. The
    // ready handshake above also replays it after listeners are registered.
    emit_status(&app, &KnownStatus::checking());
    tokio::time::sleep(STARTUP_DELAY).await;
    set_phase(&state, LifecyclePhase::Idle);
    check_once(&app, &state, CheckTrigger::Startup).await;
    let mut interval = tokio::time::interval(CHECK_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Consume the immediate first tick; subsequent ticks are 30 minutes apart.
    interval.tick().await;

    loop {
        let trigger = next_check_trigger(&mut interval, &mut check_rx, &state).await;
        check_once_after_begin(&app, &state, trigger).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn recovery_cannot_accept_health_until_the_original_stop_finishes() {
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let restored = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mark = restored.clone();
        let recovery = tokio::spawn(async move {
            recover_after_original_exit(
                async { release_rx.await.map_err(|e| e.to_string()) },
                async {
                    mark.store(true, std::sync::atomic::Ordering::SeqCst);
                    Ok(())
                },
            )
            .await
        });
        tokio::task::yield_now().await;
        assert!(!restored.load(std::sync::atomic::Ordering::SeqCst));
        release_tx.send(()).unwrap();
        recovery.await.unwrap().unwrap();
        assert!(restored.load(std::sync::atomic::Ordering::SeqCst));
        let called = std::sync::atomic::AtomicBool::new(false);
        assert!(
            recover_after_original_exit(async { Err("unknown original".into()) }, async {
                called.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            },)
            .await
            .is_err()
        );
        assert!(!called.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn installer_failure_is_reported_before_slow_remote_recovery() {
        let reported = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = reported.clone();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        report_then_recover(
            || flag.store(true, std::sync::atomic::Ordering::SeqCst),
            async move {
                let _ = release_rx.await;
                let _ = done_tx.send(());
            },
        );
        assert!(reported.load(std::sync::atomic::Ordering::SeqCst));
        let mut done_rx = done_rx;
        assert!(matches!(
            done_rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
        release_tx.send(()).unwrap();
        done_rx.await.unwrap();
    }
    use crate::test_env::EnvGuard;

    #[test]
    fn manual_checks_are_accepted_only_when_idle() {
        assert!(!LifecyclePhase::Starting.accepts_manual_check());
        assert!(LifecyclePhase::Idle.accepts_manual_check());
        assert!(!LifecyclePhase::Checking.accepts_manual_check());
        assert!(!LifecyclePhase::Prompting.accepts_manual_check());
        assert!(!LifecyclePhase::Installing.accepts_manual_check());
    }

    #[test]
    fn update_restart_requires_the_sidecar_to_be_gone_or_unowned() {
        use crate::daemon_start::SidecarStopOutcome;

        assert_eq!(
            update_restart_block_reason(&SidecarStopOutcome::NoSidecar),
            None
        );
        assert_eq!(
            update_restart_block_reason(&SidecarStopOutcome::Ended),
            None
        );
        assert!(
            update_restart_block_reason(&SidecarStopOutcome::StillRunning {
                reason: "still listening".to_string(),
            })
            .unwrap()
            .contains("still listening")
        );
        assert!(
            update_restart_block_reason(&SidecarStopOutcome::CouldNotMeasure {
                reason: "process table unavailable".to_string(),
            })
            .unwrap()
            .contains("process table unavailable")
        );

        // The caller returns through report_install_failure, which restores
        // Idle so an explicit retry can be accepted after a blocked restart.
        assert!(LifecyclePhase::Idle.accepts_manual_check());
    }

    #[test]
    fn daemon_recovery_requires_the_running_release_version() {
        assert!(check_recovered_daemon_version(env!("CARGO_PKG_VERSION")).is_ok());
        assert!(
            check_recovered_daemon_version(&format!("{}+fixture", env!("CARGO_PKG_VERSION")))
                .is_ok()
        );
        let error = check_recovered_daemon_version("0.0.0").unwrap_err();
        assert!(error.contains("daemon reports version 0.0.0"));
        assert!(error.contains(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn daemon_recovery_requires_a_ready_health_response() {
        let version = env!("CARGO_PKG_VERSION").to_string();
        let ready = wenlan_types::responses::HealthResponse {
            status: "ok".into(),
            db_initialized: true,
            version: version.clone(),
        };
        assert!(validate_recovered_daemon_health(&ready).is_ok());

        let uninitialized = wenlan_types::responses::HealthResponse {
            status: "ok".into(),
            db_initialized: false,
            version: version.clone(),
        };
        assert!(validate_recovered_daemon_health(&uninitialized)
            .unwrap_err()
            .contains("database is not initialized"));

        let unhealthy = wenlan_types::responses::HealthResponse {
            status: "starting".into(),
            db_initialized: true,
            version,
        };
        assert!(validate_recovered_daemon_health(&unhealthy)
            .unwrap_err()
            .contains("expected ok"));
    }

    #[test]
    fn release_default_and_selected_legacy_roots_allow_update_checks() {
        let root = tempfile::tempdir().unwrap();
        let current = root.path().join("wenlan");
        let legacy = root.path().join("origin");
        std::fs::create_dir_all(&current).unwrap();
        std::fs::create_dir_all(&legacy).unwrap();

        assert!(updater_enabled_for_paths(false, Some(&current), None));
        assert!(updater_enabled_for_paths(
            false,
            Some(&current),
            Some(&current)
        ));
        assert!(updater_enabled_for_paths(
            false,
            Some(&legacy),
            Some(&legacy)
        ));
    }

    #[test]
    fn release_explicit_production_root_allows_first_launch_before_directory_creation() {
        let profile = tempfile::tempdir().unwrap();
        let missing = profile.path().join("new-profile").join("wenlan");
        assert!(updater_enabled_for_paths(
            false,
            Some(&missing),
            Some(&missing)
        ));
        assert!(!updater_enabled_for_paths(
            true,
            Some(&missing),
            Some(&missing)
        ));
        assert!(!updater_enabled_for_paths(
            false,
            Some(&missing),
            Some(&profile.path().join("scratch"))
        ));
        assert!(!missing.exists(), "eligibility must not create directories");
    }

    #[cfg(unix)]
    #[test]
    fn unresolved_symlinks_do_not_establish_path_equivalence() {
        let profile = tempfile::tempdir().unwrap();
        let broken = profile.path().join("broken");
        std::os::unix::fs::symlink(profile.path().join("absent"), &broken).unwrap();
        assert!(!updater_enabled_for_paths(
            false,
            Some(&broken),
            Some(&broken)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn release_accepts_a_symlink_alias_of_the_selected_root() {
        let root = tempfile::tempdir().unwrap();
        let current = root.path().join("wenlan");
        let alias = root.path().join("alias");
        std::fs::create_dir_all(&current).unwrap();
        std::os::unix::fs::symlink(&current, &alias).unwrap();

        assert!(updater_enabled_for_paths(
            false,
            Some(&current),
            Some(&alias)
        ));
    }

    #[test]
    fn scratch_relative_and_unresolved_roots_stay_disabled() {
        let root = tempfile::tempdir().unwrap();
        let current = root.path().join("wenlan");
        let scratch = root.path().join("scratch");
        let missing = root.path().join("missing");
        std::fs::create_dir_all(&current).unwrap();
        std::fs::create_dir_all(&scratch).unwrap();

        assert!(!updater_enabled_for_paths(
            false,
            Some(&current),
            Some(&scratch)
        ));
        assert!(!updater_enabled_for_paths(
            false,
            Some(&current),
            Some(Path::new("relative/wenlan"))
        ));
        assert!(!updater_enabled_for_paths(
            false,
            Some(&current),
            Some(&missing)
        ));
        assert!(updater_enabled_for_paths(false, None, None));
        assert!(updater_enabled_for_paths(false, Some(&missing), None));
    }

    #[test]
    #[serial_test::serial]
    fn data_dir_override_precedence_matches_identity_paths() {
        let _env = EnvGuard::capture(&["WENLAN_DATA_DIR", "ORIGIN_DATA_DIR"]);
        let root = tempfile::tempdir().unwrap();
        let current = root.path().join("wenlan");
        let scratch = root.path().join("scratch");
        std::fs::create_dir_all(&current).unwrap();
        std::fs::create_dir_all(&scratch).unwrap();

        std::env::set_var("WENLAN_DATA_DIR", &current);
        std::env::set_var("ORIGIN_DATA_DIR", &scratch);
        assert!(updater_enabled_for_environment(false, Some(&current)));

        std::env::set_var("WENLAN_DATA_DIR", &scratch);
        std::env::set_var("ORIGIN_DATA_DIR", &current);
        assert!(!updater_enabled_for_environment(false, Some(&current)));

        std::env::remove_var("WENLAN_DATA_DIR");
        assert!(updater_enabled_for_environment(false, Some(&current)));
    }

    #[test]
    #[serial_test::serial]
    fn legacy_origin_selection_is_shared_with_the_updater_reference_root() {
        let _env = EnvGuard::capture(&["WENLAN_DATA_DIR", "ORIGIN_DATA_DIR"]);
        let profile = tempfile::tempdir().unwrap();
        let _roots = crate::test_env::isolate_app_roots(profile.path());
        let legacy = profile.path().join("origin");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("config.json"), b"{}").unwrap();
        std::env::remove_var("WENLAN_DATA_DIR");
        std::env::remove_var("ORIGIN_DATA_DIR");

        let selected = crate::identity_paths::production_app_data_dir().unwrap();
        assert_eq!(selected, legacy);
        assert!(updater_enabled_for_environment(false, Some(&selected)));

        std::env::set_var("ORIGIN_DATA_DIR", &selected);
        assert!(updater_enabled_for_environment(false, Some(&selected)));
    }

    #[test]
    fn debug_builds_keep_update_checks_disabled() {
        let root = tempfile::tempdir().unwrap();
        let current = root.path().join("wenlan");
        std::fs::create_dir_all(&current).unwrap();

        assert!(!updater_enabled_for_paths(true, Some(&current), None));
        assert!(!updater_enabled_for_paths(
            true,
            Some(&current),
            Some(&current)
        ));
    }

    #[test]
    fn manual_check_bypasses_recent_dismissal() {
        assert!(!should_prompt(CheckTrigger::Startup, true));
        assert!(!should_prompt(CheckTrigger::Scheduled, true));
        assert!(should_prompt(CheckTrigger::Manual, true));
        assert!(should_prompt(CheckTrigger::Scheduled, false));
    }

    #[test]
    fn action_payload_requires_an_exact_known_action() {
        assert_eq!(parse_action("\"install\""), Some(true));
        assert_eq!(parse_action("later"), Some(false));
        assert_eq!(parse_action("please install now"), None);
        assert_eq!(parse_action("{\"action\":\"install\"}"), None);
    }

    #[test]
    fn status_payload_keeps_optional_fields_absent_when_unknown() {
        assert_eq!(
            status_payload(&KnownStatus::current()),
            serde_json::json!({
                "state": "current"
            })
        );

        assert_eq!(
            status_payload(&KnownStatus {
                state: StatusState::Error,
                version: Some("0.18.5".to_string()),
                error: Some("network unavailable".to_string()),
            }),
            serde_json::json!({
                "state": "error",
                "version": "0.18.5",
                "error": "network unavailable"
            })
        );
    }

    #[test]
    fn dismissal_file_suppresses_same_version_within_24h() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dismissal_file_path(dir.path());
        // No file yet: nothing suppressed.
        assert!(!was_recently_dismissed_at(&path, "0.18.5", 1_000_000));

        let now = 1_000_000u64;
        record_dismissal_at(&path, "0.18.5", now);
        // Read back from disk, representing a relaunch.
        assert!(was_recently_dismissed_at(&path, "0.18.5", now + 3600));
        assert!(was_recently_dismissed_at(
            &path,
            "0.18.5",
            now + SUPPRESS_TTL.as_secs() - 1
        ));
        // Different version is not suppressed.
        assert!(!was_recently_dismissed_at(&path, "0.18.6", now + 3600));
        // Expiry at exactly 24h is allowed (TTL is exclusive).
        assert!(!was_recently_dismissed_at(
            &path,
            "0.18.5",
            now + SUPPRESS_TTL.as_secs()
        ));
        // Manual checks bypass suppression; scheduled ones do not.
        assert!(should_prompt(
            CheckTrigger::Manual,
            was_recently_dismissed_at(&path, "0.18.5", now + 3600)
        ));
        assert!(!should_prompt(
            CheckTrigger::Scheduled,
            was_recently_dismissed_at(&path, "0.18.5", now + 3600)
        ));
    }

    #[test]
    fn dismissal_file_with_malformed_json_is_ignored() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dismissal_file_path(dir.path());
        std::fs::write(&path, b"{not json").expect("write malformed");
        assert!(!was_recently_dismissed_at(&path, "0.18.5", 1_000_000));
    }

    fn idle_state() -> SharedLifecycleState {
        Arc::new(Mutex::new(LifecycleState {
            phase: LifecyclePhase::Idle,
            status: KnownStatus::checking(),
        }))
    }

    #[tokio::test]
    async fn scheduled_triggers_repeat_when_idle_restored() {
        let state = idle_state();
        let mut interval = tokio::time::interval(Duration::from_millis(20));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // Consume the immediate first tick; subsequent ticks drive the schedule.
        interval.tick().await;
        let (_tx, mut rx) = tokio::sync::mpsc::channel::<CheckTrigger>(1);
        for _ in 0..3 {
            let trigger = tokio::time::timeout(
                Duration::from_secs(5),
                next_check_trigger(&mut interval, &mut rx, &state),
            )
            .await
            .expect("scheduled trigger within bound");
            assert_eq!(trigger, CheckTrigger::Scheduled);
            set_phase(&state, LifecyclePhase::Idle);
        }
    }

    #[tokio::test]
    async fn manual_trigger_is_accepted_promptly() {
        let state = idle_state();
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        interval.tick().await;
        let (tx, mut rx) = tokio::sync::mpsc::channel::<CheckTrigger>(1);
        tx.send(CheckTrigger::Manual)
            .await
            .expect("queue manual check");
        let trigger = tokio::time::timeout(
            Duration::from_secs(5),
            next_check_trigger(&mut interval, &mut rx, &state),
        )
        .await
        .expect("manual trigger within bound");
        assert_eq!(trigger, CheckTrigger::Manual);
    }

    #[tokio::test]
    async fn queued_manual_wins_over_ready_scheduled_tick() {
        let state = idle_state();
        let mut interval = tokio::time::interval(Duration::from_millis(20));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        interval.tick().await;
        let (tx, mut rx) = tokio::sync::mpsc::channel::<CheckTrigger>(1);
        // Let a scheduled tick become pending, then queue a manual request so
        // both sources are ready when the helper selects.
        tokio::time::sleep(Duration::from_millis(50)).await;
        tx.try_send(CheckTrigger::Manual)
            .expect("queue manual check");
        let trigger = tokio::time::timeout(
            Duration::from_secs(5),
            next_check_trigger(&mut interval, &mut rx, &state),
        )
        .await
        .expect("trigger within bound");
        assert_eq!(trigger, CheckTrigger::Manual);
    }
}

#[cfg(all(test, target_os = "macos"))]
#[path = "updater_native_tests.rs"]
mod native_tests;
