// SPDX-License-Identifier: Apache-2.0
//! Keep `~/.wenlan/bin` pointed at the desktop app's binaries.
//!
//! `install.sh` and `npx wenlan` write regular files named `wenlan`,
//! `wenlan-server`, and `wenlan-mcp` there and put that directory first on
//! `PATH`. The desktop updater replaces only the app bundle. It records
//! `wenlan-mcp.path` for the Claude and Codex plugin runners, and nothing
//! else reads that file: a shell, and an MCP config that names
//! `~/.wenlan/bin/wenlan-mcp`, keep executing the old files against the new
//! daemon.
//!
//! A release app launch (and `wenlan doctor --repair` / `wenlan setup`) moves
//! an older copy aside under a `.bak.<version>.<time>` name and links the
//! entry at the bundle. The previous file is never deleted. A newer copy is
//! left where it is. Windows uses a symlink when the OS allows it and otherwise
//! copies, and only over a file that is already there.

use std::cmp::Ordering;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

/// Relative path of the one-line file the app writes so plugin runners can
/// find a bundle that is not in a fixed install directory.
pub const RECORDED_MCP_PATH_REL: &str = ".wenlan/bin/wenlan-mcp.path";

const RUNTIME_STEMS: [&str; 3] = ["wenlan", "wenlan-server", "wenlan-mcp"];

/// Where a desktop install keeps the three runtime binaries next to the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopHost {
    MacOs,
    Linux,
    Windows,
}

impl DesktopHost {
    pub fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Linux
        }
    }
}

/// How a refreshed entry is installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkMode {
    /// Unix default. The link keeps working when the app replaces the bundle
    /// file at the same path.
    Symlink,
    /// Write a new regular file. Used when a symlink cannot be created.
    Copy,
    /// Symlink, and fall back to a copy only when replacing a file that is
    /// already there. A failed symlink does not seed a fresh copy.
    SymlinkOrCopy,
    /// Write regular copies, including where nothing is there yet. For an app
    /// whose own directory does not outlive the run (a Linux AppImage mounts
    /// itself fresh on every launch), so a link would dangle once it quits.
    CopyAll,
}

pub fn link_mode_for(host: DesktopHost) -> LinkMode {
    match host {
        DesktopHost::Windows => LinkMode::SymlinkOrCopy,
        DesktopHost::MacOs | DesktopHost::Linux => LinkMode::Symlink,
    }
}

/// A debug build, or any run pointed at a scratch data directory, must not
/// rewrite the user's real `~/.wenlan/bin`.
pub fn may_rewrite_user_home_bin(debug_build: bool, data_dir_overridden: bool) -> bool {
    !debug_build && !data_dir_overridden
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    major: u64,
    minor: u64,
    patch: u64,
    pre: Option<String>,
}

impl Version {
    pub fn parse(raw: &str) -> Result<Self, String> {
        let raw = raw.trim();
        let raw = raw.strip_prefix('v').unwrap_or(raw);
        let raw = raw.split('+').next().unwrap_or(raw);
        if raw.is_empty() {
            return Err("empty version".to_string());
        }
        let (numbers, pre) = match raw.split_once('-') {
            Some((numbers, pre)) if !pre.is_empty() => (numbers, Some(pre.to_string())),
            Some(_) => return Err(format!("invalid version {raw:?}")),
            None => (raw, None),
        };
        let mut parts = numbers.split('.');
        let major = parse_component(parts.next(), raw)?;
        let minor = parse_component(parts.next(), raw)?;
        let patch = parse_component(parts.next(), raw)?;
        if parts.next().is_some() {
            return Err(format!("invalid version {raw:?}"));
        }
        Ok(Self {
            major,
            minor,
            patch,
            pre,
        })
    }
}

fn parse_component(part: Option<&str>, raw: &str) -> Result<u64, String> {
    part.ok_or_else(|| format!("invalid version {raw:?}"))?
        .parse()
        .map_err(|_| format!("invalid version {raw:?}"))
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.major
            .cmp(&other.major)
            .then(self.minor.cmp(&other.minor))
            .then(self.patch.cmp(&other.patch))
            .then(match (&self.pre, &other.pre) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(left), Some(right)) => cmp_pre(left, right),
            })
    }
}

fn cmp_pre(left: &str, right: &str) -> Ordering {
    let left: Vec<&str> = left.split('.').collect();
    let right: Vec<&str> = right.split('.').collect();
    for (a, b) in left.iter().zip(right.iter()) {
        let ord = match (a.parse::<u64>(), b.parse::<u64>()) {
            (Ok(a), Ok(b)) => a.cmp(&b),
            (Ok(_), Err(_)) => Ordering::Less,
            (Err(_), Ok(_)) => Ordering::Greater,
            (Err(_), Err(_)) => (*a).cmp(*b),
        };
        if ord != Ordering::Equal {
            return ord;
        }
    }
    left.len().cmp(&right.len())
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if let Some(pre) = &self.pre {
            write!(f, "-{pre}")?;
        }
        Ok(())
    }
}

/// First line of a `--version` printout (`wenlan 0.18.10`).
pub fn parse_version_output(text: &str) -> Result<Version, String> {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .ok_or("version output was empty")?;
    let token = line
        .split_whitespace()
        .last()
        .ok_or("version output was empty")?;
    Version::parse(token)
}

pub fn backup_stamp_now() -> String {
    chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string()
}

pub fn home_bin_dir(home: &Path) -> PathBuf {
    home.join(".wenlan").join("bin")
}

pub fn executable_name(stem: &str, host: DesktopHost) -> String {
    match host {
        DesktopHost::Windows => format!("{stem}.exe"),
        DesktopHost::MacOs | DesktopHost::Linux => stem.to_string(),
    }
}

fn app_marker_name(host: DesktopHost) -> &'static str {
    match host {
        DesktopHost::Windows => "wenlan-app.exe",
        DesktopHost::MacOs | DesktopHost::Linux => "wenlan-app",
    }
}

pub fn default_bundle_candidates(
    home: &Path,
    host: DesktopHost,
    local_data: Option<&Path>,
    program_files: Option<&Path>,
) -> Vec<PathBuf> {
    match host {
        DesktopHost::MacOs => vec![
            PathBuf::from("/Applications/Wenlan.app/Contents/MacOS"),
            home.join("Applications/Wenlan.app/Contents/MacOS"),
        ],
        DesktopHost::Linux => vec![
            PathBuf::from("/usr/bin"),
            PathBuf::from("/usr/lib/wenlan"),
            PathBuf::from("/opt/Wenlan"),
        ],
        DesktopHost::Windows => {
            let mut dirs = Vec::new();
            if let Some(local) = local_data {
                dirs.push(local.join("Programs").join("Wenlan"));
                dirs.push(local.join("Wenlan"));
            }
            if let Some(program_files) = program_files {
                dirs.push(program_files.join("Wenlan"));
            }
            dirs
        }
    }
}

/// A directory that contains the desktop app and the three runtime binaries,
/// and is not `~/.wenlan/bin` itself.
pub fn dir_is_app_bundle(dir: &Path, host: DesktopHost) -> bool {
    dir.join(app_marker_name(host)).is_file()
        && RUNTIME_STEMS
            .iter()
            .all(|stem| is_runnable_path(&dir.join(executable_name(stem, host))))
}

pub struct DiscoverRequest<'a> {
    pub home: &'a Path,
    pub host: DesktopHost,
    pub current_exe: Option<&'a Path>,
    pub override_dir: Option<&'a Path>,
    pub candidates: &'a [PathBuf],
}

/// First real app bundle among the override, this process's directory, the
/// path recorded for the plugin runners, and `candidates`.
pub fn discover_bundle_dir(request: &DiscoverRequest<'_>) -> Option<PathBuf> {
    let home_bin = home_bin_dir(request.home);
    let mut ordered = Vec::new();
    if let Some(dir) = request.override_dir {
        ordered.push(dir.to_path_buf());
    }
    if let Some(parent) = request.current_exe.and_then(|exe| exe.parent()) {
        ordered.push(parent.to_path_buf());
    }
    if let Some(parent) =
        read_recorded_mcp_path(request.home).and_then(|path| path.parent().map(Path::to_path_buf))
    {
        ordered.push(parent);
    }
    ordered.extend(request.candidates.iter().cloned());
    ordered
        .into_iter()
        .find(|dir| !same_dir(dir, &home_bin) && dir_is_app_bundle(dir, request.host))
}

fn read_recorded_mcp_path(home: &Path) -> Option<PathBuf> {
    let text = fs::read_to_string(home.join(RECORDED_MCP_PATH_REL)).ok()?;
    let line = text.lines().next()?.trim().trim_end_matches('\r');
    if line.is_empty() {
        None
    } else {
        Some(PathBuf::from(line))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinaryReport {
    pub host: DesktopHost,
    pub cli_version: Version,
    pub daemon_version: Option<Version>,
    pub bundle_dir: Option<PathBuf>,
    pub bundle_version: Option<Version>,
    pub bundle_version_error: Option<String>,
    pub entries: Vec<ReportedEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportedEntry {
    pub name: String,
    pub path: PathBuf,
    pub kind: ReportedKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReportedKind {
    Absent,
    FollowsBundle,
    Regular { version: Version },
    OtherLink { version: Version },
    DanglingLink,
    Unreadable { detail: String },
    NotAProgram { detail: String },
}

pub struct InspectRequest<'a> {
    pub home: &'a Path,
    pub host: DesktopHost,
    pub bundle_dir: Option<&'a Path>,
    pub bundle_version: Option<&'a Version>,
    pub bundle_version_error: Option<&'a str>,
    pub cli_version: &'a Version,
    pub daemon_version: Option<&'a Version>,
}

pub fn inspect_home_bin(
    request: &InspectRequest<'_>,
    version_of: &mut dyn FnMut(&Path) -> Result<Version, String>,
) -> BinaryReport {
    let entries = RUNTIME_STEMS
        .iter()
        .map(|stem| {
            let path = home_bin_dir(request.home).join(executable_name(stem, request.host));
            let bundle_file = request
                .bundle_dir
                .map(|dir| dir.join(executable_name(stem, request.host)));
            let kind = reported_kind(&path, bundle_file.as_deref(), version_of);
            ReportedEntry {
                name: (*stem).to_string(),
                path,
                kind,
            }
        })
        .collect();
    BinaryReport {
        host: request.host,
        cli_version: request.cli_version.clone(),
        daemon_version: request.daemon_version.cloned(),
        bundle_dir: request.bundle_dir.map(Path::to_path_buf),
        bundle_version: request.bundle_version.cloned(),
        bundle_version_error: request.bundle_version_error.map(str::to_string),
        entries,
    }
}

fn reported_kind(
    path: &Path,
    bundle_file: Option<&Path>,
    version_of: &mut dyn FnMut(&Path) -> Result<Version, String>,
) -> ReportedKind {
    match classify(path, bundle_file, version_of) {
        State::Absent => ReportedKind::Absent,
        State::PointsAtBundle => ReportedKind::FollowsBundle,
        State::Versioned {
            version: Ok(version),
            via_link: false,
        } => ReportedKind::Regular { version },
        State::Versioned {
            version: Ok(version),
            via_link: true,
        } => ReportedKind::OtherLink { version },
        State::Versioned {
            version: Err(detail),
            ..
        } => ReportedKind::Unreadable { detail },
        State::DanglingLink => ReportedKind::DanglingLink,
        State::NotAProgram { detail } => ReportedKind::NotAProgram { detail },
    }
}

enum State {
    Absent,
    PointsAtBundle,
    Versioned {
        version: Result<Version, String>,
        via_link: bool,
    },
    DanglingLink,
    NotAProgram {
        detail: String,
    },
}

fn classify(
    path: &Path,
    bundle_file: Option<&Path>,
    version_of: &mut dyn FnMut(&Path) -> Result<Version, String>,
) -> State {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return State::Absent,
        Err(error) => {
            return State::NotAProgram {
                detail: error.to_string(),
            }
        }
    };
    if metadata.file_type().is_symlink() {
        if bundle_file.is_some_and(|bundle| points_at(path, bundle)) {
            return State::PointsAtBundle;
        }
        return match fs::metadata(path) {
            Ok(target) if target.is_file() && is_runnable_meta(&target) => State::Versioned {
                version: version_of(path),
                via_link: true,
            },
            Ok(_) => State::NotAProgram {
                detail: "the link does not point at a program".to_string(),
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => State::DanglingLink,
            Err(error) => State::NotAProgram {
                detail: error.to_string(),
            },
        };
    }
    if metadata.is_file() {
        if !is_runnable_meta(&metadata) {
            return State::NotAProgram {
                detail: if metadata.len() == 0 {
                    "the file is empty".to_string()
                } else {
                    "the file is not executable".to_string()
                },
            };
        }
        return State::Versioned {
            version: version_of(path),
            via_link: false,
        };
    }
    State::NotAProgram {
        detail: "not a file".to_string(),
    }
}

pub fn render_binary_report(report: &BinaryReport) -> String {
    if !report_has_concern(report) {
        return quiet_summary(report);
    }
    let mut lines = vec!["Binaries:".to_string()];
    lines.push(format!("  this wenlan: {}", report.cli_version));
    if let Some(version) = &report.daemon_version {
        lines.push(format!("  daemon: {version}"));
    }
    if let Some(dir) = &report.bundle_dir {
        let version = match (&report.bundle_version, &report.bundle_version_error) {
            (Some(version), _) => version.to_string(),
            (None, Some(error)) => format!("version unreadable ({error})"),
            (None, None) => "version unknown".to_string(),
        };
        lines.push(format!("  app bundle ({}): {version}", dir.display()));
    } else {
        lines.push("  app bundle: not found".to_string());
    }
    for entry in &report.entries {
        lines.push(format!("  {}", entry_line(entry)));
    }
    lines.extend(concern_notes(report));
    lines.join("\n")
}

fn entry_line(entry: &ReportedEntry) -> String {
    match &entry.kind {
        ReportedKind::Absent => format!("{}: not installed", entry.path.display()),
        ReportedKind::FollowsBundle => {
            format!("{}: links to the app bundle", entry.path.display())
        }
        ReportedKind::Regular { version } => {
            format!("{}: {version} (regular file)", entry.path.display())
        }
        ReportedKind::OtherLink { version } => {
            format!(
                "{}: {version} (link to another binary)",
                entry.path.display()
            )
        }
        ReportedKind::DanglingLink => {
            format!("{}: link target is missing", entry.path.display())
        }
        ReportedKind::Unreadable { detail } => {
            format!("{}: version unreadable ({detail})", entry.path.display())
        }
        ReportedKind::NotAProgram { detail } => {
            format!("{}: left in place ({detail})", entry.path.display())
        }
    }
}

fn quiet_summary(report: &BinaryReport) -> String {
    let mut places = vec!["this wenlan".to_string()];
    if report.entries.iter().any(|entry| {
        matches!(
            entry.kind,
            ReportedKind::FollowsBundle
                | ReportedKind::Regular { .. }
                | ReportedKind::OtherLink { .. }
        )
    }) {
        places.push("~/.wenlan/bin".to_string());
    }
    if report.bundle_version.is_some() {
        places.push("the app bundle".to_string());
    }
    if report.daemon_version.is_some() {
        places.push("the daemon".to_string());
    }
    format!("Binaries: {} ({}).", report.cli_version, join_and(&places))
}

fn join_and(places: &[String]) -> String {
    match places {
        [] => String::new(),
        [one] => one.clone(),
        [one, two] => format!("{one} and {two}"),
        _ => {
            let (last, rest) = places.split_last().expect("at least three");
            format!("{}, and {last}", rest.join(", "))
        }
    }
}

fn report_has_concern(report: &BinaryReport) -> bool {
    cli_is_behind(report)
        || report
            .entries
            .iter()
            .any(|entry| entry_concern(report, entry))
}

fn entry_concern(report: &BinaryReport, entry: &ReportedEntry) -> bool {
    match &entry.kind {
        ReportedKind::Absent | ReportedKind::FollowsBundle => false,
        ReportedKind::Regular { version } | ReportedKind::OtherLink { version } => {
            version_is_behind(version, report) || version_is_ahead_of_bundle(version, report)
        }
        ReportedKind::DanglingLink
        | ReportedKind::Unreadable { .. }
        | ReportedKind::NotAProgram { .. } => true,
    }
}

fn version_is_behind(version: &Version, report: &BinaryReport) -> bool {
    report
        .bundle_version
        .as_ref()
        .is_some_and(|bundle| version < bundle)
        || report
            .daemon_version
            .as_ref()
            .is_some_and(|daemon| version < daemon)
        || version < &report.cli_version
}

fn version_is_ahead_of_bundle(version: &Version, report: &BinaryReport) -> bool {
    report
        .bundle_version
        .as_ref()
        .is_some_and(|bundle| version > bundle)
}

fn cli_is_behind(report: &BinaryReport) -> bool {
    report
        .bundle_version
        .as_ref()
        .is_some_and(|bundle| report.cli_version < *bundle)
        || report
            .daemon_version
            .as_ref()
            .is_some_and(|daemon| report.cli_version < *daemon)
}

fn home_is_stale_against_bundle(report: &BinaryReport) -> bool {
    report.bundle_version.is_some()
        && report.entries.iter().any(|entry| match &entry.kind {
            ReportedKind::DanglingLink => true,
            ReportedKind::Regular { version } | ReportedKind::OtherLink { version } => report
                .bundle_version
                .as_ref()
                .is_some_and(|bundle| version < bundle),
            _ => false,
        })
}

fn concern_notes(report: &BinaryReport) -> Vec<String> {
    let mut notes = Vec::new();
    if home_is_stale_against_bundle(report) {
        let command = report.bundle_dir.as_ref().map(|dir| {
            dir.join(executable_name("wenlan", report.host))
                .display()
                .to_string()
        });
        notes.push(
            "WARNING: PATH and MCP configs that run ~/.wenlan/bin use an older Wenlan than the app \
             bundle, so they talk to a newer daemon with an older client."
                .to_string(),
        );
        match command {
            Some(command) => notes.push(format!(
                "Repair: open the Wenlan app, or run `{command} doctor --repair`."
            )),
            None => notes.push(
                "Repair: open the Wenlan app, or run the app's wenlan with `doctor --repair`."
                    .to_string(),
            ),
        }
        notes.push(
            "That moves each older file aside (a name ending in .bak.<version>.<time>) and points \
             ~/.wenlan/bin at the app bundle. Nothing already on disk is deleted."
                .to_string(),
        );
    } else if report.entries.iter().any(|entry| match &entry.kind {
        ReportedKind::Regular { version } | ReportedKind::OtherLink { version } => {
            version_is_behind(version, report)
        }
        _ => false,
    }) || cli_is_behind(report)
    {
        notes.push(
            "WARNING: ~/.wenlan/bin is older than the running daemon. Open the Wenlan app once so \
             it can point ~/.wenlan/bin at its own binaries."
                .to_string(),
        );
    }
    if report.entries.iter().any(|entry| match &entry.kind {
        ReportedKind::Regular { version } | ReportedKind::OtherLink { version } => {
            version_is_ahead_of_bundle(version, report)
        }
        _ => false,
    }) {
        notes.push(
            "~/.wenlan/bin has a newer copy than the app bundle. It was left in place so the \
             newer install is not replaced."
                .to_string(),
        );
    }
    if report
        .entries
        .iter()
        .any(|entry| matches!(entry.kind, ReportedKind::Unreadable { .. }))
    {
        notes.push(
            "A copy in ~/.wenlan/bin did not report a version, so it was left in place."
                .to_string(),
        );
    }
    notes
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryAction {
    Linked {
        path: PathBuf,
        target: PathBuf,
        backup: Option<PathBuf>,
    },
    Copied {
        path: PathBuf,
        target: PathBuf,
        backup: Option<PathBuf>,
    },
    AlreadyCurrent {
        path: PathBuf,
    },
    LeftNewer {
        path: PathBuf,
        version: Version,
    },
    LeftUnread {
        path: PathBuf,
        detail: String,
    },
    Skipped {
        path: PathBuf,
        detail: String,
        exists: bool,
    },
    Failed {
        path: PathBuf,
        detail: String,
        original_kept: bool,
    },
}

impl EntryAction {
    pub fn should_report(&self) -> bool {
        match self {
            Self::AlreadyCurrent { .. } => false,
            Self::Skipped { exists, .. } => *exists,
            Self::Linked { .. }
            | Self::Copied { .. }
            | Self::LeftNewer { .. }
            | Self::LeftUnread { .. }
            | Self::Failed { .. } => true,
        }
    }

    pub fn summary(&self) -> String {
        match self {
            Self::Linked {
                path,
                target,
                backup,
            } => match backup {
                Some(backup) => format!(
                    "{}: moved the previous file to {} and linked it to {}",
                    path.display(),
                    backup.display(),
                    target.display()
                ),
                None => format!("{}: linked it to {}", path.display(), target.display()),
            },
            Self::Copied {
                path,
                target,
                backup,
            } => match backup {
                Some(backup) => format!(
                    "{}: moved the previous file to {} and copied {}",
                    path.display(),
                    backup.display(),
                    target.display()
                ),
                None => format!("{}: copied {}", path.display(), target.display()),
            },
            Self::AlreadyCurrent { path } => format!("{}: already current", path.display()),
            Self::LeftNewer { path, version } => format!(
                "{path}: {version} is newer than the app bundle and was left in place",
                path = path.display()
            ),
            Self::LeftUnread { path, detail } => format!(
                "{path}: version unreadable ({detail}); left in place",
                path = path.display()
            ),
            Self::Skipped { path, detail, .. } => {
                format!("{}: left in place ({detail})", path.display())
            }
            Self::Failed {
                path,
                detail,
                original_kept,
            } => {
                let kept = if *original_kept {
                    "the previous file is still in place"
                } else {
                    "the previous file could not be restored"
                };
                format!("{}: not updated ({detail}); {kept}", path.display())
            }
        }
    }
}

pub fn render_actions(actions: &[EntryAction]) -> Option<String> {
    let lines: Vec<String> = actions
        .iter()
        .filter(|action| action.should_report())
        .map(EntryAction::summary)
        .collect();
    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

/// Editors keep a running `wenlan-mcp` until they start it again, and a
/// service that was launched from `~/.wenlan/bin` keeps the old daemon inode.
pub fn relaunch_hint(actions: &[EntryAction]) -> Option<&'static str> {
    let changed = actions.iter().any(|action| {
        matches!(
            action,
            EntryAction::Linked { .. } | EntryAction::Copied { .. }
        )
    });
    changed.then_some(
        "Restart editors so they relaunch wenlan-mcp. If the background service was started from \
         ~/.wenlan/bin, run `wenlan restart` so the daemon picks up the new binary.",
    )
}

pub struct SyncRequest<'a> {
    pub home: &'a Path,
    pub bundle_dir: &'a Path,
    pub bundle_version: &'a Version,
    pub host: DesktopHost,
    pub mode: LinkMode,
    pub stamp: &'a str,
}

pub fn sync_home_bin(
    request: &SyncRequest<'_>,
    version_of: &mut dyn FnMut(&Path) -> Result<Version, String>,
) -> Vec<EntryAction> {
    // Keep this file in place: unlinking a lock lets another process lock a
    // different inode while the current owner is still replacing binaries.
    let bin = home_bin_dir(request.home);
    let lock_path = bin.join(".refresh.lock");
    let lock = (|| -> io::Result<fs::File> {
        fs::create_dir_all(&bin)?;
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)?;
        fs2::FileExt::try_lock_exclusive(&file)?;
        Ok(file)
    })();
    let _lock = match lock {
        Ok(file) => file,
        Err(error) => {
            return vec![EntryAction::Failed {
                path: lock_path,
                detail: if error.raw_os_error() == fs2::lock_contended_error().raw_os_error() {
                    "another binary refresh is in progress; retry after it finishes".to_string()
                } else {
                    format!("could not lock the binary refresh: {error}")
                },
                original_kept: true,
            }];
        }
    };
    // The lock covers classification, backup-name selection, replacement and
    // rollback for all three entries, including separate app/CLI processes.
    RUNTIME_STEMS
        .iter()
        .map(|stem| sync_one(request, stem, version_of))
        .collect()
}

fn sync_one(
    request: &SyncRequest<'_>,
    stem: &str,
    version_of: &mut dyn FnMut(&Path) -> Result<Version, String>,
) -> EntryAction {
    let path = home_bin_dir(request.home).join(executable_name(stem, request.host));
    let bundle_file = request.bundle_dir.join(executable_name(stem, request.host));
    if !is_runnable_path(&bundle_file) {
        let exists = fs::symlink_metadata(&path).is_ok();
        return EntryAction::Skipped {
            path,
            detail: format!("the app bundle has no runnable {}", bundle_file.display()),
            exists,
        };
    }
    let state = classify(&path, Some(&bundle_file), version_of);
    match state {
        State::Absent => create_missing(request.mode, &path, &bundle_file),
        State::PointsAtBundle => EntryAction::AlreadyCurrent { path },
        State::DanglingLink => replace(request, &path, &bundle_file, "link"),
        State::Versioned {
            version: Ok(version),
            ..
        } => {
            if version < *request.bundle_version {
                replace(request, &path, &bundle_file, &version.to_string())
            } else if version > *request.bundle_version {
                EntryAction::LeftNewer { path, version }
            } else {
                EntryAction::AlreadyCurrent { path }
            }
        }
        State::Versioned {
            version: Err(detail),
            ..
        } => EntryAction::LeftUnread { path, detail },
        State::NotAProgram { detail } => EntryAction::Skipped {
            path,
            detail,
            exists: true,
        },
    }
}

fn create_missing(mode: LinkMode, path: &Path, bundle_file: &Path) -> EntryAction {
    if mode == LinkMode::Copy {
        return EntryAction::Skipped {
            path: path.to_path_buf(),
            detail: "a copy is not written unless a file is already there".to_string(),
            exists: false,
        };
    }
    if let Some(parent) = path.parent() {
        if let Err(error) = fs::create_dir_all(parent) {
            return EntryAction::Failed {
                path: path.to_path_buf(),
                detail: error.to_string(),
                original_kept: true,
            };
        }
    }
    match install_at(mode, bundle_file, path, true) {
        Ok(installed) => action_from_install(installed, path, bundle_file, None),
        Err(error) if mode == LinkMode::SymlinkOrCopy => EntryAction::Skipped {
            path: path.to_path_buf(),
            detail: format!("could not create a symlink ({error})"),
            exists: false,
        },
        Err(error) => EntryAction::Failed {
            path: path.to_path_buf(),
            detail: error.to_string(),
            original_kept: true,
        },
    }
}

fn replace(
    request: &SyncRequest<'_>,
    path: &Path,
    bundle_file: &Path,
    version_tag: &str,
) -> EntryAction {
    let backup = match unique_backup(path, version_tag, request.stamp) {
        Ok(backup) => backup,
        Err(detail) => {
            return EntryAction::Failed {
                path: path.to_path_buf(),
                detail,
                original_kept: true,
            }
        }
    };
    match commit_replacement(path, &backup, |dest| {
        install_at(request.mode, bundle_file, dest, false)
    }) {
        Ok(installed) => {
            let backup = drop_backup_that_is_the_bundle(&backup, bundle_file);
            if request.mode == LinkMode::CopyAll {
                prune_older_backups(path, backup.as_deref());
            }
            action_from_install(installed, path, bundle_file, backup)
        }
        Err((detail, original_kept)) => EntryAction::Failed {
            path: path.to_path_buf(),
            detail,
            original_kept,
        },
    }
}

fn action_from_install(
    installed: InstalledAs,
    path: &Path,
    bundle_file: &Path,
    backup: Option<PathBuf>,
) -> EntryAction {
    let target = fs::canonicalize(bundle_file).unwrap_or_else(|_| bundle_file.to_path_buf());
    match installed {
        InstalledAs::Link => EntryAction::Linked {
            path: path.to_path_buf(),
            target,
            backup,
        },
        InstalledAs::Copy => EntryAction::Copied {
            path: path.to_path_buf(),
            target,
            backup,
        },
    }
}

#[derive(Debug)]
enum InstalledAs {
    Link,
    Copy,
}

fn install_at(
    mode: LinkMode,
    bundle_file: &Path,
    dest: &Path,
    creating: bool,
) -> io::Result<InstalledAs> {
    let target = fs::canonicalize(bundle_file).unwrap_or_else(|_| bundle_file.to_path_buf());
    match mode {
        LinkMode::Symlink => {
            create_symlink(&target, dest)?;
            Ok(InstalledAs::Link)
        }
        LinkMode::Copy | LinkMode::CopyAll => {
            copy_exe(bundle_file, dest)?;
            Ok(InstalledAs::Copy)
        }
        LinkMode::SymlinkOrCopy if creating => {
            create_symlink(&target, dest)?;
            Ok(InstalledAs::Link)
        }
        LinkMode::SymlinkOrCopy => match create_symlink(&target, dest) {
            Ok(()) => Ok(InstalledAs::Link),
            Err(_) => {
                copy_exe(bundle_file, dest)?;
                Ok(InstalledAs::Copy)
            }
        },
    }
}

fn commit_replacement(
    entry: &Path,
    backup: &Path,
    write: impl FnOnce(&Path) -> io::Result<InstalledAs>,
) -> Result<InstalledAs, (String, bool)> {
    if let Err(error) = fs::rename(entry, backup) {
        return Err((
            format!("could not move the existing file aside: {error}"),
            true,
        ));
    }
    match write(entry) {
        Ok(kind) => Ok(kind),
        Err(error) => {
            let restored = fs::rename(backup, entry).is_ok();
            Err((error.to_string(), restored))
        }
    }
}

/// An AppImage copies all three binaries on every update, so each update
/// would otherwise leave three more full-size backups behind for good. Keep
/// only the one this refresh just made (one step of rollback) and remove this
/// entry's older `<name>.bak.*` files. Only regular files with that exact
/// prefix go; anything else in the directory is left alone.
fn prune_older_backups(entry: &Path, keep: Option<&Path>) {
    let (Some(dir), Some(name)) = (entry.parent(), entry.file_name()) else {
        return;
    };
    let prefix = format!("{}.bak.", name.to_string_lossy());
    let Ok(read) = fs::read_dir(dir) else {
        return;
    };
    for item in read.flatten() {
        let candidate = item.path();
        if keep == Some(candidate.as_path())
            || !item.file_name().to_string_lossy().starts_with(&prefix)
            || !item.file_type().is_ok_and(|kind| kind.is_file())
        {
            continue;
        }
        let _ = fs::remove_file(&candidate);
    }
}

fn drop_backup_that_is_the_bundle(backup: &Path, bundle_file: &Path) -> Option<PathBuf> {
    if same_inode(backup, bundle_file) {
        let _ = fs::remove_file(backup);
        None
    } else {
        Some(backup.to_path_buf())
    }
}

fn unique_backup(entry: &Path, version_tag: &str, stamp: &str) -> Result<PathBuf, String> {
    let name = entry
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "wenlan".to_string());
    let tag = sanitize_tag(version_tag);
    let dir = entry.parent().unwrap_or_else(|| Path::new("."));
    let mut candidate = dir.join(format!("{name}.bak.{tag}.{stamp}"));
    for n in 2..100 {
        if fs::symlink_metadata(&candidate).is_err() {
            return Ok(candidate);
        }
        candidate = dir.join(format!("{name}.bak.{tag}.{stamp}-{n}"));
    }
    Err(format!(
        "too many backups already exist for {}",
        entry.display()
    ))
}

fn sanitize_tag(tag: &str) -> String {
    let cleaned: String = tag
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "unknown".to_string()
    } else {
        cleaned
    }
}

fn copy_exe(from: &Path, to: &Path) -> io::Result<()> {
    let file_name = to
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "wenlan".to_string());
    let tmp = to.with_file_name(format!(".{file_name}.partial"));
    let _ = fs::remove_file(&tmp);
    fs::copy(from, &tmp)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755))?;
    }
    if let Err(error) = fs::rename(&tmp, to) {
        let _ = fs::remove_file(&tmp);
        return Err(error);
    }
    Ok(())
}

fn create_symlink(target: &Path, link: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(target, link)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (target, link);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "this platform has no file symlink",
        ))
    }
}

fn points_at(link: &Path, bundle_file: &Path) -> bool {
    let Ok(raw) = fs::read_link(link) else {
        return false;
    };
    let absolute = if raw.is_absolute() {
        raw
    } else {
        match link.parent() {
            Some(parent) => parent.join(raw),
            None => raw,
        }
    };
    match (fs::canonicalize(&absolute), fs::canonicalize(bundle_file)) {
        (Ok(left), Ok(right)) => left == right,
        _ => absolute == bundle_file,
    }
}

fn same_dir(left: &Path, right: &Path) -> bool {
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

fn same_inode(left: &Path, right: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let Ok(left) = fs::metadata(left) else {
            return false;
        };
        let Ok(right) = fs::metadata(right) else {
            return false;
        };
        left.ino() == right.ino() && left.dev() == right.dev()
    }
    #[cfg(not(unix))]
    {
        let _ = (left, right);
        false
    }
}

fn is_runnable_path(path: &Path) -> bool {
    fs::metadata(path)
        .map(|metadata| metadata.is_file() && is_runnable_meta(&metadata))
        .unwrap_or(false)
}

fn is_runnable_meta(metadata: &fs::Metadata) -> bool {
    if !metadata.is_file() || metadata.len() == 0 {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Run `path --version` and parse it. The child is killed if it does not
/// answer within `timeout`, so a binary that ignores the flag cannot stall
/// `wenlan doctor`.
pub fn read_binary_version_with_timeout(path: &Path, timeout: Duration) -> Result<Version, String> {
    let mut child = Command::new(path)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| error.to_string())?;
    let started = std::time::Instant::now();
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "version probe had no stdout".to_string())?;
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let result = stdout.take(4096).read_to_end(&mut buf);
        let _ = sender.send(result.map(|_| String::from_utf8_lossy(&buf).into_owned()));
    });
    let timed_out = || format!("timed out after {}ms", timeout.as_millis());
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return Err(format!("version probe exited with {status}"));
                }
                // EOF may lag process exit if a descendant inherited stdout.
                // Reading output and waiting for exit share one deadline.
                let remaining = timeout.saturating_sub(started.elapsed());
                let text = receiver
                    .recv_timeout(remaining)
                    .map_err(|_| timed_out())?
                    .map_err(|error| error.to_string())?;
                return parse_version_output(&text);
            }
            Ok(None) if started.elapsed() < timeout => {
                std::thread::sleep(
                    Duration::from_millis(10).min(timeout.saturating_sub(started.elapsed())),
                );
            }
            result => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(match result {
                    Err(error) => error.to_string(),
                    _ => timed_out(),
                });
            }
        }
    }
}

pub fn read_binary_version(path: &Path) -> Result<Version, String> {
    read_binary_version_with_timeout(path, Duration::from_secs(3))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(text: &str) -> Version {
        Version::parse(text).expect(text)
    }

    fn write_runnable(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    fn text_version(path: &Path) -> Result<Version, String> {
        let text = fs::read_to_string(path).map_err(|error| error.to_string())?;
        Version::parse(text.trim())
    }

    fn bundle(dir: &Path, host: DesktopHost, body: &str) {
        for stem in RUNTIME_STEMS {
            write_runnable(&dir.join(executable_name(stem, host)), body);
        }
        fs::write(dir.join(app_marker_name(host)), b"app").unwrap();
    }

    #[test]
    fn versions_order_releases_prereleases_and_build_metadata() {
        assert!(version("0.18.10") < version("0.20.7"));
        assert!(version("0.20.7-alpha.1") < version("0.20.7"));
        assert!(version("0.20.7-alpha") < version("0.20.7-alpha.1"));
        assert!(version("0.20.7-alpha.1") < version("0.20.7-alpha.2"));
        assert!(version("0.20.7-alpha.1") < version("0.20.7-beta"));
        assert_eq!(version("0.20.7+gabcdef"), version("0.20.7"));
        assert_eq!(version("v0.20.7").to_string(), "0.20.7");
        assert!(Version::parse("0.20").is_err());
        assert_eq!(
            parse_version_output("wenlan 0.18.10\n")
                .unwrap()
                .to_string(),
            "0.18.10"
        );
        assert!(parse_version_output("listening on 127.0.0.1:7878").is_err());
    }

    #[test]
    fn a_debug_or_isolated_run_does_not_rewrite_home_bin() {
        assert!(!may_rewrite_user_home_bin(true, false));
        assert!(!may_rewrite_user_home_bin(false, true));
        assert!(may_rewrite_user_home_bin(false, false));
    }

    #[test]
    fn discovery_skips_the_home_bin_and_a_dir_without_the_app() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let host = DesktopHost::Linux;
        let home_bin = home_bin_dir(&home);
        bundle(&home_bin, host, "0.18.10\n");
        let loose = tmp.path().join("target").join("debug");
        for stem in RUNTIME_STEMS {
            write_runnable(&loose.join(stem), "0.20.7\n");
        }
        let app = tmp.path().join("Wenlan.app").join("Contents").join("MacOS");
        fs::create_dir_all(&app).unwrap();
        bundle(&app, host, "0.20.7\n");
        fs::create_dir_all(home.join(".wenlan/bin")).unwrap();
        fs::write(
            home.join(RECORDED_MCP_PATH_REL),
            format!("{}\n", app.join("wenlan-mcp").display()),
        )
        .unwrap();

        let candidates = vec![loose.clone(), PathBuf::from("/no/such/app")];
        let found = discover_bundle_dir(&DiscoverRequest {
            home: &home,
            host,
            current_exe: Some(&home_bin.join("wenlan")),
            override_dir: None,
            candidates: &candidates,
        });
        assert_eq!(found, Some(app));
    }

    #[test]
    fn discovery_prefers_an_explicit_bundle_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let host = DesktopHost::MacOs;
        let override_dir = tmp.path().join("override");
        let other = tmp.path().join("other");
        fs::create_dir_all(&override_dir).unwrap();
        fs::create_dir_all(&other).unwrap();
        bundle(&override_dir, host, "0.20.7\n");
        bundle(&other, host, "0.20.7\n");
        let candidates = vec![other];
        let found = discover_bundle_dir(&DiscoverRequest {
            home: &home,
            host,
            current_exe: None,
            override_dir: Some(&override_dir),
            candidates: &candidates,
        });
        assert_eq!(found, Some(override_dir));
    }

    #[test]
    fn mac_and_windows_candidates_use_their_install_dirs() {
        let home = Path::new("/Users/ada");
        let mac = default_bundle_candidates(home, DesktopHost::MacOs, None, None);
        assert_eq!(
            mac[0],
            PathBuf::from("/Applications/Wenlan.app/Contents/MacOS")
        );
        assert_eq!(
            mac[1],
            PathBuf::from("/Users/ada/Applications/Wenlan.app/Contents/MacOS")
        );
        let local = Path::new("/Users/ada/AppData/Local");
        let programs = Path::new("/Program Files");
        let windows =
            default_bundle_candidates(home, DesktopHost::Windows, Some(local), Some(programs));
        assert_eq!(
            windows,
            vec![
                local.join("Programs/Wenlan"),
                local.join("Wenlan"),
                programs.join("Wenlan"),
            ]
        );
        assert_eq!(
            executable_name("wenlan", DesktopHost::Windows),
            "wenlan.exe"
        );
    }

    fn sync_fixture(
        home: &Path,
        bundle_dir: &Path,
        mode: LinkMode,
        host: DesktopHost,
    ) -> Vec<EntryAction> {
        let bundle_version = version("0.20.7");
        sync_home_bin(
            &SyncRequest {
                home,
                bundle_dir,
                bundle_version: &bundle_version,
                host,
                mode,
                stamp: "t",
            },
            &mut text_version,
        )
    }

    #[cfg(unix)]
    #[test]
    fn an_older_regular_file_is_kept_and_replaced_with_a_link() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let bundle_dir = tmp.path().join("MacOS");
        fs::create_dir_all(&bundle_dir).unwrap();
        let host = DesktopHost::MacOs;
        bundle(&bundle_dir, host, "0.20.7\n");
        let bin = home_bin_dir(&home);
        for stem in RUNTIME_STEMS {
            write_runnable(&bin.join(stem), "0.18.10\n");
        }
        fs::write(bin.join("wenlan.bak.manual"), b"keep me").unwrap();
        fs::write(bin.join("notes.txt"), b"notes").unwrap();
        fs::write(
            bin.join("wenlan-mcp.path"),
            b"/Applications/Wenlan.app/Contents/MacOS/wenlan-mcp\n",
        )
        .unwrap();

        let actions = sync_fixture(&home, &bundle_dir, LinkMode::Symlink, host);
        assert!(actions.iter().all(|action| matches!(
            action,
            EntryAction::Linked {
                backup: Some(_),
                ..
            }
        )));
        for stem in RUNTIME_STEMS {
            let entry = bin.join(stem);
            assert!(fs::symlink_metadata(&entry)
                .unwrap()
                .file_type()
                .is_symlink());
            assert_eq!(
                fs::canonicalize(&entry).unwrap(),
                fs::canonicalize(bundle_dir.join(stem)).unwrap()
            );
            let backup = bin.join(format!("{stem}.bak.0.18.10.t"));
            assert_eq!(fs::read_to_string(&backup).unwrap(), "0.18.10\n");
        }
        assert_eq!(fs::read(bin.join("wenlan.bak.manual")).unwrap(), b"keep me");
        assert_eq!(fs::read(bin.join("notes.txt")).unwrap(), b"notes");

        let again = sync_fixture(&home, &bundle_dir, LinkMode::Symlink, host);
        assert!(again
            .iter()
            .all(|action| matches!(action, EntryAction::AlreadyCurrent { .. })));
        assert!(bin.join("wenlan.bak.0.18.10.t").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn a_second_backup_does_not_overwrite_the_first() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let bundle_dir = tmp.path().join("MacOS");
        fs::create_dir_all(&bundle_dir).unwrap();
        let host = DesktopHost::Linux;
        bundle(&bundle_dir, host, "0.20.7\n");
        let entry = home_bin_dir(&home).join("wenlan");
        write_runnable(&entry, "0.18.10\n");
        let first = sync_fixture(&home, &bundle_dir, LinkMode::Symlink, host);
        fs::remove_file(&entry).unwrap();
        write_runnable(&entry, "0.18.10\n");
        let mut second = sync_fixture(&home, &bundle_dir, LinkMode::Symlink, host);
        // Other tests can spawn a child while this process has the lock open.
        // On Unix, the child may briefly retain that open file description
        // before exec closes it. Retry only an explicit lock-contention result;
        // every other failure still reaches the backup assertions below.
        for _ in 0..100 {
            if !matches!(&second[..], [EntryAction::Failed { detail, .. }]
                if detail.contains("another binary refresh"))
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
            second = sync_fixture(&home, &bundle_dir, LinkMode::Symlink, host);
        }
        let bin = home_bin_dir(&home);
        let backups: Vec<_> = fs::read_dir(&bin)
            .unwrap()
            .map(|item| item.unwrap().file_name())
            .collect();
        assert_eq!(
            fs::read_to_string(bin.join("wenlan.bak.0.18.10.t")).unwrap(),
            "0.18.10\n"
        );
        let backup2 = fs::read_to_string(bin.join("wenlan.bak.0.18.10.t-2"));
        assert!(
            matches!(backup2.as_deref(), Ok("0.18.10\n")),
            "backup2={backup2:?}; first={first:?}; second={second:?}; entries={backups:?}"
        );
    }

    #[test]
    fn a_newer_regular_file_and_an_unreadable_one_stay() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let bundle_dir = tmp.path().join("bundle");
        fs::create_dir_all(&bundle_dir).unwrap();
        let host = DesktopHost::Linux;
        bundle(&bundle_dir, host, "0.20.7\n");
        let bin = home_bin_dir(&home);
        write_runnable(&bin.join("wenlan"), "0.21.0\n");
        write_runnable(&bin.join("wenlan-server"), "not-a-version\n");
        write_runnable(&bin.join("wenlan-mcp"), "0.20.7\n");
        let actions = sync_fixture(&home, &bundle_dir, LinkMode::Symlink, host);
        assert!(matches!(actions[0], EntryAction::LeftNewer { .. }));
        assert!(matches!(actions[1], EntryAction::LeftUnread { .. }));
        assert!(matches!(actions[2], EntryAction::AlreadyCurrent { .. }));
        assert_eq!(fs::read_to_string(bin.join("wenlan")).unwrap(), "0.21.0\n");
        assert_eq!(
            fs::read_to_string(bin.join("wenlan-server")).unwrap(),
            "not-a-version\n"
        );
        assert!(bin.join("wenlan").is_file());
        assert!(!fs::symlink_metadata(bin.join("wenlan"))
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[cfg(unix)]
    #[test]
    fn a_dangling_link_is_replaced_and_a_directory_is_not() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let bundle_dir = tmp.path().join("bundle");
        fs::create_dir_all(&bundle_dir).unwrap();
        let host = DesktopHost::Linux;
        bundle(&bundle_dir, host, "0.20.7\n");
        let bin = home_bin_dir(&home);
        fs::create_dir_all(&bin).unwrap();
        std::os::unix::fs::symlink(bin.join("missing"), bin.join("wenlan")).unwrap();
        fs::create_dir(bin.join("wenlan-server")).unwrap();
        write_runnable(&bin.join("wenlan-mcp"), "0.20.7\n");
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(bin.join("wenlan-mcp"), fs::Permissions::from_mode(0o644)).unwrap();
        }
        let actions = sync_fixture(&home, &bundle_dir, LinkMode::Symlink, host);
        assert!(matches!(actions[0], EntryAction::Linked { .. }));
        assert!(fs::symlink_metadata(bin.join("wenlan"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(fs::symlink_metadata(bin.join("wenlan.bak.link.t"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(bin.join("wenlan-server").is_dir());
        assert!(matches!(
            actions[1],
            EntryAction::Skipped { exists: true, .. }
        ));
        assert!(matches!(
            actions[2],
            EntryAction::Skipped { exists: true, .. }
        ));
        assert_eq!(
            fs::read_to_string(bin.join("wenlan-mcp")).unwrap(),
            "0.20.7\n"
        );
    }

    #[test]
    fn copy_all_mode_seeds_missing_entries_as_regular_files() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let bundle_dir = tmp.path().join("bundle");
        fs::create_dir_all(&bundle_dir).unwrap();
        let host = DesktopHost::Linux;
        bundle(&bundle_dir, host, "0.20.7\n");
        let bin = home_bin_dir(&home);
        write_runnable(&bin.join("wenlan"), "0.18.10\n");
        let actions = sync_fixture(&home, &bundle_dir, LinkMode::CopyAll, host);
        assert!(matches!(
            actions[0],
            EntryAction::Copied {
                backup: Some(_),
                ..
            }
        ));
        for name in ["wenlan", "wenlan-server", "wenlan-mcp"] {
            let meta = fs::symlink_metadata(bin.join(name)).unwrap();
            assert!(meta.file_type().is_file(), "{name} must be a regular file");
            assert_eq!(fs::read_to_string(bin.join(name)).unwrap(), "0.20.7\n");
        }
        // The bundle going away (the AppImage quit) leaves the copies working.
        fs::remove_dir_all(&bundle_dir).unwrap();
        assert_eq!(
            fs::read_to_string(bin.join("wenlan-mcp")).unwrap(),
            "0.20.7\n"
        );
    }

    #[test]
    fn copy_all_mode_keeps_only_the_newest_backup_across_updates() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let host = DesktopHost::Linux;
        let bin = home_bin_dir(&home);
        let unrelated = bin.join("notes.txt");
        let first = tmp.path().join("bundle-1");
        fs::create_dir_all(&first).unwrap();
        bundle(&first, host, "0.21.0\n");
        sync_fixture(&home, &first, LinkMode::CopyAll, host);
        fs::write(&unrelated, "mine").unwrap();
        for (n, release) in ["0.22.0\n", "0.23.0\n", "0.24.0\n"].iter().enumerate() {
            let next = tmp.path().join(format!("bundle-{}", n + 2));
            fs::create_dir_all(&next).unwrap();
            bundle(&next, host, release);
            let stamp = format!("t{n}");
            sync_home_bin(
                &SyncRequest {
                    home: &home,
                    bundle_dir: &next,
                    bundle_version: &version(release.trim()),
                    host,
                    mode: LinkMode::CopyAll,
                    stamp: &stamp,
                },
                &mut text_version,
            );
        }
        for name in ["wenlan", "wenlan-server", "wenlan-mcp"] {
            let backups: Vec<String> = fs::read_dir(&bin)
                .unwrap()
                .flatten()
                .map(|item| item.file_name().to_string_lossy().into_owned())
                .filter(|file| file.starts_with(&format!("{name}.bak.")))
                .collect();
            assert_eq!(backups, vec![format!("{name}.bak.0.23.0.t2")], "{name}");
            assert_eq!(fs::read_to_string(bin.join(name)).unwrap(), "0.24.0\n");
        }
        assert_eq!(fs::read_to_string(&unrelated).unwrap(), "mine");
    }

    #[test]
    fn copy_mode_refreshes_an_older_file_and_does_not_seed_a_missing_one() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let bundle_dir = tmp.path().join("bundle");
        fs::create_dir_all(&bundle_dir).unwrap();
        let host = DesktopHost::Windows;
        bundle(&bundle_dir, host, "0.20.7\n");
        let bin = home_bin_dir(&home);
        write_runnable(&bin.join("wenlan.exe"), "0.18.10\n");
        let actions = sync_fixture(&home, &bundle_dir, LinkMode::Copy, host);
        assert!(matches!(
            actions[0],
            EntryAction::Copied {
                backup: Some(_),
                ..
            }
        ));
        assert!(matches!(
            actions[1],
            EntryAction::Skipped { exists: false, .. }
        ));
        assert_eq!(
            fs::read_to_string(bin.join("wenlan.exe")).unwrap(),
            "0.20.7\n"
        );
        assert_eq!(
            fs::read_to_string(bin.join("wenlan.exe.bak.0.18.10.t")).unwrap(),
            "0.18.10\n"
        );
        assert!(!bin.join("wenlan-server.exe").exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_missing_name_becomes_a_symlink_and_a_failed_write_restores() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let bundle_dir = tmp.path().join("bundle");
        fs::create_dir_all(&bundle_dir).unwrap();
        let host = DesktopHost::Linux;
        bundle(&bundle_dir, host, "0.20.7\n");
        let actions = sync_fixture(&home, &bundle_dir, LinkMode::Symlink, host);
        assert!(actions
            .iter()
            .all(|action| matches!(action, EntryAction::Linked { backup: None, .. })));
        assert!(fs::symlink_metadata(home_bin_dir(&home).join("wenlan"))
            .unwrap()
            .file_type()
            .is_symlink());

        let entry = tmp.path().join("entry");
        fs::write(&entry, b"old").unwrap();
        let backup = tmp.path().join("entry.bak");
        let error = commit_replacement(&entry, &backup, |_| Err(io::Error::other("disk full")))
            .unwrap_err();
        assert!(error.1, "original restored");
        assert_eq!(fs::read(&entry).unwrap(), b"old");
        assert!(!backup.exists());
    }

    #[test]
    fn the_report_names_a_stale_home_bin_and_stays_quiet_when_versions_match() {
        let cli = version("0.20.7");
        let daemon = version("0.20.7");
        let bundle = version("0.20.7");
        let home = PathBuf::from("/Users/ada");
        let bundle_dir = PathBuf::from("/Applications/Wenlan.app/Contents/MacOS");
        let mut stale = reported(&home, &cli, Some(&daemon), Some(&bundle_dir), Some(&bundle));
        stale.entries[0].kind = ReportedKind::Regular {
            version: version("0.18.10"),
        };
        stale.entries[1].kind = ReportedKind::Regular {
            version: version("0.18.10"),
        };
        stale.entries[2].kind = ReportedKind::Regular {
            version: version("0.18.10"),
        };
        let text = render_binary_report(&stale);
        assert!(text.contains("0.18.10 (regular file)"), "{text}");
        assert!(text.contains("WARNING:"), "{text}");
        let repair_command = bundle_dir
            .join(executable_name("wenlan", DesktopHost::MacOs))
            .display()
            .to_string();
        assert!(
            text.contains(&format!("{repair_command} doctor --repair")),
            "{text}"
        );
        assert!(text.contains(".bak.<version>.<time>"), "{text}");
        assert!(
            text.contains("Nothing already on disk is deleted"),
            "{text}"
        );

        let mut current = reported(&home, &cli, Some(&daemon), Some(&bundle_dir), Some(&bundle));
        for entry in &mut current.entries {
            entry.kind = ReportedKind::FollowsBundle;
        }
        let quiet = render_binary_report(&current);
        assert!(!quiet.contains("WARNING"), "{quiet}");
        assert!(quiet.contains("0.20.7"), "{quiet}");
        assert!(quiet.contains("the daemon"), "{quiet}");
    }

    #[test]
    fn a_newer_home_copy_is_not_offered_as_a_downgrade() {
        let cli = version("0.21.0");
        let bundle = version("0.20.7");
        let home = PathBuf::from("/Users/ada");
        let bundle_dir = PathBuf::from("/Applications/Wenlan.app/Contents/MacOS");
        let mut report = reported(&home, &cli, None, Some(&bundle_dir), Some(&bundle));
        report.entries[0].kind = ReportedKind::Regular {
            version: version("0.21.0"),
        };
        let text = render_binary_report(&report);
        assert!(text.contains("left in place"), "{text}");
        assert!(!text.contains("doctor --repair"), "{text}");
    }

    #[test]
    fn a_daemon_mismatch_without_a_bundle_tells_the_user_to_open_the_app() {
        let cli = version("0.18.10");
        let daemon = version("0.20.7");
        let home = PathBuf::from("/Users/ada");
        let mut report = reported(&home, &cli, Some(&daemon), None, None);
        report.entries[0].kind = ReportedKind::Regular {
            version: version("0.18.10"),
        };
        let text = render_binary_report(&report);
        assert!(text.contains("older than the running daemon"), "{text}");
        assert!(text.contains("Open the Wenlan app"), "{text}");
        assert!(!text.contains("doctor --repair"), "{text}");
    }

    fn reported(
        home: &Path,
        cli: &Version,
        daemon: Option<&Version>,
        bundle_dir: Option<&Path>,
        bundle_version: Option<&Version>,
    ) -> BinaryReport {
        inspect_home_bin(
            &InspectRequest {
                home,
                host: DesktopHost::MacOs,
                bundle_dir,
                bundle_version,
                bundle_version_error: None,
                cli_version: cli,
                daemon_version: daemon,
            },
            &mut |_| Err("not on disk".to_string()),
        )
    }

    #[test]
    fn a_concurrent_refresh_cannot_replace_another_refreshs_backup() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let bundle_dir = tmp.path().join("bundle");
        fs::create_dir_all(&bundle_dir).unwrap();
        let host = DesktopHost::current();
        bundle(&bundle_dir, host, "0.20.7\n");
        let bin = home_bin_dir(&home);
        for stem in RUNTIME_STEMS {
            write_runnable(&bin.join(executable_name(stem, host)), "0.18.10\n");
        }
        let bundle_version = version("0.20.7");
        let request = SyncRequest {
            home: &home,
            bundle_dir: &bundle_dir,
            bundle_version: &bundle_version,
            host,
            mode: LinkMode::Copy,
            stamp: "same-second",
        };
        let mut contended = false;
        let actions = sync_home_bin(&request, &mut |path| {
            if !contended {
                contended = true;
                let second = sync_home_bin(&request, &mut text_version);
                assert!(
                    second.iter().all(|action| matches!(
                        action,
                        EntryAction::Failed {
                            original_kept: true,
                            ..
                        }
                    )),
                    "concurrent repair must refuse before touching files: {second:?}"
                );
            }
            text_version(path)
        });
        assert!(contended);
        assert!(actions.iter().all(|action| matches!(
            action,
            EntryAction::Copied {
                backup: Some(_),
                ..
            }
        )));
        for stem in RUNTIME_STEMS {
            let name = executable_name(stem, host);
            assert_eq!(
                fs::read(bin.join(format!("{name}.bak.0.18.10.same-second"))).unwrap(),
                b"0.18.10\n"
            );
        }
        let again = sync_home_bin(&request, &mut text_version);
        assert!(again
            .iter()
            .all(|action| matches!(action, EntryAction::AlreadyCurrent { .. })));
    }

    #[test]
    fn a_refresh_lock_is_shared_across_processes() {
        const TEST_ROOT: &str = "WENLAN_HOME_BIN_LOCK_TEST_ROOT";
        if let Some(root) = std::env::var_os(TEST_ROOT) {
            let root = PathBuf::from(root);
            let home = root.join("home");
            let bundle_dir = root.join("bundle");
            let actions = sync_fixture(&home, &bundle_dir, LinkMode::Copy, DesktopHost::current());
            assert!(
                matches!(&actions[..], [EntryAction::Failed { original_kept: true, detail, .. }]
                if detail.contains("another binary refresh")),
                "{actions:?}"
            );
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let bundle_dir = tmp.path().join("bundle");
        let host = DesktopHost::current();
        fs::create_dir_all(&bundle_dir).unwrap();
        bundle(&bundle_dir, host, "0.20.7\n");
        let entry = home_bin_dir(&home).join(executable_name("wenlan", host));
        write_runnable(&entry, "0.18.10\n");
        let bundle_version = version("0.20.7");
        let actions = sync_home_bin(
            &SyncRequest {
                home: &home,
                bundle_dir: &bundle_dir,
                bundle_version: &bundle_version,
                host,
                mode: LinkMode::Copy,
                stamp: "t",
            },
            &mut |path| {
                let result = Command::new(std::env::current_exe().unwrap())
                    .arg("a_refresh_lock_is_shared_across_processes")
                    .arg("--nocapture")
                    .env(TEST_ROOT, tmp.path())
                    .env("WENLAN_NO_AUTOSTART", "1")
                    .output()
                    .unwrap();
                assert!(
                    result.status.success(),
                    "{}{}",
                    String::from_utf8_lossy(&result.stdout),
                    String::from_utf8_lossy(&result.stderr)
                );
                assert_eq!(fs::read(&entry).unwrap(), b"0.18.10\n");
                text_version(path)
            },
        );
        assert!(matches!(
            actions[0],
            EntryAction::Copied {
                backup: Some(_),
                ..
            }
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_version_printed_before_hanging_does_not_disable_the_timeout() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("hang-after-output");
        write_runnable(
            &path,
            "#!/bin/sh\nprintf 'wenlan 0.18.10\\n'\nexec sleep 2\n",
        );
        let start = std::time::Instant::now();
        let result = read_binary_version_with_timeout(&path, Duration::from_millis(100));
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "probe exceeded its deadline"
        );
        assert!(result.unwrap_err().contains("timed out"));
    }

    #[cfg(unix)]
    #[test]
    fn read_binary_version_parses_a_program_and_kills_one_that_hangs() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("wenlan");
        write_runnable(&bin, "#!/bin/sh\nprintf '%s\\n' 'wenlan 0.18.10'\n");
        assert_eq!(read_binary_version(&bin).unwrap().to_string(), "0.18.10");
        let hang = tmp.path().join("hang");
        write_runnable(&hang, "#!/bin/sh\nexec sleep 30\n");
        let error =
            read_binary_version_with_timeout(&hang, Duration::from_millis(200)).unwrap_err();
        assert!(error.contains("timed out"), "{error}");
    }
}
