use super::tests::test_db;
use crate::read_scope::ReadScope;
use crate::sources::RawDocument;

fn activity_memory(source_id: &str, space: &str) -> RawDocument {
    RawDocument {
        source: "memory".to_string(),
        source_id: source_id.to_string(),
        title: source_id.to_string(),
        content: format!("activity owner {source_id}"),
        space: Some(space.to_string()),
        memory_type: Some("fact".to_string()),
        confirmed: Some(true),
        ..Default::default()
    }
}

#[tokio::test]
async fn webpage_chunks_are_exact_kind_scoped_and_ordered() {
    let (db, _temp) = test_db().await;
    let conn = db.conn.lock().await;
    for (id, source, space, index, pending) in [
        ("memory-collision", "memory", "work", 0, 0),
        ("file-collision", "file", "work", 0, 0),
        ("web-second", "webpage", "work", 1, 0),
        ("web-first", "webpage", "work", 0, 0),
        ("web-other-space", "webpage", "personal", 0, 0),
        ("web-pending", "webpage", "work", 2, 1),
    ] {
        conn.execute(
            "INSERT INTO memories (id, content, source, source_id, title, chunk_index,
                last_modified, chunk_type, space, pending_revision)
             VALUES (?1, ?1, ?2, 'shared', 'Saved excerpt', ?3, 1, 'text', ?4, ?5)",
            libsql::params![id, source, index, space, pending],
        )
        .await
        .unwrap();
    }
    drop(conn);
    let chunks = db
        .get_webpage_chunks_scoped("shared", &ReadScope::Space("work".into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        chunks
            .iter()
            .map(|chunk| chunk.id.as_str())
            .collect::<Vec<_>>(),
        vec!["web-first", "web-second"]
    );
    assert!(db
        .get_webpage_chunks_scoped("shared", &ReadScope::Uncategorized)
        .await
        .unwrap()
        .is_none());
    assert!(db
        .get_webpage_chunks_scoped("missing", &ReadScope::Global)
        .await
        .unwrap()
        .is_none());
    let legacy = db
        .get_chunks_scoped("shared", &ReadScope::Space("work".into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(legacy.len(), 1);
    assert_eq!(legacy[0].id, "memory-collision");
}

#[tokio::test]
async fn folder_chunk_locator_reads_sorted_siblings_within_scope() {
    let (db, _temp) = test_db().await;
    let conn = db.conn.lock().await;
    for (id, source, source_id, index, space) in [
        ("folder-chunk-2", "memory", "folder-doc", 2, "work"),
        ("folder-chunk-0", "memory", "folder-doc", 0, "work"),
        ("folder-chunk-1", "memory", "folder-doc", 1, "work"),
        (
            "folder-chunk-personal",
            "memory",
            "folder-doc",
            3,
            "personal",
        ),
        (
            "folder-chunk-private",
            "memory",
            "same-name-doc",
            0,
            "personal",
        ),
        ("same-name-work", "memory", "same-name-doc", 0, "work"),
        ("folder-webpage", "webpage", "folder-doc", 3, "work"),
        ("folder-episode", "episode", "folder-doc", 4, "work"),
    ] {
        conn.execute(
            "INSERT INTO memories (id, content, source, source_id, title, chunk_index,
                last_modified, chunk_type, space)
             VALUES (?1, ?1, ?2, ?3, 'Folder document', ?4, 1, 'text', ?5)",
            libsql::params![id, source, source_id, index, space],
        )
        .await
        .unwrap();
    }
    drop(conn);

    let scope = ReadScope::Space("work".into());
    let chunks = db
        .get_chunks_scoped("folder-chunk-1", &scope)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        chunks
            .iter()
            .map(|chunk| chunk.id.as_str())
            .collect::<Vec<_>>(),
        vec!["folder-chunk-0", "folder-chunk-1", "folder-chunk-2"]
    );
    assert!(
        db.get_chunks_scoped("folder-chunk-private", &scope)
            .await
            .unwrap()
            .is_none(),
        "an out-of-scope chunk locator must not resolve through a same-name in-scope source"
    );
}

#[tokio::test]
async fn scoped_chunks_prefer_canonical_source_id_over_a_chunk_id_collision() {
    let (db, _temp) = test_db().await;
    let conn = db.conn.lock().await;
    for (id, source, source_id, index) in [
        ("canonical-file-0", "file", "requested", 0),
        ("requested", "memory", "other-memory", 0),
        ("canonical-file-1", "file", "requested", 1),
    ] {
        conn.execute(
            "INSERT INTO memories (id, content, source, source_id, title, chunk_index,
                last_modified, chunk_type, space)
             VALUES (?1, ?1, ?2, ?3, 'Source', ?4, 1, 'text', 'work')",
            libsql::params![id, source, source_id, index],
        )
        .await
        .unwrap();
    }
    drop(conn);

    let chunks = db
        .get_chunks_scoped("requested", &ReadScope::Space("work".into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        chunks
            .iter()
            .map(|chunk| chunk.id.as_str())
            .collect::<Vec<_>>(),
        vec!["canonical-file-0", "canonical-file-1"],
        "an exact canonical source_id match must outrank a chunk id match"
    );
}

#[tokio::test]
async fn webpage_chunks_propagate_database_failures() {
    let (db, _temp) = test_db().await;
    db.conn
        .lock()
        .await
        .execute_batch("DROP TABLE memories;")
        .await
        .unwrap();
    assert!(db
        .get_webpage_chunks_scoped("missing", &ReadScope::Global)
        .await
        .is_err());
}

#[tokio::test]
async fn scoped_chunks_propagate_database_failures() {
    let (db, _temp) = test_db().await;
    let conn = db.conn.lock().await;
    conn.execute_batch("DROP TABLE memories;").await.unwrap();
    drop(conn);

    let result = db.get_chunks_scoped("missing", &ReadScope::Global).await;

    assert!(
        result.is_err(),
        "query failure must not become an empty success"
    );
}

#[tokio::test]
async fn scoped_briefing_stats_propagate_database_failures() {
    let (db, _temp) = test_db().await;
    let conn = db.conn.lock().await;
    conn.execute_batch("DROP TABLE memories;").await.unwrap();
    drop(conn);

    let result = db
        .get_briefing_stats_scoped(
            chrono::Utc::now().timestamp() - 3_600,
            &ReadScope::Space("work".to_string()),
        )
        .await;

    assert!(result.is_err(), "query failure must not become empty stats");
}

#[tokio::test]
async fn scoped_activity_searches_past_the_initial_candidate_window() {
    let (db, _temp) = test_db().await;
    db.upsert_documents(vec![
        activity_memory("work-owner", "work"),
        activity_memory("personal-owner", "personal"),
    ])
    .await
    .unwrap();
    let conn = db.conn.lock().await;
    conn.execute_batch(
        "WITH RECURSIVE seq(value) AS (
             SELECT 1 UNION ALL SELECT value + 1 FROM seq WHERE value < 101
         )
         INSERT INTO agent_activity
             (timestamp, agent_name, action, memory_ids, query, detail)
         SELECT 1000 + value, 'codex', 'search', 'personal-owner',
                'personal-query', 'personal-detail'
           FROM seq;
         INSERT INTO agent_activity
             (timestamp, agent_name, action, memory_ids, query, detail)
         VALUES (1, 'codex', 'search', 'work-owner', 'work-query', 'work-detail');",
    )
    .await
    .unwrap();
    drop(conn);
    let scope = ReadScope::Space("work".to_string());

    let retrievals = db.list_recent_retrievals_scoped(1, &scope).await.unwrap();
    let activities = db
        .list_agent_activity_scoped(1, None, None, &scope)
        .await
        .unwrap();

    assert_eq!(retrievals.len(), 1);
    assert_eq!(retrievals[0].query.as_deref(), Some("work-query"));
    assert_eq!(activities.len(), 1);
    assert_eq!(activities[0].query.as_deref(), Some("work-query"));
}

#[tokio::test]
async fn malformed_activity_owner_is_a_controlled_error() {
    let (db, _temp) = test_db().await;
    db.upsert_documents(vec![activity_memory("corrupt-owner", "work")])
        .await
        .unwrap();
    let conn = db.conn.lock().await;
    conn.execute(
        "UPDATE memories SET space = x'FF' WHERE source_id = 'corrupt-owner'",
        (),
    )
    .await
    .unwrap();
    conn.execute(
        "INSERT INTO agent_activity
             (timestamp, agent_name, action, memory_ids, query, detail)
         VALUES (1, 'codex', 'search', 'corrupt-owner', 'corrupt-query', 'corrupt-detail')",
        (),
    )
    .await
    .unwrap();
    drop(conn);

    let result = db
        .list_agent_activity_scoped(1, None, None, &ReadScope::Space("work".to_string()))
        .await;

    assert!(result.is_err(), "malformed owner storage must fail safely");
}

#[tokio::test]
async fn malformed_page_activity_owner_fails_both_scoped_feeds_safely() {
    let (db, _temp) = test_db().await;
    let now = chrono::Utc::now().to_rfc3339();
    db.insert_page_with_kind(
        "corrupt-page-owner",
        "Corrupt page owner",
        None,
        "page activity owner canary",
        None,
        None,
        &[],
        &now,
        "authored",
        "confirmed",
        Some("work"),
        None,
    )
    .await
    .unwrap();
    let conn = db.conn.lock().await;
    conn.execute(
        "UPDATE pages SET workspace = x'FF' WHERE id = 'corrupt-page-owner'",
        (),
    )
    .await
    .unwrap();
    conn.execute(
        "INSERT INTO agent_activity
             (timestamp, agent_name, action, memory_ids, query, detail)
         VALUES (1, 'codex', 'search', 'corrupt-page-owner', 'corrupt-query', 'corrupt-detail')",
        (),
    )
    .await
    .unwrap();
    drop(conn);
    let scope = ReadScope::Space("work".to_string());

    let retrievals = db.list_recent_retrievals_scoped(1, &scope).await;
    let activities = db.list_agent_activity_scoped(1, None, None, &scope).await;

    assert!(retrievals.is_err(), "retrieval feed must fail safely");
    assert!(activities.is_err(), "activity feed must fail safely");
}

#[tokio::test]
async fn memory_item_readers_fold_unfiled_sentinel_to_none() {
    let (db, _temp) = test_db().await;
    let mut unfiled = activity_memory("unfiled-owner", "work");
    unfiled.source_id = "unfiled-memory".to_string();
    unfiled.space = None;
    db.upsert_documents(vec![unfiled, activity_memory("work-memory", "work")])
        .await
        .unwrap();

    let listed = db
        .list_memories_scoped(&ReadScope::Global, None, None, None, 10)
        .await
        .unwrap();
    let unfiled_listed = listed
        .iter()
        .find(|item| item.source_id == "unfiled-memory")
        .expect("unfiled memory listed");
    assert_eq!(unfiled_listed.space, None);
    let work_listed = listed
        .iter()
        .find(|item| item.source_id == "work-memory")
        .expect("named-space memory listed");
    assert_eq!(work_listed.space.as_deref(), Some("work"));

    let unfiled_detail = db
        .get_memory_detail("unfiled-memory")
        .await
        .unwrap()
        .expect("unfiled memory detail");
    assert_eq!(unfiled_detail.space, None);
    let work_detail = db
        .get_memory_detail("work-memory")
        .await
        .unwrap()
        .expect("named-space memory detail");
    assert_eq!(work_detail.space.as_deref(), Some("work"));
}

#[tokio::test]
async fn indexed_file_readers_fold_unfiled_sentinel_to_none() {
    let (db, _temp) = test_db().await;
    let mut unfiled = activity_memory("unfiled-owner", "work");
    unfiled.source_id = "unfiled-file".to_string();
    unfiled.space = None;
    db.upsert_documents(vec![unfiled, activity_memory("work-file", "work")])
        .await
        .unwrap();

    for listed in [
        db.list_indexed_files_scoped(&ReadScope::Global)
            .await
            .unwrap(),
        db.list_filtered_confirmed_scoped(None, None, &ReadScope::Global, None, 10)
            .await
            .unwrap(),
    ] {
        let unfiled_listed = listed
            .iter()
            .find(|item| item.source_id == "unfiled-file")
            .expect("unfiled file listed");
        assert_eq!(unfiled_listed.space, None);
        let work_listed = listed
            .iter()
            .find(|item| item.source_id == "work-file")
            .expect("named-space file listed");
        assert_eq!(work_listed.space.as_deref(), Some("work"));
    }
}

#[tokio::test]
async fn filtered_file_scopes_partition_unfiled_and_named_spaces() {
    let (db, _temp) = test_db().await;
    let mut unfiled = activity_memory("unfiled-owner", "work");
    unfiled.source_id = "unfiled-scoped".to_string();
    unfiled.space = None;
    db.upsert_documents(vec![
        unfiled,
        activity_memory("work-scoped", "work"),
        activity_memory("personal-scoped", "personal"),
    ])
    .await
    .unwrap();

    let work_scope = ReadScope::Space("work".to_string());
    for work in [
        db.list_filtered_confirmed_scoped(None, None, &work_scope, None, 10)
            .await
            .unwrap(),
        db.list_indexed_files_scoped(&work_scope).await.unwrap(),
    ] {
        assert_eq!(work.len(), 1);
        assert_eq!(work[0].source_id, "work-scoped");
        assert_eq!(work[0].space.as_deref(), Some("work"));
    }

    for uncategorized in [
        db.list_filtered_confirmed_scoped(None, None, &ReadScope::Uncategorized, None, 10)
            .await
            .unwrap(),
        db.list_indexed_files_scoped(&ReadScope::Uncategorized)
            .await
            .unwrap(),
    ] {
        assert_eq!(uncategorized.len(), 1);
        assert_eq!(uncategorized[0].source_id, "unfiled-scoped");
        assert_eq!(uncategorized[0].space, None);
    }
}

#[tokio::test]
async fn indexed_inventory_keeps_cross_kind_metadata_within_read_scope() {
    let (db, _temp) = test_db().await;
    let conn = db.conn.lock().await;
    for (id, source, title, summary, url, space) in [
        (
            "agent-collision",
            "memory",
            "ZZ Agent note",
            "ZZ Agent summary",
            "https://z.example/private",
            "work",
        ),
        (
            "web-collision",
            "webpage",
            "AA Saved article",
            "AA Web summary",
            "https://a.example/article",
            "work",
        ),
        (
            "personal-web",
            "webpage",
            "Personal article",
            "Personal summary",
            "https://personal.example",
            "personal",
        ),
    ] {
        conn.execute(
            "INSERT INTO memories (id, content, source, source_id, title, summary, url,
                chunk_index, last_modified, chunk_type, space)
             VALUES (?1, ?1, ?2, 'shared-url', ?3, ?4, ?5, 0, 1, 'text', ?6)",
            libsql::params![id, source, title, summary, url, space],
        )
        .await
        .unwrap();
    }
    drop(conn);
    let files = db
        .list_indexed_files_scoped(&ReadScope::Space("work".into()))
        .await
        .unwrap();
    assert_eq!(files.len(), 2);
    let web = files.iter().find(|file| file.source == "webpage").unwrap();
    assert_eq!(web.source_id, "shared-url");
    assert_eq!(web.title, "AA Saved article");
    assert_eq!(web.summary.as_deref(), Some("AA Web summary"));
    assert_eq!(web.url.as_deref(), Some("https://a.example/article"));
    assert_eq!(web.chunk_count, 1);
    let memory = files.iter().find(|file| file.source == "memory").unwrap();
    assert_eq!(memory.title, "ZZ Agent note");
    assert_eq!(memory.summary.as_deref(), Some("ZZ Agent summary"));
    assert_eq!(memory.url.as_deref(), Some("https://z.example/private"));
    let chunks = db
        .get_webpage_chunks_scoped("shared-url", &ReadScope::Space("work".into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(chunks[0].content, "web-collision");
    let personal = db
        .list_indexed_files_scoped(&ReadScope::Space("personal".into()))
        .await
        .unwrap();
    assert_eq!(personal.len(), 1);
    assert_eq!(personal[0].summary.as_deref(), Some("Personal summary"));
    assert!(db
        .list_indexed_files_scoped(&ReadScope::Uncategorized)
        .await
        .unwrap()
        .is_empty());
}
