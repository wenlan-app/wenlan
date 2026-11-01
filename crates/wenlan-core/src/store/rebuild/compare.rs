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
