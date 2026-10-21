// SPDX-License-Identifier: Apache-2.0

use super::sessions::{import_session_ref, last_ai_title, session_row_id, SESSION_READS_LISTED};
use super::tests::{test_db, test_db_at};
use crate::read_scope::ReadScope;
use wenlan_types::{SessionReadKind, SessionRef, UpsertSessionRequest};

fn session(raw: &str) -> SessionRef {
    SessionRef::parse(raw).unwrap()
}

#[test]
fn row_id_is_deterministic_and_prefixed() {
    let a = session_row_id(&session("claude-code:abc"));
    assert_eq!(a, session_row_id(&session("claude-code:abc")));
    assert!(a.starts_with("ses_"));
    assert_eq!(a.len(), 4 + 24);
    assert!(a[4..].chars().all(|c| c.is_ascii_hexdigit()));
    assert_ne!(a, session_row_id(&session("claude-code:abd")));
    assert_ne!(a, session_row_id(&session("codex:abc")));
}

#[test]
fn import_source_ids_map_to_sessions() {
    let claude = import_session_ref("import_claude_6f1c-22").unwrap();
    assert_eq!(
        (claude.tool.as_str(), claude.external_id.as_str()),
        ("claude-ai", "6f1c-22")
    );
    let chatgpt = import_session_ref("import_chatgpt_abc.def").unwrap();
    assert_eq!(chatgpt.tool, "chatgpt");
    assert!(import_session_ref("mem_123").is_none());
    assert!(import_session_ref("import_claude_").is_none());
    assert!(import_session_ref("import_claude_has space").is_none());
}

#[tokio::test]
async fn write_upsert_keeps_first_space_and_widens_the_span() {
    let (db, _temp) = test_db().await;
    let a = session("claude-code:space-test");

    db.upsert_session_metadata(&UpsertSessionRequest {
        session: a.to_string(),
        cwd: Some("/work/wenlan".into()),
        ..Default::default()
    })
    .await
    .unwrap();
    {
        let conn = db.conn.lock().await;
        for (id, space) in [("m1", "work"), ("m2", "personal")] {
            conn.execute(
                "INSERT INTO memories
                    (id, content, source, source_id, title, chunk_index,
                     last_modified, chunk_type, space)
                 VALUES (?1, 'content', 'memory', ?1, 'title', 0, 1, 'text', ?2)",
                libsql::params![id, space],
            )
            .await
            .unwrap();
        }
    }
    db.stamp_memory_session("m1", &a, Some("work"))
        .await
        .unwrap();
    db.stamp_memory_session("m2", &a, Some("personal"))
        .await
        .unwrap();

    let stored = db.get_session(&session_row_id(&a)).await.unwrap().unwrap();
    assert_eq!(
        stored.space.as_deref(),
        Some("work"),
        "first write's space wins"
    );
    assert_eq!(
        stored.cwd.as_deref(),
        Some("/work/wenlan"),
        "metadata is kept"
    );
    assert!(stored.last_write_at >= stored.first_write_at);

    // A memory that does not exist creates no session.
    let ghost = session("codex:never-wrote");
    db.stamp_memory_session("missing", &ghost, None)
        .await
        .unwrap();
    assert!(db
        .get_session(&session_row_id(&ghost))
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn metadata_upsert_overwrites_non_null_and_keeps_absent() {
    let (db, _temp) = test_db().await;
    let first = db
        .upsert_session_metadata(&UpsertSessionRequest {
            session: "codex:meta".into(),
            cwd: Some("/a".into()),
            repo_branch: Some("main".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    let second = db
        .upsert_session_metadata(&UpsertSessionRequest {
            session: "codex:meta".into(),
            repo_branch: Some("topic".into()),
            title: Some("  ".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(second.id, first.id);
    assert_eq!(second.cwd.as_deref(), Some("/a"));
    assert_eq!(second.repo_branch.as_deref(), Some("topic"));
    assert_eq!(second.title, None, "blank means absent");
    assert_eq!(second.last_write_at, first.last_write_at);

    let too_long = db
        .upsert_session_metadata(&UpsertSessionRequest {
            session: "codex:meta".into(),
            cwd: Some("x".repeat(5000)),
            ..Default::default()
        })
        .await;
    assert!(too_long.is_err());
    let malformed = db
        .upsert_session_metadata(&UpsertSessionRequest {
            session: "nope".into(),
            ..Default::default()
        })
        .await;
    assert!(malformed.is_err());
}

#[tokio::test]
async fn migration_backfills_a_session_per_import_conversation() {
    let (db, _temp) = test_db_at(139).await;
    {
        let conn = db.conn.lock().await;
        // Conversation one has two chunks; conversation two is ChatGPT; the
        // last row is an ordinary memory that must stay unstamped.
        let rows = [
            ("a0", "import_claude_conv-1", "First chat", 0, 100),
            ("a1", "import_claude_conv-1", "First chat", 1, 300),
            ("b0", "import_chatgpt_conv-2", "Second chat", 0, 200),
            ("c0", "mem_plain", "Plain", 0, 400),
        ];
        for (id, source_id, title, chunk, created_at) in rows {
            conn.execute(
                "INSERT INTO memories
                    (id, content, source, source_id, title, chunk_index,
                     last_modified, chunk_type, created_at)
                 VALUES (?1, 'content', 'memory', ?2, ?3, ?4, 999, 'text', ?5)",
                libsql::params![id, source_id, title, chunk, created_at],
            )
            .await
            .unwrap();
        }
    }

    db.migrate_140_agent_sessions(139).await.unwrap();
    // Replaying a crashed migration is harmless.
    db.migrate_140_agent_sessions(139).await.unwrap();
    // The detail also lists reads, which migration 141 adds.
    db.migrate_141_session_reads().await.unwrap();

    let claude = db
        .get_session(&session_row_id(&session("claude-ai:conv-1")))
        .await
        .unwrap()
        .expect("claude conversation session");
    assert_eq!(claude.title.as_deref(), Some("First chat"));
    assert_eq!((claude.first_write_at, claude.last_write_at), (100, 300));
    let chatgpt = db
        .get_session(&session_row_id(&session("chatgpt:conv-2")))
        .await
        .unwrap()
        .expect("chatgpt conversation session");
    assert_eq!((chatgpt.first_write_at, chatgpt.last_write_at), (200, 200));

    let detail = db
        .get_session_detail(&claude.id, &ReadScope::Global)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        detail.memories.len(),
        1,
        "chunks collapse to one conversation"
    );
    assert_eq!(detail.memories[0].source_id, "import_claude_conv-1");

    let conn = db.conn.lock().await;
    let mut rows = conn
        .query("SELECT session_id FROM memories WHERE id = 'c0'", ())
        .await
        .unwrap();
    let plain: Option<String> = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(plain, None);
    let mut rows = conn.query("PRAGMA user_version", ()).await.unwrap();
    assert_eq!(
        rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
        141
    );
}

// ---------------------------------------------------------------------------
// Migration 141, title extraction, read recording, and `cited_by`.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn migration_141_is_replay_safe() {
    let (db, _temp) = test_db_at(140).await;
    db.migrate_141_session_reads().await.unwrap();
    // Replaying a crashed migration (table made, version bump lost) is harmless.
    db.migrate_141_session_reads().await.unwrap();

    let conn = db.conn.lock().await;
    let mut rows = conn
        .query(
            "SELECT COUNT(*) FROM pragma_table_info('session_reads')",
            (),
        )
        .await
        .unwrap();
    assert_eq!(
        rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
        6
    );
    let mut rows = conn.query("PRAGMA user_version", ()).await.unwrap();
    assert_eq!(
        rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
        141
    );
}

fn transcript(dir: &tempfile::TempDir, body: &str) -> std::path::PathBuf {
    let path = dir.path().join("session.jsonl");
    std::fs::write(&path, body).unwrap();
    path
}

#[test]
fn title_is_the_last_ai_title_line() {
    let dir = tempfile::tempdir().unwrap();
    let path = transcript(
        &dir,
        concat!(
            "{\"type\":\"user\",\"message\":\"hello\"}\n",
            "{\"type\":\"ai-title\",\"aiTitle\":\"First title\",\"sessionId\":\"s\"}\n",
            "{\"type\":\"assistant\",\"text\":\"mentions \\\"ai-title\\\" in prose\"}\n",
            "{\"type\":\"ai-title\",\"aiTitle\":\"  文兰 AI 会话资源  \",\"sessionId\":\"s\"}\n",
        ),
    );
    assert_eq!(last_ai_title(&path).as_deref(), Some("文兰 AI 会话资源"));
}

#[test]
fn malformed_and_empty_title_lines_are_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let path = transcript(
        &dir,
        concat!(
            "{\"type\":\"ai-title\",\"aiTitle\":\"Good\"}\n",
            "{\"type\":\"ai-title\",\"aiTitle\":\"cut off\n",
            "not json but says \"ai-title\"\n",
            "{\"type\":\"ai-title\",\"aiTitle\":\"   \"}\n",
            "{\"type\":\"ai-title\",\"aiTitle\":7}\n",
            "{\"type\":\"other\",\"aiTitle\":\"Wrong type\"}\n",
        ),
    );
    assert_eq!(last_ai_title(&path).as_deref(), Some("Good"));

    let none = transcript(&dir, "{\"type\":\"user\"}\n\n{broken\n");
    assert_eq!(last_ai_title(&none), None);
    assert_eq!(last_ai_title(&dir.path().join("missing.jsonl")), None);
}

#[test]
fn a_title_on_the_final_line_without_a_newline_counts() {
    let dir = tempfile::tempdir().unwrap();
    let path = transcript(&dir, "{\"type\":\"ai-title\",\"aiTitle\":\"No newline\"}");
    assert_eq!(last_ai_title(&path).as_deref(), Some("No newline"));
}

#[test]
fn an_overlong_line_is_skipped_and_does_not_hide_the_title() {
    let dir = tempfile::tempdir().unwrap();
    let long = format!(
        "{{\"type\":\"user\",\"text\":\"{}\"}}\n",
        "x".repeat(200_000)
    );
    let body = format!(
        "{{\"type\":\"ai-title\",\"aiTitle\":\"Before\"}}\n{long}{{\"type\":\"ai-title\",\"aiTitle\":\"After\"}}\n"
    );
    let path = transcript(&dir, &body);
    assert_eq!(last_ai_title(&path).as_deref(), Some("After"));
}

async fn announce(db: &super::MemoryDB, raw: &str) -> String {
    db.upsert_session_metadata(&UpsertSessionRequest {
        session: raw.into(),
        ..Default::default()
    })
    .await
    .unwrap()
    .id
}

async fn read_rows(db: &super::MemoryDB) -> i64 {
    let conn = db.conn.lock().await;
    let mut rows = conn
        .query("SELECT COUNT(*) FROM session_reads", ())
        .await
        .unwrap();
    rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap()
}

#[tokio::test]
async fn reads_are_recorded_collapsed_and_capped() {
    let (db, _temp) = test_db().await;
    let a = session("claude-code:reader");
    let id = announce(&db, &a.to_string()).await;

    assert!(db
        .record_session_read(&a, SessionReadKind::Recall, "  how do   we\nsync ", None)
        .await
        .unwrap());
    // An identical consecutive read moves the newest entry, not a new row.
    assert!(db
        .record_session_read(&a, SessionReadKind::Recall, "how do we sync", None)
        .await
        .unwrap());
    assert_eq!(read_rows(&db).await, 1);
    // Same text, different kind or Space, is a different entry.
    db.record_session_read(&a, SessionReadKind::Brief, "how do we sync", None)
        .await
        .unwrap();
    db.record_session_read(&a, SessionReadKind::Brief, "how do we sync", Some("work"))
        .await
        .unwrap();
    assert_eq!(read_rows(&db).await, 3);
    // Not consecutive: the first query again is a new row.
    db.record_session_read(&a, SessionReadKind::Recall, "how do we sync", None)
        .await
        .unwrap();
    assert_eq!(read_rows(&db).await, 4);

    // Blank labels record nothing; long ones are cut.
    assert!(!db
        .record_session_read(&a, SessionReadKind::Recall, "   ", None)
        .await
        .unwrap());
    db.record_session_read(&a, SessionReadKind::Recall, &"長".repeat(300), None)
        .await
        .unwrap();
    let detail = db
        .get_session_detail(&id, &ReadScope::Global)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(detail.reads[0].label.chars().count(), 80);
    assert_eq!(detail.reads[0].kind, SessionReadKind::Recall);

    // Only the newest 50 are kept.
    for i in 0..80 {
        db.record_session_read(&a, SessionReadKind::Recall, &format!("query {i}"), None)
            .await
            .unwrap();
    }
    assert_eq!(read_rows(&db).await, 50);
    let detail = db
        .get_session_detail(&id, &ReadScope::Global)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(detail.reads_total, 50);
    assert_eq!(detail.reads[0].label, "query 79");
    assert!(detail.reads.len() >= SESSION_READS_LISTED);
}

#[tokio::test]
async fn a_read_never_creates_a_session() {
    let (db, _temp) = test_db().await;
    let ghost = session("codex:never-announced");
    assert!(!db
        .record_session_read(&ghost, SessionReadKind::Recall, "anything", None)
        .await
        .unwrap());
    assert_eq!(read_rows(&db).await, 0);
    assert!(db
        .get_session(&session_row_id(&ghost))
        .await
        .unwrap()
        .is_none());
}

const T0: &str = "2026-01-01T00:00:00Z";

async fn memory(db: &super::MemoryDB, source_id: &str, space: &str) {
    let conn = db.conn.lock().await;
    conn.execute(
        "INSERT INTO memories
            (id, content, source, source_id, title, chunk_index,
             last_modified, chunk_type, space)
         VALUES (?1, 'content', 'memory', ?1, 'title', 0, 1, 'text', ?2)",
        libsql::params![source_id, space],
    )
    .await
    .unwrap();
}

async fn note(db: &super::MemoryDB, id: &str, title: &str, space: &str, cites: &[&str]) {
    db.insert_page_with_kind(
        id,
        title,
        None,
        "body",
        None,
        Some(space),
        cites,
        T0,
        "authored",
        "confirmed",
        Some(space),
        None,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn reads_respect_space_scope_and_resolve_note_titles() {
    let (db, _temp) = test_db().await;
    db.create_space("work", None, false).await.unwrap();
    db.create_space("home", None, false).await.unwrap();
    let a = session("claude-code:scoped-reads");
    let id = announce(&db, &a.to_string()).await;
    memory(&db, "m_work", "work").await;
    db.stamp_memory_session("m_work", &a, Some("work"))
        .await
        .unwrap();
    note(&db, "page_work", "Work note", "work", &[]).await;
    note(&db, "page_home", "Home note", "home", &[]).await;

    db.record_session_read(&a, SessionReadKind::Recall, "library wide", None)
        .await
        .unwrap();
    db.record_session_read(&a, SessionReadKind::Recall, "work only", Some("work"))
        .await
        .unwrap();
    db.record_session_read(&a, SessionReadKind::ReadPage, "page_work", Some("work"))
        .await
        .unwrap();
    db.record_session_read(&a, SessionReadKind::ReadPage, "page_home", Some("home"))
        .await
        .unwrap();
    db.record_session_read(&a, SessionReadKind::ReadPage, "page_deleted", Some("work"))
        .await
        .unwrap();

    let global = db
        .get_session_detail(&id, &ReadScope::Global)
        .await
        .unwrap()
        .unwrap();
    let labels: Vec<_> = global.reads.iter().map(|r| r.label.as_str()).collect();
    assert_eq!(
        labels,
        ["page_home", "page_work", "work only", "library wide"],
        "a deleted note drops out, newest first"
    );
    assert_eq!(global.reads[1].title.as_deref(), Some("Work note"));
    assert_eq!(global.reads_total, 4);

    let work = db
        .get_session_detail(&id, &ReadScope::Space("work".into()))
        .await
        .unwrap()
        .unwrap();
    let labels: Vec<_> = work.reads.iter().map(|r| r.label.as_str()).collect();
    assert_eq!(
        labels,
        ["page_work", "work only"],
        "the library wide query and the other Space stay hidden"
    );
    assert_eq!(work.reads_total, 2);
}

#[tokio::test]
async fn cited_by_lists_notes_that_used_the_sessions_memories() {
    let (db, _temp) = test_db().await;
    db.create_space("work", None, false).await.unwrap();
    db.create_space("home", None, false).await.unwrap();
    let a = session("claude-code:author");
    let b = session("codex:curator");
    let id = announce(&db, &a.to_string()).await;
    announce(&db, &b.to_string()).await;

    for (source_id, space) in [("m_a1", "work"), ("m_a2", "work"), ("m_home", "home")] {
        memory(&db, source_id, space).await;
        db.stamp_memory_session(source_id, &a, Some(space))
            .await
            .unwrap();
    }
    memory(&db, "m_other", "work").await;
    db.stamp_memory_session("m_other", &b, Some("work"))
        .await
        .unwrap();

    note(&db, "page_cites", "Cites both", "work", &["m_a1", "m_a2"]).await;
    note(&db, "page_own", "Written by the session", "work", &["m_a1"]).await;
    note(&db, "page_unrelated", "Unrelated", "work", &["m_other"]).await;
    note(&db, "page_home", "Home note", "home", &["m_home"]).await;
    {
        // The session wrote a version of `page_own`: it is listed under the
        // notes it left, not here.
        let conn = db.conn.lock().await;
        conn.execute(
            "INSERT INTO page_history (page_id, version, title, content, edited_by, created_at, session_id)
             SELECT 'page_own', COALESCE(MAX(version), 0) + 1, 'Written by the session', 'body',
                    'agent', 50, ?1
               FROM page_history WHERE page_id = 'page_own'",
            libsql::params![id.as_str()],
        )
        .await
        .unwrap();
    }

    let global = db
        .get_session_detail(&id, &ReadScope::Global)
        .await
        .unwrap()
        .unwrap();
    let mut ids: Vec<_> = global.cited_by.iter().map(|n| n.id.as_str()).collect();
    ids.sort();
    assert_eq!(ids, ["page_cites", "page_home"]);

    let work = db
        .get_session_detail(&id, &ReadScope::Space("work".into()))
        .await
        .unwrap()
        .unwrap();
    let ids: Vec<_> = work.cited_by.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(ids, ["page_cites"], "the other Space's note stays hidden");
}
