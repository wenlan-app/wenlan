// SPDX-License-Identifier: Apache-2.0
//! Sentence decisions, stored as history entries (migration 145).
//!
//! `sentence_history` holds one row per `sentence.propose`, `sentence.accept`,
//! `sentence.reject` or `sentence.flag` entry, column for column in the shape
//! of the future `history/*.jsonl` line (storage migration spec 2026-10-31,
//! section 4): `{seq, ts, tx, actor, op, target, version, why, text,
//! digest_before, digest_after, data}`. Rows are append-only; a proposal's
//! state is read from the entries that follow it, never updated in place.

use super::MemoryDB;
use crate::error::WenlanError;
use wenlan_types::authorship::{HistoryActor, HistoryEntry, HistoryTarget};

pub(super) const SENTENCE_HISTORY_DDL: &str = "
    CREATE TABLE IF NOT EXISTS sentence_history (
        seq INTEGER PRIMARY KEY AUTOINCREMENT,
        ts INTEGER NOT NULL,
        tx TEXT NOT NULL,
        actor TEXT NOT NULL,
        op TEXT NOT NULL CHECK(op IN (
            'sentence.propose','sentence.accept','sentence.reject','sentence.flag')),
        target_type TEXT NOT NULL,
        target_id TEXT NOT NULL,
        version INTEGER,
        why TEXT,
        text TEXT,
        digest_before TEXT,
        digest_after TEXT,
        data TEXT NOT NULL DEFAULT '{}'
    );
    CREATE INDEX IF NOT EXISTS sentence_history_target
        ON sentence_history(target_type, target_id, seq);";

/// A sentence history entry to append; `seq` is assigned on insert.
#[derive(Debug, Clone)]
pub struct NewHistoryEntry<'a> {
    pub ts: i64,
    pub tx: &'a str,
    pub actor: &'a HistoryActor,
    pub op: &'a str,
    pub target: &'a HistoryTarget,
    pub version: Option<i64>,
    pub why: Option<&'a str>,
    pub text: Option<&'a str>,
    pub digest_before: Option<&'a str>,
    pub digest_after: Option<&'a str>,
    pub data: &'a serde_json::Value,
}

fn db_error(context: &str) -> impl Fn(libsql::Error) -> WenlanError + '_ {
    move |e| WenlanError::VectorDb(format!("{context}: {e}"))
}

impl MemoryDB {
    /// Migration 145: the sentence history table. `CREATE TABLE IF NOT
    /// EXISTS`, so a replay after a crash before the version bump is a no-op.
    pub(super) async fn migrate_145_sentence_history(
        &self,
        prior_version: i64,
    ) -> Result<(), WenlanError> {
        self.backup_before_migration(145, prior_version).await?;
        let conn = self.conn.lock().await;
        conn.execute_batch(SENTENCE_HISTORY_DDL)
            .await
            .map_err(db_error("m145 sentence_history"))?;
        conn.execute("PRAGMA user_version = 145", ())
            .await
            .map_err(db_error("m145 bump"))?;
        log::info!("[migration] Migration 145 applied: sentence_history");
        Ok(())
    }

    /// Append one sentence history entry and return its `seq`.
    /// Migration 148: `sentence_history.op` also admits `sentence.revert`
    /// (the person's Undo of one AI write). SQLite cannot widen a CHECK in
    /// place, so the table is rebuilt with every row and its index, in one
    /// transaction.
    pub(super) async fn migrate_148_sentence_revert(
        &self,
        prior_version: i64,
    ) -> Result<(), WenlanError> {
        self.backup_before_migration(148, prior_version).await?;
        let conn = self.conn.lock().await;
        let rebuild = "
            BEGIN;
            CREATE TABLE sentence_history_m148 (
                seq INTEGER PRIMARY KEY AUTOINCREMENT,
                ts INTEGER NOT NULL,
                tx TEXT NOT NULL,
                actor TEXT NOT NULL,
                op TEXT NOT NULL CHECK(op IN (
                    'sentence.propose','sentence.accept','sentence.reject','sentence.flag',
                    'sentence.revert')),
                target_type TEXT NOT NULL,
                target_id TEXT NOT NULL,
                version INTEGER,
                why TEXT,
                text TEXT,
                digest_before TEXT,
                digest_after TEXT,
                data TEXT NOT NULL DEFAULT '{}'
            );
            INSERT INTO sentence_history_m148 (
                seq, ts, tx, actor, op, target_type, target_id, version, why, text,
                digest_before, digest_after, data)
            SELECT seq, ts, tx, actor, op, target_type, target_id, version, why, text,
                   digest_before, digest_after, data
              FROM sentence_history;
            DROP TABLE sentence_history;
            ALTER TABLE sentence_history_m148 RENAME TO sentence_history;
            CREATE INDEX IF NOT EXISTS sentence_history_target
                ON sentence_history(target_type, target_id, seq);
            COMMIT;";
        if let Err(error) = conn.execute_batch(rebuild).await {
            let _ = conn.execute("ROLLBACK", ()).await;
            return Err(WenlanError::VectorDb(format!(
                "m148 sentence_history rebuild: {error}"
            )));
        }
        conn.execute("PRAGMA user_version = 148", ())
            .await
            .map_err(db_error("m148 bump"))?;
        log::info!("[migration] Migration 148 applied: sentence_history admits sentence.revert");
        Ok(())
    }

    pub async fn append_sentence_history(
        &self,
        entry: NewHistoryEntry<'_>,
    ) -> Result<i64, WenlanError> {
        let actor = serde_json::to_string(entry.actor)?;
        let data = serde_json::to_string(entry.data)?;
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "INSERT INTO sentence_history
                     (ts, tx, actor, op, target_type, target_id, version, why, text,
                      digest_before, digest_after, data)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                 RETURNING seq",
                libsql::params![
                    entry.ts,
                    entry.tx,
                    actor,
                    entry.op,
                    entry.target.kind.as_str(),
                    entry.target.id.as_str(),
                    entry.version,
                    entry.why,
                    entry.text,
                    entry.digest_before,
                    entry.digest_after,
                    data
                ],
            )
            .await
            .map_err(db_error("append_sentence_history"))?;
        let row = rows
            .next()
            .await
            .map_err(db_error("append_sentence_history row"))?
            .ok_or_else(|| WenlanError::VectorDb("append_sentence_history: no seq".into()))?;
        row.get::<i64>(0)
            .map_err(db_error("append_sentence_history seq"))
    }

    /// Every sentence history entry about one target, oldest first.
    pub async fn list_sentence_history(
        &self,
        target_type: &str,
        target_id: &str,
    ) -> Result<Vec<HistoryEntry>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT seq, ts, tx, actor, op, target_type, target_id, version, why, text,
                        digest_before, digest_after, data
                 FROM sentence_history
                 WHERE target_type = ?1 AND target_id = ?2
                 ORDER BY seq",
                libsql::params![target_type, target_id],
            )
            .await
            .map_err(db_error("list_sentence_history"))?;
        let mut out = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(db_error("list_sentence_history row"))?
        {
            let actor: String = row.get(3).map_err(db_error("sentence_history actor"))?;
            let data: String = row.get(12).unwrap_or_else(|_| "{}".to_string());
            out.push(HistoryEntry {
                seq: row.get(0).map_err(db_error("sentence_history seq"))?,
                ts: row.get(1).unwrap_or(0),
                tx: row.get(2).unwrap_or_default(),
                actor: serde_json::from_str(&actor)?,
                op: row.get(4).unwrap_or_default(),
                target: HistoryTarget {
                    kind: row.get(5).unwrap_or_default(),
                    id: row.get(6).unwrap_or_default(),
                },
                version: row.get(7).unwrap_or(None),
                why: row.get(8).unwrap_or(None),
                text: row.get(9).unwrap_or(None),
                digest_before: row.get(10).unwrap_or(None),
                digest_after: row.get(11).unwrap_or(None),
                data: serde_json::from_str(&data).unwrap_or(serde_json::Value::Null),
            });
        }
        Ok(out)
    }

    /// A forced rebuild releases `user_edited` with the body it lands. When
    /// the sentence gate kept the person's sentences in that body, the page is
    /// still the person's: set the flag back, only on the version the rebuild
    /// wrote, so a later write is never touched.
    pub(crate) async fn keep_user_edited_at_version(
        &self,
        page_id: &str,
        version: i64,
    ) -> Result<bool, WenlanError> {
        let conn = self.conn.lock().await;
        let changed = conn
            .execute(
                "UPDATE pages SET user_edited = 1 WHERE id = ?1 AND version = ?2",
                libsql::params![page_id, version],
            )
            .await
            .map_err(db_error("keep_user_edited_at_version"))?;
        Ok(changed > 0)
    }

    /// Store a page's citation records only while the page is still at
    /// `expected_version`, so a recheck computed against one body never lands
    /// on another. Metadata only: no version bump. Returns `false` when the
    /// page moved on or is not active.
    pub(crate) async fn set_page_citations_at_page_version(
        &self,
        page_id: &str,
        citations_json: &str,
        expected_version: i64,
    ) -> Result<bool, WenlanError> {
        let conn = self.conn.lock().await;
        conn.execute("BEGIN", ())
            .await
            .map_err(db_error("set_page_citations_at_page_version begin"))?;
        let exec = async {
            let current = {
                let mut rows = conn
                    .query(
                        "SELECT 1 FROM pages WHERE id = ?1 AND version = ?2 AND status = 'active'",
                        libsql::params![page_id, expected_version],
                    )
                    .await?;
                rows.next().await?.is_some()
            };
            if !current {
                return Ok::<bool, libsql::Error>(false);
            }
            let stamped = stamp_cited_versions(&conn, Some(citations_json)).await?;
            Self::dual_write_page_citations(&conn, page_id, stamped.as_deref()).await?;
            conn.execute(
                "UPDATE pages SET citations = ?1 WHERE id = ?2 AND version = ?3",
                libsql::params![stamped, page_id, expected_version],
            )
            .await?;
            Ok(true)
        }
        .await;
        match exec {
            Ok(true) => {
                super::commit_or_rollback(&conn)
                    .await
                    .map_err(db_error("set_page_citations_at_page_version commit"))?;
                Ok(true)
            }
            Ok(false) => {
                let _ = conn.execute("ROLLBACK", ()).await;
                Ok(false)
            }
            Err(e) => {
                let _ = conn.execute("ROLLBACK", ()).await;
                Err(WenlanError::VectorDb(format!(
                    "set_page_citations_at_page_version: {e}"
                )))
            }
        }
    }

    /// Test-only: run raw SQL against the database (fixture states the public
    /// API cannot reach, such as a `migration_87` history seed).
    #[cfg(test)]
    pub(crate) async fn execute_batch_for_test(&self, sql: &str) {
        let conn = self.conn.lock().await;
        conn.execute_batch(sql).await.expect("test SQL");
    }

    /// Test-only: one integer from a raw query.
    #[cfg(test)]
    pub(crate) async fn scalar_for_test(&self, sql: &str) -> i64 {
        let conn = self.conn.lock().await;
        let mut rows = conn.query(sql, ()).await.expect("test query");
        rows.next()
            .await
            .expect("test row")
            .expect("one row")
            .get::<i64>(0)
            .expect("an integer")
    }

    /// The current version and full text of a cited source, for stamping a
    /// citation `locator@version` and checking it later. A memory locator is
    /// a memory `source_id` (all its chunks, in order); a page locator is a
    /// page id. `None` when neither exists.
    pub async fn cited_source_state(
        &self,
        locator: &str,
    ) -> Result<Option<(i64, String)>, WenlanError> {
        let conn = self.conn.lock().await;
        cited_source_state_on(&conn, locator)
            .await
            .map_err(db_error("cited_source_state"))
    }
}

pub(super) async fn cited_source_state_on(
    conn: &libsql::Connection,
    locator: &str,
) -> Result<Option<(i64, String)>, libsql::Error> {
    let mut rows = conn
        .query(
            "SELECT MAX(COALESCE(version, 1)), GROUP_CONCAT(content, char(10)), COUNT(*)
             FROM (SELECT version, content FROM memories
                   WHERE source_id = ?1 AND COALESCE(pending_revision, 0) = 0
                   ORDER BY COALESCE(chunk_index, 0))",
            libsql::params![locator],
        )
        .await?;
    if let Some(row) = rows.next().await? {
        if row.get::<i64>(2).unwrap_or(0) > 0 {
            return Ok(Some((
                row.get::<i64>(0).unwrap_or(1),
                row.get::<String>(1).unwrap_or_default(),
            )));
        }
    }
    drop(rows);
    let mut rows = conn
        .query(
            "SELECT version, content FROM pages WHERE id = ?1",
            libsql::params![locator],
        )
        .await?;
    Ok(rows.next().await?.map(|row| {
        (
            row.get::<i64>(0).unwrap_or(1),
            row.get::<String>(1).unwrap_or_default(),
        )
    }))
}

/// Digest of a cited source's text, for telling "the source moved on but
/// still says the same" from "the source's text changed".
pub fn cited_text_digest(text: &str) -> String {
    use sha2::{Digest, Sha256};
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let digest = Sha256::digest(normalized.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Stamp every citation in `citations_json` that has no `cited_version` with
/// its source's current version and text digest. Runs inside the transaction
/// that stores the citations, which is the transaction of the body they were
/// just computed for, so the stamp is the version the check read. Citations
/// that already carry a version keep it. Unparseable JSON is passed through
/// unchanged: the stamp is an addition, never a reason to refuse a write.
pub(super) async fn stamp_cited_versions(
    conn: &libsql::Connection,
    citations_json: Option<&str>,
) -> Result<Option<String>, libsql::Error> {
    let Some(json) = citations_json else {
        return Ok(None);
    };
    let Ok(mut citations) = serde_json::from_str::<Vec<wenlan_types::pages::PageCitation>>(json)
    else {
        return Ok(Some(json.to_string()));
    };
    if citations.iter().all(|c| c.cited_version.is_some()) {
        return Ok(Some(json.to_string()));
    }
    let mut states: std::collections::HashMap<String, Option<(i64, String)>> =
        std::collections::HashMap::new();
    for citation in &mut citations {
        if citation.cited_version.is_some() {
            continue;
        }
        if !states.contains_key(&citation.locator) {
            let state = cited_source_state_on(conn, &citation.locator)
                .await?
                .map(|(version, text)| (version, cited_text_digest(&text)));
            states.insert(citation.locator.clone(), state);
        }
        if let Some(Some((version, digest))) = states.get(&citation.locator) {
            citation.cited_version = Some(*version);
            citation.cited_digest = Some(digest.clone());
        }
    }
    Ok(Some(
        serde_json::to_string(&citations).unwrap_or_else(|_| json.to_string()),
    ))
}
