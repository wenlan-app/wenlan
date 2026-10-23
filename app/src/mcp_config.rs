// SPDX-License-Identifier: AGPL-3.0-only
use crate::error::AppError;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A yes/no fact about an MCP client's install, in three values.
///
/// Same three values and the same rule as [`CandidateProbe`]: only a measured
/// absence is an absence. A read the OS refused answers `Unreadable`, never
/// `No` — the wizard's "not configured" row invites a SECOND registration over
/// a working one, so the two must not be the same value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Reading {
    /// Measured: yes.
    Yes,
    /// Measured: no.
    No,
    /// Could not be measured. NOT a `No`.
    Unreadable { error: String },
}

impl Reading {
    pub(crate) fn of(value: bool) -> Self {
        if value {
            Reading::Yes
        } else {
            Reading::No
        }
    }

    /// OR over two readings of the same question, ranked so a failed read can
    /// never turn a measured yes into a no: `Yes` wins, then `Unreadable`, then
    /// `No`. A client counts as configured through EITHER a plugin or a raw
    /// entry, so if either half is a measured yes the answer is yes whatever
    /// happened to the other — and if neither is, an unread half means the
    /// answer is unknown, not "no".
    pub(crate) fn or(self, other: Reading) -> Reading {
        match (self, other) {
            (Reading::Yes, _) | (_, Reading::Yes) => Reading::Yes,
            (Reading::Unreadable { error }, _) | (_, Reading::Unreadable { error }) => {
                Reading::Unreadable { error }
            }
            (Reading::No, Reading::No) => Reading::No,
        }
    }

    /// AND over two readings, the mirror of [`Reading::or`]: a measured `No`
    /// wins (one half is certainly false, so the whole is), then
    /// `Unreadable`, then `Yes`. "Every skill is installed" is an AND, and a
    /// skill that could not be looked at must not let it read as yes.
    pub(crate) fn and(self, other: Reading) -> Reading {
        match (self, other) {
            (Reading::No, _) | (_, Reading::No) => Reading::No,
            (Reading::Unreadable { error }, _) | (_, Reading::Unreadable { error }) => {
                Reading::Unreadable { error }
            }
            (Reading::Yes, Reading::Yes) => Reading::Yes,
        }
    }
}

/// What reading one client config file actually answered. One read, three
/// answers — replacing `Path::exists()` plus a separate `read_to_string`, which
/// were two instants answering one question and collapsed every read error into
/// `false`.
enum ConfigRead {
    Contents(String),
    /// Measured absent.
    Absent,
    /// Could not look. NOT an absence.
    Unreadable(String),
}

fn read_config(path: &Path) -> ConfigRead {
    match std::fs::read_to_string(path) {
        Ok(contents) => ConfigRead::Contents(contents),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => ConfigRead::Absent,
        Err(e) => ConfigRead::Unreadable(e.to_string()),
    }
}

/// [`read_config`] for a client whose config is in `dialect`, adding the one
/// rule about a config that lives in a DIFFERENT file: OpenCode reads
/// `opencode.jsonc` as well as `opencode.json`, and JSONC (comments, trailing
/// commas) does not survive a JSON round trip. So when `opencode.json` is
/// absent and `opencode.jsonc` is there, the config is reported UNREADABLE —
/// never absent, which would let a write create a second settings file beside
/// the user's, and never parsed and rewritten, which would strip their
/// comments. A `.jsonc` that could not be looked for is unreadable too.
fn read_config_for(dialect: JsonDialect, path: &Path) -> ConfigRead {
    let read = read_config(path);
    if dialect != JsonDialect::OpenCode || !matches!(read, ConfigRead::Absent) {
        return read;
    }
    let jsonc = path.with_extension("jsonc");
    match path_exists_reading(&jsonc) {
        Reading::No => ConfigRead::Absent,
        Reading::Yes => ConfigRead::Unreadable(format!(
            "OpenCode's settings are in {}, a JSON-with-comments file Wenlan does not rewrite; \
             add the Wenlan server to it by hand",
            jsonc.display()
        )),
        Reading::Unreadable { error } => ConfigRead::Unreadable(format!(
            "could not check whether OpenCode keeps its settings in {} ({error})",
            jsonc.display()
        )),
    }
}

impl ConfigRead {
    /// Whether the file is there at all — the honest replacement for
    /// `Path::exists()`, taken from the same read that answers the content
    /// questions so the two cannot come from different instants.
    fn present(&self) -> Reading {
        match self {
            ConfigRead::Contents(_) => Reading::Yes,
            ConfigRead::Absent => Reading::No,
            ConfigRead::Unreadable(error) => Reading::Unreadable {
                error: error.clone(),
            },
        }
    }

    /// Ask a yes/no question of the contents. An absent file is a measured
    /// `No` — it genuinely holds no entry. A file that could not be read is
    /// neither, and NEITHER IS ONE THAT COULD NOT BE PARSED: the question is
    /// fallible, and a body that would not parse comes back as `Unreadable`.
    fn asks(&self, question: impl FnOnce(&str) -> Result<bool, String>) -> Reading {
        match self {
            ConfigRead::Contents(contents) => match question(contents) {
                Ok(answer) => Reading::of(answer),
                Err(error) => Reading::Unreadable { error },
            },
            ConfigRead::Absent => Reading::No,
            ConfigRead::Unreadable(error) => Reading::Unreadable {
                error: error.clone(),
            },
        }
    }
}

/// Parse a config body as JSON, or say why it could not be parsed.
///
/// The `Err` string is user-facing: it lands in `Reading::Unreadable { error }`
/// and from there in the "Setup state unknown" chip's detail and in the pasted
/// diagnostics report, so it has to name the FILE's problem rather than a
/// serde type name.
fn parse_json(body: &str) -> Result<serde_json::Value, String> {
    serde_json::from_str::<serde_json::Value>(body)
        .map_err(|e| format!("the file is not valid JSON ({e})"))
}

/// TOML counterpart of [`parse_json`]. Same contract, same reason.
fn parse_toml(body: &str) -> Result<toml_edit::DocumentMut, String> {
    body.parse::<toml_edit::DocumentMut>()
        .map_err(|e| format!("the file is not valid TOML ({e})"))
}

/// Is anything at `path`? Three answers from ONE `metadata` call.
/// `Path::exists()` is the two-answer version and gets the failure case wrong —
/// see [`CandidateProbe`], which exists for exactly this reason on the binary
/// side.
fn path_exists_reading(path: &Path) -> Reading {
    match std::fs::metadata(path) {
        Ok(_) => Reading::Yes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Reading::No,
        Err(e) => Reading::Unreadable {
            error: e.to_string(),
        },
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpClient {
    pub name: String,
    pub client_type: String,
    /// `None` when the directory the path hangs off could not be determined,
    /// so there is no path to show and nothing about this client was measured.
    pub config_path: Option<String>,
    pub detected: Reading,
    /// `has_raw_entry` OR `has_plugin`, by [`Reading::or`] — kept alongside its
    /// two halves rather than replacing them, because they point at different
    /// fixes and the UI needs to tell them apart.
    pub already_configured: Reading,
    /// A raw `wenlan`/legacy `origin` entry in the client's own config file.
    pub has_raw_entry: Reading,
    /// BOTH the `wenlan` and the legacy `origin` raw entry — the raw+raw
    /// duplicate.
    pub has_raw_duplicate: Reading,
    /// The Wenlan plugin, for the three clients that have a plugin surface.
    pub has_plugin: Reading,
    /// Whether the client PROGRAM is here, or only something it left behind.
    /// `detected` is the superset ("any trace of this client") and keeps its
    /// meaning; this says which kind of trace it was.
    pub install_state: InstallState,
    /// Whether the raw entry Wenlan wrote into the client's own config would
    /// actually launch. `already_configured` only says an entry EXISTS.
    pub entry_health: EntryHealth,
    /// Whether Wenlan's skills are installed in this client's own skills
    /// folder: `Yes` when every skill in the bundle carries the
    /// `.wenlan-managed` marker, `No` when at least one is measured absent,
    /// `Unreadable` when a look failed. `None` (JSON `null`) for a client with
    /// no skills folder (Claude Code and Codex get skills through their
    /// plugins; Claude Desktop and Muse Code are MCP only). See
    /// [`crate::skills_install::skills_reading`].
    #[serde(default)]
    pub has_skills: Option<Reading>,
}

/// What kind of trace of a client is on this machine.
///
/// `detected` cannot say: a config file survives an uninstall, so "the config
/// is there" and "the client is there" used to be one answer. They are two
/// here, and `detected` is exactly "installed or config-only" (a measured yes
/// for either, an unreadable when the question could not be answered).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InstallState {
    /// The app or CLI itself was found (an app bundle or `.exe`, an AppImage,
    /// or the CLI on `PATH` / in a usual install folder).
    Installed,
    /// No program was found at any place this app looks, but the client's
    /// config file or home folder is here: a leftover from an uninstall, or a
    /// program installed somewhere unusual. NOT a claim that it is gone.
    ConfigOnly,
    /// Neither the program nor anything it leaves behind. Measured.
    NotFound,
    /// Could not be measured (a path the OS would not let this app look at, or
    /// a folder the platform would not report). NOT `not_found`.
    Unreadable { error: String },
}

/// Does the raw `wenlan` entry (or the legacy `origin` one when that is all
/// there is) in a client's own config file launch a server?
///
/// "Launch" is checked the way a client would start it, without starting it:
/// the command resolves to a runnable file, and the arguments are the shape the
/// command needs. Whether the process then runs correctly is NOT checked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EntryHealth {
    /// The config holds no Wenlan entry (or no config file). Nothing to check;
    /// the plugin, if any, is a different surface.
    NoEntry,
    /// The command resolves to a runnable file and the arguments are sane.
    Healthy,
    /// The entry exists and would not launch. Fixed by the existing
    /// `write_mcp_config` command, which rewrites only Wenlan's own entry.
    NeedsRepair {
        reason: RepairReason,
        /// One plain sentence naming what is wrong, for the UI to show.
        detail: String,
    },
    /// The config could not be read or parsed, or the command's path could not
    /// be looked at. NOT `healthy`, and not a reason to offer a repair.
    Unreadable { error: String },
}

/// Why an entry needs repair. Stable `snake_case` strings on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepairReason {
    /// The entry has no usable `command` (and is not a `url` entry).
    CommandMissing,
    /// The command names a file that is not there: a missing absolute path, a
    /// relative path, or a bare name found nowhere on `PATH` or in the usual
    /// install folders.
    CommandNotFound,
    /// Something is at the command's path and it cannot be run: a folder, an
    /// empty file, or (Unix) a file with no execute bit.
    CommandNotRunnable,
    /// `args` is not a list of strings, or an `npx` entry does not ask for the
    /// Wenlan package.
    ArgsInvalid,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WenlanMcpEntry {
    pub command: String,
    pub args: Vec<String>,
}

/// Where a client's config file is, or why that could not be said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientConfigPath {
    Known(PathBuf),
    /// Not a client type this app knows.
    UnknownClient,
    /// The directory the path hangs off could not be determined, so nothing
    /// about this client can be measured — including whether it is installed.
    Undetermined(String),
}

/// The expected config file path for each MCP client.
pub fn client_config_path(client_type: &str) -> ClientConfigPath {
    let (home, config_dir) = crate::identity_paths::ai_tool_dirs();
    client_config_path_for(client_type, home.as_deref(), config_dir.as_deref())
}

/// Path construction, split from the directory lookups so the "the platform
/// would not report a home directory" branch is reachable from a test. It is
/// not reachable through the environment on Windows — `dirs::home_dir()`
/// resolves the known folder and IGNORES `$HOME` — so without this seam the
/// branch could only be reasoned about, never measured.
pub(crate) fn client_config_path_for(
    client_type: &str,
    home: Option<&Path>,
    config_dir: Option<&Path>,
) -> ClientConfigPath {
    // Each arm asks only for the directory it actually needs — `claude_desktop`
    // does not use `home` at all — and a directory that could not be determined
    // is a VALUE, so the row still exists and says so.
    let under = |dir: Option<&Path>, what: &str, tail: &[&str]| match dir {
        Some(dir) => ClientConfigPath::Known(
            tail.iter()
                .fold(dir.to_path_buf(), |acc, part| acc.join(part)),
        ),
        None => ClientConfigPath::Undetermined(format!("the platform would not report {what}")),
    };
    const HOME: &str = "a home directory";
    match client_type {
        // macOS: ~/Library/Application Support/Claude/claude_desktop_config.json
        "claude_desktop" => under(
            config_dir,
            "an application-configuration directory",
            &["Claude", "claude_desktop_config.json"],
        ),
        "cursor" => under(home, HOME, &[".cursor", "mcp.json"]),
        "claude_code" => under(home, HOME, &[".claude.json"]),
        "gemini_cli" => under(home, HOME, &[".gemini", "settings.json"]),
        "codex_cli" => under(home, HOME, &[".codex", "config.toml"]),
        // Muse Code documents one user settings file, `~/.config/muse/settings.json`,
        // on every platform. It is NOT resolved through `config_dir`: on macOS
        // and Windows that is `Library/Application Support` / `AppData`, which
        // is not where Muse Code looks.
        "muse_code" => under(home, HOME, &[".config", "muse", "settings.json"]),
        // Antigravity's global MCP file. Older builds kept it at
        // `~/.gemini/antigravity/mcp_config.json`; that folder still counts as
        // a leftover in `client_home_dir_reading`.
        "antigravity" => under(home, HOME, &[".gemini", "config", "mcp_config.json"]),
        "copilot_cli" => under(home, HOME, &[".copilot", "mcp-config.json"]),
        // VS Code's user-profile `mcp.json` sits in its user-data folder, which
        // IS the platform config dir: `~/Library/Application Support/Code/User`
        // on macOS, `$XDG_CONFIG_HOME/Code/User` on Linux, `%APPDATA%\Code\User`
        // on Windows.
        "vscode" => under(
            config_dir,
            "an application-configuration directory",
            &["Code", "User", "mcp.json"],
        ),
        "windsurf" => under(home, HOME, &[".codeium", "windsurf", "mcp_config.json"]),
        "kiro" => under(home, HOME, &[".kiro", "settings", "mcp.json"]),
        // OpenCode reads `~/.config/opencode/opencode.json` on every platform
        // (not `config_dir`, same as Muse Code). A JSONC-only install is
        // refused, never rewritten: see `read_config_for`.
        "opencode" => under(home, HOME, &[".config", "opencode", "opencode.json"]),
        _ => ClientConfigPath::UnknownClient,
    }
}

const MCP_SERVER_KEY: &str = "wenlan";
const LEGACY_MCP_SERVER_KEY: &str = "origin";

/// How a client's JSON config file spells "my MCP servers". Chosen by
/// `client_type` ([`JsonDialect::for_client`]) and consulted by every JSON
/// reader and writer in this file, so detection, health, write, repair and
/// removal can never disagree about which key a client's entry lives under: a
/// writer that wrote `mcp_servers` while detection read `mcpServers` would
/// report a freshly connected tool as "not set up" forever.
///
/// Codex CLI is not a dialect: it is TOML, and has its own `_toml` functions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JsonDialect {
    /// `{"mcpServers": {"wenlan": {"command": .., "args": [..]}}}` — Cursor,
    /// Claude Code, Claude Desktop and Gemini CLI.
    McpServers,
    /// Muse Code's `settings.json`: `{"schema_version": 1, "mcp_servers":
    /// {"wenlan": {"transport": "stdio", "mode": "optional", "command": ..,
    /// "args": [..]}}}`. Snake-case servers key; a mandatory `schema_version`
    /// (a settings file without it fails to load as "malformed settings file");
    /// and a per-server `mode` that defaults to `required`, which ABORTS the
    /// whole Muse Code run when that server fails to start.
    Muse,
    /// GitHub Copilot CLI's `~/.copilot/mcp-config.json`: `mcpServers`, with
    /// `"type": "local"` and `"tools": ["*"]` on each entry (a local server
    /// with no tool allowlist exposes no tools).
    Copilot,
    /// VS Code's user `mcp.json`: `{"servers": {"wenlan": {"type": "stdio",
    /// "command": .., "args": [..]}}}`.
    VsCode,
    /// OpenCode's `opencode.json`: `{"mcp": {"wenlan": {"type": "local",
    /// "command": [cmd, ..args], "enabled": true}}}`. A different ENTRY shape:
    /// the program and its arguments are one list, so the entry readers
    /// ([`entry_spec_for`]) split it rather than reading `args`.
    OpenCode,
}

impl JsonDialect {
    fn for_client(client_type: &str) -> Self {
        match client_type {
            "muse_code" => JsonDialect::Muse,
            "copilot_cli" => JsonDialect::Copilot,
            "vscode" => JsonDialect::VsCode,
            "opencode" => JsonDialect::OpenCode,
            _ => JsonDialect::McpServers,
        }
    }

    /// The top-level key the client's servers live under.
    fn servers_key(self) -> &'static str {
        match self {
            JsonDialect::McpServers | JsonDialect::Copilot => "mcpServers",
            JsonDialect::Muse => "mcp_servers",
            JsonDialect::VsCode => "servers",
            JsonDialect::OpenCode => "mcp",
        }
    }

    /// The JSON entry this dialect writes for `entry`. `{command, args}` plus
    /// [`JsonDialect::decorate_entry`]'s fields, except for OpenCode, whose
    /// entry holds the program and its arguments in one `command` list.
    fn entry_value(self, entry: WenlanMcpEntry) -> Result<serde_json::Value, AppError> {
        if self == JsonDialect::OpenCode {
            let mut command = Vec::with_capacity(entry.args.len() + 1);
            command.push(entry.command);
            command.extend(entry.args);
            return Ok(serde_json::json!({
                "type": "local",
                "command": command,
                "enabled": true,
            }));
        }
        let mut value =
            serde_json::to_value(entry).map_err(|e| AppError::Generic(e.to_string()))?;
        self.decorate_entry(&mut value);
        Ok(value)
    }

    /// Add the fields this dialect needs beside `command`/`args` to the entry
    /// about to be written. A no-op for `mcpServers` clients, whose entry stays
    /// exactly `{command, args}`.
    ///
    /// Muse Code gets `mode: "optional"` deliberately: with the default
    /// (`required`) a stopped or uninstalled Wenlan would stop the user's Muse
    /// Code from running at all, and a memory layer must never be able to do
    /// that to the tool it serves.
    fn decorate_entry(self, entry: &mut serde_json::Value) {
        let Some(entry) = entry.as_object_mut() else {
            return;
        };
        match self {
            JsonDialect::Muse => {
                entry.insert("transport".into(), serde_json::json!("stdio"));
                entry.insert("mode".into(), serde_json::json!("optional"));
            }
            JsonDialect::Copilot => {
                entry.insert("type".into(), serde_json::json!("local"));
                entry.insert("tools".into(), serde_json::json!(["*"]));
            }
            JsonDialect::VsCode => {
                entry.insert("type".into(), serde_json::json!("stdio"));
            }
            // OpenCode's entry is built whole by `entry_value`.
            JsonDialect::McpServers | JsonDialect::OpenCode => {}
        }
    }

    /// Add the root-level keys this dialect requires, WITHOUT touching a value
    /// the user already has: `schema_version` is written only when the key is
    /// absent, so a file already pinned to a newer schema is never downgraded.
    /// `root` is known to be an object by the time this runs
    /// ([`check_json_config_shape`]).
    fn prepare_root(self, root: &mut serde_json::Value) {
        if let (JsonDialect::Muse, Some(root)) = (self, root.as_object_mut()) {
            root.entry("schema_version")
                .or_insert_with(|| serde_json::json!(1));
        }
    }
}

/// Check if a JSON config string already has a Wenlan entry or legacy Origin
/// entry. `Ok(false)` is a MEASURED no — the file parsed and holds no such
/// entry. A body that would not parse is `Err`, never `Ok(false)`: see
/// [`ConfigRead::asks`].
///
/// A parsed file with no `mcpServers` key, or one whose `mcpServers` is not a
/// container, is a genuine `Ok(false)`: it demonstrably holds no
/// `mcpServers.wenlan` entry. Only the PARSE is a failed measurement.
///
/// The `mcpServers` spelling; [`has_configured_entry_in`] takes the dialect.
#[cfg(test)]
fn has_configured_entry(json_str: &str) -> Result<bool, String> {
    has_configured_entry_in(JsonDialect::McpServers, json_str)
}

/// [`has_configured_entry`] under `dialect`'s servers key. A file that holds a
/// Wenlan entry under the OTHER dialect's key is a measured `Ok(false)`: a
/// Muse Code `settings.json` with `mcpServers.wenlan` in it is an entry Muse
/// Code never reads.
fn has_configured_entry_in(dialect: JsonDialect, json_str: &str) -> Result<bool, String> {
    let value = parse_json(json_str)?;
    Ok(match value.get(dialect.servers_key()) {
        Some(servers) => {
            servers.get(MCP_SERVER_KEY).is_some() || servers.get(LEGACY_MCP_SERVER_KEY).is_some()
        }
        None => false,
    })
}

/// TOML variant for Codex CLI (`[mcp_servers.*]` tables).
fn has_configured_entry_toml(toml_str: &str) -> Result<bool, String> {
    let doc = parse_toml(toml_str)?;
    Ok(match doc.get("mcp_servers") {
        Some(servers) => {
            servers.get(MCP_SERVER_KEY).is_some() || servers.get(LEGACY_MCP_SERVER_KEY).is_some()
        }
        None => false,
    })
}

/// Check whether a JSON config holds BOTH the live `wenlan` entry AND the
/// legacy `origin` entry under `mcpServers` — the raw+raw duplicate a client
/// with no plugin path (Cursor, Gemini CLI) lands in after the origin→wenlan
/// rename, where both entries launch a server against the same daemon. Distinct
/// from `has_configured_entry`, which is an OR: the fix here removes only the
/// stale `origin`, so detection has to know both are present, not just one.
///
/// The `mcpServers` spelling; [`has_both_raw_entries_in`] takes the dialect.
#[cfg(test)]
fn has_both_raw_entries(json_str: &str) -> Result<bool, String> {
    has_both_raw_entries_in(JsonDialect::McpServers, json_str)
}

/// [`has_both_raw_entries`] under `dialect`'s servers key.
fn has_both_raw_entries_in(dialect: JsonDialect, json_str: &str) -> Result<bool, String> {
    let value = parse_json(json_str)?;
    Ok(match value.get(dialect.servers_key()) {
        Some(servers) => {
            servers.get(MCP_SERVER_KEY).is_some() && servers.get(LEGACY_MCP_SERVER_KEY).is_some()
        }
        None => false,
    })
}

/// TOML variant of `has_both_raw_entries` for Codex CLI (`[mcp_servers.*]`).
fn has_both_raw_entries_toml(toml_str: &str) -> Result<bool, String> {
    let doc = parse_toml(toml_str)?;
    Ok(match doc.get("mcp_servers") {
        Some(servers) => {
            servers.get(MCP_SERVER_KEY).is_some() && servers.get(LEGACY_MCP_SERVER_KEY).is_some()
        }
        None => false,
    })
}

/// Whether a Claude Code `settings.json` blob has the Wenlan plugin enabled.
/// `enabledPlugins` keys are `<plugin>@<marketplace>`, and the marketplace
/// name varies by install (`wenlan@7xuanlu` fresh, `wenlan@7xuanlu-wenlan` on
/// a machine that added the old self-marketplace) — match the `wenlan@`
/// prefix, never a literal marketplace name, or the check breaks for exactly
/// one of the two populations.
///
/// A missing `enabledPlugins` key is a measured "no plugin" — the file was
/// understood and it enables nothing. MALFORMED JSON IS NOT.
fn claude_code_plugin_enabled(settings_json: &str) -> Result<bool, String> {
    let value = parse_json(settings_json)?;
    Ok(value
        .get("enabledPlugins")
        .and_then(|plugins| plugins.as_object())
        .is_some_and(|plugins| {
            plugins
                .iter()
                .any(|(key, val)| key.starts_with("wenlan@") && val.as_bool() == Some(true))
        }))
}

/// Reads the real `~/.claude/settings.json` and checks it via
/// `claude_code_plugin_enabled`. Split out so the matching logic stays a pure,
/// directly testable function.
fn claude_code_plugin_enabled_on_disk(home: Option<&Path>) -> Reading {
    match home {
        Some(home) => read_config(&home.join(".claude").join("settings.json"))
            .asks(claude_code_plugin_enabled),
        None => Reading::Unreadable {
            error: "the platform would not report a home directory".to_string(),
        },
    }
}

/// Whether a Codex CLI `config.toml` blob has the Wenlan plugin enabled —
/// `[plugins."wenlan@<marketplace>"] enabled = true`. The marketplace name
/// varies (`wenlan-local` pre-wenlan-app/wenlan#348, `7xuanlu-wenlan` after),
/// so match the `wenlan@` prefix, never a literal marketplace name — same
/// reasoning as `claude_code_plugin_enabled`.
fn codex_cli_plugin_enabled(toml_str: &str) -> Result<bool, String> {
    let doc = parse_toml(toml_str)?;
    Ok(doc
        .get("plugins")
        .and_then(|plugins| plugins.as_table_like())
        .is_some_and(|plugins| {
            plugins.iter().any(|(key, item)| {
                key.starts_with("wenlan@")
                    && item.get("enabled").and_then(|v| v.as_bool()) == Some(true)
            })
        }))
}

/// Reads the real `~/.codex/config.toml` and checks it via
/// `codex_cli_plugin_enabled`. Split out so the matching logic stays a pure,
/// directly testable function.
fn codex_cli_plugin_enabled_on_disk(home: Option<&Path>) -> Reading {
    match home {
        Some(home) => {
            read_config(&home.join(".codex").join("config.toml")).asks(codex_cli_plugin_enabled)
        }
        None => Reading::Unreadable {
            error: "the platform would not report a home directory".to_string(),
        },
    }
}

/// Whether a Claude Desktop chat-side plugin manifest (`rpm/manifest.json`
/// under a session directory) lists the Wenlan plugin. True iff `plugins[]`
/// contains an entry whose `name` field is exactly `"wenlan"` — matching
/// `marketplaceName` instead would be wrong, since a user's own upload
/// marketplace can be named anything (`marketplaceName` values seen in the
/// wild: `"My Uploads"`, `"knowledge-work-plugins"`). A missing `plugins` key
/// is a measured "no plugin"; a manifest that would not parse is `Err` — same
/// policy as `claude_code_plugin_enabled`, for the same round-6 reason.
fn claude_desktop_plugin_enabled(manifest_json: &str) -> Result<bool, String> {
    let value = parse_json(manifest_json)?;
    Ok(value
        .get("plugins")
        .and_then(|plugins| plugins.as_array())
        .is_some_and(|plugins| {
            plugins
                .iter()
                .any(|p| p.get("name").and_then(|n| n.as_str()) == Some("wenlan"))
        }))
}

/// Extract the pinned account id (`lastKnownAccountUuid`) from a Claude
/// Desktop `config.json` blob.
///
/// THREE outcomes, not two. `Ok(None)` is "the file was understood and pins no
/// account" — a real negative, and the reason the sessions scan is skipped.
/// `Err` is "the file could not be parsed", which establishes nothing about
/// whether an account is pinned.
fn claude_desktop_account_id(config_json: &str) -> Result<Option<String>, String> {
    let value = parse_json(config_json)?;
    Ok(value
        .get("lastKnownAccountUuid")
        .and_then(|id| id.as_str())
        .map(String::from))
}

/// The directory holding one subdirectory per chat-side session id for a
/// given account: `<support_dir>/local-agent-mode-sessions/<account_id>`.
/// Scoping to the pinned account id means the `skills-plugin` sentinel
/// directory that lives alongside real account-id directories under
/// `local-agent-mode-sessions/` is never visited — it isn't a UUID, so it can
/// never be `account_id`.
fn claude_desktop_account_sessions_dir(support_dir: &Path, account_id: &str) -> PathBuf {
    support_dir
        .join("local-agent-mode-sessions")
        .join(account_id)
}

/// Whether any session under `account_sessions_dir` has a
/// `rpm/manifest.json` listing the Wenlan plugin. One directory per session
/// id; any single session counting is enough (a user can have several open
/// at once). A session directory without `rpm/manifest.json`, or with one
/// that fails to parse, is silently skipped — never a panic, never treated
/// as a match.
fn claude_desktop_plugin_enabled_in_sessions_dir(account_sessions_dir: &Path) -> Reading {
    let entries = match std::fs::read_dir(account_sessions_dir) {
        Ok(entries) => entries,
        // No sessions directory: Desktop's chat side has never run. A real
        // negative.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Reading::No,
        Err(e) => {
            return Reading::Unreadable {
                error: e.to_string(),
            }
        }
    };
    // A session whose manifest could NOT be read is neither a match nor an
    // absence: remembered, and reported only if nothing else matched.
    let mut unreadable: Option<String> = None;
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                unreadable.get_or_insert_with(|| e.to_string());
                continue;
            }
        };
        match read_config(&entry.path().join("rpm").join("manifest.json"))
            .asks(claude_desktop_plugin_enabled)
        {
            Reading::Yes => return Reading::Yes,
            Reading::No => {}
            Reading::Unreadable { error } => {
                unreadable.get_or_insert(error);
            }
        }
    }
    match unreadable {
        Some(error) => Reading::Unreadable { error },
        None => Reading::No,
    }
}

/// Whether the Wenlan plugin is enabled for Claude Desktop, given the
/// already-resolved support directory (normally
/// `~/Library/Application Support/Claude`). Composes
/// `claude_desktop_account_id` + `claude_desktop_account_sessions_dir` +
/// `claude_desktop_plugin_enabled_in_sessions_dir` end to end, so the full
/// real-world path — `config.json` -> account id -> sessions dir -> manifest
/// scan — is exercised under test with a tempdir standing in for `support_dir`.
fn claude_desktop_plugin_enabled_for_support_dir(support_dir: &Path) -> Reading {
    let account_id = match read_config(&support_dir.join("config.json")) {
        ConfigRead::Contents(contents) => match claude_desktop_account_id(&contents) {
            Ok(Some(account_id)) => account_id,
            // The file was read and simply pins no account: a measured no.
            // There is no account whose sessions could hold a manifest.
            Ok(None) => return Reading::No,
            // The sessions directory is named after the account id, so an
            // unparseable `config.json` means the scan never happened at all —
            // which is not "the plugin is not there".
            Err(error) => return Reading::Unreadable { error },
        },
        ConfigRead::Absent => return Reading::No,
        ConfigRead::Unreadable(error) => return Reading::Unreadable { error },
    };
    claude_desktop_plugin_enabled_in_sessions_dir(&claude_desktop_account_sessions_dir(
        support_dir,
        &account_id,
    ))
}

/// Reads the real Claude Desktop support directory
/// (`~/Library/Application Support/Claude`) and checks it via
/// `claude_desktop_plugin_enabled_for_support_dir`. READ-ONLY: never creates,
/// writes, or modifies anything under Claude Desktop's support directory —
/// that state belongs to another vendor's app.
fn claude_desktop_plugin_enabled_on_disk(config_dir: Option<&Path>) -> Reading {
    match config_dir {
        Some(config_dir) => {
            claude_desktop_plugin_enabled_for_support_dir(&config_dir.join("Claude"))
        }
        None => Reading::Unreadable {
            error: "the platform would not report an application-configuration directory"
                .to_string(),
        },
    }
}

/// The reading contributed by the home-directory lookup ITSELF, before any
/// candidate under it is probed. A `None` home is a lookup that failed, so
/// every `~/...` candidate went unprobed; seeding a bundle search with this
/// keeps a half-completed search from reporting "not installed".
fn home_lookup_reading(home: Option<&Path>) -> Reading {
    match home {
        Some(_) => Reading::No,
        None => Reading::Unreadable {
            error: "could not determine the home directory".to_string(),
        },
    }
}

/// Where ChatGPT desktop can be installed. Its Codex pane reads the same
/// `~/.codex/config.toml` as Codex CLI (OpenAI merged Codex into the ChatGPT
/// app), so finding the bundle means the `codex_cli` row applies.
fn chatgpt_app_candidates(home: Option<&Path>) -> Vec<PathBuf> {
    let mut out = vec![PathBuf::from("/Applications/ChatGPT.app")];
    if let Some(home) = home {
        out.push(home.join("Applications/ChatGPT.app"));
    }
    out
}

/// Whether the Codex CLI row should be detected: either its shared
/// `~/.codex/config.toml` exists, or ChatGPT desktop is installed. Feeds the
/// single `codex_cli` row — never a second row for ChatGPT.
///
/// `exists` is injected so the bundle paths themselves are under test: a typo
/// in a candidate path fails `codex_cli_detected_finds_chatgpt_in_*`, and the
/// call site cannot silently opt out of the probe (there is no bool to pass).
fn codex_cli_detected(
    config_exists: Reading,
    home: Option<&Path>,
    exists: impl Fn(&Path) -> Reading,
) -> Reading {
    chatgpt_app_candidates(home)
        .iter()
        .map(|p| exists(p.as_path()))
        // `Reading::or`, not `any`: a bundle path the OS refused to stat is not
        // a bundle that is absent. Seeded with the home lookup itself, since
        // with no home the `~/Applications` candidate was never built.
        .fold(config_exists.or(home_lookup_reading(home)), Reading::or)
}

/// The operating system family detection is looking at. A value rather than a
/// `cfg!`, so every OS's candidate list is testable from any host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostKind {
    MacOs,
    Windows,
    Linux,
}

impl HostKind {
    pub(crate) fn current() -> Self {
        match std::env::consts::OS {
            "macos" => HostKind::MacOs,
            "windows" => HostKind::Windows,
            _ => HostKind::Linux,
        }
    }
}

/// Everything detection reads besides the client config files themselves,
/// handed in so a test decides what the machine looks like. The real machine
/// is [`detect_mcp_clients`]; a hermetic one is [`detect_mcp_clients_from`].
pub(crate) struct DetectProbes<'a> {
    pub host: HostKind,
    /// The per-user program folder the platform reports (`%LOCALAPPDATA%` on
    /// Windows). Only consulted on Windows.
    pub local_data_dir: Option<&'a Path>,
    /// `%ProgramFiles%`-style folders. Only consulted on Windows.
    pub program_files: &'a [PathBuf],
    /// Is anything at this path, in three values.
    pub exists: &'a dyn Fn(&Path) -> Reading,
    /// The children of a folder (empty when it cannot be listed).
    pub list_dir: &'a dyn Fn(&Path) -> Vec<PathBuf>,
    /// Resolve a CLI by name without ever starting a shell: `PATH`, then the
    /// usual install folders (`plugin_install::resolve_binary_no_shell`).
    pub which: &'a dyn Fn(&str) -> Option<PathBuf>,
    /// What a command path is, for entry validation.
    pub probe_command: &'a dyn Fn(&Path) -> CandidateProbe,
}

/// What the per-user program folder lookup contributes before any candidate
/// under it is probed. Windows only: elsewhere no candidate hangs off it.
fn local_data_lookup_reading(host: HostKind, local: Option<&Path>) -> Reading {
    match (host, local) {
        (HostKind::Windows, None) => Reading::Unreadable {
            error: "could not determine the local application-data directory".to_string(),
        },
        _ => Reading::No,
    }
}

/// `%ProgramFiles%`, `%ProgramFiles(x86)%` and `%ProgramW6432%`, deduplicated.
/// Empty off Windows.
fn program_files_dirs() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    if std::env::consts::OS != "windows" {
        return out;
    }
    for key in ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"] {
        if let Some(dir) = std::env::var_os(key).filter(|value| !value.is_empty()) {
            let dir = PathBuf::from(dir);
            if !out.contains(&dir) {
                out.push(dir);
            }
        }
    }
    out
}

fn list_child_paths(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                .collect()
        })
        .unwrap_or_default()
}

/// Where a client's app (not its CLI) can be installed, per OS. Best effort:
/// a program installed somewhere else is still found through its CLI on
/// `PATH`, or reported as `config_only` rather than as absent.
fn app_candidates(
    client_type: &str,
    host: HostKind,
    home: Option<&Path>,
    local: Option<&Path>,
    program_files: &[PathBuf],
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let under_home = |tail: &str| home.map(|home| home.join(tail));
    match (client_type, host) {
        ("cursor", HostKind::MacOs) => {
            out.push(PathBuf::from("/Applications/Cursor.app"));
            out.extend(under_home("Applications/Cursor.app"));
        }
        ("cursor", HostKind::Windows) => {
            // Both spellings of the per-user installer's folder: the second
            // is a different directory on a case-sensitive volume.
            if let Some(local) = local {
                out.push(local.join("Programs").join("cursor").join("Cursor.exe"));
                out.push(local.join("Programs").join("Cursor").join("Cursor.exe"));
            }
            out.extend(
                program_files
                    .iter()
                    .map(|dir| dir.join("Cursor").join("Cursor.exe")),
            );
        }
        ("cursor", HostKind::Linux) => {
            out.extend(
                [
                    "/usr/bin/cursor",
                    "/usr/local/bin/cursor",
                    "/opt/Cursor/cursor",
                    "/opt/cursor/cursor",
                ]
                .map(PathBuf::from),
            );
            out.extend(under_home(".local/bin/cursor"));
        }
        ("claude_desktop", HostKind::MacOs) => {
            out.push(PathBuf::from("/Applications/Claude.app"));
            out.extend(under_home("Applications/Claude.app"));
        }
        ("claude_desktop", HostKind::Windows) => {
            if let Some(local) = local {
                out.push(local.join("AnthropicClaude").join("claude.exe"));
                out.push(local.join("Programs").join("Claude").join("Claude.exe"));
            }
        }
        // The VS Code family (VS Code and the editors built from it) installs
        // the same way everywhere: a `.app` bundle on macOS, a per-user
        // `%LOCALAPPDATA%\Programs\<folder>\<exe>` or a machine-wide
        // `%ProgramFiles%\<folder>\<exe>` on Windows, and
        // `/usr/share/<pkg>/<bin>` from the .deb/.rpm on Linux (its `/usr/bin`
        // launcher is found through the CLI lookup).
        (client_type, host) => {
            let Some(family) = vscode_family_install(client_type) else {
                return out;
            };
            match host {
                HostKind::MacOs => {
                    out.push(PathBuf::from("/Applications").join(family.mac_bundle));
                    out.extend(under_home(&format!("Applications/{}", family.mac_bundle)));
                }
                HostKind::Windows => {
                    if let Some(local) = local {
                        out.push(
                            local
                                .join("Programs")
                                .join(family.windows_folder)
                                .join(family.windows_exe),
                        );
                    }
                    out.extend(
                        program_files
                            .iter()
                            .map(|dir| dir.join(family.windows_folder).join(family.windows_exe)),
                    );
                }
                HostKind::Linux => {
                    out.extend(family.linux_paths.iter().map(PathBuf::from));
                }
            }
        }
    }
    out
}

/// Where one editor of the VS Code family installs, per OS.
struct VscodeFamilyInstall {
    mac_bundle: &'static str,
    windows_folder: &'static str,
    windows_exe: &'static str,
    linux_paths: &'static [&'static str],
}

fn vscode_family_install(client_type: &str) -> Option<VscodeFamilyInstall> {
    Some(match client_type {
        "antigravity" => VscodeFamilyInstall {
            mac_bundle: "Antigravity.app",
            windows_folder: "Antigravity",
            windows_exe: "Antigravity.exe",
            linux_paths: &["/usr/share/antigravity/antigravity"],
        },
        "vscode" => VscodeFamilyInstall {
            mac_bundle: "Visual Studio Code.app",
            windows_folder: "Microsoft VS Code",
            windows_exe: "Code.exe",
            linux_paths: &["/usr/share/code/code", "/snap/bin/code"],
        },
        "windsurf" => VscodeFamilyInstall {
            mac_bundle: "Windsurf.app",
            windows_folder: "Windsurf",
            windows_exe: "Windsurf.exe",
            linux_paths: &["/usr/share/windsurf/windsurf"],
        },
        "kiro" => VscodeFamilyInstall {
            mac_bundle: "Kiro.app",
            windows_folder: "Kiro",
            windows_exe: "Kiro.exe",
            linux_paths: &["/usr/share/kiro/kiro"],
        },
        _ => return None,
    })
}

/// CLI names that mean the client is installed, resolved without a shell.
fn client_cli_names(client_type: &str) -> &'static [&'static str] {
    match client_type {
        // `cursor-agent` is Cursor's CLI. It reads the same ~/.cursor config
        // and skills, and can be installed without the editor.
        "cursor" => &["cursor", "cursor-agent"],
        "claude_code" => &["claude"],
        // The unofficial Linux builds of Claude Desktop ship this launcher;
        // there is no official one on macOS or Windows, so a miss is normal.
        "claude_desktop" => &["claude-desktop"],
        "gemini_cli" => &["gemini"],
        "codex_cli" => &["codex"],
        "muse_code" => &["muse"],
        "antigravity" => &["agy", "antigravity"],
        "copilot_cli" => &["copilot"],
        "vscode" => &["code"],
        "windsurf" => &["windsurf"],
        "kiro" => &["kiro", "kiro-cli"],
        "opencode" => &["opencode"],
        _ => &[],
    }
}

/// Folders a Cursor AppImage is commonly kept in (it has no installer).
fn cursor_appimage_dirs(home: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = vec![PathBuf::from("/opt")];
    if let Some(home) = home {
        dirs.push(home.join("Applications"));
        dirs.push(home.join(".local/bin"));
        dirs.push(home.join("bin"));
    }
    dirs
}

/// Whether `dir` holds a file named like `<prefix>*.appimage`, ignoring case.
fn dir_has_appimage(dir: &Path, prefix: &str, list_dir: &dyn Fn(&Path) -> Vec<PathBuf>) -> bool {
    list_dir(dir).iter().any(|child| {
        child
            .file_name()
            .map(|name| name.to_string_lossy().to_lowercase())
            .is_some_and(|name| name.starts_with(prefix) && name.ends_with(".appimage"))
    })
}

/// Whether a Windows Store (MSIX) package folder for Claude is present.
fn has_claude_store_package(local: &Path, list_dir: &dyn Fn(&Path) -> Vec<PathBuf>) -> bool {
    list_dir(&local.join("Packages")).iter().any(|child| {
        child
            .file_name()
            .map(|name| name.to_string_lossy().to_lowercase())
            .is_some_and(|name| name.starts_with("claude_"))
    })
}

/// Whether the client PROGRAM is here: an app bundle or `.exe`, an AppImage, or
/// its CLI. Never a config file — that is the other half of
/// [`install_state_of`]. Same tri-state rule as [`codex_cli_detected`]: a
/// candidate the OS refused to stat, or a folder the platform would not
/// report, makes an empty result "unknown", never "absent".
fn program_reading(client_type: &str, home: Option<&Path>, probes: &DetectProbes) -> Reading {
    let seed = match client_type {
        "cursor" | "claude_desktop" | "antigravity" | "vscode" | "windsurf" | "kiro" => {
            home_lookup_reading(home).or(local_data_lookup_reading(
                probes.host,
                probes.local_data_dir,
            ))
        }
        _ => Reading::No,
    };
    let app = if client_type == "codex_cli" {
        // The ChatGPT desktop bundle: its Codex pane reads the same config.
        // `Reading::No` as the config half leaves just the bundle search.
        codex_cli_detected(Reading::No, home, probes.exists)
    } else {
        app_candidates(
            client_type,
            probes.host,
            home,
            probes.local_data_dir,
            probes.program_files,
        )
        .iter()
        .map(|p| (probes.exists)(p.as_path()))
        .fold(seed, Reading::or)
    };

    let mut found = app;
    for name in client_cli_names(client_type) {
        found = found.or(Reading::of((probes.which)(name).is_some()));
    }
    if client_type == "cursor" && probes.host == HostKind::Linux {
        found = found.or(Reading::of(
            cursor_appimage_dirs(home)
                .iter()
                .any(|dir| dir_has_appimage(dir, "cursor", probes.list_dir)),
        ));
    }
    if client_type == "claude_desktop" && probes.host == HostKind::Windows {
        if let Some(local) = probes.local_data_dir {
            found = found.or(Reading::of(has_claude_store_package(
                local,
                probes.list_dir,
            )));
        }
    }
    found
}

/// What a client leaves behind in its own home folder, beyond the one config
/// file `detect_mcp_clients_from` already reads: Cursor's `~/.cursor`, Codex's
/// `~/.codex` and Muse Code's `~/.config/muse` exist once the client has run,
/// with or without a `mcp.json` / `config.toml` / `settings.json`. Only the
/// folder's existence is asked — never its contents (`config.toml` is the
/// user's, and read only where it already was).
fn client_home_dir_reading(
    client_type: &str,
    home: Option<&Path>,
    exists: &dyn Fn(&Path) -> Reading,
) -> Reading {
    // Path components rather than one string, so `.config/muse` is joined with
    // the platform's own separator. Several places are an OR.
    let dirs: &[&[&str]] = match client_type {
        "cursor" => &[&[".cursor"]],
        "codex_cli" => &[&[".codex"]],
        "muse_code" => &[&[".config", "muse"]],
        // Antigravity's own folder under `~/.gemini`, current and older. NOT
        // `~/.gemini` itself, which Gemini CLI owns, and not `~/.gemini/skills`,
        // which Wenlan's own skills install creates.
        "antigravity" => &[&[".gemini", "config"], &[".gemini", "antigravity"]],
        // Files Copilot CLI writes on first run, not the `~/.copilot` folder:
        // installing skills for VS Code creates `~/.copilot/skills`, and that
        // must not make a Copilot CLI that was never here look like a leftover.
        "copilot_cli" => &[&[".copilot", "config.json"], &[".copilot", "logs"]],
        "windsurf" => &[&[".codeium", "windsurf"]],
        "kiro" => &[&[".kiro"]],
        "opencode" => &[&[".config", "opencode"]],
        _ => return Reading::No,
    };
    match home {
        Some(home) => dirs
            .iter()
            .map(|dir| {
                exists(
                    &dir.iter()
                        .fold(home.to_path_buf(), |acc, part| acc.join(part)),
                )
            })
            .fold(Reading::No, Reading::or),
        // The missing home is already reported through the config path and the
        // program search; adding it here would only repeat it.
        None => Reading::No,
    }
}

/// Program evidence and leftover evidence, combined into the one state the UI
/// shows. An unreadable half never becomes "config only": that would claim the
/// program is absent when it merely could not be looked for.
fn install_state_of(program: &Reading, leftovers: &Reading) -> InstallState {
    match (program, leftovers) {
        (Reading::Yes, _) => InstallState::Installed,
        (Reading::No, Reading::Yes) => InstallState::ConfigOnly,
        (Reading::Unreadable { error }, _) | (Reading::No, Reading::Unreadable { error }) => {
            InstallState::Unreadable {
                error: error.clone(),
            }
        }
        (Reading::No, Reading::No) => InstallState::NotFound,
    }
}

/// Whether a client config's contents hold a raw wenlan/origin `mcpServers`
/// (or Codex's `[mcp_servers.*]`, or Muse Code's `mcp_servers`) entry — the
/// file-based half of `already_configured`. `wire_state` needs the two halves
/// kept apart: a raw entry and a missing plugin point at different fixes.
fn raw_entry_reading(client_type: &str, config: &ConfigRead) -> Reading {
    config.asks(|s| {
        if client_type == "codex_cli" {
            has_configured_entry_toml(s)
        } else {
            has_configured_entry_in(JsonDialect::for_client(client_type), s)
        }
    })
}

/// Whether a client config's contents hold BOTH the `wenlan` entry and the
/// legacy `origin` entry — the raw+raw duplicate. Mirrors
/// `raw_entry_reading`'s TOML/JSON split, sharing
/// `has_both_raw_entries`/`has_both_raw_entries_toml` so detection and the
/// `remove_legacy_origin_entry` fix stay symmetric. This is the one signal a
/// no-plugin client (cursor, gemini_cli, muse_code) needs: those can never trip the
/// plugin+raw double-registration path in `wire_state`, so without it their
/// raw+raw duplicate is invisible.
fn raw_duplicate_reading(client_type: &str, config: &ConfigRead) -> Reading {
    config.asks(|s| {
        if client_type == "codex_cli" {
            has_both_raw_entries_toml(s)
        } else {
            has_both_raw_entries_in(JsonDialect::for_client(client_type), s)
        }
    })
}

/// `raw_entry_reading` against a path, for callers holding one rather than an
/// already-read file (the removal verbs' round-trip tests).
#[cfg(test)]
pub(crate) fn client_config_has_raw_entry(client_type: &str, config_path: &Path) -> Reading {
    raw_entry_reading(
        client_type,
        &read_config_for(JsonDialect::for_client(client_type), config_path),
    )
}

/// `raw_duplicate_reading` against a path. Same reason.
#[cfg(test)]
pub(crate) fn client_config_has_both_raw_entries(client_type: &str, config_path: &Path) -> Reading {
    raw_duplicate_reading(
        client_type,
        &read_config_for(JsonDialect::for_client(client_type), config_path),
    )
}

/// Whether `client_type`'s Wenlan plugin is enabled — the plugin half of
/// `already_configured` for the three clients that support one. `cursor`,
/// `gemini_cli` and `muse_code` have no plugin path, so they are a MEASURED `No` here (and
/// route to `"config"` in `wire_state`, never `"plugin"`): there is no plugin
/// surface to fail to read.
fn client_plugin_enabled_for(
    client_type: &str,
    home: Option<&Path>,
    config_dir: Option<&Path>,
) -> Reading {
    match client_type {
        "claude_code" => claude_code_plugin_enabled_on_disk(home),
        "codex_cli" => codex_cli_plugin_enabled_on_disk(home),
        "claude_desktop" => claude_desktop_plugin_enabled_on_disk(config_dir),
        _ => Reading::No,
    }
}

/// One `wenlan` entry reduced to the parts a client needs to start it.
struct EntrySpec {
    /// `None` when the entry has no usable `command` string.
    command: Option<String>,
    /// `Err` is the sentence for an `args` value that is not a list of strings.
    args: Result<Vec<String>, String>,
    /// A `url` entry (a remote server) has no command to check.
    has_url: bool,
    /// Set when the entry is not even a table/object, for the detail sentence.
    not_an_entry: Option<&'static str>,
}

fn entry_spec_json(entry: &serde_json::Value) -> EntrySpec {
    let command = entry
        .get("command")
        .and_then(|c| c.as_str())
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(String::from);
    let args = match entry.get("args") {
        None | Some(serde_json::Value::Null) => Ok(Vec::new()),
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .map(|item| item.as_str().map(String::from))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| "`args` has to be a list of strings".to_string()),
        Some(other) => Err(format!(
            "`args` is {}, but it has to be a list of strings",
            json_type_name(other)
        )),
    };
    EntrySpec {
        command,
        args,
        has_url: entry
            .get("url")
            .and_then(|u| u.as_str())
            .is_some_and(|u| !u.trim().is_empty()),
        not_an_entry: (!entry.is_object()).then(|| json_type_name(entry)),
    }
}

/// [`entry_spec_json`] for `dialect`'s entry shape. OpenCode keeps the program
/// and its arguments in ONE `command` list (`["wenlan-mcp", "--agent-name",
/// "opencode"]`), so the list is split here; a `command` string is the shape
/// every other client uses and OpenCode refuses, so it reads as invalid
/// arguments (and a repair rewrites it), never as a healthy entry.
fn entry_spec_for(dialect: JsonDialect, entry: &serde_json::Value) -> EntrySpec {
    if dialect != JsonDialect::OpenCode {
        return entry_spec_json(entry);
    }
    let has_url = entry
        .get("url")
        .and_then(|u| u.as_str())
        .is_some_and(|u| !u.trim().is_empty());
    let not_an_entry = (!entry.is_object()).then(|| json_type_name(entry));
    let (command, args) = match entry.get("command") {
        Some(serde_json::Value::Array(items)) => {
            match items
                .iter()
                .map(|item| item.as_str().map(String::from))
                .collect::<Option<Vec<_>>>()
            {
                Some(parts) => {
                    let mut parts = parts.into_iter();
                    let command = parts
                        .next()
                        .map(|c| c.trim().to_string())
                        .filter(|c| !c.is_empty());
                    (command, Ok(parts.collect()))
                }
                None => (
                    items
                        .first()
                        .and_then(|c| c.as_str())
                        .map(|c| c.trim().to_string())
                        .filter(|c| !c.is_empty()),
                    Err("`command` has to be a list of strings".to_string()),
                ),
            }
        }
        Some(serde_json::Value::String(command)) if !command.trim().is_empty() => (
            Some(command.trim().to_string()),
            Err(
                "OpenCode needs `command` to be a list (the program, then its arguments), \
                 not a single string"
                    .to_string(),
            ),
        ),
        _ => (None, Ok(Vec::new())),
    };
    EntrySpec {
        command,
        args,
        has_url,
        not_an_entry,
    }
}

fn entry_spec_toml(entry: &toml_edit::Item) -> EntrySpec {
    let command = entry
        .get("command")
        .and_then(|c| c.as_str())
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(String::from);
    let args = match entry.get("args") {
        None => Ok(Vec::new()),
        Some(item) => match item.as_array() {
            Some(items) => items
                .iter()
                .map(|value| value.as_str().map(String::from))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| "`args` has to be a list of strings".to_string()),
            None => Err(format!(
                "`args` is {}, but it has to be a list of strings",
                item.type_name()
            )),
        },
    };
    EntrySpec {
        command,
        args,
        has_url: entry
            .get("url")
            .and_then(|u| u.as_str())
            .is_some_and(|u| !u.trim().is_empty()),
        not_an_entry: (!entry.is_table_like()).then_some("not a table"),
    }
}

/// The file extensions that make a program launchable by name on Windows, in
/// the order they are tried. ONE list for both directions: the plugin installer
/// builds the file names it looks for from it, and [`command_stem`] strips
/// exactly these, so a name one side treats as a launcher extension is never a
/// plain name to the other. `.com` is deliberately not here: nothing in
/// Wenlan's toolchain ships as one, and the installer does not look for it.
pub(crate) const WINDOWS_LAUNCHER_EXTENSIONS: [&str; 3] = ["exe", "cmd", "bat"];

/// The program name a command launches, lower-cased and without a launcher
/// extension: `C:\Program Files\nodejs\npx.cmd` and `/usr/bin/npx` are both
/// `npx`.
fn command_stem(command: &str) -> String {
    let file = command.rsplit(['/', '\\']).next().unwrap_or(command);
    let lower = file.to_lowercase();
    for ext in WINDOWS_LAUNCHER_EXTENSIONS {
        if let Some(stem) = lower
            .strip_suffix(ext)
            .and_then(|without| without.strip_suffix('.'))
        {
            return stem.to_string();
        }
    }
    lower
}

/// What Wenlan's own package is called on npm, now and before the rename.
fn is_wenlan_mcp_package(arg: &str) -> bool {
    ["wenlan-mcp", "origin-mcp"]
        .iter()
        .any(|name| arg == *name || arg.strip_prefix(*name).is_some_and(|r| r.starts_with('@')))
}

enum CommandCheck {
    Runs,
    Problem(RepairReason, String),
    /// The command's path could not be looked at. NOT a problem with it.
    Unreadable(String),
}

/// Whether `command` would start, resolved the way a client would find it but
/// without starting it: an absolute path is probed as a file; a bare name is
/// looked up on `PATH` and in the usual install folders (no shell); anything
/// else (a relative path, `~/...`) is refused, because a client starts its
/// servers from a folder this app cannot know and does not expand `~`.
fn check_command(command: &str, probes: &DetectProbes) -> CommandCheck {
    let path = Path::new(command);
    if path.is_absolute() {
        return match (probes.probe_command)(path) {
            CandidateProbe::File => CommandCheck::Runs,
            CandidateProbe::Absent => CommandCheck::Problem(
                RepairReason::CommandNotFound,
                format!("{command} does not exist."),
            ),
            CandidateProbe::NotAFile => CommandCheck::Problem(
                RepairReason::CommandNotRunnable,
                format!("{command} is a folder, not a program."),
            ),
            CandidateProbe::NotExecutable { reason } => CommandCheck::Problem(
                RepairReason::CommandNotRunnable,
                format!("{command} cannot be run: {reason}."),
            ),
            CandidateProbe::Unreadable { error } => {
                CommandCheck::Unreadable(format!("could not look at {command}: {error}"))
            }
        };
    }
    if command.contains(['/', '\\']) {
        return CommandCheck::Problem(
            RepairReason::CommandNotFound,
            format!(
                "{command} is a relative path. MCP clients start servers from a folder Wenlan \
                 cannot know, and do not expand `~`, so it has to be the full path."
            ),
        );
    }
    // A bare name. The resolver adds the launcher extensions on Windows, so
    // `npx.cmd` is asked for as `npx`.
    let name = if probes.host == HostKind::Windows {
        command_stem(command)
    } else {
        command.to_string()
    };
    if (probes.which)(&name).is_some() {
        CommandCheck::Runs
    } else {
        CommandCheck::Problem(
            RepairReason::CommandNotFound,
            format!("`{command}` was not found on PATH or in the usual install folders."),
        )
    }
}

fn entry_health_of(spec: EntrySpec, probes: &DetectProbes) -> EntryHealth {
    let needs_repair = |reason, detail: String| EntryHealth::NeedsRepair { reason, detail };
    let Some(command) = spec.command else {
        if spec.has_url {
            // A remote (`url`) entry has no command to check, and is not ours
            // to second-guess.
            return EntryHealth::Healthy;
        }
        return needs_repair(
            RepairReason::CommandMissing,
            match spec.not_an_entry {
                Some(kind) => format!("The Wenlan entry is {kind}, not an entry with a command."),
                None => "The Wenlan entry has no command to run.".to_string(),
            },
        );
    };
    let args = match spec.args {
        Ok(args) => args,
        Err(why) => return needs_repair(RepairReason::ArgsInvalid, format!("{why}.")),
    };
    match check_command(&command, probes) {
        CommandCheck::Runs => {}
        CommandCheck::Problem(reason, detail) => return needs_repair(reason, detail),
        CommandCheck::Unreadable(error) => return EntryHealth::Unreadable { error },
    }
    // `npx` is only Wenlan's launcher if it is asked for Wenlan's package.
    if command_stem(&command) == "npx" && !args.iter().any(|arg| is_wenlan_mcp_package(arg)) {
        return needs_repair(
            RepairReason::ArgsInvalid,
            "The entry runs npx without asking for wenlan-mcp (expected `-y wenlan-mcp`)."
                .to_string(),
        );
    }
    EntryHealth::Healthy
}

/// Whether the raw Wenlan entry in a client's config would launch.
///
/// Judged on the `wenlan` entry, or on the legacy `origin` entry when that is
/// all there is. The same single read of the config file feeds this and
/// `has_raw_entry`, so the two cannot describe different instants. An
/// unreadable or unparseable file is `unreadable`, never `no_entry`, and never
/// a reason to offer a repair that would rewrite a file nobody could read.
fn entry_health_reading(
    client_type: &str,
    config: &ConfigRead,
    probes: &DetectProbes,
) -> EntryHealth {
    let body = match config {
        ConfigRead::Absent => return EntryHealth::NoEntry,
        ConfigRead::Unreadable(error) => {
            return EntryHealth::Unreadable {
                error: error.clone(),
            }
        }
        ConfigRead::Contents(body) => body,
    };
    if client_type == "codex_cli" {
        let doc = match parse_toml(body) {
            Ok(doc) => doc,
            Err(error) => return EntryHealth::Unreadable { error },
        };
        match doc.get("mcp_servers").and_then(|servers| {
            servers
                .get(MCP_SERVER_KEY)
                .or(servers.get(LEGACY_MCP_SERVER_KEY))
        }) {
            Some(entry) => entry_health_of(entry_spec_toml(entry), probes),
            None => EntryHealth::NoEntry,
        }
    } else {
        let value = match parse_json(body) {
            Ok(value) => value,
            Err(error) => return EntryHealth::Unreadable { error },
        };
        let dialect = JsonDialect::for_client(client_type);
        match value.get(dialect.servers_key()).and_then(|servers| {
            servers
                .get(MCP_SERVER_KEY)
                .or(servers.get(LEGACY_MCP_SERVER_KEY))
        }) {
            Some(entry) => entry_health_of(entry_spec_for(dialect, entry), probes),
            None => EntryHealth::NoEntry,
        }
    }
}

/// Detect installed MCP-compatible tools and whether Wenlan is already
/// configured — in three values per fact, never two. See [`Reading`].
pub fn detect_mcp_clients() -> Vec<McpClient> {
    let local_data_dir = dirs::data_local_dir();
    let program_files = program_files_dirs();
    let (home, config_dir) = crate::identity_paths::ai_tool_dirs();
    detect_mcp_clients_with(
        home.as_deref(),
        config_dir.as_deref(),
        &DetectProbes {
            host: HostKind::current(),
            local_data_dir: local_data_dir.as_deref(),
            program_files: &program_files,
            exists: &path_exists_reading,
            list_dir: &list_child_paths,
            which: &crate::plugin_install::resolve_binary_no_shell,
            probe_command: &probe_candidate,
        },
    )
}

/// The body, with the two directory lookups and the existence probe handed in.
/// The seam exists because `dirs::home_dir()` on Windows resolves the known
/// folder and IGNORES `$HOME`, so "the platform would not report a home
/// directory" is unreachable through the environment there.
///
/// HERMETIC: it looks at the macOS candidate set and finds no CLI on `PATH`,
/// whatever machine it runs on, so a test answers for the temp directory it
/// built and not for the developer's Cursor or Codex. The real machine is
/// [`detect_mcp_clients`]; another OS is [`detect_mcp_clients_with`].
#[cfg(test)]
pub(crate) fn detect_mcp_clients_from(
    home: Option<&Path>,
    config_dir: Option<&Path>,
    exists: impl Fn(&Path) -> Reading,
) -> Vec<McpClient> {
    detect_mcp_clients_with(
        home,
        config_dir,
        &DetectProbes {
            host: HostKind::MacOs,
            local_data_dir: None,
            program_files: &[],
            exists: &exists,
            list_dir: &|_| Vec::new(),
            which: &|_| None,
            probe_command: &probe_candidate,
        },
    )
}

pub(crate) fn detect_mcp_clients_with(
    home: Option<&Path>,
    config_dir: Option<&Path>,
    probes: &DetectProbes,
) -> Vec<McpClient> {
    let clients = [
        ("Cursor", "cursor"),
        ("Claude Code", "claude_code"),
        ("Claude Desktop", "claude_desktop"),
        ("Gemini CLI", "gemini_cli"),
        ("Codex CLI", "codex_cli"),
        ("Muse Code", "muse_code"),
        ("Antigravity", "antigravity"),
        ("GitHub Copilot", "copilot_cli"),
        ("VS Code", "vscode"),
        ("Windsurf", "windsurf"),
        ("Kiro", "kiro"),
        ("OpenCode", "opencode"),
    ];

    clients
        .iter()
        // `map`, not `filter_map`: a row that says "could not read" is the only
        // honest output for a client whose config path could not be built, and
        // it can only exist if the row exists.
        .map(|(name, client_type)| {
            let path = client_config_path_for(client_type, home, config_dir);
            let (config_path, config) = match &path {
                ClientConfigPath::Known(path) => {
                    // ONE read answers both "is the file there" and "what does
                    // it say" — see `ConfigRead`.
                    (
                        Some(path.to_string_lossy().to_string()),
                        read_config_for(JsonDialect::for_client(client_type), path),
                    )
                }
                ClientConfigPath::UnknownClient => (
                    None,
                    ConfigRead::Unreadable(format!(
                        "{client_type} is not a client type this app knows"
                    )),
                ),
                ClientConfigPath::Undetermined(why) => (None, ConfigRead::Unreadable(why.clone())),
            };

            let has_raw_entry = raw_entry_reading(client_type, &config);
            let has_raw_duplicate = raw_duplicate_reading(client_type, &config);
            let has_plugin = client_plugin_enabled_for(client_type, home, config_dir);
            let entry_health = entry_health_reading(client_type, &config, probes);

            // Two kinds of evidence that the client is here. The program (an app
            // bundle, an .exe, an AppImage, its CLI) is the real thing; the
            // config file and the client's own home folder survive an
            // uninstall. `detected` is both, as before — the install state is
            // what tells them apart. A config file alone used to be reported as
            // an installed client.
            let program = program_reading(client_type, home, probes);
            let leftovers =
                config
                    .present()
                    .or(client_home_dir_reading(client_type, home, probes.exists));
            let detected = program.clone().or(leftovers.clone());
            let install_state = install_state_of(&program, &leftovers);
            let has_skills =
                crate::skills_install::skills_reading(client_type, home, probes.exists);

            McpClient {
                name: name.to_string(),
                client_type: client_type.to_string(),
                config_path,
                detected,
                already_configured: has_raw_entry.clone().or(has_plugin.clone()),
                has_raw_entry,
                has_raw_duplicate,
                has_plugin,
                install_state,
                entry_health,
                has_skills,
            }
        })
        .collect()
}

/// The backend release this app ships against. `app/Cargo.toml`'s version is
/// lockstepped to the daemon release (Milestone B phase 4c: the app builds
/// from the same tagged commit as `wenlan-server`/`wenlan-mcp`), so the
/// crate's own compile-time version can never disagree with the daemon this
/// build was tested against — no separate pin file needed.
const BACKEND_VERSION_PIN: &str = env!("CARGO_PKG_VERSION");

/// `wenlan-mcp@^<pinned version>`, e.g. `wenlan-mcp@^0.12.0`. Falls back to the
/// bare package name only if the pinned version string is unparseable — an
/// unpinned `npx` can silently pull a backend the app was never tested against.
fn pinned_wenlan_mcp_package(pin_file: &str) -> String {
    let version = pin_file
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .trim_start_matches('v');
    if version.is_empty() || !version.starts_with(|c: char| c.is_ascii_digit()) {
        return "wenlan-mcp".to_string();
    }
    format!("wenlan-mcp@^{version}")
}

/// Give a candidate binary path `host`'s executable suffix — `wenlan-mcp.exe`
/// on Windows. Same idiom as `lifecycle::service_cli_path_for_app_exe`: a
/// suffix-less candidate never matches a real Windows install, so the bundled
/// binary next to the app exe is skipped and the `npx` fallback wins on a
/// machine that already has it. Takes the host as a value, not `cfg!`, so the
/// Windows shape is testable from any machine.
fn with_exe_suffix(mut bin: PathBuf, host: HostKind) -> PathBuf {
    if host == HostKind::Windows {
        bin.set_extension("exe");
    }
    bin
}

/// Each `wenlan-mcp` candidate paired with where it came from, most-specific
/// first — the single source of truth the plain path list and `wire_state`'s
/// candidate trail both derive from, so the two can never disagree about what
/// was tried. Mirrors the plugin's own `wenlan-mcp-runner.sh` resolution order.
///
/// Deliberately does *not* probe a cargo target dir: a target dir is a build
/// output, not an install location, and an entry pointing into one dies on the
/// next `cargo clean`.
pub(crate) fn wenlan_mcp_candidate_sources(
    home: Option<&Path>,
    dev_bin: Option<&str>,
    exe_dir: Option<&Path>,
) -> Vec<(PathBuf, &'static str)> {
    wenlan_mcp_candidate_sources_for(HostKind::current(), home, dev_bin, exe_dir)
}

/// [`wenlan_mcp_candidate_sources`] for a chosen host.
pub(crate) fn wenlan_mcp_candidate_sources_for(
    host: HostKind,
    home: Option<&Path>,
    dev_bin: Option<&str>,
    exe_dir: Option<&Path>,
) -> Vec<(PathBuf, &'static str)> {
    let mut candidates = Vec::new();
    if let Some(dev_bin) = dev_bin.filter(|p| !p.trim().is_empty()) {
        candidates.push((PathBuf::from(dev_bin), "WENLAN_MCP_DEV_BIN"));
    }
    if let Some(home) = home {
        candidates.push((
            with_exe_suffix(home.join(".wenlan/bin/wenlan-mcp"), host),
            "installed",
        ));
    }
    if let Some(exe_dir) = exe_dir {
        candidates.push((with_exe_suffix(exe_dir.join("wenlan-mcp"), host), "bundled"));
    }
    if let Some(home) = home {
        candidates.push((
            with_exe_suffix(home.join(".cargo/bin/wenlan-mcp"), host),
            "cargo",
        ));
    }
    candidates
}

#[cfg(test)]
fn wenlan_mcp_candidates(
    home: Option<&Path>,
    dev_bin: Option<&str>,
    exe_dir: Option<&Path>,
) -> Vec<PathBuf> {
    wenlan_mcp_candidate_sources(home, dev_bin, exe_dir)
        .into_iter()
        .map(|(path, _source)| path)
        .collect()
}

/// Env var a test sets to point the resolver at a fixture install tree.
/// Mirrors `lifecycle::home_dir`'s `#[cfg(test)]` HOME hook; without it a test
/// can only assert whatever the host it runs on happens to hold.
#[cfg(test)]
pub(crate) const MCP_RESOLVER_HOME_ENV: &str = "WENLAN_TEST_MCP_HOME";

/// Home directory the `installed` and `cargo` candidates hang off.
fn resolver_home_dir() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(home) = std::env::var_os(MCP_RESOLVER_HOME_ENV) {
        return Some(PathBuf::from(home));
    }
    dirs::home_dir()
}

/// What one candidate path is, as far as the filesystem would say.
///
/// `Path::exists()` gets both failure modes wrong: it is false when `metadata`
/// fails for *any* reason (a locked file, a denied ACL, a disconnected network
/// path all read as "not installed"), and true for anything at the path, so a
/// *directory* named `wenlan-mcp.exe` reads as an executable.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CandidateProbe {
    /// A regular file that nothing here could rule OUT as a program. The only
    /// state that may be used as the command -- and it is a "not disqualified",
    /// not a "certified runnable". See [`file_probe`] for exactly what was and
    /// was not established, per platform.
    File,
    /// Something is there and it is not a regular file — a directory, most
    /// plausibly. Measured, and measured unusable.
    NotAFile,
    /// A regular file that is measured NOT to be runnable: empty, or (on Unix)
    /// carrying no execute bit. Measured, and measured unusable — the same
    /// standing as `NotAFile`, and deliberately NOT `Unreadable`: nothing
    /// failed here, the answer is just no.
    NotExecutable { reason: String },
    /// Measured absent.
    Absent,
    /// Could not look. NOT an absence.
    Unreadable { error: String },
}

/// Probe one candidate. `NotFound` is the only error that is an absence;
/// every other error is a failed look and says so.
pub fn probe_candidate(path: &Path) -> CandidateProbe {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => file_probe(&metadata),
        Ok(_) => CandidateProbe::NotAFile,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => CandidateProbe::Absent,
        Err(e) => CandidateProbe::Unreadable {
            error: e.to_string(),
        },
    }
}

/// `metadata.is_file()` is a FILE witness, not an EXECUTABLE witness, and the
/// resolver writes the winner into a user's client config as a command the OS
/// then has to run.
///
/// WHAT THIS ESTABLISHES, PER PLATFORM — the honest answer differs, and
/// inventing a uniform one would be a check that cannot fail:
///
/// * Everywhere: a zero-length file is not a program. No OS on any platform
///   executes an empty file — there is no interpreter line, no PE header, no
///   ELF header, nothing to load. This is a real check with a real failure
///   mode, and it is the one that catches the fixture problem below.
/// * Unix: the execute bit. `metadata.permissions().mode() & 0o111 == 0` means
///   the kernel will refuse `execve` with `EACCES` no matter what the bytes
///   are. A real, decidable negative.
/// * WINDOWS: THERE IS NO EXECUTE BIT, and nothing here pretends otherwise.
///   NTFS ACLs carry no "executable" attribute that `std::fs::Metadata`
///   exposes, and executability on Windows is decided by the loader reading the
///   image header at `CreateProcess` time. A non-empty regular file therefore
///   probes as `File` on Windows, and that is the strongest claim available
///   without opening and parsing the file.
///
/// THE RESIDUAL: NEITHER CHECK CERTIFIES A RUNNABLE IMAGE, on either platform.
/// `File` means "nothing here could rule it out". A non-empty but corrupt
/// `.exe` probes as `File` on Windows (nothing reads the PE header), and on
/// Unix the execute bit is a permission, not a format — a corrupt ELF or a
/// `#!` line naming a missing interpreter probes as `File` at mode 0755. Only
/// the execute bit's NEGATIVE is relied on here.
fn file_probe(metadata: &std::fs::Metadata) -> CandidateProbe {
    if metadata.len() == 0 {
        return CandidateProbe::NotExecutable {
            reason: "the file is empty (0 bytes), so it is not a program".to_string(),
        };
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = metadata.permissions().mode();
        if mode & 0o111 == 0 {
            return CandidateProbe::NotExecutable {
                reason: format!("mode {:04o} has no execute bit set", mode & 0o7777),
            };
        }
    }
    CandidateProbe::File
}

/// One input the candidate paths hang off, in three values. A failed
/// measurement upstream of a hardened one is still a failed measurement: an
/// input flattened to `None` builds no candidate, so it never appears in the
/// trail and a search that looked at nothing could end in `NoneInstalled`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RootInput<T> {
    /// Measured: here it is.
    Known(T),
    /// Measured: there is no such input. `WENLAN_MCP_DEV_BIN` unset is a real
    /// negative -- the developer override simply is not in play, and no
    /// candidate is missing because of it.
    NotSet,
    /// Could NOT be determined. Every candidate hanging off it was never
    /// constructed, so the search did not cover the paths it claims to.
    Undetermined(String),
}

impl<T> RootInput<T> {
    fn known(&self) -> Option<&T> {
        match self {
            RootInput::Known(value) => Some(value),
            RootInput::NotSet | RootInput::Undetermined(_) => None,
        }
    }
}

/// An input that could not be determined, named the way a user would recognise
/// it. Deliberately not a path: there is no path, and that is the point.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UndeterminedInput {
    /// What could not be determined.
    pub input: String,
    /// The candidate sources that were therefore never probed.
    pub blocked: String,
    pub error: String,
}

/// Everything a search failed to turn into an answer it may act on, in one
/// value. An empty one is the precondition for `NoneInstalled` -- i.e. for
/// writing anything.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Unmeasured {
    /// Candidate paths that produced no usable answer: one the OS would not let
    /// this process look at, or the winner it DID look at and cannot NAME,
    /// because a path that is not valid Unicode cannot be written into a JSON or
    /// TOML config without silently naming a different file.
    pub unreadable: Vec<(PathBuf, String)>,
    /// Inputs that could not be determined, so the candidates hanging off them
    /// were never built and never looked at at all.
    pub undetermined: Vec<UndeterminedInput>,
}

/// Why a found binary is still not writable as a command. Lives beside
/// `Unmeasured` because it is one of the two things `unreadable` can hold.
const UNREPRESENTABLE_BINARY_PATH: &str =
    "a usable wenlan-mcp binary is here, but its path is not valid Unicode, so it cannot be \
     written into a config file without naming a different file";

impl Unmeasured {
    pub(crate) fn is_empty(&self) -> bool {
        self.unreadable.is_empty() && self.undetermined.is_empty()
    }
}

/// The three inputs the candidate list is built from, each of which can fail
/// to be determined rather than merely be absent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolverInputs {
    pub home: RootInput<PathBuf>,
    pub dev_bin: RootInput<String>,
    pub exe_dir: RootInput<PathBuf>,
}

impl ResolverInputs {
    /// Read the real environment.
    fn from_env() -> Self {
        Self::from_reads(
            resolver_home_dir(),
            std::env::var("WENLAN_MCP_DEV_BIN"),
            std::env::current_exe(),
        )
    }

    /// The classification, split from the reads so a test can hand it a
    /// failure directly — `current_exe()` failing cannot be staged otherwise.
    fn from_reads(
        home: Option<PathBuf>,
        dev_bin: Result<String, std::env::VarError>,
        exe: std::io::Result<PathBuf>,
    ) -> Self {
        Self {
            // `dirs::home_dir()` answering `None` is the platform declining to
            // say. Two candidates (`installed`, `cargo`) hang off it.
            home: match home {
                Some(home) => RootInput::Known(home),
                None => RootInput::Undetermined(
                    "the platform would not report a home directory".to_string(),
                ),
            },
            // `env::var` returns `Err` for BOTH "unset" and "not valid
            // Unicode". ONLY THE FIRST IS AN ABSENCE.
            dev_bin: match dev_bin {
                Ok(value) => RootInput::Known(value),
                Err(std::env::VarError::NotPresent) => RootInput::NotSet,
                Err(e) => RootInput::Undetermined(e.to_string()),
            },
            exe_dir: match exe {
                Ok(exe) => match exe.parent() {
                    Some(dir) => RootInput::Known(dir.to_path_buf()),
                    None => RootInput::Undetermined(
                        "the running executable has no parent directory".to_string(),
                    ),
                },
                Err(e) => RootInput::Undetermined(format!(
                    "the running executable's own path could not be read: {e}"
                )),
            },
        }
    }

    /// Only the inputs that were actually determined reach the path builder.
    fn as_paths(&self) -> (Option<&Path>, Option<&str>, Option<&Path>) {
        (
            self.home.known().map(PathBuf::as_path),
            self.dev_bin.known().map(String::as_str),
            self.exe_dir.known().map(PathBuf::as_path),
        )
    }

    fn undetermined(&self) -> Vec<UndeterminedInput> {
        let mut out = Vec::new();
        let mut push = |input: &str, blocked: &str, error: &String| {
            log::warn!(
                "[mcp] {input} could not be determined ({error}); the {blocked} candidate(s) were \
                 never probed, so this search cannot report that nothing is installed"
            );
            out.push(UndeterminedInput {
                input: input.to_string(),
                blocked: blocked.to_string(),
                error: error.clone(),
            });
        };
        if let RootInput::Undetermined(error) = &self.dev_bin {
            push("WENLAN_MCP_DEV_BIN", "WENLAN_MCP_DEV_BIN", error);
        }
        if let RootInput::Undetermined(error) = &self.home {
            push("the home directory", "installed and cargo", error);
        }
        if let RootInput::Undetermined(error) = &self.exe_dir {
            push("the application's own directory", "bundled", error);
        }
        out
    }
}

/// How the `wenlan-mcp` binary search ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum McpBinaryResolution {
    /// A candidate measured to be a regular file, in probe order — plus any
    /// input that could NOT be determined along the way. A find does not
    /// retroactively make an unread input read, and an undetermined input
    /// builds no candidate path, so it has no trail row to be missing from.
    Found {
        path: PathBuf,
        undetermined: Vec<UndeterminedInput>,
    },
    /// EVERY candidate was constructed AND measured, and none is a usable
    /// file. `npx` is the right answer, and it is an answer, not a fallback
    /// from ignorance.
    NoneInstalled,
    /// No candidate is a usable file, and something was not measured: a path
    /// that could not be looked at, or an input that could not be determined
    /// so its paths were never built. "No binary is installed" was never
    /// established.
    Unresolved(Unmeasured),
}

/// One candidate as the resolver actually saw it. Carried out of the resolver
/// so `wire_state` can put THE DECISION'S OWN probe results on the diagnostics
/// wire instead of re-probing afterwards: a second probe pass is a second
/// instant, and a permission that changed in between produces a trail that
/// contradicts the command beside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProbedCandidate {
    pub path: PathBuf,
    pub source: &'static str,
    pub state: CandidateProbe,
}

/// A resolution plus the exact probe readings it was decided from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct McpResolutionTrail {
    pub resolution: McpBinaryResolution,
    pub trail: Vec<ProbedCandidate>,
}

/// RANKING for the resolver below, decided and stated: a candidate that could
/// not be looked at does NOT stop the search — a readable bundled binary
/// outranks an unreadable dev override, because a measured file beats an
/// unmeasured anything. It does change the *shape* of an empty result: no file
/// plus at least one unreadable candidate is
/// [`McpBinaryResolution::Unresolved`], never `NoneInstalled`.
///
/// Every candidate is probed exactly ONCE and probing does not stop at the
/// winner, so there is one set of readings, the decision came from it, and the
/// diagnostics trail IS it.
///
/// The key two candidate SLOTS share when they name one filesystem object.
/// `canonicalize` resolves `.`/`..`, symlinks, and (on Windows) the real
/// on-disk casing, so those all collapse to one key. It is a look that can fail
/// — most often because the candidate simply is not there — and a failure falls
/// back to the literal path: two absent candidates then get two readings, which
/// cannot contradict each other since there is nothing there to disagree about.
///
/// THE RESIDUAL: a HARDLINK is still two keys. Two directory entries for one
/// inode are genuinely two paths and `canonicalize` returns each unchanged;
/// telling them apart needs a file identity `std` does not expose portably
/// (`std::os::windows::fs::MetadataExt::file_index` is unstable).
fn probe_key(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn resolve_wenlan_mcp_with_trail(
    inputs: &ResolverInputs,
    probe: impl Fn(&Path) -> CandidateProbe,
) -> McpResolutionTrail {
    let (home, dev_bin, exe_dir) = inputs.as_paths();
    // One reading per FILESYSTEM OBJECT, not one per vector entry: four SLOTS,
    // two of which can name the same file (`WENLAN_MCP_DEV_BIN` pointed at the
    // installed binary is the ordinary developer case). Every slot still
    // appears in the trail; they share the one reading.
    let mut seen: std::collections::HashMap<PathBuf, CandidateProbe> =
        std::collections::HashMap::new();
    let trail: Vec<ProbedCandidate> = wenlan_mcp_candidate_sources(home, dev_bin, exe_dir)
        .into_iter()
        .map(|(path, source)| {
            let key = probe_key(path.as_path());
            let state = match seen.get(key.as_path()) {
                Some(already) => already.clone(),
                None => {
                    let measured = probe(path.as_path());
                    seen.insert(key, measured.clone());
                    measured
                }
            };
            match &state {
                CandidateProbe::NotAFile => log::warn!(
                    "[mcp] {} exists but is not a regular file; it is not the wenlan-mcp binary \
                     and will not be written into any client config",
                    path.display()
                ),
                CandidateProbe::NotExecutable { reason } => log::warn!(
                    "[mcp] {} is a file but not a runnable one ({reason}); it will not be written \
                     into any client config",
                    path.display()
                ),
                CandidateProbe::Unreadable { error } => {
                    log::warn!("[mcp] could not look at {}: {error}", path.display())
                }
                CandidateProbe::File | CandidateProbe::Absent => {}
            }
            ProbedCandidate {
                path,
                source,
                state,
            }
        })
        .collect();

    let found = trail
        .iter()
        .find(|c| c.state == CandidateProbe::File)
        .map(|c| c.path.clone());
    // Computed unconditionally and BEFORE the outcome is known: "which inputs
    // could not be read" is a property of the search, not of an empty result,
    // so a hit anywhere must not short-circuit it (or its warnings) away.
    let undetermined = inputs.undetermined();
    let resolution = match found {
        Some(path) => McpBinaryResolution::Found { path, undetermined },
        None => {
            let unmeasured = Unmeasured {
                unreadable: trail
                    .iter()
                    .filter_map(|c| match &c.state {
                        CandidateProbe::Unreadable { error } => {
                            Some((c.path.clone(), error.clone()))
                        }
                        _ => None,
                    })
                    .collect(),
                undetermined,
            };
            if unmeasured.is_empty() {
                McpBinaryResolution::NoneInstalled
            } else {
                McpBinaryResolution::Unresolved(unmeasured)
            }
        }
    };
    McpResolutionTrail { resolution, trail }
}

/// Inputs that were all determined -- the shape the older tests state their
/// fixtures in. `None` here means `NotSet`, a MEASURED absence; a test that
/// wants an undetermined input builds [`ResolverInputs`] itself.
#[cfg(test)]
pub(crate) fn determined_inputs(
    home: Option<&Path>,
    dev_bin: Option<&str>,
    exe_dir: Option<&Path>,
) -> ResolverInputs {
    fn known<T>(value: Option<T>) -> RootInput<T> {
        match value {
            Some(value) => RootInput::Known(value),
            None => RootInput::NotSet,
        }
    }
    ResolverInputs {
        home: known(home.map(Path::to_path_buf)),
        dev_bin: known(dev_bin.map(str::to_string)),
        exe_dir: known(exe_dir.map(Path::to_path_buf)),
    }
}

/// The resolution alone, for callers that do not need the trail.
#[cfg(test)]
fn resolve_wenlan_mcp(
    home: Option<&Path>,
    dev_bin: Option<&str>,
    exe_dir: Option<&Path>,
    probe: impl Fn(&Path) -> CandidateProbe,
) -> McpBinaryResolution {
    resolve_wenlan_mcp_with_trail(&determined_inputs(home, dev_bin, exe_dir), probe).resolution
}

fn find_wenlan_mcp_binary_with_trail() -> McpResolutionTrail {
    resolve_wenlan_mcp_with_trail(&ResolverInputs::from_env(), probe_candidate)
}

/// What to do with a client config, given how the binary search ended. The
/// third value exists so a call site can say "leave the user's config alone".
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum McpEntryDecision {
    /// Write this entry — and, separately, the inputs that could not be
    /// determined while deciding it.
    ///
    /// `undetermined` is NOT a reason to withhold the entry: a candidate
    /// measured to be a usable file is still the right command, and "write
    /// nothing rather than a guess" applies to an EMPTY search. It is carried
    /// because a dropped failed measurement is indistinguishable from a
    /// negative one.
    Write {
        entry: WenlanMcpEntry,
        undetermined: Vec<UndeterminedInput>,
    },
    /// Write NOTHING. No candidate was confirmed usable and at least one could
    /// not be looked at, so no measured absence exists to justify any entry —
    /// including `npx`. Whatever is in the user's config stays there.
    PreserveExisting { unmeasured: Unmeasured },
}

/// The message the UI shows and the writers fail with. Names every path that
/// could not be read, and says plainly that nothing was changed.
pub(crate) fn unresolved_message(unmeasured: &Unmeasured) -> String {
    let mut reasons: Vec<String> = unmeasured
        .unreadable
        .iter()
        .map(|(path, error)| format!("{} ({error})", path.display()))
        .collect();
    reasons.extend(unmeasured.undetermined.iter().map(|u| {
        format!(
            "{} could not be determined ({}), so the {} candidate(s) were never checked",
            u.input, u.error, u.blocked
        )
    }));
    format!(
        "Could not determine the wenlan-mcp binary: {}. Nothing was written — your existing MCP \
         configuration is unchanged.",
        reasons.join("; ")
    )
}

/// The MCP config entry Wenlan writes into client config files: an installed
/// binary when one was measured, a version-pinned `npx` when nothing is
/// installed, and NOTHING AT ALL when the search could not be completed.
///
/// `Unresolved` maps to `PreserveExisting` rather than to `npx`: a real
/// `wenlan-mcp.exe` that is momentarily unstatable (an ACL, an antivirus lock,
/// a disconnected network path) would otherwise overwrite a working local
/// command with an entry that needs Node and a network. Writing nothing cannot
/// break a working config; writing a guess can.
fn wenlan_mcp_entry_for(resolution: McpBinaryResolution, npm_package: &str) -> McpEntryDecision {
    match resolution {
        // `to_str()`, not `to_string_lossy()`: a usable binary under a path that
        // is not valid Unicode would otherwise be written into the config with
        // the unrepresentable bytes replaced by U+FFFD — a DIFFERENT path,
        // naming a file that does not exist. `to_string_lossy` is still used for
        // the MESSAGE, where an approximate spelling is what a human needs.
        McpBinaryResolution::Found { path, undetermined } => match path.to_str() {
            Some(command) => McpEntryDecision::Write {
                entry: WenlanMcpEntry {
                    command: command.to_string(),
                    args: Vec::new(),
                },
                // A find does not erase an input that could not be read.
                undetermined,
            },
            None => {
                log::error!(
                    "[mcp] {} is a usable wenlan-mcp binary, but its path is not valid Unicode; \
                     writing it into a config file would name a different, nonexistent file, so \
                     nothing will be written",
                    path.display()
                );
                McpEntryDecision::PreserveExisting {
                    unmeasured: Unmeasured {
                        unreadable: vec![(path, UNREPRESENTABLE_BINARY_PATH.to_string())],
                        undetermined,
                    },
                }
            }
        },
        // `NoneInstalled` is only reachable when `Unmeasured::is_empty()`, so
        // by construction there is nothing undetermined to carry here.
        McpBinaryResolution::NoneInstalled => McpEntryDecision::Write {
            entry: WenlanMcpEntry {
                command: "npx".to_string(),
                args: vec!["-y".to_string(), npm_package.to_string()],
            },
            undetermined: Vec::new(),
        },
        McpBinaryResolution::Unresolved(unmeasured) => {
            for (path, error) in &unmeasured.unreadable {
                log::error!(
                    "[mcp] refusing to write any wenlan entry while {} could not be read \
                     ({error}); this is NOT a measured absence, so `npx {npm_package}` would be a \
                     guess overwriting a possibly-working config",
                    path.display()
                );
            }
            McpEntryDecision::PreserveExisting { unmeasured }
        }
    }
}

/// The decision against an explicit candidate tree and probe, so `wire_state`'s
/// tests can exercise the one-pass property without the ambient machine
/// deciding the outcome.
#[cfg(test)]
pub(crate) fn wenlan_mcp_decision_for(
    home: Option<&Path>,
    dev_bin: Option<&str>,
    exe_dir: Option<&Path>,
    probe: impl Fn(&Path) -> CandidateProbe,
    npm_package: &str,
) -> (McpEntryDecision, Vec<ProbedCandidate>) {
    wenlan_mcp_decision_from(
        &determined_inputs(home, dev_bin, exe_dir),
        probe,
        npm_package,
    )
}

/// The same, from inputs a test built itself -- the only way to stage an
/// UNDETERMINED input.
#[cfg(test)]
pub(crate) fn wenlan_mcp_decision_from(
    inputs: &ResolverInputs,
    probe: impl Fn(&Path) -> CandidateProbe,
    npm_package: &str,
) -> (McpEntryDecision, Vec<ProbedCandidate>) {
    let McpResolutionTrail { resolution, trail } = resolve_wenlan_mcp_with_trail(inputs, probe);
    (wenlan_mcp_entry_for(resolution, npm_package), trail)
}

/// One probe pass over the real machine: what to write, and the readings that
/// decided it. Every caller that needs either takes both from here, so the
/// decision and the trail describing it can never come from different instants.
pub(crate) fn wenlan_mcp_decision() -> (McpEntryDecision, Vec<ProbedCandidate>) {
    let McpResolutionTrail { resolution, trail } = find_wenlan_mcp_binary_with_trail();
    let decision =
        wenlan_mcp_entry_for(resolution, &pinned_wenlan_mcp_package(BACKEND_VERSION_PIN));
    (decision, trail)
}

/// The entry that would be written, together with every input that could not
/// be determined while deciding it. Both halves cross the public boundary: a
/// vector that reaches it and is dropped at it never travelled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WenlanMcpEntryReport {
    pub entry: WenlanMcpEntry,
    /// Inputs that could not be determined. EMPTY is itself a measurement:
    /// every input was read. Non-empty means this command was chosen by a
    /// search that did not cover the paths it looks like it covered.
    pub undetermined: Vec<UndeterminedInput>,
}

/// The entry that would be written, or an error naming what could not be read.
///
/// The `Err` arm is the only thing a caller can do with `PreserveExisting`
/// besides make no change: there is no entry to report because none was
/// established.
pub fn wenlan_mcp_entry() -> Result<WenlanMcpEntryReport, AppError> {
    match wenlan_mcp_decision().0 {
        McpEntryDecision::Write {
            entry,
            undetermined,
        } => Ok(WenlanMcpEntryReport {
            entry,
            undetermined,
        }),
        McpEntryDecision::PreserveExisting { unmeasured } => {
            Err(AppError::Generic(unresolved_message(&unmeasured)))
        }
    }
}

/// The name of a JSON value's type, for a schema-error message a user can act
/// on ("`mcpServers` is a list" beats "expected object").
fn json_type_name(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "a true/false value",
        serde_json::Value::Number(_) => "a number",
        serde_json::Value::String(_) => "a string",
        serde_json::Value::Array(_) => "a list",
        serde_json::Value::Object(_) => "an object",
    }
}

/// The shapes [`write_wenlan_entry_with`] is able to write into, checked
/// BEFORE anything is backed up or written.
///
/// `root["mcpServers"][MCP_SERVER_KEY] = ..` is `serde_json`'s `IndexMut`,
/// which PANICS — not errors — when the thing being indexed is neither an
/// object nor null, and a panic inside a Tauri command is not an error the UI
/// can show. `{"mcpServers": []}` and any valid non-object top level (`[]`,
/// `"x"`, `3`) reach it. An unexpected shape is an ordinary `Err` raised
/// before the backup, so the user's file is untouched.
fn check_json_config_shape(
    config_path: &std::path::Path,
    root: &serde_json::Value,
    dialect: JsonDialect,
) -> Result<(), AppError> {
    let unchanged = "Nothing was written — your existing MCP configuration is unchanged.";
    if !root.is_object() {
        return Err(AppError::Generic(format!(
            "Unexpected shape in {}: the top level of the file is {}, but an MCP client config \
             has to be an object. {unchanged}",
            config_path.display(),
            json_type_name(root)
        )));
    }
    let key = dialect.servers_key();
    match root.get(key) {
        // Absent, or present-and-null: both are places a fresh `mcpServers`
        // object can be put without destroying anything.
        None | Some(serde_json::Value::Null) => Ok(()),
        Some(servers) if servers.is_object() => Ok(()),
        Some(servers) => Err(AppError::Generic(format!(
            "Unexpected shape in {}: `{key}` is {}, but it has to be an object for Wenlan to \
             add its entry to it. {unchanged}",
            config_path.display(),
            json_type_name(servers)
        ))),
    }
}

/// Back the config up from the bytes that were PARSED — and only while those
/// are still the bytes on disk.
///
/// Written FROM `contents` rather than by `fs::copy`, which re-reads the path
/// at a later instant and can put a concurrently-written malformed file on top
/// of the last good backup. And the file is re-read first: if it no longer
/// holds those bytes the update is abandoned rather than applied from a stale
/// parse.
///
/// THE RESIDUAL: this narrows the window, it does not close it — between this
/// check and the caller's `fs::write` the file can still change, and closing
/// that needs a lock this codebase does not take on another vendor's config
/// file. What is unconditional is that the backup is never bytes nothing
/// parsed.
fn back_up_parsed(
    config_path: &std::path::Path,
    contents: &str,
    backup_extension: &str,
) -> Result<(), AppError> {
    match read_config(config_path) {
        ConfigRead::Contents(now) if now == contents => {}
        ConfigRead::Contents(_) | ConfigRead::Absent => {
            return Err(AppError::Generic(format!(
                "{} changed while Wenlan was updating it, so this update was built from bytes that \
                 are no longer there. Nothing was written and no backup was taken — try again.",
                config_path.display()
            )))
        }
        ConfigRead::Unreadable(error) => {
            return Err(AppError::Generic(format!(
                "Could not re-read {} to confirm it had not changed ({error}). Nothing was written \
                 — your existing MCP configuration is unchanged.",
                config_path.display()
            )))
        }
    }
    std::fs::write(config_path.with_extension(backup_extension), contents)?;
    Ok(())
}

/// The message for a config file that is THERE but could not be read.
/// `exists()` is `false` for a metadata denial as well as for an absence, so a
/// file that permits writing but not stat-ing would take the NEW-FILE branch
/// and be TRUNCATED from a `json!({})` skeleton, with no backup.
fn unreadable_config_message(config_path: &std::path::Path, error: &str) -> AppError {
    AppError::Generic(format!(
        "Could not read {} ({error}), so Wenlan cannot tell what is in it. Nothing was written — \
         your existing MCP configuration is unchanged.",
        config_path.display()
    ))
}

/// The name a client's Wenlan entry reports to the daemon, so a search from
/// Cursor is recorded as Cursor's and not as the stdio default
/// (`claude-code`). The names are the tool families the app matches presence
/// rows against (`clientTypeFamily` in `src/lib/agents.ts`).
pub fn agent_name_for_client(client_type: &str) -> Option<&'static str> {
    match client_type {
        "claude_code" => Some("claude-code"),
        "codex_cli" => Some("codex"),
        "claude_desktop" => Some("claude-desktop"),
        "cursor" => Some("cursor"),
        "gemini_cli" => Some("gemini-cli"),
        "muse_code" => Some("muse-code"),
        "antigravity" => Some("antigravity"),
        "copilot_cli" => Some("copilot-cli"),
        "vscode" => Some("vscode"),
        "windsurf" => Some("windsurf"),
        "kiro" => Some("kiro"),
        "opencode" => Some("opencode"),
        _ => None,
    }
}

/// `decision` with `--agent-name <name>` added to the entry it writes.
fn with_agent_name(decision: McpEntryDecision, agent_name: Option<&str>) -> McpEntryDecision {
    match (decision, agent_name) {
        (
            McpEntryDecision::Write {
                mut entry,
                undetermined,
            },
            Some(name),
        ) => {
            entry
                .args
                .extend(["--agent-name".to_string(), name.to_string()]);
            McpEntryDecision::Write {
                entry,
                undetermined,
            }
        }
        (decision, _) => decision,
    }
}

/// The entry a decision says to write, or its `PreserveExisting` refusal as an
/// error. Shared by both writers, which must decide BEFORE touching anything:
/// an unresolvable binary leaves the file exactly as it was — no rewrite, and
/// no `.bak` either, since a backup of an unchanged file suggests a change
/// happened.
fn entry_to_write(
    decision: McpEntryDecision,
) -> Result<(WenlanMcpEntry, Vec<UndeterminedInput>), AppError> {
    match decision {
        McpEntryDecision::Write {
            entry,
            undetermined,
        } => Ok((entry, undetermined)),
        McpEntryDecision::PreserveExisting { unmeasured } => {
            Err(AppError::Generic(unresolved_message(&unmeasured)))
        }
    }
}

/// The config file's current contents, or `None` when it is measurably absent.
/// ONE read, three answers — see `unreadable_config_message` for the branch
/// that `exists()` plus a separate `read_to_string` truncates.
fn existing_config(config_path: &Path, read: ConfigRead) -> Result<Option<String>, AppError> {
    match read {
        ConfigRead::Contents(contents) => Ok(Some(contents)),
        ConfigRead::Absent => Ok(None),
        ConfigRead::Unreadable(error) => Err(unreadable_config_message(config_path, &error)),
    }
}

/// The same read for the removal verbs, where an absent file is an error rather
/// than a fresh start. `exists()` reported "No config file found" for a metadata
/// denial too, presenting a failed look as a measured absence.
fn config_to_remove_from(config_path: &Path, read: ConfigRead) -> Result<String, AppError> {
    match read {
        ConfigRead::Contents(contents) => Ok(contents),
        ConfigRead::Absent => Err(AppError::Generic(
            "No config file found — nothing to remove".into(),
        )),
        ConfigRead::Unreadable(error) => Err(AppError::Generic(format!(
            "Could not read {} ({error}), so Wenlan cannot tell whether there is anything to \
             remove. Nothing was changed.",
            config_path.display()
        ))),
    }
}

/// Write the Wenlan MCP server entry into `client_type`'s config file, named
/// for that client (see [`agent_name_for_client`]). A legacy `origin` entry is
/// replaced, not kept beside it: it is the same server under its old name, so
/// keeping it would leave a second launch, possibly a broken one, that a
/// repair never touched.
/// For Claude Code, a missing file is an error (Claude Code manages its own
/// config file).
///
/// `Ok` carries the inputs that could NOT be determined while deciding what to
/// write: a write that succeeded while one of its inputs went unread is not the
/// same event as one where everything was measured.
pub fn write_wenlan_entry(
    config_path: &std::path::Path,
    client_type: &str,
) -> Result<Vec<UndeterminedInput>, AppError> {
    write_wenlan_entry_in(
        config_path,
        client_type == "claude_code",
        JsonDialect::for_client(client_type),
        with_agent_name(wenlan_mcp_decision().0, agent_name_for_client(client_type)),
    )
}

/// [`write_wenlan_entry_in`] in the `mcpServers` dialect — the body most
/// writer tests stage a decision through.
#[cfg(test)]
pub(crate) fn write_wenlan_entry_with(
    config_path: &std::path::Path,
    is_claude_code: bool,
    decision: McpEntryDecision,
) -> Result<Vec<UndeterminedInput>, AppError> {
    write_wenlan_entry_in(
        config_path,
        is_claude_code,
        JsonDialect::McpServers,
        decision,
    )
}

/// Body of [`write_wenlan_entry`] with the decision handed in, so a test can
/// stage `PreserveExisting` deterministically. The real OS refusal that
/// produces it (a denied ACL, a disconnected share) is not reproducible on
/// every platform a test runs on — Windows answers `NotFound` for several
/// shapes Unix answers `ENOTDIR` for — and the branch under test is this
/// function's, not the OS's.
fn write_wenlan_entry_in(
    config_path: &std::path::Path,
    is_claude_code: bool,
    dialect: JsonDialect,
    decision: McpEntryDecision,
) -> Result<Vec<UndeterminedInput>, AppError> {
    let (entry, undetermined) = entry_to_write(decision)?;
    let existing = existing_config(config_path, read_config_for(dialect, config_path))?;
    let mut root = match &existing {
        // Read, parse, CHECK THE SHAPE, build the new document, and only then
        // back up — see `back_up_parsed`. Backing up first overwrites a
        // `config.json.bak` holding the last GOOD configuration with a
        // malformed `config.json` on the way to reporting `Invalid JSON`.
        Some(contents) => {
            let parsed = serde_json::from_str::<serde_json::Value>(contents).map_err(|e| {
                AppError::Generic(format!("Invalid JSON in {}: {}", config_path.display(), e))
            })?;
            check_json_config_shape(config_path, &parsed, dialect)?;
            parsed
        }
        None if is_claude_code => {
            return Err(AppError::Generic(
                "Claude Code config file not found — Claude Code manages this file internally"
                    .into(),
            ))
        }
        // Create minimal skeleton for Claude Desktop / Cursor
        None => serde_json::json!({}),
    };

    // Ensure the servers key (`mcpServers`, or `mcp_servers` for Muse Code) is
    // an OBJECT, not merely present — `check_json_config_shape` has already
    // ruled out every shape but object/null/absent, and a present `null` still
    // has to be replaced before it can be indexed.
    let servers_key = dialect.servers_key();
    if !root.get(servers_key).is_some_and(|v| v.is_object()) {
        root[servers_key] = serde_json::json!({});
    }
    root[servers_key][MCP_SERVER_KEY] = dialect.entry_value(entry)?;
    if let Some(servers) = root[servers_key].as_object_mut() {
        servers.remove(LEGACY_MCP_SERVER_KEY);
    }
    dialect.prepare_root(&mut root);

    // Write back with pretty formatting
    let formatted =
        serde_json::to_string_pretty(&root).map_err(|e| AppError::Generic(e.to_string()))?;
    // Everything that could fail has failed by now, so the backup cannot be
    // left behind by a change that never happened.
    if let Some(contents) = &existing {
        back_up_parsed(config_path, contents, "json.bak")?;
    }
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(config_path, formatted)?;

    Ok(undetermined)
}

/// Upsert the Wenlan entry into a Codex CLI `config.toml` — format-preserving:
/// user comments, key order, and unrelated tables survive byte-for-byte
/// (toml_edit round-trips everything it didn't touch). Named and migrated as
/// in [`write_wenlan_entry`].
pub fn write_wenlan_entry_toml(
    config_path: &std::path::Path,
) -> Result<Vec<UndeterminedInput>, AppError> {
    write_wenlan_entry_toml_with(
        config_path,
        with_agent_name(wenlan_mcp_decision().0, agent_name_for_client("codex_cli")),
    )
}

/// Body of [`write_wenlan_entry_toml`] with the decision handed in. Same
/// reason as [`write_wenlan_entry_with`].
pub(crate) fn write_wenlan_entry_toml_with(
    config_path: &std::path::Path,
    decision: McpEntryDecision,
) -> Result<Vec<UndeterminedInput>, AppError> {
    use toml_edit::{DocumentMut, Item, Table};

    let (entry, undetermined) = entry_to_write(decision)?;
    let existing = existing_config(config_path, read_config(config_path))?;
    let mut doc: DocumentMut = match &existing {
        // Same ordering as `write_wenlan_entry_with`: parse FIRST, back up only
        // what parsed.
        Some(contents) => {
            let parsed: DocumentMut = contents.parse().map_err(|e| {
                AppError::Generic(format!("Invalid TOML in {}: {}", config_path.display(), e))
            })?;
            // `doc["mcp_servers"][key] = ..` is `toml_edit`'s `IndexMut`, i.e.
            // `index_mut(..).expect("index not found")` — a PANIC for any
            // `mcp_servers` that is not table-like. `mcp_servers = 5` parses
            // fine and survives the presence check. Same class as the JSON
            // crash: a schema error, raised before any backup or write.
            match parsed.get("mcp_servers") {
                None => {}
                Some(servers) if servers.as_table_like().is_some() => {}
                Some(servers) => {
                    return Err(AppError::Generic(format!(
                        "Unexpected shape in {}: `mcp_servers` is {}, but it has to be a table for \
                         Wenlan to add its entry to it. Nothing was written — your existing MCP \
                         configuration is unchanged.",
                        config_path.display(),
                        servers.type_name()
                    )))
                }
            }
            parsed
        }
        None => DocumentMut::new(),
    };

    if doc.get("mcp_servers").is_none() {
        let mut parent = Table::new();
        parent.set_implicit(true); // render only [mcp_servers.wenlan], no bare [mcp_servers]
        doc.insert("mcp_servers", Item::Table(parent));
    }

    let mut server = Table::new();
    server.insert("command", toml_edit::value(entry.command));
    let mut args = toml_edit::Array::new();
    for a in entry.args {
        args.push(a);
    }
    server.insert("args", toml_edit::value(args));
    doc["mcp_servers"][MCP_SERVER_KEY] = Item::Table(server);
    if let Some(servers) = doc["mcp_servers"].as_table_like_mut() {
        servers.remove(LEGACY_MCP_SERVER_KEY);
    }

    let formatted = doc.to_string();
    // Backup last, from the bytes that were parsed — see `back_up_parsed`.
    if let Some(contents) = &existing {
        back_up_parsed(config_path, contents, "toml.bak")?;
    }
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(config_path, formatted)?;
    Ok(undetermined)
}

/// Shared body of the JSON removal verbs below: read, parse, remove every key
/// in `keys` from `dialect`'s servers key (`mcpServers`; `mcp_servers` for Muse
/// Code), then — only once a removal is certain — back
/// the file up before writing it back, so a no-op leaves no stray `.bak`.
/// `not_found` is the caller's nothing-was-removed message, which the UI
/// surfaces verbatim. Every sibling server and unrelated key survives.
fn remove_json_keys(
    config_path: &std::path::Path,
    dialect: JsonDialect,
    keys: &[&str],
    not_found: &str,
) -> Result<(), AppError> {
    let contents = config_to_remove_from(config_path, read_config_for(dialect, config_path))?;
    let mut root = serde_json::from_str::<serde_json::Value>(&contents).map_err(|e| {
        AppError::Generic(format!("Invalid JSON in {}: {}", config_path.display(), e))
    })?;

    let removed = root
        .get_mut(dialect.servers_key())
        .and_then(|servers| servers.as_object_mut())
        .map(|servers| {
            // Deliberately not `any`/`fold`: every key must be removed, so
            // this must not short-circuit on the first match.
            let mut removed = false;
            for key in keys {
                removed |= servers.remove(*key).is_some();
            }
            removed
        })
        .unwrap_or(false);

    if !removed {
        return Err(AppError::Generic(not_found.into()));
    }

    let formatted =
        serde_json::to_string_pretty(&root).map_err(|e| AppError::Generic(e.to_string()))?;
    // Same backup rule as the writers: written from the bytes that were parsed,
    // and only while they are still on disk.
    back_up_parsed(config_path, &contents, "json.bak")?;
    std::fs::write(config_path, formatted)?;
    Ok(())
}

/// TOML counterpart of [`remove_json_keys`] for Codex CLI's `[mcp_servers.*]`
/// tables, using the format-preserving `toml_edit` round-trip
/// `write_wenlan_entry_toml` writes with. Same contract, same ordering.
fn remove_toml_keys(
    config_path: &std::path::Path,
    keys: &[&str],
    not_found: &str,
) -> Result<(), AppError> {
    use toml_edit::DocumentMut;

    let contents = config_to_remove_from(config_path, read_config(config_path))?;
    let mut doc: DocumentMut = contents.parse().map_err(|e| {
        AppError::Generic(format!("Invalid TOML in {}: {}", config_path.display(), e))
    })?;

    let removed = doc
        .get_mut("mcp_servers")
        .and_then(|servers| servers.as_table_like_mut())
        .map(|servers| {
            // Deliberately not `any`/`fold`: every key must be removed, so
            // this must not short-circuit on the first match.
            let mut removed = false;
            for key in keys {
                removed |= servers.remove(key).is_some();
            }
            removed
        })
        .unwrap_or(false);

    if !removed {
        return Err(AppError::Generic(not_found.into()));
    }

    let formatted = doc.to_string();
    back_up_parsed(config_path, &contents, "toml.bak")?;
    std::fs::write(config_path, formatted)?;
    Ok(())
}

/// Remove the raw `wenlan`/legacy `origin` `mcpServers` entries from a JSON
/// client config — the inverse of `write_wenlan_entry`, and the fix for the
/// double-registration Diagnostics surfaces (a plugin *and* a raw entry for
/// one client). Symmetric with detection: it removes exactly the keys
/// `has_configured_entry` recognizes. A missing file, or a file with neither
/// key present, is `Err`. Backs the file up, but only once a removal is
/// certain, so the no-op error path leaves no stray `.bak`.
pub fn remove_wenlan_entry(config_path: &std::path::Path) -> Result<(), AppError> {
    remove_json_keys(
        config_path,
        JsonDialect::McpServers,
        &[MCP_SERVER_KEY, LEGACY_MCP_SERVER_KEY],
        "No Wenlan MCP entry found to remove",
    )
}

/// [`remove_wenlan_entry`] for `client_type`'s own JSON dialect: Muse Code's
/// entry lives under `mcp_servers`, so removing it from `mcpServers` would
/// report "nothing to remove" about a file that holds the entry.
pub fn remove_wenlan_entry_for(
    config_path: &std::path::Path,
    client_type: &str,
) -> Result<(), AppError> {
    remove_json_keys(
        config_path,
        JsonDialect::for_client(client_type),
        &[MCP_SERVER_KEY, LEGACY_MCP_SERVER_KEY],
        "No Wenlan MCP entry found to remove",
    )
}

/// TOML variant for Codex CLI (`[mcp_servers.*]` tables) — mirrors
/// `remove_wenlan_entry`'s contract and `has_configured_entry_toml`'s key set,
/// using the same format-preserving `toml_edit` round-trip
/// `write_wenlan_entry_toml` writes with.
pub fn remove_wenlan_entry_toml(config_path: &std::path::Path) -> Result<(), AppError> {
    remove_toml_keys(
        config_path,
        &[MCP_SERVER_KEY, LEGACY_MCP_SERVER_KEY],
        "No Wenlan MCP entry found to remove",
    )
}

/// Remove ONLY the legacy `origin` `mcpServers` entry from a JSON client
/// config, keeping the live `wenlan` entry — the fix for the raw+raw
/// duplicate a no-plugin client (Cursor, Gemini CLI) lands in after the
/// rename. Critically different from `remove_wenlan_entry`, which drops both
/// keys: that is correct only where a plugin still provides the server, so
/// applying it here would delete the client's only working connection. A
/// missing file, or one with no `origin` entry, is `Err`.
pub fn remove_legacy_origin_entry(config_path: &std::path::Path) -> Result<(), AppError> {
    remove_json_keys(
        config_path,
        JsonDialect::McpServers,
        &[LEGACY_MCP_SERVER_KEY],
        "No legacy origin MCP entry found to remove",
    )
}

/// [`remove_legacy_origin_entry`] for `client_type`'s own JSON dialect. Same
/// reason as [`remove_wenlan_entry_for`].
pub fn remove_legacy_origin_entry_for(
    config_path: &std::path::Path,
    client_type: &str,
) -> Result<(), AppError> {
    remove_json_keys(
        config_path,
        JsonDialect::for_client(client_type),
        &[LEGACY_MCP_SERVER_KEY],
        "No legacy origin MCP entry found to remove",
    )
}

/// TOML variant for Codex CLI (`[mcp_servers.*]` tables) — mirrors
/// `remove_legacy_origin_entry`'s contract (removes only `origin`, keeps
/// `wenlan`) using the same format-preserving `toml_edit` round-trip.
pub fn remove_legacy_origin_entry_toml(config_path: &std::path::Path) -> Result<(), AppError> {
    remove_toml_keys(
        config_path,
        &[LEGACY_MCP_SERVER_KEY],
        "No legacy origin MCP entry found to remove",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env::EnvGuard;

    /// A [`Reading`] as a bool, for the many tests below that are about the
    /// FACT and not about readability. `Unreadable` PANICS rather than counting
    /// as `false`: a test whose fixture the OS refused to read must fail
    /// loudly, never quietly pass as "measured: no".
    #[track_caller]
    fn yes(reading: Reading) -> bool {
        match reading {
            Reading::Yes => true,
            Reading::No => false,
            Reading::Unreadable { error } => {
                panic!("the fixture could not be read, which is not a `no`: {error}")
            }
        }
    }

    /// A content question's answer as a bool. A parse failure PANICS rather
    /// than counting as `false` — same rule as [`yes`], so a fixture with a
    /// typo in it cannot pass a `…_false_when_no_wenlan_entry` test for the
    /// wrong reason.
    #[track_caller]
    fn parsed(answer: Result<bool, String>) -> bool {
        match answer {
            Ok(answer) => answer,
            Err(error) => {
                panic!("the fixture could not be parsed, which is not a `no`: {error}")
            }
        }
    }

    /// The parse error from a body that must NOT be measurable, for the tests
    /// that pin the failure itself.
    #[track_caller]
    fn unparseable(answer: Result<bool, String>) -> String {
        match answer {
            Err(error) => error,
            Ok(answer) => panic!(
                "expected an unparseable body to be unmeasurable, but it measured {answer:?}"
            ),
        }
    }

    impl ClientConfigPath {
        #[track_caller]
        fn unwrap(self) -> PathBuf {
            match self {
                ClientConfigPath::Known(path) => path,
                other => panic!("expected a known config path, got {other:?}"),
            }
        }
    }

    /// Every known client's config path, by the tail each one ends in.
    #[test]
    fn test_client_config_path_per_client() {
        for (client_type, tail) in [
            (
                "claude_desktop",
                ["Claude", "claude_desktop_config.json"].as_slice(),
            ),
            ("cursor", &[".cursor", "mcp.json"]),
            ("claude_code", &[".claude.json"]),
            ("gemini_cli", &[".gemini", "settings.json"]),
            ("codex_cli", &[".codex", "config.toml"]),
            ("muse_code", &[".config", "muse", "settings.json"]),
            ("antigravity", &[".gemini", "config", "mcp_config.json"]),
            ("copilot_cli", &[".copilot", "mcp-config.json"]),
            ("vscode", &["Code", "User", "mcp.json"]),
            ("windsurf", &[".codeium", "windsurf", "mcp_config.json"]),
            ("kiro", &[".kiro", "settings", "mcp.json"]),
            ("opencode", &[".config", "opencode", "opencode.json"]),
        ] {
            let path = client_config_path(client_type).unwrap();
            let expected = tail.iter().fold(PathBuf::new(), |acc, part| acc.join(part));
            assert!(
                path.ends_with(&expected),
                "{client_type} resolved to {} rather than a path ending in {}",
                path.display(),
                expected.display()
            );
        }
    }

    #[test]
    fn test_client_config_path_unknown() {
        assert_eq!(
            client_config_path("unknown"),
            ClientConfigPath::UnknownClient
        );
    }

    #[test]
    fn test_check_already_configured() {
        for (json, expected) in [
            (
                r#"{"mcpServers": {"origin": {"command": "npx", "args": ["-y", "origin-mcp"]}}}"#,
                true,
            ),
            (
                r#"{"mcpServers": {"wenlan": {"command": "npx", "args": ["-y", "wenlan-mcp"]}}}"#,
                true,
            ),
            (r#"{"mcpServers": {"other-server": {}}}"#, false),
            (r#"{"theme": "dark"}"#, false),
        ] {
            assert_eq!(parsed(has_configured_entry(json)), expected, "{json}");
        }
    }

    /// A body that could not be parsed must not answer what a parsed body with
    /// no entry answers. The property the old `assert!(!…("not json"))` was
    /// really guarding — garbage must not panic or hang the detector — still
    /// holds: the call returns, and the answer is now the honest one. The
    /// `unparseable(..)` assertions in the sibling predicates' tests below pin
    /// the same rule for each of them.
    #[test]
    fn an_unparseable_config_is_unmeasurable_not_a_measured_absence() {
        let error = unparseable(has_configured_entry("not json"));
        assert!(
            error.contains("not valid JSON"),
            "the reason reaches the user's chip and the pasted report, so it has to name the \
             file's problem: {error}"
        );
        // The same body, through the layer the UI actually reads.
        let reading = ConfigRead::Contents("not json".to_string()).asks(has_configured_entry);
        assert!(
            matches!(reading, Reading::Unreadable { .. }),
            "a present-but-unparseable config is a failed measurement, not `no entry`: \
             {reading:?}"
        );
    }

    /// Matching is by the `wenlan@` prefix, never a literal marketplace name:
    /// a fresh install writes the short form and a machine that added the old
    /// self-hosted marketplace (deleted upstream in 048d77a8) writes the long
    /// one, and both populations have to match.
    #[test]
    fn test_claude_code_plugin_enabled() {
        for (json, expected) in [
            (r#"{"enabledPlugins": {"wenlan@7xuanlu": true}}"#, true),
            (
                r#"{"enabledPlugins": {"wenlan@7xuanlu-wenlan": true}}"#,
                true,
            ),
            (r#"{"enabledPlugins": {"wenlan@7xuanlu": false}}"#, false),
            (
                r#"{"enabledPlugins": {"other-plugin@somewhere": true}}"#,
                false,
            ),
            (r#"{"theme": "dark"}"#, false),
        ] {
            assert_eq!(parsed(claude_code_plugin_enabled(json)), expected, "{json}");
        }
    }

    /// A `~/.claude/settings.json` that would not parse must not be
    /// indistinguishable from one with the plugin switched off — "switched off"
    /// is what licenses writing a raw entry.
    #[test]
    fn an_unparseable_claude_code_settings_file_is_unmeasurable() {
        let error = unparseable(claude_code_plugin_enabled("not json"));
        assert!(error.contains("not valid JSON"), "{error}");
    }

    /// Prefix matching again: `wenlan-local` is the pre-wenlan-app/wenlan#348
    /// marketplace name and `7xuanlu-wenlan` the post-rename one.
    #[test]
    fn test_codex_cli_plugin_enabled() {
        for (toml, expected) in [
            ("[plugins.\"wenlan@wenlan-local\"]\nenabled = true\n", true),
            (
                "[plugins.\"wenlan@7xuanlu-wenlan\"]\nenabled = true\n",
                true,
            ),
            (
                "[plugins.\"wenlan@wenlan-local\"]\nenabled = false\n",
                false,
            ),
            ("[plugins.\"other@somewhere\"]\nenabled = true\n", false),
            ("model = \"gpt-5.5\"\n", false),
        ] {
            assert_eq!(parsed(codex_cli_plugin_enabled(toml)), expected, "{toml}");
        }
    }

    #[test]
    fn an_unparseable_codex_config_is_unmeasurable() {
        let error = unparseable(codex_cli_plugin_enabled("not toml ["));
        assert!(error.contains("not valid TOML"), "{error}");
    }

    /// Manifest fixture matching the real shape seen on a live machine:
    /// `wenlan` present, plus another entry whose `marketplaceName` ("My
    /// Uploads") deliberately differs from its `name` ("social-media-skills")
    /// — a name/marketplaceName mixup would false-positive on this fixture.
    fn manifest_with_wenlan() -> &'static str {
        r#"{"plugins": [
            {"id": "plugin_1", "name": "social-media-skills", "marketplaceId": "m1", "marketplaceName": "My Uploads"},
            {"id": "plugin_2", "name": "wenlan", "marketplaceId": "m2", "marketplaceName": "wenlan"}
        ]}"#
    }

    /// The match is on `name`, exactly. `wenlan-old` rules out a
    /// `starts_with`/`contains` match, `Wenlan` a case-insensitive one, and the
    /// `other-plugin`/`marketplaceName: wenlan` row rules out matching the
    /// wrong field.
    #[test]
    fn test_claude_desktop_plugin_enabled() {
        for (json, expected) in [
            (manifest_with_wenlan(), true),
            (
                r#"{"plugins": [{"id": "p1", "name": "wenlan-old", "marketplaceName": "wenlan"}]}"#,
                false,
            ),
            (
                r#"{"plugins": [{"id": "p1", "name": "other-plugin", "marketplaceName": "wenlan"}]}"#,
                false,
            ),
            (r#"{"plugins": [{"id": "p1", "name": "Wenlan"}]}"#, false),
            (r#"{"lastUpdated": 1}"#, false),
        ] {
            assert_eq!(
                parsed(claude_desktop_plugin_enabled(json)),
                expected,
                "{json}"
            );
        }
    }

    #[test]
    fn an_unparseable_desktop_manifest_is_unmeasurable() {
        let error = unparseable(claude_desktop_plugin_enabled("not json"));
        assert!(error.contains("not valid JSON"), "{error}");
    }

    #[test]
    fn test_claude_desktop_account_id_extracts_last_known_account_uuid() {
        let json = r#"{"lastKnownAccountUuid": "acct-123", "locale": "en-US"}"#;
        assert_eq!(
            claude_desktop_account_id(json),
            Ok(Some("acct-123".to_string()))
        );
    }

    #[test]
    fn test_claude_desktop_account_id_none_when_key_missing() {
        assert_eq!(
            claude_desktop_account_id(r#"{"locale": "en-US"}"#),
            Ok(None)
        );
    }

    /// The account id names the sessions directory, so an unparseable
    /// `config.json` means the manifest scan never happened at all — which must
    /// not answer the `None` that a file pinning no account answers.
    #[test]
    fn an_unparseable_desktop_config_cannot_answer_which_account_is_pinned() {
        let error = claude_desktop_account_id("not json")
            .expect_err("an unparseable config.json pins no account it could report");
        assert!(error.contains("not valid JSON"), "{error}");
    }

    #[test]
    fn test_claude_desktop_account_sessions_dir_joins_expected_segments() {
        let support_dir = Path::new("/support");
        let dir = claude_desktop_account_sessions_dir(support_dir, "acct-1");
        assert_eq!(dir, Path::new("/support/local-agent-mode-sessions/acct-1"));
    }

    #[test]
    fn test_sessions_dir_true_when_one_session_has_wenlan() {
        let tmp = tempfile::tempdir().unwrap();
        let session_dir = tmp.path().join("sess-1").join("rpm");
        std::fs::create_dir_all(&session_dir).unwrap();
        std::fs::write(session_dir.join("manifest.json"), manifest_with_wenlan()).unwrap();
        assert!(yes(claude_desktop_plugin_enabled_in_sessions_dir(
            tmp.path()
        )));
    }

    #[test]
    fn test_sessions_dir_true_when_second_of_two_sessions_has_wenlan() {
        let tmp = tempfile::tempdir().unwrap();
        let no_wenlan = tmp.path().join("sess-a").join("rpm");
        std::fs::create_dir_all(&no_wenlan).unwrap();
        std::fs::write(
            no_wenlan.join("manifest.json"),
            r#"{"plugins": [{"id": "p1", "name": "engineering"}]}"#,
        )
        .unwrap();

        let with_wenlan = tmp.path().join("sess-b").join("rpm");
        std::fs::create_dir_all(&with_wenlan).unwrap();
        std::fs::write(with_wenlan.join("manifest.json"), manifest_with_wenlan()).unwrap();

        assert!(yes(claude_desktop_plugin_enabled_in_sessions_dir(
            tmp.path()
        )));
    }

    #[test]
    fn test_sessions_dir_false_when_dir_missing() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!yes(claude_desktop_plugin_enabled_in_sessions_dir(
            &tmp.path().join("does-not-exist")
        )));
    }

    #[test]
    fn test_sessions_dir_false_when_no_rpm_subdir() {
        let tmp = tempfile::tempdir().unwrap();
        let session_dir = tmp.path().join("sess-1");
        std::fs::create_dir_all(&session_dir).unwrap();
        // manifest.json exists but not under rpm/
        std::fs::write(session_dir.join("manifest.json"), manifest_with_wenlan()).unwrap();
        assert!(!yes(claude_desktop_plugin_enabled_in_sessions_dir(
            tmp.path()
        )));
    }

    #[test]
    fn test_sessions_dir_tolerates_malformed_manifest_alongside_a_valid_one() {
        let tmp = tempfile::tempdir().unwrap();
        let broken = tmp.path().join("sess-broken").join("rpm");
        std::fs::create_dir_all(&broken).unwrap();
        std::fs::write(broken.join("manifest.json"), "not json").unwrap();

        let good = tmp.path().join("sess-good").join("rpm");
        std::fs::create_dir_all(&good).unwrap();
        std::fs::write(good.join("manifest.json"), manifest_with_wenlan()).unwrap();

        assert!(yes(claude_desktop_plugin_enabled_in_sessions_dir(
            tmp.path()
        )));
    }

    /// Builds `<tmp>/config.json` (with `lastKnownAccountUuid`) plus
    /// `<tmp>/local-agent-mode-sessions/<account_id>/<session_id>/rpm/manifest.json`
    /// — the exact shape verified on a live Claude Desktop install — so
    /// `claude_desktop_plugin_enabled_for_support_dir` is exercised
    /// end-to-end against a fake `support_dir`.
    fn write_support_dir_fixture(root: &Path, account_id: &str, session_id: &str, manifest: &str) {
        std::fs::write(
            root.join("config.json"),
            format!(r#"{{"lastKnownAccountUuid": "{account_id}"}}"#),
        )
        .unwrap();
        let rpm_dir = root
            .join("local-agent-mode-sessions")
            .join(account_id)
            .join(session_id)
            .join("rpm");
        std::fs::create_dir_all(&rpm_dir).unwrap();
        std::fs::write(rpm_dir.join("manifest.json"), manifest).unwrap();
    }

    #[test]
    fn test_support_dir_true_when_pinned_account_session_has_wenlan() {
        let tmp = tempfile::tempdir().unwrap();
        write_support_dir_fixture(tmp.path(), "acct-1", "sess-1", manifest_with_wenlan());
        assert!(yes(claude_desktop_plugin_enabled_for_support_dir(
            tmp.path()
        )));
    }

    #[test]
    fn test_support_dir_false_when_wenlan_only_under_a_different_account() {
        let tmp = tempfile::tempdir().unwrap();
        // config.json pins "acct-1", but the manifest with wenlan lives
        // under a *different* account id — must not count.
        std::fs::write(
            tmp.path().join("config.json"),
            r#"{"lastKnownAccountUuid": "acct-1"}"#,
        )
        .unwrap();
        let rpm_dir = tmp
            .path()
            .join("local-agent-mode-sessions")
            .join("acct-2")
            .join("sess-1")
            .join("rpm");
        std::fs::create_dir_all(&rpm_dir).unwrap();
        std::fs::write(rpm_dir.join("manifest.json"), manifest_with_wenlan()).unwrap();

        assert!(!yes(claude_desktop_plugin_enabled_for_support_dir(
            tmp.path()
        )));
    }

    #[test]
    fn test_support_dir_false_when_config_json_missing() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!yes(claude_desktop_plugin_enabled_for_support_dir(
            tmp.path()
        )));
    }

    #[test]
    fn test_support_dir_never_reads_skills_plugin_sentinel() {
        // The `skills-plugin` sentinel sits alongside the real account-id
        // directory under `local-agent-mode-sessions/` on a real machine.
        // It is not a UUID, so it can never be `lastKnownAccountUuid` — a
        // manifest planted only under it must never count.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("config.json"),
            r#"{"lastKnownAccountUuid": "acct-1"}"#,
        )
        .unwrap();
        let sentinel_rpm = tmp
            .path()
            .join("local-agent-mode-sessions")
            .join("skills-plugin")
            .join("sess-1")
            .join("rpm");
        std::fs::create_dir_all(&sentinel_rpm).unwrap();
        std::fs::write(sentinel_rpm.join("manifest.json"), manifest_with_wenlan()).unwrap();

        assert!(!yes(claude_desktop_plugin_enabled_for_support_dir(
            tmp.path()
        )));
    }

    /// Live-machine sanity check, not part of the default gating suite (this
    /// machine's Claude Desktop state is not portable to CI) — run explicitly
    /// with `cargo test --lib -- --ignored`. It is the one test here that runs
    /// against the REAL support dir, so it is the only one that catches
    /// `detect_mcp_clients`'s `claude_desktop` branch being severed from
    /// `claude_desktop_plugin_enabled_on_disk`.
    #[test]
    #[ignore]
    fn claude_desktop_detected_via_real_plugin_manifest() {
        let claude_desktop = detect_mcp_clients()
            .into_iter()
            .find(|c| c.client_type == "claude_desktop")
            .expect("claude_desktop row always present");
        assert_eq!(
            claude_desktop.already_configured,
            Reading::Yes,
            "expected a MEASURED yes: this machine has the Wenlan chat-side plugin installed"
        );
    }

    /// `wenlan-mcp` under `dir`, named from `std::env::consts::EXE_SUFFIX`
    /// rather than from `wenlan_mcp_in`. A fixture built from the code under
    /// test only ever agrees with itself; std's suffix is the independent
    /// answer that lets these tests fail when the resolver forgets `.exe`.
    fn installed_wenlan_mcp(dir: &Path) -> PathBuf {
        dir.join(format!("wenlan-mcp{}", std::env::consts::EXE_SUFFIX))
    }

    /// Write a fixture that is plausibly a program: non-empty, and on Unix
    /// carrying the execute bit. A zero-byte fixture would be certified as the
    /// resolved binary by a probe that only checks `is_file()`, so the fixture
    /// has to be able to tell the two apart. `MZ` is the DOS/PE signature;
    /// nothing here parses it.
    fn write_binary_fixture(path: &Path) {
        std::fs::write(path, b"MZ\x90\x00 wenlan-mcp test fixture\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    /// Unwrap the entry a decision says to write, failing loudly (with the
    /// unreadable paths) when the decision was "write nothing".
    fn written_entry(decision: McpEntryDecision) -> WenlanMcpEntry {
        match decision {
            McpEntryDecision::Write { entry, .. } => entry,
            McpEntryDecision::PreserveExisting { unmeasured } => panic!(
                "expected an entry to be written, but the resolution was unresolved: \
                 {unmeasured:?}"
            ),
        }
    }

    /// Create a fixture home whose `.wenlan/bin` holds a real `wenlan-mcp`
    /// executable file, and point the resolver at it. Returns the binary path.
    fn install_wenlan_mcp_into(home: &Path) -> PathBuf {
        let bin_dir = home.join(".wenlan").join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let installed = installed_wenlan_mcp(&bin_dir);
        write_binary_fixture(&installed);
        installed
    }

    /// The entry the app writes into client configs must be the installed
    /// binary whenever one exists. `npx` here is a failure, not an alternative:
    /// it is a network dependency and a version-skew hazard, and it is what a
    /// Windows install gets when the candidate paths carry no `.exe` suffix.
    #[test]
    #[serial_test::serial]
    fn wenlan_mcp_entry_takes_the_installed_binary_over_npx() {
        let _env = EnvGuard::capture(&[MCP_RESOLVER_HOME_ENV, "WENLAN_MCP_DEV_BIN"]);
        let tmp = tempfile::tempdir().unwrap();
        let installed = install_wenlan_mcp_into(tmp.path());
        std::env::remove_var("WENLAN_MCP_DEV_BIN");
        std::env::set_var(MCP_RESOLVER_HOME_ENV, tmp.path());

        let report = wenlan_mcp_entry().expect("a readable installed binary resolves");
        // An empty `undetermined` is itself a measurement: every resolver input
        // was read. This fixture determines all three.
        assert!(
            report.undetermined.is_empty(),
            "nothing was undetermined in this fixture: {:?}",
            report.undetermined
        );
        let entry = report.entry;

        assert_ne!(
            entry.command,
            "npx",
            "an installed {} was ignored and the npx fallback was written instead",
            installed.display()
        );
        // Compared as paths: `join` mixes separators on Windows, and the
        // question here is which file was chosen, not how it was spelled.
        assert_eq!(Path::new(&entry.command), installed);
        assert!(entry.args.is_empty());
    }

    /// The other arm, made just as deterministic: with nothing installed the
    /// entry is the pinned `npx` fallback, typed as command + args.
    #[test]
    #[serial_test::serial]
    fn wenlan_mcp_entry_is_the_pinned_npx_fallback_when_nothing_is_installed() {
        let _env = EnvGuard::capture(&[MCP_RESOLVER_HOME_ENV, "WENLAN_MCP_DEV_BIN"]);
        let tmp = tempfile::tempdir().unwrap();
        std::env::remove_var("WENLAN_MCP_DEV_BIN");
        std::env::set_var(MCP_RESOLVER_HOME_ENV, tmp.path());

        let entry = wenlan_mcp_entry()
            .expect("an empty tree is a MEASURED absence, not a failure")
            .entry;

        assert_eq!(entry.command, "npx");
        assert_eq!(entry.args.len(), 2);
        assert_eq!(entry.args[0], "-y");
        assert!(entry.args[1].starts_with("wenlan-mcp@^"));
    }

    /// The bundled binary — the one a Windows installer drops next to the app
    /// exe — must resolve on every platform. Without the executable suffix
    /// this returns `None` on Windows and the caller writes `npx`.
    #[test]
    fn bundled_wenlan_mcp_next_to_the_app_exe_resolves_on_this_platform() {
        let tmp = tempfile::tempdir().unwrap();
        let exe_dir = tmp.path().join("Programs").join("Wenlan");
        std::fs::create_dir_all(&exe_dir).unwrap();
        let bundled = installed_wenlan_mcp(&exe_dir);
        write_binary_fixture(&bundled);

        assert_eq!(
            resolve_wenlan_mcp(None, None, Some(&exe_dir), probe_candidate),
            McpBinaryResolution::Found {
                path: bundled.clone(),
                undetermined: Vec::new(),
            },
            "the bundled {} was not found; the app would write the npx fallback",
            bundled.display()
        );
    }

    /// A directory named `wenlan-mcp[.exe]` is not the binary. `Path::exists()`
    /// said it was, and the resolver wrote that directory into the user's MCP
    /// client config as the command to run.
    #[test]
    fn a_directory_named_like_the_binary_is_never_resolved_as_the_command() {
        let tmp = tempfile::tempdir().unwrap();
        let exe_dir = tmp.path().join("Programs").join("Wenlan");
        std::fs::create_dir_all(&exe_dir).unwrap();
        let impostor = installed_wenlan_mcp(&exe_dir);
        std::fs::create_dir_all(&impostor).unwrap();

        assert_eq!(
            probe_candidate(&impostor),
            CandidateProbe::NotAFile,
            "a directory at the candidate path must not probe as a usable binary"
        );
        assert_eq!(
            resolve_wenlan_mcp(None, None, Some(&exe_dir), probe_candidate),
            McpBinaryResolution::NoneInstalled,
            "a directory named {} must not resolve as the wenlan-mcp binary",
            impostor.display()
        );
        let entry = written_entry(wenlan_mcp_entry_for(
            resolve_wenlan_mcp(None, None, Some(&exe_dir), probe_candidate),
            "wenlan-mcp@^9.9.9",
        ));
        assert_eq!(
            entry.command, "npx",
            "a directory was written into a client config as the command"
        );
    }

    /// A candidate the OS refuses to stat is not an absence. Stubbed rather
    /// than produced with real ACLs, because the permission shape differs per
    /// platform and the branch under test is the resolver's, not the OS's.
    #[test]
    fn an_unreadable_candidate_is_not_a_measured_absence() {
        let home = PathBuf::from("/Users/someone");
        let installed = installed_wenlan_mcp(&home.join(".wenlan/bin"));
        let denied = installed.clone();
        let resolution = resolve_wenlan_mcp(Some(home.as_path()), None, None, move |p| {
            if p == denied {
                CandidateProbe::Unreadable {
                    error: "Access is denied. (os error 5)".to_string(),
                }
            } else {
                CandidateProbe::Absent
            }
        });
        match &resolution {
            McpBinaryResolution::Unresolved(unmeasured) => {
                assert_eq!(unmeasured.unreadable.len(), 1);
                assert_eq!(unmeasured.unreadable[0].0, installed);
                assert!(unmeasured.unreadable[0].1.contains("os error 5"));
                assert!(
                    unmeasured.undetermined.is_empty(),
                    "every input was determined; only the path could not be read"
                );
            }
            other => panic!("an unreadable candidate must not resolve as absent: {other:?}"),
        }
        assert_ne!(
            resolution,
            McpBinaryResolution::NoneInstalled,
            "'could not look' must never be reported as 'nothing is installed'"
        );
    }

    /// The documented ranking: an unreadable candidate does not end the
    /// search, so a later candidate that IS a measured file still wins. Before
    /// this, `exists()` returning false for a denied stat had the same effect
    /// — but it also had the same effect when nothing later existed, and that
    /// case was the silent `npx`.
    #[test]
    fn an_unreadable_candidate_does_not_outrank_a_later_measured_file() {
        let home = PathBuf::from("/Users/someone");
        let installed = installed_wenlan_mcp(&home.join(".wenlan/bin"));
        let cargo = installed_wenlan_mcp(&home.join(".cargo/bin"));
        let denied = installed.clone();
        let real = cargo.clone();
        let resolution = resolve_wenlan_mcp(Some(home.as_path()), None, None, move |p| {
            if p == denied {
                CandidateProbe::Unreadable {
                    error: "Access is denied. (os error 5)".to_string(),
                }
            } else if p == real {
                CandidateProbe::File
            } else {
                CandidateProbe::Absent
            }
        });
        assert_eq!(
            resolution,
            McpBinaryResolution::Found {
                path: cargo,
                undetermined: Vec::new()
            }
        );
    }

    /// `Absent` is the only error state that is an absence; everything else the
    /// filesystem can say is its own answer.
    #[test]
    fn probe_candidate_separates_absence_from_a_failed_look() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("wenlan-mcp");
        write_binary_fixture(&file);
        assert_eq!(probe_candidate(&file), CandidateProbe::File);
        assert_eq!(probe_candidate(tmp.path()), CandidateProbe::NotAFile);
        assert_eq!(
            probe_candidate(&tmp.path().join("nope")),
            CandidateProbe::Absent
        );
    }

    /// An input that could not be determined builds no candidate, so a search
    /// that looked at NOTHING must still not end in `NoneInstalled` and write
    /// `npx` over a user's working local command. The probe here panics if it
    /// is ever called, which states the fixture exactly: there is no candidate
    /// path to probe.
    #[test]
    fn an_input_that_could_not_be_determined_is_never_none_installed() {
        let never_probed = |path: &Path| -> CandidateProbe {
            panic!("no candidate could be constructed, so nothing should be probed: {path:?}")
        };

        let no_home = ResolverInputs {
            home: RootInput::Undetermined("the platform would not report a home directory".into()),
            dev_bin: RootInput::NotSet,
            exe_dir: RootInput::NotSet,
        };
        let (decision, trail) =
            wenlan_mcp_decision_from(&no_home, never_probed, "wenlan-mcp@^9.9.9");
        assert!(
            trail.is_empty(),
            "the fixture is not staging the defect if candidates were built: {trail:?}"
        );
        match decision {
            McpEntryDecision::PreserveExisting { unmeasured } => {
                assert!(unmeasured.unreadable.is_empty());
                assert_eq!(unmeasured.undetermined.len(), 1);
                assert_eq!(unmeasured.undetermined[0].input, "the home directory");
                assert_eq!(
                    unmeasured.undetermined[0].blocked, "installed and cargo",
                    "the message has to name the candidates that were never checked"
                );
                let message = unresolved_message(&unmeasured);
                assert!(
                    message.contains("home directory") && message.contains("unchanged"),
                    "the user-facing message must name what could not be determined and say \
                     nothing was written: {message}"
                );
            }
            McpEntryDecision::Write { entry, .. } => panic!(
                "a search that could not even build its candidate paths produced `{} {}` -- a \
                 measured-absence outcome manufactured from an unread input",
                entry.command,
                entry.args.join(" ")
            ),
        }

        // Same shape, different input: `env::var` returns `Err` for BOTH
        // "unset" and "not valid Unicode", and only the first is an absence.
        let bad_dev_bin = ResolverInputs {
            home: RootInput::NotSet,
            dev_bin: RootInput::Undetermined("not valid unicode".into()),
            exe_dir: RootInput::NotSet,
        };
        match wenlan_mcp_decision_from(&bad_dev_bin, never_probed, "wenlan-mcp@^9.9.9").0 {
            McpEntryDecision::PreserveExisting { unmeasured } => {
                assert_eq!(unmeasured.undetermined[0].input, "WENLAN_MCP_DEV_BIN");
            }
            other => panic!("an unreadable dev override must not be an absence: {other:?}"),
        }

        // The control, so this is not just "always refuse": when every input is
        // genuinely NOT SET, nothing failed, and `npx` is the right answer.
        let all_absent = ResolverInputs {
            home: RootInput::NotSet,
            dev_bin: RootInput::NotSet,
            exe_dir: RootInput::NotSet,
        };
        assert_eq!(
            written_entry(
                wenlan_mcp_decision_from(&all_absent, never_probed, "wenlan-mcp@^9.9.9").0
            )
            .command,
            "npx",
            "a measured absence must still resolve to the npx entry"
        );
    }

    /// A `Found` must not short-circuit the undetermined inputs out of
    /// existence. It cannot be inferred from the trail either: an undetermined
    /// input builds no candidate path, so a three-row trail beside a chosen
    /// command looks exactly like a complete search. Fixture: `home`
    /// determined, `dev_bin` undetermined, `exe_dir` not set, real file under
    /// `home`.
    #[test]
    fn a_found_binary_still_reports_an_input_that_could_not_be_read() {
        let tmp = tempfile::tempdir().unwrap();
        let installed = install_wenlan_mcp_into(tmp.path());

        let inputs = ResolverInputs {
            home: RootInput::Known(tmp.path().to_path_buf()),
            dev_bin: RootInput::Undetermined("not valid Unicode".to_string()),
            exe_dir: RootInput::NotSet,
        };

        let resolved = resolve_wenlan_mcp_with_trail(&inputs, probe_candidate);
        match &resolved.resolution {
            McpBinaryResolution::Found { path, undetermined } => {
                // Compared as filesystem objects, not as strings: the resolver
                // spells this `<home>\.wenlan/bin/wenlan-mcp.exe` (a literal
                // `/`-joined tail) while the fixture uses platform separators —
                // the same file, two spellings.
                assert_eq!(
                    std::fs::canonicalize(path).unwrap(),
                    std::fs::canonicalize(&installed).unwrap()
                );
                assert_eq!(
                    undetermined.len(),
                    1,
                    "a hit under one input erased the input that could not be read: {undetermined:?}"
                );
                assert_eq!(undetermined[0].input, "WENLAN_MCP_DEV_BIN");
                assert_eq!(undetermined[0].blocked, "WENLAN_MCP_DEV_BIN");
                assert!(undetermined[0].error.contains("not valid Unicode"));
            }
            other => panic!("the fixture must resolve to the installed binary: {other:?}"),
        }

        // The trail cannot stand in for it: `WENLAN_MCP_DEV_BIN` has no
        // candidate row, because no path could be built from it. That is the
        // whole reason the resolution has to carry it separately.
        assert!(
            !resolved
                .trail
                .iter()
                .any(|c| c.source == "WENLAN_MCP_DEV_BIN"),
            "the fixture is not staging the defect if a dev-override candidate was built"
        );

        // And it survives the step that reaches the writers and the wire.
        match wenlan_mcp_decision_from(&inputs, probe_candidate, "wenlan-mcp@^9.9.9").0 {
            McpEntryDecision::Write {
                entry,
                undetermined,
            } => {
                assert_eq!(
                    std::fs::canonicalize(&entry.command).unwrap(),
                    std::fs::canonicalize(&installed).unwrap()
                );
                assert_eq!(
                    undetermined.len(),
                    1,
                    "the decision the writers and the diagnostics wire act on dropped it"
                );
            }
            other => panic!("a measured file must still be written: {other:?}"),
        }

        // The control, so this is not "always report something": with the same
        // find and every input determined, there is nothing to report and the
        // successful search says so.
        let clean = determined_inputs(Some(tmp.path()), None, None);
        match resolve_wenlan_mcp_with_trail(&clean, probe_candidate).resolution {
            McpBinaryResolution::Found { undetermined, .. } => assert!(
                undetermined.is_empty(),
                "a fully determined search must not invent a failure: {undetermined:?}"
            ),
            other => panic!("the control must also find the installed binary: {other:?}"),
        }
    }

    /// The classification itself: `.ok()` on `env::var` and `current_exe()`
    /// erases the difference between "not set" and "could not be read", so the
    /// arms are pinned directly rather than through the host process.
    #[test]
    fn a_read_that_failed_is_not_an_input_that_is_absent() {
        let determined = ResolverInputs::from_reads(
            Some(PathBuf::from("/home/someone")),
            Err(std::env::VarError::NotPresent),
            Ok(PathBuf::from(
                "/Applications/Wenlan.app/Contents/MacOS/wenlan",
            )),
        );
        assert_eq!(determined.dev_bin, RootInput::NotSet);
        assert!(
            determined.undetermined().is_empty(),
            "an unset env var is a measured absence and must not be reported as a failure"
        );

        let failed = ResolverInputs::from_reads(
            None,
            Err(std::env::VarError::NotUnicode("\u{fffd}".into())),
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "current_exe: Access is denied. (os error 5)",
            )),
        );
        let undetermined = failed.undetermined();
        let names: Vec<&str> = undetermined.iter().map(|u| u.input.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "WENLAN_MCP_DEV_BIN",
                "the home directory",
                "the application's own directory",
            ],
            "every read that failed must survive as a failure, not be flattened into 'not set'"
        );
    }

    /// The candidate list is four SLOTS and two of them can name the same FILE
    /// -- `WENLAN_MCP_DEV_BIN` pointed at the installed binary is the ordinary
    /// developer setup. Probing that file twice is two instants, so the trail
    /// could carry the same path as both `file` and `unreadable`, contradicting
    /// itself and the command printed beside it. One reading per filesystem
    /// object; both slots still shown, sharing it.
    #[test]
    fn the_same_file_named_by_two_slots_is_probed_once() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let installed = install_wenlan_mcp_into(home);
        // Two SPELLINGS, one file: the ordinary developer case of an override
        // built by walking a directory back (`$bin/../bin/wenlan-mcp`). It has
        // to be `..` rather than `.`: `Path`'s `Eq` and `Hash` run over
        // `components()`, which DROPS a `CurDir` component, so `a/./b` already
        // hashes equal to `a/b` and would pass against a pathname-keyed map.
        // `ParentDir` components are kept — asserted below, so the fixture
        // cannot decay back into one that could not fail.
        let walked = home
            .join(".wenlan")
            .join("bin")
            .join("..")
            .join("bin")
            .join(installed.file_name().unwrap());
        assert_ne!(
            walked, installed,
            "the fixture must be two SPELLINGS of one file; if these are equal the test is \
             back to measuring pathname equality against itself"
        );

        let probes: std::sync::Mutex<std::collections::HashMap<PathBuf, usize>> =
            std::sync::Mutex::new(std::collections::HashMap::new());
        let counting = |path: &Path| {
            *probes
                .lock()
                .unwrap()
                .entry(path.to_path_buf())
                .or_insert(0) += 1;
            probe_candidate(path)
        };

        let (decision, trail) = wenlan_mcp_decision_for(
            Some(home),
            Some(walked.to_str().unwrap()),
            None,
            counting,
            "wenlan-mcp@^9.9.9",
        );

        let readings_of_the_object = {
            let probes = probes.lock().unwrap();
            probes.get(&installed).copied().unwrap_or(0) + probes.get(&walked).copied().unwrap_or(0)
        };
        assert_eq!(
            readings_of_the_object, 1,
            "one file, named twice, was probed {readings_of_the_object} time(s); two readings of \
             one object is two instants, which is how a trail comes to contradict the command \
             beside it"
        );

        let for_this_file: Vec<&ProbedCandidate> = trail
            .iter()
            .filter(|c| c.path == installed || c.path == walked)
            .collect();
        assert_eq!(
            for_this_file.len(),
            2,
            "both slots must still appear in the trail -- the user should see that the dev \
             override and the installed path are the same file"
        );
        assert_eq!(
            for_this_file[0].state, for_this_file[1].state,
            "one object, one reading: the two slots must carry the SAME state"
        );
        // The dev override still wins the ranking, spelled the way the user
        // spelled it. Sharing a reading must not rewrite anyone's path.
        assert_eq!(written_entry(decision).command, walked.to_str().unwrap());
    }

    /// A zero-byte file is not a program on any operating system. The rule must
    /// reach the resolver, not just the probe: a `NotExecutable` candidate is
    /// measured-unusable, so the search moves on and — with no unreadable
    /// candidate anywhere — ends in the MEASURED `NoneInstalled`, never
    /// `Unresolved`. Nothing failed; the answer is just no.
    #[test]
    fn an_empty_file_is_never_resolved_as_the_binary() {
        let tmp = tempfile::tempdir().unwrap();
        let exe_dir = tmp.path().join("Programs").join("Wenlan");
        std::fs::create_dir_all(&exe_dir).unwrap();
        let empty = installed_wenlan_mcp(&exe_dir);
        std::fs::write(&empty, b"").unwrap();

        match probe_candidate(&empty) {
            CandidateProbe::NotExecutable { reason } => {
                assert!(
                    reason.contains("empty"),
                    "the reason must say what was wrong: {reason}"
                );
            }
            other => panic!("a zero-byte file must not probe as a usable binary: {other:?}"),
        }
        assert_eq!(
            resolve_wenlan_mcp(None, None, Some(&exe_dir), probe_candidate),
            McpBinaryResolution::NoneInstalled,
            "an empty {} was resolved as the wenlan-mcp binary and would be written into a \
             client config as the command to run",
            empty.display()
        );
    }

    /// The Unix half of the same rule, and the one that has a real execute bit
    /// to read. Skipped on Windows on purpose: there IS no execute bit there,
    /// `std::fs::Metadata` exposes nothing equivalent, and a check invented to
    /// make the platforms look symmetrical would be a witness that cannot fail.
    /// What Windows can establish is non-emptiness, which the test above covers;
    /// a non-empty but corrupt `.exe` remains an unfixed residual there.
    #[cfg(unix)]
    #[test]
    fn a_file_with_no_execute_bit_is_never_resolved_as_the_binary() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let exe_dir = tmp.path().join("Programs").join("Wenlan");
        std::fs::create_dir_all(&exe_dir).unwrap();
        let not_executable = installed_wenlan_mcp(&exe_dir);
        std::fs::write(&not_executable, b"MZ\x90\x00 real content\n").unwrap();
        std::fs::set_permissions(&not_executable, std::fs::Permissions::from_mode(0o644)).unwrap();

        match probe_candidate(&not_executable) {
            CandidateProbe::NotExecutable { reason } => {
                assert!(reason.contains("execute"), "reason was: {reason}");
            }
            other => panic!("a non-executable file must not probe as a usable binary: {other:?}"),
        }
        assert_eq!(
            resolve_wenlan_mcp(None, None, Some(&exe_dir), probe_candidate),
            McpBinaryResolution::NoneInstalled
        );
    }

    /// Every probed candidate carries the platform's executable suffix. The
    /// dev override is exempt: it is a full path the developer supplies.
    #[test]
    fn wenlan_mcp_candidates_carry_the_platform_executable_suffix() {
        let home = PathBuf::from("/Users/someone");
        let expected = format!("wenlan-mcp{}", std::env::consts::EXE_SUFFIX);
        for (path, source) in wenlan_mcp_candidate_sources(
            Some(home.as_path()),
            None,
            Some(Path::new("/Applications/Wenlan.app/Contents/MacOS")),
        ) {
            assert_eq!(
                path.file_name().unwrap().to_string_lossy(),
                expected.as_str(),
                "the {source} candidate cannot match a real install on this platform: {}",
                path.display()
            );
        }
    }

    /// The suffix follows the HOST value, not the machine running the test:
    /// Windows gets `.exe` on every candidate except the developer's override
    /// (a full path they supplied), and every other host gets the bare name.
    #[test]
    fn wenlan_mcp_candidate_suffix_follows_the_host_not_the_test_machine() {
        let home = PathBuf::from("/Users/someone");
        let exe_dir = Path::new("/Applications/Wenlan.app/Contents/MacOS");
        for (host, expected) in [
            (HostKind::Windows, "wenlan-mcp.exe"),
            (HostKind::MacOs, "wenlan-mcp"),
            (HostKind::Linux, "wenlan-mcp"),
        ] {
            let sources = wenlan_mcp_candidate_sources_for(
                host,
                Some(home.as_path()),
                Some("/tmp/dev/wenlan-mcp"),
                Some(exe_dir),
            );
            let labels: Vec<&str> = sources.iter().map(|(_, source)| *source).collect();
            assert_eq!(
                labels,
                ["WENLAN_MCP_DEV_BIN", "installed", "bundled", "cargo"],
                "{host:?}"
            );
            for (path, source) in sources {
                if source == "WENLAN_MCP_DEV_BIN" {
                    assert_eq!(path, PathBuf::from("/tmp/dev/wenlan-mcp"), "{host:?}");
                } else {
                    assert_eq!(
                        path.file_name().unwrap().to_string_lossy(),
                        expected,
                        "{host:?} {source}: {}",
                        path.display()
                    );
                }
            }
        }
    }

    /// The bug that broke a real machine: a maintainer's cargo target dir outranked
    /// the installed binary, so the absolute dev path was written into the user's
    /// client config and died on the next `cargo clean`.
    #[test]
    fn wenlan_mcp_candidates_never_probe_a_build_artifact_dir() {
        let home = PathBuf::from("/Users/someone");
        let candidates = wenlan_mcp_candidates(
            Some(home.as_path()),
            None,
            Some(Path::new("/Applications/Wenlan.app/Contents/MacOS")),
        );
        assert!(!candidates.is_empty());
        for candidate in &candidates {
            let path = candidate.to_string_lossy();
            assert!(
                !path.contains("/target/release/") && !path.contains("/target/debug/"),
                "candidate probes a cargo build artifact, which is not an install location: {path}"
            );
            assert!(
                !path.contains("/Repos/"),
                "candidate hardcodes a maintainer's checkout layout: {path}"
            );
        }
    }

    #[test]
    fn wenlan_mcp_candidates_rank_the_installed_binary_first() {
        let home = PathBuf::from("/Users/someone");
        let candidates = wenlan_mcp_candidates(Some(home.as_path()), None, None);
        assert_eq!(
            candidates.first().unwrap(),
            &installed_wenlan_mcp(&home.join(".wenlan/bin"))
        );
        assert!(candidates.contains(&installed_wenlan_mcp(&home.join(".cargo/bin"))));
    }

    #[test]
    fn wenlan_mcp_candidates_let_a_dev_override_win() {
        let home = PathBuf::from("/Users/someone");
        let candidates = wenlan_mcp_candidates(
            Some(home.as_path()),
            Some("/tmp/dev/wenlan-mcp"),
            Some(Path::new("/Applications/Wenlan.app/Contents/MacOS")),
        );
        assert_eq!(candidates[0], PathBuf::from("/tmp/dev/wenlan-mcp"));
        assert_eq!(
            candidates[1],
            installed_wenlan_mcp(&home.join(".wenlan/bin"))
        );
        assert_eq!(
            candidates[2],
            installed_wenlan_mcp(Path::new("/Applications/Wenlan.app/Contents/MacOS"))
        );
    }

    #[test]
    fn wenlan_mcp_candidates_survive_a_missing_home_and_empty_override() {
        let candidates = wenlan_mcp_candidates(
            None,
            Some("   "),
            Some(Path::new("/Applications/Wenlan.app/Contents/MacOS")),
        );
        assert_eq!(
            candidates,
            vec![installed_wenlan_mcp(Path::new(
                "/Applications/Wenlan.app/Contents/MacOS"
            ))]
        );
    }

    #[test]
    fn pinned_wenlan_mcp_package_tracks_the_backend_pin_file() {
        assert_eq!(
            pinned_wenlan_mcp_package("v0.13.0\ndeadbeef\n"),
            "wenlan-mcp@^0.13.0"
        );
        assert_eq!(pinned_wenlan_mcp_package("0.12.0"), "wenlan-mcp@^0.12.0");
    }

    #[test]
    fn pinned_wenlan_mcp_package_falls_back_when_the_pin_is_unparseable() {
        assert_eq!(pinned_wenlan_mcp_package(""), "wenlan-mcp");
        assert_eq!(pinned_wenlan_mcp_package("latest\n"), "wenlan-mcp");
    }

    /// The npx fallback must carry the version this app was built against, or a
    /// `.dmg`-only user silently gets whatever backend npm serves today.
    #[test]
    fn npx_fallback_is_pinned_to_the_shipped_backend_version() {
        let entry = written_entry(wenlan_mcp_entry_for(
            McpBinaryResolution::NoneInstalled,
            &pinned_wenlan_mcp_package(BACKEND_VERSION_PIN),
        ));
        assert_eq!(entry.command, "npx");
        assert_eq!(entry.args[0], "-y");
        assert!(
            entry.args[1].starts_with("wenlan-mcp@^"),
            "npx fallback is unpinned: {}",
            entry.args[1]
        );
        assert!(
            entry.args[1]
                .trim_start_matches("wenlan-mcp@^")
                .starts_with(|c: char| c.is_ascii_digit()),
            "npx fallback carries no version: {}",
            entry.args[1]
        );
    }

    #[test]
    fn wenlan_mcp_entry_prefers_a_found_binary_over_npx() {
        let entry = written_entry(wenlan_mcp_entry_for(
            McpBinaryResolution::Found {
                path: PathBuf::from("/Users/someone/.wenlan/bin/wenlan-mcp"),
                undetermined: Vec::new(),
            },
            "wenlan-mcp@^9.9.9",
        ));
        assert_eq!(entry.command, "/Users/someone/.wenlan/bin/wenlan-mcp");
        assert!(entry.args.is_empty());
    }

    /// At the decision: `Unresolved` must be "write nothing", not the `npx`
    /// entry `NoneInstalled` produces. `npx` is not a safe default — it needs
    /// Node and a network, and the user whose local binary was momentarily
    /// unstatable has neither guaranteed.
    #[test]
    fn an_unresolved_search_writes_nothing_at_all() {
        let denied = PathBuf::from("/Users/someone/.wenlan/bin/wenlan-mcp");
        let unresolved = McpBinaryResolution::Unresolved(Unmeasured {
            unreadable: vec![(denied.clone(), "Access is denied. (os error 5)".to_string())],
            undetermined: Vec::new(),
        });
        assert_ne!(unresolved, McpBinaryResolution::NoneInstalled);

        match wenlan_mcp_entry_for(unresolved, "wenlan-mcp@^9.9.9") {
            McpEntryDecision::PreserveExisting { unmeasured } => {
                assert_eq!(unmeasured.unreadable.len(), 1);
                assert_eq!(unmeasured.unreadable[0].0, denied);
                let message = unresolved_message(&unmeasured);
                assert!(
                    message.contains("os error 5") && message.contains("unchanged"),
                    "the user-facing message must name the failure and say nothing changed: \
                     {message}"
                );
            }
            McpEntryDecision::Write { entry, .. } => panic!(
                "a candidate that could not be LOOKED AT produced a written entry `{} {}` — a \
                 measured-absence outcome manufactured from a failed measurement",
                entry.command,
                entry.args.join(" ")
            ),
        }
    }

    /// At the mutation: a user HAS a local `wenlan-mcp` in a client config and
    /// the candidate is momentarily unstatable (ACL, antivirus, disconnected
    /// network path). The file must come back BYTE-IDENTICAL, and no `.bak` may
    /// be left either — a backup implies a change happened.
    #[test]
    fn an_unreadable_candidate_leaves_an_existing_config_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let unresolved = || McpEntryDecision::PreserveExisting {
            unmeasured: Unmeasured {
                unreadable: vec![(
                    PathBuf::from("/Users/someone/.wenlan/bin/wenlan-mcp"),
                    "Access is denied. (os error 5)".to_string(),
                )],
                undetermined: Vec::new(),
            },
        };

        let config_path = tmp.path().join("config.json");
        let existing = "{\n  \"mcpServers\": {\n    \"wenlan\": {\n      \"command\": \
                        \"/opt/wenlan/bin/wenlan-mcp\",\n      \"args\": []\n    }\n  }\n}\n";
        std::fs::write(&config_path, existing).unwrap();

        let err = write_wenlan_entry_with(&config_path, false, unresolved())
            .expect_err("an unresolvable binary must not silently rewrite a client config");
        assert!(
            err.to_string().contains("unchanged"),
            "the error must tell the user nothing was written: {err}"
        );
        assert_eq!(
            std::fs::read_to_string(&config_path).unwrap(),
            existing,
            "the user's working local command was overwritten with a guess"
        );
        assert!(
            !config_path.with_extension("json.bak").exists(),
            "a backup was written for a change that never happened"
        );

        // Same rule for the Codex TOML writer.
        let toml_path = tmp.path().join("config.toml");
        let existing_toml = "# hand-written\n[mcp_servers.wenlan]\ncommand = \
                             \"/opt/wenlan/bin/wenlan-mcp\"\nargs = []\n";
        std::fs::write(&toml_path, existing_toml).unwrap();
        write_wenlan_entry_toml_with(&toml_path, unresolved())
            .expect_err("same rule for the TOML writer");
        assert_eq!(std::fs::read_to_string(&toml_path).unwrap(), existing_toml);
        assert!(!toml_path.with_extension("toml.bak").exists());

        // A config file that does NOT exist yet must not be created either:
        // "write nothing" has to mean nothing, not an empty skeleton.
        let fresh = tmp.path().join("fresh.json");
        write_wenlan_entry_with(&fresh, false, unresolved()).unwrap_err();
        assert!(
            !fresh.exists(),
            "an unresolvable search created a config file"
        );
    }

    /// A decision that is definitely `Write`, so these tests measure the
    /// WRITER's ordering rather than whatever binary the host running them
    /// happens to have installed.
    #[cfg(test)]
    fn staged_write() -> McpEntryDecision {
        McpEntryDecision::Write {
            entry: WenlanMcpEntry {
                command: "/opt/wenlan/bin/wenlan-mcp".to_string(),
                args: Vec::new(),
            },
            undetermined: Vec::new(),
        }
    }

    /// `config.json` is corrupted (a crash mid-write, a bad hand-edit) and
    /// `config.json.bak` still holds the last GOOD configuration. Backing up
    /// before parsing lands the malformed bytes on top of the good ones and
    /// then returns `Invalid JSON`, destroying the recovery copy at the one
    /// moment it mattered. The assertion that catches that is on the BACKUP's
    /// bytes; `is_err()` is the same either way.
    #[test]
    fn a_malformed_config_does_not_overwrite_the_last_good_backup() {
        let tmp = tempfile::tempdir().unwrap();

        let config_path = tmp.path().join("config.json");
        let backup_path = tmp.path().join("config.json.bak");
        let good = "{\n  \"mcpServers\": {\n    \"wenlan\": {\n      \"command\": \
                    \"/opt/wenlan/bin/wenlan-mcp\",\n      \"args\": []\n    }\n  }\n}\n";
        let malformed = "{\n  \"mcpServers\": {\n    \"wenlan\": {\n  <<<< truncated";
        std::fs::write(&backup_path, good).unwrap();
        std::fs::write(&config_path, malformed).unwrap();

        let err = write_wenlan_entry_with(&config_path, false, staged_write())
            .expect_err("a malformed config must still be reported as one");
        assert!(
            err.to_string().contains("Invalid JSON"),
            "the failure must still name what could not be parsed: {err}"
        );
        assert_eq!(
            std::fs::read_to_string(&backup_path).unwrap(),
            good,
            "the last good backup was overwritten with the malformed file — the user's only \
             recoverable copy, destroyed by the failure that was about to be reported"
        );
        // And the broken file itself is left exactly as found: nothing was
        // written anywhere.
        assert_eq!(std::fs::read_to_string(&config_path).unwrap(), malformed);
    }

    /// The same rule in the Codex TOML writer — a fix to one of two identical
    /// orderings is half a fix.
    #[test]
    fn a_malformed_toml_config_does_not_overwrite_the_last_good_backup() {
        let tmp = tempfile::tempdir().unwrap();

        let config_path = tmp.path().join("config.toml");
        let backup_path = tmp.path().join("config.toml.bak");
        let good = "# hand-written\n[mcp_servers.wenlan]\ncommand = \
                    \"/opt/wenlan/bin/wenlan-mcp\"\nargs = []\n";
        let malformed = "[mcp_servers.wenlan\ncommand = ";
        std::fs::write(&backup_path, good).unwrap();
        std::fs::write(&config_path, malformed).unwrap();

        let err = write_wenlan_entry_toml_with(&config_path, staged_write())
            .expect_err("a malformed config must still be reported as one");
        assert!(
            err.to_string().contains("Invalid TOML"),
            "the failure must still name what could not be parsed: {err}"
        );
        assert_eq!(
            std::fs::read_to_string(&backup_path).unwrap(),
            good,
            "the last good backup was overwritten with the malformed file"
        );
        assert_eq!(std::fs::read_to_string(&config_path).unwrap(), malformed);
    }

    /// The other half of the ordering: a config that DOES parse must still be
    /// backed up. Without this, "fix the ordering" could be satisfied by never
    /// writing a backup at all, and the two tests above would still pass.
    #[test]
    fn a_config_that_parses_is_still_backed_up_before_it_is_rewritten() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("config.json");
        let original = "{\"mcpServers\":{\"other\":{\"command\":\"other-cmd\"}}}";
        std::fs::write(&config_path, original).unwrap();

        write_wenlan_entry_with(&config_path, false, staged_write()).unwrap();

        assert_eq!(
            std::fs::read_to_string(config_path.with_extension("json.bak")).unwrap(),
            original,
            "the pre-change bytes must be recoverable after a successful write"
        );

        let toml_path = tmp.path().join("config.toml");
        let original_toml = "model = \"gpt-5.5\"\n";
        std::fs::write(&toml_path, original_toml).unwrap();
        write_wenlan_entry_toml_with(&toml_path, staged_write()).unwrap();
        assert_eq!(
            std::fs::read_to_string(toml_path.with_extension("toml.bak")).unwrap(),
            original_toml
        );
    }

    #[test]
    #[serial_test::serial]
    fn test_write_wenlan_entry_creates_new_file() {
        let _env = EnvGuard::capture(&[MCP_RESOLVER_HOME_ENV, "WENLAN_MCP_DEV_BIN"]);
        let tmp = tempfile::tempdir().unwrap();
        // Stand up the install the resolver is supposed to find, so the written
        // command is decided by the fixture rather than by this host.
        let installed = install_wenlan_mcp_into(tmp.path());
        std::env::remove_var("WENLAN_MCP_DEV_BIN");
        std::env::set_var(MCP_RESOLVER_HOME_ENV, tmp.path());

        let config_path = tmp.path().join("config.json");
        write_wenlan_entry(&config_path, "cursor").unwrap();
        let contents = std::fs::read_to_string(&config_path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&contents).unwrap();
        // `.entry`, not the whole report: `undetermined` is for the caller and
        // must never reach the user's config file, whose entry shape stays
        // exactly `{command, args}`. The args name the client.
        let mut expected = wenlan_mcp_entry().unwrap().entry;
        expected
            .args
            .extend(["--agent-name".into(), "cursor".into()]);
        assert_eq!(
            parsed["mcpServers"]["wenlan"],
            serde_json::to_value(expected).unwrap()
        );
        let cmd = parsed["mcpServers"]["wenlan"]["command"].as_str().unwrap();
        assert_eq!(
            Path::new(cmd),
            installed,
            "wrote {cmd} into the client config while {} was installed",
            installed.display()
        );
        assert!(parsed["mcpServers"]["origin"].is_null());
    }

    /// The entry is added and nothing else in the file moves: a sibling server
    /// and an unrelated top-level key. `mcpServers` is created when it is not
    /// there.
    #[test]
    fn test_write_wenlan_entry_preserves_everything_else() {
        let tmp = tempfile::tempdir().unwrap();
        for (i, (existing, pointer, preserved)) in [
            (
                r#"{"mcpServers": {"other": {"command": "other-cmd"}}}"#,
                "/mcpServers/other/command",
                serde_json::json!("other-cmd"),
            ),
            (r#"{"theme": "dark"}"#, "/theme", serde_json::json!("dark")),
        ]
        .into_iter()
        .enumerate()
        {
            let config_path = tmp.path().join(format!("config{i}.json"));
            std::fs::write(&config_path, existing).unwrap();
            write_wenlan_entry(&config_path, "cursor").unwrap();
            let parsed: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
            assert_eq!(parsed.pointer(pointer), Some(&preserved), "{existing}");
            assert!(parsed["mcpServers"]["wenlan"].is_object(), "{existing}");
        }
    }

    /// Each client's entry carries that client's name, so a search it runs is
    /// recorded as its own and not as the stdio default (`claude-code`).
    #[test]
    fn test_write_wenlan_entry_names_the_client() {
        let tmp = tempfile::tempdir().unwrap();
        for (client_type, name) in [
            ("cursor", "cursor"),
            ("gemini_cli", "gemini-cli"),
            ("claude_desktop", "claude-desktop"),
            ("claude_code", "claude-code"),
        ] {
            let config_path = tmp.path().join(format!("{client_type}.json"));
            std::fs::write(&config_path, "{}").unwrap();
            write_wenlan_entry(&config_path, client_type).unwrap();
            let parsed: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
            let args: Vec<&str> = parsed["mcpServers"]["wenlan"]["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|a| a.as_str().unwrap())
                .collect();
            assert_eq!(
                args[args.len() - 2..],
                ["--agent-name", name],
                "{client_type}"
            );
        }

        let toml_path = tmp.path().join("config.toml");
        write_wenlan_entry_toml(&toml_path).unwrap();
        let parsed: toml::Value =
            toml::from_str(&std::fs::read_to_string(&toml_path).unwrap()).unwrap();
        let args = parsed["mcp_servers"]["wenlan"]["args"].as_array().unwrap();
        assert_eq!(args[args.len() - 2].as_str(), Some("--agent-name"));
        assert_eq!(args[args.len() - 1].as_str(), Some("codex"));
    }

    /// Writing `wenlan` replaces a legacy `origin` entry instead of leaving it
    /// beside the new one: a repair of a broken `origin` must not report
    /// success while the broken launch is still configured.
    #[test]
    fn test_write_wenlan_entry_replaces_the_legacy_origin_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("config.json");
        std::fs::write(
            &config_path,
            r#"{"mcpServers": {"origin": {"command": "/gone/origin-mcp"}, "other": {"command": "x"}}}"#,
        )
        .unwrap();
        write_wenlan_entry(&config_path, "cursor").unwrap();
        let contents = std::fs::read_to_string(&config_path).unwrap();
        let json: serde_json::Value = serde_json::from_str(&contents).unwrap();
        assert!(json["mcpServers"]["origin"].is_null(), "{contents}");
        assert!(json["mcpServers"]["wenlan"].is_object());
        assert_eq!(json["mcpServers"]["other"]["command"], "x");
        assert!(!parsed(has_both_raw_entries(&contents)));

        let toml_path = tmp.path().join("config.toml");
        std::fs::write(
            &toml_path,
            "[mcp_servers.origin]\ncommand = \"/gone/origin-mcp\"\n\n[mcp_servers.other]\ncommand = \"x\"\n",
        )
        .unwrap();
        write_wenlan_entry_toml(&toml_path).unwrap();
        let contents = std::fs::read_to_string(&toml_path).unwrap();
        let doc: toml::Value = toml::from_str(&contents).unwrap();
        assert!(doc["mcp_servers"].get("origin").is_none(), "{contents}");
        assert!(doc["mcp_servers"].get("wenlan").is_some());
        assert_eq!(doc["mcp_servers"]["other"]["command"].as_str(), Some("x"));
    }

    #[test]
    fn test_write_wenlan_entry_creates_backup() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("config.json");
        std::fs::write(&config_path, r#"{"original": true}"#).unwrap();
        write_wenlan_entry(&config_path, "cursor").unwrap();
        let backup = tmp.path().join("config.json.bak");
        assert!(backup.exists());
        let backup_contents = std::fs::read_to_string(&backup).unwrap();
        assert!(backup_contents.contains("original"));

        // The Codex TOML writer takes the same backup.
        let toml_path = tmp.path().join("config.toml");
        std::fs::write(&toml_path, "model = \"gpt-5.5\"\n").unwrap();
        write_wenlan_entry_toml(&toml_path).unwrap();
        assert!(std::fs::read_to_string(tmp.path().join("config.toml.bak"))
            .unwrap()
            .contains("gpt-5.5"));
    }

    #[test]
    fn test_write_wenlan_entry_errors_on_an_unparseable_config() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("config.json");
        std::fs::write(&config_path, "not valid json").unwrap();
        assert!(write_wenlan_entry(&config_path, "cursor").is_err());

        let toml_path = tmp.path().join("config.toml");
        std::fs::write(&toml_path, "not toml [").unwrap();
        assert!(write_wenlan_entry_toml(&toml_path).is_err());
    }

    #[test]
    fn test_write_wenlan_entry_refuses_create_for_claude_code() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("claude.json");
        // is_claude_code = true, file doesn't exist → should error
        let result = write_wenlan_entry(&config_path, "claude_code");
        assert!(result.is_err());
    }

    /// `exists` that answers true for exactly one path — so a test failure
    /// means the probed path is wrong, not merely that some boolean was false.
    fn only(hit: &str) -> impl Fn(&Path) -> Reading + '_ {
        move |p: &Path| Reading::of(p == Path::new(hit))
    }

    /// Both ChatGPT desktop bundle locations count. A typo in either candidate
    /// path fails here rather than surfacing as a missing `codex_cli` row.
    #[test]
    fn codex_cli_detected_finds_chatgpt_in_either_applications_dir() {
        let home = PathBuf::from("/Users/someone");
        for bundle in [
            "/Applications/ChatGPT.app",
            "/Users/someone/Applications/ChatGPT.app",
        ] {
            assert!(
                yes(codex_cli_detected(Reading::No, Some(&home), only(bundle))),
                "{bundle} was not probed"
            );
        }
    }

    #[test]
    fn codex_cli_detected_via_config_when_chatgpt_absent() {
        let home = PathBuf::from("/Users/someone");
        assert!(yes(codex_cli_detected(Reading::Yes, Some(&home), |_| {
            Reading::No
        })));
    }

    #[test]
    fn codex_cli_not_detected_when_neither_present() {
        let home = PathBuf::from("/Users/someone");
        assert!(!yes(codex_cli_detected(Reading::No, Some(&home), |_| {
            Reading::No
        })));
        // A *different* Mac app must not be mistaken for ChatGPT desktop.
        assert!(!yes(codex_cli_detected(
            Reading::No,
            Some(&home),
            only("/Applications/Cursor.app")
        )));
    }

    #[test]
    fn codex_cli_detected_survives_missing_home() {
        assert!(yes(codex_cli_detected(
            Reading::No,
            None,
            only("/Applications/ChatGPT.app")
        )));
    }

    #[test]
    fn test_detect_mcp_clients_has_exactly_one_codex_cli_row() {
        // ChatGPT desktop shares ~/.codex/config.toml with Codex CLI — it
        // must fold into the existing codex_cli row, never add a second row.
        let tmp = tempfile::tempdir().unwrap();
        let codex_rows: Vec<_> =
            detect_mcp_clients_from(Some(tmp.path()), Some(tmp.path()), |_| Reading::No)
                .into_iter()
                .filter(|c| c.client_type == "codex_cli")
                .collect();
        assert_eq!(
            codex_rows.len(),
            1,
            "ChatGPT.app detection must reuse the codex_cli row, not add a second one"
        );
    }

    // ── Tri-state client detection ───────────────────────────────────────

    /// A path that could not be built is not a client that is absent. Both
    /// halves are pinned: the home-based clients report the failure, and
    /// `claude_desktop` — whose path is built from the CONFIG dir and never
    /// touches `home` — is unaffected by it.
    #[test]
    fn a_home_that_could_not_be_determined_is_not_a_missing_client() {
        let config_dir = PathBuf::from("/tmp/fixture-config");
        for client_type in [
            "cursor",
            "claude_code",
            "gemini_cli",
            "codex_cli",
            "muse_code",
            "antigravity",
            "copilot_cli",
            "windsurf",
            "kiro",
            "opencode",
        ] {
            match client_config_path_for(client_type, None, Some(&config_dir)) {
                ClientConfigPath::Undetermined(why) => assert!(
                    why.contains("home directory"),
                    "the reason must name what could not be determined: {why}"
                ),
                other => panic!("{client_type} reported {other:?} for an unreadable home"),
            }
        }
        // The coupling the single `?` created: Claude Desktop's path does not
        // use `home` at all, and must not fail with it.
        assert_eq!(
            client_config_path_for("claude_desktop", None, Some(&config_dir)),
            ClientConfigPath::Known(config_dir.join("Claude").join("claude_desktop_config.json")),
        );
    }

    /// With no home and no config dir, an EMPTY VECTOR renders as "no MCP
    /// client detected" in the Diagnostics card and as nothing to set up in the
    /// wizard — a lookup that failed, stated as a fact about the machine.
    #[test]
    fn a_search_that_could_not_look_is_never_an_empty_client_list() {
        let clients = detect_mcp_clients_from(None, None, |_| Reading::No);
        assert_eq!(
            clients.len(),
            12,
            "a client whose path could not be built must still be a row that says so"
        );
        for client in &clients {
            assert!(
                matches!(client.detected, Reading::Unreadable { .. }),
                "{} reported {:?} when nothing could be looked at",
                client.client_type,
                client.detected
            );
            assert!(
                matches!(client.already_configured, Reading::Unreadable { .. }),
                "{} claimed a configuration state it never read",
                client.client_type
            );
            assert!(
                client.config_path.is_none(),
                "there is no path to show when the directory could not be determined"
            );
        }
    }

    /// A `~/.claude/settings.json` the OS will not hand over must not read as a
    /// readable settings file with the plugin switched OFF, or the user is
    /// invited into the double registration this app has a warning box for.
    ///
    /// Staged as a DIRECTORY at the settings path: the one read failure that
    /// reproduces on every platform (Windows `Access is denied`, Unix
    /// `EISDIR`). A chmod-based fixture is a no-op on Windows, which is where
    /// this app most needs the distinction.
    #[test]
    fn a_settings_file_that_could_not_be_read_is_not_a_plugin_that_is_off() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();

        // Control first, so the fixture is known to be able to say "no".
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::write(home.join(".claude").join("settings.json"), "{}").unwrap();
        assert_eq!(
            claude_code_plugin_enabled_on_disk(Some(home)),
            Reading::No,
            "a readable settings file with no plugin is a measured no"
        );

        std::fs::remove_file(home.join(".claude").join("settings.json")).unwrap();
        std::fs::create_dir_all(home.join(".claude").join("settings.json")).unwrap();
        match claude_code_plugin_enabled_on_disk(Some(home)) {
            Reading::Unreadable { error } => assert!(!error.is_empty()),
            other => panic!("a settings file that could not be read reported {other:?}"),
        }

        // And a home that could not be determined is not a plugin that is off
        // either: the `dirs::home_dir()` guard must not `return false`.
        match claude_code_plugin_enabled_on_disk(None) {
            Reading::Unreadable { error } => assert!(error.contains("home directory")),
            other => panic!("an undetermined home reported {other:?}"),
        }
    }

    /// The same collapse on the client's own config file, end to end through
    /// `detect_mcp_clients_from`: a `~/.gemini/settings.json` that cannot be
    /// read must not report exactly like one with no Wenlan entry.
    #[test]
    fn a_config_that_could_not_be_read_is_not_a_config_without_an_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        std::fs::create_dir_all(home.join(".gemini").join("settings.json")).unwrap();

        let clients = detect_mcp_clients_from(Some(home), Some(home), |_| Reading::No);
        let gemini = clients
            .iter()
            .find(|c| c.client_type == "gemini_cli")
            .expect("the gemini_cli row is always present");
        assert!(
            matches!(gemini.has_raw_entry, Reading::Unreadable { .. }),
            "an unreadable config reported {:?}",
            gemini.has_raw_entry
        );
        assert!(matches!(gemini.detected, Reading::Unreadable { .. }));

        // The control: a readable config with a real entry is a measured yes,
        // and a readable config without one is a measured no.
        let cursor_dir = home.join(".cursor");
        std::fs::create_dir_all(&cursor_dir).unwrap();
        std::fs::write(
            cursor_dir.join("mcp.json"),
            r#"{"mcpServers": {"wenlan": {"command": "wenlan-mcp"}}}"#,
        )
        .unwrap();
        let clients = detect_mcp_clients_from(Some(home), Some(home), |_| Reading::No);
        let cursor = clients
            .iter()
            .find(|c| c.client_type == "cursor")
            .expect("the cursor row is always present");
        assert_eq!(cursor.has_raw_entry, Reading::Yes);
        assert_eq!(cursor.already_configured, Reading::Yes);
        assert_eq!(cursor.has_plugin, Reading::No);
        // The injected probe says there is no Cursor app, so the config file is
        // all that is here: still `detected` (the superset), but reported as
        // what it is rather than as an installed client.
        assert_eq!(cursor.install_state, InstallState::ConfigOnly);
        assert_eq!(cursor.detected, Reading::Yes);
    }

    /// `Reading::or` is the OR that `already_configured` is built from, and the
    /// ranking is the whole content: a failed read must not be able to turn a
    /// measured yes into a no, and two halves that are "no" and "unread" are
    /// unread, not "no".
    #[test]
    fn or_never_lets_a_failed_read_outrank_a_measurement() {
        let unreadable = || Reading::Unreadable {
            error: "Access is denied. (os error 5)".to_string(),
        };
        assert_eq!(Reading::Yes.or(unreadable()), Reading::Yes);
        assert_eq!(unreadable().or(Reading::Yes), Reading::Yes);
        assert!(matches!(
            Reading::No.or(unreadable()),
            Reading::Unreadable { .. }
        ));
        assert!(matches!(
            unreadable().or(Reading::No),
            Reading::Unreadable { .. }
        ));
        assert_eq!(Reading::No.or(Reading::No), Reading::No);
        assert_eq!(Reading::Yes.or(Reading::No), Reading::Yes);
    }

    /// `read_config` is the replacement for `Path::exists()` +
    /// `read_to_string(..).unwrap_or(false)`. Only `NotFound` is an absence.
    #[test]
    fn read_config_separates_absence_from_a_failed_look() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("there.json"), "{}").unwrap();

        assert_eq!(
            read_config(&tmp.path().join("there.json")).present(),
            Reading::Yes
        );
        assert_eq!(
            read_config(&tmp.path().join("nope.json")).present(),
            Reading::No
        );
        // A directory where a config should be: measured, and not an absence.
        assert!(matches!(
            read_config(tmp.path()).present(),
            Reading::Unreadable { .. }
        ));
    }

    #[test]
    fn test_detect_includes_new_clients() {
        let tmp = tempfile::tempdir().unwrap();
        let types: Vec<String> =
            detect_mcp_clients_from(Some(tmp.path()), Some(tmp.path()), |_| Reading::No)
                .into_iter()
                .map(|c| c.client_type)
                .collect();
        for expected in [
            "cursor",
            "claude_code",
            "claude_desktop",
            "gemini_cli",
            "codex_cli",
            "muse_code",
            "antigravity",
            "copilot_cli",
            "vscode",
            "windsurf",
            "kiro",
            "opencode",
        ] {
            assert!(types.contains(&expected.to_string()), "missing {expected}");
        }
    }

    #[test]
    fn test_has_configured_entry_toml() {
        assert!(parsed(has_configured_entry_toml(
            "[mcp_servers.wenlan]\ncommand = \"npx\"\nargs = [\"-y\", \"wenlan-mcp\"]\n"
        )));
        assert!(parsed(has_configured_entry_toml(
            "[mcp_servers.origin]\ncommand = \"npx\"\n"
        )));
        assert!(!parsed(has_configured_entry_toml(
            "[mcp_servers.other]\ncommand = \"x\"\n"
        )));
        assert!(!parsed(has_configured_entry_toml("model = \"gpt-5.5\"\n")));
        assert!(unparseable(has_configured_entry_toml("not toml [")).contains("not valid TOML"));
    }

    #[test]
    fn test_write_wenlan_entry_toml_creates_new_file() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("config.toml");
        write_wenlan_entry_toml(&config_path).unwrap();
        let contents = std::fs::read_to_string(&config_path).unwrap();
        assert!(parsed(has_configured_entry_toml(&contents)));
        let parsed: toml::Value = toml::from_str(&contents).unwrap();
        let wenlan = &parsed["mcp_servers"]["wenlan"];
        assert!(wenlan.get("command").is_some());
    }

    #[test]
    fn test_write_wenlan_entry_toml_preserves_formatting_byte_for_byte() {
        // Council change (d): a user's hand-edited config must survive the
        // upsert byte-for-byte — comments, spacing, key order, other tables.
        let fixture = r#"# my codex config — do not touch
model = "gpt-5.5"   # inline comment

[profiles.fast]
model   = "gpt-5.5-mini"

[mcp_servers.other]
command = "other-cmd"  # keep me
args = ["--flag"]
"#;
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("config.toml");
        std::fs::write(&config_path, fixture).unwrap();
        write_wenlan_entry_toml(&config_path).unwrap();
        let contents = std::fs::read_to_string(&config_path).unwrap();
        // Everything that existed before is preserved verbatim; the wenlan
        // table is appended after it.
        assert!(
            contents.starts_with(fixture),
            "existing content was reformatted:\n{contents}"
        );
        assert!(parsed(has_configured_entry_toml(&contents)));
    }

    // Serial with a pinned resolver home, because both writes have to resolve
    // the *same* binary: `MCP_RESOLVER_HOME_ENV` is process-global, and a
    // concurrent serial test changing it between the two writes made this
    // compare an installed path against the `npx` fallback and fail on a
    // difference that is not the upsert's.
    #[test]
    #[serial_test::serial]
    fn test_write_wenlan_entry_toml_upsert_is_idempotent() {
        let _env = EnvGuard::capture(&[MCP_RESOLVER_HOME_ENV, "WENLAN_MCP_DEV_BIN"]);
        let home = tempfile::tempdir().unwrap();
        std::env::remove_var("WENLAN_MCP_DEV_BIN");
        std::env::set_var(MCP_RESOLVER_HOME_ENV, home.path());
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("config.toml");
        write_wenlan_entry_toml(&config_path).unwrap();
        let first = std::fs::read_to_string(&config_path).unwrap();
        write_wenlan_entry_toml(&config_path).unwrap();
        let second = std::fs::read_to_string(&config_path).unwrap();
        assert_eq!(first, second);
    }

    // ── remove_wenlan_entry (JSON) ──────────────────────────────────────

    #[test]
    fn test_remove_wenlan_entry_removes_only_the_wenlan_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("config.json");
        let existing =
            r#"{"mcpServers": {"wenlan": {"command": "npx"}, "other": {"command": "other-cmd"}}}"#;
        std::fs::write(&config_path, existing).unwrap();

        remove_wenlan_entry(&config_path).unwrap();

        let parsed: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
        assert!(parsed["mcpServers"]["wenlan"].is_null());
        // The sibling server survives untouched.
        assert_eq!(parsed["mcpServers"]["other"]["command"], "other-cmd");
    }

    /// Both keys the verb recognizes come out, and everything else — an
    /// unrelated top-level key included — stays.
    #[test]
    fn test_remove_wenlan_entry_removes_both_keys_and_keeps_the_rest() {
        let tmp = tempfile::tempdir().unwrap();
        for (i, (existing, gone)) in [
            (
                r#"{"theme": "dark", "mcpServers": {"wenlan": {"command": "npx"}}}"#,
                "wenlan",
            ),
            (
                r#"{"theme": "dark", "mcpServers": {"origin": {"command": "npx", "args": ["-y", "origin-mcp"]}}}"#,
                "origin",
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let config_path = tmp.path().join(format!("config{i}.json"));
            std::fs::write(&config_path, existing).unwrap();
            remove_wenlan_entry(&config_path).unwrap();
            let parsed: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
            assert!(parsed["mcpServers"][gone].is_null(), "{existing}");
            assert_eq!(parsed["theme"], "dark", "{existing}");
        }
    }

    /// Nothing to remove is an `Err`, for both formats and for both shapes of
    /// nothing — and the no-op error path leaves no stray `.bak` behind.
    #[test]
    fn test_remove_wenlan_entry_errs_when_there_is_nothing_to_remove() {
        let tmp = tempfile::tempdir().unwrap();
        type Remove = fn(&std::path::Path) -> Result<(), AppError>;
        for (remove, name, no_entry, bak) in [
            (
                remove_wenlan_entry as Remove,
                "config.json",
                r#"{"mcpServers": {"other": {}}}"#,
                "json.bak",
            ),
            (
                remove_wenlan_entry_toml as Remove,
                "config.toml",
                "model = \"gpt-5.5\"\n",
                "toml.bak",
            ),
        ] {
            let config_path = tmp.path().join(name);
            std::fs::write(&config_path, no_entry).unwrap();
            assert!(remove(&config_path).is_err(), "{name}");
            assert!(!config_path.with_extension(bak).exists(), "{name}");

            let missing = tmp.path().join(format!("does-not-exist-{name}"));
            assert!(remove(&missing).is_err(), "{name}");
        }
    }

    /// Removal is symmetric with detection: the written file still parses and
    /// `client_config_has_raw_entry` no longer sees an entry.
    #[test]
    fn test_remove_wenlan_entry_leaves_client_config_has_raw_entry_false() {
        let tmp = tempfile::tempdir().unwrap();
        type Remove = fn(&std::path::Path) -> Result<(), AppError>;
        for (remove, client, name, existing) in [
            (
                remove_wenlan_entry as Remove,
                "cursor",
                "config.json",
                r#"{"mcpServers": {"wenlan": {"command": "npx"}, "other": {"command": "x"}}}"#,
            ),
            (
                remove_wenlan_entry_toml as Remove,
                "codex_cli",
                "config.toml",
                "[mcp_servers.wenlan]\ncommand = \"npx\"\nargs = [\"-y\", \"wenlan-mcp\"]\n",
            ),
        ] {
            let config_path = tmp.path().join(name);
            std::fs::write(&config_path, existing).unwrap();
            assert!(yes(client_config_has_raw_entry(client, &config_path)));
            remove(&config_path).unwrap();
            assert!(!yes(client_config_has_raw_entry(client, &config_path)));
        }
    }

    // ── remove_wenlan_entry_toml (Codex CLI) ────────────────────────────

    #[test]
    fn test_remove_wenlan_entry_toml_removes_only_the_wenlan_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("config.toml");
        let fixture = r#"# my codex config
model = "gpt-5.5"

[mcp_servers.other]
command = "other-cmd"

[mcp_servers.wenlan]
command = "npx"
args = ["-y", "wenlan-mcp"]
"#;
        std::fs::write(&config_path, fixture).unwrap();

        remove_wenlan_entry_toml(&config_path).unwrap();

        let contents = std::fs::read_to_string(&config_path).unwrap();
        // The wenlan entry is gone; the sibling server and unrelated keys stay.
        assert!(!parsed(has_configured_entry_toml(&contents)));
        let parsed: toml::Value = toml::from_str(&contents).unwrap();
        assert_eq!(parsed["model"], toml::Value::from("gpt-5.5"));
        assert_eq!(
            parsed["mcp_servers"]["other"]["command"],
            toml::Value::from("other-cmd")
        );
        assert!(parsed["mcp_servers"].get("wenlan").is_none());
    }

    // ── has_both_raw_entries (raw+raw duplicate detection) ──────────────

    #[test]
    fn test_has_both_raw_entries() {
        // The first fixture is the real ~/.cursor/mcp.json duplicate shape.
        assert!(parsed(has_both_raw_entries(
            r#"{"mcpServers": {
            "origin": {"command": "npx", "args": ["-y", "origin-mcp"]},
            "wenlan": {"command": "npx", "args": ["-y", "wenlan-mcp"]}
        }}"#
        )));
        assert!(!parsed(has_both_raw_entries(
            r#"{"mcpServers": {"wenlan": {"command": "npx"}}}"#
        )));
        assert!(!parsed(has_both_raw_entries(
            r#"{"mcpServers": {"origin": {"command": "npx"}}}"#
        )));
        assert!(!parsed(has_both_raw_entries(
            r#"{"mcpServers": {"other": {}}}"#
        )));
        assert!(!parsed(has_both_raw_entries(r#"{"theme": "dark"}"#)));
        assert!(unparseable(has_both_raw_entries("not json")).contains("not valid JSON"));
    }

    #[test]
    fn test_has_both_raw_entries_toml() {
        assert!(parsed(has_both_raw_entries_toml(
            "[mcp_servers.origin]\ncommand = \"npx\"\n[mcp_servers.wenlan]\ncommand = \"npx\"\n"
        )));
        assert!(!parsed(has_both_raw_entries_toml(
            "[mcp_servers.wenlan]\ncommand = \"npx\"\n"
        )));
        assert!(!parsed(has_both_raw_entries_toml(
            "[mcp_servers.origin]\ncommand = \"npx\"\n"
        )));
        assert!(!parsed(has_both_raw_entries_toml("model = \"gpt-5.5\"\n")));
        assert!(unparseable(has_both_raw_entries_toml("not toml [")).contains("not valid TOML"));
    }

    /// HEADLINE (a): a raw+raw duplicate on a no-plugin client (cursor) IS
    /// flagged through the public detector, and neither single-entry case is.
    #[test]
    fn test_client_config_has_both_raw_entries_flags_cursor_duplicate() {
        let tmp = tempfile::tempdir().unwrap();
        let both = tmp.path().join("both.json");
        std::fs::write(
            &both,
            r#"{"mcpServers": {"origin": {"command": "npx"}, "wenlan": {"command": "npx"}}}"#,
        )
        .unwrap();
        assert!(yes(client_config_has_both_raw_entries("cursor", &both)));

        let only_wenlan = tmp.path().join("only_wenlan.json");
        std::fs::write(
            &only_wenlan,
            r#"{"mcpServers": {"wenlan": {"command": "npx"}}}"#,
        )
        .unwrap();
        assert!(!yes(client_config_has_both_raw_entries(
            "cursor",
            &only_wenlan
        )));

        // A file that doesn't exist has no duplicate.
        assert!(!yes(client_config_has_both_raw_entries(
            "cursor",
            &tmp.path().join("missing.json")
        )));
    }

    #[test]
    fn test_client_config_has_both_raw_entries_toml_for_codex() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("config.toml");
        std::fs::write(
            &config_path,
            "[mcp_servers.origin]\ncommand = \"npx\"\n[mcp_servers.wenlan]\ncommand = \"npx\"\n",
        )
        .unwrap();
        assert!(yes(client_config_has_both_raw_entries(
            "codex_cli",
            &config_path
        )));
    }

    // ── remove_legacy_origin_entry (removes origin, keeps wenlan) ────────

    /// HEADLINE (b): the fix removes `origin` and KEEPS `wenlan`. Mutating
    /// `remove_legacy_origin_entry` to also drop `wenlan` fails this test.
    #[test]
    fn test_remove_legacy_origin_entry_keeps_wenlan() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("config.json");
        let existing = r#"{"mcpServers": {
            "origin": {"command": "npx", "args": ["-y", "origin-mcp"]},
            "wenlan": {"command": "npx", "args": ["-y", "wenlan-mcp"]},
            "other": {"command": "other-cmd"}
        }}"#;
        std::fs::write(&config_path, existing).unwrap();

        remove_legacy_origin_entry(&config_path).unwrap();

        let parsed: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
        // origin is gone; wenlan and the sibling server stay.
        assert!(parsed["mcpServers"]["origin"].is_null());
        assert!(
            parsed["mcpServers"]["wenlan"].is_object(),
            "the live wenlan entry must survive — removing it would sever the client's connection"
        );
        assert_eq!(parsed["mcpServers"]["other"]["command"], "other-cmd");
    }

    #[test]
    fn test_remove_legacy_origin_entry_clears_the_duplicate() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("config.json");
        std::fs::write(
            &config_path,
            r#"{"mcpServers": {"origin": {"command": "npx"}, "wenlan": {"command": "npx"}}}"#,
        )
        .unwrap();
        assert!(yes(client_config_has_both_raw_entries(
            "cursor",
            &config_path
        )));

        remove_legacy_origin_entry(&config_path).unwrap();

        // The duplicate is resolved, and a single wenlan entry remains.
        assert!(!yes(client_config_has_both_raw_entries(
            "cursor",
            &config_path
        )));
        assert!(yes(client_config_has_raw_entry("cursor", &config_path)));
    }

    /// The `.bak` is taken (both formats), and only when something is actually
    /// removed: an already-clean file and a missing one are both `Err` and
    /// leave no stray backup.
    #[test]
    fn test_remove_legacy_origin_entry_backs_up_only_a_real_removal() {
        let tmp = tempfile::tempdir().unwrap();
        type Remove = fn(&std::path::Path) -> Result<(), AppError>;
        for (remove, name, bak, duplicate, only_wenlan) in [
            (
                remove_legacy_origin_entry as Remove,
                "config.json",
                "json.bak",
                r#"{"mcpServers": {"origin": {"command": "npx"}, "wenlan": {"command": "npx"}}}"#,
                r#"{"mcpServers": {"wenlan": {"command": "npx"}}}"#,
            ),
            (
                remove_legacy_origin_entry_toml as Remove,
                "config.toml",
                "toml.bak",
                "[mcp_servers.origin]\ncommand = \"npx\"\n[mcp_servers.wenlan]\ncommand = \"npx\"\n",
                "[mcp_servers.wenlan]\ncommand = \"npx\"\n",
            ),
        ] {
            let config_path = tmp.path().join(name);
            std::fs::write(&config_path, duplicate).unwrap();
            remove(&config_path).unwrap();
            let backup = config_path.with_extension(bak);
            assert!(backup.exists(), "{name}");
            assert!(
                std::fs::read_to_string(&backup).unwrap().contains("origin"),
                "{name}"
            );

            let clean = tmp.path().join(format!("clean-{name}"));
            std::fs::write(&clean, only_wenlan).unwrap();
            assert!(remove(&clean).is_err(), "{name}");
            assert!(!clean.with_extension(bak).exists(), "{name}");

            assert!(
                remove(&tmp.path().join(format!("nope-{name}"))).is_err(),
                "{name}"
            );
        }
    }

    #[test]
    fn test_remove_legacy_origin_entry_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("config.json");
        std::fs::write(
            &config_path,
            r#"{"mcpServers": {"origin": {"command": "npx"}, "wenlan": {"command": "npx"}}}"#,
        )
        .unwrap();
        remove_legacy_origin_entry(&config_path).unwrap();
        // Second run: origin already gone, so it's an Err (nothing to remove),
        // and wenlan is left untouched.
        assert!(remove_legacy_origin_entry(&config_path).is_err());
        let parsed: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
        assert!(parsed["mcpServers"]["wenlan"].is_object());
    }

    // ── remove_legacy_origin_entry_toml (Codex CLI) ─────────────────────

    #[test]
    fn test_remove_legacy_origin_entry_toml_keeps_wenlan() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("config.toml");
        let fixture = r#"# my codex config
model = "gpt-5.5"

[mcp_servers.origin]
command = "npx"
args = ["-y", "origin-mcp"]

[mcp_servers.wenlan]
command = "npx"
args = ["-y", "wenlan-mcp"]
"#;
        std::fs::write(&config_path, fixture).unwrap();

        remove_legacy_origin_entry_toml(&config_path).unwrap();

        let contents = std::fs::read_to_string(&config_path).unwrap();
        let parsed: toml::Value = toml::from_str(&contents).unwrap();
        assert!(parsed["mcp_servers"].get("origin").is_none());
        assert!(
            parsed["mcp_servers"].get("wenlan").is_some(),
            "the live wenlan entry must survive"
        );
        assert_eq!(parsed["model"], toml::Value::from("gpt-5.5"));
        assert!(yes(client_config_has_raw_entry("codex_cli", &config_path)));
        assert!(!yes(client_config_has_both_raw_entries(
            "codex_cli",
            &config_path
        )));
    }

    /// A decision that names a real binary, for the write tests below — so
    /// they exercise this file's branches rather than whatever the host has
    /// installed.
    fn write_decision() -> McpEntryDecision {
        McpEntryDecision::Write {
            entry: WenlanMcpEntry {
                command: "/opt/wenlan/bin/wenlan-mcp".to_string(),
                args: Vec::new(),
            },
            undetermined: Vec::new(),
        }
    }

    /// `{"mcpServers": []}` is valid JSON a user can genuinely have (an editor
    /// that serialises an empty map as `[]`, a hand-edit), and
    /// `root["mcpServers"]["wenlan"] = ..` is `serde_json`'s `IndexMut`, which
    /// PANICS rather than erroring on an array. Three assertions, and the third
    /// is what makes this a regression test: the call RETURNS, the message
    /// names the shape, and NO BACKUP EXISTS — nothing started.
    #[test]
    fn an_mcp_servers_array_is_a_schema_error_not_a_panic() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("config.json");
        let original = r#"{"mcpServers": []}"#;
        std::fs::write(&config_path, original).unwrap();

        let error = write_wenlan_entry_with(&config_path, false, write_decision())
            .expect_err("an array under `mcpServers` cannot take an entry");
        let error = error.to_string();
        assert!(
            error.contains("mcpServers") && error.contains("a list"),
            "the message has to name the shape the user has to fix: {error}"
        );
        assert_eq!(
            std::fs::read_to_string(&config_path).unwrap(),
            original,
            "the user's config was modified by a write that could not be completed"
        );
        assert!(
            !config_path.with_extension("json.bak").exists(),
            "a backup was left behind by a change that never happened"
        );
    }

    /// The same crash one line earlier: any valid non-object top level panics
    /// on the `root[\"mcpServers\"] = ..` insert instead.
    #[test]
    fn a_non_object_json_config_is_a_schema_error_not_a_panic() {
        let tmp = tempfile::tempdir().unwrap();
        for original in [r#"[]"#, r#""wenlan""#, r#"3"#] {
            let config_path = tmp.path().join(format!("config{}.json", original.len()));
            std::fs::write(&config_path, original).unwrap();
            let error = write_wenlan_entry_with(&config_path, false, write_decision())
                .expect_err("a non-object top level cannot take an entry")
                .to_string();
            assert!(
                error.contains("top level"),
                "the message has to say what is wrong with the file: {error}"
            );
            assert_eq!(std::fs::read_to_string(&config_path).unwrap(), original);
            assert!(!config_path.with_extension("json.bak").exists());
        }
    }

    /// A present-but-null `mcpServers` is NOT a schema error: it is a place an
    /// entry can go, and the schema check must not tighten into refusing it.
    #[test]
    fn a_null_mcp_servers_key_is_still_writable() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("config.json");
        std::fs::write(&config_path, r#"{"mcpServers": null, "theme": "dark"}"#).unwrap();

        write_wenlan_entry_with(&config_path, false, write_decision()).unwrap();

        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
        assert_eq!(
            written["mcpServers"]["wenlan"]["command"],
            "/opt/wenlan/bin/wenlan-mcp"
        );
        assert_eq!(written["theme"], "dark", "unrelated keys must survive");
    }

    /// The TOML half: `doc["mcp_servers"][key] = ..` is `toml_edit`'s
    /// `IndexMut`, i.e. `.expect("index not found")` — a panic for any
    /// `mcp_servers` that is not table-like.
    #[test]
    fn a_scalar_mcp_servers_key_is_a_schema_error_not_a_panic_in_toml() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("config.toml");
        let original = "mcp_servers = 5\n";
        std::fs::write(&config_path, original).unwrap();

        let error = write_wenlan_entry_toml_with(&config_path, write_decision())
            .expect_err("a scalar `mcp_servers` cannot take a table")
            .to_string();
        assert!(
            error.contains("mcp_servers"),
            "the message has to name the key the user has to fix: {error}"
        );
        assert_eq!(std::fs::read_to_string(&config_path).unwrap(), original);
        assert!(!config_path.with_extension("toml.bak").exists());
    }

    /// The backup is written from the bytes that were PARSED, and only while
    /// those are still on disk. `back_up_parsed` is exercised directly because
    /// the race it closes (read+parse A, another process writes B, backup)
    /// cannot be staged through the writers without a hook into the middle of
    /// them.
    #[test]
    fn a_config_that_changed_under_the_writer_is_not_backed_up_from_the_new_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("config.json");
        std::fs::write(&config_path, "{\"replaced\": true}").unwrap();

        let error = back_up_parsed(&config_path, "{\"parsed\": true}", "json.bak")
            .expect_err("the parse is stale, so the update it produced must not be applied")
            .to_string();
        assert!(
            error.contains("changed while Wenlan was updating it"),
            "{error}"
        );
        assert!(
            !config_path.with_extension("json.bak").exists(),
            "the last good backup was replaced with bytes this process never parsed"
        );
    }

    /// The ordinary path: unchanged file, backup holds exactly the parsed
    /// bytes.
    #[test]
    fn the_backup_holds_the_bytes_that_were_parsed() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("config.json");
        let contents = "{\"mcpServers\": {}}";
        std::fs::write(&config_path, contents).unwrap();

        back_up_parsed(&config_path, contents, "json.bak").unwrap();

        assert_eq!(
            std::fs::read_to_string(config_path.with_extension("json.bak")).unwrap(),
            contents
        );
    }

    /// The OS refusal this is really about (a file that permits writing but not
    /// `metadata`) cannot be staged portably. A DIRECTORY at the config path
    /// reaches the SAME branch: `read_config` answers `Unreadable`, not
    /// `Absent`, so the writer must refuse rather than take the new-file branch
    /// and truncate from a `json!({})` skeleton with no backup.
    #[test]
    fn a_config_path_that_cannot_be_read_is_never_treated_as_a_new_file() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("config.json");
        std::fs::create_dir(&config_path).unwrap();
        assert!(
            matches!(read_config(&config_path), ConfigRead::Unreadable(_)),
            "fixture: this path must READ as unreadable, not as an absence"
        );

        let error = write_wenlan_entry_with(&config_path, false, write_decision())
            .expect_err("a config that could not be read must not be replaced")
            .to_string();
        assert!(error.contains("Nothing was written"), "{error}");
        assert!(
            config_path.is_dir(),
            "the path was replaced by a file built from an empty skeleton"
        );
    }

    /// The removal half of the same collapse: `if !config_path.exists()`
    /// reported "No config file found — nothing to remove" for a metadata
    /// denial, presenting a failed look as a measured absence and never
    /// attempting the read.
    #[test]
    fn a_config_that_cannot_be_read_is_not_reported_as_nothing_to_remove() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("config.json");
        std::fs::create_dir(&config_path).unwrap();

        let error = remove_wenlan_entry(&config_path)
            .expect_err("an unreadable config establishes nothing about what it holds")
            .to_string();
        assert!(
            !error.contains("No config file found"),
            "a failed look was reported as a measured absence: {error}"
        );
        assert!(error.contains("Could not read"), "{error}");

        // …while a genuinely absent file still says exactly that.
        let missing = tmp.path().join("nope.json");
        assert!(remove_wenlan_entry(&missing)
            .expect_err("nothing to remove")
            .to_string()
            .contains("No config file found"));
    }

    /// `to_string_lossy` turns a path that cannot be spelled in UTF-8 into one
    /// that CAN, made of U+FFFD. Writing that into a client config names a
    /// different, nonexistent file, and the failure surfaces later as the
    /// client failing to launch a filename the user cannot find.
    #[test]
    fn a_binary_under_a_non_unicode_path_is_never_written_as_a_lossy_command() {
        #[cfg(windows)]
        let path: PathBuf = {
            use std::os::windows::ffi::OsStringExt;
            // A lone high surrogate: a valid Windows filename, not valid
            // Unicode, so `to_str()` is `None` and `to_string_lossy()` is a
            // DIFFERENT string.
            std::ffi::OsString::from_wide(&[0x0043, 0x003A, 0x005C, 0xD800, 0x002E, 0x0065]).into()
        };
        #[cfg(unix)]
        let path: PathBuf = {
            use std::os::unix::ffi::OsStringExt;
            std::ffi::OsString::from_vec(b"/opt/\xff/wenlan-mcp".to_vec()).into()
        };
        assert!(
            path.to_str().is_none(),
            "fixture: this path must not be representable as UTF-8"
        );

        let decision = wenlan_mcp_entry_for(
            McpBinaryResolution::Found {
                path: path.clone(),
                undetermined: Vec::new(),
            },
            "wenlan-mcp@^9.9.9",
        );

        match decision {
            McpEntryDecision::Write { entry, .. } => panic!(
                "wrote a command naming a file that does not exist: {:?} (the real path is \
                 {:?})",
                entry.command, path
            ),
            McpEntryDecision::PreserveExisting { unmeasured } => {
                assert_eq!(unmeasured.unreadable.len(), 1);
                assert_eq!(unmeasured.unreadable[0].0, path);
                assert!(
                    unmeasured.unreadable[0].1.contains("not valid Unicode"),
                    "{:?}",
                    unmeasured.unreadable[0].1
                );
                // …and the writers act on it the way they act on any other
                // unresolved search: no write, no backup, message names it.
                let message = unresolved_message(&unmeasured);
                assert!(message.contains("Nothing was written"), "{message}");
            }
        }
    }

    /// End to end, in the shape the UI receives: a present Gemini
    /// `settings.json` holding `not json`. The file WAS read, so `detected` is
    /// a measured yes; whether it holds a Wenlan entry could NOT be measured,
    /// so `has_raw_entry` and `already_configured` must not be `no` — "no" is
    /// what puts an unqualified "Set up" button in front of the user.
    #[test]
    fn a_present_but_unparseable_client_config_is_detected_and_unmeasurable() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        std::fs::create_dir_all(home.join(".gemini")).unwrap();
        std::fs::write(home.join(".gemini").join("settings.json"), "not json").unwrap();

        let clients = detect_mcp_clients_from(Some(home), None, |_| Reading::No);
        let gemini = clients
            .iter()
            .find(|c| c.client_type == "gemini_cli")
            .expect("every client keeps a row");

        assert_eq!(
            gemini.detected,
            Reading::Yes,
            "the file is there and was read; only its CONTENTS were unmeasurable"
        );
        assert!(
            matches!(gemini.has_raw_entry, Reading::Unreadable { .. }),
            "an unparseable config answered `no entry`: {:?}",
            gemini.has_raw_entry
        );
        assert!(
            matches!(gemini.has_raw_duplicate, Reading::Unreadable { .. }),
            "{:?}",
            gemini.has_raw_duplicate
        );
        assert!(
            matches!(gemini.already_configured, Reading::Unreadable { .. }),
            "`already_configured` is what the wizard and the Settings list read, and it said \
             `not configured`: {:?}",
            gemini.already_configured
        );
    }

    /// A MEASURED `no` for the plugin is the gate on Diagnostics' destructive
    /// raw-duplicate fix, so a malformed `~/.claude/settings.json` must not
    /// reach it.
    #[test]
    fn an_unparseable_claude_settings_file_leaves_the_plugin_state_unmeasured() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::write(home.join(".claude").join("settings.json"), "{oops").unwrap();

        let clients = detect_mcp_clients_from(Some(home), None, |_| Reading::No);
        let claude_code = clients
            .iter()
            .find(|c| c.client_type == "claude_code")
            .expect("every client keeps a row");

        assert!(
            matches!(claude_code.has_plugin, Reading::Unreadable { .. }),
            "an unparseable settings.json answered `the plugin is off`: {:?}",
            claude_code.has_plugin
        );
    }

    /// …and the client with no plugin surface at all is still a MEASURED no,
    /// so nothing above turns Cursor's honest `no` into an unknown.
    #[test]
    fn a_client_with_no_plugin_surface_is_still_a_measured_no() {
        let tmp = tempfile::tempdir().unwrap();
        let clients = detect_mcp_clients_from(Some(tmp.path()), None, |_| Reading::No);
        for client_type in ["cursor", "gemini_cli"] {
            let client = clients
                .iter()
                .find(|c| c.client_type == client_type)
                .unwrap();
            assert_eq!(
                client.has_plugin,
                Reading::No,
                "{client_type} has no plugin surface to fail to read"
            );
        }
    }

    /// At the writers: a write that succeeded while one of the resolver's
    /// inputs went unread is not the same event as one where everything was
    /// measured, so it cannot report the same thing.
    #[test]
    fn a_write_reports_the_inputs_that_could_not_be_determined() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("config.json");
        let undetermined = vec![UndeterminedInput {
            input: "WENLAN_MCP_DEV_BIN".to_string(),
            blocked: "WENLAN_MCP_DEV_BIN".to_string(),
            error: "environment variable was not valid Unicode".to_string(),
        }];

        let reported = write_wenlan_entry_with(
            &config_path,
            false,
            McpEntryDecision::Write {
                entry: WenlanMcpEntry {
                    command: "/opt/wenlan/bin/wenlan-mcp".to_string(),
                    args: Vec::new(),
                },
                undetermined: undetermined.clone(),
            },
        )
        .unwrap();

        assert_eq!(
            reported, undetermined,
            "the write succeeded off a search that skipped a candidate it never built, and said \
             exactly what an all-measured write says"
        );

        // The TOML writer is the same boundary.
        let toml_path = tmp.path().join("config.toml");
        let reported = write_wenlan_entry_toml_with(
            &toml_path,
            McpEntryDecision::Write {
                entry: WenlanMcpEntry {
                    command: "/opt/wenlan/bin/wenlan-mcp".to_string(),
                    args: Vec::new(),
                },
                undetermined: undetermined.clone(),
            },
        )
        .unwrap();
        assert_eq!(reported, undetermined);
    }

    // ── Install state: the program, or only what it left behind ──────────

    /// A described machine for [`detect_mcp_clients_with`]: every path the OS
    /// would say is there, the folders that can be listed, and the CLI names
    /// that resolve. Config files are still REAL reads of the temp home a test
    /// passes, so nothing here ever looks at the developer's own machine.
    struct Machine {
        host: HostKind,
        local: Option<PathBuf>,
        program_files: Vec<PathBuf>,
        existing: Vec<PathBuf>,
        listings: Vec<(PathBuf, Vec<PathBuf>)>,
        on_path: Vec<&'static str>,
        unreadable: Vec<PathBuf>,
    }

    impl Machine {
        fn new(host: HostKind) -> Self {
            Machine {
                host,
                local: None,
                program_files: Vec::new(),
                existing: Vec::new(),
                listings: Vec::new(),
                on_path: Vec::new(),
                unreadable: Vec::new(),
            }
        }

        fn detect(&self, home: &Path) -> Vec<McpClient> {
            let exists = |p: &Path| {
                if self.unreadable.iter().any(|u| u == p) {
                    return Reading::Unreadable {
                        error: "Access is denied. (os error 5)".to_string(),
                    };
                }
                Reading::of(self.existing.iter().any(|e| e == p))
            };
            let list_dir = |p: &Path| {
                self.listings
                    .iter()
                    .find(|(dir, _)| dir == p)
                    .map(|(_, children)| children.clone())
                    .unwrap_or_default()
            };
            let which = |name: &str| {
                self.on_path
                    .contains(&name)
                    .then(|| PathBuf::from("/fake/bin").join(name))
            };
            detect_mcp_clients_with(
                Some(home),
                Some(home),
                &DetectProbes {
                    host: self.host,
                    local_data_dir: self.local.as_deref(),
                    program_files: &self.program_files,
                    exists: &exists,
                    list_dir: &list_dir,
                    which: &which,
                    probe_command: &probe_candidate,
                },
            )
        }

        fn install_state(&self, home: &Path, client_type: &str) -> InstallState {
            row(&self.detect(home), client_type).install_state.clone()
        }
    }

    fn row<'a>(clients: &'a [McpClient], client_type: &str) -> &'a McpClient {
        clients
            .iter()
            .find(|c| c.client_type == client_type)
            .expect("every client keeps a row")
    }

    /// Every place Cursor can live that is not a macOS `/Applications`, one OS
    /// at a time. Each case makes EXACTLY that path the only thing on the
    /// machine, so a typo in a candidate fails here, and the control at the end
    /// proves the fixture can say "not found".
    #[test]
    fn cursor_is_found_beyond_applications_on_each_os() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let local = home.join("AppData").join("Local");
        let program_files = home.join("PF");

        let cases: Vec<(HostKind, PathBuf)> = vec![
            (HostKind::MacOs, PathBuf::from("/Applications/Cursor.app")),
            (HostKind::MacOs, home.join("Applications/Cursor.app")),
            (
                HostKind::Windows,
                local.join("Programs").join("cursor").join("Cursor.exe"),
            ),
            (
                HostKind::Windows,
                local.join("Programs").join("Cursor").join("Cursor.exe"),
            ),
            (
                HostKind::Windows,
                program_files.join("Cursor").join("Cursor.exe"),
            ),
            (HostKind::Linux, PathBuf::from("/usr/bin/cursor")),
            (HostKind::Linux, PathBuf::from("/usr/local/bin/cursor")),
            (HostKind::Linux, PathBuf::from("/opt/Cursor/cursor")),
            (HostKind::Linux, PathBuf::from("/opt/cursor/cursor")),
            (HostKind::Linux, home.join(".local/bin/cursor")),
        ];
        for (host, hit) in cases {
            let mut machine = Machine::new(host);
            machine.local = Some(local.clone());
            machine.program_files = vec![program_files.clone()];

            assert_eq!(
                machine.install_state(home, "cursor"),
                InstallState::NotFound,
                "control: an empty {host:?} machine has no Cursor"
            );
            machine.existing = vec![hit.clone()];
            assert_eq!(
                machine.install_state(home, "cursor"),
                InstallState::Installed,
                "{} was not probed on {host:?}",
                hit.display()
            );
        }

        // The OS branch is the point: a Windows install location means nothing
        // on a Linux host.
        let mut machine = Machine::new(HostKind::Linux);
        machine.local = Some(local.clone());
        machine.existing = vec![local.join("Programs").join("cursor").join("Cursor.exe")];
        assert_eq!(
            machine.install_state(home, "cursor"),
            InstallState::NotFound
        );
    }

    #[test]
    fn cursor_cli_on_path_or_an_appimage_counts_as_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();

        let mut machine = Machine::new(HostKind::Linux);
        machine.on_path = vec!["cursor"];
        assert_eq!(
            machine.install_state(home, "cursor"),
            InstallState::Installed
        );

        // Cursor's CLI alone, without the editor, still reads ~/.cursor.
        let mut machine = Machine::new(HostKind::Linux);
        machine.on_path = vec!["cursor-agent"];
        assert_eq!(
            machine.install_state(home, "cursor"),
            InstallState::Installed
        );

        // An AppImage has no installer; it sits wherever the user put it.
        let apps = home.join("Applications");
        for (listed, expected) in [
            ("Cursor-0.50.5-x86_64.AppImage", InstallState::Installed),
            ("cursor.appimage", InstallState::Installed),
            ("Notes-1.0.AppImage", InstallState::NotFound),
            ("cursor-notes.txt", InstallState::NotFound),
        ] {
            let mut machine = Machine::new(HostKind::Linux);
            machine.listings = vec![(apps.clone(), vec![apps.join(listed)])];
            assert_eq!(
                machine.install_state(home, "cursor"),
                expected,
                "{listed} in ~/Applications"
            );
        }
    }

    /// The finding: a leftover config used to be indistinguishable from an
    /// installed client. Now the row says which it is, and `detected` (the
    /// superset) keeps saying yes.
    #[test]
    fn a_config_with_no_program_is_config_only_and_still_detected() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        std::fs::create_dir_all(home.join(".gemini")).unwrap();
        std::fs::write(home.join(".gemini").join("settings.json"), "{}").unwrap();
        std::fs::write(home.join(".claude.json"), "{}").unwrap();

        let machine = Machine::new(HostKind::MacOs);
        let clients = machine.detect(home);
        for client_type in ["gemini_cli", "claude_code"] {
            let client = row(&clients, client_type);
            assert_eq!(
                client.install_state,
                InstallState::ConfigOnly,
                "{client_type}"
            );
            assert_eq!(client.detected, Reading::Yes, "{client_type}");
        }

        // The same configs with the CLIs present are installs.
        let mut machine = Machine::new(HostKind::MacOs);
        machine.on_path = vec!["gemini", "claude"];
        let clients = machine.detect(home);
        for client_type in ["gemini_cli", "claude_code"] {
            assert_eq!(
                row(&clients, client_type).install_state,
                InstallState::Installed,
                "{client_type}"
            );
        }
    }

    /// A fresh Claude Code has no `~/.claude.json` until it first runs; the CLI
    /// on PATH is what says it is installed.
    #[test]
    fn a_cli_with_no_config_yet_is_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let mut machine = Machine::new(HostKind::Linux);
        machine.on_path = vec!["claude"];
        let clients = machine.detect(tmp.path());
        let claude = row(&clients, "claude_code");
        assert_eq!(claude.install_state, InstallState::Installed);
        assert_eq!(claude.detected, Reading::Yes);
        assert_eq!(
            row(&clients, "gemini_cli").install_state,
            InstallState::NotFound,
            "control: an unrelated CLI is not found"
        );
    }

    /// Cursor and Codex leave a home folder once they have run, whether or not
    /// it holds a config file. The folder is leftover evidence, never install
    /// evidence.
    #[test]
    fn a_client_home_folder_alone_is_config_only() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        for (client_type, folder) in [("cursor", ".cursor"), ("codex_cli", ".codex")] {
            let mut machine = Machine::new(HostKind::Linux);
            assert_eq!(
                machine.install_state(home, client_type),
                InstallState::NotFound,
                "control: {client_type} on an empty machine"
            );
            machine.existing = vec![home.join(folder)];
            let clients = machine.detect(home);
            let client = row(&clients, client_type);
            assert_eq!(
                client.install_state,
                InstallState::ConfigOnly,
                "{client_type}: ~/{folder} is a leftover, not a program"
            );
            assert_eq!(client.detected, Reading::Yes);
        }
    }

    /// Codex used to be detected through ChatGPT.app or its config file only.
    #[test]
    fn codex_is_found_by_its_cli_and_not_only_through_chatgpt() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();

        let mut machine = Machine::new(HostKind::Linux);
        machine.on_path = vec!["codex"];
        assert_eq!(
            machine.install_state(home, "codex_cli"),
            InstallState::Installed,
            "codex on PATH, no ChatGPT, no ~/.codex"
        );

        let mut machine = Machine::new(HostKind::MacOs);
        machine.existing = vec![PathBuf::from("/Applications/ChatGPT.app")];
        assert_eq!(
            machine.install_state(home, "codex_cli"),
            InstallState::Installed,
            "the ChatGPT desktop bundle still counts"
        );

        let machine = Machine::new(HostKind::Windows);
        assert_eq!(
            machine.install_state(home, "codex_cli"),
            InstallState::NotFound
        );
    }

    #[test]
    fn claude_desktop_is_found_per_os() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let local = home.join("AppData").join("Local");

        let mut machine = Machine::new(HostKind::MacOs);
        machine.existing = vec![PathBuf::from("/Applications/Claude.app")];
        assert_eq!(
            machine.install_state(home, "claude_desktop"),
            InstallState::Installed
        );

        let mut machine = Machine::new(HostKind::Windows);
        machine.local = Some(local.clone());
        machine.existing = vec![local.join("AnthropicClaude").join("claude.exe")];
        assert_eq!(
            machine.install_state(home, "claude_desktop"),
            InstallState::Installed
        );

        // The Microsoft Store package is a folder under Packages.
        let mut machine = Machine::new(HostKind::Windows);
        machine.local = Some(local.clone());
        let packages = local.join("Packages");
        machine.listings = vec![(
            packages.clone(),
            vec![packages.join("Claude_pzs8sxrjxfjjc")],
        )];
        assert_eq!(
            machine.install_state(home, "claude_desktop"),
            InstallState::Installed
        );

        // A leftover config with no app.
        std::fs::create_dir_all(home.join("Claude")).unwrap();
        std::fs::write(home.join("Claude").join("claude_desktop_config.json"), "{}").unwrap();
        let machine = Machine::new(HostKind::MacOs);
        assert_eq!(
            machine.install_state(home, "claude_desktop"),
            InstallState::ConfigOnly
        );
    }

    /// A look the OS refused is not "no program": it cannot be promoted to
    /// `config_only` (which says the program is gone) or `not_found`.
    #[test]
    fn a_program_search_that_could_not_look_is_never_config_only_or_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let bundle = PathBuf::from("/Applications/Cursor.app");

        let mut machine = Machine::new(HostKind::MacOs);
        machine.unreadable = vec![bundle.clone()];
        let clients = machine.detect(home);
        let cursor = row(&clients, "cursor");
        assert!(
            matches!(cursor.install_state, InstallState::Unreadable { .. }),
            "{:?}",
            cursor.install_state
        );
        assert!(matches!(cursor.detected, Reading::Unreadable { .. }));

        // With a config file present the client IS detected, but whether the
        // program is there is still unknown.
        std::fs::create_dir_all(home.join(".cursor")).unwrap();
        std::fs::write(home.join(".cursor").join("mcp.json"), "{}").unwrap();
        let clients = machine.detect(home);
        let cursor = row(&clients, "cursor");
        assert!(
            matches!(cursor.install_state, InstallState::Unreadable { .. }),
            "an unread program search reported {:?}",
            cursor.install_state
        );
        assert_eq!(cursor.detected, Reading::Yes);
    }

    /// Windows candidates hang off the per-user program folder. If the platform
    /// would not report it, the search did not cover them.
    #[test]
    fn a_windows_machine_with_no_local_data_dir_is_unknown_not_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let machine = Machine::new(HostKind::Windows);
        let state = machine.install_state(tmp.path(), "cursor");
        match state {
            InstallState::Unreadable { error } => assert!(error.contains("local application-data")),
            other => panic!("an undetermined local data dir reported {other:?}"),
        }
    }

    /// `detected` is exactly "installed or config-only", across every client
    /// and a spread of machines — never a third opinion.
    #[test]
    fn detected_is_exactly_installed_or_config_only() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        std::fs::write(home.join(".claude.json"), "{}").unwrap();

        let empty = Machine::new(HostKind::MacOs);
        let mut everything_on_path = Machine::new(HostKind::Linux);
        everything_on_path.on_path = vec!["cursor", "codex", "gemini", "claude"];
        let mut leftovers_only = Machine::new(HostKind::MacOs);
        leftovers_only.existing = vec![home.join(".cursor"), home.join(".codex")];
        let mut refused = Machine::new(HostKind::MacOs);
        refused.unreadable = vec![PathBuf::from("/Applications/Cursor.app")];

        for machine in [&empty, &everything_on_path, &leftovers_only, &refused] {
            for client in machine.detect(home) {
                match &client.install_state {
                    InstallState::Installed | InstallState::ConfigOnly => assert_eq!(
                        client.detected,
                        Reading::Yes,
                        "{}: {:?}",
                        client.client_type,
                        client.install_state
                    ),
                    InstallState::NotFound => assert_eq!(
                        client.detected,
                        Reading::No,
                        "{}: not_found must be a measured no",
                        client.client_type
                    ),
                    InstallState::Unreadable { .. } => assert!(
                        matches!(client.detected, Reading::Yes | Reading::Unreadable { .. }),
                        "{}: an unread install state cannot be a measured no: {:?}",
                        client.client_type,
                        client.detected
                    ),
                }
            }
        }
    }

    // ── Entry health: would the configured command launch? ───────────────

    /// A `DetectProbes` whose only live parts are the two entry-validation
    /// lookups, handed in by the test.
    fn nothing_exists(_: &Path) -> Reading {
        Reading::No
    }

    fn nothing_to_list(_: &Path) -> Vec<PathBuf> {
        Vec::new()
    }

    fn health_probes<'a>(
        host: HostKind,
        which: &'a dyn Fn(&str) -> Option<PathBuf>,
        probe_command: &'a dyn Fn(&Path) -> CandidateProbe,
    ) -> DetectProbes<'a> {
        DetectProbes {
            host,
            local_data_dir: None,
            program_files: &[],
            exists: &nothing_exists,
            list_dir: &nothing_to_list,
            which,
            probe_command,
        }
    }

    /// `which` that resolves exactly the names given.
    fn resolves<'a>(names: &'a [&'a str]) -> impl Fn(&str) -> Option<PathBuf> + 'a {
        move |name: &str| {
            names
                .contains(&name)
                .then(|| PathBuf::from("/fake/bin").join(name))
        }
    }

    fn json_health(
        body: &str,
        names: &[&str],
        probe: impl Fn(&Path) -> CandidateProbe,
    ) -> EntryHealth {
        let which = resolves(names);
        entry_health_reading(
            "cursor",
            &ConfigRead::Contents(body.to_string()),
            &health_probes(HostKind::MacOs, &which, &probe),
        )
    }

    fn toml_health(
        body: &str,
        names: &[&str],
        probe: impl Fn(&Path) -> CandidateProbe,
    ) -> EntryHealth {
        let which = resolves(names);
        entry_health_reading(
            "codex_cli",
            &ConfigRead::Contents(body.to_string()),
            &health_probes(HostKind::MacOs, &which, &probe),
        )
    }

    fn is_a_file(_: &Path) -> CandidateProbe {
        CandidateProbe::File
    }

    #[track_caller]
    fn assert_repair(health: EntryHealth, expected: RepairReason) -> String {
        match health {
            EntryHealth::NeedsRepair { reason, detail } => {
                assert_eq!(reason, expected, "{detail}");
                assert!(!detail.is_empty());
                detail
            }
            other => panic!("expected needs_repair/{expected:?}, got {other:?}"),
        }
    }

    #[test]
    fn no_entry_when_there_is_nothing_to_check() {
        let no_probe = |_: &Path| CandidateProbe::Absent;
        let which = resolves(&[]);
        let probes = health_probes(HostKind::MacOs, &which, &no_probe);
        assert_eq!(
            entry_health_reading("cursor", &ConfigRead::Absent, &probes),
            EntryHealth::NoEntry
        );
        for body in [
            "{}",
            r#"{"mcpServers": {"other": {"command": "x"}}}"#,
            r#"{"mcpServers": []}"#,
        ] {
            assert_eq!(
                json_health(body, &[], no_probe),
                EntryHealth::NoEntry,
                "{body}"
            );
        }
        assert_eq!(
            toml_health("model = \"gpt-5.5\"\n", &[], no_probe),
            EntryHealth::NoEntry
        );
    }

    #[test]
    fn an_entry_that_resolves_is_healthy() {
        // An absolute path to a runnable file, with and without `args`.
        for entry in [
            r#"{"command": "/opt/wenlan/bin/wenlan-mcp", "args": []}"#,
            r#"{"command": "/opt/wenlan/bin/wenlan-mcp"}"#,
            r#"{"command": "/opt/wenlan/bin/wenlan-mcp", "args": ["--agent-name", "cursor"]}"#,
        ] {
            let body = format!(r#"{{"mcpServers": {{"wenlan": {entry}}}}}"#);
            assert_eq!(
                json_health(&body, &[], is_a_file),
                EntryHealth::Healthy,
                "{entry}"
            );
        }
        // A bare name found on PATH.
        assert_eq!(
            json_health(
                r#"{"mcpServers": {"wenlan": {"command": "wenlan-mcp"}}}"#,
                &["wenlan-mcp"],
                |_| CandidateProbe::Absent
            ),
            EntryHealth::Healthy
        );
        // The shape Wenlan writes when nothing is installed.
        assert_eq!(
            json_health(
                r#"{"mcpServers": {"wenlan": {"command": "npx", "args": ["-y", "wenlan-mcp@^0.18.16"]}}}"#,
                &["npx"],
                |_| CandidateProbe::Absent
            ),
            EntryHealth::Healthy
        );
    }

    #[test]
    fn a_command_that_resolves_nowhere_needs_repair() {
        let missing = json_health(
            r#"{"mcpServers": {"wenlan": {"command": "/gone/wenlan-mcp"}}}"#,
            &[],
            |_| CandidateProbe::Absent,
        );
        let detail = assert_repair(missing, RepairReason::CommandNotFound);
        assert!(detail.contains("/gone/wenlan-mcp"), "{detail}");

        let bare = json_health(
            r#"{"mcpServers": {"wenlan": {"command": "wenlan-mcp"}}}"#,
            &["something-else"],
            is_a_file,
        );
        let detail = assert_repair(bare, RepairReason::CommandNotFound);
        assert!(detail.contains("PATH"), "{detail}");

        // `npx` that is not installed: reported as the missing command before
        // anyone looks at its arguments.
        assert_repair(
            json_health(
                r#"{"mcpServers": {"wenlan": {"command": "npx", "args": ["-y", "wenlan-mcp"]}}}"#,
                &[],
                is_a_file,
            ),
            RepairReason::CommandNotFound,
        );
    }

    /// Clients start servers from a folder this app cannot know and do not
    /// expand `~`, so these never work however plausible they look.
    #[test]
    fn a_relative_command_path_needs_repair() {
        for command in ["./wenlan-mcp", "bin/wenlan-mcp", "~/.wenlan/bin/wenlan-mcp"] {
            let body = format!(r#"{{"mcpServers": {{"wenlan": {{"command": "{command}"}}}}}}"#);
            let detail = assert_repair(
                // Even a probe that says "file" and a PATH that resolves
                // everything must not rescue it.
                json_health(&body, &["wenlan-mcp", "./wenlan-mcp"], is_a_file),
                RepairReason::CommandNotFound,
            );
            assert!(detail.contains("full path"), "{command}: {detail}");
        }
    }

    #[test]
    fn something_at_the_path_that_cannot_run_needs_repair() {
        let entry = r#"{"mcpServers": {"wenlan": {"command": "/opt/wenlan-mcp"}}}"#;
        assert_repair(
            json_health(entry, &[], |_| CandidateProbe::NotAFile),
            RepairReason::CommandNotRunnable,
        );
        let detail = assert_repair(
            json_health(entry, &[], |_| CandidateProbe::NotExecutable {
                reason: "the file is empty (0 bytes), so it is not a program".to_string(),
            }),
            RepairReason::CommandNotRunnable,
        );
        assert!(detail.contains("0 bytes"), "{detail}");
    }

    /// A path the OS would not let this app stat is not a broken entry, and
    /// must not put a "Repair" button in front of the user.
    #[test]
    fn a_command_path_that_could_not_be_looked_at_is_unreadable_not_broken() {
        let health = json_health(
            r#"{"mcpServers": {"wenlan": {"command": "/opt/wenlan-mcp"}}}"#,
            &[],
            |_| CandidateProbe::Unreadable {
                error: "Access is denied. (os error 5)".to_string(),
            },
        );
        match health {
            EntryHealth::Unreadable { error } => assert!(error.contains("Access is denied")),
            other => panic!("an unreadable command path reported {other:?}"),
        }
    }

    #[test]
    fn an_entry_with_no_usable_command_needs_repair() {
        for entry in [
            "{}",
            r#"{"command": ""}"#,
            r#"{"command": "   "}"#,
            r#"{"command": 5}"#,
            r#""wenlan-mcp""#,
            "null",
        ] {
            let body = format!(r#"{{"mcpServers": {{"wenlan": {entry}}}}}"#);
            assert_repair(
                json_health(&body, &["wenlan-mcp"], is_a_file),
                RepairReason::CommandMissing,
            );
        }
    }

    #[test]
    fn arguments_that_are_not_sane_need_repair() {
        for args in [r#""-y""#, "[1, 2]", r#"["-y", null]"#, r#"{"a": "b"}"#] {
            let body = format!(
                r#"{{"mcpServers": {{"wenlan": {{"command": "/opt/wenlan-mcp", "args": {args}}}}}}}"#
            );
            assert_repair(
                json_health(&body, &[], is_a_file),
                RepairReason::ArgsInvalid,
            );
        }
        // `npx` has to be asked for Wenlan's package; any other package, or
        // none, is not Wenlan's launcher.
        for args in [
            r#"["-y", "some-other-server"]"#,
            r#"["-y"]"#,
            r#"["-y", "wenlan-mcp-evil"]"#,
            r#"["-y", "@scope/wenlan-mcp"]"#,
            "[]",
        ] {
            let body =
                format!(r#"{{"mcpServers": {{"wenlan": {{"command": "npx", "args": {args}}}}}}}"#);
            assert_repair(
                json_health(&body, &["npx"], is_a_file),
                RepairReason::ArgsInvalid,
            );
        }
        // …and the spellings that are Wenlan's, including the pre-rename name.
        for package in [
            "wenlan-mcp",
            "wenlan-mcp@^0.18.16",
            "wenlan-mcp@latest",
            "origin-mcp@1",
        ] {
            let body = format!(
                r#"{{"mcpServers": {{"wenlan": {{"command": "npx", "args": ["-y", "{package}"]}}}}}}"#
            );
            assert_eq!(
                json_health(&body, &["npx"], is_a_file),
                EntryHealth::Healthy,
                "{package}"
            );
        }
    }

    /// A remote server has no command to check and is not Wenlan's to second
    /// guess.
    #[test]
    fn a_url_entry_has_no_command_to_validate() {
        assert_eq!(
            json_health(
                r#"{"mcpServers": {"wenlan": {"url": "http://127.0.0.1:7878/mcp"}}}"#,
                &[],
                |_| CandidateProbe::Absent
            ),
            EntryHealth::Healthy
        );
        assert_eq!(
            toml_health(
                "[mcp_servers.wenlan]\nurl = \"http://127.0.0.1:7878/mcp\"\n",
                &[],
                |_| CandidateProbe::Absent
            ),
            EntryHealth::Healthy
        );
    }

    /// Judged on `wenlan`, or on the legacy `origin` entry when that is all
    /// there is — and `wenlan` wins when both exist.
    #[test]
    fn the_wenlan_entry_is_judged_and_the_legacy_one_only_when_alone() {
        let broken = r#"{"command": "/gone/origin-mcp"}"#;
        let good = r#"{"command": "/opt/wenlan-mcp"}"#;
        let only_present = |p: &Path| {
            if p == Path::new("/opt/wenlan-mcp") {
                CandidateProbe::File
            } else {
                CandidateProbe::Absent
            }
        };

        let legacy_only = format!(r#"{{"mcpServers": {{"origin": {broken}}}}}"#);
        assert_repair(
            json_health(&legacy_only, &[], only_present),
            RepairReason::CommandNotFound,
        );

        let healthy_wenlan =
            format!(r#"{{"mcpServers": {{"wenlan": {good}, "origin": {broken}}}}}"#);
        assert_eq!(
            json_health(&healthy_wenlan, &[], only_present),
            EntryHealth::Healthy,
            "a broken legacy entry beside a good one is the duplicate fix's business"
        );

        let broken_wenlan =
            format!(r#"{{"mcpServers": {{"wenlan": {broken}, "origin": {good}}}}}"#);
        assert_repair(
            json_health(&broken_wenlan, &[], only_present),
            RepairReason::CommandNotFound,
        );
    }

    /// Same rule in Codex's TOML: the legacy `origin` table is judged only
    /// when there is no `wenlan` one.
    #[test]
    fn the_legacy_codex_table_is_judged_only_when_alone() {
        let only_present = |p: &Path| {
            if p == Path::new("/opt/wenlan-mcp") {
                CandidateProbe::File
            } else {
                CandidateProbe::Absent
            }
        };
        assert_repair(
            toml_health(
                "[mcp_servers.origin]\ncommand = \"/gone/origin-mcp\"\n",
                &[],
                only_present,
            ),
            RepairReason::CommandNotFound,
        );
        assert_eq!(
            toml_health(
                "[mcp_servers.wenlan]\ncommand = \"/opt/wenlan-mcp\"\n\n[mcp_servers.origin]\ncommand = \"/gone/origin-mcp\"\n",
                &[],
                only_present,
            ),
            EntryHealth::Healthy
        );
    }

    #[test]
    fn a_config_that_could_not_be_read_is_not_a_healthy_or_broken_entry() {
        let which = resolves(&[]);
        let absent = |_: &Path| CandidateProbe::Absent;
        let probes = health_probes(HostKind::MacOs, &which, &absent);
        assert!(matches!(
            entry_health_reading("cursor", &ConfigRead::Unreadable("denied".into()), &probes),
            EntryHealth::Unreadable { .. }
        ));
        match json_health("not json", &[], is_a_file) {
            EntryHealth::Unreadable { error } => assert!(error.contains("not valid JSON")),
            other => panic!("an unparseable config reported {other:?}"),
        }
        match toml_health("not toml [", &[], is_a_file) {
            EntryHealth::Unreadable { error } => assert!(error.contains("not valid TOML")),
            other => panic!("an unparseable config reported {other:?}"),
        }
    }

    #[test]
    fn a_codex_toml_entry_is_validated_the_same_way() {
        assert_eq!(
            toml_health(
                "[mcp_servers.wenlan]\ncommand = \"/opt/wenlan-mcp\"\nargs = []\n",
                &[],
                is_a_file
            ),
            EntryHealth::Healthy
        );
        assert_repair(
            toml_health(
                "[mcp_servers.wenlan]\ncommand = \"/gone/wenlan-mcp\"\n",
                &[],
                |_| CandidateProbe::Absent,
            ),
            RepairReason::CommandNotFound,
        );
        assert_repair(
            toml_health("[mcp_servers.wenlan]\nargs = []\n", &[], is_a_file),
            RepairReason::CommandMissing,
        );
        assert_repair(
            toml_health(
                "[mcp_servers.wenlan]\ncommand = \"/opt/wenlan-mcp\"\nargs = \"-y\"\n",
                &[],
                is_a_file,
            ),
            RepairReason::ArgsInvalid,
        );
        assert_repair(
            toml_health(
                "[mcp_servers.wenlan]\ncommand = \"npx\"\nargs = [\"-y\", \"other\"]\n",
                &["npx"],
                is_a_file,
            ),
            RepairReason::ArgsInvalid,
        );
    }

    /// On Windows a bare `npx.cmd` / `wenlan-mcp.exe` is looked up by its stem
    /// (the resolver adds the launcher extensions); elsewhere the name is used
    /// as written.
    #[test]
    fn windows_launcher_extensions_are_looked_up_by_stem() {
        let body =
            r#"{"mcpServers": {"wenlan": {"command": "npx.cmd", "args": ["-y", "wenlan-mcp"]}}}"#;
        let which = resolves(&["npx"]);
        let probe = |_: &Path| CandidateProbe::Absent;
        let on = |host| {
            entry_health_reading(
                "cursor",
                &ConfigRead::Contents(body.to_string()),
                &health_probes(host, &which, &probe),
            )
        };
        assert_eq!(on(HostKind::Windows), EntryHealth::Healthy);
        assert_repair(on(HostKind::Linux), RepairReason::CommandNotFound);

        assert_eq!(command_stem("C:\\Program Files\\nodejs\\NPX.CMD"), "npx");
        assert_eq!(command_stem("/usr/local/bin/npx"), "npx");
        assert_eq!(command_stem("wenlan-mcp.exe"), "wenlan-mcp");
        assert_eq!(command_stem("npx"), "npx");
        // Only the extensions the installer actually looks for are launcher
        // extensions: `.com` is not one, and a name that merely ends in the
        // letters (`nodecmd`) is not `node.cmd`.
        assert_eq!(command_stem("C:\\tools\\npx.bat"), "npx");
        assert_eq!(command_stem("npx.com"), "npx.com");
        assert_eq!(command_stem("nodecmd"), "nodecmd");
        for ext in WINDOWS_LAUNCHER_EXTENSIONS {
            assert_eq!(command_stem(&format!("tool.{ext}")), "tool", ".{ext}");
        }
    }

    /// The real probe, on real files: the three ways a path that exists can
    /// still not be a program.
    #[test]
    fn real_files_a_folder_an_empty_file_and_a_real_binary() {
        let tmp = tempfile::tempdir().unwrap();
        let which = resolves(&[]);
        let probes = health_probes(HostKind::MacOs, &which, &probe_candidate);
        let health_of = |command: &Path| {
            let body = serde_json::json!({
                "mcpServers": {"wenlan": {"command": command.to_str().unwrap()}}
            })
            .to_string();
            entry_health_reading("cursor", &ConfigRead::Contents(body), &probes)
        };

        let binary = tmp.path().join("wenlan-mcp");
        write_binary_fixture(&binary);
        assert_eq!(health_of(&binary), EntryHealth::Healthy, "control");

        assert_repair(
            health_of(&tmp.path().join("not-there")),
            RepairReason::CommandNotFound,
        );
        assert_repair(health_of(tmp.path()), RepairReason::CommandNotRunnable);

        let empty = tmp.path().join("empty");
        std::fs::write(&empty, b"").unwrap();
        assert_repair(health_of(&empty), RepairReason::CommandNotRunnable);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let plain = tmp.path().join("plain");
            std::fs::write(&plain, b"#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert_repair(health_of(&plain), RepairReason::CommandNotRunnable);
        }
    }

    /// End to end through detection, then through the existing repair path:
    /// a stale entry is `needs_repair` while still `already_configured`;
    /// `write_wenlan_entry_with` (what `write_mcp_config` calls) fixes exactly
    /// Wenlan's own entry and nothing else in the file.
    #[test]
    fn repairing_a_stale_entry_rewrites_only_wenlans_own_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let cursor_dir = home.join(".cursor");
        std::fs::create_dir_all(&cursor_dir).unwrap();
        let config_path = cursor_dir.join("mcp.json");
        std::fs::write(
            &config_path,
            r#"{
  "theme": "dark",
  "mcpServers": {
    "other": {"command": "other-server", "args": ["--x"]},
    "wenlan": {"command": "/uninstalled/wenlan-mcp", "args": []}
  }
}"#,
        )
        .unwrap();
        let machine = Machine::new(HostKind::MacOs);

        let before = machine.detect(home);
        let cursor = row(&before, "cursor");
        assert_eq!(cursor.already_configured, Reading::Yes, "an entry EXISTS");
        assert_repair(cursor.entry_health.clone(), RepairReason::CommandNotFound);

        let binary = home.join("bin").join("wenlan-mcp");
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        write_binary_fixture(&binary);
        write_wenlan_entry_with(
            &config_path,
            false,
            McpEntryDecision::Write {
                entry: WenlanMcpEntry {
                    command: binary.to_str().unwrap().to_string(),
                    args: Vec::new(),
                },
                undetermined: Vec::new(),
            },
        )
        .unwrap();

        let after = machine.detect(home);
        assert_eq!(row(&after, "cursor").entry_health, EntryHealth::Healthy);
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
        assert_eq!(written["theme"], "dark", "unrelated keys survive");
        assert_eq!(
            written["mcpServers"]["other"],
            serde_json::json!({"command": "other-server", "args": ["--x"]}),
            "a sibling server is untouched"
        );
        assert_eq!(
            written["mcpServers"]["wenlan"]["command"],
            binary.to_str().unwrap()
        );
        assert!(
            config_path.with_extension("json.bak").exists(),
            "the old file is backed up before it is rewritten"
        );
    }

    /// Same for Codex's TOML, where comments and unrelated tables must survive.
    #[test]
    fn repairing_a_stale_codex_entry_keeps_the_rest_of_the_toml() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let codex_dir = home.join(".codex");
        std::fs::create_dir_all(&codex_dir).unwrap();
        let config_path = codex_dir.join("config.toml");
        std::fs::write(
            &config_path,
            "# my settings\nmodel = \"gpt-5.5\"\n\n[mcp_servers.wenlan]\ncommand = \"/uninstalled/wenlan-mcp\"\nargs = []\n\n[mcp_servers.other]\ncommand = \"other-server\"\n",
        )
        .unwrap();
        let machine = Machine::new(HostKind::MacOs);

        let before = machine.detect(home);
        assert_repair(
            row(&before, "codex_cli").entry_health.clone(),
            RepairReason::CommandNotFound,
        );

        let binary = home.join("wenlan-mcp");
        write_binary_fixture(&binary);
        write_wenlan_entry_toml_with(
            &config_path,
            McpEntryDecision::Write {
                entry: WenlanMcpEntry {
                    command: binary.to_str().unwrap().to_string(),
                    args: Vec::new(),
                },
                undetermined: Vec::new(),
            },
        )
        .unwrap();

        let after = machine.detect(home);
        assert_eq!(row(&after, "codex_cli").entry_health, EntryHealth::Healthy);
        let text = std::fs::read_to_string(&config_path).unwrap();
        assert!(text.contains("# my settings"), "{text}");
        assert!(text.contains("model = \"gpt-5.5\""), "{text}");
        assert!(text.contains("[mcp_servers.other]"), "{text}");
    }

    /// The wire contract the UI (`McpInstallState` / `McpEntryHealth` in
    /// `src/lib/tauri.ts`) is written against. A rename here is a breaking
    /// change to the frontend, so it fails this test first.
    #[test]
    fn the_new_fields_serialize_to_the_shape_the_frontend_expects() {
        fn json(value: &impl Serialize) -> serde_json::Value {
            serde_json::to_value(value).unwrap()
        }
        assert_eq!(
            json(&InstallState::Installed),
            serde_json::json!({"kind": "installed"})
        );
        assert_eq!(
            json(&InstallState::ConfigOnly),
            serde_json::json!({"kind": "config_only"})
        );
        assert_eq!(
            json(&InstallState::NotFound),
            serde_json::json!({"kind": "not_found"})
        );
        assert_eq!(
            json(&InstallState::Unreadable { error: "e".into() }),
            serde_json::json!({"kind": "unreadable", "error": "e"})
        );
        assert_eq!(
            json(&EntryHealth::NoEntry),
            serde_json::json!({"kind": "no_entry"})
        );
        assert_eq!(
            json(&EntryHealth::Healthy),
            serde_json::json!({"kind": "healthy"})
        );
        assert_eq!(
            json(&EntryHealth::Unreadable { error: "e".into() }),
            serde_json::json!({"kind": "unreadable", "error": "e"})
        );
        for (reason, wire) in [
            (RepairReason::CommandMissing, "command_missing"),
            (RepairReason::CommandNotFound, "command_not_found"),
            (RepairReason::CommandNotRunnable, "command_not_runnable"),
            (RepairReason::ArgsInvalid, "args_invalid"),
        ] {
            assert_eq!(
                json(&EntryHealth::NeedsRepair {
                    reason,
                    detail: "d".into()
                }),
                serde_json::json!({"kind": "needs_repair", "reason": wire, "detail": "d"})
            );
        }

        // And on the row itself, beside the existing fields.
        let tmp = tempfile::tempdir().unwrap();
        let machine = Machine::new(HostKind::MacOs);
        let clients = machine.detect(tmp.path());
        let value = json(row(&clients, "gemini_cli"));
        assert_eq!(
            value["install_state"],
            serde_json::json!({"kind": "not_found"})
        );
        assert_eq!(
            value["entry_health"],
            serde_json::json!({"kind": "no_entry"})
        );
        assert_eq!(value["detected"], serde_json::json!({"kind": "no"}));
    }

    // ── Muse Code: `mcp_servers` + `schema_version` + `mode: optional` ───

    /// The wenlan entry a Muse Code write is expected to produce for `command`
    /// and a named client: the shared `{command, args}` plus the two fields the
    /// Muse dialect adds.
    fn muse_entry(command: &str, args: &[&str]) -> serde_json::Value {
        serde_json::json!({
            "transport": "stdio",
            "mode": "optional",
            "command": command,
            "args": args,
        })
    }

    /// A decision that writes `command` for Muse Code, named the way
    /// `write_wenlan_entry` names it.
    fn muse_write() -> McpEntryDecision {
        with_agent_name(staged_write(), agent_name_for_client("muse_code"))
    }

    fn read_json(path: &Path) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn muse_code_dialect_is_chosen_by_client_type_alone() {
        assert_eq!(JsonDialect::for_client("muse_code"), JsonDialect::Muse);
        assert_eq!(JsonDialect::Muse.servers_key(), "mcp_servers");
        for client_type in ["cursor", "claude_code", "claude_desktop", "gemini_cli"] {
            assert_eq!(
                JsonDialect::for_client(client_type),
                JsonDialect::McpServers,
                "{client_type}"
            );
        }
        assert_eq!(JsonDialect::McpServers.servers_key(), "mcpServers");
        assert_eq!(agent_name_for_client("muse_code"), Some("muse-code"));
    }

    /// The other dialects write exactly `{command, args}` and no root keys: the
    /// Muse extras must not leak into Cursor's or Gemini CLI's files.
    #[test]
    fn mcp_servers_dialect_adds_nothing_beside_command_and_args() {
        let mut entry = serde_json::json!({"command": "c", "args": []});
        JsonDialect::McpServers.decorate_entry(&mut entry);
        assert_eq!(entry, serde_json::json!({"command": "c", "args": []}));
        let mut root = serde_json::json!({});
        JsonDialect::McpServers.prepare_root(&mut root);
        assert_eq!(root, serde_json::json!({}));
    }

    #[test]
    fn muse_code_config_path_is_the_documented_settings_file() {
        let home = PathBuf::from("/fixture/home");
        // `config_dir` is deliberately a different place: Muse Code's file is
        // under the home directory on every platform.
        let config_dir = PathBuf::from("/fixture/Library/Application Support");
        assert_eq!(
            client_config_path_for("muse_code", Some(&home), Some(&config_dir)),
            ClientConfigPath::Known(home.join(".config").join("muse").join("settings.json"))
        );
        assert!(matches!(
            client_config_path_for("muse_code", None, Some(&config_dir)),
            ClientConfigPath::Undetermined(_)
        ));
    }

    /// An absent file is created with the three things Muse Code needs to load
    /// it: the schema version, the snake_case servers key, and a non-fatal
    /// server. Through the real entry point, so the agent name is Muse's.
    #[test]
    #[serial_test::serial]
    fn muse_code_write_creates_a_loadable_settings_file() {
        let _env = EnvGuard::capture(&[MCP_RESOLVER_HOME_ENV, "WENLAN_MCP_DEV_BIN"]);
        let tmp = tempfile::tempdir().unwrap();
        let installed = install_wenlan_mcp_into(tmp.path());
        std::env::remove_var("WENLAN_MCP_DEV_BIN");
        std::env::set_var(MCP_RESOLVER_HOME_ENV, tmp.path());

        let config_path = tmp
            .path()
            .join(".config")
            .join("muse")
            .join("settings.json");
        assert!(!config_path.parent().unwrap().exists(), "control");
        write_wenlan_entry(&config_path, "muse_code").unwrap();

        let command = installed.to_str().unwrap();
        let written = read_json(&config_path);
        assert_eq!(
            written,
            serde_json::json!({
                "schema_version": 1,
                "mcp_servers": {
                    "wenlan": muse_entry(command, &["--agent-name", "muse-code"]),
                },
            })
        );
        // Not under the key every other client uses, which Muse Code ignores.
        assert!(written.get("mcpServers").is_none());
        // No existing file, so no backup of one.
        assert!(!config_path.with_extension("json.bak").exists());
    }

    #[test]
    fn muse_code_write_preserves_siblings_unrelated_keys_and_the_existing_schema_version() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("settings.json");
        let original = serde_json::json!({
            "schema_version": 7,
            "theme": "dark",
            "mcp_servers": {
                "other": {"transport": "stdio", "command": "other-cmd", "mode": "required"},
            },
            // A Cursor-style key Muse Code does not read must not be touched.
            "mcpServers": {"cursor-only": {"command": "x"}},
        });
        std::fs::write(&config_path, original.to_string()).unwrap();

        write_wenlan_entry_in(&config_path, false, JsonDialect::Muse, muse_write()).unwrap();

        let written = read_json(&config_path);
        assert_eq!(
            written["schema_version"], 7,
            "an existing version is never overwritten"
        );
        assert_eq!(written["theme"], "dark");
        assert_eq!(
            written["mcp_servers"]["other"],
            original["mcp_servers"]["other"]
        );
        assert_eq!(written["mcpServers"], original["mcpServers"]);
        assert_eq!(
            written["mcp_servers"]["wenlan"],
            muse_entry("/opt/wenlan/bin/wenlan-mcp", &["--agent-name", "muse-code"])
        );
        // The pre-write bytes are kept.
        assert_eq!(read_json(&config_path.with_extension("json.bak")), original);
    }

    /// A settings file that exists but predates `schema_version` is the one a
    /// Muse Code refuses as "malformed settings file"; writing the entry adds
    /// the key so the file loads. `mcp_servers` is created when absent.
    #[test]
    fn muse_code_write_adds_schema_version_only_when_it_is_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("settings.json");
        std::fs::write(&config_path, r#"{"theme": "dark"}"#).unwrap();
        write_wenlan_entry_in(&config_path, false, JsonDialect::Muse, muse_write()).unwrap();
        let written = read_json(&config_path);
        assert_eq!(written["schema_version"], 1);
        assert_eq!(written["theme"], "dark");
        assert!(written["mcp_servers"]["wenlan"].is_object());

        // `0` is a value somebody put there: still not ours to replace.
        std::fs::write(&config_path, r#"{"schema_version": 0}"#).unwrap();
        write_wenlan_entry_in(&config_path, false, JsonDialect::Muse, muse_write()).unwrap();
        assert_eq!(read_json(&config_path)["schema_version"], 0);
    }

    #[test]
    fn muse_code_write_replaces_the_legacy_origin_entry_and_refreshes_a_stale_wenlan_one() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("settings.json");
        std::fs::write(
            &config_path,
            serde_json::json!({
                "schema_version": 1,
                "mcp_servers": {
                    "origin": {"transport": "stdio", "command": "/gone/origin-mcp"},
                    "wenlan": {"transport": "stdio", "mode": "required", "command": "/gone/wenlan-mcp"},
                    "other": {"transport": "stdio", "command": "x"},
                },
            })
            .to_string(),
        )
        .unwrap();

        write_wenlan_entry_in(&config_path, false, JsonDialect::Muse, muse_write()).unwrap();

        let contents = std::fs::read_to_string(&config_path).unwrap();
        let written = read_json(&config_path);
        assert!(written["mcp_servers"]["origin"].is_null(), "{contents}");
        assert_eq!(written["mcp_servers"]["other"]["command"], "x");
        assert_eq!(
            written["mcp_servers"]["wenlan"],
            muse_entry("/opt/wenlan/bin/wenlan-mcp", &["--agent-name", "muse-code"]),
            "a repair must also reset a `required` mode back to `optional`"
        );
        assert!(!parsed(has_both_raw_entries_in(
            JsonDialect::Muse,
            &contents
        )));
    }

    /// The shape guard names the key the file actually uses, and refuses before
    /// anything is backed up or written.
    #[test]
    fn muse_code_write_refuses_a_mcp_servers_that_is_not_an_object() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("settings.json");
        let original = r#"{"schema_version": 1, "mcp_servers": []}"#;
        std::fs::write(&config_path, original).unwrap();

        let error = write_wenlan_entry_in(&config_path, false, JsonDialect::Muse, muse_write())
            .expect_err("a list cannot hold a keyed entry")
            .to_string();
        assert!(error.contains("`mcp_servers`"), "{error}");
        assert_eq!(std::fs::read_to_string(&config_path).unwrap(), original);
        assert!(!config_path.with_extension("json.bak").exists());

        // And a Cursor-style `mcpServers` list is not Muse Code's problem.
        std::fs::write(&config_path, r#"{"mcpServers": []}"#).unwrap();
        write_wenlan_entry_in(&config_path, false, JsonDialect::Muse, muse_write()).unwrap();
    }

    #[test]
    fn muse_code_detection_reads_mcp_servers_and_only_mcp_servers() {
        let muse = r#"{"schema_version": 1, "mcp_servers": {"wenlan": {"command": "c"}}}"#;
        let legacy = r#"{"schema_version": 1, "mcp_servers": {"origin": {"command": "c"}}}"#;
        let both = r#"{"mcp_servers": {"wenlan": {}, "origin": {}}}"#;
        let cursor_style = r#"{"schema_version": 1, "mcpServers": {"wenlan": {"command": "c"}}}"#;
        let other = r#"{"schema_version": 1, "mcp_servers": {"other": {"command": "c"}}}"#;

        let configured =
            |body: &str| raw_entry_reading("muse_code", &ConfigRead::Contents(body.into()));
        assert!(yes(configured(muse)));
        assert!(yes(configured(legacy)));
        assert!(!yes(configured(other)));
        assert!(!yes(configured("{}")));
        assert!(
            !yes(configured(cursor_style)),
            "an `mcpServers` entry is one Muse Code never reads"
        );
        // …and the reverse: a Muse-style file is not a Cursor entry.
        assert!(!yes(raw_entry_reading(
            "cursor",
            &ConfigRead::Contents(muse.into())
        )));

        let duplicate =
            |body: &str| raw_duplicate_reading("muse_code", &ConfigRead::Contents(body.into()));
        assert!(yes(duplicate(both)));
        assert!(!yes(duplicate(muse)));
        assert!(!yes(duplicate(cursor_style)));

        // An unparseable body is unmeasurable, not "not configured".
        assert!(matches!(configured("not json"), Reading::Unreadable { .. }));
        assert_eq!(configured_absent(), Reading::No);
    }

    fn configured_absent() -> Reading {
        raw_entry_reading("muse_code", &ConfigRead::Absent)
    }

    /// Writer and detector agree: what `write_wenlan_entry_in` wrote is read
    /// back as configured, and a removal clears it again.
    #[test]
    fn muse_code_round_trips_write_detect_and_remove() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("settings.json");
        assert_eq!(
            client_config_has_raw_entry("muse_code", &config_path),
            Reading::No
        );

        write_wenlan_entry_in(&config_path, false, JsonDialect::Muse, muse_write()).unwrap();
        assert!(yes(client_config_has_raw_entry("muse_code", &config_path)));
        assert!(!yes(client_config_has_raw_entry("cursor", &config_path)));

        remove_wenlan_entry_for(&config_path, "muse_code").unwrap();
        assert!(!yes(client_config_has_raw_entry("muse_code", &config_path)));
        let after = read_json(&config_path);
        assert_eq!(
            after["schema_version"], 1,
            "removal leaves the rest of the file alone"
        );
        assert!(after["mcp_servers"].as_object().unwrap().is_empty());

        // Nothing left to remove is the caller's message, not a silent success.
        let error = remove_wenlan_entry_for(&config_path, "muse_code")
            .unwrap_err()
            .to_string();
        assert!(error.contains("No Wenlan MCP entry"), "{error}");
    }

    #[test]
    fn muse_code_removal_keeps_siblings_and_legacy_removal_keeps_wenlan() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("settings.json");
        let body = serde_json::json!({
            "schema_version": 1,
            "theme": "dark",
            "mcp_servers": {
                "wenlan": muse_entry("/w/wenlan-mcp", &["--agent-name", "muse-code"]),
                "origin": {"transport": "stdio", "command": "/gone/origin-mcp"},
                "other": {"transport": "stdio", "command": "x"},
            },
        });
        std::fs::write(&config_path, body.to_string()).unwrap();
        assert!(yes(client_config_has_both_raw_entries(
            "muse_code",
            &config_path
        )));

        remove_legacy_origin_entry_for(&config_path, "muse_code").unwrap();
        let after = read_json(&config_path);
        assert!(after["mcp_servers"]["origin"].is_null());
        assert_eq!(
            after["mcp_servers"]["wenlan"],
            body["mcp_servers"]["wenlan"]
        );
        assert_eq!(after["mcp_servers"]["other"], body["mcp_servers"]["other"]);
        assert!(!yes(client_config_has_both_raw_entries(
            "muse_code",
            &config_path
        )));
        assert!(
            remove_legacy_origin_entry_for(&config_path, "muse_code").is_err(),
            "a second removal has nothing to remove"
        );

        remove_wenlan_entry_for(&config_path, "muse_code").unwrap();
        let after = read_json(&config_path);
        assert!(after["mcp_servers"]["wenlan"].is_null());
        assert_eq!(after["mcp_servers"]["other"], body["mcp_servers"]["other"]);
        assert_eq!(after["theme"], "dark");
        assert_eq!(after["schema_version"], 1);
    }

    /// Removal under the wrong dialect finds nothing, instead of deleting an
    /// entry from a key the client does not use.
    #[test]
    fn removal_under_the_wrong_dialect_removes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("settings.json");
        let original = r#"{"schema_version": 1, "mcp_servers": {"wenlan": {"command": "c"}}}"#;
        std::fs::write(&config_path, original).unwrap();
        assert!(remove_wenlan_entry(&config_path).is_err());
        assert!(remove_legacy_origin_entry(&config_path).is_err());
        assert_eq!(std::fs::read_to_string(&config_path).unwrap(), original);
        assert!(!config_path.with_extension("json.bak").exists());
    }

    #[test]
    fn muse_code_entry_health_reads_the_mcp_servers_entry() {
        let healthy = |body: &str, names: &[&str]| {
            let which = resolves(names);
            entry_health_reading(
                "muse_code",
                &ConfigRead::Contents(body.to_string()),
                &health_probes(HostKind::MacOs, &which, &is_a_file),
            )
        };
        let entry = |command: &str| {
            serde_json::json!({
                "schema_version": 1,
                "mcp_servers": {"wenlan": muse_entry(command, &["--agent-name", "muse-code"])},
            })
            .to_string()
        };

        assert_eq!(
            healthy(&entry("/opt/wenlan/bin/wenlan-mcp"), &[]),
            EntryHealth::Healthy
        );
        assert_eq!(
            healthy(&entry("wenlan-mcp"), &["wenlan-mcp"]),
            EntryHealth::Healthy
        );
        assert_repair(
            healthy(&entry("wenlan-mcp"), &[]),
            RepairReason::CommandNotFound,
        );
        let no_command =
            r#"{"schema_version": 1, "mcp_servers": {"wenlan": {"transport": "stdio"}}}"#;
        assert_repair(healthy(no_command, &[]), RepairReason::CommandMissing);

        // An `mcpServers` entry is invisible to Muse Code, so it is no entry.
        let cursor_style =
            r#"{"mcpServers": {"wenlan": {"command": "/opt/wenlan/bin/wenlan-mcp"}}}"#;
        assert_eq!(healthy(cursor_style, &[]), EntryHealth::NoEntry);
        assert_eq!(healthy("{}", &[]), EntryHealth::NoEntry);
        assert!(matches!(
            healthy("not json", &[]),
            EntryHealth::Unreadable { .. }
        ));
    }

    /// The `muse` CLI and the `~/.config/muse` folder are the two traces; the
    /// settings file is a third, config-only one.
    #[test]
    fn muse_code_is_found_by_its_cli_and_remembered_by_its_home_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();

        let machine = Machine::new(HostKind::Linux);
        assert_eq!(
            machine.install_state(home, "muse_code"),
            InstallState::NotFound,
            "control: an empty machine has no Muse Code"
        );

        let mut machine = Machine::new(HostKind::Linux);
        machine.on_path = vec!["muse"];
        let clients = machine.detect(home);
        assert_eq!(
            row(&clients, "muse_code").install_state,
            InstallState::Installed
        );
        assert_eq!(row(&clients, "muse_code").detected, Reading::Yes);
        assert_eq!(row(&clients, "muse_code").name, "Muse Code");

        let mut machine = Machine::new(HostKind::MacOs);
        machine.existing = vec![home.join(".config").join("muse")];
        let clients = machine.detect(home);
        assert_eq!(
            row(&clients, "muse_code").install_state,
            InstallState::ConfigOnly
        );
        assert_eq!(row(&clients, "muse_code").detected, Reading::Yes);

        // The settings file alone is a leftover too.
        let settings = home.join(".config").join("muse").join("settings.json");
        std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
        std::fs::write(
            &settings,
            r#"{"schema_version": 1, "mcp_servers": {"wenlan": {"command": "c"}}}"#,
        )
        .unwrap();
        let clients = Machine::new(HostKind::MacOs).detect(home);
        let muse = row(&clients, "muse_code");
        assert_eq!(muse.install_state, InstallState::ConfigOnly);
        assert_eq!(muse.config_path.as_deref(), settings.to_str());
        assert_eq!(muse.already_configured, Reading::Yes);
        assert_eq!(muse.has_raw_entry, Reading::Yes);
        assert_eq!(
            muse.has_plugin,
            Reading::No,
            "Muse Code has no plugin route"
        );
        // Not a neighbour's row: nothing else claims Muse's settings file.
        assert_eq!(row(&clients, "gemini_cli").has_raw_entry, Reading::No);
    }

    // ── Antigravity, GitHub Copilot, VS Code, Windsurf, Kiro, OpenCode ───

    /// What each platform's `dirs` lookups report, as fixtures: the home
    /// directory and the application-configuration directory.
    fn host_dirs() -> Vec<(HostKind, PathBuf, PathBuf)> {
        vec![
            (
                HostKind::MacOs,
                PathBuf::from("/Users/a"),
                PathBuf::from("/Users/a/Library/Application Support"),
            ),
            (
                HostKind::Linux,
                PathBuf::from("/home/a"),
                PathBuf::from("/home/a/.config"),
            ),
            (
                HostKind::Windows,
                PathBuf::from(r"C:\Users\a"),
                PathBuf::from(r"C:\Users\a\AppData\Roaming"),
            ),
        ]
    }

    fn joined(base: &Path, tail: &[&str]) -> PathBuf {
        tail.iter()
            .fold(base.to_path_buf(), |acc, part| acc.join(part))
    }

    #[test]
    fn new_client_config_paths_on_each_os() {
        for (host, home, config) in host_dirs() {
            for (client_type, tail) in [
                (
                    "antigravity",
                    [".gemini", "config", "mcp_config.json"].as_slice(),
                ),
                ("copilot_cli", &[".copilot", "mcp-config.json"]),
                ("windsurf", &[".codeium", "windsurf", "mcp_config.json"]),
                ("kiro", &[".kiro", "settings", "mcp.json"]),
                // Not `config_dir`, on any OS.
                ("opencode", &[".config", "opencode", "opencode.json"]),
            ] {
                assert_eq!(
                    client_config_path_for(client_type, Some(&home), Some(&config)),
                    ClientConfigPath::Known(joined(&home, tail)),
                    "{client_type} on {host:?}"
                );
            }
            // VS Code's user mcp.json is under the config dir, and only that.
            assert_eq!(
                client_config_path_for("vscode", Some(&home), Some(&config)),
                ClientConfigPath::Known(joined(&config, &["Code", "User", "mcp.json"])),
                "vscode on {host:?}"
            );
            assert_eq!(
                client_config_path_for("vscode", None, Some(&config)),
                ClientConfigPath::Known(joined(&config, &["Code", "User", "mcp.json"])),
            );
            match client_config_path_for("vscode", Some(&home), None) {
                ClientConfigPath::Undetermined(why) => {
                    assert!(why.contains("application-configuration"), "{why}")
                }
                other => panic!("vscode without a config dir reported {other:?}"),
            }
        }
    }

    #[test]
    fn new_clients_pick_their_dialect_and_agent_name() {
        for (client_type, dialect, key, agent) in [
            (
                "antigravity",
                JsonDialect::McpServers,
                "mcpServers",
                "antigravity",
            ),
            (
                "copilot_cli",
                JsonDialect::Copilot,
                "mcpServers",
                "copilot-cli",
            ),
            ("vscode", JsonDialect::VsCode, "servers", "vscode"),
            (
                "windsurf",
                JsonDialect::McpServers,
                "mcpServers",
                "windsurf",
            ),
            ("kiro", JsonDialect::McpServers, "mcpServers", "kiro"),
            ("opencode", JsonDialect::OpenCode, "mcp", "opencode"),
        ] {
            assert_eq!(
                JsonDialect::for_client(client_type),
                dialect,
                "{client_type}"
            );
            assert_eq!(dialect.servers_key(), key, "{client_type}");
            assert_eq!(agent_name_for_client(client_type), Some(agent));
        }
    }

    /// The entry each new dialect writes for `/w/wenlan-mcp --agent-name <n>`.
    fn dialect_entry(dialect: JsonDialect, command: &str, agent: &str) -> serde_json::Value {
        let args = ["--agent-name", agent];
        match dialect {
            JsonDialect::Copilot => serde_json::json!({
                "type": "local", "tools": ["*"], "command": command, "args": args,
            }),
            JsonDialect::VsCode => serde_json::json!({
                "type": "stdio", "command": command, "args": args,
            }),
            JsonDialect::OpenCode => serde_json::json!({
                "type": "local",
                "command": [command, "--agent-name", agent],
                "enabled": true,
            }),
            other => panic!("not a new dialect: {other:?}"),
        }
    }

    /// Write, detect, migrate the legacy `origin` entry, back up, and remove —
    /// the same contract Cursor's `mcpServers` file has — for each dialect
    /// that is not plain `{command, args}`.
    #[test]
    fn new_dialects_round_trip_write_detect_migrate_and_remove() {
        for (client_type, dialect) in [
            ("copilot_cli", JsonDialect::Copilot),
            ("vscode", JsonDialect::VsCode),
            ("opencode", JsonDialect::OpenCode),
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let config_path = tmp.path().join("nested").join("config.json");
            let key = dialect.servers_key();
            let agent = agent_name_for_client(client_type).unwrap();
            let decision = || with_agent_name(staged_write(), Some(agent));
            let expected = dialect_entry(dialect, "/opt/wenlan/bin/wenlan-mcp", agent);

            // A fresh file: created, holding only the servers key.
            assert_eq!(
                client_config_has_raw_entry(client_type, &config_path),
                Reading::No
            );
            write_wenlan_entry_in(&config_path, false, dialect, decision()).unwrap();
            assert_eq!(
                read_json(&config_path),
                serde_json::json!({ key: { "wenlan": expected.clone() } }),
                "{client_type}"
            );
            assert!(!config_path.with_extension("json.bak").exists());
            assert!(yes(client_config_has_raw_entry(client_type, &config_path)));

            // An existing file with a legacy entry, a sibling and unrelated
            // keys: the legacy entry goes, the rest stays, the old bytes are
            // backed up, and no duplicate is left.
            let original = serde_json::json!({
                "theme": "dark",
                key: {
                    "origin": {"command": "/gone/origin-mcp"},
                    "other": {"command": "x"},
                },
            });
            std::fs::write(&config_path, original.to_string()).unwrap();
            assert!(yes(client_config_has_raw_entry(client_type, &config_path)));
            write_wenlan_entry_in(&config_path, false, dialect, decision()).unwrap();
            let written = read_json(&config_path);
            assert_eq!(written["theme"], "dark", "{client_type}");
            assert_eq!(written[key]["other"], original[key]["other"]);
            assert!(written[key]["origin"].is_null(), "{client_type}");
            assert_eq!(written[key]["wenlan"], expected, "{client_type}");
            assert_eq!(read_json(&config_path.with_extension("json.bak")), original);
            assert!(!yes(client_config_has_both_raw_entries(
                client_type,
                &config_path
            )));

            // The raw duplicate is seen, and the legacy removal keeps wenlan.
            let mut both = written.clone();
            both[key]["origin"] = serde_json::json!({"command": "/gone/origin-mcp"});
            std::fs::write(&config_path, both.to_string()).unwrap();
            assert!(yes(client_config_has_both_raw_entries(
                client_type,
                &config_path
            )));
            remove_legacy_origin_entry_for(&config_path, client_type).unwrap();
            let after = read_json(&config_path);
            assert!(after[key]["origin"].is_null());
            assert_eq!(after[key]["wenlan"], expected);

            // Removal clears exactly Wenlan's entry.
            remove_wenlan_entry_for(&config_path, client_type).unwrap();
            let after = read_json(&config_path);
            assert!(after[key]["wenlan"].is_null(), "{client_type}");
            assert_eq!(after[key]["other"], original[key]["other"]);
            assert_eq!(
                client_config_has_raw_entry(client_type, &config_path),
                Reading::No
            );
            assert!(remove_wenlan_entry_for(&config_path, client_type).is_err());
        }
    }

    /// Each dialect reads its own key only: a Cursor-style `mcpServers` entry
    /// is invisible to VS Code and OpenCode, and theirs to Cursor.
    #[test]
    fn new_dialects_read_their_own_key_and_only_it() {
        let cursor_style = r#"{"mcpServers": {"wenlan": {"command": "c"}}}"#;
        let vscode_style = r#"{"servers": {"wenlan": {"type": "stdio", "command": "c"}}}"#;
        let opencode_style = r#"{"mcp": {"wenlan": {"type": "local", "command": ["c"]}}}"#;
        let configured = |client: &str, body: &str| {
            yes(raw_entry_reading(
                client,
                &ConfigRead::Contents(body.into()),
            ))
        };
        assert!(configured("copilot_cli", cursor_style));
        assert!(configured("vscode", vscode_style));
        assert!(!configured("vscode", cursor_style));
        assert!(configured("opencode", opencode_style));
        assert!(!configured("opencode", cursor_style));
        assert!(!configured("cursor", vscode_style));
        assert!(!configured("cursor", opencode_style));
        for client in ["antigravity", "windsurf", "kiro"] {
            assert!(configured(client, cursor_style), "{client}");
            assert!(!configured(client, vscode_style), "{client}");
        }
        assert!(matches!(
            raw_entry_reading("vscode", &ConfigRead::Contents("// jsonc\n{}".into())),
            Reading::Unreadable { .. }
        ));
    }

    /// The plain `mcpServers` clients write exactly what Cursor writes, named
    /// for themselves.
    #[test]
    #[serial_test::serial]
    fn mcp_servers_clients_write_command_and_args_named_for_themselves() {
        let _env = EnvGuard::capture(&[MCP_RESOLVER_HOME_ENV, "WENLAN_MCP_DEV_BIN"]);
        let tmp = tempfile::tempdir().unwrap();
        let installed = install_wenlan_mcp_into(tmp.path());
        std::env::remove_var("WENLAN_MCP_DEV_BIN");
        std::env::set_var(MCP_RESOLVER_HOME_ENV, tmp.path());
        let command = installed.to_str().unwrap();

        for (client_type, agent) in [
            ("antigravity", "antigravity"),
            ("windsurf", "windsurf"),
            ("kiro", "kiro"),
        ] {
            let config_path = client_config_path_for(client_type, Some(tmp.path()), None).unwrap();
            write_wenlan_entry(&config_path, client_type).unwrap();
            assert_eq!(
                read_json(&config_path),
                serde_json::json!({"mcpServers": {"wenlan": {
                    "command": command, "args": ["--agent-name", agent],
                }}}),
                "{client_type}"
            );
        }
        // And the decorated dialects through the real entry point.
        for client_type in ["copilot_cli", "opencode"] {
            let config_path = client_config_path_for(client_type, Some(tmp.path()), None).unwrap();
            write_wenlan_entry(&config_path, client_type).unwrap();
            let dialect = JsonDialect::for_client(client_type);
            assert_eq!(
                read_json(&config_path)[dialect.servers_key()]["wenlan"],
                dialect_entry(
                    dialect,
                    command,
                    agent_name_for_client(client_type).unwrap()
                ),
            );
        }
    }

    /// OpenCode with only `opencode.jsonc`: unreadable to detection, and a
    /// write or removal is refused without touching either file.
    #[test]
    fn an_opencode_jsonc_only_config_is_unreadable_and_never_rewritten() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let config_path = client_config_path_for("opencode", Some(home), None).unwrap();
        let jsonc = config_path.with_extension("jsonc");
        std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        let body = "{\n  // mine\n  \"mcp\": {\"wenlan\": {\"type\": \"local\", \"command\": [\"c\"]},},\n}\n";
        std::fs::write(&jsonc, body).unwrap();

        let reading = client_config_has_raw_entry("opencode", &config_path);
        match &reading {
            Reading::Unreadable { error } => assert!(error.contains("opencode.jsonc"), "{error}"),
            other => panic!("a JSONC-only OpenCode reported {other:?}"),
        }

        let error = write_wenlan_entry_in(
            &config_path,
            false,
            JsonDialect::OpenCode,
            with_agent_name(staged_write(), Some("opencode")),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("Nothing was written"), "{error}");
        assert!(error.contains("opencode.jsonc"), "{error}");
        assert!(
            !config_path.exists(),
            "no opencode.json is created beside it"
        );
        assert_eq!(std::fs::read_to_string(&jsonc).unwrap(), body);
        assert!(remove_wenlan_entry_for(&config_path, "opencode").is_err());
        assert!(remove_legacy_origin_entry_for(&config_path, "opencode").is_err());
        assert_eq!(std::fs::read_to_string(&jsonc).unwrap(), body);

        // Through detection: the row says unreadable, never "not configured",
        // and the entry is not judged. The folder still says OpenCode was here.
        let mut machine = Machine::new(HostKind::Linux);
        machine.existing = vec![home.join(".config").join("opencode")];
        let clients = machine.detect(home);
        let opencode = row(&clients, "opencode");
        assert!(matches!(opencode.has_raw_entry, Reading::Unreadable { .. }));
        assert!(matches!(
            opencode.already_configured,
            Reading::Unreadable { .. }
        ));
        assert!(matches!(
            opencode.entry_health,
            EntryHealth::Unreadable { .. }
        ));
        assert_eq!(opencode.detected, Reading::Yes);
        assert_eq!(opencode.install_state, InstallState::ConfigOnly);

        // Only the JSONC-only case: with an `opencode.json` too, that file is
        // the one Wenlan edits, and the `.jsonc` is left alone.
        std::fs::write(
            &config_path,
            r#"{"$schema": "https://opencode.ai/config.json"}"#,
        )
        .unwrap();
        write_wenlan_entry_in(
            &config_path,
            false,
            JsonDialect::OpenCode,
            with_agent_name(staged_write(), Some("opencode")),
        )
        .unwrap();
        let written = read_json(&config_path);
        assert_eq!(written["$schema"], "https://opencode.ai/config.json");
        assert!(written["mcp"]["wenlan"].is_object());
        assert_eq!(std::fs::read_to_string(&jsonc).unwrap(), body);

        // The rule is OpenCode's alone: a `.jsonc` beside another client's
        // absent file changes nothing.
        let cursor = home.join("mcp.json");
        std::fs::write(home.join("mcp.jsonc"), "{}").unwrap();
        assert_eq!(client_config_has_raw_entry("cursor", &cursor), Reading::No);
    }

    #[test]
    fn opencode_entry_health_reads_the_command_list() {
        let health = |entry: serde_json::Value, names: &[&str]| {
            let which = resolves(names);
            entry_health_reading(
                "opencode",
                &ConfigRead::Contents(serde_json::json!({"mcp": {"wenlan": entry}}).to_string()),
                &health_probes(HostKind::MacOs, &which, &is_a_file),
            )
        };
        assert_eq!(
            health(
                dialect_entry(
                    JsonDialect::OpenCode,
                    "/opt/wenlan/bin/wenlan-mcp",
                    "opencode"
                ),
                &[]
            ),
            EntryHealth::Healthy
        );
        assert_eq!(
            health(
                serde_json::json!({"type": "local", "command": ["wenlan-mcp"]}),
                &["wenlan-mcp"]
            ),
            EntryHealth::Healthy
        );
        assert_repair(
            health(
                serde_json::json!({"type": "local", "command": ["wenlan-mcp"]}),
                &[],
            ),
            RepairReason::CommandNotFound,
        );
        // npx is judged on the list's tail, as `args` elsewhere.
        assert_eq!(
            health(
                serde_json::json!({"command": ["npx", "-y", "wenlan-mcp"]}),
                &["npx"]
            ),
            EntryHealth::Healthy
        );
        assert_repair(
            health(
                serde_json::json!({"command": ["npx", "-y", "other"]}),
                &["npx"],
            ),
            RepairReason::ArgsInvalid,
        );
        // The shape every other client uses is one OpenCode refuses.
        let detail = assert_repair(
            health(
                serde_json::json!({"command": "/opt/wenlan/bin/wenlan-mcp", "args": []}),
                &[],
            ),
            RepairReason::ArgsInvalid,
        );
        assert!(detail.contains("list"), "{detail}");
        assert_repair(
            health(serde_json::json!({"command": ["/w", 3]}), &[]),
            RepairReason::ArgsInvalid,
        );
        assert_repair(
            health(serde_json::json!({"type": "local", "command": []}), &[]),
            RepairReason::CommandMissing,
        );
        assert_eq!(
            health(
                serde_json::json!({"type": "remote", "url": "https://x"}),
                &[]
            ),
            EntryHealth::Healthy
        );
        // A `servers`/`mcpServers` entry is not OpenCode's.
        let which = resolves(&[]);
        assert_eq!(
            entry_health_reading(
                "opencode",
                &ConfigRead::Contents(r#"{"mcpServers": {"wenlan": {"command": "/w"}}}"#.into()),
                &health_probes(HostKind::MacOs, &which, &is_a_file),
            ),
            EntryHealth::NoEntry
        );
    }

    #[test]
    fn copilot_and_vscode_entry_health_read_their_entry() {
        let which = resolves(&[]);
        let probes = health_probes(HostKind::MacOs, &which, &is_a_file);
        for (client_type, dialect) in [
            ("copilot_cli", JsonDialect::Copilot),
            ("vscode", JsonDialect::VsCode),
        ] {
            let body = serde_json::json!({ dialect.servers_key(): {
                "wenlan": dialect_entry(dialect, "/opt/wenlan/bin/wenlan-mcp", "x"),
            }})
            .to_string();
            assert_eq!(
                entry_health_reading(client_type, &ConfigRead::Contents(body), &probes),
                EntryHealth::Healthy,
                "{client_type}"
            );
            let broken = serde_json::json!({ dialect.servers_key(): {
                "wenlan": {"type": "stdio", "command": "wenlan-mcp"},
            }})
            .to_string();
            assert_repair(
                entry_health_reading(client_type, &ConfigRead::Contents(broken), &probes),
                RepairReason::CommandNotFound,
            );
        }
    }

    /// Every app location, one at a time, on each OS: each case makes exactly
    /// that path the only thing on the machine, after an empty-machine control.
    #[test]
    fn vscode_family_apps_are_found_on_each_os() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let local = home.join("AppData").join("Local");
        let program_files = home.join("PF");
        for (client_type, bundle, folder, exe, linux) in [
            (
                "antigravity",
                "Antigravity.app",
                "Antigravity",
                "Antigravity.exe",
                ["/usr/share/antigravity/antigravity"].as_slice(),
            ),
            (
                "vscode",
                "Visual Studio Code.app",
                "Microsoft VS Code",
                "Code.exe",
                &["/usr/share/code/code", "/snap/bin/code"],
            ),
            (
                "windsurf",
                "Windsurf.app",
                "Windsurf",
                "Windsurf.exe",
                &["/usr/share/windsurf/windsurf"],
            ),
            (
                "kiro",
                "Kiro.app",
                "Kiro",
                "Kiro.exe",
                &["/usr/share/kiro/kiro"],
            ),
        ] {
            let mut cases: Vec<(HostKind, PathBuf)> = vec![
                (HostKind::MacOs, PathBuf::from("/Applications").join(bundle)),
                (HostKind::MacOs, home.join("Applications").join(bundle)),
                (
                    HostKind::Windows,
                    local.join("Programs").join(folder).join(exe),
                ),
                (HostKind::Windows, program_files.join(folder).join(exe)),
            ];
            cases.extend(linux.iter().map(|p| (HostKind::Linux, PathBuf::from(p))));
            for (host, hit) in cases {
                let mut machine = Machine::new(host);
                machine.local = Some(local.clone());
                machine.program_files = vec![program_files.clone()];
                assert_eq!(
                    machine.install_state(home, client_type),
                    InstallState::NotFound,
                    "control: {client_type} on an empty {host:?} machine"
                );
                machine.existing = vec![hit.clone()];
                assert_eq!(
                    machine.install_state(home, client_type),
                    InstallState::Installed,
                    "{client_type}: {} was not probed on {host:?}",
                    hit.display()
                );
            }
        }
        // A Windows machine whose local-data folder is unknown has not been
        // searched, so it is not "not found".
        assert!(matches!(
            Machine::new(HostKind::Windows).install_state(home, "vscode"),
            InstallState::Unreadable { .. }
        ));
    }

    #[test]
    fn new_clients_are_found_by_their_cli() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        for (client_type, names) in [
            ("antigravity", ["agy", "antigravity"].as_slice()),
            ("copilot_cli", &["copilot"]),
            ("vscode", &["code"]),
            ("windsurf", &["windsurf"]),
            ("kiro", &["kiro", "kiro-cli"]),
            ("opencode", &["opencode"]),
        ] {
            assert_eq!(
                Machine::new(HostKind::Linux).install_state(home, client_type),
                InstallState::NotFound,
                "control: {client_type}"
            );
            for name in names {
                let mut machine = Machine::new(HostKind::Linux);
                machine.on_path = vec![*name];
                let clients = machine.detect(home);
                assert_eq!(
                    row(&clients, client_type).install_state,
                    InstallState::Installed,
                    "{client_type} via `{name}`"
                );
                // The CLI is that client's only: no neighbour claims it.
                for other in clients.iter().filter(|c| c.client_type != client_type) {
                    assert_ne!(
                        other.install_state,
                        InstallState::Installed,
                        "`{name}` also installed {}",
                        other.client_type
                    );
                }
            }
        }
    }

    #[test]
    fn new_clients_home_folders_are_leftovers() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        for (client_type, folder) in [
            ("antigravity", [".gemini", "config"].as_slice()),
            ("antigravity", &[".gemini", "antigravity"]),
            ("copilot_cli", &[".copilot", "config.json"]),
            ("copilot_cli", &[".copilot", "logs"]),
            ("windsurf", &[".codeium", "windsurf"]),
            ("kiro", &[".kiro"]),
            ("opencode", &[".config", "opencode"]),
        ] {
            let mut machine = Machine::new(HostKind::MacOs);
            machine.existing = vec![joined(home, folder)];
            let clients = machine.detect(home);
            let client = row(&clients, client_type);
            assert_eq!(
                client.install_state,
                InstallState::ConfigOnly,
                "{client_type}: {folder:?}"
            );
            assert_eq!(client.detected, Reading::Yes);
        }
        // Wenlan's own skills folders are not traces of a tool: VS Code's
        // skills live in `~/.copilot/skills`, and Gemini CLI's (shared with
        // Antigravity) in `~/.gemini/skills`.
        let mut machine = Machine::new(HostKind::MacOs);
        machine.existing = vec![
            home.join(".copilot"),
            home.join(".copilot").join("skills"),
            home.join(".gemini"),
            home.join(".gemini").join("skills"),
        ];
        let clients = machine.detect(home);
        assert_eq!(
            row(&clients, "copilot_cli").install_state,
            InstallState::NotFound
        );
        assert_eq!(
            row(&clients, "antigravity").install_state,
            InstallState::NotFound
        );
        assert_eq!(
            row(&clients, "vscode").install_state,
            InstallState::NotFound,
            "control: VS Code leaves nothing in the home folder this app looks for"
        );
    }

    #[test]
    fn has_skills_is_reported_for_skills_clients_only() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let mut skills: Vec<&str> = crate::agent_skills_bundle::FILES
            .iter()
            .filter_map(|(path, _)| path.split('/').next())
            .collect();
        skills.sort_unstable();
        skills.dedup();

        let clients = Machine::new(HostKind::MacOs).detect(home);
        for client_type in [
            "cursor",
            "gemini_cli",
            "antigravity",
            "copilot_cli",
            "vscode",
            "windsurf",
            "kiro",
            "opencode",
        ] {
            assert_eq!(
                row(&clients, client_type).has_skills,
                Some(Reading::No),
                "{client_type}"
            );
        }
        for client_type in ["claude_code", "codex_cli", "claude_desktop", "muse_code"] {
            assert_eq!(row(&clients, client_type).has_skills, None, "{client_type}");
        }

        // Every marker present under Kiro's folder: yes for Kiro only.
        let mut machine = Machine::new(HostKind::MacOs);
        let kiro = home.join(".kiro").join("skills");
        machine.existing = skills
            .iter()
            .map(|skill| kiro.join(skill).join(crate::skills_install::MANAGED_MARKER))
            .collect();
        let clients = machine.detect(home);
        assert_eq!(row(&clients, "kiro").has_skills, Some(Reading::Yes));
        assert_eq!(row(&clients, "cursor").has_skills, Some(Reading::No));

        // A marker the OS would not stat is unknown, not "no skills".
        let mut machine = Machine::new(HostKind::MacOs);
        machine.unreadable = vec![kiro
            .join(skills[0])
            .join(crate::skills_install::MANAGED_MARKER)];
        machine.existing = skills
            .iter()
            .skip(1)
            .map(|skill| kiro.join(skill).join(crate::skills_install::MANAGED_MARKER))
            .collect();
        let clients = machine.detect(home);
        assert!(matches!(
            row(&clients, "kiro").has_skills,
            Some(Reading::Unreadable { .. })
        ));

        // No home: unknown for skills clients, still absent for the others.
        let clients = detect_mcp_clients_from(None, None, |_| Reading::No);
        assert!(matches!(
            row(&clients, "opencode").has_skills,
            Some(Reading::Unreadable { .. })
        ));
        assert_eq!(row(&clients, "muse_code").has_skills, None);

        // On the wire: `null` for a client with no skills folder.
        let value = serde_json::to_value(row(
            &Machine::new(HostKind::MacOs).detect(home),
            "claude_code",
        ))
        .unwrap();
        assert_eq!(value["has_skills"], serde_json::Value::Null);
        let value =
            serde_json::to_value(row(&Machine::new(HostKind::MacOs).detect(home), "cursor"))
                .unwrap();
        assert_eq!(value["has_skills"], serde_json::json!({"kind": "no"}));
    }

    #[test]
    fn new_client_rows_carry_their_names() {
        let tmp = tempfile::tempdir().unwrap();
        let clients = Machine::new(HostKind::MacOs).detect(tmp.path());
        for (client_type, name) in [
            ("antigravity", "Antigravity"),
            ("copilot_cli", "GitHub Copilot"),
            ("vscode", "VS Code"),
            ("windsurf", "Windsurf"),
            ("kiro", "Kiro"),
            ("opencode", "OpenCode"),
        ] {
            let client = row(&clients, client_type);
            assert_eq!(client.name, name);
            assert_eq!(
                client.has_plugin,
                Reading::No,
                "{client_type} has no plugin route"
            );
            assert_eq!(client.install_state, InstallState::NotFound);
        }
    }
}
