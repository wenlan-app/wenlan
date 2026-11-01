// SPDX-License-Identifier: Apache-2.0
//! Originals kept from imported chats (migration 147).
//!
//! A chat export carries more than text: Claude artifacts and ChatGPT
//! canvases. The importer keeps each as a file under
//! `<data root>/originals/<vendor>/<conversation id>/` and gives it a page
//! whose `creation_kind` is `original`. This table records, for each such
//! page, where the original came from (vendor, the vendor's conversation id
//! and URL, the artifact's own id) and where its file is, so the page can
//! open the conversation in the original AI tool.
//!
//! Migration 147 also widens `pages.creation_kind`'s CHECK to admit
//! `'original'`, with the same `writable_schema` text patch migration 92 used
//! to admit `'entity'`.

use super::MemoryDB;
use crate::error::WenlanError;

pub(super) const ORIGINALS_DDL: &str = "
    CREATE TABLE IF NOT EXISTS originals (
        page_id TEXT PRIMARY KEY,
        vendor TEXT NOT NULL,
        conversation_id TEXT NOT NULL,
        artifact_id TEXT NOT NULL,
        kind TEXT NOT NULL,
        title TEXT,
        file_path TEXT NOT NULL,
        conversation_url TEXT NOT NULL,
        written_at INTEGER,
        imported_at INTEGER NOT NULL,
        UNIQUE(vendor, conversation_id, artifact_id)
    );
    CREATE INDEX IF NOT EXISTS originals_conversation ON originals(vendor, conversation_id);
";

const CREATION_KIND_CHECK_BEFORE: &str =
    "creation_kind IN ('distilled','authored','research','imported','source','entity')";
const CREATION_KIND_CHECK_AFTER: &str =
    "creation_kind IN ('distilled','authored','research','imported','source','entity','original')";

/// One kept original, as recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OriginalRecord {
    pub page_id: String,
    pub vendor: String,
    pub conversation_id: String,
    pub artifact_id: String,
    pub kind: String,
    pub title: Option<String>,
    /// Relative to the data root, with `/` separators.
    pub file_path: String,
    pub conversation_url: String,
    /// When the AI last wrote it in the chat (unix seconds), when known.
    pub written_at: Option<i64>,
    pub imported_at: i64,
}

fn db_error(context: &str) -> impl Fn(libsql::Error) -> WenlanError + '_ {
    move |e| WenlanError::VectorDb(format!("{context}: {e}"))
}

impl MemoryDB {
    /// Migration 147: the `originals` table and `'original'` in
    /// `pages.creation_kind`'s CHECK. Both steps are idempotent.
    pub(super) async fn migrate_147_originals(
        &self,
        prior_version: i64,
    ) -> Result<(), WenlanError> {
        self.backup_before_migration(147, prior_version).await?;
        let conn = self.conn.lock().await;
        conn.execute_batch(ORIGINALS_DDL)
            .await
            .map_err(db_error("m147 originals"))?;
        let current_sql: Option<String> = {
            let mut rows = conn
                .query(
                    "SELECT sql FROM sqlite_master WHERE type='table' AND name='pages'",
                    (),
                )
                .await
                .map_err(db_error("m147 read pages sql"))?;
            match rows.next().await.map_err(db_error("m147 pages sql row"))? {
                Some(row) => Some(
                    row.get::<String>(0)
                        .map_err(db_error("m147 pages sql col"))?,
                ),
                None => None,
            }
        };
        if let Some(sql) = current_sql {
            let patched = sql.replace(CREATION_KIND_CHECK_BEFORE, CREATION_KIND_CHECK_AFTER);
            if patched != sql {
                conn.execute("PRAGMA writable_schema=ON", ())
                    .await
                    .map_err(db_error("m147 writable on"))?;
                // Always turn writable_schema back off, even if the patch
                // fails, as migration 92 does.
                let patch = conn
                    .execute(
                        "UPDATE sqlite_master SET sql=?1 WHERE type='table' AND name='pages'",
                        libsql::params![patched],
                    )
                    .await
                    .map_err(db_error("m147 patch schema"));
                conn.execute("PRAGMA writable_schema=RESET", ())
                    .await
                    .map_err(db_error("m147 writable reset"))?;
                patch?;
            } else if !sql.contains(CREATION_KIND_CHECK_AFTER) && sql.contains("creation_kind IN") {
                log::warn!(
                    "[migration] m147: pages.creation_kind CHECK has an unexpected form; \
                     'original' pages cannot be created until it is widened"
                );
            }
        }
        conn.execute("PRAGMA user_version = 147", ())
            .await
            .map_err(db_error("m147 bump"))?;
        log::info!("[migration] Migration 147 applied: originals, creation_kind 'original'");
        Ok(())
    }

    /// Record a kept original. A second record for the same page, or for the
    /// same vendor, conversation and artifact, is refused.
    pub async fn insert_original(&self, record: &OriginalRecord) -> Result<(), WenlanError> {
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO originals
                 (page_id, vendor, conversation_id, artifact_id, kind, title, file_path,
                  conversation_url, written_at, imported_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            libsql::params![
                record.page_id.as_str(),
                record.vendor.as_str(),
                record.conversation_id.as_str(),
                record.artifact_id.as_str(),
                record.kind.as_str(),
                record.title.as_deref(),
                record.file_path.as_str(),
                record.conversation_url.as_str(),
                record.written_at,
                record.imported_at,
            ],
        )
        .await
        .map_err(db_error("insert original"))?;
        Ok(())
    }

    /// The original a page was made from, if it was made from one.
    pub async fn get_original(&self, page_id: &str) -> Result<Option<OriginalRecord>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT page_id, vendor, conversation_id, artifact_id, kind, title, file_path,
                        conversation_url, written_at, imported_at
                 FROM originals WHERE page_id = ?1",
                libsql::params![page_id],
            )
            .await
            .map_err(db_error("get original"))?;
        Ok(rows
            .next()
            .await
            .map_err(db_error("get original row"))?
            .map(|row| record_from_row(&row)))
    }

    /// Every original kept for one conversation, in artifact id order.
    pub async fn originals_for_conversation(
        &self,
        vendor: &str,
        conversation_id: &str,
    ) -> Result<Vec<OriginalRecord>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT page_id, vendor, conversation_id, artifact_id, kind, title, file_path,
                        conversation_url, written_at, imported_at
                 FROM originals WHERE vendor = ?1 AND conversation_id = ?2
                 ORDER BY artifact_id",
                libsql::params![vendor, conversation_id],
            )
            .await
            .map_err(db_error("originals for conversation"))?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.map_err(db_error("original row"))? {
            out.push(record_from_row(&row));
        }
        Ok(out)
    }
}

fn record_from_row(row: &libsql::Row) -> OriginalRecord {
    OriginalRecord {
        page_id: row.get::<String>(0).unwrap_or_default(),
        vendor: row.get::<String>(1).unwrap_or_default(),
        conversation_id: row.get::<String>(2).unwrap_or_default(),
        artifact_id: row.get::<String>(3).unwrap_or_default(),
        kind: row.get::<String>(4).unwrap_or_default(),
        title: row.get::<Option<String>>(5).unwrap_or(None),
        file_path: row.get::<String>(6).unwrap_or_default(),
        conversation_url: row.get::<String>(7).unwrap_or_default(),
        written_at: row.get::<Option<i64>>(8).unwrap_or(None),
        imported_at: row.get::<i64>(9).unwrap_or(0),
    }
}

#[cfg(test)]
impl MemoryDB {
    /// Test-only: one integer from a raw query.
    pub(crate) async fn originals_scalar_for_test(&self, sql: &str) -> i64 {
        let conn = self.conn.lock().await;
        let mut rows = conn.query(sql, ()).await.expect("test query");
        rows.next()
            .await
            .expect("test row")
            .expect("one row")
            .get::<i64>(0)
            .expect("an integer")
    }
}
