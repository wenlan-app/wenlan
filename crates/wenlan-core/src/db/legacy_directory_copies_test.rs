use super::*;
use crate::db::tests::test_db;
use crate::read_scope::ReadScope;
use crate::sources::RawDocument;
use std::collections::HashMap;

const LEGACY_PATH: &str = "/tmp/cedar-project/decision.md";
const SOURCE_ID: &str = "directory-cedar";
const CANONICAL_ID: &str = "directory-cedar::/tmp/cedar-project/decision.md";
const OTHER_PATH: &str = "/tmp/another-project/decision.md";
const HASH: &str = "sha256-cedar-decision-v2";
const SPACE: &str = "00000000-0000-4000-8000-000000000001";

fn legacy_doc(path: &str, content: &str) -> RawDocument {
    RawDocument {
        source: "local_files".into(),
        source_id: path.into(),
        title: "decision.md".into(),
        content: content.into(),
        url: Some(format!("file://{path}")),
        last_modified: 1,
        metadata: HashMap::new(),
        source_agent: None,
        memory_type: None,
        confirmed: None,
        pending_revision: false,
        supersedes: None,
        space: Some(SPACE.into()),
        ..Default::default()
    }
}

fn multi_chunk_legacy_content() -> String {
    format!(
        "The Cedar project chose the amber kestrel marker for the next client review.\n{}",
        "Additional generated file context remains in this older copy. ".repeat(1800)
    )
}

fn canonical_doc(source_id: &str, content: &str, hash: &str, space: &str) -> RawDocument {
    RawDocument {
        source: "memory".into(),
        source_id: source_id.into(),
        title: "Decision".into(),
        content: content.into(),
        last_modified: 2,
        metadata: HashMap::new(),
        source_agent: Some("folder".into()),
        memory_type: None,
        confirmed: Some(false),
        pending_revision: false,
        supersedes: None,
        content_hash: Some(hash.into()),
        space: Some(space.into()),
        ..Default::default()
    }
}

async fn searchable_source_ids(db: &MemoryDB, query: &str) -> Vec<String> {
    db.search_memory(query, 20, None, &ReadScope::Global, None, None, None, None)
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.source_id)
        .collect()
}

async fn generally_searchable_source_ids(db: &MemoryDB, query: &str) -> Vec<String> {
    db.search(query, 20, None, &ReadScope::Global)
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.source_id)
        .collect()
}

async fn row_count(db: &MemoryDB, source: &str, source_id: &str) -> i64 {
    let conn = db.conn.lock().await;
    let mut rows = conn
        .query(
            "SELECT COUNT(*) FROM memories WHERE source = ?1 AND source_id = ?2",
            libsql::params![source, source_id],
        )
        .await
        .unwrap();
    rows.next().await.unwrap().unwrap().get(0).unwrap()
}

async fn document_version(db: &MemoryDB, source: &str, source_id: &str) -> i64 {
    let conn = db.conn.lock().await;
    let mut rows = conn
        .query(
            "SELECT MAX(version) FROM memories WHERE source = ?1 AND source_id = ?2",
            libsql::params![source, source_id],
        )
        .await
        .unwrap();
    rows.next().await.unwrap().unwrap().get(0).unwrap()
}

#[tokio::test]
async fn retirement_removes_old_copy_from_search_and_preserves_its_rows() {
    let (db, _dir) = test_db().await;
    let old = multi_chunk_legacy_content();
    let current = "The Cedar project chose the cobalt kestrel marker for the next client review.";
    db.upsert_documents(vec![
        legacy_doc(LEGACY_PATH, &old),
        canonical_doc(CANONICAL_ID, current, HASH, SPACE),
    ])
    .await
    .unwrap();
    // Startup may refresh an untouched connector copy more than once before
    // the canonical folder generation is ready. Version alone is not proof of
    // user editing; the structured user-change guards below remain authoritative.
    for _ in 0..2 {
        db.upsert_documents(vec![legacy_doc(LEGACY_PATH, &old)])
            .await
            .unwrap();
    }
    assert_eq!(document_version(&db, "local_files", LEGACY_PATH).await, 3);

    let before = searchable_source_ids(&db, "Cedar kestrel marker").await;
    assert!(
        before.iter().any(|id| id == LEGACY_PATH),
        "old copy should be searchable before retirement: {before:?}"
    );
    assert!(
        before.iter().any(|id| id == CANONICAL_ID),
        "current copy should be searchable before retirement: {before:?}"
    );
    let general_before = generally_searchable_source_ids(&db, "Cedar kestrel marker").await;
    assert!(
        general_before.iter().any(|id| id == LEGACY_PATH),
        "old copy should appear in general search before retirement: {general_before:?}"
    );
    assert!(
        general_before.iter().any(|id| id == CANONICAL_ID),
        "current copy should appear in general search before retirement: {general_before:?}"
    );

    let stored_legacy_chunks = row_count(&db, "local_files", LEGACY_PATH).await;
    assert!(
        stored_legacy_chunks > 1,
        "fixture must exercise a multi-chunk legacy file"
    );
    let changed = db
        .retire_legacy_directory_copies(
            LEGACY_PATH,
            SOURCE_ID,
            std::path::Path::new(LEGACY_PATH),
            None,
            HASH,
            Some(SPACE),
        )
        .await
        .unwrap();
    assert_eq!(
        changed as i64, stored_legacy_chunks,
        "the entire eligible legacy document is retired"
    );

    let after = searchable_source_ids(&db, "Cedar kestrel marker").await;
    assert!(
        !after.iter().any(|id| id == LEGACY_PATH),
        "old copy must no longer be searchable: {after:?}"
    );
    assert!(
        after.iter().any(|id| id == CANONICAL_ID),
        "canonical copy remains searchable: {after:?}"
    );
    let general_after = generally_searchable_source_ids(&db, "Cedar kestrel marker").await;
    assert!(
        !general_after.iter().any(|id| id == LEGACY_PATH),
        "retired old copy must not leak into general search: {general_after:?}"
    );
    assert!(
        general_after.iter().any(|id| id == CANONICAL_ID),
        "canonical copy remains in general search: {general_after:?}"
    );
    assert_eq!(
        row_count(&db, "local_files", LEGACY_PATH).await,
        stored_legacy_chunks,
        "legacy content and IDs remain stored"
    );
    assert_eq!(row_count(&db, "memory", CANONICAL_ID).await, 1);

    let again = db
        .retire_legacy_directory_copies(
            LEGACY_PATH,
            SOURCE_ID,
            std::path::Path::new(LEGACY_PATH),
            None,
            HASH,
            Some(SPACE),
        )
        .await
        .unwrap();
    assert_eq!(again, 0, "retirement is idempotent");
}

#[tokio::test]
async fn retirement_requires_matching_committed_canonical_generation_and_whole_generated_group() {
    let (db, _dir) = test_db().await;
    db.upsert_documents(vec![legacy_doc(
        LEGACY_PATH,
        "The Cedar project chose the amber kestrel marker.",
    )])
    .await
    .unwrap();

    assert_eq!(
        db.retire_legacy_directory_copies(
            LEGACY_PATH,
            SOURCE_ID,
            std::path::Path::new(LEGACY_PATH),
            None,
            HASH,
            Some(SPACE)
        )
        .await
        .unwrap(),
        0,
        "an absent canonical generation cannot retire the old copy"
    );

    db.upsert_documents(vec![canonical_doc(
        CANONICAL_ID,
        "The Cedar project chose the cobalt kestrel marker.",
        "different-hash",
        SPACE,
    )])
    .await
    .unwrap();
    assert_eq!(
        db.retire_legacy_directory_copies(
            LEGACY_PATH,
            SOURCE_ID,
            std::path::Path::new(LEGACY_PATH),
            None,
            HASH,
            Some(SPACE)
        )
        .await
        .unwrap(),
        0,
        "a stale expected hash cannot retire the old copy"
    );

    // A manually confirmed/pinned legacy group is treated as user-owned. The
    // whole group remains live rather than partially retiring the untouched
    // chunk beside the protected one.
    db.upsert_documents(vec![canonical_doc(
        CANONICAL_ID,
        "The Cedar project chose the cobalt kestrel marker.",
        HASH,
        SPACE,
    )])
    .await
    .unwrap();
    {
        let conn = db.conn.lock().await;
        conn.execute(
            "UPDATE memories SET pinned = 1 WHERE source = 'local_files' AND source_id = ?1 AND chunk_index = 0",
            libsql::params![LEGACY_PATH],
        )
        .await
        .unwrap();
    }
    assert_eq!(
        db.retire_legacy_directory_copies(
            LEGACY_PATH,
            SOURCE_ID,
            std::path::Path::new(LEGACY_PATH),
            None,
            HASH,
            Some(SPACE)
        )
        .await
        .unwrap(),
        0,
        "one user-owned chunk prevents partial retirement of its document"
    );
    let after = searchable_source_ids(&db, "Cedar kestrel marker").await;
    assert!(after.iter().any(|id| id == LEGACY_PATH));
}

#[tokio::test]
async fn manual_changelog_prevents_legacy_retirement_even_after_repeated_generated_imports() {
    let (db, _dir) = test_db().await;
    let content = "The Cedar project chose the amber kestrel marker.";
    db.upsert_documents(vec![
        legacy_doc(LEGACY_PATH, content),
        canonical_doc(
            CANONICAL_ID,
            "The Cedar project chose the cobalt kestrel marker.",
            HASH,
            SPACE,
        ),
    ])
    .await
    .unwrap();
    for _ in 0..2 {
        db.upsert_documents(vec![legacy_doc(LEGACY_PATH, content)])
            .await
            .unwrap();
    }
    {
        let conn = db.conn.lock().await;
        conn.execute(
            "UPDATE memories SET changelog = '[{\"note\":\"manual edit\"}]' \
             WHERE source = 'local_files' AND source_id = ?1 AND chunk_index = 0",
            libsql::params![LEGACY_PATH],
        )
        .await
        .unwrap();
    }

    assert_eq!(
        db.retire_legacy_directory_copies(
            LEGACY_PATH,
            SOURCE_ID,
            Path::new(LEGACY_PATH),
            None,
            HASH,
            Some(SPACE),
        )
        .await
        .unwrap(),
        0,
        "manually edited history remains searchable regardless of generated version"
    );
    assert!(searchable_source_ids(&db, "Cedar kestrel marker")
        .await
        .iter()
        .any(|id| id == LEGACY_PATH));
}

#[tokio::test]
async fn actual_content_edit_receipt_prevents_legacy_retirement() {
    let (db, _dir) = test_db().await;
    db.upsert_documents(vec![
        legacy_doc(
            LEGACY_PATH,
            "The Cedar project chose the amber kestrel marker for review.",
        ),
        canonical_doc(
            CANONICAL_ID,
            "The Cedar project chose the cobalt kestrel marker for review.",
            HASH,
            SPACE,
        ),
    ])
    .await
    .unwrap();

    {
        let conn = db.test_primary_session().await;
        conn.execute(
            "INSERT INTO enrichment_origin
                 (source_id, memory_type_explicit, structured_fields_explicit,
                  space_rejected, service_class)
             VALUES (?1, 0, 0, 0, 1)",
            [LEGACY_PATH],
        )
        .await
        .unwrap();
    }

    db.update_memory(
        LEGACY_PATH,
        "The Cedar project chose the user-edited garnet kestrel marker for review.",
    )
    .await
    .unwrap();

    {
        let conn = db.test_primary_session().await;
        let mut rows = conn
            .query(
                "SELECT memory_type_explicit, structured_fields_explicit,
                        space_rejected, service_class
                 FROM enrichment_origin WHERE source_id = ?1",
                [LEGACY_PATH],
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().expect("origin receipt");
        assert_eq!(row.get::<i64>(0).unwrap(), 0);
        assert_eq!(row.get::<i64>(1).unwrap(), 0);
        assert_eq!(row.get::<i64>(2).unwrap(), 0);
        assert_eq!(row.get::<i64>(3).unwrap(), 0);
    }

    assert_eq!(
        db.retire_legacy_directory_copies(
            LEGACY_PATH,
            SOURCE_ID,
            std::path::Path::new(LEGACY_PATH),
            None,
            HASH,
            Some(SPACE),
        )
        .await
        .unwrap(),
        0,
        "the actual content update's explicit edit receipt protects the row"
    );
    assert!(searchable_source_ids(&db, "garnet kestrel marker")
        .await
        .iter()
        .any(|id| id == LEGACY_PATH));
    assert!(
        generally_searchable_source_ids(&db, "garnet kestrel marker")
            .await
            .iter()
            .any(|id| id == LEGACY_PATH)
    );
}

#[tokio::test]
async fn same_basename_with_different_path_or_space_does_not_authorize_retirement() {
    let (db, _dir) = test_db().await;
    db.upsert_documents(vec![
        legacy_doc(
            LEGACY_PATH,
            "The Cedar project chose the amber kestrel marker.",
        ),
        canonical_doc(
            "directory-cedar::/tmp/another-project/decision.md",
            "The Cedar project chose cobalt kestrel marker.",
            HASH,
            SPACE,
        ),
    ])
    .await
    .unwrap();
    assert_eq!(
        db.retire_legacy_directory_copies(
            LEGACY_PATH,
            SOURCE_ID,
            std::path::Path::new(OTHER_PATH),
            None,
            HASH,
            Some(SPACE)
        )
        .await
        .unwrap(),
        0,
        "same basename cannot bridge distinct provenance paths"
    );
    assert_eq!(
        db.retire_legacy_directory_copies(
            LEGACY_PATH,
            SOURCE_ID,
            std::path::Path::new(LEGACY_PATH),
            None,
            HASH,
            Some("different-space")
        )
        .await
        .unwrap(),
        0,
        "space mismatch cannot retire"
    );
    assert!(searchable_source_ids(&db, "Cedar kestrel marker")
        .await
        .iter()
        .any(|id| id == LEGACY_PATH));
}

#[tokio::test]
async fn inactive_canonical_generation_does_not_retire_legacy_history() {
    let (db, _dir) = test_db().await;
    db.upsert_documents(vec![
        legacy_doc(
            LEGACY_PATH,
            "The Cedar project chose the amber kestrel marker.",
        ),
        canonical_doc(
            CANONICAL_ID,
            "The Cedar project chose the cobalt kestrel marker.",
            HASH,
            SPACE,
        ),
        RawDocument {
            source: "memory".into(),
            source_id: "manual-newer-decision".into(),
            title: "Newer decision".into(),
            content: "The decision about the source is now replaced.".into(),
            last_modified: 3,
            metadata: HashMap::new(),
            source_agent: Some("claude".into()),
            memory_type: Some("fact".into()),
            confirmed: Some(false),
            pending_revision: false,
            supersedes: Some(CANONICAL_ID.into()),
            supersede_mode: "hide".into(),
            space: Some(SPACE.into()),
            ..Default::default()
        },
    ])
    .await
    .unwrap();

    assert_eq!(
        db.retire_legacy_directory_copies(
            LEGACY_PATH,
            SOURCE_ID,
            std::path::Path::new(LEGACY_PATH),
            None,
            HASH,
            Some(SPACE),
        )
        .await
        .unwrap(),
        0,
        "a superseded canonical generation cannot retire the old source"
    );
    assert!(
        searchable_source_ids(&db, "Cedar kestrel marker")
            .await
            .iter()
            .any(|id| id == LEGACY_PATH),
        "the old copy remains available when the canonical row is inactive"
    );
}
