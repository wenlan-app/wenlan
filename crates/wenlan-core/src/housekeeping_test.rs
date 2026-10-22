use super::*;
use std::sync::Arc;

async fn test_db() -> (MemoryDB, tempfile::TempDir) {
    let dir = tempfile::TempDir::new().unwrap();
    let db_path = dir.path().join("test.db");
    let db = MemoryDB::new(db_path.as_path(), Arc::new(crate::events::NoopEmitter))
        .await
        .unwrap();
    (db, dir)
}

#[test]
fn due_once_a_day_and_when_never_run() {
    let now = 1_800_000_000;
    assert!(housekeeping_due(None, now));
    assert!(!housekeeping_due(Some(now - 60), now));
    assert!(!housekeeping_due(
        Some(now - HOUSEKEEPING_INTERVAL_SECS + 1),
        now
    ));
    assert!(housekeeping_due(
        Some(now - HOUSEKEEPING_INTERVAL_SECS),
        now
    ));
    assert!(
        housekeeping_due(Some(now + 3_600), now),
        "a run recorded in the future must not stall housekeeping"
    );
}

#[tokio::test]
async fn a_run_needs_no_model_and_records_itself() {
    let (db, _dir) = test_db().await;
    assert_eq!(last_housekeeping_run(&db).await, None);

    let report = run_housekeeping(&db, 1_800_000_000).await;

    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(last_housekeeping_run(&db).await, Some(1_800_000_000));
    assert!(!housekeeping_due(
        last_housekeeping_run(&db).await,
        1_800_000_000 + 60
    ));
}
