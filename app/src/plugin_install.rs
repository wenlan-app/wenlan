//! Installs Wenlan's Claude Code and Codex plugins by shelling out to each
//! CLI's supported, non-interactive plugin subcommands, instead of
//! hand-writing either CLI's private plugin-state file
//! (`~/.claude/plugins/installed_plugins.json` is versioned, undocumented,
//! and carries a git cache with commit SHAs; the `.bak-*` files next to it
//! show the format churns). Both plugins declare their own `mcpServers`, so
//! callers must never ALSO write a raw MCP entry for `claude_code` /
//! `codex_cli` via `mcp_config.rs` — that would duplicate it.
//!
//! Binary resolution is platform-neutral and never blocks on a shell: it
//! searches the process `PATH` (with `.exe`/`.cmd`/`.bat` suffixes on
//! Windows), then the conventional install dirs a GUI-launched app's minimal
//! PATH misses (`~/.local/bin`, Homebrew, npm/volta/bun/pnpm globals, every
//! `~/.nvm/versions/node/*/bin`, `%APPDATA%\npm` on Windows), then the Codex
//! CLI bundled with ChatGPT Desktop on macOS, and only then asks the user's
//! own login shell (`$SHELL -l -i -c 'command -v <bin>'`, Unix only) under a
//! hard 3 s bound. Both CLIs are commonly installed via
//! `npm i -g` under a version manager whose bin dir only the user's rc
//! files know; an interactive login shell can hang on a slow rc file, so it
//! is the last resort rather than the first.
//!
//! Marketplace *selectors* are resolved at runtime, never hardcoded: the
//! name a CLI derives from a GitHub source can differ from the repo slug
//! (Codex derived `wenlan-local` for this repo as of codex-cli
//! 0.144.0, before the repo's `.agents/plugins/marketplace.json` was named
//! `7xuanlu-wenlan`) — so after `marketplace add`, each installer reads the
//! real name back out of `plugin marketplace list --json` and only falls
//! back to a hardcoded default if that lookup fails.

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// The GitHub source both CLIs' `plugin marketplace add` clone from.
const WENLAN_REPO: &str = "wenlan-app/wenlan";
/// The source before the repo moved to the `wenlan-app` org. Installs made
/// from it stay registered under this slug, so marketplace lookup accepts
/// both; `marketplace add` only ever uses `WENLAN_REPO`.
const LEGACY_WENLAN_REPO: &str = "7xuanlu/wenlan";
/// Last-resort marketplace names if runtime resolution fails for any reason
/// (unexpected CLI output, `marketplace list` unsupported, etc). Both must
/// equal the `name` in the repo's own marketplace manifests —
/// `.claude-plugin/marketplace.json` for Claude and
/// `.agents/plugins/marketplace.json` for Codex — which a test reads from
/// disk so the constants cannot drift from what the CLIs actually register.
const FALLBACK_MARKETPLACE_CLAUDE: &str = "7xuanlu-wenlan";
const FALLBACK_MARKETPLACE_CODEX: &str = "7xuanlu-wenlan";

/// Hard bound on the login-shell fallback. An interactive login shell runs
/// the user's rc files, which can block on the network, a keychain prompt or
/// a slow version-manager init; the lookup is a convenience and must never
/// hang the setup UI.
const SHELL_LOOKUP_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PluginInstallError {
    #[error("{0} CLI not found")]
    CliNotFound(&'static str),
    #[error("{0}")]
    StepFailed(String),
    #[error("unsupported client type: {0}")]
    UnknownClient(String),
}

// ── Binary resolution ───────────────────────────────────────────────────

/// Everything resolution reads from the environment, captured once so the
/// probe order is unit-testable for every OS from any host.
#[derive(Debug, Clone, Default)]
struct LookupEnv {
    home: Option<PathBuf>,
    path_var: Option<OsString>,
    windows: bool,
    macos: bool,
    appdata: Option<PathBuf>,
    local_appdata: Option<PathBuf>,
}

impl LookupEnv {
    fn from_process() -> Self {
        let nonempty = |key: &str| {
            std::env::var_os(key)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        };
        Self {
            home: dirs::home_dir(),
            path_var: std::env::var_os("PATH"),
            windows: std::env::consts::OS == "windows",
            macos: std::env::consts::OS == "macos",
            appdata: nonempty("APPDATA"),
            local_appdata: nonempty("LOCALAPPDATA"),
        }
    }
}

/// File names that can launch `binary_name`: the bare name on Unix; the
/// `.exe`/`.cmd`/`.bat` forms on Windows (an npm-installed CLI is a `.cmd`
/// shim there, which `CreateProcess` would not find by bare name). The
/// extension list is `mcp_config`'s, which `command_stem` strips, so the two
/// can never disagree about what counts as a launcher extension.
fn executable_names(binary_name: &str, windows: bool) -> Vec<String> {
    if windows {
        crate::mcp_config::WINDOWS_LAUNCHER_EXTENSIONS
            .iter()
            .map(|ext| format!("{binary_name}.{ext}"))
            .collect()
    } else {
        vec![binary_name.to_string()]
    }
}

/// Every `PATH` entry crossed with the launchable names, in PATH order.
/// Relative entries are skipped: they resolve against the current directory,
/// which for a GUI app is arbitrary and for a hostile checkout is a planted
/// binary.
fn path_candidates(env: &LookupEnv, binary_name: &str) -> Vec<PathBuf> {
    let Some(path_var) = env.path_var.as_ref() else {
        return Vec::new();
    };
    let names = executable_names(binary_name, env.windows);
    std::env::split_paths(path_var)
        .filter(|dir| dir.is_absolute())
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .collect()
}

/// Version-manager bin dirs under `~/.nvm/versions/node`, newest first.
fn nvm_bin_dirs(home: &Path, list_dir: &impl Fn(&Path) -> Vec<PathBuf>) -> Vec<PathBuf> {
    fn version_key(dir: &Path) -> Vec<u64> {
        dir.file_name()
            .map(|name| name.to_string_lossy().trim_start_matches('v').to_string())
            .unwrap_or_default()
            .split('.')
            .map(|part| part.parse::<u64>().unwrap_or(0))
            .collect()
    }
    let mut versions = list_dir(&home.join(".nvm/versions/node"));
    versions.sort_by_key(|dir| std::cmp::Reverse(version_key(dir)));
    versions.into_iter().map(|dir| dir.join("bin")).collect()
}

/// Conventional install dirs a GUI-launched app's minimal PATH misses, in
/// probe order. `.claude/local` is Claude Code's own self-managed install dir
/// and only applies to `claude`.
fn known_install_dirs(
    env: &LookupEnv,
    binary_name: &str,
    list_dir: &impl Fn(&Path) -> Vec<PathBuf>,
) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = &env.home {
        dirs.push(home.join(".local/bin"));
        if binary_name == "claude" {
            dirs.push(home.join(".claude/local"));
        }
    }
    if env.windows {
        if let Some(appdata) = &env.appdata {
            dirs.push(appdata.join("npm"));
        }
        if let Some(local) = &env.local_appdata {
            dirs.push(local.join("Microsoft").join("WinGet").join("Links"));
            dirs.push(local.join("pnpm"));
        }
        if let Some(home) = &env.home {
            dirs.push(home.join("scoop").join("shims"));
            dirs.push(home.join(".volta").join("bin"));
            dirs.push(home.join(".bun").join("bin"));
        }
    } else {
        dirs.push(PathBuf::from("/opt/homebrew/bin"));
        dirs.push(PathBuf::from("/usr/local/bin"));
        dirs.push(PathBuf::from("/home/linuxbrew/.linuxbrew/bin"));
        if let Some(home) = &env.home {
            dirs.push(home.join(".npm-global/bin"));
            dirs.push(home.join(".volta/bin"));
            dirs.push(home.join(".bun/bin"));
            dirs.push(home.join(".local/share/pnpm"));
            dirs.extend(nvm_bin_dirs(home, list_dir));
        }
    }
    dirs
}

fn known_candidates(
    env: &LookupEnv,
    binary_name: &str,
    list_dir: &impl Fn(&Path) -> Vec<PathBuf>,
) -> Vec<PathBuf> {
    let names = executable_names(binary_name, env.windows);
    known_install_dirs(env, binary_name, list_dir)
        .into_iter()
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .collect()
}

/// Codex is bundled inside ChatGPT Desktop on macOS. Desktop-only users have
/// no standalone `codex` on PATH, so plugin setup must be able to invoke the
/// copy shipped with the detected app. Keep this lookup macOS-only and scoped
/// to Codex; the app's bundled CLI is an implementation detail of that bundle.
fn bundled_codex_candidates(env: &LookupEnv, binary_name: &str) -> Vec<PathBuf> {
    if !env.macos || binary_name != "codex" {
        return Vec::new();
    }
    let mut apps = vec![PathBuf::from("/Applications/ChatGPT.app")];
    if let Some(home) = &env.home {
        apps.push(home.join("Applications/ChatGPT.app"));
    }
    apps.into_iter()
        .map(|app| app.join("Contents/Resources/codex-cli/bin/codex"))
        .collect()
}

fn first_existing(candidates: &[PathBuf], exists: &impl Fn(&Path) -> bool) -> Option<PathBuf> {
    candidates.iter().find(|p| exists(p)).cloned()
}

/// Full resolution with every filesystem/process dependency injected, so the
/// probe order (PATH, known install dirs, ChatGPT's bundled Codex, then the
/// bounded login shell) is unit-testable without spawning a shell or touching
/// the real filesystem.
fn resolve_binary_with(
    binary_name: &str,
    env: &LookupEnv,
    exists: impl Fn(&Path) -> bool,
    list_dir: impl Fn(&Path) -> Vec<PathBuf>,
    login_shell: impl FnOnce(&str) -> Option<PathBuf>,
) -> Option<PathBuf> {
    first_existing(&path_candidates(env, binary_name), &exists)
        .or_else(|| first_existing(&known_candidates(env, binary_name, &list_dir), &exists))
        .or_else(|| first_existing(&bundled_codex_candidates(env, binary_name), &exists))
        .or_else(|| login_shell(binary_name))
}

/// A regular file the current user can run.
fn is_executable_file(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn list_child_dirs(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.is_dir())
                .collect()
        })
        .unwrap_or_default()
}

/// Runs `command` to completion or kills it after `timeout`. Returns the
/// exit success flag and stdout, or `None` when it could not start or ran out
/// of time. stdout is drained on a helper thread so a grandchild that keeps
/// the pipe open after the shell exits cannot hold this call past its bound.
fn run_bounded(command: &mut Command, timeout: Duration) -> Option<(bool, Vec<u8>)> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command.spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        let _ = tx.send(buf);
    });
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                let buf = rx
                    .recv_timeout(remaining.max(Duration::from_millis(50)))
                    .unwrap_or_default();
                return Some((status.success(), buf));
            }
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// The shell to ask: `$SHELL` when it is an absolute path to a file, else the
/// first of zsh/bash that exists. Unix only; Windows has no equivalent of an
/// rc-file PATH, so the PATH and known-dir searches are the whole story.
fn lookup_shell() -> Option<PathBuf> {
    if std::env::consts::OS == "windows" {
        return None;
    }
    std::env::var_os("SHELL")
        .map(PathBuf::from)
        .into_iter()
        .chain(["/bin/zsh", "/bin/bash"].map(PathBuf::from))
        .find(|shell| shell.is_absolute() && shell.is_file())
}

/// Asks `shell` (as an interactive login shell, so version-manager rc files
/// run) where `binary_name` lives, giving up after `timeout`. rc files may
/// print banners, so the answer is the LAST absolute-path line, not the whole
/// output; an alias or function line (`alias codex=...`) is not a path and is
/// ignored.
fn shell_lookup(binary_name: &str, shell: &Path, timeout: Duration) -> Option<PathBuf> {
    if binary_name.is_empty()
        || !binary_name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return None;
    }
    let mut command = Command::new(shell);
    command
        .arg("-l")
        .arg("-i")
        .arg("-c")
        .arg(format!("command -v {binary_name}"));
    let (success, stdout) = run_bounded(&mut command, timeout)?;
    if !success {
        return None;
    }
    String::from_utf8_lossy(&stdout)
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| Path::new(line).is_absolute())
        .map(PathBuf::from)
        .filter(|path| is_executable_file(path))
}

fn login_shell_binary_path(binary_name: &str) -> Option<PathBuf> {
    shell_lookup(binary_name, &lookup_shell()?, SHELL_LOOKUP_TIMEOUT)
}

/// Resolve a CLI binary by name ("claude" / "codex") without ever starting a
/// shell: PATH, then the known install dirs. Cheap enough for detection
/// code that runs on every wizard refresh. Never panics — a missing CLI is
/// expected, not exceptional.
pub fn resolve_binary_no_shell(binary_name: &str) -> Option<PathBuf> {
    resolve_binary_with(
        binary_name,
        &LookupEnv::from_process(),
        is_executable_file,
        list_child_dirs,
        |_| None,
    )
}

/// Resolve a CLI binary by name ("claude" / "codex"): PATH, known install
/// dirs, ChatGPT Desktop's bundled Codex CLI, then the user's login shell
/// under a 3 s bound as the last resort.
pub fn resolve_binary(binary_name: &str) -> Option<PathBuf> {
    resolve_binary_with(
        binary_name,
        &LookupEnv::from_process(),
        is_executable_file,
        list_child_dirs,
        login_shell_binary_path,
    )
}

// ── Marketplace selector resolution ─────────────────────────────────────

/// Reads the registered marketplace name for `wenlan-app/wenlan` (or its
/// pre-move slug `LEGACY_WENLAN_REPO`) out of `claude plugin marketplace
/// list --json` output, e.g.
/// `[{"name":"7xuanlu-wenlan","source":"github","repo":"wenlan-app/wenlan",...}]`.
fn find_marketplace_name_claude(json: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    value.as_array()?.iter().find_map(|entry| {
        let repo = entry.get("repo")?.as_str()?;
        (repo == WENLAN_REPO || repo == LEGACY_WENLAN_REPO)
            .then(|| entry.get("name")?.as_str().map(String::from))
            .flatten()
    })
}

/// Reads the registered marketplace name for `wenlan-app/wenlan` (or its
/// pre-move slug `LEGACY_WENLAN_REPO`) out of `codex plugin marketplace
/// list --json` output, e.g.
/// `{"marketplaces":[{"name":"wenlan-local","marketplaceSource":{"sourceType":"git","source":"https://github.com/wenlan-app/wenlan.git"}}]}`.
/// Matches by substring on the git URL rather than exact equality, since the
/// URL carries a protocol and `.git` suffix the bare repo slug doesn't.
fn find_marketplace_name_codex(json: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    value
        .get("marketplaces")?
        .as_array()?
        .iter()
        .find_map(|entry| {
            let source = entry.get("marketplaceSource")?.get("source")?.as_str()?;
            (source.contains(WENLAN_REPO) || source.contains(LEGACY_WENLAN_REPO))
                .then(|| entry.get("name")?.as_str().map(String::from))
                .flatten()
        })
}

/// Builds the `wenlan@<marketplace>` selector from a `marketplace list
/// --json` result, falling back to `fallback` if the process failed or the
/// output didn't parse. Pure — the process call is injected via `(stdout,
/// succeeded)` so probe/parse logic is unit-testable without spawning a CLI.
fn build_selector_with(
    stdout: &str,
    succeeded: bool,
    parse: impl Fn(&str) -> Option<String>,
    fallback: &str,
) -> String {
    let name = succeeded
        .then(|| parse(stdout))
        .flatten()
        .unwrap_or_else(|| fallback.to_string());
    format!("wenlan@{name}")
}

/// A command for the client's own CLI. In an isolated dev run the CLI sees
/// the run's stand-in home (see `identity_paths::ai_tool_dirs`), so a plugin
/// installed from a test copy of the app lands there, not in the user's real
/// `~/.claude` or `~/.codex`.
fn client_command(bin: &Path) -> Command {
    client_command_with_path(bin, std::env::var_os("PATH"))
}

fn codex_command(bin: &Path, codex_home: &Path) -> Command {
    codex_command_with_path(bin, std::env::var_os("PATH"), codex_home)
}

fn codex_command_with_path(
    bin: &Path,
    inherited_path: Option<OsString>,
    codex_home: &Path,
) -> Command {
    let mut command = client_command_with_path(bin, inherited_path);
    // Always override an inherited empty or stale CODEX_HOME so the CLI uses
    // the same effective root Wenlan selected and prepared for this install.
    command.env("CODEX_HOME", codex_home);
    command
}

/// Builds a client command with the selected executable's directory at the
/// front of its PATH. Some CLI shims use `#!/usr/bin/env node`, so they need
/// sibling runtimes even when Wenlan itself was launched with a minimal GUI
/// PATH. Preserve every inherited PATH entry after the executable directory.
fn client_command_with_path(bin: &Path, inherited_path: Option<OsString>) -> Command {
    let mut command = Command::new(bin);
    if bin.is_absolute() {
        if let Some(parent) = bin.parent().filter(|parent| parent.is_absolute()) {
            let mut paths = vec![parent.to_path_buf()];
            if let Some(inherited_path) = inherited_path {
                paths.extend(std::env::split_paths(&inherited_path));
            }
            if let Ok(path) = std::env::join_paths(paths) {
                command.env("PATH", path);
            }
        }
    }
    if crate::identity_paths::isolated_dev_state_dir().is_some() {
        if let (Some(home), _) = crate::identity_paths::ai_tool_dirs() {
            command.env("HOME", &home).env("USERPROFILE", &home);
        }
    }
    command
}

fn resolve_selector(
    bin: &Path,
    codex_home: Option<&Path>,
    parse: impl Fn(&str) -> Option<String>,
    fallback: &str,
) -> String {
    let mut command = match codex_home {
        Some(codex_home) => codex_command(bin, codex_home),
        None => client_command(bin),
    };
    match command
        .args(["plugin", "marketplace", "list", "--json"])
        .output()
    {
        Ok(output) => build_selector_with(
            &String::from_utf8_lossy(&output.stdout),
            output.status.success(),
            parse,
            fallback,
        ),
        Err(_) => build_selector_with("", false, parse, fallback),
    }
}

// ── Output classification ───────────────────────────────────────────────

/// Strips ANSI SGR escape sequences (`ESC [ ... m`) — both CLIs colorize
/// failure output even without a TTY, but not success output.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Codex prints a benign PATH-alias warning on stderr on both success and
/// failure paths; strip it so a failure message doesn't lead with noise.
fn strip_warning_lines(s: &str) -> String {
    s.lines()
        .filter(|line| !line.trim_start().starts_with("WARNING:"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Classifies a completed step: success on exit 0, success if the combined
/// output mentions "already" (both CLIs exit 0 on repeat too, but this
/// covers a hypothetical future CLI version that doesn't), otherwise a
/// `StepFailed` carrying the CLI's own message with ANSI codes and the
/// benign Codex warning line stripped out.
fn classify_step(
    step: &'static str,
    exit_code: Option<i32>,
    stdout: &str,
    stderr: &str,
) -> Result<(), PluginInstallError> {
    if exit_code == Some(0) {
        return Ok(());
    }
    let combined = format!("{stdout} {stderr}").to_lowercase();
    if combined.contains("already") {
        return Ok(());
    }
    let raw = if !stderr.trim().is_empty() {
        stderr
    } else {
        stdout
    };
    let filtered = strip_warning_lines(raw);
    let message_source = if filtered.trim().is_empty() {
        raw
    } else {
        &filtered
    };
    Err(PluginInstallError::StepFailed(format!(
        "{step}: {}",
        strip_ansi(message_source).trim()
    )))
}

fn run_step(
    bin: &Path,
    codex_home: Option<&Path>,
    step: &'static str,
    args: &[&str],
) -> Result<(), PluginInstallError> {
    let mut command = match codex_home {
        Some(codex_home) => codex_command(bin, codex_home),
        None => client_command(bin),
    };
    let output = command
        .args(args)
        .output()
        .map_err(|e| PluginInstallError::StepFailed(format!("{step}: {e}")))?;
    classify_step(
        step,
        output.status.code(),
        &String::from_utf8_lossy(&output.stdout),
        &String::from_utf8_lossy(&output.stderr),
    )
}

fn prepare_codex_home(codex_home: Option<PathBuf>) -> Result<PathBuf, PluginInstallError> {
    let codex_home = codex_home.ok_or_else(|| {
        PluginInstallError::StepFailed(
            "Codex state directory could not be determined (CODEX_HOME)".to_string(),
        )
    })?;
    std::fs::create_dir_all(&codex_home).map_err(|error| {
        PluginInstallError::StepFailed(format!(
            "Could not create Codex state directory {}: {error}",
            codex_home.display()
        ))
    })?;
    Ok(codex_home)
}

// ── Public install entry points ─────────────────────────────────────────

/// Installs the Wenlan Claude Code plugin: `claude plugin marketplace add
/// wenlan-app/wenlan`, then `claude plugin install wenlan@<resolved>`.
/// Idempotent — succeeds if the marketplace or plugin is already present.
pub fn install_claude_code_plugin() -> Result<(), PluginInstallError> {
    let bin = resolve_binary("claude").ok_or(PluginInstallError::CliNotFound("Claude Code"))?;
    run_step(
        &bin,
        None,
        "marketplace add",
        &["plugin", "marketplace", "add", WENLAN_REPO],
    )?;
    let selector = resolve_selector(
        &bin,
        None,
        find_marketplace_name_claude,
        FALLBACK_MARKETPLACE_CLAUDE,
    );
    run_step(
        &bin,
        None,
        "plugin install",
        &["plugin", "install", &selector],
    )?;
    Ok(())
}

/// Installs the Wenlan Codex plugin: `codex plugin marketplace add
/// wenlan-app/wenlan`, then `codex plugin add wenlan@<resolved>` (note: `add`,
/// not `install` — Codex's subcommand name differs from Claude Code's).
/// Idempotent — succeeds if the marketplace or plugin is already present.
pub fn install_codex_plugin() -> Result<(), PluginInstallError> {
    let bin = resolve_binary("codex").ok_or(PluginInstallError::CliNotFound("Codex"))?;
    let codex_home = prepare_codex_home(crate::identity_paths::codex_home_dir())?;
    run_step(
        &bin,
        Some(&codex_home),
        "marketplace add",
        &["plugin", "marketplace", "add", WENLAN_REPO],
    )?;
    let selector = resolve_selector(
        &bin,
        Some(&codex_home),
        find_marketplace_name_codex,
        FALLBACK_MARKETPLACE_CODEX,
    );
    run_step(
        &bin,
        Some(&codex_home),
        "plugin add",
        &["plugin", "add", &selector],
    )?;
    Ok(())
}

/// Installs the Wenlan plugin for `client_type` (`"claude_code"` /
/// `"codex_cli"`, matching the wizard's client-type strings elsewhere in the
/// codebase — see `mcp_config::detect_mcp_clients`). The single dispatch
/// point `search::install_client_plugin` calls into.
pub fn install_client_plugin(client_type: &str) -> Result<(), PluginInstallError> {
    match client_type {
        "claude_code" => install_claude_code_plugin(),
        "codex_cli" => install_codex_plugin(),
        other => Err(PluginInstallError::UnknownClient(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── test fixtures ───────────────────────────────────────────────────

    fn unix_env(home: &str, path: &[&str]) -> LookupEnv {
        LookupEnv {
            home: Some(PathBuf::from(home)),
            path_var: Some(std::env::join_paths(path).expect("join PATH")),
            windows: false,
            macos: false,
            appdata: None,
            local_appdata: None,
        }
    }

    fn windows_env() -> LookupEnv {
        LookupEnv {
            home: Some(PathBuf::from("/users/u")),
            path_var: Some(std::env::join_paths(["/tools/bin", "/more/bin"]).expect("join PATH")),
            windows: true,
            macos: false,
            appdata: Some(PathBuf::from("/users/u/AppData/Roaming")),
            local_appdata: Some(PathBuf::from("/users/u/AppData/Local")),
        }
    }

    fn macos_env(home: &str, path: &[&str]) -> LookupEnv {
        LookupEnv {
            home: Some(PathBuf::from(home)),
            path_var: Some(std::env::join_paths(path).expect("join PATH")),
            windows: false,
            macos: true,
            appdata: None,
            local_appdata: None,
        }
    }

    fn no_dirs(_: &Path) -> Vec<PathBuf> {
        Vec::new()
    }

    #[cfg(unix)]
    #[test]
    fn client_command_finds_sibling_runtime_and_preserves_inherited_path() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let bin_dir = temp.path().join("nvm-node-bin");
        std::fs::create_dir(&bin_dir).unwrap();
        let node = bin_dir.join("node");
        let shim = bin_dir.join("codex");
        std::fs::write(&node, "#!/bin/sh\nprintf 'sibling-node-ran\\n'\n").unwrap();
        std::fs::write(&shim, "#!/usr/bin/env node\n").unwrap();
        for path in [&node, &shim] {
            let mut permissions = std::fs::metadata(path).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(path, permissions).unwrap();
        }

        let inherited = std::env::join_paths(["/usr/bin", "/bin"]).unwrap();
        let mut command = client_command_with_path(&shim, Some(inherited));
        let configured_path = command
            .get_envs()
            .find(|(name, _)| *name == std::ffi::OsStr::new("PATH"))
            .and_then(|(_, value)| value)
            .expect("client command should set PATH");
        assert_eq!(
            std::env::split_paths(configured_path).collect::<Vec<_>>(),
            vec![
                bin_dir.clone(),
                PathBuf::from("/usr/bin"),
                PathBuf::from("/bin")
            ],
            "prepend the selected executable directory and retain inherited entries"
        );

        let output = command.output().unwrap();
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "sibling-node-ran\n"
        );
    }

    #[test]
    fn client_command_leaves_path_inherited_for_nonabsolute_executables() {
        let inherited = std::env::join_paths(["/usr/bin", "/bin"]).unwrap();
        let command = client_command_with_path(Path::new("codex"), Some(inherited));
        assert!(command
            .get_envs()
            .all(|(name, _)| name != std::ffi::OsStr::new("PATH")));
    }

    #[test]
    fn codex_command_sets_the_selected_home_over_inherited_environment() {
        let temp = tempfile::tempdir().unwrap();
        let codex_home = temp.path().join("chosen-codex-home");
        let inherited = std::env::join_paths(["/usr/bin", "/bin"]).unwrap();
        let command =
            codex_command_with_path(Path::new("/tools/bin/codex"), Some(inherited), &codex_home);

        let selected = command
            .get_envs()
            .find(|(name, _)| *name == std::ffi::OsStr::new("CODEX_HOME"))
            .and_then(|(_, value)| value)
            .expect("Codex command should explicitly set CODEX_HOME");
        assert_eq!(PathBuf::from(selected), codex_home);
    }

    #[test]
    fn prepare_codex_home_creates_the_selected_root() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("not-created-yet").join(".codex");

        assert!(!root.exists());
        let selected = prepare_codex_home(Some(root.clone())).unwrap();

        assert_eq!(selected, root);
        assert!(selected.is_dir());
    }

    #[test]
    fn prepare_codex_home_reports_creation_and_resolution_failures() {
        let temp = tempfile::tempdir().unwrap();
        let blocker = temp.path().join("file-not-directory");
        std::fs::write(&blocker, "block").unwrap();
        let impossible_child = blocker.join(".codex");

        let error = prepare_codex_home(Some(impossible_child.clone())).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("Could not create Codex state directory"));
        assert!(message.contains(&impossible_child.display().to_string()));

        let error = prepare_codex_home(None).unwrap_err();
        assert!(error.to_string().contains("CODEX_HOME"));
    }

    // ── PATH search ─────────────────────────────────────────────────────

    #[test]
    fn path_candidates_follow_path_order_with_bare_name_on_unix() {
        let env = unix_env("/home/u", &["/a/bin", "/b/bin"]);
        assert_eq!(
            path_candidates(&env, "codex"),
            vec![PathBuf::from("/a/bin/codex"), PathBuf::from("/b/bin/codex")]
        );
    }

    #[test]
    fn path_candidates_try_exe_cmd_bat_per_dir_on_windows() {
        let env = windows_env();
        assert_eq!(
            path_candidates(&env, "claude"),
            vec![
                PathBuf::from("/tools/bin/claude.exe"),
                PathBuf::from("/tools/bin/claude.cmd"),
                PathBuf::from("/tools/bin/claude.bat"),
                PathBuf::from("/more/bin/claude.exe"),
                PathBuf::from("/more/bin/claude.cmd"),
                PathBuf::from("/more/bin/claude.bat"),
            ]
        );
    }

    #[test]
    fn path_candidates_skip_relative_and_empty_entries() {
        // A relative PATH entry resolves against the GUI app's arbitrary cwd.
        let env = unix_env("/home/u", &["", ".", "bin", "/abs/bin"]);
        assert_eq!(
            path_candidates(&env, "codex"),
            vec![PathBuf::from("/abs/bin/codex")]
        );
    }

    #[test]
    fn path_candidates_empty_without_a_path_variable() {
        let mut env = unix_env("/home/u", &[]);
        env.path_var = None;
        assert!(path_candidates(&env, "codex").is_empty());
    }

    // ── known install dirs ──────────────────────────────────────────────

    #[test]
    fn known_dirs_claude_includes_self_managed_dir_before_system_dirs() {
        let env = unix_env("/home/u", &[]);
        let dirs = known_install_dirs(&env, "claude", &no_dirs);
        assert_eq!(
            &dirs[..4],
            &[
                PathBuf::from("/home/u/.local/bin"),
                PathBuf::from("/home/u/.claude/local"),
                PathBuf::from("/opt/homebrew/bin"),
                PathBuf::from("/usr/local/bin"),
            ]
        );
    }

    #[test]
    fn known_dirs_codex_excludes_claude_local_dir() {
        let env = unix_env("/home/u", &[]);
        let dirs = known_install_dirs(&env, "codex", &no_dirs);
        assert!(!dirs.contains(&PathBuf::from("/home/u/.claude/local")));
        assert_eq!(dirs[0], PathBuf::from("/home/u/.local/bin"));
    }

    #[test]
    fn known_dirs_list_every_nvm_node_version_newest_first() {
        let env = unix_env("/home/u", &[]);
        let list = |dir: &Path| {
            if dir == Path::new("/home/u/.nvm/versions/node") {
                vec![
                    PathBuf::from("/home/u/.nvm/versions/node/v18.20.4"),
                    PathBuf::from("/home/u/.nvm/versions/node/v24.11.1"),
                    PathBuf::from("/home/u/.nvm/versions/node/v9.0.0"),
                ]
            } else {
                Vec::new()
            }
        };
        let dirs = known_install_dirs(&env, "codex", &list);
        let nvm: Vec<_> = dirs
            .iter()
            .filter(|d| d.starts_with("/home/u/.nvm"))
            .cloned()
            .collect();
        assert_eq!(
            nvm,
            vec![
                PathBuf::from("/home/u/.nvm/versions/node/v24.11.1/bin"),
                PathBuf::from("/home/u/.nvm/versions/node/v18.20.4/bin"),
                PathBuf::from("/home/u/.nvm/versions/node/v9.0.0/bin"),
            ],
            "numeric version order, not lexical (v9 must sort below v18)"
        );
    }

    #[test]
    fn known_dirs_on_windows_use_appdata_npm_and_skip_unix_prefixes() {
        let dirs = known_install_dirs(&windows_env(), "codex", &no_dirs);
        assert!(dirs.contains(&PathBuf::from("/users/u/AppData/Roaming/npm")));
        assert!(dirs.contains(&PathBuf::from(
            "/users/u/AppData/Local/Microsoft/WinGet/Links"
        )));
        assert!(!dirs.contains(&PathBuf::from("/opt/homebrew/bin")));
        assert!(!dirs.contains(&PathBuf::from("/usr/local/bin")));
    }

    #[test]
    fn known_candidates_on_windows_find_the_npm_cmd_shim() {
        let env = windows_env();
        let want = PathBuf::from("/users/u/AppData/Roaming/npm/codex.cmd");
        let exists = |p: &Path| p == want;
        assert_eq!(
            resolve_binary_with("codex", &env, exists, no_dirs, |_| None),
            Some(PathBuf::from("/users/u/AppData/Roaming/npm/codex.cmd"))
        );
    }

    #[test]
    fn desktop_only_codex_install_resolves_the_chatgpt_bundle_cli() {
        let env = macos_env("/Users/u", &[]);
        let bundled = Path::new("/Applications/ChatGPT.app/Contents/Resources/codex-cli/bin/codex");
        let shell_calls = std::cell::Cell::new(0);
        let resolved = resolve_binary_with(
            "codex",
            &env,
            |path| path == bundled,
            no_dirs,
            |_| {
                shell_calls.set(shell_calls.get() + 1);
                None
            },
        );
        assert_eq!(resolved, Some(bundled.to_path_buf()));
        assert_eq!(shell_calls.get(), 0, "bundle hit should avoid shell lookup");
    }

    #[test]
    fn codex_bundle_search_includes_user_applications() {
        let env = macos_env("/Users/u", &[]);
        let system =
            PathBuf::from("/Applications/ChatGPT.app/Contents/Resources/codex-cli/bin/codex");
        let user = PathBuf::from(
            "/Users/u/Applications/ChatGPT.app/Contents/Resources/codex-cli/bin/codex",
        );
        assert_eq!(
            bundled_codex_candidates(&env, "codex"),
            vec![system, user.clone()]
        );
        assert_eq!(
            resolve_binary_with(
                "codex",
                &env,
                |candidate| candidate == user,
                no_dirs,
                |_| None
            ),
            Some(user)
        );
    }

    #[test]
    fn bundled_codex_is_excluded_for_other_platforms_and_binaries() {
        assert!(bundled_codex_candidates(&unix_env("/Users/u", &[]), "codex").is_empty());
        assert!(bundled_codex_candidates(&windows_env(), "codex").is_empty());
        assert!(bundled_codex_candidates(&macos_env("/Users/u", &[]), "claude").is_empty());
    }

    #[test]
    fn codex_bundle_does_not_override_path_or_standard_install_dirs() {
        let env = macos_env("/Users/u", &["/on/path"]);
        let path_cli = Path::new("/on/path/codex");
        let standard_cli = Path::new("/Users/u/.local/bin/codex");
        let bundled_cli =
            Path::new("/Applications/ChatGPT.app/Contents/Resources/codex-cli/bin/codex");
        let exists = |candidate: &Path| {
            candidate == path_cli || candidate == standard_cli || candidate == bundled_cli
        };
        assert_eq!(
            resolve_binary_with("codex", &env, exists, no_dirs, |_| None),
            Some(path_cli.to_path_buf())
        );

        let exists = |candidate: &Path| candidate == standard_cli || candidate == bundled_cli;
        assert_eq!(
            resolve_binary_with("codex", &env, exists, no_dirs, |_| None),
            Some(standard_cli.to_path_buf())
        );
    }

    #[test]
    fn missing_bundle_and_cli_still_returns_none() {
        let env = macos_env("/Users/u", &[]);
        assert_eq!(
            resolve_binary_with("codex", &env, |_| false, no_dirs, |_| None),
            None
        );
    }

    // ── resolve_binary_with: order ──────────────────────────────────────

    #[test]
    fn resolve_binary_prefers_path_over_known_dir_over_shell() {
        let env = unix_env("/home/u", &["/on/path"]);
        let exists = |p: &Path| {
            p == Path::new("/on/path/codex") || p == Path::new("/home/u/.local/bin/codex")
        };
        let shell = |_: &str| Some(PathBuf::from("/from/shell/codex"));
        assert_eq!(
            resolve_binary_with("codex", &env, exists, no_dirs, shell),
            Some(PathBuf::from("/on/path/codex"))
        );

        let exists = |p: &Path| p == Path::new("/home/u/.local/bin/codex");
        let shell = |_: &str| Some(PathBuf::from("/from/shell/codex"));
        assert_eq!(
            resolve_binary_with("codex", &env, exists, no_dirs, shell),
            Some(PathBuf::from("/home/u/.local/bin/codex"))
        );
    }

    #[test]
    fn resolve_binary_consults_the_shell_only_when_nothing_else_matched() {
        let env = unix_env("/home/u", &["/on/path"]);
        let shell_calls = std::cell::Cell::new(0);
        let shell = |_: &str| {
            shell_calls.set(shell_calls.get() + 1);
            Some(PathBuf::from("/from/shell/codex"))
        };
        let resolved = resolve_binary_with("codex", &env, |_| false, no_dirs, shell);
        assert_eq!(resolved, Some(PathBuf::from("/from/shell/codex")));
        assert_eq!(shell_calls.get(), 1);

        let shell_calls = std::cell::Cell::new(0);
        let shell = |_: &str| {
            shell_calls.set(shell_calls.get() + 1);
            None
        };
        let _ = resolve_binary_with("codex", &env, |_| true, no_dirs, shell);
        assert_eq!(shell_calls.get(), 0, "a PATH hit must not spawn a shell");
    }

    #[test]
    fn resolve_binary_none_when_nothing_matches() {
        let env = unix_env("/home/u", &["/on/path"]);
        assert_eq!(
            resolve_binary_with("codex", &env, |_| false, no_dirs, |_| None),
            None
        );
    }

    #[test]
    fn resolve_binary_without_home_or_path_still_checks_system_dirs() {
        // No home means no per-user dirs, but /opt/homebrew/bin and
        // /usr/local/bin are home-independent and must still be probed.
        let env = LookupEnv::default();
        let exists = |p: &Path| p == Path::new("/opt/homebrew/bin/codex");
        assert_eq!(
            resolve_binary_with("codex", &env, exists, no_dirs, |_| None),
            Some(PathBuf::from("/opt/homebrew/bin/codex"))
        );
        let exists = |p: &Path| p.starts_with("/home/u");
        assert_eq!(
            resolve_binary_with("codex", &env, exists, no_dirs, |_| None),
            None,
            "per-user dirs must not be probed without a home"
        );
    }

    // ── bounded shell fallback (real subprocesses, Unix only) ───────────

    #[cfg(unix)]
    fn write_fake_shell(dir: &Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("fake-shell");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[cfg(unix)]
    #[test]
    fn shell_lookup_takes_last_absolute_line_and_passes_login_interactive_flags() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("codex");
        std::fs::write(&target, "#!/bin/sh\n").unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let argv_log = dir.path().join("argv.txt");
        let shell = write_fake_shell(
            dir.path(),
            &format!(
                "printf '%s|' \"$@\" > '{}'\nprintf 'welcome banner\\nalias codex=foo\\n{}\\n'",
                argv_log.display(),
                target.display()
            ),
        );
        let found = shell_lookup("codex", &shell, Duration::from_secs(5));
        assert_eq!(found, Some(target));
        assert_eq!(
            std::fs::read_to_string(&argv_log).unwrap(),
            "-l|-i|-c|command -v codex|"
        );
    }

    #[cfg(unix)]
    #[test]
    fn shell_lookup_gives_up_on_a_hanging_shell_within_the_bound() {
        let dir = tempfile::tempdir().unwrap();
        let shell = write_fake_shell(dir.path(), "sleep 30");
        let started = Instant::now();
        let found = shell_lookup("codex", &shell, Duration::from_millis(300));
        assert_eq!(found, None);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "hung shell must be abandoned at the bound, took {:?}",
            started.elapsed()
        );
    }

    #[cfg(unix)]
    #[test]
    fn shell_lookup_ignores_a_non_path_answer_and_a_failing_shell() {
        let dir = tempfile::tempdir().unwrap();
        let alias_only = write_fake_shell(dir.path(), "echo 'alias codex=foo'");
        assert_eq!(
            shell_lookup("codex", &alias_only, Duration::from_secs(5)),
            None
        );
        let failing = write_fake_shell(dir.path(), "echo /bin/sh; exit 1");
        assert_eq!(
            shell_lookup("codex", &failing, Duration::from_secs(5)),
            None
        );
    }

    #[test]
    fn shell_lookup_rejects_binary_names_that_could_inject_shell_syntax() {
        let sh = Path::new("/bin/sh");
        assert_eq!(
            shell_lookup("codex; touch /tmp/x", sh, Duration::from_secs(1)),
            None
        );
        assert_eq!(shell_lookup("", sh, Duration::from_secs(1)), None);
    }

    // ── find_marketplace_name_claude ────────────────────────────────────

    const CLAUDE_MARKETPLACE_JSON: &str = r#"[
  {
    "name": "7xuanlu-wenlan",
    "source": "github",
    "repo": "wenlan-app/wenlan",
    "installLocation": "/home/u/.claude/plugins/marketplaces/7xuanlu-wenlan"
  }
]"#;

    #[test]
    fn find_marketplace_name_claude_matches_by_repo() {
        assert_eq!(
            find_marketplace_name_claude(CLAUDE_MARKETPLACE_JSON),
            Some("7xuanlu-wenlan".to_string())
        );
    }

    #[test]
    fn find_marketplace_name_claude_matches_pre_move_repo() {
        // Installs made before the repo moved to the org stay registered
        // under the old slug.
        let json = CLAUDE_MARKETPLACE_JSON.replace(WENLAN_REPO, LEGACY_WENLAN_REPO);
        assert_eq!(
            find_marketplace_name_claude(&json),
            Some("7xuanlu-wenlan".to_string())
        );
    }

    #[test]
    fn find_marketplace_name_claude_ignores_other_repos() {
        let json = r#"[{"name":"other","source":"github","repo":"someone/else"}]"#;
        assert_eq!(find_marketplace_name_claude(json), None);
    }

    #[test]
    fn find_marketplace_name_claude_none_on_garbage() {
        assert_eq!(find_marketplace_name_claude("not json"), None);
    }

    // ── find_marketplace_name_codex ─────────────────────────────────────

    /// Shape captured from codex-cli 0.144.0, which derived `wenlan-local`.
    const CODEX_MARKETPLACE_JSON: &str = r#"{
  "marketplaces": [
    {
      "name": "wenlan-local",
      "root": "/home/u/.codex/.tmp/marketplaces/wenlan-local",
      "marketplaceSource": {
        "sourceType": "git",
        "source": "https://github.com/wenlan-app/wenlan.git"
      }
    }
  ]
}"#;

    #[test]
    fn find_marketplace_name_codex_matches_by_source_url_substring() {
        assert_eq!(
            find_marketplace_name_codex(CODEX_MARKETPLACE_JSON),
            Some("wenlan-local".to_string())
        );
    }

    #[test]
    fn find_marketplace_name_codex_matches_pre_move_repo() {
        // Installs made before the repo moved to the org stay registered
        // under the old source URL.
        let json = CODEX_MARKETPLACE_JSON.replace(WENLAN_REPO, LEGACY_WENLAN_REPO);
        assert_eq!(
            find_marketplace_name_codex(&json),
            Some("wenlan-local".to_string())
        );
    }

    #[test]
    fn find_marketplace_name_codex_matches_renamed_marketplace() {
        // Older Codex builds registered this repo as `wenlan-local`; newer
        // ones use the manifest's `7xuanlu-wenlan`. The source URL (what we
        // match on) is the same either way, so resolution needs no change.
        let json = CODEX_MARKETPLACE_JSON.replace("wenlan-local", "7xuanlu-wenlan");
        assert_eq!(
            find_marketplace_name_codex(&json),
            Some("7xuanlu-wenlan".to_string())
        );
    }

    #[test]
    fn find_marketplace_name_codex_ignores_other_repos() {
        let json = r#"{"marketplaces":[{"name":"other","marketplaceSource":{"sourceType":"git","source":"https://github.com/someone/else.git"}}]}"#;
        assert_eq!(find_marketplace_name_codex(json), None);
    }

    #[test]
    fn find_marketplace_name_codex_none_on_garbage() {
        assert_eq!(find_marketplace_name_codex("not json"), None);
    }

    // ── build_selector_with ─────────────────────────────────────────────

    #[test]
    fn build_selector_uses_resolved_name_on_success() {
        let selector = build_selector_with(
            CODEX_MARKETPLACE_JSON,
            true,
            find_marketplace_name_codex,
            FALLBACK_MARKETPLACE_CODEX,
        );
        assert_eq!(selector, "wenlan@wenlan-local");
    }

    #[test]
    fn build_selector_falls_back_when_process_failed() {
        let selector = build_selector_with(
            CODEX_MARKETPLACE_JSON,
            false,
            find_marketplace_name_codex,
            FALLBACK_MARKETPLACE_CODEX,
        );
        assert_eq!(selector, format!("wenlan@{FALLBACK_MARKETPLACE_CODEX}"));
    }

    #[test]
    fn build_selector_falls_back_when_parse_fails() {
        let selector = build_selector_with(
            "not json",
            true,
            find_marketplace_name_codex,
            FALLBACK_MARKETPLACE_CODEX,
        );
        assert_eq!(selector, format!("wenlan@{FALLBACK_MARKETPLACE_CODEX}"));
    }

    /// The `name` field of a marketplace manifest checked into this repo.
    fn manifest_name(relative: &str) -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join(relative);
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let value: serde_json::Value =
            serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));
        value["name"]
            .as_str()
            .unwrap_or_else(|| panic!("{} has no string `name`", path.display()))
            .to_string()
    }

    #[test]
    fn fallback_marketplace_names_match_the_checked_in_manifests() {
        // The fallback is what gets installed when `marketplace list --json`
        // cannot be read; if it names a marketplace the CLI never registered,
        // `plugin add wenlan@<name>` fails with "marketplace not found".
        assert_eq!(
            FALLBACK_MARKETPLACE_CLAUDE,
            manifest_name(".claude-plugin/marketplace.json")
        );
        assert_eq!(
            FALLBACK_MARKETPLACE_CODEX,
            manifest_name(".agents/plugins/marketplace.json")
        );
    }

    #[test]
    fn build_selector_fallback_uses_each_clients_manifest_name() {
        let claude_selector = build_selector_with(
            "not json",
            false,
            find_marketplace_name_claude,
            FALLBACK_MARKETPLACE_CLAUDE,
        );
        let codex_selector = build_selector_with(
            "not json",
            false,
            find_marketplace_name_codex,
            FALLBACK_MARKETPLACE_CODEX,
        );
        assert_eq!(claude_selector, "wenlan@7xuanlu-wenlan");
        assert_eq!(codex_selector, "wenlan@7xuanlu-wenlan");
    }

    // ── strip_ansi / strip_warning_lines ────────────────────────────────

    #[test]
    fn strip_ansi_removes_sgr_codes() {
        assert_eq!(
            strip_ansi("\u{1b}[31mError:\u{1b}[0m broke"),
            "Error: broke"
        );
    }

    #[test]
    fn strip_ansi_passes_through_plain_text() {
        assert_eq!(strip_ansi("plain text"), "plain text");
    }

    #[test]
    fn strip_warning_lines_removes_only_warning_prefixed_lines() {
        let input = "WARNING: proceeding, even though...\nError: plugin not found";
        assert_eq!(strip_warning_lines(input), "Error: plugin not found");
    }

    #[test]
    fn strip_warning_lines_keeps_non_warning_content_untouched() {
        assert_eq!(
            strip_warning_lines("Error: plugin not found"),
            "Error: plugin not found"
        );
    }

    // ── classify_step ────────────────────────────────────────────────────

    #[test]
    fn classify_step_success_on_exit_zero() {
        assert_eq!(classify_step("marketplace add", Some(0), "", ""), Ok(()));
    }

    #[test]
    fn classify_step_success_when_output_mentions_already() {
        assert_eq!(
            classify_step("plugin install", Some(1), "already installed", ""),
            Ok(())
        );
    }

    #[test]
    fn classify_step_real_claude_failure_message_is_ansi_stripped() {
        // Captured verbatim from `claude plugin install wenlan@nonexistent`
        // against a throwaway HOME.
        let stderr = "\u{1b}[31mError: Marketplace \"nonexistent\" not found\u{1b}[0m\n";
        let result = classify_step("plugin install", Some(1), "", stderr);
        assert_eq!(
            result,
            Err(PluginInstallError::StepFailed(
                "plugin install: Error: Marketplace \"nonexistent\" not found".to_string()
            ))
        );
    }

    #[test]
    fn classify_step_real_codex_failure_strips_warning_line() {
        // Captured verbatim from `codex plugin add wenlan@nonexistent`
        // against a throwaway HOME.
        let stderr = "WARNING: proceeding, even though we could not create PATH aliases: Refusing to create helper binaries under temporary dir\nError: plugin `wenlan` was not found in marketplace `nonexistent`";
        let result = classify_step("plugin add", Some(1), "", stderr);
        assert_eq!(
            result,
            Err(PluginInstallError::StepFailed(
                "plugin add: Error: plugin `wenlan` was not found in marketplace `nonexistent`"
                    .to_string()
            ))
        );
    }

    #[test]
    fn classify_step_falls_back_to_stdout_when_stderr_empty() {
        let result = classify_step("plugin add", Some(1), "install failed", "");
        assert_eq!(
            result,
            Err(PluginInstallError::StepFailed(
                "plugin add: install failed".to_string()
            ))
        );
    }

    #[test]
    fn classify_step_keeps_raw_message_if_warning_strip_empties_it() {
        // Defensive: if stderr were somehow ONLY a WARNING line, don't
        // collapse the error message to nothing.
        let stderr = "WARNING: only a warning, no error text";
        let result = classify_step("plugin add", Some(1), "", stderr);
        assert_eq!(
            result,
            Err(PluginInstallError::StepFailed(
                "plugin add: WARNING: only a warning, no error text".to_string()
            ))
        );
    }

    // ── install_client_plugin dispatcher ────────────────────────────────

    #[test]
    fn install_client_plugin_rejects_unsupported_client_type() {
        // Only the two error-free branches spawn a process (unreachable in
        // a unit test without a real CLI); the unsupported branch is pure
        // and directly testable.
        assert_eq!(
            install_client_plugin("cursor"),
            Err(PluginInstallError::UnknownClient("cursor".to_string()))
        );
    }
}
