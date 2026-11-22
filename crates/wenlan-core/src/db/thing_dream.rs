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

use super::night_links::{remove_night_links, NightChange, NightLink};
use super::night_tidy::{apply_tidies, undo_tidies, TidyApplied};
use super::thing_map::DreamMapChanges;
use super::things::history;
use super::MemoryDB;
use crate::conversation_intake::{PriorPoint, PriorWorking, WorkingKind, MAX_DREAM_CONVERSATIONS};
use crate::read_scope::ReadScope;
use crate::WenlanError;
use std::collections::{HashMap, HashSet};
use wenlan_types::{
    NightLinkMade, NightTidied, ThingActor, ThingConclusionSentence, ThingDream, ThingLastNight,
    ThingNightPoint, ThingNightPoints, ThingReplaced, ThingTidy,
};

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
    /// The preferences, tool problems and lessons it kept. They are not
    /// points of the thing (nothing lists them as settled or open); the
    /// entry holds them so Undo forgets them with the night and later nights
    /// can tell which are standing. Older entries have none.
    #[serde(default)]
    working: Vec<String>,
    /// The night kept only preferences, tool problems or lessons: it is not
    /// "the night" of the thing (no morning line, no Undo of its own, no
    /// conclusion), only a record of what it kept.
    #[serde(default)]
    working_only: bool,
    /// What it changed on the thing's map.
    #[serde(default)]
    map: DreamMapChanges,
    /// Everything it changed on the big map. Older entries have none.
    #[serde(default)]
    changes: Vec<NightChange>,
    /// Old points connected once by hand, not a night: it wrote no point and
    /// has its own undo (see `night_links`).
    #[serde(default)]
    backfill: bool,
    /// What the work has come to, written after the night's points from all
    /// the thing's standing points (UX-VIEW-6). Older entries, and a night
    /// whose conclusion call failed, have none.
    #[serde(default)]
    conclusion: Vec<ThingConclusionSentence>,
}

impl DreamEntry {
    /// A night of the thing: not the one-time connecting of old points, and
    /// not a record of preferences, tool problems and lessons alone.
    fn is_night(&self) -> bool {
        !self.backfill && !self.working_only
    }
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

    /// Migration 158: `conversation_labels.unread_at`, the last line of a
    /// conversation whose transcript was gone when a night came to read it,
    /// and `memories.said_line`, the line the person typed that a night's
    /// point came from (UX-WHO-6). Both columns are added only when missing,
    /// so a crash before the version bump replays cleanly.
    pub(super) async fn migrate_158_dream_reading(
        &self,
        prior_version: i64,
    ) -> Result<(), WenlanError> {
        self.backup_before_migration(158, prior_version).await?;
        let conn = self.conn.lock().await;
        for (table, column) in [
            ("conversation_labels", "unread_at INTEGER"),
            ("memories", "said_line TEXT"),
        ] {
            let name = column.split(' ').next().unwrap_or_default();
            let mut rows = conn
                .query(
                    &format!("SELECT 1 FROM pragma_table_info('{table}') WHERE name = '{name}'"),
                    (),
                )
                .await
                .map_err(db_err("m158 columns"))?;
            let has_column = rows
                .next()
                .await
                .map_err(db_err("m158 column row"))?
                .is_some();
            drop(rows);
            if !has_column {
                conn.execute(&format!("ALTER TABLE {table} ADD COLUMN {column}"), ())
                    .await
                    .map_err(db_err("m158 add column"))?;
            }
        }
        conn.execute("PRAGMA user_version = 158", ())
            .await
            .map_err(db_err("m158 bump"))?;
        log::info!("[migration] Migration 158 applied: what a night could not read, and where its points came from");
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
                    AND (cl.unread_at IS NULL OR cl.last_at > cl.unread_at)
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
        self.dream_points_where(thing_id, |_| true).await
    }

    /// [`Self::dream_points`] kept in `space` (`""` for none), by the Space
    /// of the memory itself, never by a conversation's label (a label is a
    /// model's guess, and a conversation labelled with a shared Space may
    /// have been kept personal): a night reading one Space's conversations
    /// is never shown another's and always sees its own.
    pub async fn dream_points_in(
        &self,
        thing_id: &str,
        space: &str,
    ) -> Result<Vec<PriorPoint>, WenlanError> {
        self.dream_points_where(thing_id, |point| {
            point.space.as_deref().unwrap_or_default() == space
        })
        .await
    }

    /// The preferences, tool problems and lessons the thing's nights kept
    /// that were not taken back and still stand (not forgotten, not
    /// replaced), oldest first, kept in `space` (`""` for none): a night
    /// reading one Space's conversations is never shown what another's kept.
    pub async fn dream_working_in(
        &self,
        thing_id: &str,
        space: &str,
    ) -> Result<Vec<PriorWorking>, WenlanError> {
        let nights = self.dream_nights(thing_id).await?;
        let ids = Self::working_ids(nights.into_iter().filter(|(_, undone, _)| !undone));
        Ok(self
            .standing_working(ids, Some(space))
            .await?
            .into_iter()
            .collect())
    }

    /// The preferences any thing's nights kept that were not taken back and
    /// still stand, oldest first, at most the newest `limit`. A preference is
    /// about the person, not about one piece of work, so a night is shown
    /// the person's standing preferences from every thing, never the same
    /// one twice.
    pub async fn dream_preferences(&self, limit: usize) -> Result<Vec<PriorWorking>, WenlanError> {
        let rows: Vec<String> = {
            let conn = self.conn.lock().await;
            let mut rows = conn
                .query(
                    "SELECT h.data FROM thing_history h
                      WHERE h.op = 'thing.dream'
                        AND NOT EXISTS (SELECT 1 FROM thing_history u
                                         WHERE u.thing_id = h.thing_id
                                           AND u.op = 'thing.dream_undo'
                                           AND json_extract(u.data, '$.seq') = h.seq)
                      ORDER BY h.seq",
                    (),
                )
                .await
                .map_err(db_err("dream preferences"))?;
            let mut out = Vec::new();
            while let Some(row) = rows.next().await.map_err(db_err("dream preferences row"))? {
                out.push(row.get::<String>(0).unwrap_or_default());
            }
            out
        };
        let mut ids = Vec::new();
        for data in rows {
            let entry: DreamEntry = serde_json::from_str(&data).unwrap_or_default();
            ids.extend(Self::working_ids(std::iter::once((
                0,
                false,
                DreamEntryWithAt { at: 0, entry },
            ))));
        }
        // And the confirmed preferences in no Space that the person or their
        // tools saved themselves: a night's newer one can replace them too.
        let saved = self.person_preferences(limit).await?;
        ids.extend(saved.into_iter().rev().map(|(id, _)| id));
        let mut seen = HashSet::new();
        ids.retain(|id| seen.insert(id.clone()));
        let mut standing: Vec<PriorWorking> = self
            .standing_working(ids, None)
            .await?
            .into_iter()
            .filter(|w| w.kind == WorkingKind::Preference)
            .collect();
        let skip = standing.len().saturating_sub(limit);
        standing.drain(..skip);
        Ok(standing)
    }

    /// The memories nights kept as preferences, tool problems or lessons, or
    /// tidied them into one, oldest first.
    fn working_ids(nights: impl Iterator<Item = (i64, bool, DreamEntryWithAt)>) -> Vec<String> {
        let mut ids = Vec::new();
        for (_, _, night) in nights {
            ids.extend(night.entry.working);
            for change in &night.entry.changes {
                if let NightChange::Tidy {
                    new, wrote: true, ..
                } = change
                {
                    ids.push(new.clone());
                }
            }
        }
        ids
    }

    /// Those of `ids` that still stand (not forgotten, hidden or replaced)
    /// and are a preference, tool problem or lesson, in the order of `ids`.
    /// With `space`, only those kept in that Space (`""` for none). A memory
    /// the person has not confirmed yet is still here: it stands.
    async fn standing_working(
        &self,
        ids: Vec<String>,
        space: Option<&str>,
    ) -> Result<Vec<PriorWorking>, WenlanError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let wanted = serde_json::to_string(&ids).unwrap_or_else(|_| "[]".into());
        let mut found: HashMap<String, PriorWorking> = HashMap::new();
        {
            let conn = self.conn.lock().await;
            // One query for all of them.
            let mut rows = conn
                .query(
                    "SELECT m.source_id, MAX(m.content), MAX(m.memory_type), MAX(m.space)
                       FROM memories m
                      WHERE m.source = 'memory' AND m.pending_revision = 0
                        AND m.source_id IN (SELECT value FROM json_each(?1))
                        AND COALESCE(m.supersede_mode, '') <> 'evicted'
                        AND NOT EXISTS (SELECT 1 FROM memories s
                                         WHERE s.supersedes = m.source_id AND s.source = 'memory'
                                           AND s.pending_revision = 0)
                      GROUP BY m.source_id",
                    libsql::params![wanted],
                )
                .await
                .map_err(db_err("dream working"))?;
            while let Some(row) = rows.next().await.map_err(db_err("dream working row"))? {
                let id = row.get::<String>(0).unwrap_or_default();
                let text = row.get::<String>(1).unwrap_or_default();
                let kind = row
                    .get::<Option<String>>(2)
                    .unwrap_or(None)
                    .and_then(|t| WorkingKind::from_memory_type(&t));
                let memory_space = crate::space_context::normalize_unfiled_space(
                    row.get::<Option<String>>(3).unwrap_or(None),
                );
                if let (false, Some(kind), true) = (
                    text.is_empty(),
                    kind,
                    space.is_none_or(|space| memory_space.as_deref().unwrap_or_default() == space),
                ) {
                    found.insert(
                        id.clone(),
                        PriorWorking {
                            id,
                            text,
                            kind,
                            space: memory_space,
                        },
                    );
                }
            }
        }
        // In the order the nights kept them.
        Ok(ids.into_iter().filter_map(|id| found.remove(&id)).collect())
    }

    async fn dream_points_where(
        &self,
        thing_id: &str,
        keep: impl Fn(&StandingPoint) -> bool,
    ) -> Result<Vec<PriorPoint>, WenlanError> {
        Ok(self
            .standing_dream_points(thing_id)
            .await?
            .into_iter()
            .filter(|point| keep(point))
            .map(|point| PriorPoint {
                id: point.id,
                text: point.text,
                decided: point.decided,
                space: point.space,
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
            let last_night = self.thing_last_night(&thing.id, &points, scope).await?;
            // A thing whose night only tidied still has a night to show.
            if points.is_empty() && last_night.is_none() {
                continue;
            }
            out.push(ThingNightPoints {
                thing_id: thing.id,
                title: thing.title,
                spaces: thing.spaces,
                quiet: thing.quiet,
                points,
                last_night,
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
            // What a tidy wrote stands in for what it hid, as a point of the
            // thing: settled when one of those was.
            for change in &entry.changes {
                if let NightChange::Tidy {
                    old,
                    new,
                    wrote: true,
                    ..
                } = change
                {
                    if old.iter().any(|id| decided.contains(id)) {
                        decided.insert(new.clone());
                    }
                    ids.push(new.clone());
                }
            }
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
                            COALESCE(MIN(m.created_at), MIN(m.last_modified)), MAX(m.space),
                            MAX(m.memory_type)
                       FROM memories m
                      WHERE m.source = 'memory' AND m.source_id = ?1 AND m.pending_revision = 0
                        AND COALESCE(m.supersede_mode, '') <> 'evicted'
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
                // A merged tool problem or lesson is not a point of the thing.
                let working = row
                    .get::<Option<String>>(4)
                    .unwrap_or(None)
                    .is_some_and(|kind| WorkingKind::from_memory_type(&kind).is_some());
                if !text.is_empty() && !working {
                    let space = crate::space_context::normalize_unfiled_space(
                        row.get::<Option<String>>(3).unwrap_or(None),
                    );
                    points.push(StandingPoint {
                        decided: decided.contains(&id),
                        id,
                        text,
                        title: row.get::<String>(1).unwrap_or_default(),
                        created_at: row.get::<i64>(2).unwrap_or(0),
                        space,
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
        self.thing_dream_in(thing_id, &ReadScope::Global).await
    }

    /// [`Self::thing_dream`] for a caller that sees only `scope`. The
    /// conclusion is that same night's, never an older one, so the first
    /// paragraph and the night's Undo speak of one night. A sentence is left
    /// out unless every point it rests on still stands (not taken back,
    /// forgotten or replaced) and is in scope: it would otherwise say
    /// something its sources no longer back.
    pub async fn thing_dream_in(
        &self,
        thing_id: &str,
        scope: &ReadScope,
    ) -> Result<Option<ThingDream>, WenlanError> {
        let nights = self.dream_nights(thing_id).await?;
        let Some((_, _, night)) = nights
            .into_iter()
            .rev()
            .find(|(_, undone, night)| !undone && night.entry.is_night())
        else {
            return Ok(None);
        };
        let conclusion = if night.entry.conclusion.is_empty() {
            Vec::new()
        } else {
            let standing: HashSet<String> = self
                .standing_dream_points(thing_id)
                .await?
                .into_iter()
                .filter(|point| scope.matches(point.space.as_deref()))
                .map(|point| point.id)
                .collect();
            night
                .entry
                .conclusion
                .into_iter()
                .filter(|sentence| {
                    !sentence.points.is_empty()
                        && sentence.points.iter().all(|id| standing.contains(id))
                })
                .collect()
        };
        Ok(Some(ThingDream {
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
            conclusion,
        }))
    }

    /// Put the conclusion on the night `seq` that [`Self::record_dream`]
    /// wrote. Nothing changes when that entry is not this thing's night.
    pub async fn set_dream_conclusion(
        &self,
        thing_id: &str,
        seq: i64,
        conclusion: &[ThingConclusionSentence],
    ) -> Result<(), WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT data FROM thing_history
                  WHERE seq = ?1 AND thing_id = ?2 AND op = 'thing.dream'",
                libsql::params![seq, thing_id],
            )
            .await
            .map_err(db_err("dream conclusion read"))?;
        let Some(row) = rows.next().await.map_err(db_err("dream conclusion row"))? else {
            return Ok(());
        };
        let mut data: serde_json::Value =
            serde_json::from_str(&row.get::<String>(0).unwrap_or_default())
                .unwrap_or_else(|_| serde_json::json!({}));
        drop(rows);
        let Some(object) = data.as_object_mut() else {
            return Ok(());
        };
        object.insert(
            "conclusion".into(),
            serde_json::to_value(conclusion).unwrap_or_default(),
        );
        conn.execute(
            "UPDATE thing_history SET data = ?1 WHERE seq = ?2",
            libsql::params![data.to_string(), seq],
        )
        .await
        .map_err(db_err("dream conclusion write"))?;
        Ok(())
    }

    /// What a thing's newest night not taken back did on the big map
    /// (UX-GRAPH-5): the points it wrote and the links it made, kept only
    /// where the point is among `standing` (still there, and in the caller's
    /// scope). `None` when that leaves nothing, so the morning line counts
    /// only what can still be seen.
    async fn thing_last_night(
        &self,
        thing_id: &str,
        standing: &[ThingNightPoint],
        scope: &ReadScope,
    ) -> Result<Option<ThingLastNight>, WenlanError> {
        let nights = self.dream_nights(thing_id).await?;
        let Some((_, _, night)) = nights
            .into_iter()
            .rev()
            .find(|(_, undone, night)| !undone && night.entry.is_night())
        else {
            return Ok(None);
        };
        // A replace keeps a memory that may not be one of the thing's
        // points: it counts while it still stands in the caller's scope.
        let mut kept = HashSet::new();
        let conn = self.conn.lock().await;
        for change in &night.entry.changes {
            let NightChange::Tidy {
                new, wrote: false, ..
            } = change
            else {
                continue;
            };
            let mut rows = conn
                .query(
                    "SELECT space FROM memories
                      WHERE source = 'memory' AND source_id = ?1 AND chunk_index = 0
                        AND COALESCE(supersede_mode, '') <> 'evicted'",
                    libsql::params![new.as_str()],
                )
                .await
                .map_err(db_err("tidy kept"))?;
            if let Some(row) = rows.next().await.map_err(db_err("tidy kept row"))? {
                let space = crate::space_context::normalize_unfiled_space(
                    row.get::<Option<String>>(0).unwrap_or(None),
                );
                if scope.matches(space.as_deref()) {
                    kept.insert(new.clone());
                }
            }
        }
        let Some(mut last) = last_night_of(&night, standing, &kept) else {
            return Ok(None);
        };
        // The words each tidy took away, for Replay (UX-GRAPH-7). A row no
        // longer hidden says nothing; the order stays `replaced`'s.
        for tidy in &mut last.tidied {
            for id in &tidy.replaced {
                let mut rows = conn
                    .query(
                        "SELECT content FROM memories
                          WHERE source = 'memory' AND source_id = ?1 AND chunk_index = 0
                            AND supersede_mode = 'evicted'",
                        libsql::params![id.as_str()],
                    )
                    .await
                    .map_err(db_err("tidy said"))?;
                if let Some(row) = rows.next().await.map_err(db_err("tidy said row"))? {
                    tidy.said.push(row.get::<String>(0).unwrap_or_default());
                }
            }
        }
        Ok(Some(last))
    }

    /// What a thing's nights tidied that can still be taken back one at a
    /// time (UX-GRAPH-7): nights not taken back, whose hidden rows are still
    /// hidden, newest first, with the text that was there.
    pub async fn thing_tidies(&self, thing_id: &str) -> Result<Vec<ThingTidy>, WenlanError> {
        let nights = self.dream_nights(thing_id).await?;
        let conn = self.conn.lock().await;
        let mut out = Vec::new();
        for (_, _, night) in nights.into_iter().rev().filter(|(_, undone, _)| !undone) {
            for change in night.entry.changes {
                let NightChange::Tidy { how, old, new, .. } = change else {
                    continue;
                };
                // A later tidy took this one's place: that one is shown.
                if out.iter().any(|t: &ThingTidy| t.memory_id == new) {
                    continue;
                }
                let mut rows = conn
                    .query(
                        "SELECT 1 FROM memories
                          WHERE source = 'memory' AND source_id = ?1 AND chunk_index = 0
                            AND COALESCE(supersede_mode, '') <> 'evicted'",
                        libsql::params![new.as_str()],
                    )
                    .await
                    .map_err(db_err("tidy stands"))?;
                let standing = rows
                    .next()
                    .await
                    .map_err(db_err("tidy stands row"))?
                    .is_some();
                drop(rows);
                if !standing {
                    continue;
                }
                // In id order, like the night's line: the model may name
                // them in any order.
                let mut old = old;
                old.sort();
                let mut replaced = Vec::new();
                for id in old {
                    let mut rows = conn
                        .query(
                            "SELECT content FROM memories
                              WHERE source = 'memory' AND source_id = ?1 AND chunk_index = 0
                                AND supersede_mode = 'evicted'",
                            libsql::params![id.as_str()],
                        )
                        .await
                        .map_err(db_err("tidy hidden"))?;
                    if let Some(row) = rows.next().await.map_err(db_err("tidy hidden row"))? {
                        replaced.push(ThingReplaced {
                            source_id: id,
                            text: row.get::<String>(0).unwrap_or_default(),
                        });
                    }
                }
                if !replaced.is_empty() {
                    out.push(ThingTidy {
                        kind: how,
                        memory_id: new,
                        replaced,
                    });
                }
            }
        }
        Ok(out)
    }

    /// The person takes back one tidy from the memory that stands now
    /// (UX-GRAPH-7): what it hid comes back, what it wrote is forgotten, and
    /// the night no longer lists it, all in one transaction. Returns the
    /// memories forgotten; `None` when no night not taken back made it.
    pub async fn undo_tidy(
        &self,
        thing_id: &str,
        memory_id: &str,
    ) -> Result<Option<Vec<String>>, WenlanError> {
        let nights = self.dream_nights(thing_id).await?;
        // Newest night first, as the page lists them.
        let Some((seq, change)) = nights
            .into_iter()
            .rev()
            .filter(|(_, undone, _)| !undone)
            .find_map(|(seq, _, night)| {
                night
                    .entry
                    .changes
                    .into_iter()
                    .find(|c| matches!(c, NightChange::Tidy { new, .. } if new == memory_id))
                    .map(|c| (seq, c))
            })
        else {
            return Ok(None);
        };
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(libsql::TransactionBehavior::Immediate)
            .await
            .map_err(db_err("undo tidy begin"))?;
        // A later tidy hid what this one wrote: take that one back first.
        let mut rows = tx
            .query(
                "SELECT 1 FROM memories
                  WHERE source = 'memory' AND source_id = ?1 AND chunk_index = 0
                    AND COALESCE(supersede_mode, '') <> 'evicted'",
                libsql::params![memory_id],
            )
            .await
            .map_err(db_err("undo tidy stands"))?;
        let standing = rows
            .next()
            .await
            .map_err(db_err("undo tidy stands row"))?
            .is_some();
        drop(rows);
        if !standing {
            return Ok(None);
        }
        let forgotten = undo_tidies(&tx, thing_id, std::slice::from_ref(&change)).await?;
        for id in &forgotten {
            self.delete_by_source_id_in_transaction(&tx, "memory", id, false)
                .await?;
        }
        let mut rows = tx
            .query(
                "SELECT data FROM thing_history WHERE seq = ?1",
                libsql::params![seq],
            )
            .await
            .map_err(db_err("undo tidy read"))?;
        let data = match rows.next().await.map_err(db_err("undo tidy row"))? {
            Some(row) => row.get::<String>(0).unwrap_or_default(),
            None => String::new(),
        };
        drop(rows);
        let mut entry: serde_json::Value =
            serde_json::from_str(&data).unwrap_or_else(|_| serde_json::json!({}));
        if let Some(changes) = entry.get_mut("changes").and_then(|c| c.as_array_mut()) {
            let gone = serde_json::to_value(&change).unwrap_or_default();
            changes.retain(|c| *c != gone);
        }
        tx.execute(
            "UPDATE thing_history SET data = ?1 WHERE seq = ?2",
            libsql::params![entry.to_string(), seq],
        )
        .await
        .map_err(db_err("undo tidy write"))?;
        tx.commit().await.map_err(db_err("undo tidy commit"))?;
        Ok(Some(forgotten))
    }

    /// The person takes back a whole night on the big map (UX-GRAPH-5):
    /// the newest night of every thing in `scope` that ran at or after
    /// `since`, each as its own page's Undo does. Returns how many things'
    /// nights were taken back and the memories they wrote, now forgotten.
    pub async fn undo_nights_since(
        &self,
        scope: &ReadScope,
        since: i64,
    ) -> Result<(usize, Vec<String>), WenlanError> {
        let mut things = 0;
        let mut memories = Vec::new();
        for thing in self.list_things(scope, None).await? {
            if thing.dreamed_at.is_none_or(|at| at < since) {
                continue;
            }
            let nights = self.dream_nights(&thing.id).await?;
            let newest = nights
                .iter()
                .rev()
                .find(|(_, undone, night)| !undone && night.entry.is_night());
            if newest.is_none_or(|(_, _, night)| night.at < since) {
                continue;
            }
            let wrote = self.undo_thing_dream(&thing.id).await?;
            things += 1;
            memories.extend(wrote);
        }
        Ok((things, memories))
    }

    /// Record a night's reading of a thing: the memories it wrote, settled
    /// and open, what that changed on its map, the topics it connected them
    /// to, and that its conversations were read up to their last line. A
    /// night that found nothing new records only the reading. Returns the
    /// night's sequence number when it wrote points.
    pub async fn record_dream(
        &self,
        thing_id: &str,
        decided: &[String],
        open: &[String],
        map: &DreamMapChanges,
        links: &[NightLink],
        conversations: &[DueConversation],
    ) -> Result<Option<i64>, WenlanError> {
        self.record_dream_tidied(thing_id, decided, open, map, links, conversations, &[])
            .await
    }

    /// [`Self::record_dream`] with the memories the night tidied
    /// (UX-GRAPH-7): what they take the place of is hidden in the same
    /// transaction as the record, so Undo always finds it. A night that only
    /// tidied is recorded too.
    #[allow(clippy::too_many_arguments)]
    pub async fn record_dream_tidied(
        &self,
        thing_id: &str,
        decided: &[String],
        open: &[String],
        map: &DreamMapChanges,
        links: &[NightLink],
        conversations: &[DueConversation],
        tidied: &[TidyApplied],
    ) -> Result<Option<i64>, WenlanError> {
        self.record_dream_full(
            thing_id,
            decided,
            open,
            &[],
            map,
            links,
            conversations,
            tidied,
        )
        .await
    }

    /// [`Self::record_dream_tidied`] with the preferences, tool problems and
    /// lessons the night kept (`working`): recorded so Undo forgets them with
    /// the night. They are not points: nothing lists them as settled or open.
    /// A night that only kept those is recorded too.
    #[allow(clippy::too_many_arguments)]
    pub async fn record_dream_full(
        &self,
        thing_id: &str,
        decided: &[String],
        open: &[String],
        working: &[String],
        map: &DreamMapChanges,
        links: &[NightLink],
        conversations: &[DueConversation],
        tidied: &[TidyApplied],
    ) -> Result<Option<i64>, WenlanError> {
        let mut changes: Vec<NightChange> = decided
            .iter()
            .chain(open)
            .map(|memory| NightChange::New {
                memory: memory.clone(),
            })
            .chain(links.iter().map(NightChange::from))
            .collect();
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(libsql::TransactionBehavior::Immediate)
            .await
            .map_err(db_err("record dream begin"))?;
        let mut seq = None;
        let applied = apply_tidies(&tx, tidied).await?;
        changes.extend(applied.changes);
        // What a tidy wrote but could not record stands for nothing.
        for id in &applied.orphans {
            self.delete_by_source_id_in_transaction(&tx, "memory", id, false)
                .await?;
        }
        // A tidy of preferences, tool problems or lessons is working too.
        let mut tidies = false;
        let mut working_tidies = false;
        for change in &changes {
            if let NightChange::Tidy { new, .. } = change {
                if working_memory_in(&tx, new).await? {
                    working_tidies = true;
                } else {
                    tidies = true;
                }
            }
        }
        let working_only = decided.is_empty()
            && open.is_empty()
            && !tidies
            && (!working.is_empty() || working_tidies);
        if !decided.is_empty()
            || !open.is_empty()
            || !working.is_empty()
            || tidies
            || working_tidies
        {
            let mut data = serde_json::json!({ "decided": decided, "open": open, "working": working, "map": map, "changes": changes });
            if working_only {
                data["working_only"] = serde_json::Value::Bool(true);
            }
            history(
                &tx,
                now,
                ThingActor::Ai,
                "thing.dream",
                thing_id,
                None,
                data,
            )
            .await?;
            // A night of the thing has a sequence number (for its
            // conclusion); a record of working memories alone is not one.
            if !working_only {
                seq = Some(tx.last_insert_rowid());
            }
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
        Ok(seq)
    }

    /// Attach a point written overnight to the conversation it came from, so
    /// the thing's page lists it, without counting as activity in that
    /// conversation: a night's reading must not keep a thing on home.
    ///
    /// A point is Wenlan's own writing: it is marked so (`source_agent`
    /// `wenlan`) and not put in the review list, since the person sees it on
    /// the thing and is asked about a conflict there (UX-CHAT-5). `line` is
    /// what the person typed that it came from, when the model named one.
    pub async fn attach_dream_point(
        &self,
        source_id: &str,
        session_id: &str,
        line: Option<&str>,
    ) -> Result<(), WenlanError> {
        let conn = self.conn.lock().await;
        conn.execute(
            "UPDATE memories SET session_id = ?1, confirmed = 1,
                    source_agent = COALESCE(source_agent, 'wenlan'), said_line = ?3
              WHERE source = 'memory' AND source_id = ?2 AND session_id IS NULL",
            libsql::params![session_id, source_id, line],
        )
        .await
        .map_err(db_err("attach dream point"))?;
        Ok(())
    }

    /// [`Self::attach_dream_point`] for a preference, tool problem or lesson:
    /// the same source and Wenlan's own mark, but not confirmed. The person
    /// does not see it on a thing's page, so it waits for their OK like any
    /// other memory an AI extracted.
    pub async fn attach_dream_working(
        &self,
        source_id: &str,
        session_id: &str,
        line: Option<&str>,
    ) -> Result<(), WenlanError> {
        let conn = self.conn.lock().await;
        conn.execute(
            "UPDATE memories SET session_id = ?1,
                    source_agent = COALESCE(source_agent, 'wenlan'), said_line = ?3
              WHERE source = 'memory' AND source_id = ?2 AND session_id IS NULL",
            libsql::params![session_id, source_id, line],
        )
        .await
        .map_err(db_err("attach dream working"))?;
        Ok(())
    }

    /// The memories that stand (not forgotten, replaced or only proposed)
    /// of the same kind whose text is `content` up to case and spacing, as
    /// `(source_id, space)`. Equal, not alike: "Ship on Friday" is not a copy
    /// of "Ship on Friday after QA signs off".
    pub async fn live_copies_of(
        &self,
        content: &str,
        memory_type: &str,
    ) -> Result<Vec<(String, Option<String>)>, WenlanError> {
        let wanted = same_words(content);
        // Cheap to narrow, whatever the spacing: the first word, and a length
        // no shorter than the words alone (spacing only ever adds to it).
        let first = wanted.split(' ').next().unwrap_or_default();
        let pattern = format!(
            "{}%",
            first
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );
        let shortest = wanted.chars().count() as i64;
        let longest = shortest * 2 + 16;
        let sql = format!(
            "SELECT m.source_id, m.content, m.space FROM memories m
              WHERE m.source = 'memory' AND m.chunk_index = 0
                AND m.memory_type = ?2 AND ltrim(m.content) LIKE ?1 ESCAPE '\\'
                AND length(m.content) BETWEEN ?3 AND ?4
                AND COALESCE(m.pending_revision, 0) = 0
                AND {} AND {}
              ORDER BY m.created_at",
            crate::db::not_self_archived("m"),
            crate::db::not_hidden_by_superseder(&crate::read_scope::ReadScope::Global, "m"),
        );
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                &sql,
                libsql::params![pattern, memory_type, shortest, longest],
            )
            .await
            .map_err(db_err("live copies"))?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.map_err(db_err("live copies row"))? {
            if same_words(&row.get::<String>(1).unwrap_or_default()) == wanted {
                out.push((
                    row.get::<String>(0).unwrap_or_default(),
                    crate::space_context::normalize_unfiled_space(
                        row.get::<Option<String>>(2).unwrap_or(None),
                    ),
                ));
            }
        }
        Ok(out)
    }

    /// Link a memory a night found it had already kept to the conversation
    /// it came up in, only when it belongs to none and is nobody's own to
    /// keep: not pinned, not confirmed by the person (a hand-saved one is).
    /// Nothing else about it changes, and it is not recorded as written by
    /// the night.
    pub async fn link_existing_memory(
        &self,
        source_id: &str,
        session_id: &str,
    ) -> Result<(), WenlanError> {
        let conn = self.conn.lock().await;
        conn.execute(
            "UPDATE memories SET session_id = ?1
              WHERE source = 'memory' AND source_id = ?2 AND session_id IS NULL
                AND COALESCE(pinned, 0) = 0 AND COALESCE(confirmed, 0) = 0
                AND COALESCE(pending_revision, 0) = 0",
            libsql::params![session_id, source_id],
        )
        .await
        .map_err(db_err("link existing memory"))?;
        Ok(())
    }

    /// A night came to read these conversations and their transcripts were
    /// gone. They are not marked read: they are left out of the nights until
    /// they go on again, and the thing's page says one could not be read.
    pub async fn mark_dream_unread(
        &self,
        conversations: &[DueConversation],
    ) -> Result<(), WenlanError> {
        if conversations.is_empty() {
            return Ok(());
        }
        let conn = self.conn.lock().await;
        for conversation in conversations {
            conn.execute(
                "UPDATE conversation_labels SET unread_at = ?1 WHERE session_id = ?2",
                libsql::params![conversation.last_at, conversation.session_id.as_str()],
            )
            .await
            .map_err(db_err("dream unread"))?;
        }
        Ok(())
    }

    /// The AI tools of a thing's conversations a night cannot read, one
    /// entry per conversation, limited to the caller's Space scope:
    /// - labelled work conversations that went on since a night last read
    ///   them and whose transcript was gone;
    /// - conversations from a tool Wenlan has no transcript reader for. They
    ///   never get a `conversation_labels` row (labels come only from a
    ///   scanned transcript), only an `agent_sessions` row from the tool's
    ///   own MCP calls. Only those active since a night last read the thing
    ///   (its latest night with points, or the last line of its newest read
    ///   conversation) count (all of them when no night has), so an old one stops being
    ///   said once the thing has been read since.
    pub async fn thing_unread_tools(
        &self,
        thing_id: &str,
        scope: &ReadScope,
    ) -> Result<Vec<String>, WenlanError> {
        let readers = wenlan_types::session_context::TRANSCRIPT_TOOLS
            .iter()
            .map(|tool| format!("'{tool}'"))
            .collect::<Vec<_>>()
            .join(", ");
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                &format!(
                    "SELECT cl.tool, cl.space FROM conversation_labels cl
                       JOIN thing_sessions ts ON ts.session_id = cl.session_id
                      WHERE ts.thing_id = ?1 AND cl.work = 1
                        AND cl.unread_at IS NOT NULL AND cl.unread_at >= cl.last_at
                        AND (cl.dreamed_at IS NULL OR cl.dreamed_at < cl.last_at)
                     UNION ALL
                     SELECT s.tool, COALESCE(s.space, '') FROM agent_sessions s
                       JOIN thing_sessions ts ON ts.session_id = s.id
                      WHERE ts.thing_id = ?1 AND s.tool NOT IN ({readers})
                        AND NOT EXISTS (SELECT 1 FROM conversation_labels l
                                         WHERE l.session_id = s.id)
                        AND s.last_write_at > MAX(
                              COALESCE((SELECT MAX(h.ts) FROM thing_history h
                                WHERE h.thing_id = ?1 AND h.op = 'thing.dream'), 0),
                              COALESCE((SELECT MAX(l2.dreamed_at) FROM conversation_labels l2
                                JOIN thing_sessions t2 ON t2.session_id = l2.session_id
                                WHERE t2.thing_id = ?1), 0))"
                ),
                libsql::params![thing_id],
            )
            .await
            .map_err(db_err("thing unread tools"))?;
        let mut tools = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(db_err("thing unread tools row"))?
        {
            let space = row.get::<String>(1).unwrap_or_default();
            if scope.matches(Some(space.as_str()).filter(|s| !s.is_empty())) {
                tools.push(row.get::<String>(0).unwrap_or_default());
            }
        }
        Ok(tools)
    }

    /// How many of a thing's conversations are due for reading (last line at
    /// or before `quiet_since`, not read, not gone) in the caller's Space
    /// scope. The caller passes the cutoff of a night that had no AI to read
    /// with; the daemon keeps that in memory (`things_intake`).
    pub async fn thing_waiting_conversations(
        &self,
        thing_id: &str,
        quiet_since: i64,
        scope: &ReadScope,
    ) -> Result<usize, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT cl.space FROM conversation_labels cl
                   JOIN thing_sessions ts ON ts.session_id = cl.session_id
                   JOIN things t ON t.id = ts.thing_id
                  WHERE ts.thing_id = ?1 AND cl.work = 1
                    AND t.merged_into IS NULL AND t.title IS NOT NULL
                    AND (cl.dreamed_at IS NULL OR cl.last_at > cl.dreamed_at)
                    AND (cl.unread_at IS NULL OR cl.last_at > cl.unread_at)
                    AND cl.last_at <= ?2",
                libsql::params![thing_id, quiet_since],
            )
            .await
            .map_err(db_err("thing waiting"))?;
        let mut count = 0;
        while let Some(row) = rows.next().await.map_err(db_err("thing waiting row"))? {
            let space = row.get::<String>(0).unwrap_or_default();
            if scope.matches(Some(space.as_str()).filter(|s| !s.is_empty())) {
                count += 1;
            }
        }
        Ok(count)
    }

    /// The Space a night's point goes to when its conversation's label named
    /// none (trust audit F4). The conversation's own Space, when it ran in a
    /// repository (`in_repo`: outside one, the tool's default Space says
    /// nothing about what was talked about); else the one Space the thing's
    /// other conversations are all in. `None` leaves it to the default.
    /// Only a registered Space other than Uncategorized is returned.
    pub async fn dream_point_space(
        &self,
        thing_id: &str,
        session_id: &str,
        in_repo: bool,
    ) -> Result<Option<String>, WenlanError> {
        let conn = self.conn.lock().await;
        let registered = "SELECT sp.name FROM spaces sp
                           WHERE sp.name = a.space AND sp.id <> '00000000-0000-4000-8000-000000000001'";
        if in_repo {
            let mut rows = conn
                .query(
                    &format!("SELECT ({registered}) FROM agent_sessions a WHERE a.id = ?1"),
                    libsql::params![session_id],
                )
                .await
                .map_err(db_err("point space session"))?;
            if let Some(row) = rows
                .next()
                .await
                .map_err(db_err("point space session row"))?
            {
                if let Some(space) = row.get::<Option<String>>(0).unwrap_or(None) {
                    return Ok(Some(space));
                }
            }
        }
        let mut rows = conn
            .query(
                &format!(
                    "SELECT DISTINCT ({registered}) FROM thing_sessions ts
                       JOIN agent_sessions a ON a.id = ts.session_id
                      WHERE ts.thing_id = ?1 AND ts.session_id <> ?2"
                ),
                libsql::params![thing_id, session_id],
            )
            .await
            .map_err(db_err("point space thing"))?;
        let mut spaces = Vec::new();
        while let Some(row) = rows.next().await.map_err(db_err("point space thing row"))? {
            spaces.push(row.get::<Option<String>>(0).unwrap_or(None));
        }
        Ok(match spaces.as_slice() {
            [Some(space)] => Some(space.clone()),
            _ => None,
        })
    }

    /// The person takes back the newest night of a thing that was not taken
    /// back yet, what it changed on the thing's map, and the topics it
    /// connected its points to, and forgets the memories it wrote, all in one
    /// transaction so an Undo is never left half done (trust audit F14).
    /// Returns those memories; empty when there is nothing to take back. Its conversations are not read again for the
    /// same lines.
    pub async fn undo_thing_dream(&self, thing_id: &str) -> Result<Vec<String>, WenlanError> {
        let nights = self.dream_nights(thing_id).await?;
        let Some((seq, _, night)) = nights
            .into_iter()
            .rev()
            .find(|(_, undone, night)| !undone && night.entry.is_night())
        else {
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
        remove_night_links(&tx, &night.entry.changes).await?;
        // The tidies came last, so they go first.
        let tidied = undo_tidies(&tx, thing_id, &night.entry.changes).await?;
        super::thing_map::undo_dream_map_in(&tx, thing_id, &night.entry.map, now).await?;
        let mut ids = night.entry.decided;
        ids.extend(night.entry.open);
        ids.extend(night.entry.working);
        ids.extend(tidied);
        for id in &ids {
            self.delete_by_source_id_in_transaction(&tx, "memory", id, false)
                .await?;
        }
        tx.commit().await.map_err(db_err("undo dream commit"))?;
        Ok(ids)
    }
}

/// Text compared up to case and spacing.
fn same_words(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
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

/// A night's points and links, kept to the points in `standing`.
fn last_night_of(
    night: &DreamEntryWithAt,
    standing: &[ThingNightPoint],
    kept: &HashSet<String>,
) -> Option<ThingLastNight> {
    let here: HashSet<&str> = standing.iter().map(|p| p.memory_id.as_str()).collect();
    let entry = &night.entry;
    let written: Vec<String> = entry
        .decided
        .iter()
        .chain(&entry.open)
        .filter(|id| here.contains(id.as_str()))
        .cloned()
        .collect();
    let linked: Vec<NightLinkMade> = entry
        .changes
        .iter()
        .filter_map(|change| match change {
            NightChange::Link { memory, entity } if here.contains(memory.as_str()) => {
                Some(NightLinkMade {
                    memory_id: memory.clone(),
                    entity_id: entity.clone(),
                })
            }
            _ => None,
        })
        .collect();
    let tidied: Vec<NightTidied> = entry
        .changes
        .iter()
        .filter_map(|change| match change {
            NightChange::Tidy { how, old, new, .. }
                if here.contains(new.as_str()) || kept.contains(new) =>
            {
                // In id order, whatever order the model named them in.
                let mut replaced = old.clone();
                replaced.sort();
                Some(NightTidied {
                    kind: *how,
                    memory_id: new.clone(),
                    replaced,
                    said: Vec::new(),
                })
            }
            _ => None,
        })
        .collect();
    // A tidy's new point is counted once, as tidied, not also as written.
    let written: Vec<String> = written
        .into_iter()
        .filter(|id| !tidied.iter().any(|t| &t.memory_id == id))
        .collect();
    if written.is_empty() && linked.is_empty() && tidied.is_empty() {
        return None;
    }
    Some(ThingLastNight {
        at: night.at,
        written,
        linked,
        tidied,
    })
}

/// Whether `id` is a preference, tool problem or lesson.
async fn working_memory_in(tx: &libsql::Transaction, id: &str) -> Result<bool, WenlanError> {
    let mut rows = tx
        .query(
            "SELECT memory_type FROM memories
              WHERE source = 'memory' AND source_id = ?1 AND chunk_index = 0",
            libsql::params![id],
        )
        .await
        .map_err(db_err("working memory"))?;
    Ok(rows
        .next()
        .await
        .map_err(db_err("working memory row"))?
        .and_then(|row| row.get::<Option<String>>(0).unwrap_or(None))
        .is_some_and(|kind| WorkingKind::from_memory_type(&kind).is_some()))
}

/// One night's entry and when it ran.
#[derive(Debug, Default)]
struct DreamEntryWithAt {
    at: i64,
    entry: DreamEntry,
}
