// SPDX-License-Identifier: Apache-2.0
//! Migration 165 and who archived or restored a page.

use crate::db::tests::{test_db, test_db_at};
use crate::db::MemoryDB;

async fn scalar(db: &MemoryDB, sql: &str) -> i64 {
    let conn = db.conn.lock().await;
    let mut rows = conn.query(sql, ()).await.unwrap();
    rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap()
}

async fn page_with_history(db: &MemoryDB, marker: &str) {
    let conn = db.conn.lock().await;
    conn.execute_batch(
        "INSERT INTO pages
             (id,title,summary,content,source_memory_ids,version,status,
              kind,creation_kind,created_at,last_compiled,last_modified)
         VALUES ('p-actor','Trail log','','Body.','[]',2,'archived','concept','distilled',
                 '2026-01-01','2026-01-01','2026-01-02');",
    )
    .await
    .unwrap();
    MemoryDB::append_page_history(&conn, "p-actor", marker, 5)
        .await
        .unwrap();
}

#[tokio::test]
async fn migration_165_adds_a_nullable_column_and_keeps_old_rows() {
    let (db, _dir) = test_db_at(164).await;
    {
        let conn = db.conn.lock().await;
        conn.execute_batch(
            "INSERT INTO page_history (page_id,version,content,source_memory_ids,edited_by,created_at)
             VALUES ('old','1','text','[]','archive',1);",
        )
        .await
        .unwrap();
    }
    db.run_migrations_up_to(&crate::events::NoopEmitter, 165)
        .await
        .unwrap();
    // Safe to replay after a crash before the bump.
    db.migrate_165_page_history_actor(164).await.unwrap();
    assert_eq!(scalar(&db, "PRAGMA user_version").await, 165);
    assert_eq!(
        scalar(
            &db,
            "SELECT COUNT(*) FROM page_history WHERE page_id='old' AND actor IS NULL"
        )
        .await,
        1
    );
    let rows = db.page_lifecycle_rows("old").await.unwrap();
    assert_eq!(rows, vec![(1, "archive".to_string(), 1, None)]);
}

#[tokio::test]
async fn the_actor_is_stamped_once_and_only_on_lifecycle_rows() {
    let (db, _dir) = test_db().await;
    page_with_history(&db, "archive").await;
    db.stamp_page_history_actor("p-actor", 2, "shared_person:m-amy")
        .await
        .unwrap();
    // A second stamp never overwrites the first.
    db.stamp_page_history_actor("p-actor", 2, "manual_edit")
        .await
        .unwrap();
    assert_eq!(
        db.page_lifecycle_at("p-actor", 2).await,
        Some(("archive".into(), Some("shared_person:m-amy".into())))
    );
}

#[tokio::test]
async fn an_edit_row_is_never_given_an_actor() {
    let (db, _dir) = test_db().await;
    page_with_history(&db, "manual_edit").await;
    db.stamp_page_history_actor("p-actor", 2, "shared_person:m-amy")
        .await
        .unwrap();
    assert_eq!(
        scalar(
            &db,
            "SELECT COUNT(*) FROM page_history WHERE actor IS NOT NULL"
        )
        .await,
        0
    );
    assert_eq!(db.page_lifecycle_at("p-actor", 2).await, None);
}
