use super::catalog::{catalog_group, LintCheckGroup};
use super::context::{LintContext, PopulationBasis};
use super::runner::failed_results_for_group;
use wenlan_types::lint::{
    LintApplicability, LintCheckResult, LintMetric, LintPrecondition, LintSeverity,
};

mod assessment;
mod query;
mod result;
use assessment::assessments;
#[cfg(test)]
use assessment::{derived, structural};
use query::{apply_fact_index_visibility, load_records};
use result::finish;
#[cfg(test)]
use result::partition_assessment;

const LIFECYCLE_ID: &str = "memories.lifecycle_integrity";
const SUPERSESSION_ID: &str = "memories.supersession_integrity";
const EMBEDDING_ID: &str = "memories.embedding_integrity";
const ENRICHMENT_ID: &str = "memories.enrichment_failures";
const PARTITIONS_ID: &str = "memories.partition_inventory";
const EPISODE_ID: &str = "memories.derived.episode";
const FACT_ID: &str = "memories.derived.fact";
const PAGE_ID: &str = "memories.derived.page_links";
const SUMMARY_ID: &str = "memories.derived.summary";
const TEMPORAL_ID: &str = "memories.derived.temporal";

#[derive(Debug, Clone)]
struct MemoryRecord {
    source_id: String,
    lifecycle_valid: bool,
    supersedes: Option<String>,
    target_exists: bool,
    replaced_by_active: bool,
    pending_revision: bool,
    recap: bool,
    evicted: bool,
    embedding_valid: bool,
    needs_reembed: bool,
    failed_steps: u64,
    classified: bool,
    event_dated: bool,
    episode: bool,
    fact: bool,
    page_link: bool,
    summary: bool,
    episode_eligible: bool,
    fact_eligible: bool,
    summary_eligible: bool,
    episode_receipt: Option<DerivedSweepReceipt>,
    fact_receipt: Option<DerivedSweepReceipt>,
    summary_receipt: Option<DerivedSweepReceipt>,
    head: bool,
}

#[derive(Debug, Clone, Copy)]
struct DerivedSweepReceipt {
    first_missing_at: i64,
    completed_sweeps: u64,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct MemoryFeatureConfig {
    pub(crate) episode: bool,
    pub(crate) fact: bool,
    pub(crate) page: bool,
    pub(crate) summary: bool,
    pub(crate) temporal: bool,
    episode_active: bool,
    fact_active: bool,
    summary_active: bool,
    embedding_backlog: bool,
    artifact_sample: crate::derived_artifact_state::DerivedArtifactSample,
}

impl MemoryFeatureConfig {
    pub(crate) fn capture(database: &crate::db::MemoryDB, page: bool) -> Self {
        use crate::derived_artifact_state::DerivedArtifact;
        let artifact_sample = database.derived_artifact_sample();
        Self {
            episode: crate::db::episode_channel_enabled(),
            fact: crate::retrieval::fact_channel::fact_channel_enabled(),
            page,
            summary: crate::db::global_prelude_enabled(),
            temporal: crate::db::temporal_grounding_enabled(),
            episode_active: artifact_sample.is_active(DerivedArtifact::Episode),
            fact_active: artifact_sample.is_active(DerivedArtifact::Fact),
            summary_active: artifact_sample.is_active(DerivedArtifact::Summary),
            embedding_backlog: artifact_sample.embedding_backlog(),
            artifact_sample,
        }
    }

    pub(crate) fn artifact_state_changed(self, database: &crate::db::MemoryDB) -> bool {
        self.artifact_sample != database.derived_artifact_sample()
    }

    #[cfg(test)]
    pub(super) fn for_test(database: &crate::db::MemoryDB, features: TestMemoryFeatures) -> Self {
        use crate::derived_artifact_state::DerivedArtifact;
        let artifact_sample = database.derived_artifact_sample();
        Self {
            episode: features.episode,
            fact: features.fact,
            page: features.page,
            summary: features.summary,
            temporal: features.temporal,
            episode_active: artifact_sample.is_active(DerivedArtifact::Episode),
            fact_active: artifact_sample.is_active(DerivedArtifact::Fact),
            summary_active: artifact_sample.is_active(DerivedArtifact::Summary),
            embedding_backlog: artifact_sample.embedding_backlog(),
            artifact_sample,
        }
    }
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct TestMemoryFeatures {
    pub(super) episode: bool,
    pub(super) fact: bool,
    pub(super) page: bool,
    pub(super) summary: bool,
    pub(super) temporal: bool,
}

#[derive(Debug, Clone, Copy)]
struct DerivedReadiness {
    provider_ready: bool,
    active_backfill: bool,
    completed_sweeps: u64,
    missing_age_seconds: u64,
}

impl DerivedReadiness {
    fn overdue(self) -> bool {
        self.provider_ready
            && !self.active_backfill
            && self.completed_sweeps >= 2
            && self.missing_age_seconds >= 3_600
    }
}

impl DerivedSweepReceipt {
    fn readiness(self, observed_at: i64, active_backfill: bool) -> DerivedReadiness {
        DerivedReadiness {
            provider_ready: true,
            active_backfill,
            completed_sweeps: self.completed_sweeps,
            missing_age_seconds: u64::try_from(observed_at.saturating_sub(self.first_missing_at))
                .unwrap_or(0),
        }
    }
}

pub(crate) async fn run(
    context: &LintContext<'_, '_>,
    config: MemoryFeatureConfig,
) -> Vec<LintCheckResult> {
    let mut records = match load_records(context).await {
        Ok(records) => records,
        Err(()) => return failed_memory_results(context),
    };
    if config.fact
        && apply_fact_index_visibility(context, &mut records)
            .await
            .is_err()
    {
        return failed_memory_results(context);
    }
    mark_heads_and_supersession(&mut records);
    assessments(&records, config, context.clock().epoch_seconds())
        .into_iter()
        .map(|assessment| finish(context, assessment))
        .collect()
}

fn failed_memory_results(context: &LintContext<'_, '_>) -> Vec<LintCheckResult> {
    let basis = if context.scope().filter().is_selected() {
        PopulationBasis::SelectedScope
    } else {
        PopulationBasis::Global
    };
    for entry in catalog_group(LintCheckGroup::Memories) {
        let _ = context.record_population(entry.id, basis, 0);
    }
    failed_results_for_group(context.clock(), LintCheckGroup::Memories)
}

struct CheckAssessment {
    id: &'static str,
    levels: Vec<LintSeverity>,
    applicability: LintApplicability,
    precondition: LintPrecondition,
    metrics: Vec<LintMetric>,
}

fn mark_heads_and_supersession(records: &mut [MemoryRecord]) {
    for record in records {
        record.head = !record.pending_revision
            && !record.recap
            && !record.evicted
            && !record.replaced_by_active;
    }
}

#[cfg(test)]
#[path = "memories_test.rs"]
mod tests;

#[cfg(test)]
#[path = "memories_integration_test.rs"]
mod integration_tests;

#[cfg(test)]
#[path = "memories_review_regression_test.rs"]
mod review_regression_tests;

#[cfg(test)]
#[path = "memories_lifecycle_regression_test.rs"]
mod lifecycle_regression_tests;
