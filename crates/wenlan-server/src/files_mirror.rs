// SPDX-License-Identifier: Apache-2.0
//! Storage migration step 3 in the daemon: keep the file copy of the
//! database current (`wenlan_core::store::mirror`) and check it once a day.
//!
//! The copy is rewritten at most once an hour, and only after the database
//! changed. It starts ten minutes after the daemon does, so a short-lived
//! daemon (a test, a quick restart) never runs it. `WENLAN_FILES_MIRROR=0`
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
/// Least time between two writes of the files.
const WRITE_EVERY: i64 = 60 * 60;
/// Least time between two checks.
const CHECK_EVERY: i64 = 24 * 60 * 60;
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

/// What the next tick should do.
#[derive(Debug, PartialEq, Eq)]
enum Due {
    Nothing,
    Write,
    WriteAndCheck,
}

fn due(status: &FilesMirrorStatus, db_changed_at: i64, now: i64) -> Due {
    let check_due = status.checked_at.is_none_or(|at| now - at >= CHECK_EVERY);
    let write_due = match status.written_at {
        None => true,
        Some(at) => db_changed_at > at && now - at >= WRITE_EVERY,
    };
    match (write_due, check_due) {
        (_, true) => Due::WriteAndCheck,
        (true, false) => Due::Write,
        (false, false) => Due::Nothing,
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
    let todo = due(&status, db_changed_at, now);
    if todo == Due::Nothing {
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
    };
    let check = todo == Due::WriteAndCheck;
    let next = match mirror::run_once(&db, &folder, &data_root, &dir, embedder, check).await {
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
    fn the_first_tick_writes_and_checks_then_waits_for_a_change_and_a_day() {
        let fresh = FilesMirrorStatus::default();
        assert_eq!(due(&fresh, 0, 1_000), Due::WriteAndCheck);

        let written = FilesMirrorStatus {
            written_at: Some(10_000),
            checked_at: Some(10_000),
            ..FilesMirrorStatus::default()
        };
        // No change since the last write.
        assert_eq!(due(&written, 9_000, 10_000 + 2 * WRITE_EVERY), Due::Nothing);
        // Changed, but within the hour.
        assert_eq!(
            due(&written, 10_500, 10_000 + WRITE_EVERY - 1),
            Due::Nothing
        );
        assert_eq!(due(&written, 10_500, 10_000 + WRITE_EVERY), Due::Write);
        // A day later the check is due, changed or not.
        assert_eq!(
            due(&written, 9_000, 10_000 + CHECK_EVERY),
            Due::WriteAndCheck
        );
    }
}
