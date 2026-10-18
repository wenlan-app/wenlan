// SPDX-License-Identifier: Apache-2.0

use super::MemoryDB;
use crate::{error::WenlanError, pages::Page, read_scope::ReadScope};
use std::path::Path;
use wenlan_types::responses::RenamePageResponse;

const PAGE_TITLE_MAX_CHARS: usize = 500;
const PAGE_TITLE_RENAME_HISTORY_WRITER: &str = "title_rename";
const DEFAULT_CHANGELOG_CAP: usize = 20;

#[cfg(test)]
tokio::task_local! {
    static FAIL_AFTER_PROJECTION_WRITE: bool;
}

#[cfg(test)]
pub(crate) async fn with_failure_after_projection_write<T>(
    future: impl std::future::Future<Output = T>,
) -> T {
    FAIL_AFTER_PROJECTION_WRITE.scope(true, future).await
}

#[cfg(test)]
fn fail_after_projection_write() -> Result<(), WenlanError> {
    if FAIL_AFTER_PROJECTION_WRITE
        .try_with(|fail| *fail)
        .unwrap_or(false)
    {
        Err(WenlanError::VectorDb(
            "injected failure after projection write".to_string(),
        ))
    } else {
        Ok(())
    }
}

impl MemoryDB {
    /// Rename an ordinary active page with a version fence. If its Markdown
    /// projection already exists, update that same pinned filename atomically
    /// with the database write. Embeddings use only an already-loaded model;
    /// this operation never initializes or downloads one.
    pub async fn rename_page(
        &self,
        id: &str,
        title: &str,
        expected_version: i64,
        scope: &ReadScope,
        knowledge_path: Option<&Path>,
    ) -> Result<RenamePageResponse, WenlanError> {
        let title = validate_page_title(title)?;
        if expected_version < 1 {
            return Err(WenlanError::Validation(
                "expected_version must be at least 1".to_string(),
            ));
        }

        // Resolve through the same workspace scope as the public Page read so
        // an inaccessible id is indistinguishable from a missing one. The
        // transaction below repeats the binding/version checks before write.
        let visible = self
            .get_page_scoped(id, scope)
            .await?
            .ok_or_else(|| WenlanError::NotFound("page not found".to_string()))?;
        if visible.version != expected_version {
            return Err(WenlanError::Conflict("page version conflict".to_string()));
        }
        if visible.status != "active"
            || visible.kind == "entity"
            || matches!(
                visible.creation_kind.as_str(),
                "entity" | "source" | "imported"
            )
        {
            return Err(WenlanError::Validation(
                "only active knowledge pages can be renamed".to_string(),
            ));
        }
        // The loaded engine is the only embedding path used here. It never
        // initializes the model or downloads weights. If this process has no
        // loaded model, preserve the existing vector so the unchanged body and
        // summary remain semantically searchable; its title contribution will
        // remain stale until a later normal content-indexing write.
        let embedding_sql = if self.embedder.is_some() && visible.title != title {
            let embedding_text = crate::pages::page_embedding_text(
                &title,
                visible.summary.as_deref(),
                &visible.content,
            );
            match self
                .generate_embeddings_blocking(vec![embedding_text])
                .await
            {
                Ok(embeddings) => embeddings
                    .first()
                    .map(|embedding| Self::vec_to_sql(embedding)),
                Err(error) => {
                    log::warn!("[page_rename] embedding refresh unavailable for {id}: {error}");
                    None
                }
            }
        } else {
            None
        };

        let fence = self.cutover_fence().await?;
        if fence.phase == crate::db::CutoverPhase::Preparing {
            return Err(WenlanError::Conflict(
                "page writes are fenced during truth cutover".to_string(),
            ));
        }
        let permit = crate::truth_adapter::page_write_permit(self, id)
            .await?
            .ok_or_else(|| {
                WenlanError::Conflict(
                    "page_review_required: review this page before renaming".to_string(),
                )
            })?;
        // Missing roots are valid DB-only installation states. Existing
        // projections are writable only after the automatic-reader permit.
        let projection = if visible.title == title {
            None
        } else if let Some(path) = knowledge_path {
            match std::fs::symlink_metadata(path) {
                Ok(_) => Some(
                    crate::export::knowledge::KnowledgeProjectionWrite::begin_owned_repair_session(
                        path.to_path_buf(),
                        self,
                    )?,
                ),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => {
                    return Err(WenlanError::VectorDb(format!(
                        "rename page projection root metadata: {error}"
                    )))
                }
            }
        } else {
            None
        };

        let conn = self.conn.lock().await;
        conn.execute("BEGIN IMMEDIATE", ())
            .await
            .map_err(|error| WenlanError::VectorDb(format!("rename page begin: {error}")))?;
        let mut projection_before: Option<(
            String,
            Vec<wenlan_types::repair::RepairRollbackFileEntry>,
        )> = None;
        let mut projection_written = false;
        let result = async {
            let mut rows = conn
                .query(
                    "SELECT id,title,version,status,space,COALESCE(kind,'concept'),
                            COALESCE(creation_kind,'distilled'),workspace,content
                       FROM pages WHERE id=?1 LIMIT 2",
                    libsql::params![id],
                )
                .await
                .map_err(|error| WenlanError::VectorDb(format!("rename page target: {error}")))?;
            let row = rows
                .next()
                .await
                .map_err(|error| WenlanError::VectorDb(format!("rename page row: {error}")))?
                .ok_or_else(|| WenlanError::NotFound("page not found".to_string()))?;
            let current_title = row
                .get::<String>(1)
                .map_err(|error| WenlanError::VectorDb(format!("rename page title: {error}")))?;
            let current_version = row
                .get::<i64>(2)
                .map_err(|error| WenlanError::VectorDb(format!("rename page version: {error}")))?;
            let status = row
                .get::<String>(3)
                .map_err(|error| WenlanError::VectorDb(format!("rename page status: {error}")))?;
            let space = row
                .get::<Option<String>>(4)
                .map_err(|error| WenlanError::VectorDb(format!("rename page scope: {error}")))?;
            let kind = row
                .get::<String>(5)
                .map_err(|error| WenlanError::VectorDb(format!("rename page kind: {error}")))?;
            let creation_kind = row.get::<String>(6).map_err(|error| {
                WenlanError::VectorDb(format!("rename page creation kind: {error}"))
            })?;
            let workspace = row.get::<Option<String>>(7).map_err(|error| {
                WenlanError::VectorDb(format!("rename page workspace: {error}"))
            })?;
            let current_content = row.get::<String>(8).map_err(|error| {
                WenlanError::VectorDb(format!("rename page content: {error}"))
            })?;
            if rows
                .next()
                .await
                .map_err(|error| WenlanError::VectorDb(format!("rename page duplicate: {error}")))?
                .is_some()
            {
                return Err(WenlanError::Conflict(
                    "page identity is ambiguous".to_string(),
                ));
            }
            drop(rows);

            if !scope.matches(workspace.as_deref()) {
                return Err(WenlanError::NotFound("page not found".to_string()));
            }
            if status != "active"
                || kind == "entity"
                || matches!(creation_kind.as_str(), "entity" | "source" | "imported")
            {
                return Err(WenlanError::Validation(
                    "only active knowledge pages can be renamed".to_string(),
                ));
            }
            if current_version != expected_version {
                return Err(WenlanError::Conflict("page version conflict".to_string()));
            }
            if current_title == title {
                return Ok(RenamePageResponse {
                    id: id.to_string(),
                    title: current_title,
                    version: current_version,
                });
            }

            let review_before = snapshot_page_review(&conn, id).await?;

            if let Some(projection) = projection.as_ref() {
                let current_page = page_for_title_rename_on_connection(&conn, id).await?;
                projection_before = projection
                    .locked()
                    .capture_checked_page_projection(&current_page)?;
            }

            let wanted_key = MemoryDB::page_title_key(&title);
            let mut collision_rows = conn
                .query(
                    "SELECT id,title FROM pages
                      WHERE status='active' AND id<>?1
                        AND space=COALESCE(?2,'00000000-0000-4000-8000-000000000001')",
                    libsql::params![id, space.clone()],
                )
                .await
                .map_err(|error| {
                    WenlanError::VectorDb(format!("rename page collisions: {error}"))
                })?;
            while let Some(collision) = collision_rows.next().await.map_err(|error| {
                WenlanError::VectorDb(format!("rename page collision row: {error}"))
            })? {
                let other_title = collision.get::<String>(1).map_err(|error| {
                    WenlanError::VectorDb(format!("rename page collision title: {error}"))
                })?;
                if MemoryDB::page_title_key(&other_title) == wanted_key {
                    return Err(WenlanError::Conflict(
                        "page title already exists".to_string(),
                    ));
                }
            }
            drop(collision_rows);

            let now = chrono::Utc::now().to_rfc3339();
            let new_version = current_version + 1;
            let new_kind = crate::pages::page_kind_for(&title, &creation_kind, &status);
            let entry = serde_json::json!({
                "version": new_version,
                "at": chrono::Utc::now().timestamp(),
                "edited_by": PAGE_TITLE_RENAME_HISTORY_WRITER,
                "delta_summary": format!("Title changed from ‘{current_title}’ to ‘{title}’"),
            });
            let old_changelog: String = conn
                .query(
                    "SELECT COALESCE(changelog,'[]') FROM pages WHERE id=?1",
                    libsql::params![id],
                )
                .await
                .map_err(|error| WenlanError::VectorDb(format!("rename page changelog: {error}")))?
                .next()
                .await
                .map_err(|error| {
                    WenlanError::VectorDb(format!("rename page changelog row: {error}"))
                })?
                .ok_or_else(|| WenlanError::NotFound("page not found".to_string()))?
                .get(0)
                .map_err(|error| {
                    WenlanError::VectorDb(format!("rename page changelog value: {error}"))
                })?;
            let changelog =
                crate::db::append_changelog_entry(&old_changelog, entry, DEFAULT_CHANGELOG_CAP)?;
            let changed = conn
                .execute(
                    "UPDATE pages SET title=?1,version=version+1,last_modified=?2,kind=?3,
                                      embedding=CASE WHEN ?4 IS NULL THEN embedding ELSE vector32(?4) END,
                                      changelog=?5
                      WHERE id=?6 AND version=?7 AND status='active' AND space IS ?8",
                    libsql::params![
                        title.as_str(),
                        now,
                        new_kind,
                        embedding_sql,
                        changelog,
                        id,
                        expected_version,
                        space
                    ],
                )
                .await
                .map_err(|error| WenlanError::VectorDb(format!("rename page update: {error}")))?;
            if changed != 1 {
                return Err(WenlanError::Conflict("page version conflict".to_string()));
            }
            // The title-only version still needs the normal machine derivation
            // run: its version-scoped claims and judgment attempts are not
            // copied. The page-update trigger leaves the new job pending.
            MemoryDB::append_page_history(
                &conn,
                id,
                PAGE_TITLE_RENAME_HISTORY_WRITER,
                chrono::Utc::now().timestamp(),
            )
            .await
            .map_err(|error| WenlanError::VectorDb(format!("rename page history: {error}")))?;

            let current_digest = crate::provenance::revision_content_digest(&current_content);
            // Human approval is body-bound. Advance only an approval already
            // recorded for this exact version and body; keep its existing
            // presence receipt untouched and never manufacture a review.
            let carries_review = review_before.as_ref().is_some_and(|review| {
                review.human_reviewed == 1
                    && review.reviewed_page_version == Some(expected_version)
                    && review.reviewed_page_digest.as_deref() == Some(current_digest.as_str())
            });
            if carries_review {
                conn.execute(
                    "UPDATE page_truth_state SET reviewed_page_version=?1
                      WHERE page_id=?2 AND human_reviewed=1
                        AND reviewed_page_version=?3 AND reviewed_page_digest=?4",
                    libsql::params![
                        new_version,
                        id,
                        expected_version,
                        current_digest
                    ],
                )
                .await
                .map_err(|error| {
                    WenlanError::VectorDb(format!("rename page review carry: {error}"))
                })?;
            }

            if projection_before.is_some() {
                let page = page_for_title_rename_on_connection(&conn, id).await?;
                let latest_projection = projection
                    .as_ref()
                    .expect("projection snapshot requires a session")
                    .locked()
                    .capture_rename_page_projection(id)?;
                if projection_before.as_ref() != Some(&latest_projection) {
                    return Err(WenlanError::Conflict(
                        "page projection changed during rename".to_string(),
                    ));
                }
                projection_written = true;
                let path = projection
                    .as_ref()
                    .expect("projection snapshot requires a session")
                    .locked()
                    .write_page_permitted(&permit, &page)?;
                let expected_file = projection_before
                    .as_ref()
                    .and_then(|(path, _)| Path::new(path).file_name())
                    .and_then(|name| name.to_str());
                if Path::new(&path).file_name().and_then(|name| name.to_str()) != expected_file {
                    return Err(WenlanError::Conflict(
                        "page projection filename changed during rename".to_string(),
                    ));
                }
                #[cfg(test)]
                fail_after_projection_write()?;
            }
            Ok(RenamePageResponse {
                id: id.to_string(),
                title,
                version: new_version,
            })
        }
        .await;

        match result {
            Ok(response) => {
                if let Err(error) = conn.execute("COMMIT", ()).await {
                    let projection_error = restore_projection(
                        projection.as_ref(),
                        projection_before.as_ref(),
                        projection_written,
                    );
                    let rollback_error = conn.execute("ROLLBACK", ()).await.err();
                    return if projection_error.is_none() && rollback_error.is_none() {
                        Err(WenlanError::VectorDb(format!(
                            "rename page commit: {error}"
                        )))
                    } else {
                        Err(WenlanError::Conflict(
                            "rename recovery required".to_string(),
                        ))
                    };
                }
                if let Some(projection) = projection.as_ref() {
                    if let Err(error) = projection.locked().refresh_index() {
                        log::warn!(
                            "[page_rename] generated index refresh failed for {id}: {error}"
                        );
                    }
                }
                Ok(response)
            }
            Err(error) => {
                let projection_error = restore_projection(
                    projection.as_ref(),
                    projection_before.as_ref(),
                    projection_written,
                );
                let rollback_error = conn.execute("ROLLBACK", ()).await.err();
                if projection_error.is_some() || rollback_error.is_some() {
                    Err(WenlanError::Conflict(
                        "rename recovery required".to_string(),
                    ))
                } else {
                    Err(error)
                }
            }
        }
    }
}

struct PageReviewSnapshot {
    human_reviewed: i64,
    reviewed_page_version: Option<i64>,
    reviewed_page_digest: Option<String>,
}

async fn snapshot_page_review(
    conn: &libsql::Connection,
    page_id: &str,
) -> Result<Option<PageReviewSnapshot>, WenlanError> {
    let mut rows = conn
        .query(
            "SELECT human_reviewed,reviewed_page_version,reviewed_page_digest
               FROM page_truth_state WHERE page_id=?1",
            libsql::params![page_id],
        )
        .await
        .map_err(|error| WenlanError::VectorDb(format!("rename truth snapshot: {error}")))?;
    let Some(row) = rows
        .next()
        .await
        .map_err(|error| WenlanError::VectorDb(format!("rename truth snapshot row: {error}")))?
    else {
        return Ok(None);
    };
    Ok(Some(PageReviewSnapshot {
        human_reviewed: row.get(0).map_err(|error| {
            WenlanError::VectorDb(format!("rename review snapshot flag: {error}"))
        })?,
        reviewed_page_version: row.get(1).map_err(|error| {
            WenlanError::VectorDb(format!("rename review snapshot version: {error}"))
        })?,
        reviewed_page_digest: row.get(2).map_err(|error| {
            WenlanError::VectorDb(format!("rename review snapshot digest: {error}"))
        })?,
    }))
}

fn validate_page_title(title: &str) -> Result<String, WenlanError> {
    let trimmed = title.trim();
    let count = trimmed.chars().count();
    if count == 0 || count > PAGE_TITLE_MAX_CHARS || trimmed.chars().any(char::is_control) {
        return Err(WenlanError::Validation(
            "title must be a non-empty single line of at most 500 characters".to_string(),
        ));
    }
    Ok(trimmed.to_string())
}

fn restore_projection(
    projection: Option<&crate::export::knowledge::OwnedRepairProjectionSession>,
    before: Option<&(String, Vec<wenlan_types::repair::RepairRollbackFileEntry>)>,
    written: bool,
) -> Option<WenlanError> {
    if !written {
        return None;
    }
    let (path, entries) = before?;
    projection?
        .locked()
        .restore_rename_page_projection(path, entries)
        .err()
}

async fn page_for_title_rename_on_connection(
    connection: &libsql::Connection,
    page_id: &str,
) -> Result<Page, WenlanError> {
    let entity_id_column = crate::db::page_entity_id_column("");
    let mut rows = connection
        .query(
            &format!(
                "SELECT id,title,summary,content,{entity_id_column},space,source_memory_ids,
                        version,status,created_at,last_compiled,last_modified,
                        COALESCE(sources_updated_count,0),stale_reason,COALESCE(user_edited,0),
                        COALESCE(changelog,'[]'),COALESCE(creation_kind,'distilled'),
                        COALESCE(review_status,'confirmed'),workspace,citations,
                        COALESCE(kind,'concept'),refresh_blocked_reason,COALESCE(review_required,0)
                   FROM pages WHERE id=?1 AND COALESCE(kind,'concept')!='entity'"
            ),
            libsql::params![page_id],
        )
        .await
        .map_err(|error| WenlanError::VectorDb(format!("rename page projection read: {error}")))?;
    let row = rows
        .next()
        .await
        .map_err(|error| WenlanError::VectorDb(format!("rename page projection row: {error}")))?
        .ok_or_else(|| WenlanError::NotFound("page not found".to_string()))?;
    MemoryDB::row_to_page(&row)
}
