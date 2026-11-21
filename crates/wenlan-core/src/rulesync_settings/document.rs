// SPDX-License-Identifier: Apache-2.0
//! In-memory edits to a single native settings entry. The setup writer owns disk I/O.

use std::{ops::Range, sync::LazyLock};

use anyhow::{bail, ensure, Context, Result};
use regex::Regex;
use serde_json::{Map, Value};
use toml_edit::{DocumentMut, ImDocument, Item};

use super::parse_jsonc_value;
use crate::rulesync_personal::line_changes;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Part<'a> {
    Key(&'a str),
    Index(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Format {
    Json,
    Toml,
    CommandRules,
}

pub(super) fn read(format: Format, bytes: &[u8]) -> Result<Value> {
    let value = match format {
        Format::Json => parse_jsonc_value(bytes)?,
        Format::Toml => {
            let text = std::str::from_utf8(bytes).context("settings are not UTF-8")?;
            let doc = text
                .parse::<toml::Table>()
                .map_err(|_| anyhow::anyhow!("invalid TOML settings"))?;
            serde_json::to_value(doc)?
        }
        Format::CommandRules => Value::Object(Map::from_iter([(
            "rules".into(),
            Value::Array(
                command_rules(bytes)?
                    .into_iter()
                    .map(|(_, value)| value)
                    .collect(),
            ),
        )])),
    };
    ensure!(value.is_object(), "settings must be an object");
    Ok(value)
}

pub(super) fn edit(
    format: Format,
    bytes: &[u8],
    path: &[Part<'_>],
    value: Value,
) -> Result<Vec<u8>> {
    match format {
        Format::Json => {
            let tree = json_tree(bytes)?;
            let mut next = bytes.to_vec();
            tree.edit(&mut next, path, value, 0)?;
            Ok(next)
        }
        Format::Toml => {
            let text = std::str::from_utf8(bytes)?;
            let mut doc = text
                .parse::<DocumentMut>()
                .map_err(|_| anyhow::anyhow!("invalid TOML settings"))?;
            let native = match toml::Value::try_from(value) {
                Ok(value) => native_value(value),
                Err(_) => bail!("the settings entry cannot be expressed in TOML"),
            };
            edit_toml(doc.as_item_mut(), path, native)?;
            Ok(keep_toml_line_endings(text, doc.to_string())?.into_bytes())
        }
        Format::CommandRules => {
            let [Part::Key("rules"), Part::Index(index)] = path else {
                bail!("invalid command rule location");
            };
            let rules = command_rules(bytes)?;
            let map = value.as_object().context("invalid command rule")?;
            let fields = map
                .iter()
                .map(|(name, value)| format!("{name} = {value}"))
                .collect::<Vec<_>>()
                .join(", ");
            let line = format!("prefix_rule({fields})");
            let mut next = bytes.to_vec();
            if let Some((span, _)) = rules.get(*index) {
                next.splice(span.clone(), line.bytes());
            } else {
                ensure!(*index == rules.len(), "invalid command rule position");
                if !next.is_empty() && !next.ends_with(b"\n") {
                    next.push(b'\n');
                }
                next.extend_from_slice(line.as_bytes());
                next.push(b'\n');
            }
            Ok(next)
        }
    }
}

// toml_edit's encoder normalizes CRLF even in untouched decoration.
// Apply its changes to the original lines and keep their individual endings.
fn keep_toml_line_endings(original: &str, rendered: String) -> Result<String> {
    if !original.contains("\r\n") {
        return Ok(rendered);
    }
    let normalized = original.replace("\r\n", "\n");
    let lines: Vec<_> = original.split_inclusive('\n').collect();
    let mut next = String::new();
    let mut at = 0;
    for change in line_changes(&normalized, &rendered)? {
        next.extend(lines[at..change.start].iter().copied());
        let old_end = change.start + change.removed.len();
        let boundary = lines
            .get(change.start)
            .or_else(|| lines.last())
            .copied()
            .unwrap_or_default();
        for (i, line) in change.added.iter().enumerate() {
            let original_line = lines[change.start..old_end]
                .get(i)
                .copied()
                .unwrap_or(boundary);
            if let Some(content) = line
                .strip_suffix('\n')
                .filter(|_| original_line.ends_with("\r\n"))
            {
                next.push_str(content);
                next.push_str("\r\n");
            } else {
                next.push_str(line);
            }
        }
        at = old_end;
    }
    next.extend(lines[at..].iter().copied());
    Ok(next)
}

pub(super) fn entry_span(format: Format, bytes: &[u8], path: &[Part<'_>]) -> Result<Range<usize>> {
    let span = match format {
        Format::Json => json_tree(bytes)?
            .node_at(path)
            .map(|node| node.span.clone()),
        Format::Toml => {
            let doc = ImDocument::parse(std::str::from_utf8(bytes)?)
                .map_err(|_| anyhow::anyhow!("invalid TOML settings"))?;
            toml_span(doc.as_item(), path)
        }
        Format::CommandRules => None,
    };
    span.context("the native setting has no contiguous editable span")
}

fn toml_span(item: &Item, path: &[Part<'_>]) -> Option<Range<usize>> {
    // Child tables may be declared later with unrelated tables in between.
    // Require a contiguous table before treating it as an owned fragment.
    let table_span = |table: &toml_edit::Table| {
        table
            .iter()
            .all(|(_, item)| item.is_value())
            .then(|| table.span())
            .flatten()
    };
    match path {
        [] if item.is_table() => table_span(item.as_table()?),
        [] => item.as_value()?.span(),
        [Part::Key(key), rest @ ..] => toml_span(item.as_table_like()?.get(key)?, rest),
        [Part::Index(index)] if item.is_array_of_tables() => {
            table_span(item.as_array_of_tables()?.get(*index)?)
        }
        [Part::Index(index)] => item.as_array()?.get(*index)?.span(),
        _ => None,
    }
}

fn json_tree(bytes: &[u8]) -> Result<Node> {
    let at = if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        3
    } else {
        0
    };
    Scanner { bytes, at }.node()
}

pub(super) fn object_order(bytes: &[u8], path: &[&str]) -> Result<Vec<Vec<String>>> {
    let tree = json_tree(bytes)?;
    let node = path.iter().try_fold(&tree, |parent, key| {
        let Items::Object(items) = &parent.items else {
            bail!("expected an ordered settings object");
        };
        items
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, child)| child)
            .context("missing ordered settings object")
    })?;
    let mut order = Vec::new();
    node.object_order(&mut Vec::new(), &mut order);
    Ok(order)
}

fn native_value(value: toml::Value) -> toml_edit::Value {
    use toml::Value::*;
    match value {
        String(value) => value.into(),
        Integer(value) => value.into(),
        Float(value) => value.into(),
        Boolean(value) => value.into(),
        Datetime(value) => value.into(),
        Array(values) => toml_edit::Value::Array(values.into_iter().map(native_value).collect()),
        Table(values) => toml_edit::Value::InlineTable(
            values
                .into_iter()
                .map(|(key, value)| (key, native_value(value)))
                .collect(),
        ),
    }
}

fn edit_toml(item: &mut Item, path: &[Part<'_>], value: toml_edit::Value) -> Result<()> {
    let Some((first, rest)) = path.split_first() else {
        let old = item.as_value_mut().context("expected a TOML value")?;
        let mut value = value;
        *value.decor_mut() = old.decor().clone();
        *old = value;
        return Ok(());
    };
    match first {
        Part::Key(key) => {
            let table = item
                .as_table_like_mut()
                .context("settings parent is not a table")?;
            if let Some(child) = table.get_mut(key) {
                if rest.is_empty() && child.is_table() {
                    let toml_edit::Value::InlineTable(replacement) = value else {
                        bail!("expected a TOML table");
                    };
                    let child_table = child.as_table_mut().context("expected a table")?;
                    let mut replacement = replacement.into_table();
                    replacement.set_position(child_table.position().unwrap_or_default());
                    *replacement.decor_mut() = child_table.decor().clone();
                    *child_table = replacement;
                    return Ok(());
                }
                return edit_toml(child, rest, value);
            }
            let child = if rest.is_empty() {
                Item::Value(value)
            } else {
                let mut new = Item::Value(match rest[0] {
                    Part::Key(_) => toml_edit::Value::InlineTable(toml_edit::InlineTable::new()),
                    Part::Index(_) => toml_edit::Value::Array(toml_edit::Array::new()),
                });
                edit_toml(&mut new, rest, value)?;
                new
            };
            table.insert(key, child);
        }
        Part::Index(index) => {
            ensure!(rest.is_empty(), "expected a complete TOML array entry");
            if let Some(tables) = item.as_array_of_tables_mut() {
                let toml_edit::Value::InlineTable(table) = value else {
                    bail!("expected a TOML array table");
                };
                let mut table = table.into_table();
                if let Some(old) = tables.get_mut(*index) {
                    table.set_position(old.position().unwrap_or_default());
                    *table.decor_mut() = old.decor().clone();
                    *old = table;
                } else {
                    ensure!(*index == tables.len(), "invalid TOML array position");
                    tables.push(table);
                }
            } else {
                let array = item
                    .as_array_mut()
                    .context("settings parent is not an array")?;
                if let Some(old) = array.get_mut(*index) {
                    let mut value = value;
                    *value.decor_mut() = old.decor().clone();
                    *old = value;
                } else {
                    ensure!(*index == array.len(), "invalid TOML array position");
                    array.push(value);
                }
            }
        }
    }
    Ok(())
}

struct Node {
    span: Range<usize>,
    items: Items,
}

enum Items {
    Object(Vec<(String, Node)>),
    Array(Vec<Node>),
    Scalar,
}

impl Node {
    fn node_at(&self, path: &[Part<'_>]) -> Option<&Self> {
        let Some((first, rest)) = path.split_first() else {
            return Some(self);
        };
        let next = match (first, &self.items) {
            (Part::Key(key), Items::Object(items)) => {
                &items.iter().find(|(name, _)| name == key)?.1
            }
            (Part::Index(index), Items::Array(items)) => items.get(*index)?,
            _ => return None,
        };
        next.node_at(rest)
    }

    fn object_order(&self, path: &mut Vec<String>, order: &mut Vec<Vec<String>>) {
        if let Items::Object(items) = &self.items {
            for (key, value) in items {
                path.push(key.clone());
                order.push(path.clone());
                value.object_order(path, order);
                path.pop();
            }
        }
    }

    fn edit(
        &self,
        bytes: &mut Vec<u8>,
        path: &[Part<'_>],
        value: Value,
        depth: usize,
    ) -> Result<()> {
        let Some((first, rest)) = path.split_first() else {
            bytes.splice(self.span.clone(), serde_json::to_vec(&value)?);
            return Ok(());
        };
        let (last, mut addition) = match (first, &self.items) {
            (Part::Key(key), Items::Object(entries)) => {
                if let Some((_, child)) = entries.iter().find(|(existing, _)| existing == key) {
                    return child.edit(bytes, rest, value, depth + 1);
                }
                (
                    entries.last().map(|(_, node)| node),
                    format!("{}: ", serde_json::to_string(key)?),
                )
            }
            (Part::Index(index), Items::Array(entries)) => {
                if let Some(child) = entries.get(*index) {
                    return child.edit(bytes, rest, value, depth + 1);
                }
                ensure!(*index == entries.len(), "invalid JSON array position");
                (entries.last(), String::new())
            }
            _ => bail!("settings parent has a different shape"),
        };
        let close = self.span.end - 1;
        let indent = "  ".repeat(depth + 1);
        addition.insert_str(0, &format!("\n{indent}"));
        addition.push_str(&serde_json::to_string(&nested(rest, value)?)?);
        addition.push_str(&format!("\n{}", "  ".repeat(depth)));
        if let Some(last) = last {
            let mut scanner = Scanner {
                bytes,
                at: last.span.end,
            };
            scanner.layout()?;
            let needs_comma = scanner.at == close;
            bytes.splice(close..close, addition.bytes());
            if needs_comma {
                bytes.insert(last.span.end, b',');
            }
        } else {
            bytes.splice(close..close, addition.bytes());
        }
        Ok(())
    }
}

fn nested(path: &[Part<'_>], value: Value) -> Result<Value> {
    path.iter().rev().try_fold(value, |child, part| match part {
        Part::Key(key) => Ok(Value::Object(Map::from_iter([(key.to_string(), child)]))),
        Part::Index(0) => Ok(Value::Array(vec![child])),
        Part::Index(_) => bail!("invalid JSON array position"),
    })
}

// Input was parsed and checked for duplicate keys before editing. This pass
// keeps source offsets, including the bytes serde does not retain.
struct Scanner<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Scanner<'_> {
    fn peek(&self) -> u8 {
        self.bytes.get(self.at).copied().unwrap_or_default()
    }

    fn layout(&mut self) -> Result<()> {
        loop {
            let rest = &self.bytes[self.at..];
            if self.peek().is_ascii_whitespace() {
                self.at += 1;
            } else if rest.starts_with(b"//") {
                self.at += rest
                    .iter()
                    .position(|c| matches!(c, b'\r' | b'\n'))
                    .unwrap_or(rest.len());
            } else if rest.starts_with(b"/*") {
                self.at += rest[2..]
                    .windows(2)
                    .position(|pair| pair == b"*/")
                    .context("unterminated settings comment")?
                    + 4;
            } else {
                return Ok(());
            }
        }
    }

    fn string(&mut self) -> Result<Range<usize>> {
        ensure!(self.peek() == b'"', "expected a settings key");
        let start = self.at;
        self.at += 1;
        while self.at < self.bytes.len() {
            self.at += match self.peek() {
                b'"' => return Ok(start..self.at + 1),
                b'\\' => 2,
                _ => 1,
            };
        }
        bail!("unterminated settings string")
    }

    fn node(&mut self) -> Result<Node> {
        self.layout()?;
        let start = self.at;
        let items = match self.peek() {
            b'{' => Items::Object(self.items(b'}', |scanner| {
                let key = scanner.string()?;
                scanner.at = key.end;
                let key = serde_json::from_slice(&scanner.bytes[key])
                    .map_err(|_| anyhow::anyhow!("invalid settings key"))?;
                scanner.layout()?;
                ensure!(scanner.peek() == b':', "invalid settings object");
                scanner.at += 1;
                Ok((key, scanner.node()?))
            })?),
            b'[' => Items::Array(self.items(b']', Self::node)?),
            b'"' => {
                self.at = self.string()?.end;
                Items::Scalar
            }
            _ => {
                while self.peek() != 0
                    && !self.peek().is_ascii_whitespace()
                    && !matches!(self.peek(), b',' | b'}' | b']' | b'/')
                {
                    self.at += 1;
                }
                ensure!(self.at > start, "invalid settings value");
                Items::Scalar
            }
        };
        Ok(Node {
            span: start..self.at,
            items,
        })
    }

    fn items<T>(
        &mut self,
        close: u8,
        mut read: impl FnMut(&mut Self) -> Result<T>,
    ) -> Result<Vec<T>> {
        self.at += 1;
        let mut items = Vec::new();
        loop {
            self.layout()?;
            if self.peek() == close {
                self.at += 1;
                return Ok(items);
            }
            items.push(read(self)?);
            self.layout()?;
            match self.peek() {
                b',' => self.at += 1,
                byte if byte == close => (),
                _ => bail!("invalid settings collection"),
            }
        }
    }
}

type CommandRule = (Range<usize>, Value);

fn command_rules(bytes: &[u8]) -> Result<Vec<CommandRule>> {
    static CALL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
        r##"(?m)^\s*(prefix_rule\s*\(\s*((?:[^()"#]+|"(?:[^"\\]|\\.)*")*)\))[^\S\r\n]*(?:#[^\r\n]*)?$"##,
    ).expect("prefix_rule syntax")
    });
    static FIELD: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^\s*(pattern|decision|justification)\s*=\s*")
            .expect("prefix_rule field syntax")
    });
    let text = std::str::from_utf8(bytes).context("command rules are not UTF-8")?;
    let comment_or_empty = |part: &str| {
        part.lines().all(|line| {
            let line = line.trim();
            line.is_empty() || line.starts_with('#')
        })
    };
    let mut rules = Vec::new();
    let mut end = 0;
    for call in CALL.captures_iter(text) {
        let whole = call.get(0).expect("whole rule match");
        ensure!(
            comment_or_empty(&text[end..whole.start()]),
            "unsupported command rule syntax"
        );
        end = whole.end();
        let mut fields = call[2].trim();
        let mut values = Map::new();
        while !fields.is_empty() {
            let field = FIELD
                .captures(fields)
                .context("unsupported command rule field")?;
            let tail = &fields[field.get(0).expect("rule field match").end()..];
            let mut parser = serde_json::Deserializer::from_str(tail).into_iter::<Value>();
            let value = parser
                .next()
                .and_then(Result::ok)
                .context("unsupported command rule value")?;
            ensure!(
                values.insert(field[1].to_owned(), value).is_none(),
                "duplicate command rule field"
            );
            fields = tail[parser.byte_offset()..].trim();
            if !fields.is_empty() {
                fields = fields
                    .strip_prefix(',')
                    .context("unsupported command rule separator")?
                    .trim();
            }
        }
        ensure!(
            values.get("pattern").and_then(Value::as_array).is_some()
                && values.get("decision").and_then(Value::as_str).is_some(),
            "unsupported command rule shape"
        );
        let call = call.get(1).expect("prefix_rule match");
        rules.push((call.start()..call.end(), Value::Object(values)));
    }
    ensure!(
        comment_or_empty(&text[end..]),
        "unsupported command rule syntax"
    );
    Ok(rules)
}
