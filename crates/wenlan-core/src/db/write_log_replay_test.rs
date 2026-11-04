// SPDX-License-Identifier: Apache-2.0
use std::path::PathBuf;
use std::sync::Arc;

use super::*;
use crate::events::NoopEmitter;
use crate::store::export::tests::fixture;
use crate::store::write_log::{read_after, read_applied, register, unregister, write_applied};

const DB_FILE: &str = "origin_memory.db";

/// The export fixture, opened again with a write log armed.
async fn logged_fixture() -> (
    MemoryDB,
    tempfile::TempDir,
    tempfile::TempDir,
    Arc<WriteLog>,
) {
    let (seed, dir) = fixture().await;
    let embedder = seed
        .shared_embedder()
        .expect("the test database has a model");
    drop(seed);
    let log_dir = tempfile::tempdir().unwrap();
    let log = WriteLog::open(log_dir.path()).unwrap();
    register(&dir.path().join(DB_FILE), log.clone()).unwrap();
    let db = MemoryDB::new_with_shared_embedder(dir.path(), Arc::new(NoopEmitter), embedder)
        .await
        .unwrap();
    db.arm_write_log(&log).await.unwrap();
    (db, dir, log_dir, log)
}

async fn run(conn: &Connection, statement: &str) {
    conn.execute_batch(statement)
        .await
        .unwrap_or_else(|error| panic!("{statement}: {error}"));
}

async fn copy_of(db: &MemoryDB) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join(DB_FILE);
    let conn = db.conn.lock().await;
    conn.execute("VACUUM INTO ?1", [file.to_str().unwrap()])
        .await
        .unwrap();
    (dir, file)
}

async fn open(file: &Path) -> (libsql::Database, Connection) {
    let database = libsql::Builder::new_local(file.to_str().unwrap())
        .build()
        .await
        .unwrap();
    let conn = database.connect().unwrap();
    (database, conn)
}

/// Every logged table's rows, vectors left out, as sorted text.
async fn dump(conn: &Connection) -> Vec<(String, Vec<String>)> {
    let shapes = shapes(conn).await.unwrap();
    let mut tables: Vec<_> = shapes.into_iter().collect();
    tables.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out = Vec::new();
    for (table, shape) in tables {
        let columns: Vec<String> = shape
            .columns
            .iter()
            .zip(&shape.skip)
            .filter(|(_, skip)| !**skip)
            .map(|(name, _)| quote(name))
            .collect();
        if columns.is_empty() {
            continue;
        }
        let mut rows = conn
            .query(
                &format!("SELECT {} FROM {}", columns.join(", "), quote(&table)),
                (),
            )
            .await
            .unwrap();
        let mut found = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            let values: Vec<String> = (0..columns.len() as i32)
                .map(|i| format!("{:?}", row.get_value(i).unwrap()))
                .collect();
            found.push(values.join("|"));
        }
        found.sort();
        out.push((table, found));
    }
    out
}

async fn count(conn: &Connection, statement: &str) -> i64 {
    let mut rows = conn.query(statement, ()).await.unwrap();
    rows.next().await.unwrap().unwrap().get(0).unwrap()
}

#[tokio::test]
async fn a_copy_plus_the_log_after_it_is_the_database() {
    let (db, dir, log_dir, log) = logged_fixture().await;
    let position = db.write_log_position(&log).await;
    let (_copy_dir, copy_file) = copy_of(&db).await;

    {
        let conn = db.conn.lock().await;
        // A new memory, an edited page (its triggers queue work and keep
        // the search index), a deleted chunk.
        run(
            &conn,
            "INSERT INTO memories (id, content, source, source_id, title, chunk_index, \
             last_modified, chunk_type) VALUES ('z1', 'zebra crossing', 'memory', 'mem_z', \
             'Zebra', 0, 0, 'text');
             UPDATE pages SET content = 'The launch moved to 2026-04-01.' WHERE id = 'a';
             DELETE FROM memories WHERE id = 'c1';",
        )
        .await;
        // Taken back: logged nowhere.
        run(
            &conn,
            "BEGIN;
             INSERT INTO memories (id, content, source, source_id, title, chunk_index, \
             last_modified, chunk_type) VALUES ('x1', 'rolled back', 'memory', 'mem_x', '', 0, \
             0, 'text');
             ROLLBACK;",
        )
        .await;
        // Only the change after the savepoint's rollback stays.
        run(
            &conn,
            "BEGIN;
             SAVEPOINT s;
             INSERT INTO memories (id, content, source, source_id, title, chunk_index, \
             last_modified, chunk_type) VALUES ('y1', 'taken back', 'memory', 'mem_y', '', 0, \
             0, 'text');
             ROLLBACK TO s;
             RELEASE s;
             UPDATE memories SET title = 'Zebra crossing' WHERE id = 'z1';
             COMMIT;",
        )
        .await;
    }

    let (txs, problems) = read_after(log_dir.path(), Some(&position)).unwrap();
    assert!(problems.is_empty(), "{problems:?}");
    let tables: BTreeSet<&str> = txs
        .iter()
        .flat_map(|tx| tx.changes.iter().map(|c| c.table.as_str()))
        .collect();
    assert!(
        tables.contains("memories") && tables.contains("pages"),
        "{tables:?}"
    );
    let ids: Vec<String> = txs
        .iter()
        .flat_map(|tx| &tx.changes)
        .filter(|c| c.table == "memories")
        .filter_map(|c| {
            c.row
                .iter()
                .chain(&c.key)
                .find(|(name, _)| name == "id")
                .map(|(_, v)| format!("{v:?}"))
        })
        .collect();
    assert!(
        !ids.iter().any(|id| id.contains("x1") || id.contains("y1")),
        "{ids:?}"
    );

    let (_copy, copy) = open(&copy_file).await;
    let replayed = replay_changes(&copy, &txs).await.unwrap();
    assert!(replayed.problems.is_empty(), "{:?}", replayed.problems);
    assert_eq!(replayed.txs, txs.len());
    let live = {
        let conn = db.conn.lock().await;
        dump(&conn).await
    };
    let rebuilt = dump(&copy).await;
    for ((table, want), (_, got)) in live.iter().zip(&rebuilt) {
        assert_eq!(want, got, "table {table} differs after the replay");
    }
    assert_eq!(live.len(), rebuilt.len());
    // The search index follows the replayed rows, and the triggers are back.
    assert_eq!(
        count(
            &copy,
            "SELECT COUNT(*) FROM memories_fts WHERE memories_fts MATCH 'zebra'"
        )
        .await,
        1
    );
    assert_eq!(
        count(
            &copy,
            "SELECT COUNT(*) FROM pages_fts WHERE pages_fts MATCH 'moved'"
        )
        .await,
        1
    );
    let triggers = "SELECT COUNT(*) FROM sqlite_master WHERE type = 'trigger'";
    let conn = db.conn.lock().await;
    assert_eq!(count(&copy, triggers).await, count(&conn, triggers).await);
    drop(conn);

    // Applying the same lines again changes nothing.
    replay_changes(&copy, &txs).await.unwrap();
    assert_eq!(dump(&copy).await, rebuilt);
    unregister(&dir.path().join(DB_FILE));
}

#[tokio::test]
async fn the_last_write_a_database_lost_is_applied_at_start() {
    let (db, dir, log_dir, _log) = logged_fixture().await;
    let before = read_applied(log_dir.path()).expect("arming records the position");
    let (_copy_dir, copy_file) = copy_of(&db).await;
    {
        let conn = db.conn.lock().await;
        run(
            &conn,
            "INSERT INTO memories (id, content, source, source_id, title, chunk_index, \
             last_modified, chunk_type) VALUES ('late', 'written last', 'memory', 'mem_late', \
             '', 0, 0, 'text');",
        )
        .await;
    }
    // The connection's next statement shows the commit landed.
    let after = db.write_log_position(&_log).await;
    assert_eq!(read_applied(log_dir.path()).unwrap(), after);
    assert!(after.seq > before.seq, "{before:?} -> {after:?}");
    // The process stopped after the log line, before the database commit:
    // the copy is the database, and the position was never moved.
    write_applied(log_dir.path(), &before).unwrap();
    let settled = settle(&copy_file, log_dir.path()).await.unwrap();
    assert!(settled.is_some_and(|r| r.changes > 0 && r.problems.is_empty()));
    let (_copy, copy) = open(&copy_file).await;
    assert_eq!(
        count(&copy, "SELECT COUNT(*) FROM memories WHERE id = 'late'").await,
        1
    );
    drop(copy);
    assert_eq!(read_applied(log_dir.path()).unwrap().seq, after.seq);
    // Once there, nothing more is done; the live database had it already.
    assert!(settle(&copy_file, log_dir.path()).await.unwrap().is_none());
    write_applied(log_dir.path(), &before).unwrap();
    assert!(settle(&dir.path().join(DB_FILE), log_dir.path())
        .await
        .unwrap()
        .is_none());
    unregister(&dir.path().join(DB_FILE));
}

#[tokio::test]
async fn a_change_made_without_the_log_is_not_undone_at_start() {
    let (db, dir, log_dir, _log) = logged_fixture().await;
    {
        let conn = db.conn.lock().await;
        run(&conn, "UPDATE pages SET title = 'Logged' WHERE id = 'a';").await;
    }
    let (_copy_dir, copy_file) = copy_of(&db).await;
    {
        // A session with the log turned off changes the same row.
        let (_copy, copy) = open(&copy_file).await;
        run(&copy, "UPDATE pages SET title = 'Unlogged' WHERE id = 'a';").await;
    }
    assert!(settle(&copy_file, log_dir.path()).await.unwrap().is_none());
    let (_copy, copy) = open(&copy_file).await;
    assert_eq!(
        count(
            &copy,
            "SELECT COUNT(*) FROM pages WHERE id = 'a' AND title = 'Unlogged'"
        )
        .await,
        1
    );
    unregister(&dir.path().join(DB_FILE));
}

#[tokio::test]
async fn nothing_is_logged_before_the_log_is_armed() {
    let (seed, dir) = fixture().await;
    let embedder = seed.shared_embedder().unwrap();
    drop(seed);
    let log_dir = tempfile::tempdir().unwrap();
    let log = WriteLog::open(log_dir.path()).unwrap();
    register(&dir.path().join(DB_FILE), log.clone()).unwrap();
    // Opening runs the migrations and start-up writes: not logged.
    let db = MemoryDB::new_with_shared_embedder(dir.path(), Arc::new(NoopEmitter), embedder)
        .await
        .unwrap();
    {
        let conn = db.conn.lock().await;
        run(
            &conn,
            "UPDATE pages SET title = 'Alpha one' WHERE id = 'a';",
        )
        .await;
    }
    assert_eq!(read_after(log_dir.path(), None).unwrap().0.len(), 0);
    db.arm_write_log(&log).await.unwrap();
    {
        let conn = db.conn.lock().await;
        run(
            &conn,
            "UPDATE pages SET title = 'Alpha two' WHERE id = 'a';",
        )
        .await;
        // A write that changes nothing the files hold is not logged.
        run(&conn, "UPDATE pages SET title = title WHERE id = 'a';").await;
    }
    let (txs, _) = read_after(log_dir.path(), None).unwrap();
    assert_eq!(txs.len(), 1, "{txs:#?}");
    unregister(&dir.path().join(DB_FILE));
}
