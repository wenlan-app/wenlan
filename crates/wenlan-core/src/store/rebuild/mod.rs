// SPDX-License-Identifier: Apache-2.0
//! The rebuild (storage migration step 2, spec section 10): a lossless
//! export read back into a fresh database, so [`compare`] can check it
//! field by field against the database it came from. Only ever writes the
//! fresh database it is given.
//!
//! So far: the tables copied whole (`settings/`, `records/`) and every
//! history table, replayed from the log. Pages, entities, memories,
//! originals, Briefs and canvases follow, read from their files.

pub mod compare;

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;

use crate::db::MemoryDB;
use crate::error::WenlanError;
use crate::store::history;

/// Rows written per table, and what could not be read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rebuilt {
    pub tables: BTreeMap<String, usize>,
    pub problems: Vec<String>,
}

/// Reads the whole-table files and the history log under `from` (an
/// export folder) into `into`.
pub async fn rebuild_tables(from: &Path, into: &MemoryDB) -> Result<Rebuilt, WenlanError> {
    let mut rebuilt = Rebuilt::default();
    let backstage = from.join("backstage");
    for folder in ["settings", "records"] {
        let dir = backstage.join(folder);
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut files: Vec<_> = read.collect::<Result<_, _>>()?;
        files.sort_by_key(|entry| entry.file_name());
        for file in files {
            let path = file.path();
            let text = std::fs::read_to_string(&path)?;
            let value: Value = match serde_json::from_str(&text) {
                Ok(value) => value,
                Err(error) => {
                    rebuilt
                        .problems
                        .push(format!("{}: {error}", path.display()));
                    continue;
                }
            };
            let Some(tables) = value.get("tables").and_then(Value::as_object) else {
                rebuilt
                    .problems
                    .push(format!("{}: no tables", path.display()));
                continue;
            };
            for (table, held) in tables {
                let columns: Vec<String> = held["columns"]
                    .as_array()
                    .map(|columns| {
                        columns
                            .iter()
                            .filter_map(|c| c.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                let rows: Vec<Vec<libsql::Value>> = held["rows"]
                    .as_array()
                    .map(|rows| {
                        rows.iter()
                            .map(|row| {
                                row.as_array()
                                    .map(|cells| cells.iter().map(value_of).collect())
                                    .unwrap_or_default()
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                into.rebuild_replace_table(table, &columns, &rows).await?;
                rebuilt.tables.insert(table.clone(), rows.len());
            }
        }
    }

    // Every history table is replaced, so one with no entries ends empty.
    let mut by_table: BTreeMap<String, Vec<serde_json::Map<String, Value>>> =
        crate::store::export::history::HISTORY_TABLES
            .iter()
            .map(|spec| (spec.table.to_string(), Vec::new()))
            .collect();
    let root = backstage.join("history");
    for log in [root.clone(), root.join("reads")] {
        if !log.is_dir() {
            continue;
        }
        let replay = history::replay(&log)?;
        if !replay.report.is_clean() {
            rebuilt
                .problems
                .push(format!("{}: {:?}", log.display(), replay.report));
        }
        for entry in &replay.entries {
            let Some(data) = &entry.data else {
                continue;
            };
            let (Some(table), Some(row)) = (
                data.get("table").and_then(Value::as_str),
                data.get("row").and_then(Value::as_object),
            ) else {
                rebuilt
                    .problems
                    .push(format!("history entry {} has no row", entry.seq));
                continue;
            };
            let mut row = row.clone();
            if let Some(column) = data.get("text_column").and_then(Value::as_str) {
                match history::read_text(&log, entry)? {
                    Some(text) => {
                        row.insert(column.to_string(), Value::String(text));
                    }
                    None => rebuilt
                        .problems
                        .push(format!("history entry {} lost its text", entry.seq)),
                }
            }
            by_table.entry(table.to_string()).or_default().push(row);
        }
    }
    for (table, rows) in by_table {
        let columns: Vec<String> = rows
            .iter()
            .flat_map(|row| row.keys().cloned())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let values: Vec<Vec<libsql::Value>> = rows
            .iter()
            .map(|row| {
                columns
                    .iter()
                    .map(|column| row.get(column).map(value_of).unwrap_or(libsql::Value::Null))
                    .collect()
            })
            .collect();
        into.rebuild_replace_table(&table, &columns, &values)
            .await?;
        rebuilt.tables.insert(table, values.len());
    }
    Ok(rebuilt)
}

/// The stored value a cell of an export file stands for: the inverse of
/// the export's `cell`.
pub(crate) fn value_of(cell: &Value) -> libsql::Value {
    match cell {
        Value::Null => libsql::Value::Null,
        Value::Bool(flag) => libsql::Value::Integer(i64::from(*flag)),
        Value::Number(number) => match number.as_i64() {
            Some(n) if !number.is_f64() => libsql::Value::Integer(n),
            _ => libsql::Value::Real(number.as_f64().unwrap_or(0.0)),
        },
        Value::String(text) => libsql::Value::Text(text.clone()),
        Value::Object(object) => {
            if let Some(hex) = object.get("hex").and_then(Value::as_str) {
                let bytes = (0..hex.len() / 2)
                    .filter_map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok())
                    .collect();
                libsql::Value::Blob(bytes)
            } else if let Some(real) = object.get("real").and_then(Value::as_str) {
                libsql::Value::Real(real.parse().unwrap_or(f64::NAN))
            } else {
                libsql::Value::Text(Value::Object(object.clone()).to_string())
            }
        }
        Value::Array(_) => libsql::Value::Text(cell.to_string()),
    }
}

#[cfg(test)]
#[path = "rebuild_test.rs"]
mod tests;
