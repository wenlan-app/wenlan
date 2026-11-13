//! A log line for a save that took long enough for an AI tool to give up.
//!
//! The line is written when the guard drops, so a save the caller stopped
//! waiting for (its request future dropped) is logged too, as cancelled.

use std::time::{Duration, Instant};

/// A save slower than this is logged with its duration.
pub(crate) const SLOW_SAVE: Duration = Duration::from_secs(30);

pub(crate) struct SlowSaveLog {
    what: &'static str,
    started: Instant,
    outcome: Option<&'static str>,
}

impl SlowSaveLog {
    pub(crate) fn start(what: &'static str) -> Self {
        Self {
            what,
            started: Instant::now(),
            outcome: None,
        }
    }

    /// Record how the save ended; without this the drop says "cancelled".
    pub(crate) fn finish<T, E>(&mut self, result: &Result<T, E>) {
        self.outcome = Some(if result.is_ok() { "saved" } else { "failed" });
    }
}

impl Drop for SlowSaveLog {
    fn drop(&mut self) {
        let took = self.started.elapsed();
        if took >= SLOW_SAVE {
            tracing::warn!(
                "[save] {} took {:.1}s ({})",
                self.what,
                took.as_secs_f64(),
                self.outcome
                    .unwrap_or("cancelled: the caller stopped waiting")
            );
        }
    }
}
