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

    /// Active notes not otherwise waiting for steep that hold code basis rows
    /// steep can act on, newest change first, ties by id. Each comes with how
    /// many rows need the note rewritten (`pending` rows a model judged
    /// `stale` for its current version, and rows whose file was deleted) and
    /// how many `pending` rows no model has judged yet (not counting rows
    /// whose lines cannot be found or whose attempts for this change are used
    /// up).
    pub(crate) async fn list_steep_code_pages(&self) -> Result<Vec<(Page, u32, u32)>, WenlanError> {
        use super::code_basis::RECHECK_CURRENT;
        let unresolved = crate::code_basis_recheck::UNRESOLVED_DIGEST;
        let max_attempts = crate::code_basis_recheck::RECHECK_MAX_ATTEMPTS;
        let deleted = crate::code_basis_follow::FILE_DELETED;
        let sql = format!(
            "SELECT c.id, c.title, c.summary, c.content, c.entity_id, c.space, \
                    c.source_memory_ids, c.version, c.status, c.created_at, c.last_compiled, \
                    c.last_modified, COALESCE(c.sources_updated_count, 0), c.stale_reason, \
                    COALESCE(c.user_edited, 0), COALESCE(c.changelog, '[]'), \
                    COALESCE(c.creation_kind, 'distilled'), \
                    COALESCE(c.review_status, 'confirmed'), c.workspace, c.citations, \
                    COALESCE(c.kind, 'concept'), c.refresh_blocked_reason, \
                    COALESCE(c.review_required, 0), \
                    SUM(CASE WHEN b.state = 'pending' AND b.recheck_verdict = 'stale' \
                             AND {RECHECK_CURRENT} THEN 1 \
                        WHEN b.state = 'unavailable' AND b.unavailable_reason = '{deleted}' \
                        THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN b.state <> 'pending' THEN 0 \
                        WHEN COALESCE(b.recheck_digest, '') = '{unresolved}' THEN 0 \
                        WHEN {RECHECK_CURRENT} AND (TRIM(COALESCE(b.recheck_verdict, '')) <> '' \
                             OR b.recheck_attempts >= {max_attempts}) THEN 0 \
                        ELSE 1 END) \
             FROM pages c JOIN page_code_basis b ON b.page_id = c.id \
             WHERE c.status = 'active' AND COALESCE(c.kind, 'concept') != 'entity' \
               AND (c.stale_reason IS NULL OR c.stale_reason NOT IN (?1, ?2)) \
               AND (b.state = 'pending' \
                    OR (b.state = 'unavailable' AND b.unavailable_reason = '{deleted}')) \
             GROUP BY c.id \
             ORDER BY c.last_modified DESC, c.id ASC"
        );
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                &sql,
                libsql::params![STEEP_STALE_REASONS[0], STEEP_STALE_REASONS[1]],
            )
            .await
            .map_err(|e| WenlanError::VectorDb(format!("list_steep_code_pages: {e}")))?;
        let mut out = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| WenlanError::VectorDb(format!("list_steep_code_pages row: {e}")))?
        {
            let count = |index: i32| -> Result<u32, WenlanError> {
                let n = row
                    .get::<Option<i64>>(index)
                    .map_err(|e| WenlanError::VectorDb(format!("list_steep_code_pages: {e}")))?
                    .unwrap_or(0);
                Ok(u32::try_from(n).unwrap_or(0))
            };
            let (stale, unjudged) = (count(23)?, count(24)?);
            if stale + unjudged > 0 {
                out.push((Self::row_to_page(&row)?, stale, unjudged));
            }
        }
        Ok(out)
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
