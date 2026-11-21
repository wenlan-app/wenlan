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
    is_env_name, validate_text, MAX_SETTINGS_BYTES,
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
    fn label(self) -> &'static str {
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
        std::str::from_utf8(bytes).is_ok_and(|text| {
            if self.format == Format::Json {
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
        }) && material_safe(self.tool, &doc.raw)
            && doc.sections.iter().all(|&Section(feature, path, shape)| {
                let Some(value) = lookup(&doc.raw, path.iter().copied()) else {
                    return true;
                };
                match (feature, shape) {
                    (Mcp, Named) => value.as_object().is_some_and(|servers| {
                        servers
                            .values()
                            .all(|server| mcp_payload_safe(self.tool, server))
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
        self.entries
            .values()
            .flatten()
            .enumerate()
            .filter(|(_, (entry, _))| !reviewable(location.tool, entry))
            .map(move |(index, (entry, _))| format!(
                "{path}: skipped {} (entry {}) because it contains possible credentials or unsafe text.",
                entry.key.feature.label(), index + 1
            ))
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

fn credential_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase().replace(['-', '_', '.', ' '], "");
    matches!(
        key.as_str(),
        "key"
            | "auth"
            | "authentication"
            | "bearer"
            | "oauth"
            | "oauth2"
            | "signature"
            | "session"
            | "userinfo"
            | "username"
    ) || [
        "apikey",
        "password",
        "passwd",
        "passphrase",
        "secret",
        "privatekey",
        "accesskey",
        "token",
        "authorization",
        "cookie",
        "credential",
        "clientsecret",
        "sessionid",
    ]
    .iter()
    .any(|word| key.contains(word))
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
            .is_some_and(is_env_name)
    };
    match tool {
        "claudecode" => wrapped("${", "}"),
        "geminicli" => wrapped("${", "}") || value.strip_prefix('$').is_some_and(is_env_name),
        "cursor" => wrapped("${env:", "}"),
        "opencode" => wrapped("{env:", "}"),
        // Codex's env-valued fields use bare variable names below.
        _ => false,
    }
}

fn text_risky(tool: &str, value: &str) -> bool {
    static RISK: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
        r"(?i)(?:[a-z][a-z0-9+.-]*://[^/\s?#]*@|[a-z][a-z0-9+.-]*://[^\s]*[?#]|\b(?:sk[-_]|gh[pousr]_|github_pat_|xox[baprs]-|AKIA|eyJ[a-zA-Z0-9_-]+\.)[a-z0-9_./+=-]{6,}|(?:^|\s)-(?:u|H|p|e)(?:\s+|=)[^\s]+|(?:^|\s)--(?:key|auth|header|env|client[-_]?secret)(?:\s+|=)[^\s]+|-----BEGIN [A-Z ]*PRIVATE KEY-----|\$\{[^}\n]+:-[^}\n]+|\b(?:api[-_]?key|client[-_]?secret|password|passwd|secret|token)\s*[:=]\s*[^\s]+)",
    ).expect("settings credential indicators")
    });
    if exact_reference(tool, value) {
        return false;
    }
    RISK.is_match(value)
        || validate_text(value).is_err()
        || value
            .split(|ch: char| !(ch.is_ascii_alphanumeric() || "-_".contains(ch)))
            .any(|word| {
                word.len() >= 24
                    && word.bytes().any(|b| b.is_ascii_uppercase())
                    && word.bytes().any(|b| b.is_ascii_lowercase())
                    && word.bytes().filter(u8::is_ascii_digit).count() >= 3
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

fn command_safe(tool: &str, command: &Value) -> bool {
    match command {
        Value::String(command) => {
            // Refuse shell operators, assignments, quoting we cannot
            // interpret losslessly, and inline scripts.
            let mut parts = command.split_ascii_whitespace();
            !command.contains(['\r', '\n'])
                && parts.next().is_some_and(|name| executable(tool, name))
                && parts.all(|arg| safe_argument(tool, arg))
        }
        Value::Array(parts) => parts.split_first().is_some_and(|(name, args)| {
            name.as_str().is_some_and(|name| executable(tool, name))
                && args
                    .iter()
                    .all(|arg| arg.as_str().is_some_and(|arg| safe_argument(tool, arg)))
        }),
        _ => false,
    }
}

fn material_safe(tool: &str, value: &Value) -> bool {
    match value {
        Value::Object(map) => map
            .iter()
            .all(|(name, value)| !text_risky(tool, name) && safe_field(tool, name, value)),
        Value::Array(items) => items.iter().all(|item| material_safe(tool, item)),
        Value::String(value) => !text_risky(tool, value),
        _ => true,
    }
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
        "bearertokenenvvar" => tool == "codexcli" && value.as_str().is_some_and(is_env_name),
        "envvars" => {
            tool == "codexcli"
                && value.as_array().is_some_and(|names| {
                    names
                        .iter()
                        .all(|name| name.as_str().is_some_and(is_env_name))
                })
        }
        "url" | "serverurl" | "httpurl" => value.as_str().is_some_and(|url| {
            exact_reference(tool, url)
                || (!text_risky(tool, url)
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
                map.iter().all(|(key, value)| {
                    !text_risky(tool, key)
                        && !credential_key(key)
                        && value.as_str().is_some_and(|value| {
                            if normalized == "envhttpheaders" && tool == "codexcli" {
                                is_env_name(value)
                            } else {
                                exact_reference(tool, value)
                            }
                        })
                })
            })
        }
        _ => !credential_key(name) && material_safe(tool, value),
    }
}

fn mcp_payload_safe(tool: &str, value: &Value) -> bool {
    value.as_object().is_some_and(|fields| {
        !fields.is_empty()
            && fields.iter().all(|(name, value)| {
                let normalized = name.to_ascii_lowercase().replace(['-', '_'], "");
                match normalized.as_str() {
                    "command" => command_safe(tool, value),
                    "args" | "env" | "environment" | "envvars" | "headers" | "httpheaders"
                    | "envhttpheaders" | "bearertokenenvvar" | "url" | "serverurl" | "httpurl" => {
                        safe_field(tool, name, value)
                    }
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
                    | "powershell" => value.is_string() && command_safe(tool, value),
                    "type" => matches!(value.as_str(), Some("command" | "http")),
                    "url" | "headers" | "httpheaders" => safe_field(tool, name, value),
                    "allowedenvvars" => value.as_array().is_some_and(|names| {
                        names
                            .iter()
                            .all(|name| name.as_str().is_some_and(is_env_name))
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
    entry
        .container
        .iter()
        .all(|name| !text_risky(tool, name) && !credential_key(name))
        && entry
            .key
            .member
            .as_deref()
            .is_none_or(|name| !text_risky(tool, name))
        && match entry.key.feature {
            Mcp => mcp_payload_safe(tool, &entry.value),
            Hook if tool == "antigravity-ide" => hook_groups_safe(tool, &entry.value),
            Hook => hook_payload_safe(tool, &entry.value),
            Permission => entry.container.last().map_or_else(
                || material_safe(tool, &entry.value),
                |name| safe_field(tool, name, &entry.value),
            ),
        }
}

fn reference_free(text: &str) -> bool {
    !text.contains(['$', '%', '{', '}']) && !text.contains('\\')
}

#[derive(PartialEq, Eq)]
enum PortableMcp {
    Stdio(Vec<String>),
    Http(String),
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
        PortableMcp::Stdio(command)
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
        PortableMcp::Http(url)
    };
    fields.is_empty().then_some(result)
}

fn native_mcp(tool: &str, portable: &PortableMcp) -> Value {
    match portable {
        PortableMcp::Stdio(command) => match tool {
            "opencode" => json!({"type":"local", "command":command}),
            "musecode" => json!({"transport":"stdio", "command":command[0], "args":command[1..]}),
            "copilotcli" => json!({"type":"stdio", "command":command[0], "args":command[1..]}),
            _ => json!({"command":command[0], "args":command[1..]}),
        },
        PortableMcp::Http(url) => match tool {
            "opencode" => json!({"type":"remote", "url":url}),
            "musecode" => json!({"transport":"streamable_http", "url":url}),
            "antigravity-ide" => json!({"serverUrl":url}),
            "geminicli" => json!({"httpUrl":url}),
            "codexcli" => json!({"url":url}),
            _ => json!({"type":"http", "url":url}),
        },
    }
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
            let value = native_mcp(to.tool, &portable);
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
            .unwrap()
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
}
