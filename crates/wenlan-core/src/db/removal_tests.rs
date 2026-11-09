use super::*;
use crate::db::tests::test_db;
use crate::read_scope::ReadScope;
use crate::sources::RawDocument;
use std::collections::HashMap;

const SOURCE_ID: &str = "mem-kestrel-forget";

fn memory_doc() -> RawDocument {
    RawDocument {
        source: "memory".into(),
        source_id: SOURCE_ID.into(),
        title: "Kestrel marker".into(),
        content: "The Cedar project uses the amber kestrel marker for client reviews.".into(),
        last_modified: 1,
        metadata: HashMap::new(),
        source_agent: Some("claude-code".into()),
        confirmed: Some(true),
        ..Default::default()
    }
}

async fn modes(db: &MemoryDB) -> Vec<Option<String>> {
    let conn = db.conn.lock().await;
    let mut rows = conn
        .query(
            "SELECT supersede_mode FROM memories WHERE source = 'memory' AND source_id = ?1",
            libsql::params![SOURCE_ID],
        )
        .await
        .unwrap();
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.unwrap() {
        out.push(row.get::<Option<String>>(0).ok().flatten());
    }
    out
}

async fn found_by_search(db: &MemoryDB) -> bool {
    db.search_memory(
        "amber kestrel marker",
        20,
        None,
        &ReadScope::Global,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap()
    .iter()
    .any(|row| row.source_id == SOURCE_ID)
}

#[tokio::test]
async fn ux_who_7_forget_keeps_the_rows_and_restore_brings_the_memory_back() {
    let (db, _tmp) = test_db().await;
    db.upsert_documents(vec![memory_doc()]).await.unwrap();
    let before = modes(&db).await;
    assert!(!before.is_empty());
    assert!(found_by_search(&db).await);

    let outcome = db
        .forget_memory(SOURCE_ID, "claude-code", "forgot \"Kestrel marker\"")
        .await
        .unwrap();
    assert_eq!(outcome, ForgetOutcome::Forgotten);
    let forgotten = modes(&db).await;
    assert_eq!(forgotten.len(), before.len(), "forget must not delete rows");
    assert!(forgotten.iter().all(|m| m.as_deref() == Some("evicted")));
    assert!(
        !found_by_search(&db).await,
        "a forgotten memory leaves search"
    );

    assert_eq!(
        db.forget_memory(SOURCE_ID, "claude-code", "again")
            .await
            .unwrap(),
        ForgetOutcome::AlreadyForgotten
    );

    assert!(db
        .restore_forgotten_memory(SOURCE_ID, "person")
        .await
        .unwrap());
    assert_eq!(
        modes(&db).await,
        before,
        "restore puts back each row's mode"
    );
    assert!(found_by_search(&db).await);
    assert!(!db
        .restore_forgotten_memory(SOURCE_ID, "person")
        .await
        .unwrap());
}

#[tokio::test]
async fn ux_who_7_forget_and_restore_each_write_an_activity_record() {
    let (db, _tmp) = test_db().await;
    db.upsert_documents(vec![memory_doc()]).await.unwrap();
    db.forget_memory(SOURCE_ID, "codex", "forgot \"Kestrel marker\"")
        .await
        .unwrap();
    db.restore_forgotten_memory(SOURCE_ID, "person")
        .await
        .unwrap();
    let rows = db.list_agent_activity(50, None, None).await.unwrap();
    let forget = rows
        .iter()
        .find(|row| row.action == "forget")
        .expect("forget row");
    assert_eq!(forget.agent_name, "codex");
    assert_eq!(forget.detail.as_deref(), Some("forgot \"Kestrel marker\""));
    assert!(forget
        .memory_ids
        .as_deref()
        .unwrap_or_default()
        .contains(SOURCE_ID));
    let restore = rows
        .iter()
        .find(|row| row.action == "memory_restore")
        .expect("restore row");
    assert_eq!(restore.agent_name, "person");
}

#[tokio::test]
async fn forgetting_a_missing_memory_changes_and_records_nothing() {
    let (db, _tmp) = test_db().await;
    assert_eq!(
        db.forget_memory("no-such-memory", "codex", "x")
            .await
            .unwrap(),
        ForgetOutcome::NotFound
    );
    let rows = db.list_agent_activity(50, None, None).await.unwrap();
    assert!(rows.iter().all(|row| row.action != "forget"));
}

#[tokio::test]
async fn restore_leaves_rows_evicted_for_another_reason() {
    let (db, _tmp) = test_db().await;
    db.upsert_documents(vec![memory_doc()]).await.unwrap();
    {
        let conn = db.conn.lock().await;
        conn.execute(
            "UPDATE memories SET supersede_mode = 'evicted' WHERE source_id = ?1",
            libsql::params![SOURCE_ID],
        )
        .await
        .unwrap();
    }
    assert!(!db
        .restore_forgotten_memory(SOURCE_ID, "person")
        .await
        .unwrap());
    assert!(modes(&db)
        .await
        .iter()
        .all(|m| m.as_deref() == Some("evicted")));
}

#[tokio::test]
async fn a_forget_is_logged_once_with_who_asked() {
    let (db, _tmp) = test_db().await;
    db.upsert_documents(vec![memory_doc()]).await.unwrap();
    let logs = crate::test_log::LogCapture::start();
    db.forget_memory(SOURCE_ID, "codex", "forgot")
        .await
        .unwrap();
    db.forget_memory(SOURCE_ID, "codex", "again").await.unwrap();
    let lines = logs.matching("INFO", &["[forget]", SOURCE_ID, "forgotten by codex"]);
    assert_eq!(lines.len(), 1, "{:?}", logs.lines());
}
