use super::code_basis::CODE_BASIS_DRIFT_ID;
use super::*;
use crate::code_basis::prepare_code_basis;
use crate::db::code_basis_test::test_db;
use crate::db::MemoryDB;
use crate::lint::context::{
    AppliedScope, CancellationToken, ExecutionGate, LintClock, LintContext,
};
use wenlan_types::lint::{
    LintApplicability, LintCheckResult, LintEvidenceRef, LintMetricCode, LintMetricValue,
    LintOpaqueId, LintOutcome, LintProfile, LintSeverity,
};
use wenlan_types::CodeBasisInput;

struct Repo {
    dir: tempfile::TempDir,
}

impl Repo {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn write(&self, path: &str, text: &str) {
        std::fs::write(self.dir.path().join(path), text).unwrap();
    }

    fn input(&self, path: &str, start: u32, end: u32) -> CodeBasisInput {
        CodeBasisInput {
            repo_root: self.dir.path().to_str().unwrap().to_string(),
            path: path.to_string(),
            line_start: start,
            line_end: end,
        }
    }
}

/// `n` numbered lines: `line 1`, `line 2`, ...
fn numbered(n: u32) -> String {
    (1..=n).map(|i| format!("line {i}\n")).collect()
}

async fn page(db: &MemoryDB, id: &str, workspace: &str) {
    // `insert_page_with_kind` mirrors the space into `workspace`.
    db.insert_page_with_kind(
        id,
        id,
        None,
        "body",
        None,
        Some(workspace),
        &[],
        "2026-01-01T00:00:00Z",
        "authored",
        "confirmed",
        None,
        None,
    )
    .await
    .unwrap();
}

/// The way the daemon records a reliance: read the lines now, store the row.
async fn rely(db: &MemoryDB, page_id: &str, input: CodeBasisInput) {
    let prepared = prepare_code_basis(&[input]).await.unwrap();
    db.upsert_page_code_basis(page_id, &prepared, "agent")
        .await
        .unwrap();
}

async fn state_of(db: &MemoryDB, page_id: &str) -> String {
    db.list_page_code_basis(page_id).await.unwrap()[0]
        .state
        .clone()
}

async fn drift(db: &MemoryDB, scope: &AppliedScope, profile: LintProfile) -> LintCheckResult {
    let snapshot = db.open_lint_snapshot().await.unwrap();
    let clock = LintClock::fixed();
    let gate = ExecutionGate::new(CancellationToken::new());
    let context = LintContext::new(&snapshot, scope, None, &clock, &gate, profile);
    run_code_basis_drift(&context).await
}

fn metric(result: &LintCheckResult, code: LintMetricCode) -> u64 {
    result
        .metrics()
        .iter()
        .find(|metric| metric.code() == code)
        .and_then(|metric| match metric.value() {
            LintMetricValue::Count { value } => Some(*value),
            LintMetricValue::Boolean { .. } | LintMetricValue::CatalogCode { .. } => None,
        })
        .unwrap()
}

#[tokio::test]
async fn no_code_bases_pass_with_an_empty_population() {
    let (db, _tmp) = test_db().await;
    page(&db, "page-plain", "workspace-a").await;
    let result = drift(&db, &AppliedScope::global(), LintProfile::Deep).await;
    assert_eq!(result.check_id(), CODE_BASIS_DRIFT_ID);
    assert_eq!(result.outcome(), LintOutcome::Pass);
    assert_eq!(result.applicability(), LintApplicability::Inventory);
    assert_eq!(result.coverage().denominator(), 0);
    for code in [
        LintMetricCode::EligibleRecords,
        LintMetricCode::AffectedRecords,
        LintMetricCode::ObservedRecords,
        LintMetricCode::PendingRecords,
        LintMetricCode::CodeBasisUnavailableRows,
    ] {
        assert_eq!(metric(&result, code), 0, "{code:?}");
    }
}

#[tokio::test]
async fn a_row_that_still_reads_the_same_passes() {
    let (db, _tmp) = test_db().await;
    let repo = Repo::new();
    repo.write("a.rs", &numbered(10));
    page(&db, "page-ok", "workspace-a").await;
    rely(&db, "page-ok", repo.input("a.rs", 3, 5)).await;
    // Same lines, different stamp: the file is read and found unchanged.
    repo.write("a.rs", &numbered(10));

    let result = drift(&db, &AppliedScope::global(), LintProfile::Deep).await;
    assert_eq!(result.outcome(), LintOutcome::Pass);
    assert_eq!(result.severity(), LintSeverity::Info);
    assert_eq!(result.coverage().denominator(), 1);
    assert_eq!(metric(&result, LintMetricCode::EligibleRecords), 1);
    assert_eq!(metric(&result, LintMetricCode::ObservedRecords), 1);
    assert_eq!(metric(&result, LintMetricCode::AffectedRecords), 0);
}

#[tokio::test]
async fn a_changed_file_is_a_warning_that_names_the_page_and_writes_nothing() {
    let (db, _tmp) = test_db().await;
    let repo = Repo::new();
    repo.write("a.rs", &numbered(10));
    repo.write("b.rs", &numbered(10));
    page(&db, "page-a-drift", "workspace-a").await;
    page(&db, "page-b-quiet", "workspace-a").await;
    rely(&db, "page-a-drift", repo.input("a.rs", 3, 5)).await;
    rely(&db, "page-b-quiet", repo.input("b.rs", 3, 5)).await;
    repo.write(
        "a.rs",
        "line 1\nline 2\nline three, rewritten at length\nline 4\nline 5 changed\nline 6\n",
    );

    let result = drift(&db, &AppliedScope::global(), LintProfile::Deep).await;
    assert_eq!(result.outcome(), LintOutcome::Finding);
    assert_eq!(result.severity(), LintSeverity::Warning);
    assert_eq!(result.coverage().denominator(), 2);
    assert_eq!(metric(&result, LintMetricCode::EligibleRecords), 2);
    assert_eq!(metric(&result, LintMetricCode::AffectedRecords), 1);
    assert_eq!(metric(&result, LintMetricCode::ObservedRecords), 2);
    assert_eq!(metric(&result, LintMetricCode::PendingRecords), 1);
    assert_eq!(metric(&result, LintMetricCode::CodeBasisUnavailableRows), 0);
    // Evidence is the sorted position of the warned page among the pages that
    // have a code basis: `page-a-drift` sorts first.
    assert_eq!(
        result.evidence(),
        &[LintEvidenceRef::OpaqueId {
            opaque_id: LintOpaqueId::from_sorted_position(0).unwrap()
        }]
    );
    // Read-only: the lint reported the drift but did not record it.
    assert_eq!(state_of(&db, "page-a-drift").await, "ok");
    println!("{}", serde_json::to_string(&result).unwrap());
}

#[tokio::test]
async fn a_missing_file_is_a_warning_counted_as_unavailable() {
    let (db, _tmp) = test_db().await;
    let repo = Repo::new();
    repo.write("gone.rs", &numbered(10));
    page(&db, "page-gone", "workspace-a").await;
    rely(&db, "page-gone", repo.input("gone.rs", 1, 2)).await;
    std::fs::remove_file(repo.dir.path().join("gone.rs")).unwrap();

    let result = drift(&db, &AppliedScope::global(), LintProfile::Deep).await;
    assert_eq!(result.outcome(), LintOutcome::Finding);
    assert_eq!(result.severity(), LintSeverity::Warning);
    assert_eq!(metric(&result, LintMetricCode::AffectedRecords), 1);
    assert_eq!(metric(&result, LintMetricCode::PendingRecords), 0);
    assert_eq!(metric(&result, LintMetricCode::CodeBasisUnavailableRows), 1);
    assert_eq!(state_of(&db, "page-gone").await, "ok");
}

#[tokio::test]
async fn lines_that_only_moved_pass() {
    let (db, _tmp) = test_db().await;
    let repo = Repo::new();
    repo.write("a.rs", &numbered(10));
    page(&db, "page-moved", "workspace-a").await;
    rely(&db, "page-moved", repo.input("a.rs", 3, 5)).await;
    repo.write("a.rs", &format!("new one\nnew two\n{}", numbered(10)));

    let result = drift(&db, &AppliedScope::global(), LintProfile::Deep).await;
    assert_eq!(result.outcome(), LintOutcome::Pass);
    assert_eq!(metric(&result, LintMetricCode::EligibleRecords), 1);
    assert_eq!(metric(&result, LintMetricCode::AffectedRecords), 0);
    assert_eq!(metric(&result, LintMetricCode::PendingRecords), 0);
    assert_eq!(state_of(&db, "page-moved").await, "ok");
}

#[tokio::test]
async fn a_registered_scope_reads_only_its_own_pages() {
    let (db, _tmp) = test_db().await;
    let repo = Repo::new();
    repo.write("a.rs", &numbered(10));
    repo.write("outside.rs", &numbered(10));
    page(&db, "page-in", "workspace-a").await;
    page(&db, "page-out", "workspace-b").await;
    rely(&db, "page-in", repo.input("a.rs", 1, 3)).await;
    rely(&db, "page-out", repo.input("outside.rs", 1, 3)).await;
    // The out-of-scope file is gone; were it read it would show as unavailable.
    std::fs::remove_file(repo.dir.path().join("outside.rs")).unwrap();

    let scope = AppliedScope::registered(
        LintOpaqueId::from_sorted_position(0).unwrap(),
        "workspace-a".to_string(),
    );
    let scoped = drift(&db, &scope, LintProfile::Deep).await;
    assert_eq!(scoped.outcome(), LintOutcome::Pass);
    assert_eq!(metric(&scoped, LintMetricCode::EligibleRecords), 1);
    assert_eq!(metric(&scoped, LintMetricCode::ObservedRecords), 1);
    assert_eq!(metric(&scoped, LintMetricCode::CodeBasisUnavailableRows), 0);

    let global = drift(&db, &AppliedScope::global(), LintProfile::Deep).await;
    assert_eq!(global.outcome(), LintOutcome::Finding);
    assert_eq!(metric(&global, LintMetricCode::EligibleRecords), 2);
    assert_eq!(metric(&global, LintMetricCode::CodeBasisUnavailableRows), 1);
}

/// Through the whole runner: the deep profile reports the check (and counts
/// it in its catalog), the general profile does not know it.
#[tokio::test]
async fn only_the_deep_profile_runs_it_and_a_drifted_note_does_not_fail_the_gate() {
    use crate::lint::runner::LintRunner;
    use wenlan_types::lint::LintQuery;

    let (db, _tmp) = test_db().await;
    let repo = Repo::new();
    repo.write("a.rs", &numbered(10));
    page(&db, "page-drift", "workspace-a").await;
    rely(&db, "page-drift", repo.input("a.rs", 3, 5)).await;
    std::fs::remove_file(repo.dir.path().join("a.rs")).unwrap();

    let runner = LintRunner::new(LintClock::fixed(), CancellationToken::new());
    let general = runner
        .run(
            &db,
            &LintQuery {
                profile: None,
                space: None,
            },
            None,
            false,
        )
        .await
        .unwrap();
    assert!(general
        .checks()
        .iter()
        .all(|check| check.check_id() != CODE_BASIS_DRIFT_ID));
    let deep = runner
        .run(
            &db,
            &LintQuery {
                profile: Some(LintProfile::Deep),
                space: None,
            },
            None,
            false,
        )
        .await
        .unwrap();
    assert_eq!(
        deep.checks().len(),
        wenlan_types::lint::LINT_DEEP_CHECK_COUNT
    );
    let check = deep
        .checks()
        .iter()
        .find(|check| check.check_id() == CODE_BASIS_DRIFT_ID)
        .expect("the deep profile reports the check");
    assert_eq!(check.outcome(), LintOutcome::Finding);
    assert_eq!(check.severity(), LintSeverity::Warning);
}
