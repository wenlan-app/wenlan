// SPDX-License-Identifier: Apache-2.0
//! UX-GRAPH-9: a topic name that only looks like code or a sentence is not
//! made a topic by an automatic write, and an alias a vector merge added is
//! recorded as a guess: it is never an exact match, a person can detach it,
//! and a detached name is never merged back.

use super::entity_alias_sources::source;
use crate::db::tests::{test_db, test_db_at};
use crate::db::MemoryDB;
use crate::error::WenlanError;
use crate::read_scope::ReadScope;

/// A stored topic and a near-identical mention of it: the pair the vector
/// step merges (cosine distance under 0.1, high-entropy name).
const TOPIC: &str = "Vorpalblade Jabberwock Project";
const NEAR: &str = "Vorpalblade-Jabberwock Project";

async fn scalar(db: &MemoryDB, sql: &str) -> i64 {
    let conn = db.conn.lock().await;
    let mut rows = conn.query(sql, ()).await.unwrap();
    rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap()
}

async fn aliases_of(db: &MemoryDB, entity_id: &str) -> Vec<String> {
    let conn = db.conn.lock().await;
    let mut rows = conn
        .query(
            "SELECT p.aliases FROM entity_page_map epm JOIN pages p ON p.id = epm.page_id
              WHERE epm.entity_id = ?1",
            libsql::params![entity_id],
        )
        .await
        .unwrap();
    let json = rows
        .next()
        .await
        .unwrap()
        .unwrap()
        .get::<Option<String>>(0)
        .unwrap();
    super::parse_pages_aliases(json)
}

async fn mention_type_of(db: &MemoryDB, entity_id: &str, alias: &str) -> Option<String> {
    let conn = db.conn.lock().await;
    let mut rows = conn
        .query(
            "SELECT mention_type FROM entity_alias_sources WHERE entity_id = ?1 AND alias = ?2",
            libsql::params![entity_id, alias],
        )
        .await
        .unwrap();
    rows.next()
        .await
        .unwrap()
        .and_then(|row| row.get::<Option<String>>(0).unwrap())
}

async fn insert_memory(db: &MemoryDB, source_id: &str) {
    db.conn
        .lock()
        .await
        .execute(
            "INSERT INTO memories
                 (id, content, source, source_id, title, chunk_index,
                  last_modified, chunk_type, pending_revision, version)
             VALUES (?1, 'alias source fixture', 'memory', ?2, '', 0, 1, 'text', 0, 1)",
            libsql::params![format!("c_{source_id}"), source_id],
        )
        .await
        .unwrap();
}

fn extracted(names: &[(&str, &str)]) -> Vec<crate::extract::KgExtractionResult> {
    vec![crate::extract::KgExtractionResult {
        index: 0,
        entities: names
            .iter()
            .map(|(name, entity_type)| crate::extract::ExtractedEntity {
                name: name.to_string(),
                entity_type: entity_type.to_string(),
            })
            .collect(),
        observations: Vec::new(),
        relations: Vec::new(),
    }]
}

/// The fixture pair really is inside the vector step's merge distance, so the
/// tests below exercise the vector merge and not some other resolution step.
async fn assert_pair_merges_by_vector(db: &MemoryDB, topic_id: &str) {
    let hit = db
        .search_entities_by_vector(NEAR, 1)
        .await
        .unwrap()
        .into_iter()
        .next()
        .expect("the stored topic is found by vector");
    assert_eq!(hit.entity.id, topic_id);
    assert!(
        hit.distance < 0.1,
        "fixture pair must embed inside the merge distance, got {}",
        hit.distance
    );
}

async fn vector_merged_topic(db: &MemoryDB) -> String {
    let (topic_id, created) = db
        .resolve_or_create_entity(TOPIC, "project", None, Some("test"), None)
        .await
        .unwrap();
    assert!(created);
    assert_pair_merges_by_vector(db, &topic_id).await;
    let (merged_id, created) = db
        .resolve_or_create_entity(NEAR, "project", None, Some("test"), None)
        .await
        .unwrap();
    assert_eq!(merged_id, topic_id, "the near mention merges by vector");
    assert!(!created);
    topic_id
}

#[tokio::test]
async fn ux_graph_9_migration_161_keeps_every_existing_alias_exact() {
    let (db, _dir) = test_db_at(160).await;
    let id = db
        .store_entity("Tumtum Tree Grove", "concept", None, Some("test"), None)
        .await
        .unwrap();
    db.add_entity_alias("tumtum grove", &id, "auto")
        .await
        .unwrap();

    db.run_migrations_up_to(&crate::events::NoopEmitter, 161)
        .await
        .unwrap();
    // Re-running the chain at the same version is a no-op.
    db.run_migrations_up_to(&crate::events::NoopEmitter, 161)
        .await
        .unwrap();

    assert_eq!(scalar(&db, "PRAGMA user_version").await, 161);
    assert_eq!(
        scalar(&db, "SELECT COUNT(*) FROM entity_alias_sources").await,
        0,
        "no alias written before the migration is marked"
    );
    assert_eq!(
        db.resolve_entity_by_alias("tumtum grove").await.unwrap(),
        Some(id.clone()),
        "an alias of unknown origin keeps resolving exactly"
    );
    assert_eq!(
        db.entity_alias_source(&id, "tumtum grove").await.unwrap(),
        None
    );
}

#[tokio::test]
async fn ux_graph_9_vector_merged_alias_is_auto_and_not_an_exact_match() {
    let (db, _dir) = test_db().await;
    let topic_id = vector_merged_topic(&db).await;
    let near = NEAR.to_lowercase();

    assert!(aliases_of(&db, &topic_id).await.contains(&near));
    assert_eq!(
        db.entity_alias_source(&topic_id, NEAR).await.unwrap(),
        Some(source::AUTO.to_string())
    );
    assert_eq!(
        mention_type_of(&db, &topic_id, &near).await,
        Some("project".to_string()),
        "the mention's own type is kept for lint"
    );
    assert_eq!(
        db.resolve_entity_by_alias(&near).await.unwrap(),
        None,
        "an auto alias is never an exact match"
    );
    assert!(
        db.entity_name_candidates(NEAR).await.unwrap().is_empty(),
        "the lookup that never guesses does not take an auto alias either"
    );
    // The topic's own name stays exact.
    assert_eq!(
        db.resolve_entity_by_alias(&TOPIC.to_lowercase())
            .await
            .unwrap(),
        Some(topic_id.clone())
    );
    assert_eq!(
        db.entity_alias_source(&topic_id, TOPIC).await.unwrap(),
        None
    );
}

#[tokio::test]
async fn ux_graph_9_detached_alias_is_removed_and_never_merged_back() {
    let (db, _dir) = test_db().await;
    let topic_id = vector_merged_topic(&db).await;
    let near = NEAR.to_lowercase();

    assert!(db.detach_entity_alias(&topic_id, NEAR).await.unwrap());
    assert!(!aliases_of(&db, &topic_id).await.contains(&near));
    assert_eq!(
        db.entity_alias_source(&topic_id, NEAR).await.unwrap(),
        Some(source::DETACHED.to_string())
    );
    assert!(
        !db.detach_entity_alias(&topic_id, NEAR).await.unwrap(),
        "an alias the topic no longer carries is a no-op"
    );
    assert!(matches!(
        db.detach_entity_alias(&topic_id, TOPIC).await,
        Err(WenlanError::Validation(_))
    ));
    assert!(matches!(
        db.detach_entity_alias("no-such-topic", NEAR).await,
        Err(WenlanError::NotFound(_))
    ));
    assert!(aliases_of(&db, &topic_id)
        .await
        .contains(&TOPIC.to_lowercase()));

    // The same mention again: the vector step still finds the topic, but the
    // detached name gets a topic of its own.
    let (again, created) = db
        .resolve_or_create_entity(NEAR, "project", None, Some("test"), None)
        .await
        .unwrap();
    assert_ne!(again, topic_id);
    assert!(created);
    assert!(!aliases_of(&db, &topic_id).await.contains(&near));
}

#[tokio::test]
async fn ux_graph_9_ambient_lane_records_auto_and_respects_detach() {
    let (db, _dir) = test_db().await;
    for id in ["mem_a", "mem_b", "mem_c"] {
        insert_memory(&db, id).await;
    }
    assert!(db
        .commit_entity_enrichment_at_version(
            "mem_a",
            1,
            &extracted(&[(TOPIC, "project")]),
            "alias source fixture"
        )
        .await
        .unwrap());
    let topic_id = db.list_entities(None, None).await.unwrap()[0].id.clone();
    assert_pair_merges_by_vector(&db, &topic_id).await;

    assert!(db
        .commit_entity_enrichment_at_version(
            "mem_b",
            1,
            &extracted(&[(NEAR, "person")]),
            "alias source fixture"
        )
        .await
        .unwrap());
    assert_eq!(db.list_entities(None, None).await.unwrap().len(), 1);
    assert_eq!(
        db.entity_alias_source(&topic_id, NEAR).await.unwrap(),
        Some(source::AUTO.to_string())
    );
    assert_eq!(
        mention_type_of(&db, &topic_id, &NEAR.to_lowercase()).await,
        Some("person".to_string())
    );
    assert_eq!(db.resolve_entity_by_alias(NEAR).await.unwrap(), None);

    assert!(db.detach_entity_alias(&topic_id, NEAR).await.unwrap());
    assert!(db
        .commit_entity_enrichment_at_version(
            "mem_c",
            1,
            &extracted(&[(NEAR, "project")]),
            "alias source fixture"
        )
        .await
        .unwrap());
    let entities = db.list_entities(None, None).await.unwrap();
    assert_eq!(entities.len(), 2, "a detached name gets its own topic");
    assert!(!aliases_of(&db, &topic_id)
        .await
        .contains(&NEAR.to_lowercase()));
}

#[tokio::test]
async fn ux_graph_9_ambient_lane_does_not_double_claim_an_auto_alias() {
    let (db, _dir) = test_db().await;
    let topic_id = db
        .store_entity(TOPIC, "project", None, Some("test"), None)
        .await
        .unwrap();
    assert_pair_merges_by_vector(&db, &topic_id).await;
    // Another topic already holds the near name as an auto alias.
    let other = db
        .store_entity("Tumtum Tree Grove", "concept", None, Some("test"), None)
        .await
        .unwrap();
    let near = NEAR.to_lowercase();
    db.add_entity_alias(&near, &other, "auto").await.unwrap();
    db.record_auto_alias(&other, &near, "concept")
        .await
        .unwrap();
    assert_eq!(
        db.entity_alias_source(&other, NEAR).await.unwrap(),
        Some(source::AUTO.to_string())
    );

    insert_memory(&db, "mem_d").await;
    assert!(db
        .commit_entity_enrichment_at_version(
            "mem_d",
            1,
            &extracted(&[(NEAR, "project")]),
            "alias source fixture"
        )
        .await
        .unwrap());
    assert_eq!(
        scalar(
            &db,
            &format!(
                "SELECT COUNT(*) FROM memory_entities
                  WHERE memory_id = 'mem_d' AND entity_id = '{topic_id}'"
            )
        )
        .await,
        1,
        "the other topic's auto alias is not an exact match; the vector step decides"
    );
    assert!(
        !aliases_of(&db, &topic_id).await.contains(&near),
        "a second topic must not claim a name another topic holds"
    );
    assert_eq!(db.entity_alias_source(&topic_id, NEAR).await.unwrap(), None);
}

#[tokio::test]
async fn ux_graph_9_declaring_an_auto_alias_makes_it_exact() {
    let (db, _dir) = test_db().await;
    let topic_id = vector_merged_topic(&db).await;

    let aliases = db
        .add_entity_alias_in_scope(&ReadScope::Global, &topic_id, NEAR)
        .await
        .unwrap();
    assert!(aliases.contains(&NEAR.to_lowercase()));
    assert_eq!(db.entity_alias_source(&topic_id, NEAR).await.unwrap(), None);
    assert_eq!(
        db.resolve_entity_by_alias(NEAR).await.unwrap(),
        Some(topic_id)
    );
}

#[tokio::test]
async fn ux_graph_9_merge_keeps_auto_aliases_auto_and_the_merged_name_exact() {
    let (db, _dir) = test_db().await;
    let canonical = db
        .store_entity("Tumtum Tree Grove", "concept", None, Some("test"), None)
        .await
        .unwrap();
    let loser = db
        .store_entity("Slithy Tove Burrow", "concept", None, Some("test"), None)
        .await
        .unwrap();
    db.add_entity_alias("borogove nest", &loser, "auto")
        .await
        .unwrap();
    db.record_auto_alias(&loser, "borogove nest", "concept")
        .await
        .unwrap();
    // A stray auto row for the loser's own name, so the merge has to clear it.
    db.conn
        .lock()
        .await
        .execute(
            "INSERT INTO entity_alias_sources (entity_id, alias, source, mention_type, created_at)
             VALUES (?1, 'slithy tove burrow', 'auto', 'concept', 1)",
            libsql::params![loser.as_str()],
        )
        .await
        .unwrap();

    db.merge_entities(&canonical, &loser).await.unwrap();

    assert_eq!(
        db.entity_alias_source(&canonical, "borogove nest")
            .await
            .unwrap(),
        Some(source::AUTO.to_string())
    );
    assert_eq!(
        db.resolve_entity_by_alias("borogove nest").await.unwrap(),
        None
    );
    assert_eq!(
        db.entity_alias_source(&canonical, "slithy tove burrow")
            .await
            .unwrap(),
        None,
        "the merged topic's own name is exact on the canonical"
    );
    assert_eq!(
        db.resolve_entity_by_alias("slithy tove burrow")
            .await
            .unwrap(),
        Some(canonical.clone())
    );
    assert_eq!(
        scalar(
            &db,
            &format!("SELECT COUNT(*) FROM entity_alias_sources WHERE entity_id = '{loser}'")
        )
        .await,
        0
    );
}

#[tokio::test]
async fn ux_graph_9_both_write_lanes_refuse_code_shaped_names_and_long_phrases() {
    let (db, _dir) = test_db().await;
    let refused = [
        "WENLAN_HOST",
        "#1157",
        "FixedSizeStrategy::MAX_CHARS",
        "config.reranker_mode",
        "--nocapture",
        "PR #12",
        "7xuanlu/wenlan#12",
        "daemon or CLI write paths",
    ];
    for name in refused {
        assert!(
            matches!(
                db.resolve_or_create_entity(name, "concept", None, Some("test"), None)
                    .await,
                Err(WenlanError::Validation(_))
            ),
            "{name} must be refused by resolve_or_create_entity"
        );
    }
    assert!(db.list_entities(None, None).await.unwrap().is_empty());

    insert_memory(&db, "mem_code").await;
    let mut names: Vec<(&str, &str)> = refused.iter().map(|name| (*name, "concept")).collect();
    names.push(("relay.wenlan.app", "concept"));
    assert!(db
        .commit_entity_enrichment_at_version(
            "mem_code",
            1,
            &extracted(&names),
            "alias source fixture"
        )
        .await
        .unwrap());
    let stored: Vec<String> = db
        .list_entities(None, None)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert_eq!(stored, vec!["relay.wenlan.app".to_string()]);
}
