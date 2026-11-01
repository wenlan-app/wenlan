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
