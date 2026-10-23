// SPDX-License-Identifier: Apache-2.0

use super::tests::test_db;
use super::{presence_agent_id, AgentPresence, MemoryDB};

async fn row_count(db: &MemoryDB) -> usize {
    db.list_agents().await.unwrap().len()
}

#[test]
fn presence_agent_id_normalises_like_the_write_path() {
    assert_eq!(
        presence_agent_id("Claude Code").as_deref(),
        Some("claude-code")
    );
    assert_eq!(
        presence_agent_id("  CLAUDE_CODE ").as_deref(),
        Some("claude-code")
    );
    assert_eq!(
        presence_agent_id("openai.mcp").as_deref(),
        Some("openai-mcp")
    );
}

#[test]
fn presence_agent_id_rejects_names_that_must_create_nothing() {
    for bad in [
        "",
        "   ",
        "---",
        "unknown",
        "Unknown",
        "wenlan-setup",
        "Wenlan Setup",
        "bad\u{0}name",
        "line\nbreak",
        "tab\tname",
    ] {
        assert_eq!(presence_agent_id(bad), None, "{bad:?} must be rejected");
    }
    let too_long = "a".repeat(65);
    assert_eq!(presence_agent_id(&too_long), None);
    let longest = "a".repeat(64);
    assert_eq!(
        presence_agent_id(&longest).as_deref(),
        Some(longest.as_str())
    );
}

#[tokio::test]
async fn a_read_marks_an_unseen_agent_as_seen_without_write_trust() {
    let (db, _dir) = test_db().await;

    let outcome = db.record_agent_presence("Claude Code").await.unwrap();

    assert_eq!(outcome, AgentPresence::Created);
    let agent = db.get_agent("claude-code").await.unwrap().unwrap();
    assert_eq!(agent.trust_level, "unknown");
    assert!(agent.last_seen_at.is_some(), "a read must mark it seen");
    assert_eq!(agent.memory_count, 0, "a read is not a memory write");
    assert_eq!(agent.updated_at, agent.created_at, "the untouched marker");
    assert!(agent.enabled);
    assert_eq!(agent.display_name.as_deref(), Some("Claude Code"));
    assert_eq!(db.count_agents_with_writes().await.unwrap(), 0);
}

#[tokio::test]
async fn a_read_from_a_known_agent_only_moves_last_seen() {
    let (db, _dir) = test_db().await;
    db.check_agent_for_write("cursor").await.unwrap();
    // Age the row so a fresh last_seen_at is observable.
    {
        let conn = db.conn.lock().await;
        conn.execute(
            "UPDATE agent_connections SET last_seen_at = 100, updated_at = 200 WHERE name = 'cursor'",
            (),
        )
        .await
        .unwrap();
    }

    let outcome = db.record_agent_presence("Cursor").await.unwrap();

    assert_eq!(outcome, AgentPresence::Seen);
    let agent = db.get_agent("cursor").await.unwrap().unwrap();
    assert!(agent.last_seen_at.unwrap() > 100);
    assert_eq!(agent.trust_level, "full");
    assert_eq!(agent.memory_count, 1);
    assert_eq!(agent.updated_at, 200);
    assert_eq!(row_count(&db).await, 1);
}

#[tokio::test]
async fn invalid_names_create_nothing() {
    let (db, _dir) = test_db().await;
    for bad in ["", "   ", "unknown", "Unknown", "wenlan-setup", "a\u{7}b"] {
        let outcome = db.record_agent_presence(bad).await.unwrap();
        assert_eq!(outcome, AgentPresence::Ignored, "{bad:?}");
    }
    db.record_agent_presence(&"x".repeat(65)).await.unwrap();
    assert_eq!(row_count(&db).await, 0);
}

#[tokio::test]
async fn repeated_reads_never_count_as_memory_writes() {
    let (db, _dir) = test_db().await;
    for _ in 0..5 {
        db.record_agent_presence("codex").await.unwrap();
    }
    let agent = db.get_agent("codex").await.unwrap().unwrap();
    assert_eq!(agent.memory_count, 0);
    assert_eq!(row_count(&db).await, 1);
}

// (a) read then first write gives the same trust as write-only today.
#[tokio::test]
async fn read_then_first_write_matches_a_write_only_first_contact() {
    let (db, _dir) = test_db().await;
    let write_only = db.check_agent_for_write("write-only-bot").await.unwrap();

    db.record_agent_presence("read-first-bot").await.unwrap();
    let after_read = db.get_agent("read-first-bot").await.unwrap().unwrap();
    assert_eq!(after_read.trust_level, "unknown");
    let read_first = db.check_agent_for_write("read-first-bot").await.unwrap();

    assert_eq!(write_only, "full");
    assert_eq!(read_first, write_only);
    let agent = db.get_agent("read-first-bot").await.unwrap().unwrap();
    assert_eq!(agent.trust_level, "full");
    assert_eq!(agent.memory_count, 1);
    // The later writes keep the promoted trust.
    assert_eq!(
        db.check_agent_for_write("read-first-bot").await.unwrap(),
        "full"
    );
}

// (b) a human trust edit between the read and the first write blocks promotion.
#[tokio::test]
async fn a_human_trust_edit_between_read_and_write_blocks_promotion() {
    let (db, _dir) = test_db().await;
    db.record_agent_presence("edited-bot").await.unwrap();
    db.update_agent("edited-bot", None, None, None, Some("review"), None)
        .await
        .unwrap();

    let trust = db.check_agent_for_write("edited-bot").await.unwrap();

    assert_eq!(trust, "review");
    let agent = db.get_agent("edited-bot").await.unwrap().unwrap();
    assert_eq!(agent.trust_level, "review");
}

#[tokio::test]
async fn any_human_edit_of_the_row_blocks_promotion() {
    let (db, _dir) = test_db().await;
    db.record_agent_presence("renamed-bot").await.unwrap();
    db.update_agent("renamed-bot", None, Some("my note"), None, None, None)
        .await
        .unwrap();

    let trust = db.check_agent_for_write("renamed-bot").await.unwrap();

    assert_eq!(trust, "unknown");
}

// (c) a row the user explicitly set to unknown is never promoted, even when the
// edit lands in the same second the row was created.
#[tokio::test]
async fn a_row_the_user_set_to_unknown_is_never_promoted() {
    let (db, _dir) = test_db().await;
    db.record_agent_presence("muted-bot").await.unwrap();
    db.update_agent("muted-bot", None, None, None, Some("unknown"), None)
        .await
        .unwrap();
    let edited = db.get_agent("muted-bot").await.unwrap().unwrap();
    assert!(
        edited.updated_at > edited.created_at,
        "an edit must always move updated_at past created_at"
    );

    for _ in 0..3 {
        let trust = db.check_agent_for_write("muted-bot").await.unwrap();
        assert_eq!(trust, "unknown");
    }
    let agent = db.get_agent("muted-bot").await.unwrap().unwrap();
    assert_eq!(agent.trust_level, "unknown");
    assert_eq!(agent.memory_count, 3);
}

#[tokio::test]
async fn a_row_set_to_unknown_after_a_write_is_never_promoted() {
    let (db, _dir) = test_db().await;
    db.check_agent_for_write("later-muted-bot").await.unwrap();
    db.update_agent("later-muted-bot", None, None, None, Some("unknown"), None)
        .await
        .unwrap();

    let trust = db.check_agent_for_write("later-muted-bot").await.unwrap();

    assert_eq!(trust, "unknown");
}

// (d) concurrent first writes are safe: every writer reports "full", exactly
// one row exists, and every write is counted.
async fn concurrent_first_writes(db: std::sync::Arc<MemoryDB>, name: &'static str, writers: usize) {
    let mut tasks = Vec::new();
    for _ in 0..writers {
        let db = db.clone();
        tasks.push(tokio::spawn(
            async move { db.check_agent_for_write(name).await },
        ));
    }
    for task in tasks {
        assert_eq!(task.await.unwrap().unwrap(), "full");
    }
    let agent = db.get_agent(name).await.unwrap().unwrap();
    assert_eq!(agent.trust_level, "full");
    assert_eq!(agent.memory_count, writers as i64);
    assert_eq!(row_count(&db).await, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_first_writes_on_a_read_created_row_all_get_full() {
    let (db, _dir) = test_db().await;
    let db = std::sync::Arc::new(db);
    db.record_agent_presence("racing-bot").await.unwrap();
    assert_eq!(
        db.get_agent("racing-bot")
            .await
            .unwrap()
            .unwrap()
            .trust_level,
        "unknown"
    );

    concurrent_first_writes(db, "racing-bot", 8).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_first_writes_without_a_read_row_all_get_full() {
    let (db, _dir) = test_db().await;

    concurrent_first_writes(std::sync::Arc::new(db), "cold-racing-bot", 8).await;
}

#[tokio::test]
async fn a_concurrent_read_and_first_write_agree_on_one_row() {
    let (db, _dir) = test_db().await;

    let (read, write) = tokio::join!(
        db.record_agent_presence("mixed-bot"),
        db.check_agent_for_write("mixed-bot")
    );

    read.unwrap();
    assert_eq!(write.unwrap(), "full");
    assert_eq!(row_count(&db).await, 1);
    let agent = db.get_agent("mixed-bot").await.unwrap().unwrap();
    assert_eq!(agent.trust_level, "full");
    assert_eq!(agent.memory_count, 1);
}

#[tokio::test]
async fn a_disabled_read_created_agent_still_cannot_write() {
    let (db, _dir) = test_db().await;
    db.record_agent_presence("off-bot").await.unwrap();
    db.update_agent("off-bot", None, None, Some(false), None, None)
        .await
        .unwrap();

    let result = db.check_agent_for_write("off-bot").await;

    assert!(matches!(
        result,
        Err(crate::error::WenlanError::AgentDisabled(_))
    ));
}

#[tokio::test]
async fn read_created_rows_are_capped() {
    let (db, _dir) = test_db().await;
    for i in 0..64 {
        let outcome = db.record_agent_presence(&format!("bot-{i}")).await.unwrap();
        assert_eq!(outcome, AgentPresence::Created);
    }

    let over = db.record_agent_presence("bot-over-the-cap").await.unwrap();

    assert_eq!(over, AgentPresence::AtCapacity);
    assert!(db.get_agent("bot-over-the-cap").await.unwrap().is_none());
    assert_eq!(row_count(&db).await, 64);
    // An existing row is still marked seen at the cap.
    assert_eq!(
        db.record_agent_presence("bot-0").await.unwrap(),
        AgentPresence::Seen
    );
    // A write promotes a row out of the untouched set and frees its slot.
    db.check_agent_for_write("bot-1").await.unwrap();
    assert_eq!(
        db.record_agent_presence("bot-over-the-cap").await.unwrap(),
        AgentPresence::Created
    );
}

// The promotion statement checks the trust level itself, not only the
// untouched marker: a row that is not "unknown" keeps its trust even when it
// has never been written to or edited (the caller's own gate must not be the
// only thing standing between the two).
#[tokio::test]
async fn promotion_leaves_a_non_unknown_trust_alone_even_when_untouched() {
    let (db, _dir) = test_db().await;
    db.register_agent("review-bot").await.unwrap();
    {
        // Set the trust underneath the API so `updated_at` still equals
        // `created_at`: the only row shape the marker alone would let through.
        let conn = db.conn.lock().await;
        conn.execute(
            "UPDATE agent_connections SET trust_level = 'review' WHERE name = 'review-bot'",
            (),
        )
        .await
        .unwrap();
    }
    let before = db.get_agent("review-bot").await.unwrap().unwrap();
    assert_eq!(before.memory_count, 0);
    assert_eq!(before.updated_at, before.created_at);

    let trust = db.promote_untouched_read_agent("review-bot").await.unwrap();

    assert_eq!(trust, "review");
    let after = db.get_agent("review-bot").await.unwrap().unwrap();
    assert_eq!(after.trust_level, "review");
}

// The write path and the read path create the same row for the same caller.
// The write path used to store the untrimmed label (" Cursor ") while the read
// path stored the trimmed one ("Cursor"), so which route met an agent first
// decided what the Connections list called it.
#[tokio::test]
async fn a_padded_label_gives_the_same_display_name_through_a_read_and_a_write() {
    for (padded, expected) in [
        (" Cursor ", "Cursor"),
        ("\tClaude Code\n", "Claude Code"),
        // Already canonical once trimmed: the known client's friendly name.
        ("  cursor  ", "Cursor"),
    ] {
        let (read_db, _read_dir) = test_db().await;
        let (write_db, _write_dir) = test_db().await;
        let canonical = crate::db::canonicalize_agent_id(padded);

        read_db.record_agent_presence(padded).await.unwrap();
        write_db.register_agent(padded).await.unwrap();

        let from_read = read_db.get_agent(&canonical).await.unwrap().unwrap();
        let from_write = write_db.get_agent(&canonical).await.unwrap().unwrap();
        assert_eq!(
            from_read.display_name.as_deref(),
            Some(expected),
            "read path, {padded:?}"
        );
        assert_eq!(
            from_write.display_name, from_read.display_name,
            "write path must match the read path for {padded:?}"
        );
    }
}

// Connecting a tool is the user's authorisation: a name nobody has seen is
// registered at "full", but it has not been *used*, so `last_seen_at` stays
// empty (the app counts a tool as connected only once it has been seen).
#[tokio::test]
async fn registering_a_connected_tool_creates_it_at_full_without_a_sighting() {
    let (db, _dir) = test_db().await;

    let agent = db.register_trusted_agent("Claude Code").await.unwrap();

    assert_eq!(agent.name, "claude-code");
    assert_eq!(agent.trust_level, "full");
    assert!(agent.enabled);
    assert_eq!(agent.last_seen_at, None);
    assert_eq!(agent.memory_count, 0);
    assert_eq!(row_count(&db).await, 1);
}

#[tokio::test]
async fn registering_a_connected_tool_promotes_an_untouched_read_row() {
    let (db, _dir) = test_db().await;
    assert_eq!(
        db.record_agent_presence("claude-code").await.unwrap(),
        AgentPresence::Created
    );
    let before = db.get_agent("claude-code").await.unwrap().unwrap();
    assert_eq!(before.trust_level, "unknown");

    let agent = db.register_trusted_agent("claude-code").await.unwrap();

    assert_eq!(agent.trust_level, "full");
    let after = db.get_agent("claude-code").await.unwrap().unwrap();
    assert_eq!(after.trust_level, "full");
    // Promotion is not an edit and not a use: the sighting and the untouched
    // marker survive, so a second registration is a no-op.
    assert_eq!(after.last_seen_at, before.last_seen_at);
    assert_eq!(after.updated_at, after.created_at);
    assert_eq!(after.memory_count, 0);
    let again = db.register_trusted_agent("claude-code").await.unwrap();
    assert_eq!(again.trust_level, "full");
    assert_eq!(row_count(&db).await, 1);
}

#[tokio::test]
async fn registering_a_connected_tool_leaves_user_edited_and_written_rows_alone() {
    let (db, _dir) = test_db().await;

    // A user who set the trust to "unknown" themselves.
    db.record_agent_presence("edited-bot").await.unwrap();
    db.update_agent("edited-bot", None, Some("hands off"), None, None, None)
        .await
        .unwrap();
    let edited_before = db.get_agent("edited-bot").await.unwrap().unwrap();
    assert_ne!(edited_before.updated_at, edited_before.created_at);

    // A tool that already wrote (memory_count > 0) but is still "unknown".
    db.record_agent_presence("written-bot").await.unwrap();
    {
        let conn = db.conn.lock().await;
        conn.execute(
            "UPDATE agent_connections SET memory_count = 3 WHERE name = 'written-bot'",
            (),
        )
        .await
        .unwrap();
    }

    // A trust level the user chose.
    db.register_agent("review-bot").await.unwrap();
    db.update_agent("review-bot", None, None, None, Some("review"), None)
        .await
        .unwrap();

    for name in ["edited-bot", "written-bot", "review-bot"] {
        let before = db.get_agent(name).await.unwrap().unwrap();
        let returned = db.register_trusted_agent(name).await.unwrap();
        let after = db.get_agent(name).await.unwrap().unwrap();
        assert_eq!(returned.trust_level, before.trust_level, "{name}");
        assert_eq!(after.trust_level, before.trust_level, "{name}");
        assert_eq!(after.updated_at, before.updated_at, "{name}");
        assert_eq!(after.memory_count, before.memory_count, "{name}");
    }
    assert_eq!(
        db.get_agent("edited-bot")
            .await
            .unwrap()
            .unwrap()
            .trust_level,
        "unknown"
    );
    assert_eq!(
        db.get_agent("written-bot")
            .await
            .unwrap()
            .unwrap()
            .trust_level,
        "unknown"
    );
    assert_eq!(
        db.get_agent("review-bot")
            .await
            .unwrap()
            .unwrap()
            .trust_level,
        "review"
    );
}

#[tokio::test]
async fn registering_a_connected_tool_keeps_a_disabled_row_disabled() {
    let (db, _dir) = test_db().await;
    db.record_agent_presence("off-bot").await.unwrap();
    // Disable underneath the API so the row still has the untouched marker:
    // the only shape on which the `enabled` check is the guard.
    {
        let conn = db.conn.lock().await;
        conn.execute(
            "UPDATE agent_connections SET enabled = 0 WHERE name = 'off-bot'",
            (),
        )
        .await
        .unwrap();
    }

    let agent = db.register_trusted_agent("off-bot").await.unwrap();

    assert!(!agent.enabled);
    assert_eq!(agent.trust_level, "unknown");
    let after = db.get_agent("off-bot").await.unwrap().unwrap();
    assert!(!after.enabled);
    assert_eq!(after.trust_level, "unknown");
}

#[tokio::test]
async fn registering_a_connected_tool_rejects_names_the_read_path_rejects() {
    let (db, _dir) = test_db().await;
    let too_long = "x".repeat(65);
    for bad in [
        "",
        "   ",
        "---",
        "unknown",
        "wenlan-setup",
        "bad\u{0}name",
        "line\nbreak",
        too_long.as_str(),
    ] {
        let err = db
            .register_trusted_agent(bad)
            .await
            .expect_err("an invalid name must not register");
        assert!(
            matches!(err, crate::error::WenlanError::Validation(_)),
            "{bad:?}: {err:?}"
        );
    }
    assert_eq!(row_count(&db).await, 0);
}

// A tool that writes a page has used Wenlan, the same as one that stores a
// memory: a name nobody has seen is registered at "full" and marked seen,
// without counting a memory.
#[tokio::test]
async fn a_page_write_registers_a_new_tool_at_full_and_marks_it_seen() {
    let (db, _dir) = test_db().await;

    let trust = db.note_page_writer("Claude Code").await.unwrap();

    assert_eq!(trust.as_deref(), Some("full"));
    let agent = db.get_agent("claude-code").await.unwrap().unwrap();
    assert_eq!(agent.trust_level, "full");
    assert!(agent.last_seen_at.is_some());
    assert_eq!(agent.memory_count, 0);
}

// The hero run: the AI recalled first (a read-created "unknown" row), then
// wrote pages through steep. The page write must promote it, or the page
// gate keeps hiding those pages from its recall.
#[tokio::test]
async fn a_page_write_promotes_an_untouched_read_row() {
    let (db, _dir) = test_db().await;
    db.record_agent_presence("claude-code").await.unwrap();
    assert_eq!(
        db.get_agent("claude-code")
            .await
            .unwrap()
            .unwrap()
            .trust_level,
        "unknown"
    );

    let trust = db.note_page_writer("claude-code").await.unwrap();

    assert_eq!(trust.as_deref(), Some("full"));
    assert_eq!(
        db.get_agent("claude-code")
            .await
            .unwrap()
            .unwrap()
            .trust_level,
        "full"
    );
    assert_eq!(row_count(&db).await, 1);
}

#[tokio::test]
async fn a_page_write_keeps_a_trust_level_the_user_chose() {
    let (db, _dir) = test_db().await;
    db.record_agent_presence("careful-bot").await.unwrap();
    db.update_agent("careful-bot", None, None, None, Some("unknown"), None)
        .await
        .unwrap();

    let trust = db.note_page_writer("careful-bot").await.unwrap();

    assert_eq!(trust.as_deref(), Some("unknown"));
    assert_eq!(
        db.get_agent("careful-bot")
            .await
            .unwrap()
            .unwrap()
            .trust_level,
        "unknown"
    );
}

#[tokio::test]
async fn a_page_write_from_a_disabled_tool_is_refused() {
    let (db, _dir) = test_db().await;
    db.register_trusted_agent("off-bot").await.unwrap();
    db.update_agent("off-bot", None, None, Some(false), None, None)
        .await
        .unwrap();

    let error = db.note_page_writer("off-bot").await.unwrap_err();

    assert!(matches!(error, crate::error::WenlanError::AgentDisabled(_)));
}

#[tokio::test]
async fn a_page_write_with_a_name_a_read_may_not_record_creates_nothing() {
    let (db, _dir) = test_db().await;

    for name in ["", "   ", "unknown", "wenlan-setup"] {
        assert_eq!(db.note_page_writer(name).await.unwrap(), None, "{name:?}");
    }
    assert_eq!(row_count(&db).await, 0);
}
