// SPDX-License-Identifier: Apache-2.0
//! Migration 144: keep quick-capture bodies exactly as typed, and bring back
//! desktop quick captures stored before captures moved to the `memory`
//! namespace.
//!
//! Desktop quick captures used to be stored as `source = 'manual'` with a
//! `manual_<unix seconds>` id. The Memory readers only list `source =
//! 'memory'`, so those captures, their tags and their notes became invisible.
//! This migration moves each such document, with its tags and its source
//! annotations, to `memory`, and adds `memories.authored_content`, which new
//! captures use to keep the typed Markdown next to its retrieval chunks.
use super::{commit_or_rollback, MemoryDB};
use crate::error::WenlanError;

/// A legacy capture id is `manual_` followed by digits only (a unix
/// timestamp). Anything else under `source = 'manual'` is not ours to move.
const LEGACY_CAPTURE_IDS: &str = "old.source = 'manual'
       AND old.source_id GLOB 'manual_[0-9]*'
       AND substr(old.source_id, 8) NOT GLOB '*[^0-9]*'";

/// What migration 144 changed, for the log line.
#[derive(Debug, Default, PartialEq, Eq)]
struct LegacyCaptureMove {
    documents: i64,
    chunks: u64,
    tags: u64,
    annotations: u64,
    /// Documents left under `manual` because a `memory` document with the
    /// same id already exists.
    skipped_documents: i64,
    /// Annotations left under `manual` because the `memory` document already
    /// has an annotation for the same context.
    skipped_annotations: i64,
}

impl MemoryDB {
    pub(super) async fn migrate_144_legacy_quick_captures(
        &self,
        prior_version: i64,
    ) -> Result<(), WenlanError> {
        self.backup_before_migration(144, prior_version).await?;
        let conn = self.conn.lock().await;
        conn.execute("BEGIN IMMEDIATE", ())
            .await
            .map_err(|e| WenlanError::VectorDb(format!("m144 begin: {e}")))?;
        let moved = match move_legacy_captures(&conn).await {
            Ok(moved) => moved,
            Err(error) => {
                let _ = conn.execute("ROLLBACK", ()).await;
                return Err(WenlanError::VectorDb(format!(
                    "m144 promote captures: {error}"
                )));
            }
        };
        commit_or_rollback(&conn)
            .await
            .map_err(|e| WenlanError::VectorDb(format!("m144 commit: {e}")))?;
        log::info!(
            "[migration] Migration 144 applied: memories.authored_content; moved {} legacy quick \
             captures ({} chunks, {} tags, {} annotations) to memory; skipped {} captures and {} \
             annotations whose id is already used under memory",
            moved.documents,
            moved.chunks,
            moved.tags,
            moved.annotations,
            moved.skipped_documents,
            moved.skipped_annotations,
        );
        Ok(())
    }
}

async fn count(conn: &libsql::Connection, sql: &str) -> Result<i64, libsql::Error> {
    let mut rows = conn.query(sql, ()).await?;
    let value = match rows.next().await? {
        Some(row) => row.get::<i64>(0)?,
        None => 0,
    };
    Ok(value)
}

async fn move_legacy_captures(
    conn: &libsql::Connection,
) -> Result<LegacyCaptureMove, libsql::Error> {
    let exists = count(
        conn,
        "SELECT COUNT(*) FROM pragma_table_info('memories') WHERE name = 'authored_content'",
    )
    .await?
        > 0;
    if !exists {
        conn.execute("ALTER TABLE memories ADD COLUMN authored_content TEXT", ())
            .await?;
    }

    // Freeze the document candidates before changing any chunks. A correlated
    // NOT EXISTS inside the UPDATE would see the first moved chunk and skip
    // its siblings. A collision skips the whole document: its chunks, tags
    // and annotations all stay under `manual`, so nothing is merged into
    // another document.
    conn.execute("DROP TABLE IF EXISTS temp.quick_capture_m144_ids", ())
        .await?;
    conn.execute(
        "CREATE TEMP TABLE quick_capture_m144_ids (source_id TEXT PRIMARY KEY)",
        (),
    )
    .await?;
    let documents = conn
        .execute(
            &format!(
                "INSERT INTO quick_capture_m144_ids
                 SELECT DISTINCT old.source_id FROM memories old
                 WHERE {LEGACY_CAPTURE_IDS}
                   AND NOT EXISTS (
                     SELECT 1 FROM memories existing
                     WHERE existing.source = 'memory' AND existing.source_id = old.source_id
                   )"
            ),
            (),
        )
        .await? as i64;
    let skipped_documents = count(
        conn,
        &format!(
            "SELECT COUNT(DISTINCT old.source_id) FROM memories old
             WHERE {LEGACY_CAPTURE_IDS}
               AND old.source_id NOT IN (SELECT source_id FROM quick_capture_m144_ids)"
        ),
    )
    .await?;

    // Tags are a set, so a tag the `memory` id already carries is merged.
    conn.execute(
        "INSERT OR IGNORE INTO document_tags (source, source_id, tag)
         SELECT 'memory', source_id, tag FROM document_tags
         WHERE source = 'manual' AND source_id IN (SELECT source_id FROM quick_capture_m144_ids)",
        (),
    )
    .await?;
    let tags = conn
        .execute(
            "DELETE FROM document_tags
             WHERE source = 'manual'
               AND source_id IN (SELECT source_id FROM quick_capture_m144_ids)",
            (),
        )
        .await?;

    // Annotations outlive their chunks, so a `memory` annotation can exist for
    // an id with no `memory` chunks. Two notes cannot be merged; the `manual`
    // one stays where it is rather than overwrite or be overwritten.
    let annotations = conn
        .execute(
            "UPDATE source_annotations SET source = 'memory'
             WHERE source = 'manual'
               AND source_id IN (SELECT source_id FROM quick_capture_m144_ids)
               AND NOT EXISTS (
                 SELECT 1 FROM source_annotations current
                 WHERE current.source = 'memory'
                   AND current.source_id = source_annotations.source_id
                   AND current.context_key = source_annotations.context_key
               )",
            (),
        )
        .await?;
    let skipped_annotations = count(
        conn,
        "SELECT COUNT(*) FROM source_annotations
         WHERE source = 'manual'
           AND source_id IN (SELECT source_id FROM quick_capture_m144_ids)",
    )
    .await?;

    let chunks = conn
        .execute(
            "UPDATE memories SET source = 'memory'
             WHERE source = 'manual'
               AND source_id IN (SELECT source_id FROM quick_capture_m144_ids)",
            (),
        )
        .await?;
    conn.execute("DROP TABLE temp.quick_capture_m144_ids", ())
        .await?;
    conn.execute("PRAGMA user_version = 144", ()).await?;
    Ok(LegacyCaptureMove {
        documents,
        chunks,
        tags,
        annotations,
        skipped_documents,
        skipped_annotations,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn seed(db: &MemoryDB, id: &str, source: &str, source_id: &str, chunk: i64) {
        let vector = serde_json::to_string(&vec![0.01_f32; 768]).unwrap();
        let conn = db.conn.lock().await;
        conn.execute(
            "INSERT INTO memories (id, content, source, source_id, title, chunk_index,
                last_modified, chunk_type, embedding, version, space, source_agent,
                structured_fields, changelog)
             VALUES (?1, 'Keep **body** | pipes | untouched', ?2, ?3, 'Original title', ?4,
                1700000000, 'text', vector32(?5), 7, ?6, 'manual', '{\"keep\":true}', '[\"history\"]')",
            libsql::params![id, source, source_id, chunk, vector, crate::db::UNFILED_SPACE_ID],
        ).await.unwrap();
    }

    async fn snapshot(db: &MemoryDB, source_id: &str) -> Vec<String> {
        let conn = db.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT json_object('rowid', rowid, 'id', id, 'content', content, 'title', title,
                'embedding', hex(embedding), 'version', version, 'space', space,
                'source_agent', source_agent, 'structured_fields', structured_fields,
                'source_text', source_text, 'changelog', changelog, 'last_modified', last_modified)
             FROM memories WHERE source_id = ?1 ORDER BY id",
                libsql::params![source_id],
            )
            .await
            .unwrap();
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            out.push(row.get(0).unwrap());
        }
        out
    }

    async fn sources(db: &MemoryDB, source_id: &str) -> Vec<String> {
        let conn = db.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT source FROM memories WHERE source_id=?1 ORDER BY id",
                libsql::params![source_id],
            )
            .await
            .unwrap();
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            out.push(row.get(0).unwrap());
        }
        out
    }

    async fn annotate(db: &MemoryDB, source: &str, source_id: &str, context: &str, notes: &str) {
        let conn = db.conn.lock().await;
        conn.execute(
            "INSERT INTO source_annotations
                (source, source_id, context_key, notes, page_ids, version, updated_at)
             VALUES (?1, ?2, ?3, ?4, '[\"page-1\"]', 3, 1700000000)",
            libsql::params![source, source_id, context, notes],
        )
        .await
        .unwrap();
    }

    async fn notes(db: &MemoryDB, source: &str, source_id: &str, context: &str) -> Option<String> {
        db.get_source_annotation(source, source_id, context)
            .await
            .unwrap()
            .map(|annotation| {
                assert_eq!(annotation.page_ids, ["page-1"]);
                assert_eq!(annotation.version, 3);
                annotation.notes
            })
    }

    #[tokio::test]
    async fn a_fresh_database_has_the_authored_content_column() {
        let (db, _tmp) = crate::db::tests::test_db().await;
        let conn = db.conn.lock().await;
        // A fresh database replays the whole chain from version 0, so it
        // gets the column from migration 144 like an upgraded one.
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM pragma_table_info('memories') WHERE name = 'authored_content'"
            )
            .await
            .unwrap(),
            1
        );
        assert!(count(&conn, "PRAGMA user_version").await.unwrap() >= 144);
    }

    #[tokio::test]
    async fn legacy_capture_migration_preserves_chunks_and_tags_and_skips_collisions() {
        let (db, _tmp) = crate::db::tests::test_db_at(143).await;
        seed(&db, "old-a", "manual", "manual_1700000000", 0).await;
        seed(&db, "old-b", "manual", "manual_1700000000", 1).await;
        seed(&db, "collision-old", "manual", "manual_1700000001", 0).await;
        seed(&db, "collision-current", "memory", "manual_1700000001", 1).await;
        for id in [
            "arbitrary",
            "manual_",
            "manual_12x",
            "manual_-12",
            "manual_08d389a3-0415-46ae-a198-f0cc1a15947b",
        ] {
            seed(&db, id, "manual", id, 0).await;
        }
        seed(&db, "other-source", "local_files", "manual_1700000002", 0).await;
        db.set_document_tags(
            "manual",
            "manual_1700000000",
            vec!["alpha".into(), "shared".into()],
        )
        .await
        .unwrap();
        db.set_document_tags(
            "memory",
            "manual_1700000000",
            vec!["beta".into(), "shared".into()],
        )
        .await
        .unwrap();
        db.set_document_tags("manual", "manual_1700000001", vec!["collision".into()])
            .await
            .unwrap();
        // Annotations move with their capture, per context. One whose context
        // already has a `memory` annotation (annotations outlive chunks) stays
        // under `manual` instead of overwriting the other note.
        annotate(&db, "manual", "manual_1700000000", "global", "moved global").await;
        annotate(
            &db,
            "manual",
            "manual_1700000000",
            "space-a",
            "kept in place",
        )
        .await;
        annotate(
            &db,
            "memory",
            "manual_1700000000",
            "space-a",
            "already there",
        )
        .await;
        annotate(
            &db,
            "manual",
            "manual_1700000001",
            "global",
            "collision note",
        )
        .await;
        annotate(&db, "manual", "manual_12x", "global", "not a capture").await;
        let before = snapshot(&db, "manual_1700000000").await;
        let collision = snapshot(&db, "manual_1700000001").await;
        db.migrate_144_legacy_quick_captures(143).await.unwrap();
        assert_eq!(
            notes(&db, "memory", "manual_1700000000", "global")
                .await
                .as_deref(),
            Some("moved global")
        );
        assert_eq!(
            notes(&db, "manual", "manual_1700000000", "global").await,
            None
        );
        assert_eq!(
            notes(&db, "memory", "manual_1700000000", "space-a")
                .await
                .as_deref(),
            Some("already there")
        );
        assert_eq!(
            notes(&db, "manual", "manual_1700000000", "space-a")
                .await
                .as_deref(),
            Some("kept in place")
        );
        assert_eq!(
            notes(&db, "manual", "manual_1700000001", "global")
                .await
                .as_deref(),
            Some("collision note")
        );
        assert_eq!(
            notes(&db, "memory", "manual_1700000001", "global").await,
            None
        );
        assert_eq!(
            notes(&db, "manual", "manual_12x", "global")
                .await
                .as_deref(),
            Some("not a capture")
        );
        assert_eq!(snapshot(&db, "manual_1700000000").await, before);
        assert_eq!(
            sources(&db, "manual_1700000000").await,
            ["memory", "memory"]
        );
        assert_eq!(snapshot(&db, "manual_1700000001").await, collision);
        assert_eq!(
            sources(&db, "manual_1700000001").await,
            ["memory", "manual"]
        );
        assert_eq!(
            db.get_document_tags("memory", "manual_1700000000")
                .await
                .unwrap(),
            ["alpha", "beta", "shared"]
        );
        assert!(db
            .get_document_tags("manual", "manual_1700000000")
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            db.get_document_tags("manual", "manual_1700000001")
                .await
                .unwrap(),
            ["collision"]
        );
        for id in [
            "arbitrary",
            "manual_",
            "manual_12x",
            "manual_-12",
            "manual_08d389a3-0415-46ae-a198-f0cc1a15947b",
        ] {
            assert_eq!(sources(&db, id).await, ["manual"]);
        }
        assert_eq!(sources(&db, "manual_1700000002").await, ["local_files"]);
        db.migrate_144_legacy_quick_captures(143).await.unwrap();
        assert_eq!(snapshot(&db, "manual_1700000000").await, before);
    }

    #[tokio::test]
    async fn legacy_capture_migration_rolls_back_tags_and_version_on_failure() {
        let (db, _tmp) = crate::db::tests::test_db_at(143).await;
        seed(&db, "old", "manual", "manual_1700000000", 0).await;
        db.set_document_tags("manual", "manual_1700000000", vec!["keep".into()])
            .await
            .unwrap();
        annotate(&db, "manual", "manual_1700000000", "global", "keep note").await;
        {
            let conn = db.conn.lock().await;
            conn.execute_batch(
                "PRAGMA user_version=143;
                CREATE TRIGGER fail_capture_repair BEFORE UPDATE OF source ON memories
                BEGIN SELECT RAISE(ABORT, 'injected migration failure'); END;",
            )
            .await
            .unwrap();
        }
        assert!(db.migrate_144_legacy_quick_captures(143).await.is_err());
        assert_eq!(sources(&db, "manual_1700000000").await, ["manual"]);
        assert_eq!(
            db.get_document_tags("manual", "manual_1700000000")
                .await
                .unwrap(),
            ["keep"]
        );
        assert!(db
            .get_document_tags("memory", "manual_1700000000")
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            notes(&db, "manual", "manual_1700000000", "global")
                .await
                .as_deref(),
            Some("keep note")
        );
        assert_eq!(
            notes(&db, "memory", "manual_1700000000", "global").await,
            None
        );
        {
            let conn = db.conn.lock().await;
            let version: i64 = conn
                .query("PRAGMA user_version", ())
                .await
                .unwrap()
                .next()
                .await
                .unwrap()
                .unwrap()
                .get(0)
                .unwrap();
            assert_eq!(version, 143);
            conn.execute("DROP TRIGGER fail_capture_repair", ())
                .await
                .unwrap();
        }
        db.migrate_144_legacy_quick_captures(143).await.unwrap();
        assert_eq!(sources(&db, "manual_1700000000").await, ["memory"]);
        assert_eq!(
            notes(&db, "memory", "manual_1700000000", "global")
                .await
                .as_deref(),
            Some("keep note")
        );
    }

    fn capture(source_id: &str, content: &str) -> crate::sources::RawDocument {
        crate::sources::RawDocument {
            source: "memory".into(),
            source_id: source_id.into(),
            title: "Capture".into(),
            content: content.into(),
            last_modified: 1700000000,
            metadata: [("capture_source".to_string(), "manual".to_string())].into(),
            ..Default::default()
        }
    }

    async fn authored(db: &MemoryDB, source_id: &str) -> Vec<Option<String>> {
        let conn = db.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT authored_content FROM memories WHERE source_id = ?1 ORDER BY chunk_index",
                libsql::params![source_id],
            )
            .await
            .unwrap();
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            out.push(row.get::<Option<String>>(0).unwrap());
        }
        out
    }

    #[tokio::test]
    async fn every_content_rewrite_keeps_the_authored_body_in_step() {
        let (db, _tmp) = crate::db::tests::test_db().await;
        let id = "manual_08d389a3-0415-46ae-a198-f0cc1a15947b";
        let typed = "  \n# Typed\n\n| a | b |\n| - | - |\n\nA line with a hard break  \n";
        db.upsert_documents(vec![capture(id, typed)]).await.unwrap();
        assert_eq!(authored(&db, id).await[0].as_deref(), Some(typed));
        assert_eq!(
            db.get_memory_detail(id).await.unwrap().unwrap().content,
            typed
        );

        // An edit (the Memory editor, update_chunk, MCP) rewrites the body.
        let edited = "  \n# Edited\n\n    indented code\n  \n";
        db.update_memory(id, edited).await.unwrap();
        assert_eq!(authored(&db, id).await, [Some(edited.to_string())]);
        assert_eq!(
            db.get_memory_detail(id).await.unwrap().unwrap().content,
            edited
        );

        // A full replacement re-chunks through ingest and keeps the flag.
        let replaced = "\n## Replaced\n\nNew body with trailing spaces  \n";
        db.update_document_content(id, replaced).await.unwrap();
        assert_eq!(authored(&db, id).await[0].as_deref(), Some(replaced));
        assert_eq!(
            db.get_memory_detail(id).await.unwrap().unwrap().content,
            replaced
        );

        // A re-ingest without the capture marker is an ordinary memory again:
        // no stale authored body is left to shadow the new chunks.
        let mut plain = capture(id, "Plain body from another writer.");
        plain.metadata.clear();
        db.upsert_documents(vec![plain]).await.unwrap();
        assert!(authored(&db, id).await.iter().all(Option::is_none));
        assert_eq!(
            db.get_memory_detail(id).await.unwrap().unwrap().content,
            "Plain body from another writer."
        );
    }

    #[tokio::test]
    async fn legacy_capture_move_reports_what_it_moved_and_skipped() {
        let (db, _tmp) = crate::db::tests::test_db_at(143).await;
        seed(&db, "a0", "manual", "manual_1700000000", 0).await;
        seed(&db, "a1", "manual", "manual_1700000000", 1).await;
        seed(&db, "b0", "manual", "manual_1700000005", 0).await;
        seed(&db, "c-old", "manual", "manual_1700000001", 0).await;
        seed(&db, "c-new", "memory", "manual_1700000001", 0).await;
        db.set_document_tags("manual", "manual_1700000000", vec!["x".into(), "y".into()])
            .await
            .unwrap();
        annotate(&db, "manual", "manual_1700000005", "global", "moved").await;
        annotate(&db, "manual", "manual_1700000005", "space-a", "kept").await;
        annotate(&db, "memory", "manual_1700000005", "space-a", "other").await;
        let conn = db.conn.lock().await;
        conn.execute("BEGIN IMMEDIATE", ()).await.unwrap();
        let moved = move_legacy_captures(&conn).await.unwrap();
        conn.execute("COMMIT", ()).await.unwrap();
        assert_eq!(
            moved,
            LegacyCaptureMove {
                documents: 2,
                chunks: 3,
                tags: 2,
                annotations: 1,
                skipped_documents: 1,
                skipped_annotations: 1,
            }
        );
    }
}
