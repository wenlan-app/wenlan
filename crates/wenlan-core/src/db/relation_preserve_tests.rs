// SPDX-License-Identifier: Apache-2.0
//! Regression tests for relation writes that must preserve existing graph facts.

use super::{tests::test_db, MemoryDB, WenlanError};
use crate::sources::RawDocument;

async fn fixture(db: &MemoryDB) -> (String, String, String, String) {
    let from = db
        .create_entity("Preserve From", "concept", Some("space-preserve"))
        .await
        .unwrap();
    let to = db
        .create_entity("Preserve To", "concept", Some("space-preserve"))
        .await
        .unwrap();
    let canonicals = db.relation_canonicals().await.unwrap();
    let first = canonicals
        .iter()
        .find(|value| value.as_str() == "related_to")
        .cloned()
        .unwrap_or_else(|| canonicals[0].clone());
    let second = canonicals
        .iter()
        .find(|value| *value != &first)
        .unwrap()
        .clone();
    (from, to, first, second)
}

#[allow(clippy::too_many_arguments)]
async fn create_preserving(
    db: &MemoryDB,
    from: &str,
    to: &str,
    relation_type: &str,
    source_agent: Option<&str>,
    confidence: Option<f64>,
    explanation: Option<&str>,
    source_memory_id: Option<&str>,
) -> Result<(String, bool), WenlanError> {
    db.create_relation_with_span_preserving(
        from,
        to,
        relation_type,
        source_agent,
        confidence,
        explanation,
        source_memory_id,
        None,
        None,
        None,
        None,
    )
    .await
}

async fn edge_payload(db: &MemoryDB, edge_id: &str) -> Option<String> {
    let guard = db.conn.lock().await;
    let mut rows = guard
        .query(
            "SELECT payload FROM edges WHERE edge_id = ?1",
            libsql::params![edge_id],
        )
        .await
        .unwrap();
    rows.next()
        .await
        .unwrap()
        .and_then(|row| row.get::<Option<String>>(0).unwrap_or(None))
}

async fn seed_source_memory(db: &MemoryDB, source_id: &str, pending_revision: bool) {
    db.upsert_documents_without_embeddings(vec![RawDocument {
        source: "memory".to_string(),
        source_id: source_id.to_string(),
        title: format!("source {source_id}"),
        content: format!("Current relation evidence from {source_id}."),
        memory_type: Some("fact".to_string()),
        pending_revision,
        ..Default::default()
    }])
    .await
    .unwrap();
}

#[tokio::test]
async fn preserve_create_rejects_different_active_type_without_writing() {
    let (db, _tmp) = test_db().await;
    let (from, to, original_type, requested_type) = fixture(&db).await;
    let (original_id, created) = create_preserving(
        &db,
        &from,
        &to,
        &original_type,
        Some("first-writer"),
        Some(0.6),
        Some("original explanation"),
        None,
    )
    .await
    .unwrap();
    assert!(created);

    let error = create_preserving(
        &db,
        &from,
        &to,
        &requested_type,
        Some("organizer"),
        Some(1.0),
        Some("must not replace the original"),
        None,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(error, WenlanError::Conflict(message) if message.starts_with("relation_changed:"))
    );

    let active = db.list_relations_between(&from, &to).await.unwrap();
    assert_eq!(active, vec![(original_id, original_type)]);
}

#[tokio::test]
async fn preserve_same_type_retry_does_not_mutate_existing_evidence() {
    let (db, _tmp) = test_db().await;
    let (from, to, relation_type, _) = fixture(&db).await;
    seed_source_memory(&db, "source-original", false).await;
    seed_source_memory(&db, "source-replacement", false).await;
    let (edge_id, created) = create_preserving(
        &db,
        &from,
        &to,
        &relation_type,
        Some("first-writer"),
        Some(0.6),
        Some("original explanation"),
        Some("source-original"),
    )
    .await
    .unwrap();
    assert!(created);
    let before = edge_payload(&db, &edge_id).await;

    let (retry_id, retry_created) = create_preserving(
        &db,
        &from,
        &to,
        &relation_type,
        Some("later-writer"),
        Some(1.0),
        Some("replacement explanation"),
        Some("source-replacement"),
    )
    .await
    .unwrap();

    assert_eq!(retry_id, edge_id);
    assert!(!retry_created);
    assert_eq!(edge_payload(&db, &edge_id).await, before);
}

#[tokio::test]
async fn concurrent_preserving_conflicting_creates_leave_one_active_relation() {
    let (db, _tmp) = test_db().await;
    let (from, to, first_type, second_type) = fixture(&db).await;
    let (left, right) = tokio::join!(
        create_preserving(
            &db,
            &from,
            &to,
            &first_type,
            Some("writer-a"),
            None,
            Some("first candidate"),
            None,
        ),
        create_preserving(
            &db,
            &from,
            &to,
            &second_type,
            Some("writer-b"),
            None,
            Some("second candidate"),
            None,
        ),
    );

    assert_ne!(left.is_ok(), right.is_ok());
    let rejected = if let Err(error) = left {
        error
    } else {
        right.unwrap_err()
    };
    assert!(
        matches!(rejected, WenlanError::Conflict(message) if message.starts_with("relation_changed:"))
    );
    let active = db.list_relations_between(&from, &to).await.unwrap();
    assert_eq!(active.len(), 1);
}

#[tokio::test]
async fn preserve_create_does_not_resurrect_retired_same_type_edge() {
    let (db, _tmp) = test_db().await;
    let (from, to, relation_type, _) = fixture(&db).await;
    let (edge_id, _) = create_preserving(
        &db,
        &from,
        &to,
        &relation_type,
        Some("first-writer"),
        None,
        Some("retired fact"),
        None,
    )
    .await
    .unwrap();
    db.supersede_relation(&edge_id, "replacement-edge")
        .await
        .unwrap();

    let error = create_preserving(
        &db,
        &from,
        &to,
        &relation_type,
        Some("organizer"),
        Some(1.0),
        Some("must not resurrect"),
        None,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(error, WenlanError::Conflict(message) if message.starts_with("relation_changed:"))
    );
    assert!(db
        .list_relations_between(&from, &to)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn preserve_create_rejects_superseded_source_without_writing_edge() {
    let (db, _tmp) = test_db().await;
    let (from, to, relation_type, _) = fixture(&db).await;
    seed_source_memory(&db, "source-superseded", false).await;
    db.upsert_documents_without_embeddings(vec![RawDocument {
        source: "memory".to_string(),
        source_id: "source-superseder".to_string(),
        title: "Replacement source".to_string(),
        content: "The source evidence has been replaced.".to_string(),
        memory_type: Some("decision".to_string()),
        supersede_mode: "archive".to_string(),
        supersedes: Some("source-superseded".to_string()),
        ..Default::default()
    }])
    .await
    .unwrap();

    let error = create_preserving(
        &db,
        &from,
        &to,
        &relation_type,
        Some("organizer"),
        None,
        Some("stale source must not assert"),
        Some("source-superseded"),
    )
    .await
    .unwrap_err();

    assert!(
        matches!(error, WenlanError::Conflict(message) if message.starts_with("relation_source_changed:"))
    );
    assert!(db
        .list_relations_between(&from, &to)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn preserve_create_rejects_hide_superseded_source_without_writing_edge() {
    let (db, _tmp) = test_db().await;
    let (from, to, relation_type, _) = fixture(&db).await;
    seed_source_memory(&db, "source-hide-superseded", false).await;
    db.upsert_documents_without_embeddings(vec![RawDocument {
        source: "memory".to_string(),
        source_id: "source-hide-superseder".to_string(),
        title: "Replacement fact".to_string(),
        content: "The source fact was replaced.".to_string(),
        memory_type: Some("fact".to_string()),
        supersede_mode: "hide".to_string(),
        supersedes: Some("source-hide-superseded".to_string()),
        ..Default::default()
    }])
    .await
    .unwrap();

    let error = create_preserving(
        &db,
        &from,
        &to,
        &relation_type,
        Some("organizer"),
        None,
        Some("hidden source must not assert"),
        Some("source-hide-superseded"),
    )
    .await
    .unwrap_err();

    assert!(
        matches!(error, WenlanError::Conflict(message) if message.starts_with("relation_source_changed:"))
    );
    assert!(db
        .list_relations_between(&from, &to)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn preserve_create_rejects_pending_source_without_writing_edge() {
    let (db, _tmp) = test_db().await;
    let (from, to, relation_type, _) = fixture(&db).await;
    seed_source_memory(&db, "source-pending", true).await;

    let error = create_preserving(
        &db,
        &from,
        &to,
        &relation_type,
        Some("organizer"),
        None,
        Some("pending source must not assert"),
        Some("source-pending"),
    )
    .await
    .unwrap_err();

    assert!(
        matches!(error, WenlanError::Conflict(message) if message.starts_with("relation_source_changed:"))
    );
    assert!(db
        .list_relations_between(&from, &to)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn preserve_create_rejects_evicted_source_without_writing_edge() {
    let (db, _tmp) = test_db().await;
    let (from, to, relation_type, _) = fixture(&db).await;
    seed_source_memory(&db, "source-evicted", false).await;
    {
        let conn = db.conn.lock().await;
        conn.execute(
            "UPDATE memories SET supersede_mode = 'evicted' WHERE source = 'memory' AND source_id = ?1",
            libsql::params!["source-evicted"],
        )
        .await
        .unwrap();
    }

    let error = create_preserving(
        &db,
        &from,
        &to,
        &relation_type,
        Some("organizer"),
        None,
        Some("evicted source must not assert"),
        Some("source-evicted"),
    )
    .await
    .unwrap_err();

    assert!(
        matches!(error, WenlanError::Conflict(message) if message.starts_with("relation_source_changed:"))
    );
    assert!(db
        .list_relations_between(&from, &to)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn preserve_create_rejects_unknown_type_without_queueing_vocabulary_proposal() {
    let (db, _tmp) = test_db().await;
    let (from, to, _, _) = fixture(&db).await;

    let error = create_preserving(
        &db,
        &from,
        &to,
        "unrecognized_predicate",
        Some("organizer"),
        None,
        Some("unknown predicate must not be coerced"),
        None,
    )
    .await
    .unwrap_err();

    assert!(
        matches!(error, WenlanError::Validation(message) if message.contains("unknown relation type"))
    );
    assert!(db
        .list_relations_between(&from, &to)
        .await
        .unwrap()
        .is_empty());
    let proposals = db.get_pending_refinements().await.unwrap();
    assert!(!proposals
        .iter()
        .any(|proposal| proposal.action == "vocab_promote"));
}
