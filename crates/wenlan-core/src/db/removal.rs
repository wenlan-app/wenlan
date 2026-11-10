// SPDX-License-Identifier: Apache-2.0
//! Removals that can be undone, each with its `agent_activity` record written
//! in the same transaction.
//!
//! - A page archive or delete records who asked, the page id, its title and
//!   version ([`PageRemovalAudit`], written by `archive_page_recorded` and
//!   `delete_page_recorded`).
//! - Forgetting a memory retires its rows with `supersede_mode = 'evicted'`
//!   instead of deleting them: every reader already leaves evicted rows out
//!   (`not_self_archived`). The changelog keeps the mode each row had, so
//!   [`MemoryDB::restore_forgotten_memory`] puts it back exactly.

use super::MemoryDB;
use crate::error::WenlanError;

/// Who archived or deleted a page, for its `agent_activity` row.
#[derive(Debug, Clone, Copy)]
pub struct PageRemovalAudit<'a> {
    /// The caller's name: the agent name, or `person` for the person in the
    /// app.
    pub agent: &'a str,
    /// The page's title at the time.
    pub title: &'a str,
}

/// What [`MemoryDB::forget_memory`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForgetOutcome {
    /// No memory has this id.
    NotFound,
    /// The memory was forgotten now.
    Forgotten,
    /// The memory had already been forgotten; nothing changed.
    AlreadyForgotten,
}

/// The changelog `retirement` a forget writes, and a restore looks for.
const FORGET_RETIREMENT: &str = "forget";

struct ForgetRow {
    id: String,
    version: i64,
    changelog: Vec<serde_json::Value>,
    mode: Option<String>,
}

fn db_err(context: &str) -> impl Fn(libsql::Error) -> WenlanError + '_ {
    move |e| WenlanError::VectorDb(format!("{context}: {e}"))
}

impl MemoryDB {
    /// Forget memory `source_id` so that it can be restored: its rows leave
    /// search, lists and page compiles, the Pages built from it are marked to
    /// rebuild, and one `forget` row naming `agent` is written to
    /// `agent_activity` with `detail`. Nothing is deleted.
    pub async fn forget_memory(
        &self,
        source_id: &str,
        agent: &str,
        detail: &str,
    ) -> Result<ForgetOutcome, WenlanError> {
        let conn = self.conn.lock().await;
        conn.execute("BEGIN", ())
            .await
            .map_err(db_err("forget_memory begin"))?;
        let result = async {
            let rows = Self::forget_rows(&conn, source_id).await?;
            if rows.is_empty() {
                return Ok(ForgetOutcome::NotFound);
            }
            let live: Vec<ForgetRow> = rows
                .into_iter()
                .filter(|row| row.mode.as_deref() != Some("evicted"))
                .collect();
            if live.is_empty() {
                return Ok(ForgetOutcome::AlreadyForgotten);
            }
            let now = chrono::Utc::now().timestamp();
            for mut row in live {
                let version = row.version + 1;
                row.changelog.push(serde_json::json!({
                    "version": version,
                    "at": now,
                    "delta": "Forgotten; restore brings it back",
                    "source_agent": agent,
                    "previous_supersede_mode": row.mode,
                    "retirement": FORGET_RETIREMENT,
                }));
                let changelog = serde_json::to_string(&row.changelog)
                    .map_err(|e| WenlanError::VectorDb(format!("forget changelog: {e}")))?;
                conn.execute(
                    "UPDATE memories SET supersede_mode = 'evicted', version = ?1, changelog = ?2 \
                     WHERE id = ?3",
                    libsql::params![version, changelog, row.id.as_str()],
                )
                .await
                .map_err(db_err("forget_memory row"))?;
            }
            Self::mark_pages_depending_on_memory_source(&conn, "memory", source_id).await?;
            Self::log_agent_activity_on_connection(
                &conn,
                agent,
                "forget",
                std::slice::from_ref(&source_id.to_string()),
                None,
                detail,
                now,
            )
            .await
            .map_err(db_err("forget_memory activity"))?;
            Ok(ForgetOutcome::Forgotten)
        }
        .await;
        let outcome = Self::finish_removal_transaction(&conn, result, "forget_memory").await;
        match &outcome {
            Ok(ForgetOutcome::Forgotten) => {
                log::info!("[forget] memory {source_id} forgotten by {agent}");
            }
            Ok(_) => {}
            Err(error) => log::warn!("[forget] memory {source_id} by {agent} failed: {error}"),
        }
        outcome
    }

    /// Put back memory `source_id` after [`Self::forget_memory`]: each row
    /// forgotten that way gets the mode it had back, the Pages built from it
    /// are marked to rebuild, and a `memory_restore` row naming `agent` is
    /// written. Rows evicted for any other reason stay evicted. Returns
    /// whether anything was restored.
    pub async fn restore_forgotten_memory(
        &self,
        source_id: &str,
        agent: &str,
    ) -> Result<bool, WenlanError> {
        let conn = self.conn.lock().await;
        conn.execute("BEGIN", ())
            .await
            .map_err(db_err("restore_forgotten_memory begin"))?;
        let result = async {
            let now = chrono::Utc::now().timestamp();
            let mut restored = false;
            for mut row in Self::forget_rows(&conn, source_id).await? {
                if row.mode.as_deref() != Some("evicted") {
                    continue;
                }
                let Some(last) = row.changelog.last() else {
                    continue;
                };
                if last.get("retirement").and_then(serde_json::Value::as_str)
                    != Some(FORGET_RETIREMENT)
                {
                    continue;
                }
                let previous = last
                    .get("previous_supersede_mode")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);
                let version = row.version + 1;
                row.changelog.push(serde_json::json!({
                    "version": version,
                    "at": now,
                    "delta": "Restored after it was forgotten",
                    "source_agent": agent,
                    "retirement": null,
                    "restored_from": FORGET_RETIREMENT,
                }));
                let changelog = serde_json::to_string(&row.changelog)
                    .map_err(|e| WenlanError::VectorDb(format!("restore changelog: {e}")))?;
                conn.execute(
                    "UPDATE memories SET supersede_mode = ?1, version = ?2, changelog = ?3 \
                     WHERE id = ?4 AND supersede_mode = 'evicted'",
                    libsql::params![previous, version, changelog, row.id.as_str()],
                )
                .await
                .map_err(db_err("restore_forgotten_memory row"))?;
                restored = true;
            }
            if restored {
                Self::mark_pages_depending_on_memory_source(&conn, "memory", source_id).await?;
                Self::log_agent_activity_on_connection(
                    &conn,
                    agent,
                    "memory_restore",
                    std::slice::from_ref(&source_id.to_string()),
                    None,
                    "restored a forgotten memory",
                    now,
                )
                .await
                .map_err(db_err("restore_forgotten_memory activity"))?;
            }
            Ok(restored)
        }
        .await;
        Self::finish_removal_transaction(&conn, result, "restore_forgotten_memory").await
    }

    async fn forget_rows(
        conn: &libsql::Connection,
        source_id: &str,
    ) -> Result<Vec<ForgetRow>, WenlanError> {
        let mut rows = conn
            .query(
                "SELECT id, COALESCE(version, 1), changelog, supersede_mode FROM memories \
                 WHERE source = 'memory' AND source_id = ?1",
                libsql::params![source_id],
            )
            .await
            .map_err(db_err("forget rows"))?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.map_err(db_err("forget row"))? {
            let changelog = row
                .get::<Option<String>>(2)
                .ok()
                .flatten()
                .filter(|value| !value.trim().is_empty())
                .and_then(|value| serde_json::from_str(&value).ok())
                .unwrap_or_default();
            out.push(ForgetRow {
                id: row.get::<String>(0).map_err(db_err("forget row id"))?,
                version: row.get::<i64>(1).map_err(db_err("forget row version"))?,
                changelog,
                mode: row.get::<Option<String>>(3).ok().flatten(),
            });
        }
        Ok(out)
    }

    async fn finish_removal_transaction<T>(
        conn: &libsql::Connection,
        result: Result<T, WenlanError>,
        context: &str,
    ) -> Result<T, WenlanError> {
        match result {
            Ok(value) => {
                super::commit_or_rollback(conn)
                    .await
                    .map_err(|e| WenlanError::VectorDb(format!("{context} commit: {e}")))?;
                Ok(value)
            }
            Err(error) => {
                let _ = conn.execute("ROLLBACK", ()).await;
                Err(error)
            }
        }
    }
}

#[cfg(test)]
#[path = "removal_tests.rs"]
mod tests;
