use super::result::{base_metrics, partition_assessment};
use super::{
    CheckAssessment, DerivedReadiness, MemoryFeatureConfig, MemoryRecord, EMBEDDING_ID,
    ENRICHMENT_ID, EPISODE_ID, FACT_ID, LIFECYCLE_ID, PAGE_ID, SUMMARY_ID, SUPERSESSION_ID,
    TEMPORAL_ID,
};
use wenlan_types::lint::{LintApplicability, LintPrecondition, LintSeverity};

pub(super) fn assessments(
    records: &[MemoryRecord],
    config: MemoryFeatureConfig,
    observed_at: i64,
) -> Vec<CheckAssessment> {
    let heads = records
        .iter()
        .filter(|record| record.head)
        .collect::<Vec<_>>();
    let lifecycle = structural(LIFECYCLE_ID, records, |record| record.lifecycle_valid);
    let supersession = structural(SUPERSESSION_ID, records, |record| {
        record.supersedes.as_deref() != Some(record.source_id.as_str())
            && record.target_exists
            && (!record.pending_revision || record.supersedes.is_some())
    });
    // A big import stores rows without a vector and embeds them in the
    // background; until that catch-up ends a missing vector is pending work.
    let embedding = structural(EMBEDDING_ID, &heads, |record| {
        record.embedding_valid || config.embedding_backlog
    });
    let enrichment = structural(ENRICHMENT_ID, records, |record| record.failed_steps == 0);
    let mut checks = vec![lifecycle, supersession, embedding, enrichment];
    checks.push(partition_assessment(&heads, records));
    checks.extend([
        derived(
            EPISODE_ID,
            &heads,
            config.episode,
            |record| record.episode_eligible,
            |record| record.episode,
            |record| {
                record
                    .episode_receipt
                    .map(|receipt| receipt.readiness(observed_at, config.episode_active))
            },
        ),
        derived(
            FACT_ID,
            &heads,
            config.fact,
            |record| record.fact_eligible,
            |record| record.fact,
            |record| {
                record
                    .fact_receipt
                    .map(|receipt| receipt.readiness(observed_at, config.fact_active))
            },
        ),
        derived(
            PAGE_ID,
            &heads,
            config.page,
            |_| true,
            |record| record.page_link,
            |_| None,
        ),
        derived(
            SUMMARY_ID,
            &heads,
            config.summary,
            |record| record.summary_eligible,
            |record| record.summary,
            |record| {
                record
                    .summary_receipt
                    .map(|receipt| receipt.readiness(observed_at, config.summary_active))
            },
        ),
        derived(
            TEMPORAL_ID,
            &heads,
            config.temporal,
            |_| true,
            |record| record.event_dated,
            |_| None,
        ),
    ]);
    checks
}

pub(super) fn structural<T>(
    id: &'static str,
    records: &[T],
    valid: impl Fn(&T) -> bool,
) -> CheckAssessment {
    CheckAssessment {
        id,
        levels: records
            .iter()
            .map(|record| {
                if valid(record) {
                    LintSeverity::Info
                } else {
                    LintSeverity::Error
                }
            })
            .collect(),
        applicability: if records.is_empty() {
            LintApplicability::Inventory
        } else {
            LintApplicability::Applicable
        },
        precondition: LintPrecondition::Ready,
        metrics: base_metrics(
            records.len(),
            records.iter().filter(|record| !valid(record)).count(),
        ),
    }
}

pub(super) fn derived(
    id: &'static str,
    heads: &[&MemoryRecord],
    enabled: bool,
    eligible: impl Fn(&MemoryRecord) -> bool,
    present: impl Fn(&MemoryRecord) -> bool,
    readiness: impl Fn(&MemoryRecord) -> Option<DerivedReadiness>,
) -> CheckAssessment {
    let eligible_heads = heads
        .iter()
        .copied()
        .filter(|record| eligible(record))
        .collect::<Vec<_>>();
    let missing = eligible_heads
        .iter()
        .filter(|record| !present(record))
        .count();
    let actionable = |record: &MemoryRecord| {
        enabled && !present(record) && readiness(record).is_some_and(DerivedReadiness::overdue)
    };
    let actionable_count = eligible_heads
        .iter()
        .filter(|record| actionable(record))
        .count();
    CheckAssessment {
        id,
        levels: eligible_heads
            .iter()
            .map(|record| {
                if actionable(record) {
                    LintSeverity::Warning
                } else {
                    LintSeverity::Info
                }
            })
            .collect(),
        applicability: if !enabled {
            LintApplicability::ExpectedEmpty
        } else if actionable_count > 0 {
            LintApplicability::Applicable
        } else {
            LintApplicability::Inventory
        },
        precondition: if enabled {
            LintPrecondition::Ready
        } else {
            LintPrecondition::ConfiguredOff
        },
        metrics: base_metrics(eligible_heads.len(), missing),
    }
}
