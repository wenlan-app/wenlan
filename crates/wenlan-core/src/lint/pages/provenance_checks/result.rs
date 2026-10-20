use crate::lint::catalog::catalog_entry;
use wenlan_types::lint::{
    LintApplicability, LintCheckResult, LintCheckResultInput, LintCoverage, LintEvidenceRef,
    LintGateEffect, LintMetric, LintOpaqueId, LintOutcome, LintPrecondition,
    LintRecommendationCode, LintSeverity, LintSummaryCode, LintValidationMethod,
    LINT_MAX_EVIDENCE_PER_CHECK,
};

/// The gate effect the catalog gives `check_id`: a result must carry it, and an
/// advisory check's Warning is only legal with it.
fn catalog_gate_effect(check_id: &str) -> LintGateEffect {
    catalog_entry(check_id).map_or(LintGateEffect::Actionable, |entry| entry.gate_effect)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::lint::pages) enum Level {
    Pass,
    Warning,
    Error,
}

#[derive(Debug, Default)]
pub(in crate::lint::pages) struct Assessment {
    rows: Vec<Level>,
    inventory: bool,
    metrics: Vec<LintMetric>,
    aggregate: Option<AggregateAssessment>,
}

#[derive(Debug)]
struct AggregateAssessment {
    population: u64,
    warning_count: u64,
    error_count: u64,
    evidence_positions: Vec<usize>,
}

impl Assessment {
    pub(in crate::lint::pages) fn push(&mut self, level: Level) {
        self.rows.push(level);
    }

    pub(in crate::lint::pages) fn mark_inventory(&mut self) {
        self.inventory = true;
    }

    pub(in crate::lint::pages) fn set_metrics(&mut self, metrics: Vec<LintMetric>) {
        self.metrics = metrics;
    }

    pub(in crate::lint::pages) fn from_aggregate(
        population: u64,
        warning_count: u64,
        error_count: u64,
        inventory: bool,
        evidence_positions: Vec<usize>,
    ) -> Self {
        Self {
            rows: Vec::new(),
            inventory,
            metrics: Vec::new(),
            aggregate: Some(AggregateAssessment {
                population,
                warning_count,
                error_count,
                evidence_positions,
            }),
        }
    }

    pub(in crate::lint::pages) fn population(&self) -> u64 {
        self.aggregate.as_ref().map_or_else(
            || u64::try_from(self.rows.len()).unwrap_or(u64::MAX),
            |aggregate| aggregate.population,
        )
    }

    pub(in crate::lint::pages) fn result(
        self,
        check_id: &str,
        duration_ms: u64,
    ) -> Result<LintCheckResult, wenlan_types::lint::LintContractError> {
        let (level, defect_count) = self.aggregate.as_ref().map_or_else(
            || {
                let level = self.rows.iter().copied().max().unwrap_or(Level::Pass);
                let count = self
                    .rows
                    .iter()
                    .filter(|level| **level != Level::Pass)
                    .count();
                (level, u64::try_from(count).unwrap_or(u64::MAX))
            },
            |aggregate| {
                let level = if aggregate.error_count > 0 {
                    Level::Error
                } else if aggregate.warning_count > 0 {
                    Level::Warning
                } else {
                    Level::Pass
                };
                (
                    level,
                    aggregate
                        .warning_count
                        .saturating_add(aggregate.error_count),
                )
            },
        );
        let evidence_positions = self.aggregate.as_ref().map_or_else(
            || {
                self.rows
                    .iter()
                    .enumerate()
                    .filter(|(_, level)| **level != Level::Pass)
                    .map(|(position, _)| position)
                    .collect::<Vec<_>>()
            },
            |aggregate| aggregate.evidence_positions.clone(),
        );
        let evidence = evidence_positions
            .into_iter()
            .take(usize::from(LINT_MAX_EVIDENCE_PER_CHECK))
            .filter_map(|position| {
                LintOpaqueId::from_sorted_position(position)
                    .map(|opaque_id| LintEvidenceRef::OpaqueId { opaque_id })
            })
            .collect::<Vec<_>>();
        let population = self.population();
        let (outcome, severity, applicability, summary_code, recommendation_code) = match level {
            Level::Pass => (
                LintOutcome::Pass,
                LintSeverity::Info,
                if self.inventory || self.rows.is_empty() {
                    LintApplicability::Inventory
                } else {
                    LintApplicability::Applicable
                },
                LintSummaryCode::CheckPassed,
                None,
            ),
            Level::Warning => (
                LintOutcome::Finding,
                LintSeverity::Warning,
                LintApplicability::Applicable,
                LintSummaryCode::FindingDetected,
                Some(LintRecommendationCode::ReviewFinding),
            ),
            Level::Error => (
                LintOutcome::Finding,
                LintSeverity::Error,
                LintApplicability::Applicable,
                LintSummaryCode::FindingDetected,
                Some(LintRecommendationCode::ReviewFinding),
            ),
        };
        LintCheckResult::try_new_with_gate_effect(
            LintCheckResultInput {
                check_id: check_id.to_string(),
                outcome,
                severity,
                applicability,
                precondition: LintPrecondition::Ready,
                coverage: LintCoverage::new(
                    LintValidationMethod::FullEnumeration,
                    population,
                    population,
                    LINT_MAX_EVIDENCE_PER_CHECK,
                    defect_count > u64::from(LINT_MAX_EVIDENCE_PER_CHECK),
                    u64::try_from(evidence.len()).unwrap_or(u64::MAX),
                )?,
                metrics: self.metrics,
                summary_code,
                recommendation_code,
                evidence,
                duration_ms,
            },
            catalog_gate_effect(check_id),
        )
    }
}

pub(in crate::lint::pages) fn failed_result(check_id: &str, duration_ms: u64) -> LintCheckResult {
    LintCheckResult::try_new_with_gate_effect(
        LintCheckResultInput {
            check_id: check_id.to_string(),
            outcome: LintOutcome::FailedToRun,
            severity: LintSeverity::Error,
            applicability: LintApplicability::Applicable,
            precondition: LintPrecondition::Ready,
            coverage: LintCoverage::new(
                LintValidationMethod::FullEnumeration,
                0,
                0,
                LINT_MAX_EVIDENCE_PER_CHECK,
                false,
                0,
            )
            .expect("static failed coverage is valid"),
            metrics: Vec::new(),
            summary_code: LintSummaryCode::ExecutionFailed,
            recommendation_code: Some(LintRecommendationCode::InspectRuntime),
            evidence: Vec::new(),
            duration_ms,
        },
        catalog_gate_effect(check_id),
    )
    .expect("static failed result is valid")
}
