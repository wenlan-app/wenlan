// SPDX-License-Identifier: Apache-2.0
//! The whole lossless export in one call, in the order each part needs:
//! pages first (memories link to entity files, canvases sit beside page
//! files), then everything else, then the manifest last.
//!
//! The palace layout (`<data root>/layout/`) is already a file and is
//! copied as it is into `backstage/layout/`.
//!
//! Today's pages folder is only read. A page `state.json` forgot is found by
//! the `origin_id` in its file; a file no page row claims (a page the
//! database no longer has, or a file someone put there) is copied as it is
//! into `backstage/pages-folder/`, so nothing in that folder is lost.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};

use super::{
    artifacts, briefs, canvases, history, memories, pages, records, settings, ExportDirs, Manifest,
    NotFiles,
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
    /// Pages placed by the `origin_id` in their file, not by `state.json`.
    pub found_by_id: usize,
    /// Files in the pages folder no page row claims, copied as they are.
    pub folder_only: usize,
    /// `state.json` entries for pages the database no longer has.
    pub stale_records: usize,
}

/// Today's pages folder: the folder (read only, never written) and what its
/// `state.json` records, page id to file.
#[derive(Debug, Clone, Default)]
pub struct PagesFolder {
    pub dir: Option<PathBuf>,
    pub recorded: HashMap<String, String>,
}

impl PagesFolder {
    /// Only recorded paths, no folder to read.
    pub fn recorded(recorded: HashMap<String, String>) -> Self {
        Self {
            dir: None,
            recorded,
        }
    }
}

/// Exports `db` into `out`, which must not exist or be empty. `folder` is
/// today's pages folder; `data_root` is the folder that holds `memorydb/`,
/// artifact originals and `layout/`.
pub async fn export_lossless(
    db: &MemoryDB,
    folder: &PagesFolder,
    data_root: &Path,
    out: &Path,
) -> Result<LosslessSummary, WenlanError> {
    let dirs = ExportDirs::create(out)?;
    let mut manifest = Manifest::new();
    let mut summary = LosslessSummary::default();

    let on_disk = match &folder.dir {
        Some(dir) => folder_files(dir, "")?,
        None => Vec::new(),
    };
    let mut placement = folder.recorded.clone();
    let recorded: HashSet<&String> = folder.recorded.values().collect();
    let mut found_by_id = HashSet::new();
    if let Some(dir) = &folder.dir {
        for path in on_disk.iter().filter(|path| path.ends_with(".md")) {
            if recorded.contains(path) {
                continue;
            }
            if let Some(id) = origin_id(&dir.join(path)) {
                if !placement.contains_key(&id) {
                    placement.insert(id.clone(), path.clone());
                    found_by_id.insert(id);
                }
            }
        }
    }

    let done = pages::export_pages(db, &placement, &dirs, &mut manifest).await?;
    summary.pages = done.pages;
    summary.archived = done.archived;
    summary.entities = done.entities;
    summary.found_by_id = found_by_id
        .iter()
        .filter(|id| done.row_ids.contains(*id))
        .count();
    summary.stale_records = folder
        .recorded
        .keys()
        .filter(|id| !done.row_ids.contains(*id))
        .count();
    if summary.stale_records > 0 {
        manifest.not_files.push(NotFiles {
            what: "pages-folder records of pages the database no longer has".to_string(),
            count: summary.stale_records,
            why: "state.json still lists them; any file they name is copied below".to_string(),
        });
    }
    let claimed: HashSet<&String> = placement
        .iter()
        .filter(|(id, _)| done.row_ids.contains(*id))
        .map(|(_, path)| path)
        .collect();
    if let Some(dir) = &folder.dir {
        for path in on_disk.iter().filter(|path| !claimed.contains(path)) {
            let bytes = std::fs::read(dir.join(path))?;
            manifest.files.push(dirs.write_bytes(
                &format!("backstage/pages-folder/{path}"),
                path,
                "pages_folder_only",
                &bytes,
            )?);
            summary.folder_only += 1;
        }
    }
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

/// Every regular file under `dir`, relative with forward slashes, sorted.
/// Dot files and folders (`.wenlan/`, `.obsidian/`, `.trash/`) and symlinks
/// are skipped; nothing is followed or written.
fn folder_files(dir: &Path, relative: &str) -> Result<Vec<String>, WenlanError> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Ok(Vec::new());
    };
    let mut entries: Vec<_> = read.collect::<Result<_, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    let mut found = Vec::new();
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let inner = if relative.is_empty() {
            name
        } else {
            format!("{relative}/{name}")
        };
        let kind = entry.file_type()?;
        if kind.is_dir() {
            found.extend(folder_files(&entry.path(), &inner)?);
        } else if kind.is_file() {
            found.push(inner);
        }
    }
    Ok(found)
}

/// The `origin_id:` in a file's frontmatter, read from its first 8 KiB.
fn origin_id(path: &Path) -> Option<String> {
    let mut head = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(8 * 1024)
        .read_to_end(&mut head)
        .ok()?;
    let head = String::from_utf8_lossy(&head);
    let mut lines = head.lines();
    if lines.next() != Some("---") {
        return None;
    }
    lines
        .take_while(|line| *line != "---")
        .find_map(|line| line.strip_prefix("origin_id:"))
        .map(|id| id.trim().trim_matches('"').to_string())
        .filter(|id| !id.is_empty())
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
