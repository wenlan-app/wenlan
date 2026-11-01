// SPDX-License-Identifier: Apache-2.0
//! The whole step-2 check in one call: rebuild an export into a fresh
//! database, export that again, and compare. Nothing but `work` (a new
//! folder) is written; the source database and the export are only read.
//!
//! What it compares:
//! - every file of the two exports, by path and digest (the fixpoint), so a
//!   value the rebuild lost shows as a file that differs;
//! - `pages` column by column (cache columns skipped), so a history column
//!   the state seeds did not restore shows by name;
//! - Briefs, canvases, settings, records and every history table row for row.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use serde::Serialize;

use super::compare::{compare_keyed, compare_tables, KeyedDiff};
use super::{compare_exports, export_again, rebuild, ExportDiff};
use crate::db::MemoryDB;
use crate::error::WenlanError;
use crate::events::NoopEmitter;
use crate::store::export::history::HISTORY_TABLES;
use crate::store::export::records::RECORD_TABLES;
use crate::store::export::settings::SETTINGS_FILES;
use crate::store::export::DEFERRED_COLUMNS;

/// Tables compared row for row besides settings, records and history.
const WHOLE_ROWS: &[&str] = &[
    "briefs",
    "brief_items",
    "page_maps",
    "page_map_nodes",
    "page_map_edges",
];

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct TableCount {
    pub table: String,
    pub only_in_source: usize,
    pub only_in_rebuilt: usize,
}

/// A `pages` column that differs, split by whether the export's manifest
/// already lists the value as left out (expected) or not (a loss).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ColumnCount {
    pub column: String,
    pub listed: usize,
    pub unlisted: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CheckReport {
    /// Rows the rebuild wrote, by table.
    pub rebuilt: BTreeMap<String, usize>,
    /// Files the rebuild could not read, keys it could not set.
    pub problems: Vec<String>,
    pub files: ExportDiff,
    pub pages: KeyedDiff,
    pub page_columns: Vec<ColumnCount>,
    /// Tables whose rows differ; equal tables are left out.
    pub tables: Vec<TableCount>,
    pub tables_compared: usize,
}

impl CheckReport {
    pub fn is_clean(&self) -> bool {
        self.problems.is_empty()
            && self.files.is_empty()
            && self.pages.only_in_source.is_empty()
            && self.pages.only_in_rebuilt.is_empty()
            && self.page_columns.iter().all(|c| c.unlisted.is_empty())
            && self.tables.is_empty()
    }

    /// Counts and paths only, never row content, so it can be shared.
    pub fn summary(&self) -> String {
        let mut out = String::new();
        let verdict = if self.is_clean() {
            "CLEAN: the rebuild matches the source"
        } else {
            "DIFFERENCES FOUND"
        };
        out.push_str(&format!("{verdict}\n\n"));
        out.push_str("Rebuilt rows:\n");
        for (table, rows) in &self.rebuilt {
            out.push_str(&format!("  {table}: {rows}\n"));
        }
        out.push_str(&format!("\nProblems: {}\n", self.problems.len()));
        out.push_str(&format!(
            "Files: {} differ, {} only in the first export, {} only in the second\n",
            self.files.differ.len(),
            self.files.only_in_first.len(),
            self.files.only_in_second.len()
        ));
        for path in self.files.differ.iter().take(20) {
            out.push_str(&format!("  differs: {path}\n"));
        }
        out.push_str(&format!(
            "Pages: {} rows, {} only in source, {} only in rebuild\n",
            self.pages.rows,
            self.pages.only_in_source.len(),
            self.pages.only_in_rebuilt.len()
        ));
        for column in &self.page_columns {
            out.push_str(&format!(
                "  column {}: {} rows differ that are not listed as left out, {} listed\n",
                column.column,
                column.unlisted.len(),
                column.listed
            ));
        }
        out.push_str(&format!(
            "Tables: {} compared, {} differ\n",
            self.tables_compared,
            self.tables.len()
        ));
        for table in &self.tables {
            out.push_str(&format!(
                "  {}: {} rows only in source, {} only in rebuild\n",
                table.table, table.only_in_source, table.only_in_rebuilt
            ));
        }
        out
    }
}

/// Rebuilds `export` into a new database under `work/rebuilt/`, exports it
/// into `work/second/`, and compares with `source`, the database `export`
/// was written from. `work` must not exist or be empty.
pub async fn check_export(
    source: &MemoryDB,
    export: &Path,
    work: &Path,
) -> Result<CheckReport, WenlanError> {
    crate::store::export::ExportDirs::create(work)?;
    let fresh_dir = work.join("rebuilt");
    std::fs::create_dir_all(&fresh_dir)?;
    let fresh = MemoryDB::new(&fresh_dir, Arc::new(NoopEmitter)).await?;
    let rebuilt = rebuild(export, &fresh).await?;
    export_again(&fresh, export, &work.join("second")).await?;
    let files = compare_exports(export, &work.join("second"))?;

    let cache: Vec<&str> = DEFERRED_COLUMNS
        .iter()
        .filter(|(table, to, _)| *table == "pages" && *to == "cache")
        .flat_map(|(_, _, columns)| columns.iter().copied())
        .collect();
    let pages = compare_keyed(source, &fresh, "pages", &["id"], &cache).await?;
    let manifest: crate::store::export::Manifest = serde_json::from_str(&std::fs::read_to_string(
        export.join(crate::store::export::MANIFEST_FILE),
    )?)
    .map_err(|error| WenlanError::Validation(format!("export manifest: {error}")))?;
    let listed = |key: &str, column: &str| {
        manifest
            .left_out
            .iter()
            .any(|left| left.id == key && left.field.starts_with(column))
    };
    let page_columns = pages
        .columns
        .iter()
        .map(|(column, diff)| {
            let (known, unlisted): (Vec<&String>, Vec<&String>) =
                diff.keys.iter().partition(|key| listed(key, column));
            ColumnCount {
                column: column.clone(),
                listed: known.len(),
                unlisted: unlisted.into_iter().cloned().collect(),
            }
        })
        .collect();

    let mut tables: Vec<&'static str> = WHOLE_ROWS.to_vec();
    tables.extend(
        SETTINGS_FILES
            .iter()
            .flat_map(|(_, tables)| tables.iter().copied()),
    );
    tables.extend(RECORD_TABLES.iter().copied());
    tables.extend(HISTORY_TABLES.iter().map(|spec| spec.table));
    let tables_compared = tables.len();
    let tables = compare_tables(source, &fresh, &tables)
        .await?
        .into_iter()
        .map(|diff| TableCount {
            table: diff.table,
            only_in_source: diff.only_in_source.len(),
            only_in_rebuilt: diff.only_in_rebuilt.len(),
        })
        .collect();
    Ok(CheckReport {
        rebuilt: rebuilt.tables,
        problems: rebuilt.problems,
        files,
        pages,
        page_columns,
        tables,
        tables_compared,
    })
}
