// SPDX-License-Identifier: Apache-2.0
//! Who archived or restored a page (migration 165), and which AI tool made
//! it.
//!
//! `page_history.edited_by` keeps its `archive` / `restore` marker; the new,
//! nullable `actor` column says who asked, in the writer vocabulary the
//! history already speaks: `manual_edit` for the person at this computer,
//! `shared_person:<member>` for a team member in person, `ai:<agent>` for an
//! AI tool. Rows written before it stay `NULL` and are shown without a name;
//! no name is ever guessed for them. A page's `agent_create` first row keeps
//! the AI tool's own name (`claude-code`) there.

use std::collections::HashMap;

use super::MemoryDB;
use crate::error::WenlanError;

fn db_err(context: &str) -> impl Fn(libsql::Error) -> WenlanError + '_ {
    move |e| WenlanError::VectorDb(format!("{context}: {e}"))
}

/// The actor the person at this computer is recorded as.
pub const PERSON_ACTOR: &str = "manual_edit";

/// The actor an AI tool is recorded as.
pub fn ai_actor(agent: &str) -> String {
    format!("ai:{agent}")
}

impl MemoryDB {
    /// Migration 165: `page_history.actor`. Added only when missing, so a
    /// crash before the version bump replays cleanly.
    pub(super) async fn migrate_165_page_history_actor(
        &self,
        prior_version: i64,
    ) -> Result<(), WenlanError> {
        self.backup_before_migration(165, prior_version).await?;
        let conn = self.conn.lock().await;
        let has_column = {
            let mut rows = conn
                .query(
                    "SELECT 1 FROM pragma_table_info('page_history') WHERE name = 'actor'",
                    (),
                )
                .await
                .map_err(db_err("m165 columns"))?;
            rows.next()
                .await
                .map_err(db_err("m165 column row"))?
                .is_some()
        };
        if !has_column {
            conn.execute("ALTER TABLE page_history ADD COLUMN actor TEXT", ())
                .await
                .map_err(db_err("m165 add column"))?;
        }
        conn.execute("PRAGMA user_version = 165", ())
            .await
            .map_err(db_err("m165 bump"))?;
        log::info!("[migration] Migration 165 applied: who archived or restored a page");
        Ok(())
    }

    /// Records who archived or restored `page_id` at `version`: only on that
    /// version's `archive` or `restore` row, and only when none is recorded.
    pub async fn stamp_page_history_actor(
        &self,
        page_id: &str,
        version: i64,
        actor: &str,
    ) -> Result<(), WenlanError> {
        let conn = self.conn.lock().await;
        conn.execute(
            "UPDATE page_history SET actor = ?1
              WHERE page_id = ?2 AND version = ?3 AND actor IS NULL
                AND edited_by IN ('archive', 'restore')",
            libsql::params![actor, page_id, version],
        )
        .await
        .map_err(db_err("stamp page history actor"))?;
        Ok(())
    }

    /// The `archive` and `restore` rows of `page_id`, newest first:
    /// (version, edited_by, created_at, actor).
    pub async fn page_lifecycle_rows(
        &self,
        page_id: &str,
    ) -> Result<Vec<(i64, String, i64, Option<String>)>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT version, edited_by, created_at, actor FROM page_history
                  WHERE page_id = ?1 AND edited_by IN ('archive', 'restore')
                  ORDER BY version DESC",
                libsql::params![page_id],
            )
            .await
            .map_err(db_err("page lifecycle rows"))?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.map_err(db_err("page lifecycle row"))? {
            out.push((
                row.get::<i64>(0).unwrap_or_default(),
                row.get::<String>(1).unwrap_or_default(),
                row.get::<i64>(2).unwrap_or_default(),
                row.get::<Option<String>>(3).unwrap_or(None),
            ));
        }
        Ok(out)
    }

    /// The marker and actor of `page_id`'s history row at `version`, when it
    /// is an `archive` or `restore` row.
    pub(crate) async fn page_lifecycle_at(
        &self,
        page_id: &str,
        version: i64,
    ) -> Option<(String, Option<String>)> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT edited_by, actor FROM page_history
                  WHERE page_id = ?1 AND version = ?2 AND edited_by IN ('archive', 'restore')
                  ORDER BY rowid DESC LIMIT 1",
                libsql::params![page_id, version],
            )
            .await
            .ok()?;
        let row = rows.next().await.ok()??;
        Some((
            row.get::<String>(0).ok()?,
            row.get::<Option<String>>(1).unwrap_or(None),
        ))
    }

    /// Every actor recorded on `page_id`'s lifecycle rows, by version.
    pub async fn page_lifecycle_actors(
        &self,
        page_id: &str,
    ) -> Result<HashMap<i64, Option<String>>, WenlanError> {
        Ok(self
            .page_lifecycle_rows(page_id)
            .await?
            .into_iter()
            .map(|(version, _, _, actor)| (version, actor))
            .collect())
    }

    /// The AI tool that made `page_id`, as it named itself (`claude-code`),
    /// when its first version is an `agent_create` row that recorded one.
    /// Pages made before the name was kept answer `None`; no name is guessed.
    pub async fn page_birth_agent(&self, page_id: &str) -> Result<Option<String>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT actor FROM page_history
                  WHERE page_id = ?1 AND version = 1 AND edited_by = 'agent_create'
                  ORDER BY rowid ASC LIMIT 1",
                libsql::params![page_id],
            )
            .await
            .map_err(db_err("page birth agent"))?;
        let Some(row) = rows.next().await.map_err(db_err("page birth agent row"))? else {
            return Ok(None);
        };
        Ok(row
            .get::<Option<String>>(0)
            .unwrap_or(None)
            .map(|agent| agent.trim().to_string())
            .filter(|agent| !agent.is_empty()))
    }
}

#[cfg(test)]
#[path = "page_history_actor_test.rs"]
mod tests;
