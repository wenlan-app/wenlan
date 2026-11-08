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
use super::thing_map::DreamMapChanges;
use super::{not_hidden_by_superseder, not_self_archived, MemoryDB};
use crate::authorship::{memory_author, MemoryWriter};
use crate::night_tidy::{date_passed, TidyItem, MAX_NEIGHBOURS, MAX_TOUCHED, MIN_SIMILARITY};
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
#[derive(Debug, Clone, PartialEq)]
pub struct TidyApplied {
    pub how: NightTidyKind,
    pub old: Vec<String>,
    pub new: String,
    pub wrote: bool,
    /// What the night wrote for `new`, for the thing's map.
    pub text: Option<String>,
    /// What it changed on the thing's map, for undo.
    pub map: DreamMapChanges,
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
                             OR (m.event_date + 86400 >= ?2 AND m.event_date + 86400 <= ?3))
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
            let passed = item.event_date.is_some_and(|at| date_passed(at, now));
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

/// Most links of a supersede chain followed when hiding a tidied row.
const MAX_CHAIN: usize = 16;

/// What [`apply_tidies`] did: the changes to record, and what a tidy wrote
/// that it could not record (everything it stood for had changed since the
/// offer), for the caller to forget in the same transaction.
#[derive(Debug, Default)]
pub(super) struct AppliedTidies {
    pub changes: Vec<NightChange>,
    pub orphans: Vec<String>,
}

/// The mode of a row still fit to hide at the moment the night records it:
/// live, not pinned, not a person's own writing. `None` when it is gone or
/// changed since the offer.
async fn hideable_mode(
    tx: &libsql::Transaction,
    id: &str,
) -> Result<Option<Option<String>>, WenlanError> {
    let mut rows = tx
        .query(
            "SELECT supersede_mode FROM memories
              WHERE source = 'memory' AND source_id = ?1 AND chunk_index = 0
                AND COALESCE(supersede_mode, '') <> 'evicted'
                AND COALESCE(pinned, 0) = 0 AND authored_content IS NULL",
            libsql::params![id],
        )
        .await
        .map_err(db_err("tidy old mode"))?;
    Ok(rows
        .next()
        .await
        .map_err(db_err("tidy old mode row"))?
        .map(|row| row.get::<Option<String>>(0).unwrap_or(None)))
}

/// Whether `id` stands: a live memory not hidden by a tidy.
async fn stands(tx: &libsql::Transaction, id: &str) -> Result<bool, WenlanError> {
    let mut rows = tx
        .query(
            "SELECT 1 FROM memories
              WHERE source = 'memory' AND source_id = ?1 AND chunk_index = 0
                AND COALESCE(supersede_mode, '') <> 'evicted'",
            libsql::params![id],
        )
        .await
        .map_err(db_err("tidy stands"))?;
    Ok(rows
        .next()
        .await
        .map_err(db_err("tidy stands row"))?
        .is_some())
}

/// The row `id` hides as its superseder (`supersedes` with mode `'hide'`)
/// when nothing else live hides it too, with its own mode.
async fn hidden_only_by(
    tx: &libsql::Transaction,
    id: &str,
) -> Result<Option<(String, Option<String>)>, WenlanError> {
    let mut rows = tx
        .query(
            "SELECT p.source_id, p.supersede_mode FROM memories s
               JOIN memories p ON p.source = 'memory' AND p.source_id = s.supersedes
                              AND p.chunk_index = 0
              WHERE s.source = 'memory' AND s.source_id = ?1 AND s.chunk_index = 0
                AND s.supersede_mode = 'hide'
                AND COALESCE(p.supersede_mode, '') <> 'evicted'
                AND NOT EXISTS (
                    SELECT 1 FROM memories o
                     WHERE o.source = 'memory' AND o.supersedes = p.source_id
                       AND o.source_id <> ?1 AND o.pending_revision = 0
                       AND o.supersede_mode = 'hide')",
            libsql::params![id],
        )
        .await
        .map_err(db_err("tidy chain"))?;
    Ok(rows
        .next()
        .await
        .map_err(db_err("tidy chain row"))?
        .map(|row| {
            (
                row.get::<String>(0).unwrap_or_default(),
                row.get::<Option<String>>(1).unwrap_or(None),
            )
        }))
}

/// Hide what each tidy takes the place of, inside the night's transaction,
/// and return the changes to record. Each row is checked again here, since
/// the model's call took time: one that is gone, already hidden, pinned or
/// now the person's own is left as it is and not recorded, so Undo never
/// puts back a mode some other step set. A replace whose kept memory no
/// longer stands is dropped.
///
/// Hiding a row stops it hiding what it superseded (`supersede_mode` is
/// also the row's behaviour as a superseder), so those rows are hidden too
/// and recorded under `also`.
pub(super) async fn apply_tidies(
    tx: &libsql::Transaction,
    tidied: &[TidyApplied],
) -> Result<AppliedTidies, WenlanError> {
    let mut applied = AppliedTidies::default();
    for tidy in tidied {
        if !tidy.wrote && !stands(tx, &tidy.new).await? {
            continue;
        }
        let (mut old, mut before) = (Vec::new(), Vec::new());
        let (mut also, mut also_before) = (Vec::new(), Vec::new());
        for id in &tidy.old {
            let Some(mode) = hideable_mode(tx, id).await? else {
                continue;
            };
            let mut chain = id.clone();
            for _ in 0..MAX_CHAIN {
                let Some((prior, prior_mode)) = hidden_only_by(tx, &chain).await? else {
                    break;
                };
                if prior == tidy.new || also.contains(&prior) || tidy.old.contains(&prior) {
                    break;
                }
                also.push(prior.clone());
                also_before.push(prior_mode);
                chain = prior;
            }
            old.push(id.clone());
            before.push(mode);
        }
        if old.is_empty() {
            if tidy.wrote {
                applied.orphans.push(tidy.new.clone());
            }
            continue;
        }
        for id in old.iter().chain(also.iter()) {
            tx.execute(
                "UPDATE memories SET supersede_mode = 'evicted'
                  WHERE source = 'memory' AND source_id = ?1",
                libsql::params![id.as_str()],
            )
            .await
            .map_err(db_err("tidy hide"))?;
        }
        applied.changes.push(NightChange::Tidy {
            how: tidy.how,
            old,
            before,
            new: tidy.new.clone(),
            wrote: tidy.wrote,
            map: tidy.map.clone(),
            also,
            also_before,
        });
    }
    Ok(applied)
}

/// Put back what a night's tidies hid, inside its Undo's transaction, and
/// return the memories the night wrote for them, to forget. A row some
/// later step changed again is left alone.
pub(super) async fn undo_tidies(
    tx: &libsql::Transaction,
    thing_id: &str,
    changes: &[NightChange],
) -> Result<Vec<String>, WenlanError> {
    let mut wrote = Vec::new();
    let now = chrono::Utc::now().timestamp();
    // Newest first: a later tidy's map change sits on an earlier one's.
    for change in changes.iter().rev() {
        let NightChange::Tidy {
            old,
            before,
            new,
            wrote: written,
            map,
            also,
            also_before,
            ..
        } = change
        else {
            continue;
        };
        super::thing_map::undo_dream_map_in(tx, thing_id, map, now).await?;
        let put_back = old
            .iter()
            .enumerate()
            .map(|(i, id)| (id, before.get(i).cloned().flatten()))
            .chain(
                also.iter()
                    .enumerate()
                    .map(|(i, id)| (id, also_before.get(i).cloned().flatten())),
            );
        for (id, mode) in put_back {
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
