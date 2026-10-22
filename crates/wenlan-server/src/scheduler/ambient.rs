use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AmbientJob {
    Document,
    Classification,
    StructuredExtract,
    Entity,
    Title,
    PageGrowth,
    Reconcile,
    Citation,
    EdgeGroundingPromote,
    /// Housekeeping for the detected-entity index (#708): archive detected
    /// entities idle for `refinery.entity_archive_idle_days`. Pure SQL, no
    /// provider, so it stays available when every inference lane is parked.
    EntityIdleArchive,
    /// Ask the everyday model about code basis rows whose cited lines changed:
    /// settle the harmless ones, rewrite the note where the change makes it
    /// wrong, and leave a one-sentence note for a person on the rest.
    CodeBasisRecheck,
}

impl AmbientJob {
    pub(super) const ALL: [Self; 11] = [
        Self::Document,
        Self::Classification,
        Self::StructuredExtract,
        Self::Entity,
        Self::Title,
        Self::PageGrowth,
        Self::Reconcile,
        Self::Citation,
        Self::EdgeGroundingPromote,
        Self::EntityIdleArchive,
        Self::CodeBasisRecheck,
    ];

    /// Stable key for this job, used for the `app_metadata` last-run record
    /// and the `/api/ambient/status` wire response. Never renamed once
    /// shipped — it is a persisted key, not just a debug label.
    pub(super) const fn as_key(self) -> &'static str {
        match self {
            Self::Document => "document",
            Self::Classification => "classification",
            Self::StructuredExtract => "structured_extract",
            Self::Entity => "entity",
            Self::Title => "title",
            Self::PageGrowth => "page_growth",
            Self::Reconcile => "reconcile",
            Self::Citation => "citation",
            Self::EdgeGroundingPromote => "edge_grounding_promote",
            Self::EntityIdleArchive => "entity_idle_archive",
            Self::CodeBasisRecheck => "code_basis_recheck",
        }
    }
}

const AMBIENT_LAST_RUN_KEY_PREFIX: &str = "ambient_last_run_v1";

/// `app_metadata` key recording the last time this job was attempted
/// (selected or not), by either the passive scheduler tick or a forced sweep.
pub(super) fn ambient_last_run_key(job: AmbientJob) -> String {
    format!("{AMBIENT_LAST_RUN_KEY_PREFIX}_{}", job.as_key())
}

async fn persist_ambient_last_run(db: &wenlan_core::db::MemoryDB, job: AmbientJob) {
    let epoch = chrono::Utc::now().timestamp().to_string();
    if let Err(error) = db
        .set_app_metadata(&ambient_last_run_key(job), &epoch)
        .await
    {
        tracing::warn!(
            "[scheduler] failed to persist ambient last-run timestamp for {job:?}: {error}"
        );
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct AmbientAvailability {
    pub(super) document: bool,
    pub(super) classification: bool,
    pub(super) structured_extract: bool,
    pub(super) entity: bool,
    pub(super) title: bool,
    pub(super) page_growth: bool,
    pub(super) reconcile: bool,
    pub(super) citation: bool,
    pub(super) edge_grounding_promote: bool,
    pub(super) entity_idle_archive: bool,
    pub(super) code_basis_recheck: bool,
}

/// The model recheck of changed code basis rows spends inference on the
/// person's behalf, so it rides the one switch every automatic lane rides:
/// a pinned and healthy provider with background AI on. There is no second
/// switch; without it the same judging happens on demand, in the caller's
/// steep (`code_check` work, `/api/steep`), so a changed note is never left
/// unlooked at.
pub(super) const fn code_basis_recheck_allowed(
    provider_available: bool,
    background_ai_enabled: bool,
) -> bool {
    provider_available && background_ai_enabled
}

impl AmbientAvailability {
    /// Automatic lanes are a consent boundary as well as a capability check.
    /// Deterministic document preparation remains available without a model so
    /// source data becomes searchable; all inference lanes stay parked until
    /// the pinned provider is both authorized and healthy.
    pub(super) fn for_provider(provider_available: bool) -> Self {
        let config = wenlan_core::config::load_config();
        Self {
            document: true,
            classification: provider_available,
            structured_extract: provider_available,
            entity: provider_available && wenlan_core::db::entity_sweep_enabled(),
            title: provider_available,
            page_growth: provider_available,
            reconcile: provider_available && wenlan_core::db::doc_reconcile_enabled(),
            citation: provider_available && wenlan_core::db::citation_backfill_enabled(),
            // Provider-gated: the mandatory entailment check spends an LLM call,
            // so the lane parks unless the pinned provider is authorized+healthy
            // AND the opt-in flag is set (mirroring reconcile / citation).
            edge_grounding_promote: provider_available
                && wenlan_core::db::edge_grounding_promote_enabled(),
            // Deterministic housekeeping: no model involved, so it runs
            // whether or not a provider is pinned. The tuning key itself
            // (`entity_archive_idle_days`, default 90; `0` opts out) is the
            // on/off switch,
            // checked inside the job arm.
            entity_idle_archive: true,
            code_basis_recheck: code_basis_recheck_allowed(
                provider_available,
                config.background_ai_enabled(),
            ),
        }
    }

    pub(super) const fn supports(self, job: AmbientJob) -> bool {
        match job {
            AmbientJob::Document => self.document,
            AmbientJob::Classification => self.classification,
            AmbientJob::StructuredExtract => self.structured_extract,
            AmbientJob::Entity => self.entity,
            AmbientJob::Title => self.title,
            AmbientJob::PageGrowth => self.page_growth,
            AmbientJob::Reconcile => self.reconcile,
            AmbientJob::Citation => self.citation,
            AmbientJob::EdgeGroundingPromote => self.edge_grounding_promote,
            AmbientJob::EntityIdleArchive => self.entity_idle_archive,
            AmbientJob::CodeBasisRecheck => self.code_basis_recheck,
        }
    }
}

pub(super) struct AmbientSchedule {
    cursor: usize,
    pub(super) next_allowed_at: Instant,
    last_classification: Option<Instant>,
    last_structured_extract: Option<Instant>,
    pub(super) last_entity: Option<Instant>,
    last_title: Option<Instant>,
    last_page_growth: Option<Instant>,
    pub(super) last_reconcile: Option<Instant>,
    pub(super) last_citation: Option<Instant>,
    last_edge_grounding_promote: Option<Instant>,
    pub(super) last_entity_idle_archive: Option<Instant>,
    pub(super) last_code_basis_recheck: Option<Instant>,
}

impl AmbientSchedule {
    pub(super) fn new(now: Instant) -> Self {
        Self {
            cursor: 0,
            next_allowed_at: now,
            last_classification: None,
            last_structured_extract: None,
            last_entity: None,
            last_title: None,
            last_page_growth: None,
            last_reconcile: None,
            last_citation: None,
            last_edge_grounding_promote: None,
            last_entity_idle_archive: None,
            last_code_basis_recheck: None,
        }
    }

    pub(super) fn select_due(
        &mut self,
        now: Instant,
        availability: AmbientAvailability,
    ) -> Option<AmbientJob> {
        for _ in 0..AmbientJob::ALL.len() {
            let job = AmbientJob::ALL[self.cursor];
            self.cursor = (self.cursor + 1) % AmbientJob::ALL.len();
            if !availability.supports(job) {
                continue;
            }
            let due = match job {
                AmbientJob::Document => true,
                AmbientJob::Classification => self
                    .last_classification
                    .is_none_or(|last| now.duration_since(last) >= ENRICHMENT_SWEEP_INTERVAL),
                AmbientJob::StructuredExtract => self
                    .last_structured_extract
                    .is_none_or(|last| now.duration_since(last) >= ENRICHMENT_SWEEP_INTERVAL),
                AmbientJob::Entity => self
                    .last_entity
                    .is_none_or(|last| now.duration_since(last) >= ENRICHMENT_SWEEP_INTERVAL),
                AmbientJob::Title => self
                    .last_title
                    .is_none_or(|last| now.duration_since(last) >= ENRICHMENT_SWEEP_INTERVAL),
                AmbientJob::PageGrowth => self
                    .last_page_growth
                    .is_none_or(|last| now.duration_since(last) >= ENRICHMENT_SWEEP_INTERVAL),
                AmbientJob::Reconcile => self
                    .last_reconcile
                    .is_none_or(|last| now.duration_since(last) >= RECONCILE_SWEEP_INTERVAL),
                AmbientJob::Citation => self
                    .last_citation
                    .is_none_or(|last| now.duration_since(last) >= CITATION_SWEEP_INTERVAL),
                AmbientJob::EdgeGroundingPromote => self
                    .last_edge_grounding_promote
                    .is_none_or(|last| now.duration_since(last) >= EDGE_GROUNDING_SWEEP_INTERVAL),
                AmbientJob::EntityIdleArchive => self.last_entity_idle_archive.is_none_or(|last| {
                    now.duration_since(last) >= ENTITY_IDLE_ARCHIVE_SWEEP_INTERVAL
                }),
                AmbientJob::CodeBasisRecheck => self.last_code_basis_recheck.is_none_or(|last| {
                    now.duration_since(last) >= CODE_BASIS_RECHECK_SWEEP_INTERVAL
                }),
            };
            if !due {
                continue;
            }
            return Some(job);
        }
        None
    }

    /// Enumerate every currently-due job for one bounded scheduler tick — at
    /// most one full round-robin lap.
    ///
    /// Without this, a quiet tick only ever fires the single job the cursor
    /// lands on; an always-due job at an earlier cursor position (Document,
    /// which has no interval) can starve a job several slots further round
    /// (e.g. Citation) for many ticks.
    ///
    /// The loop bound is "stop the first time `select_due` repeats a job
    /// already collected this call", not merely "call `select_due` at most
    /// `ALL.len()` times": collecting due jobs up front does not call
    /// `note_job_result` in between (that only happens once each job has
    /// actually run, back in the caller), so a job's due-ness is invariant
    /// for the whole `drain_due` call. If fewer than `ALL.len()` jobs are
    /// currently due-and-supported, `select_due`'s own internal scan (itself
    /// bounded to one lap) can still cross a full circuit within a single
    /// call and land back on an already-returned job — counting outer
    /// `select_due` *calls* does not bound total *probes*. Detecting the
    /// repeat, rather than counting calls, is what actually keeps every job
    /// in the result at most once.
    pub(super) fn drain_due(
        &mut self,
        now: Instant,
        availability: AmbientAvailability,
    ) -> Vec<AmbientJob> {
        let mut due = Vec::with_capacity(AmbientJob::ALL.len());
        for _ in 0..AmbientJob::ALL.len() {
            match self.select_due(now, availability) {
                Some(job) if due.contains(&job) => break,
                Some(job) => due.push(job),
                None => break,
            }
        }
        due
    }

    /// Back off an empty periodic lane, but leave known backlog due. The global
    /// thermal cooldown still limits actual work; this only prevents a second
    /// 30-minute delay from turning catch-up into a multi-week drain.
    pub(super) fn note_job_result(&mut self, job: AmbientJob, now: Instant, selected: bool) {
        // A slice of model calls is paced, not drained: every attempt waits
        // the full interval, whether or not it found work.
        if job == AmbientJob::CodeBasisRecheck {
            self.last_code_basis_recheck = Some(now);
            return;
        }
        if selected {
            return;
        }
        match job {
            AmbientJob::Document => {}
            AmbientJob::Classification => self.last_classification = Some(now),
            AmbientJob::StructuredExtract => self.last_structured_extract = Some(now),
            AmbientJob::Entity => self.last_entity = Some(now),
            AmbientJob::Title => self.last_title = Some(now),
            AmbientJob::PageGrowth => self.last_page_growth = Some(now),
            AmbientJob::Reconcile => self.last_reconcile = Some(now),
            AmbientJob::Citation => self.last_citation = Some(now),
            // Backlog drainer, not a full pass: this arm only runs when the
            // slice made no progress (empty backlog), so stamp the interval to
            // back off. A progressing slice returned early above, staying due.
            AmbientJob::EdgeGroundingPromote => self.last_edge_grounding_promote = Some(now),
            // A sweep that archived something (`selected`) returned early and
            // stays due so the next tick drains the rest of the backlog in
            // 500-row slices; an empty or disabled sweep backs off a day.
            AmbientJob::EntityIdleArchive => self.last_entity_idle_archive = Some(now),
            AmbientJob::CodeBasisRecheck => self.last_code_basis_recheck = Some(now),
        }
    }

    pub(super) fn note_thermal_work_completion(
        &mut self,
        now: Instant,
        elapsed: Duration,
        policy: ThermalPolicy,
    ) {
        self.next_allowed_at = now + policy.cooldown_after(elapsed);
    }
}

pub(super) fn ambient_turn_allowed(
    system_resources_idle: bool,
    now: Instant,
    next_allowed_at: Instant,
) -> bool {
    system_resources_idle && now >= next_allowed_at
}

pub(super) fn should_backoff_ambient_lane(selected: bool, llm_calls: usize) -> bool {
    !selected && llm_calls == 0
}

pub(super) fn ambient_work_consumes_thermal_turn(
    job: AmbientJob,
    selected: bool,
    llm_calls: usize,
    page_growth_terminal_no_match_committed: bool,
) -> bool {
    llm_calls > 0
        || (selected
            && (matches!(job, AmbientJob::Document | AmbientJob::Reconcile)
                || (matches!(job, AmbientJob::PageGrowth)
                    && !page_growth_terminal_no_match_committed)))
}

#[derive(Debug)]
pub(super) struct AmbientTurnReport {
    pub(super) job: AmbientJob,
    pub(super) selected: bool,
    pub(super) page_growth_terminal_no_match_committed: bool,
    pub(super) llm_calls: usize,
    pub(super) panicked: bool,
    pub(super) elapsed: Duration,
}

pub(super) fn resolve_ambient_provider(
    everyday_pin: Option<wenlan_core::refinery::EverydaySource>,
    api_llm: Option<&Arc<dyn wenlan_core::llm_provider::LlmProvider>>,
    external_llm: Option<&Arc<dyn wenlan_core::llm_provider::LlmProvider>>,
    llm: Option<&Arc<dyn wenlan_core::llm_provider::LlmProvider>>,
) -> Option<Arc<dyn wenlan_core::llm_provider::LlmProvider>> {
    wenlan_core::refinery::resolve_everyday(everyday_pin, api_llm, external_llm, llm)
        .llm
        .cloned()
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn run_ambient_job_safe(
    job: AmbientJob,
    db: &Arc<wenlan_core::db::MemoryDB>,
    llm: Option<&Arc<dyn wenlan_core::llm_provider::LlmProvider>>,
    api_llm: Option<&Arc<dyn wenlan_core::llm_provider::LlmProvider>>,
    external_llm: Option<&Arc<dyn wenlan_core::llm_provider::LlmProvider>>,
    everyday_pin: Option<wenlan_core::refinery::EverydaySource>,
    prompts: &wenlan_core::prompts::PromptRegistry,
    refinery: &wenlan_core::tuning::RefineryConfig,
    distillation: &wenlan_core::tuning::DistillationConfig,
    knowledge_path: Option<&std::path::Path>,
) -> AmbientTurnReport {
    let started = Instant::now();
    let future = std::panic::AssertUnwindSafe(run_ambient_job(
        job,
        db,
        llm,
        api_llm,
        external_llm,
        everyday_pin,
        prompts,
        refinery,
        distillation,
        knowledge_path,
    ));
    let report = match futures::FutureExt::catch_unwind(future).await {
        Ok(report) => report,
        Err(error) => {
            let message = if let Some(message) = error.downcast_ref::<&str>() {
                (*message).to_string()
            } else if let Some(message) = error.downcast_ref::<String>() {
                message.clone()
            } else {
                "unknown panic".to_string()
            };
            tracing::error!(
                "[scheduler] ambient job={job:?} PANICKED — scheduler continues: {message}"
            );
            AmbientTurnReport {
                job,
                selected: true,
                page_growth_terminal_no_match_committed: false,
                llm_calls: 0,
                panicked: true,
                elapsed: started.elapsed(),
            }
        }
    };
    // Record every attempt (selected or not) so `/api/ambient/status` can
    // show liveness even for a job that keeps finding nothing to do.
    persist_ambient_last_run(db, job).await;
    report
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn run_ambient_job(
    job: AmbientJob,
    db: &Arc<wenlan_core::db::MemoryDB>,
    llm: Option<&Arc<dyn wenlan_core::llm_provider::LlmProvider>>,
    api_llm: Option<&Arc<dyn wenlan_core::llm_provider::LlmProvider>>,
    external_llm: Option<&Arc<dyn wenlan_core::llm_provider::LlmProvider>>,
    everyday_pin: Option<wenlan_core::refinery::EverydaySource>,
    prompts: &wenlan_core::prompts::PromptRegistry,
    refinery: &wenlan_core::tuning::RefineryConfig,
    distillation: &wenlan_core::tuning::DistillationConfig,
    knowledge_path: Option<&std::path::Path>,
) -> AmbientTurnReport {
    let started = Instant::now();
    let call_budget = match job {
        AmbientJob::CodeBasisRecheck => CODE_BASIS_RECHECK_ROWS_PER_SLICE,
        _ => 1,
    };
    let observed = resolve_ambient_provider(everyday_pin, api_llm, external_llm, llm)
        .map(|provider| Arc::new(AmbientBudgetProvider::with_limit(provider, call_budget)));
    let provider: Option<Arc<dyn wenlan_core::llm_provider::LlmProvider>> = observed
        .as_ref()
        .map(|provider| provider.clone() as Arc<dyn wenlan_core::llm_provider::LlmProvider>);

    let mut page_growth_terminal_no_match_committed = false;
    let selected = match job {
        AmbientJob::Document => {
            run_document_enrichment_slice_tick(db, provider.as_ref(), prompts).await > 0
        }
        AmbientJob::Classification => {
            let Some(provider) = provider.as_ref() else {
                return AmbientTurnReport {
                    job,
                    selected: false,
                    page_growth_terminal_no_match_committed: false,
                    llm_calls: 0,
                    panicked: false,
                    elapsed: started.elapsed(),
                };
            };
            match wenlan_core::ingest::run_classification_enrichment_slice(db, provider, prompts)
                .await
            {
                Ok(report) => report.selected,
                Err(error) => {
                    tracing::warn!("[scheduler] classification slice error: {error}");
                    false
                }
            }
        }
        AmbientJob::StructuredExtract => {
            let Some(provider) = provider.as_ref() else {
                return AmbientTurnReport {
                    job,
                    selected: false,
                    page_growth_terminal_no_match_committed: false,
                    llm_calls: 0,
                    panicked: false,
                    elapsed: started.elapsed(),
                };
            };
            match wenlan_core::ingest::run_structured_extract_slice(db, provider, prompts).await {
                Ok(report) => report.selected,
                Err(error) => {
                    tracing::warn!("[scheduler] structured extraction slice error: {error}");
                    false
                }
            }
        }
        AmbientJob::Entity if !wenlan_core::db::entity_sweep_enabled() => false,
        AmbientJob::Entity => {
            let Some(provider) = provider.clone() else {
                return AmbientTurnReport {
                    job,
                    selected: false,
                    page_growth_terminal_no_match_committed: false,
                    llm_calls: 0,
                    panicked: false,
                    elapsed: started.elapsed(),
                };
            };
            let prompts = prompts.clone();
            match db
                .run_entity_enrichment_slice_with_auto_link(
                    refinery.entity_link_distance as f32,
                    move |content: String| {
                        let provider = provider.clone();
                        let prompts = prompts.clone();
                        async move {
                            wenlan_core::kg::entity_extraction::extract_kg(
                                &provider, &prompts, &content,
                            )
                            .await
                        }
                    },
                )
                .await
            {
                Ok(selected) => selected > 0,
                Err(error) => {
                    tracing::warn!("[scheduler] entity enrichment slice error: {error}");
                    false
                }
            }
        }
        AmbientJob::Title => {
            let Some(provider) = provider.as_ref() else {
                return AmbientTurnReport {
                    job,
                    selected: false,
                    page_growth_terminal_no_match_committed: false,
                    llm_calls: 0,
                    panicked: false,
                    elapsed: started.elapsed(),
                };
            };
            match wenlan_core::post_ingest::run_title_enrichment_slice(db, provider).await {
                Ok(report) => report.selected,
                Err(error) => {
                    tracing::warn!("[scheduler] title enrichment slice error: {error}");
                    false
                }
            }
        }
        AmbientJob::PageGrowth => {
            let Some(provider) = provider.as_ref() else {
                return AmbientTurnReport {
                    job,
                    selected: false,
                    page_growth_terminal_no_match_committed: false,
                    llm_calls: 0,
                    panicked: false,
                    elapsed: started.elapsed(),
                };
            };
            match wenlan_core::post_ingest::run_page_growth_slice(
                db,
                provider,
                prompts,
                distillation.page_growth_threshold,
                knowledge_path,
            )
            .await
            {
                Ok(report) => {
                    page_growth_terminal_no_match_committed =
                        report.terminal_no_match && report.committed;
                    report.selected
                }
                Err(error) => {
                    tracing::warn!("[scheduler] page growth slice error: {error}");
                    false
                }
            }
        }
        AmbientJob::Reconcile => {
            let Some(provider) = provider.as_ref() else {
                return AmbientTurnReport {
                    job,
                    selected: false,
                    page_growth_terminal_no_match_committed: false,
                    llm_calls: 0,
                    panicked: false,
                    elapsed: started.elapsed(),
                };
            };
            match wenlan_core::reconcile::run_reconcile_slice(
                db,
                provider,
                prompts,
                refinery,
                distillation,
            )
            .await
            {
                Ok(report) => report.progressed,
                Err(error) => {
                    tracing::warn!("[scheduler] reconcile slice error: {error}");
                    false
                }
            }
        }
        AmbientJob::Citation => {
            let Some(provider) = provider.as_ref() else {
                return AmbientTurnReport {
                    job,
                    selected: false,
                    page_growth_terminal_no_match_committed: false,
                    llm_calls: 0,
                    panicked: false,
                    elapsed: started.elapsed(),
                };
            };
            match wenlan_core::citations::run_citation_backfill_slice(db, provider, prompts).await {
                Ok(selected) => selected > 0,
                Err(error) => {
                    tracing::warn!("[scheduler] citation backfill slice error: {error}");
                    false
                }
            }
        }
        AmbientJob::EdgeGroundingPromote => {
            let Some(provider) = provider.as_ref() else {
                return AmbientTurnReport {
                    job,
                    selected: false,
                    page_growth_terminal_no_match_committed: false,
                    llm_calls: 0,
                    panicked: false,
                    elapsed: started.elapsed(),
                };
            };
            match wenlan_core::edge_grounding::run_edge_grounding_slice(db, provider, prompts).await
            {
                Ok(report) => report.progressed,
                Err(error) => {
                    tracing::warn!("[scheduler] edge grounding slice error: {error}");
                    false
                }
            }
        }
        AmbientJob::CodeBasisRecheck => {
            let Some(provider) = provider.as_ref() else {
                return AmbientTurnReport {
                    job,
                    selected: false,
                    page_growth_terminal_no_match_committed: false,
                    llm_calls: 0,
                    panicked: false,
                    elapsed: started.elapsed(),
                };
            };
            match wenlan_core::code_basis_recheck::run_code_basis_recheck_slice(
                db,
                provider,
                prompts,
                CODE_BASIS_RECHECK_ROWS_PER_SLICE,
            )
            .await
            {
                Ok(report) => report.progressed(),
                Err(error) => {
                    tracing::warn!("[scheduler] code basis recheck slice error: {error}");
                    false
                }
            }
        }
        AmbientJob::EntityIdleArchive => {
            // `0` is the documented "off" value: report an unselected turn
            // without touching the database so the lane backs off a day.
            let idle_days = refinery.entity_archive_idle_days;
            if idle_days == 0 {
                return AmbientTurnReport {
                    job,
                    selected: false,
                    page_growth_terminal_no_match_committed: false,
                    llm_calls: 0,
                    panicked: false,
                    elapsed: started.elapsed(),
                };
            }
            match db
                .archive_idle_detected_entities(idle_days, ENTITY_IDLE_ARCHIVE_SWEEP_LIMIT)
                .await
            {
                Ok(archived) => {
                    if archived > 0 {
                        tracing::info!(
                            "[scheduler] entity idle-archive sweep archived {archived} detected \
                             entities idle for more than {idle_days} days"
                        );
                    }
                    archived > 0
                }
                Err(error) => {
                    tracing::warn!("[scheduler] entity idle-archive sweep error: {error}");
                    false
                }
            }
        }
    };

    AmbientTurnReport {
        job,
        selected,
        page_growth_terminal_no_match_committed,
        llm_calls: observed
            .as_ref()
            .map_or(0, |provider| provider.call_count()),
        panicked: false,
        elapsed: started.elapsed(),
    }
}

/// Server copy of the document queue's attempt cap
/// (`MemoryDB::DOC_ENRICHMENT_MAX_ATTEMPTS`, crate-private in wenlan-core).
/// `document_prep_outcome_distinguishes_retryable_and_exhausted_rows` pins it
/// to the real claim query.
const DOCUMENT_PREP_MAX_ATTEMPTS: i64 = 5;

#[derive(Debug)]
pub(super) struct DocumentPrepReport {
    pub(super) outcome: DocumentPrepOutcome,
    pub(super) panicked: bool,
    pub(super) elapsed: Duration,
}

/// Classify a claimed document's queue row after one prep attempt.
pub(super) fn document_prep_outcome(
    entry: Option<&wenlan_core::db::DocEnrichmentQueueEntry>,
) -> DocumentPrepOutcome {
    match entry {
        Some(entry) if entry.status == "paused" => {
            if entry.attempt_count >= DOCUMENT_PREP_MAX_ATTEMPTS {
                DocumentPrepOutcome::Exhausted
            } else {
                DocumentPrepOutcome::RetryableFailure {
                    retry_at: entry.next_retry_at,
                }
            }
        }
        _ => DocumentPrepOutcome::Prepared,
    }
}

/// Rate-limits the import-lane claim-failure warning. A failed claim spends no
/// slice, so a persistent queue error would otherwise warn on every
/// import-priority tick until the lane's deadline. The first failure of a
/// consecutive run warns; the rest log at debug until a claim succeeds.
#[derive(Debug, Default)]
pub(super) struct DocumentPrepClaimLog {
    failing: bool,
}

impl DocumentPrepClaimLog {
    /// Record a failed claim. True only for the first failure of a run.
    pub(super) fn note_failure(&mut self) -> bool {
        !std::mem::replace(&mut self.failing, true)
    }

    /// Record a successful claim (a document or an empty queue).
    pub(super) fn note_success(&mut self) {
        self.failing = false;
    }
}

/// Import-lane document prep: claim one fresh document and parse, embed and
/// park it without any model, so a new folder becomes searchable without
/// waiting for the idle gate. Model enrichment of the parked row stays on the
/// ordinary ambient lap. A panic pauses the claimed generation and is reported
/// rather than unwound, like `run_ambient_job_safe`.
pub(super) async fn run_import_document_prep_slice(
    db: &Arc<wenlan_core::db::MemoryDB>,
    prompts: &wenlan_core::prompts::PromptRegistry,
    knowledge_path: &std::path::Path,
    claim_log: &mut DocumentPrepClaimLog,
) -> DocumentPrepReport {
    let started = Instant::now();
    let claim = db.claim_next_pending_for_provider(false).await;
    if claim.is_ok() {
        claim_log.note_success();
    }
    let entry = match claim {
        Ok(Some(entry)) => entry,
        Ok(None) => {
            persist_ambient_last_run(db, AmbientJob::Document).await;
            return DocumentPrepReport {
                outcome: DocumentPrepOutcome::Idle,
                panicked: false,
                elapsed: started.elapsed(),
            };
        }
        Err(error) => {
            if claim_log.note_failure() {
                tracing::warn!(
                    "[scheduler] import document prep claim failed: {error}; repeats log at \
                     debug until a claim succeeds"
                );
            } else {
                tracing::debug!("[scheduler] import document prep claim failed again: {error}");
            }
            return DocumentPrepReport {
                outcome: DocumentPrepOutcome::ClaimFailed,
                panicked: false,
                elapsed: started.elapsed(),
            };
        }
    };
    let slice = std::panic::AssertUnwindSafe(
        wenlan_core::document_enrichment::run_document_enrichment_slice(
            db,
            &entry,
            Some(knowledge_path),
            None,
            prompts,
        ),
    );
    let panicked = match futures::FutureExt::catch_unwind(slice).await {
        Ok(_) => false,
        Err(_) => {
            tracing::error!(
                "[scheduler] import document prep PANICKED for {}; scheduler continues",
                entry.file_path
            );
            wenlan_core::document_enrichment::pause_document_enrichment_after_panic(db, &entry)
                .await;
            true
        }
    };
    let outcome = match db.get_queue_entry(&entry.source_id, &entry.file_path).await {
        Ok(row) => document_prep_outcome(row.as_ref()),
        Err(error) => {
            tracing::warn!("[scheduler] import document prep outcome read failed: {error}");
            DocumentPrepOutcome::Prepared
        }
    };
    persist_ambient_last_run(db, AmbientJob::Document).await;
    DocumentPrepReport {
        outcome,
        panicked,
        elapsed: started.elapsed(),
    }
}

/// Claim at most one document and advance it by at most one LLM request.
/// Paused rows retain their existing backoff through `claim_next_pending`.
pub(super) async fn run_document_enrichment_slice_tick(
    db: &Arc<wenlan_core::db::MemoryDB>,
    llm: Option<&Arc<dyn wenlan_core::llm_provider::LlmProvider>>,
    prompts: &wenlan_core::prompts::PromptRegistry,
) -> usize {
    let knowledge_path = wenlan_core::config::load_config().knowledge_path_or_default();
    match db.claim_next_pending_for_provider(llm.is_some()).await {
        Ok(Some(entry)) => {
            let slice = std::panic::AssertUnwindSafe(
                wenlan_core::document_enrichment::run_document_enrichment_slice(
                    db,
                    &entry,
                    Some(&knowledge_path),
                    llm,
                    prompts,
                ),
            );
            match futures::FutureExt::catch_unwind(slice).await {
                Ok(_) => 1,
                Err(panic) => {
                    wenlan_core::document_enrichment::pause_document_enrichment_after_panic(
                        db, &entry,
                    )
                    .await;
                    std::panic::resume_unwind(panic);
                }
            }
        }
        Ok(None) => 0,
        Err(error) => {
            tracing::warn!("[scheduler] claim_next_pending failed: {error}");
            0
        }
    }
}
