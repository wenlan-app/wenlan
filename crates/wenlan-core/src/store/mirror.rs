// SPDX-License-Identifier: Apache-2.0
//! Storage migration steps 3 and 4: keep a file copy of the database
//! current, and check it every day.
//!
//! The database is still where a write lands. Shortly after it changes,
//! [`refresh`] reads it through a snapshot and brings
//! `<data root>/files-mirror/files/` up to date where it is: only the files
//! whose bytes changed are written (each beside the old one, then renamed
//! over it), and the files of records that are gone are removed. Once a day
//! [`run_once`] copies the database (`VACUUM INTO`), refreshes the files
//! from the copy, rebuilds them into a new database and compares it with
//! the copy ([`check_export_with`]). A run that fails leaves every file
//! whole; the next one finishes it. The files are not read back yet, so
//! turning this off loses nothing.
//!
//! Everything is written under `<data root>/files-mirror/`. The person's
//! pages folder is only read (its `state.json` and file ids), never written.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use wenlan_types::responses::{FilesMirrorState, FilesMirrorStatus};

use crate::db::{MemoryDB, MemoryEmbeddingRecovery, SharedEmbedder};
use crate::error::WenlanError;
use crate::events::NoopEmitter;
use crate::store::export::run::{refresh_lossless, PagesFolder};
use crate::store::rebuild::check::check_export_with;

/// The folder under the data root.
pub const MIRROR_DIR: &str = "files-mirror";
/// The latest files.
pub const FILES_DIR: &str = "files";
pub const STATUS_FILE: &str = "status.json";
/// Counts and paths only, safe to share.
pub const REPORT_TEXT: &str = "check-report.txt";
/// With row keys (ids), for finding a difference on this computer.
pub const REPORT_JSON: &str = "check-report.json";

/// Left by step 3 builds, which wrote a new folder and swapped it in.
const NEXT_DIR: &str = "files.next";
const OLD_DIR: &str = "files.old";
const SNAPSHOT_DIR: &str = "snapshot";
const CHECK_DIR: &str = "check-work";

/// The mirror folder of `data_root`.
pub fn mirror_root(data_root: &Path) -> PathBuf {
    data_root.join(MIRROR_DIR)
}

/// The last status written, if any.
pub fn read_status(mirror: &Path) -> Option<FilesMirrorStatus> {
    let text = std::fs::read_to_string(mirror.join(STATUS_FILE)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Writes `status` (write a temporary file, then rename over).
pub fn write_status(mirror: &Path, status: &FilesMirrorStatus) -> Result<(), WenlanError> {
    std::fs::create_dir_all(mirror)?;
    let json = serde_json::to_string_pretty(status)
        .map_err(|error| WenlanError::Validation(format!("mirror status: {error}")))?;
    let temp = mirror.join(format!("{STATUS_FILE}.tmp"));
    std::fs::write(&temp, json + "\n")?;
    std::fs::rename(&temp, mirror.join(STATUS_FILE))?;
    Ok(())
}

fn remove_dir(path: &Path) -> Result<(), WenlanError> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// What one run did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MirrorRun {
    pub files: usize,
    /// Files written because they were new or changed, and files removed
    /// because the database no longer has them.
    pub written: usize,
    pub removed: usize,
    pub write_ms: u64,
    /// `Some(clean)` when the run also checked the files.
    pub check_clean: Option<bool>,
    pub check_ms: u64,
    /// A leftover folder that could not be removed after the files were
    /// written (on Windows, another program may hold a file open). The run
    /// still worked; the next run tries again.
    pub cleanup: Option<String>,
}

/// Gets `files/` ready for a refresh: the last good files put back from
/// `files.old` (left by a step 3 build that stopped between its renames),
/// and a folder with no manifest (an export that stopped before its
/// first manifest) cleared, since nothing tells which of its files are
/// current.
fn prepare(mirror: &Path) -> Result<PathBuf, WenlanError> {
    std::fs::create_dir_all(mirror)?;
    let files = mirror.join(FILES_DIR);
    let old = mirror.join(OLD_DIR);
    if !files.exists() && old.exists() {
        std::fs::rename(&old, &files)?;
    }
    // A folder with no manifest, or one this build cannot read, is
    // refreshed like any other: the refresh removes what the new manifest
    // does not list, after writing it.
    Ok(files)
}

/// Least files an earlier export must list before [`refuse_a_collapse`]
/// compares.
const COLLAPSE_FLOOR: usize = 20;

/// Refuses to refresh `files` from `db` when the database holds less than
/// half the pages, or less than half the memories, the files do: a
/// database that came back empty or damaged must not empty the copy it
/// could be rebuilt from. Pages and memories are compared apart, and only
/// what the export writes a file for is counted, so many memories (or
/// connected-folder documents, which get no file) cannot hide lost pages.
pub async fn refuse_a_collapse(db: &MemoryDB, files: &Path) -> Result<(), WenlanError> {
    let Some(old) = crate::store::export::read_manifest(files) else {
        return Ok(());
    };
    let listed = |types: &[&str]| {
        old.files
            .iter()
            .filter(|file| types.contains(&file.file_type.as_str()))
            .count()
    };
    let (listed_pages, listed_memories) = (
        listed(&["page", "archived_page"]),
        listed(&["memory", "original"]),
    );
    if listed_pages < COLLAPSE_FLOOR && listed_memories < COLLAPSE_FLOOR {
        return Ok(());
    }
    let (pages, heads) = db.export_content_heads().await?;
    let memories = heads
        .iter()
        .filter(|(source, class, agent)| {
            crate::store::export::memories::gets_a_file(source, class.as_deref(), agent.as_deref())
        })
        .count();
    for (what, now, listed) in [
        ("pages", pages, listed_pages),
        ("memories", memories, listed_memories),
    ] {
        if listed >= COLLAPSE_FLOOR && now.saturating_mul(2) < listed {
            return Err(WenlanError::Validation(format!(
                "the database holds {now} {what} and the files {listed}: the files are kept \
                 as they are and not refreshed. Delete files-mirror/ to start the copy again \
                 from this database."
            )));
        }
    }
    Ok(())
}

/// Brings `files/` up to date with the live database file `db_file`, where
/// it is: read through a snapshot of the file (the daemon keeps writing),
/// only the files that changed are written. No copy of the database is
/// made and nothing is checked; [`run_once`] does that once a day.
pub async fn refresh(
    db_file: &Path,
    pages: &PagesFolder,
    data_root: &Path,
    mirror: &Path,
) -> Result<MirrorRun, WenlanError> {
    let files = prepare(mirror)?;
    let started = Instant::now();
    let db = MemoryDB::open_read_snapshot(db_file).await?;
    refuse_a_collapse(&db, &files).await?;
    let summary = refresh_lossless(&db, pages, data_root, &files).await?;
    Ok(MirrorRun {
        files: summary.files,
        written: summary.written,
        removed: summary.removed,
        write_ms: started.elapsed().as_millis() as u64,
        ..MirrorRun::default()
    })
}

/// One full run: snapshot the database file `db_file` (`VACUUM INTO`),
/// bring `files/` up to date with the snapshot, and, when `check`, rebuild
/// the files and compare them with it. `pages` is today's pages folder
/// (read only); `data_root` holds the artifact originals and `layout/`
/// (read only). `embedder` opens the snapshot and the rebuilt database
/// without loading the model again.
pub async fn run_once(
    db_file: &Path,
    pages: &PagesFolder,
    data_root: &Path,
    mirror: &Path,
    embedder: SharedEmbedder,
    check: bool,
) -> Result<MirrorRun, WenlanError> {
    let files = prepare(mirror)?;
    let snapshot_dir = mirror.join(SNAPSHOT_DIR);
    let next = mirror.join(NEXT_DIR);
    let work = mirror.join(CHECK_DIR);
    let old = mirror.join(OLD_DIR);
    // Leftovers of a run that stopped part way.
    for dir in [&snapshot_dir, &next, &work, &old] {
        remove_dir(dir)?;
    }

    let result = async {
        let started = Instant::now();
        let db_dir = snapshot_dir.join("memorydb");
        std::fs::create_dir_all(&db_dir)?;
        crate::db::copy_database_file(db_file, &db_dir.join("origin_memory.db")).await?;
        // Deferred: the snapshot is exported as it is, rows without a
        // vector included, never re-embedded.
        let snapshot = MemoryDB::new_with_shared_embedder_and_recovery(
            &db_dir,
            Arc::new(NoopEmitter),
            embedder.clone(),
            MemoryEmbeddingRecovery::Deferred,
        )
        .await?;
        refuse_a_collapse(&snapshot, &files).await?;
        let summary = refresh_lossless(&snapshot, pages, data_root, &files).await?;
        let mut run = MirrorRun {
            files: summary.files,
            written: summary.written,
            removed: summary.removed,
            write_ms: started.elapsed().as_millis() as u64,
            ..MirrorRun::default()
        };
        if check {
            let started = Instant::now();
            let report = check_export_with(&snapshot, &files, &work, Some(embedder)).await?;
            let json = serde_json::to_string_pretty(&report)
                .map_err(|error| WenlanError::Validation(format!("check report: {error}")))?;
            std::fs::write(mirror.join(REPORT_JSON), json + "\n")?;
            std::fs::write(mirror.join(REPORT_TEXT), report.summary())?;
            run.check_clean = Some(report.is_clean());
            run.check_ms = started.elapsed().as_millis() as u64;
        }
        Ok::<_, WenlanError>(run)
    }
    .await;

    // The snapshot and the rebuilt database are as large as the database;
    // never keep them.
    let cleanup = [&snapshot_dir, &next, &work, &old]
        .into_iter()
        .try_for_each(|dir| remove_dir(dir));
    let mut run = result?;
    if let Err(error) = cleanup {
        run.cleanup = Some(format!("could not remove a leftover folder: {error}"));
    }
    Ok(run)
}

/// `status` after `run` finished at `now` (unix seconds).
pub fn after_run(mut status: FilesMirrorStatus, run: &MirrorRun, now: i64) -> FilesMirrorStatus {
    status.written_at = Some(now);
    status.files = run.files;
    status.write_ms = run.write_ms;
    status.note = run.cleanup.clone();
    if let Some(clean) = run.check_clean {
        status.checked_at = Some(now);
        status.check_clean = Some(clean);
        status.check_ms = run.check_ms;
        status.checks += 1;
        if clean {
            status.clean_checks += 1;
        }
    }
    status.state = match status.check_clean {
        Some(false) => FilesMirrorState::Differences,
        _ => FilesMirrorState::Ok,
    };
    status
}

/// `status` after a refresh `run` finished at `now`: the files are
/// current; the last check's verdict, and any note on why a check waits,
/// stand.
pub fn after_refresh(
    mut status: FilesMirrorStatus,
    run: &MirrorRun,
    now: i64,
) -> FilesMirrorStatus {
    status.written_at = Some(now);
    status.files = run.files;
    status.write_ms = run.write_ms;
    if status.state == FilesMirrorState::Failed {
        // The failure is behind it.
        status.note = None;
    }
    status.state = match status.check_clean {
        Some(false) => FilesMirrorState::Differences,
        _ => FilesMirrorState::Ok,
    };
    status
}

/// `status` after a run failed with `error`. The previous files and check
/// stay as they were.
pub fn after_failure(mut status: FilesMirrorStatus, error: &WenlanError) -> FilesMirrorStatus {
    status.state = FilesMirrorState::Failed;
    status.note = Some(error.to_string());
    status
}

#[cfg(test)]
#[path = "mirror_test.rs"]
mod tests;
