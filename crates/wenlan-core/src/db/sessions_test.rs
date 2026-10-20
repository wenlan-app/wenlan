// SPDX-License-Identifier: Apache-2.0

use super::sessions::{import_session_ref, session_row_id};
use super::tests::{test_db, test_db_at};
use crate::read_scope::ReadScope;
use wenlan_types::{SessionRef, UpsertSessionRequest};

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
    let (db, _temp) = test_db_at(138).await;
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

    db.migrate_139_agent_sessions(138).await.unwrap();
    // Replaying a crashed migration is harmless.
    db.migrate_139_agent_sessions(138).await.unwrap();

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
        139
    );
}
