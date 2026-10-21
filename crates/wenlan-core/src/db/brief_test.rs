// SPDX-License-Identifier: Apache-2.0

use super::tests::test_db;
use super::MemoryDB;
use wenlan_types::{
    BriefConflictReason, BriefItemState, BriefMutation, BriefSummaryUpdate, BriefUpdateRequest,
};

#[tokio::test]
async fn brief_migration_creates_fk_backed_tables() {
    let (db, _temp) = test_db().await;
    {
        let conn = db.conn.lock().await;
        conn.execute_batch(
            "DROP TABLE brief_items;
             DROP TABLE briefs;
             DROP TABLE brief_legacy_imports;
             PRAGMA user_version=101;",
        )
        .await
        .unwrap();
    }
    db.migrate_102_brief(101).await.unwrap();
    let conn = db.conn.lock().await;

    for table in ["briefs", "brief_items", "brief_legacy_imports"] {
        let mut rows = conn
            .query(
                "SELECT name FROM sqlite_master WHERE type='table' AND name=?1",
                libsql::params![table],
            )
            .await
            .unwrap();
        assert!(rows.next().await.unwrap().is_some(), "missing {table}");
    }

    // 102, not `SCHEMA_VERSION`. This test winds the database back to 101 and
    // calls migration 102 alone, so what it can prove is that migration 102
    // stamps *its own* version. Comparing against the head instead held only
    // while 102 happened to be the last migration in the chain, which made the
    // next one -- any next one -- fail a test about briefs.
    let mut version_rows = conn.query("PRAGMA user_version", ()).await.unwrap();
    assert_eq!(
        version_rows
            .next()
            .await
            .unwrap()
            .unwrap()
            .get::<i64>(0)
            .unwrap(),
        102
    );
}

#[tokio::test]
async fn brief_migration_follows_space_identity_across_rename_and_delete() {
    let (db, _temp) = test_db().await;
    let space = db.create_space("before", None, false).await.unwrap();
    {
        let conn = db.conn.lock().await;
        conn.execute(
            "INSERT INTO briefs
                (space_id, last_session_summary, last_handoff_at, version, updated_at)
             VALUES (?1, 'summary', 10, 1, 10)",
            libsql::params![space.id.as_str()],
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO brief_items
                (id, space_id, text, state, added_at, gate, version, updated_at)
             VALUES ('item-1', ?1, 'next step', 'active', 10, NULL, 1, 10)",
            libsql::params![space.id.as_str()],
        )
        .await
        .unwrap();
    }

    db.update_space("before", "after", None).await.unwrap();
    assert_eq!(
        brief_space_id(&db).await.as_deref(),
        Some(space.id.as_str())
    );

    db.delete_space("after", "keep").await.unwrap();
    assert!(brief_space_id(&db).await.is_none());
    let conn = db.conn.lock().await;
    let mut rows = conn
        .query("SELECT COUNT(*) FROM brief_items", ())
        .await
        .unwrap();
    assert_eq!(
        rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
        0
    );
}

async fn brief_space_id(db: &MemoryDB) -> Option<String> {
    let conn = db.conn.lock().await;
    let mut rows = conn.query("SELECT space_id FROM briefs", ()).await.unwrap();
    rows.next()
        .await
        .unwrap()
        .map(|row| row.get::<String>(0).unwrap())
}

fn update_request(
    space: &str,
    operation_id: &str,
    mutations: Vec<BriefMutation>,
) -> BriefUpdateRequest {
    BriefUpdateRequest {
        space: space.into(),
        caller_id: "brief-test".into(),
        operation_id: operation_id.into(),
        summary: None,
        mutations,
    }
}

fn add(text: &str, state: BriefItemState) -> BriefMutation {
    BriefMutation::Add {
        text: text.into(),
        state,
        added_at: None,
        gate: None,
    }
}

#[tokio::test]
async fn brief_read_without_stored_brief_is_write_free() {
    let (db, _temp) = test_db().await;
    db.create_space("empty", None, false).await.unwrap();

    assert!(db.get_brief_by_space_name("empty").await.unwrap().is_none());

    let conn = db.conn.lock().await;
    let mut rows = conn.query("SELECT COUNT(*) FROM briefs", ()).await.unwrap();
    assert_eq!(
        rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
        0
    );
}

#[tokio::test]
async fn brief_update_cannot_create_the_whole_library_scope_as_a_space() {
    let (db, _temp) = test_db().await;
    let error = db
        .apply_brief_update(&update_request(
            " * ",
            "reserved",
            vec![add("ship the release", BriefItemState::Active)],
        ))
        .await
        .unwrap_err();

    assert!(matches!(error, crate::error::WenlanError::Validation(_)));
    assert!(db.get_space("*").await.unwrap().is_none());
    assert!(brief_space_id(&db).await.is_none());
}

#[tokio::test]
async fn brief_first_update_creates_brief_and_stable_item_id() {
    let (db, _temp) = test_db().await;
    let receipt = db
        .apply_brief_update(&update_request(
            "new-space",
            "create",
            vec![add("ship the release", BriefItemState::Active)],
        ))
        .await
        .unwrap();

    assert!(db.get_space("new-space").await.unwrap().is_some());
    assert_eq!(receipt.applied.len(), 1);
    let item_id = receipt.applied[0].item_id.as_deref().unwrap();
    assert!(!item_id.is_empty());

    let brief = db
        .get_brief_by_space_name("new-space")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(brief.active.len(), 1);
    assert_eq!(brief.active[0].id, item_id);
    assert_eq!(brief.active[0].version, 1);
}

#[tokio::test]
async fn brief_mutations_require_current_item_version_and_completion_removes_item() {
    let (db, _temp) = test_db().await;
    let created = db
        .apply_brief_update(&update_request(
            "mutations",
            "add",
            vec![add("draft", BriefItemState::Active)],
        ))
        .await
        .unwrap();
    let item_id = created.applied[0].item_id.clone().unwrap();

    let edited = db
        .apply_brief_update(&update_request(
            "mutations",
            "edit",
            vec![BriefMutation::Edit {
                item_id: item_id.clone(),
                expected_version: 1,
                text: "edited".into(),
            }],
        ))
        .await
        .unwrap();
    assert_eq!(edited.applied[0].version, Some(2));

    let moved = db
        .apply_brief_update(&update_request(
            "mutations",
            "move",
            vec![BriefMutation::Move {
                item_id: item_id.clone(),
                expected_version: 2,
                state: BriefItemState::Backlog,
            }],
        ))
        .await
        .unwrap();
    assert_eq!(moved.applied[0].version, Some(3));

    let gated = db
        .apply_brief_update(&update_request(
            "mutations",
            "gate",
            vec![BriefMutation::SetGate {
                item_id: item_id.clone(),
                expected_version: 3,
                gate: Some("CI green".into()),
            }],
        ))
        .await
        .unwrap();
    assert_eq!(gated.applied[0].version, Some(4));

    let completed = db
        .apply_brief_update(&update_request(
            "mutations",
            "complete",
            vec![BriefMutation::Complete {
                item_id,
                expected_version: 4,
            }],
        ))
        .await
        .unwrap();
    assert_eq!(completed.applied.len(), 1);

    let brief = db
        .get_brief_by_space_name("mutations")
        .await
        .unwrap()
        .unwrap();
    assert!(brief.active.is_empty());
    assert!(brief.backlog.is_empty());
}

#[tokio::test]
async fn brief_stale_item_conflict_does_not_block_unrelated_mutation() {
    let (db, _temp) = test_db().await;
    let created = db
        .apply_brief_update(&update_request(
            "concurrency",
            "add-two",
            vec![
                add("first", BriefItemState::Active),
                add("second", BriefItemState::Active),
            ],
        ))
        .await
        .unwrap();
    let first = created.applied[0].item_id.clone().unwrap();
    let second = created.applied[1].item_id.clone().unwrap();

    db.apply_brief_update(&update_request(
        "concurrency",
        "advance-first",
        vec![BriefMutation::Edit {
            item_id: first.clone(),
            expected_version: 1,
            text: "newer first".into(),
        }],
    ))
    .await
    .unwrap();

    let partial = db
        .apply_brief_update(&update_request(
            "concurrency",
            "partial",
            vec![
                BriefMutation::Edit {
                    item_id: first.clone(),
                    expected_version: 1,
                    text: "stale overwrite".into(),
                },
                BriefMutation::Move {
                    item_id: second.clone(),
                    expected_version: 1,
                    state: BriefItemState::Backlog,
                },
            ],
        ))
        .await
        .unwrap();

    assert_eq!(partial.applied.len(), 1);
    assert_eq!(partial.applied[0].item_id.as_deref(), Some(second.as_str()));
    assert_eq!(partial.conflicts.len(), 1);
    assert_eq!(
        partial.conflicts[0].item_id.as_deref(),
        Some(first.as_str())
    );
    assert_eq!(
        partial.conflicts[0].reason,
        BriefConflictReason::VersionMismatch
    );

    let brief = db
        .get_brief_by_space_name("concurrency")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(brief.active.len(), 1);
    assert_eq!(brief.active[0].text, "newer first");
    assert_eq!(brief.backlog.len(), 1);
    assert_eq!(brief.backlog[0].id, second);
}

#[tokio::test]
async fn brief_same_request_can_apply_multiple_deltas_from_one_item_snapshot() {
    let (db, _temp) = test_db().await;
    let created = db
        .apply_brief_update(&update_request(
            "snapshot",
            "create-item",
            vec![add("draft release", BriefItemState::Active)],
        ))
        .await
        .unwrap();
    let item_id = created.applied[0].item_id.clone().unwrap();

    let receipt = db
        .apply_brief_update(&update_request(
            "snapshot",
            "compose-deltas",
            vec![
                BriefMutation::Edit {
                    item_id: item_id.clone(),
                    expected_version: 1,
                    text: "ship release".into(),
                },
                BriefMutation::Move {
                    item_id: item_id.clone(),
                    expected_version: 1,
                    state: BriefItemState::Backlog,
                },
                BriefMutation::SetGate {
                    item_id: item_id.clone(),
                    expected_version: 1,
                    gate: Some("CI passes".into()),
                },
            ],
        ))
        .await
        .unwrap();

    assert_eq!(receipt.applied.len(), 3);
    assert!(receipt.conflicts.is_empty());
    assert_eq!(
        receipt
            .applied
            .iter()
            .map(|entry| entry.version)
            .collect::<Vec<_>>(),
        vec![Some(2), Some(3), Some(4)]
    );

    let brief = db
        .get_brief_by_space_name("snapshot")
        .await
        .unwrap()
        .unwrap();
    assert!(brief.active.is_empty());
    assert_eq!(brief.backlog[0].id, item_id);
    assert_eq!(brief.backlog[0].text, "ship release");
    assert_eq!(brief.backlog[0].gate.as_deref(), Some("CI passes"));
    assert_eq!(brief.backlog[0].version, 4);
}

#[tokio::test]
async fn brief_operation_replay_is_idempotent_and_digest_mismatch_conflicts() {
    let (db, _temp) = test_db().await;
    let request = update_request(
        "replay",
        "same-operation",
        vec![add("only once", BriefItemState::Active)],
    );
    let first = db.apply_brief_update(&request).await.unwrap();
    let replay = db.apply_brief_update(&request).await.unwrap();
    assert_eq!(first, replay);

    let brief = db.get_brief_by_space_name("replay").await.unwrap().unwrap();
    assert_eq!(brief.active.len(), 1);

    let reused = update_request(
        "replay",
        "same-operation",
        vec![add("different request", BriefItemState::Active)],
    );
    let error = db.apply_brief_update(&reused).await.unwrap_err();
    assert!(matches!(error, crate::error::WenlanError::Conflict(_)));
}

#[tokio::test]
async fn brief_summary_uses_brief_version_without_blocking_item_changes() {
    let (db, _temp) = test_db().await;
    db.apply_brief_update(&update_request(
        "summary",
        "create",
        vec![add("keep moving", BriefItemState::Active)],
    ))
    .await
    .unwrap();

    let first_summary = BriefUpdateRequest {
        space: "summary".into(),
        caller_id: "brief-test".into(),
        operation_id: "summary-one".into(),
        summary: Some(BriefSummaryUpdate {
            text: "first summary".into(),
            expected_version: 1,
        }),
        mutations: Vec::new(),
    };
    db.apply_brief_update(&first_summary).await.unwrap();

    let partial = BriefUpdateRequest {
        space: "summary".into(),
        caller_id: "brief-test".into(),
        operation_id: "summary-stale".into(),
        summary: Some(BriefSummaryUpdate {
            text: "stale summary".into(),
            expected_version: 1,
        }),
        mutations: vec![add("safe addition", BriefItemState::Backlog)],
    };
    let receipt = db.apply_brief_update(&partial).await.unwrap();
    assert_eq!(receipt.applied.len(), 1);
    assert_eq!(receipt.conflicts.len(), 1);
    assert_eq!(
        receipt.conflicts[0].reason,
        BriefConflictReason::BriefVersionMismatch
    );

    let brief = db
        .get_brief_by_space_name("summary")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(brief.last_session_summary, "first summary");
    assert_eq!(brief.backlog.len(), 1);
}

#[tokio::test]
async fn brief_history_records_one_row_per_applied_update_with_snapshot() {
    let (db, _temp) = test_db().await;
    let first = db
        .apply_brief_update(&update_request(
            "history",
            "op-one",
            vec![add("first item", BriefItemState::Active)],
        ))
        .await
        .unwrap();
    let item_id = first.applied[0].item_id.clone().unwrap();
    let second = BriefUpdateRequest {
        space: "history".into(),
        caller_id: "other-caller".into(),
        operation_id: "op-two".into(),
        summary: Some(BriefSummaryUpdate {
            text: "second summary".into(),
            expected_version: first.brief_version,
        }),
        mutations: vec![BriefMutation::Move {
            item_id,
            expected_version: 1,
            state: BriefItemState::Backlog,
        }],
    };
    let second_receipt = db.apply_brief_update(&second).await.unwrap();
    assert_eq!(second_receipt.brief_version, 2);

    let history = db.list_brief_history("history", 10).await.unwrap();
    assert_eq!(
        history.iter().map(|e| e.version).collect::<Vec<_>>(),
        vec![2, 1]
    );
    assert_eq!(history[0].caller_id, "other-caller");
    assert_eq!(history[0].operation_id, "op-two");
    assert_eq!(history[0].summary, "second summary");
    assert_eq!(history[0].mutation_count, 2);
    assert_eq!(history[1].caller_id, "brief-test");
    assert_eq!(history[1].mutation_count, 1);
    assert_eq!(history[1].summary, "");

    // limit keeps the newest entries
    let limited = db.list_brief_history("history", 1).await.unwrap();
    assert_eq!(limited.len(), 1);
    assert_eq!(limited[0].version, 2);

    // v1 shows the Brief as it was, v2 the current one
    let v1 = db.get_brief_version("history", 1).await.unwrap().unwrap();
    assert_eq!(v1.brief.version, 1);
    assert_eq!(v1.brief.active.len(), 1);
    assert!(v1.brief.backlog.is_empty());
    assert_eq!(v1.brief.last_session_summary, "");
    let v2 = db.get_brief_version("history", 2).await.unwrap().unwrap();
    assert!(v2.brief.active.is_empty());
    assert_eq!(v2.brief.backlog.len(), 1);
    assert_eq!(v2.brief.last_session_summary, "second summary");
    assert_eq!(
        db.get_brief_by_space_name("history")
            .await
            .unwrap()
            .unwrap(),
        v2.brief
    );
    assert!(db.get_brief_version("history", 3).await.unwrap().is_none());
    assert!(db
        .get_brief_version("elsewhere", 1)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn brief_history_skips_conflict_only_updates_and_replays() {
    let (db, _temp) = test_db().await;
    let created = db
        .apply_brief_update(&update_request(
            "quiet",
            "create",
            vec![add("only item", BriefItemState::Active)],
        ))
        .await
        .unwrap();
    let item_id = created.applied[0].item_id.clone().unwrap();

    // every mutation conflicts: nothing applies, so no history row
    let conflict = update_request(
        "quiet",
        "stale",
        vec![BriefMutation::Edit {
            item_id,
            expected_version: 99,
            text: "stale".into(),
        }],
    );
    let receipt = db.apply_brief_update(&conflict).await.unwrap();
    assert!(receipt.applied.is_empty());
    assert_eq!(receipt.conflicts.len(), 1);

    // an exact replay of an applied operation adds nothing either
    let replay = update_request(
        "quiet",
        "replayed",
        vec![add("replayed item", BriefItemState::Backlog)],
    );
    db.apply_brief_update(&replay).await.unwrap();
    db.apply_brief_update(&replay).await.unwrap();

    let versions: Vec<i64> = db
        .list_brief_history("quiet", 10)
        .await
        .unwrap()
        .iter()
        .map(|e| e.version)
        .collect();
    assert_eq!(versions, vec![2, 1]);
}

#[tokio::test]
async fn brief_history_migration_creates_table_and_stamps_136() {
    let (db, _temp) = test_db().await;
    {
        let conn = db.conn.lock().await;
        conn.execute_batch("DROP TABLE brief_history; PRAGMA user_version=135;")
            .await
            .unwrap();
    }
    db.migrate_136_brief_history().await.unwrap();
    db.migrate_136_brief_history().await.unwrap();
    // list_brief_history reads session_id, which only migration 140 adds,
    // so check the bare 136 table directly.
    let conn = db.conn.lock().await;
    let mut count = conn
        .query("SELECT COUNT(*) FROM brief_history", ())
        .await
        .unwrap();
    assert_eq!(
        count.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
        0
    );
    drop(count);
    let mut rows = conn.query("PRAGMA user_version", ()).await.unwrap();
    assert_eq!(
        rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
        136
    );
}
