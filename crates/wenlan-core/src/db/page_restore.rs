// SPDX-License-Identifier: Apache-2.0
use super::{
    page_rename::validate_page_title, CutoverFence, CutoverPhase, MemoryDB, TRUTH_CUTOVER_FENCE_KEY,
};
use crate::{read_scope::ReadScope, WenlanError};
use wenlan_types::responses::{PageProjectionStatus, RestorePageErrorCode, RestorePageResponse};

/// A restore either failed for an ordinary reason (not found, storage, the
/// truth-cutover fence) or was refused for a reason the caller can act on.
/// Refusals carry a typed code so a client never has to read English text.
#[derive(Debug, thiserror::Error)]
pub enum RestorePageError {
    #[error("{}", restore_refusal_message(*.code))]
    Refused {
        code: RestorePageErrorCode,
        /// The page's version when the refusal was decided, so the caller can
        /// reload before trying again.
        current_version: i64,
    },
    #[error(transparent)]
    Failed(#[from] WenlanError),
}

/// The English text sent next to each refusal code. Callers switch on the
/// code; this text is for logs and command-line users.
pub fn restore_refusal_message(code: RestorePageErrorCode) -> &'static str {
    match code {
        RestorePageErrorCode::PageVersionConflict => "page version conflict",
        RestorePageErrorCode::PageTitleTaken => "page title already exists",
        RestorePageErrorCode::PageNotArchived => "page is no longer archived",
        RestorePageErrorCode::PageTitleFixed => {
            "source and imported pages keep their original title"
        }
        RestorePageErrorCode::PageNotRestorable => "only archived ordinary pages can be restored",
    }
}

fn storage(error: libsql::Error) -> RestorePageError {
    WenlanError::VectorDb(format!("restore page: {error}")).into()
}

fn refused(code: RestorePageErrorCode, current_version: i64) -> RestorePageError {
    RestorePageError::Refused {
        code,
        current_version,
    }
}

struct ArchivedRow {
    title: String,
    version: i64,
    status: String,
    space: Option<String>,
    workspace: Option<String>,
    kind: String,
    creation_kind: String,
    changelog: String,
}

/// A committed restore, plus the title the page had while archived when the
/// restore gave it a new one. `previous_title` is `None` when the title did
/// not change (or the restore was a replay that wrote nothing).
pub(crate) struct PageRestore {
    pub(crate) response: RestorePageResponse,
    pub(crate) previous_title: Option<String>,
}

impl MemoryDB {
    /// Restore the same archived page. Sources, body, embedding, scope and
    /// existing links are not rewritten; this does not undo an earlier merge.
    ///
    /// Archiving an ordinary page only flips `status`, bumps `version` and
    /// appends an `archive` history row; search, links, maps and the page list
    /// all read `status = 'active'`. Setting the row active again is therefore
    /// the whole reversal. The projected Markdown file is the route's job.
    ///
    /// This restores the page only. `post_write::restore_page` is the
    /// capability the route calls; under a new title it also updates other
    /// pages' links, as a rename does.
    pub async fn restore_page(
        &self,
        id: &str,
        title: Option<&str>,
        expected_version: i64,
        scope: &ReadScope,
    ) -> Result<RestorePageResponse, RestorePageError> {
        self.commit_page_restore(id, title, expected_version, scope)
            .await
            .map(|restored| restored.response)
    }

    pub(crate) async fn commit_page_restore(
        &self,
        id: &str,
        title: Option<&str>,
        expected_version: i64,
        scope: &ReadScope,
    ) -> Result<PageRestore, RestorePageError> {
        if expected_version < 1 {
            return Err(
                WenlanError::Validation("expected_version must be at least 1".into()).into(),
            );
        }
        let requested_title = title.map(validate_page_title).transpose()?;
        self.get_page_scoped(id, scope)
            .await?
            .ok_or_else(|| WenlanError::NotFound("page not found".into()))?;
        let conn = self.conn.lock().await;
        conn.execute("BEGIN IMMEDIATE", ())
            .await
            .map_err(|error| WenlanError::VectorDb(format!("restore page begin: {error}")))?;
        let result = Self::restore_page_in_transaction(
            &conn,
            id,
            requested_title.as_deref(),
            expected_version,
            scope,
        )
        .await;
        match result {
            Ok(restored) => {
                super::commit_or_rollback(&conn).await.map_err(|error| {
                    WenlanError::VectorDb(format!("restore page commit: {error}"))
                })?;
                Ok(restored)
            }
            Err(error) => {
                conn.execute("ROLLBACK", ()).await.map_err(|rollback| {
                    WenlanError::VectorDb(format!(
                        "restore page rollback: {rollback}; original: {error}"
                    ))
                })?;
                Err(error)
            }
        }
    }

    async fn restore_page_in_transaction(
        conn: &libsql::Connection,
        id: &str,
        requested_title: Option<&str>,
        expected_version: i64,
        scope: &ReadScope,
    ) -> Result<PageRestore, RestorePageError> {
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
        let row = Self::archived_row(conn, id).await?;
        if !scope.matches(row.workspace.as_deref()) {
            return Err(WenlanError::NotFound("page not found".into()).into());
        }
        if row.kind == "entity" || row.creation_kind == "entity" || row.status == "draft" {
            return Err(refused(
                RestorePageErrorCode::PageNotRestorable,
                row.version,
            ));
        }
        if row.version != expected_version {
            return Err(refused(
                RestorePageErrorCode::PageVersionConflict,
                row.version,
            ));
        }
        let title = requested_title.unwrap_or(&row.title).to_string();
        if row.status == "active" && title == row.title {
            // A replay of a restore that already landed: same answer, no write.
            return Ok(PageRestore {
                response: RestorePageResponse {
                    id: id.into(),
                    title: row.title,
                    version: row.version,
                    status: row.status,
                    projection_status: PageProjectionStatus::NotConfigured,
                    projection_error: None,
                    links_updated: 0,
                    links_pending_review: 0,
                    links_not_updated: 0,
                },
                previous_title: None,
            });
        }
        if row.status != "archived" {
            return Err(refused(RestorePageErrorCode::PageNotArchived, row.version));
        }
        if title != row.title && matches!(row.creation_kind.as_str(), "source" | "imported") {
            return Err(refused(RestorePageErrorCode::PageTitleFixed, row.version));
        }
        // Titles are unique per Space among active pages; an unfiled page
        // shares the unfiled Space's namespace. The other page's identity is
        // never named in the refusal.
        let wanted = Self::page_title_key(&title);
        let mut collisions = conn
            .query(
                "SELECT title FROM pages
                 WHERE status='active' AND id<>?1
                   AND space=COALESCE(?2,'00000000-0000-4000-8000-000000000001')",
                libsql::params![id, row.space.clone()],
            )
            .await
            .map_err(storage)?;
        while let Some(other) = collisions.next().await.map_err(storage)? {
            if Self::page_title_key(&other.get::<String>(0).map_err(storage)?) == wanted {
                return Err(refused(RestorePageErrorCode::PageTitleTaken, row.version));
            }
        }
        drop(collisions);
        let now = chrono::Utc::now();
        let next_version = row.version + 1;
        let entry = serde_json::json!({
            "version": next_version,
            "at": now.timestamp(),
            "edited_by": "restore",
            "delta_summary": "Restored from archive",
        });
        let changelog = super::append_changelog_entry(&row.changelog, entry, 20)?;
        let changed = conn
            .execute(
                "UPDATE pages
                 SET status='active',title=?1,kind=?2,version=version+1,last_modified=?3,changelog=?4
                 WHERE id=?5 AND version=?6 AND status='archived' AND workspace IS ?7 AND space IS ?8",
                libsql::params![
                    title.as_str(),
                    crate::pages::page_kind_for(&title, &row.creation_kind, "active"),
                    now.to_rfc3339(),
                    changelog,
                    id,
                    expected_version,
                    row.workspace,
                    row.space
                ],
            )
            .await
            .map_err(storage)?;
        if changed != 1 {
            return Err(refused(
                RestorePageErrorCode::PageVersionConflict,
                row.version,
            ));
        }
        // Existing status/version triggers requeue derivation. Old claims
        // and human approvals are not copied onto this new version.
        Self::append_page_history(conn, id, "restore", now.timestamp())
            .await
            .map_err(storage)?;
        let previous_title = (title != row.title).then_some(row.title);
        Ok(PageRestore {
            response: RestorePageResponse {
                id: id.into(),
                title,
                version: next_version,
                status: "active".into(),
                projection_status: PageProjectionStatus::NotConfigured,
                projection_error: None,
                links_updated: 0,
                links_pending_review: 0,
                links_not_updated: 0,
            },
            previous_title,
        })
    }

    async fn archived_row(
        conn: &libsql::Connection,
        id: &str,
    ) -> Result<ArchivedRow, RestorePageError> {
        let mut rows = conn
            .query(
                "SELECT title,version,status,space,workspace,COALESCE(kind,'concept'),
                        COALESCE(creation_kind,'distilled'),COALESCE(changelog,'[]')
                 FROM pages WHERE id=?1",
                libsql::params![id],
            )
            .await
            .map_err(storage)?;
        let row = rows
            .next()
            .await
            .map_err(storage)?
            .ok_or_else(|| WenlanError::NotFound("page not found".into()))?;
        Ok(ArchivedRow {
            title: row.get(0).map_err(storage)?,
            version: row.get(1).map_err(storage)?,
            status: row.get(2).map_err(storage)?,
            space: row.get(3).map_err(storage)?,
            workspace: row.get(4).map_err(storage)?,
            kind: row.get(5).map_err(storage)?,
            creation_kind: row.get(6).map_err(storage)?,
            changelog: row.get(7).map_err(storage)?,
        })
    }
}
