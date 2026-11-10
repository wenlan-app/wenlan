// SPDX-License-Identifier: Apache-2.0
//! Bounded, event-driven priority lane for freshly imported memories.
//!
//! An import should become useful while the user is still looking at it.  This
//! module only owns the small in-memory state that gives the existing ambient
//! selectors a short head start.  It deliberately does not create another
//! worker, queue, or database table.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::sync::{watch, Notify};

use super::{
    AmbientJob, HostActivitySnapshot, ResourceBlockReason, ResourcePolicy, ResourceSnapshot,
    ResourceStatus, WriteSignal, ON_DEVICE_INFERENCE_HEADROOM_BYTES,
};

pub(super) const IMPORT_PRIORITY_METADATA_KEY: &str = "import_priority_until_v1";
pub(super) const IMPORT_PRIORITY_WINDOW: Duration = Duration::from_secs(10 * 60);
pub(super) const IMPORT_PRIORITY_MAX_AMBIENT_SLICES: u8 = 64;
pub(super) const IMPORT_PRIORITY_TICK: Duration = Duration::from_secs(2);

const IMPORT_PRIORITY_JOBS: [AmbientJob; 5] = [
    AmbientJob::Classification,
    AmbientJob::StructuredExtract,
    AmbientJob::Entity,
    AmbientJob::Title,
    AmbientJob::PageGrowth,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ImportPriorityRequest {
    pub(super) deadline_epoch: i64,
    pub(super) generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ImportSynthesisPhase {
    Detect,
    Emergence,
}

/// What one synthesis turn of the lane did. Emergence writes at most one
/// page per turn, so the lane keeps it going while material remains.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct ImportSynthesisOutcome {
    pub(super) progressed: bool,
    pub(super) more: bool,
    pub(super) pages_created: usize,
}

/// Result of one model-free document preparation attempt in the import lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DocumentPrepOutcome {
    /// No fresh document was claimable.
    Idle,
    /// The queue claim itself failed. Nothing ran, so no slice is spent and
    /// prep stays pending for the next tick.
    ClaimFailed,
    /// A document was parsed, embedded and parked (or requeued because the
    /// file changed after the claim).
    Prepared,
    /// The document failed and paused until `retry_at`.
    RetryableFailure { retry_at: Option<i64> },
    /// The document failed and reached the queue's attempt cap; it is never
    /// claimed again.
    Exhausted,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ImportPrioritySnapshot {
    pub(super) deadline: Instant,
    pub(super) deadline_epoch: i64,
    pub(super) generation: u64,
    pub(super) ambient_slices: u8,
    pub(super) next_job: AmbientJob,
    pub(super) synthesis_phase: Option<ImportSynthesisPhase>,
    document_prep_drained: bool,
    /// Earliest pending in-window retry. Set exactly when at least one
    /// retryable document still holds the lane.
    document_prep_retry_at: Option<i64>,
    prefer_document_prep: bool,
    inference_round_idle: bool,
    /// Detect/Emergence passes started again after a pass that wrote no page.
    pub(super) synthesis_retries: u8,
}

impl ImportPrioritySnapshot {
    /// Prep may still have work: the shared slice budget is not spent and
    /// either no claim has come back empty yet or a paused document is due to
    /// retry before the deadline.
    ///
    /// Intentional bound: prep shares the 64-slice budget with inference, so a
    /// folder with more fresh files than the budget leaves the rest queued for
    /// the ordinary idle-gated ambient lap after the lane finishes.
    pub(super) fn document_prep_open(&self) -> bool {
        self.ambient_slices < IMPORT_PRIORITY_MAX_AMBIENT_SLICES
            && (!self.document_prep_drained || self.document_prep_retry_at.is_some())
    }

    fn document_prep_due(&self, now_epoch: i64) -> bool {
        self.document_prep_open()
            && (!self.document_prep_drained
                || self
                    .document_prep_retry_at
                    .is_some_and(|retry_at| retry_at <= now_epoch))
    }
}

/// State is bounded by both a wall-clock deadline and an ambient-slice count.
/// The scheduler may be restarted at any point; only the wall-clock deadline
/// is persisted, so a restart never revives an old import forever.
#[derive(Debug, Default)]
pub(super) struct ImportPriority {
    deadline: Option<Instant>,
    deadline_epoch: i64,
    generation: u64,
    ambient_slices: u8,
    next_job: usize,
    round_attempts: u8,
    round_had_work: bool,
    synthesis_phase: Option<ImportSynthesisPhase>,
    started_logged: bool,
    synthesis_finished: bool,
    document_prep_drained: bool,
    /// Retry times of documents that failed inside the window and have not
    /// been confirmed resolved. The lane wakes for the earliest and stays open
    /// until every one is resolved. Bounded: each entry spends a slice, so it
    /// never exceeds `IMPORT_PRIORITY_MAX_AMBIENT_SLICES`.
    document_prep_retries: Vec<i64>,
    prefer_document_prep: bool,
    inference_round_idle: bool,
    /// A document was prepared or an inference job found work since this
    /// pass began. A pass that wrote no page is tried again only then.
    pass_had_work: bool,
    pages_written: usize,
    synthesis_retries: u8,
}

impl ImportPriority {
    fn deadline_epoch_now() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs() as i64)
            .unwrap_or(0)
            .saturating_add(IMPORT_PRIORITY_WINDOW.as_secs() as i64)
    }

    fn activate(&mut self, deadline_epoch: i64, now: Instant, now_epoch: i64) {
        let remaining = deadline_epoch.saturating_sub(now_epoch);
        if remaining <= 0 {
            self.deadline = None;
            self.deadline_epoch = 0;
            return;
        }
        // Be defensive about hand-edited/corrupt metadata. A persisted import
        // can never extend a fresh scheduler window beyond ten minutes.
        let bounded_remaining = remaining.min(IMPORT_PRIORITY_WINDOW.as_secs() as i64);
        self.deadline = Some(now + Duration::from_secs(bounded_remaining as u64));
        self.deadline_epoch = now_epoch.saturating_add(bounded_remaining);
        self.ambient_slices = 0;
        self.next_job = 0;
        self.round_attempts = 0;
        self.round_had_work = false;
        self.synthesis_phase = None;
        self.started_logged = false;
        self.synthesis_finished = false;
        self.document_prep_drained = false;
        self.document_prep_retries.clear();
        self.prefer_document_prep = true;
        self.inference_round_idle = false;
        self.pass_had_work = false;
        self.pages_written = 0;
        self.synthesis_retries = 0;
    }

    pub(super) fn prepare_request(&mut self, now: Instant) -> ImportPriorityRequest {
        self.generation = self.generation.wrapping_add(1);
        let now_epoch = chrono::Utc::now().timestamp();
        let deadline_epoch = Self::deadline_epoch_now();
        self.activate(deadline_epoch, now, now_epoch);
        ImportPriorityRequest {
            deadline_epoch,
            generation: self.generation,
        }
    }

    pub(super) fn restore(&mut self, deadline_epoch: i64, now: Instant, now_epoch: i64) -> bool {
        if self.deadline.is_some() {
            return true;
        }
        if deadline_epoch <= now_epoch {
            return false;
        }
        self.generation = self.generation.wrapping_add(1);
        self.activate(deadline_epoch, now, now_epoch);
        self.deadline.is_some()
    }

    pub(super) fn snapshot(&self) -> Option<ImportPrioritySnapshot> {
        let deadline = self.deadline?;
        Some(ImportPrioritySnapshot {
            deadline,
            deadline_epoch: self.deadline_epoch,
            generation: self.generation,
            ambient_slices: self.ambient_slices,
            next_job: IMPORT_PRIORITY_JOBS[self.next_job],
            synthesis_phase: self.synthesis_phase,
            document_prep_drained: self.document_prep_drained,
            document_prep_retry_at: self.document_prep_retries.iter().copied().min(),
            prefer_document_prep: self.prefer_document_prep,
            inference_round_idle: self.inference_round_idle,
            synthesis_retries: self.synthesis_retries,
        })
    }

    fn document_prep_open(&self) -> bool {
        self.snapshot()
            .is_some_and(|snapshot| snapshot.document_prep_open())
    }

    pub(super) fn mark_started(&mut self) -> bool {
        if self.started_logged {
            false
        } else {
            self.started_logged = true;
            true
        }
    }

    pub(super) fn note_ambient(&mut self, generation: u64, selected: bool) -> bool {
        if self.generation != generation {
            return false;
        }
        self.ambient_slices = self
            .ambient_slices
            .saturating_add(1)
            .min(IMPORT_PRIORITY_MAX_AMBIENT_SLICES);
        self.round_attempts = self.round_attempts.saturating_add(1);
        self.round_had_work |= selected;
        self.pass_had_work |= selected;
        self.next_job = (self.next_job + 1) % IMPORT_PRIORITY_JOBS.len();
        if self.round_attempts == IMPORT_PRIORITY_JOBS.len() as u8 {
            if !self.round_had_work {
                if self.document_prep_open() {
                    // Fresh documents remain: an empty inference round must
                    // not start synthesis (and so finish the lane). Inference
                    // sits out until prep closes.
                    self.inference_round_idle = true;
                } else {
                    self.synthesis_phase = Some(ImportSynthesisPhase::Detect);
                }
            }
            self.round_attempts = 0;
            self.round_had_work = false;
        }
        if self.ambient_slices >= IMPORT_PRIORITY_MAX_AMBIENT_SLICES {
            self.synthesis_phase = Some(ImportSynthesisPhase::Detect);
        }
        self.prefer_document_prep = true;
        true
    }

    /// Record one document prep attempt. Prep spends the shared slice budget
    /// but never rotates the inference jobs or counts toward an inference
    /// round.
    pub(super) fn note_document_slice(
        &mut self,
        generation: u64,
        outcome: DocumentPrepOutcome,
        now_epoch: i64,
    ) -> bool {
        if self.generation != generation {
            return false;
        }
        match outcome {
            DocumentPrepOutcome::ClaimFailed => return true,
            DocumentPrepOutcome::Idle => {
                self.document_prep_drained = true;
                // Nothing was claimable, so every retry already due has been
                // resolved (claimed or rescheduled); later ones still hold
                // the lane.
                self.document_prep_retries
                    .retain(|retry_at| *retry_at > now_epoch);
            }
            DocumentPrepOutcome::Prepared
            | DocumentPrepOutcome::RetryableFailure { .. }
            | DocumentPrepOutcome::Exhausted => {
                self.pass_had_work |= outcome == DocumentPrepOutcome::Prepared;
                self.ambient_slices = self
                    .ambient_slices
                    .saturating_add(1)
                    .min(IMPORT_PRIORITY_MAX_AMBIENT_SLICES);
                if let DocumentPrepOutcome::RetryableFailure {
                    retry_at: Some(retry_at),
                } = outcome
                {
                    // Only a retry the lane can still reach holds it open.
                    if retry_at < self.deadline_epoch {
                        self.document_prep_retries.push(retry_at);
                    }
                }
                if self.ambient_slices >= IMPORT_PRIORITY_MAX_AMBIENT_SLICES {
                    self.synthesis_phase = Some(ImportSynthesisPhase::Detect);
                }
            }
        }
        self.prefer_document_prep = false;
        if self.inference_round_idle && !self.document_prep_open() {
            // Prep just closed. The prepared documents may carry inference
            // work, so run one fresh full round before synthesis.
            self.inference_round_idle = false;
            self.round_attempts = 0;
            self.round_had_work = false;
        }
        true
    }

    /// Record one synthesis turn. Detect is followed by Emergence, which runs
    /// one new page per turn while material remains. When an Emergence pass ends:
    /// it finishes the lane once it has written a page; a pass that wrote
    /// nothing but whose prep/inference did work (the material was still
    /// being linked, or the slice budget ran out first) starts again with a
    /// fresh slice budget, so Detect runs after that round; a pass with no
    /// work at all finishes the lane. The window bounds every pass.
    pub(super) fn note_synthesis_phase(
        &mut self,
        generation: u64,
        phase: ImportSynthesisPhase,
        outcome: ImportSynthesisOutcome,
    ) -> bool {
        if self.generation != generation {
            return false;
        }
        self.prefer_document_prep = true;
        if phase == ImportSynthesisPhase::Detect {
            self.synthesis_phase = Some(ImportSynthesisPhase::Emergence);
            return true;
        }
        self.pages_written = self.pages_written.saturating_add(outcome.pages_created);
        if outcome.progressed && outcome.more {
            self.synthesis_phase = Some(ImportSynthesisPhase::Emergence);
            return true;
        }
        self.synthesis_phase = None;
        if self.pages_written == 0 && self.pass_had_work {
            self.pass_had_work = false;
            self.synthesis_retries = self.synthesis_retries.saturating_add(1);
            self.ambient_slices = 0;
            self.round_attempts = 0;
            self.round_had_work = false;
            self.inference_round_idle = false;
        } else {
            self.synthesis_finished = true;
        }
        true
    }

    /// End the window's page writing early (the paid writer's daily cap).
    pub(super) fn finish_synthesis(&mut self, generation: u64) -> bool {
        if self.generation != generation {
            return false;
        }
        self.synthesis_phase = None;
        self.synthesis_finished = true;
        true
    }

    pub(super) fn should_finish(&self, now: Instant) -> bool {
        self.deadline.is_none_or(|deadline| now >= deadline) || self.synthesis_finished
    }

    pub(super) fn generation(&self) -> u64 {
        self.generation
    }

    pub(super) fn clear_if_generation(&mut self, generation: u64) -> bool {
        if self.generation != generation {
            return false;
        }
        self.deadline = None;
        self.deadline_epoch = 0;
        self.synthesis_phase = None;
        self.started_logged = false;
        self.synthesis_finished = false;
        true
    }

    pub(super) fn active(&self) -> bool {
        self.deadline
            .is_some_and(|deadline| Instant::now() < deadline)
    }
}

impl WriteSignal {
    /// Return a request token and activate the bounded state without waking
    /// the scheduler. Routes persist the token's deadline first, then call
    /// `signal_import_request`, which closes the persistence race with a
    /// concurrent completion clear.
    pub(super) fn prepare_import_request(&self) -> ImportPriorityRequest {
        self.import_priority
            .lock()
            .expect("import priority mutex poisoned")
            .prepare_request(Instant::now())
    }

    /// Activate and wake an already-persisted import request. A stale token
    /// cannot wake a newer generation, but a notification itself is harmless
    /// because `Notify` coalesces wakeups.
    pub(super) fn signal_import_request(&self, request: ImportPriorityRequest) {
        if self
            .import_priority
            .lock()
            .expect("import priority mutex poisoned")
            .generation()
            == request.generation
        {
            // Both startup admission and the scheduler may be waiting. Wake
            // all current waiters and retain one permit for a registration race.
            self.import_wake.notify_waiters();
            self.import_wake.notify_one();
        }
    }

    /// Convenience for non-route callers that do not need to persist a
    /// deadline. Import routes use the prepare/persist/signal sequence above.
    #[cfg(test)]
    pub(super) fn request_import(&self) -> ImportPriorityRequest {
        let request = self.prepare_import_request();
        self.signal_import_request(request);
        request
    }

    /// Persist and publish one import request under the same async mutex used
    /// by completion. This makes the deadline write and the wake one atomic
    /// handoff with respect to a scheduler completion: a newer request can
    /// never have its durable deadline erased by an older clear.
    pub(crate) async fn persist_and_request_import(
        &self,
        db: &wenlan_core::db::MemoryDB,
        batch_id: Option<&str>,
    ) -> Result<(), wenlan_core::WenlanError> {
        let _persist_guard = self.import_persist_lock.lock().await;
        let request = self.prepare_import_request();
        db.set_app_metadata(
            IMPORT_PRIORITY_METADATA_KEY,
            &request.deadline_epoch.to_string(),
        )
        .await?;
        if let Some(batch_id) = batch_id {
            db.persist_import_batch_priority(batch_id, request.deadline_epoch)
                .await?;
        }
        self.signal_import_request(request);
        Ok(())
    }

    pub(super) fn restore_import_priority(&self, deadline_epoch: i64) -> bool {
        self.import_priority
            .lock()
            .expect("import priority mutex poisoned")
            .restore(
                deadline_epoch,
                Instant::now(),
                chrono::Utc::now().timestamp(),
            )
    }

    pub(super) fn import_priority_snapshot(&self) -> Option<ImportPrioritySnapshot> {
        self.import_priority
            .lock()
            .expect("import priority mutex poisoned")
            .snapshot()
    }

    pub(super) fn import_priority_mark_started(&self) -> bool {
        self.import_priority
            .lock()
            .expect("import priority mutex poisoned")
            .mark_started()
    }

    pub(super) fn import_priority_note_ambient(&self, generation: u64, selected: bool) -> bool {
        self.import_priority
            .lock()
            .expect("import priority mutex poisoned")
            .note_ambient(generation, selected)
    }

    pub(super) fn import_priority_note_document(
        &self,
        generation: u64,
        outcome: DocumentPrepOutcome,
        now_epoch: i64,
    ) -> bool {
        self.import_priority
            .lock()
            .expect("import priority mutex poisoned")
            .note_document_slice(generation, outcome, now_epoch)
    }

    pub(super) fn import_priority_note_phase(
        &self,
        generation: u64,
        phase: ImportSynthesisPhase,
        outcome: ImportSynthesisOutcome,
    ) -> bool {
        self.import_priority
            .lock()
            .expect("import priority mutex poisoned")
            .note_synthesis_phase(generation, phase, outcome)
    }

    pub(super) fn import_priority_finish_synthesis(&self, generation: u64) -> bool {
        self.import_priority
            .lock()
            .expect("import priority mutex poisoned")
            .finish_synthesis(generation)
    }

    pub(super) fn import_priority_should_finish(&self, now: Instant) -> bool {
        self.import_priority
            .lock()
            .expect("import priority mutex poisoned")
            .should_finish(now)
    }

    pub(super) fn clear_import_priority_if_generation(&self, generation: u64) -> bool {
        self.import_priority
            .lock()
            .expect("import priority mutex poisoned")
            .clear_if_generation(generation)
    }

    pub(super) async fn finish_import_priority(
        &self,
        db: &wenlan_core::db::MemoryDB,
        generation: u64,
    ) -> Result<bool, wenlan_core::WenlanError> {
        let _persist_guard = self.import_persist_lock.lock().await;
        if self
            .import_priority
            .lock()
            .expect("import priority mutex poisoned")
            .generation()
            != generation
        {
            return Ok(false);
        }
        db.set_app_metadata(IMPORT_PRIORITY_METADATA_KEY, "0")
            .await?;
        Ok(self.clear_import_priority_if_generation(generation))
    }

    pub(super) fn import_priority_active(&self) -> bool {
        self.import_priority
            .lock()
            .expect("import priority mutex poisoned")
            .active()
    }

    pub(super) fn import_wake(&self) -> std::sync::Arc<Notify> {
        self.import_wake.clone()
    }
}

/// Watches the page writer (the `synthesis_source` pin and its provider) from
/// poll to poll, to notice when it first becomes usable: a pin is set, the
/// pinned on-device model finishes loading, or a key arrives. The first poll
/// only records what it sees, so a daemon start with a ready writer does not
/// count as an arrival.
#[derive(Debug, Default)]
pub(super) struct PageWriterArrival {
    last: Option<(Option<wenlan_core::refinery::SynthesisSource>, bool)>,
}

impl PageWriterArrival {
    /// True when the writer is usable now and was not usable (or was another
    /// source) on the previous poll.
    pub(super) fn observe(
        &mut self,
        pin: Option<wenlan_core::refinery::SynthesisSource>,
        usable: bool,
    ) -> bool {
        let previous = self.last.replace((pin, usable));
        usable
            && previous.is_some_and(|(previous_pin, previous_usable)| {
                !previous_usable || previous_pin != pin
            })
    }
}

/// The import wake is a coalesced event, so a fresh import does not wait for
/// either the startup delay or the ordinary 30-second poll interval.
pub(super) async fn sleep_or_import_or_shutdown(
    shutdown: &mut watch::Receiver<bool>,
    import_wake: &Notify,
    duration: Duration,
) -> bool {
    if crate::lifecycle::shutdown_requested(shutdown) {
        return true;
    }
    let notified = import_wake.notified();
    tokio::select! {
        _ = tokio::time::sleep(duration) => false,
        _ = notified => false,
        result = shutdown.changed() => result.is_err() || crate::lifecycle::shutdown_requested(shutdown),
    }
}

/// Import priority ignores foreground activity and the CPU threshold, but
/// continues to honour memory reserve, model headroom, thermal pressure,
/// unavailable host signals, and an in-flight startup model reservation.
pub(super) fn import_priority_block_reason(
    status: ResourceStatus,
    host_activity: HostActivitySnapshot,
    startup_model_load_reserved: bool,
    route_uses_on_device: bool,
) -> Option<ResourceBlockReason> {
    import_priority_block_reason_with_headroom(
        status,
        host_activity,
        startup_model_load_reserved,
        if route_uses_on_device {
            ON_DEVICE_INFERENCE_HEADROOM_BYTES
        } else {
            0
        },
    )
}

/// Variant used while admitting the startup model itself. Its working set is
/// the extra memory reserve, rather than the regular ambient two-gibibyte
/// headroom, and the startup load is the operation being admitted so it must
/// not veto itself as an already-reserved route.
pub(super) fn import_priority_block_reason_with_headroom(
    status: ResourceStatus,
    host_activity: HostActivitySnapshot,
    startup_model_load_reserved: bool,
    additional_memory_headroom_bytes: u64,
) -> Option<ResourceBlockReason> {
    let snapshot: ResourceSnapshot = if let Some(snapshot) = status.snapshot {
        snapshot
    } else {
        return Some(
            status
                .block_reason
                .filter(|reason| {
                    matches!(
                        reason,
                        ResourceBlockReason::Warming | ResourceBlockReason::Unavailable
                    )
                })
                .unwrap_or(ResourceBlockReason::Unavailable),
        );
    };
    let memory_policy = ResourcePolicy::conservative()
        .with_additional_memory_headroom(additional_memory_headroom_bytes);
    if let Some(reason) = memory_policy.block_reason(snapshot) {
        if !matches!(reason, ResourceBlockReason::CpuBusy) {
            return Some(reason);
        }
    }
    if startup_model_load_reserved && additional_memory_headroom_bytes > 0 {
        return Some(ResourceBlockReason::Warming);
    }
    match host_activity {
        HostActivitySnapshot::Unavailable => Some(ResourceBlockReason::HostActivityUnavailable),
        HostActivitySnapshot::Observed { thermal_state, .. } if thermal_state != 0 => {
            Some(ResourceBlockReason::ThermalPressure)
        }
        _ => None,
    }
}

/// Model-free document prep loads no inference model, so it gets no on-device
/// inference headroom. It still honours the memory reserve, thermal pressure,
/// unavailable host signals and an in-flight startup model reservation, since
/// parsing and embedding compete for the memory that reservation protects.
pub(super) fn import_document_prep_block_reason(
    status: ResourceStatus,
    host_activity: HostActivitySnapshot,
    startup_model_load_reserved: bool,
) -> Option<ResourceBlockReason> {
    import_priority_block_reason_with_headroom(status, host_activity, false, 0)
        .or(startup_model_load_reserved.then_some(ResourceBlockReason::Warming))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ImportLaneWait {
    Resource(ResourceBlockReason),
    DocumentPrepRetry,
    PinnedProviderLoading,
}

impl ImportLaneWait {
    pub(super) fn log_reason(self) -> String {
        match self {
            Self::Resource(reason) => format!("{reason:?}"),
            Self::DocumentPrepRetry => "document_prep_retry".to_string(),
            Self::PinnedProviderLoading => "pinned_provider_loading".to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ImportLaneTurn {
    DocumentPrep,
    /// A synthesis phase when one is due, otherwise the next inference job.
    Inference,
    Wait(ImportLaneWait),
    FinishNoAuthorizedProvider,
}

/// Choose this tick's import-lane turn. Prep and inference have separate
/// admission; while both can run they alternate. The lane only finishes for
/// want of a provider once prep has closed, so a lock-contended tick, a
/// retryable failure waiting inside the window, or an exhausted document never
/// ends it while fresh documents remain. The caller takes the ambient run lock
/// only for a turn that runs, and records nothing when that lock is busy.
pub(super) fn plan_import_lane_turn(
    import: &ImportPrioritySnapshot,
    now_epoch: i64,
    inference_block: Option<ResourceBlockReason>,
    document_prep_block: Option<ResourceBlockReason>,
    provider_available: bool,
    everyday_pinned: bool,
) -> ImportLaneTurn {
    let prep_open = import.document_prep_open();
    let prep_ready = import.document_prep_due(now_epoch) && document_prep_block.is_none();
    let inference_ready = provider_available
        && inference_block.is_none()
        && !(prep_open && import.inference_round_idle);
    if prep_ready && (import.prefer_document_prep || !inference_ready) {
        return ImportLaneTurn::DocumentPrep;
    }
    if inference_ready {
        return ImportLaneTurn::Inference;
    }
    if prep_open {
        return ImportLaneTurn::Wait(
            document_prep_block.map_or(ImportLaneWait::DocumentPrepRetry, ImportLaneWait::Resource),
        );
    }
    if let Some(reason) = inference_block {
        return ImportLaneTurn::Wait(ImportLaneWait::Resource(reason));
    }
    if everyday_pinned {
        return ImportLaneTurn::Wait(ImportLaneWait::PinnedProviderLoading);
    }
    ImportLaneTurn::FinishNoAuthorizedProvider
}

/// True when this tick's import-priority lane would run work: a request is
/// active, it is not past its deadline or finished, and nothing it still honours
/// holds it. The scheduler publishes this beside the ordinary gate so a reader
/// can tell an import still running from one waiting like everything else.
pub(super) fn import_lane_open(
    import: Option<ImportPrioritySnapshot>,
    should_finish: bool,
    now: Instant,
    block_reason: Option<ResourceBlockReason>,
) -> bool {
    import.is_some_and(|import| now < import.deadline && !should_finish && block_reason.is_none())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_lane_is_open_only_while_its_window_can_run() {
        let now = Instant::now();
        let mut priority = ImportPriority::default();
        assert!(!import_lane_open(priority.snapshot(), false, now, None));

        priority.prepare_request(now);
        let active = priority.snapshot();
        assert!(import_lane_open(active, false, now, None));
        assert!(!import_lane_open(active, true, now, None));
        assert!(!import_lane_open(
            active,
            false,
            now,
            Some(ResourceBlockReason::MemoryPressure)
        ));
        assert!(!import_lane_open(
            active,
            false,
            now + IMPORT_PRIORITY_WINDOW,
            None
        ));
    }

    fn admitted(cpu_usage_percent: f32, available_memory_bytes: u64) -> ResourceStatus {
        ResourceStatus {
            admitted: true,
            snapshot: Some(ResourceSnapshot {
                cpu_usage_percent,
                available_memory_bytes,
                total_memory_bytes: 16 * super::super::GIB,
            }),
            block_reason: None,
        }
    }

    #[test]
    fn priority_round_rotates_only_import_jobs_and_caps_slices() {
        let now = Instant::now();
        let mut priority = ImportPriority::default();
        let generation = priority.prepare_request(now).generation;
        // No fresh documents: an empty round starts synthesis only once prep
        // has drained (see empty_inference_round_with_pending_documents_does_not_finish_lane).
        priority.note_document_slice(
            generation,
            DocumentPrepOutcome::Idle,
            chrono::Utc::now().timestamp(),
        );
        let mut jobs = Vec::new();
        for _ in 0..IMPORT_PRIORITY_JOBS.len() {
            jobs.push(priority.snapshot().expect("active priority").next_job);
            priority.note_ambient(generation, false);
        }
        assert_eq!(
            jobs,
            vec![
                AmbientJob::Classification,
                AmbientJob::StructuredExtract,
                AmbientJob::Entity,
                AmbientJob::Title,
                AmbientJob::PageGrowth,
            ]
        );
        assert_eq!(
            priority.snapshot().unwrap().synthesis_phase,
            Some(ImportSynthesisPhase::Detect)
        );
        for _ in 0..IMPORT_PRIORITY_MAX_AMBIENT_SLICES {
            priority.note_ambient(generation, false);
        }
        assert_eq!(
            priority.snapshot().unwrap().ambient_slices,
            IMPORT_PRIORITY_MAX_AMBIENT_SLICES
        );
    }

    #[test]
    fn completed_phases_finish_and_new_request_is_not_advanced_by_old_work() {
        let now = Instant::now();
        let mut priority = ImportPriority::default();
        let old = priority.prepare_request(now).generation;
        for _ in 0..IMPORT_PRIORITY_MAX_AMBIENT_SLICES {
            priority.note_ambient(old, true);
        }
        assert_eq!(
            priority.snapshot().unwrap().synthesis_phase,
            Some(ImportSynthesisPhase::Detect)
        );
        assert!(!priority.should_finish(now));
        priority.note_synthesis_phase(
            old,
            ImportSynthesisPhase::Detect,
            ImportSynthesisOutcome::default(),
        );
        priority.note_synthesis_phase(old, ImportSynthesisPhase::Emergence, wrote_one_page());
        assert!(priority.should_finish(now));
        let fresh = priority.prepare_request(now).generation;
        assert_ne!(old, fresh);
        assert!(!priority.note_synthesis_phase(
            old,
            ImportSynthesisPhase::Emergence,
            wrote_one_page()
        ));
        assert!(!priority.note_ambient(old, true));
        assert!(!priority.clear_if_generation(old));
        assert!(!priority.should_finish(now));
        assert_eq!(priority.snapshot().unwrap().ambient_slices, 0);
    }

    fn wrote_one_page() -> ImportSynthesisOutcome {
        ImportSynthesisOutcome {
            progressed: true,
            more: false,
            pages_created: 1,
        }
    }

    fn wrote_nothing() -> ImportSynthesisOutcome {
        ImportSynthesisOutcome {
            progressed: false,
            more: false,
            pages_created: 0,
        }
    }

    /// Drive one empty inference round so the lane starts Detect.
    fn empty_round(priority: &mut ImportPriority, generation: u64) {
        for _ in 0..IMPORT_PRIORITY_JOBS.len() {
            priority.note_ambient(generation, false);
        }
    }

    #[test]
    fn a_pass_that_wrote_no_page_runs_detect_again_after_the_prep_round() {
        let now = Instant::now();
        let epoch = chrono::Utc::now().timestamp();
        let mut priority = ImportPriority::default();
        let generation = priority.prepare_request(now).generation;
        // The measured failure: prep spends the whole slice budget, so Detect
        // starts before anything is linked and finds nothing.
        for _ in 0..IMPORT_PRIORITY_MAX_AMBIENT_SLICES {
            priority.note_document_slice(generation, DocumentPrepOutcome::Prepared, epoch);
        }
        assert_eq!(
            priority.snapshot().unwrap().synthesis_phase,
            Some(ImportSynthesisPhase::Detect)
        );
        priority.note_synthesis_phase(
            generation,
            ImportSynthesisPhase::Detect,
            ImportSynthesisOutcome::default(),
        );
        priority.note_synthesis_phase(generation, ImportSynthesisPhase::Emergence, wrote_nothing());

        // The lane does not end: prep and inference get a fresh budget first.
        assert!(!priority.should_finish(now));
        let after = priority.snapshot().unwrap();
        assert_eq!(after.synthesis_phase, None);
        assert_eq!(after.synthesis_retries, 1);
        assert_eq!(after.ambient_slices, 0);
        assert!(
            after.document_prep_open(),
            "the rest of the files are prepared"
        );

        // Prep drains, inference finds work, then a quiet round: Detect again.
        priority.note_document_slice(generation, DocumentPrepOutcome::Idle, epoch);
        priority.note_ambient(generation, true);
        for _ in 1..IMPORT_PRIORITY_JOBS.len() {
            priority.note_ambient(generation, false);
        }
        assert_eq!(priority.snapshot().unwrap().synthesis_phase, None);
        empty_round(&mut priority, generation);
        assert_eq!(
            priority.snapshot().unwrap().synthesis_phase,
            Some(ImportSynthesisPhase::Detect)
        );
        priority.note_synthesis_phase(
            generation,
            ImportSynthesisPhase::Detect,
            ImportSynthesisOutcome::default(),
        );
        // Emergence writes one page per turn while material remains.
        priority.note_synthesis_phase(
            generation,
            ImportSynthesisPhase::Emergence,
            ImportSynthesisOutcome {
                progressed: true,
                more: true,
                pages_created: 1,
            },
        );
        assert_eq!(
            priority.snapshot().unwrap().synthesis_phase,
            Some(ImportSynthesisPhase::Emergence)
        );
        assert!(!priority.should_finish(now));
        priority.note_synthesis_phase(
            generation,
            ImportSynthesisPhase::Emergence,
            wrote_one_page(),
        );
        assert_eq!(priority.pages_written, 2);
        assert!(priority.should_finish(now), "pages were written: done");
    }

    #[test]
    fn a_pass_with_no_work_and_no_page_finishes_the_lane() {
        let now = Instant::now();
        let epoch = chrono::Utc::now().timestamp();
        let mut priority = ImportPriority::default();
        let generation = priority.prepare_request(now).generation;
        priority.note_document_slice(generation, DocumentPrepOutcome::Idle, epoch);
        empty_round(&mut priority, generation);
        priority.note_synthesis_phase(
            generation,
            ImportSynthesisPhase::Detect,
            ImportSynthesisOutcome::default(),
        );
        priority.note_synthesis_phase(generation, ImportSynthesisPhase::Emergence, wrote_nothing());
        assert!(priority.should_finish(now));
        assert_eq!(priority.snapshot().unwrap().synthesis_retries, 0);
    }

    #[test]
    fn the_page_cap_finishes_synthesis_for_this_generation_only() {
        let now = Instant::now();
        let mut priority = ImportPriority::default();
        let old = priority.prepare_request(now).generation;
        let fresh = priority.prepare_request(now).generation;
        assert!(!priority.finish_synthesis(old));
        assert!(!priority.should_finish(now));
        assert!(priority.finish_synthesis(fresh));
        assert!(priority.should_finish(now));
    }

    #[test]
    fn a_late_page_writer_arms_once_when_it_becomes_usable() {
        use wenlan_core::refinery::SynthesisSource;
        let mut arrival = PageWriterArrival::default();
        // Daemon start with a ready writer only records it.
        assert!(!arrival.observe(Some(SynthesisSource::Anthropic), true));
        assert!(!arrival.observe(Some(SynthesisSource::Anthropic), true));

        // The pinned on-device model is still loading, then finishes.
        let mut arrival = PageWriterArrival::default();
        assert!(!arrival.observe(Some(SynthesisSource::OnDevice), false));
        assert!(!arrival.observe(Some(SynthesisSource::OnDevice), false));
        assert!(arrival.observe(Some(SynthesisSource::OnDevice), true));
        assert!(!arrival.observe(Some(SynthesisSource::OnDevice), true));

        // No pin at import time, a pin set later.
        let mut arrival = PageWriterArrival::default();
        assert!(!arrival.observe(None, false));
        assert!(arrival.observe(Some(SynthesisSource::External), true));
        // Switching to another ready source counts as a new writer.
        assert!(arrival.observe(Some(SynthesisSource::OnDevice), true));
    }

    #[test]
    fn expired_restore_is_rejected_and_deadline_is_bounded() {
        let mut priority = ImportPriority::default();
        let now = Instant::now();
        let epoch = chrono::Utc::now().timestamp();
        assert!(!priority.restore(epoch - 1, now, epoch));
        assert!(priority.restore(epoch + 3600, now, epoch));
        let snapshot = priority.snapshot().unwrap();
        assert!(snapshot.deadline <= now + IMPORT_PRIORITY_WINDOW);
    }

    #[tokio::test]
    async fn import_wake_interrupts_wait_and_shutdown_stays_authoritative() {
        let signal = WriteSignal::default();
        let (sender, mut shutdown) = watch::channel(false);
        signal.request_import();
        assert!(!tokio::time::timeout(
            Duration::from_millis(100),
            sleep_or_import_or_shutdown(
                &mut shutdown,
                &signal.import_wake(),
                Duration::from_secs(60)
            )
        )
        .await
        .expect("import wake must not wait for idle"));
        sender.send(true).unwrap();
        assert!(
            sleep_or_import_or_shutdown(
                &mut shutdown,
                &signal.import_wake(),
                Duration::from_secs(60)
            )
            .await
        );
    }

    #[test]
    fn document_prep_slice_counts_toward_cap_without_rotating_import_jobs() {
        let now = Instant::now();
        let now_epoch = chrono::Utc::now().timestamp();
        let mut priority = ImportPriority::default();
        let generation = priority.prepare_request(now).generation;

        assert!(priority.note_document_slice(generation, DocumentPrepOutcome::Prepared, now_epoch));
        let snapshot = priority.snapshot().unwrap();
        assert_eq!(snapshot.ambient_slices, 1);
        assert_eq!(snapshot.next_job, AmbientJob::Classification);
        assert_eq!(snapshot.synthesis_phase, None);

        for _ in 1..IMPORT_PRIORITY_MAX_AMBIENT_SLICES {
            priority.note_document_slice(generation, DocumentPrepOutcome::Prepared, now_epoch);
        }
        let snapshot = priority.snapshot().unwrap();
        assert_eq!(snapshot.ambient_slices, IMPORT_PRIORITY_MAX_AMBIENT_SLICES);
        assert_eq!(snapshot.next_job, AmbientJob::Classification);
        assert_eq!(snapshot.synthesis_phase, Some(ImportSynthesisPhase::Detect));
        assert!(!snapshot.document_prep_open());

        let fresh = priority.prepare_request(now).generation;
        assert_ne!(generation, fresh);
        assert!(!priority.note_document_slice(
            generation,
            DocumentPrepOutcome::Prepared,
            now_epoch
        ));
        assert_eq!(priority.snapshot().unwrap().ambient_slices, 0);
    }

    #[test]
    fn empty_inference_round_with_pending_documents_does_not_finish_lane() {
        let now = Instant::now();
        let now_epoch = chrono::Utc::now().timestamp();
        let mut priority = ImportPriority::default();
        let generation = priority.prepare_request(now).generation;
        for _ in 0..IMPORT_PRIORITY_JOBS.len() {
            priority.note_ambient(generation, false);
        }
        let snapshot = priority.snapshot().unwrap();
        assert_eq!(
            snapshot.synthesis_phase, None,
            "an empty inference round must not start synthesis while documents remain"
        );
        assert!(!priority.should_finish(now));
        assert_eq!(
            plan_import_lane_turn(&snapshot, now_epoch, None, None, true, true),
            ImportLaneTurn::DocumentPrep,
            "an idle inference round hands the remaining turns to document prep"
        );

        priority.note_document_slice(generation, DocumentPrepOutcome::Prepared, now_epoch);
        let snapshot = priority.snapshot().unwrap();
        assert_eq!(
            plan_import_lane_turn(&snapshot, now_epoch, None, None, true, true),
            ImportLaneTurn::DocumentPrep
        );

        // Once prep drains, inference gets one fresh full round before
        // synthesis, because the prepared documents may carry new work.
        priority.note_document_slice(generation, DocumentPrepOutcome::Idle, now_epoch);
        let snapshot = priority.snapshot().unwrap();
        assert!(!snapshot.document_prep_open());
        assert_eq!(
            plan_import_lane_turn(&snapshot, now_epoch, None, None, true, true),
            ImportLaneTurn::Inference
        );
        for _ in 0..IMPORT_PRIORITY_JOBS.len() - 1 {
            priority.note_ambient(generation, false);
            assert_eq!(priority.snapshot().unwrap().synthesis_phase, None);
        }
        priority.note_ambient(generation, false);
        assert_eq!(
            priority.snapshot().unwrap().synthesis_phase,
            Some(ImportSynthesisPhase::Detect)
        );
    }

    #[test]
    fn document_prep_and_inference_alternate_while_both_have_work() {
        let now = Instant::now();
        let now_epoch = chrono::Utc::now().timestamp();
        let mut priority = ImportPriority::default();
        let generation = priority.prepare_request(now).generation;
        let plan = |priority: &ImportPriority| {
            plan_import_lane_turn(
                &priority.snapshot().unwrap(),
                now_epoch,
                None,
                None,
                true,
                true,
            )
        };
        assert_eq!(plan(&priority), ImportLaneTurn::DocumentPrep);
        priority.note_document_slice(generation, DocumentPrepOutcome::Prepared, now_epoch);
        assert_eq!(plan(&priority), ImportLaneTurn::Inference);
        priority.note_ambient(generation, true);
        assert_eq!(plan(&priority), ImportLaneTurn::DocumentPrep);
    }

    #[test]
    fn document_prep_lock_contention_retry_and_failures_keep_lane_open() {
        let now = Instant::now();
        let now_epoch = chrono::Utc::now().timestamp();
        let mut priority = ImportPriority::default();
        let generation = priority.prepare_request(now).generation;
        let deadline_epoch = priority.snapshot().unwrap().deadline_epoch;
        // No provider and no pin: before this fix the lane finished at once.
        let plan = |priority: &ImportPriority, epoch: i64, prep_block| {
            plan_import_lane_turn(
                &priority.snapshot().unwrap(),
                epoch,
                None,
                prep_block,
                false,
                false,
            )
        };
        assert_eq!(
            plan(&priority, now_epoch, None),
            ImportLaneTurn::DocumentPrep
        );

        // Lock contention runs nothing and records nothing, so the next
        // tick plans the same prep turn and the lane stays open.
        assert_eq!(
            plan(&priority, now_epoch, None),
            ImportLaneTurn::DocumentPrep
        );
        assert!(!priority.should_finish(now));

        // A failed claim spends no slice and leaves prep pending.
        priority.note_document_slice(generation, DocumentPrepOutcome::ClaimFailed, now_epoch);
        assert_eq!(priority.snapshot().unwrap().ambient_slices, 0);
        assert_eq!(
            plan(&priority, now_epoch, None),
            ImportLaneTurn::DocumentPrep
        );

        // A retryable failure inside the window keeps the lane waiting for
        // its retry instead of finishing as "no provider".
        let retry_at = now_epoch + 60;
        priority.note_document_slice(
            generation,
            DocumentPrepOutcome::RetryableFailure {
                retry_at: Some(retry_at),
            },
            now_epoch,
        );
        assert_eq!(priority.snapshot().unwrap().ambient_slices, 1);
        priority.note_document_slice(generation, DocumentPrepOutcome::Idle, now_epoch);
        assert_eq!(
            plan(&priority, now_epoch, None),
            ImportLaneTurn::Wait(ImportLaneWait::DocumentPrepRetry)
        );
        assert!(!priority.should_finish(now));
        assert_eq!(
            plan(&priority, retry_at, Some(ResourceBlockReason::Warming)),
            ImportLaneTurn::Wait(ImportLaneWait::Resource(ResourceBlockReason::Warming))
        );
        assert_eq!(
            plan(&priority, retry_at, None),
            ImportLaneTurn::DocumentPrep
        );

        // The retry exhausts the document: it spends a slice, and the lane
        // only finishes after a later claim confirms nothing is left.
        priority.note_document_slice(generation, DocumentPrepOutcome::Exhausted, retry_at);
        assert_eq!(priority.snapshot().unwrap().ambient_slices, 2);
        assert_eq!(
            plan(&priority, retry_at, None),
            ImportLaneTurn::DocumentPrep
        );
        priority.note_document_slice(generation, DocumentPrepOutcome::Idle, retry_at);
        assert_eq!(
            plan(&priority, retry_at, None),
            ImportLaneTurn::FinishNoAuthorizedProvider
        );

        // A retry that falls after the lane's deadline does not hold it open.
        let generation = priority.prepare_request(now).generation;
        priority.note_document_slice(
            generation,
            DocumentPrepOutcome::RetryableFailure {
                retry_at: Some(deadline_epoch + 3600),
            },
            now_epoch,
        );
        priority.note_document_slice(generation, DocumentPrepOutcome::Idle, now_epoch);
        assert_eq!(
            plan(&priority, now_epoch, None),
            ImportLaneTurn::FinishNoAuthorizedProvider
        );
        // A pinned provider that is still loading keeps waiting, as before.
        assert_eq!(
            plan_import_lane_turn(
                &priority.snapshot().unwrap(),
                now_epoch,
                None,
                None,
                false,
                true
            ),
            ImportLaneTurn::Wait(ImportLaneWait::PinnedProviderLoading)
        );
    }

    #[test]
    fn document_prep_wakes_for_earliest_retry_and_holds_lane_for_later_ones() {
        let now = Instant::now();
        let now_epoch = chrono::Utc::now().timestamp();
        let mut priority = ImportPriority::default();
        let generation = priority.prepare_request(now).generation;
        let deadline_epoch = priority.snapshot().unwrap().deadline_epoch;
        let plan = |priority: &ImportPriority, epoch: i64| {
            plan_import_lane_turn(
                &priority.snapshot().unwrap(),
                epoch,
                None,
                None,
                false,
                false,
            )
        };
        let early = now_epoch + 60;
        let late = deadline_epoch - 1;
        assert!(early < late);

        // Two documents fail inside the window, the early one first.
        for retry_at in [early, late] {
            priority.note_document_slice(
                generation,
                DocumentPrepOutcome::RetryableFailure {
                    retry_at: Some(retry_at),
                },
                now_epoch,
            );
        }
        priority.note_document_slice(generation, DocumentPrepOutcome::Idle, now_epoch);
        assert_eq!(
            plan(&priority, now_epoch),
            ImportLaneTurn::Wait(ImportLaneWait::DocumentPrepRetry)
        );

        // The lane wakes for the earliest retry, not the latest.
        assert_eq!(plan(&priority, early), ImportLaneTurn::DocumentPrep);
        priority.note_document_slice(generation, DocumentPrepOutcome::Prepared, early);
        priority.note_document_slice(generation, DocumentPrepOutcome::Idle, early);

        // Once the early retry resolves, the later one still holds the lane.
        assert_eq!(
            plan(&priority, early),
            ImportLaneTurn::Wait(ImportLaneWait::DocumentPrepRetry)
        );
        assert!(priority.snapshot().unwrap().document_prep_open());
        assert_eq!(plan(&priority, late), ImportLaneTurn::DocumentPrep);
        priority.note_document_slice(generation, DocumentPrepOutcome::Prepared, late);
        priority.note_document_slice(generation, DocumentPrepOutcome::Idle, late);
        assert!(!priority.snapshot().unwrap().document_prep_open());
        assert_eq!(
            plan(&priority, late),
            ImportLaneTurn::FinishNoAuthorizedProvider
        );
    }

    #[test]
    fn document_prep_cap_leaves_files_pending_and_lane_finishes() {
        let now = Instant::now();
        let now_epoch = chrono::Utc::now().timestamp();
        let mut priority = ImportPriority::default();
        let generation = priority.prepare_request(now).generation;
        for _ in 0..IMPORT_PRIORITY_MAX_AMBIENT_SLICES {
            priority.note_document_slice(generation, DocumentPrepOutcome::Prepared, now_epoch);
        }
        let snapshot = priority.snapshot().unwrap();
        // Prep never drained, yet the shared budget is spent: remaining
        // files stay queued for the ordinary idle-gated ambient lap.
        assert!(!snapshot.document_prep_open());
        assert_eq!(
            plan_import_lane_turn(&snapshot, now_epoch, None, None, false, false),
            ImportLaneTurn::FinishNoAuthorizedProvider
        );
        assert_eq!(
            plan_import_lane_turn(&snapshot, now_epoch, None, None, true, true),
            ImportLaneTurn::Inference
        );
        assert_eq!(snapshot.synthesis_phase, Some(ImportSynthesisPhase::Detect));
    }

    #[test]
    fn document_prep_admission_skips_on_device_headroom_but_keeps_guards() {
        let host = HostActivitySnapshot::Observed {
            idle_for: Duration::ZERO,
            thermal_state: 0,
        };
        let three_gib = admitted(95.0, 3 * super::super::GIB);
        assert_eq!(
            import_priority_block_reason(three_gib, host, false, true),
            Some(ResourceBlockReason::MemoryPressure),
            "on-device inference keeps its two-gibibyte headroom"
        );
        assert_eq!(
            import_document_prep_block_reason(three_gib, host, false),
            None
        );
        assert_eq!(
            import_document_prep_block_reason(three_gib, host, true),
            Some(ResourceBlockReason::Warming),
            "the startup model reservation still blocks prep"
        );
        assert_eq!(
            import_document_prep_block_reason(admitted(1.0, super::super::GIB), host, false),
            Some(ResourceBlockReason::MemoryPressure)
        );
        assert_eq!(
            import_document_prep_block_reason(
                three_gib,
                HostActivitySnapshot::Observed {
                    idle_for: Duration::ZERO,
                    thermal_state: 2,
                },
                false
            ),
            Some(ResourceBlockReason::ThermalPressure)
        );
        assert_eq!(
            import_document_prep_block_reason(three_gib, HostActivitySnapshot::Unavailable, false),
            Some(ResourceBlockReason::HostActivityUnavailable)
        );

        let now = Instant::now();
        let mut priority = ImportPriority::default();
        priority.prepare_request(now);
        assert_eq!(
            plan_import_lane_turn(
                &priority.snapshot().unwrap(),
                chrono::Utc::now().timestamp(),
                import_priority_block_reason(three_gib, host, false, true),
                import_document_prep_block_reason(three_gib, host, false),
                true,
                true,
            ),
            ImportLaneTurn::DocumentPrep
        );
    }

    #[test]
    fn active_cpu_is_allowed_but_memory_and_thermal_are_not() {
        let active_host = HostActivitySnapshot::Observed {
            idle_for: Duration::ZERO,
            thermal_state: 0,
        };
        assert_eq!(
            import_priority_block_reason(
                admitted(95.0, 8 * super::super::GIB),
                active_host,
                false,
                false
            ),
            None
        );
        assert_eq!(
            import_priority_block_reason(
                admitted(1.0, super::super::GIB),
                active_host,
                false,
                false
            ),
            Some(ResourceBlockReason::MemoryPressure)
        );
        assert_eq!(
            import_priority_block_reason(
                admitted(1.0, 8 * super::super::GIB),
                HostActivitySnapshot::Observed {
                    idle_for: Duration::from_secs(90),
                    thermal_state: 1
                },
                false,
                false,
            ),
            Some(ResourceBlockReason::ThermalPressure)
        );
    }
}
