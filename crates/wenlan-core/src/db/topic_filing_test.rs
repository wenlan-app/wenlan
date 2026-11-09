// SPDX-License-Identifier: Apache-2.0
//! UX-GRAPH-8: a memory is filed under topics by what it says, never under
//! an archived topic or only the project-wide hub, and every move of its
//! main topic is logged and can be undone.

use super::topic_filing::{reason, writer, RefileReport};
use crate::db::tests::{test_db, test_db_at};
use crate::db::MemoryDB;
use crate::error::WenlanError;
use crate::sources::RawDocument;

async fn add_memory(db: &MemoryDB, id: &str, content: &str) {
    db.upsert_documents(vec![RawDocument {
        source: "memory".to_string(),
        source_id: id.to_string(),
        title: format!("memory-{id}"),
        content: content.to_string(),
        last_modified: chrono::Utc::now().timestamp(),
        ..Default::default()
    }])
    .await
    .expect("upsert memory");
}

async fn topic(db: &MemoryDB, name: &str) -> String {
    db.store_entity(name, "concept", Some("work"), Some("test"), Some(1.0))
        .await
        .expect("store entity")
}

async fn scalar(db: &MemoryDB, sql: &str) -> i64 {
    let conn = db.conn.lock().await;
    let mut rows = conn.query(sql, ()).await.unwrap();
    rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap()
}

async fn links_of(db: &MemoryDB, memory: &str) -> Vec<(String, String)> {
    let conn = db.conn.lock().await;
    let mut rows = conn
        .query(
            "SELECT entity_id, writer FROM memory_entities WHERE memory_id = ?1 ORDER BY rowid",
            libsql::params![memory],
        )
        .await
        .unwrap();
    let mut links = Vec::new();
    while let Some(row) = rows.next().await.unwrap() {
        links.push((row.get::<String>(0).unwrap(), row.get::<String>(1).unwrap()));
    }
    links
}

/// Force a main topic the way data written before the rule has it.
async fn set_primary_raw(db: &MemoryDB, memory: &str, entity: &str) {
    db.conn
        .lock()
        .await
        .execute(
            "UPDATE memories SET entity_id = ?1 WHERE source_id = ?2",
            libsql::params![entity, memory],
        )
        .await
        .unwrap();
}

fn graph(names: &[&str]) -> Vec<crate::extract::KgExtractionResult> {
    vec![crate::extract::KgExtractionResult {
        index: 0,
        entities: names
            .iter()
            .map(|name| crate::extract::ExtractedEntity {
                name: name.to_string(),
                entity_type: "concept".to_string(),
            })
            .collect(),
        observations: Vec::new(),
        relations: Vec::new(),
    }]
}

#[tokio::test]
async fn ux_graph_8_migration_160_adds_link_writers_and_the_change_log() {
    let (db, _dir) = test_db_at(158).await;
    db.conn
        .lock()
        .await
        .execute(
            "INSERT INTO memory_entities (memory_id, entity_id) VALUES ('old-memory', 'old-topic')",
            (),
        )
        .await
        .unwrap();

    db.run_migrations_up_to(&crate::events::NoopEmitter, 160)
        .await
        .unwrap();
    // Re-running the chain at the same version is a no-op.
    db.run_migrations_up_to(&crate::events::NoopEmitter, 160)
        .await
        .unwrap();

    assert_eq!(scalar(&db, "PRAGMA user_version").await, 160);
    assert_eq!(
        links_of(&db, "old-memory").await,
        vec![("old-topic".to_string(), "legacy".to_string())],
        "links written before the migration are marked legacy"
    );
    assert_eq!(
        scalar(
            &db,
            "SELECT COUNT(*) FROM memory_entities WHERE created_at > 0"
        )
        .await,
        1,
        "old links get a time, not zero"
    );
    assert_eq!(
        scalar(&db, "SELECT COUNT(*) FROM memory_topic_changes").await,
        0
    );
}

#[tokio::test]
async fn ux_graph_8_archiving_a_topic_refiles_its_memories_and_restoring_puts_them_back() {
    let (db, _dir) = test_db().await;
    let alpha = topic(&db, "Alpha").await;
    let beta = topic(&db, "Beta").await;
    add_memory(&db, "m1", "Alpha and Beta").await;
    add_memory(&db, "m2", "Only Alpha").await;
    db.update_memory_entity_id_as("m1", &alpha, "test")
        .await
        .unwrap();
    db.link_memory_entities_as("m1", &[&beta], "test")
        .await
        .unwrap();
    db.update_memory_entity_id_as("m2", &alpha, "test")
        .await
        .unwrap();

    assert!(db.archive_entity(&alpha).await.unwrap());

    assert_eq!(
        db.get_memory_entity_id("m1").await.unwrap(),
        Some(beta.clone())
    );
    assert_eq!(db.get_memory_entity_id("m2").await.unwrap(), None);
    assert_eq!(links_of(&db, "m2").await.len(), 1, "no link is deleted");
    let m1 = db.memory_topic_changes("m1").await.unwrap();
    assert_eq!(m1.len(), 1);
    assert_eq!(m1[0].old_entity_id.as_deref(), Some(alpha.as_str()));
    assert_eq!(m1[0].new_entity_id.as_deref(), Some(beta.as_str()));
    assert_eq!(m1[0].writer, "archive_entity");
    assert_eq!(m1[0].reason, reason::TOPIC_ARCHIVED);
    let m2 = db.memory_topic_changes("m2").await.unwrap();
    assert_eq!(m2[0].new_entity_id, None);

    assert!(db.restore_entity(&alpha).await.unwrap());

    assert_eq!(
        db.get_memory_entity_id("m1").await.unwrap(),
        Some(alpha.clone())
    );
    assert_eq!(
        db.get_memory_entity_id("m2").await.unwrap(),
        Some(alpha.clone())
    );
    let m1 = db.memory_topic_changes("m1").await.unwrap();
    assert_eq!(m1.len(), 2);
    assert_eq!(m1[0].undone_by, Some(m1[1].id));
    assert_eq!(m1[1].writer, writer::RESTORE);
    assert_eq!(m1[1].reason, reason::TOPIC_RESTORED);
}

#[tokio::test]
async fn ux_graph_8_linking_an_archived_topic_never_makes_it_the_main_topic() {
    let (db, _dir) = test_db().await;
    let alpha = topic(&db, "Alpha").await;
    let beta = topic(&db, "Beta").await;
    add_memory(&db, "m1", "Alpha and Beta").await;
    db.update_memory_entity_id_as("m1", &beta, "test")
        .await
        .unwrap();
    assert!(db.archive_entity(&alpha).await.unwrap());

    // A route, capture or night link naming the archived topic directly.
    assert_eq!(
        db.update_memory_entity_id_as("m1", &alpha, writer::LINK_ENTITY_ROUTE)
            .await
            .unwrap(),
        1
    );

    assert_eq!(db.get_memory_entity_id("m1").await.unwrap(), Some(beta));
    assert!(links_of(&db, "m1")
        .await
        .contains(&(alpha, writer::LINK_ENTITY_ROUTE.to_string())));
    assert!(db.memory_topic_changes("m1").await.unwrap().is_empty());
}

#[tokio::test]
async fn ux_graph_8_restore_leaves_a_memory_filed_elsewhere_since() {
    let (db, _dir) = test_db().await;
    let alpha = topic(&db, "Alpha").await;
    let beta = topic(&db, "Beta").await;
    let gamma = topic(&db, "Gamma").await;
    add_memory(&db, "m1", "Alpha and Beta").await;
    db.update_memory_entity_id_as("m1", &alpha, "test")
        .await
        .unwrap();
    db.link_memory_entities_as("m1", &[&beta, &gamma], "test")
        .await
        .unwrap();
    assert!(db.archive_entity(&alpha).await.unwrap());
    assert_eq!(
        db.get_memory_entity_id("m1").await.unwrap(),
        Some(beta.clone())
    );
    // The person files it under Gamma by hand before restoring Alpha.
    db.update_memory_entity_id_as("m1", &gamma, "test")
        .await
        .unwrap();

    assert!(db.restore_entity(&alpha).await.unwrap());

    assert_eq!(db.get_memory_entity_id("m1").await.unwrap(), Some(gamma));
}

#[tokio::test]
async fn ux_graph_8_one_time_repair_refiles_archived_main_topics_and_can_be_undone() {
    let (db, _dir) = test_db().await;
    let alpha = topic(&db, "Alpha").await;
    let beta = topic(&db, "Beta").await;
    for id in ["moved", "unfiled", "untouched"] {
        add_memory(&db, id, "body").await;
    }
    db.link_memory_entities_as("moved", &[&alpha, &beta], "test")
        .await
        .unwrap();
    db.link_memory_entities_as("unfiled", &[&alpha], "test")
        .await
        .unwrap();
    db.update_memory_entity_id_as("untouched", &beta, "test")
        .await
        .unwrap();
    assert!(db.archive_entity(&alpha).await.unwrap());
    // Main topics written before the rule still point at the archived one.
    set_primary_raw(&db, "moved", &alpha).await;
    set_primary_raw(&db, "unfiled", &alpha).await;

    let report = db.refile_archived_primaries_once().await.unwrap();

    assert_eq!(
        report,
        RefileReport {
            moved: 1,
            unfiled: 1
        }
    );
    assert_eq!(
        db.get_memory_entity_id("moved").await.unwrap(),
        Some(beta.clone())
    );
    assert_eq!(db.get_memory_entity_id("unfiled").await.unwrap(), None);
    assert_eq!(
        db.get_memory_entity_id("untouched").await.unwrap(),
        Some(beta.clone())
    );
    assert!(db
        .memory_topic_changes("untouched")
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        links_of(&db, "unfiled").await.len(),
        1,
        "no link is deleted"
    );
    let marker = db
        .get_app_metadata("kg_refile_archived_primaries_v1")
        .await
        .unwrap()
        .expect("marker recorded");
    assert!(marker.contains("\"moved\":1"), "{marker}");
    assert_eq!(
        db.refile_archived_primaries_once().await.unwrap(),
        RefileReport::default()
    );

    let change = db.memory_topic_changes("moved").await.unwrap()[0].clone();
    assert_eq!(change.writer, writer::REFILE);
    assert_eq!(change.reason, reason::ARCHIVED_PRIMARY);
    // While Alpha is archived the change cannot be undone by id.
    assert!(matches!(
        db.undo_memory_topic_change(change.id).await,
        Err(WenlanError::Conflict(_))
    ));

    // Restoring Alpha also puts back what the repair moved off it.
    assert!(db.restore_entity(&alpha).await.unwrap());
    assert_eq!(
        db.get_memory_entity_id("moved").await.unwrap(),
        Some(alpha.clone())
    );
    assert_eq!(
        db.get_memory_entity_id("unfiled").await.unwrap(),
        Some(alpha.clone())
    );
    assert_eq!(
        db.get_memory_entity_id("untouched").await.unwrap(),
        Some(beta.clone())
    );
    let changes = db.memory_topic_changes("moved").await.unwrap();
    assert_eq!(changes.len(), 2);
    assert_eq!(changes[0].undone_by, Some(changes[1].id));
    assert_eq!(changes[1].writer, writer::RESTORE);
    assert_eq!(changes[1].reason, reason::TOPIC_RESTORED);
    assert!(matches!(
        db.undo_memory_topic_change(change.id).await,
        Err(WenlanError::Conflict(_))
    ));

    // The restore's own move is logged, so it too can be undone by id.
    let undo = db.undo_memory_topic_change(changes[1].id).await.unwrap();
    assert_eq!(db.get_memory_entity_id("moved").await.unwrap(), Some(beta));
    let changes = db.memory_topic_changes("moved").await.unwrap();
    assert_eq!(changes[1].undone_by, Some(undo));
    assert_eq!(changes[2].reason, reason::UNDO);
    assert!(matches!(
        db.undo_memory_topic_change(changes[1].id).await,
        Err(WenlanError::Conflict(_))
    ));
    assert!(matches!(
        db.undo_memory_topic_change(i64::MAX).await,
        Err(WenlanError::NotFound(_))
    ));
}

#[tokio::test]
async fn ux_graph_8_restore_after_a_chain_of_archives_puts_the_memory_back() {
    let (db, _dir) = test_db().await;
    let alpha = topic(&db, "Alpha").await;
    let beta = topic(&db, "Beta").await;
    for id in ["chain", "stuck"] {
        add_memory(&db, id, "Alpha and Beta").await;
        db.update_memory_entity_id_as(id, &alpha, "test")
            .await
            .unwrap();
        db.link_memory_entities_as(id, &[&beta], "test")
            .await
            .unwrap();
    }

    // Alpha's memories move to Beta, then Beta's archive leaves them unfiled.
    assert!(db.archive_entity(&alpha).await.unwrap());
    assert_eq!(
        db.get_memory_entity_id("chain").await.unwrap(),
        Some(beta.clone())
    );
    assert!(db.archive_entity(&beta).await.unwrap());
    assert_eq!(db.get_memory_entity_id("chain").await.unwrap(), None);
    // Data written before the rule can still sit on the archived Beta.
    set_primary_raw(&db, "stuck", &beta).await;

    assert!(db.restore_entity(&alpha).await.unwrap());

    for id in ["chain", "stuck"] {
        assert_eq!(
            db.get_memory_entity_id(id).await.unwrap(),
            Some(alpha.clone()),
            "{id} goes back to Alpha: no main topic or an archived one overrides nothing"
        );
    }
    let changes = db.memory_topic_changes("chain").await.unwrap();
    let back = changes.last().unwrap();
    assert_eq!(back.old_entity_id, None);
    assert_eq!(back.new_entity_id.as_deref(), Some(alpha.as_str()));
    assert_eq!(back.writer, writer::RESTORE);
    assert_eq!(back.reason, reason::TOPIC_RESTORED);
    assert_eq!(changes[0].undone_by, Some(back.id));
    let stuck = db.memory_topic_changes("stuck").await.unwrap();
    assert_eq!(
        stuck.last().unwrap().old_entity_id.as_deref(),
        Some(beta.as_str())
    );
}

#[tokio::test]
async fn ux_graph_8_a_topic_that_comes_back_by_itself_brings_its_memories_back() {
    let (db, _dir) = test_db().await;
    let alpha = topic(&db, "Alpha").await;
    let beta = topic(&db, "Beta").await;
    add_memory(&db, "m1", "Alpha and Beta").await;
    db.update_memory_entity_id_as("m1", &alpha, "test")
        .await
        .unwrap();
    db.link_memory_entities_as("m1", &[&beta], "test")
        .await
        .unwrap();
    assert!(db.archive_entity(&alpha).await.unwrap());
    assert_eq!(
        db.get_memory_entity_id("m1").await.unwrap(),
        Some(beta.clone())
    );
    // A memory the daily repair moved off the archived Alpha.
    add_memory(&db, "repaired", "Alpha and Beta").await;
    db.link_memory_entities_as("repaired", &[&alpha, &beta], "test")
        .await
        .unwrap();
    set_primary_raw(&db, "repaired", &alpha).await;
    db.refile_archived_primaries("test").await.unwrap();
    assert_eq!(
        db.get_memory_entity_id("repaired").await.unwrap(),
        Some(beta.clone())
    );

    // #708: the mentions that carry the archived, unconfirmed topic over
    // the threshold restore it through the same path as a person's restore.
    add_memory(&db, "m2", "Alpha again").await;
    db.update_memory_entity_id_as("m2", &alpha, "test")
        .await
        .unwrap();

    assert_eq!(
        db.active_entity_ids(&[&alpha]).await.unwrap(),
        vec![alpha.clone()],
        "Alpha came back"
    );
    assert_eq!(
        db.get_memory_entity_id("m1").await.unwrap(),
        Some(alpha.clone())
    );
    assert_eq!(
        db.get_memory_entity_id("repaired").await.unwrap(),
        Some(alpha)
    );
}

/// A topic linked to so many memories that it is a hub.
async fn hub_topic(db: &MemoryDB) -> String {
    let hub = topic(db, "Hubtopic").await;
    let conn = db.conn.lock().await;
    for index in 0..200 {
        conn.execute(
            "INSERT INTO memory_entities (memory_id, entity_id) VALUES (?1, ?2)",
            libsql::params![format!("filler-{index:03}"), hub.as_str()],
        )
        .await
        .unwrap();
    }
    hub
}

#[tokio::test]
async fn ux_graph_8_archiving_never_refiles_a_memory_under_the_hub() {
    let (db, _dir) = test_db().await;
    let hub = hub_topic(&db).await;
    let alpha = topic(&db, "Alpha").await;
    for id in ["archived", "repaired"] {
        add_memory(&db, id, "Alpha").await;
        db.link_memory_entities_as(id, &[&hub], "test")
            .await
            .unwrap();
    }
    db.update_memory_entity_id_as("archived", &alpha, "test")
        .await
        .unwrap();
    db.link_memory_entities_as("repaired", &[&alpha], "test")
        .await
        .unwrap();

    assert!(db.archive_entity(&alpha).await.unwrap());
    assert_eq!(
        db.get_memory_entity_id("archived").await.unwrap(),
        None,
        "the hub is its only other shown topic, so it is left unfiled"
    );
    assert_eq!(
        db.memory_topic_changes("archived").await.unwrap()[0].new_entity_id,
        None
    );

    // The daily repair follows the same rule.
    set_primary_raw(&db, "repaired", &alpha).await;
    assert_eq!(
        db.refile_archived_primaries("test").await.unwrap(),
        RefileReport {
            moved: 0,
            unfiled: 1
        }
    );
    assert_eq!(db.get_memory_entity_id("repaired").await.unwrap(), None);
    assert_eq!(db.first_active_topic("repaired").await.unwrap(), None);
}

#[tokio::test]
async fn ux_graph_8_the_ambient_lane_never_makes_the_hub_the_main_topic() {
    let (db, _dir) = test_db().await;
    let hub = hub_topic(&db).await;
    add_memory(&db, "m1", "Hubtopic").await;
    db.link_memory_entities_as("m1", &[&hub], "test")
        .await
        .unwrap();

    assert_eq!(
        db.run_entity_enrichment_slice(|_| async move { Ok(Vec::new()) })
            .await
            .unwrap(),
        1
    );

    assert_eq!(db.get_memory_entity_id("m1").await.unwrap(), None);
    assert_eq!(links_of(&db, "m1").await.len(), 1, "the link is kept");
}

#[tokio::test]
async fn ux_graph_8_enrichment_never_links_the_hub_or_files_under_an_archived_topic() {
    let (db, _dir) = test_db().await;
    let hub = hub_topic(&db).await;
    add_memory(&db, "m1", "Alpha and Beta").await;
    assert!(db
        .commit_entity_enrichment_at_version("m1", 1, &graph(&["Alpha", "Beta"]), "Alpha and Beta")
        .await
        .unwrap());
    let alpha = db
        .get_memory_entity_id("m1")
        .await
        .unwrap()
        .expect("m1 filed");
    let beta = links_of(&db, "m1")
        .await
        .into_iter()
        .map(|(entity, _)| entity)
        .find(|entity| entity != &alpha)
        .expect("m1 linked to Beta");
    assert!(db.archive_entity(&alpha).await.unwrap());

    add_memory(&db, "m2", "Hubtopic, Alpha and Beta").await;
    assert!(db
        .commit_entity_enrichment_at_version(
            "m2",
            1,
            &graph(&["Hubtopic", "Alpha", "Beta"]),
            "Hubtopic, Alpha and Beta",
        )
        .await
        .unwrap());

    let links = links_of(&db, "m2").await;
    assert!(
        links.iter().all(|(entity, _)| entity != &hub),
        "the hub is not linked: {links:?}"
    );
    assert!(
        links.iter().any(|(entity, _)| entity == &alpha),
        "the archived topic may still be linked: {links:?}"
    );
    assert!(links.iter().all(|(_, by)| by == writer::ENTITY_ENRICHMENT));
    let primary = db
        .get_memory_entity_id("m2")
        .await
        .unwrap()
        .expect("m2 filed");
    assert_eq!(
        primary, beta,
        "the first shown, specific topic: never the archived one or the hub"
    );

    // Only the hub named: nothing is linked and the memory stays unfiled.
    add_memory(&db, "m3", "Hubtopic").await;
    db.commit_entity_enrichment_at_version("m3", 1, &graph(&["Hubtopic"]), "Hubtopic")
        .await
        .unwrap();
    assert!(links_of(&db, "m3").await.is_empty());
    assert_eq!(db.get_memory_entity_id("m3").await.unwrap(), None);
}
