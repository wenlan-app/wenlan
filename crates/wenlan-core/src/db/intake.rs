// SPDX-License-Identifier: Apache-2.0
//! What Wenlan learned from reading a conversation's transcript, and putting
//! labelled conversations into things (UX-THING-1). See
//! [`crate::conversation_intake`] for how conversations are found and read.
//!
//! Migration 154 adds `conversation_labels`: one row per conversation a model
//! has read, whether or not it was work, so it is not read again until it goes
//! on; and `thing_lines`: where a whole thing stands, written by the grouping
//! model from its conversations' labels. Both are derived: they are rebuilt by
//! reading the transcripts again.
//! A conversation that is work becomes an `agent_sessions` row like one that
//! wrote to Wenlan, so the rest of the app shows it the same way.

use super::sessions::{session_row_id, upsert_session_span};
use super::MemoryDB;
use crate::conversation_intake::{FoundConversation, Grouping, Label, OpenThing, Pending, Piece};
use crate::WenlanError;
use std::collections::HashMap;
use wenlan_types::{SessionRef, ThingActor, THING_QUIET_AFTER_SECS};

const LABELS_DDL: &str = "
CREATE TABLE IF NOT EXISTS conversation_labels (
    session_id TEXT PRIMARY KEY,
    tool TEXT NOT NULL,
    external_id TEXT NOT NULL,
    last_at INTEGER NOT NULL,
    work INTEGER NOT NULL,
    title TEXT NOT NULL DEFAULT '',
    about TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT '',
    space TEXT NOT NULL DEFAULT '',
    labeled_at INTEGER NOT NULL,
    grouped_at INTEGER
);
CREATE TABLE IF NOT EXISTS thing_lines (
    thing_id TEXT PRIMARY KEY,
    status TEXT NOT NULL,
    written_at INTEGER NOT NULL
);
";

/// A conversation that goes on is read again once its last line is this much
/// newer than when it was last read, so where it stands stays current
/// without reading it after every message.
pub const RELABEL_AFTER_SECS: i64 = 30 * 60;
/// Most things the grouping model is shown.
const OPEN_THINGS: i64 = 80;
/// A space with nothing written to it for this long is not offered to the
/// labelling model.
pub const SPACE_IN_USE_SECS: i64 = 30 * 86_400;
/// Most news lines shown for one thing.
const NEWS_PER_THING: usize = 3;

fn db_err(context: &str) -> impl Fn(libsql::Error) -> WenlanError + '_ {
    move |e| WenlanError::VectorDb(format!("{context}: {e}"))
}

/// The `agent_sessions` id a found conversation has (or would have).
pub fn found_session_id(found: &FoundConversation) -> Option<String> {
    SessionRef::new(&found.tool, &found.external_id)
        .ok()
        .map(|session| session_row_id(&session))
}

/// Whether a found conversation needs reading: never read, or gone on for a
/// while since it was.
pub fn needs_label(found: &FoundConversation, read: Option<&(i64, i64)>) -> bool {
    match read {
        None => true,
        Some(&(last_at, _)) => found.last_at > last_at + RELABEL_AFTER_SECS,
    }
}

impl MemoryDB {
    /// Migration 154: `conversation_labels`. `IF NOT EXISTS`, so a crash
    /// before the version bump replays cleanly. It starts empty.
    pub(super) async fn migrate_154_conversation_labels(
        &self,
        prior_version: i64,
    ) -> Result<(), WenlanError> {
        self.backup_before_migration(154, prior_version).await?;
        let conn = self.conn.lock().await;
        conn.execute_batch(LABELS_DDL)
            .await
            .map_err(db_err("m154 labels DDL"))?;
        conn.execute("PRAGMA user_version = 154", ())
            .await
            .map_err(db_err("m154 bump"))?;
        log::info!("[migration] Migration 154 applied: conversation labels");
        Ok(())
    }

    /// When each of these conversations was last read: its last line then,
    /// and when it was read. Conversations never read are left out.
    pub async fn conversation_reads(
        &self,
        session_ids: &[String],
    ) -> Result<HashMap<String, (i64, i64)>, WenlanError> {
        let mut out = HashMap::new();
        let conn = self.conn.lock().await;
        for chunk in session_ids.chunks(200) {
            let marks = (1..=chunk.len())
                .map(|i| format!("?{i}"))
                .collect::<Vec<_>>()
                .join(",");
            let mut rows = conn
                .query(
                    &format!(
                        "SELECT session_id, last_at, labeled_at FROM conversation_labels
                          WHERE session_id IN ({marks})"
                    ),
                    libsql::params_from_iter(chunk.iter().cloned()),
                )
                .await
                .map_err(db_err("conversation reads"))?;
            while let Some(row) = rows.next().await.map_err(db_err("conversation read row"))? {
                out.insert(
                    row.get::<String>(0).unwrap_or_default(),
                    (
                        row.get::<i64>(1).unwrap_or(0),
                        row.get::<i64>(2).unwrap_or(0),
                    ),
                );
            }
        }
        Ok(out)
    }

    /// Record what the model said about a conversation. Work becomes (or
    /// widens) its `agent_sessions` row, with the folder, branch and
    /// transcript filled where unknown and its Space set when it has none.
    /// Reading it again keeps the thing it was put in.
    pub async fn record_conversation_label(
        &self,
        found: &FoundConversation,
        label: &Label,
        repo_root: Option<&str>,
    ) -> Result<(), WenlanError> {
        let session =
            SessionRef::new(&found.tool, &found.external_id).map_err(WenlanError::Validation)?;
        let id = session_row_id(&session);
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(libsql::TransactionBehavior::Immediate)
            .await
            .map_err(db_err("label begin"))?;
        tx.execute(
            "INSERT INTO conversation_labels
                (session_id, tool, external_id, last_at, work, title, about, status, space,
                 labeled_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(session_id) DO UPDATE SET
                last_at = excluded.last_at, work = excluded.work, title = excluded.title,
                about = excluded.about, status = excluded.status, space = excluded.space,
                labeled_at = excluded.labeled_at,
                grouped_at = CASE WHEN excluded.work = 1 AND conversation_labels.work = 0
                                  THEN NULL ELSE conversation_labels.grouped_at END",
            libsql::params![
                id.as_str(),
                found.tool.as_str(),
                found.external_id.as_str(),
                found.last_at,
                i64::from(label.work),
                label.title.as_str(),
                label.about.as_str(),
                label.status.as_str(),
                label.space.as_str(),
                now
            ],
        )
        .await
        .map_err(db_err("label upsert"))?;
        if label.work {
            let space = (!label.space.is_empty()).then_some(label.space.as_str());
            upsert_session_span(&tx, &session, None, space, found.first_at, found.last_at)
                .await
                .map_err(db_err("label session"))?;
            tx.execute(
                "UPDATE agent_sessions SET
                    cwd = COALESCE(cwd, ?2), repo_root = COALESCE(repo_root, ?3),
                    repo_branch = COALESCE(repo_branch, ?4),
                    transcript_path = COALESCE(transcript_path, ?5)
                  WHERE id = ?1",
                libsql::params![
                    id.as_str(),
                    found.cwd.as_deref(),
                    repo_root,
                    found.branch.as_deref(),
                    found.path.to_string_lossy().into_owned()
                ],
            )
            .await
            .map_err(db_err("label session place"))?;
        }
        tx.commit().await.map_err(db_err("label commit"))?;
        Ok(())
    }

    /// The spaces a conversation may be put in: those with a page or memory
    /// written in the past [`SPACE_IN_USE_SECS`], so a space long out of use
    /// is not offered; every space when none was written to lately.
    pub async fn intake_spaces(&self) -> Result<Vec<String>, WenlanError> {
        let since = chrono::Utc::now().timestamp() - SPACE_IN_USE_SECS;
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT s.name FROM spaces s
                  WHERE EXISTS (SELECT 1 FROM memories m
                                 WHERE m.space = s.name AND m.last_modified >= ?1)
                     OR EXISTS (SELECT 1 FROM pages p
                                 WHERE p.space = s.name AND p.status = 'active'
                                   AND CAST(strftime('%s', p.last_modified) AS INTEGER) >= ?1)
                  ORDER BY s.sort_order, s.name",
                libsql::params![since],
            )
            .await
            .map_err(db_err("intake spaces"))?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.map_err(db_err("intake spaces row"))? {
            out.push(row.get::<String>(0).unwrap_or_default());
        }
        drop(rows);
        drop(conn);
        if out.is_empty() {
            out = self
                .list_spaces()
                .await?
                .into_iter()
                .map(|s| s.name)
                .collect();
        }
        Ok(out)
    }

    /// Work conversations of the past week not yet put into a thing, oldest
    /// first. A conversation the person or the grouping model already placed
    /// is not offered, so rebuilding the labels never places one twice.
    pub async fn intake_pending(&self, limit: usize) -> Result<Vec<Pending>, WenlanError> {
        let since = chrono::Utc::now().timestamp() - THING_QUIET_AFTER_SECS;
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT l.session_id, l.title, l.about, l.space, l.status FROM conversation_labels l
                   JOIN agent_sessions a ON a.id = l.session_id
                  WHERE l.work = 1 AND l.grouped_at IS NULL AND l.last_at >= ?1
                    AND NOT EXISTS (SELECT 1 FROM thing_sessions ts
                                      JOIN things t ON t.id = ts.thing_id
                                     WHERE ts.session_id = l.session_id AND t.merged_into IS NULL
                                       AND (ts.assigned_by = 'person' OR EXISTS
                                            (SELECT 1 FROM thing_history h
                                              WHERE h.thing_id = ts.thing_id
                                                AND h.session_id = ts.session_id
                                                AND h.op = 'thing.assign'
                                                AND json_extract(h.data, '$.rule') = 'model')))
                  ORDER BY l.last_at, l.session_id
                  LIMIT ?2",
                libsql::params![since, limit as i64],
            )
            .await
            .map_err(db_err("intake pending"))?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.map_err(db_err("intake pending row"))? {
            out.push(Pending {
                session_id: row.get::<String>(0).unwrap_or_default(),
                title: row.get::<String>(1).unwrap_or_default(),
                about: row.get::<String>(2).unwrap_or_default(),
                space: row.get::<String>(3).unwrap_or_default(),
                status: row.get::<String>(4).unwrap_or_default(),
            });
        }
        Ok(out)
    }

    /// The named things with a conversation in the past week that are not
    /// hidden, newest first, each with what its newest read conversation is
    /// about: what the grouping model may put a conversation into.
    pub async fn intake_open_things(&self) -> Result<Vec<OpenThing>, WenlanError> {
        let since = chrono::Utc::now().timestamp() - THING_QUIET_AFTER_SECS;
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT t.id, t.title, MAX(a.last_write_at) AS last,
                        (SELECT l.about FROM thing_sessions ts2
                           JOIN conversation_labels l ON l.session_id = ts2.session_id
                          WHERE ts2.thing_id = t.id AND l.work = 1
                          ORDER BY l.last_at DESC LIMIT 1),
                        (SELECT l.space FROM thing_sessions ts2
                           JOIN conversation_labels l ON l.session_id = ts2.session_id
                          WHERE ts2.thing_id = t.id AND l.work = 1 AND l.space <> ''
                          ORDER BY l.last_at DESC LIMIT 1),
                        (SELECT a2.space FROM thing_sessions ts3
                           JOIN agent_sessions a2 ON a2.id = ts3.session_id
                          WHERE ts3.thing_id = t.id AND a2.space IS NOT NULL
                          ORDER BY a2.last_write_at DESC LIMIT 1)
                   FROM things t
                   JOIN thing_sessions ts ON ts.thing_id = t.id
                   JOIN agent_sessions a ON a.id = ts.session_id
                  WHERE t.merged_into IS NULL AND t.title IS NOT NULL
                  GROUP BY t.id
                 HAVING last >= ?1 AND (t.hidden_at IS NULL OR t.hidden_at < last)
                  ORDER BY last DESC, t.id
                  LIMIT ?2",
                libsql::params![since, OPEN_THINGS],
            )
            .await
            .map_err(db_err("intake open things"))?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.map_err(db_err("intake open row"))? {
            out.push(OpenThing {
                id: row.get::<String>(0).unwrap_or_default(),
                title: row.get::<String>(1).unwrap_or_default(),
                about: row
                    .get::<Option<String>>(3)
                    .unwrap_or(None)
                    .unwrap_or_default(),
                space: row
                    .get::<Option<String>>(4)
                    .unwrap_or(None)
                    .or(row.get::<Option<String>>(5).unwrap_or(None))
                    .unwrap_or_default(),
                news: Vec::new(),
            });
        }
        drop(rows);
        // Conversations read since the thing's line was written.
        let at: HashMap<String, usize> = out
            .iter()
            .enumerate()
            .map(|(i, t)| (t.id.clone(), i))
            .collect();
        let mut rows = conn
            .query(
                "SELECT ts.thing_id, l.status FROM thing_sessions ts
                   JOIN conversation_labels l ON l.session_id = ts.session_id
                   LEFT JOIN thing_lines tl ON tl.thing_id = ts.thing_id
                  WHERE l.work = 1 AND trim(l.status) <> ''
                    AND l.labeled_at > COALESCE(tl.written_at, 0)
                  ORDER BY l.last_at DESC",
                (),
            )
            .await
            .map_err(db_err("intake news"))?;
        while let Some(row) = rows.next().await.map_err(db_err("intake news row"))? {
            if let Some(&i) = at.get(&row.get::<String>(0).unwrap_or_default()) {
                if out[i].news.len() < NEWS_PER_THING {
                    out[i].news.push(row.get::<String>(1).unwrap_or_default());
                }
            }
        }
        Ok(out)
    }

    /// Put the conversations where the grouping model said, all at once. A
    /// conversation the daemon's rules had placed (in a thing that is not a
    /// Brief item's) leaves that thing first; the person's own placements are
    /// never touched. Returns the number of things made.
    pub async fn apply_intake_grouping(
        &self,
        pending: &[Pending],
        open: &[OpenThing],
        grouping: &Grouping,
    ) -> Result<usize, WenlanError> {
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(libsql::TransactionBehavior::Immediate)
            .await
            .map_err(db_err("grouping begin"))?;
        let mut targets: Vec<(usize, String)> = Vec::new();
        for &(c, t) in &grouping.joins {
            if let (Some(_), Some(thing)) = (pending.get(c), open.get(t)) {
                targets.push((c, thing.id.clone()));
            }
        }
        let mut made = 0;
        let mut created: Vec<Option<String>> = Vec::with_capacity(grouping.new.len());
        for (title, members) in &grouping.new {
            let members: Vec<usize> = members
                .iter()
                .copied()
                .filter(|&c| c < pending.len())
                .collect();
            let title = super::things::clean_thing_title(title);
            let (Some(title), false) = (title, members.is_empty()) else {
                created.push(None);
                continue;
            };
            let thing =
                super::things::create_thing(&tx, now, Some(&title), ThingActor::Ai, None, "model")
                    .await?;
            made += 1;
            created.push(Some(thing.clone()));
            targets.extend(members.into_iter().map(|c| (c, thing.clone())));
        }
        for (c, thing) in targets {
            let session = pending[c].session_id.as_str();
            // The person may have placed it while the model was thinking.
            let mut placed = tx
                .query(
                    "SELECT 1 FROM thing_sessions ts JOIN things t ON t.id = ts.thing_id
                      WHERE ts.session_id = ?1 AND ts.assigned_by = 'person'
                        AND t.merged_into IS NULL LIMIT 1",
                    libsql::params![session],
                )
                .await
                .map_err(db_err("grouping person check"))?;
            let by_person = placed
                .next()
                .await
                .map_err(db_err("grouping person row"))?
                .is_some();
            drop(placed);
            if !by_person {
                super::things::unassign_rule_placements(&tx, now, session).await?;
                super::things::assign(
                    &tx,
                    now,
                    &thing,
                    session,
                    ThingActor::Ai,
                    serde_json::json!({ "rule": "model" }),
                )
                .await?;
            }
            tx.execute(
                "UPDATE conversation_labels SET grouped_at = ?2 WHERE session_id = ?1",
                libsql::params![session, now],
            )
            .await
            .map_err(db_err("grouping mark"))?;
        }
        // Where each piece stands now. A new thing, or an open one that
        // gained a conversation or had news, that got no line is marked
        // written with an empty one, so it shows its newest conversation's status and is not
        // offered again for nothing.
        let mut lines: HashMap<String, String> = HashMap::new();
        for (c, t) in &grouping.joins {
            if let (Some(_), Some(thing)) = (pending.get(*c), open.get(*t)) {
                lines.entry(thing.id.clone()).or_default();
            }
        }
        for thing in open.iter().filter(|t| !t.news.is_empty()) {
            lines.entry(thing.id.clone()).or_default();
        }
        for thing in created.iter().flatten() {
            lines.entry(thing.clone()).or_default();
        }
        for (piece, line) in &grouping.lines {
            let thing = match *piece {
                Piece::Open(t) => open.get(t).map(|t| t.id.clone()),
                Piece::New(n) => created.get(n).cloned().flatten(),
            };
            if let Some(thing) = thing {
                lines.insert(thing, line.clone());
            }
        }
        for (thing, line) in &lines {
            tx.execute(
                "INSERT INTO thing_lines (thing_id, status, written_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(thing_id) DO UPDATE SET status = excluded.status,
                                                     written_at = excluded.written_at",
                libsql::params![thing.as_str(), line.as_str(), now],
            )
            .await
            .map_err(db_err("grouping line"))?;
        }
        // A small chore is no piece of work: it leaves any thing the rules
        // gave it and is not offered for grouping again.
        for &c in &grouping.left_out {
            let Some(p) = pending.get(c) else { continue };
            super::things::unassign_rule_placements(&tx, now, &p.session_id).await?;
            tx.execute(
                "UPDATE conversation_labels SET work = 0, grouped_at = ?2 WHERE session_id = ?1",
                libsql::params![p.session_id.as_str(), now],
            )
            .await
            .map_err(db_err("grouping leave out"))?;
        }
        tx.commit().await.map_err(db_err("grouping commit"))?;
        Ok(made)
    }
}

#[cfg(test)]
#[path = "intake_test.rs"]
mod tests;
