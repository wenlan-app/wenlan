// SPDX-License-Identifier: Apache-2.0
//! Pages, archived pages and entities read back into `pages` rows: the
//! inverse of `store::export::pages`. The columns the files hold come from
//! the files; the history columns come from the log's state seeds
//! ([`super::apply_states`]); cache columns get placeholders the comparison
//! skips.

use std::path::Path;

use serde_json::{json, Value};

use crate::db::MemoryDB;
use crate::error::WenlanError;
use crate::store::export::Manifest;
use crate::store::format::entity;
use crate::store::format::page::{self, Locator, PageFile};

/// The columns this part writes, in order.
const COLUMNS: &[&str] = &[
    "id",
    "title",
    "content",
    "status",
    "space",
    "created_at",
    "last_modified",
    "last_compiled",
    "citations",
    "entity_id",
    "entity_type",
    "entity_confirmed",
    "aliases",
    "established_by",
    "summary",
    "entity_created_at",
    "entity_updated_at",
];

#[derive(Default)]
struct Row {
    id: String,
    title: String,
    content: String,
    status: String,
    space: Option<String>,
    created_at: String,
    last_modified: String,
    citations: Option<String>,
    entity_id: Option<String>,
    entity_type: Option<String>,
    entity_confirmed: Option<i64>,
    aliases: Option<String>,
    established_by: Option<String>,
    summary: Option<String>,
    entity_created_at: Option<i64>,
    entity_updated_at: Option<i64>,
}

impl Row {
    fn values(self) -> Vec<libsql::Value> {
        let text = |value: Option<String>| value.map_or(libsql::Value::Null, libsql::Value::Text);
        let int = |value: Option<i64>| value.map_or(libsql::Value::Null, libsql::Value::Integer);
        vec![
            libsql::Value::Text(self.id),
            libsql::Value::Text(self.title),
            libsql::Value::Text(self.content),
            libsql::Value::Text(self.status),
            text(self.space),
            libsql::Value::Text(self.created_at),
            libsql::Value::Text(self.last_modified.clone()),
            // `last_compiled` is cache, but NOT NULL: it gets the edit time.
            libsql::Value::Text(self.last_modified),
            text(self.citations),
            text(self.entity_id),
            text(self.entity_type),
            int(self.entity_confirmed),
            text(self.aliases),
            text(self.established_by),
            text(self.summary),
            int(self.entity_created_at),
            int(self.entity_updated_at),
        ]
    }
}

/// Writes one `pages` row per page, archived page and entity file the
/// manifest lists; returns how many. A file that does not read is a
/// problem, not an error.
pub async fn rebuild_pages(
    from: &Path,
    manifest: &Manifest,
    into: &MemoryDB,
    problems: &mut Vec<String>,
) -> Result<usize, WenlanError> {
    let mut rows = Vec::new();
    for file in &manifest.files {
        let path = from.join(&file.path);
        let row = match file.file_type.as_str() {
            "page" | "archived_page" => {
                let text = std::fs::read_to_string(&path)?;
                match page::parse(&text, "") {
                    Ok(parsed) => page_row(parsed, file.file_type == "archived_page", problems),
                    Err(error) => {
                        problems.push(format!("{}: {error}", file.path));
                        continue;
                    }
                }
            }
            "entity" => {
                let text = std::fs::read_to_string(&path)?;
                match entity::parse(&text) {
                    Ok(parsed) => entity_row(parsed, problems),
                    Err(error) => {
                        problems.push(format!("{}: {error}", file.path));
                        continue;
                    }
                }
            }
            _ => continue,
        };
        match row {
            Some(row) => rows.push(row.values()),
            None => problems.push(format!("{}: no page id", file.path)),
        }
    }
    let columns: Vec<String> = COLUMNS.iter().map(|c| c.to_string()).collect();
    let count = rows.len();
    into.rebuild_replace_table("pages", &columns, &rows).await?;
    Ok(count)
}

fn page_row(mut file: PageFile, archived: bool, problems: &mut Vec<String>) -> Option<Row> {
    let id = file.id.clone()?;
    let mut other = std::mem::take(&mut file.other);
    let status = take_text(&mut other, "status")
        .unwrap_or_else(|| if archived { "archived" } else { "active" }.to_string());
    let citations = citations_of(&file, &id, problems);
    Some(Row {
        id,
        title: file.title,
        status,
        space: file.tags.first().cloned(),
        created_at: file.created.unwrap_or_default(),
        last_modified: file.modified.unwrap_or_default(),
        citations,
        summary: take_text(&mut other, "summary"),
        entity_id: take_text(&mut other, "entity_id"),
        entity_type: take_text(&mut other, "entity_type"),
        aliases: take_text(&mut other, "entity_aliases"),
        established_by: take_text(&mut other, "established_by"),
        entity_confirmed: other
            .shift_remove("entity_confirmed")
            .and_then(|value| value.as_bool())
            .map(i64::from),
        entity_created_at: take_int(&mut other, "entity_created_at"),
        entity_updated_at: take_int(&mut other, "entity_updated_at"),
        content: file.body,
    })
}

fn entity_row(mut file: entity::EntityFile, problems: &mut Vec<String>) -> Option<Row> {
    let id = file.page.clone()?;
    let other = &mut file.other;
    let seen = |other: &mut serde_yaml::Mapping, key: &str, problems: &mut Vec<String>| {
        let text = take_text(other, key)?;
        let at = seconds_of(&text);
        if at.is_none() {
            problems.push(format!("entity {id}: {key} {text:?} is not a time"));
        }
        at
    };
    let entity_created_at = seen(other, "first_seen", problems);
    let entity_updated_at = seen(other, "last_seen", problems);
    Some(Row {
        title: file.name,
        content: file.description,
        status: take_text(other, "status").unwrap_or_else(|| "active".to_string()),
        space: file.space,
        created_at: take_text(other, "created").unwrap_or_default(),
        last_modified: take_text(other, "modified").unwrap_or_default(),
        // An entity's page has no citations; the export lists any it had.
        citations: Some("[]".to_string()),
        entity_type: file.entity_type,
        entity_confirmed: if file.confirmed {
            Some(1)
        } else {
            take_bool(other, "entity_confirmed").map(|_| 0)
        },
        aliases: (!file.aliases.is_empty())
            .then(|| serde_json::to_string(&file.aliases).unwrap_or_default()),
        established_by: file.established_by,
        entity_id: Some(file.id),
        entity_created_at,
        entity_updated_at,
        summary: None,
        id,
    })
}

/// The citation map: one record per `[N]` occurrence in the body whose
/// marker has a sources line, in occurrence order. Verification fields
/// (score, status, scope, digest) are cache; they get the values of a
/// citation not yet checked.
fn citations_of(file: &PageFile, id: &str, problems: &mut Vec<String>) -> Option<String> {
    let mut records = Vec::new();
    for (occurrence, (start, end)) in crate::citations::marker_spans(&file.body)
        .into_iter()
        .enumerate()
    {
        let Ok(marker) = file.body[start + 1..end - 1].parse::<u32>() else {
            continue;
        };
        let Some(line) = file.citations.iter().find(|line| line.marker == marker) else {
            continue;
        };
        let (kind, locator, version) = source_of(&line.locator);
        let mut record = json!({
            "occurrence": occurrence as u32 + 1,
            "marker": marker,
            "source_kind": kind,
            "locator": locator,
            "score": 0.0,
            "status": "unverified",
            "scope": "paragraph",
        });
        if let Some(version) = version {
            record["cited_version"] = json!(version);
        }
        records.push(record);
    }
    let lines_used = file
        .citations
        .iter()
        .filter(|line| {
            crate::citations::marker_spans(&file.body)
                .iter()
                .any(|(start, end)| file.body[start + 1..end - 1] == line.marker.to_string())
        })
        .count();
    if lines_used < file.citations.len() {
        problems.push(format!(
            "page {id}: a sources line has no marker in the body"
        ));
    }
    if records.is_empty() {
        return Some("[]".to_string());
    }
    Some(Value::Array(records).to_string())
}

/// `(source_kind, locator, cited_version)` from a sources line's locator,
/// the inverse of the export's `locator_of`.
pub(crate) fn source_of(locator: &Locator) -> (String, String, Option<i64>) {
    match locator {
        Locator::Wenlan { id, version } => ("memory".to_string(), id.clone(), Some(*version)),
        Locator::Other(text) => {
            if let Some(id) = text.strip_prefix("wenlan:") {
                return ("memory".to_string(), id.to_string(), None);
            }
            let (kind, rest) = text.split_once(':').unwrap_or(("", text.as_str()));
            match rest.rsplit_once('@') {
                Some((locator, version)) if version.parse::<i64>().is_ok() => {
                    (kind.to_string(), locator.to_string(), version.parse().ok())
                }
                _ => (kind.to_string(), rest.to_string(), None),
            }
        }
    }
}

pub(super) fn take_text(other: &mut serde_yaml::Mapping, key: &str) -> Option<String> {
    match other.shift_remove(key)? {
        serde_yaml::Value::String(text) => Some(text),
        value => serde_yaml::to_string(&value)
            .ok()
            .map(|text| text.trim_end().to_string()),
    }
}

fn take_bool(other: &mut serde_yaml::Mapping, key: &str) -> Option<bool> {
    other.shift_remove(key)?.as_bool()
}

fn take_int(other: &mut serde_yaml::Mapping, key: &str) -> Option<i64> {
    other.shift_remove(key)?.as_i64()
}

/// Unix seconds from the export's `utc` text (or the number it wrote when
/// the value was not a time).
pub(super) fn seconds_of(text: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(text)
        .map(|at| at.timestamp())
        .ok()
        .or_else(|| text.parse().ok())
}
