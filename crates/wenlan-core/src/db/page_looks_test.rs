// SPDX-License-Identifier: Apache-2.0
//! Migration 164 and the stored looks of a page (UX-LOOK-5).

use crate::db::tests::{test_db, test_db_at};
use crate::db::MemoryDB;

async fn scalar(db: &MemoryDB, sql: &str) -> i64 {
    let conn = db.conn.lock().await;
    let mut rows = conn.query(sql, ()).await.unwrap();
    rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap()
}

#[tokio::test]
async fn migration_164_adds_the_table_and_is_safe_to_run_again() {
    let (db, _dir) = test_db_at(163).await;
    assert_eq!(scalar(&db, "PRAGMA user_version").await, 163);
    assert_eq!(
        scalar(
            &db,
            "SELECT COUNT(*) FROM sqlite_master WHERE name = 'page_looks'"
        )
        .await,
        0
    );
    db.run_migrations_up_to(&crate::events::NoopEmitter, 164)
        .await
        .unwrap();
    db.run_migrations_up_to(&crate::events::NoopEmitter, 164)
        .await
        .unwrap();
    assert_eq!(scalar(&db, "PRAGMA user_version").await, 164);
    assert_eq!(scalar(&db, "SELECT COUNT(*) FROM page_looks").await, 0);
    // A replay after a crash before the version bump keeps rows.
    db.set_page_look("p1", "<p>one</p>", 1, None).await.unwrap();
    db.migrate_164_page_looks(163).await.unwrap();
    assert_eq!(scalar(&db, "SELECT COUNT(*) FROM page_looks").await, 1);
}

#[tokio::test]
async fn a_page_has_one_active_look_and_earlier_ones_are_kept() {
    let (db, _dir) = test_db().await;
    assert!(db.get_active_page_look("p1").await.unwrap().is_none());

    let (first, created) = db
        .set_page_look("p1", "<p>one</p>", 1, Some("claude-code:s1"))
        .await
        .unwrap();
    assert!(created);
    // The same HTML again changes nothing.
    assert_eq!(
        db.set_page_look("p1", "<p>one</p>", 1, None).await.unwrap(),
        (first, false)
    );
    let (second, created) = db.set_page_look("p1", "<p>two</p>", 1, None).await.unwrap();
    assert!(created);
    assert_ne!(first, second);

    let active = db.get_active_page_look("p1").await.unwrap().unwrap();
    assert_eq!(active.id, second);
    assert_eq!(active.html, "<p>two</p>");
    let versions = db.list_page_look_versions("p1").await.unwrap();
    assert_eq!(versions.len(), 2);
    assert_eq!(versions.iter().filter(|v| v.active).count(), 1);
    assert_eq!(versions[0].id, second, "newest first");
    assert_eq!(
        versions[1].session.as_deref(),
        Some("claude-code:s1"),
        "the session that saved it is kept"
    );
    // Another page is its own.
    assert!(db.get_active_page_look("p2").await.unwrap().is_none());
}

#[tokio::test]
async fn removing_a_look_keeps_every_version_and_restore_brings_it_back() {
    let (db, _dir) = test_db().await;
    let (first, _) = db.set_page_look("p1", "<p>one</p>", 1, None).await.unwrap();
    let (second, _) = db.set_page_look("p1", "<p>two</p>", 1, None).await.unwrap();

    assert_eq!(db.remove_page_look("p1").await.unwrap(), Some(second));
    assert!(db.get_active_page_look("p1").await.unwrap().is_none());
    assert_eq!(db.list_page_look_versions("p1").await.unwrap().len(), 2);
    assert_eq!(
        db.remove_page_look("p1").await.unwrap(),
        None,
        "nothing left to remove"
    );

    // Undo: the version that was removed.
    assert_eq!(
        db.restore_page_look("p1", Some(second)).await.unwrap(),
        Some(second)
    );
    assert_eq!(
        db.get_active_page_look("p1").await.unwrap().unwrap().id,
        second
    );
    // Put the one before it back.
    assert_eq!(
        db.restore_page_look("p1", Some(first)).await.unwrap(),
        Some(first)
    );
    let active: Vec<_> = db
        .list_page_look_versions("p1")
        .await
        .unwrap()
        .into_iter()
        .filter(|v| v.active)
        .collect();
    assert_eq!(active.len(), 1, "exactly one active");
    assert_eq!(active[0].id, first);
    // No id: the newest version that is not active.
    assert_eq!(
        db.restore_page_look("p1", None).await.unwrap(),
        Some(second)
    );
    // An unknown version, or another page's, restores nothing.
    assert_eq!(db.restore_page_look("p1", Some(9999)).await.unwrap(), None);
    assert_eq!(db.restore_page_look("p2", Some(first)).await.unwrap(), None);
}

#[tokio::test]
async fn the_last_error_belongs_to_the_active_look_and_clears_on_restore() {
    let (db, _dir) = test_db().await;
    assert!(!db.record_page_look_error("p1", "boom").await.unwrap());
    let (first, _) = db.set_page_look("p1", "<p>one</p>", 1, None).await.unwrap();
    assert!(db
        .record_page_look_error("p1", "x is not defined")
        .await
        .unwrap());
    assert_eq!(
        db.get_active_page_look("p1")
            .await
            .unwrap()
            .unwrap()
            .last_error
            .as_deref(),
        Some("x is not defined")
    );
    // A new look starts clean.
    db.set_page_look("p1", "<p>two</p>", 1, None).await.unwrap();
    assert_eq!(
        db.get_active_page_look("p1")
            .await
            .unwrap()
            .unwrap()
            .last_error,
        None
    );
    db.remove_page_look("p1").await.unwrap();
    db.restore_page_look("p1", Some(first)).await.unwrap();
    assert_eq!(
        db.get_active_page_look("p1")
            .await
            .unwrap()
            .unwrap()
            .last_error,
        None
    );
}

#[tokio::test]
async fn pages_with_a_look_are_found_in_one_read() {
    let (db, _dir) = test_db().await;
    db.set_page_look("a", "<p>a</p>", 1, None).await.unwrap();
    db.set_page_look("b", "<p>b</p>", 1, None).await.unwrap();
    db.remove_page_look("b").await.unwrap();
    let found = db
        .page_ids_with_look(&["a".into(), "b".into(), "c".into()])
        .await
        .unwrap();
    assert_eq!(found, std::collections::HashSet::from(["a".to_string()]));
}

#[tokio::test]
async fn deleting_a_page_deletes_its_looks() {
    let (db, _dir) = test_db().await;
    db.insert_page_with_kind(
        "pg1",
        "Trip",
        None,
        "Body.",
        None,
        Some("sp"),
        &[],
        "2026-01-01T00:00:00Z",
        "authored",
        "confirmed",
        None,
        None,
    )
    .await
    .unwrap();
    db.set_page_look("pg1", "<p>x</p>", 1, None).await.unwrap();
    db.delete_page("pg1").await.unwrap();
    assert_eq!(scalar(&db, "SELECT COUNT(*) FROM page_looks").await, 0);
}

/// UX-LOOK-6 (and the byte-for-byte promise): a look is its own row on this
/// computer. Setting one leaves the page as it was, and the table is not one
/// the file migration moves into the vault.
#[tokio::test]
async fn ux_look_6_a_look_is_its_own_local_row_and_leaves_the_page_untouched() {
    use crate::db::table_classes::{table_class, TableClass};
    let (db, _dir) = test_db().await;
    db.insert_page_with_kind(
        "pg1",
        "Trip",
        None,
        "Body.",
        None,
        Some("sp"),
        &[],
        "2026-01-01T00:00:00Z",
        "authored",
        "confirmed",
        None,
        None,
    )
    .await
    .unwrap();
    let before = db.get_page("pg1").await.unwrap().unwrap();
    db.set_page_look("pg1", "<p>one</p>", 1, None)
        .await
        .unwrap();
    db.set_page_look("pg1", "<p>two</p>", 1, None)
        .await
        .unwrap();
    db.remove_page_look("pg1").await.unwrap();
    let after = db.get_page("pg1").await.unwrap().unwrap();
    assert_eq!(
        (
            &after.content,
            after.version,
            &after.last_modified,
            &after.summary
        ),
        (
            &before.content,
            before.version,
            &before.last_modified,
            &before.summary
        )
    );
    assert_eq!(table_class("page_looks"), Some(TableClass::Cache));
}
