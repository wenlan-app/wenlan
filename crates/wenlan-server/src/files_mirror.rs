// SPDX-License-Identifier: Apache-2.0
//! Storage migration step 3 in the daemon: keep the file copy of the
//! database current (`wenlan_core::store::mirror`) and check it once a day.
//!
//! A run copies the database, writes the files and checks them: once a day,
//! and only when the database changed since the last run. (Measured on a
//! 600 MB database: about 70 s for the files and 70 s for the check, so not
//! hourly.) The first run is ten minutes after the daemon starts, so a
//! short-lived daemon (a test, a quick restart) never runs it. `WENLAN_FILES_MIRROR=0`
//! turns it off. The files are never read back: the database stays the
//! truth, and nothing the person sees changes.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use wenlan_core::store::export::run::PagesFolder;
use wenlan_core::store::mirror;
use wenlan_types::responses::{FilesMirrorState, FilesMirrorStatus};

use crate::state::SharedState;

const FIRST_RUN_AFTER: Duration = Duration::from_secs(10 * 60);
const TICK: Duration = Duration::from_secs(10 * 60);
/// Least time between two runs, counted from when the last one started.
const RUN_EVERY: i64 = 24 * 60 * 60;
/// Free space a run needs beyond three times the database's size (the
/// snapshot, the rebuilt database and the files).
const SPARE_BYTES: u64 = 512 * 1024 * 1024;

fn turned_off() -> bool {
    std::env::var("WENLAN_FILES_MIRROR").is_ok_and(|value| value.trim() == "0")
}

fn mirror_dir() -> PathBuf {
    mirror::mirror_root(&wenlan_core::config::data_root())
}

/// For `/api/status`: the last status written, or `Off` when turned off.
pub fn read_status() -> Option<FilesMirrorStatus> {
    if turned_off() {
        return Some(FilesMirrorStatus {
            state: FilesMirrorState::Off,
            ..mirror::read_status(&mirror_dir()).unwrap_or_default()
        });
    }
    mirror::read_status(&mirror_dir())
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

/// The database file and its write-ahead log: their size, and the latest
/// time either was written (unix seconds).
fn database_files(db_dir: &Path) -> (u64, i64) {
    let mut size = 0;
    let mut changed = 0;
    for name in ["origin_memory.db", "origin_memory.db-wal"] {
        if let Ok(meta) = std::fs::metadata(db_dir.join(name)) {
            size += meta.len();
            if let Ok(modified) = meta.modified() {
                if let Ok(at) = modified.duration_since(UNIX_EPOCH) {
                    changed = changed.max(at.as_secs() as i64);
                }
            }
        }
    }
    (size, changed)
}

/// Whether a run is due: never run, or a day since the last one started
/// and the database changed since then.
fn due(status: &FilesMirrorStatus, db_changed_at: i64, now: i64) -> bool {
    match status.tried_at.or(status.written_at) {
        None => true,
        Some(at) => now - at >= RUN_EVERY && db_changed_at > at,
    }
}

/// Before the database opens: when its file is gone and a file copy
/// exists, builds it again from the files (`wenlan_core::store::restore`).
/// When that fails, the files are moved aside so the empty database the
/// daemon then starts with cannot overwrite them, and the status says so.
pub async fn restore_missing_database(data_root: &Path, db_dir: &Path) {
    use wenlan_core::store::restore::{self, Restore};
    if turned_off() {
        return;
    }
    let mirror = mirror::mirror_root(data_root);
    match restore::restore_missing_database(data_root, db_dir).await {
        Ok(Restore::NotNeeded) | Ok(Restore::NoFiles) => {}
        Ok(Restore::Restored { rows, problems, ms }) => {
            tracing::warn!(
                "[files_mirror] the database was missing: rebuilt it from {} in {ms} ms \
                 ({rows} rows, {} problems, see {})",
                mirror.join(mirror::FILES_DIR).display(),
                problems.len(),
                mirror.join(restore::RESTORE_REPORT).display()
            );
            note(
                &mirror,
                format!("the database was rebuilt from these files at {}", now()),
            );
        }
        Err(error) => {
            let kept = restore::keep_files_after_failure(data_root, now());
            tracing::error!(
                "[files_mirror] the database was missing and could not be rebuilt from the \
                 files: {error}; files kept in {}",
                kept.as_deref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "place".to_string())
            );
            note(
                &mirror,
                format!("the database could not be rebuilt from these files: {error}"),
            );
        }
    }
}

fn note(mirror: &Path, text: String) {
    let status = FilesMirrorStatus {
        note: Some(text),
        ..mirror::read_status(mirror).unwrap_or_default()
    };
    if let Err(error) = mirror::write_status(mirror, &status) {
        tracing::warn!("[files_mirror] status not written: {error}");
    }
}

pub fn start(shared: SharedState, shutdown: tokio::sync::watch::Receiver<bool>) {
    if turned_off() {
        return;
    }
    tokio::spawn(run(shared, shutdown));
}

async fn run(shared: SharedState, mut shutdown: tokio::sync::watch::Receiver<bool>) {
    let mut wait = FIRST_RUN_AFTER;
    loop {
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = shutdown.changed() => {}
        }
        if *shutdown.borrow() {
            return;
        }
        wait = TICK;
        if let Err(error) = tick(&shared).await {
            tracing::warn!("[files_mirror] {error}");
        }
    }
}

async fn tick(shared: &SharedState) -> anyhow::Result<()> {
    let db = { shared.read().await.db.clone() };
    let Some(db) = db else { return Ok(()) };
    let dir = mirror_dir();
    let status = mirror::read_status(&dir).unwrap_or_default();
    let data_root = wenlan_core::config::data_root();
    let (db_size, db_changed_at) = database_files(&data_root.join("memorydb"));
    let now = now();
    if !due(&status, db_changed_at, now) {
        return Ok(());
    }
    let wait = |note: &str| -> anyhow::Result<()> {
        let status = FilesMirrorStatus {
            state: if status.written_at.is_some() {
                status.state.clone()
            } else {
                FilesMirrorState::Waiting
            },
            note: Some(note.to_string()),
            ..status.clone()
        };
        mirror::write_status(&dir, &status)?;
        Ok(())
    };
    let Some(embedder) = db.shared_embedder() else {
        return wait("waiting for the search model to load");
    };
    std::fs::create_dir_all(&dir)?;
    let free = fs2::available_space(&dir).unwrap_or(u64::MAX);
    if free < db_size.saturating_mul(3).saturating_add(SPARE_BYTES) {
        return wait("skipped: not enough free disk space");
    }

    let pages = wenlan_core::config::load_config().knowledge_path_or_default();
    let writer = wenlan_core::export::knowledge::KnowledgeWriter::new(pages.clone(), &db);
    let folder = PagesFolder {
        dir: Some(pages),
        recorded: writer.page_files(),
        digests: writer.page_file_digests(),
        skip_unclaimed: true,
    };
    let status = FilesMirrorStatus {
        tried_at: Some(now),
        ..status
    };
    mirror::write_status(&dir, &status)?;
    let db_file = data_root.join("memorydb").join("origin_memory.db");
    // libsql runs each statement on the calling thread, and a run takes a
    // minute or more: keep it off the async workers that serve requests.
    let handle = tokio::runtime::Handle::current();
    let (root, mirror_dir) = (data_root.clone(), dir.clone());
    let result = tokio::task::spawn_blocking(move || {
        handle.block_on(mirror::run_once(
            &db_file,
            &folder,
            &root,
            &mirror_dir,
            embedder,
            true,
        ))
    })
    .await?;
    let next = match result {
        Ok(run) => {
            tracing::info!(
                "[files_mirror] wrote {} files in {} ms{}",
                run.files,
                run.write_ms,
                match run.check_clean {
                    Some(true) => format!(", check clean in {} ms", run.check_ms),
                    Some(false) => format!(", check found differences in {} ms", run.check_ms),
                    None => String::new(),
                }
            );
            mirror::after_run(status, &run, now)
        }
        Err(error) => mirror::after_failure(status, &error),
    };
    mirror::write_status(&dir, &next)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_is_due_once_a_day_and_only_after_a_change() {
        assert!(due(&FilesMirrorStatus::default(), 0, 1_000));
        let ran = FilesMirrorStatus {
            tried_at: Some(10_000),
            ..FilesMirrorStatus::default()
        };
        // Changed, but within the day.
        assert!(!due(&ran, 10_500, 10_000 + RUN_EVERY - 1));
        assert!(due(&ran, 10_500, 10_000 + RUN_EVERY));
        // A day later with no change since: nothing to do.
        assert!(!due(&ran, 9_000, 10_000 + 2 * RUN_EVERY));
        // A failed run counts from when it started, so it is not retried
        // every tick.
        let failed = FilesMirrorStatus {
            tried_at: Some(20_000),
            written_at: Some(10_000),
            state: FilesMirrorState::Failed,
            ..FilesMirrorStatus::default()
        };
        assert!(!due(&failed, 30_000, 20_000 + 600));
    }
}
