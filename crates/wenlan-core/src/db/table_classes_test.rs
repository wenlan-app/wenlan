// SPDX-License-Identifier: Apache-2.0
use super::super::tests::{test_db, test_db_at};
use super::{table_class, DROPPED_TABLES, TABLE_CLASSES};
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
