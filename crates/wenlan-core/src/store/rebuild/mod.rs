// SPDX-License-Identifier: Apache-2.0
//! The rebuild (storage migration step 2, spec section 10): a lossless
//! export read back into a fresh database, so [`compare`] can check it
//! field by field against the database it came from. Only ever writes the
//! fresh database it is given.
//!
//! So far: the tables copied whole (`settings/`, `records/`), every
//! history table, replayed from the log, and pages and entities from their
//! files ([`pages::rebuild_pages`]), memories and originals
//! ([`memories::rebuild_memories`]), Briefs ([`briefs::rebuild_briefs`]) and
//! canvases ([`canvases::rebuild_canvases`]). [`rebuild`] runs every part
//! in order; [`export_again`] and [`compare_exports`] check the result.

pub mod briefs;
pub mod canvases;
pub mod check;
pub mod compare;
pub mod memories;
pub mod pages;

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
            if data.get("state_of").is_some() {
                // Applied by `apply_states`, once the files' rows exist.
                continue;
            }
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

/// Rebuilds every part the export folder `from` holds into `into` (a
/// fresh database): the file rows, then the state seeds on them, then the
/// whole tables and history.
pub async fn rebuild(from: &Path, into: &MemoryDB) -> Result<Rebuilt, WenlanError> {
    let manifest = read_manifest(from)?;
    if manifest.format != crate::store::export::EXPORT_FORMAT {
        return Err(WenlanError::Validation(format!(
            "export format {} is not {}",
            manifest.format,
            crate::store::export::EXPORT_FORMAT
        )));
    }
    if !into.rebuild_target_is_empty().await? {
        return Err(WenlanError::Validation(
            "the rebuild writes only into an empty database".to_string(),
        ));
    }
    // Tables are written one at a time, so a row can name a row of a table
    // written later (records name pages, a canvas node its parent). Keys are
    // checked once everything is in.
    into.rebuild_foreign_keys(false).await?;
    let written = rebuild_parts(from, &manifest, into).await;
    let checked = into.rebuild_foreign_key_problems().await;
    into.rebuild_foreign_keys(true).await?;
    let mut rebuilt = written?;
    rebuilt.problems.extend(checked?);
    Ok(rebuilt)
}

async fn rebuild_parts(
    from: &Path,
    manifest: &crate::store::export::Manifest,
    into: &MemoryDB,
) -> Result<Rebuilt, WenlanError> {
    let mut problems = Vec::new();
    let pages = pages::rebuild_pages(from, manifest, into, &mut problems).await?;
    let memories = memories::rebuild_memories(from, manifest, into, &mut problems).await?;
    let (briefs, items) = briefs::rebuild_briefs(from, manifest, into, &mut problems).await?;
    let canvases = canvases::rebuild_canvases(from, manifest, into, &mut problems).await?;
    let (_, unset) = apply_states(from, into).await?;
    problems.extend(unset);
    // Whole tables last. Setting the state seeds edits page rows, and the
    // database's triggers answer an edit (a `supported` verdict is demoted,
    // a derivation queued); the tables copied whole must end as exported.
    let mut rebuilt = rebuild_tables(from, into).await?;
    rebuilt.tables.insert("briefs".to_string(), briefs);
    rebuilt.tables.insert("brief_items".to_string(), items);
    rebuilt.tables.insert("page_maps".to_string(), canvases);
    rebuilt.tables.insert("pages".to_string(), pages);
    rebuilt.tables.insert("memories".to_string(), memories.rows);
    rebuilt
        .tables
        .insert("memory_entities".to_string(), memories.links);
    problems.append(&mut rebuilt.problems);
    rebuilt.problems = problems;
    Ok(rebuilt)
}

/// Files of two exports that differ, by path. History files are left out:
/// an entry with no stored time is stamped with each export's own time, so
/// the log is compared row by row instead ([`compare::compare_tables`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct ExportDiff {
    pub only_in_first: Vec<String>,
    pub only_in_second: Vec<String>,
    pub differ: Vec<String>,
}

impl ExportDiff {
    pub fn is_empty(&self) -> bool {
        self.only_in_first.is_empty() && self.only_in_second.is_empty() && self.differ.is_empty()
    }
}

/// File types the second export cannot have: the log (see [`ExportDiff`])
/// and copies of a pages folder the rebuilt database never had.
const NOT_COMPARED: &[&str] = &["history", "pages_folder_only", "pages_folder_edited"];

fn read_manifest(from: &Path) -> Result<crate::store::export::Manifest, WenlanError> {
    let text = std::fs::read_to_string(from.join(crate::store::export::MANIFEST_FILE))?;
    serde_json::from_str(&text)
        .map_err(|error| WenlanError::Validation(format!("export manifest: {error}")))
}

/// Exports `rebuilt` into `out` the way `first` was laid out: each page in
/// the folder `first` put it, with the layout and artifact files `first`
/// copied. With [`compare_exports`], the fixpoint check:
/// export(rebuild(export(db))) == export(db).
pub async fn export_again(
    rebuilt: &MemoryDB,
    first: &Path,
    out: &Path,
) -> Result<crate::store::export::run::LosslessSummary, WenlanError> {
    let manifest = read_manifest(first)?;
    let mut recorded = std::collections::HashMap::new();
    for file in &manifest.files {
        let path = match file.file_type.as_str() {
            "page" => file.path.strip_prefix("visible/").map(str::to_string),
            "archived_page" => file
                .path
                .strip_prefix("backstage/archive/")
                .map(|rest| format!("archive/{rest}")),
            _ => None,
        };
        if let Some(path) = path {
            recorded.insert(file.id.clone(), path);
        }
    }
    // A data folder holding only what the first export copied from one:
    // the palace layout and artifact files. (`backstage/layout/` also holds
    // canvases the export wrote itself, which must not be copied again.)
    let data_root = tempfile::tempdir()?;
    for file in &manifest.files {
        let inner = match file.file_type.as_str() {
            "layout" => file
                .path
                .strip_prefix("backstage/layout/")
                .map(|rest| format!("layout/{rest}")),
            "artifact" => file.path.strip_prefix("backstage/").map(str::to_string),
            _ => None,
        };
        if let Some(inner) = inner {
            let target = data_root.path().join(&inner);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(first.join(&file.path), target)?;
        }
    }
    let folder = crate::store::export::run::PagesFolder::recorded(recorded);
    crate::store::export::run::export_lossless(rebuilt, &folder, data_root.path(), out).await
}

/// Compares two exports' manifests file by file (path and digest).
pub fn compare_exports(first: &Path, second: &Path) -> Result<ExportDiff, WenlanError> {
    let files = |root: &Path| -> Result<BTreeMap<String, String>, WenlanError> {
        Ok(read_manifest(root)?
            .files
            .into_iter()
            .filter(|file| !NOT_COMPARED.contains(&file.file_type.as_str()))
            .map(|file| (file.path, file.sha256))
            .collect())
    };
    let a = files(first)?;
    let mut b = files(second)?;
    let mut diff = ExportDiff::default();
    for (path, digest) in a {
        match b.remove(&path) {
            None => diff.only_in_first.push(path),
            Some(other) if other != digest => diff.differ.push(path),
            Some(_) => {}
        }
    }
    diff.only_in_second = b.into_keys().collect();
    Ok(diff)
}

/// Applies the `state.seed` entries of the main log: the columns the files
/// leave to history, set on the rows the files rebuilt. Returns how many
/// rows were set; a key that matches no row is a problem.
pub async fn apply_states(
    from: &Path,
    into: &MemoryDB,
) -> Result<(usize, Vec<String>), WenlanError> {
    let log = from.join("backstage").join("history");
    if !log.is_dir() {
        return Ok((0, Vec::new()));
    }
    let replay = history::replay(&log)?;
    let mut set = 0;
    let mut problems = Vec::new();
    for entry in &replay.entries {
        let Some(data) = &entry.data else {
            continue;
        };
        let Some(table) = data.get("state_of").and_then(Value::as_str) else {
            continue;
        };
        let pairs = |value: Option<&Value>| -> Vec<(String, libsql::Value)> {
            value
                .and_then(Value::as_object)
                .map(|object| {
                    object
                        .iter()
                        .map(|(column, cell)| (column.clone(), value_of(cell)))
                        .collect()
                })
                .unwrap_or_default()
        };
        let key = pairs(data.get("key"));
        let columns = pairs(data.get("columns"));
        let changed = into.rebuild_update_rows(table, &key, &columns).await?;
        if changed == 0 && !columns.is_empty() {
            problems.push(format!("{table} {:?}: no row to set", data.get("key")));
        }
        set += usize::from(changed > 0);
    }
    Ok((set, problems))
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
