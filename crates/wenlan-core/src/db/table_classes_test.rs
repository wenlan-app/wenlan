// SPDX-License-Identifier: Apache-2.0
use super::super::tests::{test_db, test_db_at};
use super::{table_class, DROPPED_TABLES, GENESIS_LANE_TABLES, TABLE_CLASSES};
use std::collections::BTreeSet;

async fn table_names(db: &crate::db::MemoryDB) -> BTreeSet<String> {
    let conn = db.conn.lock().await;
    let mut rows = conn
        .query(
            "SELECT name FROM sqlite_master
              WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite_%'",
            (),
        )
        .await
        .unwrap();
    let mut names = BTreeSet::new();
    while let Some(row) = rows.next().await.unwrap() {
        names.insert(row.get::<String>(0).unwrap());
    }
    names
}

#[tokio::test]
async fn every_table_in_a_fresh_database_has_a_class() {
    let (db, _dir) = test_db().await;
    let actual = table_names(&db).await;
    let listed: BTreeSet<String> = TABLE_CLASSES.iter().map(|(t, _)| t.to_string()).collect();

    let unlisted: Vec<_> = actual.difference(&listed).collect();
    let missing: Vec<_> = listed.difference(&actual).collect();
    assert!(
        unlisted.is_empty(),
        "tables with no class in TABLE_CLASSES (add each with File, History, Cache or Retire): \
         {unlisted:?}"
    );
    assert!(
        missing.is_empty(),
        "TABLE_CLASSES lists tables a fresh database does not have: {missing:?}"
    );
    assert_eq!(listed.len(), TABLE_CLASSES.len(), "a table is listed twice");
}

#[tokio::test]
async fn migration_150_drops_the_retired_tables_and_nothing_else() {
    let (db, _dir) = test_db_at(145).await;
    let before = table_names(&db).await;
    for table in DROPPED_TABLES {
        assert!(
            before.contains(*table),
            "{table} exists before migration 150"
        );
        assert_eq!(
            table_class(table),
            None,
            "{table} is dropped, so it has no class"
        );
    }

    db.run_migrations(&crate::events::NoopEmitter)
        .await
        .unwrap();

    let after = table_names(&db).await;
    let removed: BTreeSet<&str> = before
        .iter()
        .filter(|t| !after.contains(*t))
        .map(String::as_str)
        .collect();
    assert_eq!(removed, DROPPED_TABLES.iter().copied().collect());
}

#[tokio::test]
async fn migration_150_runs_again_without_error() {
    let (db, _dir) = test_db().await;
    {
        let conn = db.conn.lock().await;
        conn.execute("PRAGMA user_version = 145", ()).await.unwrap();
    }
    db.run_migrations(&crate::events::NoopEmitter)
        .await
        .unwrap();
    let names = table_names(&db).await;
    for table in DROPPED_TABLES {
        assert!(!names.contains(*table), "{table} stays dropped");
    }
}

async fn scalar(db: &crate::db::MemoryDB, sql: &str) -> i64 {
    let conn = db.conn.lock().await;
    let mut rows = conn.query(sql, ()).await.unwrap();
    rows.next().await.unwrap().unwrap().get(0).unwrap()
}

/// A database the lane ran on: every lane table holds a row, a root points
/// at its candidate, and the lane's lease and scan cursor sit next to rows
/// that are not the lane's.
async fn database_the_genesis_lane_ran_on() -> (crate::db::MemoryDB, tempfile::TempDir) {
    let (db, dir) = test_db_at(150).await;
    {
        let conn = db.conn.lock().await;
        conn.execute_batch(
            "CREATE TABLE genesis_candidates (candidate_id TEXT PRIMARY KEY, space TEXT);
             CREATE TABLE genesis_candidate_roots (
                 candidate_id TEXT NOT NULL REFERENCES genesis_candidates(candidate_id),
                 root_id TEXT NOT NULL,
                 PRIMARY KEY (candidate_id, root_id));
             INSERT INTO genesis_candidates VALUES ('cand-1', 'Work');
             INSERT INTO genesis_candidate_roots VALUES ('cand-1', 'root-1');",
        )
        .await
        .unwrap();
        for table in GENESIS_LANE_TABLES
            .iter()
            .filter(|table| !table.starts_with("genesis_candidate"))
        {
            conn.execute_batch(&format!(
                "CREATE TABLE {table} (space TEXT);
                 INSERT INTO {table} VALUES ('Work');
                 CREATE TRIGGER {table}_no_delete BEFORE DELETE ON {table}
                 BEGIN SELECT RAISE(ABORT, 'append-only'); END;"
            ))
            .await
            .unwrap();
        }
        conn.execute_batch(
            "INSERT INTO grouping_leases (phase, space, input_generation, token, expires_at)
                  VALUES ('genesis', 'Work', 1, 't1', 0),
                         ('frontier', 'Work', 1, 't2', 0),
                         ('community', 'Work', 1, 't3', 0);
             INSERT INTO app_metadata (key, value)
                  VALUES ('m6_frontier_cursor_v1:Work', 'cursor'),
                         ('unrelated_key', 'kept');",
        )
        .await
        .unwrap();
    }
    (db, dir)
}

#[tokio::test]
async fn migration_151_drops_the_genesis_lane_and_nothing_else() {
    let (db, _dir) = database_the_genesis_lane_ran_on().await;
    db.conn
        .lock()
        .await
        .execute("PRAGMA foreign_keys = ON", ())
        .await
        .unwrap();
    let before = table_names(&db).await;
    for table in GENESIS_LANE_TABLES {
        assert_eq!(
            table_class(table),
            None,
            "{table} is dropped, so it has no class"
        );
    }

    db.run_migrations(&crate::events::NoopEmitter)
        .await
        .unwrap();

    let after = table_names(&db).await;
    let removed: BTreeSet<&str> = before
        .iter()
        .filter(|t| !after.contains(*t))
        .map(String::as_str)
        .collect();
    assert_eq!(removed, GENESIS_LANE_TABLES.iter().copied().collect());
    assert_eq!(
        scalar(
            &db,
            "SELECT COUNT(*) FROM grouping_leases WHERE phase = 'community'"
        )
        .await,
        1,
        "the community lease is not the lane's"
    );
    assert_eq!(scalar(&db, "SELECT COUNT(*) FROM grouping_leases").await, 1);
    assert_eq!(
        scalar(
            &db,
            "SELECT COUNT(*) FROM app_metadata WHERE key LIKE 'm6_%'"
        )
        .await,
        0
    );
    assert_eq!(
        scalar(
            &db,
            "SELECT COUNT(*) FROM app_metadata WHERE key = 'unrelated_key'"
        )
        .await,
        1
    );
    assert_eq!(scalar(&db, "PRAGMA user_version").await, 151);
}

#[tokio::test]
async fn a_fresh_database_never_creates_the_genesis_lane() {
    let (db, _dir) = test_db().await;
    let names = table_names(&db).await;
    for table in GENESIS_LANE_TABLES {
        assert!(!names.contains(*table), "a fresh database has {table}");
    }
}
