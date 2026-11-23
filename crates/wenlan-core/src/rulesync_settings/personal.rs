// SPDX-License-Identifier: Apache-2.0
//! Native, home-scoped settings and the credential-free personal source.

use std::{
    collections::{BTreeMap, BTreeSet},
    io::ErrorKind,
    ops::Range,
    path::{Path, PathBuf},
    sync::LazyLock,
};

use anyhow::{ensure, Context, Result};
use cap_fs_ext::{DirExt as _, FollowSymlinks, OpenOptionsFollowExt as _};
use cap_std::fs::{Dir, OpenOptions};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{
    document::{self, Format, Part},
    validate_text, MAX_SETTINGS_BYTES,
};
use crate::{
    rulesync_personal::{check_delivery_paths, personal_target},
    rulesync_personal_files::{require_plain_personal_directory, require_plain_personal_file},
    skill_registry::printable,
};
use wenlan_types::rulesync::AdoptionChange;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Feature {
    Mcp,
    Hook,
    Permission,
}

impl Feature {
    pub fn label(self) -> &'static str {
        match self {
            Self::Mcp => "MCP server",
            Self::Hook => "hook",
            Self::Permission => "permission",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    Map,
    Leaves,
    Hooks,
    Decisions,
    PolicyRules,
    OrderedPolicy,
    Items,
    Field,
}

#[derive(Clone, Copy)]
struct Section(Feature, &'static [&'static str], Shape);

use Feature::{Hook, Mcp, Permission};
use Shape::Map as Named;
use Shape::{Decisions, Field, Hooks, Items, Leaves, PolicyRules};

#[derive(Clone)]
pub struct Location {
    pub tool: &'static str,
    pub path: PathBuf,
    format: Format,
    sections: &'static [Section],
    initial: &'static str,
}

fn json_file(tool: &'static str, path: &str, sections: &'static [Section]) -> Location {
    Location {
        tool,
        path: path.into(),
        format: Format::Json,
        sections,
        initial: "{}\n",
    }
}

fn versioned(
    tool: &'static str,
    path: &str,
    sections: &'static [Section],
    initial: &'static str,
) -> Location {
    Location {
        initial,
        ..json_file(tool, path, sections)
    }
}

fn toml_file(tool: &'static str, path: &str, sections: &'static [Section]) -> Location {
    Location {
        tool,
        path: path.into(),
        format: Format::Toml,
        sections,
        initial: "",
    }
}

const HOOKS: &[Section] = &[Section(Hook, &["hooks"], Hooks)];
const OPENCODE_V1: &[Section] = &[
    Section(Mcp, &["mcp"], Named),
    Section(Permission, &["permission"], Shape::OrderedPolicy),
];
const OPENCODE_V2: &[Section] = &[
    Section(Mcp, &["mcp", "servers"], Named),
    Section(Permission, &["permissions"], Shape::OrderedPolicy),
];

fn opencode_sections(raw: &Value) -> Result<&'static [Section]> {
    // Both releases have used this unversioned schema URL. It is
    // corroboration of the tool, not evidence of its config generation.
    ensure!(
        raw.get("$schema")
            .is_none_or(|schema| schema == "https://opencode.ai/config.json"),
        "unknown OpenCode schema"
    );
    ensure!(
        raw.get("permission")
            .is_none_or(|policy| policy.is_string() || policy.is_object())
            && raw.get("permissions").is_none_or(Value::is_array)
            && raw.get("mcp").is_none_or(Value::is_object),
        "invalid OpenCode settings shape"
    );
    let legacy_server = |value: &Value| {
        matches!(
            value.get("type").and_then(Value::as_str),
            Some("local" | "remote")
        ) || value.as_object().is_some_and(|fields| {
            fields.len() == 1 && fields.get("enabled").is_some_and(Value::is_boolean)
        })
    };
    let mcp = raw.get("mcp").and_then(Value::as_object);
    let legacy = ["permission", "snapshot", "attachment"]
        .into_iter()
        .any(|key| raw.get(key).is_some())
        || mcp.is_some_and(|map| map.values().any(legacy_server));
    let current = ["permissions", "snapshots", "attachments", "policies"]
        .into_iter()
        .any(|key| raw.get(key).is_some())
        || mcp
            .and_then(|map| map.get("servers"))
            .is_some_and(|value| value.is_object() && !legacy_server(value));
    match (legacy, current) {
        (true, false) => Ok(OPENCODE_V1),
        (false, true) => Ok(OPENCODE_V2),
        _ => anyhow::bail!("OpenCode V1 or V2 settings shape is ambiguous"),
    }
}

static DEFAULTS: LazyLock<Vec<Location>> = LazyLock::new(|| {
    vec![
        json_file("claudecode", ".claude.json", &[Section(Mcp, &["mcpServers"], Named)]),
        json_file("claudecode", ".claude/settings.json", &[
            Section(Hook, &["hooks"], Hooks),
            Section(Permission, &["permissions"], Decisions),
            Section(Permission, &["sandbox"], Leaves),
        ]),
        toml_file("codexcli", ".codex/config.toml", &[
            Section(Mcp, &["mcp_servers"], Named),
            Section(Hook, &["hooks"], Hooks),
            Section(Permission, &["permissions"], Leaves),
            Section(Permission, &["default_permissions"], Field),
            Section(Permission, &["approval_policy"], Field),
            Section(Permission, &["approvals_reviewer"], Field),
            Section(Permission, &["sandbox_mode"], Field),
            Section(Permission, &["sandbox_workspace_write"], Leaves),
        ]),
        json_file("codexcli", ".codex/hooks.json", HOOKS),
        json_file("geminicli", ".gemini/settings.json", &[
            Section(Mcp, &["mcpServers"], Named),
            Section(Hook, &["hooks"], Hooks),
            Section(Permission, &["tools", "allowed"], Items),
            Section(Permission, &["tools", "exclude"], Items),
            Section(Permission, &["tools", "core"], Items),
            Section(Permission, &["tools", "sandbox"], Field),
            Section(Permission, &["mcp", "allowed"], Items),
            Section(Permission, &["mcp", "excluded"], Items),
        ]),
        json_file("antigravity-ide", ".gemini/config/mcp_config.json", &[Section(Mcp, &["mcpServers"], Named)]),
        json_file("antigravity-ide", ".gemini/config/hooks.json", &[Section(Hook, &[], Named)]),
        json_file("antigravity-ide", ".gemini/antigravity-cli/settings.json", &[
            Section(Permission, &["permissions"], Decisions),
        ]),
        json_file("copilotcli", ".copilot/mcp-config.json", &[Section(Mcp, &["mcpServers"], Named)]),
        versioned("copilotcli", ".copilot/hooks/copilot-hooks.json", HOOKS, "{\"version\":1}\n"),
        json_file("copilotcli", ".copilot/settings.json", &[
            Section(Permission, &["defaultPermissionMode"], Field),
            Section(Permission, &["allowedUrls"], Items),
            Section(Permission, &["permissions"], Leaves),
        ]),
        json_file("cursor", ".cursor/mcp.json", &[Section(Mcp, &["mcpServers"], Named)]),
        versioned("cursor", ".cursor/hooks.json", HOOKS, "{\"version\":1}\n"),
        versioned("cursor", ".cursor/cli-config.json", &[Section(Permission, &["permissions"], Decisions)],
            "{\"version\":1,\"editor\":{\"vimMode\":false},\"permissions\":{\"allow\":[],\"deny\":[]}}\n"),
        json_file("cursor", ".cursor/permissions.json", &[
            Section(Permission, &["mcpAllowlist"], Items),
            Section(Permission, &["terminalAllowlist"], Items),
            Section(Permission, &["autoRun", "allow_instructions"], Items),
            Section(Permission, &["autoRun", "block_instructions"], Items),
        ]),
        json_file("opencode", ".config/opencode/opencode.jsonc", &[]),
        json_file("opencode", ".config/opencode/opencode.json", &[]),
        versioned("musecode", ".config/muse/settings.json", &[Section(Mcp, &["mcp_servers"], Named)],
            "{\"schema_version\":1}\n"),
    ]
});

const DYNAMIC_LOCATIONS: &[(&str, &str)] = &[
    (".copilot/hooks", "json"),
    (".gemini/policies", "toml"),
    (".codex/rules", "rules"),
    (".config/opencode/plugins", ""),
];

pub fn is_settings_path(path: &Path) -> bool {
    DEFAULTS.iter().any(|location| location.path == path)
        || DYNAMIC_LOCATIONS.iter().any(|(dir, ext)| {
            !ext.is_empty()
                && path.parent() == Some(Path::new(dir))
                && path.extension().is_some_and(|extension| extension == *ext)
        })
}

const POLICIES: &[Section] = &[Section(Permission, &["rule"], PolicyRules)];
const COMMAND_RULES: &[Section] = &[Section(Permission, &["rules"], PolicyRules)];

pub struct Catalog {
    pub locations: Vec<Location>,
    pub notes: Vec<String>,
}

impl Catalog {
    pub fn read(home: &Path) -> Result<Self> {
        let root = Dir::open_ambient_dir(home, cap_std::ambient_authority())?;
        let mut locations = DEFAULTS.clone();
        let defaults_len = locations.len();
        let mut notes = Vec::new();
        for &(dir, ext) in DYNAMIC_LOCATIONS {
            let dir_path = Path::new(dir);
            let Ok(Some(directory)) = open_parent(&root, dir_path) else {
                if present(&root, dir_path) {
                    notes.push(format!(
                        "{dir}: unmanaged because the directory is linked or unreadable."
                    ));
                }
                continue;
            };
            if ext.is_empty() {
                if directory.entries()?.next().is_some() {
                    notes.push(format!(
                        "{dir}: executable plugins and their hooks stay unmanaged."
                    ));
                }
                continue;
            }
            for entry in directory.entries()?.take(2048) {
                let name = entry?.file_name();
                let path = dir_path.join(name);
                if path.extension().and_then(|ext| ext.to_str()) != Some(ext) {
                    continue;
                }
                if !path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| safe_identifier(name) && !credential_key(name))
                {
                    notes.push(format!(
                        "{dir}: a settings filename was omitted because it is unsafe."
                    ));
                    continue;
                }
                let native = path.to_str().context("settings path is not UTF-8")?;
                match ext {
                    "json" if locations.iter().all(|location| location.path != path) => {
                        locations.push(versioned("copilotcli", native, HOOKS, "{\"version\":1}\n"));
                    }
                    "toml" => locations.push(toml_file("geminicli", native, POLICIES)),
                    "rules" => locations.push(Location {
                        tool: "codexcli",
                        path,
                        format: Format::CommandRules,
                        sections: COMMAND_RULES,
                        initial: "",
                    }),
                    _ => {}
                }
            }
        }
        // Keep preferred destinations ordered ahead of fallback paths.
        locations[defaults_len..].sort_by(|left, right| left.path.cmp(&right.path));
        notes.sort();
        notes.dedup();
        Ok(Self { locations, notes })
    }

    pub fn retain_managed_locations<'a>(&mut self, paths: impl IntoIterator<Item = &'a Path>) {
        for path in paths {
            let (Some(parent), Some(ext), Some(name)) = (
                path.parent().and_then(Path::to_str),
                path.extension().and_then(|ext| ext.to_str()),
                path.file_name().and_then(|name| name.to_str()),
            ) else {
                continue;
            };
            if !safe_identifier(name) || self.locations.iter().any(|location| location.path == path)
            {
                continue;
            }
            let location = match (parent.replace('\\', "/").as_str(), ext) {
                (".copilot/hooks", "json") => versioned(
                    "copilotcli",
                    &path.to_string_lossy(),
                    HOOKS,
                    "{\"version\":1}\n",
                ),
                (".gemini/policies", "toml") => {
                    toml_file("geminicli", &path.to_string_lossy(), POLICIES)
                }
                (".codex/rules", "rules") => Location {
                    tool: "codexcli",
                    path: path.to_owned(),
                    format: Format::CommandRules,
                    sections: COMMAND_RULES,
                    initial: "",
                },
                _ => continue,
            };
            self.locations.push(location);
        }
    }
}

fn open_parent(root: &Dir, path: &Path) -> std::io::Result<Option<Dir>> {
    let mut directory = root.try_clone()?;
    for component in path.components() {
        match directory.open_dir_nofollow(Path::new(component.as_os_str())) {
            Ok(next) => {
                require_plain_personal_directory(&next)?;
                directory = next;
            }
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        }
    }
    Ok(Some(directory))
}

fn present(root: &Dir, path: &Path) -> bool {
    root.symlink_metadata(path).is_ok()
}

fn read_local(root: &Dir, path: &Path) -> Result<Option<Vec<u8>>> {
    let Some(parent) = open_parent(root, path.parent().context("invalid settings path")?)? else {
        return Ok(None);
    };
    let name = path.file_name().context("invalid settings name")?;
    let metadata = match parent.symlink_metadata(name) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    ensure!(
        metadata.is_file() && metadata.len() <= MAX_SETTINGS_BYTES,
        "settings file is unsafe or too large"
    );
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt as _;
        options.custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32);
    }
    let file = parent.open_with(name, &options)?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file(),
        "settings file changed during inspection"
    );
    require_plain_personal_file(&file, &metadata)?;
    super::read_limited(file).map(Some)
}

/// A native settings identity. Arrays have stable, format-specific member
/// identities so an unowned sibling can remain byte-for-byte in place.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Key {
    pub feature: Feature,
    pub name: Vec<String>,
    member: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub key: Key,
    pub value: Value,
    container: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    order: Vec<Vec<String>>,
}

impl Entry {
    pub fn label(&self) -> String {
        let name = self.key.name.join(".");
        let name = if let Some(member) = self
            .key
            .member
            .as_deref()
            .filter(|member| !member.is_empty())
        {
            format!("{name} [{member}]")
        } else {
            name
        };
        format!("{} {}", self.key.feature.label(), printable(&name))
    }

    pub fn preview(&self) -> Result<String> {
        let mut preview = format!(
            "{} = {}",
            self.label(),
            printable(&serde_json::to_string(&self.value)?)
        );
        if !self.order.is_empty() {
            preview.push_str(" (key order ");
            preview.push_str(&printable(&serde_json::to_string(&self.order)?));
            preview.push(')');
        }
        Ok(preview)
    }

    pub fn can_reapply(&self) -> bool {
        match self.key.feature {
            Mcp => true,
            Permission => false,
            Hook => matches!(
                self.container.as_slice(),
                [hooks, event] if hooks == "hooks" && matches!(event.as_str(), "SessionStart" | "sessionStart")
            ),
        }
    }
}

pub struct Document {
    // The full native document must never enter a preview or source.
    raw: Value,
    sections: &'static [Section],
    entries: BTreeMap<Key, Vec<(Entry, Option<usize>)>>,
}

/// The exact fragment delivered at one native setting. The insertion layout
/// includes only the separators and scaffolding needed to undo that edit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordedEntry {
    original: OriginalEntry,
    delivered: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum OriginalEntry {
    Existing(Vec<u8>),
    Inserted {
        prefix: Vec<u8>,
        suffix: Vec<u8>,
        replaced: Vec<u8>,
    },
}

impl RecordedEntry {
    pub fn changed(&self) -> bool {
        !matches!(&self.original, OriginalEntry::Existing(bytes) if *bytes == self.delivered)
    }
}

impl Location {
    pub fn unsupported_shape(&self) -> &'static str {
        if self.tool == "opencode" {
            "the file does not identify an unambiguous OpenCode V1 or V2 settings shape"
        } else {
            "the settings format is invalid, duplicate, or unsupported"
        }
    }

    pub fn display(&self) -> String {
        self.path
            .iter()
            .map(|part| part.to_string_lossy())
            .collect::<Vec<_>>()
            .join("/")
    }

    pub fn document(&self, bytes: Option<&[u8]>) -> Result<Document> {
        ensure!(
            bytes.is_none_or(|bytes| bytes.len() as u64 <= MAX_SETTINGS_BYTES),
            "settings file exceeds 10 MiB"
        );
        let bytes = bytes.unwrap_or(self.initial.as_bytes());
        let raw = document::read(self.format, bytes)?;
        let sections = if self.tool == "opencode" {
            opencode_sections(&raw)?
        } else {
            self.sections
        };
        let mut doc = Document {
            raw,
            sections,
            entries: BTreeMap::new(),
        };
        for &Section(feature, path, shape) in sections {
            if shape == Shape::OrderedPolicy {
                if let Some(value) = lookup(&doc.raw, path.iter().copied()) {
                    let mut entry = self.entry(feature, path, value.clone());
                    if value.is_object() {
                        entry.order = document::object_order(bytes, path)?;
                    }
                    doc.entries.insert(entry.key.clone(), vec![(entry, None)]);
                }
            } else {
                doc.extract(self, feature, path, shape);
            }
        }
        Ok(doc)
    }

    pub fn edit(&self, bytes: Option<&[u8]>, entry: &Entry) -> Result<Vec<u8>> {
        ensure!(
            entry.can_reapply(),
            "the native permission or approval hook can only be reviewed locally"
        );
        ensure!(
            reviewable(self.tool, entry),
            "the settings entry contains possible credentials"
        );
        let bytes = bytes.unwrap_or(self.initial.as_bytes());
        let doc = self.document(Some(bytes))?;
        let path = doc.path_to(entry)?;
        let next = document::edit(self.format, bytes, &path, entry.value.clone())?;
        let checked = self.document(Some(&next))?;
        ensure!(
            checked.entry(&entry.key) == Some(entry),
            "the native settings entry did not round trip"
        );
        Ok(next)
    }

    fn span(&self, bytes: &[u8], doc: &Document, entry: &Entry) -> Result<Range<usize>> {
        document::entry_span(self.format, bytes, &doc.path_to(entry)?)
    }

    pub fn record_original(
        &self,
        bytes: &[u8],
        doc: &Document,
        entry: &Entry,
    ) -> Result<RecordedEntry> {
        let delivered = bytes[self.span(bytes, doc, entry)?].to_vec();
        Ok(RecordedEntry {
            original: OriginalEntry::Existing(delivered.clone()),
            delivered,
        })
    }

    pub fn record_review(
        &self,
        receipt: &RecordedEntry,
        bytes: &[u8],
        doc: &Document,
        entry: &Entry,
    ) -> Result<RecordedEntry> {
        Ok(RecordedEntry {
            original: receipt.original.clone(),
            delivered: bytes[self.span(bytes, doc, entry)?].to_vec(),
        })
    }

    pub fn record_write(
        &self,
        receipt: Option<&RecordedEntry>,
        before: Option<&[u8]>,
        after: &[u8],
        entry: &Entry,
    ) -> Result<RecordedEntry> {
        let doc = self.document(Some(after))?;
        if let Some(receipt) = receipt {
            return self.record_review(receipt, after, &doc, entry);
        }
        let before = before.unwrap_or(self.initial.as_bytes());
        let span = self.span(after, &doc, entry)?;
        let start = before
            .iter()
            .zip(after)
            .take_while(|(a, b)| a == b)
            .count()
            .min(span.start);
        let tail = before[start..]
            .iter()
            .rev()
            .zip(after[span.end..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        Ok(RecordedEntry {
            original: OriginalEntry::Inserted {
                prefix: after[start..span.start].to_vec(),
                suffix: after[span.end..after.len() - tail].to_vec(),
                replaced: before[start..before.len() - tail].to_vec(),
            },
            delivered: after[span].to_vec(),
        })
    }

    pub fn restore_entry(
        &self,
        bytes: &mut Vec<u8>,
        key: &Key,
        receipt: &RecordedEntry,
    ) -> Result<()> {
        let doc = self.document(Some(bytes))?;
        let entry = doc
            .entry(key)
            .context("the recorded entry is missing or ambiguous")?;
        let span = self.span(bytes, &doc, entry)?;
        ensure!(
            bytes[span.clone()] == receipt.delivered,
            "the recorded entry changed"
        );
        match &receipt.original {
            OriginalEntry::Existing(original) => {
                bytes.splice(span, original.iter().copied());
            }
            OriginalEntry::Inserted {
                prefix,
                suffix,
                replaced,
            } => {
                let start = span
                    .start
                    .checked_sub(prefix.len())
                    .context("the entry layout changed")?;
                let end = span.end + suffix.len();
                ensure!(
                    bytes[start..span.start] == *prefix && bytes.get(span.end..end) == Some(suffix),
                    "the entry layout changed"
                );
                bytes.splice(start..end, replaced.iter().copied());
            }
        }
        Ok(())
    }

    pub fn is_initial(&self, bytes: &[u8]) -> bool {
        bytes == self.initial.as_bytes()
    }

    /// A whole-file backup and exchange would also copy unrelated secrets.
    /// Such files can be inspected, but are ineligible for automatic writes.
    pub fn snapshot_safe(&self, bytes: &[u8], doc: &Document) -> bool {
        static JSON_TEXT: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r#"(?s)"(?:[^"\\]|\\.)*"|//[^\r\n]*|/\*.*?\*/"#)
                .expect("JSON strings and comments")
        });
        // The raw text, comments included, is checked for key-looking parts
        // as well as the parsed values, and comments also for a credential
        // word followed by a value (`text_risky`).
        std::str::from_utf8(bytes).is_ok_and(|text| {
            leaf_safe(&without_catalog_names(text))
                && if self.format == Format::Json {
                    JSON_TEXT
                        .find_iter(text)
                        .map(|item| item.as_str())
                        .filter(|item| item.starts_with('/'))
                        .all(|comment| !text_risky(self.tool, comment))
                } else {
                    text.lines()
                        .filter_map(|line| line.split_once('#').map(|(_, comment)| comment))
                        .all(|comment| !text_risky(self.tool, comment))
                }
        }) && material_safe(self.tool, &self.generic_part(doc))
            && doc.sections.iter().all(|&Section(feature, path, shape)| {
                let Some(value) = lookup(&doc.raw, path.iter().copied()) else {
                    return true;
                };
                match (feature, shape) {
                    (Mcp, Named) => value.as_object().is_some_and(|servers| {
                        servers.values().all(|server| {
                            wenlan_own_mcp(self.tool, server) || mcp_payload_safe(self.tool, server)
                        })
                    }),
                    (Hook, Hooks) => hook_groups_safe(self.tool, value),
                    (Hook, Named) => value.as_object().is_some_and(|groups| {
                        groups
                            .values()
                            .all(|group| hook_groups_safe(self.tool, group))
                    }),
                    _ => true,
                }
            })
    }

    /// The file as the generic check reads it. MCP servers and hooks are left
    /// to their own stricter checks, so only their names stay here. Gemini's
    /// sign-in method and Claude Code's plain number or on/off environment
    /// settings name no secret, so they are left out too.
    fn generic_part(&self, doc: &Document) -> Value {
        let mut raw = doc.raw.clone();
        for &Section(feature, path, shape) in doc.sections {
            if !matches!((feature, shape), (Mcp, Named) | (Hook, _)) {
                continue;
            }
            let mut value = Some(&mut raw);
            for name in path {
                value = value.and_then(|value| value.get_mut(*name));
            }
            if let Some(Value::Object(children)) = value {
                children.values_mut().for_each(|child| *child = json!({}));
            }
        }
        if self.tool == "geminicli" && self.path == Path::new(".gemini/settings.json") {
            if let Some(Value::Object(security)) = raw.get_mut("security") {
                if let Some(Value::Object(auth)) = security.get_mut("auth") {
                    if matches!(
                        auth.get("selectedType").and_then(Value::as_str),
                        Some("oauth-personal" | "gemini-api-key" | "vertex-ai" | "cloud-shell")
                    ) {
                        auth.remove("selectedType");
                    }
                    if auth.is_empty() {
                        security.remove("auth");
                    }
                }
            }
        }
        if self.tool == "claudecode" && self.path == Path::new(".claude/settings.json") {
            if let Some(Value::Object(env)) = raw.get_mut("env") {
                env.retain(|name, value| !plain_setting(name, value));
            }
        }
        raw
    }

    fn entry(&self, feature: Feature, path: &[&str], value: Value) -> Entry {
        Entry {
            key: Key {
                feature,
                name: path.iter().map(|name| (*name).into()).collect(),
                member: None,
            },
            container: path.iter().map(|name| (*name).into()).collect(),
            value,
            order: Vec::new(),
        }
    }
}

impl Document {
    fn path_to<'a>(&self, entry: &'a Entry) -> Result<Vec<Part<'a>>> {
        let mut path = entry
            .container
            .iter()
            .map(|part| Part::Key(part))
            .collect::<Vec<_>>();
        if entry.key.member.is_some() {
            let index = match self.entries.get(&entry.key).map(Vec::as_slice) {
                Some([(old, Some(index))]) if old.container == entry.container => *index,
                Some(_) => anyhow::bail!("this entry is ambiguous or moved to a different list"),
                None => lookup(&self.raw, entry.container.iter().map(String::as_str))
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len),
            };
            path.push(Part::Index(index));
        }
        Ok(path)
    }

    pub fn entry(&self, key: &Key) -> Option<&Entry> {
        match self.entries.get(key).map(Vec::as_slice) {
            Some([(entry, _)]) => Some(entry),
            _ => None,
        }
    }

    pub fn has(&self, key: &Key) -> bool {
        self.entries.contains_key(key)
    }

    /// Whether this file has a place for `feature` at all.
    pub fn holds(&self, feature: Feature) -> bool {
        self.sections.iter().any(|section| section.0 == feature)
    }

    pub fn reviewable_entries<'a>(&'a self, tool: &'a str) -> impl Iterator<Item = &'a Entry> {
        self.entries
            .values()
            .filter_map(move |entries| match entries.as_slice() {
                [(entry, _)] if reviewable(tool, entry) => Some(entry),
                _ => None,
            })
    }

    pub fn refusal_notices<'a>(
        &'a self,
        location: &'a Location,
    ) -> impl Iterator<Item = String> + 'a {
        let path = location.display();
        self.refused_entries(location.tool)
            .map(move |(number, feature, _)| format!(
                "{path}: skipped {} (entry {number}) because it contains possible credentials or unsafe text.",
                feature.label()
            ))
    }

    /// Entries the safety check refuses, by 1-based position and feature,
    /// and whether a key is written in the entry itself. Names are left out
    /// so a suspected value is never echoed. Wenlan's own server entry is
    /// set up per tool, so it is neither managed nor reported.
    pub fn refused_entries<'a>(
        &'a self,
        tool: &'a str,
    ) -> impl Iterator<Item = (usize, Feature, bool)> + 'a {
        self.entries
            .values()
            .flatten()
            .enumerate()
            .filter(move |(_, (entry, _))| {
                let own = entry.key.feature == Mcp && wenlan_own_mcp(tool, &entry.value);
                !own && !reviewable(tool, entry)
            })
            .map(move |(index, (entry, _))| {
                (
                    index + 1,
                    entry.key.feature,
                    entry.key.feature == Mcp && literal_key(tool, &entry.value),
                )
            })
    }

    fn extract(&mut self, location: &Location, feature: Feature, path: &[&str], shape: Shape) {
        let Some(value) = lookup(&self.raw, path.iter().copied()) else {
            return;
        };
        let mut found = Vec::new();
        match shape {
            Shape::Map => {
                if let Some(map) = value.as_object() {
                    for (name, value) in map {
                        if feature == Hook && !value.is_object() {
                            continue;
                        }
                        let mut entry_path = path.to_vec();
                        entry_path.push(name);
                        found.push((location.entry(feature, &entry_path, value.clone()), None));
                    }
                }
            }
            Field if !value.is_object() => {
                found.push((location.entry(feature, path, value.clone()), None))
            }
            Leaves => {
                collect_leaves(location, feature, &mut path.to_vec(), value, &mut found);
            }
            Decisions | Hooks => {
                if let Some(map) = value.as_object() {
                    for (name, value) in map {
                        if let Some(items) = value.as_array().filter(|_| {
                            shape == Hooks || matches!(name.as_str(), "allow" | "deny" | "ask")
                        }) {
                            for (index, item) in items.iter().enumerate() {
                                let mut full = path.to_vec();
                                full.push(name);
                                let mut entry = location.entry(feature, &full, item.clone());
                                entry.key.member = Some(if shape == Hooks {
                                    hook_identity(item, items)
                                } else {
                                    entry.key.name =
                                        path.iter().map(|name| (*name).into()).collect();
                                    item.as_str()
                                        .map(str::to_owned)
                                        .unwrap_or_else(|| item.to_string())
                                });
                                found.push((entry, Some(index)));
                            }
                        } else if shape == Decisions {
                            let mut full = path.to_vec();
                            full.push(name);
                            found.push((location.entry(feature, &full, value.clone()), None));
                        }
                    }
                }
            }
            PolicyRules | Items => {
                if let Some(items) = value.as_array() {
                    for (index, item) in items.iter().enumerate() {
                        let mut entry = location.entry(feature, path, item.clone());
                        let mut identity = item.clone();
                        if shape == PolicyRules {
                            if let Some(fields) = identity.as_object_mut() {
                                for key in ["decision", "effect", "justification", "deny_message"] {
                                    fields.remove(key);
                                }
                            }
                        }
                        entry.key.member = Some(
                            identity
                                .as_str()
                                .map_or_else(|| identity.to_string(), str::to_owned),
                        );
                        found.push((entry, Some(index)));
                    }
                }
            }
            Field | Shape::OrderedPolicy => {}
        }
        for (entry, index) in found {
            self.entries
                .entry(entry.key.clone())
                .or_default()
                .push((entry, index));
        }
    }
}

fn collect_leaves<'a>(
    location: &Location,
    feature: Feature,
    path: &mut Vec<&'a str>,
    value: &'a Value,
    found: &mut Vec<(Entry, Option<usize>)>,
) {
    if let Some(map) = value.as_object() {
        for (key, value) in map {
            path.push(key);
            collect_leaves(location, feature, path, value, found);
            path.pop();
        }
    } else {
        found.push((location.entry(feature, path, value.clone()), None));
    }
}

fn lookup<'a, 'b>(value: &'a Value, mut path: impl Iterator<Item = &'b str>) -> Option<&'a Value> {
    path.try_fold(value, |value, name| value.get(name))
}

fn hook_identity(value: &Value, siblings: &[Value]) -> String {
    let identity = |item: &Value| {
        ["id", "name", "matcher"].into_iter().find_map(|field| {
            item.get(field)
                .and_then(Value::as_str)
                .map(|name| format!("{field} {name}"))
        })
    };
    if let Some(id) = identity(value) {
        return id;
    }
    if siblings
        .iter()
        .filter(|item| identity(item).is_none())
        .count()
        == 1
    {
        return String::new();
    }
    // Flat formats have no required ID. With several unnamed commands their
    // native command is the only identity that survives a reorder.
    ["command", "bash", "powershell", "prompt"]
        .into_iter()
        .find_map(|field| value.get(field).and_then(Value::as_str))
        .unwrap_or_default()
        .to_owned()
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Adoption {
    pub sources: BTreeMap<PathBuf, Vec<Entry>>,
    pub notes: Vec<String>,
}

impl Adoption {
    pub fn discover(home: &Path) -> Result<Self> {
        let catalog = Catalog::read(home)?;
        let root = Dir::open_ambient_dir(home, cap_std::ambient_authority())?;
        let mut adoption = Self {
            sources: BTreeMap::new(),
            notes: catalog.notes,
        };
        for location in catalog.locations {
            let path = location.display();
            if check_delivery_paths(home, [location.path.as_path()]).is_err() {
                if present(&root, &location.path) {
                    adoption.notes.push(format!(
                        "{path}: unmanaged because it aliases the shared harness or has an unsafe path."
                    ));
                }
                continue;
            }
            let bytes = match read_local(&root, &location.path) {
                Ok(Some(bytes)) => bytes,
                Ok(None) => continue,
                Err(_) => {
                    adoption.notes.push(format!(
                        "{path}: unmanaged because the file is unreadable, has an unsupported mode or ACL, is too large, or has a symlink or hard link."
                    ));
                    continue;
                }
            };
            let Ok(doc) = location.document(Some(&bytes)) else {
                let reason = location.unsupported_shape();
                adoption
                    .notes
                    .push(format!("{path}: unmanaged because {reason}."));
                continue;
            };
            let mut accepted = Vec::new();
            let snapshot_safe = location.snapshot_safe(&bytes, &doc);
            adoption.notes.extend(doc.refusal_notices(&location));
            for entries in doc.entries.into_values() {
                for (entry, _) in &entries {
                    if !reviewable(location.tool, entry) {
                        continue;
                    }
                    if entries.len() > 1 {
                        adoption.notes.push(format!(
                            "{path}: skipped {} because its native identity is ambiguous.",
                            entry.label()
                        ));
                    } else {
                        accepted.push(entry.clone());
                    }
                }
            }
            if !accepted.is_empty() {
                if !snapshot_safe {
                    adoption.notes.push(format!("{path}: safe entries can be imported, but Wenlan cannot write or back up this file while it contains possible credentials."));
                }
                adoption.sources.insert(location.path, accepted);
            }
        }
        adoption.notes.sort();
        Ok(adoption)
    }

    pub fn preview(&self) -> Result<Vec<AdoptionChange>> {
        self.sources.iter().map(|(path, entries)| {
            let path = path.iter().map(|part| part.to_string_lossy()).collect::<Vec<_>>().join("/");
            let mut diff = String::from("Import personal settings. Existing file bytes stay in place until a managed entry needs delivery.\n");
            for entry in entries {
                diff.push_str(&format!("+{}\n", entry.preview()?));
            }
            Ok(AdoptionChange { path, added: entries.len(), removed: 0, diff, tool: None, notice_codes: Vec::new(), text_diff: None })
        }).collect()
    }
}

fn safe_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 160
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        && !text_risky("", value)
}

/// An environment variable name in the usual form, such as `API_TOKEN`.
/// A lowercase, mixed-case or token-looking "name" may be the key itself.
fn variable_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    name.len() <= 64
        && bytes.next().is_some_and(|byte| byte.is_ascii_uppercase())
        && bytes.all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
        && !secret_looking(name)
}

/// Text with a part that looks like a key whatever its context: a known
/// token prefix, a Slack webhook path, a run of 16 or more hex digits (a
/// UUID with its hyphens taken out counts), a run of 16 or more letters and
/// digits with any digit in it (a base32 key such as `JBSWY3DPEHPK3PXP`
/// included), or a run of 20 or more letters in mixed case that does not
/// split into camelCase words (`camel_name`).
fn secret_looking(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let prefix = [
        "ghp_",
        "gho_",
        "ghu_",
        "ghs_",
        "ghr_",
        "github_pat_",
        "sk-",
        "sk_",
        "sk-ant-",
        "rk_live_",
        "sk_live_",
        "xoxa-",
        "xoxb-",
        "xoxe-",
        "xoxp-",
        "xoxr-",
        "xoxs-",
        "akia",
        "asia",
        "eyj",
        "glpat-",
        "npm_",
    ];
    let hex = |run: &str| run.len() >= 16 && run.bytes().all(|byte| byte.is_ascii_hexdigit());
    let mixed = |run: &str| {
        (run.len() >= 16 && run.bytes().any(|byte| byte.is_ascii_digit()))
            || (run.len() >= 20
                && run.bytes().any(|byte| byte.is_ascii_uppercase())
                && run.bytes().any(|byte| byte.is_ascii_lowercase())
                && !camel_name(run))
    };
    lower.contains("hooks.slack.com/services")
        || lower
            .split(|ch: char| !(ch.is_ascii_alphanumeric() || "_-".contains(ch)))
            .any(|word| {
                prefix.iter().any(|prefix| word.starts_with(prefix))
                    || hex(&word.replace(['-', '_'], ""))
            })
        || text
            .split(|ch: char| !ch.is_ascii_alphanumeric())
            .any(|run| hex(run) || mixed(run))
}

/// Letters that split wholly into camelCase words, so they read as a name
/// such as `skipDangerousModePermissionPrompt`, not a key. Each word is an
/// optional capital and two or more lowercase letters, and may follow an
/// acronym of up to five capitals (`MCPServers`).
fn camel_name(run: &str) -> bool {
    static CAMEL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^(?:[A-Z]{0,6}[a-z]{2,})+$").expect("camelCase words"));
    CAMEL.is_match(run)
}

/// Whether no part of `text` looks like a key. Every string in a setting,
/// its names included, goes through this check, through `text_risky`.
fn leaf_safe(text: &str) -> bool {
    !secret_looking(text)
}

/// A setting name this catalog itself declares (`defaultPermissionMode`).
/// Its spelling is fixed in this file, so it holds no value of the person's
/// and is left out of the key-looking check; any other name is still checked.
fn catalog_name(name: &str) -> bool {
    DEFAULTS
        .iter()
        .map(|location| location.sections)
        .chain([HOOKS, OPENCODE_V1, OPENCODE_V2, POLICIES, COMMAND_RULES])
        .flatten()
        .any(|Section(_, path, _)| path.contains(&name))
}

/// `text` with every word that is a catalog-declared name taken out, for the
/// raw-file check.
fn without_catalog_names(text: &str) -> std::borrow::Cow<'_, str> {
    static WORD: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"[A-Za-z0-9_-]+").expect("setting words"));
    WORD.replace_all(text, |word: &regex::Captures| {
        let word = &word[0];
        if catalog_name(word) {
            String::new()
        } else {
            word.to_owned()
        }
    })
}

/// The lowercase words of a name, split at separators, at a capital after a
/// lowercase letter or digit, before the last capital of an acronym
/// (`APIKey`), and between letters and digits, so `doorCode`, `DOOR_CODE` and
/// `sqlite3` give `door code`, `door code` and `sqlite 3`.
fn name_words(name: &str) -> Vec<String> {
    let chars = name.chars().collect::<Vec<_>>();
    let mut words = Vec::new();
    let mut word = String::new();
    for (index, &ch) in chars.iter().enumerate() {
        if !ch.is_ascii_alphanumeric() {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
            continue;
        }
        if let Some(&before) = index.checked_sub(1).and_then(|before| chars.get(before)) {
            let next_lower = chars
                .get(index + 1)
                .is_some_and(|next| next.is_ascii_lowercase());
            let boundary = (ch.is_ascii_uppercase()
                && (before.is_ascii_lowercase()
                    || before.is_ascii_digit()
                    || (before.is_ascii_uppercase() && next_lower)))
                || (ch.is_ascii_digit() && before.is_ascii_alphabetic())
                || (ch.is_ascii_alphabetic() && before.is_ascii_digit());
            if boundary && !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
        }
        word.push(ch.to_ascii_lowercase());
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

/// Whole words that name a credential.
const CREDENTIAL_WORDS: &[&str] = &[
    "password",
    "passwords",
    "passwd",
    "pass",
    "pwd",
    "pw",
    "pin",
    "otp",
    "totp",
    "mfa",
    "cvc",
    "cvv",
    "puk",
    "ssn",
    "secret",
    "secrets",
    "token",
    "tokens",
    "key",
    "keys",
    "apikey",
    "auth",
    "authorization",
    "cred",
    "creds",
    "credential",
    "credentials",
    "cookie",
    "cookies",
    "session",
    "sessionid",
    "accountnumber",
];

/// Words before `code` that make it a credential, as in `DOOR_CODE`.
const CODE_WORDS: &[&str] = &[
    "auth",
    "access",
    "backup",
    "door",
    "verification",
    "otp",
    "pin",
    "recovery",
];

/// Whether a name, flag or word names a credential. Its words
/// (`name_words`) are matched whole, so `author`, `spinner`, `bypass`,
/// `keyboard`, `tokenizer` and `EXIT_CODE` name none, while `DB_PASS`,
/// `authCode` and `ACCOUNT_NUMBER` do. One word also counts when it joins
/// two credential words (`authtoken`), ends in a long credential word
/// (`PGPASSWORD`, `clientsecret`, `githubtoken`) or ends in `code` after a
/// word that makes it one (`backupcode`). A short credential word inside a
/// longer word, as in `MYPIN`, is not found (UX-SKILL-17).
fn credential_word(name: &str) -> bool {
    let credential = |word: &str| CREDENTIAL_WORDS.contains(&word);
    let words = name_words(name);
    words.iter().enumerate().any(|(index, word)| {
        let joined = (1..word.len()).any(|split| {
            let (head, tail) = word.split_at(split);
            (credential(head) && credential(tail)) || (tail == "code" && CODE_WORDS.contains(&head))
        });
        credential(word)
            || joined
            || [
                "password",
                "passwd",
                "secret",
                "token",
                "credential",
                "apikey",
            ]
            .iter()
            .any(|long| word.len() > long.len() && word.ends_with(long))
            || (word == "code"
                && index
                    .checked_sub(1)
                    .is_some_and(|before| CODE_WORDS.contains(&words[before].as_str())))
            || (word == "number" && index > 0 && words[index - 1] == "account")
            || (word == "id" && index > 0 && words[index - 1] == "session")
    })
}

/// A setting in Claude Code's `env` that names no key: a number, an on/off
/// switch or a plain string under a name with no credential word, or a number
/// under a name whose only credential word is `TOKEN(S)` and which counts or
/// times something, such as `..._MAX_OUTPUT_TOKENS`.
fn plain_setting(name: &str, value: &Value) -> bool {
    let digits = |text: &str| {
        !text.is_empty() && text.len() <= 10 && text.bytes().all(|byte| byte.is_ascii_digit())
    };
    let (number, switch, text) = match value {
        Value::String(text) => (
            digits(text),
            matches!(text.as_str(), "true" | "false"),
            Some(text.as_str()),
        ),
        Value::Number(number) => (
            number.as_u64().is_some_and(|n| digits(&n.to_string())),
            false,
            None,
        ),
        Value::Bool(_) => (false, true, None),
        _ => (false, false, None),
    };
    if text_risky("", name) {
        false
    } else if credential_key(name) {
        number && token_measure(name)
    } else {
        number || switch || text.is_some_and(|text| !text_risky("claudecode", text))
    }
}

/// A name that counts or times something and whose only credential word is
/// `token(s)`, such as `CLAUDE_CODE_MAX_OUTPUT_TOKENS`: its last word is
/// `tokens`, `limit`, `timeout`, `ms`, `seconds`, `port`, `size`, `count` or
/// `retries`. A number under it is a measure, not a key.
fn token_measure(name: &str) -> bool {
    let words = name_words(name);
    let measure = words.last().is_some_and(|word| {
        matches!(
            word.as_str(),
            "tokens"
                | "limit"
                | "timeout"
                | "ms"
                | "seconds"
                | "port"
                | "size"
                | "count"
                | "retries"
        )
    });
    let without_tokens = words
        .into_iter()
        .filter(|word| !matches!(word.as_str(), "token" | "tokens"))
        .collect::<Vec<_>>()
        .join("_");
    measure && !credential_key(&without_tokens)
}

fn credential_key(key: &str) -> bool {
    credential_word(key)
        || matches!(
            key.to_ascii_lowercase()
                .replace(['-', '_', '.', ' '], "")
                .as_str(),
            "bearer" | "signature" | "session" | "userinfo" | "username"
        )
}

fn exact_reference(tool: &str, value: &str) -> bool {
    let value = value
        .strip_prefix("Bearer ")
        .or_else(|| value.strip_prefix("Basic "))
        .unwrap_or(value);
    let wrapped = |open, close| {
        value
            .strip_prefix(open)
            .and_then(|text| text.strip_suffix(close))
            .is_some_and(variable_name)
    };
    match tool {
        "claudecode" => wrapped("${", "}"),
        "geminicli" => wrapped("${", "}") || value.strip_prefix('$').is_some_and(variable_name),
        "cursor" => wrapped("${env:", "}"),
        "opencode" => wrapped("{env:", "}"),
        // Codex's env-valued fields use bare variable names below.
        _ => false,
    }
}

/// Whether a string in a setting may hold a key. Every string leaf and name
/// goes through this: a token-looking part (`leaf_safe`), a flag or
/// assignment that carries a value (`command_risky`), a credential word
/// followed by a value (`near_credential`), and the older patterns below.
fn text_risky(tool: &str, value: &str) -> bool {
    static RISK: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
        r"(?i)(?:[a-z][a-z0-9+.-]*://[^/\s?#]*@|[a-z][a-z0-9+.-]*://[^\s]*[?#]|\b(?:sk[-_]|gh[pousr]_|github_pat_|xox[baprs]-|AKIA|eyJ[a-zA-Z0-9_-]+\.)[a-z0-9_./+=-]{6,}|(?:^|\s)-(?:u|H)(?:\s+|=)[^\s]+|(?:^|\s)--(?:key|auth|header|env|client[-_]?secret)(?:\s+|=)[^\s]+|-----BEGIN [A-Z ]*PRIVATE KEY-----|\$\{[^}\n]+:-[^}\n]+|\b(?:api[-_]?key|client[-_]?secret|password|passwd|secret|token)\s*[:=]\s*[^\s]+)",
    ).expect("settings credential indicators")
    });
    if exact_reference(tool, value) {
        return false;
    }
    RISK.is_match(value)
        || !leaf_safe(value)
        || command_risky(value)
        || near_credential(value)
        || validate_text(value).is_err()
}

/// `NAME=value`, split at the first `=`, when NAME is a variable name.
fn assignment(word: &str) -> Option<(&str, &str)> {
    let (name, value) = word.split_once('=')?;
    let mut bytes = name.bytes();
    (bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'))
    .then_some((name, value))
}

/// `$NAME` or `${NAME}`.
fn variable_reference(value: &str) -> bool {
    let name = value
        .strip_prefix("${")
        .and_then(|value| value.strip_suffix('}'))
        .or_else(|| value.strip_prefix('$'));
    name.is_some_and(|name| assignment(&format!("{name}=")).is_some())
}

/// Command-like text that passes a value in a flag or an assignment. In each
/// part of the text (split at brackets, quotes and shell operators):
/// - `NAME=value` is refused unless the value is `$NAME`/`${NAME}`, or the
///   name has no credential word and the value looks like no key;
/// - each flag goes through `flag_risky`.
fn command_risky(text: &str) -> bool {
    text.split([
        '(', ')', ';', '&', '|', '\n', '\r', ',', '[', ']', '"', '\'', '`',
    ])
    .any(|part| {
        let words = part
            .split_ascii_whitespace()
            .map(|word| {
                word.strip_suffix(":*")
                    .or_else(|| word.strip_suffix('*'))
                    .unwrap_or(word)
            })
            .filter(|word| !word.is_empty())
            .collect::<Vec<_>>();
        words
            .iter()
            .enumerate()
            .any(|(index, word)| match assignment(word) {
                Some((name, value)) => {
                    !(variable_reference(value) || (!credential_key(name) && leaf_safe(value)))
                }
                None => {
                    let programs = words[..index]
                        .iter()
                        .filter(|word| !word.starts_with('-') && assignment(word).is_none())
                        .map(|word| word.rsplit(['/', '\\']).next().unwrap_or(word))
                        .collect::<Vec<_>>();
                    flag_risky(&programs, word, words.get(index + 1).copied())
                }
            })
    })
}

/// Whether a flag in a command may carry a key. `programs` are the words
/// before it that are not flags.
/// - One dash and up to four letters (`-la`, `-rf`, `-xzf`) are switches; a
///   longer one, or one with a digit or other character (`-phunter2`), is
///   refused.
/// - A credential option refuses any value but a `$VAR` reference: a long
///   flag with a credential word (`--pass`, `--pin`, `--otp`, `--token`,
///   `--auth`, `--key`, `--secret`, `--password`, but not `--session`, nor
///   `--author`), `--user` or `-u` with a `user:password` value, and the
///   password flag of a named program: `mysql`/`mariadb -p`, `sshpass -p`,
///   `redis-cli -a`, `openssl -k`/`-pass` and `zip`/`7z -P`/`-p`. Such a
///   flag without a value (`--passWithNoTests`) is fine.
/// - Any other flag's value passes `flag_value_safe`.
fn flag_risky(programs: &[&str], word: &str, next: Option<&str>) -> bool {
    let Some(body) = word.strip_prefix('-') else {
        return false;
    };
    let letter = |text: &str| text.starts_with(|ch: char| ch.is_ascii_alphabetic());
    let next = next.filter(|next| !next.starts_with('-'));
    let ran = |names: &[&str]| programs.iter().any(|program| names.contains(program));
    let secret_value = |value: Option<&str>| value.is_some_and(|value| !variable_reference(value));
    match body.strip_prefix('-') {
        Some(long) if letter(long) => {
            let (name, value) = match long.split_once('=') {
                Some((name, value)) => (name, Some(value)),
                None => (long, next),
            };
            if name != "session" && credential_key(name) {
                secret_value(value)
            } else if name == "user" && value.is_some_and(|value| value.contains(':')) {
                true
            } else {
                value.is_some_and(|value| !flag_value_safe(value))
            }
        }
        None if letter(body) => {
            if body.len() > 4 || !body.bytes().all(|byte| byte.is_ascii_alphabetic()) {
                return true;
            }
            let first = &body[..1];
            let password = (ran(&["mysql", "mariadb", "sshpass"]) && first == "p")
                || (ran(&["redis-cli"]) && first == "a")
                || (ran(&["openssl"]) && (first == "k" || body == "pass"))
                || (ran(&["zip", "7z", "7za"]) && matches!(first, "p" | "P"));
            if password {
                (body.len() > 1 && body != "pass") || secret_value(next)
            } else if body == "u" && next.is_some_and(|value| value.contains(':')) {
                true
            } else {
                next.is_some_and(|value| !flag_value_safe(value))
            }
        }
        _ => false,
    }
}

/// A value after an ordinary flag: a `$VAR` reference, a path (with `/`, or
/// starting with `~` or `.`), or text whose words are each letters only, a
/// version such as `v2`, or a number of up to six digits (`HEAD`,
/// `f.tgz`, `no:cacheprovider`, `4096`). Letters and digits mixed in one
/// word, as in `hunter2`, or a longer number may be a password or PIN.
/// Every value also passes `leaf_safe`.
fn flag_value_safe(value: &str) -> bool {
    let path = value.contains('/') || value.starts_with(['~', '.']);
    let plain = value
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .all(|run| {
            run.bytes().all(|byte| byte.is_ascii_alphabetic())
                || (run.len() <= 6 && run.bytes().all(|byte| byte.is_ascii_digit()))
                || run.strip_prefix('v').is_some_and(|number| {
                    !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
                })
        });
    variable_reference(value) || ((path || plain) && leaf_safe(value))
}

/// A credential word followed within three words by a value, as in
/// `vpn password hunter2`, `pw hunter2` or `add-authtoken 2abc`. A lowercase
/// word (`gh auth status`, `npm run auth:test`), a path or a flag after the
/// word is not a value; a password made only of lowercase letters cannot be
/// told from a word (UX-SKILL-17). A reference to the environment right
/// after the word, as in `TOKEN=${TOKEN}`, is the word's value, so the text
/// is not refused for it.
fn near_credential(text: &str) -> bool {
    static REFERENCE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"\$\{(?:env:)?[A-Za-z_][A-Za-z0-9_]*\}|\{env:[A-Za-z_][A-Za-z0-9_]*\}|\$[A-Za-z_][A-Za-z0-9_]*",
        )
        .expect("environment references")
    });
    const MARK: &str = "\u{1}";
    let marked = REFERENCE.replace_all(text, " \u{1} ");
    let words = marked
        .split(|ch: char| ch.is_whitespace() || "(),;\"'[]=:<>|{}`".contains(ch))
        .filter(|word| *word == MARK || word.bytes().any(|byte| byte.is_ascii_alphanumeric()))
        .collect::<Vec<_>>();
    let value = |word: &str| {
        let plain = word.starts_with(|ch: char| ch.is_ascii_lowercase())
            && word
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || b"-_.".contains(&byte));
        let path = word.contains('/') || word.starts_with(['~', '.']);
        !(word.starts_with('-') || plain || path)
    };
    words.iter().enumerate().any(|(index, word)| {
        credential_word(word)
            && words[index + 1..]
                .iter()
                .take(3)
                .take_while(|word| **word != MARK)
                .any(|word| value(word))
    })
}

// Free-form argv and shell text can contain even very short credentials.
// Admit only a simple executable, unassigned flags and exact references.
fn executable(tool: &str, value: &str) -> bool {
    let windows_drive = value.as_bytes().get(1) == Some(&b':')
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphabetic)
        && value[2..].starts_with(['/', '\\']);
    let path = if windows_drive { &value[2..] } else { value };
    !path.is_empty()
        && !value.starts_with('-')
        && path
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"/\\._-~".contains(&byte))
        && !text_risky(tool, value)
}

fn safe_argument(tool: &str, arg: &str) -> bool {
    exact_reference(tool, arg)
        || matches!(
            arg,
            "-q" | "-v"
                | "-y"
                | "--check"
                | "--help"
                | "--no-color"
                | "--quiet"
                | "--stdio"
                | "--verbose"
                | "--version"
                | "--yes"
        )
}

/// The program and arguments of a command string or list. A string is split
/// on whitespace; one with a line break is refused.
fn argv(command: &Value) -> Option<Vec<&str>> {
    match command {
        Value::String(command) if !command.contains(['\r', '\n']) => {
            Some(command.split_ascii_whitespace().collect())
        }
        Value::Array(parts) => parts.iter().map(Value::as_str).collect(),
        _ => None,
    }
}

fn command_safe(tool: &str, command: &Value) -> bool {
    // Refuse shell operators, assignments, quoting we cannot interpret
    // losslessly, and inline scripts.
    argv(command).is_some_and(|parts| {
        parts.split_first().is_some_and(|(name, args)| {
            executable(tool, name) && args.iter().all(|arg| safe_argument(tool, arg))
        })
    })
}

/// A hook flag or header label that can carry a user name, password, header,
/// cookie or other key, in the argument after it or attached to it. A hook
/// refuses it with or without a value; permission rules use `flag_risky`.
fn credential_flag(flag: &str) -> bool {
    let name = flag.trim_start_matches('-').to_ascii_lowercase();
    matches!(flag, "-u" | "-U" | "-H" | "-a" | "-k")
        || credential_key(&name)
        || ["bearer", "user", "header", "login", "env"]
            .iter()
            .any(|word| name.contains(word))
}

/// A hook or status line command: a simple executable, then exact
/// references, plain flags and positional words. A word after a flag counts
/// as that flag's value, which is public only for a few named flags such as
/// `--agent`; after any other flag it may be a key. Other positional words
/// are only a path, a lowercase word without digits, or a number of up to
/// four digits after a word that names no credential. A password made only
/// of lowercase letters and dashes cannot be told from a word (UX-SKILL-17).
fn shell_command_safe(tool: &str, command: &Value) -> bool {
    const VALUE_FLAGS: &[&str] = &[
        "--agent",
        "--agent-name",
        "--tool",
        "--tool-profile",
        "--format",
        "--space",
        "--session",
    ];
    let plain_flag = |arg: &str| {
        let name = arg.strip_prefix("--").or_else(|| {
            arg.strip_prefix('-')
                .filter(|name| name.len() == 1 && !name.starts_with('-'))
        });
        name.is_some_and(|name| {
            !name.is_empty()
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        }) && (VALUE_FLAGS.contains(&arg) || !credential_flag(arg))
    };
    let flag_value = |arg: &str| arg.len() <= 64 && safe_identifier(arg) && !secret_looking(arg);
    let path = |arg: &str| {
        (arg.contains('/') || arg.starts_with('~'))
            && arg.len() <= 200
            && arg
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"/\\._-~".contains(&byte))
    };
    let word = |arg: &str| {
        arg.len() <= 64
            && arg.starts_with(|ch: char| ch.is_ascii_lowercase())
            && arg
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || b"-_".contains(&byte))
    };
    let number = |arg: &str, before: &str| {
        !arg.is_empty()
            && arg.len() <= 4
            && arg.bytes().all(|byte| byte.is_ascii_digit())
            && !credential_word(before)
            && !matches!(
                before.to_ascii_lowercase().as_str(),
                "lock" | "unlock" | "code"
            )
    };
    let text = match command {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(" "),
        _ => return false,
    };
    leaf_safe(&text)
        && !near_credential(&text)
        && argv(command).is_some_and(|parts| {
            parts.split_first().is_some_and(|(name, args)| {
                executable(tool, name)
                    && args.iter().enumerate().all(|(index, arg)| {
                        let before = index.checked_sub(1).map_or(*name, |before| args[before]);
                        if safe_argument(tool, arg) {
                            true
                        } else if arg.starts_with('-') {
                            plain_flag(arg)
                        } else if before.starts_with('-') {
                            VALUE_FLAGS.contains(&before) && flag_value(arg)
                        } else {
                            path(arg) || word(arg) || number(arg, before)
                        }
                    })
            })
        })
}

/// A package name for `npx`, `bunx`, `uvx` or `pnpm dlx` in npm or PyPI
/// form: lowercase letters, digits, `-`, `.` and `_`, with an optional
/// `@scope/` and an optional `@version` that is a version number or a
/// lowercase release tag (`@21st-dev/magic@latest`, `mcp-server-sqlite3`).
/// No part may look like a key. A password shaped like a package name, such
/// as `hunter2`, cannot be told from one (UX-SKILL-17); one in the version,
/// as in `some-mcp@hunter2pass`, is refused.
fn package_spec(spec: &str) -> bool {
    static VERSION: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"^(?:[a-z][a-z-]*|[~^]?\d+(?:\.\d+){0,2}(?:-(?:alpha|beta|rc|next|canary|preview)(?:\.\d+)*)?)$",
        )
        .expect("package versions")
    });
    let name_part = |part: &str| {
        !part.is_empty()
            && part.starts_with(|ch: char| ch.is_ascii_lowercase() || ch.is_ascii_digit())
            && part.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
            })
    };
    let (scope, rest) = match spec.strip_prefix('@') {
        Some(rest) => match rest.split_once('/') {
            Some((scope, rest)) => (Some(scope), rest),
            None => return false,
        },
        None => (None, spec),
    };
    let (name, version) = match rest.split_once('@') {
        Some((name, version)) => (name, Some(version)),
        None => (rest, None),
    };
    spec.len() <= 214
        && scope.is_none_or(name_part)
        && name_part(name)
        && version.is_none_or(|version| VERSION.is_match(version))
        && !secret_looking(spec)
}

/// An MCP server run through a package launcher, with exactly one package
/// spec and otherwise only the arguments `command_safe` admits.
fn launcher_safe(tool: &str, fields: &serde_json::Map<String, Value>) -> bool {
    let Some(mut parts) = fields.get("command").and_then(argv) else {
        return false;
    };
    if let Some(args) = fields.get("args") {
        match args
            .as_array()
            .and_then(|args| args.iter().map(Value::as_str).collect::<Option<Vec<_>>>())
        {
            Some(args) => parts.extend(args),
            None => return false,
        }
    }
    let Some((program, rest)) = parts.split_first() else {
        return false;
    };
    let name = program.rsplit(['/', '\\']).next().unwrap_or(program);
    let name = name
        .strip_suffix(".cmd")
        .or_else(|| name.strip_suffix(".exe"))
        .unwrap_or(name);
    let rest = match (name, rest) {
        ("npx" | "bunx" | "uvx", rest) => rest,
        ("pnpm", ["dlx", rest @ ..]) => rest,
        _ => return false,
    };
    let mut packages = 0;
    executable(tool, program)
        && rest.iter().all(|arg| {
            safe_argument(tool, arg) || {
                packages += 1;
                package_spec(arg)
            }
        })
        && packages == 1
}

/// A literal header value that names a client or a format, never a key: a
/// `x-<label>-source` label, or a MIME type or product token in `Accept`,
/// `Content-Type` or `User-Agent`.
fn public_header(name: &str, value: &Value) -> bool {
    let name = name.to_ascii_lowercase();
    let source_label = name
        .strip_prefix("x-")
        .and_then(|name| name.strip_suffix("-source"))
        .is_some_and(|label| {
            !label.is_empty()
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                && !credential_flag(label)
        });
    let format = matches!(name.as_str(), "accept" | "content-type" | "user-agent");
    value.as_str().is_some_and(|value| {
        if source_label {
            safe_identifier(value) && !secret_looking(value)
        } else {
            format
                && !value.is_empty()
                && value.len() <= 200
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"/.+;=-_ ,*".contains(&byte))
                // A space only after a separator, as in `text/html, */*`.
                && value
                    .match_indices(' ')
                    .all(|(index, _)| index > 0 && b";, ".contains(&value.as_bytes()[index - 1]))
                && leaf_safe(value)
                && !near_credential(value)
        }
    })
}

/// Whether a name the person chose (a server, a setting key, a map key) is
/// safe to copy. A name such as `github-auth` or `apiKeyHelper` names a
/// thing and is not a value, so it is only checked for key-looking parts;
/// the value under a credential-like name is checked in `safe_field`. A
/// "name" with spaces or other characters, such as an OpenCode permission
/// pattern, is a command and goes through `text_risky`.
fn name_safe(tool: &str, name: &str) -> bool {
    catalog_name(name)
        || if !name.is_empty()
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        {
            leaf_safe(name)
        } else {
            !text_risky(tool, name)
        }
}

/// A value under a credential-like name that holds no key: an on/off value,
/// a reference to the environment, a path such as `apiKeyHelper`'s script,
/// or a number under a measure such as `model_auto_compact_token_limit`.
/// Every value inside a list or table under that name must be one too, so a
/// server named `github-auth` passes while `auth = { type = "x" }` does not.
fn credential_value_safe(tool: &str, name: &str, value: &Value) -> bool {
    match value {
        Value::Bool(_) | Value::Null => true,
        Value::Array(items) => items
            .iter()
            .all(|item| credential_value_safe(tool, name, item)),
        Value::Object(map) => map
            .iter()
            .all(|(child, value)| credential_value_safe(tool, child, value)),
        Value::Number(_) => token_measure(name),
        Value::String(text) => {
            exact_reference(tool, text)
                || variable_reference(text)
                || ((text.contains('/') || text.starts_with(['~', '.']))
                    && text.len() <= 200
                    && text
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"/\\._-~".contains(&byte))
                    && leaf_safe(text))
        }
    }
}

fn material_safe(tool: &str, value: &Value) -> bool {
    match value {
        Value::Object(map) => map
            .iter()
            .all(|(name, value)| name_safe(tool, name) && safe_field(tool, name, value)),
        Value::Array(items) => items.iter().all(|item| material_safe(tool, item)),
        Value::String(value) => !text_risky(tool, value),
        _ => true,
    }
}

/// An `oauth` block with only the fields a provider publishes: a client id,
/// the local callback port and the scopes asked for. The tool keeps tokens in
/// its own store; a client secret, or any other field, is refused.
fn public_oauth(value: &Value) -> bool {
    let scope = |scope: &str| {
        !scope.is_empty()
            && !scope.starts_with('-')
            && scope.len() <= 200
            && scope
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b":._/-".contains(&byte))
            && !text_risky("", scope)
    };
    value.as_object().is_some_and(|fields| {
        fields.iter().all(|(name, value)| {
            match name.to_ascii_lowercase().replace(['-', '_'], "").as_str() {
                "clientid" => value.as_str().is_some_and(safe_identifier),
                "callbackport" => value
                    .as_u64()
                    .is_some_and(|port| (1..=65535).contains(&port)),
                "scopes" => match value {
                    Value::Array(scopes) => {
                        scopes.iter().all(|value| value.as_str().is_some_and(scope))
                    }
                    Value::String(scopes) => {
                        !text_risky("", scopes) && scopes.split(' ').all(scope)
                    }
                    _ => false,
                },
                _ => false,
            }
        })
    })
}

fn safe_field(tool: &str, name: &str, value: &Value) -> bool {
    let normalized = name.to_ascii_lowercase().replace(['-', '_'], "");
    match normalized.as_str() {
        "command" | "commandwindows" | "commandlinux" | "commandmacos" | "bash" | "powershell"
        | "script"
            if value.is_string() || value.is_array() =>
        {
            command_safe(tool, value)
        }
        "args" | "arguments" | "commandargs" => value.as_array().is_some_and(|args| {
            args.iter()
                .all(|arg| arg.as_str().is_some_and(|arg| safe_argument(tool, arg)))
        }),
        "bearertokenenvvar" | "envkey" => {
            tool == "codexcli" && value.as_str().is_some_and(variable_name)
        }
        "oauth" => public_oauth(value),
        "statusline" => value.as_object().is_some_and(|fields| {
            fields.iter().all(|(name, value)| match name.as_str() {
                "command" => value.is_string() && shell_command_safe(tool, value),
                _ => name_safe(tool, name) && safe_field(tool, name, value),
            })
        }),
        "envvars" => {
            tool == "codexcli"
                && value.as_array().is_some_and(|names| {
                    names
                        .iter()
                        .all(|name| name.as_str().is_some_and(variable_name))
                })
        }
        "url" | "serverurl" | "httpurl" => value.as_str().is_some_and(|url| {
            // Path segments, host labels and matrix parameters go through
            // the same leaf check as any other text; `;` and `=` in the path
            // can only carry a parameter, so they are refused.
            exact_reference(tool, url)
                || (!text_risky(tool, url)
                    && !url.contains([';', '='])
                    && reqwest::Url::parse(url).is_ok_and(|url| {
                        matches!(url.scheme(), "http" | "https")
                            && url.username().is_empty()
                            && url.password().is_none()
                            && url.query().is_none()
                            && url.fragment().is_none()
                    }))
        }),
        "env" | "environment" | "headers" | "httpheaders" | "envhttpheaders" => {
            value.as_object().is_some_and(|map| {
                // Every value must be a reference to the person's environment,
                // so a credential-named key carries only a variable name.
                map.iter().all(|(key, value)| {
                    !text_risky(tool, key)
                        && (value.as_str().is_some_and(|value| {
                            if normalized == "envhttpheaders" && tool == "codexcli" {
                                variable_name(value)
                            } else {
                                exact_reference(tool, value)
                            }
                        }) || matches!(normalized.as_str(), "headers" | "httpheaders")
                            && public_header(key, value))
                })
            })
        }
        _ => {
            (!credential_key(name) || credential_value_safe(tool, name, value))
                && material_safe(tool, value)
                && (!argv_field(&normalized) || !text_risky(tool, &joined(value)))
        }
    }
}

/// A field that holds a command line as a list, such as a Codex rule's
/// `pattern` or a Gemini policy's `commandPrefix`. Besides each part alone,
/// the parts joined with spaces are checked, so `["sshpass", "-p", "x"]`
/// counts as `sshpass -p x`.
fn argv_field(normalized: &str) -> bool {
    matches!(
        normalized,
        "pattern" | "commandprefix" | "prefix" | "argv" | "cmd" | "notify"
    )
}

/// The strings in a value, nested lists included, joined with spaces.
fn joined(value: &Value) -> String {
    fn collect<'a>(value: &'a Value, parts: &mut Vec<&'a str>) {
        match value {
            Value::String(text) => parts.push(text),
            Value::Array(items) => items.iter().for_each(|item| collect(item, parts)),
            _ => {}
        }
    }
    let mut parts = Vec::new();
    collect(value, &mut parts);
    parts.join(" ")
}

fn mcp_payload_safe(tool: &str, value: &Value) -> bool {
    value.as_object().is_some_and(|fields| {
        let launcher = launcher_safe(tool, fields);
        !fields.is_empty()
            && fields.iter().all(|(name, value)| {
                let normalized = name.to_ascii_lowercase().replace(['-', '_'], "");
                match normalized.as_str() {
                    "command" => launcher || command_safe(tool, value),
                    "args" if launcher => true,
                    "args" | "env" | "environment" | "envvars" | "headers" | "httpheaders"
                    | "envhttpheaders" | "bearertokenenvvar" | "url" | "serverurl" | "httpurl"
                    | "oauth" => safe_field(tool, name, value),
                    "type" | "transport" => matches!(
                        value.as_str(),
                        Some(
                            "stdio"
                                | "local"
                                | "remote"
                                | "http"
                                | "sse"
                                | "streamable-http"
                                | "streamable_http"
                        )
                    ),
                    "enabled" | "disabled" | "required" | "trust" => value.is_boolean(),
                    "timeout" => {
                        value.is_number()
                            || value.as_object().is_some_and(|limits| {
                                limits.iter().all(|(name, value)| {
                                    matches!(name.as_str(), "catalog" | "execution")
                                        && value.is_number()
                                })
                            })
                    }
                    "timeoutms" | "startuptimeoutsec" | "tooltimeoutsec" => value.is_number(),
                    "cwd" => value.as_str().is_some_and(|path| executable(tool, path)),
                    "tools" | "includetools" | "excludetools" | "enabledtools"
                    | "disabledtools" => value.as_array().is_some_and(|tools| {
                        tools.iter().all(|value| {
                            value
                                .as_str()
                                .is_some_and(|name| name == "*" || safe_identifier(name))
                        })
                    }),
                    _ => false,
                }
            })
    })
}

fn hook_groups_safe(tool: &str, value: &Value) -> bool {
    value.as_object().is_some_and(|events| {
        events.iter().all(|(event, groups)| {
            safe_identifier(event)
                && groups
                    .as_array()
                    .is_some_and(|groups| groups.iter().all(|group| hook_payload_safe(tool, group)))
        })
    })
}

fn hook_payload_safe(tool: &str, value: &Value) -> bool {
    value.as_object().is_some_and(|fields| {
        !fields.is_empty()
            && fields.iter().all(|(name, value)| {
                let normalized = name.to_ascii_lowercase().replace(['-', '_'], "");
                match normalized.as_str() {
                    "hooks" => value.as_array().is_some_and(|hooks| {
                        hooks.iter().all(|hook| hook_payload_safe(tool, hook))
                    }),
                    "command" | "commandwindows" | "commandlinux" | "commandmacos" | "bash"
                    | "powershell" => value.is_string() && shell_command_safe(tool, value),
                    "type" => matches!(value.as_str(), Some("command" | "http")),
                    "url" | "headers" | "httpheaders" => safe_field(tool, name, value),
                    "allowedenvvars" => value.as_array().is_some_and(|names| {
                        names
                            .iter()
                            .all(|name| name.as_str().is_some_and(variable_name))
                    }),
                    "matcher" => value
                        .as_str()
                        .is_some_and(|matcher| !text_risky(tool, matcher)),
                    "name" | "id" => value.as_str().is_some_and(safe_identifier),
                    "async" | "once" | "failclosed" => value.is_boolean(),
                    "timeout" | "timeoutsec" | "additionalcontextlimit" | "looplimit" => {
                        value.is_number()
                    }
                    "statusmessage" | "description" => {
                        value.as_str().is_some_and(|text| !text_risky(tool, text))
                    }
                    "cwd" => value.as_str().is_some_and(|path| executable(tool, path)),
                    _ => false,
                }
            })
    })
}

pub fn reviewable(tool: &str, entry: &Entry) -> bool {
    entry.container.iter().all(|name| name_safe(tool, name))
        && entry
            .key
            .member
            .as_deref()
            .is_none_or(|name| !text_risky(tool, name))
        && match entry.key.feature {
            // Wenlan sets its own entry up in each tool, so it is never
            // managed or copied, even when it would pass the check.
            Mcp => !wenlan_own_mcp(tool, &entry.value) && mcp_payload_safe(tool, &entry.value),
            Hook if tool == "antigravity-ide" => hook_groups_safe(tool, &entry.value),
            Hook => hook_payload_safe(tool, &entry.value),
            Permission => entry.container.last().map_or_else(
                || material_safe(tool, &entry.value),
                |name| safe_field(tool, name, &entry.value),
            ),
        }
}

/// The entry `wenlan mcp connect` writes into each tool. It names that tool
/// in `--agent-name`, so it is set up per tool and never copied between them.
pub fn wenlan_own_mcp(tool: &str, value: &Value) -> bool {
    let Some(fields) = value.as_object() else {
        return false;
    };
    fn strings(value: &Value) -> Option<Vec<&str>> {
        value
            .as_array()?
            .iter()
            .map(Value::as_str)
            .collect::<Option<Vec<_>>>()
    }
    let mut parts = match fields.get("command") {
        Some(Value::String(command)) => vec![command.as_str()],
        Some(command @ Value::Array(_)) => match strings(command) {
            Some(parts) => parts,
            None => return false,
        },
        _ => return false,
    };
    if let Some(args) = fields.get("args") {
        match strings(args) {
            Some(args) => parts.extend(args),
            None => return false,
        }
    }
    let Some((program, rest)) = parts.split_first() else {
        return false;
    };
    let name = program.rsplit(['/', '\\']).next().unwrap_or(program);
    let mut rest = match (name, rest) {
        ("npx", ["-y", "wenlan-mcp", rest @ ..]) if executable(tool, program) => rest,
        ("wenlan-mcp" | "wenlan-mcp.exe", rest) if executable(tool, program) => rest,
        _ => return false,
    };
    while let [flag, value, tail @ ..] = rest {
        if !matches!(*flag, "--agent-name" | "--tool-profile") || !safe_identifier(value) {
            return false;
        }
        rest = tail;
    }
    rest.is_empty()
        && fields.iter().all(|(name, value)| {
            matches!(name.as_str(), "command" | "args")
                || mcp_payload_safe(tool, &json!({ name.clone(): value.clone() }))
        })
}

/// Whether an MCP entry writes a value into an environment, header or
/// credential-named field instead of referring to an environment variable.
fn literal_key(tool: &str, value: &Value) -> bool {
    value.as_object().is_some_and(|fields| {
        fields.iter().any(|(name, value)| {
            let normalized = name.to_ascii_lowercase().replace(['-', '_'], "");
            match normalized.as_str() {
                "env" | "environment" | "headers" | "httpheaders" | "envhttpheaders" => {
                    !safe_field(tool, name, value)
                }
                "oauth" => !public_oauth(value),
                _ => credential_key(name),
            }
        })
    })
}

fn reference_free(text: &str) -> bool {
    !text.contains(['$', '%', '{', '}']) && !text.contains('\\')
}

/// A reference to an environment variable: an optional auth scheme and the
/// variable name.
type Reference = (Option<String>, String);

#[derive(PartialEq, Eq)]
enum PortableMcp {
    /// The command line and environment names mapped to variables.
    Stdio(Vec<String>, BTreeMap<String, String>),
    /// The URL and header names mapped to variable references.
    Http(String, BTreeMap<String, Reference>),
}

/// The variable behind an exact reference in `tool`'s own syntax.
fn reference_parts(tool: &str, value: &str) -> Option<Reference> {
    if !exact_reference(tool, value) {
        return None;
    }
    let (scheme, rest) = match value.split_once(' ') {
        Some((scheme @ ("Bearer" | "Basic"), rest)) => (Some(scheme.to_owned()), rest),
        _ => (None, value),
    };
    let wrapped = |open: &str, close: &str| {
        rest.strip_prefix(open)
            .and_then(|text| text.strip_suffix(close))
    };
    let name = match tool {
        "claudecode" => wrapped("${", "}"),
        "geminicli" => wrapped("${", "}").or_else(|| rest.strip_prefix('$')),
        "cursor" => wrapped("${env:", "}"),
        "opencode" => wrapped("{env:", "}"),
        _ => None,
    }?;
    variable_name(name).then(|| (scheme, name.to_owned()))
}

fn native_reference(tool: &str, (scheme, name): &Reference) -> Option<String> {
    let reference = match tool {
        "claudecode" | "geminicli" => format!("${{{name}}}"),
        "cursor" => format!("${{env:{name}}}"),
        "opencode" => format!("{{env:{name}}}"),
        _ => return None,
    };
    Some(match scheme {
        Some(scheme) => format!("{scheme} {reference}"),
        None => reference,
    })
}

fn portable_mcp(tool: &str, value: &Value) -> Option<PortableMcp> {
    let mut fields = value.as_object()?.clone();
    let tag_key = if tool == "musecode" {
        "transport"
    } else {
        "type"
    };
    let tag = fields.remove(tag_key);
    let tag = match tag.as_ref() {
        Some(Value::String(tag)) => Some(tag.as_str()),
        Some(_) => return None,
        None => None,
    };
    let result = if let Some(command) = fields.remove("command") {
        let command = if tool == "opencode" {
            if tag != Some("local") {
                return None;
            }
            command
                .as_array()?
                .iter()
                .map(|part| part.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()?
        } else {
            let supported = match tool {
                "copilotcli" => matches!(tag, None | Some("stdio" | "local")),
                "musecode" => tag == Some("stdio"),
                "claudecode" | "cursor" => matches!(tag, None | Some("stdio")),
                _ => tag.is_none(),
            };
            if !supported {
                return None;
            }
            let mut parts = vec![command.as_str()?.to_owned()];
            if let Some(args) = fields.remove("args") {
                for argument in args.as_array()? {
                    parts.push(argument.as_str()?.to_owned());
                }
            }
            parts
        };
        if command.is_empty()
            || command[0].is_empty()
            || command[0].starts_with('.')
            || command.iter().any(|arg| !reference_free(arg))
        {
            return None;
        }
        let mut env = BTreeMap::new();
        if tool == "codexcli" {
            // Codex passes these variables through under their own names.
            if let Some(names) = fields.remove("env_vars") {
                for name in names.as_array()? {
                    let name = name.as_str().filter(|name| variable_name(name))?;
                    env.insert(name.to_owned(), name.to_owned());
                }
            }
        } else if let Some(map) = fields.remove(if tool == "opencode" {
            "environment"
        } else {
            "env"
        }) {
            for (name, value) in map.as_object()? {
                let (scheme, variable) = reference_parts(tool, value.as_str()?)?;
                if scheme.is_some() || !variable_name(name) {
                    return None;
                }
                env.insert(name.clone(), variable);
            }
        }
        PortableMcp::Stdio(command, env)
    } else {
        let url_field = match tool {
            "antigravity-ide" => "serverUrl",
            "geminicli" => "httpUrl",
            _ => "url",
        };
        let http_tag = match tool {
            "antigravity-ide" | "geminicli" | "codexcli" => None,
            "opencode" => Some("remote"),
            "musecode" => Some("streamable_http"),
            _ => Some("http"),
        };
        let url = fields.remove(url_field)?.as_str()?.to_owned();
        if tag != http_tag
            || !reference_free(&url)
            || !url.starts_with("https://") && !url.starts_with("http://")
        {
            return None;
        }
        let mut headers = BTreeMap::new();
        if tool == "codexcli" {
            if let Some(name) = fields.remove("bearer_token_env_var") {
                let name = name.as_str().filter(|name| variable_name(name))?;
                headers.insert(
                    "Authorization".to_owned(),
                    (Some("Bearer".to_owned()), name.to_owned()),
                );
            }
            if let Some(map) = fields.remove("env_http_headers") {
                for (header, name) in map.as_object()? {
                    let name = name.as_str().filter(|name| variable_name(name))?;
                    if headers
                        .insert(header.clone(), (None, name.to_owned()))
                        .is_some()
                    {
                        return None;
                    }
                }
            }
        } else if let Some(map) = fields.remove("headers") {
            for (header, value) in map.as_object()? {
                headers.insert(header.clone(), reference_parts(tool, value.as_str()?)?);
            }
        }
        PortableMcp::Http(url, headers)
    };
    fields.is_empty().then_some(result)
}

/// The same server in `tool`'s native shape, or None when that tool cannot
/// express one of its environment references.
fn native_mcp(tool: &str, portable: &PortableMcp) -> Option<Value> {
    let mut value = match portable {
        PortableMcp::Stdio(command, _) => match tool {
            "opencode" => json!({"type":"local", "command":command}),
            "musecode" => json!({"transport":"stdio", "command":command[0], "args":command[1..]}),
            "copilotcli" => json!({"type":"stdio", "command":command[0], "args":command[1..]}),
            _ => json!({"command":command[0], "args":command[1..]}),
        },
        PortableMcp::Http(url, _) => match tool {
            "opencode" => json!({"type":"remote", "url":url}),
            "musecode" => json!({"transport":"streamable_http", "url":url}),
            "antigravity-ide" => json!({"serverUrl":url}),
            "geminicli" => json!({"httpUrl":url}),
            "codexcli" => json!({"url":url}),
            _ => json!({"type":"http", "url":url}),
        },
    };
    match portable {
        PortableMcp::Stdio(_, env) if !env.is_empty() => {
            if tool == "codexcli" {
                if env.iter().any(|(name, variable)| name != variable) {
                    return None;
                }
                value["env_vars"] = json!(env.keys().collect::<Vec<_>>());
            } else {
                let mut map = serde_json::Map::new();
                for (name, variable) in env {
                    map.insert(
                        name.clone(),
                        Value::String(native_reference(tool, &(None, variable.clone()))?),
                    );
                }
                let field = if tool == "opencode" {
                    "environment"
                } else {
                    "env"
                };
                value[field] = Value::Object(map);
            }
        }
        PortableMcp::Http(_, headers) if !headers.is_empty() => {
            let mut map = serde_json::Map::new();
            for (header, reference) in headers {
                if tool != "codexcli" {
                    map.insert(
                        header.clone(),
                        Value::String(native_reference(tool, reference)?),
                    );
                    continue;
                }
                match reference {
                    (Some(scheme), name) if scheme == "Bearer" && header == "Authorization" => {
                        value["bearer_token_env_var"] = json!(name);
                    }
                    (None, name) => {
                        map.insert(header.clone(), json!(name));
                    }
                    _ => return None,
                }
            }
            if !map.is_empty() {
                let field = if tool == "codexcli" {
                    "env_http_headers"
                } else {
                    "headers"
                };
                value[field] = Value::Object(map);
            }
        }
        _ => {}
    }
    Some(value)
}

/// Translate only the common server fields, or a hook with the same event and
/// native schema. Tool-specific keys, substitutions and policies remain local.
pub fn project(from: &Location, to: &Location, entry: &Entry, current: &Document) -> Option<Entry> {
    if from.tool == to.tool || !reviewable(from.tool, entry) {
        return None;
    }
    let next = match entry.key.feature {
        Mcp => {
            let portable = portable_mcp(from.tool, &entry.value)?;
            let &Section(_, path, _) = current.sections.iter().find(|section| section.0 == Mcp)?;
            let mut path = path.to_vec();
            path.push(entry.key.name.last()?);
            let value = native_mcp(to.tool, &portable)?;
            if portable_mcp(to.tool, &value)? != portable {
                return None;
            }
            to.entry(Mcp, &path, value)
        }
        Hook if matches!(
            (from.tool, to.tool),
            ("claudecode", "codexcli") | ("codexcli", "claudecode")
        ) =>
        {
            if !current.sections.iter().any(|section| section.0 == Hook)
                || to.format == Format::Toml && current.raw.get("hooks").is_none()
            {
                return None;
            }
            let event = entry.container.last()?;
            // The two tools can interpret a blocking hook's output
            // differently, including Claude's PreToolUse ask decision.
            if event != "SessionStart" {
                return None;
            }
            let group = entry.value.as_object()?;
            let hooks = group.get("hooks")?.as_array()?;
            if group.len() != 1
                || hooks.is_empty()
                || hooks.iter().any(|hook| {
                    hook.as_object().is_none_or(|hook| hook.len() != 2)
                        || hook.get("type").and_then(Value::as_str) != Some("command")
                        || !hook
                            .get("command")
                            .and_then(Value::as_str)
                            .is_some_and(|command| {
                                reference_free(command) && !command.starts_with('.')
                            })
                })
            {
                return None;
            }
            entry.clone()
        }
        _ => return None,
    };
    reviewable(to.tool, &next).then_some(next)
}

pub fn equivalent(location: &Location, left: &Entry, right: &Entry) -> bool {
    left == right
        || left.key == right.key
            && left.key.feature == Mcp
            && portable_mcp(location.tool, &left.value)
                .is_some_and(|value| portable_mcp(location.tool, &right.value) == Some(value))
}

/// Only enrolled, native home paths can receive shared settings.
pub fn enrolled(home: &Path, location: &Location, tools: &BTreeSet<&str>) -> bool {
    tools.contains(location.tool)
        && personal_target(location.tool).is_some_and(|target| {
            home.join(target.folder()).is_dir() || home.join(&location.path).is_file()
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn file(path: &str) -> Location {
        DEFAULTS
            .iter()
            .find(|location| location.path == Path::new(path))
            .cloned()
            .unwrap_or_else(|| {
                if path.starts_with(".codex/rules/") {
                    Location {
                        tool: "codexcli",
                        path: path.into(),
                        format: Format::CommandRules,
                        sections: COMMAND_RULES,
                        initial: "",
                    }
                } else {
                    assert!(path.starts_with(".gemini/policies/"), "{path}");
                    toml_file("geminicli", path, POLICIES)
                }
            })
    }

    #[test]
    fn rulesync_personal_settings_uses_native_opencode_and_inline_codex_shapes() {
        let cursor = file(".cursor/mcp.json");
        let source = cursor
            .document(Some(br#"{"mcpServers":{"sample":{"command":"fixture"}}}"#))
            .unwrap();
        let entry = source.reviewable_entries(cursor.tool).next().unwrap();
        let opencode = file(".config/opencode/opencode.jsonc");
        for (bytes, server_path) in [
            (
                br#"{"$schema":"https://opencode.ai/config.json","permission":{"bash":"ask"}}"#
                    .as_slice(),
                "/mcp/sample",
            ),
            (
                br#"{"$schema":"https://opencode.ai/config.json","permissions":[]}"#.as_slice(),
                "/mcp/servers/sample",
            ),
        ] {
            let doc = opencode.document(Some(bytes)).unwrap();
            let projected = project(&cursor, &opencode, entry, &doc).unwrap();
            let updated = opencode.edit(Some(bytes), &projected).unwrap();
            let raw = document::read(Format::Json, &updated).unwrap();
            assert_eq!(
                raw.pointer(server_path).unwrap()["command"],
                json!(["fixture"])
            );
        }
        for unknown in [
            br#"{"$schema":"https://opencode.ai/config.json","model":"fixture"}"#.as_slice(),
            br#"{"$schema":"https://example.invalid/opencode-v3.json","permissions":[]}"#
                .as_slice(),
            br#"{"permission":"ask","permissions":[]}"#.as_slice(),
            br#"{"permissions":false}"#.as_slice(),
        ] {
            assert!(opencode.document(Some(unknown)).is_err());
        }
        assert!(opencode.document(None).is_err());

        let codex = file(".codex/config.toml");
        let doc = codex.document(Some(b"[[hooks.SessionStart]]\nmatcher = 'startup'\n[[hooks.SessionStart.hooks]]\ntype = 'command'\ncommand = 'fixture-hook'\n")).unwrap();
        let hook = doc.reviewable_entries(codex.tool).next().unwrap();
        assert_eq!(hook.key.feature, Hook);
        assert_eq!(hook.value["hooks"][0]["command"], "fixture-hook");
    }

    fn mcp_entry<'a>(doc: &'a Document, tool: &'a str, name: &str) -> &'a Entry {
        doc.reviewable_entries(tool)
            .find(|entry| entry.key.name.last().map(String::as_str) == Some(name))
            .unwrap_or_else(|| panic!("{name} is not reviewable"))
    }

    #[test]
    fn rulesync_personal_settings_translate_environment_references_between_tools() {
        let claude = file(".claude.json");
        let source = claude
            .document(Some(
                br#"{"mcpServers":{
                "api":{"command":"server","env":{"API_KEY":"${API_KEY}"}},
                "remote":{"type":"http","url":"https://example.invalid/mcp",
                    "headers":{"Authorization":"Bearer ${REMOTE_TOKEN}","X-Team":"${TEAM_ID}"}},
                "renamed":{"command":"server","env":{"API_KEY":"${OTHER_NAME}"}}
            }}"#,
            ))
            .unwrap();
        let api = mcp_entry(&source, claude.tool, "api");
        let remote = mcp_entry(&source, claude.tool, "remote");
        let renamed = mcp_entry(&source, claude.tool, "renamed");

        let cursor = file(".cursor/mcp.json");
        let doc = cursor.document(Some(br#"{"mcpServers":{}}"#)).unwrap();
        let projected = project(&claude, &cursor, api, &doc).unwrap();
        assert_eq!(projected.value["env"], json!({"API_KEY": "${env:API_KEY}"}));
        let projected = project(&claude, &cursor, remote, &doc).unwrap();
        assert_eq!(
            projected.value["headers"],
            json!({"Authorization": "Bearer ${env:REMOTE_TOKEN}", "X-Team": "${env:TEAM_ID}"})
        );
        assert!(reviewable(cursor.tool, &projected));

        let codex = file(".codex/config.toml");
        let doc = codex.document(Some(b"# fixture\n")).unwrap();
        let projected = project(&claude, &codex, api, &doc).unwrap();
        assert_eq!(projected.value["env_vars"], json!(["API_KEY"]));
        assert!(reviewable(codex.tool, &projected));
        let projected = project(&claude, &codex, remote, &doc).unwrap();
        assert_eq!(
            projected.value["bearer_token_env_var"],
            json!("REMOTE_TOKEN")
        );
        assert_eq!(
            projected.value["env_http_headers"],
            json!({"X-Team": "TEAM_ID"})
        );
        assert!(reviewable(codex.tool, &projected));
        // Codex passes a variable through only under its own name.
        assert!(project(&claude, &codex, renamed, &doc).is_none());
        let back = project(
            &codex,
            &claude,
            &projected,
            &claude.document(Some(b"{}")).unwrap(),
        )
        .unwrap();
        assert_eq!(back.value, remote.value);

        let opencode = file(".config/opencode/opencode.jsonc");
        let doc = opencode
            .document(Some(
                br#"{"$schema":"https://opencode.ai/config.json","permission":{"bash":"ask"}}"#,
            ))
            .unwrap();
        let projected = project(&claude, &opencode, api, &doc).unwrap();
        assert_eq!(
            projected.value["environment"],
            json!({"API_KEY": "{env:API_KEY}"})
        );
        // Copilot has no reference syntax Wenlan knows, so nothing is copied.
        let copilot = file(".copilot/mcp-config.json");
        let doc = copilot.document(Some(br#"{"mcpServers":{}}"#)).unwrap();
        assert!(project(&claude, &copilot, api, &doc).is_none());
    }

    #[test]
    fn rulesync_personal_settings_mark_a_written_key_and_keep_it_out() {
        let claude = file(".claude.json");
        let doc = claude
            .document(Some(
                br#"{"mcpServers":{
                "literal-env":{"command":"server","env":{"API_KEY":"fixture-literal-marker"}},
                "literal-header":{"type":"http","url":"https://example.invalid/mcp",
                    "headers":{"Authorization":"Bearer fixture-literal-marker"}},
                "secret-oauth":{"type":"http","url":"https://example.invalid/mcp",
                    "oauth":{"clientId":"fixture-client","clientSecret":"fixture-literal-marker"}},
                "unsafe-arg":{"command":"server","args":["--token","fixture-literal-marker"]},
                "reference":{"command":"server","env":{"API_KEY":"${API_KEY}"}}
            }}"#,
            ))
            .unwrap();
        let refused = doc.refused_entries(claude.tool).collect::<Vec<_>>();
        assert_eq!(
            refused.iter().map(|(_, _, key)| *key).collect::<Vec<_>>(),
            [true, true, true, false],
            "{refused:?}"
        );
        assert!(doc.reviewable_entries(claude.tool).all(|entry| entry
            .key
            .name
            .last()
            .map(String::as_str)
            == Some("reference")));
    }

    #[test]
    fn rulesync_personal_settings_keep_public_oauth_fields_and_refuse_a_client_secret() {
        let claude = file(".claude.json");
        let public = br#"{"mcpServers":{"github":{"type":"http","url":"https://api.example.invalid/mcp/",
            "oauth":{"clientId":"Iv1.fixture-client","callbackPort":8080,"scopes":["repo","read:org"]}}}}"#;
        let doc = claude.document(Some(public)).unwrap();
        mcp_entry(&doc, claude.tool, "github");
        assert!(claude.snapshot_safe(public, &doc));
        for oauth in [
            r#"{"clientId":"fixture","clientSecret":"fixture-marker"}"#,
            r#"{"clientId":"fixture","client_secret":"fixture-marker"}"#,
            r#"{"clientId":"fixture --token fixture-marker"}"#,
            r#"{"clientId":"fixture","scopes":"repo -p fixture-marker"}"#,
        ] {
            let bytes = format!(
                r#"{{"mcpServers":{{"github":{{"type":"http","url":"https://api.example.invalid/mcp/","oauth":{oauth}}}}}}}"#
            );
            let doc = claude.document(Some(bytes.as_bytes())).unwrap();
            assert_eq!(doc.reviewable_entries(claude.tool).count(), 0, "{oauth}");
            assert!(!claude.snapshot_safe(bytes.as_bytes(), &doc), "{oauth}");
        }
    }

    #[test]
    fn rulesync_personal_settings_recognize_wenlan_own_server_without_sharing_it() {
        for (path, bytes) in [
            (
                ".codex/config.toml",
                b"[mcp_servers.wenlan]\ncommand = '/Users/fixture/.wenlan/bin/wenlan-mcp'\nargs = ['--agent-name', 'codex', '--tool-profile', 'daily']\n".as_slice(),
            ),
            (
                ".claude.json",
                br#"{"mcpServers":{"wenlan":{"command":"npx","args":["-y","wenlan-mcp","--agent-name","claude-code"]}}}"#.as_slice(),
            ),
            (
                ".cursor/mcp.json",
                br#"{"mcpServers":{"wenlan":{"command":"C:\\Fixture\\wenlan-mcp.exe","args":["--agent-name","cursor"]}}}"#.as_slice(),
            ),
        ] {
            let location = file(path);
            let doc = location.document(Some(bytes)).unwrap();
            assert_eq!(doc.refused_entries(location.tool).count(), 0, "{path}");
            // Each tool gets its own entry from Wenlan, so it is never managed
            // or copied.
            assert_eq!(doc.reviewable_entries(location.tool).count(), 0, "{path}");
        }
        let claude = file(".claude.json");
        for args in [
            r#"["-y","wenlan-mcp","--agent-name","claude-code","--token","fixture-marker"]"#,
            r#"["-y","wenlan-mcp","--agent-name","fixture marker"]"#,
            r#"["-y","other-mcp","--agent-name","claude-code"]"#,
        ] {
            let bytes =
                format!(r#"{{"mcpServers":{{"wenlan":{{"command":"npx","args":{args}}}}}}}"#);
            let doc = claude.document(Some(bytes.as_bytes())).unwrap();
            assert_eq!(doc.refused_entries(claude.tool).count(), 1, "{args}");
        }
        let bytes = br#"{"mcpServers":{"wenlan":{"command":"wenlan-mcp","args":["--agent-name","claude-code"],"env":{"TOKEN":"fixture-marker"}}}}"#;
        let doc = claude.document(Some(bytes)).unwrap();
        assert_eq!(doc.refused_entries(claude.tool).count(), 1);
    }

    #[test]
    fn rulesync_personal_settings_use_home_paths_on_macos_linux_windows() {
        let expected = [
            ("claudecode", ".claude.json"),
            ("claudecode", ".claude/settings.json"),
            ("codexcli", ".codex/config.toml"),
            ("codexcli", ".codex/hooks.json"),
            ("codexcli", ".codex/rules/default.rules"),
            ("geminicli", ".gemini/settings.json"),
            ("geminicli", ".gemini/policies/user.toml"),
            ("antigravity-ide", ".gemini/config/mcp_config.json"),
            ("antigravity-ide", ".gemini/config/hooks.json"),
            ("antigravity-ide", ".gemini/antigravity-cli/settings.json"),
            ("copilotcli", ".copilot/mcp-config.json"),
            ("copilotcli", ".copilot/hooks/copilot-hooks.json"),
            ("copilotcli", ".copilot/hooks/local.json"),
            ("copilotcli", ".copilot/settings.json"),
            ("cursor", ".cursor/mcp.json"),
            ("cursor", ".cursor/hooks.json"),
            ("cursor", ".cursor/cli-config.json"),
            ("cursor", ".cursor/permissions.json"),
            ("opencode", ".config/opencode/opencode.jsonc"),
            ("opencode", ".config/opencode/opencode.json"),
            ("musecode", ".config/muse/settings.json"),
        ];
        let fixture = tempfile::tempdir().unwrap();
        for path in [
            ".gemini/policies/user.toml",
            ".codex/rules/default.rules",
            ".copilot/hooks/local.json",
        ] {
            let path = fixture.path().join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "").unwrap();
        }
        let catalog = Catalog::read(fixture.path()).unwrap();
        assert_eq!(catalog.locations.len(), expected.len());
        for (tool, path) in expected {
            let location = catalog
                .locations
                .iter()
                .find(|location| location.path == Path::new(path))
                .unwrap();
            assert_eq!(location.tool, tool);
            assert_eq!(location.display(), path);
            assert!(!location.path.is_absolute());
            let parts: Vec<_> = location
                .path
                .iter()
                .map(|part| part.to_str().unwrap())
                .collect();
            for (root, separator) in [
                ("/Users/fixture", "/"),
                ("/home/fixture", "/"),
                (r"C:\Users\fixture", "\\"),
            ] {
                assert_eq!(
                    format!("{root}{separator}{}", parts.join(separator)),
                    format!("{root}{separator}{}", path.replace('/', separator))
                );
            }
        }
    }

    #[test]
    fn rulesync_personal_settings_keep_jsonc_and_toml_siblings_byte_stable() {
        let cursor = file(".cursor/mcp.json");
        let current = br#"{
    "mcpServers": {
        "\u006fwned": { "command": "before" },
        // A comment belongs to the independent server.
        "other" :  { "command":  "other" },
    },
    "other-settings":   [ true, /* keep */ 42 ],
}"#;
        let doc = cursor.document(Some(current)).unwrap();
        let mut owned = doc
            .reviewable_entries(cursor.tool)
            .find(|entry| entry.key.name.last().unwrap() == "owned")
            .unwrap()
            .clone();
        owned.value["command"] = json!("after");
        let result = cursor.edit(Some(current), &owned).unwrap();
        let expected = std::str::from_utf8(current)
            .unwrap()
            .replace("{ \"command\": \"before\" }", "{\"command\":\"after\"}");
        assert_eq!(result, expected.as_bytes());
        let mut new = owned.clone();
        *new.key.name.last_mut().unwrap() = "added".into();
        *new.container.last_mut().unwrap() = "added".into();
        let result = cursor.edit(Some(&result), &new).unwrap();
        assert!(std::str::from_utf8(&result)
            .unwrap()
            .contains("\"other\" :  { \"command\":  \"other\" },"));

        let codex = file(".codex/config.toml");
        let current = b"# top comment\nmodel = 'fixture' # keep this\n\n[mcp_servers.owned]\ncommand = 'before'\n\n[mcp_servers.other]\ncommand='other' # this comment belongs to other\n\n[unrelated]\nkeep = [ 2, 1 ]\n";
        let doc = codex.document(Some(current)).unwrap();
        let mut owned = doc
            .reviewable_entries(codex.tool)
            .find(|entry| entry.key.name.last().unwrap() == "owned")
            .unwrap()
            .clone();
        owned.value["command"] = json!("after");
        let result = codex.edit(Some(current), &owned).unwrap();
        let text = std::str::from_utf8(&result).unwrap();
        assert!(text.starts_with("# top comment\nmodel = 'fixture' # keep this\n"));
        assert!(text.contains("[mcp_servers.other]\ncommand='other' # this comment belongs to other\n\n[unrelated]\nkeep = [ 2, 1 ]\n"), "{text}");
    }

    #[test]
    fn rulesync_personal_settings_native_references_are_backup_safe_but_comments_are_checked() {
        let cursor = file(".cursor/mcp.json");
        let cfg = br#"{"mcpServers":{"reference":{"type":"http","url":"https://example.invalid/mcp","headers":{"X-Custom":"${env:EXAMPLE_TOKEN}"}}}}"#;
        let doc = cursor.document(Some(cfg)).unwrap();
        assert!(cursor.snapshot_safe(cfg, &doc));

        let mut commented = b"/* note\nsome settings\npassword = synthetic-marker\n*/\n".to_vec();
        commented.extend(cfg);
        let doc = cursor.document(Some(&commented)).unwrap();
        assert!(!cursor.snapshot_safe(&commented, &doc));
    }

    #[test]
    fn rulesync_personal_settings_accept_only_native_credential_references() {
        let path = tempfile::tempdir().unwrap();
        let root = path.path();
        fs::create_dir_all(root.join(".cursor")).unwrap();
        fs::create_dir_all(root.join(".config/opencode")).unwrap();
        fs::write(root.join(".cursor/mcp.json"), serde_json::to_vec(&json!({"mcpServers": {
            "preserved": {"type":"http", "url":"https://example.invalid/mcp",
                "headers":{"X-Custom":"${env:EXAMPLE_TOKEN}"}},
            "bad-arg": {"command":"server","args":["--api-key","fixture-argument-marker"]},
            "bad-url": {"type":"http","url":"https://example.invalid/mcp?other=fixture-query-marker"},
            "bad-key": {"command":"server","nested":{"clientSecret":{"value":"fixture-nested-marker"}}},
            "bad-header": {"type":"http","url":"https://example.invalid/mcp","headers":{"Custom":"fixture-header-marker"}},
            "bad-env": {"command":"server","env":{"CUSTOM":"fixture-environment-marker"}},
            "wrong-syntax": {"command":"server","env":{"CUSTOM":"${EXAMPLE_TOKEN}"}}
        }})).unwrap()).unwrap();
        fs::write(root.join(".config/opencode/opencode.jsonc"), br#"{
            "mcp":{"servers":{"kept":{"type":"local","command":["fixture"],"environment":{"CONFIG_PATH":"{env:EXAMPLE_TOKEN}"}},
            "held":{"type":"local","command":["fixture"],"environment":{"GENERIC":"fixture-opencode-marker"}}}}}
        "#).unwrap();
        let adoption = Adoption::discover(root).unwrap();
        let preview = serde_json::to_string(&adoption.preview().unwrap()).unwrap();
        let notes = adoption.notes.join("\n");
        let source = serde_json::to_string(&adoption.sources).unwrap();
        assert_eq!(adoption.sources.values().map(Vec::len).sum::<usize>(), 2);
        assert!(
            preview.contains("${env:EXAMPLE_TOKEN}") && preview.contains("{env:EXAMPLE_TOKEN}")
        );
        assert_eq!(
            adoption
                .notes
                .iter()
                .filter(|note| note.contains("skipped MCP server (entry"))
                .count(),
            7
        );
        for serialized in [preview, notes, source] {
            assert!(!serialized.contains("-marker"));
        }
    }

    #[test]
    fn rulesync_personal_settings_read_codex_and_gemini_permissions_without_writing() {
        let fixture = tempfile::tempdir().unwrap();
        for path in [".codex/rules", ".gemini/policies"] {
            fs::create_dir_all(fixture.path().join(path)).unwrap();
        }
        let codex = b"# fixture comment\nprefix_rule(pattern = [\"git\", [\"status\", \"diff\"]], decision = \"deny\") # keep\n";
        let gemini = b"# fixture comment\n[[rule]]\ntoolName = 'run_shell_command'\ncommandPrefix = 'git status'\ndecision = 'ask_user'\n\n[[rule]]\ntoolName = 'read_file'\ndecision = 'deny' # keep\n";
        for (path, bytes) in [
            (".codex/rules/default.rules", codex.as_slice()),
            (".gemini/policies/user.toml", gemini.as_slice()),
        ] {
            fs::write(fixture.path().join(path), bytes).unwrap();
            let location = Catalog::read(fixture.path())
                .unwrap()
                .locations
                .into_iter()
                .find(|location| location.path == Path::new(path))
                .unwrap();
            let doc = location.document(Some(bytes)).unwrap();
            let mut permission = doc
                .reviewable_entries(location.tool)
                .find(|entry| {
                    entry.value.get("pattern").is_some()
                        || entry.value.get("commandPrefix").is_some()
                })
                .unwrap()
                .clone();
            permission.value["decision"] = json!("allow");
            assert!(location.edit(Some(bytes), &permission).is_err());
            assert_eq!(fs::read(fixture.path().join(path)).unwrap(), bytes);
        }
    }

    /// Whether the whole file may be written and backed up, and how many of
    /// its entries Wenlan may manage.
    fn checked(path: &str, bytes: &str) -> (bool, usize) {
        let location = file(path);
        let doc = location.document(Some(bytes.as_bytes())).unwrap();
        (
            location.snapshot_safe(bytes.as_bytes(), &doc),
            doc.reviewable_entries(location.tool).count(),
        )
    }

    #[test]
    fn rulesync_personal_settings_keep_a_public_header_label_and_refuse_other_literals() {
        let server = |headers: &str| {
            format!(
                r#"{{"mcpServers":{{"exa":{{"type":"http","url":"https://mcp.example.invalid/mcp","headers":{headers}}}}}}}"#
            )
        };
        assert_eq!(
            checked(".claude.json", &server(r#"{"x-exa-source":"claude-code"}"#)),
            (true, 1)
        );
        for headers in [
            r#"{"X-Custom":"fixture-marker"}"#,
            r#"{"Authorization":"Bearer fixturemarker"}"#,
            r#"{"x-exa-source":"fixture:marker"}"#,
            r#"{"x-api-key-source":"fixture"}"#,
            r#"{"user-agent":"fixture marker"}"#,
        ] {
            assert_eq!(
                checked(".claude.json", &server(headers)),
                (false, 0),
                "{headers}"
            );
        }
    }

    #[test]
    fn rulesync_personal_settings_keep_one_package_after_a_launcher_and_refuse_other_words() {
        let server = |command: &str, args: &str| {
            format!(r#"{{"mcpServers":{{"browser":{{"command":"{command}","args":{args}}}}}}}"#)
        };
        for (command, args) in [
            ("npx", r#"["@browsermcp/mcp@latest"]"#),
            ("npx", r#"["-y","fixture-mcp@1.2.0"]"#),
            ("uvx", r#"["fixture-mcp"]"#),
            ("pnpm", r#"["dlx","fixture-mcp"]"#),
        ] {
            assert_eq!(
                checked(".gemini/settings.json", &server(command, args)),
                (true, 1),
                "{command} {args}"
            );
        }
        for (command, args) in [
            ("npx", r#"["fixture-mcp","fixture-marker"]"#),
            ("npx", r#"["fixture-mcp","sk-fixturemarker123"]"#),
            ("npx", r#"["fixture-mcp","--api-key=fixture-marker"]"#),
            ("npx", r#"["--token","fixture-marker","fixture-mcp"]"#),
            ("npx", r#"["Fixture-Marker"]"#),
            ("npx", r#"["fixture-mcp","other-mcp"]"#),
            ("pnpm", r#"["fixture-mcp"]"#),
            ("example", r#"["fixture-marker"]"#),
        ] {
            assert_eq!(
                checked(".gemini/settings.json", &server(command, args)),
                (false, 0),
                "{command} {args}"
            );
        }
    }

    #[test]
    fn rulesync_personal_settings_file_check_skips_wenlan_own_server_only() {
        let own = "model = 'fixture'\n[mcp_servers.wenlan]\ncommand = '/Users/fixture/.wenlan/bin/wenlan-mcp'\nargs = ['--agent-name', 'codex']\n";
        assert_eq!(checked(".codex/config.toml", own), (true, 0));
        let literal = format!("{own}[mcp_servers.other]\ncommand = 'fixture'\nenv = {{ API_TOKEN = 'fixture-marker' }}\n");
        assert_eq!(checked(".codex/config.toml", &literal), (false, 0));
        let not_own = own.replace("'codex']", "'codex', '--token', 'fixture-marker']");
        assert_eq!(checked(".codex/config.toml", &not_own), (false, 0));
    }

    #[test]
    fn rulesync_personal_settings_gemini_sign_in_method_is_not_a_credential() {
        let settings = |auth: &str| format!(r#"{{"security":{{"auth":{auth}}}}}"#);
        for method in [
            "oauth-personal",
            "gemini-api-key",
            "vertex-ai",
            "cloud-shell",
        ] {
            let bytes = settings(&format!(r#"{{"selectedType":"{method}"}}"#));
            assert!(checked(".gemini/settings.json", &bytes).0, "{method}");
        }
        for auth in [
            r#"{"selectedType":"oauth-personal","apiKey":"fixture-marker"}"#,
            r#"{"selectedType":"fixture-marker"}"#,
            r#"{"token":"fixture-marker"}"#,
        ] {
            assert!(
                !checked(".gemini/settings.json", &settings(auth)).0,
                "{auth}"
            );
        }
    }

    #[test]
    fn rulesync_personal_settings_claude_plain_number_and_switch_settings_are_not_credentials() {
        let plain = r#"{"env":{"CLAUDE_CODE_MAX_OUTPUT_TOKENS":"64000","DISABLE_TELEMETRY":"1",
            "USE_FIXTURE":"true","SHARED":"${SHARED_TOKEN}","ANTHROPIC_MODEL":"claude-opus"}}"#;
        assert!(checked(".claude/settings.json", plain).0);
        for env in [
            r#"{"ANTHROPIC_AUTH_TOKEN":"fixture-marker"}"#,
            r#"{"ANTHROPIC_AUTH_TOKEN":"true"}"#,
            r#"{"DB_PASSWORD":"73920184"}"#,
            r#"{"VAULT_PIN":"4821"}"#,
            r#"{"API_TOKEN_LIMIT":"true"}"#,
        ] {
            let bytes = format!(r#"{{"env":{env}}}"#);
            assert!(!checked(".claude/settings.json", &bytes).0, "{env}");
        }
    }

    #[test]
    fn rulesync_personal_settings_flag_values_are_checked_by_shape() {
        let allow = |rules: &[&str]| format!(r#"{{"permissions":{{"allow":{}}}}}"#, json!(rules));
        let rules = [
            "Bash(cargo test -p wenlan:*)",
            "Bash(rg -e fixture-pattern)",
            "Bash(sed -E s/a/b/)",
            // A lowercase word after a flag is a word (UX-SKILL-17).
            "Bash(fixture -e script-name)",
            "Bash(docker run -p 8080:80 fixture:v2)",
            "Bash(fixture --pin)",
            "Bash(mysql -p $MYSQL_PASSWORD)",
        ];
        assert_eq!(
            checked(".claude/settings.json", &allow(&rules)),
            (true, rules.len())
        );
        for rule in [
            "Bash(mysql -p user=fixture)",
            "Bash(fixture -p u:q4)",
            "Bash(fixture -E Fixture4Marker9Value8Abc7Def)",
            "Bash(curl -H fixture)",
            "Bash(curl -u fixture)",
            "Bash(sshpass -p hunter2 ssh fixture)",
            "Bash(zip -P hunter2 fixture.zip)",
            "Bash(mysql -p hunter2)",
            "Bash(cargo test -P hunter2)",
            "Bash(mysql -pabc)",
            "Bash(fixture -maxdepth)",
            "Bash(fixture --count 1234567)",
            "Bash(fixture --user=admin:fixture)",
            "Bash(fixture -u admin:fixture)",
            "Bash(fixture --token fixture)",
            "Bash(openssl enc -pass fixture)",
        ] {
            assert!(text_risky("claudecode", rule), "{rule}");
            assert_eq!(
                checked(".claude/settings.json", &allow(&[rule])),
                (false, 0),
                "{rule}"
            );
        }
    }

    #[test]
    fn rulesync_personal_settings_hook_and_status_line_admit_plain_words_only() {
        let settings = |hook: &str, status: &str| {
            format!(
                r#"{{"hooks":{{"SessionStart":[{{"hooks":[{{"type":"command","command":"{hook}"}}]}}]}},
                "statusLine":{{"type":"command","command":"{status}"}}}}"#
            )
        };
        assert_eq!(
            checked(
                ".claude/settings.json",
                &settings(
                    "wenlan hook session-start --agent claude-code",
                    "fixture-status --compact --format plain"
                )
            ),
            (true, 1)
        );
        for hook in [
            "curl --user fixture",
            "curl -H fixture",
            "fixture --token fixture-marker",
            "fixture --oauth2-bearer ${SHARED_TOKEN}",
            "fixture --agent=claude-code",
            "fixture --env FIXTURE",
            "fixture fixture:marker",
            "fixture a;b",
            "redis-cli -a fixture",
            "sshpass -p hunter2 ssh fixture",
            "deploy --pw hunter2",
            "fixture -k 123e4567-e89b-12d3-a456-426614174000",
            "notify-hook 0123456789abcdef0123456789abcdef",
            "fixture --verbose hunter2",
            "fixture ghp_fixturemarker",
        ] {
            assert_eq!(
                checked(".claude/settings.json", &settings(hook, "fixture-status")),
                (false, 0),
                "{hook}"
            );
        }
        // A status line script by path is public; a credential flag is not.
        assert!(
            checked(
                ".claude/settings.json",
                &settings("fixture", "bash ~/.claude/statusline.sh")
            )
            .0
        );
        assert!(
            !checked(
                ".claude/settings.json",
                &settings("fixture", "fixture --token fixture-marker")
            )
            .0
        );
    }

    #[test]
    fn rulesync_personal_settings_codex_variable_fields_hold_only_usual_names() {
        let remote = |field: &str| {
            format!("[mcp_servers.remote]\nurl = 'https://example.invalid/mcp'\n{field}\n")
        };
        for field in [
            "bearer_token_env_var = 'GITHUB_TOKEN'",
            "env_http_headers = { 'X-Team' = 'TEAM_ID' }",
        ] {
            assert_eq!(
                checked(".codex/config.toml", &remote(field)),
                (true, 1),
                "{field}"
            );
        }
        let local = |name: &str| {
            format!("[mcp_servers.local]\ncommand = 'fixture'\nenv_vars = ['{name}']\n")
        };
        assert_eq!(
            checked(".codex/config.toml", &local("FIXTURE_TOKEN")),
            (true, 1)
        );
        assert_eq!(
            checked(".codex/config.toml", &local("ghp_fixturemarker0123456789")),
            (false, 0)
        );
        for field in [
            "bearer_token_env_var = 'ghp_fixturemarker0123456789'",
            "bearer_token_env_var = 'fixture_marker'",
            "bearer_token_env_var = 'AKIAFIXTUREMARKER'",
            "bearer_token_env_var = 'A1B2C3D4E5F6A7B8C9D0'",
            "env_http_headers = { 'X-Team' = 'ghp_fixturemarker0123456789' }",
        ] {
            assert_eq!(
                checked(".codex/config.toml", &remote(field)),
                (false, 0),
                "{field}"
            );
        }
    }

    #[test]
    fn rulesync_personal_settings_wenlan_own_server_needs_a_plain_program() {
        let own = |command: &str, name: &str| {
            format!(
                r#"{{"mcpServers":{{"wenlan":{{"command":"{command}","args":["-y","wenlan-mcp","--agent-name","{name}"]}}}}}}"#
            )
        };
        assert!(checked(".claude.json", &own("npx", "claude-code")).0);
        for (command, name) in [
            ("FIXTURE=marker npx", "claude-code"),
            ("npx --token fixture-marker", "claude-code"),
            ("npx", "ghp_fixturemarker0123456789"),
        ] {
            assert_eq!(
                checked(".claude.json", &own(command, name)),
                (false, 0),
                "{command} {name}"
            );
        }
    }

    /// Every input from the two independent reviews of this check
    /// (sol-reviews/followups/brief-send-to-all-r3-cases.md). Each one must
    /// keep its file from a whole-file backup and offer no entry.
    #[test]
    fn rulesync_personal_settings_review_inputs_are_refused() {
        let server =
            |url: &str| format!(r#"{{"mcpServers":{{"remote":{{"type":"http","url":"{url}"}}}}}}"#);
        let allow = |rule: &str| format!(r#"{{"permissions":{{"allow":{}}}}}"#, json!([rule]));
        let hook = |command: &str| {
            format!(
                r#"{{"hooks":{{"SessionStart":[{{"hooks":[{{"type":"command","command":{}}}]}}]}}}}"#,
                json!(command)
            )
        };
        let launcher = |args: &str| {
            format!(r#"{{"mcpServers":{{"local":{{"command":"npx","args":{args}}}}}}}"#)
        };
        let mut entries: Vec<(&str, String)> = Vec::new();
        for url in [
            "https://mcp.example.invalid/s/3f9a8b7c6d5e4f3a2b1c0d9e8f7a6b5c/mcp",
            "https://mcp.example.invalid/k/hunter2secretvalue",
            "https://mcp.example.invalid/123e4567-e89b-12d3-a456-426614174000/mcp",
            "https://mcp.example.invalid/mcp;key=hunter2",
            "https://mcp.example.invalid/key=hunter2/mcp",
            "https://3f9a8b7c6d5e4f3a2b1c0d9e8f7a6b5c.example.invalid/mcp",
            "https://hooks.slack.com/services/T00000000/B00000000/XXXXXXXXXXXXXXXXXXXXXXXX",
        ] {
            entries.push((".claude.json", server(url)));
            entries.push((
                ".codex/config.toml",
                format!("[mcp_servers.remote]\nurl = '{url}'\n"),
            ));
        }
        for rule in [
            "Bash(mysql -uroot -phunter2permpw db)",
            "Bash(mysql -uroot -pS3cretPassw0rd)",
            "Bash(7z x -phunter2)",
            "Bash(curl --user admin:hunter2 https://example.invalid)",
            "Bash(redis-cli -a hunter2)",
            "Bash(openssl enc -k hunter2)",
            "Bash(fixture --pass hunter2)",
            "Bash(fixture --pin 4821)",
            "Bash(fixture --otp 123456)",
            "Bash(ngrok config add-authtoken 2abcDEF3ghiJKL)",
            "Bash(PGPASSWORD=hunter2 psql)",
            "Bash(MYSQL_PWD=hunter2 mysql)",
            "Bash(curl -d key=0123456789abcdef0123456789abcdef https://example.invalid)",
            "Bash(fixture 0123456789abcdef0123456789abcdef)",
            "Bash(fixture 0123456789abcdef0123456789abcdef01234567)",
            "Bash(fixture JBSWY3DPEHPK3PXP)",
            "Bash(fixture glpat-AbCdEfGhIjKlMnOpQr12)",
            "Bash(aws configure set aws_secret_access_key wJalrXUtnFEMIK7MDENGbPxRfiCYEXAMPLEKEY)",
            "Bash(sshpass -p hunter2 ssh fixture)",
            "Bash(zip -P hunter2 fixture.zip)",
            "Bash(mysql -p hunter2)",
            "Bash(cargo test -P hunter2)",
            "Bash(mysql -p user=fixture)",
            "Bash(fixture -p u:q4)",
            "Bash(fixture -E Fixture4Marker9Value8Abc7Def)",
            "Bash(curl -H fixture)",
            "Bash(curl -u fixture)",
        ] {
            entries.push((".claude/settings.json", allow(rule)));
        }
        entries.extend([
            (
                ".cursor/permissions.json",
                r#"{"terminalAllowlist":["mysql -uroot -phunter2"]}"#.into(),
            ),
            (
                ".gemini/settings.json",
                r#"{"tools":{"allowed":["run_shell_command(mysql -phunter2)"]}}"#.into(),
            ),
            (
                ".config/opencode/opencode.json",
                r#"{"permission":{"bash":{"redis-cli -a hunter2 *":"allow"}}}"#.into(),
            ),
        ]);
        for rule in [
            r#"pattern = ["sshpass", "-p", "hunter2rulepw"], decision = "allow""#,
            r#"pattern = ["mysql", "--password", "hunter2"], decision = "allow""#,
            r#"pattern = ["curl", "-u", "admin:hunter2"], decision = "allow""#,
            r#"pattern = ["git"], decision = "allow", justification = "pw hunter2""#,
        ] {
            entries.push((
                ".codex/rules/default.rules",
                format!("prefix_rule({rule})\n"),
            ));
        }
        entries.push((
            ".gemini/policies/user.toml",
            "[[rule]]\ntoolName = 'run_shell_command'\ncommandPrefix = ['curl', '-u', 'admin:hunter2']\ndecision = 'allow'\n".into(),
        ));
        for command in [
            "notify hunter2",
            "unlock 4821",
            "notify a1b2c3d4e5f6g7h",
            "redis-cli -a fixture",
            "sshpass -p hunter2 ssh fixture",
            "deploy --pw hunter2",
            "fixture -k 123e4567-e89b-12d3-a456-426614174000",
            "notify-hook 0123456789abcdef0123456789abcdef",
            "fixture --verbose hunter2",
            "fixture ghp_fixturemarker",
        ] {
            entries.push((".claude/settings.json", hook(command)));
        }
        // `npx -y hunter2` is shaped like a package name and passes by
        // approval (UX-SKILL-17); a password in the version does not.
        entries.push((".claude.json", launcher(r#"["-y","some-mcp@hunter2pass"]"#)));
        for (path, bytes) in &entries {
            assert_eq!(checked(path, bytes), (false, 0), "{path}: {bytes}");
        }

        // Inputs that are not entries keep the whole file from a backup.
        let mut files: Vec<(&str, String)> = Vec::new();
        for setting in [
            "[shell_environment_policy]\nset = { DB_PASS = 'hunter2codexpw' }",
            "[shell_environment_policy]\nset = { MYSQL_PWD = \"hunter2\" }",
            "[shell_environment_policy]\nset = { ADMIN_PIN = \"4821\" }",
            "proxy_pass = \"hunter2\"",
            "[mcp_servers.remote]\nurl = 'https://mcp.example.invalid/mcp'\nquery_params = { code = '4/0AfJohXnFixtureMarker' }",
            "model = 'fixture' # vpn password hunter2",
            "# vpn password hunter2\nmodel = 'fixture'",
        ] {
            files.push((".codex/config.toml", format!("{setting}\n")));
        }
        files.push((
            ".claude/settings.json",
            r#"{"companyAnnouncements":["vpn pass is hunter2"]}"#.into(),
        ));
        for env in [
            r#""MFA_BACKUP_CODE":"84736251""#,
            r#""CARD_CVC":"123""#,
            r#""SIM_PUK":"12345678""#,
            r#""ACCOUNT_NUMBER":"0123456789""#,
            r#""TOTP":"123456""#,
            r#""DOOR_CODE":4821"#,
            r#""API_SECRET_MS":"84736251""#,
            r#""DB_PASSWORD":"73920184""#,
            r#""VAULT_PIN":"4821""#,
            r#""API_TOKEN_LIMIT":"true""#,
            r#""PGPASSWORD":"hunter2""#,
            r#""GITHUBTOKEN":"fixture""#,
        ] {
            files.push((".claude/settings.json", format!(r#"{{"env":{{{env}}}}}"#)));
        }
        files.push((
            ".codex/config.toml",
            "[model_providers.fixture]\nenv_key = 'fixture_marker'\n".into(),
        ));
        for (path, bytes) in &files {
            assert!(!checked(path, bytes).0, "{path}: {bytes}");
        }
    }

    /// The settings the owner approved as public keep passing.
    #[test]
    fn rulesync_personal_settings_approved_public_settings_pass() {
        let server = |fields: &str| format!(r#"{{"mcpServers":{{"fixture":{{{fields}}}}}}}"#);
        for fields in [
            r#""command":"npx","args":["-y","@scope/pkg@1.2.3"]"#,
            r#""command":"npx","args":["-y","@browsermcp/mcp@latest"]"#,
            r#""command":"uvx","args":["fixture-mcp@1.2.0"]"#,
            r#""command":"pnpm","args":["dlx","fixture-mcp"]"#,
            r#""type":"http","url":"https://mcp.example.invalid/mcp","oauth":{"clientId":"fixture-client","callbackPort":8080,"scopes":["read","write"]}"#,
            r#""type":"http","url":"https://mcp.example.invalid/mcp","headers":{"x-exa-source":"claude-code"}"#,
            r#""type":"http","url":"https://mcp.example.invalid/mcp","headers":{"Authorization":"Bearer ${FIXTURE_TOKEN}"}"#,
            r#""type":"http","url":"https://mcp.example.invalid/v1/mcp","headers":{"Accept":"application/json, text/event-stream","User-Agent":"claude-code/1.0","Content-Type":"application/json; charset=utf-8"}"#,
        ] {
            assert_eq!(
                checked(".claude.json", &server(fields)),
                (true, 1),
                "{fields}"
            );
        }
        let cursor = server(
            r#""url":"https://mcp.example.invalid/mcp","headers":{"Authorization":"Bearer ${env:FIXTURE_TOKEN}"}"#,
        );
        assert_eq!(checked(".cursor/mcp.json", &cursor), (true, 1));
        let gemini = server(
            r#""httpUrl":"https://mcp.example.invalid/mcp","headers":{"Authorization":"Bearer $FIXTURE_TOKEN"}"#,
        );
        assert_eq!(checked(".gemini/settings.json", &gemini), (true, 1));
        for field in [
            "bearer_token_env_var = 'GITHUB_TOKEN'",
            "bearer_token_env_var = 'CORRECTHORSEBATTERY'",
        ] {
            let bytes =
                format!("[mcp_servers.remote]\nurl = 'https://mcp.example.invalid/mcp'\n{field}\n");
            assert_eq!(checked(".codex/config.toml", &bytes), (true, 1), "{field}");
        }
        let bytes = "[mcp_servers.local]\ncommand = 'fixture'\nenv_vars = ['FIXTURE_TOKEN', 'GITHUB_TOKEN']\n";
        assert_eq!(checked(".codex/config.toml", bytes), (true, 1));
        // A setting name the catalog declares is a fixed spelling, not a key;
        // a long mixed-case name the person chose is still refused.
        let copilot = r#"{"defaultPermissionMode":"prompt"}"#;
        assert_eq!(checked(".copilot/settings.json", copilot), (true, 1));
        let chosen = r#"{"permissions":{"wJalrXUtnFEMIKMDENGbPxRfiCYEXAMPLEKEY":"allow"}}"#;
        assert_eq!(checked(".copilot/settings.json", chosen), (false, 0));

        let rules = [
            "Bash(cargo test -p wenlan-core:*)",
            "Bash(grep -e fixture-pattern)",
            "Bash(rg -e fixture-pattern)",
            "Bash(sed -E s/a/b/)",
            "Bash(pytest -p no:cacheprovider)",
            "Bash(echo -e fixture)",
            "WebFetch(domain:docs.example.com)",
            "Bash(git push:*)",
        ];
        let allow = format!(r#"{{"permissions":{{"allow":{}}}}}"#, json!(rules));
        assert_eq!(
            checked(".claude/settings.json", &allow),
            (true, rules.len())
        );

        let settings = format!(
            r#"{{"env":{{"CLAUDE_CODE_MAX_OUTPUT_TOKENS":"64000","BASH_DEFAULT_TIMEOUT_MS":"120000","ANTHROPIC_MODEL":"claude-opus"}},
            "hooks":{{"SessionStart":[{{"hooks":[{{"type":"command","command":{}}}]}}]}},
            "statusLine":{{"type":"command","command":"bash ~/.claude/statusline.sh"}}}}"#,
            json!("wenlan hook session-start --agent claude-code --agent-name claude-code --tool claude --tool-profile full --format json --space work --session current")
        );
        assert_eq!(checked(".claude/settings.json", &settings), (true, 1));
        let own = r#"{"mcpServers":{"wenlan":{"command":"wenlan-mcp","args":["--agent-name","claude-code"]}}}"#;
        assert_eq!(checked(".claude.json", own), (true, 0));
    }

    /// Everyday public settings keep syncing (round 3b corpus). The inputs
    /// that are public only by the person's approval, because a password
    /// shaped like a word cannot be told apart, pass too (UX-SKILL-17).
    #[test]
    fn rulesync_personal_settings_everyday_settings_pass() {
        let rules = [
            "Bash(git diff --stat HEAD)",
            "Bash(ls -la src)",
            "Bash(mkdir -p dir)",
            "Bash(rm -rf dir)",
            "Bash(tar -xzf f.tgz)",
            "Bash(git commit -am msg)",
            "Bash(docker ps -a)",
            "Bash(curl -k https://localhost:8443)",
            "Bash(gh auth status)",
            "Bash(npm run auth:test)",
            "Bash(git log --author=me)",
            "Bash(jest --passWithNoTests)",
            "Bash(cargo nextest run -p wenlan-core)",
            "Bash(gh pr view:*)",
        ];
        for rule in rules {
            assert!(!text_risky("claudecode", rule), "{rule}");
        }
        let settings = format!(
            r#"{{"enableAllProjectMcpServers":true,"enabledMcpjsonServers":["github-auth"],
            "alwaysThinkingEnabled":true,"includeCoAuthoredBy":false,"spinnerTipsEnabled":false,
            "spinnerVerbs":{{"mode":"append","verbs":["Pondering"]}},
            "skipDangerousModePermissionPrompt":true,"cleanupPeriodDays":30,
            "outputStyle":"Explanatory",
            "statusLine":{{"type":"command","command":"bash ~/.claude/statusline.sh"}},
            "apiKeyHelper":"~/.claude/api-key-helper.sh","forceLoginMethod":"claudeai",
            "env":{{"NODE_OPTIONS":"--max-old-space-size=4096","EXIT_CODE":"0","ANTHROPIC_MODEL":"claude-opus"}},
            "permissions":{{"defaultMode":"acceptEdits","additionalDirectories":["~/work"],"allow":{}}}}}"#,
            json!(rules)
        );
        // Every rule plus `defaultMode` and `additionalDirectories`.
        assert_eq!(
            checked(".claude/settings.json", &settings),
            (true, rules.len() + 2)
        );

        let server = |name: &str, args: &str| {
            format!(r#"{{"mcpServers":{{"{name}":{{"command":"npx","args":{args}}}}}}}"#)
        };
        for (name, args) in [
            ("context7", r#"["-y","@upstash/context7-mcp"]"#),
            ("magic", r#"["@21st-dev/magic@latest"]"#),
            (
                "github-auth",
                r#"["-y","@modelcontextprotocol/server-github"]"#,
            ),
            // Public by approval: a word shaped like a package name.
            ("local", r#"["-y","hunter2"]"#),
        ] {
            assert_eq!(
                checked(".claude.json", &server(name, args)),
                (true, 1),
                "{name} {args}"
            );
        }
        let sqlite = r#"{"mcpServers":{"sqlite":{"command":"uvx","args":["mcp-server-sqlite3"]}}}"#;
        assert_eq!(checked(".claude.json", sqlite), (true, 1));
        let codex = "[model_providers.openai]\nname = 'OpenAI'\nbase_url = 'https://api.openai.com/v1'\nenv_key = 'OPENAI_API_KEY'\n\n[mcp_servers.github-auth]\ncommand = 'fixture'\n";
        assert_eq!(checked(".codex/config.toml", codex), (true, 1));
        // Public by approval: a short credential word inside one word.
        assert!(checked(".claude/settings.json", r#"{"env":{"MYPIN":"4821"}}"#).0);

        for word in [
            "author",
            "spinner",
            "bypass",
            "hotpath",
            "keyboard",
            "tokenizer",
            "EXIT_CODE",
            "classname",
            "MYPIN",
            "includeCoAuthoredBy",
        ] {
            assert!(!credential_word(word), "{word}");
        }
        for word in [
            "DB_PASS",
            "authCode",
            "ACCOUNT_NUMBER",
            "PGPASSWORD",
            "add-authtoken",
            "MFA_BACKUP_CODE",
            "SESSION_ID",
            "APIKey",
            "TOTP",
        ] {
            assert!(credential_word(word), "{word}");
        }
        assert!(!secret_looking("skipDangerousModePermissionPrompt"));
        assert!(secret_looking("wJalrXUtnFEMIKMDENGbPxRfiCYEXAMPLEKEY"));
    }
}
