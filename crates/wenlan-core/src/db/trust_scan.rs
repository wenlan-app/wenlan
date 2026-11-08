// SPDX-License-Identifier: Apache-2.0
//! Reads for the nightly trust scan ([`crate::trust_scan`]), the one row that
//! keeps its latest report, and the explicit re-queue of a page whose
//! citation backfill gave up.
//!
//! Migration 159 adds `trust_scan_reports`: one row, the latest report as
//! JSON. It is derived: the next scan writes it again.

use super::MemoryDB;
use crate::WenlanError;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

fn db_err(context: &str) -> impl Fn(libsql::Error) -> WenlanError + '_ {
    move |error| WenlanError::VectorDb(format!("{context}: {error}"))
}

const REPORTS_DDL: &str = "
CREATE TABLE IF NOT EXISTS trust_scan_reports (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    ran_at INTEGER NOT NULL,
    report TEXT NOT NULL
);";

/// The changelog test the backfill's give-up branches leave behind: the
/// page's last entry is the backfill's, and it says it gave up.
const GAVE_UP: &str = "citations = '[]'
    AND json_valid(changelog)
    AND json_extract(changelog, '$[#-1].edited_by') = 'citation_backfill'
    AND json_extract(changelog, '$[#-1].citations_summary') LIKE 'citation backfill gave up:%'";

/// One live page as the scan reads it.
#[derive(Debug, Clone)]
pub struct ScanPage {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub version: i64,
    pub content: String,
    /// The page's own `source_memory_ids`.
    pub listed: Vec<String>,
}

/// The version before a page's current one.
#[derive(Debug, Clone)]
pub struct PreviousVersion {
    pub version: i64,
    pub content: String,
    /// Who wrote the *current* version.
    pub current_edited_by: String,
}

/// What [`orphan_source_inventory`] found, per page.
#[derive(Debug, Clone, Default)]
pub struct OrphanInventory {
    pub pages: HashMap<String, HashSet<String>>,
    pub links: usize,
    pub listed: usize,
}

/// The sources the daily `cleanup_orphaned_page_sources` removes: live
/// `cites` links to a memory that no longer exists, and ids in any page's
/// own source list with no memory. Shared by that cleanup and the scan, so
/// the scan reports exactly what the cleanup will remove.
pub(super) async fn orphan_source_inventory(
    conn: &libsql::Connection,
) -> Result<OrphanInventory, WenlanError> {
    let mut inventory = OrphanInventory::default();
    let mut orphan_rows = conn
        .query(
            "SELECT src_id, dst_id
             FROM edges
             WHERE edge_type = 'cites' AND src_kind = 'page' AND dst_kind = 'memory'
               AND valid_until IS NULL
               AND NOT EXISTS (
                   SELECT 1 FROM memories m
                   WHERE m.source != 'episode'
                     AND (m.source_id = edges.dst_id OR m.id = edges.dst_id)
               )",
            (),
        )
        .await
        .map_err(db_err("cleanup_orphaned_page_sources inventory"))?;
    while let Some(row) = orphan_rows
        .next()
        .await
        .map_err(db_err("cleanup_orphaned_page_sources inventory row"))?
    {
        let page_id = row
            .get::<String>(0)
            .map_err(db_err("cleanup_orphaned_page_sources page id"))?;
        let locator = row
            .get::<String>(1)
            .map_err(db_err("cleanup_orphaned_page_sources locator"))?;
        inventory.links += 1;
        inventory.pages.entry(page_id).or_default().insert(locator);
    }
    drop(orphan_rows);

    // Legacy JSON may contain provenance that never received a join row.
    // Inventory it independently so JSON-only ghosts cannot survive the
    // canonical orphan sweep.
    let mut json_orphan_rows = conn
        .query(
            "SELECT p.id, CAST(legacy_source.value AS TEXT)
             FROM pages p,
                  json_each(COALESCE(p.source_memory_ids, '[]')) legacy_source
             WHERE NOT EXISTS (
                 SELECT 1 FROM memories m
                 WHERE m.source != 'episode'
                   AND (m.source_id = CAST(legacy_source.value AS TEXT)
                        OR m.id = CAST(legacy_source.value AS TEXT))
             )",
            (),
        )
        .await
        .map_err(db_err("cleanup_orphaned_page_sources JSON inventory"))?;
    while let Some(row) = json_orphan_rows
        .next()
        .await
        .map_err(db_err("cleanup_orphaned_page_sources JSON inventory row"))?
    {
        let page_id = row
            .get::<String>(0)
            .map_err(db_err("cleanup_orphaned_page_sources JSON page id"))?;
        let locator = row
            .get::<String>(1)
            .map_err(db_err("cleanup_orphaned_page_sources JSON locator"))?;
        inventory.listed += 1;
        inventory.pages.entry(page_id).or_default().insert(locator);
    }
    Ok(inventory)
}

impl MemoryDB {
    /// Migration 159: `trust_scan_reports`. `CREATE TABLE IF NOT EXISTS`, so
    /// a crash before the version bump replays cleanly.
    pub(super) async fn migrate_159_trust_scan_reports(
        &self,
        prior_version: i64,
    ) -> Result<(), WenlanError> {
        self.backup_before_migration(159, prior_version).await?;
        let conn = self.conn.lock().await;
        conn.execute_batch(REPORTS_DDL)
            .await
            .map_err(db_err("m159 trust scan DDL"))?;
        conn.execute("PRAGMA user_version = 159", ())
            .await
            .map_err(db_err("m159 bump"))?;
        log::info!("[migration] Migration 159 applied: nightly trust scan report");
        Ok(())
    }

    /// Every live page: id, title, kind, version, body and own source list.
    pub async fn trust_scan_pages(&self) -> Result<Vec<ScanPage>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT id, title, kind, version, content, COALESCE(source_memory_ids, '[]')
                   FROM pages WHERE status = 'active' ORDER BY id",
                (),
            )
            .await
            .map_err(db_err("trust scan pages"))?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.map_err(db_err("trust scan page row"))? {
            let listed: String = row.get(5).map_err(db_err("trust scan page sources"))?;
            out.push(ScanPage {
                id: row.get(0).map_err(db_err("trust scan page id"))?,
                title: row.get(1).map_err(db_err("trust scan page title"))?,
                kind: row.get(2).map_err(db_err("trust scan page kind"))?,
                version: row.get(3).map_err(db_err("trust scan page version"))?,
                content: row.get(4).map_err(db_err("trust scan page content"))?,
                listed: serde_json::from_str(&listed).unwrap_or_default(),
            });
        }
        Ok(out)
    }

    /// For each live page with an earlier version in history: that version
    /// and who wrote the current one.
    pub async fn trust_scan_previous_versions(
        &self,
    ) -> Result<HashMap<String, PreviousVersion>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT p.id, prev.version, prev.content, COALESCE(cur.edited_by, '')
                   FROM pages p
                   JOIN page_history prev
                     ON prev.page_id = p.id
                    AND prev.version = (SELECT MAX(h.version) FROM page_history h
                                         WHERE h.page_id = p.id AND h.version < p.version)
                   LEFT JOIN page_history cur
                     ON cur.page_id = p.id AND cur.version = p.version
                  WHERE p.status = 'active'",
                (),
            )
            .await
            .map_err(db_err("trust scan previous versions"))?;
        let mut out = HashMap::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(db_err("trust scan previous version row"))?
        {
            out.insert(
                row.get::<String>(0)
                    .map_err(db_err("trust scan previous id"))?,
                PreviousVersion {
                    version: row.get(1).map_err(db_err("trust scan previous version"))?,
                    content: row.get(2).map_err(db_err("trust scan previous content"))?,
                    current_edited_by: row.get(3).map_err(db_err("trust scan edited_by"))?,
                },
            );
        }
        Ok(out)
    }

    /// Live `cites` links from live pages, any destination kind: the set
    /// `get_page_sources` shows.
    pub async fn trust_scan_live_cites(
        &self,
    ) -> Result<HashMap<String, BTreeSet<String>>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT e.src_id, e.dst_id FROM edges e
                   JOIN pages p ON p.id = e.src_id AND p.status = 'active'
                  WHERE e.edge_type = 'cites' AND e.src_kind = 'page'
                    AND e.valid_until IS NULL",
                (),
            )
            .await
            .map_err(db_err("trust scan cites"))?;
        let mut out: HashMap<String, BTreeSet<String>> = HashMap::new();
        while let Some(row) = rows.next().await.map_err(db_err("trust scan cite row"))? {
            out.entry(row.get(0).map_err(db_err("trust scan cite page"))?)
                .or_default()
                .insert(row.get(1).map_err(db_err("trust scan cite dst"))?);
        }
        Ok(out)
    }

    /// What the next `cleanup_orphaned_page_sources` would remove. Read only.
    pub async fn orphaned_page_sources_preview(
        &self,
    ) -> Result<(usize, usize, BTreeMap<String, BTreeSet<String>>), WenlanError> {
        let conn = self.conn.lock().await;
        let inventory = orphan_source_inventory(&conn).await?;
        let pages = inventory
            .pages
            .into_iter()
            .map(|(page, ids)| (page, ids.into_iter().collect()))
            .collect();
        Ok((inventory.links, inventory.listed, pages))
    }

    /// The older `page_sources` table against the pages' own lists.
    pub async fn trust_scan_legacy_page_sources(
        &self,
    ) -> Result<wenlan_types::trust_scan::LegacyPageSources, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT
                   (SELECT COUNT(*) FROM page_sources),
                   (SELECT COUNT(*) FROM pages p WHERE p.status = 'active'
                       AND json_array_length(COALESCE(p.source_memory_ids, '[]'))
                           <> (SELECT COUNT(*) FROM page_sources s WHERE s.page_id = p.id)),
                   (SELECT COUNT(*) FROM pages p WHERE p.status = 'active' AND p.kind <> 'entity'
                       AND NOT EXISTS (SELECT 1 FROM page_sources s WHERE s.page_id = p.id)),
                   (SELECT COUNT(*) FROM page_sources s WHERE NOT EXISTS (
                       SELECT 1 FROM memories m
                        WHERE m.source != 'episode'
                          AND (m.source_id = s.memory_source_id OR m.id = s.memory_source_id)))",
                (),
            )
            .await
            .map_err(db_err("trust scan legacy sources"))?;
        let Some(row) = rows
            .next()
            .await
            .map_err(db_err("trust scan legacy sources row"))?
        else {
            return Ok(Default::default());
        };
        let n = |i: i32| row.get::<i64>(i).map(|v| v as usize);
        Ok(wenlan_types::trust_scan::LegacyPageSources {
            rows: n(0).map_err(db_err("legacy rows"))?,
            count_mismatches: n(1).map_err(db_err("legacy mismatches"))?,
            pages_without_rows: n(2).map_err(db_err("legacy without rows"))?,
            dangling_rows: n(3).map_err(db_err("legacy dangling"))?,
        })
    }

    /// Live pages whose citation backfill gave up: `(page id, the give-up
    /// note, when)`, the note as the backfill wrote it.
    pub async fn trust_scan_backfill_give_ups(
        &self,
    ) -> Result<Vec<(String, String, Option<i64>)>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                &format!(
                    "SELECT id,
                            json_extract(changelog, '$[#-1].citations_summary'),
                            json_extract(changelog, '$[#-1].at')
                       FROM pages WHERE status = 'active' AND {GAVE_UP} ORDER BY id"
                ),
                (),
            )
            .await
            .map_err(db_err("trust scan give-ups"))?;
        let mut out = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(db_err("trust scan give-up row"))?
        {
            out.push((
                row.get(0).map_err(db_err("give-up id"))?,
                row.get(1).map_err(db_err("give-up reason"))?,
                row.get::<Option<i64>>(2).ok().flatten(),
            ));
        }
        Ok(out)
    }

    /// Put a page whose citation backfill gave up back in the backfill's
    /// queue: its citations go from the give-up `[]` to unset, and any
    /// attempt count left for it is dropped, so the backfill tries it again
    /// with three fresh attempts. Only ever called on request (the person's
    /// approval of the scan's finding); nothing re-queues automatically.
    /// False when the page had not given up.
    pub async fn requeue_citation_backfill(&self, page_id: &str) -> Result<bool, WenlanError> {
        let conn = self.conn.lock().await;
        let changed = conn
            .execute(
                &format!(
                    "UPDATE pages SET citations = NULL
                      WHERE id = ?1 AND status = 'active' AND {GAVE_UP}"
                ),
                libsql::params![page_id],
            )
            .await
            .map_err(db_err("citation requeue"))?;
        if changed > 0 {
            conn.execute(
                "DELETE FROM app_metadata WHERE key = ?1",
                libsql::params![crate::citations::attempt_key(page_id)],
            )
            .await
            .map_err(db_err("citation requeue attempts"))?;
        }
        Ok(changed > 0)
    }

    /// Keep `report` as the latest trust scan.
    pub async fn save_trust_scan_report(
        &self,
        ran_at: i64,
        report: &str,
    ) -> Result<(), WenlanError> {
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO trust_scan_reports (id, ran_at, report) VALUES (1, ?1, ?2)
             ON CONFLICT(id) DO UPDATE SET ran_at = excluded.ran_at, report = excluded.report",
            libsql::params![ran_at, report],
        )
        .await
        .map_err(db_err("save trust scan"))?;
        Ok(())
    }

    /// The latest trust scan's report JSON, if one ran.
    pub async fn latest_trust_scan_report(&self) -> Result<Option<String>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query("SELECT report FROM trust_scan_reports WHERE id = 1", ())
            .await
            .map_err(db_err("latest trust scan"))?;
        match rows.next().await.map_err(db_err("latest trust scan row"))? {
            Some(row) => Ok(Some(
                row.get(0).map_err(db_err("latest trust scan report"))?,
            )),
            None => Ok(None),
        }
    }
}
