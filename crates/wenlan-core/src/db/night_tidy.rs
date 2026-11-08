// SPDX-License-Identifier: Apache-2.0
//! The memories a thing's night may tidy, and what tidying does to them
//! (docs/ux/spec.md, UX-GRAPH-7). See [`crate::night_tidy`] for the prompt
//! and the checks on the model's answer.
//!
//! A night looks at the AI-written memories of the thing's conversations
//! written since its last night, and those whose planned date passed since
//! then, each with its nearest AI-written neighbours in the same Space.
//! What a change takes the place of is hidden (`supersede_mode =
//! 'evicted'`, which every reader already skips) and kept; the night's
//! entry records the mode it had, so Undo puts it back.

use super::night_links::NightChange;
use super::{not_hidden_by_superseder, not_self_archived, MemoryDB};
use crate::authorship::{memory_author, MemoryWriter};
use crate::night_tidy::{TidyItem, MAX_NEIGHBOURS, MAX_TOUCHED, MIN_SIMILARITY};
use crate::read_scope::ReadScope;
use crate::WenlanError;
use std::collections::HashMap;
use wenlan_types::authorship::SentenceAuthor;
use wenlan_types::NightTidyKind;

fn db_err(context: &str) -> impl Fn(libsql::Error) -> WenlanError + '_ {
    move |e| WenlanError::VectorDb(format!("{context}: {e}"))
}

/// How far back a thing's first night looks.
const FIRST_NIGHT_SECS: i64 = 24 * 60 * 60;

/// A tidy the night applied, before it is recorded: what is hidden and what
/// stands for it. See [`NightChange::Tidy`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TidyApplied {
    pub how: NightTidyKind,
    pub old: Vec<String>,
    pub new: String,
    pub wrote: bool,
}

/// What a night may tidy for one thing: the memories offered, and the
/// groups the prompt shows (indices into them).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TidyOffer {
    pub items: Vec<TidyItem>,
    pub groups: Vec<Vec<usize>>,
}

/// A row is live, a whole memory, not waiting for review, not pinned and
/// not a recap.
fn tidy_live(alias: &str) -> String {
    format!(
        "{alias}.source = 'memory' AND {alias}.chunk_index = 0 AND {alias}.pending_revision = 0 \
         AND COALESCE({alias}.pinned, 0) = 0 AND COALESCE({alias}.is_recap, 0) = 0 \
         AND {} AND {}",
        not_self_archived(alias),
        not_hidden_by_superseder(&ReadScope::Global, alias)
    )
}

/// The columns [`row_item`] reads, from `alias`.
fn item_columns(alias: &str) -> String {
    format!(
        "{alias}.source_id, {alias}.content, {alias}.space, \
         COALESCE({alias}.created_at, {alias}.last_modified), {alias}.event_date, \
         {alias}.source_agent, {alias}.origin_class, {alias}.authored_content IS NOT NULL, \
         {alias}.memory_type, {alias}.session_id, {alias}.said_line"
    )
}

/// The item in `row`, when an AI wrote it.
fn row_item(row: &libsql::Row) -> Option<TidyItem> {
    let id = row.get::<String>(0).ok()?;
    let agent = row.get::<Option<String>>(5).unwrap_or(None);
    let origin = row.get::<Option<String>>(6).unwrap_or(None);
    let author = memory_author(MemoryWriter {
        source_id: &id,
        source_agent: agent.as_deref(),
        origin_class: origin.as_deref(),
        authored: row.get::<i64>(7).unwrap_or(0) != 0,
    });
    if author != SentenceAuthor::Ai {
        return None;
    }
    let text = row.get::<String>(1).unwrap_or_default();
    if text.trim().is_empty() {
        return None;
    }
    Some(TidyItem {
        id,
        text,
        space: crate::space_context::normalize_unfiled_space(
            row.get::<Option<String>>(2).unwrap_or(None),
        ),
        created_at: row.get::<i64>(3).unwrap_or(0),
        event_date: row.get::<Option<i64>>(4).unwrap_or(None),
        decided: row.get::<Option<String>>(8).unwrap_or(None).as_deref() == Some("decision"),
        session_id: row.get::<Option<String>>(9).unwrap_or(None),
        line: row.get::<Option<String>>(10).unwrap_or(None),
    })
}

impl MemoryDB {
    /// What a merge or update wrote keeps the source of what it stands for:
    /// its conversation and line. It is Wenlan's own writing and stays off
    /// the review list, like a night's point.
    pub async fn adopt_tidy_memory(
        &self,
        source_id: &str,
        session_id: Option<&str>,
        line: Option<&str>,
    ) -> Result<(), WenlanError> {
        let conn = self.conn.lock().await;
        conn.execute(
            "UPDATE memories SET session_id = COALESCE(session_id, ?2), confirmed = 1,
                    source_agent = COALESCE(source_agent, 'wenlan'),
                    said_line = COALESCE(said_line, ?3)
              WHERE source = 'memory' AND source_id = ?1",
            libsql::params![source_id, session_id, line],
        )
        .await
        .map_err(db_err("tidy adopt"))?;
        Ok(())
    }

    /// What a thing's night may tidy at `now` (see the module docs). Empty
    /// when nothing has a neighbour or a passed date.
    pub async fn tidy_offer(&self, thing_id: &str, now: i64) -> Result<TidyOffer, WenlanError> {
        self.tidy_offer_since(thing_id, None, now).await
    }

    /// [`Self::tidy_offer`] looking back to `since` instead of the thing's
    /// last night: the dry run on a database copy asks what a week of
    /// nights would have offered.
    pub async fn tidy_offer_since(
        &self,
        thing_id: &str,
        since: Option<i64>,
        now: i64,
    ) -> Result<TidyOffer, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT MAX(ts) FROM thing_history WHERE thing_id = ?1 AND op = 'thing.dream'",
                libsql::params![thing_id],
            )
            .await
            .map_err(db_err("tidy last night"))?;
        let since_last = match rows.next().await.map_err(db_err("tidy last night row"))? {
            Some(row) => row.get::<Option<i64>>(0).unwrap_or(None),
            None => None,
        }
        .unwrap_or(now - FIRST_NIGHT_SECS);
        drop(rows);
        let since = since.unwrap_or(since_last);
        let mut rows = conn
            .query(
                &format!(
                    "SELECT {} FROM memories m
                      WHERE {} AND m.session_id IN
                            (SELECT session_id FROM thing_sessions WHERE thing_id = ?1)
                        AND (COALESCE(m.created_at, m.last_modified) >= ?2
                             OR (m.event_date >= ?2 AND m.event_date < ?3))
                      ORDER BY COALESCE(m.created_at, m.last_modified) DESC
                      LIMIT ?4",
                    item_columns("m"),
                    tidy_live("m")
                ),
                libsql::params![thing_id, since, now, (MAX_TOUCHED * 4) as i64],
            )
            .await
            .map_err(db_err("tidy touched"))?;
        let mut touched = Vec::new();
        while let Some(row) = rows.next().await.map_err(db_err("tidy touched row"))? {
            if let Some(item) = row_item(&row) {
                touched.push(item);
                if touched.len() == MAX_TOUCHED {
                    break;
                }
            }
        }
        drop(rows);

        let mut offer = TidyOffer::default();
        let mut at: HashMap<String, usize> = HashMap::new();
        let mut place = |offer: &mut TidyOffer, item: TidyItem| -> usize {
            *at.entry(item.id.clone()).or_insert_with(|| {
                offer.items.push(item);
                offer.items.len() - 1
            })
        };
        for item in touched {
            let mut rows = conn
                .query(
                    "SELECT vector_extract(embedding) FROM memories
                      WHERE source = 'memory' AND source_id = ?1 AND chunk_index = 0
                        AND embedding IS NOT NULL",
                    libsql::params![item.id.as_str()],
                )
                .await
                .map_err(db_err("tidy embedding"))?;
            let vector = match rows.next().await.map_err(db_err("tidy embedding row"))? {
                Some(row) => row.get::<String>(0).ok(),
                None => None,
            };
            drop(rows);
            let mut neighbours = Vec::new();
            if let Some(vector) = vector {
                let mut rows = conn
                    .query(
                        &format!(
                            "SELECT {}, 1.0 - vector_distance_cos(m.embedding, vector32(?1))
                               FROM vector_top_k('memories_vec_idx', vector32(?1), ?2) AS vt
                               JOIN memories m ON m.rowid = vt.id
                              WHERE {} AND m.source_id <> ?3",
                            item_columns("m"),
                            tidy_live("m")
                        ),
                        libsql::params![
                            vector.as_str(),
                            (MAX_NEIGHBOURS * 4) as i64,
                            item.id.as_str()
                        ],
                    )
                    .await
                    .map_err(db_err("tidy neighbours"))?;
                while let Some(row) = rows.next().await.map_err(db_err("tidy neighbour row"))? {
                    let similarity = row.get::<f64>(11).unwrap_or(0.0) as f32;
                    if similarity < MIN_SIMILARITY {
                        continue;
                    }
                    if let Some(next) = row_item(&row).filter(|n| n.space == item.space) {
                        if !neighbours.iter().any(|n: &TidyItem| n.id == next.id) {
                            neighbours.push(next);
                        }
                    }
                    if neighbours.len() == MAX_NEIGHBOURS {
                        break;
                    }
                }
            }
            let passed = item.event_date.is_some_and(|at| at < now);
            if neighbours.is_empty() && !passed {
                continue;
            }
            let mut group = vec![place(&mut offer, item)];
            for next in neighbours {
                let i = place(&mut offer, next);
                if !group.contains(&i) {
                    group.push(i);
                }
            }
            offer.groups.push(group);
        }
        Ok(offer)
    }
}

/// Hide what each tidy takes the place of, inside the night's transaction,
/// and return the changes to record. A row that is gone or already hidden
/// is left as it is and not recorded, so Undo never puts back a mode some
/// other step set.
pub(super) async fn apply_tidies(
    tx: &libsql::Transaction,
    tidied: &[TidyApplied],
) -> Result<Vec<NightChange>, WenlanError> {
    let mut changes = Vec::new();
    for tidy in tidied {
        let mut old = Vec::new();
        let mut before = Vec::new();
        for id in &tidy.old {
            let mut rows = tx
                .query(
                    "SELECT supersede_mode FROM memories
                      WHERE source = 'memory' AND source_id = ?1 AND chunk_index = 0
                        AND COALESCE(supersede_mode, '') <> 'evicted'",
                    libsql::params![id.as_str()],
                )
                .await
                .map_err(db_err("tidy old mode"))?;
            let Some(row) = rows.next().await.map_err(db_err("tidy old mode row"))? else {
                continue;
            };
            before.push(row.get::<Option<String>>(0).unwrap_or(None));
            drop(rows);
            tx.execute(
                "UPDATE memories SET supersede_mode = 'evicted'
                  WHERE source = 'memory' AND source_id = ?1",
                libsql::params![id.as_str()],
            )
            .await
            .map_err(db_err("tidy hide"))?;
            old.push(id.clone());
        }
        if old.is_empty() {
            continue;
        }
        changes.push(NightChange::Tidy {
            how: tidy.how,
            old,
            before,
            new: tidy.new.clone(),
            wrote: tidy.wrote,
        });
    }
    Ok(changes)
}

/// Put back what a night's tidies hid, inside its Undo's transaction, and
/// return the memories the night wrote for them, to forget. A row some
/// later step changed again is left alone.
pub(super) async fn undo_tidies(
    tx: &libsql::Transaction,
    changes: &[NightChange],
) -> Result<Vec<String>, WenlanError> {
    let mut wrote = Vec::new();
    for change in changes {
        let NightChange::Tidy {
            old,
            before,
            new,
            wrote: written,
            ..
        } = change
        else {
            continue;
        };
        for (i, id) in old.iter().enumerate() {
            let mode = before.get(i).cloned().flatten();
            tx.execute(
                "UPDATE memories SET supersede_mode = COALESCE(?2, 'hide')
                  WHERE source = 'memory' AND source_id = ?1 AND supersede_mode = 'evicted'",
                libsql::params![id.as_str(), mode],
            )
            .await
            .map_err(db_err("tidy put back"))?;
        }
        if *written {
            wrote.push(new.clone());
        }
    }
    Ok(wrote)
}
