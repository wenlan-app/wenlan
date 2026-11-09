// SPDX-License-Identifier: Apache-2.0
//! What the one-time pass over old memories would tidy (docs/ux/spec.md,
//! UX-GRAPH-7; /mnt/project-files/nightly-dream/2026-11-08-design.md §2).
//! The report reads only, so the owner can see it before anything runs;
//! the pass itself (`apply_backlog`) runs once, after the owner's OK, and
//! `undo_backlog` takes all of it back.
//!
//! Four kinds of old work:
//! - near-identical memories in one Space (the later copy would hide behind
//!   the earlier; a person's or unknown writer's memory is never hidden);
//! - memories about a date that has passed (only an AI's are rewritten);
//! - contradiction checks still queued from code that no longer runs;
//! - night points written before nights marked them as Wenlan's own.

use super::night_links::NightChange;
use super::night_tidy::{apply_tidies, undo_tidies, TidyApplied};
use super::{not_hidden_by_superseder, not_self_archived, MemoryDB, UNFILED_SPACE_ID};
use crate::authorship::{memory_author, MemoryWriter};
use crate::read_scope::ReadScope;
use crate::WenlanError;
use std::collections::HashMap;
use wenlan_types::authorship::SentenceAuthor;
use wenlan_types::NightTidyKind;

/// Where the pass keeps what it did while it stands, for its Undo. Unset,
/// the pass may run (again).
const PASS_KEY: &str = "old_memory_pass_v1";

fn db_err(context: &str) -> impl Fn(libsql::Error) -> WenlanError + '_ {
    move |e| WenlanError::VectorDb(format!("{context}: {e}"))
}

/// How alike two memories must be to count as the same, by embedding. The
/// 11-08 measurement on the owner's copy used this value (416 pairs).
pub const BACKLOG_MIN_SIMILARITY: f32 = 0.92;

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
    /// unknown writer's, and an imported original goes only when it is the
    /// same text again (an import made twice). Notes that only look alike,
    /// such as one daily template on different days, stay (11-08 dry run).
    pub fn hideable(&self) -> impl Iterator<Item = &BacklogPair> {
        self.pairs.iter().filter(|pair| {
            may_hide(pair.hide.author) && (pair.exact || pair.hide.author == SentenceAuthor::Ai)
        })
    }
}

/// `vector` scaled to length one, so a dot product is the cosine; empty
/// when it has no length.
fn unit(vector: Vec<f32>) -> Vec<f32> {
    let length = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
    if length <= f32::EPSILON {
        return Vec::new();
    }
    vector.into_iter().map(|x| x / length).collect()
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
    /// only.
    pub async fn backlog_report(&self, now: i64) -> Result<BacklogReport, WenlanError> {
        let conn = self.conn.lock().await;
        let mut report = BacklogReport::default();

        let mut rows = conn
            .query(
                &format!(
                    "SELECT {}, m.embedding FROM memories m WHERE {}",
                    columns("m"),
                    live("m")
                ),
                (),
            )
            .await
            .map_err(db_err("backlog memories"))?;
        let mut all: Vec<(BacklogMemory, Vec<f32>)> = Vec::new();
        while let Some(row) = rows.next().await.map_err(db_err("backlog memory row"))? {
            let Some(memory) = row_memory(&row) else {
                continue;
            };
            if memory
                .event_date
                .is_some_and(|at| crate::night_tidy::plan_passed(at, memory.created_at, now))
            {
                report.passed.push(memory.clone());
            }
            let vector = unit(
                row.get::<Vec<u8>>(8)
                    .unwrap_or_default()
                    .chunks_exact(4)
                    .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                    .collect(),
            );
            all.push((memory, vector));
        }
        drop(rows);
        report.memories = all.len();

        // Every two memories in the same Space, compared in memory: one
        // pass over a few thousand is seconds, where a nearest-neighbour
        // lookup per memory took hours on the owner's copy (11-08 dry run).
        let mut by_space: HashMap<Option<&str>, Vec<usize>> = HashMap::new();
        for (i, (memory, vector)) in all.iter().enumerate() {
            if !vector.is_empty() {
                by_space.entry(memory.space.as_deref()).or_default().push(i);
            }
        }
        for members in by_space.values() {
            for (n, &i) in members.iter().enumerate() {
                for &j in &members[n + 1..] {
                    let (a, va) = &all[i];
                    let (b, vb) = &all[j];
                    if va.len() != vb.len() {
                        continue;
                    }
                    let similarity: f32 = va.iter().zip(vb).map(|(x, y)| x * y).sum();
                    if similarity < BACKLOG_MIN_SIMILARITY {
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

/// What the owner OK'd the pass to do, from the report: the later copies to
/// hide behind the earlier, and the memories about a passed date each with
/// the memory the caller already wrote to stand for it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BacklogPlan {
    /// `(keep, hide)`.
    pub hide: Vec<(String, String)>,
    /// `(old, new)`.
    pub updated: Vec<(String, String)>,
}

/// What the pass did, kept for its Undo and for the map's line.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BacklogPass {
    pub at: i64,
    /// Merges (a copy hidden behind its earlier one) and updates, the same
    /// record a night keeps.
    pub changes: Vec<NightChange>,
    /// Contradiction checks set to expired.
    #[serde(default)]
    pub expired: Vec<String>,
    /// Old night points taken off the review list.
    #[serde(default)]
    pub confirmed: Vec<String>,
    /// Old night points filed from the unfiled Space: `(id, space)`.
    #[serde(default)]
    pub filed: Vec<(String, String)>,
}

impl BacklogPass {
    /// Memories hidden behind an earlier copy.
    pub fn merged(&self) -> usize {
        self.tidies(NightTidyKind::Merge)
    }

    /// Memories about a passed date said again in the past tense.
    pub fn updated(&self) -> usize {
        self.tidies(NightTidyKind::Update)
    }

    fn tidies(&self, kind: NightTidyKind) -> usize {
        self.changes
            .iter()
            .map(|change| match change {
                NightChange::Tidy { how, old, .. } if *how == kind => old.len(),
                _ => 0,
            })
            .sum()
    }
}

/// The earliest copy each hidden one stands behind: `a` hides `b` and `b`
/// hides `c` makes both stand behind `a`. Pairs count in order: one that
/// would hide a row twice, or close a loop, is left out.
fn root_of<'a>(behind: &HashMap<&'a str, &'a str>, mut id: &'a str) -> String {
    let mut steps = 0;
    while let Some(next) = behind.get(id) {
        id = next;
        steps += 1;
        if steps > behind.len() {
            break;
        }
    }
    id.to_string()
}

fn roots(hide: &[(String, String)]) -> Vec<(String, Vec<String>)> {
    let mut behind: HashMap<&str, &str> = HashMap::new();
    let mut order: Vec<&str> = Vec::new();
    for (keep, hidden) in hide {
        if keep == hidden
            || behind.contains_key(hidden.as_str())
            || root_of(&behind, keep) == *hidden
        {
            continue;
        }
        behind.insert(hidden.as_str(), keep.as_str());
        order.push(hidden.as_str());
    }
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for hidden in order {
        let root = root_of(&behind, hidden);
        match out.iter_mut().find(|(keep, _)| *keep == root) {
            Some((_, hides)) => hides.push(hidden.to_string()),
            None => out.push((root, vec![hidden.to_string()])),
        }
    }
    out
}

/// Whether `id` may be hidden now: still an AI's or an import's, and not one
/// of a thing's points (those go only through a night, which keeps its map).
async fn may_hide_now(tx: &libsql::Transaction, id: &str) -> Result<bool, WenlanError> {
    let mut rows = tx
        .query(
            "SELECT m.source_agent, m.origin_class, m.authored_content IS NOT NULL,
                    EXISTS (SELECT 1 FROM thing_map_nodes n, json_each(n.sources) s
                             WHERE json_extract(s.value, '$.memory_id') = m.source_id)
               FROM memories m
              WHERE m.source = 'memory' AND m.source_id = ?1 AND m.chunk_index = 0",
            libsql::params![id],
        )
        .await
        .map_err(db_err("backlog author"))?;
    let Some(row) = rows.next().await.map_err(db_err("backlog author row"))? else {
        return Ok(false);
    };
    let agent = row.get::<Option<String>>(0).unwrap_or(None);
    let origin = row.get::<Option<String>>(1).unwrap_or(None);
    let author = memory_author(MemoryWriter {
        source_id: id,
        source_agent: agent.as_deref(),
        origin_class: origin.as_deref(),
        authored: row.get::<i64>(2).unwrap_or(0) != 0,
    });
    Ok(may_hide(author) && row.get::<i64>(3).unwrap_or(0) == 0)
}

impl MemoryDB {
    /// The pass, if it ran and was not taken back.
    pub async fn backlog_pass(&self) -> Result<Option<BacklogPass>, WenlanError> {
        Ok(self
            .get_app_metadata(PASS_KEY)
            .await?
            .and_then(|value| serde_json::from_str(&value).ok()))
    }

    /// Run the one-time pass the owner OK'd (UX-GRAPH-7): hide each later
    /// copy behind the earliest, stand each update's new memory for its old
    /// one, expire the contradiction checks still pending, take old night
    /// points off the review list and file the unfiled ones where their
    /// thing is. Every row is checked again first; nothing is deleted
    /// except what the caller wrote and nothing now stands for. Refused
    /// while an earlier pass stands.
    pub async fn apply_backlog(&self, plan: &BacklogPlan) -> Result<BacklogPass, WenlanError> {
        if self.backlog_pass().await?.is_some() {
            return Err(WenlanError::Conflict(
                "the old-memory pass already ran; take it back first".into(),
            ));
        }
        let now = chrono::Utc::now().timestamp();
        // Filing goes through the usual Space move, outside the transaction,
        // and is put back if the rest fails.
        let mut filed = Vec::new();
        for (id, thing, session) in self.unfiled_night_points().await? {
            if let Some(space) = self.dream_point_space(&thing, &session, false).await? {
                self.update_memory_space(&id, &space).await?;
                filed.push((id, space));
            }
        }
        let result = self.apply_backlog_rows(plan, now, &filed).await;
        if result.is_err() {
            for (id, _) in &filed {
                let _ = self.update_memory_space_opt(id, None).await;
            }
        }
        result
    }

    async fn apply_backlog_rows(
        &self,
        plan: &BacklogPlan,
        now: i64,
        filed: &[(String, String)],
    ) -> Result<BacklogPass, WenlanError> {
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(libsql::TransactionBehavior::Immediate)
            .await
            .map_err(db_err("backlog begin"))?;
        let mut tidies = Vec::new();
        for (keep, hides) in roots(&plan.hide) {
            let mut old = Vec::new();
            for id in hides {
                if may_hide_now(&tx, &id).await? {
                    old.push(id);
                }
            }
            if !old.is_empty() {
                tidies.push(TidyApplied {
                    how: NightTidyKind::Merge,
                    old,
                    new: keep,
                    wrote: false,
                    text: None,
                    map: Default::default(),
                });
            }
        }
        for (old, new) in &plan.updated {
            let old = if may_hide_now(&tx, old).await? {
                vec![old.clone()]
            } else {
                Vec::new()
            };
            tidies.push(TidyApplied {
                how: NightTidyKind::Update,
                old,
                new: new.clone(),
                wrote: true,
                text: None,
                map: Default::default(),
            });
        }
        let applied = apply_tidies(&tx, &tidies).await?;
        for id in &applied.orphans {
            self.delete_by_source_id_in_transaction(&tx, "memory", id, false)
                .await?;
        }
        let expired = ids(
            &tx,
            "SELECT id FROM refinement_queue
              WHERE action LIKE '%contradiction%' AND COALESCE(status, 'pending') = 'pending'",
        )
        .await?;
        tx.execute(
            "UPDATE refinement_queue
                SET status = 'expired', resolved_at = datetime('now'),
                    created_at = COALESCE(created_at, datetime('now'))
              WHERE action LIKE '%contradiction%' AND COALESCE(status, 'pending') = 'pending'",
            (),
        )
        .await
        .map_err(db_err("backlog expire"))?;
        let confirmed = ids(
            &tx,
            "SELECT m.source_id FROM memories m
              WHERE m.source = 'memory' AND m.chunk_index = 0 AND COALESCE(m.confirmed, 0) = 0
                AND m.source_id IN (SELECT json_extract(s.value, '$.memory_id')
                                      FROM thing_map_nodes n, json_each(n.sources) s)",
        )
        .await?;
        for id in &confirmed {
            tx.execute(
                "UPDATE memories SET confirmed = 1 WHERE source = 'memory' AND source_id = ?1",
                libsql::params![id.as_str()],
            )
            .await
            .map_err(db_err("backlog confirm"))?;
        }
        let pass = BacklogPass {
            at: now,
            changes: applied.changes,
            expired,
            confirmed,
            filed: filed.to_vec(),
        };
        let value = serde_json::to_string(&pass)
            .map_err(|e| WenlanError::VectorDb(format!("backlog record: {e}")))?;
        tx.execute(
            "INSERT INTO app_metadata (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            libsql::params![PASS_KEY, value],
        )
        .await
        .map_err(db_err("backlog record"))?;
        tx.commit().await.map_err(db_err("backlog commit"))?;
        Ok(pass)
    }

    /// Old night points in the unfiled Space: `(id, thing, conversation)`.
    async fn unfiled_night_points(&self) -> Result<Vec<(String, String, String)>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT DISTINCT m.source_id, n.thing_id, COALESCE(json_extract(s.value, '$.session_id'), '')
                   FROM thing_map_nodes n, json_each(n.sources) s
                   JOIN memories m ON m.source = 'memory' AND m.chunk_index = 0
                                  AND m.source_id = json_extract(s.value, '$.memory_id')
                  WHERE COALESCE(m.space, ?1) = ?1 OR m.space = ''",
                libsql::params![UNFILED_SPACE_ID],
            )
            .await
            .map_err(db_err("backlog unfiled"))?;
        let mut out: Vec<(String, String, String)> = Vec::new();
        while let Some(row) = rows.next().await.map_err(db_err("backlog unfiled row"))? {
            let id = row.get::<String>(0).unwrap_or_default();
            if out.iter().any(|(seen, _, _)| *seen == id) {
                continue;
            }
            out.push((
                id,
                row.get::<String>(1).unwrap_or_default(),
                row.get::<String>(2).unwrap_or_default(),
            ));
        }
        Ok(out)
    }

    /// Take the whole pass back: what it hid shows again, what it wrote is
    /// forgotten, the checks it expired wait again, the points it confirmed
    /// go back on the review list and the ones it filed back to unfiled. A
    /// row some later step changed again is left alone. Returns what was
    /// taken back, or `None` when no pass stands.
    pub async fn undo_backlog(&self) -> Result<Option<BacklogPass>, WenlanError> {
        let Some(pass) = self.backlog_pass().await? else {
            return Ok(None);
        };
        {
            let conn = self.conn.lock().await;
            let tx = conn
                .transaction_with_behavior(libsql::TransactionBehavior::Immediate)
                .await
                .map_err(db_err("backlog undo begin"))?;
            for id in undo_tidies(&tx, "", &pass.changes).await? {
                self.delete_by_source_id_in_transaction(&tx, "memory", &id, false)
                    .await?;
            }
            for id in &pass.expired {
                tx.execute(
                    "UPDATE refinement_queue SET status = 'pending', resolved_at = NULL
                      WHERE id = ?1 AND status = 'expired'",
                    libsql::params![id.as_str()],
                )
                .await
                .map_err(db_err("backlog undo expire"))?;
            }
            for id in &pass.confirmed {
                tx.execute(
                    "UPDATE memories SET confirmed = 0 WHERE source = 'memory' AND source_id = ?1",
                    libsql::params![id.as_str()],
                )
                .await
                .map_err(db_err("backlog undo confirm"))?;
            }
            tx.execute(
                "DELETE FROM app_metadata WHERE key = ?1",
                libsql::params![PASS_KEY],
            )
            .await
            .map_err(db_err("backlog undo record"))?;
            tx.commit().await.map_err(db_err("backlog undo commit"))?;
        }
        for (id, space) in &pass.filed {
            if self.get_memory_space(id).await?.as_deref() == Some(space.as_str()) {
                self.update_memory_space_opt(id, None).await?;
            }
        }
        Ok(Some(pass))
    }
}

async fn ids(tx: &libsql::Transaction, sql: &str) -> Result<Vec<String>, WenlanError> {
    let mut rows = tx.query(sql, ()).await.map_err(db_err("backlog ids"))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.map_err(db_err("backlog id row"))? {
        if let Ok(id) = row.get::<String>(0) {
            out.push(id);
        }
    }
    Ok(out)
}

#[cfg(test)]
#[path = "night_backlog_test.rs"]
mod tests;
