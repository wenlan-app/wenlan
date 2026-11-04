// SPDX-License-Identifier: Apache-2.0
//! Storage migration steps 3 and 4 in the daemon: keep the file copy of
//! the database current (`wenlan_core::store::mirror`) and check it once a
//! day.
//!
//! - **Refresh:** when the database changed, at most every
//!   [`REFRESH_EVERY`] (longer when a refresh is slow, see
//!   [`REFRESH_SHARE`]), the files are brought up to date where they are.
//!   Only changed files are written, so a refresh costs reading and
//!   rendering, not a copy of the database.
//! - **Check:** once a day, and only when the database changed since the
//!   last check, a run copies the database, refreshes the files from the
//!   copy and rebuilds them to compare. (Measured on a 600 MB database:
//!   about 70 s for the check, so not hourly.)
//!
//! - **Write log:** every write to what the files hold is logged before
//!   the database commits it (`wenlan_core::store::write_log`); each
//!   refresh records how far into the log its files go and drops the log
//!   segments they now hold. The files plus the log are the truth: a
//!   deleted database is built again from them at start, the last writes
//!   included.
//!
//! Nothing runs in the first [`FIRST_RUN_AFTER`], so a short-lived daemon
//! (a test, a quick restart) never does. `WENLAN_FILES_MIRROR=0` turns it
//! all off, the write log included.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use wenlan_core::store::export::run::PagesFolder;
use wenlan_core::store::mirror;
use wenlan_core::store::write_log::{self, LogPosition, WriteLog};
use wenlan_types::responses::{FilesMirrorState, FilesMirrorStatus};

use crate::state::SharedState;

const FIRST_RUN_AFTER: Duration = Duration::from_secs(2 * 60);
const TICK: Duration = Duration::from_secs(30);
/// Least time between two refreshes, counted from when the last one
/// started.
const REFRESH_EVERY: i64 = 2 * 60;
/// A refresh reads and renders everything, so the gap also grows with its
/// cost: at most a tenth of the time is spent refreshing.
const REFRESH_SHARE: u64 = 10;
/// Most time between two refreshes, however slow the last one was: the
/// first refresh writes every file and can take minutes, and the writes
/// made after it must not wait ten times that long to reach the files.
const REFRESH_GAP_MAX: i64 = 10 * 60;
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
fn refresh_due(last: &Refreshed, db_changed_at: i64, now: i64) -> bool {
    match last.at {
        None => true,
        Some(at) => {
            now - at >= last.gap.max(refresh_every()) && (last.failed || db_changed_at >= at)
        }
    }
}

/// The gap after a refresh that took `ms`.
fn refresh_gap(ms: u64) -> i64 {
    (ms.saturating_mul(REFRESH_SHARE) / 1_000).min(REFRESH_GAP_MAX as u64) as i64
}

/// Before the database opens: when its file is gone and a file copy
/// exists, builds it again from the files (`wenlan_core::store::restore`).
/// When that fails, the files are moved aside so the empty database the
/// daemon then starts with cannot overwrite them, and the status says so.
pub async fn restore_missing_database(data_root: &Path, db_dir: &Path) {
    use wenlan_core::store::restore::{self, Restore};
    let mirror = mirror::mirror_root(data_root);
    if turned_off() {
        if !db_dir.join(restore::DATABASE_FILE).exists()
            && restore::restore_source(data_root).is_some()
        {
            tracing::warn!(
                "[files_mirror] the database is missing and {} holds a file copy, but \
                 WENLAN_FILES_MIRROR=0: starting with an empty database. Unset it and \
                 restart to rebuild the database from the files.",
                mirror.display()
            );
        }
        return;
    }
    match restore::restore_missing_database(data_root, db_dir, now()).await {
        Ok(Restore::NotNeeded) | Ok(Restore::NoFiles) => {}
        Ok(Restore::Restored {
            rows,
            logged,
            problems,
            ms,
            set_aside,
        }) => {
            tracing::warn!(
                "[files_mirror] the database was missing: rebuilt it from the file copy in \
                 {ms} ms ({rows} rows, {logged} logged writes after them, {} problems, see {})",
                problems.len(),
                mirror.join(restore::RESTORE_REPORT).display()
            );
            let where_ = match &set_aside {
                Some(path) => format!("kept in {}", path.display()),
                None => {
                    // The files could not be moved: a refresh from the
                    // rebuilt database must not replace them.
                    hold();
                    "left in place; the copy is not refreshed until restart".to_string()
                }
            };
            note(
                &mirror,
                format!(
                    "the database was rebuilt from the file copy at {}; those files are {where_}",
                    now()
                ),
            );
        }
        Err(error) => {
            let kept = restore::keep_files_after_failure(data_root, now());
            if kept.is_none() {
                hold();
            }
            tracing::error!(
                "[files_mirror] the database was missing and could not be rebuilt from the \
                 files: {error}; files kept in {}",
                kept.as_deref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "place, and the copy is not refreshed".to_string())
            );
            note(
                &mirror,
                format!("the database could not be rebuilt from the file copy: {error}"),
            );
        }
    }
}

/// The database's write log, once [`open_write_log`] opened it.
static WRITE_LOG: OnceLock<Arc<WriteLog>> = OnceLock::new();

/// Before the database opens, after a restore: opens the write log, applies
/// the last logged write when the database lost it (the process stopped
/// between the log and the database commit), and has every connection to
/// the database file logged from now on. When the log cannot be opened the
/// daemon still starts, the database written first as before, and the
/// status says so.
pub async fn open_write_log(data_root: &Path, db_dir: &Path) {
    use wenlan_core::store::restore::DATABASE_FILE;
    if turned_off() {
        return;
    }
    let mirror = mirror::mirror_root(data_root);
    let dir = mirror.join(write_log::LOG_DIR);
    let db_file = db_dir.join(DATABASE_FILE);
    let log = match WriteLog::open(&dir) {
        Ok(log) => log,
        Err(error) => {
            tracing::error!("[files_mirror] the write log could not be opened: {error}");
            note(
                &mirror,
                format!("the write log could not be opened: {error}"),
            );
            return;
        }
    };
    match wenlan_core::db::write_log_replay::settle(&db_file, &dir).await {
        Ok(Some(replayed)) => tracing::warn!(
            "[files_mirror] the database had lost its last write: applied it from the write \
             log ({} changes, {} problems)",
            replayed.changes,
            replayed.problems.len()
        ),
        Ok(None) => {}
        Err(error) => tracing::error!(
            "[files_mirror] the last write in the log could not be checked against the \
             database: {error}"
        ),
    }
    if let Err(error) = std::fs::create_dir_all(db_dir)
        .map_err(wenlan_core::error::WenlanError::from)
        .and_then(|()| write_log::register(&db_file, log.clone()))
    {
        tracing::error!("[files_mirror] the write log could not be attached: {error}");
        note(
            &mirror,
            format!("the write log could not be attached: {error}"),
        );
        return;
    }
    let _ = WRITE_LOG.set(log);
}

/// After the database opened and migrated: logging starts.
pub async fn arm_write_log(db: &wenlan_core::db::MemoryDB) {
    let Some(log) = WRITE_LOG.get() else {
        return;
    };
    if let Err(error) = db.arm_write_log(log).await {
        tracing::error!("[files_mirror] the write log could not start: {error}");
    }
}

/// Where the log stands with no write running, before a refresh reads the
/// database: the files it writes hold every line up to there.
async fn log_position(db: &wenlan_core::db::MemoryDB) -> Option<(Arc<WriteLog>, LogPosition)> {
    let log = WRITE_LOG.get().filter(|log| log.is_armed())?;
    Some((log.clone(), db.write_log_position(log).await))
}

/// After a refresh wrote `files`: records the position and drops the log
/// segments the files now hold.
fn files_hold(dir: &Path, logged: Option<(Arc<WriteLog>, LogPosition)>) {
    let Some((log, position)) = logged else {
        return;
    };
    let files = dir.join(mirror::FILES_DIR);
    if let Err(error) = write_log::write_position(&files, &position) {
        tracing::warn!("[files_mirror] log position not recorded: {error}");
        return;
    }
    if let Err(error) = log.prune_through(&position) {
        tracing::warn!("[files_mirror] old write log segments not removed: {error}");
    }
}

/// Set when files a restore needed could not be moved aside: the copy is
/// then left alone until the daemon restarts.
static HELD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn hold() {
    HELD.store(true, std::sync::atomic::Ordering::SeqCst);
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
    if turned_off() || HELD.load(std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    tokio::spawn(run(shared, shutdown));
}

/// `WENLAN_FILES_MIRROR_REFRESH_SECS` lets a process test see the files
/// follow the database without waiting minutes: the first refresh and the
/// least gap between two refreshes, in seconds.
fn refresh_override() -> Option<i64> {
    std::env::var("WENLAN_FILES_MIRROR_REFRESH_SECS")
        .ok()
        .and_then(|value| value.trim().parse().ok())
}

fn first_run_after() -> Duration {
    refresh_override()
        .map(|secs| Duration::from_secs(secs.max(0) as u64))
        .unwrap_or(FIRST_RUN_AFTER)
}

fn refresh_every() -> i64 {
    refresh_override().unwrap_or(REFRESH_EVERY)
}

async fn run(shared: SharedState, mut shutdown: tokio::sync::watch::Receiver<bool>) {
    let mut wait = first_run_after();
    let mut last_refresh = Refreshed::default();
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

/// When the last refresh started, whatever its outcome, so a failing one
/// is not retried every tick, and the gap it asks for.
#[derive(Default)]
struct Refreshed {
    at: Option<i64>,
    gap: i64,
    /// A failed refresh is tried again after the gap, change or not.
    failed: bool,
}

async fn tick(
    shared: &SharedState,
    last_refresh: &mut Refreshed,
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
        if refresh_due(last_refresh, db_changed_at, now) {
            last_refresh.at = Some(now);
            let started = std::time::Instant::now();
            let result = refresh(&db, &dir, status, now).await;
            last_refresh.gap = refresh_gap(started.elapsed().as_millis() as u64);
            last_refresh.failed = !matches!(result, Ok(true));
            result?;
        }
        return Ok(());
    }
    last_refresh.at = Some(now);
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
    let logged = log_position(&db).await;
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
            files_hold(&dir, logged);
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
) -> anyhow::Result<bool> {
    let folder = pages_folder(db);
    let data_root = wenlan_core::config::data_root();
    let db_file = data_root.join("memorydb").join("origin_memory.db");
    let logged = log_position(db).await;
    let handle = tokio::runtime::Handle::current();
    let mirror_dir = dir.to_path_buf();
    // Off the async workers, like a check: libsql runs on the calling thread.
    let result = tokio::task::spawn_blocking(move || {
        handle.block_on(mirror::refresh(&db_file, &folder, &data_root, &mirror_dir))
    })
    .await?;
    let ok = result.is_ok();
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
            files_hold(dir, logged);
            mirror::after_refresh(status, &run, now)
        }
        Err(error) => mirror::after_failure(status, &error),
    };
    mirror::write_status(dir, &next)?;
    Ok(ok)
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
        let at = |at: i64, failed: bool| Refreshed {
            at: Some(at),
            gap: 0,
            failed,
        };
        assert!(refresh_due(&Refreshed::default(), 0, 1_000));
        // Changed since the last refresh, but too soon.
        assert!(!refresh_due(
            &at(1_000, false),
            1_050,
            1_000 + REFRESH_EVERY - 1
        ));
        assert!(refresh_due(&at(1_000, false), 1_050, 1_000 + REFRESH_EVERY));
        // A change in the same second as the last start counts.
        assert!(refresh_due(&at(1_000, false), 1_000, 1_000 + REFRESH_EVERY));
        // No change since: nothing to do.
        assert!(!refresh_due(
            &at(1_000, false),
            999,
            1_000 + 10 * REFRESH_EVERY
        ));
        // Unless the last one failed: then it is tried again.
        assert!(!refresh_due(
            &at(1_000, true),
            999,
            1_000 + REFRESH_EVERY - 1
        ));
        assert!(refresh_due(&at(1_000, true), 999, 1_000 + REFRESH_EVERY));
    }

    #[test]
    fn a_slow_refresh_waits_longer() {
        let after = |ms: u64| Refreshed {
            at: Some(1_000),
            gap: refresh_gap(ms),
            failed: false,
        };
        // 30 s of work: at least five minutes until the next one.
        assert_eq!(refresh_gap(30_000), 300);
        assert!(!refresh_due(&after(30_000), 1_050, 1_000 + 299));
        assert!(refresh_due(&after(30_000), 1_050, 1_000 + 300));
        // A first refresh that wrote every file in 250 s: ten minutes, not
        // forty-two.
        assert_eq!(refresh_gap(250_000), REFRESH_GAP_MAX);
        assert!(refresh_due(&after(250_000), 1_050, 1_000 + REFRESH_GAP_MAX));
        // A quick one keeps the two minutes.
        assert!(!refresh_due(&after(3_000), 1_050, 1_000 + 60));
        assert!(refresh_due(&after(3_000), 1_050, 1_000 + REFRESH_EVERY));
    }
}
