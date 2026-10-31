// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::events::NoopEmitter;
use std::sync::Arc;

async fn test_db() -> (MemoryDB, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let db = MemoryDB::new(&dir.path().join("test.db"), Arc::new(NoopEmitter))
        .await
        .unwrap();
    (db, dir)
}

async fn insert_decision(
    db: &MemoryDB,
    source_id: &str,
    content: &str,
    fields: Option<&str>,
    space: Option<&str>,
) {
    let conn = db.test_primary_session().await;
    conn.execute(
        "INSERT INTO memories (
            id, content, source, source_id, title, chunk_index, last_modified,
            chunk_type, memory_type, space, source_agent, confidence,
            confirmed, word_count, enrichment_status, quality, is_recap,
            supersede_mode, stability, structured_fields
         )
         VALUES (
            ?1, ?2, 'memory', ?1, ?3, 0, ?4, 'document', 'decision',
            ?5, 'test', 0.9, 1, 8, 'enriched', 'high', 0,
            'archive', 'learned', ?6
         )",
        libsql::params![
            source_id,
            content,
            content.chars().take(40).collect::<String>(),
            1_760_000_000i64,
            space.unwrap_or(crate::db::UNFILED_SPACE_ID).to_string(),
            fields.map(str::to_string),
        ],
    )
    .await
    .unwrap();
}

fn noon() -> DateTime<Local> {
    Local
        .with_ymd_and_hms(2026, 10, 31, 12, 0, 0)
        .single()
        .unwrap()
}

#[test]
fn sentences_skip_code_and_list_markers() {
    let reply =
        "First point. Second point!\n- a bullet item here\n```\nuse neo4j;\n```\n短句。另一句";
    assert_eq!(
        split_sentences(reply),
        vec![
            "First point.",
            "Second point!",
            "a bullet item here",
            "短句。",
            "另一句"
        ]
    );
    assert!(split_sentences("").is_empty());
}

#[test]
fn history_entry_has_the_spec_shape_and_a_valid_crc() {
    let entry = reminder_history_entry(ReminderEntry {
        seq: 7,
        at: Utc
            .with_ymd_and_hms(2026, 10, 31, 4, 0, 0)
            .single()
            .unwrap(),
        session: Some("claude-code:abc"),
        target_id: "mem_1",
        version: 2,
        text: "Earlier decision",
        data: serde_json::json!({"k": 1}),
    });
    for key in [
        "seq", "ts", "tx", "actor", "op", "target", "version", "why", "text", "data", "crc",
    ] {
        assert!(entry.get(key).is_some(), "missing {key}");
    }
    assert_eq!(entry["op"], "chat.reminder");
    assert_eq!(entry["seq"], 7);
    assert_eq!(entry["ts"], "2026-10-31T04:00:00.000Z");
    assert_eq!(entry["actor"]["kind"], "system");
    assert_eq!(entry["actor"]["agent"], "claude-code");
    assert_eq!(entry["actor"]["session"], "claude-code:abc");
    assert_eq!(
        entry["target"],
        serde_json::json!({"type": "memory", "id": "mem_1"})
    );
    let mut without = entry.clone();
    let crc = without
        .as_object_mut()
        .unwrap()
        .remove("crc")
        .unwrap()
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        crc,
        format!("{:08x}", crc32(without.to_string().as_bytes()))
    );
}

#[test]
fn crc32_matches_the_ieee_check_value() {
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
}

#[test]
fn reminder_text_follows_the_decision_language() {
    assert_eq!(
        reminder_text("Use libSQL.", "2026-10-12", "wenlan://memory/m"),
        "Earlier decision (2026-10-12): Use libSQL. wenlan://memory/m"
    );
    assert_eq!(
        reminder_text("用 libSQL。", "2026-10-12", "wenlan://page/p"),
        "之前的決定（2026-10-12）：用 libSQL。 wenlan://page/p"
    );
}

#[tokio::test]
async fn a_conflicting_reply_gets_one_logged_reminder_with_its_source() {
    let (db, _dir) = test_db().await;
    insert_decision(
        &db,
        "mem_storage",
        "Use libSQL instead of a dedicated graph database.",
        Some(r#"{"decision":"Use libSQL instead of a dedicated graph database","context":"entity storage","date":"2026-10-12"}"#),
        None,
    )
    .await;
    let reply = "Sure. Let's switch to a dedicated graph database for the entity store.";
    let response = check_against_decisions(
        &db,
        reply,
        Some("claude-code:s1"),
        &ReadScope::Global,
        noon(),
    )
    .await
    .unwrap();
    assert_eq!(response.skipped, None);
    assert_eq!(response.reminders.len(), 1, "{response:?}");
    let reminder = &response.reminders[0];
    assert_eq!(reminder.decided_on, "2026-10-12");
    assert_eq!(reminder.source.memory_id, "mem_storage");
    assert_eq!(reminder.source.link, "wenlan://memory/mem_storage");
    assert!(reminder.text.contains("2026-10-12"));
    assert!(reminder.text.contains("wenlan://memory/mem_storage"));
    assert!(reminder.reply_excerpt.contains("graph database"));
    assert_eq!(response.remaining_today, CHAT_REMINDER_DAILY_CAP - 1);

    let entries = db.list_chat_reminder_entries().await.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["op"], "chat.reminder");
    assert_eq!(entries[0]["seq"], reminder.seq);
    assert_eq!(entries[0]["target"]["id"], "mem_storage");
    assert_eq!(entries[0]["data"]["source"]["memory_id"], "mem_storage");

    // The same decision is not repeated in the same session that day.
    let again = check_against_decisions(
        &db,
        reply,
        Some("claude-code:s1"),
        &ReadScope::Global,
        noon(),
    )
    .await
    .unwrap();
    assert!(again.reminders.is_empty());
    // Another session still hears about it.
    let other = check_against_decisions(
        &db,
        reply,
        Some("claude-code:s2"),
        &ReadScope::Global,
        noon(),
    )
    .await
    .unwrap();
    assert_eq!(other.reminders.len(), 1);
}

#[tokio::test]
async fn an_unrelated_or_agreeing_reply_is_silent_and_logs_nothing() {
    let (db, _dir) = test_db().await;
    insert_decision(
        &db,
        "mem_storage",
        "Use libSQL instead of a dedicated graph database.",
        None,
        None,
    )
    .await;
    for reply in [
        "I fixed the typo in the README.",
        "We keep using libSQL for the entity store.",
    ] {
        let response = check_against_decisions(&db, reply, None, &ReadScope::Global, noon())
            .await
            .unwrap();
        assert!(response.reminders.is_empty(), "{reply}: {response:?}");
        assert_eq!(response.skipped, None);
    }
    assert!(db.list_chat_reminder_entries().await.unwrap().is_empty());
    let empty = check_against_decisions(&db, "  ", None, &ReadScope::Global, noon())
        .await
        .unwrap();
    assert_eq!(empty.skipped, Some(ChatCheckSkipped::Empty));
}

#[tokio::test]
async fn the_daily_cap_is_counted_from_the_log() {
    let (db, _dir) = test_db().await;
    for i in 0..7 {
        insert_decision(
            &db,
            &format!("mem_{i}"),
            &format!("Use libSQL instead of graphstore{i}."),
            None,
            None,
        )
        .await;
    }
    let reply = (0..7)
        .map(|i| format!("Let's switch to graphstore{i}."))
        .collect::<Vec<_>>()
        .join(" ");
    let first = check_against_decisions(
        &db,
        &reply,
        Some("claude-code:a"),
        &ReadScope::Global,
        noon(),
    )
    .await
    .unwrap();
    assert_eq!(first.reminders.len(), CHAT_REMINDER_DAILY_CAP as usize);
    assert_eq!(first.remaining_today, 0);
    let capped = check_against_decisions(
        &db,
        &reply,
        Some("claude-code:b"),
        &ReadScope::Global,
        noon(),
    )
    .await
    .unwrap();
    assert!(capped.reminders.is_empty());
    assert_eq!(capped.skipped, Some(ChatCheckSkipped::DailyCap));
    // The next local day starts a fresh count.
    let tomorrow = noon() + chrono::Duration::days(1);
    let next_day = check_against_decisions(
        &db,
        &reply,
        Some("claude-code:b"),
        &ReadScope::Global,
        tomorrow,
    )
    .await
    .unwrap();
    assert_eq!(next_day.reminders.len(), CHAT_REMINDER_DAILY_CAP as usize);
    let seqs: Vec<i64> = db
        .list_chat_reminder_entries()
        .await
        .unwrap()
        .iter()
        .map(|e| e["seq"].as_i64().unwrap())
        .collect();
    assert_eq!(seqs, (1..=10).collect::<Vec<_>>());
}

#[tokio::test]
async fn decisions_outside_the_scope_are_not_used() {
    let (db, _dir) = test_db().await;
    insert_decision(
        &db,
        "mem_work",
        "Use libSQL instead of a dedicated graph database.",
        None,
        Some("work"),
    )
    .await;
    let reply = "Let's switch to a dedicated graph database.";
    let other = check_against_decisions(
        &db,
        reply,
        None,
        &ReadScope::Space("someone-else".into()),
        noon(),
    )
    .await
    .unwrap();
    assert!(other.reminders.is_empty());
    let inside =
        check_against_decisions(&db, reply, None, &ReadScope::Space("work".into()), noon())
            .await
            .unwrap();
    assert_eq!(inside.reminders.len(), 1);
}

#[tokio::test]
async fn context_returns_the_decisions_a_prompt_touches() {
    let (db, _dir) = test_db().await;
    insert_decision(
        &db,
        "mem_storage",
        "Use libSQL instead of a dedicated graph database for entity storage.",
        None,
        None,
    )
    .await;
    insert_decision(
        &db,
        "mem_css",
        "Use Tailwind for styling the desktop app.",
        None,
        None,
    )
    .await;
    let decisions = decision_context(
        &db,
        "Should the entity storage move to a graph database?",
        &ReadScope::Global,
    )
    .await
    .unwrap();
    assert_eq!(decisions.len(), 1, "{decisions:?}");
    assert_eq!(decisions[0].source.memory_id, "mem_storage");
    let block = context_block(&decisions);
    assert!(block.starts_with("Past decisions in Wenlan"));
    assert!(block.contains("wenlan://memory/mem_storage"));
    assert!(decision_context(&db, "hello there", &ReadScope::Global)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(context_block(&[]), "");
}
