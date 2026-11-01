// SPDX-License-Identifier: Apache-2.0
//! Settings as files (spec section 3, "Settings"): `backstage/settings/`.
//!
//! | File              | Tables                       |
//! |-------------------|------------------------------|
//! | `spaces.json`     | `spaces`                     |
//! | `things.json`     | `things`, `thing_sessions`   |
//! | `agents.json`     | `agent_connections`          |
//! | `profile.json`    | `profiles`                   |
//! | `onboarding.json` | `onboarding_milestones`      |
//! | `metadata.json`   | `app_metadata`               |
//!
//! Each table is copied whole, every column and row as stored, so a column a
//! later migration adds is carried without a change here:
//! `{"tables": {"<table>": {"columns": [...], "rows": [[...], ...]}}}`.
//! Rows are in primary key order. A number keeps whether it was stored as an
//! integer or a real; bytes are written as `{"hex": "..."}`. The history of
//! things (`thing_history`) goes with the history export.

use serde_json::{json, Map, Value};

use super::{ExportDirs, Manifest};
use crate::db::export_reads::ExportTable;
use crate::db::MemoryDB;
use crate::error::WenlanError;

/// Each settings file and the tables it holds.
pub const SETTINGS_FILES: &[(&str, &[&str])] = &[
    ("spaces", &["spaces"]),
    ("things", &["things", "thing_sessions"]),
    ("agents", &["agent_connections"]),
    ("profile", &["profiles"]),
    ("onboarding", &["onboarding_milestones"]),
    ("metadata", &["app_metadata"]),
];

/// Writes the settings files; returns how many rows they hold.
pub async fn export_settings(
    db: &MemoryDB,
    dirs: &ExportDirs,
    manifest: &mut Manifest,
) -> Result<usize, WenlanError> {
    let mut rows = 0;
    for (file, tables) in SETTINGS_FILES {
        rows += write_tables(
            db,
            dirs,
            manifest,
            &format!("backstage/settings/{file}.json"),
            file,
            "settings",
            tables,
        )
        .await?;
    }
    Ok(rows)
}

/// Writes `tables`, each copied whole, as one JSON file at `path`; returns
/// how many rows it holds. A table this database does not have is left out
/// of the file.
pub(super) async fn write_tables(
    db: &MemoryDB,
    dirs: &ExportDirs,
    manifest: &mut Manifest,
    path: &str,
    id: &str,
    file_type: &str,
    tables: &[&'static str],
) -> Result<usize, WenlanError> {
    let mut rows = 0;
    let mut held = Map::new();
    for table in tables {
        let Some(copied) = db.export_table(table).await? else {
            continue;
        };
        rows += copied.rows.len();
        held.insert(table.to_string(), table_value(&copied));
    }
    let value = json!({ "tables": held });
    let text = serde_json::to_string_pretty(&value)
        .map_err(|error| WenlanError::Validation(format!("{path}: {error}")))?
        + "\n";
    let reads_back = serde_json::from_str::<Value>(&text).ok() == Some(value);
    super::memories::check(manifest, id, reads_back);
    manifest.files.push(dirs.write(path, id, file_type, &text)?);
    Ok(rows)
}

pub(super) fn table_value(table: &ExportTable) -> Value {
    json!({
        "columns": table.columns,
        "rows": table
            .rows
            .iter()
            .map(|row| Value::Array(row.iter().map(cell).collect()))
            .collect::<Vec<_>>(),
    })
}

pub(crate) fn cell(value: &libsql::Value) -> Value {
    match value {
        libsql::Value::Null => Value::Null,
        libsql::Value::Integer(number) => json!(number),
        libsql::Value::Real(number) => serde_json::Number::from_f64(*number)
            .map(Value::Number)
            .unwrap_or_else(|| json!({ "real": number.to_string() })),
        libsql::Value::Text(text) => json!(text),
        libsql::Value::Blob(bytes) => {
            let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
            json!({ "hex": hex })
        }
    }
}
