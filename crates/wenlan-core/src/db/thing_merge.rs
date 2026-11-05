// SPDX-License-Identifier: Apache-2.0
//! Two things that look like the same goal (docs/ux/spec.md, UX-THING-3).
//!
//! The grouping model only ever proposes a merge; the things merge when the
//! person presses it, and "keep apart" stops the same pair being proposed
//! again.
//!
//! Migration 155 adds `thing_merge_proposals`: one row per pair, keyed by
//! the two thing ids in sorted order, with where it stands: `open` (waiting
//! for the person), `apart` (they kept them apart) or `merged`. The person's
//! answers are their decisions, so the table is exported with the things in
//! `settings/things.json`.

use super::things::{assign, history};
use super::MemoryDB;
use crate::WenlanError;
use wenlan_types::{ThingActor, ThingRef};

const MERGE_PROPOSALS_DDL: &str = "
CREATE TABLE IF NOT EXISTS thing_merge_proposals (
    a TEXT NOT NULL,
    b TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('open', 'apart', 'merged')),
    proposed_at INTEGER NOT NULL,
    decided_at INTEGER,
    PRIMARY KEY (a, b),
    CHECK (a < b)
);
";

fn db_err(context: &str) -> impl Fn(libsql::Error) -> WenlanError + '_ {
    move |e| WenlanError::VectorDb(format!("{context}: {e}"))
}

/// A pair in the table's order.
fn pair<'a>(x: &'a str, y: &'a str) -> (&'a str, &'a str) {
    if x < y {
        (x, y)
    } else {
        (y, x)
    }
}

impl MemoryDB {
    /// Migration 155: the merge proposals table. Idempotent, so a crash
    /// before the version bump replays cleanly. It starts empty.
    pub(super) async fn migrate_155_thing_merge_proposals(
        &self,
        prior_version: i64,
    ) -> Result<(), WenlanError> {
        self.backup_before_migration(155, prior_version).await?;
        let conn = self.conn.lock().await;
        conn.execute_batch(MERGE_PROPOSALS_DDL)
            .await
            .map_err(db_err("m155 merge proposals DDL"))?;
        conn.execute("PRAGMA user_version = 155", ())
            .await
            .map_err(db_err("m155 bump"))?;
        log::info!("[migration] Migration 155 applied: thing merge proposals");
        Ok(())
    }

    /// The grouping model thinks these two things are one goal. A pair the
    /// person already answered is not asked again; a pair with a thing that
    /// is gone, or a thing with itself, is ignored. Returns whether the
    /// proposal is new.
    pub(super) async fn propose_thing_merge(
        tx: &libsql::Transaction,
        now: i64,
        x: &str,
        y: &str,
    ) -> Result<bool, WenlanError> {
        if x == y {
            return Ok(false);
        }
        let (a, b) = pair(x, y);
        let added = tx
            .execute(
                "INSERT OR IGNORE INTO thing_merge_proposals (a, b, status, proposed_at)
                 SELECT ?1, ?2, 'open', ?3
                  WHERE (SELECT COUNT(*) FROM things
                          WHERE id IN (?1, ?2) AND merged_into IS NULL) = 2",
                libsql::params![a, b, now],
            )
            .await
            .map_err(db_err("thing merge propose"))?;
        Ok(added > 0)
    }

    /// The things proposed to merge with `id` that still wait for the
    /// person, both things live: what its page offers to merge.
    pub async fn thing_merge_proposals(&self, id: &str) -> Result<Vec<ThingRef>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT t.id, t.title FROM thing_merge_proposals p
                   JOIN things t ON t.id = CASE WHEN p.a = ?1 THEN p.b ELSE p.a END
                   JOIN things me ON me.id = ?1
                  WHERE (p.a = ?1 OR p.b = ?1) AND p.status = 'open'
                    AND t.merged_into IS NULL AND me.merged_into IS NULL
                    AND t.title IS NOT NULL
                  ORDER BY p.proposed_at DESC, t.id",
                libsql::params![id],
            )
            .await
            .map_err(db_err("thing merge proposals"))?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.map_err(db_err("thing merge row"))? {
            out.push(ThingRef {
                id: row.get::<String>(0).unwrap_or_default(),
                title: row.get::<Option<String>>(1).unwrap_or(None),
            });
        }
        Ok(out)
    }

    /// The person presses "merge" on `id`'s page: `other` folds into `id`.
    /// Its conversations move over as the person's own placements, so the
    /// grouping model leaves them there, and `other` points at `id`. 404 for
    /// a thing that is unknown or already merged.
    pub async fn merge_things(&self, id: &str, other: &str) -> Result<(), WenlanError> {
        if id == other {
            return Err(WenlanError::Validation(
                "thing_merge_self: a thing cannot merge with itself".into(),
            ));
        }
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(libsql::TransactionBehavior::Immediate)
            .await
            .map_err(db_err("thing merge begin"))?;
        let mut rows = tx
            .query(
                "SELECT COUNT(*) FROM things WHERE id IN (?1, ?2) AND merged_into IS NULL",
                libsql::params![id, other],
            )
            .await
            .map_err(db_err("thing merge check"))?;
        let live = match rows.next().await.map_err(db_err("thing merge check row"))? {
            Some(row) => row.get::<i64>(0).unwrap_or(0),
            None => 0,
        };
        drop(rows);
        if live != 2 {
            return Err(WenlanError::NotFound("thing not found".into()));
        }
        let mut rows = tx
            .query(
                "SELECT session_id FROM thing_sessions WHERE thing_id = ?1 ORDER BY session_id",
                libsql::params![other],
            )
            .await
            .map_err(db_err("thing merge sessions"))?;
        let mut sessions = Vec::new();
        while let Some(row) = rows.next().await.map_err(db_err("thing merge session"))? {
            sessions.push(row.get::<String>(0).unwrap_or_default());
        }
        drop(rows);
        for session in &sessions {
            tx.execute(
                "DELETE FROM thing_sessions WHERE thing_id = ?1 AND session_id = ?2",
                libsql::params![other, session.as_str()],
            )
            .await
            .map_err(db_err("thing merge unassign"))?;
            history(
                &tx,
                now,
                ThingActor::Person,
                "thing.unassign",
                other,
                Some(session),
                serde_json::json!({ "rule": "merge", "merged_into": id }),
            )
            .await?;
            assign(
                &tx,
                now,
                id,
                session,
                ThingActor::Person,
                serde_json::json!({ "rule": "merge", "from": other }),
            )
            .await?;
        }
        tx.execute(
            "UPDATE things SET merged_into = ?1, updated_at = ?2 WHERE id = ?3",
            libsql::params![id, now, other],
        )
        .await
        .map_err(db_err("thing merge mark"))?;
        tx.execute(
            "UPDATE things SET updated_at = ?1 WHERE id = ?2",
            libsql::params![now, id],
        )
        .await
        .map_err(db_err("thing merge touch"))?;
        let (a, b) = pair(id, other);
        tx.execute(
            "INSERT INTO thing_merge_proposals (a, b, status, proposed_at, decided_at)
             VALUES (?1, ?2, 'merged', ?3, ?3)
             ON CONFLICT (a, b) DO UPDATE SET status = 'merged', decided_at = ?3",
            libsql::params![a, b, now],
        )
        .await
        .map_err(db_err("thing merge record"))?;
        // Proposals the merged thing still had now wait on the thing it went into.
        let mut rows = tx
            .query(
                "SELECT CASE WHEN a = ?1 THEN b ELSE a END FROM thing_merge_proposals
                  WHERE (a = ?1 OR b = ?1) AND status = 'open'",
                libsql::params![other],
            )
            .await
            .map_err(db_err("thing merge carry"))?;
        let mut carry = Vec::new();
        while let Some(row) = rows.next().await.map_err(db_err("thing merge carry row"))? {
            carry.push(row.get::<String>(0).unwrap_or_default());
        }
        drop(rows);
        for third in carry {
            Self::propose_thing_merge(&tx, now, id, &third).await?;
        }
        // Its map and its nights go along: the map hangs under this one's
        // top point, and its nights can be taken back from here.
        super::thing_map::carry_whole_map(&tx, id, other, now).await?;
        tx.execute(
            "UPDATE thing_history SET thing_id = ?1
              WHERE thing_id = ?2 AND op IN ('thing.dream', 'thing.dream_undo')",
            libsql::params![id, other],
        )
        .await
        .map_err(db_err("thing merge nights"))?;
        tx.commit().await.map_err(db_err("thing merge commit"))?;
        Ok(())
    }

    /// The person presses "keep apart": the pair is not proposed again.
    pub async fn keep_things_apart(&self, id: &str, other: &str) -> Result<(), WenlanError> {
        if id == other {
            return Err(WenlanError::Validation(
                "thing_merge_self: a thing cannot merge with itself".into(),
            ));
        }
        let now = chrono::Utc::now().timestamp();
        let (a, b) = pair(id, other);
        let conn = self.conn.lock().await;
        let changed = conn
            .execute(
                "INSERT INTO thing_merge_proposals (a, b, status, proposed_at, decided_at)
                 SELECT ?1, ?2, 'apart', ?3, ?3
                  WHERE (SELECT COUNT(*) FROM things WHERE id IN (?1, ?2)) = 2
                 ON CONFLICT (a, b) DO UPDATE SET status = 'apart', decided_at = ?3",
                libsql::params![a, b, now],
            )
            .await
            .map_err(db_err("thing keep apart"))?;
        if changed == 0 {
            return Err(WenlanError::NotFound("thing not found".into()));
        }
        Ok(())
    }
}
