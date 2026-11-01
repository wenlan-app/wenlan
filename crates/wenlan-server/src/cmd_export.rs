// SPDX-License-Identifier: Apache-2.0
//! `export-lossless` internal CLI subcommand: storage migration step 2.
//!
//! Writes a copy of a data root's database out as the Markdown, JSON Canvas
//! and history files it will become (`wenlan_core::store::export`), into a
//! new folder, with `export-manifest.json` listing every file and every
//! value the files do not hold.
//!
//! It only ever runs on a copy: `--data-dir` (or `WENLAN_DATA_DIR`) must
//! name a data root other than the installed app's, so the person's live
//! database is never opened by it. Opening the copy runs this build's
//! migrations on the copy. Page placement comes from the pages folder's own
//! `state.json`, which is only read.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use wenlan_core::db::MemoryDB;
use wenlan_core::events::NoopEmitter;
use wenlan_core::export::knowledge::KnowledgeWriter;
use wenlan_core::store::export::run::export_lossless;

fn installed_root() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("wenlan")
}

fn same_folder(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

pub async fn run(out: PathBuf, pages: Option<PathBuf>) -> Result<()> {
    let Some(root) = std::env::var_os("WENLAN_DATA_DIR").map(PathBuf::from) else {
        bail!(
            "export-lossless runs on a copy of the data folder only. Copy it first, then pass \
             --data-dir <copy>."
        );
    };
    if same_folder(&root, &installed_root()) {
        bail!(
            "{} is the installed app's data folder. Copy it and pass --data-dir <copy>.",
            root.display()
        );
    }
    let _lock = super::DaemonDataLock::acquire(&root, true)?;
    let data_dir = root.join("memorydb");
    let db = MemoryDB::new(&data_dir, Arc::new(NoopEmitter))
        .await
        .with_context(|| format!("opening MemoryDB at {}", data_dir.display()))?;

    let pages =
        pages.unwrap_or_else(|| wenlan_core::config::load_config().knowledge_path_or_default());
    let page_files = KnowledgeWriter::new(pages.clone(), &db).page_files();
    println!(
        "Pages folder: {} ({} page files recorded)",
        pages.display(),
        page_files.len()
    );

    let summary = export_lossless(&db, &page_files, &root, &out)
        .await
        .with_context(|| format!("exporting into {}", out.display()))?;
    println!("Exported into {}:", out.display());
    println!(
        "  pages {} (archived {}), entities {}, memories {}, originals {}, briefs {}",
        summary.pages,
        summary.archived,
        summary.entities,
        summary.memories,
        summary.originals,
        summary.briefs
    );
    println!(
        "  canvases {} (+{} hidden), artifacts {}, layout files {}",
        summary.canvases, summary.hidden_canvases, summary.artifacts, summary.layout_files
    );
    println!(
        "  settings rows {}, record rows {}, history entries {} (+{} reads)",
        summary.settings_rows, summary.record_rows, summary.history_entries, summary.history_reads
    );
    println!(
        "  {} files; {} values left out, listed in export-manifest.json",
        summary.files, summary.left_out
    );
    Ok(())
}
