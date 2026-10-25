// SPDX-License-Identifier: Apache-2.0
//! Counts backing `GET /api/activity`: one read-only pass over the memories,
//! enrichment-step, entity-shadow, page and refinement-queue tables.
//!
//! Same shape as the other `impl MemoryDB` reader files (e.g.
//! `scoped_entities.rs`): pure counts, no prose, one held connection guard.

use super::MemoryDB;
use crate::WenlanError;

/// One `(step_name, status)` cell of the `enrichment_steps` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepCount {
    pub step_name: String,
    pub status: String,
    pub count: u64,
}

/// Memories a step has not started: distinct `source_id`s with `source =
/// 'memory'` and no `enrichment_steps` row for `step_name`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepBacklog {
    pub step_name: String,
    pub count: u64,
}

/// Memories (distinct `source_id`, `source = 'memory'`) by how their entity
/// detection stands: `done` has `entity_extract` in a done status OR is
/// linked to an entity in `memory_entities`; `failed` has `entity_extract` or
/// `entity_link` in a failed status and is not done.
///
/// Done is not "both steps ok": the three writers do not agree on receipts.
/// Post-ingest writes both; the ambient slice writes only `entity_extract`
/// when it creates a new entity; the refinery entity phase writes neither and
/// only links. Requiring both rows showed "0 of 9 scanned" beside 5 entities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectTally {
    pub done: u64,
    pub failed: u64,
}

/// Exact distinct-memory counts for one asset's two steps: `failed` is
/// distinct memories with at least one step in a failed status;
/// `unfinished` is distinct memories not fully done — at least one step
/// failed, pending, or with no row at all. A superset of `failed`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetBacklog {
    pub failed: u64,
    pub unfinished: u64,
}

/// Open `refinement_queue` rows for one `(action, status)` pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefinementOpenCount {
    pub action: String,
    pub status: String,
    pub count: u64,
}

/// Memory rows waiting for a vector after a big import stored them without
/// one, from live counters kept beside the write path and the background
/// catch-up (`MemoryDB::embedding_backlog`), not from a table scan. `total`
/// is how many rows the current catch-up has had in all, so a reader can say
/// "done of total"; it never shrinks while the catch-up runs, and both are
/// zero when none is running.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EmbeddingBacklog {
    pub remaining: u64,
    pub total: u64,
}

impl EmbeddingBacklog {
    /// Rows the current catch-up has embedded (or that no longer need it).
    pub const fn done(self) -> u64 {
        self.total.saturating_sub(self.remaining)
    }
}

/// Every count `GET /api/activity` composes into an `ActivityResponse`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityCounts {
    /// Distinct `source_id`s in `memories` with `source = 'memory'`.
    pub memories_total: u64,
    /// `(step_name, status, count)` cells from `enrichment_steps`.
    pub step_counts: Vec<StepCount>,
    /// Memories (source = 'memory') with no `enrichment_steps` row for this
    /// step name. Keyed by step name: the backlog a step has not started.
    /// One entry each for `title_enrich`, `page_growth`, `entity_extract`
    /// and `entity_link`, even when the count is zero.
    pub memories_without_step: Vec<StepBacklog>,
    /// Exact entity-detection standing over distinct memories.
    pub detect: DetectTally,
    /// Exact distinct-memory counts for the Memories steps
    /// (`title_enrich`, `page_growth`).
    pub memories_backlog: AssetBacklog,
    /// Exact distinct-memory counts for the Entities steps
    /// (`entity_extract`, `entity_link`).
    pub entities_backlog: AssetBacklog,
    /// Entity shadow pages that are live but unconfirmed.
    pub entities_detected: u64,
    /// Entity shadow pages that are live and confirmed.
    pub entities_confirmed: u64,
    /// The whole live non-entity page population. `pages_stale` and
    /// `pages_refresh_blocked` are SUBSETS of this count, not a separate
    /// population: derive current/blocked from it, never add to it.
    pub pages_total: u64,
    /// Live non-entity pages with `stale_reason` set and refreshable.
    pub pages_stale: u64,
    /// Live non-entity pages whose refresh is blocked.
    pub pages_refresh_blocked: u64,
    /// Latest background write, epoch seconds: the newer of
    /// `MAX(enrichment_steps.updated_at)` and the latest page compile. Pages
    /// count because the refinery writes entity and concept pages without
    /// step rows, and "Nothing has run yet" beside a written page is false.
    /// `None` when neither has happened.
    pub last_work_at: Option<i64>,
    /// Open `refinement_queue` rows only (`pending` or `awaiting_review`), by
    /// action and status, ordered by action then status. Served by the partial
    /// index `idx_refinement_status`, which covers exactly those two statuses.
    pub refinement_open: Vec<RefinementOpenCount>,
    /// Rows waiting for a vector, from the live counters (no query).
    pub embedding_backlog: EmbeddingBacklog,
}

impl MemoryDB {
    /// Read every count `GET /api/activity` needs in one pass. Holds the
    /// connection guard for the whole function; every query is read-only.
    pub async fn activity_counts(&self) -> Result<ActivityCounts, WenlanError> {
        let conn = self.conn.lock().await;

        let memories_total = scalar_u64(
            &conn,
            "SELECT COUNT(DISTINCT source_id) FROM memories WHERE source = 'memory'",
            "activity_counts memories_total",
        )
        .await?;

        let mut step_rows = conn
            .query(
                "SELECT step_name, status, COUNT(*) FROM enrichment_steps \
                 GROUP BY step_name, status",
                (),
            )
            .await
            .map_err(|e| WenlanError::VectorDb(format!("activity_counts step_counts: {e}")))?;
        let mut step_counts = Vec::new();
        while let Some(row) = step_rows
            .next()
            .await
            .map_err(|e| WenlanError::VectorDb(format!("activity_counts step_counts row: {e}")))?
        {
            step_counts.push(StepCount {
                step_name: row.get::<String>(0).map_err(|e| {
                    WenlanError::VectorDb(format!("activity_counts step_counts name: {e}"))
                })?,
                status: row.get::<String>(1).map_err(|e| {
                    WenlanError::VectorDb(format!("activity_counts step_counts status: {e}"))
                })?,
                count: row
                    .get::<i64>(2)
                    .map_err(|e| {
                        WenlanError::VectorDb(format!("activity_counts step_counts count: {e}"))
                    })?
                    .max(0) as u64,
            });
        }
        drop(step_rows);

        // Per-step backlog: memories with no row for that step at all. A
        // single "raw" count cannot answer this per step — a memory with
        // only a `title_enrich` row is still waiting on `page_growth` — so
        // each step gets its own correlated subquery.
        let mut backlog_rows = conn
            .query(
                // DISTINCT: `memories` holds one row per chunk, so a bare
                // COUNT(*) would count chunks, not memories.
                "SELECT s.name, (SELECT COUNT(DISTINCT m.source_id) FROM memories m \
                 WHERE m.source = 'memory' \
                 AND NOT EXISTS (SELECT 1 FROM enrichment_steps e \
                 WHERE e.source_id = m.source_id AND e.step_name = s.name)) \
                 FROM (SELECT 'title_enrich' AS name UNION ALL SELECT 'page_growth' \
                 UNION ALL SELECT 'entity_extract' UNION ALL SELECT 'entity_link') s",
                (),
            )
            .await
            .map_err(|e| {
                WenlanError::VectorDb(format!("activity_counts memories_without_step: {e}"))
            })?;
        let mut memories_without_step = Vec::new();
        while let Some(row) = backlog_rows.next().await.map_err(|e| {
            WenlanError::VectorDb(format!("activity_counts memories_without_step row: {e}"))
        })? {
            memories_without_step.push(StepBacklog {
                step_name: row.get::<String>(0).map_err(|e| {
                    WenlanError::VectorDb(format!(
                        "activity_counts memories_without_step name: {e}"
                    ))
                })?,
                count: row
                    .get::<i64>(1)
                    .map_err(|e| {
                        WenlanError::VectorDb(format!(
                            "activity_counts memories_without_step count: {e}"
                        ))
                    })?
                    .max(0) as u64,
            });
        }
        drop(backlog_rows);

        // Exact per-memory standing for both two-step lanes in one pass over
        // distinct memories. The inner DISTINCT collapses chunks so each
        // memory counts once; the outer CASE sums use only portable SQL (no
        // FILTER clause). `ok`/`bad` count ROWS in a done/failed status for
        // the lane's steps. A memory's page work is finished only when both
        // steps are ok; its entity scan is finished per `DetectTally`.
        let mut tally_rows = conn
            .query(
                "SELECT \
                 COALESCE(SUM(CASE WHEN ent_ok > 0 OR ent_linked > 0 THEN 1 ELSE 0 END), 0), \
                 COALESCE(SUM(CASE WHEN ent_ok = 0 AND ent_linked = 0 AND ent_bad > 0 \
                                   THEN 1 ELSE 0 END), 0), \
                 COALESCE(SUM(CASE WHEN mem_bad > 0 THEN 1 ELSE 0 END), 0), \
                 COALESCE(SUM(CASE WHEN mem_ok < 2 THEN 1 ELSE 0 END), 0), \
                 COALESCE(SUM(CASE WHEN ent_ok = 0 AND ent_linked = 0 AND ent_bad > 0 \
                                   THEN 1 ELSE 0 END), 0), \
                 COALESCE(SUM(CASE WHEN ent_ok = 0 AND ent_linked = 0 THEN 1 ELSE 0 END), 0) \
                 FROM ( \
                   SELECT m.source_id, \
                     SUM(CASE WHEN e.step_name IN ('title_enrich', 'page_growth') \
                              AND e.status IN ('ok', 'skipped') THEN 1 ELSE 0 END) AS mem_ok, \
                     SUM(CASE WHEN e.step_name IN ('title_enrich', 'page_growth') \
                              AND e.status IN ('failed', 'abandoned') THEN 1 ELSE 0 END) AS mem_bad, \
                     SUM(CASE WHEN e.step_name = 'entity_extract' \
                              AND e.status IN ('ok', 'skipped') THEN 1 ELSE 0 END) AS ent_ok, \
                     (SELECT COUNT(*) FROM memory_entities me \
                      WHERE me.memory_id = m.source_id) AS ent_linked, \
                     SUM(CASE WHEN e.step_name IN ('entity_extract', 'entity_link') \
                              AND e.status IN ('failed', 'abandoned') THEN 1 ELSE 0 END) AS ent_bad \
                   FROM (SELECT DISTINCT source_id FROM memories WHERE source = 'memory') m \
                   LEFT JOIN enrichment_steps e ON e.source_id = m.source_id \
                   GROUP BY m.source_id \
                 )",
                (),
            )
            .await
            .map_err(|e| WenlanError::VectorDb(format!("activity_counts tallies: {e}")))?;
        let tally_row = tally_rows
            .next()
            .await
            .map_err(|e| WenlanError::VectorDb(format!("activity_counts tallies row: {e}")))?
            .ok_or_else(|| WenlanError::Generic("activity_counts tallies: no rows".into()))?;
        let tally_cell = |index: i32| {
            tally_row
                .get::<i64>(index)
                .map(|value| value.max(0) as u64)
                .map_err(|e| {
                    WenlanError::VectorDb(format!("activity_counts tallies cell {index}: {e}"))
                })
        };
        let detect = DetectTally {
            done: tally_cell(0)?,
            failed: tally_cell(1)?,
        };
        let memories_backlog = AssetBacklog {
            failed: tally_cell(2)?,
            unfinished: tally_cell(3)?,
        };
        let entities_backlog = AssetBacklog {
            failed: tally_cell(4)?,
            unfinished: tally_cell(5)?,
        };
        drop(tally_rows);

        // Entity lifecycle predicates mirror `entity_status_predicate` in
        // db.rs, over the same `entity_page_map`/`pages` shadow shape the
        // scoped entity readers use.
        let entities_detected = scalar_u64(
            &conn,
            "SELECT COUNT(*) FROM entity_page_map epm JOIN pages p ON p.id = epm.page_id \
             WHERE p.kind = 'entity' \
             AND (p.status != 'archived' AND COALESCE(p.entity_confirmed, 0) = 0)",
            "activity_counts entities_detected",
        )
        .await?;
        let entities_confirmed = scalar_u64(
            &conn,
            "SELECT COUNT(*) FROM entity_page_map epm JOIN pages p ON p.id = epm.page_id \
             WHERE p.kind = 'entity' \
             AND (p.status != 'archived' AND COALESCE(p.entity_confirmed, 0) = 1)",
            "activity_counts entities_confirmed",
        )
        .await?;

        // Same SQL as `count_active_pages`: the WHOLE live non-entity page
        // population. Stale and refresh-blocked pages are subsets of this
        // count — compose derives current/blocked from it, never adds to it.
        let pages_total = scalar_u64(
            &conn,
            "SELECT COUNT(*) FROM pages \
             WHERE status = 'active' AND COALESCE(kind, 'concept') != 'entity'",
            "activity_counts pages_total",
        )
        .await?;
        // Stale-page predicates follow `list_stale_pages_scoped`'s base shape
        // (live non-entity pages with `stale_reason` set), split by whether a
        // refresh is blocked.
        let pages_stale = scalar_u64(
            &conn,
            "SELECT COUNT(*) FROM pages \
             WHERE status = 'active' AND COALESCE(kind, 'concept') != 'entity' \
             AND stale_reason IS NOT NULL AND refresh_blocked_reason IS NULL",
            "activity_counts pages_stale",
        )
        .await?;
        let pages_refresh_blocked = scalar_u64(
            &conn,
            "SELECT COUNT(*) FROM pages \
             WHERE status = 'active' AND COALESCE(kind, 'concept') != 'entity' \
             AND stale_reason IS NOT NULL AND refresh_blocked_reason IS NOT NULL",
            "activity_counts pages_refresh_blocked",
        )
        .await?;

        let mut updated_rows = conn
            // `last_compiled` is RFC 3339 text in production; a non-text value
            // is skipped rather than read as a Julian day number.
            .query(
                "SELECT MAX(t) FROM ( \
                   SELECT MAX(updated_at) AS t FROM enrichment_steps \
                   UNION ALL \
                   SELECT CAST(strftime('%s', MAX(last_compiled)) AS INTEGER) FROM pages \
                   WHERE typeof(last_compiled) = 'text' \
                 )",
                (),
            )
            .await
            .map_err(|e| WenlanError::VectorDb(format!("activity_counts last_work_at: {e}")))?;
        let last_work_at =
            match updated_rows.next().await.map_err(|e| {
                WenlanError::VectorDb(format!("activity_counts last_work_at row: {e}"))
            })? {
                Some(row) => row.get::<Option<i64>>(0).map_err(|e| {
                    WenlanError::VectorDb(format!("activity_counts last_work_at get: {e}"))
                })?,
                None => None,
            };
        drop(updated_rows);

        let mut refinement_rows = conn
            .query(
                "SELECT action, status, COUNT(*) FROM refinement_queue \
                 WHERE status IN ('pending', 'awaiting_review') \
                 GROUP BY action, status ORDER BY action, status",
                (),
            )
            .await
            .map_err(|e| WenlanError::VectorDb(format!("activity_counts refinement_open: {e}")))?;
        let mut refinement_open = Vec::new();
        while let Some(row) = refinement_rows.next().await.map_err(|e| {
            WenlanError::VectorDb(format!("activity_counts refinement_open row: {e}"))
        })? {
            refinement_open.push(RefinementOpenCount {
                action: row.get::<String>(0).map_err(|e| {
                    WenlanError::VectorDb(format!("activity_counts refinement_open action: {e}"))
                })?,
                status: row.get::<String>(1).map_err(|e| {
                    WenlanError::VectorDb(format!("activity_counts refinement_open status: {e}"))
                })?,
                count: row
                    .get::<i64>(2)
                    .map_err(|e| {
                        WenlanError::VectorDb(format!("activity_counts refinement_open count: {e}"))
                    })?
                    .max(0) as u64,
            });
        }
        drop(refinement_rows);

        Ok(ActivityCounts {
            memories_total,
            step_counts,
            memories_without_step,
            detect,
            memories_backlog,
            entities_backlog,
            entities_detected,
            entities_confirmed,
            pages_total,
            pages_stale,
            pages_refresh_blocked,
            last_work_at,
            refinement_open,
            embedding_backlog: self.embedding_backlog(),
        })
    }
}

async fn scalar_u64(conn: &libsql::Connection, sql: &str, what: &str) -> Result<u64, WenlanError> {
    let mut rows = conn
        .query(sql, ())
        .await
        .map_err(|e| WenlanError::VectorDb(format!("{what} query: {e}")))?;
    let row = rows
        .next()
        .await
        .map_err(|e| WenlanError::VectorDb(format!("{what} next: {e}")))?
        .ok_or_else(|| WenlanError::Generic(format!("{what}: no rows")))?;
    Ok(row
        .get::<i64>(0)
        .map_err(|e| WenlanError::VectorDb(format!("{what} get: {e}")))?
        .max(0) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn seed_memory(db: &MemoryDB, id: &str, source_id: &str, steps: &[(&str, &str, i64)]) {
        let conn = db.conn.lock().await;
        conn.execute(
            "INSERT INTO memories (id, content, source, source_id, title, chunk_index, \
             last_modified, chunk_type, space) \
             VALUES (?1, 'content', 'memory', ?2, 'title', 0, 0, 'text', 'work')",
            libsql::params![id, source_id],
        )
        .await
        .unwrap();
        for (step_name, status, updated_at) in steps {
            conn.execute(
                "INSERT INTO enrichment_steps (source_id, step_name, status, attempts, updated_at) \
                 VALUES (?1, ?2, ?3, 1, ?4)",
                libsql::params![source_id, *step_name, *status, *updated_at],
            )
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn activity_counts_empty_db_is_all_zero() {
        let (db, _tmp) = super::super::tests::test_db().await;
        let counts = db.activity_counts().await.unwrap();
        assert_eq!(
            counts,
            ActivityCounts {
                memories_total: 0,
                step_counts: vec![],
                memories_without_step: vec![
                    StepBacklog {
                        step_name: "title_enrich".to_string(),
                        count: 0,
                    },
                    StepBacklog {
                        step_name: "page_growth".to_string(),
                        count: 0,
                    },
                    StepBacklog {
                        step_name: "entity_extract".to_string(),
                        count: 0,
                    },
                    StepBacklog {
                        step_name: "entity_link".to_string(),
                        count: 0,
                    },
                ],
                detect: DetectTally { done: 0, failed: 0 },
                memories_backlog: AssetBacklog {
                    failed: 0,
                    unfinished: 0,
                },
                entities_backlog: AssetBacklog {
                    failed: 0,
                    unfinished: 0,
                },
                entities_detected: 0,
                entities_confirmed: 0,
                pages_total: 0,
                pages_stale: 0,
                pages_refresh_blocked: 0,
                last_work_at: None,
                refinement_open: vec![],
                embedding_backlog: super::EmbeddingBacklog::default(),
            }
        );
    }

    #[tokio::test]
    async fn activity_counts_groups_steps_and_splits_pages_and_entities() {
        let (db, _tmp) = super::super::tests::test_db().await;
        {
            let conn = db.conn.lock().await;
            conn.execute_batch(
                "INSERT INTO spaces (id, name, created_at, updated_at) \
                 VALUES ('space-work', 'work', 1, 1);",
            )
            .await
            .unwrap();
        }
        // Eight memories: one with a done `title_enrich` row, none with a
        // `page_growth` row, plus one non-memory row the reader must ignore.
        for index in 1..=8 {
            let id = format!("row-{index}");
            let source_id = format!("mem-{index}");
            let steps: &[(&str, &str, i64)] = if index == 1 {
                &[("title_enrich", "ok", 100)]
            } else {
                &[]
            };
            seed_memory(&db, &id, &source_id, steps).await;
        }
        {
            let conn = db.conn.lock().await;
            conn.execute(
                "INSERT INTO memories (id, content, source, source_id, title, chunk_index, \
                 last_modified, chunk_type, space) \
                 VALUES ('row-note-1', 'content', 'note', 'note-1', 'title', 0, 0, 'text', 'work')",
                (),
            )
            .await
            .unwrap();
            conn.execute(
                "INSERT INTO pages (id, title, content, created_at, last_compiled, last_modified) \
                 VALUES ('page-active', 'Active', 'body', 0, 0, 0)",
                (),
            )
            .await
            .unwrap();
            conn.execute(
                "INSERT INTO pages (id, title, content, created_at, last_compiled, last_modified, \
                 stale_reason) \
                 VALUES ('page-stale', 'Stale', 'body', 0, 0, 0, 'source_updated')",
                (),
            )
            .await
            .unwrap();
            conn.execute(
                "INSERT INTO pages (id, title, content, created_at, last_compiled, last_modified, \
                 stale_reason, refresh_blocked_reason) \
                 VALUES ('page-blocked', 'Blocked', 'body', 0, 0, 0, 'source_updated', 'citations')",
                (),
            )
            .await
            .unwrap();
        }
        db.test_seed_entity_shadow_page(crate::db::TestEntity::new("ent-1", "One", "concept"))
            .await
            .unwrap();
        db.test_seed_entity_shadow_page(
            crate::db::TestEntity::new("ent-2", "Two", "concept").confirmed(true),
        )
        .await
        .unwrap();

        let counts = db.activity_counts().await.unwrap();
        assert_eq!(counts.memories_total, 8);
        let mut cells: Vec<(&str, &str, u64)> = counts
            .step_counts
            .iter()
            .map(|cell| (cell.step_name.as_str(), cell.status.as_str(), cell.count))
            .collect();
        cells.sort();
        assert_eq!(cells, vec![("title_enrich", "ok", 1)]);
        let backlog = |step_name: &str| {
            counts
                .memories_without_step
                .iter()
                .find(|entry| entry.step_name == step_name)
                .map(|entry| entry.count)
        };
        // The one memory with a `title_enrich` row is still waiting on every
        // other step; the seven without any row wait on all four.
        assert_eq!(backlog("title_enrich"), Some(7));
        assert_eq!(backlog("page_growth"), Some(8));
        assert_eq!(backlog("entity_extract"), Some(8));
        assert_eq!(backlog("entity_link"), Some(8));
        assert_eq!(counts.detect, DetectTally { done: 0, failed: 0 });
        assert_eq!(
            counts.memories_backlog,
            AssetBacklog {
                failed: 0,
                unfinished: 8,
            }
        );
        assert_eq!(
            counts.entities_backlog,
            AssetBacklog {
                failed: 0,
                unfinished: 8,
            }
        );
        assert_eq!(counts.entities_detected, 1);
        assert_eq!(counts.entities_confirmed, 1);
        assert_eq!(counts.pages_total, 3);
        assert_eq!(counts.pages_stale, 1);
        assert_eq!(counts.pages_refresh_blocked, 1);
        // The seeded entity pages are compiled "now", which is newer than the
        // step row at 100.
        assert!(matches!(counts.last_work_at, Some(t) if t > 1_700_000_000));
    }

    #[tokio::test]
    async fn backlog_counts_memories_not_chunks() {
        let (db, _tmp) = super::super::tests::test_db().await;
        {
            let conn = db.conn.lock().await;
            conn.execute_batch(
                "INSERT INTO spaces (id, name, created_at, updated_at) \
                 VALUES ('space-work', 'work', 1, 1);",
            )
            .await
            .unwrap();
            // One memory in three chunks, no step rows: every backlog is 1,
            // not 3.
            for chunk_index in 0..3i64 {
                conn.execute(
                    "INSERT INTO memories (id, content, source, source_id, title, chunk_index, \
                     last_modified, chunk_type, space) \
                     VALUES (?1, 'content', 'memory', 'mem-chunked', 'title', ?2, 0, 'text', 'work')",
                    libsql::params![format!("row-chunk-{chunk_index}"), chunk_index],
                )
                .await
                .unwrap();
            }
        }

        let counts = db.activity_counts().await.unwrap();
        assert_eq!(counts.memories_total, 1);
        for step_name in [
            "title_enrich",
            "page_growth",
            "entity_extract",
            "entity_link",
        ] {
            let backlog = counts
                .memories_without_step
                .iter()
                .find(|entry| entry.step_name == step_name)
                .map(|entry| entry.count);
            assert_eq!(backlog, Some(1), "backlog for {step_name}");
        }
        assert_eq!(
            counts.memories_backlog,
            AssetBacklog {
                failed: 0,
                unfinished: 1,
            }
        );
    }

    #[tokio::test]
    async fn detect_counts_memories_scanned_by_any_entity_writer() {
        let (db, _tmp) = super::super::tests::test_db().await;
        {
            let conn = db.conn.lock().await;
            conn.execute_batch(
                "INSERT INTO spaces (id, name, created_at, updated_at) \
                 VALUES ('space-work', 'work', 1, 1);",
            )
            .await
            .unwrap();
        }
        // Post-ingest writes both receipts.
        seed_memory(
            &db,
            "row-a",
            "mem-a",
            &[("entity_extract", "ok", 10), ("entity_link", "ok", 11)],
        )
        .await;
        seed_memory(
            &db,
            "row-b",
            "mem-b",
            &[("entity_extract", "ok", 12), ("entity_link", "skipped", 13)],
        )
        .await;
        // The ambient slice writes only `entity_extract` when it creates a
        // new entity.
        seed_memory(&db, "row-c", "mem-c", &[("entity_extract", "ok", 14)]).await;
        // Link failed and nothing else: not scanned, failed.
        seed_memory(&db, "row-d", "mem-d", &[("entity_link", "failed", 15)]).await;
        // The refinery entity phase writes no receipt, only the link.
        seed_memory(&db, "row-e", "mem-e", &[]).await;
        // Extract finished even though linking failed: scanned, not failed.
        seed_memory(
            &db,
            "row-f",
            "mem-f",
            &[("entity_extract", "ok", 16), ("entity_link", "failed", 17)],
        )
        .await;
        // No work at all: waiting.
        seed_memory(&db, "row-g", "mem-g", &[]).await;
        db.test_seed_entity_shadow_page(crate::db::TestEntity::new("ent-e", "E", "concept"))
            .await
            .unwrap();
        {
            let conn = db.conn.lock().await;
            conn.execute(
                "INSERT INTO memory_entities (memory_id, entity_id) VALUES ('mem-e', 'ent-e')",
                (),
            )
            .await
            .unwrap();
        }

        let counts = db.activity_counts().await.unwrap();
        assert_eq!(counts.detect, DetectTally { done: 5, failed: 1 });
        assert_eq!(
            counts.entities_backlog,
            AssetBacklog {
                failed: 1,
                unfinished: 2,
            }
        );
    }

    #[tokio::test]
    async fn last_work_counts_page_compiles_without_step_rows() {
        let (db, _tmp) = super::super::tests::test_db().await;
        {
            let conn = db.conn.lock().await;
            // The refinery compiles pages and writes no step rows.
            conn.execute(
                "INSERT INTO pages (id, title, content, created_at, last_compiled, last_modified) \
                 VALUES ('page-refinery', 'Refinery', 'body', 0, \
                 '2023-11-14T22:15:00.123456+00:00', 0)",
                (),
            )
            .await
            .unwrap();
        }
        let counts = db.activity_counts().await.unwrap();
        assert_eq!(counts.last_work_at, Some(1_700_000_100));

        // A newer step row wins over an older compile.
        seed_memory(
            &db,
            "row-new",
            "mem-new",
            &[("title_enrich", "ok", 1_700_000_200)],
        )
        .await;
        let counts = db.activity_counts().await.unwrap();
        assert_eq!(counts.last_work_at, Some(1_700_000_200));
    }

    #[tokio::test]
    async fn refinement_open_groups_only_open_rows() {
        let (db, _tmp) = super::super::tests::test_db().await;
        {
            let conn = db.conn.lock().await;
            // Every written status across two actions: only `pending` and
            // `awaiting_review` are open.
            let rows: &[(&str, &str, &str)] = &[
                ("r-1", "entity_merge", "pending"),
                ("r-2", "entity_merge", "pending"),
                ("r-3", "entity_merge", "awaiting_review"),
                ("r-4", "entity_merge", "dismissed"),
                ("r-5", "entity_merge", "resolved"),
                ("r-6", "community_split", "awaiting_review"),
                ("r-7", "community_split", "awaiting_review"),
                ("r-8", "community_split", "awaiting_review"),
                ("r-9", "community_split", "auto_applied"),
                ("r-10", "community_split", "superseded"),
            ];
            for (id, action, status) in rows {
                conn.execute(
                    "INSERT INTO refinement_queue (id, action, source_ids, status) \
                     VALUES (?1, ?2, '[]', ?3)",
                    libsql::params![*id, *action, *status],
                )
                .await
                .unwrap();
            }
        }

        let counts = db.activity_counts().await.unwrap();
        let open = |action: &str, status: &str, count: u64| RefinementOpenCount {
            action: action.to_string(),
            status: status.to_string(),
            count,
        };
        assert_eq!(
            counts.refinement_open,
            vec![
                open("community_split", "awaiting_review", 3),
                open("entity_merge", "awaiting_review", 1),
                open("entity_merge", "pending", 2),
            ]
        );
    }
}
