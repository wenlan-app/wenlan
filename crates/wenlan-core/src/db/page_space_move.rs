// SPDX-License-Identifier: Apache-2.0
//! Move one ordinary page into another Space, or into no Space.
//!
//! Pages written before Spaces existed sit in no Space while the notes they
//! cite have since moved into one, so they can neither cite those notes nor be
//! refreshed. This is the one write that changes a page's Space. Its sources
//! and body are not rewritten; the move is a history entry the person can see
//! and undo by moving the page back.

use super::{
    citation_sources::read_on, CutoverFence, CutoverPhase, MemoryDB, TRUTH_CUTOVER_FENCE_KEY,
    UNFILED_SPACE_ID,
};
use crate::{read_scope::ReadScope, WenlanError};
use wenlan_types::responses::{PageProjectionStatus, SetPageSpaceErrorCode, SetPageSpaceResponse};

/// The page-history writer for a Space move.
pub(crate) const PAGE_SPACE_MOVE_HISTORY_WRITER: &str = "space_move";

/// A Space move either failed for an ordinary reason (not found, storage, the
/// truth-cutover fence) or was refused for a reason the caller can act on.
/// Refusals carry a typed code so a client never has to read English text.
#[derive(Debug, thiserror::Error)]
pub enum SetPageSpaceError {
    #[error("{}", set_page_space_refusal_message(*.code))]
    Refused {
        code: SetPageSpaceErrorCode,
        /// The page's version when the refusal was decided, so the caller can
        /// reload before trying again. `None` when the refusal came before the
        /// page was read (a Space-locked caller).
        current_version: Option<i64>,
    },
    #[error(transparent)]
    Failed(#[from] WenlanError),
}

/// The English text sent next to each refusal code. Callers switch on the
/// code; this text is for logs and command-line users.
pub fn set_page_space_refusal_message(code: SetPageSpaceErrorCode) -> &'static str {
    match code {
        SetPageSpaceErrorCode::PageSpaceVersionConflict => "page version conflict",
        SetPageSpaceErrorCode::PageSpaceTitleTaken => {
            "a page with this title already exists in the target Space"
        }
        SetPageSpaceErrorCode::PageSpaceNotMovable => {
            "only active ordinary notes move between Spaces; drafts, source, overview and \
             entity pages do not"
        }
        SetPageSpaceErrorCode::PageSpaceUnknown => "the target Space is not registered",
        SetPageSpaceErrorCode::PageSpaceNeedsGlobalScope => {
            "moving a page between Spaces needs a caller that is not locked to one Space"
        }
    }
}

fn storage(error: libsql::Error) -> SetPageSpaceError {
    WenlanError::VectorDb(format!("set page space: {error}")).into()
}

fn refused(code: SetPageSpaceErrorCode, current_version: Option<i64>) -> SetPageSpaceError {
    SetPageSpaceError::Refused {
        code,
        current_version,
    }
}

struct MovableRow {
    title: String,
    version: i64,
    status: String,
    space: String,
    kind: String,
    creation_kind: String,
    changelog: String,
    source_memory_ids: Vec<String>,
}

/// The read scope a page in `space` cites under, as a write to it does.
fn citation_scope(space: &str) -> ReadScope {
    if space == UNFILED_SPACE_ID {
        ReadScope::Uncategorized
    } else {
        ReadScope::Space(space.to_string())
    }
}

/// How the move reads on the page's history.
fn space_label(space: &str) -> String {
    if space == UNFILED_SPACE_ID {
        "no Space".to_string()
    } else {
        format!("Space '{space}'")
    }
}

impl MemoryDB {
    /// Every active page's Space by page id (`None` for none), for keeping
    /// each Space's pages in its folder (`space_folders`).
    pub async fn active_page_spaces(
        &self,
    ) -> Result<std::collections::HashMap<String, Option<String>>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query("SELECT id, space FROM pages WHERE status='active'", ())
            .await
            .map_err(|e| WenlanError::VectorDb(format!("page spaces: {e}")))?;
        let mut out = std::collections::HashMap::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| WenlanError::VectorDb(format!("page spaces row: {e}")))?
        {
            let id: String = row
                .get(0)
                .map_err(|e| WenlanError::VectorDb(format!("page id: {e}")))?;
            let space: Option<String> = row
                .get(1)
                .map_err(|e| WenlanError::VectorDb(format!("page space: {e}")))?;
            out.insert(id, space.filter(|s| !s.is_empty()));
        }
        Ok(out)
    }
    /// Move one active ordinary page (kind `concept` or `authored`) into the
    /// registered Space `target`, or into no Space when `target` is `None` or
    /// empty. Only an unlocked caller (`ReadScope::Global`) may move a page
    /// between Spaces.
    ///
    /// One transaction moves `space` and `workspace`, bumps the version,
    /// records a `space_move` history entry (which re-evaluates the page's
    /// basis), re-stamps the page's own edges that its new Space can hold, and
    /// drops the page's Space-scoped community assignment. Sources and body
    /// are untouched. A move to the Space the page is already in writes
    /// nothing and returns the current state.
    ///
    /// `unreadable_source_positions` lists, by 1-based position in the page's
    /// `source_memory_ids`, the sources its new Space cannot read, through the
    /// same read path a write's citation check uses. Positions only: a source
    /// in another Space reads exactly like one that is gone.
    ///
    /// The projected Markdown file is the route's job.
    pub async fn set_page_space(
        &self,
        id: &str,
        target: Option<&str>,
        expected_version: i64,
        scope: &ReadScope,
    ) -> Result<SetPageSpaceResponse, SetPageSpaceError> {
        if !matches!(scope, ReadScope::Global) {
            return Err(refused(
                SetPageSpaceErrorCode::PageSpaceNeedsGlobalScope,
                None,
            ));
        }
        if expected_version < 1 {
            return Err(
                WenlanError::Validation("expected_version must be at least 1".into()).into(),
            );
        }
        let target = target.map(str::trim).filter(|name| !name.is_empty());
        // Held through the commit so a Space rename or delete cannot land
        // between checking the target and moving the page into it.
        let _space_writes = self.lock_space_writes().await;
        let conn = self.conn.lock().await;
        conn.execute("BEGIN IMMEDIATE", ())
            .await
            .map_err(|error| WenlanError::VectorDb(format!("set page space begin: {error}")))?;
        let result = Self::set_page_space_in_transaction(&conn, id, target, expected_version).await;
        match result {
            Ok(response) => {
                super::commit_or_rollback(&conn).await.map_err(|error| {
                    WenlanError::VectorDb(format!("set page space commit: {error}"))
                })?;
                Ok(response)
            }
            Err(error) => {
                conn.execute("ROLLBACK", ()).await.map_err(|rollback| {
                    WenlanError::VectorDb(format!(
                        "set page space rollback: {rollback}; original: {error}"
                    ))
                })?;
                Err(error)
            }
        }
    }

    async fn set_page_space_in_transaction(
        conn: &libsql::Connection,
        id: &str,
        target: Option<&str>,
        expected_version: i64,
    ) -> Result<SetPageSpaceResponse, SetPageSpaceError> {
        // Read the durable fence under the same write transaction so an
        // intervening cutover cannot be missed between a check and write.
        let raw_fence = conn
            .query(
                "SELECT value FROM app_metadata WHERE key=?1",
                libsql::params![TRUTH_CUTOVER_FENCE_KEY],
            )
            .await
            .map_err(storage)?
            .next()
            .await
            .map_err(storage)?
            .map(|row| row.get::<String>(0))
            .transpose()
            .map_err(storage)?;
        let fence = match raw_fence {
            Some(raw) => CutoverFence::parse(raw.trim())
                .ok_or_else(|| WenlanError::VectorDb("cutover fence is unreadable".into()))?,
            None => CutoverFence::INITIAL,
        };
        if fence.phase == CutoverPhase::Preparing {
            return Err(WenlanError::Conflict(
                "page writes are fenced during truth cutover".into(),
            )
            .into());
        }
        let row = Self::movable_row(conn, id).await?;
        let derived = crate::pages::page_kind_for(&row.title, &row.creation_kind, &row.status);
        if row.kind == "entity"
            || row.status != "active"
            || !matches!(derived, "concept" | "authored")
        {
            return Err(refused(
                SetPageSpaceErrorCode::PageSpaceNotMovable,
                Some(row.version),
            ));
        }
        let wanted = match target {
            None => UNFILED_SPACE_ID.to_string(),
            Some(name) => {
                let mut spaces = conn
                    .query("SELECT id FROM spaces WHERE name=?1", libsql::params![name])
                    .await
                    .map_err(storage)?;
                let registered = match spaces.next().await.map_err(storage)? {
                    Some(space) => space.get::<String>(0).map_err(storage)? != UNFILED_SPACE_ID,
                    None => false,
                };
                drop(spaces);
                if !registered || name == UNFILED_SPACE_ID {
                    return Err(refused(
                        SetPageSpaceErrorCode::PageSpaceUnknown,
                        Some(row.version),
                    ));
                }
                name.to_string()
            }
        };
        if row.version != expected_version {
            return Err(refused(
                SetPageSpaceErrorCode::PageSpaceVersionConflict,
                Some(row.version),
            ));
        }
        let target_scope = citation_scope(&wanted);
        if row.space == wanted {
            // Already there: same answer, no write.
            return Ok(SetPageSpaceResponse {
                page_id: id.into(),
                space: target.map(str::to_string),
                version: row.version,
                unreadable_source_positions: Self::unreadable_positions(
                    conn,
                    &row.source_memory_ids,
                    &target_scope,
                )
                .await?,
                projection_status: PageProjectionStatus::NotConfigured,
                projection_error: None,
            });
        }
        // Titles are unique per Space among active pages; an unfiled page
        // shares the unfiled Space's namespace. The other page's identity is
        // never named in the refusal.
        let title_key = Self::page_title_key(&row.title);
        let mut collisions = conn
            .query(
                "SELECT title FROM pages WHERE status='active' AND id<>?1 AND space=?2",
                libsql::params![id, wanted.as_str()],
            )
            .await
            .map_err(storage)?;
        while let Some(other) = collisions.next().await.map_err(storage)? {
            if Self::page_title_key(&other.get::<String>(0).map_err(storage)?) == title_key {
                return Err(refused(
                    SetPageSpaceErrorCode::PageSpaceTitleTaken,
                    Some(row.version),
                ));
            }
        }
        drop(collisions);
        let now = chrono::Utc::now();
        let next_version = row.version + 1;
        let entry = serde_json::json!({
            "version": next_version,
            "at": now.timestamp(),
            "edited_by": PAGE_SPACE_MOVE_HISTORY_WRITER,
            "delta_summary": format!(
                "Moved from {} to {}",
                space_label(&row.space),
                space_label(&wanted)
            ),
        });
        let changelog = super::append_changelog_entry(&row.changelog, entry, 20)?;
        let changed = conn
            .execute(
                "UPDATE pages
                 SET space=?1,workspace=?1,version=version+1,last_modified=?2,changelog=?3
                 WHERE id=?4 AND version=?5 AND status='active' AND COALESCE(space,?7)=?6",
                libsql::params![
                    wanted.as_str(),
                    now.to_rfc3339(),
                    changelog,
                    id,
                    expected_version,
                    row.space.as_str(),
                    UNFILED_SPACE_ID
                ],
            )
            .await
            .map_err(storage)?;
        if changed != 1 {
            return Err(refused(
                SetPageSpaceErrorCode::PageSpaceVersionConflict,
                Some(row.version),
            ));
        }
        // A live edge carries the Space of its source endpoint, and the space
        // fence re-checks both endpoints on every later in-place update. Move
        // the stamp on the page's own edges whose other end the new Space can
        // hold, as a source page's move re-stamps its cites edges. An edge to
        // a note left behind keeps its stamp until the next write of that
        // citation reconciles it, as after a memory's move. The page's
        // claims' edges are retracted by the storage triggers on
        // `pages.space` (`m5_retract_claim_edges_on_page_space_move`).
        conn.execute(
            "UPDATE edges SET space=?1
             WHERE valid_until IS NULL AND space IS NOT ?1
               AND src_kind='page' AND src_id=?2
               AND (lineage='legacy'
                    OR (edge_type='cites' AND dst_kind='external')
                    OR (CASE dst_kind
                          WHEN 'page' THEN (SELECT space FROM pages WHERE id=edges.dst_id)
                          WHEN 'memory' THEN (SELECT space FROM memories WHERE source_id=edges.dst_id)
                          WHEN 'entity' THEN (
                              SELECT p.space FROM entity_page_map epm
                                JOIN pages p ON p.id=epm.page_id
                               WHERE epm.entity_id=edges.dst_id AND p.kind='entity')
                          WHEN 'claim_revision' THEN (
                              SELECT p.space FROM claim_revisions cr
                                JOIN claims c ON c.claim_id=cr.claim_id
                                JOIN pages p ON p.id=c.page_id
                               WHERE cr.claim_revision_id=edges.dst_id)
                          ELSE NULL
                        END) IS ?1)",
            libsql::params![wanted.as_str(), id],
        )
        .await
        .map_err(storage)?;
        // The page's community belonged to its old Space; the next grouping
        // pass places it in the new one.
        conn.execute(
            "DELETE FROM page_community_assignments WHERE page_id=?1",
            libsql::params![id],
        )
        .await
        .map_err(storage)?;
        // Records the move in history and re-evaluates the page's basis: the
        // notes it relies on, and the notes relying on it, may now sit in
        // different Spaces.
        Self::append_page_history(conn, id, PAGE_SPACE_MOVE_HISTORY_WRITER, now.timestamp())
            .await
            .map_err(storage)?;
        Ok(SetPageSpaceResponse {
            page_id: id.into(),
            space: target.map(str::to_string),
            version: next_version,
            unreadable_source_positions: Self::unreadable_positions(
                conn,
                &row.source_memory_ids,
                &target_scope,
            )
            .await?,
            projection_status: PageProjectionStatus::NotConfigured,
            projection_error: None,
        })
    }

    /// 1-based positions of the `sources` that `scope` cannot read, through
    /// the read a write's citation check uses.
    async fn unreadable_positions(
        conn: &libsql::Connection,
        sources: &[String],
        scope: &ReadScope,
    ) -> Result<Vec<u32>, SetPageSpaceError> {
        let mut positions = Vec::new();
        for (index, locator) in sources.iter().enumerate() {
            // A source over its complete-read limit, or stored malformed, is
            // a read refusal for a write; here it is one more unreadable
            // source, not a failed move.
            let readable = match read_on(conn, locator, scope).await {
                Ok(source) => source.is_some(),
                Err(WenlanError::Validation(_)) => false,
                Err(error) => return Err(error.into()),
            };
            if !readable {
                positions.push(u32::try_from(index + 1).unwrap_or(u32::MAX));
            }
        }
        Ok(positions)
    }

    async fn movable_row(
        conn: &libsql::Connection,
        id: &str,
    ) -> Result<MovableRow, SetPageSpaceError> {
        let mut rows = conn
            .query(
                "SELECT title,version,status,COALESCE(space,?2),COALESCE(kind,'concept'),
                        COALESCE(creation_kind,'distilled'),COALESCE(changelog,'[]'),
                        COALESCE(source_memory_ids,'[]')
                 FROM pages WHERE id=?1",
                libsql::params![id, UNFILED_SPACE_ID],
            )
            .await
            .map_err(storage)?;
        let row = rows
            .next()
            .await
            .map_err(storage)?
            .ok_or_else(|| WenlanError::NotFound("page not found".into()))?;
        let sources: String = row.get(7).map_err(storage)?;
        Ok(MovableRow {
            title: row.get(0).map_err(storage)?,
            version: row.get(1).map_err(storage)?,
            status: row.get(2).map_err(storage)?,
            space: row.get(3).map_err(storage)?,
            kind: row.get(4).map_err(storage)?,
            creation_kind: row.get(5).map_err(storage)?,
            changelog: row.get(6).map_err(storage)?,
            source_memory_ids: serde_json::from_str(&sources).unwrap_or_default(),
        })
    }
}
