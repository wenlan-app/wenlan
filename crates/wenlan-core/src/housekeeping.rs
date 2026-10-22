// SPDX-License-Identifier: Apache-2.0
//! Daily housekeeping that needs no model: confidence decay, promotion of
//! settled memories, old rejection pruning, orphaned page-source cleanup and,
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

#[cfg(test)]
#[path = "housekeeping_test.rs"]
mod tests;
