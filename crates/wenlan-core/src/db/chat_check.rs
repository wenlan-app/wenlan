// SPDX-License-Identifier: Apache-2.0
//! Storage for the in-chat check and the procedural page (phase A, A6/A8).
//!
//! Migration 145 adds two things, both in the shape phase B exports
//! mechanically:
//!
//! - `chat_reminders`: one row per reminder handed out. `entry` is the exact
//!   history entry (`op: chat.reminder`, spec section 4) with `seq` equal to
//!   the row's `seq`; the other columns only index it for the daily cap.
//! - `pages.role`: the page's role (`instructions` for the page that says how
//!   the person wants AI tools to work). It becomes the `role:` frontmatter
//!   key in phase B.
use super::MemoryDB;
use crate::error::WenlanError;

const CHAT_REMINDERS_DDL: &str = "\
CREATE TABLE IF NOT EXISTS chat_reminders (
    seq INTEGER PRIMARY KEY,
    ts INTEGER NOT NULL,
    session TEXT,
    target_type TEXT NOT NULL,
    target_id TEXT NOT NULL,
    entry TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_chat_reminders_ts ON chat_reminders(ts);";

fn db_err(context: &str) -> impl Fn(libsql::Error) -> WenlanError + '_ {
    move |e| WenlanError::VectorDb(format!("{context}: {e}"))
}

impl MemoryDB {
    /// Migration 145: `chat_reminders` and `pages.role`. The table is `IF NOT
    /// EXISTS` and the column add is guarded, so a crash before the version
    /// bump replays cleanly.
    pub(super) async fn migrate_145_chat_reminders_and_page_role(
        &self,
        prior_version: i64,
    ) -> Result<(), WenlanError> {
        self.backup_before_migration(145, prior_version).await?;
        let conn = self.conn.lock().await;
        conn.execute_batch(CHAT_REMINDERS_DDL)
            .await
            .map_err(db_err("m145 chat_reminders DDL"))?;
        let has_role: bool = {
            let mut rows = conn
                .query(
                    "SELECT COUNT(*) FROM pragma_table_info('pages') WHERE name = 'role'",
                    (),
                )
                .await
                .map_err(db_err("m145 role check"))?;
            match rows.next().await {
                Ok(Some(row)) => row.get::<i64>(0).unwrap_or(0) > 0,
                _ => false,
            }
        };
        if !has_role {
            conn.execute("ALTER TABLE pages ADD COLUMN role TEXT", ())
                .await
                .map_err(db_err("m145 add pages.role"))?;
        }
        conn.execute("PRAGMA user_version = 145", ())
            .await
            .map_err(db_err("m145 bump"))?;
        log::info!("[migration] Migration 145 applied: chat_reminders and pages.role");
        Ok(())
    }

    /// Append one `chat.reminder` entry. `build` receives the sequence number
    /// the entry gets and returns the entry JSON; the row and its `seq` are
    /// written in one transaction, so two reminders never share a number.
    pub async fn append_chat_reminder<F>(
        &self,
        ts: i64,
        session: Option<&str>,
        target_type: &str,
        target_id: &str,
        build: F,
    ) -> Result<(i64, serde_json::Value), WenlanError>
    where
        F: FnOnce(i64) -> serde_json::Value,
    {
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(libsql::TransactionBehavior::Immediate)
            .await
            .map_err(db_err("chat_reminder begin"))?;
        let seq = {
            let mut rows = tx
                .query("SELECT COALESCE(MAX(seq), 0) + 1 FROM chat_reminders", ())
                .await
                .map_err(db_err("chat_reminder seq"))?;
            match rows.next().await.map_err(db_err("chat_reminder seq row"))? {
                Some(row) => row.get::<i64>(0).unwrap_or(1),
                None => 1,
            }
        };
        let entry = build(seq);
        tx.execute(
            "INSERT INTO chat_reminders (seq, ts, session, target_type, target_id, entry) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            libsql::params![
                seq,
                ts,
                session.map(str::to_string),
                target_type,
                target_id,
                entry.to_string()
            ],
        )
        .await
        .map_err(db_err("chat_reminder insert"))?;
        tx.commit().await.map_err(db_err("chat_reminder commit"))?;
        Ok((seq, entry))
    }

    /// How many reminders were handed out at or after `since` (unix seconds).
    pub async fn count_chat_reminders_since(&self, since: i64) -> Result<u32, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT COUNT(*) FROM chat_reminders WHERE ts >= ?1",
                libsql::params![since],
            )
            .await
            .map_err(db_err("count_chat_reminders_since"))?;
        Ok(match rows.next().await.map_err(db_err("count row"))? {
            Some(row) => u32::try_from(row.get::<i64>(0).unwrap_or(0)).unwrap_or(u32::MAX),
            None => 0,
        })
    }

    /// Targets already reminded at or after `since` in `session` (any
    /// session when `None`).
    pub async fn chat_reminder_targets_since(
        &self,
        since: i64,
        session: Option<&str>,
    ) -> Result<std::collections::HashSet<String>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = match session {
            Some(session) => {
                conn.query(
                    "SELECT target_id FROM chat_reminders WHERE ts >= ?1 AND session = ?2",
                    libsql::params![since, session],
                )
                .await
            }
            None => {
                conn.query(
                    "SELECT target_id FROM chat_reminders WHERE ts >= ?1",
                    libsql::params![since],
                )
                .await
            }
        }
        .map_err(db_err("chat_reminder_targets_since"))?;
        let mut out = std::collections::HashSet::new();
        while let Some(row) = rows.next().await.map_err(db_err("targets row"))? {
            out.insert(row.get::<String>(0).unwrap_or_default());
        }
        Ok(out)
    }

    /// Every `chat.reminder` entry, oldest first (for export and tests).
    pub async fn list_chat_reminder_entries(&self) -> Result<Vec<serde_json::Value>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query("SELECT entry FROM chat_reminders ORDER BY seq", ())
            .await
            .map_err(db_err("list_chat_reminder_entries"))?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.map_err(db_err("entry row"))? {
            let raw = row.get::<String>(0).unwrap_or_default();
            if let Ok(value) = serde_json::from_str(&raw) {
                out.push(value);
            }
        }
        Ok(out)
    }

    /// The newest active, non-entity page with `role`, if any.
    pub async fn find_page_id_by_role(&self, role: &str) -> Result<Option<String>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT id FROM pages \
                 WHERE role = ?1 AND status = 'active' \
                   AND COALESCE(kind, 'concept') != 'entity' \
                 ORDER BY last_modified DESC, id LIMIT 1",
                libsql::params![role],
            )
            .await
            .map_err(db_err("find_page_id_by_role"))?;
        Ok(match rows.next().await.map_err(db_err("role row"))? {
            Some(row) => row.get::<Option<String>>(0).unwrap_or(None),
            None => None,
        })
    }

    /// Set a page's role. Page metadata, like the review lock: it does not
    /// bump `version`. Returns `false` when no non-entity page has the id.
    pub async fn set_page_role(
        &self,
        page_id: &str,
        role: Option<&str>,
    ) -> Result<bool, WenlanError> {
        let conn = self.conn.lock().await;
        let changed = conn
            .execute(
                "UPDATE pages SET role = ?1 \
                 WHERE id = ?2 AND COALESCE(kind, 'concept') != 'entity'",
                libsql::params![role.map(str::to_string), page_id],
            )
            .await
            .map_err(db_err("set_page_role"))?;
        Ok(changed > 0)
    }

    /// A page's role, `None` when it has none or does not exist.
    pub async fn page_role(&self, page_id: &str) -> Result<Option<String>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT role FROM pages WHERE id = ?1",
                libsql::params![page_id],
            )
            .await
            .map_err(db_err("page_role"))?;
        Ok(match rows.next().await.map_err(db_err("page_role row"))? {
            Some(row) => row.get::<Option<String>>(0).unwrap_or(None),
            None => None,
        })
    }
}
