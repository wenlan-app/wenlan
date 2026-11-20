// SPDX-License-Identifier: Apache-2.0
//! Prepare native skill metadata for rulesync and check what it preserved.
//! Only the legacy, flag-off offline preview projects formatting.

use anyhow::{bail, ensure, Context, Result};
use serde_yaml::{Mapping, Value};

use crate::{
    rulesync_personal::personal_target,
    skill_registry::without_leading_bom,
    store::format::{normalize_line_ends, split_frontmatter},
};

const CLAUDE_FIELDS: &[&str] = &[
    "when_to_use",
    "allowed-tools",
    "disallowed-tools",
    "model",
    "effort",
    "argument-hint",
    "context",
    "agent",
    "shell",
    "background",
    "arguments",
    "hooks",
    "disable-model-invocation",
    "user-invocable",
    "paths",
    "license",
    "compatibility",
    "metadata",
];
const CODEX_FIELDS: &[&str] = &["disable-model-invocation", "metadata"];
const ANTIGRAVITY_FIELDS: &[&str] = &[
    "license",
    "compatibility",
    "disable-slash-command",
    "disable-model-invocation",
    "metadata",
];
const COPILOT_FIELDS: &[&str] = &["license", "user-invocable", "disable-model-invocation"];
const CURSOR_FIELDS: &[&str] = &[
    "paths",
    "disable-model-invocation",
    "user-invocable",
    "icon",
    "color",
    "metadata",
];
const OPENCODE_FIELDS: &[&str] = &[
    "license",
    "compatibility",
    "metadata",
    "allowed-tools",
    "disable-model-invocation",
];
const MUSE_FIELDS: &[&str] = &["license", "compatibility", "metadata"];

/// The pinned writer places some section fields before the common fields.
fn field_order(renderer: &str) -> Result<(&'static [&'static str], &'static [&'static str])> {
    Ok(match renderer {
        "claudecode" => (&[], CLAUDE_FIELDS),
        "codexcli" => (&[], CODEX_FIELDS),
        "antigravity-ide" => (ANTIGRAVITY_FIELDS, &[]),
        "copilotcli" => (&["allowed-tools", "argument-hint"], COPILOT_FIELDS),
        "cursor" => (&[], CURSOR_FIELDS),
        "opencode" => (&[], OPENCODE_FIELDS),
        "musecode" => (&[], MUSE_FIELDS),
        _ => bail!("there is no read-only skill preview for {renderer}"),
    })
}

struct Document {
    native: Mapping,
    body: String,
}

impl Document {
    fn from_installed(renderer: &str, bytes: &[u8]) -> Result<Self> {
        let text = without_leading_bom(std::str::from_utf8(bytes)?);
        let text = normalize_line_ends(text);
        let split = split_frontmatter(&text)?.context("SKILL.md needs YAML frontmatter")?;
        let mut raw = split.frontmatter;
        if renderer == "cursor" {
            if let Some(globs) = raw.remove("globs") {
                raw.entry("paths".into()).or_insert(globs);
            }
        }
        let (leading, trailing) = field_order(renderer)?;
        let mut native = Mapping::new();
        copy_fields(&raw, &mut native, leading);
        for key in ["name", "description"] {
            let value = raw
                .get(key)
                .and_then(Value::as_str)
                .with_context(|| format!("SKILL.md needs a text `{key}` in its frontmatter"))?;
            native.insert(key.into(), value.into());
        }
        ensure!(
            ["targets", "claudecode", "codexcli", renderer]
                .iter()
                .all(|key| !raw.contains_key(*key)),
            "SKILL.md contains rulesync-specific options; adopt a tool-installed skill instead"
        );
        copy_fields(&raw, &mut native, trailing);
        if !matches!(renderer, "claudecode" | "codexcli") {
            for (key, value) in &raw {
                ensure!(
                    value.is_null() || native.contains_key(key),
                    "the {renderer} skill renderer does not preserve frontmatter field `{}`",
                    key.as_str().unwrap_or("(non-text key)")
                );
            }
        }
        validate_fields(renderer, &mut native)?;
        let body = split.body.trim().to_owned();
        Ok(Self { native, body })
    }
}

fn copy_fields(raw: &Mapping, native: &mut Mapping, keys: &[&str]) {
    for key in keys {
        if let Some(value) = raw.get(*key).filter(|value| !value.is_null()) {
            native.insert((*key).into(), value.clone());
        }
    }
}

fn validate_fields(target: &str, fields: &mut Mapping) -> Result<()> {
    for key in [
        "disable-model-invocation",
        "disable-slash-command",
        "user-invocable",
        "background",
    ] {
        ensure!(
            fields.get(key).is_none_or(Value::is_bool),
            "`{key}` must be true or false"
        );
    }
    if target == "claudecode" {
        for key in CLAUDE_FIELDS.iter().take_while(|&&key| key != "background") {
            let Some(value) = fields.get(*key) else {
                continue;
            };
            if matches!(*key, "allowed-tools" | "disallowed-tools") && value.is_sequence() {
                ensure!(
                    value.as_sequence().unwrap().iter().all(Value::is_string),
                    "`{key}` needs text entries"
                );
                continue;
            }
            let value = value
                .as_str()
                .with_context(|| format!("`{key}` needs a text value"))?;
            if value.is_empty() {
                fields.remove(*key);
            }
        }
    }
    if let Some(metadata) = fields.get("metadata") {
        let metadata = metadata.as_mapping().context("`metadata` must be a map")?;
        if matches!(target, "codexcli" | "musecode") {
            ensure!(
                metadata.values().all(Value::is_string),
                "skill metadata needs text values"
            );
            if metadata.is_empty() && target == "codexcli" {
                fields.remove("metadata");
            }
        }
    }
    ensure!(
        target != "codexcli" || fields.get("disable-model-invocation").and_then(Value::as_bool) != Some(true),
        "this Codex skill needs invocation settings generated in agents/openai.yaml; review those settings manually before adopting"
    );
    Ok(())
}

pub(super) fn preview(target: &str, bytes: &[u8]) -> Result<Vec<u8>> {
    let renderer = personal_target(target)
        .context("unknown personal skill target")?
        .skills
        .renderer;
    let document = Document::from_installed(renderer, bytes)?;
    let mut yaml = String::new();
    emit_map(&mut yaml, &document.native, 0)?;
    Ok(skill_file(yaml, &document.body).into_bytes())
}

/// Stage installed metadata in the pinned renderer's native target section.
/// Codex's supported fields are read from the source root.
pub(super) fn stage(target: &str, bytes: &[u8]) -> Result<Vec<u8>> {
    let renderer = personal_target(target)
        .context("unknown personal skill target")?
        .skills
        .renderer;
    let mut document = Document::from_installed(renderer, bytes)?;
    if renderer != "codexcli" {
        let (leading, trailing) = field_order(renderer)?;
        let section: Mapping = leading
            .iter()
            .chain(trailing)
            .filter_map(|key| {
                document
                    .native
                    .remove(*key)
                    .map(|value| ((*key).into(), value))
            })
            .collect();
        if !section.is_empty() {
            document
                .native
                .insert(renderer.into(), Value::Mapping(section));
        }
    }
    let yaml = serde_yaml::to_string(&document.native).context("cannot stage skill frontmatter")?;
    Ok(skill_file(yaml, &document.body).into_bytes())
}

pub(super) fn check_frontmatter(source: &[u8], rendered: &[u8]) -> Result<()> {
    let read = |bytes: &[u8]| -> Result<Mapping> {
        let text = without_leading_bom(std::str::from_utf8(bytes)?);
        let normalized = normalize_line_ends(text);
        Ok(split_frontmatter(&normalized)?
            .context("SKILL.md needs YAML frontmatter")?
            .frontmatter)
    };
    let original = read(source).context("cannot read the source skill frontmatter")?;
    let output = read(rendered).context("cannot read the rendered skill frontmatter")?;
    for (field, value) in &original {
        ensure!(
            output.get(field) == Some(value),
            "rulesync did not preserve frontmatter field `{}`",
            field.as_str().unwrap_or("(non-text key)")
        );
    }
    for field in output.keys() {
        ensure!(
            original.contains_key(field),
            "rulesync added frontmatter field `{}`",
            field.as_str().unwrap_or("(non-text key)")
        );
    }
    Ok(())
}

fn skill_file(yaml: String, body: &str) -> String {
    let mut file = format!("---\n{yaml}---\n{body}");
    if !file.ends_with('\n') {
        file.push('\n');
    }
    file
}

fn emit_map(out: &mut String, map: &Mapping, indent: usize) -> Result<()> {
    for (key, value) in map {
        if value.is_null() {
            continue;
        }
        let key = key
            .as_str()
            .context("skill metadata keys need to be text")?;
        out.push_str(&" ".repeat(indent));
        out.push_str(&scalar(key, indent, true)?);
        out.push(':');
        emit_value(out, value, indent + 2)?;
    }
    Ok(())
}

fn emit_value(out: &mut String, value: &Value, indent: usize) -> Result<()> {
    match value {
        Value::Mapping(map) if map.values().any(|value| !value.is_null()) => {
            out.push('\n');
            emit_map(out, map, indent)?;
        }
        Value::Sequence(seq) if seq.iter().any(|value| !value.is_null()) => {
            out.push('\n');
            for value in seq.iter().filter(|value| !value.is_null()) {
                out.push_str(&" ".repeat(indent));
                out.push('-');
                emit_value(out, value, indent + 2)?;
            }
        }
        _ => {
            out.push(' ');
            match value {
                Value::String(text) => out.push_str(&scalar(text, indent, false)?),
                Value::Mapping(_) => out.push_str("{}"),
                Value::Sequence(_) => out.push_str("[]"),
                Value::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
                Value::Number(value) => out.push_str(&value.to_string()),
                _ => bail!("cannot preview this YAML value"),
            }
            out.push('\n');
        }
    }
    Ok(())
}

fn scalar(text: &str, indent: usize, key: bool) -> Result<String> {
    ensure!(
        !text.chars().any(|ch| ch != '\n' && ch.is_control()),
        "skill YAML contains a control character"
    );
    let width = 80usize.saturating_sub(indent).max(40);
    let fold = !key
        && text
            .lines()
            .any(|line| !line.starts_with(' ') && line.encode_utf16().count() > width);
    if fold || text.contains('\n') {
        ensure!(!key, "skill YAML keys must be on one line");
        return Ok(block_scalar(text, indent, width, fold));
    }
    if plain_scalar(text) {
        Ok(text.to_owned())
    } else {
        Ok(format!("'{}'", text.replace('\'', "''")))
    }
}

fn plain_scalar(text: &str) -> bool {
    !text.is_empty()
        && !text.starts_with(['-', '?', ':', ',', '[', ']', '{', '}', '#', '&', '*', '!', '|', '=', '>', '\'', '"', '%', '@', '`'])
        && text.trim() == text
        && !text.ends_with(':')
        && !matches!(text.to_ascii_lowercase().as_str(), "y" | "n" | "yes" | "no" | "on" | "off")
        // js-yaml resolves dates to timestamps by default, while serde_yaml
        // leaves a bare date as a string.
        && !text.as_bytes().get(..5).is_some_and(|head| head[4] == b'-' && head[..4].iter().all(u8::is_ascii_digit) && text.as_bytes().get(7) == Some(&b'-'))
        && serde_yaml::from_str::<Value>(text).is_ok_and(|value| value.as_str() == Some(text))
}

fn block_scalar(text: &str, indent: usize, width: usize, fold: bool) -> String {
    let style = if fold { '>' } else { '|' };
    let indicator = if text.trim_start_matches('\n').starts_with(' ') {
        "2"
    } else {
        ""
    };
    let chomp = if text == "\n" || text.ends_with("\n\n") {
        "+"
    } else if text.ends_with('\n') {
        ""
    } else {
        "-"
    };
    let mut out = format!("{style}{indicator}{chomp}");
    let padding = " ".repeat(indent);
    let mut previous_indented = true;
    for line in text.strip_suffix('\n').unwrap_or(text).split('\n') {
        let indented = line.starts_with(' ');
        if fold && !line.is_empty() && !indented && !previous_indented {
            out.push('\n');
        }
        out.push('\n');
        if !line.is_empty() {
            out.push_str(&padding);
            if fold && !indented {
                fold_line(&mut out, line, &padding, width);
            } else {
                out.push_str(line);
            }
            previous_indented = indented;
        }
    }
    out
}

fn fold_line(out: &mut String, line: &str, padding: &str, width: usize) {
    let mut start = (0, 0);
    let mut last_break = start;
    let mut columns = 0;
    let mut break_at = |from, (index, column)| {
        out.push_str(&line[from..index]);
        out.push('\n');
        out.push_str(padding);
        (index + 1, column + 1)
    };
    // Keep each possible break's byte offset and UTF-16 column. A word
    // wider than the normal line stays intact.
    for (index, ch) in line.char_indices() {
        if ch == ' '
            && line
                .as_bytes()
                .get(index + 1)
                .is_some_and(|byte| *byte != b' ')
        {
            if columns - start.1 > width {
                let at = if last_break.0 > start.0 {
                    last_break
                } else {
                    (index, columns)
                };
                start = break_at(start.0, at);
            }
            last_break = (index, columns);
        }
        columns += ch.len_utf16();
    }
    if columns - start.1 > width && last_break.0 > start.0 {
        start = break_at(start.0, last_break);
    }
    out.push_str(&line[start.0..]);
}
