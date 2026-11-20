// SPDX-License-Identifier: Apache-2.0
//! Plan an explicit local settings migration from pinned rulesync output.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::{ErrorKind, Read};
use std::path::{Component, Path, PathBuf};
use std::sync::{LazyLock, OnceLock};

use anyhow::{bail, ensure, Context, Result};
use cap_fs_ext::{DirExt as _, FollowSymlinks, OpenOptionsFollowExt as _};
use cap_std::fs::{Dir, OpenOptions};
use hmac::{Hmac, Mac};
use regex::Regex;
use serde::de::{MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};
use sha2::Sha256;
use wenlan_types::rulesync::AdoptionChange;

use crate::rulesync_personal::{fingerprint, reject_secret_lines};
use crate::rulesync_renderer;
use crate::skill_registry::{is_invisible, printable};

type JsonObject = Map<String, Value>;

const MAX_SETTINGS_BYTES: u64 = 10 * 1024 * 1024;
const SOURCES: &[(&str, &[&str])] = &[
    ("mcp", &["mcp.jsonc", "mcp.json", ".mcp.json"]),
    ("hooks", &["hooks.jsonc", "hooks.json"]),
    ("permissions", &["permissions.jsonc", "permissions.json"]),
];

pub struct RenderedSettings {
    pub source_digest: String,
    pub files: BTreeMap<PathBuf, Vec<u8>>,
}

/// Stage only settings sources accepted by rulesync. Neither rulesync project
/// config nor other files from the selected folder reach the renderer.
pub fn render_source(
    binary: &Path,
    stage_parent: &Path,
    source: &Path,
) -> Result<RenderedSettings> {
    let inputs = read_sources(source)?;
    let source_digest = fingerprint(&inputs)?;
    let stage = rulesync_renderer::new_stage(stage_parent)?;
    let input = stage.path().join("input");
    let output = stage.path().join("output");
    fs::create_dir(&input)?;
    fs::create_dir(&output)?;
    for (feature, bytes) in &inputs {
        fs::write(input.join(format!("{feature}.jsonc")), bytes)?;
    }
    let features: Vec<_> = inputs.keys().copied().collect();
    rulesync_renderer::render_settings(binary, stage.path(), &input, &output, &features)?;
    let mut files = BTreeMap::new();
    read_outputs(&output, &output, &mut files)?;
    Ok(RenderedSettings {
        source_digest,
        files,
    })
}

fn read_sources(path: &Path) -> Result<BTreeMap<&'static str, Vec<u8>>> {
    let mut root = open_source(path)?;
    if path.file_name().is_some_and(|name| name != ".rulesync") {
        match root.open_dir_nofollow(".rulesync") {
            Ok(nested) => root = nested,
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(_) => bail!("the .rulesync source must be a regular directory"),
        }
    }
    let mut sources = BTreeMap::new();
    for (feature, names) in SOURCES {
        for &name in *names {
            let metadata = match root.symlink_metadata(name) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == ErrorKind::NotFound => continue,
                Err(_) => bail!("could not inspect {name}"),
            };
            ensure!(metadata.is_file(), "{name} must be a regular file");
            let mut options = OpenOptions::new();
            options.read(true).follow(FollowSymlinks::No);
            #[cfg(unix)]
            {
                use cap_std::fs::OpenOptionsExt as _;
                options.custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32);
            }
            let file = root
                .open_with(name, &options)
                .with_context(|| format!("open {name}"))?;
            ensure!(file.metadata()?.is_file(), "{name} changed while reading");
            let bytes = read_limited(file).with_context(|| format!("read {name}"))?;
            let json =
                parse_json(&without_jsonc(&bytes)?).with_context(|| format!("parse {name}"))?;
            validate_material(&Value::Object(json.clone()))
                .with_context(|| format!("check {name}"))?;
            sources.insert(*feature, serde_json::to_vec(&json)?);
            break;
        }
    }
    ensure!(!sources.is_empty(), "no rulesync settings sources found");
    Ok(sources)
}

fn open_source(path: &Path) -> Result<Dir> {
    ensure!(
        path.is_absolute() && path.file_name().is_some(),
        "settings source must be an absolute local directory"
    );
    let mut base = PathBuf::new();
    let mut directory: Option<Dir> = None;
    for part in path.components() {
        match part {
            #[cfg(windows)]
            Component::Prefix(prefix)
                if matches!(
                    prefix.kind(),
                    std::path::Prefix::Disk(_) | std::path::Prefix::VerbatimDisk(_)
                ) =>
            {
                base.push(part.as_os_str());
            }
            Component::RootDir => {
                base.push(part.as_os_str());
                directory = Some(Dir::open_ambient_dir(&base, cap_std::ambient_authority())?);
            }
            Component::Normal(name) => {
                directory = Some(
                    directory
                        .context("missing source path root")?
                        .open_dir_nofollow(Path::new(name))
                        .context("settings source must have regular directory components")?,
                );
            }
            _ => bail!("settings source must be a local path without parent traversal"),
        }
    }
    directory.context("missing settings source directory")
}

fn read_limited(reader: impl Read) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_SETTINGS_BYTES + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= MAX_SETTINGS_BYTES,
        "settings file exceeds 10 MiB"
    );
    Ok(bytes)
}

fn read_outputs(
    root: &Path,
    directory: &Path,
    files: &mut BTreeMap<PathBuf, Vec<u8>>,
) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let kind = entry.file_type()?;
        if kind.is_dir() {
            read_outputs(root, &path, files)?;
        } else {
            ensure!(kind.is_file(), "unexpected staged settings file type");
            files.insert(
                path.strip_prefix(root)?.to_owned(),
                read_limited(fs::File::open(&path)?)?,
            );
        }
    }
    Ok(())
}

/// Bind the preview to exact snapshots without exposing an unsalted digest
/// of secrets already present in local configuration. Restarting requires a
/// new preview.
pub fn confirmation_token(value: &impl Serialize) -> Result<String> {
    static KEY: OnceLock<[u8; 16]> = OnceLock::new();
    let key = KEY.get_or_init(|| *uuid::Uuid::new_v4().as_bytes());
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("valid HMAC key");
    mac.update(fingerprint(value)?.as_bytes());
    Ok(format!("{:x}", mac.finalize().into_bytes()))
}

pub struct SettingsChange {
    pub bytes: Vec<u8>,
    pub preview: AdoptionChange,
}

/// Return a new file and the additions the person would approve. Live bytes
/// serve as the merge base and are never included in the preview.
pub fn merge_file(
    path: &Path,
    current: Option<&[u8]>,
    staged: &[u8],
) -> Result<Option<SettingsChange>> {
    let merged = match path.extension().and_then(|ext| ext.to_str()) {
        Some("json") => merge_json(current, staged)?,
        Some("toml") => merge_toml(current, staged)?,
        Some("rules") => merge_command_rules(current, staged)?,
        _ => bail!("unsupported output from the settings renderer"),
    };
    let Some((bytes, additions)) = merged else {
        return Ok(None);
    };
    let path = path
        .to_str()
        .context("settings destination must be UTF-8")?;
    let display_path = printable(path);
    let mut diff = format!("+++ {display_path}\n");
    for line in additions.lines() {
        diff.push('+');
        diff.push_str(&printable(line));
        diff.push('\n');
    }
    Ok(Some(SettingsChange {
        bytes,
        preview: AdoptionChange {
            path: display_path,
            added: additions.lines().count(),
            removed: 0,
            diff,
        },
    }))
}

type Merged = Option<(Vec<u8>, String)>;

fn merge_json(current: Option<&[u8]>, staged: &[u8]) -> Result<Merged> {
    let mut next = current.map(parse_json).transpose()?.unwrap_or_default();
    let mut patch = parse_json(staged)?;
    validate_material(&Value::Object(patch.clone()))?;
    let schema = patch.remove("$schema");
    let mut delta = Map::new();
    for (key, value) in patch {
        let proposed = value
            .as_object()
            .context("generated section is not an object")?;
        let changes = match key.as_str() {
            "mcpServers" => edit_json_section(&mut next, &key, proposed, merge_mcp)?,
            "hooks" => edit_json_section(&mut next, &key, proposed, merge_hooks)?,
            "permissions" => edit_json_section(&mut next, &key, proposed, merge_permissions)?,
            "sandbox" => edit_json_section(&mut next, &key, proposed, merge_entries)?,
            _ => bail!("unsupported JSON settings section from rulesync"),
        };
        if !changes.is_empty() {
            delta.insert(key, Value::Object(changes));
        }
    }
    if delta.is_empty() {
        return Ok(None);
    }
    if let Some(schema) = schema.filter(|_| !next.contains_key("$schema")) {
        next.insert("$schema".into(), schema.clone());
        delta.insert("$schema".into(), schema);
    }
    let mut bytes = serde_json::to_vec_pretty(&next)?;
    bytes.push(b'\n');
    Ok(Some((bytes, serde_json::to_string_pretty(&delta)?)))
}

fn edit_json_section(
    config: &mut JsonObject,
    key: &str,
    proposal: &JsonObject,
    merge: fn(&mut JsonObject, &JsonObject) -> Result<JsonObject>,
) -> Result<JsonObject> {
    if proposal.is_empty() {
        return Ok(Map::new());
    }
    let section = config
        .entry(key.to_owned())
        .or_insert_with(|| Value::Object(Map::new()));
    merge(
        section
            .as_object_mut()
            .context("existing section is not an object")?,
        proposal,
    )
}

fn merge_mcp(
    existing: &mut Map<String, Value>,
    proposal: &Map<String, Value>,
) -> Result<Map<String, Value>> {
    let mut delta = Map::new();
    for (name, proposed) in proposal {
        ensure!(
            proposed.is_object(),
            "generated MCP server is not an object"
        );
        if let Some(current) = existing.get(name) {
            ensure!(
                json_contains(current, proposed),
                "MCP server name already has different settings"
            );
        } else {
            existing.insert(name.clone(), proposed.clone());
            delta.insert(name.clone(), proposed.clone());
        }
    }
    Ok(delta)
}

fn json_contains(current: &Value, proposed: &Value) -> bool {
    match (current.as_object(), proposed.as_object()) {
        (Some(current), Some(proposed)) => proposed.iter().all(|(key, value)| {
            current
                .get(key)
                .is_some_and(|entry| json_contains(entry, value))
        }),
        _ => current == proposed,
    }
}

/// Keep existing hooks in execution order. Append only new handlers to a
/// matching group or append a new group when the matcher is new.
fn merge_hooks(
    existing: &mut Map<String, Value>,
    proposal: &Map<String, Value>,
) -> Result<Map<String, Value>> {
    let mut delta = Map::new();
    for (event, groups) in proposal {
        let groups = groups
            .as_array()
            .context("generated event hooks must be an array")?;
        if groups.is_empty() {
            continue;
        }
        let current = existing
            .entry(event.clone())
            .or_insert_with(|| Value::Array(Vec::new()));
        let current = current
            .as_array_mut()
            .context("existing event hooks must be an array")?;
        let mut added = Vec::new();
        for group in groups {
            if let Some(change) = merge_hook_group(current, group)? {
                added.push(change);
            }
        }
        if !added.is_empty() {
            delta.insert(event.clone(), Value::Array(added));
        }
    }
    Ok(delta)
}

fn hook_matcher(group: &Value) -> Result<Option<&str>> {
    let group = group
        .as_object()
        .context("hook matcher group must be an object")?;
    match group.get("matcher") {
        Some(Value::String(matcher)) => {
            Ok(Some(matcher.as_str()).filter(|matcher| !matcher.is_empty()))
        }
        Some(_) => bail!("hook matcher must be a string"),
        None => Ok(None),
    }
}

fn merge_hook_group(current: &mut Vec<Value>, group: &Value) -> Result<Option<Value>> {
    let matcher = hook_matcher(group)?;
    let proposed = group
        .as_object()
        .context("generated hook group must be an object")?;
    let handlers = proposed
        .get("hooks")
        .and_then(Value::as_array)
        .context("generated hook group has no handlers")?;
    if handlers.is_empty() {
        return Ok(None);
    }
    let mut matches = Vec::new();
    for (index, other) in current.iter().enumerate() {
        if hook_matcher(other)? == matcher {
            matches.push(index);
        }
    }
    match matches.as_slice() {
        [] => {
            current.push(group.clone());
            Ok(Some(group.clone()))
        }
        [index] => {
            let existing = current[*index]
                .as_object_mut()
                .expect("verified hook group");
            ensure!(
                proposed.iter().all(|(key, value)| {
                    matches!(key.as_str(), "matcher" | "hooks") || existing.get(key) == Some(value)
                }),
                "hook matcher group has different settings"
            );
            let handlers_at_matcher = existing
                .get_mut("hooks")
                .and_then(Value::as_array_mut)
                .context("existing hook group has no handlers")?;
            let additions = append_hooks(handlers_at_matcher, handlers)?;
            if additions.is_empty() {
                return Ok(None);
            }
            let mut added = proposed.clone();
            added.insert("hooks".into(), Value::Array(additions));
            Ok(Some(Value::Object(added)))
        }
        _ => {
            ensure!(
                current.contains(group),
                "multiple existing hook groups match a proposed group"
            );
            Ok(None)
        }
    }
}

fn append_hooks(existing: &mut Vec<Value>, proposal: &[Value]) -> Result<Vec<Value>> {
    let mut additions = Vec::new();
    for hook in proposal {
        ensure!(hook.is_object(), "generated hook must be an object");
        if existing.iter().any(|current| json_contains(current, hook)) {
            continue;
        }
        let kind = hook.get("type").and_then(Value::as_str);
        let keys: &[&str] = match kind {
            Some("command") => &["command"],
            Some("prompt" | "agent") => &["prompt"],
            Some("http") => &["url"],
            Some("mcp_tool") => &["server", "tool"],
            _ => &[],
        };
        if !keys.is_empty() {
            ensure!(
                !existing.iter().any(|other| {
                    other.get("type").and_then(Value::as_str) == kind
                        && keys.iter().all(|key| {
                            hook.get(*key)
                                .is_some_and(|value| other.get(*key) == Some(value))
                        })
                }),
                "the same hook has different settings"
            );
        }
        existing.push(hook.clone());
        additions.push(hook.clone());
    }
    Ok(additions)
}

const DECISIONS: &[&str] = &["allow", "ask", "deny"];

fn merge_permissions(
    existing: &mut Map<String, Value>,
    proposal: &Map<String, Value>,
) -> Result<Map<String, Value>> {
    let mut delta = Map::new();
    for (key, value) in proposal {
        if !DECISIONS.contains(&key.as_str()) && key != "additionalDirectories" {
            let mut leaf = Map::new();
            leaf.insert(key.clone(), value.clone());
            delta.extend(merge_entries(existing, &leaf)?);
            continue;
        }
        let proposed = value
            .as_array()
            .context("generated permission list must be an array")?;
        ensure!(
            proposed.iter().all(Value::is_string),
            "generated permission list must contain strings"
        );
        if proposed.is_empty() {
            continue;
        }
        if DECISIONS.contains(&key.as_str()) {
            for &other in DECISIONS.iter().filter(|action| **action != key.as_str()) {
                for current in [existing.get(other), proposal.get(other)]
                    .into_iter()
                    .flatten()
                {
                    let current = current
                        .as_array()
                        .context("permission list must be an array")?;
                    ensure!(
                        proposed.iter().all(|entry| !current.contains(entry)),
                        "the same permission has different decisions"
                    );
                }
            }
        }
        let current = existing
            .entry(key.clone())
            .or_insert_with(|| Value::Array(Vec::new()));
        let current = current
            .as_array_mut()
            .context("existing permission list must be an array")?;
        let additions = append_new(current, proposed);
        if !additions.is_empty() {
            delta.insert(key.clone(), Value::Array(additions));
        }
    }
    Ok(delta)
}

fn append_new(current: &mut Vec<Value>, proposal: &[Value]) -> Vec<Value> {
    let mut additions = Vec::new();
    for value in proposal {
        if !current.contains(value) {
            current.push(value.clone());
            additions.push(value.clone());
        }
    }
    additions
}

fn merge_entries(
    existing: &mut Map<String, Value>,
    proposal: &Map<String, Value>,
) -> Result<Map<String, Value>> {
    let mut delta = Map::new();
    for (key, proposed) in proposal {
        match existing.get_mut(key) {
            None => {
                existing.insert(key.clone(), proposed.clone());
                delta.insert(key.clone(), proposed.clone());
            }
            Some(current) if current == proposed => {}
            Some(Value::Object(current)) if proposed.is_object() => {
                let additions = merge_entries(current, proposed.as_object().expect("object"))?;
                if !additions.is_empty() {
                    delta.insert(key.clone(), Value::Object(additions));
                }
            }
            Some(_) => bail!("a proposed permission conflicts with existing settings"),
        }
    }
    Ok(delta)
}

fn merge_toml(current: Option<&[u8]>, staged: &[u8]) -> Result<Merged> {
    let current =
        std::str::from_utf8(current.unwrap_or_default()).context("existing TOML must be UTF-8")?;
    let staged = std::str::from_utf8(staged).context("generated TOML must be UTF-8")?;
    let mut patch = parse_toml(staged)?;
    validate_material(&serde_json::to_value(&patch)?)?;
    validate_codex_mcp(&patch)?;
    let base = parse_toml(current)?;
    if let Some(features) = patch.remove("features") {
        ensure!(
            features.as_table().is_some_and(toml::Table::is_empty),
            "unexpected generated tool features"
        );
    }
    ensure!(
        patch.keys().all(|key| matches!(
            key.as_str(),
            "mcp_servers"
                | "permissions"
                | "default_permissions"
                | "approval_policy"
                | "sandbox_mode"
                | "sandbox_workspace_write"
                | "apps"
                | "approvals_reviewer"
                | "auto_review"
                | "tui"
                | "tools"
        )),
        "unsupported TOML settings section from rulesync"
    );
    if patch.contains_key("default_permissions") && !base.contains_key("default_permissions") {
        ensure!(
            ["sandbox_mode", "sandbox_workspace_write"]
                .iter()
                .all(|key| !base.contains_key(*key)),
            "the generated permissions profile conflicts with the existing sandbox mode"
        );
    }
    let delta = toml_additions(&base, &patch, true)?;
    if delta.is_empty() {
        return Ok(None);
    }
    let mut next: toml_edit::DocumentMut = current
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid existing TOML settings"))?;
    let generated: toml_edit::DocumentMut = staged
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid generated TOML settings"))?;
    insert_toml_keys(next.as_table_mut(), generated.as_table(), &delta)?;
    let next = next.to_string();
    let mut expected = base;
    extend_toml(&mut expected, delta.clone());
    // A dotted key / inline table can print differently after a structural
    // edit. Verify the resulting file still carries the expected entries.
    ensure!(
        toml::to_string(&parse_toml(&next)?)? == toml::to_string(&expected)?,
        "the merged TOML settings could not preserve existing entries"
    );
    Ok(Some((next.into_bytes(), toml::to_string_pretty(&delta)?)))
}

fn validate_codex_mcp(patch: &toml::Table) -> Result<()> {
    let Some(servers) = patch.get("mcp_servers") else {
        return Ok(());
    };
    for server in servers
        .as_table()
        .context("generated MCP servers must be a table")?
        .values()
    {
        let server = server
            .as_table()
            .context("generated MCP server must be a table")?;
        ensure!(
            server
                .get("env")
                .and_then(toml::Value::as_table)
                .is_none_or(toml::Table::is_empty),
            "Codex MCP environment references must use env_vars"
        );
        ensure!(
            server
                .get("http_headers")
                .and_then(toml::Value::as_table)
                .is_none_or(toml::Table::is_empty),
            "Codex MCP authentication needs a whole environment reference or bearer_token_env_var"
        );
    }
    Ok(())
}

fn parse_toml(text: &str) -> Result<toml::Table> {
    toml::from_str(text).map_err(|error| {
        let location = error
            .span()
            .map(|span| format!(" at byte {}", span.start))
            .unwrap_or_default();
        anyhow::anyhow!("invalid TOML settings{location}")
    })
}

fn toml_additions(
    current: &toml::Table,
    proposal: &toml::Table,
    root: bool,
) -> Result<toml::Table> {
    let mut delta = toml::Table::new();
    for (key, proposed) in proposal {
        if root && key == "mcp_servers" {
            let proposed = proposed
                .as_table()
                .context("generated MCP servers must be a table")?;
            if proposed.is_empty() {
                continue;
            }
            let empty = toml::Table::new();
            let servers = match current.get(key) {
                Some(current) => current
                    .as_table()
                    .context("existing MCP servers must be a table")?,
                None => &empty,
            };
            let mut additions = toml::Table::new();
            for (name, server) in proposed {
                ensure!(server.is_table(), "generated MCP server must be a table");
                if let Some(current) = servers.get(name) {
                    ensure!(
                        toml_contains(current, server),
                        "MCP server name already has different settings"
                    );
                } else {
                    additions.insert(name.clone(), server.clone());
                }
            }
            if !additions.is_empty() {
                delta.insert(key.clone(), toml::Value::Table(additions));
            }
            continue;
        }
        match (current.get(key), proposed) {
            (None, _) => {
                delta.insert(key.clone(), proposed.clone());
            }
            (Some(toml::Value::Table(current)), toml::Value::Table(proposed)) => {
                let additions = toml_additions(current, proposed, false)?;
                if !additions.is_empty() {
                    delta.insert(key.clone(), toml::Value::Table(additions));
                }
            }
            (Some(current), _) => {
                ensure!(
                    current == proposed,
                    "a proposed permission conflicts with existing settings"
                );
            }
        }
    }
    Ok(delta)
}

fn toml_contains(current: &toml::Value, proposed: &toml::Value) -> bool {
    match (current.as_table(), proposed.as_table()) {
        (Some(current), Some(proposed)) => proposed.iter().all(|(key, value)| {
            current
                .get(key)
                .is_some_and(|entry| toml_contains(entry, value))
        }),
        _ => current == proposed,
    }
}

fn extend_toml(base: &mut toml::Table, patch: toml::Table) {
    for (key, item) in patch {
        match (base.get_mut(&key).and_then(toml::Value::as_table_mut), item) {
            (Some(base), toml::Value::Table(patch)) => extend_toml(base, patch),
            (_, item) => {
                base.insert(key, item);
            }
        }
    }
}

fn insert_toml_keys(
    current: &mut dyn toml_edit::TableLike,
    generated: &dyn toml_edit::TableLike,
    keys: &toml::Table,
) -> Result<()> {
    for (key, desired) in keys {
        let generated = generated
            .get(key)
            .context("generated TOML key is missing")?;
        if let Some(existing) = current.get_mut(key) {
            insert_toml_keys(
                existing
                    .as_table_like_mut()
                    .context("existing value must be a table")?,
                generated
                    .as_table_like()
                    .context("generated value must be a table")?,
                desired.as_table().context("merged value must be a table")?,
            )?;
        } else {
            current.insert(key, generated.clone());
        }
    }
    Ok(())
}

fn merge_command_rules(current: Option<&[u8]>, staged: &[u8]) -> Result<Merged> {
    let proposed = std::str::from_utf8(staged).context("generated rules must be UTF-8")?;
    validate_text(proposed)?;
    if current == Some(staged)
        || proposed
            .lines()
            .all(|line| line.trim().is_empty() || line.trim_start().starts_with('#'))
    {
        return Ok(None);
    }
    ensure!(
        current.is_none(),
        "the existing command rules conflict with the generated rules"
    );
    Ok(Some((staged.to_vec(), proposed.to_owned())))
}

fn without_jsonc(mut bytes: &[u8]) -> Result<Vec<u8>> {
    bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    let mut output = Vec::with_capacity(bytes.len());
    let mut in_string = false;
    let mut escaped = false;
    while let Some((&byte, rest)) = bytes.split_first() {
        bytes = rest;
        if in_string {
            output.push(byte);
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        if byte == b'/' {
            if bytes.starts_with(b"/") {
                let end = bytes
                    .iter()
                    .position(|&byte| matches!(byte, b'\r' | b'\n'))
                    .unwrap_or(bytes.len());
                bytes = &bytes[end..];
                output.push(b' ');
                continue;
            }
            if bytes.starts_with(b"*") {
                let end = bytes[1..]
                    .windows(2)
                    .position(|pair| pair == b"*/")
                    .context("unterminated settings comment")?;
                bytes = &bytes[end + 3..];
                output.push(b' ');
                continue;
            }
        }
        if matches!(byte, b'}' | b']') {
            let mut significant = output
                .iter()
                .enumerate()
                .rev()
                .filter(|(_, byte)| !byte.is_ascii_whitespace());
            if let Some((comma, b',')) = significant.next() {
                if significant
                    .next()
                    .is_some_and(|(_, byte)| !matches!(byte, b'{' | b'['))
                {
                    output.truncate(comma);
                }
            }
        }
        in_string = byte == b'"';
        output.push(byte);
    }
    Ok(output)
}

/// serde_json normally keeps the last definition of a duplicate object key.
/// Reject such files so a settings write cannot silently discard an entry.
struct UniqueJson(Value);

impl<'de> Deserialize<'de> for UniqueJson {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Unique;
        impl<'de> Visitor<'de> for Unique {
            type Value = UniqueJson;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("JSON with unique settings keys")
            }

            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<UniqueJson, M::Error> {
                let mut result = Map::new();
                while let Some((key, UniqueJson(value))) = map.next_entry::<String, UniqueJson>()? {
                    if result.insert(key, value).is_some() {
                        return Err(serde::de::Error::custom("duplicate settings key"));
                    }
                }
                Ok(UniqueJson(Value::Object(result)))
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<UniqueJson, A::Error> {
                let mut result = Vec::new();
                while let Some(UniqueJson(value)) = seq.next_element()? {
                    result.push(value);
                }
                Ok(UniqueJson(Value::Array(result)))
            }

            fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<UniqueJson, E> {
                Ok(UniqueJson(Value::from(value)))
            }

            fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<UniqueJson, E> {
                Ok(UniqueJson(Value::from(value)))
            }

            fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<UniqueJson, E> {
                Ok(UniqueJson(Value::from(value)))
            }

            fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<UniqueJson, E> {
                let number = serde_json::Number::from_f64(value)
                    .ok_or_else(|| E::custom("invalid settings number"))?;
                Ok(UniqueJson(Value::Number(number)))
            }

            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<UniqueJson, E> {
                Ok(UniqueJson(Value::from(value)))
            }

            fn visit_unit<E: serde::de::Error>(self) -> Result<UniqueJson, E> {
                Ok(UniqueJson(Value::Null))
            }
        }

        deserializer.deserialize_any(Unique)
    }
}

fn parse_json(bytes: &[u8]) -> Result<Map<String, Value>> {
    let UniqueJson(value) = serde_json::from_slice(bytes).map_err(|error| {
        anyhow::anyhow!(
            "invalid or duplicate JSON settings at line {}, column {}",
            error.line(),
            error.column()
        )
    })?;
    match value {
        Value::Object(map) => Ok(map),
        _ => bail!("JSON settings must be an object"),
    }
}

fn is_env_reference(value: &str) -> bool {
    let Some(name) = value.strip_prefix('$') else {
        return false;
    };
    is_env_name(
        name.strip_prefix('{')
            .and_then(|name| name.strip_suffix('}'))
            .unwrap_or(name),
    )
}

fn is_env_name(name: &str) -> bool {
    let mut chars = name.bytes();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

fn is_reference(value: &str) -> bool {
    is_env_reference(value)
        || value.split_once(' ').is_some_and(|(scheme, reference)| {
            (scheme.eq_ignore_ascii_case("bearer") || scheme.eq_ignore_ascii_case("basic"))
                && is_env_reference(reference)
        })
}

fn credential_field(value: &str) -> bool {
    let key = value.to_ascii_lowercase().replace(['-', '_'], "");
    if key.ends_with("envvar") || key.ends_with("envvars") {
        return false;
    }
    [
        "apikey",
        "password",
        "passphrase",
        "secret",
        "privatekey",
        "accesskey",
        "token",
        "authorization",
        "cookie",
    ]
    .iter()
    .any(|suffix| key.ends_with(suffix))
}

fn validate_text(text: &str) -> Result<()> {
    static AUTH_VALUE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
        r#"(?i)(?:\b(?:bearer|basic)\s+|(?:--|\b[\w-]*[_-])(?:api[_-]?key|token|password|secret)\s*(?:[:=]\s*|\s+))["']?([^\s"',;&]+)"#
    ).expect("credential argument pattern")
    });
    static URL_CREDENTIAL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
        r"(?i)\b(?:https?|wss?|socks[45]?)://[^/\s?#]*@|[?&](?:key|api[_-]?key|access[_-]?token|token|secret|password)=[^$\s&#]+"
    ).expect("URL credential pattern")
    });

    ensure!(
        !text
            .chars()
            .any(|ch| is_invisible(ch) || (ch.is_control() && !matches!(ch, '\n' | '\r' | '\t'))),
        "invisible characters in settings"
    );
    reject_secret_lines(text).map_err(|_| anyhow::anyhow!("literal credential in settings"))?;
    ensure!(
        !URL_CREDENTIAL.is_match(text)
            && AUTH_VALUE
                .captures_iter(text)
                .all(|capture| is_env_reference(&capture[1])),
        "literal credential in settings"
    );
    Ok(())
}

fn validate_material(value: &Value) -> Result<()> {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                validate_text(key)?;
                let name = key.to_ascii_lowercase();
                if name == "bearer_token_env_var" {
                    ensure!(
                        value.as_str().is_some_and(is_env_name),
                        "bearer_token_env_var needs an environment variable name"
                    );
                }
                if name == "env_http_headers" {
                    let entries = value
                        .as_object()
                        .context("expected an environment header map")?;
                    for (name, value) in entries {
                        validate_text(name)?;
                        ensure!(
                            value.as_str().is_some_and(is_env_name),
                            "an environment header needs an environment variable name"
                        );
                        validate_material(value)?;
                    }
                    continue;
                }
                if matches!(
                    name.as_str(),
                    "env" | "headers" | "http_headers" | "httpheaders"
                ) {
                    let entries = value
                        .as_object()
                        .context("expected a settings environment or header map")?;
                    ensure!(
                        entries.values().all(|entry| entry.as_str().is_some_and(is_reference)),
                        "settings environment and header values must use references to local environment variables"
                    );
                } else if credential_field(key) && (value.is_string() || value.is_number()) {
                    ensure!(
                        value.as_str().is_some_and(is_reference),
                        "literal credential in settings"
                    );
                }
                validate_material(value)?;
            }
        }
        Value::Array(values) => {
            for pair in values.windows(2) {
                if pair[0]
                    .as_str()
                    .is_some_and(|name| name.starts_with("--") && credential_field(name))
                {
                    ensure!(
                        pair[1].as_str().is_some_and(is_env_reference),
                        "literal credential in settings"
                    );
                }
            }
            for value in values {
                validate_material(value)?;
            }
        }
        Value::String(text) => validate_text(text)?,
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rulesync_settings_merges_servers_and_only_previews_staged_additions() {
        let path = Path::new(".claude.json");
        let existing = br#"{
          "oauthAccount": {"unknownCredential": "fixture-live-value"},
          "mcpServers": {
            "manual": {"command": "manual-command", "env": {"SERVICE_KEY":"fixture-local-key"}},
            "same": {"command": "checked", "localApproval":true}
          }
        }"#;
        let generated =
            br#"{"mcpServers":{"same":{"command":"checked"},"new":{"command":"from-rulesync"}}}"#;
        let change = merge_file(path, Some(existing), generated)
            .unwrap()
            .unwrap();
        let merged: Value = serde_json::from_slice(&change.bytes).unwrap();
        assert_eq!(
            merged["oauthAccount"]["unknownCredential"],
            "fixture-live-value"
        );
        assert_eq!(
            merged["mcpServers"]["manual"]["env"]["SERVICE_KEY"],
            "fixture-local-key"
        );
        assert_eq!(merged["mcpServers"]["same"]["localApproval"], true);
        assert_eq!(merged["mcpServers"]["new"]["command"], "from-rulesync");
        assert!(change.preview.diff.contains("from-rulesync"));
        assert!(!change.preview.diff.contains("fixture-live-value"));
        assert!(!change.preview.diff.contains("manual-command"));
        assert!(!change.preview.diff.contains("checked"));
        assert!(merge_file(path, Some(&change.bytes), generated)
            .unwrap()
            .is_none());
        assert!(merge_file(
            path,
            Some(existing),
            br#"{"mcpServers":{"same":{"command":"changed"}}}"#,
        )
        .is_err());
    }

    #[test]
    fn rulesync_settings_merges_nested_hooks_and_permissions() {
        let path = Path::new(".claude/settings.json");
        let existing = br#"{
          "model":"untouched",
          "hooks": {"PreToolUse":[
            {"matcher":"Bash","customField":true,"hooks":[{"type":"command","command":"check-existing"}]},
            {"matcher":"Read","hooks":[{"type":"command","command":"record"}]}
          ]},
          "permissions": {"allow":["Bash(git status)"],"deny":["Read(.env)"],"defaultMode":"default"},
          "sandbox": {"network":{"allowLocalBinding":false}}
        }"#;
        let generated = br#"{
          "hooks": {"PreToolUse":[{"matcher":"Bash","hooks":[
            {"type":"command","command":"check-existing"},
            {"type":"command","command":"check-added"}
          ]}]},
          "permissions": {"allow":["Bash(git diff)"],"deny":[]},
          "sandbox": {"network":{"allowUnixSockets":["/socket"]}}
        }"#;
        let change = merge_file(path, Some(existing), generated)
            .unwrap()
            .unwrap();
        let merged: Value = serde_json::from_slice(&change.bytes).unwrap();
        assert_eq!(merged["model"], "untouched");
        assert_eq!(
            merged["permissions"]["allow"],
            serde_json::json!(["Bash(git status)", "Bash(git diff)"])
        );
        assert_eq!(
            merged["permissions"]["deny"],
            serde_json::json!(["Read(.env)"])
        );
        assert_eq!(merged["permissions"]["defaultMode"], "default");
        assert_eq!(merged["sandbox"]["network"]["allowLocalBinding"], false);
        assert_eq!(
            merged["sandbox"]["network"]["allowUnixSockets"],
            serde_json::json!(["/socket"])
        );
        let hooks = &merged["hooks"]["PreToolUse"];
        assert_eq!(hooks.as_array().unwrap().len(), 2);
        assert_eq!(hooks[0]["customField"], true);
        assert_eq!(hooks[0]["hooks"][0]["command"], "check-existing");
        assert_eq!(hooks[0]["hooks"][1]["command"], "check-added");
        assert_eq!(hooks[1]["hooks"][0]["command"], "record");
        assert!(!change.preview.diff.contains("check-existing"));
        assert!(merge_file(path, Some(&change.bytes), generated)
            .unwrap()
            .is_none());
    }

    #[test]
    fn rulesync_settings_rejects_ambiguous_conflicts() {
        let path = Path::new("settings.json");
        let existing = br#"{
          "permissions":{"allow":["Bash(build)"]},
          "hooks":{"Stop":[{"hooks":[{"type":"command","command":"check","timeout":10}]}]}
        }"#;
        for update in [
            br#"{"permissions":{"deny":["Bash(build)"]}}"#.as_slice(),
            br#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"check","timeout":20}]}]}}"#.as_slice(),
        ] {
            assert!(merge_file(path, Some(existing), update).is_err());
        }
        let duplicate = br#"{"settings":{"unowned":true,"unowned":false}}"#;
        assert!(merge_file(
            path,
            Some(duplicate),
            br#"{"permissions":{"deny":["Bash(build)"]}}"#
        )
        .is_err());
        assert!(merge_file(
            Path::new("config.toml"),
            Some(b"sandbox_mode = 'workspace-write'"),
            b"default_permissions = 'rulesync'"
        )
        .is_err());
        assert!(merge_file(
            Path::new("rulesync.rules"),
            Some(b"# existing rules"),
            b"prefix_rule(pattern = [\"ls\"], decision = \"allow\")"
        )
        .is_err());
    }

    #[test]
    fn rulesync_settings_preserves_toml_comments_inline_tables_and_unowned_settings() {
        let path = Path::new(".codex/config.toml");
        let existing = b"# keep this\nmodel = 'local' # keep this too\n\n[mcp_servers.manual]\ncommand='keep'\n\n[permissions.rulesync.filesystem]\n\"/keep\" = 'read'\n";
        let generated = b"default_permissions = 'rulesync'\n\n[mcp_servers.new]\ncommand='from-rulesync'\nenv = {}\n\n[permissions.rulesync.filesystem]\n\"/additional\" = 'read'\n";
        let change = merge_file(path, Some(existing), generated)
            .unwrap()
            .unwrap();
        let text = std::str::from_utf8(&change.bytes).unwrap();
        assert!(text.contains("model = 'local' # keep this too"));
        assert!(text.contains("# keep this"));
        assert!(text.contains("env = {}"));
        let parsed = parse_toml(text).unwrap();
        assert_eq!(
            parsed["mcp_servers"]["manual"]["command"].as_str(),
            Some("keep")
        );
        assert_eq!(
            parsed["mcp_servers"]["new"]["command"].as_str(),
            Some("from-rulesync")
        );
        assert_eq!(
            parsed["permissions"]["rulesync"]["filesystem"]["/keep"].as_str(),
            Some("read")
        );
        assert_eq!(
            parsed["permissions"]["rulesync"]["filesystem"]["/additional"].as_str(),
            Some("read")
        );
        assert!(!change.preview.diff.contains("/keep"));
        assert!(merge_file(path, Some(&change.bytes), generated)
            .unwrap()
            .is_none());
    }

    #[test]
    fn rulesync_settings_reads_only_recognized_jsonc_sources() {
        let temp = tempfile::tempdir().unwrap();
        let temp = temp.path().canonicalize().unwrap();
        let source = temp.join(".rulesync");
        fs::create_dir(&source).unwrap();
        fs::write(temp.join("rulesync.jsonc"), r#"{"delete":true}"#).unwrap();
        fs::write(source.join("unrelated.txt"), "must not be staged").unwrap();
        fs::write(source.join("mcp.json"), "deprecated source ignored").unwrap();
        fs::write(
            source.join("mcp.jsonc"),
            r#"{
            // URLs and slash characters inside strings survive normalization.
            "mcpServers":{"new":{"type":"http","url":"http://localhost/api/*",},},
        }"#,
        )
        .unwrap();
        let actual = read_sources(&temp).unwrap();
        assert_eq!(actual.len(), 1);
        assert_eq!(
            serde_json::from_slice::<Value>(&actual["mcp"]).unwrap()["mcpServers"]["new"]["url"],
            "http://localhost/api/*"
        );
        assert_eq!(actual, read_sources(&source).unwrap());
        assert!(parse_json(&without_jsonc(br#"{"a":{,}}"#).unwrap()).is_err());
    }

    #[test]
    fn rulesync_settings_refuses_literals_in_secret_carriers() {
        for json in [
            serde_json::json!({"mcpServers":{"mail":{"env":{"MAIL_KEY":"fixture-literal"}}}}),
            serde_json::json!({"mcpServers":{"mail":{"url":"https://name:fixture-literal@example.invalid/mcp"}}}),
            serde_json::json!({"mcpServers":{"mail":{"headers":{"Authorization":"Bearer fixture-literal"}}}}),
            serde_json::json!({"mcpServers":{"mail":{"args":["--token","fixture-literal"]}}}),
            serde_json::json!({"hooks":{"stop":[{"command":"curl --password=fixture-literal"}]}}),
        ] {
            let error = validate_material(&json).unwrap_err();
            assert!(!format!("{error:#}").contains("fixture-literal"));
        }
        assert!(validate_material(&serde_json::json!({
            "mcpServers":{"mail":{"headers":{"Authorization":"Bearer ${SERVICE_TOKEN}"},"env":{"OTHER_KEY":"$OTHER_KEY"}}}
        })).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn rulesync_settings_refuses_symbolic_source_paths_and_files() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let temp = temp.path().canonicalize().unwrap();
        let source = temp.join("source");
        let nested = source.join("nested");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("mcp.jsonc"), "{}").unwrap();
        let symlink_parent = temp.join("alias");
        symlink(&source, &symlink_parent).unwrap();
        assert!(read_sources(&symlink_parent.join("nested")).is_err());
        symlink(nested.join("mcp.jsonc"), source.join("mcp.jsonc")).unwrap();
        assert!(read_sources(&source).is_err());
    }
}
