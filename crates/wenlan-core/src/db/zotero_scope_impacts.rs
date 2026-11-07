// SPDX-License-Identifier: Apache-2.0
//! Selection loss is a durable research-review fact, independent of source bytes/lifecycle.
use crate::{pages::Page, WenlanError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

#[derive(Clone, Serialize, Deserialize)]
struct Impact {
    #[serde(default)]
    event_id: String,
    #[serde(default)]
    observed_at: i64,
    #[serde(default)]
    reviewed: bool,
    incarnation: String,
    space_id: String,
    source_revision: i64,
    page_version: i64,
    locators: BTreeSet<String>,
    reason: String,
}
fn key(id: &str) -> String {
    format!("zotero_scope_impact:{id}")
}
fn sql(e: libsql::Error) -> WenlanError {
    WenlanError::VectorDb(format!("Zotero scope impact: {e}"))
}
async fn load(conn: &libsql::Connection, id: &str) -> Result<Vec<Impact>, WenlanError> {
    let mut rows = conn
        .query("SELECT value FROM app_metadata WHERE key=?1", [key(id)])
        .await
        .map_err(sql)?;
    Ok(match rows.next().await.map_err(sql)? {
        Some(row) => serde_json::from_str(&row.get::<String>(0).map_err(sql)?)?,
        None => vec![],
    })
}
async fn save(conn: &libsql::Connection, id: &str, impacts: &[Impact]) -> Result<(), WenlanError> {
    conn.execute("INSERT INTO app_metadata(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",libsql::params![key(id),serde_json::to_string(impacts)?]).await.map_err(sql)?;
    Ok(())
}
pub(super) async fn zotero_scope_preview_on(
    conn: &libsql::Connection,
    space_id: &str,
    sources: &[String],
) -> Result<Vec<String>, WenlanError> {
    let mut rows=conn.query("SELECT p.id FROM pages p JOIN spaces s ON s.name=p.space WHERE s.id=?1 AND p.status='active' AND (EXISTS(SELECT 1 FROM json_each(COALESCE(p.source_memory_ids,'[]')) j WHERE CAST(j.value AS TEXT) IN (SELECT value FROM json_each(?2)) OR CAST(j.value AS TEXT) IN (SELECT m.id FROM memories m WHERE m.source='file' AND m.source_id IN (SELECT value FROM json_each(?2)) AND m.space=p.space)) OR EXISTS(SELECT 1 FROM edges e WHERE e.src_id=p.id AND e.edge_type='cites' AND e.valid_until IS NULL AND (e.dst_id IN (SELECT value FROM json_each(?2)) OR e.dst_id IN (SELECT m.id FROM memories m WHERE m.source='file' AND m.source_id IN (SELECT value FROM json_each(?2)) AND m.space=p.space)))) ORDER BY p.id LIMIT 10001",libsql::params![space_id,serde_json::to_string(sources)?]).await.map_err(sql)?;
    let mut pages = vec![];
    while let Some(row) = rows.next().await.map_err(sql)? {
        pages.push(row.get(0).map_err(sql)?);
    }
    if pages.len() > 10000 {
        return Err(WenlanError::Validation(
            "Zotero impact exceeds 10000 pages".into(),
        ));
    }
    Ok(pages)
}
pub(super) async fn zotero_scope_impact_on(
    conn: &libsql::Connection,
    space_id: &str,
    sources: &[String],
    reason: &str,
) -> Result<Vec<String>, WenlanError> {
    let pages = zotero_scope_preview_on(conn, space_id, sources).await?;
    let mut locators: BTreeSet<String> = sources.iter().cloned().collect();
    let mut rows=conn.query("SELECT m.id FROM memories m JOIN spaces s ON m.space=s.name WHERE s.id=?1 AND m.source='file' AND m.source_id IN (SELECT value FROM json_each(?2))",libsql::params![space_id,serde_json::to_string(sources)?]).await.map_err(sql)?;
    while let Some(row) = rows.next().await.map_err(sql)? {
        locators.insert(row.get(0).map_err(sql)?);
    }
    drop(rows);
    for page in &pages {
        let mut rows = conn
            .query(
                "SELECT incarnation,COALESCE(source_revision,0),version FROM pages WHERE id=?1",
                [page.as_str()],
            )
            .await
            .map_err(sql)?;
        let row = rows
            .next()
            .await
            .map_err(sql)?
            .ok_or_else(|| WenlanError::NotFound("Affected page disappeared".into()))?;
        let mut impacts = load(conn, page).await?;
        if impacts.len() >= 1000 {
            return Err(WenlanError::Validation(
                "Zotero page impact exceeds 1000 events".into(),
            ));
        }
        let event_id = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&(
                page,
                space_id,
                &locators,
                reason,
                row.get::<String>(0).map_err(sql)?,
                row.get::<i64>(1).map_err(sql)? + 1
            ))?)
        );
        impacts.push(Impact {
            event_id,
            observed_at: chrono::Utc::now().timestamp(),
            reviewed: false,
            incarnation: row.get(0).map_err(sql)?,
            space_id: space_id.into(),
            source_revision: row.get::<i64>(1).map_err(sql)? + 1,
            page_version: row.get(2).map_err(sql)?,
            locators: locators.clone(),
            reason: reason.into(),
        });
        drop(rows);
        super::citation_provenance::capture_on(
            conn,
            page,
            &locators.iter().cloned().collect::<Vec<_>>(),
            reason,
        )
        .await?;
        save(conn, page, &impacts).await?;
        conn.execute("UPDATE pages SET stale_reason='source_updated',source_revision=COALESCE(source_revision,0)+1,sources_updated_count=COALESCE(sources_updated_count,0)+1,citations=NULL WHERE id=?1",[page.as_str()]).await.map_err(sql)?;
    }
    Ok(pages)
}
/// Match against the current Page identity, never a reused id or former Space.
async fn pending_on(conn: &libsql::Connection, page_id: &str) -> Result<Vec<Impact>, WenlanError> {
    let mut rows = conn
        .query(
            "SELECT p.incarnation,s.id FROM pages p JOIN spaces s ON s.name=p.space WHERE p.id=?1",
            [page_id],
        )
        .await
        .map_err(sql)?;
    let Some(row) = rows.next().await.map_err(sql)? else {
        return Ok(vec![]);
    };
    let incarnation: String = row.get(0).map_err(sql)?;
    let space: String = row.get(1).map_err(sql)?;
    drop(rows);
    Ok(load(conn, page_id)
        .await?
        .into_iter()
        .filter(|i| !i.reviewed && i.incarnation == incarnation && i.space_id == space)
        .collect())
}
/// Internal locators are returned only to the authorized page-detail adapter.
/// The adapter must resolve them within the page Space and never expose them.
impl super::MemoryDB {
    pub async fn pending_zotero_scope_impacts(
        &self,
        page_id: &str,
        expected_space: &str,
    ) -> Result<Vec<(String, Vec<String>)>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query("SELECT space FROM pages WHERE id=?1", [page_id])
            .await
            .map_err(sql)?;
        let matches = match rows.next().await.map_err(sql)? {
            Some(row) => row.get::<String>(0).ok().as_deref() == Some(expected_space),
            None => false,
        };
        drop(rows);
        if !matches {
            return Ok(Vec::new());
        }
        Ok(pending_on(&conn, page_id).await?.into_iter().map(|impact| {
            let reason = match impact.reason.as_str() {
                "Zotero item is no longer in the synchronized selection; retained evidence requires review" => "selection_removed",
                "Zotero connection was disabled; retained evidence requires review" => "disconnected",
                _ => "unknown",
            };
            (reason.to_string(), impact.locators.into_iter().collect())
        }).collect())
    }
}

/// Caller holds the write transaction so selection loss cannot race the body write.
pub(super) async fn has_pending_on(
    conn: &libsql::Connection,
    page_id: &str,
) -> Result<bool, WenlanError> {
    Ok(!pending_on(conn, page_id).await?.is_empty())
}
/// Projection survives generic producer re-reads, body edits, citation rebuilds and version increments.
pub(super) async fn apply_on(
    conn: &libsql::Connection,
    page: &mut Page,
) -> Result<(), WenlanError> {
    let impacts = pending_on(conn, &page.id).await?;
    if impacts.is_empty() {
        return Ok(());
    }
    page.stale_reason = Some("source_updated".into());
    page.refresh_blocked_reason =
        Some(wenlan_types::zotero::ZOTERO_SELECTION_REVIEW_REQUIRED.into());
    let affected: BTreeSet<_> = impacts
        .iter()
        .flat_map(|i| i.locators.iter().cloned())
        .collect();
    let occurrences: BTreeSet<_> =
        match crate::citations::citation_claim_groups(&page.content, &page.citations) {
            Some(groups) => groups
                .into_iter()
                .filter(|g| {
                    page.citations
                        .iter()
                        .any(|c| g.contains(&c.occurrence) && affected.contains(&c.locator))
                })
                .flatten()
                .collect(),
            None => page.citations.iter().map(|c| c.occurrence).collect(),
        };
    for c in &mut page.citations {
        if occurrences.contains(&c.occurrence) {
            c.status = "unevaluated".into();
            c.score = 0.0;
        }
    }
    Ok(())
}
/// Caller holds the adoption transaction and has checked its stage source fence.
pub(super) async fn clear_reviewed_on(
    conn: &libsql::Connection,
    page_id: &str,
    incarnation: &str,
    stage_source_revision: i64,
) -> Result<(), WenlanError> {
    let mut impacts = load(conn, page_id).await?;
    for impact in &mut impacts {
        if impact.incarnation == incarnation && impact.source_revision <= stage_source_revision {
            impact.reviewed = true;
        }
    }
    save(conn, page_id, &impacts).await
}
