// SPDX-License-Identifier: Apache-2.0
//! Daily housekeeping that needs no model: confidence decay, promotion of
//! settled memories, old rejection pruning, orphaned page-source cleanup, hub
//! links on memories that have a specific topic and,
//! when enabled, soft eviction.
//!
//! Steep runs in two modes, Automatic and On demand, and both must keep the
//! library tidy. These steps used to run only inside a full steep pass (the
//! foreground `POST /api/steep`), which the automatic scheduler never
//! selects, so in practice they ran in neither mode. The scheduler now calls
//! [`run_housekeeping`] once a day in both modes.

use crate::db::MemoryDB;

/// App metadata key holding the epoch seconds of the last completed run.
pub const HOUSEKEEPING_LAST_RUN_KEY: &str = "last_housekeeping_ts";

/// Minimum seconds between two runs.
pub const HOUSEKEEPING_INTERVAL_SECS: i64 = 24 * 60 * 60;

/// Promotion waits this many days for a contradiction before a memory counts
/// as learned; the same value the steep Promote phase uses.
const PROMOTE_MIN_AGE_DAYS: i64 = 7;

/// Rejection log retention, the same value the steep PruneRejections phase uses.
const REJECTION_RETENTION_DAYS: i64 = 30;

/// What one housekeeping run changed. A step that failed leaves its count at
/// zero and adds its error to `errors`; the other steps still run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HousekeepingReport {
    pub memories_decayed: u64,
    pub memories_promoted: usize,
    pub rejections_pruned: usize,
    pub orphaned_page_sources: usize,
    pub hub_links_removed: u64,
    /// Memories moved off an archived main topic (UX-GRAPH-8), each logged.
    pub archived_primaries_refiled: u64,
    pub memories_evicted: usize,
    pub errors: Vec<String>,
}

/// True when no run is recorded, or the last one is at least a day old. A
/// timestamp in the future (clock moved back) counts as due, so a skewed
/// clock cannot stall housekeeping for the size of the skew.
pub fn housekeeping_due(last_run_epoch: Option<i64>, now_epoch: i64) -> bool {
    match last_run_epoch {
        None => true,
        Some(last) if last > now_epoch => true,
        Some(last) => now_epoch - last >= HOUSEKEEPING_INTERVAL_SECS,
    }
}

/// Read the last completed run from app metadata.
pub async fn last_housekeeping_run(db: &MemoryDB) -> Option<i64> {
    db.get_app_metadata(HOUSEKEEPING_LAST_RUN_KEY)
        .await
        .ok()
        .flatten()
        .and_then(|value| value.parse::<i64>().ok())
}

/// Run every step once and record the run. Never calls a model.
pub async fn run_housekeeping(db: &MemoryDB, now_epoch: i64) -> HousekeepingReport {
    let mut report = HousekeepingReport::default();

    match db.decay_update_confidence().await {
        Ok(count) => report.memories_decayed = count,
        Err(error) => report.errors.push(format!("decay: {error}")),
    }
    match db.promote_uncontradicted(PROMOTE_MIN_AGE_DAYS).await {
        Ok(count) => report.memories_promoted = count,
        Err(error) => report.errors.push(format!("promote: {error}")),
    }
    match db.prune_rejections(REJECTION_RETENTION_DAYS).await {
        Ok(count) => report.rejections_pruned = count,
        Err(error) => report.errors.push(format!("prune rejections: {error}")),
    }
    match db.cleanup_orphaned_page_sources().await {
        Ok(count) => report.orphaned_page_sources = count,
        Err(error) => report
            .errors
            .push(format!("orphaned page sources: {error}")),
    }
    match db.unfile_hub_links_daily().await {
        Ok(count) => report.hub_links_removed = count,
        Err(error) => report.errors.push(format!("hub links: {error}")),
    }
    // Archiving a topic re-files its memories itself; this catches a main
    // topic archived by any other path, so lint's count stays at zero.
    match db
        .refile_archived_primaries("refile_archived_primaries_daily")
        .await
    {
        Ok(refiled) => report.archived_primaries_refiled = refiled.moved + refiled.unfiled,
        Err(error) => report.errors.push(format!("archived main topics: {error}")),
    }
    if crate::db::eviction_enabled() {
        match db
            .evict_stale(&crate::tuning::EvictionConfig::default())
            .await
        {
            Ok(eviction) => report.memories_evicted = eviction.archived,
            Err(error) => report.errors.push(format!("evict: {error}")),
        }
    }

    // Record the run even when a step failed: a step that fails every time
    // must not turn the daily run into one every scheduler poll.
    if let Err(error) = db
        .set_app_metadata(HOUSEKEEPING_LAST_RUN_KEY, &now_epoch.to_string())
        .await
    {
        report.errors.push(format!("record run: {error}"));
    }
    report
}

/// What the one-time cleanups of data written under older rules changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OneTimeCleanupReport {
    pub hub_links_removed: u64,
    pub primaries_moved_off_archived: u64,
    /// The one-time re-filing of every memory whose main topic is archived
    /// (UX-GRAPH-8): moved to another shown topic, or left with none.
    pub archived_primaries: crate::db::topic_filing::RefileReport,
    pub duplicate_recaps_removed: usize,
    pub errors: Vec<String>,
}

/// Clean, once per database, what older rules wrote: links from a memory to
/// a hub topic when the memory also has a more specific one (its main topic
/// moving to a shown topic before an archived one), and recaps that
/// cover the same memories as an older recap. Each cleanup records itself in
/// app metadata (which also makes it undoable) and returns zero afterwards.
///
/// Reweave and the recap pass also call these, but only a full foreground
/// steep runs those phases, so the scheduler calls this directly: otherwise
/// an upgraded library would keep the old links and duplicates. Model-free.
pub async fn run_one_time_cleanups(db: &MemoryDB) -> OneTimeCleanupReport {
    let mut report = OneTimeCleanupReport::default();
    match db.unfile_old_hub_links_once().await {
        Ok(count) => report.hub_links_removed = count,
        Err(error) => report.errors.push(format!("hub links: {error}")),
    }
    match db.move_unfiled_primaries_off_archived_topics_once().await {
        Ok(count) => report.primaries_moved_off_archived = count,
        Err(error) => report.errors.push(format!("archived main topics: {error}")),
    }
    match db.refile_archived_primaries_once().await {
        Ok(refiled) => report.archived_primaries = refiled,
        Err(error) => report
            .errors
            .push(format!("archived main topics v1: {error}")),
    }
    match crate::synthesis::recaps::dedupe_old_recaps_once(db).await {
        Ok(count) => report.duplicate_recaps_removed = count,
        Err(error) => report.errors.push(format!("duplicate recaps: {error}")),
    }
    report
}

#[cfg(test)]
#[path = "housekeeping_test.rs"]
mod tests;
