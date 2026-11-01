// SPDX-License-Identifier: Apache-2.0
//! The whole lossless export in one call, in the order each part needs:
//! pages first (memories link to entity files, canvases sit beside page
//! files), then everything else, then the manifest last.
//!
//! The palace layout (`<data root>/layout/`) is already a file and is
//! copied as it is into `backstage/layout/`.

use std::path::Path;

use super::{
    artifacts, briefs, canvases, history, memories, pages, records, settings, ExportDirs, Manifest,
};
use crate::db::MemoryDB;
use crate::error::WenlanError;

/// What one export wrote, for the report.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LosslessSummary {
    pub pages: usize,
    pub archived: usize,
    pub entities: usize,
    pub memories: usize,
    pub originals: usize,
    pub briefs: usize,
    pub canvases: usize,
    pub hidden_canvases: usize,
    pub artifacts: usize,
    pub layout_files: usize,
    pub settings_rows: usize,
    pub record_rows: usize,
    pub history_entries: usize,
    pub history_reads: usize,
    pub files: usize,
    pub left_out: usize,
}

/// Exports `db` into `out`, which must not exist or be empty. `page_files`
/// maps page ids to their file in today's pages folder; `data_root` is the
/// folder that holds `memorydb/`, artifact originals and `layout/`.
pub async fn export_lossless(
    db: &MemoryDB,
    page_files: &std::collections::HashMap<String, String>,
    data_root: &Path,
    out: &Path,
) -> Result<LosslessSummary, WenlanError> {
    let dirs = ExportDirs::create(out)?;
    let mut manifest = Manifest::new();
    let mut summary = LosslessSummary::default();

    let done = pages::export_pages(db, page_files, &dirs, &mut manifest).await?;
    summary.pages = done.pages;
    summary.archived = done.archived;
    summary.entities = done.entities;
    let kept = memories::export_memories(db, &done.entity_stems, &dirs, &mut manifest).await?;
    summary.memories = kept.memories;
    summary.originals = kept.originals;
    summary.briefs = briefs::export_briefs(db, &dirs, &mut manifest).await?;
    let drawn = canvases::export_canvases(db, &done.page_paths, &dirs, &mut manifest).await?;
    summary.canvases = drawn.canvases;
    summary.hidden_canvases = drawn.hidden;
    summary.artifacts = artifacts::export_artifacts(db, data_root, &dirs, &mut manifest).await?;
    summary.layout_files = copy_layout(&data_root.join("layout"), "", &dirs, &mut manifest)?;
    summary.settings_rows = settings::export_settings(db, &dirs, &mut manifest).await?;
    summary.record_rows = records::export_records(db, &dirs, &mut manifest).await?;
    let logged = history::export_history(db, &dirs, &mut manifest).await?;
    summary.history_entries = logged.entries;
    summary.history_reads = logged.reads;

    summary.files = manifest.files.len();
    summary.left_out = manifest.left_out.len();
    dirs.write_manifest(&manifest)?;
    Ok(summary)
}

/// Copies every file under `dir` to `backstage/layout/<relative>`; returns
/// how many. A missing folder copies nothing.
fn copy_layout(
    dir: &Path,
    relative: &str,
    dirs: &ExportDirs,
    manifest: &mut Manifest,
) -> Result<usize, WenlanError> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Ok(0);
    };
    let mut entries: Vec<_> = read.collect::<Result<_, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    let mut copied = 0;
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        let inner = if relative.is_empty() {
            name.clone()
        } else {
            format!("{relative}/{name}")
        };
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copied += copy_layout(&entry.path(), &inner, dirs, manifest)?;
        } else if kind.is_file() {
            let bytes = std::fs::read(entry.path())?;
            manifest.files.push(dirs.write_bytes(
                &format!("backstage/layout/{inner}"),
                &inner,
                "layout",
                &bytes,
            )?);
            copied += 1;
        }
    }
    Ok(copied)
}
