// SPDX-License-Identifier: Apache-2.0
//! What the one-time pass over old memories would tidy (docs/ux/spec.md,
//! UX-GRAPH-7; /mnt/project-files/nightly-dream/2026-11-08-design.md §2).
//! Read-only: this only counts and picks samples, so the owner can see the
//! report before anything runs.
//!
//! Four kinds of old work:
//! - near-identical memories in one Space (the later copy would hide behind
//!   the earlier; a person's or unknown writer's memory is never hidden);
//! - memories about a date that has passed (only an AI's are rewritten);
//! - contradiction checks still queued from code that no longer runs;
//! - night points written before nights marked them as Wenlan's own.

use super::{not_hidden_by_superseder, not_self_archived, MemoryDB, UNFILED_SPACE_ID};
use crate::authorship::{memory_author, MemoryWriter};
use crate::read_scope::ReadScope;
use crate::WenlanError;
use std::collections::{HashMap, HashSet};
use wenlan_types::authorship::SentenceAuthor;

fn db_err(context: &str) -> impl Fn(libsql::Error) -> WenlanError + '_ {
    move |e| WenlanError::VectorDb(format!("{context}: {e}"))
}

/// How alike two memories must be to count as the same, by embedding. The
/// 11-08 measurement on the owner's copy used this value (416 pairs).
pub const BACKLOG_MIN_SIMILARITY: f32 = 0.92;
/// Neighbours looked at for each memory.
const BACKLOG_NEIGHBOURS: i64 = 6;

/// One memory as the report shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct BacklogMemory {
    pub id: String,
    pub text: String,
    /// `None` when unfiled.
    pub space: Option<String>,
    pub created_at: i64,
    pub event_date: Option<i64>,
    pub author: SentenceAuthor,
}

/// Two memories that say the same: `hide` is the later copy.
#[derive(Debug, Clone, PartialEq)]
pub struct BacklogPair {
    pub keep: BacklogMemory,
    pub hide: BacklogMemory,
    pub similarity: f32,
    /// The texts are the same once case, spaces and punctuation are set aside.
    pub exact: bool,
}

/// The night points written before nights marked them as Wenlan's own.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NightPointBacklog {
    pub total: usize,
    /// Still on the person's review list.
    pub unconfirmed: usize,
    /// In the unfiled Space.
    pub unfiled: usize,
    /// No tool recorded as their writer.
    pub no_writer: usize,
}

/// What the pass would do, before it does anything.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BacklogReport {
    /// Live memories looked at.
    pub memories: usize,
    pub pairs: Vec<BacklogPair>,
    /// Memories about a date that has passed, of every writer.
    pub passed: Vec<BacklogMemory>,
    /// Contradiction checks by status.
    pub contradictions: Vec<(String, i64)>,
    pub night_points: NightPointBacklog,
}

impl BacklogReport {
    /// The pairs the pass may act on: the later copy is not a person's or an
    /// unknown writer's.
    pub fn hideable(&self) -> impl Iterator<Item = &BacklogPair> {
        self.pairs.iter().filter(|pair| may_hide(pair.hide.author))
    }
}

/// Only what an AI wrote or an import copied in may be hidden.
pub fn may_hide(author: SentenceAuthor) -> bool {
    matches!(author, SentenceAuthor::Ai | SentenceAuthor::Original)
}

/// Text with case, spacing and punctuation set aside, for telling exact
/// copies from paraphrases.
pub fn normalized(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn live(alias: &str) -> String {
    format!(
        "{alias}.source = 'memory' AND {alias}.chunk_index = 0 AND {alias}.pending_revision = 0 \
         AND COALESCE({alias}.pinned, 0) = 0 AND COALESCE({alias}.is_recap, 0) = 0 \
         AND {} AND {}",
        not_self_archived(alias),
        not_hidden_by_superseder(&ReadScope::Global, alias)
    )
}

fn columns(alias: &str) -> String {
    format!(
        "{alias}.source_id, {alias}.content, {alias}.space, \
         COALESCE({alias}.created_at, {alias}.last_modified), {alias}.event_date, \
         {alias}.source_agent, {alias}.origin_class, {alias}.authored_content IS NOT NULL"
    )
}

fn row_memory(row: &libsql::Row) -> Option<BacklogMemory> {
    let id = row.get::<String>(0).ok()?;
    let agent = row.get::<Option<String>>(5).unwrap_or(None);
    let origin = row.get::<Option<String>>(6).unwrap_or(None);
    let author = memory_author(MemoryWriter {
        source_id: &id,
        source_agent: agent.as_deref(),
        origin_class: origin.as_deref(),
        authored: row.get::<i64>(7).unwrap_or(0) != 0,
    });
    Some(BacklogMemory {
        id,
        text: row.get::<String>(1).unwrap_or_default(),
        space: crate::space_context::normalize_unfiled_space(
            row.get::<Option<String>>(2).unwrap_or(None),
        ),
        created_at: row.get::<i64>(3).unwrap_or(0),
        event_date: row.get::<Option<i64>>(4).unwrap_or(None),
        author,
    })
}

impl MemoryDB {
    /// What the one-time pass over old memories would do at `now`. Reads
    /// only. Slow on a large store (one nearest-neighbour lookup per
    /// memory); meant for a dry run, not a request.
    pub async fn backlog_report(&self, now: i64) -> Result<BacklogReport, WenlanError> {
        let conn = self.conn.lock().await;
        let mut report = BacklogReport::default();

        let mut rows = conn
            .query(
                &format!(
                    "SELECT {}, m.embedding IS NOT NULL FROM memories m WHERE {}",
                    columns("m"),
                    live("m")
                ),
                (),
            )
            .await
            .map_err(db_err("backlog memories"))?;
        let mut all: HashMap<String, BacklogMemory> = HashMap::new();
        let mut embedded = Vec::new();
        while let Some(row) = rows.next().await.map_err(db_err("backlog memory row"))? {
            let Some(memory) = row_memory(&row) else {
                continue;
            };
            if memory
                .event_date
                .is_some_and(|at| crate::night_tidy::date_passed(at, now))
            {
                report.passed.push(memory.clone());
            }
            if row.get::<i64>(8).unwrap_or(0) != 0 {
                embedded.push(memory.id.clone());
            }
            all.insert(memory.id.clone(), memory);
        }
        drop(rows);
        report.memories = all.len();

        let mut seen: HashSet<(String, String)> = HashSet::new();
        for id in &embedded {
            let mut rows = conn
                .query(
                    "SELECT vector_extract(embedding) FROM memories
                      WHERE source = 'memory' AND source_id = ?1 AND chunk_index = 0",
                    libsql::params![id.as_str()],
                )
                .await
                .map_err(db_err("backlog embedding"))?;
            let vector = match rows.next().await.map_err(db_err("backlog embedding row"))? {
                Some(row) => row.get::<String>(0).ok(),
                None => None,
            };
            drop(rows);
            let Some(vector) = vector else { continue };
            let mut rows = conn
                .query(
                    "SELECT m.source_id, 1.0 - vector_distance_cos(m.embedding, vector32(?1))
                       FROM vector_top_k('memories_vec_idx', vector32(?1), ?2) AS vt
                       JOIN memories m ON m.rowid = vt.id
                      WHERE m.source = 'memory' AND m.chunk_index = 0 AND m.source_id <> ?3",
                    libsql::params![vector.as_str(), BACKLOG_NEIGHBOURS, id.as_str()],
                )
                .await
                .map_err(db_err("backlog neighbours"))?;
            while let Some(row) = rows.next().await.map_err(db_err("backlog neighbour row"))? {
                let other = row.get::<String>(0).unwrap_or_default();
                let similarity = row.get::<f64>(1).unwrap_or(0.0) as f32;
                if similarity < BACKLOG_MIN_SIMILARITY {
                    continue;
                }
                let (Some(a), Some(b)) = (all.get(id), all.get(&other)) else {
                    continue;
                };
                if a.space != b.space {
                    continue;
                }
                let key = if a.id < b.id {
                    (a.id.clone(), b.id.clone())
                } else {
                    (b.id.clone(), a.id.clone())
                };
                if !seen.insert(key) {
                    continue;
                }
                // The earlier stays; ties go to the smaller id, so the
                // report is the same each run.
                let (keep, hide) = if (a.created_at, &a.id) <= (b.created_at, &b.id) {
                    (a, b)
                } else {
                    (b, a)
                };
                report.pairs.push(BacklogPair {
                    exact: normalized(&keep.text) == normalized(&hide.text),
                    keep: keep.clone(),
                    hide: hide.clone(),
                    similarity,
                });
            }
        }
        report.pairs.sort_by(|a, b| {
            b.similarity
                .partial_cmp(&a.similarity)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.hide.id.cmp(&b.hide.id))
        });
        report
            .passed
            .sort_by_key(|m| std::cmp::Reverse(m.event_date));

        let mut rows = conn
            .query(
                "SELECT COALESCE(status, ''), COUNT(*) FROM refinement_queue
                  WHERE action LIKE '%contradiction%' GROUP BY 1 ORDER BY 2 DESC",
                (),
            )
            .await
            .map_err(db_err("backlog contradictions"))?;
        while let Some(row) = rows
            .next()
            .await
            .map_err(db_err("backlog contradiction row"))?
        {
            report.contradictions.push((
                row.get::<String>(0).unwrap_or_default(),
                row.get::<i64>(1).unwrap_or(0),
            ));
        }
        drop(rows);

        let mut rows = conn
            .query(
                "SELECT COUNT(*),
                        COALESCE(SUM(COALESCE(m.confirmed, 0) = 0), 0),
                        COALESCE(SUM(COALESCE(m.space, ?1) = ?1 OR m.space = ''), 0),
                        COALESCE(SUM(m.source_agent IS NULL OR m.source_agent = ''), 0)
                   FROM memories m
                  WHERE m.source = 'memory' AND m.chunk_index = 0 AND m.source_id IN (
                        SELECT DISTINCT json_extract(s.value, '$.memory_id')
                          FROM thing_map_nodes n, json_each(n.sources) s
                         WHERE json_extract(s.value, '$.memory_id') IS NOT NULL)",
                libsql::params![UNFILED_SPACE_ID],
            )
            .await
            .map_err(db_err("backlog night points"))?;
        if let Some(row) = rows
            .next()
            .await
            .map_err(db_err("backlog night point row"))?
        {
            let count = |i: i32| row.get::<i64>(i).unwrap_or(0).max(0) as usize;
            report.night_points = NightPointBacklog {
                total: count(0),
                unconfirmed: count(1),
                unfiled: count(2),
                no_writer: count(3),
            };
        }
        Ok(report)
    }
}

#[cfg(test)]
#[path = "night_backlog_test.rs"]
mod tests;
