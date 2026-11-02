// SPDX-License-Identifier: Apache-2.0
//! Field-by-field comparison of two databases, table by table: every row of
//! the source must be in the rebuilt database with the same stored values,
//! and nothing else. Rows are compared as stored values (an integer is not a
//! real, text is not bytes), in any order.

use std::collections::BTreeMap;

use crate::db::MemoryDB;
use crate::error::WenlanError;
use crate::store::export::settings::cell;

/// The rows of one table that differ.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TableDiff {
    pub table: String,
    pub only_in_source: Vec<String>,
    pub only_in_rebuilt: Vec<String>,
}

/// Compares `tables` in both databases; returns only the tables that
/// differ. A table missing from one side counts as having no rows there.
pub async fn compare_tables(
    source: &MemoryDB,
    rebuilt: &MemoryDB,
    tables: &[&'static str],
) -> Result<Vec<TableDiff>, WenlanError> {
    let mut diffs = Vec::new();
    for table in tables {
        let a = rows_of(source, table).await?;
        let mut b = rows_of(rebuilt, table).await?;
        let mut diff = TableDiff {
            table: table.to_string(),
            ..TableDiff::default()
        };
        for (row, count) in a {
            let other = b.remove(&row).unwrap_or(0);
            for _ in other..count {
                diff.only_in_source.push(row.clone());
            }
            for _ in count..other {
                diff.only_in_rebuilt.push(row.clone());
            }
        }
        for (row, count) in b {
            for _ in 0..count {
                diff.only_in_rebuilt.push(row.clone());
            }
        }
        if !diff.only_in_source.is_empty() || !diff.only_in_rebuilt.is_empty() {
            diffs.push(diff);
        }
    }
    Ok(diffs)
}

/// Each row as `{column: value}` JSON, with how many times it occurs.
async fn rows_of(
    db: &MemoryDB,
    table: &'static str,
) -> Result<BTreeMap<String, usize>, WenlanError> {
    let mut out = BTreeMap::new();
    let Some(copied) = db.export_table(table).await? else {
        return Ok(out);
    };
    for values in &copied.rows {
        let row: serde_json::Map<String, serde_json::Value> = copied
            .columns
            .iter()
            .cloned()
            .zip(values.iter().map(cell))
            .collect();
        *out.entry(serde_json::Value::Object(row).to_string())
            .or_default() += 1;
    }
    Ok(out)
}

/// Rows of one table matched by key, column by column.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct KeyedDiff {
    pub table: String,
    /// Rows in the source.
    pub rows: usize,
    pub only_in_source: Vec<String>,
    pub only_in_rebuilt: Vec<String>,
    /// Each column with a different value in some matched row.
    pub columns: BTreeMap<String, ColumnDiff>,
}

impl KeyedDiff {
    pub fn is_empty(&self) -> bool {
        self.only_in_source.is_empty() && self.only_in_rebuilt.is_empty() && self.columns.is_empty()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct ColumnDiff {
    pub rows: usize,
    /// Keys of the rows that differ.
    pub keys: Vec<String>,
}

/// Compares `table` row by row, matching rows on `key`, every column but
/// `skip`. Values are compared as stored, except where [`normalized`] says
/// a part of a value is cache.
pub async fn compare_keyed(
    source: &MemoryDB,
    rebuilt: &MemoryDB,
    table: &'static str,
    key: &[&str],
    skip: &[&str],
) -> Result<KeyedDiff, WenlanError> {
    let a = keyed_rows(source, table, key).await?;
    let mut b = keyed_rows(rebuilt, table, key).await?;
    let mut diff = KeyedDiff {
        table: table.to_string(),
        rows: a.len(),
        ..KeyedDiff::default()
    };
    for (row_key, row) in a {
        let Some(other) = b.remove(&row_key) else {
            diff.only_in_source.push(row_key);
            continue;
        };
        let columns: std::collections::BTreeSet<&String> = row.keys().chain(other.keys()).collect();
        for column in columns {
            if skip.contains(&column.as_str()) {
                continue;
            }
            let null = serde_json::Value::Null;
            let left = normalized(table, column, row.get(column).unwrap_or(&null));
            let right = normalized(table, column, other.get(column).unwrap_or(&null));
            if left != right {
                let entry = diff.columns.entry(column.clone()).or_default();
                entry.rows += 1;
                entry.keys.push(row_key.clone());
            }
        }
    }
    diff.only_in_rebuilt = b.into_keys().collect();
    Ok(diff)
}

/// A page's citation map without its verification fields, which the
/// rebuild recomputes: occurrence, marker, source and cited version.
fn normalized(table: &str, column: &str, value: &serde_json::Value) -> serde_json::Value {
    if table != "pages" || column != "citations" {
        return value.clone();
    }
    let raw = value.as_str().unwrap_or("").trim();
    let raw = if raw.is_empty() { "[]" } else { raw };
    let Ok(serde_json::Value::Array(records)) = serde_json::from_str::<serde_json::Value>(raw)
    else {
        return value.clone();
    };
    let mut kept: Vec<serde_json::Value> = records
        .iter()
        .map(|record| {
            let mut out = serde_json::Map::new();
            for field in [
                "occurrence",
                "marker",
                "source_kind",
                "locator",
                "cited_version",
            ] {
                if let Some(value) = record.get(field).filter(|value| !value.is_null()) {
                    out.insert(field.to_string(), value.clone());
                }
            }
            serde_json::Value::Object(out)
        })
        .collect();
    kept.sort_by_key(|record| record["occurrence"].as_u64());
    serde_json::Value::Array(kept)
}

async fn keyed_rows(
    db: &MemoryDB,
    table: &'static str,
    key: &[&str],
) -> Result<BTreeMap<String, serde_json::Map<String, serde_json::Value>>, WenlanError> {
    let mut out = BTreeMap::new();
    let Some(copied) = db.export_table(table).await? else {
        return Ok(out);
    };
    for values in &copied.rows {
        let row: serde_json::Map<String, serde_json::Value> = copied
            .columns
            .iter()
            .cloned()
            .zip(values.iter().map(cell))
            .collect();
        let row_key = key
            .iter()
            .map(|column| match row.get(*column) {
                Some(serde_json::Value::String(text)) => text.clone(),
                Some(value) => value.to_string(),
                None => String::new(),
            })
            .collect::<Vec<_>>()
            .join("/");
        out.insert(row_key, row);
    }
    Ok(out)
}

/// Memories compared as memories, not rows: both databases are grouped by
/// `(source, source_id)` and each memory's decisions are compared, field by
/// field, as the export reads them. Only memories the export wrote as files
/// (`written`, by source id) are compared on the source side; the rest are
/// listed in its manifest.
///
/// What counts as the same, and why:
/// - the text is the person's typed text when there is one, else the chunks
///   joined in order; chunk boundaries and the tidied text are cache;
/// - per-chunk fields come from the first chunk, the one the file speaks for;
/// - a classified type or fields enrichment chose are cache: only explicit
///   ones are compared;
/// - an empty title or cue is no title or cue, a source text equal to the
///   text is no source text, an unset supersede mode is `hide`, and an unset
///   edit time is 0, as the columns read;
/// - stated fields are compared as JSON values, not as text;
/// - `about` is the memory's entity links, sorted.
pub async fn compare_memories(
    source: &MemoryDB,
    rebuilt: &MemoryDB,
    written: &std::collections::HashSet<String>,
) -> Result<KeyedDiff, WenlanError> {
    let a = memory_decisions(source, Some(written)).await?;
    let mut b = memory_decisions(rebuilt, None).await?;
    let mut diff = KeyedDiff {
        table: "memories".to_string(),
        rows: a.len(),
        ..KeyedDiff::default()
    };
    for (key, left) in a {
        let Some(right) = b.remove(&key) else {
            diff.only_in_source.push(key);
            continue;
        };
        for (field, value) in &left {
            if right.get(field) != Some(value) {
                let entry = diff.columns.entry(field.clone()).or_default();
                entry.rows += 1;
                entry.keys.push(key.clone());
            }
        }
    }
    diff.only_in_rebuilt = b.into_keys().collect();
    Ok(diff)
}

async fn memory_decisions(
    db: &MemoryDB,
    written: Option<&std::collections::HashSet<String>>,
) -> Result<BTreeMap<String, BTreeMap<String, serde_json::Value>>, WenlanError> {
    use serde_json::{json, Value};
    let rows = db.export_memory_rows().await?;
    // `about` links, by memory (source id).
    let mut about: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (memory, entity) in db.export_memory_entities().await? {
        about.entry(memory).or_default().push(entity);
    }
    let mut groups: BTreeMap<String, Vec<&crate::db::export_reads::ExportMemoryRow>> =
        BTreeMap::new();
    for row in &rows {
        if written.is_some_and(|ids| !ids.contains(&row.source_id)) {
            continue;
        }
        groups
            .entry(format!("{}/{}", row.source, row.source_id))
            .or_default()
            .push(row);
    }
    let some = |value: &Option<String>| {
        value
            .as_deref()
            .filter(|text| !text.is_empty())
            .map_or(Value::Null, |text| json!(text))
    };
    let fields = |value: &Option<String>| match value.as_deref() {
        None => Value::Null,
        Some(raw) => serde_json::from_str(raw).unwrap_or_else(|_| json!(raw)),
    };
    let mut out = BTreeMap::new();
    for (key, mut chunks) in groups {
        chunks.sort_by_key(|chunk| chunk.chunk_index);
        let head = chunks[0];
        let authored = chunks
            .iter()
            .find_map(|chunk| chunk.authored_content.clone());
        let text = authored.clone().unwrap_or_else(|| {
            chunks
                .iter()
                .map(|chunk| chunk.content.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        });
        let source_text = head.source_text.clone().filter(|source| *source != text);
        let mut map = BTreeMap::new();
        let mut put = |name: &str, value: Value| {
            map.insert(name.to_string(), value);
        };
        put("text", json!(text));
        put("typed", json!(authored.is_some()));
        put("title", some(&head.title));
        put("url", some(&head.url));
        put(
            "memory_type",
            if head.memory_type_explicit {
                some(&head.memory_type)
            } else {
                Value::Null
            },
        );
        put("memory_type_explicit", json!(head.memory_type_explicit));
        put("space", some(&head.space));
        put("source_agent", some(&head.source_agent));
        put("confirmed", json!(head.confirmed));
        put("pinned", json!(head.pinned));
        put("supersedes", some(&head.supersedes));
        put(
            "supersede_mode",
            json!(head.supersede_mode.as_deref().unwrap_or("hide")),
        );
        put("pending_revision", json!(head.pending_revision));
        put(
            "structured_fields",
            if head.structured_fields_explicit {
                fields(&head.structured_fields)
            } else {
                Value::Null
            },
        );
        put(
            "structured_fields_explicit",
            json!(head.structured_fields_explicit),
        );
        put("source_text", some(&source_text));
        put("created_at", json!(head.created_at));
        put("event_date", json!(head.event_date));
        put("event_end", json!(head.event_end));
        put("importance", json!(head.importance));
        put("episode_of", some(&head.episode_of));
        put("origin_class", some(&head.origin_class));
        put("session_id", some(&head.session_id));
        put("entity_id", some(&head.entity_id));
        put("is_recap", json!(head.is_recap));
        put("last_modified", json!(head.last_modified.unwrap_or(0)));
        put("retrieval_cue", some(&head.retrieval_cue));
        put("space_rejected", json!(head.space_rejected));
        put(
            "about",
            json!(about.get(&head.source_id).cloned().unwrap_or_default()),
        );
        out.insert(key, map);
    }
    Ok(out)
}
