// SPDX-License-Identifier: Apache-2.0
//! Storage migration step 4: when the database is gone, the daemon builds
//! it again from the file copy ([`super::mirror`]) before it opens it.
//!
//! The rebuild ([`super::rebuild::rebuild`]) writes a new database in
//! `<data root>/files-mirror/restore/`. A byte copy of it, folded and
//! checked (`online_backup`), is then renamed into the database folder, so
//! the database file appears whole or not at all. What the files do not
//! hold is cache: page vectors are computed here, memory vectors by the
//! daemon after it serves.
//!
//! Only a missing database file starts this. An existing database is
//! never touched, and the files are only read.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use crate::db::{MemoryDB, MemoryEmbeddingRecovery};
use crate::error::WenlanError;
use crate::events::NoopEmitter;
use crate::store::export::MANIFEST_FILE;
use crate::store::mirror::{mirror_root, FILES_DIR};
use crate::store::rebuild::rebuild;

/// The database file under the database folder (mirrors `MemoryDB`).
pub const DATABASE_FILE: &str = "origin_memory.db";
/// Work folder under the mirror folder.
pub const RESTORE_DIR: &str = "restore";
/// Row counts and problems of the last restore.
pub const RESTORE_REPORT: &str = "restore-report.txt";
/// Where the files are kept when a restore fails, so the empty database
/// the daemon then starts with cannot overwrite them.
pub const KEPT_PREFIX: &str = "files-kept-";
/// Where the files a restore read are kept: a file the rebuild could not
/// read would otherwise be removed by the next refresh.
pub const RESTORED_PREFIX: &str = "files-restored-";

/// What [`restore_missing_database`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Restore {
    /// The database file exists: nothing to do.
    NotNeeded,
    /// No database and no file copy: the daemon starts empty, as before.
    NoFiles,
    /// The database was built again from the files.
    Restored {
        rows: usize,
        problems: Vec<String>,
        ms: u64,
        /// Where the files read now are, or `None` when they could not be
        /// moved aside (then the daemon must not refresh over them).
        set_aside: Option<PathBuf>,
    },
}

/// The file copy a restore reads: the latest one with a manifest. An older
/// build swapped `files.old` in place of `files`, so a crash mid-swap can
/// leave only that one.
pub fn restore_source(data_root: &Path) -> Option<PathBuf> {
    let mirror = mirror_root(data_root);
    [mirror.join(FILES_DIR), mirror.join("files.old")]
        .into_iter()
        .find(|dir| dir.join(MANIFEST_FILE).is_file())
}

/// Builds `<db_dir>/origin_memory.db` from the file copy under
/// `data_root` when it is missing or empty (0 bytes: SQLite would open it
/// as a new database). On success the new file is in place and the files
/// it was built from are moved to `files-restored-<now>`; on failure
/// nothing in `db_dir` changed but an empty file moved aside.
pub async fn restore_missing_database(
    data_root: &Path,
    db_dir: &Path,
    now: i64,
) -> Result<Restore, WenlanError> {
    let db_file = db_dir.join(DATABASE_FILE);
    let empty = std::fs::metadata(&db_file).is_ok_and(|meta| meta.is_file() && meta.len() == 0);
    if db_file.exists() && !empty {
        return Ok(Restore::NotNeeded);
    }
    let Some(from) = restore_source(data_root) else {
        return Ok(Restore::NoFiles);
    };
    let started = Instant::now();
    let work = mirror_root(data_root).join(RESTORE_DIR);
    // Left by an earlier restore that stopped part way: ours, never read.
    if work.exists() {
        std::fs::remove_dir_all(&work)?;
    }
    let rebuilt_dir = work.join("rebuilt");
    std::fs::create_dir_all(&rebuilt_dir)?;
    if empty {
        std::fs::rename(&db_file, work.join("empty.db"))?;
    }
    let fresh = MemoryDB::new_with_embedding_recovery(
        &rebuilt_dir,
        Arc::new(NoopEmitter),
        MemoryEmbeddingRecovery::Deferred,
    )
    .await?;
    let rebuilt = rebuild(&from, &fresh).await?;
    if let Err(error) = fresh.backfill_page_embeddings().await {
        // Cache: the daemon embeds pages again once its model loads.
        log::warn!("[restore] page vectors not computed: {error}");
    }
    let copy = work.join(DATABASE_FILE);
    let receipt = fresh.online_backup(&copy).await?;
    drop(fresh);
    if !receipt.integrity_ok {
        return Err(WenlanError::Validation(
            "the rebuilt database failed its integrity check".to_string(),
        ));
    }
    std::fs::create_dir_all(db_dir)?;
    // The rebuild took a while: if a database appeared meanwhile, it wins
    // and the rebuilt copy stays in the work folder.
    if db_file.exists() {
        log::warn!("[restore] a database appeared during the rebuild; it is kept");
        return Ok(Restore::NotNeeded);
    }
    // A log or shared-memory file with no database is from a database that
    // is gone; next to the new file it would be read as its log.
    for suffix in ["-wal", "-shm"] {
        let stray = db_dir.join(format!("{DATABASE_FILE}{suffix}"));
        if stray.exists() {
            std::fs::rename(&stray, work.join(format!("stray{suffix}")))?;
        }
    }
    std::fs::rename(&copy, &db_file)?;
    let rows = rebuilt.tables.values().sum();
    let ms = started.elapsed().as_millis() as u64;
    let report = report_text(&from, &rebuilt.tables, &rebuilt.problems, ms);
    let _ = std::fs::write(mirror_root(data_root).join(RESTORE_REPORT), report);
    let _ = std::fs::remove_dir_all(rebuilt_dir);
    let aside = mirror_root(data_root).join(format!("{RESTORED_PREFIX}{now}"));
    let set_aside = std::fs::rename(&from, &aside).ok().map(|_| aside);
    Ok(Restore::Restored {
        rows,
        problems: rebuilt.problems,
        ms,
        set_aside,
    })
}

/// After a failed restore: moves the file copy aside, so the daemon's next
/// refresh from the empty database it starts with cannot replace it.
/// Returns where it went.
pub fn keep_files_after_failure(data_root: &Path, now: i64) -> Option<PathBuf> {
    let from = restore_source(data_root)?;
    let kept = mirror_root(data_root).join(format!("{KEPT_PREFIX}{now}"));
    std::fs::rename(&from, &kept).ok()?;
    Some(kept)
}

fn report_text(
    from: &Path,
    tables: &std::collections::BTreeMap<String, usize>,
    problems: &[String],
    ms: u64,
) -> String {
    let mut out = format!("Rebuilt the database from {} in {ms} ms.\n", from.display());
    for (table, rows) in tables {
        out.push_str(&format!("  {table}: {rows} rows\n"));
    }
    out.push_str(&format!("{} problems\n", problems.len()));
    for problem in problems.iter().take(50) {
        out.push_str(&format!("  {problem}\n"));
    }
    out
}

#[cfg(test)]
#[path = "restore_test.rs"]
mod restore_test;
