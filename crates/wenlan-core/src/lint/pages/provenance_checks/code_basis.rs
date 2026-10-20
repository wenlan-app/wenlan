use super::result::{Assessment, Level};
use super::source::scoped_pages;
use crate::code_basis::classify_code_basis_rows;
use crate::db::{code_basis_row_from, CodeBasisRow, CodeBasisVerdict, CODE_BASIS_ROW_SELECT};
use crate::lint::context::LintContext;
use std::collections::BTreeMap;
use wenlan_types::lint::{LintMetric, LintMetricCode, LintMetricValue};

/// Advisory: a note relies on lines of code that changed or went missing.
///
/// Read-only. The files are read to say what each row WOULD be now; no verdict
/// is written (`recheck_code_basis` does that on the daemon's own schedule), so
/// a lint run never changes what a later run or a reader sees.
pub(crate) const CODE_BASIS_DRIFT_ID: &str = "pages.provenance.code_basis_drift";

pub(super) async fn load_and_assess_code_basis(
    context: &LintContext<'_, '_>,
) -> Result<Assessment, ()> {
    let rows = load_rows(context).await?;
    context
        .gate()
        .check_run_for(context.profile(), context.clock().elapsed())
        .map_err(|_| ())?;
    let verdicts = classify_code_basis_rows(&rows).await;
    Ok(assess_code_basis(&rows, &verdicts))
}

/// Every code basis of an active, non-entity note in the lint scope, notes in
/// id order. Only these notes' files are ever read.
async fn load_rows(context: &LintContext<'_, '_>) -> Result<Vec<CodeBasisRow>, ()> {
    let (scope_sql, params) = scoped_pages(context.scope().filter());
    let sql = format!(
        "{CODE_BASIS_ROW_SELECT} \
          WHERE b.page_id IN ({scope_sql}) \
            AND c.status = 'active' AND COALESCE(c.kind, 'concept') <> 'entity' \
          ORDER BY b.page_id, b.repo_root, b.path, b.line_start, b.line_end"
    );
    let mut rows = context
        .snapshot()
        .query(&sql, params)
        .await
        .map_err(|_| ())?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.map_err(|_| ())? {
        if let Some(found) = code_basis_row_from(&row).map_err(|_| ())? {
            out.push(found);
        }
    }
    Ok(out)
}

/// A note is a Warning when any of its rows is `pending` or `unavailable` now;
/// `moved` is fine. `verdicts` holds the rows that were read (see
/// `classify_code_basis_rows`); a row without one is unchanged on disk.
fn assess_code_basis(
    rows: &[CodeBasisRow],
    verdicts: &[(&CodeBasisRow, CodeBasisVerdict)],
) -> Assessment {
    let (mut pending, mut unavailable) = (0_u64, 0_u64);
    // Rows arrive in note order, so the map's order is the evidence order.
    let mut drifted: BTreeMap<&str, bool> = BTreeMap::new();
    for row in rows {
        drifted.entry(row.page_id.as_str()).or_insert(false);
    }
    for (row, verdict) in verdicts {
        let bad = match verdict {
            CodeBasisVerdict::Pending => {
                pending += 1;
                true
            }
            CodeBasisVerdict::Unavailable(_) => {
                unavailable += 1;
                true
            }
            CodeBasisVerdict::Unchanged { .. } | CodeBasisVerdict::Moved { .. } => false,
        };
        if bad {
            drifted.insert(row.page_id.as_str(), true);
        }
    }
    let mut assessment = Assessment::default();
    for page_drifted in drifted.values() {
        assessment.push(if *page_drifted {
            Level::Warning
        } else {
            Level::Pass
        });
    }
    let affected = drifted
        .values()
        .filter(|page_drifted| **page_drifted)
        .count();
    assessment.set_metrics(vec![
        count_metric(LintMetricCode::EligibleRecords, drifted.len() as u64),
        count_metric(LintMetricCode::AffectedRecords, affected as u64),
        count_metric(LintMetricCode::ObservedRecords, rows.len() as u64),
        count_metric(LintMetricCode::PendingRecords, pending),
        count_metric(LintMetricCode::CodeBasisUnavailableRows, unavailable),
    ]);
    assessment
}

fn count_metric(code: LintMetricCode, value: u64) -> LintMetric {
    LintMetric::new(code, LintMetricValue::Count { value })
}
