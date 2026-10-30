// SPDX-License-Identifier: Apache-2.0
use super::{
    page_rename::validate_page_title, CutoverFence, CutoverPhase, MemoryDB, TRUTH_CUTOVER_FENCE_KEY,
};
use crate::{read_scope::ReadScope, WenlanError};
use wenlan_types::responses::{PageProjectionStatus, RestorePageResponse};

fn storage(error: libsql::Error) -> WenlanError {
    WenlanError::VectorDb(format!("restore page: {error}"))
}

impl MemoryDB {
    /// Restore the same archived page. Sources, body, embedding, scope and
    /// existing links are not rewritten; this does not undo an earlier merge.
    pub async fn restore_page(
        &self,
        id: &str,
        title: Option<&str>,
        expected_version: i64,
        scope: &ReadScope,
    ) -> Result<RestorePageResponse, WenlanError> {
        if expected_version < 1 {
            return Err(WenlanError::Validation(
                "expected_version must be at least 1".into(),
            ));
        }
        let requested_title = title.map(validate_page_title).transpose()?;
        self.get_page_scoped(id, scope)
            .await?
            .ok_or_else(|| WenlanError::NotFound("page not found".into()))?;
        let conn = self.conn.lock().await;
        conn.execute("BEGIN IMMEDIATE", ())
            .await
            .map_err(|error| WenlanError::VectorDb(format!("restore page begin: {error}")))?;
        let result = async {
            // Read the durable fence under the same write transaction so an
            // intervening cutover cannot be missed between a check and write.
            let raw_fence = conn.query("SELECT value FROM app_metadata WHERE key=?1", libsql::params![TRUTH_CUTOVER_FENCE_KEY]).await.map_err(storage)?
                .next().await.map_err(storage)?.map(|row| row.get::<String>(0)).transpose().map_err(storage)?;
            let fence = match raw_fence {
                Some(raw) => CutoverFence::parse(raw.trim()).ok_or_else(|| WenlanError::VectorDb("cutover fence is unreadable".into()))?,
                None => CutoverFence::INITIAL,
            };
            if fence.phase == CutoverPhase::Preparing { return Err(WenlanError::Conflict("page writes are fenced during truth cutover".into())); }
            let mut rows = conn.query("SELECT title,version,status,space,workspace,COALESCE(kind,'concept'),COALESCE(creation_kind,'distilled'),COALESCE(changelog,'[]') FROM pages WHERE id=?1", libsql::params![id]).await.map_err(storage)?;
            let row = rows.next().await.map_err(storage)?.ok_or_else(|| WenlanError::NotFound("page not found".into()))?;
            let current_title: String = row.get(0).map_err(storage)?;
            let version: i64 = row.get(1).map_err(storage)?;
            let status: String = row.get(2).map_err(storage)?;
            let space: Option<String> = row.get(3).map_err(storage)?;
            let workspace: Option<String> = row.get(4).map_err(storage)?;
            let kind: String = row.get(5).map_err(storage)?;
            let creation_kind: String = row.get(6).map_err(storage)?;
            let changelog: String = row.get(7).map_err(storage)?;
            drop(rows);
            if !scope.matches(workspace.as_deref()) { return Err(WenlanError::NotFound("page not found".into())); }
            if kind == "entity" || creation_kind == "entity" || status == "draft" { return Err(WenlanError::Validation("only archived ordinary pages can be restored".into())); }
            if version != expected_version { return Err(WenlanError::Conflict("page version conflict".into())); }
            let title = requested_title.as_ref().unwrap_or(&current_title);
            if status == "active" && title == &current_title {
                return Ok(RestorePageResponse { id: id.into(), title: current_title, version, status, projection_status: PageProjectionStatus::NotConfigured, projection_error: None });
            }
            if status != "archived" { return Err(WenlanError::Conflict("page is no longer archived".into())); }
            if title != &current_title && matches!(creation_kind.as_str(), "source" | "imported") {
                return Err(WenlanError::Validation("source and imported notes must retain their original title".into()));
            }
            let wanted = Self::page_title_key(title);
            let mut collisions = conn.query("SELECT title FROM pages WHERE status='active' AND id<>?1 AND space=COALESCE(?2,'00000000-0000-4000-8000-000000000001')", libsql::params![id, space.clone()]).await.map_err(storage)?;
            while let Some(other) = collisions.next().await.map_err(storage)? {
                if Self::page_title_key(&other.get::<String>(0).map_err(storage)?) == wanted { return Err(WenlanError::Conflict("page title already exists".into())); }
            }
            drop(collisions);
            let now = chrono::Utc::now();
            let next_version = version + 1;
            let entry = serde_json::json!({"version":next_version,"at":now.timestamp(),"edited_by":"restore","delta_summary":"Restored from archive"});
            let changelog = super::append_changelog_entry(&changelog, entry, 20)?;
            let changed = conn.execute("UPDATE pages SET status='active',title=?1,kind=?2,version=version+1,last_modified=?3,changelog=?4 WHERE id=?5 AND version=?6 AND status='archived' AND workspace IS ?7 AND space IS ?8", libsql::params![title.as_str(),crate::pages::page_kind_for(title,&creation_kind,"active"),now.to_rfc3339(),changelog,id,expected_version,workspace,space]).await.map_err(storage)?;
            if changed != 1 { return Err(WenlanError::Conflict("page version conflict".into())); }
            // Existing status/version triggers requeue derivation. Old claims
            // and human approvals are not copied onto this new version.
            Self::append_page_history(&conn, id, "restore", now.timestamp()).await.map_err(storage)?;
            Ok(RestorePageResponse { id: id.into(), title: title.clone(), version: next_version, status: "active".into(), projection_status: PageProjectionStatus::NotConfigured, projection_error: None })
        }.await;
        match result {
            Ok(response) => {
                super::commit_or_rollback(&conn).await.map_err(|error| {
                    WenlanError::VectorDb(format!("restore page commit: {error}"))
                })?;
                Ok(response)
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
}
