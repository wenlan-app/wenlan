// SPDX-License-Identifier: Apache-2.0
//! Looks: an HTML layout an AI tool drew for one page (migration 164,
//! UX-LOOK-1..6).
//!
//! A look is the page's own setting, not an original and not frontmatter, so
//! the page file and the vault stay byte for byte as they were. Every save is
//! a new row; the one before it stays as an inactive row so it can be put
//! back. A page has exactly one active look or none.

use super::MemoryDB;
use crate::error::WenlanError;
use wenlan_types::page_look::{PageLook, PageLookVersion};

fn db_err(context: &str) -> impl Fn(libsql::Error) -> WenlanError + '_ {
    move |e| WenlanError::VectorDb(format!("{context}: {e}"))
}

fn look_from_row(row: &libsql::Row) -> PageLook {
    PageLook {
        id: row.get::<i64>(0).unwrap_or_default(),
        html: row.get::<String>(1).unwrap_or_default(),
        api: row.get::<i64>(2).unwrap_or(1).max(0) as u32,
        kept_at: row.get::<i64>(3).unwrap_or_default(),
        session: row.get::<Option<String>>(4).unwrap_or(None),
        last_error: row.get::<Option<String>>(5).unwrap_or(None),
    }
}

const LOOK_COLUMNS: &str = "id, html, api, kept_at, session, last_error";

impl MemoryDB {
    /// Migration 164: the `page_looks` table. Created only when missing, so a
    /// crash before the version bump replays cleanly.
    pub(super) async fn migrate_164_page_looks(
        &self,
        prior_version: i64,
    ) -> Result<(), WenlanError> {
        self.backup_before_migration(164, prior_version).await?;
        let conn = self.conn.lock().await;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS page_looks (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 page_id TEXT NOT NULL,
                 html TEXT NOT NULL,
                 api INTEGER NOT NULL DEFAULT 1,
                 kept_at INTEGER NOT NULL,
                 session TEXT,
                 active INTEGER NOT NULL DEFAULT 0,
                 last_error TEXT
             );
             CREATE INDEX IF NOT EXISTS page_looks_page ON page_looks(page_id, kept_at);
             CREATE UNIQUE INDEX IF NOT EXISTS page_looks_one_active
                 ON page_looks(page_id) WHERE active = 1;",
        )
        .await
        .map_err(db_err("m164 table"))?;
        conn.execute("PRAGMA user_version = 164", ())
            .await
            .map_err(db_err("m164 bump"))?;
        log::info!("[migration] Migration 164 applied: page looks");
        Ok(())
    }

    /// The page's active look, or `None` when it has none.
    pub async fn get_active_page_look(
        &self,
        page_id: &str,
    ) -> Result<Option<PageLook>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                &format!(
                    "SELECT {LOOK_COLUMNS} FROM page_looks
                      WHERE page_id = ?1 AND active = 1"
                ),
                libsql::params![page_id],
            )
            .await
            .map_err(db_err("get active look"))?;
        Ok(rows
            .next()
            .await
            .map_err(db_err("active look row"))?
            .map(|row| look_from_row(&row)))
    }

    /// Whether each of `page_ids` has an active look, for a list of pages.
    pub async fn page_ids_with_look(
        &self,
        page_ids: &[String],
    ) -> Result<std::collections::HashSet<String>, WenlanError> {
        let mut found = std::collections::HashSet::new();
        if page_ids.is_empty() {
            return Ok(found);
        }
        let conn = self.conn.lock().await;
        for chunk in page_ids.chunks(500) {
            let marks = vec!["?"; chunk.len()].join(",");
            let mut rows = conn
                .query(
                    &format!(
                        "SELECT page_id FROM page_looks WHERE active = 1 AND page_id IN ({marks})"
                    ),
                    chunk
                        .iter()
                        .map(|id| libsql::Value::Text(id.clone()))
                        .collect::<Vec<_>>(),
                )
                .await
                .map_err(db_err("pages with look"))?;
            while let Some(row) = rows.next().await.map_err(db_err("pages with look row"))? {
                if let Ok(id) = row.get::<String>(0) {
                    found.insert(id);
                }
            }
        }
        Ok(found)
    }

    /// Save `html` as the page's look. The previous active look stays as an
    /// inactive version. Saving the HTML that is already active changes
    /// nothing and returns `(its id, false)`.
    pub async fn set_page_look(
        &self,
        page_id: &str,
        html: &str,
        api: u32,
        session: Option<&str>,
    ) -> Result<(i64, bool), WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT id, html FROM page_looks WHERE page_id = ?1 AND active = 1",
                libsql::params![page_id],
            )
            .await
            .map_err(db_err("set look read"))?;
        if let Some(row) = rows.next().await.map_err(db_err("set look row"))? {
            let id = row.get::<i64>(0).unwrap_or_default();
            if row.get::<String>(1).ok().as_deref() == Some(html) {
                return Ok((id, false));
            }
        }
        drop(rows);
        conn.execute("BEGIN IMMEDIATE", ())
            .await
            .map_err(db_err("set look begin"))?;
        let result = async {
            conn.execute(
                "UPDATE page_looks SET active = 0 WHERE page_id = ?1 AND active = 1",
                libsql::params![page_id],
            )
            .await
            .map_err(db_err("set look deactivate"))?;
            conn.execute(
                "INSERT INTO page_looks (page_id, html, api, kept_at, session, active)
                 VALUES (?1, ?2, ?3, ?4, ?5, 1)",
                libsql::params![
                    page_id,
                    html,
                    i64::from(api),
                    chrono::Utc::now().timestamp(),
                    session
                ],
            )
            .await
            .map_err(db_err("set look insert"))?;
            Ok::<i64, WenlanError>(conn.last_insert_rowid())
        }
        .await;
        match result {
            Ok(id) => {
                conn.execute("COMMIT", ())
                    .await
                    .map_err(db_err("set look commit"))?;
                Ok((id, true))
            }
            Err(error) => {
                let _ = conn.execute("ROLLBACK", ()).await;
                Err(error)
            }
        }
    }

    /// Take the page's look off. Every version stays, none is active. Returns
    /// the version that was active, so the removal can be undone.
    pub async fn remove_page_look(&self, page_id: &str) -> Result<Option<i64>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT id FROM page_looks WHERE page_id = ?1 AND active = 1",
                libsql::params![page_id],
            )
            .await
            .map_err(db_err("remove look read"))?;
        let active = rows
            .next()
            .await
            .map_err(db_err("remove look row"))?
            .and_then(|row| row.get::<i64>(0).ok());
        drop(rows);
        if active.is_some() {
            conn.execute(
                "UPDATE page_looks SET active = 0 WHERE page_id = ?1",
                libsql::params![page_id],
            )
            .await
            .map_err(db_err("remove look"))?;
        }
        Ok(active)
    }

    /// Make an earlier version the page's look. `version` names it; `None`
    /// takes the newest version that is not active. Returns the version now
    /// active, or `None` when there is none to bring back.
    pub async fn restore_page_look(
        &self,
        page_id: &str,
        version: Option<i64>,
    ) -> Result<Option<i64>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = match version {
            Some(id) => {
                conn.query(
                    "SELECT id FROM page_looks WHERE page_id = ?1 AND id = ?2",
                    libsql::params![page_id, id],
                )
                .await
            }
            None => {
                conn.query(
                    "SELECT id FROM page_looks WHERE page_id = ?1 AND active = 0
                      ORDER BY kept_at DESC, id DESC LIMIT 1",
                    libsql::params![page_id],
                )
                .await
            }
        }
        .map_err(db_err("restore look read"))?;
        let Some(target) = rows
            .next()
            .await
            .map_err(db_err("restore look row"))?
            .and_then(|row| row.get::<i64>(0).ok())
        else {
            return Ok(None);
        };
        drop(rows);
        conn.execute("BEGIN IMMEDIATE", ())
            .await
            .map_err(db_err("restore look begin"))?;
        let result = async {
            conn.execute(
                "UPDATE page_looks SET active = 0 WHERE page_id = ?1 AND active = 1",
                libsql::params![page_id],
            )
            .await
            .map_err(db_err("restore look deactivate"))?;
            conn.execute(
                "UPDATE page_looks SET active = 1, last_error = NULL WHERE id = ?1",
                libsql::params![target],
            )
            .await
            .map_err(db_err("restore look activate"))?;
            Ok::<(), WenlanError>(())
        }
        .await;
        match result {
            Ok(()) => {
                conn.execute("COMMIT", ())
                    .await
                    .map_err(db_err("restore look commit"))?;
                Ok(Some(target))
            }
            Err(error) => {
                let _ = conn.execute("ROLLBACK", ()).await;
                Err(error)
            }
        }
    }

    /// Every kept version of the page's look, newest first.
    pub async fn list_page_look_versions(
        &self,
        page_id: &str,
    ) -> Result<Vec<PageLookVersion>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT id, kept_at, session, active, LENGTH(CAST(html AS BLOB))
                   FROM page_looks WHERE page_id = ?1
                  ORDER BY kept_at DESC, id DESC",
                libsql::params![page_id],
            )
            .await
            .map_err(db_err("list look versions"))?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.map_err(db_err("look version row"))? {
            out.push(PageLookVersion {
                id: row.get::<i64>(0).unwrap_or_default(),
                kept_at: row.get::<i64>(1).unwrap_or_default(),
                session: row.get::<Option<String>>(2).unwrap_or(None),
                active: row.get::<i64>(3).unwrap_or_default() == 1,
                bytes: row.get::<i64>(4).unwrap_or_default(),
            });
        }
        Ok(out)
    }

    /// Record what went wrong while showing the active look, so the AI can
    /// read it when asked to fix the look. Returns false when the page has no
    /// active look.
    pub async fn record_page_look_error(
        &self,
        page_id: &str,
        message: &str,
    ) -> Result<bool, WenlanError> {
        let conn = self.conn.lock().await;
        let changed = conn
            .execute(
                "UPDATE page_looks SET last_error = ?2 WHERE page_id = ?1 AND active = 1",
                libsql::params![page_id, message],
            )
            .await
            .map_err(db_err("record look error"))?;
        Ok(changed > 0)
    }
}
