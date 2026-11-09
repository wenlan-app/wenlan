// SPDX-License-Identifier: Apache-2.0
//! Where a topic's alias came from (docs/ux/spec.md, UX-GRAPH-9).
//!
//! `pages.aliases` is a plain list of names, so an alias added because a
//! mention merely embedded near a topic looked the same as one a person
//! declared. Every later mention of that name then resolved to the topic
//! before any similarity check ran, which is how `WENLAN_HOST` came to absorb
//! "wenlan-server".
//!
//! `entity_alias_sources` records, per topic and alias, a source:
//!
//! - [`source::AUTO`]: added by a vector or minhash merge, because the
//!   mention read or was spelled like the topic. Resolution never takes it as
//!   an exact match; the similarity check runs again on every mention. The
//!   mention's own type is kept, so lint can count auto aliases that point at
//!   a topic of a different type.
//! - [`source::DETACHED`]: a person took the alias off the topic
//!   ([`MemoryDB::detach_entity_alias`]). Neither a vector or minhash merge
//!   nor merging another topic into it puts that name back on that topic.
//!
//! An alias with no row is exact or of unknown origin: every alias written
//! before migration 161, the topic's own name, a person's declared alias and
//! a merged topic's names. Those keep resolving exactly as before.

use std::collections::HashSet;

use super::{commit_or_rollback, parse_pages_aliases, MemoryDB};
use crate::error::WenlanError;

/// Values of `entity_alias_sources.source`.
pub mod source {
    /// Added by a vector or minhash merge.
    pub const AUTO: &str = "auto";
    /// Taken off the topic by a person.
    pub const DETACHED: &str = "detached";
}

/// SQL, true when the alias `?{param}` on the entity `epm.entity_id` was
/// added by a vector or minhash merge. Used to keep auto aliases out of exact lookups.
pub(crate) fn auto_alias_sql(param: &str) -> String {
    format!(
        "EXISTS (SELECT 1 FROM entity_alias_sources eas
                  WHERE eas.entity_id = epm.entity_id AND eas.alias = {param}
                    AND eas.source = '{}')",
        source::AUTO
    )
}

/// A merge's names for the canonical, minus every name a person detached
/// from it: the loser's alias array (as JSON) and its lowercased name (empty
/// when detached). A merge never puts a detached name back.
pub(crate) fn without_detached(
    loser_aliases_json: Option<String>,
    loser_name_lower: String,
    detached: &HashSet<String>,
) -> (Option<String>, String) {
    if detached.is_empty() {
        return (loser_aliases_json, loser_name_lower);
    }
    let kept: Vec<String> = parse_pages_aliases(loser_aliases_json)
        .into_iter()
        .filter(|alias| !detached.contains(alias))
        .collect();
    let name = if detached.contains(&loser_name_lower) {
        String::new()
    } else {
        loser_name_lower
    };
    (
        Some(serde_json::to_string(&kept).unwrap_or_else(|_| "[]".to_string())),
        name,
    )
}

fn db_err(context: &str) -> impl Fn(libsql::Error) -> WenlanError + '_ {
    move |e| WenlanError::VectorDb(format!("{context}: {e}"))
}

impl MemoryDB {
    /// Migration 161: the table of alias sources. Existing aliases get no
    /// row, so they keep resolving exactly; nothing is dropped.
    pub(super) async fn migrate_161_entity_alias_sources(
        &self,
        prior_version: i64,
    ) -> Result<(), WenlanError> {
        self.backup_before_migration(161, prior_version).await?;
        let conn = self.conn.lock().await;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS entity_alias_sources (
                 entity_id TEXT NOT NULL,
                 alias TEXT NOT NULL,
                 source TEXT NOT NULL,
                 mention_type TEXT,
                 created_at INTEGER NOT NULL,
                 PRIMARY KEY (entity_id, alias)
             )",
            (),
        )
        .await
        .map_err(db_err("m161 table"))?;
        conn.execute("PRAGMA user_version = 161", ())
            .await
            .map_err(db_err("m161 bump"))?;
        log::info!("[migration] Migration 161 applied: where each topic alias came from");
        Ok(())
    }

    /// Marks `alias` on `entity_id` as added by a similarity merge, but only when
    /// the alias is now on the topic's page (another topic may own it) and
    /// carries no source yet. For callers already holding the connection.
    pub(super) async fn record_auto_alias_on(
        conn: &libsql::Connection,
        entity_id: &str,
        alias: &str,
        mention_type: &str,
    ) -> Result<(), WenlanError> {
        conn.execute(
            "INSERT INTO entity_alias_sources (entity_id, alias, source, mention_type, created_at)
             SELECT ?1, ?2, ?3, ?4, ?5
              WHERE EXISTS (
                  SELECT 1 FROM entity_page_map epm
                    JOIN pages p ON p.id = epm.page_id
                   WHERE epm.entity_id = ?1 AND p.kind = 'entity'
                     AND LOWER(p.title) <> ?2
                     AND EXISTS (SELECT 1 FROM json_each(p.aliases) WHERE value = ?2))
             ON CONFLICT (entity_id, alias) DO NOTHING",
            libsql::params![
                entity_id,
                alias,
                source::AUTO,
                mention_type,
                chrono::Utc::now().timestamp()
            ],
        )
        .await
        .map_err(db_err("record auto alias"))?;
        Ok(())
    }

    /// Adds `alias` to `entity_id` (first registration wins, as in
    /// [`Self::add_entity_alias`]) and records it as auto, in one transaction
    /// under one lock: the alias never lands without its source, which would
    /// leave it an exact match for good.
    pub(super) async fn add_auto_alias(
        &self,
        entity_id: &str,
        alias: &str,
        mention_type: &str,
    ) -> Result<(), WenlanError> {
        let conn = self.conn.lock().await;
        conn.execute("BEGIN", ())
            .await
            .map_err(db_err("auto alias begin"))?;
        let result: Result<(), WenlanError> = async {
            Self::add_alias_on(&conn, alias, entity_id).await?;
            Self::record_auto_alias_on(&conn, entity_id, alias, mention_type).await
        }
        .await;
        match result {
            Ok(()) => commit_or_rollback(&conn)
                .await
                .map(|_| ())
                .map_err(db_err("auto alias commit")),
            Err(e) => {
                let _ = conn.execute("ROLLBACK", ()).await;
                Err(e)
            }
        }
    }

    /// [`Self::add_auto_alias`] for the resolver, which keeps the topic it
    /// matched even when the alias could not be written: nothing was
    /// written, so the next mention is simply checked again.
    pub(super) async fn add_auto_alias_logged(
        &self,
        entity_id: &str,
        alias: &str,
        mention_type: &str,
    ) {
        if let Err(e) = self.add_auto_alias(entity_id, alias, mention_type).await {
            log::warn!("[resolve_or_create_entity] auto alias for {entity_id} not written: {e}");
        }
    }

    /// True when a person detached `alias` from `entity_id`, so a vector
    /// merge must not attach it again.
    pub(super) async fn alias_detached_on(
        conn: &libsql::Connection,
        entity_id: &str,
        alias: &str,
    ) -> Result<bool, WenlanError> {
        let mut rows = conn
            .query(
                "SELECT 1 FROM entity_alias_sources
                  WHERE entity_id = ?1 AND alias = ?2 AND source = ?3",
                libsql::params![entity_id, alias, source::DETACHED],
            )
            .await
            .map_err(db_err("alias detached"))?;
        Ok(rows
            .next()
            .await
            .map_err(db_err("alias detached row"))?
            .is_some())
    }

    /// Every alias a person detached from `entity_id`.
    pub(super) async fn detached_aliases_on(
        conn: &libsql::Connection,
        entity_id: &str,
    ) -> Result<HashSet<String>, WenlanError> {
        let mut rows = conn
            .query(
                "SELECT alias FROM entity_alias_sources WHERE entity_id = ?1 AND source = ?2",
                libsql::params![entity_id, source::DETACHED],
            )
            .await
            .map_err(db_err("detached aliases"))?;
        let mut detached = HashSet::new();
        while let Some(row) = rows.next().await.map_err(db_err("detached aliases row"))? {
            detached.insert(
                row.get::<String>(0)
                    .map_err(db_err("detached aliases alias"))?,
            );
        }
        Ok(detached)
    }

    /// [`Self::alias_detached_on`], taking the lock.
    pub(super) async fn alias_detached(
        &self,
        entity_id: &str,
        alias: &str,
    ) -> Result<bool, WenlanError> {
        let conn = self.conn.lock().await;
        Self::alias_detached_on(&conn, entity_id, alias).await
    }

    /// The recorded source of `alias` on `entity_id`: `Some("auto")`,
    /// `Some("detached")`, or `None` for an exact or older alias.
    pub async fn entity_alias_source(
        &self,
        entity_id: &str,
        alias: &str,
    ) -> Result<Option<String>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT source FROM entity_alias_sources WHERE entity_id = ?1 AND alias = ?2",
                libsql::params![entity_id, alias.trim().to_lowercase()],
            )
            .await
            .map_err(db_err("alias source"))?;
        Ok(rows
            .next()
            .await
            .map_err(db_err("alias source row"))?
            .and_then(|row| row.get::<String>(0).ok()))
    }

    /// Clears any recorded source of `alias` on `entity_id`: a person or a
    /// merge declared it, so it is exact from now on.
    pub(super) async fn clear_alias_source_on(
        conn: &libsql::Connection,
        entity_id: &str,
        alias: &str,
    ) -> Result<(), WenlanError> {
        conn.execute(
            "DELETE FROM entity_alias_sources WHERE entity_id = ?1 AND alias = ?2",
            libsql::params![entity_id, alias],
        )
        .await
        .map_err(db_err("clear alias source"))?;
        Ok(())
    }

    /// Merge bookkeeping: the loser's recorded sources move to the canonical
    /// topic, except for names the canonical already had before the merge
    /// (those stay as they were) and the loser's own name (the person merged
    /// it on purpose, so it is exact).
    pub(super) async fn move_alias_sources_on(
        conn: &libsql::Connection,
        loser_id: &str,
        canonical_id: &str,
        canonical_aliases_before: &HashSet<String>,
        loser_name_lower: &str,
    ) -> Result<(), WenlanError> {
        let mut rows = conn
            .query(
                "SELECT alias FROM entity_alias_sources WHERE entity_id = ?1",
                libsql::params![loser_id],
            )
            .await
            .map_err(db_err("move alias sources read"))?;
        let mut aliases = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(db_err("move alias sources row"))?
        {
            aliases.push(
                row.get::<String>(0)
                    .map_err(db_err("move alias sources alias"))?,
            );
        }
        drop(rows);
        for alias in aliases {
            if canonical_aliases_before.contains(&alias) {
                continue;
            }
            conn.execute(
                "INSERT INTO entity_alias_sources (entity_id, alias, source, mention_type, created_at)
                 SELECT ?1, alias, source, mention_type, created_at
                   FROM entity_alias_sources WHERE entity_id = ?2 AND alias = ?3
                 ON CONFLICT (entity_id, alias) DO NOTHING",
                libsql::params![canonical_id, loser_id, alias.as_str()],
            )
            .await
            .map_err(db_err("move alias sources write"))?;
        }
        Self::clear_alias_source_on(conn, canonical_id, loser_name_lower).await?;
        conn.execute(
            "DELETE FROM entity_alias_sources WHERE entity_id = ?1",
            libsql::params![loser_id],
        )
        .await
        .map_err(db_err("move alias sources cleanup"))?;
        Ok(())
    }

    /// Takes one alias off a topic and remembers that a person did, so a
    /// later similarity or topic merge never attaches the same name to the
    /// same topic again. Works on an alias of any source. The topic's own name cannot be
    /// detached; rename the topic instead. Returns `false` when the topic
    /// does not carry the alias. Memories already linked are not moved.
    pub async fn detach_entity_alias(
        &self,
        entity_id: &str,
        alias: &str,
    ) -> Result<bool, WenlanError> {
        let alias = alias.trim().to_lowercase();
        let conn = self.conn.lock().await;
        // Archived topics included: they still own their aliases.
        let mut rows = conn
            .query(
                "SELECT p.id, p.title, p.aliases FROM entity_page_map epm
                   JOIN pages p ON p.id = epm.page_id
                  WHERE epm.entity_id = ?1 AND p.kind = 'entity'",
                libsql::params![entity_id],
            )
            .await
            .map_err(db_err("detach alias read"))?;
        let Some(row) = rows.next().await.map_err(db_err("detach alias row"))? else {
            return Err(WenlanError::NotFound("entity not found".to_string()));
        };
        let page_id = row
            .get::<String>(0)
            .map_err(db_err("detach alias page id"))?;
        let title = row.get::<String>(1).unwrap_or_default();
        let aliases_json = row.get::<Option<String>>(2).unwrap_or(None);
        drop(rows);
        if title.to_lowercase() == alias {
            return Err(WenlanError::Validation(format!(
                "{alias:?} is the topic's own name; rename the topic instead"
            )));
        }
        if !parse_pages_aliases(aliases_json).contains(&alias) {
            return Ok(false);
        }
        conn.execute("BEGIN", ())
            .await
            .map_err(db_err("detach alias begin"))?;
        let result: Result<(), WenlanError> = async {
            conn.execute(
                "UPDATE pages SET
                     aliases = (SELECT json_group_array(value) FROM (
                         SELECT value FROM json_each(pages.aliases)
                          WHERE value <> ?1
                          ORDER BY value
                     )),
                     last_modified = ?2
                  WHERE kind = 'entity' AND id = ?3",
                libsql::params![
                    alias.as_str(),
                    chrono::Utc::now().to_rfc3339(),
                    page_id.as_str()
                ],
            )
            .await
            .map_err(db_err("detach alias page"))?;
            conn.execute(
                "INSERT INTO entity_alias_sources (entity_id, alias, source, mention_type, created_at)
                 VALUES (?1, ?2, ?3, NULL, ?4)
                 ON CONFLICT (entity_id, alias) DO UPDATE SET
                     source = excluded.source, created_at = excluded.created_at",
                libsql::params![
                    entity_id,
                    alias.as_str(),
                    source::DETACHED,
                    chrono::Utc::now().timestamp()
                ],
            )
            .await
            .map_err(db_err("detach alias source"))?;
            Ok(())
        }
        .await;
        match result {
            Ok(()) => {
                commit_or_rollback(&conn)
                    .await
                    .map_err(|e| WenlanError::VectorDb(format!("detach alias commit: {e}")))?;
                Ok(true)
            }
            Err(e) => {
                let _ = conn.execute("ROLLBACK", ()).await;
                Err(e)
            }
        }
    }
}
