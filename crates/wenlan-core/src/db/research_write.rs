// SPDX-License-Identifier: Apache-2.0
//! Explicit run fences carried only by external organizing page writes.
use super::research_runs::{key, run, save, source_snapshot, space_name};
use super::MemoryDB;
use crate::{read_scope::ReadScope, WenlanError};
use serde::{Deserialize, Serialize};
use wenlan_types::research::{OrganizeRunState, ResearchCoverage};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResearchWriteFence {
    pub space_id: String,
    pub run_id: String,
    pub revision: u64,
}
impl MemoryDB {
    pub async fn prepare_research_write(
        &self,
        run_id: &str,
        scope: &ReadScope,
        target: Option<&str>,
        sources: &[String],
    ) -> Result<ResearchWriteFence, WenlanError> {
        let ReadScope::Space(name) = scope else {
            return Err(WenlanError::Validation(
                "organizing writes require a registered Space".into(),
            ));
        };
        let space = self
            .get_space(name)
            .await?
            .ok_or_else(|| WenlanError::NotFound("research Space not found".into()))?;
        let conn = self.conn.lock().await;
        let value = run(&conn, &space.id, run_id).await?;
        let fence = ResearchWriteFence {
            space_id: space.id,
            run_id: run_id.into(),
            revision: value.revision,
        };
        guard(&conn, &fence, target, sources, None).await?;
        Ok(fence)
    }
    pub async fn bind_research_unchanged(
        &self,
        fence: &ResearchWriteFence,
        page_id: &str,
        sources: &[String],
        expected_version: i64,
    ) -> Result<(), WenlanError> {
        let conn = self.conn.lock().await;
        conn.execute("BEGIN", ()).await.map_err(sql)?;
        let result = async {
            let mut rows = conn
                .query("SELECT version FROM pages WHERE id=?1", [page_id])
                .await
                .map_err(sql)?;
            let version = rows
                .next()
                .await
                .map_err(sql)?
                .ok_or_else(|| WenlanError::NotFound("research page gone".into()))?
                .get::<i64>(0)
                .map_err(sql)?;
            drop(rows);
            if version != expected_version {
                return Err(WenlanError::Conflict("research page changed".into()));
            }
            bind(&conn, fence, page_id, sources, None, None).await
        }
        .await;
        match result {
            Ok(()) => {
                super::commit_or_rollback(&conn).await.map_err(sql)?;
                Ok(())
            }
            Err(e) => {
                let _ = conn.execute("ROLLBACK", ()).await;
                Err(e)
            }
        }
    }
}
fn sql(e: libsql::Error) -> WenlanError {
    WenlanError::VectorDb(e.to_string())
}
pub(super) async fn guard(
    conn: &libsql::Connection,
    fence: &ResearchWriteFence,
    target: Option<&str>,
    sources: &[String],
    adopting: Option<&str>,
) -> Result<(), WenlanError> {
    let value = run(conn, &fence.space_id, &fence.run_id).await?;
    if value.revision != fence.revision {
        return Err(WenlanError::Conflict(
            "organizing run changed; reread it".into(),
        ));
    }
    if matches!(
        value.state,
        OrganizeRunState::Cancelled | OrganizeRunState::Completed
    ) {
        return Err(WenlanError::Conflict("organizing run is terminal".into()));
    }
    if value.pending_revision_id.as_deref() != adopting {
        return Err(WenlanError::Conflict(
            "organizing run is waiting for another review".into(),
        ));
    }
    if value
        .target_page_id
        .as_deref()
        .is_some_and(|fixed| Some(fixed) != target)
    {
        return Err(WenlanError::Conflict(
            "organizing run target is fixed; refresh that page".into(),
        ));
    }
    if let Some(fixed) = value.target_page_id.as_deref() {
        let expected = value.target_page_incarnation.as_deref().ok_or_else(|| {
            WenlanError::Conflict(
                "legacy organizing target lacks incarnation; begin a new run".into(),
            )
        })?;
        let mut rows = conn
            .query("SELECT incarnation FROM pages WHERE id=?1", [fixed])
            .await
            .map_err(sql)?;
        let current = rows
            .next()
            .await
            .map_err(sql)?
            .ok_or_else(|| {
                WenlanError::Conflict("organizing target was deleted; begin a new run".into())
            })?
            .get::<String>(0)
            .map_err(sql)?;
        if current != expected {
            return Err(WenlanError::Conflict(
                "organizing target was recreated; begin a new run".into(),
            ));
        }
    }
    let mut unique = std::collections::BTreeSet::new();
    for source in sources {
        if !unique.insert(source) {
            return Err(WenlanError::Validation("duplicate result source".into()));
        }
        let item = value
            .items
            .iter()
            .find(|item| item.source_ref.source_id == *source)
            .ok_or_else(|| {
                WenlanError::Validation("result source is outside exact run selection".into())
            })?;
        super::citation_provenance::research_fence_on(
            conn,
            &fence.space_id,
            &item.source_ref.source,
            &item.source_ref.source_id,
            item.lifecycle_revision,
            true,
        )
        .await?;
        if item.coverage != ResearchCoverage::Complete
            || !super::research_runs::complete_receipt(item)
        {
            return Err(WenlanError::Validation(
                "result cites unread or failed source".into(),
            ));
        }
    }
    for item in value
        .items
        .iter()
        .filter(|item| item.coverage == ResearchCoverage::Complete)
    {
        if !super::research_runs::complete_receipt(item) {
            return Err(WenlanError::Validation(
                "complete organizing source lacks server-read receipt".into(),
            ));
        }
        super::citation_provenance::research_fence_on(
            conn,
            &fence.space_id,
            &item.source_ref.source,
            &item.source_ref.source_id,
            item.lifecycle_revision,
            false,
        )
        .await?;
        let (_, digest) = source_snapshot(
            conn,
            &fence.space_id,
            &item.source_ref.source,
            &item.source_ref.source_id,
        )
        .await?;
        if digest != item.content_digest {
            return Err(WenlanError::Conflict(
                "selected source changed; begin a fresh exact-scope run".into(),
            ));
        }
    }
    Ok(())
}
pub(super) async fn bind(
    conn: &libsql::Connection,
    fence: &ResearchWriteFence,
    page_id: &str,
    sources: &[String],
    pending: Option<&str>,
    adopting: Option<&str>,
) -> Result<(), WenlanError> {
    guard(conn, fence, Some(page_id), sources, adopting).await?;
    let name = space_name(conn, &fence.space_id).await?;
    let mut rows = conn
        .query(
            "SELECT version,incarnation FROM pages WHERE id=?1 AND workspace=?2 AND status='active'",
            libsql::params![page_id, name],
        )
        .await
        .map_err(sql)?;
    let row = rows
        .next()
        .await
        .map_err(sql)?
        .ok_or_else(|| WenlanError::Conflict("result page outside research Space".into()))?;
    let version = row.get::<i64>(0).map_err(sql)?;
    let incarnation = row.get::<String>(1).map_err(sql)?;
    drop(rows);
    let mut value = run(conn, &fence.space_id, &fence.run_id).await?;
    value.target_page_id = Some(page_id.into());
    value.target_page_version = Some(version);
    value.target_page_incarnation = Some(incarnation);
    value.pending_revision_id = pending.map(str::to_string);
    value.result_page_version = pending.is_none().then_some(version);
    value.state = if pending.is_some() {
        OrganizeRunState::AwaitingReview
    } else if value
        .items
        .iter()
        .all(|i| i.coverage == ResearchCoverage::Complete)
    {
        OrganizeRunState::Completed
    } else {
        OrganizeRunState::Partial
    };
    value.revision += 1;
    value.updated_at = chrono::Utc::now().timestamp();
    save(
        conn,
        &key("run", &(&fence.space_id, &fence.run_id))?,
        &value,
    )
    .await
}

/// A dismissed or invalidated revision stops waiting in the same transaction
/// that removes its card. Successful adoption already cleared pending_revision_id.
pub(super) async fn revision_removed(
    conn: &libsql::Connection,
    revision_id: &str,
) -> Result<(), WenlanError> {
    let mut rows=conn.query("SELECT key,value FROM app_metadata WHERE key LIKE 'research_v1:run:%' AND json_extract(value,'$.pending_revision_id')=?1",[revision_id]).await.map_err(sql)?;
    let mut updates = Vec::new();
    while let Some(row) = rows.next().await.map_err(sql)? {
        let key = row.get::<String>(0).map_err(sql)?;
        let mut value: wenlan_types::research::OrganizeRun =
            serde_json::from_str(&row.get::<String>(1).map_err(sql)?)?;
        value.pending_revision_id = None;
        value.state = OrganizeRunState::Partial;
        value.revision += 1;
        value.updated_at = chrono::Utc::now().timestamp();
        updates.push((key, value));
    }
    drop(rows);
    for (key, value) in updates {
        save(conn, &key, &value).await?;
    }
    Ok(())
}
#[cfg(test)]
#[path = "research_write_tests.rs"]
mod tests;
