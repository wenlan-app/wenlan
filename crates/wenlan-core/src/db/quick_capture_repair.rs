// SPDX-License-Identifier: Apache-2.0
use super::{commit_or_rollback, MemoryDB};
use crate::error::WenlanError;

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
        // Freeze document candidates before changing any chunks. A correlated
        // NOT EXISTS inside UPDATE would see the first promoted chunk and skip
        // its siblings. A collision skips the whole document, including tags.
        let result: Result<(), libsql::Error> = async {
            let mut columns = conn.query(
                "SELECT COUNT(*) FROM pragma_table_info('memories') WHERE name='authored_content'", (),
            ).await?;
            let exists = columns.next().await?.is_some_and(|row| row.get::<i64>(0).unwrap_or(0) > 0);
            drop(columns);
            if !exists {
                conn.execute("ALTER TABLE memories ADD COLUMN authored_content TEXT", ()).await?;
            }
            conn.execute_batch(
            "CREATE TEMP TABLE quick_capture_m144_ids (source_id TEXT PRIMARY KEY);
             INSERT INTO quick_capture_m144_ids
             SELECT DISTINCT old.source_id FROM memories old
             WHERE old.source = 'manual'
               AND old.source_id GLOB 'manual_[0-9]*'
               AND substr(old.source_id, 8) NOT GLOB '*[^0-9]*'
               AND NOT EXISTS (
                 SELECT 1 FROM memories existing
                 WHERE existing.source = 'memory' AND existing.source_id = old.source_id
               );
             INSERT OR IGNORE INTO document_tags (source, source_id, tag)
             SELECT 'memory', source_id, tag FROM document_tags
             WHERE source = 'manual' AND source_id IN (SELECT source_id FROM quick_capture_m144_ids);
             DELETE FROM document_tags
             WHERE source = 'manual' AND source_id IN (SELECT source_id FROM quick_capture_m144_ids);
             UPDATE memories SET source = 'memory'
             WHERE source = 'manual' AND source_id IN (SELECT source_id FROM quick_capture_m144_ids);
             DROP TABLE quick_capture_m144_ids;
             PRAGMA user_version = 144;"
            ).await?;
            Ok(())
        }.await;
        if let Err(error) = result {
            let _ = conn.execute("ROLLBACK", ()).await;
            return Err(WenlanError::VectorDb(format!(
                "m144 promote captures: {error}"
            )));
        }
        commit_or_rollback(&conn)
            .await
            .map_err(|e| WenlanError::VectorDb(format!("m144 commit: {e}")))?;
        Ok(())
    }
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
        let before = snapshot(&db, "manual_1700000000").await;
        let collision = snapshot(&db, "manual_1700000001").await;
        db.migrate_144_legacy_quick_captures(143).await.unwrap();
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
    }
}
