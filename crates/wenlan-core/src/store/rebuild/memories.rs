// SPDX-License-Identifier: Apache-2.0
//! Memories and originals read back into `memories` rows: the inverse of
//! `store::export::memories`. A memory file becomes one row (its chunk
//! boundaries are cache); an original becomes one row per message, in
//! order. `about` links become `memory_entities` rows. What the files do
//! not hold (classified types, extracted fields, embeddings) is cache and
//! is recomputed; the explicit flags come back with `enrichment_origin`
//! from the history log.

use std::collections::HashMap;
use std::path::Path;

use crate::db::MemoryDB;
use crate::error::WenlanError;
use crate::store::export::Manifest;
use crate::store::format::memory::{self, MemoryFile};
use crate::store::format::original::{self, OriginalFile};

const COLUMNS: &[&str] = &[
    "id",
    "source",
    "source_id",
    "chunk_index",
    "content",
    "title",
    "url",
    "memory_type",
    "space",
    "source_agent",
    "confirmed",
    "pinned",
    "supersedes",
    "supersede_mode",
    "structured_fields",
    "source_text",
    "created_at",
    "event_date",
    "event_end",
    "importance",
    "episode_of",
    "origin_class",
    "session_id",
    "entity_id",
    "is_recap",
    "last_modified",
    "retrieval_cue",
    "chunk_type",
    "authored_content",
];

/// What the rebuild wrote.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemoriesRebuilt {
    pub memories: usize,
    pub originals: usize,
    pub rows: usize,
    pub links: usize,
}

pub async fn rebuild_memories(
    from: &Path,
    manifest: &Manifest,
    into: &MemoryDB,
    problems: &mut Vec<String>,
) -> Result<MemoriesRebuilt, WenlanError> {
    // `[[stem]]` of an entity file → its entity id.
    let mut entities: HashMap<String, String> = HashMap::new();
    for file in manifest.files.iter().filter(|f| f.file_type == "entity") {
        let stem = file
            .path
            .rsplit('/')
            .next()
            .and_then(|name| name.strip_suffix(".md"))
            .unwrap_or_default();
        entities.insert(stem.to_string(), file.id.clone());
    }

    let mut done = MemoriesRebuilt::default();
    let mut rows = Vec::new();
    let mut links = Vec::new();
    for file in &manifest.files {
        let path = from.join(&file.path);
        match file.file_type.as_str() {
            "memory" => {
                let text = std::fs::read_to_string(&path)?;
                let parsed = match memory::parse(&text) {
                    Ok(parsed) => parsed,
                    Err(error) => {
                        problems.push(format!("{}: {error}", file.path));
                        continue;
                    }
                };
                for stem in &parsed.about {
                    match entities.get(stem) {
                        Some(entity_id) => links.push(vec![
                            libsql::Value::Text(parsed.id.clone()),
                            libsql::Value::Text(entity_id.clone()),
                        ]),
                        None => problems.push(format!(
                            "{}: about [[{stem}]] names no entity file",
                            file.path
                        )),
                    }
                }
                rows.push(memory_row(parsed, problems));
                done.memories += 1;
            }
            "original" => {
                let text = std::fs::read_to_string(&path)?;
                let parsed = match original::parse(&text) {
                    Ok(parsed) => parsed,
                    Err(error) => {
                        problems.push(format!("{}: {error}", file.path));
                        continue;
                    }
                };
                for stem in about_of(&parsed.other) {
                    match entities.get(&stem) {
                        Some(entity_id) => links.push(vec![
                            libsql::Value::Text(parsed.id.clone()),
                            libsql::Value::Text(entity_id.clone()),
                        ]),
                        None => problems.push(format!(
                            "{}: about [[{stem}]] names no entity file",
                            file.path
                        )),
                    }
                }
                rows.extend(original_rows(parsed, problems));
                done.originals += 1;
            }
            _ => {}
        }
    }
    done.rows = rows.len();
    done.links = links.len();
    let columns: Vec<String> = COLUMNS.iter().map(|c| c.to_string()).collect();
    into.rebuild_replace_table("memories", &columns, &rows)
        .await?;
    into.rebuild_replace_table(
        "memory_entities",
        &["memory_id".to_string(), "entity_id".to_string()],
        &links,
    )
    .await?;
    Ok(done)
}

/// The memory's decisions, as one memory file holds them, whether they
/// come from the file's own keys or (for an original) its other keys.
#[derive(Default)]
struct Decisions {
    source: String,
    kind: Option<String>,
    space: Option<String>,
    agent: Option<String>,
    created: Option<i64>,
    pinned: bool,
    confirmed: bool,
    supersedes: Option<String>,
    supersede_mode: Option<String>,
    episode_of: Option<String>,
    event_date: Option<i64>,
    event_end: Option<i64>,
    importance: Option<i64>,
    origin: Option<String>,
    session: Option<String>,
    entity: Option<String>,
    recap: bool,
    fields: Option<String>,
    source_text: Option<String>,
    modified: Option<i64>,
    cue: Option<String>,
    /// The text is what the person typed: it is also `authored_content`.
    typed: bool,
}

fn memory_row(mut file: MemoryFile, problems: &mut Vec<String>) -> Vec<libsql::Value> {
    let id = file.id.clone();
    let mut time = |text: Option<String>, what: &str| time_of(text, &id, what, problems);
    let created = time(file.created.take(), "created");
    let event_date = time(file.event_date.take(), "event_date");
    let event_end = time(file.event_end.take(), "event_end");
    let modified = time(text_key(&mut file.other, "modified"), "modified");
    let decisions = Decisions {
        source: text_key(&mut file.other, "source").unwrap_or_else(|| "memory".to_string()),
        kind: file.kind,
        space: file.space,
        agent: file.agent,
        created,
        pinned: file.pinned,
        confirmed: file.confirmed,
        supersedes: file.supersedes,
        supersede_mode: file.supersede_mode,
        episode_of: file.episode_of,
        event_date,
        event_end,
        importance: file.importance,
        origin: file.origin,
        session: file.session,
        entity: file.entity,
        recap: file.recap,
        fields: file.fields.as_ref().map(fields_text),
        source_text: file.source_text,
        modified,
        cue: text_key(&mut file.other, "cue"),
        typed: file
            .other
            .shift_remove("typed")
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
    };
    row_of(
        &file.id, 0, file.text, file.title, file.url, &decisions, None,
    )
}

fn original_rows(mut file: OriginalFile, problems: &mut Vec<String>) -> Vec<Vec<libsql::Value>> {
    let other = &mut file.other;
    let id = file.id.clone();
    let mut time = |text: Option<String>, what: &str| time_of(text, &id, what, problems);
    let created = time(text_key(other, "created"), "created");
    let event_date = time(text_key(other, "event_date"), "event_date");
    let event_end = time(text_key(other, "event_end"), "event_end");
    let modified = time(text_key(other, "modified"), "modified");
    let flag = |other: &mut serde_yaml::Mapping, key: &str| {
        other
            .shift_remove(key)
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
    };
    let decisions = Decisions {
        source: text_key(other, "source").unwrap_or_else(|| "memory".to_string()),
        kind: text_key(other, "kind"),
        space: text_key(other, "space"),
        agent: text_key(other, "agent"),
        created,
        pinned: flag(other, "pinned"),
        confirmed: flag(other, "confirmed"),
        supersedes: text_key(other, "supersedes"),
        supersede_mode: text_key(other, "supersede_mode"),
        episode_of: text_key(other, "episode_of"),
        event_date,
        event_end,
        importance: other.shift_remove("importance").and_then(|v| v.as_i64()),
        origin: text_key(other, "origin"),
        session: text_key(other, "session"),
        entity: text_key(other, "entity"),
        recap: flag(other, "recap"),
        fields: other.shift_remove("fields").as_ref().map(fields_text),
        source_text: text_key(other, "source_text"),
        modified,
        cue: text_key(other, "cue"),
        typed: flag(other, "typed"),
    };
    file.messages
        .into_iter()
        .enumerate()
        .map(|(index, message)| {
            let at = message.at.as_deref().and_then(seconds_of);
            row_of(
                &file.id,
                index as i64,
                message.text,
                file.title.clone(),
                file.url.clone(),
                &decisions,
                at,
            )
        })
        .collect()
}

fn row_of(
    source_id: &str,
    chunk: i64,
    content: String,
    title: Option<String>,
    url: Option<String>,
    d: &Decisions,
    created: Option<i64>,
) -> Vec<libsql::Value> {
    let text = |value: &Option<String>| {
        value
            .clone()
            .map_or(libsql::Value::Null, libsql::Value::Text)
    };
    let int = |value: Option<i64>| value.map_or(libsql::Value::Null, libsql::Value::Integer);
    let flag = |value: bool| libsql::Value::Integer(i64::from(value));
    let authored = if d.typed && chunk == 0 {
        libsql::Value::Text(content.clone())
    } else {
        libsql::Value::Null
    };
    vec![
        // Row ids are not part of a memory; a stable one per chunk.
        libsql::Value::Text(format!("{source_id}#{chunk}")),
        libsql::Value::Text(d.source.clone()),
        libsql::Value::Text(source_id.to_string()),
        libsql::Value::Integer(chunk),
        libsql::Value::Text(content),
        libsql::Value::Text(title.unwrap_or_default()),
        text(&url),
        text(&d.kind),
        text(&d.space),
        text(&d.agent),
        flag(d.confirmed),
        flag(d.pinned),
        text(&d.supersedes),
        libsql::Value::Text(
            d.supersede_mode
                .clone()
                .unwrap_or_else(|| "hide".to_string()),
        ),
        text(&d.fields),
        text(&d.source_text),
        int(created.or(d.created)),
        int(d.event_date),
        int(d.event_end),
        int(d.importance),
        text(&d.episode_of),
        text(&d.origin),
        text(&d.session),
        text(&d.entity),
        flag(d.recap),
        libsql::Value::Integer(d.modified.unwrap_or(0)),
        text(&d.cue),
        libsql::Value::Text("text".to_string()),
        authored,
    ]
}

/// An original's `about` key, unlinked.
fn about_of(other: &serde_yaml::Mapping) -> Vec<String> {
    other
        .get("about")
        .and_then(|value| value.as_sequence())
        .map(|links| {
            links
                .iter()
                .filter_map(|link| link.as_str())
                .map(|link| memory::unlink(link).to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn text_key(other: &mut serde_yaml::Mapping, key: &str) -> Option<String> {
    match other.shift_remove(key)? {
        serde_yaml::Value::String(text) => Some(text),
        value => serde_yaml::to_string(&value)
            .ok()
            .map(|text| text.trim_end().to_string()),
    }
}

/// Stated fields back as the JSON they were stored as; YAML that was text
/// stays that text.
fn fields_text(value: &serde_yaml::Value) -> String {
    if let serde_yaml::Value::String(text) = value {
        return text.clone();
    }
    serde_json::to_value(value)
        .map(|json| json.to_string())
        .unwrap_or_default()
}

fn time_of(text: Option<String>, id: &str, what: &str, problems: &mut Vec<String>) -> Option<i64> {
    let text = text?;
    let at = seconds_of(&text);
    if at.is_none() {
        problems.push(format!("memory {id}: {what} {text:?} is not a time"));
    }
    at
}

fn seconds_of(text: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(text)
        .map(|at| at.timestamp())
        .ok()
        .or_else(|| text.parse().ok())
}
