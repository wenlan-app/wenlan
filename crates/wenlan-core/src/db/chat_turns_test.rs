// SPDX-License-Identifier: Apache-2.0
use super::*;
use crate::db::tests::test_db;
use crate::read_scope::ReadScope;

const SPACE: &str = "wenlan";

/// One `memories` row as an earlier import (or a person) left it: no
/// `semantic_unit`, a machine-assigned type, unconfirmed.
async fn seed_memory(
    db: &MemoryDB,
    id: &str,
    source_id: &str,
    chunk_index: i64,
    content: &str,
    session_id: Option<&str>,
    memory_type: Option<&str>,
) {
    db.test_primary_session()
        .await
        .execute(
            "INSERT INTO memories (id, content, source, source_id, title, chunk_index,
                                   last_modified, chunk_type, space, session_id,
                                   memory_type, confirmed)
             VALUES (?1, ?2, 'memory', ?3, ?3, ?4, 100, 'text', ?5, ?6, ?7, 0)",
            libsql::params![
                id,
                content,
                source_id,
                chunk_index,
                SPACE,
                session_id,
                memory_type
            ],
        )
        .await
        .unwrap();
}

async fn unit_and_type(db: &MemoryDB, id: &str) -> (Option<String>, Option<String>) {
    let conn = db.test_primary_session().await;
    let mut rows = conn
        .query(
            "SELECT semantic_unit, memory_type FROM memories WHERE id = ?1",
            libsql::params![id],
        )
        .await
        .unwrap();
    let row = rows.next().await.unwrap().unwrap();
    (
        row.get::<Option<String>>(0).unwrap(),
        row.get::<Option<String>>(1).unwrap(),
    )
}

async fn scalar(db: &MemoryDB, sql: &str) -> i64 {
    let conn = db.test_primary_session().await;
    let mut rows = conn.query(sql, ()).await.unwrap();
    rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap()
}

async fn text(db: &MemoryDB, sql: &str) -> Option<String> {
    let conn = db.test_primary_session().await;
    let mut rows = conn.query(sql, ()).await.unwrap();
    rows.next()
        .await
        .unwrap()
        .and_then(|row| row.get::<Option<String>>(0).unwrap())
}

/// Two earlier imported conversations (three turns), plus the rows that
/// look close but are not imported turns.
async fn seed_earlier_import(db: &MemoryDB) {
    seed_memory(
        db,
        "t1",
        "import_claude_c1",
        0,
        "User: I prefer tea\n\nAssistant: Noted.",
        Some("sess-c1"),
        Some("preference"),
    )
    .await;
    seed_memory(
        db,
        "t2",
        "import_claude_c1",
        1,
        "User: I live in Taipei\n\nAssistant: Nice.",
        Some("sess-c1"),
        Some("identity"),
    )
    .await;
    seed_memory(
        db,
        "t3",
        "import_chatgpt_g1",
        0,
        "User: we picked Rust\n\nAssistant: Good call.",
        Some("sess-g1"),
        Some("decision"),
    )
    .await;
    // Negative controls: an import row with no session, a non-vendor import
    // id, a person's own note that happens to start with `User: `, and a
    // vendor import row that is not a `User:` turn.
    seed_memory(
        db,
        "n1",
        "import_claude_nosession",
        0,
        "User: no session here",
        None,
        Some("fact"),
    )
    .await;
    seed_memory(
        db,
        "n2",
        "import_abc_0",
        0,
        "User: other importer",
        Some("sess-x"),
        Some("fact"),
    )
    .await;
    seed_memory(
        db,
        "n3",
        "mem_note",
        0,
        "User: research says users want speed",
        Some("sess-y"),
        Some("fact"),
    )
    .await;
    seed_memory(
        db,
        "n4",
        "import_claude_c2",
        0,
        "Summary of the conversation",
        Some("sess-c2"),
        Some("fact"),
    )
    .await;
}

#[tokio::test]
async fn ux_add_4_flag_pass_marks_only_imported_turns_and_keeps_their_type() {
    let (db, _dir) = test_db().await;
    seed_earlier_import(&db).await;
    // A turn-shaped row that already carries another unit is left alone:
    // flagging it would lose that value, so the flag could not be undone.
    seed_memory(
        &db,
        "u1",
        "import_claude_c9",
        0,
        "User: cut elsewhere\n\nAssistant: ok",
        Some("sess-c9"),
        Some("fact"),
    )
    .await;
    db.test_primary_session()
        .await
        .execute(
            "UPDATE memories SET semantic_unit = 'paragraph' WHERE id = 'u1'",
            (),
        )
        .await
        .unwrap();

    let dry = db.flag_imported_chat_turns(true).await.unwrap();
    assert_eq!(
        dry,
        ChatTurnFlagOutcome {
            already_ran: false,
            dry_run: true,
            matched: 3,
            skipped_other_unit: 1,
            flagged: 0,
            conversations: 2,
            typed_fact: 0,
            typed_preference: 1,
            typed_identity: 1,
            typed_decision: 1,
            typed_other: 0,
        }
    );
    // A dry run writes nothing, not even the marker.
    assert_eq!(
        unit_and_type(&db, "t1").await,
        (None, Some("preference".into()))
    );
    assert_eq!(
        scalar(
            &db,
            "SELECT COUNT(*) FROM app_metadata WHERE key = 'chat_turns_flagged_v1'"
        )
        .await,
        0
    );

    let run = db.flag_imported_chat_turns(false).await.unwrap();
    assert_eq!(run.flagged, 3);
    assert_eq!(run.matched, 3);
    assert!(!run.already_ran);
    // The flag is the only change: each turn keeps the type it carried.
    for (id, kept) in [("t1", "preference"), ("t2", "identity"), ("t3", "decision")] {
        assert_eq!(
            unit_and_type(&db, id).await,
            (Some(CHAT_TURN_UNIT.into()), Some(kept.into())),
            "{id} should be flagged and keep its type"
        );
    }
    assert_eq!(
        unit_and_type(&db, "u1").await,
        (Some("paragraph".into()), Some("fact".into()))
    );
    for id in ["n1", "n2", "n3", "n4"] {
        assert_eq!(
            unit_and_type(&db, id).await,
            (None, Some("fact".into())),
            "{id} is not an imported turn"
        );
    }
    let marker = text(
        &db,
        "SELECT value FROM app_metadata WHERE key = 'chat_turns_flagged_v1'",
    )
    .await
    .unwrap();
    assert!(marker.contains("\"matched\":3"), "{marker}");
    // Content stays the conversation's original.
    assert_eq!(
        text(&db, "SELECT content FROM memories WHERE id = 't1'")
            .await
            .as_deref(),
        Some("User: I prefer tea\n\nAssistant: Noted.")
    );
}

#[tokio::test]
async fn ux_add_4_flag_pass_runs_once_behind_its_marker() {
    let (db, _dir) = test_db().await;
    seed_earlier_import(&db).await;
    assert_eq!(db.flag_imported_chat_turns(false).await.unwrap().flagged, 3);

    // A row written later in the old shape is left to the insert path; the
    // marker keeps the pass from running a second time.
    seed_memory(
        &db,
        "t4",
        "import_claude_c3",
        0,
        "User: later\n\nAssistant: ok",
        Some("sess-c3"),
        Some("fact"),
    )
    .await;
    let again = db.flag_imported_chat_turns(false).await.unwrap();
    assert!(again.already_ran);
    assert_eq!(again.flagged, 0);
    assert_eq!(unit_and_type(&db, "t4").await, (None, Some("fact".into())));

    // A dry run still reports what is unflagged, and still writes nothing.
    let dry = db.flag_imported_chat_turns(true).await.unwrap();
    assert!(dry.already_ran && dry.dry_run);
    assert_eq!(dry.matched, 1);
    assert_eq!(unit_and_type(&db, "t4").await, (None, Some("fact".into())));
}

#[tokio::test]
async fn ux_add_4_report_lists_turns_and_what_rests_on_them_without_unlinking() {
    let (db, _dir) = test_db().await;
    seed_earlier_import(&db).await;
    db.insert_page(
        "page-tea",
        "Tea habits",
        None,
        "Prefers tea.",
        None,
        Some(SPACE),
        &["import_claude_c1", "mem_note"],
        "2026-11-08T00:00:00Z",
    )
    .await
    .unwrap();
    db.insert_page(
        "page-topic",
        "Rust",
        None,
        "The Rust topic.",
        None,
        Some(SPACE),
        &[],
        "2026-11-08T00:00:00Z",
    )
    .await
    .unwrap();
    db.test_primary_session()
        .await
        .execute_batch(
            "INSERT OR IGNORE INTO entity_page_map (entity_id, page_id, created_at)
                 VALUES ('ent-rust', 'page-topic', '2026-11-08T00:00:00Z');
             INSERT INTO memory_entities (memory_id, entity_id)
                 VALUES ('import_chatgpt_g1', 'ent-rust'), ('mem_note', 'ent-rust');
             INSERT INTO page_sources (page_id, memory_source_id, linked_at)
                 VALUES ('page-tea', 'import_claude_c1', 1);",
        )
        .await
        .unwrap();
    let before_links = scalar(
        &db,
        "SELECT (SELECT COUNT(*) FROM memory_entities) * 1000
              + (SELECT COUNT(*) FROM page_sources) * 100
              + (SELECT COUNT(*) FROM edges WHERE edge_type = 'cites' AND valid_until IS NULL)",
    )
    .await;
    let before_content = text(&db, "SELECT content FROM pages WHERE id = 'page-tea'").await;

    // Nothing is flagged yet, so nothing is reported.
    assert_eq!(db.chat_turn_report().await.unwrap().turns, 0);
    db.flag_imported_chat_turns(false).await.unwrap();
    let report = db.chat_turn_report().await.unwrap();

    assert_eq!(report.turns, 3);
    assert_eq!(report.conversations, 2);
    assert_eq!(
        report
            .rows
            .iter()
            .map(|row| (row.source_id.as_str(), row.chunk_index))
            .collect::<Vec<_>>(),
        vec![
            ("import_chatgpt_g1", 0),
            ("import_claude_c1", 0),
            ("import_claude_c1", 1)
        ]
    );
    assert_eq!(report.page_citations, 1, "{report:?}");
    assert_eq!(
        report.pages,
        vec![ChatTurnPage {
            page_id: "page-tea".into(),
            title: "Tea habits".into(),
            status: report.pages[0].status.clone(),
            conversations: 1,
        }]
    );
    assert_eq!(report.legacy_page_source_links, 1);
    assert_eq!(report.topic_links, 1);
    assert_eq!(
        report.topics,
        vec![ChatTurnTopic {
            entity_id: "ent-rust".into(),
            name: Some("Rust".into()),
            conversations: 1,
        }]
    );

    // Flagging removed no citation, topic link or page text.
    let after_links = scalar(
        &db,
        "SELECT (SELECT COUNT(*) FROM memory_entities) * 1000
              + (SELECT COUNT(*) FROM page_sources) * 100
              + (SELECT COUNT(*) FROM edges WHERE edge_type = 'cites' AND valid_until IS NULL)",
    )
    .await;
    assert_eq!(before_links, after_links);
    assert_eq!(
        before_content,
        text(&db, "SELECT content FROM pages WHERE id = 'page-tea'").await
    );
}

#[tokio::test]
async fn ux_add_4_new_import_stores_turns_flagged_as_conversation_text() {
    let (db, _dir) = test_db().await;
    let entries = vec![
        (
            "import_claude_new1".to_string(),
            "User: I like hiking\n\nAssistant: Great.".to_string(),
            Some("Hiking".to_string()),
            None,
            0,
        ),
        (
            "import_claude_new1".to_string(),
            "Conversation summary line".to_string(),
            Some("Hiking".to_string()),
            None,
            1,
        ),
        (
            "import_abc_7".to_string(),
            "User: not a vendor import".to_string(),
            None,
            None,
            0,
        ),
    ];
    assert_eq!(
        db.store_raw_import_memories_batch(&entries).await.unwrap(),
        3
    );
    assert_eq!(
        text(
            &db,
            "SELECT semantic_unit FROM memories
              WHERE source_id = 'import_claude_new1' AND chunk_index = 0"
        )
        .await
        .as_deref(),
        Some(CHAT_TURN_UNIT)
    );
    assert_eq!(
        text(
            &db,
            "SELECT semantic_unit FROM memories
              WHERE source_id = 'import_claude_new1' AND chunk_index = 1"
        )
        .await,
        None
    );
    assert_eq!(
        text(
            &db,
            "SELECT semantic_unit FROM memories WHERE source_id = 'import_abc_7'"
        )
        .await,
        None
    );
    // The turn keeps its session and stays readable as the original.
    assert_eq!(
        scalar(
            &db,
            "SELECT COUNT(*) FROM memories
              WHERE source_id = 'import_claude_new1' AND session_id IS NOT NULL"
        )
        .await,
        2
    );
    // No lane will type it: it is never a classification candidate.
    let candidate = db.get_classification_candidate(3).await.unwrap();
    assert_ne!(
        candidate.map(|input| input.source_id).as_deref(),
        Some("import_claude_new1")
    );
}

#[tokio::test]
async fn ux_add_4_a_flagged_turn_is_never_classified_as_the_persons_memory() {
    let (db, _dir) = test_db().await;
    seed_memory(
        &db,
        "t1",
        "import_claude_c1",
        0,
        "User: I prefer tea\n\nAssistant: Noted.",
        Some("sess-c1"),
        None,
    )
    .await;
    // Before the flag, the turn is exactly what the lane would type.
    assert_eq!(
        db.get_classification_candidate(3)
            .await
            .unwrap()
            .map(|input| input.source_id)
            .as_deref(),
        Some("import_claude_c1")
    );
    db.flag_imported_chat_turns(false).await.unwrap();
    assert!(db.get_classification_candidate(3).await.unwrap().is_none());
    assert!(db
        .get_classification_candidate_for("import_claude_c1", 3)
        .await
        .unwrap()
        .is_none());

    // The person's own memories are still classified.
    seed_memory(&db, "own", "mem_own", 0, "We picked Rust.", None, None).await;
    assert_eq!(
        db.get_classification_candidate(3)
            .await
            .unwrap()
            .map(|input| input.source_id)
            .as_deref(),
        Some("mem_own")
    );
}

#[tokio::test]
async fn ux_add_4_a_flagged_turn_is_not_waiting_for_the_person_to_confirm() {
    let (db, _dir) = test_db().await;
    seed_earlier_import(&db).await;
    let queue = |items: Vec<wenlan_types::RecentActivityItem>| {
        items.into_iter().map(|item| item.id).collect::<Vec<_>>()
    };
    let unconfirmed = |items: Vec<wenlan_types::IndexedFileInfo>| {
        items
            .into_iter()
            .map(|item| item.source_id)
            .collect::<Vec<_>>()
    };

    let before = queue(db.list_unconfirmed_memories(50).await.unwrap());
    assert!(
        before.contains(&"import_claude_c1".to_string()),
        "{before:?}"
    );
    let before_list = unconfirmed(
        db.list_filtered_confirmed_scoped(None, None, &ReadScope::Global, Some(false), 50)
            .await
            .unwrap(),
    );
    assert!(before_list.contains(&"import_claude_c1".to_string()));

    db.flag_imported_chat_turns(false).await.unwrap();

    let after = queue(db.list_unconfirmed_memories(50).await.unwrap());
    for turn in ["import_claude_c1", "import_chatgpt_g1"] {
        assert!(!after.contains(&turn.to_string()), "{after:?}");
    }
    assert!(after.contains(&"mem_note".to_string()), "{after:?}");
    let after_list = unconfirmed(
        db.list_filtered_confirmed_scoped(None, None, &ReadScope::Global, Some(false), 50)
            .await
            .unwrap(),
    );
    assert!(!after_list.contains(&"import_claude_c1".to_string()));
    assert!(after_list.contains(&"mem_note".to_string()));
}

#[tokio::test]
async fn ux_add_4_memory_detail_says_whether_a_row_is_conversation_text() {
    let (db, _dir) = test_db().await;
    seed_earlier_import(&db).await;
    db.flag_imported_chat_turns(false).await.unwrap();
    let turn = db
        .get_memory_detail("import_claude_c1")
        .await
        .unwrap()
        .unwrap();
    assert!(turn.conversation_text);
    // The stored type is still there; the flag is what readers go by.
    assert!(turn.memory_type.is_some());
    let own = db.get_memory_detail("mem_note").await.unwrap().unwrap();
    assert!(!own.conversation_text);
    assert_eq!(own.memory_type.as_deref(), Some("fact"));
    // The wire shape leaves the flag out when it is false.
    let json = serde_json::to_value(&own).unwrap();
    assert!(json.get("conversation_text").is_none());
    let json = serde_json::to_value(&turn).unwrap();
    assert_eq!(json["conversation_text"], serde_json::json!(true));
}

/// Every column of every `memories` row, quoted, in a stable order.
async fn all_memory_rows(db: &MemoryDB) -> Vec<String> {
    let conn = db.test_primary_session().await;
    let mut columns = Vec::new();
    let mut rows = conn
        .query("SELECT name FROM pragma_table_info('memories')", ())
        .await
        .unwrap();
    while let Some(row) = rows.next().await.unwrap() {
        columns.push(format!("quote({})", row.get::<String>(0).unwrap()));
    }
    drop(rows);
    assert!(columns.len() > 20, "{columns:?}");
    let mut rows = conn
        .query(
            &format!(
                "SELECT {} FROM memories ORDER BY id",
                columns.join(" || '|' || ")
            ),
            (),
        )
        .await
        .unwrap();
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.unwrap() {
        out.push(row.get::<String>(0).unwrap());
    }
    out
}

#[tokio::test]
async fn ux_add_4_unflag_puts_every_row_back_exactly_as_it_was() {
    let (db, _dir) = test_db().await;
    seed_earlier_import(&db).await;
    let before = all_memory_rows(&db).await;

    assert_eq!(db.flag_imported_chat_turns(false).await.unwrap().flagged, 3);
    assert_ne!(all_memory_rows(&db).await, before, "the flag must show");
    assert_eq!(db.unflag_imported_chat_turns().await.unwrap(), 3);
    assert_eq!(all_memory_rows(&db).await, before);
    // The marker stays, so the startup pass does not flag them again.
    assert!(
        db.flag_imported_chat_turns(false)
            .await
            .unwrap()
            .already_ran
    );
    assert_eq!(all_memory_rows(&db).await, before);
}

fn is_not_a_memory_refusal(result: Result<impl std::fmt::Debug, WenlanError>) -> bool {
    matches!(result, Err(WenlanError::Conflict(ref message))
        if message.starts_with(CHAT_TURN_NOT_A_MEMORY)
            && message.contains("not a memory the person stated"))
}

#[tokio::test]
async fn ux_add_4_confirming_or_retyping_a_flagged_turn_is_refused() {
    let (db, _dir) = test_db().await;
    seed_earlier_import(&db).await;
    db.flag_imported_chat_turns(false).await.unwrap();
    let before = all_memory_rows(&db).await;
    let update =
        |confirm: bool, memory_type: Option<&'static str>| crate::post_write::MemoryUpdate {
            content: None,
            expected_content: None,
            space: None,
            confirm,
            memory_type,
        };

    assert!(is_not_a_memory_refusal(
        db.set_stability("import_claude_c1", "confirmed").await
    ));
    assert!(is_not_a_memory_refusal(
        db.confirm_memory("import_claude_c1").await
    ));
    assert!(is_not_a_memory_refusal(
        crate::post_write::update_memory(&db, "import_claude_c1", update(true, None)).await
    ));
    assert!(is_not_a_memory_refusal(
        crate::post_write::update_memory(&db, "import_claude_c1", update(false, Some("fact")))
            .await
    ));
    let receipt = wenlan_types::repair::RepairDigest::parse(&"0".repeat(64)).unwrap();
    assert!(is_not_a_memory_refusal(
        db.reclassify_memory_repair_cas(
            "import_claude_c1",
            &receipt,
            Some(SPACE),
            None,
            wenlan_types::MemoryType::Fact,
            |_| Ok(()),
        )
        .await
    ));
    // Nothing was written by any refused call.
    assert_eq!(all_memory_rows(&db).await, before);

    // Moving a turn back to `new` is not a claim, and the person's own
    // memory is still confirmed and retyped as before.
    assert!(db.set_stability("import_claude_c1", "new").await.unwrap());
    assert!(db.confirm_memory("mem_note").await.unwrap());
    crate::post_write::update_memory(&db, "mem_note", update(false, Some("decision")))
        .await
        .unwrap();
    assert_eq!(
        unit_and_type(&db, "n3").await,
        (None, Some("decision".into()))
    );
}

#[tokio::test]
async fn ux_add_4_readers_that_pick_by_type_leave_flagged_turns_out() {
    let (db, _dir) = test_db().await;
    seed_earlier_import(&db).await;
    seed_memory(
        &db,
        "own",
        "mem_decided",
        0,
        "We picked Go.",
        None,
        Some("decision"),
    )
    .await;
    let scope = ReadScope::Global;

    // What each reader returns for `decision` (or for review), as source ids.
    async fn picked(db: &MemoryDB, scope: &ReadScope) -> Vec<Vec<String>> {
        let listed = db
            .list_memories_scoped(scope, Some("decision"), None, None, 50)
            .await
            .unwrap()
            .into_iter()
            .map(|item| item.source_id)
            .collect();
        let filtered = db
            .list_filtered_confirmed_scoped(None, Some("decision"), scope, None, 50)
            .await
            .unwrap()
            .into_iter()
            .map(|item| item.source_id)
            .collect();
        let recap = db
            .get_recent_decisions_for_recap(0, 50)
            .await
            .unwrap()
            .into_iter()
            .map(|(source_id, ..)| source_id)
            .collect();
        let same_type = db
            .find_same_type_memories("mem_none", "decision", Some(SPACE), 50)
            .await
            .unwrap()
            .into_iter()
            .map(|(source_id, ..)| source_id)
            .collect();
        let nurture = db
            .get_nurture_cards_scoped(50, scope)
            .await
            .unwrap()
            .into_iter()
            .map(|item| item.source_id)
            .collect();
        vec![listed, filtered, recap, same_type, nurture]
    }

    // Before the flag, every reader picks the turn typed `decision`.
    for (reader, ids) in picked(&db, &scope).await.iter().enumerate() {
        assert!(
            ids.contains(&"import_chatgpt_g1".to_string()),
            "reader {reader}: {ids:?}"
        );
        assert!(
            ids.contains(&"mem_decided".to_string()),
            "reader {reader}: {ids:?}"
        );
    }
    db.flag_imported_chat_turns(false).await.unwrap();
    for (reader, ids) in picked(&db, &scope).await.iter().enumerate() {
        assert!(
            !ids.contains(&"import_chatgpt_g1".to_string()),
            "reader {reader}: {ids:?}"
        );
        assert!(
            ids.contains(&"mem_decided".to_string()),
            "reader {reader}: {ids:?}"
        );
    }
}

#[tokio::test]
async fn ux_add_4_a_type_filtered_search_leaves_flagged_turns_out() {
    let (db, _dir) = test_db().await;
    seed_earlier_import(&db).await;
    seed_memory(
        &db,
        "own",
        "mem_decided",
        0,
        "We picked Rust for the daemon.",
        None,
        Some("decision"),
    )
    .await;
    async fn found(db: &MemoryDB, memory_type: Option<&str>) -> Vec<String> {
        db.search_memory(
            "picked Rust",
            20,
            memory_type,
            &ReadScope::Global,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap()
        .into_iter()
        .map(|result| result.source_id)
        .collect()
    }

    // Before the flag, a search for decisions finds the turn typed decision.
    for filter in [Some("decision"), Some("decision,fact")] {
        let ids = found(&db, filter).await;
        assert!(
            ids.contains(&"import_chatgpt_g1".to_string()),
            "{filter:?}: {ids:?}"
        );
        assert!(
            ids.contains(&"mem_decided".to_string()),
            "{filter:?}: {ids:?}"
        );
    }
    db.flag_imported_chat_turns(false).await.unwrap();
    // After it, both the one-type and the several-type filters leave it out,
    // while an unfiltered search still finds the conversation's text.
    for filter in [Some("decision"), Some("decision,fact")] {
        let ids = found(&db, filter).await;
        assert!(
            !ids.contains(&"import_chatgpt_g1".to_string()),
            "{filter:?}: {ids:?}"
        );
        assert!(
            ids.contains(&"mem_decided".to_string()),
            "{filter:?}: {ids:?}"
        );
    }
    let ids = found(&db, None).await;
    assert!(ids.contains(&"import_chatgpt_g1".to_string()), "{ids:?}");
}

#[tokio::test]
async fn ux_add_4_marker_records_how_many_rows_the_pass_flagged() {
    let (db, _dir) = test_db().await;
    seed_earlier_import(&db).await;
    let outcome = db.flag_imported_chat_turns(false).await.unwrap();
    assert_eq!(outcome.flagged, 3);
    let marker = db
        .get_app_metadata(CHAT_TURNS_FLAGGED_MARKER)
        .await
        .unwrap()
        .expect("marker is written");
    let recorded: serde_json::Value = serde_json::from_str(&marker).unwrap();
    assert_eq!(recorded["flagged"], 3, "{marker}");
    assert_eq!(recorded["matched"], 3, "{marker}");
    assert_eq!(recorded["conversations"], 2, "{marker}");
    assert_eq!(recorded["dry_run"], false, "{marker}");
    assert_eq!(recorded["already_ran"], false, "{marker}");
}

#[tokio::test]
async fn ux_add_4_memory_lists_carry_the_conversation_text_flag() {
    let (db, _dir) = test_db().await;
    seed_earlier_import(&db).await;
    let scope = ReadScope::Global;
    async fn flags(db: &MemoryDB, scope: &ReadScope) -> [Vec<(String, bool)>; 2] {
        let mut listed: Vec<(String, bool)> = db
            .list_filtered_confirmed_scoped(Some("memory"), None, scope, None, 50)
            .await
            .unwrap()
            .into_iter()
            .map(|file| (file.source_id, file.conversation_text))
            .collect();
        let mut indexed: Vec<(String, bool)> = db
            .list_indexed_files_scoped(scope)
            .await
            .unwrap()
            .into_iter()
            .map(|file| (file.source_id, file.conversation_text))
            .collect();
        listed.sort();
        indexed.sort();
        [listed, indexed]
    }
    let flagged_sources = ["import_chatgpt_g1", "import_claude_c1"];
    for list in flags(&db, &scope).await {
        assert!(list.iter().all(|(_, flag)| !flag), "{list:?}");
    }
    db.flag_imported_chat_turns(false).await.unwrap();
    for list in flags(&db, &scope).await {
        assert!(!list.is_empty());
        for (source_id, flag) in &list {
            assert_eq!(
                *flag,
                flagged_sources.contains(&source_id.as_str()),
                "{source_id}: {list:?}"
            );
        }
        for source_id in flagged_sources {
            assert!(list.iter().any(|(id, _)| id == source_id), "{list:?}");
        }
    }
}

/// Runs the pass on a scratch copy of a real database:
/// `WENLAN_CHAT_TURNS_DB_DIR=<dir holding origin_memory.db> cargo test -p
/// wenlan-core --lib ux_add_4_pass_on_a_database_copy -- --ignored --nocapture`.
/// It writes to that copy; never point it at a live data root.
#[tokio::test]
#[ignore = "needs WENLAN_CHAT_TURNS_DB_DIR pointing at a scratch copy"]
async fn ux_add_4_pass_on_a_database_copy() {
    let dir = std::env::var("WENLAN_CHAT_TURNS_DB_DIR").expect("WENLAN_CHAT_TURNS_DB_DIR");
    let db = MemoryDB::open_for_repair(std::path::Path::new(&dir))
        .await
        .unwrap();
    let dry = db.flag_imported_chat_turns(true).await.unwrap();
    println!("dry run: {}", serde_json::to_string(&dry).unwrap());
    let run = db.flag_imported_chat_turns(false).await.unwrap();
    println!("real run: {}", serde_json::to_string(&run).unwrap());
    let again = db.flag_imported_chat_turns(false).await.unwrap();
    println!("second run: {}", serde_json::to_string(&again).unwrap());
    let report = db.chat_turn_report().await.unwrap();
    println!(
        "report: turns={} conversations={} page_citations={} pages={} legacy_page_source_links={} topic_links={} topics={}",
        report.turns,
        report.conversations,
        report.page_citations,
        report.pages.len(),
        report.legacy_page_source_links,
        report.topic_links,
        report.topics.len()
    );
    let flagged_typed = scalar(
        &db,
        "SELECT COUNT(*) FROM memories
          WHERE semantic_unit = 'chat_turn' AND memory_type IS NOT NULL",
    )
    .await;
    let queue = db.list_unconfirmed_memories(100_000).await.unwrap();
    let queued_turns = queue
        .iter()
        .filter(|item| {
            item.id.starts_with("import_claude_") || item.id.starts_with("import_chatgpt_")
        })
        .count();
    let nurture_turns = db
        .get_nurture_cards_scoped(100_000, &ReadScope::Global)
        .await
        .unwrap()
        .iter()
        .filter(|item| item.conversation_text || item.source_id.starts_with("import_c"))
        .count();
    println!(
        "after: flagged_keeping_type={flagged_typed} review_queue={} queued_vendor_imports={queued_turns} nurture_vendor_imports={nurture_turns}",
        queue.len()
    );
    let cleared = db.unflag_imported_chat_turns().await.unwrap();
    let still_flagged = scalar(
        &db,
        "SELECT COUNT(*) FROM memories WHERE semantic_unit = 'chat_turn'",
    )
    .await;
    println!("unflag: cleared={cleared} still_flagged={still_flagged}");
}

#[test]
fn ux_add_4_rust_shape_matches_the_sql_shape() {
    assert!(is_imported_chat_turn("import_claude_c1", true, "User: hi"));
    assert!(is_imported_chat_turn("import_chatgpt_g1", true, "User: hi"));
    assert!(!is_imported_chat_turn("import_claude_", true, "User: hi"));
    assert!(!is_imported_chat_turn(
        "import_claude_c1",
        false,
        "User: hi"
    ));
    assert!(!is_imported_chat_turn("import_abc_0", true, "User: hi"));
    assert!(!is_imported_chat_turn("import_claude_c1", true, "user: hi"));
    assert!(!is_imported_chat_turn("import_claude_c1", true, "Summary"));
    assert_eq!(
        not_chat_turn("m"),
        "COALESCE(m.semantic_unit, '') <> 'chat_turn'"
    );
    assert_eq!(
        not_chat_turn(""),
        "COALESCE(semantic_unit, '') <> 'chat_turn'"
    );
}
