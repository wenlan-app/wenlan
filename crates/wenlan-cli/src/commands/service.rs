// SPDX-License-Identifier: Apache-2.0
//! Cross-platform service registration for the Wenlan daemon.
//!
//! - macOS: launchd LaunchAgent via the `service-manager` crate.
//! - Linux: systemd --user unit via the `service-manager` crate.
//! - Windows: per-user ONLOGON Task Scheduler entry via `schtasks.exe`.
//!   We bypass `service-manager`'s `ScServiceManager` because wenlan-server
//!   is a plain console app and does not implement the Windows Service
//!   Control Protocol (`sc start` would time out at 30s with error 1053).

use anyhow::{Context, Result};
use service_manager::ServiceLabel;
#[cfg(not(target_os = "macos"))]
use service_manager::{ServiceInstallCtx, ServiceLevel, ServiceManager, ServiceStartCtx};
use std::path::{Path, PathBuf};

#[cfg(windows)]
#[path = "windows_task.rs"]
mod windows_task;

#[path = "service_binding.rs"]
mod service_binding;
use service_binding::OwnershipRefusal;

use crate::client::origin_host_from_env;
use crate::client::recovery::poll_health;

pub const SERVICE_LABEL: &str = "com.wenlan.server";
const DEFAULT_LOCAL_BIND_ADDR: &str = "127.0.0.1:7878";
const SHUTDOWN_PROBE_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);
const SHUTDOWN_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(250);
const SHUTDOWN_STABILITY_WINDOW: std::time::Duration = std::time::Duration::from_secs(1);
const SHUTDOWN_VERIFY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);
const DAEMON_PROCESS_ID_HEADER: &str = "x-wenlan-process-id";
const AUTOSTART_OFF_MARKER: &str = "autostart.off";
// Windows loopback refusal can take over two seconds to reach WSAECONNREFUSED.
// A shorter deadline hides measured absence behind an inconclusive timeout.
const OWNERSHIP_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
/// How long `background on` waits for `/api/health` before reporting.
const FIRST_HEALTH_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

#[derive(Clone, Copy, Debug)]
struct DaemonProcessIdentity {
    pid: sysinfo::Pid,
    started_at: Option<u64>,
}

impl DaemonProcessIdentity {
    fn capture(raw_pid: u32) -> Self {
        let pid = sysinfo::Pid::from_u32(raw_pid);
        let mut system = sysinfo::System::new();
        system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
        Self {
            pid,
            started_at: system.process(pid).map(sysinfo::Process::start_time),
        }
    }

    fn is_running(self, system: &mut sysinfo::System) -> bool {
        system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[self.pid]), true);
        match (self.started_at, system.process(self.pid)) {
            (Some(started_at), Some(process)) => process.start_time() == started_at,
            // The process had already exited by the time the CLI captured its
            // identity. Do not let PID reuse turn that completed exit back
            // into a live match.
            (None, _) | (_, None) => false,
        }
    }
}

enum DaemonShutdownRequest {
    NotRunning,
    Requested(Option<DaemonProcessIdentity>),
}

/// Windows Task Scheduler does not love dots in task names. The macOS launchd
/// and systemd-user paths still use the canonical reverse-DNS `SERVICE_LABEL`.
#[cfg(target_os = "windows")]
pub const WINDOWS_TASK_NAME: &str = "WenlanServer";

#[cfg(windows)]
fn windows_task_name() -> String {
    let root = explicit_data_root();
    if root.as_deref().is_some_and(|root| {
        dirs::data_local_dir()
            .is_some_and(|base| windows_task::same_root(root, &base.join("wenlan")))
    }) {
        return WINDOWS_TASK_NAME.into();
    }
    windows_task::task_name(root.as_deref())
}

#[derive(clap::Subcommand)]
pub enum BackgroundCommand {
    /// Start Wenlan now and keep it running in the background after login.
    On,
    /// Stop Wenlan now while keeping its background registration.
    Off,
}

pub async fn run_background(command: BackgroundCommand) -> Result<()> {
    match command {
        BackgroundCommand::On => install().await,
        BackgroundCommand::Off => stop().await,
    }
}

fn label() -> Result<ServiceLabel> {
    SERVICE_LABEL.parse().context("invalid service label")
}

#[cfg(target_os = "windows")]
fn run_schtasks(args: &[&str], action: &str) -> Result<std::process::Output> {
    let output = std::process::Command::new("schtasks.exe")
        .args(args)
        .output()
        .with_context(|| format!("spawn schtasks.exe ({action})"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        anyhow::bail!(
            "schtasks.exe {} failed (exit {}): {}{}",
            action,
            output.status.code().unwrap_or(-1),
            stderr.trim(),
            if stdout.trim().is_empty() {
                String::new()
            } else {
                format!("\nstdout: {}", stdout.trim())
            }
        );
    }
    Ok(output)
}

#[cfg(not(target_os = "macos"))]
fn manager() -> Result<Box<dyn ServiceManager>> {
    // macOS + Linux only. Windows install/stop short-circuit before
    // calling this and drive schtasks.exe directly (see install/stop).
    let mut m = <dyn ServiceManager>::native().context("detect native service manager")?;
    let _ = m.set_level(ServiceLevel::User);
    Ok(m)
}

/// Configuration is not runtime evidence. No supervisor mutation is allowed
/// while its instance is active, queued, or unmeasurable.
fn verify_registered_stopped() -> Result<()> {
    verify_registered_root()?;
    #[cfg(windows)]
    {
        let output = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", "$ErrorActionPreference='Stop'; $service=New-Object -ComObject Schedule.Service; $service.Connect(); $task=$service.GetFolder('\\').GetTask($env:WENLAN_TASK_OWNERSHIP_NAME); @{state=[uint32]$task.State; instances=[uint32]$task.GetInstances(0).Count} | ConvertTo-Json -Compress"])
            .env("WENLAN_TASK_OWNERSHIP_NAME", windows_task_name())
            .output().context("read scheduled task runtime state")?;
        if !output.status.success() {
            return Err(
                OwnershipRefusal("scheduled task runtime could not be measured".into()).into(),
            );
        }
        #[derive(serde::Deserialize)]
        struct Runtime {
            state: u32,
            instances: u32,
        }
        let runtime: Runtime = serde_json::from_slice(&output.stdout)?;
        service_binding::verify_windows_stopped(runtime.state, runtime.instances)?;
    }
    #[cfg(target_os = "linux")]
    {
        let unit = format!("{}.service", label()?.to_script_name());
        let output = std::process::Command::new("systemctl")
            .args([
                "--user",
                "show",
                &unit,
                "--property=ActiveState",
                "--property=MainPID",
            ])
            .output()
            .context("read systemd runtime state")?;
        if !output.status.success() {
            return Err(OwnershipRefusal("systemd runtime could not be measured".into()).into());
        }
        service_binding::verify_systemd_stopped(&String::from_utf8(output.stdout)?)?;
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(output) = loaded_launchd_job()? {
            service_binding::verify_launchd_stopped(&output, &wenlan_core::config::data_root())?;
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn loaded_launchd_job() -> Result<Option<String>> {
    let target = format!("gui/{}/{}", current_user_id()?, SERVICE_LABEL);
    let output = std::process::Command::new("launchctl")
        .args(["print", &target])
        .output()?;
    if output.status.success() {
        return Ok(Some(String::from_utf8(output.stdout)?));
    }
    if output.status.code() == Some(113) {
        return Ok(None);
    }
    Err(OwnershipRefusal("loaded launchd job could not be measured".into()).into())
}

#[cfg(target_os = "macos")]
fn require_unloaded_launchd_job() -> Result<()> {
    if loaded_launchd_job()?.is_some() {
        return Err(OwnershipRefusal("launchd job is still loaded; unload the verified old registration manually before replacing it".into()).into());
    }
    Ok(())
}

/// A reachable daemon and a globally named service can belong to different
/// stores. Prove the registration independently before any supervisor action.
fn verify_registered_root() -> Result<()> {
    #[cfg(windows)]
    {
        // A task name alone is not proof: an old or manually edited task may
        // point at another store. Inspect its actual action without changing it.
        let output = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", "$ErrorActionPreference='Stop'; [Console]::OutputEncoding=[Text.UTF8Encoding]::new($false); $service=New-Object -ComObject Schedule.Service; $service.Connect(); $task=$service.GetFolder('\\').GetTask($env:WENLAN_TASK_OWNERSHIP_NAME); $actions=$task.Definition.Actions; if($actions.Count -ne 1 -or $actions.Item(1).Type -ne 0){throw 'unverified task actions'}; @{arguments=$actions.Item(1).Arguments} | ConvertTo-Json -Compress"])
            .env("WENLAN_TASK_OWNERSHIP_NAME", windows_task_name())
            .output().context("read scheduled task ownership")?;
        if !output.status.success() {
            return Err(
                OwnershipRefusal("scheduled task action could not be verified".into()).into(),
            );
        }
        #[derive(serde::Deserialize)]
        struct TaskAction {
            arguments: String,
        }
        let action: TaskAction = serde_json::from_slice(&output.stdout)
            .map_err(|_| OwnershipRefusal("scheduled task action is unreadable".into()))?;
        let arguments = windows_task::parse_arguments(&action.arguments)?;
        service_binding::verify_launch_root(&wenlan_core::config::data_root(), &arguments, &[])?;
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        let value = plist::Value::from_file(service_unit_path()?)
            .context("read registered LaunchAgent ownership")?;
        let dict = value
            .as_dictionary()
            .context("LaunchAgent is not a dictionary")?;
        let args: Vec<String> = dict
            .get("ProgramArguments")
            .and_then(plist::Value::as_array)
            .context("LaunchAgent has no ProgramArguments")?
            .iter()
            .map(|value| {
                value
                    .as_string()
                    .map(str::to_owned)
                    .context("LaunchAgent argument is not a string")
            })
            .collect::<Result<_>>()?;
        let environment = dict
            .get("EnvironmentVariables")
            .and_then(plist::Value::as_dictionary)
            .map(|env| {
                env.iter()
                    .filter_map(|(key, value)| {
                        value
                            .as_string()
                            .map(|value| (key.clone(), value.to_owned()))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        service_binding::verify_launch_root(
            &wenlan_core::config::data_root(),
            &args,
            &environment,
        )?;
        Ok(())
    }
    #[cfg(target_os = "linux")]
    {
        let unit = format!("{}.service", label()?.to_script_name());
        let output = std::process::Command::new("systemctl")
            .args([
                "--user",
                "show",
                &unit,
                "--property=Environment",
                "--property=ExecStart",
                "--property=EnvironmentFiles",
            ])
            .output()
            .context("read effective systemd service ownership")?;
        if !output.status.success() {
            return Err(OwnershipRefusal(
                "systemd could not prove the registered data root".into(),
            )
            .into());
        }
        verify_systemd_root(
            &String::from_utf8(output.stdout)?,
            &wenlan_core::config::data_root(),
        )
    }
}

#[cfg(any(target_os = "linux", test))]
fn verify_systemd_root(properties: &str, selected: &Path) -> Result<()> {
    // EnvironmentFiles and command-line overrides need full systemd parsing;
    // refuse unknown registrations rather than guessing their effective root.
    let mut environment = Vec::new();
    for line in properties.lines() {
        if let Some(value) = line.strip_prefix("Environment=") {
            let tokens = shlex::split(value)
                .ok_or_else(|| OwnershipRefusal("systemd environment is ambiguous".into()))?;
            environment.extend(tokens.into_iter().filter_map(|token| {
                token
                    .split_once('=')
                    .map(|(key, value)| (key.to_owned(), value.to_owned()))
            }));
        } else if line
            .strip_prefix("EnvironmentFiles=")
            .is_some_and(|value| !value.trim().is_empty())
            || (line.starts_with("ExecStart=") && line.contains("--data-dir"))
        {
            return Err(OwnershipRefusal(
                "systemd registration has unverified root overrides".into(),
            )
            .into());
        }
    }
    service_binding::verify_launch_root(selected, &[], &environment)?;
    Ok(())
}

/// Resolves the platform-specific path to the Wenlan service unit file.
///
/// Mirrors the on-disk path that `service-manager` 0.11 actually writes:
/// - macOS (launchd): `~/Library/LaunchAgents/<qualified_name>.plist`
///   (`to_qualified_name()` keeps the qualifier, e.g. `com.wenlan.server.plist`).
/// - Linux (systemd-user): `<config_dir>/systemd/user/<script_name>.service`
///   (`ServiceLabel::to_script_name()` joins org+app with `-` and DROPS the
///   qualifier, so `com.wenlan.server` becomes `wenlan-server.service`).
/// - Windows: no on-disk unit file. The scheduled task lives in the Task
///   Scheduler database — see `is_installed()` for the schtasks-based probe.
#[cfg(not(target_os = "windows"))]
pub fn service_unit_path() -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        Ok(dirs::home_dir()
            .context("HOME not set")?
            .join("Library/LaunchAgents")
            .join(format!("{}.plist", SERVICE_LABEL)))
    }
    #[cfg(target_os = "linux")]
    {
        let label = label()?;
        Ok(dirs::config_dir()
            .context("XDG_CONFIG_HOME not set")?
            .join("systemd/user")
            .join(format!("{}.service", label.to_script_name())))
    }
}

fn current_server_path() -> Result<PathBuf> {
    let origin_exe = std::env::current_exe().context("cannot determine origin CLI path")?;
    let mut server = origin_exe
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("wenlan-server");
    if cfg!(target_os = "windows") {
        server.set_extension("exe");
    }
    if !server.exists() {
        anyhow::bail!(
            "wenlan-server not found next to wenlan at {}. If you installed with Homebrew, \
             run `brew upgrade 7xuanlu/tap/wenlan` (the current formula ships the daemon); \
             otherwise re-run the installer with `npx wenlan setup`.",
            server.display()
        );
    }
    Ok(server)
}

fn autostart_off_marker_path() -> PathBuf {
    wenlan_core::config::data_root().join(AUTOSTART_OFF_MARKER)
}

pub(crate) fn autostart_off_marker_exists() -> bool {
    autostart_off_marker_path().is_file()
}

fn write_autostart_off_marker() -> Result<PathBuf> {
    let path = autostart_off_marker_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create Wenlan data root for {}", parent.display()))?;
    }
    std::fs::write(&path, b"")
        .with_context(|| format!("write autostart marker {}", path.display()))?;
    Ok(path)
}

fn remove_autostart_off_marker() -> Result<()> {
    let path = autostart_off_marker_path();
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => {
            Err(error).with_context(|| format!("remove autostart marker {}", path.display()))
        }
    }
}

#[cfg(target_os = "macos")]
fn origin_data_root() -> PathBuf {
    wenlan_core::config::data_root()
}

/// Builds a launchd plist that mirrors `service-manager`'s default output for
/// `OnFailure` restart + user-level + autostart, with the extra keys the old
/// embedded `com.wenlan.server.plist` template carried: stdout/stderr routing,
/// `EnvironmentVariables.RUST_LOG`, and the canonical `WENLAN_DATA_DIR`
/// ownership marker consumed by the desktop app. The daemon owns bounded file
/// logging, so launchd's stdout goes to `/dev/null`; stderr goes to
/// `<data root>/logs/launchd-stderr.log` so a crash before file logging is up
/// (a missing library, an abort) is never silent. launchd does not rotate
/// that file: the daemon writes only bootstrap failures there (progress bars
/// are hidden off a terminal), so it grows a couple of lines per attempt only
/// while launchd keeps retrying a daemon that cannot start.
///
/// `LaunchdInstallConfig` in service-manager 0.11 only exposes `keep_alive`;
/// stdout/stderr paths must come through `ServiceInstallCtx.contents` as a
/// pre-rendered plist string. This function is the minimal stand-in for the
/// crate's internal `make_plist`.
#[cfg(target_os = "macos")]
fn build_launchd_plist(
    program: &Path,
    stdout_path: &Path,
    stderr_path: &Path,
    rust_log: &str,
    data_root: &Path,
) -> String {
    let mut buf = String::new();
    buf.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    buf.push_str(
        "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n",
    );
    buf.push_str("<plist version=\"1.0\">\n<dict>\n");
    buf.push_str("\t<key>Label</key>\n");
    buf.push_str(&format!("\t<string>{}</string>\n", SERVICE_LABEL));
    buf.push_str("\t<key>ProgramArguments</key>\n");
    buf.push_str("\t<array>\n");
    buf.push_str(&format!(
        "\t\t<string>{}</string>\n",
        program.to_string_lossy()
    ));
    buf.push_str("\t</array>\n");
    // Mirrors service-manager's RestartPolicy::OnFailure rendering: KeepAlive
    // dict with SuccessfulExit=false. The matching `Disabled` key keeps the
    // service from auto-loading until start() removes it (cross-platform parity).
    buf.push_str("\t<key>KeepAlive</key>\n");
    buf.push_str("\t<dict>\n");
    buf.push_str("\t\t<key>SuccessfulExit</key>\n");
    buf.push_str("\t\t<false/>\n");
    buf.push_str("\t</dict>\n");
    buf.push_str("\t<key>RunAtLoad</key>\n\t<true/>\n");
    buf.push_str("\t<key>Disabled</key>\n\t<true/>\n");
    buf.push_str("\t<key>StandardOutPath</key>\n");
    buf.push_str(&format!(
        "\t<string>{}</string>\n",
        stdout_path.to_string_lossy()
    ));
    buf.push_str("\t<key>StandardErrorPath</key>\n");
    buf.push_str(&format!(
        "\t<string>{}</string>\n",
        stderr_path.to_string_lossy()
    ));
    buf.push_str("\t<key>EnvironmentVariables</key>\n");
    buf.push_str("\t<dict>\n");
    buf.push_str("\t\t<key>RUST_LOG</key>\n");
    buf.push_str(&format!("\t\t<string>{}</string>\n", rust_log));
    buf.push_str("\t\t<key>WENLAN_DATA_DIR</key>\n");
    buf.push_str(&format!(
        "\t\t<string>{}</string>\n",
        data_root.to_string_lossy()
    ));
    buf.push_str("\t</dict>\n");
    buf.push_str("</dict>\n</plist>\n");
    buf
}

/// The data root this CLI run was told to use, if any: `WENLAN_DATA_DIR` (or
/// its legacy `ORIGIN_DATA_DIR` spelling) in the CLI's own environment, which
/// is also how the desktop app hands over its selected store.
#[cfg(any(target_os = "macos", windows))]
fn explicit_data_root() -> Option<PathBuf> {
    wenlan_core::env_compat::var_compat("WENLAN_DATA_DIR").map(PathBuf::from)
}

/// The plist `background on` registers: a fresh one when none exists, else
/// the existing file with the CLI-owned keys brought up to date.
///
/// Ownership rule for an existing `com.wenlan.server.plist` (re-registering
/// over one is the upgrade path, and the user or the desktop app may have
/// edited it since):
/// - The CLI owns the launch mechanics and rewrites them every time: `Label`,
///   `ProgramArguments`, `KeepAlive`, `RunAtLoad`, `Disabled`,
///   `StandardOutPath`, `StandardErrorPath`.
/// - Every other top-level key and every `EnvironmentVariables` entry belongs
///   to whoever put it there and is kept as is (a full rewrite on 2026-08-16
///   dropped the user's `WENLAN_ENABLE_EDGE_GROUNDING_PROMOTE=1`).
/// - `RUST_LOG` is seeded to `info` only when absent.
/// - `WENLAN_DATA_DIR` is written only when this run was given one explicitly
///   (see [`explicit_data_root`]); otherwise an existing value stays and a
///   missing one stays missing, so a plist that relies on the platform
///   default keeps doing so instead of gaining a pinned copy of it.
///
/// A file that does not parse as a plist dictionary carries nothing worth
/// keeping and is replaced by a fresh one; the CLI says so on stderr.
#[cfg(target_os = "macos")]
fn launchd_plist_contents(
    existing: Option<&str>,
    program: &Path,
    stderr_log: &Path,
    data_root: &Path,
    explicit_data_root: Option<&Path>,
) -> String {
    let stdout = Path::new("/dev/null");
    let fresh = || build_launchd_plist(program, stdout, stderr_log, "info", data_root);
    let Some(existing) = existing else {
        return fresh();
    };
    match merge_launchd_plist(existing, program, stdout, stderr_log, explicit_data_root) {
        Ok(merged) => merged,
        Err(error) => {
            eprintln!(
                "The existing {SERVICE_LABEL} plist could not be read ({error:#}); writing a fresh one."
            );
            fresh()
        }
    }
}

/// Applies the ownership rule in [`launchd_plist_contents`] to an existing
/// plist and renders the result.
#[cfg(target_os = "macos")]
fn merge_launchd_plist(
    existing: &str,
    program: &Path,
    stdout_path: &Path,
    stderr_path: &Path,
    explicit_data_root: Option<&Path>,
) -> Result<String> {
    use plist::{Dictionary, Value};

    let path_value = |path: &Path| Value::String(path.to_string_lossy().into_owned());
    let mut root = match Value::from_reader_xml(existing.as_bytes()).context("parse plist")? {
        Value::Dictionary(root) => root,
        _ => anyhow::bail!("plist root is not a dictionary"),
    };
    root.insert("Label".into(), Value::String(SERVICE_LABEL.into()));
    root.insert(
        "ProgramArguments".into(),
        Value::Array(vec![path_value(program)]),
    );
    let mut keep_alive = Dictionary::new();
    keep_alive.insert("SuccessfulExit".into(), Value::Boolean(false));
    root.insert("KeepAlive".into(), Value::Dictionary(keep_alive));
    root.insert("RunAtLoad".into(), Value::Boolean(true));
    root.insert("Disabled".into(), Value::Boolean(true));
    root.insert("StandardOutPath".into(), path_value(stdout_path));
    root.insert("StandardErrorPath".into(), path_value(stderr_path));

    if root
        .get("EnvironmentVariables")
        .and_then(Value::as_dictionary)
        .is_none()
    {
        root.insert(
            "EnvironmentVariables".into(),
            Value::Dictionary(Dictionary::new()),
        );
    }
    let env = root
        .get_mut("EnvironmentVariables")
        .and_then(Value::as_dictionary_mut)
        .context("EnvironmentVariables is not a dictionary")?;
    if !env.contains_key("RUST_LOG") {
        env.insert("RUST_LOG".into(), Value::String("info".into()));
    }
    if let Some(data_root) = explicit_data_root {
        env.insert("WENLAN_DATA_DIR".into(), path_value(data_root));
    }

    let mut rendered = Vec::new();
    Value::Dictionary(root)
        .to_writer_xml(&mut rendered)
        .context("render plist")?;
    String::from_utf8(rendered).context("rendered plist is not UTF-8")
}

pub async fn install() -> Result<()> {
    if is_installed() {
        verify_registered_root()?;
    }
    #[cfg(target_os = "windows")]
    {
        // wenlan-server is a plain console app and does not speak the Windows
        // Service Control Protocol, so sc.exe install + start would time out
        // at 30s with error 1053. Use Task Scheduler instead: register a
        // per-user ONLOGON task and trigger it immediately. Matches the
        // user-scope semantics of launchd LaunchAgent (macOS) and
        // systemd --user (Linux), without needing a service dispatcher in
        // wenlan-server.
        let program = current_server_path()?;
        let program_str = program.to_str().context("server path is not Unicode")?;
        let bind = configured_bind_addr()?;
        match request_daemon_shutdown().await? {
            DaemonShutdownRequest::Requested(process) => {
                verify_local_daemon_shutdown(process).await?;
            }
            DaemonShutdownRequest::NotRunning => {}
        }
        let data_root = Some(std::path::absolute(wenlan_core::config::data_root())?);
        if let Some(root) = data_root.as_deref() {
            std::fs::create_dir_all(root).context(
                "create selected background data root before computing its task identity",
            )?;
        }
        let data_root = data_root
            .as_ref()
            .map(|path| path.to_str().context("data path is not Unicode"))
            .transpose()?;
        let xml = windows_task::task_xml(program_str, data_root, bind);
        let definition = windows_task::write_task_definition(&xml)?;
        let definition_path = definition
            .to_str()
            .context("task definition path is not Unicode")?;
        if is_installed() {
            verify_registered_stopped()?;
        }
        run_schtasks(
            &[
                "/create",
                "/tn",
                &windows_task_name(),
                "/xml",
                definition_path,
                "/f",
            ],
            "create scheduled task",
        )?;
        let log_marks = DaemonLogMarks::capture();
        run_schtasks(&["/run", "/tn", &windows_task_name()], "run scheduled task")?;
        remove_autostart_off_marker()?;
        return wait_for_first_health(
            &format!(
                "Windows scheduled task '{}' (wenlan-server)",
                windows_task_name()
            ),
            &log_marks,
        )
        .await;
    }

    #[cfg_attr(target_os = "windows", allow(unreachable_code))]
    let label_value = label()?;
    let program = current_server_path()?;
    #[cfg(not(target_os = "macos"))]
    let m = manager()?;

    // Gracefully stop any daemon that is currently answering, and wait for it
    // to fully exit, before reinstalling. Swapping the binary under a live
    // process would otherwise leave the OLD daemon running until something
    // else kills it (see restart()'s doc comment for the full story).
    match request_daemon_shutdown().await {
        Ok(DaemonShutdownRequest::Requested(process)) => {
            verify_local_daemon_shutdown(process)
                .await
                .context("graceful daemon shutdown before reinstall did not complete")?;
        }
        Ok(DaemonShutdownRequest::NotRunning) => {}
        Err(error) => {
            return Err(error).context("request graceful daemon shutdown before reinstall")
        }
    }

    if is_installed() {
        verify_registered_stopped()?;
    }
    #[cfg(target_os = "macos")]
    require_unloaded_launchd_job()?;

    // Apply RUST_LOG=info to every platform. launchd consumes
    // `EnvironmentVariables`, systemd-user consumes `Environment=`. winsw +
    // sc.exe ignore the field (Windows daemons still rely on `RUST_LOG`
    // exported in the user environment).
    let environment = Some(vec![
        ("RUST_LOG".to_string(), "info".to_string()),
        (
            "WENLAN_DATA_DIR".to_string(),
            std::path::absolute(wenlan_core::config::data_root())?
                .to_string_lossy()
                .into_owned(),
        ),
        (
            "WENLAN_BIND_ADDR".to_string(),
            configured_bind_addr()?.to_string(),
        ),
    ]);

    // macOS: hand-roll the plist so launchd does not append an unbounded
    // duplicate of the daemon-owned rotating log. service-manager 0.11 has no
    // struct field for these keys, so the only honest knob is
    // `ServiceInstallCtx.contents`.
    let contents = {
        #[cfg(target_os = "macos")]
        {
            let data_root = origin_data_root();
            let stderr_log = launchd_stderr_log_path(&data_root);
            if let Some(log_dir) = stderr_log.parent() {
                std::fs::create_dir_all(log_dir).with_context(|| {
                    format!("create the daemon log directory {}", log_dir.display())
                })?;
            }
            let plist_path = service_unit_path()?;
            let existing = match std::fs::read_to_string(&plist_path) {
                Ok(existing) => Some(existing),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => {
                    return Err(error).with_context(|| format!("read {}", plist_path.display()))
                }
            };
            Some(launchd_plist_contents(
                existing.as_deref(),
                &program,
                &stderr_log,
                &data_root,
                explicit_data_root().as_deref(),
            ))
        }
        #[cfg(not(target_os = "macos"))]
        {
            None
        }
    };

    let log_marks = DaemonLogMarks::capture();
    #[cfg(not(target_os = "macos"))]
    {
        m.install(ServiceInstallCtx {
            label: label_value.clone(),
            program,
            args: vec![],
            contents,
            username: None,
            working_directory: None,
            environment,
            autostart: true,
            restart_policy: service_manager::RestartPolicy::OnFailure {
                delay_secs: None,
                max_retries: None,
                reset_after_secs: None,
            },
        })
        .context("install service")?;

        m.start(ServiceStartCtx { label: label_value })
            .context("start service")?;
    }
    #[cfg(target_os = "macos")]
    {
        let _ = (label_value, environment);
        let mut value = plist::Value::from_reader_xml(
            contents
                .as_deref()
                .context("missing LaunchAgent contents")?
                .as_bytes(),
        )?;
        value
            .as_dictionary_mut()
            .context("invalid LaunchAgent")?
            .remove("Disabled");
        let path = service_unit_path()?;
        std::fs::create_dir_all(path.parent().context("missing LaunchAgent parent")?)?;
        value.to_file_xml(&path)?;
        bootstrap_launchd_job()?;
    }
    remove_autostart_off_marker()?;
    wait_for_first_health(SERVICE_LABEL, &log_marks).await
}

/// Where launchd writes the daemon's raw stderr (see `build_launchd_plist`).
#[cfg(target_os = "macos")]
pub(crate) fn launchd_stderr_log_path(data_root: &Path) -> PathBuf {
    data_root.join("logs/launchd-stderr.log")
}

/// Where the daemon's logs ended before this command started the daemon, so
/// an error reported afterwards is one this start produced and not a stale
/// line from an earlier run (the logs outlive the daemon).
struct DaemonLogMarks {
    #[cfg(target_os = "macos")]
    daemon_log: (PathBuf, u64),
    #[cfg(target_os = "macos")]
    stderr_log: (PathBuf, u64),
}

impl DaemonLogMarks {
    fn capture() -> Self {
        #[cfg(target_os = "macos")]
        {
            Self::at(
                crate::commands::setup::daemon_log_path(),
                launchd_stderr_log_path(&origin_data_root()),
            )
        }
        #[cfg(not(target_os = "macos"))]
        {
            Self {}
        }
    }

    #[cfg(target_os = "macos")]
    fn at(daemon_log: PathBuf, stderr_log: PathBuf) -> Self {
        let len = |path: &Path| std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        Self {
            daemon_log: (daemon_log.clone(), len(&daemon_log)),
            stderr_log: (stderr_log.clone(), len(&stderr_log)),
        }
    }

    /// The error this start produced, if any: the daemon log's last `ERROR`
    /// line written after the mark, else the last line launchd captured on
    /// stderr since then (a crash before file logging is up, such as a
    /// missing library).
    #[cfg(target_os = "macos")]
    fn error_since(&self) -> Option<(PathBuf, String)> {
        let (daemon_log, daemon_mark) = &self.daemon_log;
        if let Some(line) =
            crate::commands::setup::last_daemon_error_since(daemon_log, *daemon_mark)
        {
            return Some((daemon_log.clone(), line));
        }
        let (stderr_log, stderr_mark) = &self.stderr_log;
        let bytes = std::fs::read(stderr_log).ok()?;
        let start = usize::try_from(*stderr_mark)
            .unwrap_or(usize::MAX)
            .min(bytes.len());
        let line = String::from_utf8_lossy(&bytes[start..])
            .lines()
            .map(str::trim)
            .rfind(|line| !line.is_empty())?
            .chars()
            .take(400)
            .collect();
        Some((stderr_log.clone(), line))
    }
}

/// `background on` used to return as soon as the service manager accepted the
/// job, so a daemon that crash-looped under launchd still printed "Installed
/// and started" (first-run gauntlet finding F4). Wait for the daemon to
/// answer before claiming it started; when it does not, tell a first start
/// that is still downloading the embedding model apart from a daemon that
/// failed during this start.
async fn wait_for_first_health(installed: &str, marks: &DaemonLogMarks) -> Result<()> {
    // A routable `WENLAN_BIND_ADDR` has no local URL to poll; the install
    // itself succeeded, so there is nothing more to report.
    let Ok(base_url) = local_daemon_base_url() else {
        println!("Installed and started {installed}.");
        return Ok(());
    };
    let health_url = format!("{base_url}/api/health");
    if poll_health(&health_url, FIRST_HEALTH_WAIT).await.is_ok() {
        verify_daemon_data_root(
            &reqwest::Client::new(),
            &base_url,
            &wenlan_core::config::data_root(),
        )
        .await?;
        println!("Installed and started {installed}; daemon healthy at {base_url}.");
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    if let Some((log_path, line)) = marks.error_since() {
        anyhow::bail!(
            "installed {installed}, but the daemon stopped with an error ({}):\n  {line}\nFix the cause above, then run `wenlan background on` again.",
            log_path.display()
        );
    }
    #[cfg(not(target_os = "macos"))]
    let _ = marks;
    println!("Installed {installed}; the daemon is still starting.");
    println!("{}", first_health_pending_note(&health_url));
    Ok(())
}

/// The message for a daemon that has not answered yet but has not failed
/// either, as far as this command can see.
fn first_health_pending_note(health_url: &str) -> String {
    let mut note = format!(
        "Daemon not answering yet at {health_url} after {}s. A first start downloads the 210 MB embedding model; check `wenlan status` in a minute.",
        FIRST_HEALTH_WAIT.as_secs()
    );
    #[cfg(target_os = "macos")]
    note.push_str(&format!(
        "\nIf it stays down, `wenlan doctor` shows the daemon's last error (log: {}).",
        crate::commands::setup::daemon_log_path().display()
    ));
    #[cfg(target_os = "linux")]
    note.push_str("\nIf it stays down, `journalctl --user -u wenlan-server` says why.");
    #[cfg(target_os = "windows")]
    note.push_str("\nIf it stays down, run `wenlan doctor`.");
    note
}

#[cfg(target_os = "macos")]
fn current_user_id() -> Result<String> {
    let output = std::process::Command::new("id")
        .arg("-u")
        .output()
        .context("run id -u for launchd user domain")?;
    if !output.status.success() {
        anyhow::bail!("id -u failed (exit {})", output.status.code().unwrap_or(-1));
    }
    let uid = std::str::from_utf8(&output.stdout)
        .context("id -u returned non-UTF-8 output")?
        .trim();
    if uid.is_empty() || !uid.bytes().all(|byte| byte.is_ascii_digit()) {
        anyhow::bail!("id -u returned invalid user id: {uid:?}");
    }
    Ok(uid.to_owned())
}

#[cfg(target_os = "macos")]
fn bootstrap_launchd_job() -> Result<()> {
    require_unloaded_launchd_job()?;
    let domain = format!("gui/{}", current_user_id()?);
    let output = std::process::Command::new("launchctl")
        .arg("bootstrap")
        .arg(domain)
        .arg(service_unit_path()?)
        .output()?;
    if !output.status.success() {
        anyhow::bail!(
            "launchctl bootstrap failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn kickstart_registered_service() -> Result<()> {
    let uid = current_user_id()?;
    let target = format!("gui/{uid}/{SERVICE_LABEL}");
    let output = std::process::Command::new("launchctl")
        .args(["kickstart", &target])
        .output()
        .context("spawn launchctl kickstart")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let details = if stderr.trim().is_empty() {
            stdout.trim()
        } else {
            stderr.trim()
        };
        anyhow::bail!(
            "launchctl kickstart failed (exit {}): {}",
            output.status.code().unwrap_or(-1),
            details
        );
    }
    Ok(())
}

/// Best-effort `launchctl print` dump for the restart failure path, so a
/// timed-out health poll leaves the user something to look at besides the URL
/// that never answered.
#[cfg(target_os = "macos")]
fn print_service_diagnostics() {
    let Ok(uid) = current_user_id() else {
        return;
    };
    let target = format!("gui/{uid}/{SERVICE_LABEL}");
    if let Ok(output) = std::process::Command::new("launchctl")
        .args(["print", &target])
        .output()
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        if !stdout.trim().is_empty() {
            eprintln!("{}", stdout.trim());
        }
    }
}

// Supervisors start proven inactive registrations; they never force-stop.
async fn start_after_graceful_stop() -> Result<()> {
    start_registered(false)
}

async fn request_daemon_shutdown() -> Result<DaemonShutdownRequest> {
    let base_url = local_daemon_base_url()?;
    let root = wenlan_core::config::data_root();
    request_daemon_shutdown_at(&base_url, Some(&root)).await
}

async fn request_daemon_shutdown_at(
    base_url: &str,
    selected_root: Option<&Path>,
) -> Result<DaemonShutdownRequest> {
    let shutdown_url = format!("{base_url}/api/shutdown");
    let shutdown_client = reqwest::Client::builder()
        .timeout(OWNERSHIP_PROBE_TIMEOUT)
        .pool_max_idle_per_host(0)
        .build()
        .context("build daemon shutdown client")?;

    let expected_root = if let Some(root) = selected_root {
        match verify_daemon_data_root(&shutdown_client, base_url, root).await {
            Ok(identity) => Some(identity),
            Err(error)
                if error
                    .downcast_ref::<reqwest::Error>()
                    .is_some_and(|error| error.is_connect()) =>
            {
                return Ok(DaemonShutdownRequest::NotRunning)
            }
            Err(error) => return Err(error),
        }
    } else {
        None
    };
    let mut request = build_shutdown_request(&shutdown_client, &shutdown_url);
    if let Some(identity) = expected_root {
        request = request.header("x-wenlan-expected-root", identity);
    }

    let response = match request.send().await {
        Ok(response) => response,
        Err(error) if error.is_connect() => return Ok(DaemonShutdownRequest::NotRunning),
        Err(error) => {
            return Err(error).with_context(|| format!("POST {shutdown_url} failed"));
        }
    };
    let response = response
        .error_for_status()
        .with_context(|| format!("daemon returned an error for {shutdown_url}"))?;
    let process = response
        .headers()
        .get(DAEMON_PROCESS_ID_HEADER)
        .map(|value| {
            value
                .to_str()
                .context("daemon shutdown process id header is not UTF-8")?
                .parse::<u32>()
                .context("daemon shutdown process id header is not a valid PID")
        })
        .transpose()?
        .map(DaemonProcessIdentity::capture);
    response
        .bytes()
        .await
        .with_context(|| format!("read daemon shutdown response from {shutdown_url}"))?;
    drop(shutdown_client);
    Ok(DaemonShutdownRequest::Requested(process))
}

async fn verify_daemon_data_root(
    client: &reqwest::Client,
    base_url: &str,
    root: &Path,
) -> Result<String> {
    let health: wenlan_types::responses::HealthResponse = client
        .get(format!("{base_url}/api/health"))
        .timeout(OWNERSHIP_PROBE_TIMEOUT)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let expected = wenlan_core::config::data_root_identity(root)?;
    match health.data_root_identity.as_deref() {
        Some(identity) if identity == expected => {}
        Some(_) => return Err(OwnershipRefusal(format!("daemon at {base_url} does not prove the selected data root; check WENLAN_DATA_DIR and WENLAN_BIND_ADDR")).into()),
        None => return Err(OwnershipRefusal(format!("legacy daemon at {base_url} does not report its data root; stop the verified old instance manually before upgrading")).into()),
    }
    Ok(expected)
}

fn build_shutdown_request(client: &reqwest::Client, shutdown_url: &str) -> reqwest::RequestBuilder {
    // This is a server-visible shutdown contract, not merely a client pool
    // preference. Hyper graceful shutdown waits for accepted HTTP/1.1
    // connections to close, so the shutdown request must not leave its
    // connection alive for the health verifier to reuse.
    client
        .post(shutdown_url)
        .header(reqwest::header::CONNECTION, "close")
}

fn local_daemon_base_url() -> Result<String> {
    let mut address = configured_bind_addr()?;
    if address.ip().is_unspecified() {
        address.set_ip(if address.is_ipv4() {
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
        } else {
            std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)
        });
    }
    Ok(format!("http://{address}"))
}

fn configured_bind_addr() -> Result<std::net::SocketAddr> {
    let raw = std::env::var("WENLAN_BIND_ADDR").unwrap_or_else(|_| {
        std::env::var("WENLAN_PORT")
            .map(|port| format!("127.0.0.1:{port}"))
            .unwrap_or_else(|_| DEFAULT_LOCAL_BIND_ADDR.to_string())
    });
    let address: std::net::SocketAddr = raw
        .parse()
        .with_context(|| format!("invalid local WENLAN_BIND_ADDR {raw:?}"))?;
    if !address.ip().is_unspecified() && !address.ip().is_loopback() {
        anyhow::bail!(
            "refusing background lifecycle control through non-loopback WENLAN_BIND_ADDR {raw:?}"
        );
    }
    Ok(address)
}

async fn verify_daemon_unreachable(
    client: &reqwest::Client,
    health_url: &str,
    expected_process: Option<DaemonProcessIdentity>,
) -> Result<()> {
    let deadline = std::time::Instant::now() + SHUTDOWN_VERIFY_TIMEOUT;
    let mut unreachable_since = None;
    let mut process_system = expected_process.map(|_| sysinfo::System::new());
    loop {
        tokio::time::sleep(SHUTDOWN_PROBE_INTERVAL).await;
        let expected_process_running = match (expected_process, process_system.as_mut()) {
            (Some(process), Some(system)) => process.is_running(system),
            _ => false,
        };
        match client
            .get(health_url)
            .timeout(SHUTDOWN_PROBE_TIMEOUT)
            .send()
            .await
        {
            // During cooperative shutdown the socket can still accept a
            // connection after the HTTP service has stopped answering. A
            // timeout is neither proof of exit nor a terminal verification
            // error: keep probing until the bounded overall deadline. Reset
            // the refusal window because a listening-but-hung socket is not
            // yet a confirmed stop.
            Err(error) if error.is_timeout() => {
                if expected_process.is_some() && !expected_process_running {
                    unreachable_since.get_or_insert_with(std::time::Instant::now);
                } else {
                    unreachable_since = None;
                }
            }
            Err(error) if is_shutdown_disconnect(&error) => {
                if expected_process.is_none() || !expected_process_running {
                    unreachable_since.get_or_insert_with(std::time::Instant::now);
                } else {
                    unreachable_since = None;
                }
            }
            Ok(_) => {
                if !expected_process_running {
                    if let Some(process) = expected_process {
                        anyhow::bail!(
                            "a different daemon remained reachable at {health_url} after process {} exited",
                            process.pid
                        );
                    }
                }
                if unreachable_since.is_some() {
                    anyhow::bail!("daemon remained reachable at {health_url} after disconnecting");
                }
            }
            Err(error) => {
                return Err(error).with_context(|| format!("verify shutdown via {health_url}"));
            }
        }
        if unreachable_since.is_some_and(|since| since.elapsed() >= SHUTDOWN_STABILITY_WINDOW) {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            if expected_process_running {
                if let Some(process) = expected_process {
                    anyhow::bail!("daemon process {} remained running", process.pid);
                }
            }
            anyhow::bail!("daemon remained reachable at {health_url}");
        }
    }
}

fn is_shutdown_disconnect(error: &reqwest::Error) -> bool {
    if error.is_connect() {
        return true;
    }
    let mut cause: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(current) = cause {
        if let Some(io_error) = current.downcast_ref::<std::io::Error>() {
            return matches!(
                io_error.kind(),
                std::io::ErrorKind::ConnectionRefused
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::ConnectionAborted
                    | std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::NotConnected
            );
        }
        cause = current.source();
    }
    false
}

async fn verify_local_daemon_shutdown(
    expected_process: Option<DaemonProcessIdentity>,
) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .pool_max_idle_per_host(0)
        .build()
        .context("build daemon shutdown verification client")?;
    let health_url = format!("{}/api/health", local_daemon_base_url()?);
    verify_daemon_unreachable(&client, &health_url, expected_process).await
}

async fn stop() -> Result<()> {
    let registration_present = is_installed();
    if registration_present {
        verify_registered_root()?;
    }
    let shutdown_requested = match request_daemon_shutdown().await? {
        DaemonShutdownRequest::Requested(process) => {
            verify_local_daemon_shutdown(process).await?;
            true
        }
        DaemonShutdownRequest::NotRunning => false,
    };
    if registration_present {
        verify_registered_stopped()?;
    }
    let marker = write_autostart_off_marker()?;
    if registration_present {
        println!(
            "Stopped {}. Background registration kept. Autostart disabled via {}.",
            SERVICE_LABEL,
            marker.display()
        );
    } else if shutdown_requested {
        println!(
            "Stopped {}. No background registration found. Autostart disabled via {}.",
            SERVICE_LABEL,
            marker.display()
        );
    } else {
        println!(
            "Wenlan background process is already stopped; no registration found. Autostart disabled via {}.",
            marker.display()
        );
    }
    Ok(())
}

/// Restart the Wenlan daemon: gracefully stop the running process, wait for
/// it to fully exit, then start the freshly registered binary and verify
/// `/api/health` answers before returning. A bare stop-then-start with no
/// wait races the old process's exit: launchd's `KeepAlive
/// {SuccessfulExit=false}` never relaunches an exit-0 process, so `start`
/// issued while the old process is still alive is a silent no-op. Restarting
/// is also required after an upgrade — installing a new binary does not
/// replace an already-running daemon (the new process detects the healthy
/// incumbent on port 7878 and exits). See wenlan-server/src/main.rs:582-615.
pub async fn restart() -> Result<()> {
    if !is_installed() {
        anyhow::bail!("Wenlan background process is not set up. Run `wenlan background on` first.");
    }
    verify_registered_root()?;

    match request_daemon_shutdown().await? {
        DaemonShutdownRequest::Requested(process) => verify_local_daemon_shutdown(process).await?,
        DaemonShutdownRequest::NotRunning => {}
    }
    start_after_graceful_stop().await?;

    let base_url = local_daemon_base_url()?;
    let health_url = format!("{base_url}/api/health");
    if let Err(error) = poll_health(&health_url, std::time::Duration::from_secs(30)).await {
        #[cfg(target_os = "macos")]
        print_service_diagnostics();
        anyhow::bail!(
            "daemon at {health_url} did not become healthy after restart ({error:#}); the daemon may still be starting; re-check with `wenlan status`"
        );
    }

    verify_daemon_data_root(
        &reqwest::Client::new(),
        &base_url,
        &wenlan_core::config::data_root(),
    )
    .await?;
    remove_autostart_off_marker()?;
    println!(
        "Restarted {}; daemon healthy at {}.",
        SERVICE_LABEL, base_url
    );
    Ok(())
}

/// Start the already-registered Wenlan service without stopping or replacing it.
pub fn start_registered(explicit_user_command: bool) -> Result<()> {
    if !is_installed() {
        anyhow::bail!("Wenlan background process is not set up. Run `wenlan background on` first.");
    }
    verify_registered_stopped()?;
    #[cfg(windows)]
    run_schtasks(&["/run", "/tn", &windows_task_name()], "run scheduled task")?;
    #[cfg(target_os = "macos")]
    {
        if loaded_launchd_job()?.is_some() {
            kickstart_registered_service()?;
        } else {
            bootstrap_launchd_job()?;
        }
    }
    #[cfg(target_os = "linux")]
    manager()?
        .start(ServiceStartCtx { label: label()? })
        .context("start service")?;
    if explicit_user_command {
        remove_autostart_off_marker()?;
    }
    Ok(())
}

pub fn is_installed() -> bool {
    #[cfg(target_os = "windows")]
    {
        // `schtasks /query /tn <name>` exits 0 when the task exists, 1 when
        // it does not. No admin rights needed for the read-only query.
        std::process::Command::new("schtasks.exe")
            .args(["/query", "/tn", &windows_task_name()])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "windows"))]
    {
        service_unit_path().map(|p| p.exists()).unwrap_or(false)
    }
}

pub async fn print_status() -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        if is_installed() {
            println!(
                "Service: scheduled task '{}' (registered)",
                windows_task_name()
            );
        } else {
            println!(
                "Service: scheduled task '{}' (not installed)",
                windows_task_name()
            );
        }
    }
    #[cfg(not(target_os = "windows"))]
    match service_unit_path() {
        Ok(path) if path.exists() => println!("Service unit: {} (installed)", path.display()),
        Ok(path) => println!("Service unit: {} (not installed)", path.display()),
        Err(e) => println!("Service unit: unable to resolve ({})", e),
    }

    let url = format!("{}/api/health", origin_host_from_env());
    match reqwest::get(&url).await {
        Ok(resp) if resp.status().is_success() => {
            println!("Health: ok ({})", url);
        }
        Ok(resp) => {
            println!("Health: unhealthy (status {})", resp.status());
        }
        Err(e) => {
            println!("Health: not reachable ({})", e);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_systemd_registration_requires_an_explicit_matching_root() {
        let root = tempfile::tempdir().unwrap();
        let escaped = root.path().to_string_lossy().replace('\\', "\\\\");
        let properties = format!("Environment=\"WENLAN_DATA_DIR={escaped}\" RUST_LOG=info\nExecStart={{ path=/usr/bin/wenlan-server ; argv[]=/usr/bin/wenlan-server ; }}\nEnvironmentFiles=\n");
        assert!(verify_systemd_root(&properties, root.path()).is_ok());
        let other = tempfile::tempdir().unwrap();
        assert!(verify_systemd_root(&properties, other.path()).is_err());
        assert!(verify_systemd_root("Environment=RUST_LOG=info\n", root.path()).is_err());
        assert!(verify_systemd_root(
            &(properties.clone() + "EnvironmentFiles=/tmp/overrides\n"),
            root.path()
        )
        .is_err());
        assert!(verify_systemd_root(
            &(properties + "ExecStart=server --data-dir /other\n"),
            root.path()
        )
        .is_err());
    }

    #[tokio::test]
    async fn absent_daemon_is_measured_before_the_ownership_probe_deadline() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let root = tempfile::tempdir().unwrap();
        let result = request_daemon_shutdown_at(&format!("http://{address}"), Some(root.path()))
            .await
            .unwrap();
        assert!(matches!(result, DaemonShutdownRequest::NotRunning));
    }

    #[tokio::test]
    async fn legacy_health_refusal_gives_migration_guidance_without_posting() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            {
                let mut reader = tokio::io::BufReader::new(&mut stream);
                let mut request = String::new();
                assert!(reader.read_line(&mut request).await.unwrap() > 0);
                assert!(request.starts_with("GET /api/health "));
                loop {
                    let mut header = String::new();
                    assert!(reader.read_line(&mut header).await.unwrap() > 0);
                    if header == "\r\n" {
                        break;
                    }
                }
            }
            let body = r#"{"status":"ok","db_initialized":true,"version":"legacy"}"#;
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(200), listener.accept())
                    .await
                    .is_err()
            );
        });
        let root = tempfile::tempdir().unwrap();
        let error = request_daemon_shutdown_at(&format!("http://{address}"), Some(root.path()))
            .await
            .err()
            .unwrap();
        assert!(error.downcast_ref::<OwnershipRefusal>().is_some());
        assert!(error.to_string().contains("Legacy service upgrade"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn wrong_root_is_rejected_before_any_shutdown_request() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 4096];
            let size = stream.read(&mut request).await.unwrap();
            assert!(String::from_utf8_lossy(&request[..size]).starts_with("GET /api/health "));
            let body = r#"{"status":"ok","db_initialized":true,"version":"test","data_root_identity":"another-instance"}"#;
            stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(200), listener.accept())
                    .await
                    .is_err(),
                "wrong instance must never receive POST /api/shutdown"
            );
        });
        let root = tempfile::tempdir().unwrap();
        let result =
            request_daemon_shutdown_at(&format!("http://{address}"), Some(root.path())).await;
        assert!(result.is_err());
        server.await.unwrap();
    }

    #[test]
    fn daemon_process_identity_observes_child_exit() {
        #[cfg(target_os = "windows")]
        let mut child = std::process::Command::new("cmd.exe")
            .args(["/C", "ping -n 30 127.0.0.1 >NUL"])
            .spawn()
            .expect("spawn process identity test child");
        #[cfg(not(target_os = "windows"))]
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn process identity test child");

        let identity = DaemonProcessIdentity::capture(child.id());
        let mut system = sysinfo::System::new();
        assert!(identity.is_running(&mut system));

        child.kill().expect("kill process identity test child");
        child.wait().expect("reap process identity test child");
        // Windows can keep a terminated process visible in the enumeration
        // snapshot briefly after wait(); poll instead of racing one check.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while identity.is_running(&mut system) {
            assert!(
                std::time::Instant::now() < deadline,
                "child process still visible 10s after exit"
            );
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn launchd_plist_keeps_stderr_in_the_data_root() {
        let data_root = Path::new("/tmp/wenlan-plist-test-root");
        let stderr_log = launchd_stderr_log_path(data_root);
        assert_eq!(
            stderr_log,
            Path::new("/tmp/wenlan-plist-test-root/logs/launchd-stderr.log")
        );
        let plist = build_launchd_plist(
            Path::new("/opt/wenlan/wenlan-server"),
            Path::new("/dev/null"),
            &stderr_log,
            "info",
            data_root,
        );
        assert!(plist.contains(
            "<key>StandardErrorPath</key>\n\t<string>/tmp/wenlan-plist-test-root/logs/launchd-stderr.log</string>"
        ));
        assert!(plist.contains("<key>StandardOutPath</key>\n\t<string>/dev/null</string>"));
        assert!(plist.contains(
            "<key>WENLAN_DATA_DIR</key>\n\t\t<string>/tmp/wenlan-plist-test-root</string>"
        ));
    }

    /// A plist as a user leaves it: an older binary path and stderr path, a
    /// tuning key the CLI knows nothing about, a `RUST_LOG` they changed, and
    /// an extra environment variable — with no `WENLAN_DATA_DIR`.
    #[cfg(target_os = "macos")]
    const USER_EDITED_PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>com.wenlan.server</string>
	<key>ProgramArguments</key>
	<array>
		<string>/old/bin/wenlan-server</string>
	</array>
	<key>StandardErrorPath</key>
	<string>/old/root/logs/launchd-stderr.log</string>
	<key>ThrottleInterval</key>
	<integer>30</integer>
	<key>EnvironmentVariables</key>
	<dict>
		<key>RUST_LOG</key>
		<string>debug</string>
		<key>WENLAN_ENABLE_EDGE_GROUNDING_PROMOTE</key>
		<string>1</string>
	</dict>
</dict>
</plist>
"#;

    #[cfg(target_os = "macos")]
    fn plist_dict(rendered: &str) -> plist::Dictionary {
        match plist::Value::from_reader_xml(rendered.as_bytes()).expect("rendered plist parses") {
            plist::Value::Dictionary(root) => root,
            other => panic!("plist root is not a dictionary: {other:?}"),
        }
    }

    #[cfg(target_os = "macos")]
    fn plist_env(root: &plist::Dictionary) -> &plist::Dictionary {
        root.get("EnvironmentVariables")
            .and_then(plist::Value::as_dictionary)
            .expect("EnvironmentVariables dictionary")
    }

    #[cfg(target_os = "macos")]
    fn plist_string<'a>(dict: &'a plist::Dictionary, key: &str) -> Option<&'a str> {
        dict.get(key).and_then(plist::Value::as_string)
    }

    #[cfg(target_os = "macos")]
    fn merged_user_plist(explicit_data_root: Option<&Path>) -> plist::Dictionary {
        plist_dict(&launchd_plist_contents(
            Some(USER_EDITED_PLIST),
            Path::new("/new/bin/wenlan-server"),
            Path::new("/new/root/logs/launchd-stderr.log"),
            Path::new("/new/root"),
            explicit_data_root,
        ))
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn launchd_plist_without_an_existing_file_is_the_fresh_template() {
        let program = Path::new("/opt/wenlan/wenlan-server");
        let stderr_log = Path::new("/data/logs/launchd-stderr.log");
        let data_root = Path::new("/data");
        assert_eq!(
            launchd_plist_contents(None, program, stderr_log, data_root, Some(data_root)),
            build_launchd_plist(
                program,
                Path::new("/dev/null"),
                stderr_log,
                "info",
                data_root
            )
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn launchd_plist_merge_keeps_user_keys_and_updates_cli_owned_ones() {
        let root = merged_user_plist(None);
        let env = plist_env(&root);
        assert_eq!(
            plist_string(env, "WENLAN_ENABLE_EDGE_GROUNDING_PROMOTE"),
            Some("1"),
            "user environment variables survive re-registration"
        );
        assert_eq!(
            plist_string(env, "RUST_LOG"),
            Some("debug"),
            "a RUST_LOG the user changed is theirs"
        );
        assert_eq!(
            env.get("WENLAN_DATA_DIR"),
            None,
            "no default WENLAN_DATA_DIR is added when the CLI was not given one"
        );
        assert_eq!(
            root.get("ThrottleInterval")
                .and_then(plist::Value::as_signed_integer),
            Some(30),
            "unknown top-level keys survive"
        );
        assert_eq!(
            root.get("ProgramArguments")
                .and_then(plist::Value::as_array)
                .and_then(|args| args.first())
                .and_then(plist::Value::as_string),
            Some("/new/bin/wenlan-server"),
            "the program path is CLI-owned and moves with the binary"
        );
        assert_eq!(
            plist_string(&root, "StandardErrorPath"),
            Some("/new/root/logs/launchd-stderr.log")
        );
        assert_eq!(plist_string(&root, "StandardOutPath"), Some("/dev/null"));
        assert_eq!(plist_string(&root, "Label"), Some(SERVICE_LABEL));
        assert_eq!(
            root.get("KeepAlive")
                .and_then(plist::Value::as_dictionary)
                .and_then(|keep_alive| keep_alive.get("SuccessfulExit"))
                .and_then(plist::Value::as_boolean),
            Some(false)
        );
        assert_eq!(
            root.get("RunAtLoad").and_then(plist::Value::as_boolean),
            Some(true)
        );
        assert_eq!(
            root.get("Disabled").and_then(plist::Value::as_boolean),
            Some(true)
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn launchd_plist_merge_writes_only_an_explicit_data_root() {
        let root = merged_user_plist(Some(Path::new("/explicit/root")));
        assert_eq!(
            plist_string(plist_env(&root), "WENLAN_DATA_DIR"),
            Some("/explicit/root")
        );
        assert_eq!(
            plist_string(plist_env(&root), "WENLAN_ENABLE_EDGE_GROUNDING_PROMOTE"),
            Some("1")
        );

        let pinned = USER_EDITED_PLIST.replace(
            "\t\t<key>RUST_LOG</key>",
            "\t\t<key>WENLAN_DATA_DIR</key>\n\t\t<string>/private/tmp/scratch</string>\n\t\t<key>RUST_LOG</key>",
        );
        let program = Path::new("/new/bin/wenlan-server");
        let stderr_log = Path::new("/new/root/logs/launchd-stderr.log");
        let data_root = Path::new("/new/root");
        let kept = plist_dict(&launchd_plist_contents(
            Some(&pinned),
            program,
            stderr_log,
            data_root,
            None,
        ));
        assert_eq!(
            plist_string(plist_env(&kept), "WENLAN_DATA_DIR"),
            Some("/private/tmp/scratch"),
            "an existing WENLAN_DATA_DIR is kept when the CLI was not given one"
        );
        let replaced = plist_dict(&launchd_plist_contents(
            Some(&pinned),
            program,
            stderr_log,
            data_root,
            Some(data_root),
        ));
        assert_eq!(
            plist_string(plist_env(&replaced), "WENLAN_DATA_DIR"),
            Some("/new/root"),
            "an explicit data root replaces the existing one"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn launchd_plist_merge_seeds_rust_log_and_replaces_an_unreadable_file() {
        let no_env = USER_EDITED_PLIST.replace(
            "\t<key>EnvironmentVariables</key>\n\t<dict>\n\t\t<key>RUST_LOG</key>\n\t\t<string>debug</string>\n\t\t<key>WENLAN_ENABLE_EDGE_GROUNDING_PROMOTE</key>\n\t\t<string>1</string>\n\t</dict>\n",
            "",
        );
        assert!(!no_env.contains("EnvironmentVariables"), "{no_env}");
        let program = Path::new("/new/bin/wenlan-server");
        let stderr_log = Path::new("/new/root/logs/launchd-stderr.log");
        let data_root = Path::new("/new/root");
        let seeded = plist_dict(&launchd_plist_contents(
            Some(&no_env),
            program,
            stderr_log,
            data_root,
            None,
        ));
        assert_eq!(plist_string(plist_env(&seeded), "RUST_LOG"), Some("info"));
        assert_eq!(plist_env(&seeded).get("WENLAN_DATA_DIR"), None);

        assert_eq!(
            launchd_plist_contents(
                Some("fake registered launch agent"),
                program,
                stderr_log,
                data_root,
                None
            ),
            build_launchd_plist(
                program,
                Path::new("/dev/null"),
                stderr_log,
                "info",
                data_root
            ),
            "a file that is not a plist has nothing to keep"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn install_reports_only_errors_written_after_it_started_the_daemon() {
        let dir = tempfile::tempdir().expect("tempdir");
        let daemon_log = dir.path().join("wenlan-server.log");
        let stderr_log = dir.path().join("launchd-stderr.log");
        std::fs::write(
            &daemon_log,
            "INFO wenlan-server v0.17.0\nERROR wenlan_server: stale failure\n",
        )
        .expect("write daemon log");
        let marks = DaemonLogMarks::at(daemon_log.clone(), stderr_log.clone());
        assert_eq!(
            marks.error_since(),
            None,
            "an error from an earlier run must not be blamed on this start"
        );
        std::fs::write(
            &stderr_log,
            "dyld[123]: Library not loaded: libonnxruntime.dylib\n",
        )
        .expect("write stderr log");
        assert_eq!(
            marks.error_since(),
            Some((
                stderr_log.clone(),
                "dyld[123]: Library not loaded: libonnxruntime.dylib".to_string()
            ))
        );
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&daemon_log)
            .expect("open daemon log");
        std::io::Write::write_all(
            &mut file,
            b"INFO wenlan-server v0.17.1\nERROR wenlan_server: fresh failure\n",
        )
        .expect("append");
        assert_eq!(
            marks.error_since(),
            Some((daemon_log, "ERROR wenlan_server: fresh failure".to_string())),
            "the daemon log's own error wins over launchd's stderr"
        );
    }

    #[test]
    fn first_health_note_says_how_long_it_waited_and_what_to_do() {
        let note = first_health_pending_note("http://127.0.0.1:7878/api/health");
        assert!(note.contains("after 10s"), "{note}");
        assert!(note.contains("210 MB embedding model"), "{note}");
        assert!(note.contains("`wenlan status`"), "{note}");
        assert!(note.contains("If it stays down"), "{note}");
    }

    #[test]
    fn shutdown_request_declares_connection_close() {
        let client = reqwest::Client::builder()
            .build()
            .expect("build shutdown request test client");
        let request = build_shutdown_request(&client, "http://127.0.0.1:7878/api/shutdown")
            .build()
            .expect("build shutdown request");

        assert_eq!(
            request.headers().get(reqwest::header::CONNECTION),
            Some(&reqwest::header::HeaderValue::from_static("close"))
        );
    }
}
