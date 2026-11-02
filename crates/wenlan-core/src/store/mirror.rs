// SPDX-License-Identifier: Apache-2.0
//! Storage migration step 3: keep a file copy of the database current, and
//! check it every day.
//!
//! The database stays the truth. From time to time the daemon copies it
//! (`VACUUM INTO`, a consistent snapshot), exports the copy with the
//! lossless export into `<data root>/files-mirror/files/`, and once a day
//! rebuilds those files into a new database and compares it with the copy
//! ([`check_export_with`]). A run that fails leaves the previous files and
//! report in place; the files are never read back, so turning this off
//! loses nothing.
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
use crate::store::export::run::{export_lossless, PagesFolder};
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
    pub write_ms: u64,
    /// `Some(clean)` when the run also checked the files.
    pub check_clean: Option<bool>,
    pub check_ms: u64,
}

/// One run: snapshot `db`, export the snapshot into `files/` (replacing the
/// previous files only once the new ones are complete), and, when `check`,
/// rebuild the files and compare. `pages` is today's pages folder (read
/// only); `data_root` holds the artifact originals and `layout/` (read
/// only). `embedder` opens the snapshot and the rebuilt database without
/// loading the model again.
pub async fn run_once(
    db: &MemoryDB,
    pages: &PagesFolder,
    data_root: &Path,
    mirror: &Path,
    embedder: SharedEmbedder,
    check: bool,
) -> Result<MirrorRun, WenlanError> {
    std::fs::create_dir_all(mirror)?;
    let snapshot_dir = mirror.join(SNAPSHOT_DIR);
    let next = mirror.join(NEXT_DIR);
    let work = mirror.join(CHECK_DIR);
    // Leftovers of a run that stopped part way.
    for dir in [&snapshot_dir, &next, &work, &mirror.join(OLD_DIR)] {
        remove_dir(dir)?;
    }

    let result = async {
        let started = Instant::now();
        let db_dir = snapshot_dir.join("memorydb");
        std::fs::create_dir_all(&db_dir)?;
        db.copy_into(&db_dir.join("origin_memory.db")).await?;
        // Deferred: the snapshot is exported as it is, rows without a
        // vector included, never re-embedded.
        let snapshot = MemoryDB::new_with_shared_embedder_and_recovery(
            &db_dir,
            Arc::new(NoopEmitter),
            embedder.clone(),
            MemoryEmbeddingRecovery::Deferred,
        )
        .await?;
        let summary = export_lossless(&snapshot, pages, data_root, &next).await?;
        let files = mirror.join(FILES_DIR);
        let old = mirror.join(OLD_DIR);
        if files.exists() {
            std::fs::rename(&files, &old)?;
        }
        std::fs::rename(&next, &files)?;
        remove_dir(&old)?;
        let mut run = MirrorRun {
            files: summary.files,
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
    let cleanup = [&snapshot_dir, &next, &work]
        .into_iter()
        .try_for_each(|dir| remove_dir(dir));
    let run = result?;
    cleanup?;
    Ok(run)
}

/// `status` after `run` finished at `now` (unix seconds).
pub fn after_run(mut status: FilesMirrorStatus, run: &MirrorRun, now: i64) -> FilesMirrorStatus {
    status.written_at = Some(now);
    status.files = run.files;
    status.write_ms = run.write_ms;
    status.note = None;
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
