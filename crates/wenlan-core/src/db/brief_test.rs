// SPDX-License-Identifier: Apache-2.0

use super::tests::test_db;
use super::MemoryDB;
use crate::conversation_intake::FoundConversation;
use std::path::PathBuf;
use wenlan_types::{
    BriefConflictReason, BriefItemState, BriefMutation, BriefSummaryUpdate, BriefUpdateRequest,
    SessionRef,
};

fn not_shared(_: &str) -> Result<bool, String> {
    Ok(false)
}

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
async fn handoff_origin_migration_marks_existing_items_confirmed() {
    let (db, _temp) = test_db().await;
    let created = db
        .apply_brief_update(&update_request(
            "work",
            "before-168",
            vec![
                add("Existing line", BriefItemState::Active),
                add("No legacy thing", BriefItemState::Active),
            ],
        ))
        .await
        .unwrap();
    let legacy_item_id = created.applied[0].item_id.as_deref().unwrap();
    let unlinked_item_id = created.applied[1].item_id.as_deref().unwrap();
    {
        let conn = db.conn.lock().await;
        conn.execute(
            "INSERT INTO things
                (id, title, title_by, brief_item_id, created_at, updated_at)
             VALUES ('thg_legacy_brief', 'Existing line', 'ai', ?1, 1, 1)",
            libsql::params![legacy_item_id],
        )
        .await
        .unwrap();
        conn.execute_batch(
            "ALTER TABLE brief_items DROP COLUMN filled_by_wenlan;
             ALTER TABLE brief_items DROP COLUMN confirmed;
             PRAGMA user_version=167;",
        )
        .await
        .unwrap();
    }

    db.migrate_168_handoff_origin(167).await.unwrap();
    let brief = db.get_brief_by_space_name("work").await.unwrap().unwrap();
    assert!(brief.active.iter().all(|item| item.confirmed));
    assert!(brief.active.iter().all(|item| !item.filled_by_wenlan));
    let conn = db.conn.lock().await;
    let mut rows = conn
        .query(
            "SELECT COUNT(*) FROM things WHERE brief_item_id=?1",
            libsql::params![legacy_item_id],
        )
        .await
        .unwrap();
    assert_eq!(
        rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
        1,
        "migration preserves a thing made by an older daemon"
    );
    let mut rows = conn
        .query(
            "SELECT COUNT(*) FROM things WHERE brief_item_id=?1",
            libsql::params![unlinked_item_id],
        )
        .await
        .unwrap();
    assert_eq!(
        rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
        0,
        "migration does not backfill a thing for a Brief item"
    );
}

#[tokio::test]
async fn brief_details_migration_preserves_items_and_adds_nullable_column() {
    let (db, _temp) = test_db().await;
    db.apply_brief_update(&update_request(
        "work",
        "before-169",
        vec![add("Existing line", BriefItemState::Active)],
    ))
    .await
    .unwrap();
    {
        let conn = db.conn.lock().await;
        conn.execute_batch(
            "ALTER TABLE brief_items DROP COLUMN details;
             PRAGMA user_version=168;",
        )
        .await
        .unwrap();
    }

    db.migrate_169_brief_item_details(168).await.unwrap();
    let brief = db.get_brief_by_space_name("work").await.unwrap().unwrap();
    assert_eq!(brief.active[0].text, "Existing line");
    assert_eq!(brief.active[0].details, None);
    let conn = db.conn.lock().await;
    let mut version = conn.query("PRAGMA user_version", ()).await.unwrap();
    assert_eq!(
        version
            .next()
            .await
            .unwrap()
            .unwrap()
            .get::<i64>(0)
            .unwrap(),
        169
    );
}

#[tokio::test]
async fn handoff_details_round_trip_and_edit_without_details_preserves_them() {
    let (db, _temp) = test_db().await;
    let created = db
        .apply_brief_update(&update_request(
            "work",
            "details-add",
            vec![BriefMutation::Add {
                text: "Ship the importer".into(),
                details: Some("Decision: keep files after import. Next: verify replay.".into()),
                state: BriefItemState::Active,
                added_at: None,
                gate: None,
            }],
        ))
        .await
        .unwrap();
    let item_id = created.applied[0].item_id.clone().unwrap();
    db.apply_brief_update(&update_request(
        "work",
        "details-edit",
        vec![BriefMutation::Edit {
            item_id,
            expected_version: 1,
            text: "Ship and verify the importer".into(),
            details: None,
        }],
    ))
    .await
    .unwrap();

    let item = &db
        .get_brief_by_space_name("work")
        .await
        .unwrap()
        .unwrap()
        .active[0];
    assert_eq!(item.text, "Ship and verify the importer");
    assert_eq!(
        item.details.as_deref(),
        Some("Decision: keep files after import. Next: verify replay.")
    );
}

#[tokio::test]
async fn handoff_refuses_oversize_details() {
    let (db, _temp) = test_db().await;
    let error = db
        .apply_brief_update(&update_request(
            "work",
            "details-too-long",
            vec![BriefMutation::Add {
                text: "Line".into(),
                details: Some("x".repeat(crate::db::brief::BRIEF_ITEM_DETAILS_MAX_CHARS + 1)),
                state: BriefItemState::Active,
                added_at: None,
                gate: None,
            }],
        ))
        .await
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("details exceeds the 8000-character limit"));
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
        details: None,
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
                details: None,
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
async fn ux_hand_4_stale_item_conflict_does_not_overwrite_and_unrelated_mutation_applies() {
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
            details: None,
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
                    details: None,
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
                    details: None,
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
async fn ux_hand_4_replay_is_idempotent_and_digest_mismatch_conflicts() {
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
async fn handoff_add_makes_no_thing_and_replay_stays_single() {
    let (db, _temp) = test_db().await;
    let request = update_request(
        "work",
        "handoff-linked",
        vec![add("Ship the handoff route", BriefItemState::Active)],
    );

    let first = db.apply_brief_update(&request).await.unwrap();
    let replay = db.apply_brief_update(&request).await.unwrap();
    assert_eq!(first, replay);
    let brief = db.get_brief_by_space_name("work").await.unwrap().unwrap();
    assert_eq!(brief.active.len(), 1, "the replay applies the add once");
    let conn = db.conn.lock().await;
    let mut rows = conn
        .query(
            "SELECT COUNT(*) FROM things WHERE brief_item_id IS NOT NULL",
            (),
        )
        .await
        .unwrap();
    let row = rows.next().await.unwrap().unwrap();
    assert_eq!(row.get::<i64>(0).unwrap(), 0, "a Brief item is not a thing");
}

#[tokio::test]
async fn ux_hand_3_idle_fill_is_replaced_by_a_real_handoff() {
    let (db, _temp) = test_db().await;
    let session = SessionRef::parse("claude-code:idle-session").unwrap();
    let created = db
        .apply_brief_update_in_session(
            &update_request(
                "work",
                "real-1",
                vec![add("Implement Rule 2", BriefItemState::Active)],
            ),
            Some(&session),
        )
        .await
        .unwrap();
    let item_id = created.applied[0].item_id.clone().unwrap();
    let handed_at = chrono::Utc::now().timestamp();
    let conversation = FoundConversation {
        tool: "claude-code".into(),
        external_id: "idle-session".into(),
        path: PathBuf::from("/tmp/idle.jsonl"),
        len: 10,
        cwd: None,
        branch: None,
        said: vec!["keep going".into()],
        last_reply: "Regression test is now passing.".into(),
        last_content: "Regression test is now passing.".into(),
        first_at: handed_at - 60,
        last_content_at: handed_at + 1,
        last_at: handed_at + 1,
        automated: false,
        on_behalf: false,
        touched: vec![],
    };

    assert_eq!(
        db.fill_idle_handoffs(
            std::slice::from_ref(&conversation),
            &not_shared,
            None,
            "English",
            handed_at + 3001,
            3000,
        )
        .await
        .unwrap(),
        1
    );
    assert_eq!(
        db.fill_idle_handoffs(
            std::slice::from_ref(&conversation),
            &not_shared,
            None,
            "English",
            handed_at + 4000,
            3000,
        )
        .await
        .unwrap(),
        0
    );
    let filled = db.get_brief_by_space_name("work").await.unwrap().unwrap();
    let filled = &filled.active[0];
    assert!(!filled.confirmed);
    assert!(filled.filled_by_wenlan);
    assert_eq!(filled.text, "Implement Rule 2");
    assert!(filled
        .details
        .as_deref()
        .is_some_and(|details| details.contains("Regression test is now passing.")));

    db.apply_brief_update_in_session(
        &update_request(
            "work",
            "real-2",
            vec![BriefMutation::Edit {
                item_id,
                expected_version: filled.version,
                text: "Rule 2 is verified".into(),
                details: None,
            }],
        ),
        Some(&session),
    )
    .await
    .unwrap();
    let real = db.get_brief_by_space_name("work").await.unwrap().unwrap();
    assert!(real.active[0].confirmed);
    assert!(!real.active[0].filled_by_wenlan);
    assert_eq!(real.active[0].text, "Rule 2 is verified");

    let older_transcript = FoundConversation {
        last_content_at: handed_at,
        last_at: handed_at,
        ..conversation
    };
    assert_eq!(
        db.fill_idle_handoffs(
            &[older_transcript],
            &not_shared,
            None,
            "English",
            handed_at + 4000,
            3000,
        )
        .await
        .unwrap(),
        0,
        "a handoff at or after the last transcript line prevents another fill"
    );
}

#[tokio::test]
async fn idle_fill_uses_model_for_line_and_details_without_copying_raw_transcript() {
    let (db, _temp) = test_db().await;
    let session = SessionRef::parse("claude-code:model-fill").unwrap();
    db.apply_brief_update_in_session(
        &update_request(
            "model-work",
            "model-handoff",
            vec![BriefMutation::Add {
                text: "Continue the importer".into(),
                details: Some("The parser is complete; verification remains.".into()),
                state: BriefItemState::Active,
                added_at: None,
                gate: None,
            }],
        ),
        Some(&session),
    )
    .await
    .unwrap();
    let handed_at = db.list_brief_history("model-work", 1).await.unwrap()[0].created_at;
    let raw = "PRIVATE RAW TRANSCRIPT TEXT";
    let conversation = FoundConversation {
        tool: "claude-code".into(),
        external_id: "model-fill".into(),
        path: PathBuf::from("/tmp/model-fill.jsonl"),
        len: 10,
        cwd: None,
        branch: None,
        said: vec!["please continue".into()],
        last_reply: raw.into(),
        last_content: raw.into(),
        first_at: handed_at - 60,
        last_content_at: handed_at + 1,
        last_at: handed_at + 1,
        automated: false,
        on_behalf: false,
        touched: vec![],
    };
    let model = crate::llm_provider::CannedLlmProvider::new(
        r#"{"text":"Importer verification is ready","details":"Parser done; run the focused import tests next."}"#,
    );

    assert_eq!(
        db.fill_idle_handoffs(
            &[conversation],
            &not_shared,
            Some(&model),
            "English",
            handed_at + 3001,
            3000,
        )
        .await
        .unwrap(),
        1
    );
    let item = &db
        .get_brief_by_space_name("model-work")
        .await
        .unwrap()
        .unwrap()
        .active[0];
    assert_eq!(item.text, "Importer verification is ready");
    assert_eq!(
        item.details.as_deref(),
        Some("Parser done; run the focused import tests next.")
    );
    assert!(!item.text.contains(raw));
}

#[tokio::test]
async fn idle_fill_asks_the_model_for_the_ui_language() {
    let (db, _dir) = test_db().await;
    let not_shared = |_: &str| Ok(false);
    let session = SessionRef::parse("claude-code:lang-fill").unwrap();
    db.apply_brief_update_in_session(
        &update_request(
            "lang-work",
            "lang-handoff",
            vec![BriefMutation::Add {
                text: "Importer".into(),
                details: None,
                state: BriefItemState::Active,
                added_at: None,
                gate: None,
            }],
        ),
        Some(&session),
    )
    .await
    .unwrap();
    let handed_at = db.list_brief_history("lang-work", 1).await.unwrap()[0].created_at;
    let conversation = FoundConversation {
        tool: "claude-code".into(),
        external_id: "lang-fill".into(),
        path: PathBuf::from("/tmp/lang-fill.jsonl"),
        len: 10,
        cwd: None,
        branch: None,
        said: vec!["please continue".into()],
        last_reply: "the import tests pass now".into(),
        last_content: "the import tests pass now".into(),
        first_at: handed_at - 60,
        last_content_at: handed_at + 1,
        last_at: handed_at + 1,
        automated: false,
        on_behalf: false,
        touched: vec![],
    };
    // The model only gives a usable answer when the system prompt names the
    // UI language; otherwise the fill falls back and keeps the old line.
    let model = crate::llm_provider::CannedLlmProvider::new("not json").with(
        "in Traditional Chinese",
        r#"{"text":"匯入：測試已通過","details":"下一步：合併前再跑一次。"}"#,
    );

    assert_eq!(
        db.fill_idle_handoffs(
            &[conversation],
            &not_shared,
            Some(&model),
            "Traditional Chinese",
            handed_at + 3001,
            3000,
        )
        .await
        .unwrap(),
        1
    );
    let item = &db
        .get_brief_by_space_name("lang-work")
        .await
        .unwrap()
        .unwrap()
        .active[0];
    assert_eq!(item.text, "匯入：測試已通過");
    assert!(item.filled_by_wenlan);
}

#[tokio::test]
async fn idle_fill_rejects_model_output_that_echoes_the_transcript() {
    let (db, _temp) = test_db().await;
    let session = SessionRef::parse("claude-code:echo-fill").unwrap();
    db.apply_brief_update_in_session(
        &update_request(
            "echo-work",
            "echo-handoff",
            vec![add("Keep the safe line", BriefItemState::Active)],
        ),
        Some(&session),
    )
    .await
    .unwrap();
    let handed_at = db.list_brief_history("echo-work", 1).await.unwrap()[0].created_at;
    let raw = "This private transcript sentence is deliberately much longer than forty characters and must never become the Brief line.";
    let conversation = FoundConversation {
        tool: "claude-code".into(),
        external_id: "echo-fill".into(),
        path: PathBuf::from("/tmp/echo-fill.jsonl"),
        len: 10,
        cwd: None,
        branch: None,
        said: vec!["continue".into()],
        last_reply: raw.into(),
        last_content: raw.into(),
        first_at: handed_at - 60,
        last_content_at: handed_at + 1,
        last_at: handed_at + 1,
        automated: false,
        on_behalf: false,
        touched: vec![],
    };
    let answer = serde_json::json!({"text": raw, "details": "Next: keep going."}).to_string();
    let model = crate::llm_provider::CannedLlmProvider::new(&answer);

    assert_eq!(
        db.fill_idle_handoffs(
            &[conversation],
            &not_shared,
            Some(&model),
            "English",
            handed_at + 3001,
            3000,
        )
        .await
        .unwrap(),
        1
    );
    let item = &db
        .get_brief_by_space_name("echo-work")
        .await
        .unwrap()
        .unwrap()
        .active[0];
    assert_eq!(item.text, "Keep the safe line");
    assert_eq!(item.details.as_deref(), Some(raw));
}

#[tokio::test]
async fn idle_fill_rechecks_when_space_becomes_shared_during_generation() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let (db, _temp) = test_db().await;
    let session = SessionRef::parse("claude-code:shared-during-fill").unwrap();
    db.apply_brief_update_in_session(
        &update_request(
            "becomes-shared",
            "shared-handoff",
            vec![add("Keep this private", BriefItemState::Active)],
        ),
        Some(&session),
    )
    .await
    .unwrap();
    let handed_at = db.list_brief_history("becomes-shared", 1).await.unwrap()[0].created_at;
    let conversation = FoundConversation {
        tool: "claude-code".into(),
        external_id: "shared-during-fill".into(),
        path: PathBuf::from("/tmp/shared-during-fill.jsonl"),
        len: 10,
        cwd: None,
        branch: None,
        said: vec!["continue".into()],
        last_reply: "Private content after the handoff.".into(),
        last_content: "Private content after the handoff.".into(),
        first_at: handed_at - 60,
        last_content_at: handed_at + 1,
        last_at: handed_at + 1,
        automated: false,
        on_behalf: false,
        touched: vec![],
    };
    let model = crate::llm_provider::CannedLlmProvider::new(
        r#"{"text":"Generated line","details":"Generated details"}"#,
    );
    let checks = AtomicUsize::new(0);
    let shared = |_: &str| Ok(checks.fetch_add(1, Ordering::SeqCst) > 0);

    assert_eq!(
        db.fill_idle_handoffs(
            &[conversation],
            &shared,
            Some(&model),
            "English",
            handed_at + 3001,
            3000,
        )
        .await
        .unwrap(),
        0
    );
    assert!(checks.load(Ordering::SeqCst) >= 2);
    let item = &db
        .get_brief_by_space_name("becomes-shared")
        .await
        .unwrap()
        .unwrap()
        .active[0];
    assert_eq!(item.text, "Keep this private");
    assert_eq!(item.details, None);
}

#[tokio::test]
async fn idle_fill_skips_item_changed_after_sessions_last_handoff() {
    let (db, _temp) = test_db().await;
    let session_a = SessionRef::parse("claude-code:session-a").unwrap();
    let session_b = SessionRef::parse("claude-code:session-b").unwrap();
    let created = db
        .apply_brief_update_in_session(
            &update_request(
                "work",
                "session-a-handoff",
                vec![add("A started the work", BriefItemState::Active)],
            ),
            Some(&session_a),
        )
        .await
        .unwrap();
    let item_id = created.applied[0].item_id.clone().unwrap();
    let version = created.applied[0].version.unwrap();
    db.apply_brief_update_in_session(
        &update_request(
            "work",
            "session-b-handoff",
            vec![BriefMutation::Edit {
                item_id,
                expected_version: version,
                text: "B finished the newer work".into(),
                details: None,
            }],
        ),
        Some(&session_b),
    )
    .await
    .unwrap();

    let handed_at = chrono::Utc::now().timestamp();
    let conversation = FoundConversation {
        tool: "claude-code".into(),
        external_id: "session-a".into(),
        path: PathBuf::from("/tmp/session-a.jsonl"),
        len: 10,
        cwd: None,
        branch: None,
        said: vec!["continue".into()],
        last_reply: "A private later reply".into(),
        last_content: "A private later reply".into(),
        first_at: handed_at - 60,
        last_content_at: handed_at + 1,
        last_at: handed_at + 1,
        automated: false,
        on_behalf: false,
        touched: vec![],
    };

    assert_eq!(
        db.fill_idle_handoffs(
            &[conversation],
            &not_shared,
            None,
            "English",
            handed_at + 3001,
            3000,
        )
        .await
        .unwrap(),
        0
    );
    let brief = db.get_brief_by_space_name("work").await.unwrap().unwrap();
    assert_eq!(brief.active[0].text, "B finished the newer work");
    assert!(!brief.active[0].filled_by_wenlan);
}

#[tokio::test]
async fn idle_fill_skips_when_only_non_content_events_are_newer_than_handoff() {
    let (db, _temp) = test_db().await;
    let session = SessionRef::parse("claude-code:metadata-after-handoff").unwrap();
    db.apply_brief_update_in_session(
        &update_request(
            "metadata-work",
            "confirmed-handoff",
            vec![add("Confirmed after the reply", BriefItemState::Active)],
        ),
        Some(&session),
    )
    .await
    .unwrap();
    let handed_at = db.list_brief_history("metadata-work", 1).await.unwrap()[0].created_at;
    let conversation = FoundConversation {
        tool: "claude-code".into(),
        external_id: "metadata-after-handoff".into(),
        path: PathBuf::from("/tmp/metadata-after-handoff.jsonl"),
        len: 10,
        cwd: None,
        branch: None,
        said: vec!["old person message".into()],
        last_reply: "old AI reply".into(),
        last_content: "old AI reply".into(),
        first_at: handed_at - 60,
        last_content_at: handed_at,
        last_at: handed_at + 1,
        automated: false,
        on_behalf: false,
        touched: vec![],
    };

    assert_eq!(
        db.fill_idle_handoffs(
            &[conversation],
            &not_shared,
            None,
            "English",
            handed_at + 3002,
            3000,
        )
        .await
        .unwrap(),
        0
    );
    let brief = db
        .get_brief_by_space_name("metadata-work")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(brief.active[0].text, "Confirmed after the reply");
    assert!(brief.active[0].confirmed);
    assert!(!brief.active[0].filled_by_wenlan);
}

#[tokio::test]
async fn ux_hand_8_idle_fill_skips_team_space() {
    let (db, _temp) = test_db().await;
    let session = SessionRef::parse("claude-code:team-session").unwrap();
    db.apply_brief_update_in_session(
        &update_request(
            "team-work",
            "team-handoff",
            vec![add("Shared status", BriefItemState::Active)],
        ),
        Some(&session),
    )
    .await
    .unwrap();

    let handed_at = chrono::Utc::now().timestamp();
    let conversation = FoundConversation {
        tool: "claude-code".into(),
        external_id: "team-session".into(),
        path: PathBuf::from("/tmp/team-session.jsonl"),
        len: 10,
        cwd: None,
        branch: None,
        said: vec!["continue".into()],
        last_reply: "PRIVATE TEAM CONVERSATION TEXT".into(),
        last_content: "PRIVATE TEAM CONVERSATION TEXT".into(),
        first_at: handed_at - 60,
        last_content_at: handed_at + 1,
        last_at: handed_at + 1,
        automated: false,
        on_behalf: false,
        touched: vec![],
    };
    let shared = |space: &str| Ok(space == "team-work");

    assert_eq!(
        db.fill_idle_handoffs(
            &[conversation],
            &shared,
            None,
            "English",
            handed_at + 3001,
            3000
        )
        .await
        .unwrap(),
        0
    );
    let brief = db
        .get_brief_by_space_name("team-work")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(brief.active[0].text, "Shared status");
    assert!(!brief.active[0]
        .text
        .contains("PRIVATE TEAM CONVERSATION TEXT"));
    assert!(!brief.active[0].filled_by_wenlan);
}

#[tokio::test]
async fn ux_hand_7_handoff_writes_no_memory_rows() {
    let (db, _temp) = test_db().await;
    let before = {
        let conn = db.conn.lock().await;
        let mut rows = conn
            .query("SELECT COUNT(*) FROM memories", ())
            .await
            .unwrap();
        rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap()
    };
    db.apply_brief_update(&update_request(
        "work",
        "brief-only",
        vec![add("Brief only", BriefItemState::Active)],
    ))
    .await
    .unwrap();
    let conn = db.conn.lock().await;
    let mut rows = conn
        .query("SELECT COUNT(*) FROM memories", ())
        .await
        .unwrap();
    assert_eq!(
        rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
        before
    );
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
            details: None,
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
