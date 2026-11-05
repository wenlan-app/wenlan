// SPDX-License-Identifier: Apache-2.0
//! Overnight, what a piece of work settled goes into long-term memory
//! (docs/ux/spec.md, UX-CHAT-5).
//!
//! On screen a thing stays one thing until it has been quiet for a week.
//! Behind the scenes, each night the grouping model reads the conversations
//! that went on in it that day and writes what the person settled and left
//! open as memories, attached to the conversation each came from. Memories
//! are what the big map is drawn from, so the points join it like anything
//! else Wenlan remembers, and the usual checks run on them.
//!
//! Each night's points are one `thing.dream` entry in `thing_history`, with
//! the memories it wrote; the person can take a night back
//! (`thing.dream_undo`), which forgets them. Migration 156 widens the
//! history's list of operations for these two, and gives
//! `conversation_labels` a `dreamed_at`: the conversation's last line when it
//! was last read for points, so a conversation is read again only once it
//! went on.

use super::thing_map::DreamMapChanges;
use super::things::history;
use super::MemoryDB;
use crate::conversation_intake::{PriorPoint, MAX_DREAM_CONVERSATIONS};
use crate::read_scope::ReadScope;
use crate::WenlanError;
use std::collections::HashSet;
use wenlan_types::{ThingActor, ThingDream, ThingNightPoint, ThingNightPoints};

/// `thing_history` with the two operations added. Same columns, rows and
/// sequence numbers as before.
const HISTORY_REBUILD: &str = "
CREATE TABLE thing_history_156 (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    ts INTEGER NOT NULL,
    actor TEXT NOT NULL CHECK (actor IN ('ai', 'person')),
    op TEXT NOT NULL CHECK (op IN ('thing.create', 'thing.rename', 'thing.assign',
                                   'thing.unassign', 'thing.dream', 'thing.dream_undo')),
    thing_id TEXT NOT NULL,
    session_id TEXT,
    data TEXT NOT NULL DEFAULT '{}'
);
INSERT INTO thing_history_156 (seq, ts, actor, op, thing_id, session_id, data)
    SELECT seq, ts, actor, op, thing_id, session_id, data FROM thing_history;
DROP TABLE thing_history;
ALTER TABLE thing_history_156 RENAME TO thing_history;
CREATE INDEX IF NOT EXISTS idx_thing_history_thing ON thing_history(thing_id, seq);
";

fn db_err(context: &str) -> impl Fn(libsql::Error) -> WenlanError + '_ {
    move |e| WenlanError::VectorDb(format!("{context}: {e}"))
}

/// A conversation of a thing that went on since it was last read for points.
#[derive(Debug, Clone, PartialEq)]
pub struct DueConversation {
    pub session_id: String,
    pub tool: String,
    pub external_id: String,
    /// Its last line, when it was labelled.
    pub last_at: i64,
    /// The Space its label put it in, or empty.
    pub space: String,
}

/// A thing with conversations to read tonight, newest first.
#[derive(Debug, Clone, PartialEq)]
pub struct DreamDue {
    pub thing_id: String,
    pub title: String,
    pub conversations: Vec<DueConversation>,
}

/// One night's points, as `thing_history` keeps them.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct DreamEntry {
    #[serde(default)]
    decided: Vec<String>,
    #[serde(default)]
    open: Vec<String>,
    /// What it changed on the thing's map.
    #[serde(default)]
    map: DreamMapChanges,
}

impl MemoryDB {
    /// Migration 156: the two history operations and `dreamed_at`. A crash
    /// before the version bump replays cleanly: the column is added only
    /// when missing, and the history is rebuilt only while it still refuses
    /// the new operations.
    pub(super) async fn migrate_156_thing_dreams(
        &self,
        prior_version: i64,
    ) -> Result<(), WenlanError> {
        self.backup_before_migration(156, prior_version).await?;
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'thing_history'",
                (),
            )
            .await
            .map_err(db_err("m156 history sql"))?;
        let sql = match rows.next().await.map_err(db_err("m156 history sql row"))? {
            Some(row) => row.get::<String>(0).unwrap_or_default(),
            None => String::new(),
        };
        drop(rows);
        if !sql.contains("thing.dream") {
            conn.execute_batch(&format!(
                "BEGIN IMMEDIATE;\nDROP TABLE IF EXISTS thing_history_156;\n{HISTORY_REBUILD}\nCOMMIT;"
            ))
            .await
            .map_err(db_err("m156 history rebuild"))?;
        }
        let mut rows = conn
            .query(
                "SELECT 1 FROM pragma_table_info('conversation_labels') WHERE name = 'dreamed_at'",
                (),
            )
            .await
            .map_err(db_err("m156 labels columns"))?;
        let has_column = rows
            .next()
            .await
            .map_err(db_err("m156 labels column row"))?
            .is_some();
        drop(rows);
        if !has_column {
            conn.execute(
                "ALTER TABLE conversation_labels ADD COLUMN dreamed_at INTEGER",
                (),
            )
            .await
            .map_err(db_err("m156 dreamed_at"))?;
        }
        conn.execute("PRAGMA user_version = 156", ())
            .await
            .map_err(db_err("m156 bump"))?;
        log::info!("[migration] Migration 156 applied: overnight points of a thing");
        Ok(())
    }

    /// Things whose conversations went on since they were last read for
    /// points, newest first, at most `limit`. Only conversations whose last
    /// line is at or before `quiet_since` count, so one still going on waits.
    pub async fn things_to_dream(
        &self,
        quiet_since: i64,
        limit: usize,
    ) -> Result<Vec<DreamDue>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT t.id, t.title, cl.session_id, cl.tool, cl.external_id, cl.last_at, cl.space
                   FROM conversation_labels cl
                   JOIN thing_sessions ts ON ts.session_id = cl.session_id
                   JOIN things t ON t.id = ts.thing_id
                  WHERE cl.work = 1 AND t.merged_into IS NULL AND t.title IS NOT NULL
                    AND (cl.dreamed_at IS NULL OR cl.last_at > cl.dreamed_at)
                    AND cl.last_at <= ?1
                  ORDER BY cl.last_at DESC, cl.session_id",
                libsql::params![quiet_since],
            )
            .await
            .map_err(db_err("things to dream"))?;
        let mut due: Vec<DreamDue> = Vec::new();
        while let Some(row) = rows.next().await.map_err(db_err("things to dream row"))? {
            let thing_id = row.get::<String>(0).unwrap_or_default();
            let conversation = DueConversation {
                session_id: row.get::<String>(2).unwrap_or_default(),
                tool: row.get::<String>(3).unwrap_or_default(),
                external_id: row.get::<String>(4).unwrap_or_default(),
                last_at: row.get::<i64>(5).unwrap_or(0),
                space: row.get::<String>(6).unwrap_or_default(),
            };
            match due.iter().position(|d| d.thing_id == thing_id) {
                Some(i) if due[i].conversations.len() < MAX_DREAM_CONVERSATIONS => {
                    due[i].conversations.push(conversation);
                }
                Some(_) => {}
                None if due.len() < limit => due.push(DreamDue {
                    thing_id,
                    title: row.get::<String>(1).unwrap_or_default(),
                    conversations: vec![conversation],
                }),
                None => {}
            }
        }
        Ok(due)
    }

    /// The points earlier nights wrote for a thing that still stand: not
    /// taken back, not forgotten, not replaced. Oldest first.
    pub async fn dream_points(&self, thing_id: &str) -> Result<Vec<PriorPoint>, WenlanError> {
        Ok(self
            .standing_dream_points(thing_id)
            .await?
            .into_iter()
            .map(|point| PriorPoint {
                id: point.id,
                text: point.text,
                decided: point.decided,
            })
            .collect())
    }

    /// Every live thing the caller may see with points its nights wrote that
    /// still stand, as the big map draws them: the thing, and each point's
    /// memory, title and when it was written (UX-CHAT-5). The point's own
    /// Space must be in scope too. Newest activity first, as `list_things`.
    pub async fn thing_night_points(
        &self,
        scope: &ReadScope,
    ) -> Result<Vec<ThingNightPoints>, WenlanError> {
        let mut out = Vec::new();
        for thing in self.list_things(scope, None).await? {
            // `dreamed_at` is the newest night not taken back: none, no points.
            if thing.dreamed_at.is_none() {
                continue;
            }
            let points: Vec<ThingNightPoint> = self
                .standing_dream_points(&thing.id)
                .await?
                .into_iter()
                .filter(|point| scope.matches(point.space.as_deref()))
                .map(|point| ThingNightPoint {
                    title: short_title(&point.title, &point.text),
                    memory_id: point.id,
                    decided: point.decided,
                    created_at: point.created_at,
                })
                .collect();
            if points.is_empty() {
                continue;
            }
            out.push(ThingNightPoints {
                thing_id: thing.id,
                title: thing.title,
                spaces: thing.spaces,
                quiet: thing.quiet,
                points,
            });
        }
        Ok(out)
    }

    /// The points of a thing's nights that were not taken back and whose
    /// memory still stands (not forgotten, not replaced), oldest first.
    async fn standing_dream_points(
        &self,
        thing_id: &str,
    ) -> Result<Vec<StandingPoint>, WenlanError> {
        let nights = self.dream_nights(thing_id).await?;
        let mut decided = HashSet::new();
        let mut ids = Vec::new();
        for (_, _, night) in nights.into_iter().filter(|(_, undone, _)| !undone) {
            let entry = night.entry;
            decided.extend(entry.decided.iter().cloned());
            ids.extend(entry.decided);
            ids.extend(entry.open);
        }
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.conn.lock().await;
        let mut points = Vec::new();
        for id in ids {
            let mut rows = conn
                .query(
                    "SELECT MAX(m.content), MAX(m.title),
                            COALESCE(MIN(m.created_at), MIN(m.last_modified)), MAX(m.space)
                       FROM memories m
                      WHERE m.source = 'memory' AND m.source_id = ?1 AND m.pending_revision = 0
                        AND NOT EXISTS (SELECT 1 FROM memories s
                                         WHERE s.supersedes = m.source_id AND s.source = 'memory'
                                           AND s.pending_revision = 0)
                      GROUP BY m.source_id",
                    libsql::params![id.as_str()],
                )
                .await
                .map_err(db_err("dream point"))?;
            if let Some(row) = rows.next().await.map_err(db_err("dream point row"))? {
                let text = row.get::<String>(0).unwrap_or_default();
                if !text.is_empty() {
                    points.push(StandingPoint {
                        decided: decided.contains(&id),
                        id,
                        text,
                        title: row.get::<String>(1).unwrap_or_default(),
                        created_at: row.get::<i64>(2).unwrap_or(0),
                        space: crate::space_context::normalize_unfiled_space(
                            row.get::<Option<String>>(3).unwrap_or(None),
                        ),
                    });
                }
            }
        }
        Ok(points)
    }

    /// Every night's entry for a thing, oldest first: its sequence number,
    /// whether it was taken back, and what it wrote.
    async fn dream_nights(
        &self,
        thing_id: &str,
    ) -> Result<Vec<(i64, bool, DreamEntryWithAt)>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT seq, ts, op, data FROM thing_history
                  WHERE thing_id = ?1 AND op IN ('thing.dream', 'thing.dream_undo')
                  ORDER BY seq",
                libsql::params![thing_id],
            )
            .await
            .map_err(db_err("thing dreams"))?;
        let mut nights: Vec<(i64, bool, DreamEntryWithAt)> = Vec::new();
        while let Some(row) = rows.next().await.map_err(db_err("thing dream row"))? {
            let seq = row.get::<i64>(0).unwrap_or(0);
            let at = row.get::<i64>(1).unwrap_or(0);
            let op = row.get::<String>(2).unwrap_or_default();
            let data = row.get::<String>(3).unwrap_or_default();
            if op == "thing.dream" {
                let entry: DreamEntry = serde_json::from_str(&data).unwrap_or_default();
                nights.push((seq, false, DreamEntryWithAt { at, entry }));
            } else if let Some(undone) = serde_json::from_str::<serde_json::Value>(&data)
                .ok()
                .and_then(|v| v.get("seq").and_then(|s| s.as_i64()))
            {
                if let Some(night) = nights.iter_mut().find(|(s, _, _)| *s == undone) {
                    night.1 = true;
                }
            }
        }
        Ok(nights)
    }

    /// The newest night that wrote points for a thing and was not taken
    /// back: what its page says was put away.
    pub async fn thing_dream(&self, thing_id: &str) -> Result<Option<ThingDream>, WenlanError> {
        let nights = self.dream_nights(thing_id).await?;
        Ok(nights
            .into_iter()
            .rev()
            .find(|(_, undone, _)| !undone)
            .map(|(_, _, night)| ThingDream {
                at: night.at,
                decided: night.entry.decided.len(),
                open: night.entry.open.len(),
                fresh: night
                    .entry
                    .decided
                    .iter()
                    .chain(&night.entry.open)
                    .cloned()
                    .collect(),
            }))
    }

    /// Record a night's reading of a thing: the memories it wrote, settled
    /// and open, what that changed on its map, and that its conversations
    /// were read up to their last line. A night that found nothing new
    /// records only the reading.
    pub async fn record_dream(
        &self,
        thing_id: &str,
        decided: &[String],
        open: &[String],
        map: &DreamMapChanges,
        conversations: &[DueConversation],
    ) -> Result<(), WenlanError> {
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(libsql::TransactionBehavior::Immediate)
            .await
            .map_err(db_err("record dream begin"))?;
        if !decided.is_empty() || !open.is_empty() {
            history(
                &tx,
                now,
                ThingActor::Ai,
                "thing.dream",
                thing_id,
                None,
                serde_json::json!({ "decided": decided, "open": open, "map": map }),
            )
            .await?;
        }
        for conversation in conversations {
            tx.execute(
                "UPDATE conversation_labels SET dreamed_at = ?1 WHERE session_id = ?2",
                libsql::params![conversation.last_at, conversation.session_id.as_str()],
            )
            .await
            .map_err(db_err("record dream read"))?;
        }
        tx.commit().await.map_err(db_err("record dream commit"))?;
        Ok(())
    }

    /// Attach a point written overnight to the conversation it came from, so
    /// the thing's page lists it, without counting as activity in that
    /// conversation: a night's reading must not keep a thing on home.
    pub async fn attach_dream_point(
        &self,
        source_id: &str,
        session_id: &str,
    ) -> Result<(), WenlanError> {
        let conn = self.conn.lock().await;
        conn.execute(
            "UPDATE memories SET session_id = ?1
              WHERE source = 'memory' AND source_id = ?2 AND session_id IS NULL",
            libsql::params![session_id, source_id],
        )
        .await
        .map_err(db_err("attach dream point"))?;
        Ok(())
    }

    /// The person takes back the newest night of a thing that was not taken
    /// back yet, and what it changed on the thing's map. Returns the
    /// memories it wrote, for the caller to forget; empty when there is
    /// nothing to take back. Its conversations are not read again for the
    /// same lines.
    pub async fn undo_thing_dream(&self, thing_id: &str) -> Result<Vec<String>, WenlanError> {
        let nights = self.dream_nights(thing_id).await?;
        let Some((seq, _, night)) = nights.into_iter().rev().find(|(_, undone, _)| !undone) else {
            return Ok(Vec::new());
        };
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(libsql::TransactionBehavior::Immediate)
            .await
            .map_err(db_err("undo dream begin"))?;
        history(
            &tx,
            now,
            ThingActor::Person,
            "thing.dream_undo",
            thing_id,
            None,
            serde_json::json!({ "seq": seq }),
        )
        .await?;
        tx.commit().await.map_err(db_err("undo dream commit"))?;
        drop(conn);
        self.undo_dream_map(thing_id, &night.entry.map).await?;
        let mut ids = night.entry.decided;
        ids.extend(night.entry.open);
        Ok(ids)
    }
}

/// A night point whose memory still stands.
#[derive(Debug)]
struct StandingPoint {
    id: String,
    text: String,
    title: String,
    created_at: i64,
    /// The memory's Space, `None` when unfiled.
    space: Option<String>,
    decided: bool,
}

/// Longest title the map labels a point with, in characters.
const POINT_TITLE_MAX_CHARS: usize = 120;

/// A point's title, else the first line of its text, cut to
/// [`POINT_TITLE_MAX_CHARS`].
fn short_title(title: &str, text: &str) -> String {
    let source = if title.trim().is_empty() {
        text.lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("")
    } else {
        title
    };
    let source = source.trim();
    if source.chars().count() <= POINT_TITLE_MAX_CHARS {
        return source.to_string();
    }
    let cut: String = source.chars().take(POINT_TITLE_MAX_CHARS - 1).collect();
    format!("{}…", cut.trim_end())
}

/// One night's entry and when it ran.
#[derive(Debug, Default)]
struct DreamEntryWithAt {
    at: i64,
    entry: DreamEntry,
}
