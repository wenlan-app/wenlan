// SPDX-License-Identifier: Apache-2.0
//! Reads behind steep on demand (`crate::steep`): which notes are waiting
//! for steep, and the fenced staleness clear an agent refresh ends with.

use super::MemoryDB;
use crate::error::WenlanError;
use crate::pages::Page;

/// The `stale_reason` values a steep run settles.
pub(crate) const STEEP_STALE_REASONS: [&str; 2] = ["source_updated", "source_removed"];

impl MemoryDB {
    /// The holds steep runs have on notes right now.
    pub fn steep_leases(&self) -> &crate::steep::SteepLeases {
        &self.steep_leases
    }

    /// Every active note waiting for steep, newest change first, ties by id.
    /// Entity pages are left out: their body is owned by the entity graph.
    pub(crate) async fn list_steep_pages(&self) -> Result<Vec<Page>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT id, title, summary, content, entity_id, space, source_memory_ids, version, status, created_at, last_compiled, last_modified, COALESCE(sources_updated_count, 0), stale_reason, COALESCE(user_edited, 0), COALESCE(changelog, '[]'), COALESCE(creation_kind, 'distilled'), COALESCE(review_status, 'confirmed'), workspace, citations, COALESCE(kind, 'concept'), refresh_blocked_reason, COALESCE(review_required, 0)
                 FROM pages
                 WHERE stale_reason IN (?1, ?2) AND status = 'active'
                   AND COALESCE(kind, 'concept') != 'entity'
                 ORDER BY last_modified DESC, id ASC",
                libsql::params![STEEP_STALE_REASONS[0], STEEP_STALE_REASONS[1]],
            )
            .await
            .map_err(|e| WenlanError::VectorDb(format!("list_steep_pages: {e}")))?;
        let mut pages = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| WenlanError::VectorDb(format!("list_steep_pages row: {e}")))?
        {
            pages.push(Self::row_to_page(&row)?);
        }
        Ok(pages)
    }

    /// Clear a note's staleness only while its source revision is still
    /// `source_revision`: the value a write just left behind. A source change
    /// or stale mark that landed after that write moved the revision on, and
    /// its staleness is kept. Returns whether the row matched.
    pub async fn clear_page_staleness_at_revision(
        &self,
        page_id: &str,
        source_revision: i64,
    ) -> Result<bool, WenlanError> {
        let conn = self.conn.lock().await;
        let affected = conn
            .execute(
                "UPDATE pages
                 SET stale_reason = NULL, refresh_blocked_reason = NULL, sources_updated_count = 0
                 WHERE id = ?1 AND stale_reason IS NOT NULL
                   AND COALESCE(source_revision, 0) = ?2",
                libsql::params![page_id, source_revision],
            )
            .await
            .map_err(|e| WenlanError::VectorDb(format!("clear_page_staleness_at_revision: {e}")))?;
        Ok(affected == 1)
    }
}
