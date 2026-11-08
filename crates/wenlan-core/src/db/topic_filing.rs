// SPDX-License-Identifier: Apache-2.0
//! Which topic a memory is filed under (docs/ux/spec.md, UX-GRAPH-8).
//!
//! A memory's main topic (`memories.entity_id`) is chosen from what it says,
//! and is never a topic the person archived or the project-wide hub topic
//! when anything more specific is linked. Every write to `memory_entities`
//! names its writer, and every time a memory's main topic moves off a topic
//! or is cleared, `memory_topic_changes` records the old and the new topic,
//! who moved it and why, so the change can be looked up and undone
//! ([`MemoryDB::undo_memory_topic_change`]).
//!
//! Migration 160 adds `memory_entities.writer` and `memory_entities.created_at`
//! (rows written before it read `legacy` and the migration's time) and the
//! change log. Links are never deleted here: moving a main topic only changes
//! which of the memory's links it points at.

use super::{commit_or_rollback, is_hub_entity, MemoryDB, HUB_MIN_LINKS};
use crate::error::WenlanError;

fn db_err(context: &str) -> impl Fn(libsql::Error) -> WenlanError + '_ {
    move |e| WenlanError::VectorDb(format!("{context}: {e}"))
}

/// Names the writers of `memory_entities` rows and topic changes use.
pub mod writer {
    /// Model extraction at ingest (`kg::entity_extraction::commit_kg`).
    pub const KG_EXTRACT: &str = "kg_extract";
    /// The ambient entity backfill and steep's entity extraction.
    pub const ENTITY_ENRICHMENT: &str = "entity_enrichment";
    /// The ambient lane's vector auto-link to an existing topic.
    pub const ENTITY_LINK: &str = "entity_link";
    /// Ingest's vector auto-link (`post_ingest::auto_link_entity`).
    pub const AUTO_LINK: &str = "auto_link";
    /// Steep's reweave of memories with no topic.
    pub const REWEAVE: &str = "reweave";
    /// A night's links (`db::night_links`).
    pub const NIGHT_LINKS: &str = "night_links";
    /// `POST /api/memory/link-entity`.
    pub const LINK_ENTITY_ROUTE: &str = "link_entity_route";
    /// A capture that named its topic.
    pub const CAPTURE: &str = "capture";
    /// Merging one topic into another.
    pub const MERGE: &str = "merge_entities";
    /// A lint repair that re-adds a memory's extracted topics.
    pub const LINT_REPAIR: &str = "lint_repair";
    /// Eval harness seeding.
    pub const EVAL: &str = "eval";
    /// Re-filing memories whose main topic is archived.
    pub const REFILE: &str = "refile_archived_primaries";
    /// Undoing a logged topic change.
    pub const UNDO: &str = "undo_topic_change";
    /// Restoring an archived topic, which undoes the re-filing done because
    /// it was archived.
    pub const RESTORE: &str = "restore_entity";
}

/// Reasons recorded in `memory_topic_changes.reason`.
pub mod reason {
    /// The memory's main topic was archived.
    pub const TOPIC_ARCHIVED: &str = "topic_archived";
    /// A repair found the main topic already archived.
    pub const ARCHIVED_PRIMARY: &str = "archived_primary";
    /// The topic came back, so a re-filing done because it was archived
    /// was undone.
    pub const TOPIC_RESTORED: &str = "topic_restored";
    /// The main topic moved off a hub topic onto a specific one.
    pub const HUB_PRIMARY: &str = "hub_primary";
    /// A new extraction chose a different main topic.
    pub const RE_EXTRACTED: &str = "re_extracted";
    /// A person or tool undid an earlier change.
    pub const UNDO: &str = "undo";
    /// A writer linked the memory to a shown topic and made it the main one.
    pub const LINKED: &str = "linked";
}

/// SQL that is true when the topic of link `o` is shown: its entity page is
/// active.
pub(super) const ACTIVE_LINK_SQL: &str = "EXISTS (SELECT 1 FROM entity_page_map epm_s
                                     JOIN pages p_s ON p_s.id = epm_s.page_id
                                    WHERE epm_s.entity_id = o.entity_id
                                      AND p_s.kind = 'entity' AND p_s.status = 'active')";

const REFILE_ONCE_MARKER: &str = "kg_refile_archived_primaries_v1";

/// One row of `memory_topic_changes`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryTopicChange {
    pub id: i64,
    pub memory_id: String,
    pub old_entity_id: Option<String>,
    pub new_entity_id: Option<String>,
    pub writer: String,
    pub reason: String,
    pub created_at: i64,
    /// The change that undid this one, when one did.
    pub undone_by: Option<i64>,
}

/// What a re-filing pass did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RefileReport {
    /// Main topic moved to another topic the memory is linked to.
    pub moved: u64,
    /// No shown topic left, so the main topic was cleared.
    pub unfiled: u64,
}

impl MemoryDB {
    /// Migration 160: who wrote each topic link and when, and the log of
    /// main-topic changes. Columns and table are added only when missing,
    /// so a crash before the version bump replays cleanly.
    pub(super) async fn migrate_160_topic_filing(
        &self,
        prior_version: i64,
    ) -> Result<(), WenlanError> {
        self.backup_before_migration(160, prior_version).await?;
        let conn = self.conn.lock().await;
        // SQLite cannot add a column whose default is not constant, so
        // `created_at` defaults to 0 and existing rows get this moment.
        for column in [
            "writer TEXT NOT NULL DEFAULT 'legacy'",
            "created_at INTEGER NOT NULL DEFAULT 0",
        ] {
            let name = column.split(' ').next().unwrap_or_default();
            let mut rows = conn
                .query(
                    &format!(
                        "SELECT 1 FROM pragma_table_info('memory_entities') WHERE name = '{name}'"
                    ),
                    (),
                )
                .await
                .map_err(db_err("m160 columns"))?;
            let has_column = rows
                .next()
                .await
                .map_err(db_err("m160 column row"))?
                .is_some();
            drop(rows);
            if !has_column {
                conn.execute(
                    &format!("ALTER TABLE memory_entities ADD COLUMN {column}"),
                    (),
                )
                .await
                .map_err(db_err("m160 add column"))?;
            }
        }
        conn.execute(
            "UPDATE memory_entities SET created_at = ?1 WHERE created_at = 0",
            libsql::params![chrono::Utc::now().timestamp()],
        )
        .await
        .map_err(db_err("m160 created_at"))?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS memory_topic_changes (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 memory_id TEXT NOT NULL,
                 old_entity_id TEXT,
                 new_entity_id TEXT,
                 writer TEXT NOT NULL,
                 reason TEXT NOT NULL,
                 created_at INTEGER NOT NULL,
                 undone_by INTEGER
             )",
            (),
        )
        .await
        .map_err(db_err("m160 change log"))?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_memory_topic_changes_memory
                 ON memory_topic_changes(memory_id, id)",
            (),
        )
        .await
        .map_err(db_err("m160 change log index"))?;
        conn.execute("PRAGMA user_version = 160", ())
            .await
            .map_err(db_err("m160 bump"))?;
        log::info!(
            "[migration] Migration 160 applied: who wrote each topic link, and a log of main-topic changes"
        );
        Ok(())
    }

    /// Every hub topic: linked to so many memories that a link says nothing
    /// about the memory ([`is_hub_entity`]). For callers that already hold
    /// the connection; [`Self::hub_entity_ids`] is the same rule.
    pub(super) async fn hub_ids_on(conn: &libsql::Connection) -> Result<Vec<String>, WenlanError> {
        let mut rows = conn
            .query("SELECT COUNT(DISTINCT memory_id) FROM memory_entities", ())
            .await
            .map_err(db_err("hub ids total"))?;
        let total = match rows.next().await.map_err(db_err("hub ids total row"))? {
            Some(row) => row.get::<i64>(0).unwrap_or(0),
            None => 0,
        };
        drop(rows);
        let mut rows = conn
            .query(
                "SELECT entity_id, COUNT(*) FROM memory_entities
                  GROUP BY entity_id HAVING COUNT(*) >= ?1 ORDER BY entity_id",
                libsql::params![HUB_MIN_LINKS],
            )
            .await
            .map_err(db_err("hub ids"))?;
        let mut hubs = Vec::new();
        while let Some(row) = rows.next().await.map_err(db_err("hub ids row"))? {
            if is_hub_entity(row.get::<i64>(1).unwrap_or(0), total) {
                hubs.push(row.get::<String>(0).map_err(db_err("hub id"))?);
            }
        }
        Ok(hubs)
    }

    /// The candidates that are hub topics, by the same rule as
    /// [`Self::hub_ids_on`], counting only the candidates' links.
    pub(super) async fn hub_subset_on(
        conn: &libsql::Connection,
        candidates: &[&str],
    ) -> Result<std::collections::HashSet<String>, WenlanError> {
        let mut hubs = std::collections::HashSet::new();
        if candidates.is_empty() {
            return Ok(hubs);
        }
        let count = |sql: &'static str, params: Vec<libsql::Value>| async move {
            let mut rows = conn
                .query(sql, params)
                .await
                .map_err(db_err("hub_entity_ids"))?;
            let n = match rows.next().await.map_err(db_err("hub_entity_ids"))? {
                Some(row) => row.get::<i64>(0).unwrap_or(0),
                None => 0,
            };
            Ok::<i64, WenlanError>(n)
        };
        let total = count(
            "SELECT COUNT(DISTINCT memory_id) FROM memory_entities",
            Vec::new(),
        )
        .await?;
        for id in candidates {
            let links = count(
                "SELECT COUNT(*) FROM memory_entities WHERE entity_id = ?1",
                vec![libsql::Value::Text((*id).to_string())],
            )
            .await?;
            if is_hub_entity(links, total) {
                hubs.insert((*id).to_string());
            }
        }
        Ok(hubs)
    }

    /// True when `entity_id` has an active entity page.
    pub(super) async fn entity_is_active_on(
        conn: &libsql::Connection,
        entity_id: &str,
    ) -> Result<bool, WenlanError> {
        let mut rows = conn
            .query(
                "SELECT 1 FROM entity_page_map epm
                   JOIN pages p ON p.id = epm.page_id
                  WHERE epm.entity_id = ?1 AND p.kind = 'entity' AND p.status = 'active'
                  LIMIT 1",
                libsql::params![entity_id],
            )
            .await
            .map_err(db_err("entity active"))?;
        Ok(rows
            .next()
            .await
            .map_err(db_err("entity active row"))?
            .is_some())
    }

    /// The topic a memory should be filed under from the links it already
    /// has: its first shown link that is not a hub, else none. A hub is never
    /// a main topic, even when it is the only shown link (same rule as
    /// `commit_kg`). `excluding` is left out (the topic being archived).
    pub(super) async fn first_active_topic_on(
        conn: &libsql::Connection,
        memory_id: &str,
        excluding: Option<&str>,
        hubs: &[String],
    ) -> Result<Option<String>, WenlanError> {
        let marks = (3..hubs.len() + 3)
            .map(|i| format!("?{i}"))
            .collect::<Vec<_>>()
            .join(",");
        let not_hub = if hubs.is_empty() {
            String::new()
        } else {
            format!("AND o.entity_id NOT IN ({marks})")
        };
        let mut params = vec![
            libsql::Value::Text(memory_id.to_string()),
            excluding.map_or(libsql::Value::Null, |id| {
                libsql::Value::Text(id.to_string())
            }),
        ];
        params.extend(hubs.iter().cloned().map(libsql::Value::Text));
        let mut rows = conn
            .query(
                &format!(
                    "SELECT o.entity_id FROM memory_entities o
                      WHERE o.memory_id = ?1
                        AND (?2 IS NULL OR o.entity_id <> ?2)
                        AND {ACTIVE_LINK_SQL}
                        {not_hub}
                      ORDER BY o.rowid LIMIT 1"
                ),
                params,
            )
            .await
            .map_err(db_err("first active topic"))?;
        match rows
            .next()
            .await
            .map_err(db_err("first active topic row"))?
        {
            Some(row) => Ok(Some(
                row.get::<String>(0)
                    .map_err(db_err("first active topic id"))?,
            )),
            None => Ok(None),
        }
    }

    /// [`Self::first_active_topic_on`] for a caller without the connection.
    pub async fn first_active_topic(&self, memory_id: &str) -> Result<Option<String>, WenlanError> {
        let conn = self.conn.lock().await;
        let hubs = Self::hub_ids_on(&conn).await?;
        Self::first_active_topic_on(&conn, memory_id, None, &hubs).await
    }

    /// The candidates that are shown (active) topics, in their order.
    pub async fn active_entity_ids(&self, candidates: &[&str]) -> Result<Vec<String>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut active = Vec::new();
        for id in candidates {
            if Self::entity_is_active_on(&conn, id).await? {
                active.push((*id).to_string());
            }
        }
        Ok(active)
    }

    /// A memory's main topic as stored on its first chunk row.
    pub(super) async fn primary_topic_on(
        conn: &libsql::Connection,
        memory_id: &str,
    ) -> Result<Option<Option<String>>, WenlanError> {
        let mut rows = conn
            .query(
                "SELECT entity_id FROM memories WHERE source_id = ?1
                  ORDER BY chunk_index LIMIT 1",
                libsql::params![memory_id],
            )
            .await
            .map_err(db_err("primary topic"))?;
        match rows.next().await.map_err(db_err("primary topic row"))? {
            Some(row) => Ok(Some(
                row.get::<Option<String>>(0)
                    .map_err(db_err("primary topic id"))?,
            )),
            None => Ok(None),
        }
    }

    /// Append one row to `memory_topic_changes`; returns its id.
    pub(super) async fn log_topic_change_on(
        conn: &libsql::Connection,
        memory_id: &str,
        old_entity_id: Option<&str>,
        new_entity_id: Option<&str>,
        writer: &str,
        reason: &str,
    ) -> Result<i64, WenlanError> {
        conn.execute(
            "INSERT INTO memory_topic_changes
                 (memory_id, old_entity_id, new_entity_id, writer, reason, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            libsql::params![
                memory_id,
                old_entity_id,
                new_entity_id,
                writer,
                reason,
                chrono::Utc::now().timestamp()
            ],
        )
        .await
        .map_err(db_err("log topic change"))?;
        Ok(conn.last_insert_rowid())
    }

    /// Set a memory's main topic on every chunk row. A move off a topic (the
    /// old one was set and differs) is logged; a first filing is not.
    /// Returns the logged change's id.
    pub(super) async fn set_primary_topic_on(
        conn: &libsql::Connection,
        memory_id: &str,
        new_entity_id: Option<&str>,
        writer: &str,
        reason: &str,
    ) -> Result<Option<i64>, WenlanError> {
        let Some(old) = Self::primary_topic_on(conn, memory_id).await? else {
            return Ok(None);
        };
        if old.as_deref() == new_entity_id {
            return Ok(None);
        }
        conn.execute(
            "UPDATE memories SET entity_id = ?1 WHERE source_id = ?2",
            libsql::params![new_entity_id, memory_id],
        )
        .await
        .map_err(db_err("set primary topic"))?;
        match old {
            Some(old) => Ok(Some(
                Self::log_topic_change_on(
                    conn,
                    memory_id,
                    Some(&old),
                    new_entity_id,
                    writer,
                    reason,
                )
                .await?,
            )),
            None => Ok(None),
        }
    }

    /// Re-file every memory whose main topic is `entity_id` (being archived)
    /// onto its first other shown topic, or clear it. Each move is logged so
    /// restoring the topic can put it back. Returns how many moved.
    pub(super) async fn refile_memories_off_entity_on(
        conn: &libsql::Connection,
        entity_id: &str,
        writer: &str,
    ) -> Result<u64, WenlanError> {
        let mut rows = conn
            .query(
                "SELECT DISTINCT source_id FROM memories WHERE entity_id = ?1 ORDER BY source_id",
                libsql::params![entity_id],
            )
            .await
            .map_err(db_err("refile off entity"))?;
        let mut memories = Vec::new();
        while let Some(row) = rows.next().await.map_err(db_err("refile off entity row"))? {
            memories.push(
                row.get::<String>(0)
                    .map_err(db_err("refile off entity id"))?,
            );
        }
        drop(rows);
        if memories.is_empty() {
            return Ok(0);
        }
        let hubs = Self::hub_ids_on(conn).await?;
        let mut moved = 0;
        for memory in &memories {
            let next = Self::first_active_topic_on(conn, memory, Some(entity_id), &hubs).await?;
            if Self::set_primary_topic_on(
                conn,
                memory,
                next.as_deref(),
                writer,
                reason::TOPIC_ARCHIVED,
            )
            .await?
            .is_some()
            {
                moved += 1;
            }
        }
        Ok(moved)
    }

    /// Undo, for a topic that came back, every re-filing done because it was
    /// archived: its own archive's, and the repair passes' that found it
    /// archived. A memory goes back to the topic when it is still where the
    /// re-filing left it, or when it now has no main topic or an archived
    /// one (a person can set neither, so nothing is overridden -- this is
    /// how a chain of archives, Alpha then the topic Alpha's memories moved
    /// to, comes undone). A memory a person or writer filed under another
    /// shown topic since keeps where it is. Each move back is logged.
    /// Returns how many went back.
    pub(super) async fn undo_archive_refiles_on(
        conn: &libsql::Connection,
        entity_id: &str,
    ) -> Result<u64, WenlanError> {
        let mut rows = conn
            .query(
                "SELECT id, memory_id, new_entity_id FROM memory_topic_changes
                  WHERE old_entity_id = ?1 AND reason IN (?2, ?3) AND undone_by IS NULL
                  ORDER BY id DESC",
                libsql::params![entity_id, reason::TOPIC_ARCHIVED, reason::ARCHIVED_PRIMARY],
            )
            .await
            .map_err(db_err("undo archive refiles"))?;
        let mut changes = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(db_err("undo archive refile row"))?
        {
            changes.push((
                row.get::<i64>(0)
                    .map_err(db_err("undo archive refile id"))?,
                row.get::<String>(1)
                    .map_err(db_err("undo archive refile memory"))?,
                row.get::<Option<String>>(2)
                    .map_err(db_err("undo archive refile new"))?,
            ));
        }
        drop(rows);
        let mut restored = 0;
        for (change, memory, filed_to) in changes {
            let Some(current) = Self::primary_topic_on(conn, &memory).await? else {
                continue;
            };
            if current.as_deref() == Some(entity_id) {
                continue;
            }
            let unshown = match current.as_deref() {
                None => true,
                Some(now) => !Self::entity_is_active_on(conn, now).await?,
            };
            if current != filed_to && !unshown {
                continue;
            }
            conn.execute(
                "UPDATE memories SET entity_id = ?1 WHERE source_id = ?2",
                libsql::params![entity_id, memory.as_str()],
            )
            .await
            .map_err(db_err("undo archive refile"))?;
            let undo = Self::log_topic_change_on(
                conn,
                &memory,
                current.as_deref(),
                Some(entity_id),
                writer::RESTORE,
                reason::TOPIC_RESTORED,
            )
            .await?;
            conn.execute(
                "UPDATE memory_topic_changes SET undone_by = ?1 WHERE id = ?2",
                libsql::params![undo, change],
            )
            .await
            .map_err(db_err("undo archive refile mark"))?;
            restored += 1;
        }
        Ok(restored)
    }

    /// Move every memory whose main topic is archived onto its first shown
    /// topic that is not a hub, or clear it when it has none, so
    /// it is eligible to be linked again. Every move is logged under
    /// `writer`. Links are left as they are.
    pub async fn refile_archived_primaries(
        &self,
        writer: &str,
    ) -> Result<RefileReport, WenlanError> {
        let conn = self.conn.lock().await;
        conn.execute("BEGIN IMMEDIATE", ())
            .await
            .map_err(db_err("refile archived primaries begin"))?;
        let result = Self::refile_archived_primaries_on(&conn, writer).await;
        match result {
            Ok(report) => {
                commit_or_rollback(&conn)
                    .await
                    .map_err(db_err("refile archived primaries commit"))?;
                Ok(report)
            }
            Err(e) => {
                let _ = conn.execute("ROLLBACK", ()).await;
                Err(e)
            }
        }
    }

    async fn refile_archived_primaries_on(
        conn: &libsql::Connection,
        writer: &str,
    ) -> Result<RefileReport, WenlanError> {
        let mut rows = conn
            .query(
                "SELECT DISTINCT m.source_id FROM memories m
                   JOIN entity_page_map epm ON epm.entity_id = m.entity_id
                   JOIN pages p ON p.id = epm.page_id
                  WHERE p.kind = 'entity' AND p.status = 'archived'
                  ORDER BY m.source_id",
                (),
            )
            .await
            .map_err(db_err("archived primaries"))?;
        let mut memories = Vec::new();
        while let Some(row) = rows.next().await.map_err(db_err("archived primary row"))? {
            memories.push(
                row.get::<String>(0)
                    .map_err(db_err("archived primary id"))?,
            );
        }
        drop(rows);
        let mut report = RefileReport::default();
        if memories.is_empty() {
            return Ok(report);
        }
        let hubs = Self::hub_ids_on(conn).await?;
        for memory in &memories {
            let next = Self::first_active_topic_on(conn, memory, None, &hubs).await?;
            if Self::set_primary_topic_on(
                conn,
                memory,
                next.as_deref(),
                writer,
                reason::ARCHIVED_PRIMARY,
            )
            .await?
            .is_some()
            {
                if next.is_some() {
                    report.moved += 1;
                } else {
                    report.unfiled += 1;
                }
            }
        }
        Ok(report)
    }

    /// [`Self::refile_archived_primaries`] once per database: the repair of
    /// main topics written before the rule. Records what it did under an
    /// `app_metadata` marker and returns zeros afterwards; every move is in
    /// `memory_topic_changes`.
    pub async fn refile_archived_primaries_once(&self) -> Result<RefileReport, WenlanError> {
        if self.get_app_metadata(REFILE_ONCE_MARKER).await?.is_some() {
            return Ok(RefileReport::default());
        }
        let conn = self.conn.lock().await;
        conn.execute("BEGIN IMMEDIATE", ())
            .await
            .map_err(db_err("refile once begin"))?;
        let result: Result<RefileReport, WenlanError> = async {
            let mut rows = conn
                .query(
                    "SELECT 1 FROM app_metadata WHERE key = ?1",
                    libsql::params![REFILE_ONCE_MARKER],
                )
                .await
                .map_err(db_err("refile once marker"))?;
            if rows
                .next()
                .await
                .map_err(db_err("refile once marker row"))?
                .is_some()
            {
                return Ok(RefileReport::default());
            }
            drop(rows);
            let report = Self::refile_archived_primaries_on(&conn, writer::REFILE).await?;
            conn.execute(
                "INSERT INTO app_metadata (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                libsql::params![
                    REFILE_ONCE_MARKER,
                    serde_json::json!({"moved": report.moved, "unfiled": report.unfiled})
                        .to_string()
                ],
            )
            .await
            .map_err(db_err("refile once record"))?;
            Ok(report)
        }
        .await;
        match result {
            Ok(report) => {
                commit_or_rollback(&conn)
                    .await
                    .map_err(db_err("refile once commit"))?;
                Ok(report)
            }
            Err(e) => {
                let _ = conn.execute("ROLLBACK", ()).await;
                Err(e)
            }
        }
    }

    /// The logged main-topic changes of one memory, oldest first.
    pub async fn memory_topic_changes(
        &self,
        memory_id: &str,
    ) -> Result<Vec<MemoryTopicChange>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT id, memory_id, old_entity_id, new_entity_id, writer, reason,
                        created_at, undone_by
                   FROM memory_topic_changes WHERE memory_id = ?1 ORDER BY id",
                libsql::params![memory_id],
            )
            .await
            .map_err(db_err("memory topic changes"))?;
        let mut changes = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(db_err("memory topic change row"))?
        {
            changes.push(topic_change_from_row(&row)?);
        }
        Ok(changes)
    }

    /// Put a logged main-topic change back: the memory returns to the topic
    /// it had before, and the undo is itself logged (its id is returned).
    /// Refused when the change was already undone, when the memory has been
    /// filed elsewhere since, or when the old topic is archived now (restore
    /// it instead; that brings its memories back).
    pub async fn undo_memory_topic_change(&self, change_id: i64) -> Result<i64, WenlanError> {
        let conn = self.conn.lock().await;
        conn.execute("BEGIN IMMEDIATE", ())
            .await
            .map_err(db_err("undo topic change begin"))?;
        let result: Result<i64, WenlanError> = async {
            let mut rows = conn
                .query(
                    "SELECT id, memory_id, old_entity_id, new_entity_id, writer, reason,
                            created_at, undone_by
                       FROM memory_topic_changes WHERE id = ?1",
                    libsql::params![change_id],
                )
                .await
                .map_err(db_err("undo topic change"))?;
            let change = match rows.next().await.map_err(db_err("undo topic change row"))? {
                Some(row) => topic_change_from_row(&row)?,
                None => {
                    return Err(WenlanError::NotFound(format!(
                        "topic change {change_id} does not exist"
                    )))
                }
            };
            drop(rows);
            if change.undone_by.is_some() {
                return Err(WenlanError::Conflict(format!(
                    "topic change {change_id} was already undone"
                )));
            }
            let Some(current) = Self::primary_topic_on(&conn, &change.memory_id).await? else {
                return Err(WenlanError::NotFound(format!(
                    "memory '{}' does not exist",
                    change.memory_id
                )));
            };
            if current != change.new_entity_id {
                return Err(WenlanError::Conflict(format!(
                    "memory '{}' was filed elsewhere after topic change {change_id}",
                    change.memory_id
                )));
            }
            if let Some(old) = change.old_entity_id.as_deref() {
                if !Self::entity_is_active_on(&conn, old).await? {
                    return Err(WenlanError::Conflict(format!(
                        "topic {old} is archived; restore it to put its memories back"
                    )));
                }
                conn.execute(
                    "INSERT INTO memory_entities (memory_id, entity_id, writer, created_at)
                     VALUES (?1, ?2, ?3, ?4) ON CONFLICT DO NOTHING",
                    libsql::params![
                        change.memory_id.as_str(),
                        old,
                        writer::UNDO,
                        chrono::Utc::now().timestamp()
                    ],
                )
                .await
                .map_err(db_err("undo topic change link"))?;
            }
            conn.execute(
                "UPDATE memories SET entity_id = ?1 WHERE source_id = ?2",
                libsql::params![change.old_entity_id.as_deref(), change.memory_id.as_str()],
            )
            .await
            .map_err(db_err("undo topic change primary"))?;
            let undo = Self::log_topic_change_on(
                &conn,
                &change.memory_id,
                current.as_deref(),
                change.old_entity_id.as_deref(),
                writer::UNDO,
                reason::UNDO,
            )
            .await?;
            conn.execute(
                "UPDATE memory_topic_changes SET undone_by = ?1 WHERE id = ?2",
                libsql::params![undo, change_id],
            )
            .await
            .map_err(db_err("undo topic change mark"))?;
            Ok(undo)
        }
        .await;
        match result {
            Ok(undo) => {
                commit_or_rollback(&conn)
                    .await
                    .map_err(db_err("undo topic change commit"))?;
                Ok(undo)
            }
            Err(e) => {
                let _ = conn.execute("ROLLBACK", ()).await;
                Err(e)
            }
        }
    }
}

fn topic_change_from_row(row: &libsql::Row) -> Result<MemoryTopicChange, WenlanError> {
    let err = db_err("topic change column");
    Ok(MemoryTopicChange {
        id: row.get::<i64>(0).map_err(&err)?,
        memory_id: row.get::<String>(1).map_err(&err)?,
        old_entity_id: row.get::<Option<String>>(2).map_err(&err)?,
        new_entity_id: row.get::<Option<String>>(3).map_err(&err)?,
        writer: row.get::<String>(4).map_err(&err)?,
        reason: row.get::<String>(5).map_err(&err)?,
        created_at: row.get::<i64>(6).map_err(&err)?,
        undone_by: row.get::<Option<i64>>(7).map_err(&err)?,
    })
}
