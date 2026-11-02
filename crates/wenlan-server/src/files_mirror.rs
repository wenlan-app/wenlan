// SPDX-License-Identifier: Apache-2.0
//! Storage migration steps 3 and 4 in the daemon: keep the file copy of
//! the database current (`wenlan_core::store::mirror`) and check it once a
//! day.
//!
//! - **Refresh:** when the database changed, at most every
//!   [`REFRESH_EVERY`], the files are brought up to date where they are.
//!   Only changed files are written, so a refresh costs reading and
//!   rendering, not a copy of the database.
//! - **Check:** once a day, and only when the database changed since the
//!   last check, a run copies the database, refreshes the files from the
//!   copy and rebuilds them to compare. (Measured on a 600 MB database:
//!   about 70 s for the check, so not hourly.)
//!
//! Nothing runs in the first [`FIRST_RUN_AFTER`], so a short-lived daemon
//! (a test, a quick restart) never does. `WENLAN_FILES_MIRROR=0` turns it
//! off. The files are not read back yet: nothing the person sees changes.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use wenlan_core::store::export::run::PagesFolder;
use wenlan_core::store::mirror;
use wenlan_types::responses::{FilesMirrorState, FilesMirrorStatus};

use crate::state::SharedState;

const FIRST_RUN_AFTER: Duration = Duration::from_secs(2 * 60);
const TICK: Duration = Duration::from_secs(30);
/// Least time between two refreshes, counted from when the last one
/// started.
const REFRESH_EVERY: i64 = 2 * 60;
/// Least time between two checks, counted from when the last one started.
const RUN_EVERY: i64 = 24 * 60 * 60;
/// A check copies the whole database: never in a daemon's first minutes.
const FIRST_CHECK_AFTER: Duration = Duration::from_secs(10 * 60);
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

/// Whether a check is due: never run, or a day since the last one started
/// and the database changed since then.
fn due(status: &FilesMirrorStatus, db_changed_at: i64, now: i64) -> bool {
    match status.tried_at {
        None => true,
        Some(at) => now - at >= RUN_EVERY && db_changed_at > at,
    }
}

/// Whether a refresh is due: the database changed since the last refresh
/// started (`last`), and that was at least [`REFRESH_EVERY`] ago. The
/// change time has whole seconds, so a change in the same second as the
/// last start counts as after it.
fn refresh_due(last: Option<i64>, db_changed_at: i64, now: i64) -> bool {
    match last {
        None => true,
        Some(at) => now - at >= REFRESH_EVERY && db_changed_at >= at,
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
    // When the last refresh started, whatever its outcome, so a failing one
    // is not retried every tick.
    let mut last_refresh: Option<i64> = None;
    let started = std::time::Instant::now();
    loop {
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = shutdown.changed() => {}
        }
        if *shutdown.borrow() {
            return;
        }
        wait = TICK;
        let may_check = started.elapsed() >= FIRST_CHECK_AFTER;
        if let Err(error) = tick(&shared, &mut last_refresh, may_check).await {
            tracing::warn!("[files_mirror] {error}");
        }
    }
}

/// The pages folder as the export reads it: the folder and what its
/// `state.json` records.
fn pages_folder(db: &wenlan_core::db::MemoryDB) -> PagesFolder {
    let pages = wenlan_core::config::load_config().knowledge_path_or_default();
    let writer = wenlan_core::export::knowledge::KnowledgeWriter::new(pages.clone(), db);
    PagesFolder {
        dir: Some(pages),
        recorded: writer.page_files(),
        digests: writer.page_file_digests(),
        skip_unclaimed: true,
    }
}

async fn tick(
    shared: &SharedState,
    last_refresh: &mut Option<i64>,
    may_check: bool,
) -> anyhow::Result<()> {
    let db = { shared.read().await.db.clone() };
    let Some(db) = db else { return Ok(()) };
    let dir = mirror_dir();
    let status = mirror::read_status(&dir).unwrap_or_default();
    let data_root = wenlan_core::config::data_root();
    let (db_size, db_changed_at) = database_files(&data_root.join("memorydb"));
    let now = now();
    // A check also refreshes the files. When one is due but cannot run
    // yet, the files are still refreshed, and the status says why.
    let blocked = if !may_check || !due(&status, db_changed_at, now) {
        None
    } else if db.shared_embedder().is_none() {
        Some("waiting for the search model to load")
    } else {
        std::fs::create_dir_all(&dir)?;
        let free = fs2::available_space(&dir).unwrap_or(u64::MAX);
        (free < db_size.saturating_mul(3).saturating_add(SPARE_BYTES))
            .then_some("check skipped: not enough free disk space")
    };
    let check_due = may_check && due(&status, db_changed_at, now);
    if !check_due || blocked.is_some() {
        let mut status = status;
        if let Some(note) = blocked {
            if status.written_at.is_none() {
                status.state = FilesMirrorState::Waiting;
            }
            status.note = Some(note.to_string());
            mirror::write_status(&dir, &status)?;
        }
        if refresh_due(*last_refresh, db_changed_at, now) {
            *last_refresh = Some(now);
            refresh(&db, &dir, status, now).await?;
        }
        return Ok(());
    }
    *last_refresh = Some(now);
    let Some(embedder) = db.shared_embedder() else {
        return Ok(());
    };

    let folder = pages_folder(&db);
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
                "[files_mirror] {} files, {} written, {} removed, in {} ms{}",
                run.files,
                run.written,
                run.removed,
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

/// Brings the files up to date with the live database, between checks.
async fn refresh(
    db: &wenlan_core::db::MemoryDB,
    dir: &Path,
    status: FilesMirrorStatus,
    now: i64,
) -> anyhow::Result<()> {
    let folder = pages_folder(db);
    let data_root = wenlan_core::config::data_root();
    let db_file = data_root.join("memorydb").join("origin_memory.db");
    let handle = tokio::runtime::Handle::current();
    let mirror_dir = dir.to_path_buf();
    // Off the async workers, like a check: libsql runs on the calling thread.
    let result = tokio::task::spawn_blocking(move || {
        handle.block_on(mirror::refresh(&db_file, &folder, &data_root, &mirror_dir))
    })
    .await?;
    let next = match result {
        Ok(run) => {
            if run.written + run.removed > 0 {
                tracing::info!(
                    "[files_mirror] refreshed: {} written, {} removed, in {} ms",
                    run.written,
                    run.removed,
                    run.write_ms
                );
            }
            mirror::after_refresh(status, &run, now)
        }
        Err(error) => mirror::after_failure(status, &error),
    };
    mirror::write_status(dir, &next)?;
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

    #[test]
    fn a_refresh_follows_a_change_at_most_every_two_minutes() {
        assert!(refresh_due(None, 0, 1_000));
        // Changed since the last refresh, but too soon.
        assert!(!refresh_due(Some(1_000), 1_050, 1_000 + REFRESH_EVERY - 1));
        assert!(refresh_due(Some(1_000), 1_050, 1_000 + REFRESH_EVERY));
        // A change in the same second as the last start counts.
        assert!(refresh_due(Some(1_000), 1_000, 1_000 + REFRESH_EVERY));
        // No change since: nothing to do.
        assert!(!refresh_due(Some(1_000), 999, 1_000 + 10 * REFRESH_EVERY));
    }
}
