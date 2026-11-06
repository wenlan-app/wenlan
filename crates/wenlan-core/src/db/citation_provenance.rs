// SPDX-License-Identifier: Apache-2.0
//! Historical reference identity is separate from the NULL citation-work queue.
//! Records live in existing app_metadata and participate in the caller's DB transaction.
use super::{
    citation_sources::CitationSourceSnapshot, commit_or_rollback, MemoryDB, PageFence,
    UNFILED_SPACE_ID,
};
use crate::{pages::Page, read_scope::ReadScope, WenlanError};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use wenlan_types::{pages::PageCitation, source_lifecycle::*};

fn sql(error: libsql::Error) -> WenlanError {
    WenlanError::VectorDb(format!("citation provenance: {error}"))
}
fn digest(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}
fn key(kind: &str, identity: &impl Serialize) -> Result<String, WenlanError> {
    Ok(format!(
        "source_lifecycle:{kind}:{}",
        digest(&serde_json::to_string(identity)?)
    ))
}
async fn load<T: DeserializeOwned>(
    conn: &libsql::Connection,
    key: &str,
) -> Result<Option<T>, WenlanError> {
    let mut rows = conn
        .query("SELECT value FROM app_metadata WHERE key=?1", [key])
        .await
        .map_err(sql)?;
    rows.next()
        .await
        .map_err(sql)?
        .map(|row| -> Result<T, WenlanError> {
            Ok(serde_json::from_str(&row.get::<String>(0).map_err(sql)?)?)
        })
        .transpose()
}
async fn save(
    conn: &libsql::Connection,
    key: &str,
    value: &impl Serialize,
) -> Result<(), WenlanError> {
    conn.execute("INSERT INTO app_metadata(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", libsql::params![key,serde_json::to_string(value)?]).await.map_err(sql)?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SourceHistory {
    records: Vec<SourceLifecycleRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CitationProvenance {
    page_id: String,
    incarnation: String,
    version: i64,
    space: Option<String>,
    space_id: Option<String>,
    body_digest: String,
    citations: Vec<PageCitation>,
    affected_locators: BTreeSet<String>,
    #[serde(default)]
    historical_locators: BTreeSet<String>,
    impacts: Vec<SourceLifecycleImpact>,
}

struct PageState {
    content: String,
    incarnation: String,
    version: i64,
    space: Option<String>,
    space_id: Option<String>,
    source_revision: i64,
    citations: Option<String>,
}
async fn page_on(conn: &libsql::Connection, id: &str) -> Result<Option<PageState>, WenlanError> {
    let mut rows = conn.query("SELECT content,incarnation,version,NULLIF(space,?2),COALESCE(source_revision,0),citations,(SELECT id FROM spaces WHERE name=NULLIF(p.space,?2)) FROM pages p WHERE p.id=?1",libsql::params![id,UNFILED_SPACE_ID]).await.map_err(sql)?;
    rows.next()
        .await
        .map_err(sql)?
        .map(|row| {
            Ok(PageState {
                content: row.get(0).map_err(sql)?,
                incarnation: row.get(1).map_err(sql)?,
                version: row.get(2).map_err(sql)?,
                space: row.get(3).map_err(sql)?,
                source_revision: row.get(4).map_err(sql)?,
                citations: row.get(5).map_err(sql)?,
                space_id: row.get(6).map_err(sql)?,
            })
        })
        .transpose()
}
fn matches_page(saved: &CitationProvenance, page: &PageState) -> bool {
    saved.incarnation == page.incarnation
        && saved.version == page.version
        && saved.space_id == page.space_id
        && (saved.space_id.is_some() || saved.space == page.space)
        && saved.body_digest == digest(&page.content)
}

/// Caller holds the DB lock and its source-change transaction. This must run
/// BEFORE clearing citations, while the original occurrence map still exists.
pub(super) async fn capture_on(
    conn: &libsql::Connection,
    page_id: &str,
    locators: &[String],
    reason: &str,
) -> Result<(), WenlanError> {
    capture_kind_on(conn, page_id, locators, None, reason).await
}

pub(super) async fn capture_detached_on(
    conn: &libsql::Connection,
    page_id: &str,
    locators: &[String],
    reason: &str,
) -> Result<(), WenlanError> {
    capture_kind_on(conn, page_id, locators, Some(true), reason).await
}

async fn capture_kind_on(
    conn: &libsql::Connection,
    page_id: &str,
    locators: &[String],
    historical: Option<bool>,
    reason: &str,
) -> Result<(), WenlanError> {
    super::page_export::capture_on(conn, page_id).await?;
    let Some(page) = page_on(conn, page_id).await? else {
        return Ok(());
    };
    let key = key("page", &page_id)?;
    let previous: Option<CitationProvenance> = load(conn, &key).await?;
    let prior_impacts = previous
        .as_ref()
        .filter(|saved| {
            saved.incarnation == page.incarnation
                && saved.space_id == page.space_id
                && (saved.space_id.is_some() || saved.space == page.space)
        })
        .map(|saved| saved.impacts.clone())
        .unwrap_or_default();
    if let Some(saved) = previous
        .as_ref()
        .filter(|saved| !matches_page(saved, &page))
    {
        save(
            conn,
            &self::key(
                "page-history",
                &(
                    page_id,
                    &saved.incarnation,
                    saved.version,
                    page.source_revision,
                ),
            )?,
            saved,
        )
        .await?;
    }
    let previous = previous.filter(|saved| matches_page(saved, &page));
    // A later explicit recheck can update the reference map without changing
    // prose/version. Keep the prior map in history, then snapshot that new map.
    let previous = if let (Some(saved), Some(json)) = (&previous, page.citations.as_deref()) {
        let live: Vec<PageCitation> = serde_json::from_str(json)?;
        if serde_json::to_value(&live)?
            != serde_json::to_value(effective_map(
                &page.content,
                &saved.citations,
                &saved.affected_locators,
                &saved.historical_locators,
            ))?
        {
            save(
                conn,
                &self::key(
                    "page-history",
                    &(
                        page_id,
                        &page.incarnation,
                        page.version,
                        page.source_revision,
                    ),
                )?,
                saved,
            )
            .await?;
            None
        } else {
            previous
        }
    } else {
        previous
    };
    let mut saved = match previous {
        Some(saved) => saved,
        None => {
            let Some(json) = page.citations.as_deref() else {
                return Ok(());
            };
            let citations: Vec<PageCitation> = serde_json::from_str(json)?;
            // An explicit empty map has no historical targets to resurrect.
            if citations.is_empty() {
                return Ok(());
            }
            CitationProvenance {
                page_id: page_id.into(),
                incarnation: page.incarnation.clone(),
                version: page.version,
                space: page.space.clone(),
                space_id: page.space_id.clone(),
                body_digest: digest(&page.content),
                citations,
                affected_locators: BTreeSet::new(),
                historical_locators: BTreeSet::new(),
                impacts: prior_impacts,
            }
        }
    };
    saved.affected_locators.extend(locators.iter().cloned());
    if let Some(historical) = historical {
        for locator in locators {
            if historical {
                saved.historical_locators.insert(locator.clone());
            } else {
                saved.historical_locators.remove(locator);
            }
        }
    }
    let occurrences = affected_occurrences(
        &page.content,
        &saved.citations,
        &locators.iter().cloned().collect(),
    )
    .into_iter()
    .collect();
    if saved.impacts.len() >= 1000 {
        return Err(WenlanError::Validation(
            "source impact history exceeds its bound; no change was applied".into(),
        ));
    }
    saved.impacts.push(SourceLifecycleImpact {
        page_id: page_id.into(),
        page_version: page.version,
        source_revision: page.source_revision + 1,
        source_ids: locators.to_vec(),
        occurrences,
        reason: reason.into(),
    });
    save(conn, &key, &saved).await
}

/// Lower trust for a whole producer claim group, never just an individual
/// source whose score was only an audit component of the union-level check.
fn effective_map(
    body: &str,
    citations: &[PageCitation],
    affected: &BTreeSet<String>,
    historical: &BTreeSet<String>,
) -> Vec<PageCitation> {
    let affected_occurrences = affected_occurrences(body, citations, affected);
    citations
        .iter()
        .cloned()
        .map(|mut citation| {
            if affected_occurrences.contains(&citation.occurrence) {
                citation.status = "unevaluated".into();
                citation.score = 0.0;
            }
            if historical.contains(&citation.locator) {
                citation.historical = true;
            }
            citation
        })
        .collect()
}

fn affected_occurrences(
    body: &str,
    citations: &[PageCitation],
    affected: &BTreeSet<String>,
) -> BTreeSet<u32> {
    match crate::citations::citation_claim_groups(body, citations) {
        Some(groups) => groups
            .into_iter()
            .filter(|group| {
                citations
                    .iter()
                    .any(|c| group.contains(&c.occurrence) && affected.contains(&c.locator))
            })
            .flatten()
            .collect(),
        None => citations.iter().map(|c| c.occurrence).collect(), // No exact grouping proof: never retain a verified claim.
    }
}

async fn preserved_on(
    conn: &libsql::Connection,
    id: &str,
) -> Result<Option<(PageState, CitationProvenance)>, WenlanError> {
    let Some(page) = page_on(conn, id).await? else {
        return Ok(None);
    };
    if page.citations.is_some() {
        return Ok(None);
    } // SQL NULL only, never [] or an intentional clear.
    let saved: Option<CitationProvenance> = load(conn, &key("page", &id)?).await?;
    Ok(saved
        .filter(|saved| matches_page(saved, &page))
        .map(|saved| (page, saved)))
}

/// Shared by scoped/unscoped by-id readers and sentence evidence. Reads only
/// the already authorized page identity and cannot expose another Space's map.
pub(super) async fn apply_on(
    conn: &libsql::Connection,
    page: &mut Page,
) -> Result<(), WenlanError> {
    if let Some((stored, saved)) = preserved_on(conn, &page.id).await? {
        if page.version == stored.version
            && page.space == stored.space
            && page.content == stored.content
        {
            page.citations = effective_map(
                &stored.content,
                &saved.citations,
                &saved.affected_locators,
                &saved.historical_locators,
            );
        }
    }
    page.citations = downgrade_on(
        conn,
        &page.content,
        page.space.as_deref(),
        &page.citations,
        &[],
    )
    .await?;
    super::zotero_scope_impacts::apply_on(conn, page).await?;
    Ok(())
}

/// Authoritative trust normalization shared by readers and every canonical
/// citation write. Uses the incoming body, not a prior version's sentences.
async fn downgrade_on(
    conn: &libsql::Connection,
    body: &str,
    space: Option<&str>,
    citations: &[PageCitation],
    expected_indexed: &[String],
) -> Result<Vec<PageCitation>, WenlanError> {
    let mut historical = BTreeSet::new();
    let mut availability = std::collections::HashMap::new();
    for citation in citations {
        if citation.historical {
            historical.insert(citation.locator.clone());
            continue;
        }
        let mut rows=conn.query("SELECT DISTINCT source,source_id FROM memories WHERE (source_id=?1 OR id=?1) AND NULLIF(space,?3) IS ?2 AND COALESCE(pending_revision,0)=0 LIMIT 2",libsql::params![citation.locator.as_str(),space,UNFILED_SPACE_ID]).await.map_err(sql)?;
        let mut sources = Vec::new();
        while let Some(row) = rows.next().await.map_err(sql)? {
            sources.push((
                row.get::<String>(0).map_err(sql)?,
                row.get::<String>(1).map_err(sql)?,
            ));
        }
        drop(rows);
        let expected = expected_indexed
            .iter()
            .any(|id| id == &citation.locator || sources.iter().any(|(_, root)| root == id));
        if expected {
            if !availability.contains_key(&citation.locator) {
                let scope = space
                    .map(|name| ReadScope::Space(name.into()))
                    .unwrap_or(ReadScope::Uncategorized);
                let mut available =
                    match super::citation_sources::read_on(conn, &citation.locator, &scope).await {
                        Ok(Some(_)) => true,
                        Ok(None) | Err(WenlanError::Validation(_)) => false,
                        Err(error) => return Err(error),
                    };
                // Page-basis references use the memory kind too. They are
                // scoped pages, not indexed file/webpage inputs.
                if !available && citation.source_kind == "memory" {
                    let mut rows = conn
                        .query(
                            "SELECT 1 FROM pages WHERE id=?1 AND NULLIF(space,?3) IS ?2",
                            libsql::params![citation.locator.as_str(), space, UNFILED_SPACE_ID],
                        )
                        .await
                        .map_err(sql)?;
                    available = rows.next().await.map_err(sql)?.is_some();
                }
                availability.insert(citation.locator.clone(), available);
            }
            if availability.get(&citation.locator) == Some(&false) {
                historical.insert(citation.locator.clone());
                continue;
            }
        }
        if sources.len() == 1 {
            let (source, id) = &sources[0];
            if identity_record_on(conn, space, source, id)
                .await?
                .is_some_and(|record| record.state != SourceLifecycleState::Active)
            {
                historical.insert(citation.locator.clone());
            }
        } else if citation.source_kind == "memory" {
            let mut rows = conn
                .query(
                    "SELECT 1 FROM pages WHERE id=?1 AND NULLIF(space,?3) IS ?2",
                    libsql::params![citation.locator.as_str(), space, UNFILED_SPACE_ID],
                )
                .await
                .map_err(sql)?;
            if rows.next().await.map_err(sql)?.is_none() {
                historical.insert(citation.locator.clone());
            }
        }
    }
    if !historical.is_empty() {
        return Ok(effective_map(body, citations, &historical, &historical));
    }
    Ok(citations.to_vec())
}

/// Preserve SQL NULL and intentional empty maps. This path never stamps a
/// source generation, so historical restoration cannot certify old references.
pub(super) async fn normalize_on(
    conn: &libsql::Connection,
    body: &str,
    space: Option<&str>,
    json: Option<&str>,
    expected_indexed: &[String],
) -> Result<Option<String>, WenlanError> {
    let Some(json) = json else {
        return Ok(None);
    };
    let Ok(citations) = serde_json::from_str::<Vec<PageCitation>>(json) else {
        return Ok(Some(json.into()));
    };
    Ok(Some(serde_json::to_string(
        &downgrade_on(conn, body, space, &citations, expected_indexed).await?,
    )?))
}

/// The latest machine citation-count line must describe the normalized map.
/// Earlier history and human text are retained verbatim.
pub(super) fn normalize_summary(changelog: &str, normalized: Option<&str>) -> Option<String> {
    let citations: Vec<PageCitation> = serde_json::from_str(normalized?).ok()?;
    if !citations.iter().any(|c| c.historical) {
        return None;
    }
    let mut entries: serde_json::Value = serde_json::from_str(changelog).ok()?;
    let latest = entries.as_array_mut()?.last_mut()?.as_object_mut()?;
    if !latest.contains_key("citations_summary") {
        return None;
    }
    latest.insert(
        "citations_summary".into(),
        serde_json::Value::String(format!(
            "{} verified, {} unverified, {} unevaluated; retained source lifecycle requires review",
            citations.iter().filter(|c| c.status == "verified").count(),
            citations
                .iter()
                .filter(|c| c.status == "unverified")
                .count(),
            citations
                .iter()
                .filter(|c| c.status == "unevaluated")
                .count()
        )),
    );
    serde_json::to_string(&entries).ok()
}

pub(super) async fn capture_sources_on(
    conn: &libsql::Connection,
    page_id: &str,
    sources: &[(String, String)],
    reason: &str,
) -> Result<(), WenlanError> {
    let mut locators = BTreeSet::new();
    for (source, id) in sources {
        locators.insert(id.clone());
        let mut rows = conn
            .query(
                "SELECT id FROM memories WHERE source=?1 AND source_id=?2",
                libsql::params![source.as_str(), id.as_str()],
            )
            .await
            .map_err(sql)?;
        while let Some(row) = rows.next().await.map_err(sql)? {
            locators.insert(row.get::<String>(0).map_err(sql)?);
        }
    }
    capture_on(
        conn,
        page_id,
        &locators.into_iter().collect::<Vec<_>>(),
        reason,
    )
    .await
}

async fn exact_space_source(
    conn: &libsql::Connection,
    space_id: &str,
    source: &str,
    source_id: &str,
) -> Result<String, WenlanError> {
    if !matches!(source, "file" | "webpage") || source_id.is_empty() || source_id.len() > 8192 {
        return Err(WenlanError::Validation(
            "exact file/webpage source identity is required".into(),
        ));
    }
    let mut rows = conn
        .query("SELECT name FROM spaces WHERE id=?1", [space_id])
        .await
        .map_err(sql)?;
    let name = rows
        .next()
        .await
        .map_err(sql)?
        .ok_or_else(|| WenlanError::NotFound("source Space not found".into()))?
        .get::<String>(0)
        .map_err(sql)?;
    drop(rows);
    // Scope and whole-document ambiguity are checked together. Even a status
    // request containing an out-of-Space locator never reads its text.
    let value = super::citation_sources::read_on(conn, source_id, &ReadScope::Space(name.clone()))
        .await?
        .ok_or_else(|| {
            WenlanError::Validation(
                "source missing, ambiguous, incomplete or outside this Space".into(),
            )
        })?;
    if value.snapshot.source != source || value.snapshot.source_id != source_id {
        return Err(WenlanError::Validation(
            "source identity does not match the exact document".into(),
        ));
    }
    Ok(name)
}

async fn lifecycle_history(
    conn: &libsql::Connection,
    space_id: &str,
    source: &str,
    source_id: &str,
) -> Result<SourceHistory, WenlanError> {
    Ok(load(conn, &key("source", &(space_id, source, source_id))?)
        .await?
        .unwrap_or(SourceHistory {
            records: Vec::new(),
        }))
}
fn default_record(space_id: &str, source: &str, source_id: &str) -> SourceLifecycleRecord {
    SourceLifecycleRecord {
        space_id: space_id.into(),
        source: source.into(),
        source_id: source_id.into(),
        revision: 0,
        state: SourceLifecycleState::Active,
        reason: String::new(),
        actor: String::new(),
        updated_at: 0,
    }
}

/// A write/recheck must check this under its final source-snapshot transaction.
/// Active status is not inferred from text still being readable in storage.
pub(super) async fn ensure_active_on(
    conn: &libsql::Connection,
    snapshots: &[CitationSourceSnapshot],
) -> Result<(), WenlanError> {
    if !inactive_locators_on(conn, snapshots).await?.is_empty() {
        return Err(WenlanError::Validation("source_requires_review: excluded, retracted or removed source cannot verify a current claim".into()));
    }
    Ok(())
}

async fn record_on(
    conn: &libsql::Connection,
    snapshot: &CitationSourceSnapshot,
) -> Result<Option<SourceLifecycleRecord>, WenlanError> {
    identity_record_on(
        conn,
        snapshot.space.as_deref(),
        &snapshot.source,
        &snapshot.source_id,
    )
    .await
}

async fn identity_record_on(
    conn: &libsql::Connection,
    space: Option<&str>,
    source: &str,
    source_id: &str,
) -> Result<Option<SourceLifecycleRecord>, WenlanError> {
    if !matches!(source, "file" | "webpage") {
        return Ok(None);
    }
    let Some(space) = space else {
        return Ok(None);
    };
    let mut rows = conn
        .query("SELECT id FROM spaces WHERE name=?1", [space])
        .await
        .map_err(sql)?;
    let Some(row) = rows.next().await.map_err(sql)? else {
        return Ok(None);
    };
    let id = row.get::<String>(0).map_err(sql)?;
    drop(rows);
    Ok(lifecycle_history(conn, &id, source, source_id)
        .await?
        .records
        .last()
        .cloned())
}

async fn inactive_locators_on(
    conn: &libsql::Connection,
    snapshots: &[CitationSourceSnapshot],
) -> Result<BTreeSet<String>, WenlanError> {
    let mut inactive = BTreeSet::new();
    for snapshot in snapshots {
        if record_on(conn, snapshot)
            .await?
            .is_some_and(|r| r.state != SourceLifecycleState::Active)
        {
            inactive.insert(snapshot.locator.clone());
        }
    }
    Ok(inactive)
}

/// Internal optimistic-read fingerprint includes lifecycle revision. Text
/// digest and cited version remain the actual original source's identity.
pub(super) async fn stamp_snapshot_on(
    conn: &libsql::Connection,
    snapshot: &mut CitationSourceSnapshot,
) -> Result<(), WenlanError> {
    if let Some(record) = record_on(conn, snapshot).await? {
        snapshot.fingerprint = digest(&format!(
            "{}:lifecycle:{}",
            snapshot.fingerprint, record.revision
        ));
    }
    Ok(())
}

async fn affected_pages(
    conn: &libsql::Connection,
    space: &str,
    source: &str,
    source_id: &str,
) -> Result<Vec<String>, WenlanError> {
    let mut rows=conn.query("SELECT p.id FROM pages p WHERE p.space=?1 AND p.status='active' AND (EXISTS (SELECT 1 FROM json_each(COALESCE(p.source_memory_ids,'[]')) s WHERE CAST(s.value AS TEXT)=?3 OR CAST(s.value AS TEXT) IN (SELECT id FROM memories WHERE source=?2 AND source_id=?3)) OR EXISTS (SELECT 1 FROM edges e WHERE e.src_id=p.id AND e.edge_type='cites' AND e.valid_until IS NULL AND (e.dst_id=?3 OR e.dst_id IN (SELECT id FROM memories WHERE source=?2 AND source_id=?3)))) ORDER BY p.id LIMIT 10001",libsql::params![space,source,source_id]).await.map_err(sql)?;
    let mut result = Vec::new();
    while let Some(row) = rows.next().await.map_err(sql)? {
        result.push(row.get(0).map_err(sql)?);
    }
    if result.len() > 10000 {
        return Err(WenlanError::Validation(
            "source impact exceeds 10000 pages; no change was applied".into(),
        ));
    }
    Ok(result)
}

impl MemoryDB {
    pub async fn resolve_source_reference(
        &self,
        space_id: &str,
        locator: &str,
    ) -> Result<SourceReferenceIdentity, WenlanError> {
        if locator.is_empty() || locator.len() > 8192 {
            return Err(WenlanError::Validation(
                "exact reference locator required".into(),
            ));
        }
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query("SELECT name FROM spaces WHERE id=?1", [space_id])
            .await
            .map_err(sql)?;
        let name = rows
            .next()
            .await
            .map_err(sql)?
            .ok_or_else(|| WenlanError::NotFound("source Space not found".into()))?
            .get::<String>(0)
            .map_err(sql)?;
        drop(rows);
        let value =
            super::citation_sources::read_on(&conn, locator, &ReadScope::Space(name.clone()))
                .await?
                .ok_or_else(|| {
                    WenlanError::NotFound("reference unavailable in this Space".into())
                })?;
        if !matches!(value.snapshot.source.as_str(), "file" | "webpage") {
            return Err(WenlanError::Validation(
                "reference is not a retained document".into(),
            ));
        }
        let mut rows=conn.query("SELECT title,source_agent FROM memories WHERE source=?1 AND source_id=?2 AND space=?3 AND chunk_index=0",libsql::params![value.snapshot.source.as_str(),value.snapshot.source_id.as_str(),name]).await.map_err(sql)?;
        let row = rows.next().await.map_err(sql)?.ok_or_else(|| {
            WenlanError::NotFound("reference document metadata unavailable".into())
        })?;
        Ok(SourceReferenceIdentity {
            space_id: space_id.into(),
            source: value.snapshot.source,
            source_id: value.snapshot.source_id,
            source_agent: row.get(1).map_err(sql)?,
            title: row.get(0).map_err(sql)?,
        })
    }
    pub async fn source_lifecycle(
        &self,
        space_id: &str,
        source: &str,
        source_id: &str,
    ) -> Result<SourceLifecycleResponse, WenlanError> {
        let conn = self.conn.lock().await;
        let name = exact_space_source(&conn, space_id, source, source_id).await?;
        let mut response = response_on(&conn, space_id, &name, source, source_id).await?;
        let mut locators = BTreeSet::from([source_id.to_string()]);
        let mut chunks = conn
            .query(
                "SELECT id FROM memories WHERE source=?1 AND source_id=?2",
                libsql::params![source, source_id],
            )
            .await
            .map_err(sql)?;
        while let Some(row) = chunks.next().await.map_err(sql)? {
            locators.insert(row.get(0).map_err(sql)?);
        }
        drop(chunks);
        for id in affected_pages(&conn, &name, source, source_id).await? {
            let Some(page) = page_on(&conn, &id).await? else {
                continue;
            };
            let citations = if let Some(json) = page.citations.as_deref() {
                serde_json::from_str::<Vec<PageCitation>>(json)?
            } else {
                preserved_on(&conn, &id)
                    .await?
                    .map(|(_, saved)| saved.citations)
                    .unwrap_or_default()
            };
            response.preview_impacts.push(SourceLifecycleImpact {
                page_id: id,
                page_version: page.version,
                source_revision: page.source_revision,
                source_ids: locators.iter().cloned().collect(),
                occurrences: affected_occurrences(&page.content, &citations, &locators)
                    .into_iter()
                    .collect(),
                reason: "Prospective source-status change: cited claim groups would require review"
                    .into(),
            });
        }
        Ok(response)
    }
    pub async fn set_source_lifecycle(
        &self,
        space_id: &str,
        request: SetSourceLifecycleRequest,
        actor: &str,
    ) -> Result<SourceLifecycleResponse, WenlanError> {
        if request.reason.trim().is_empty()
            || request.reason.len() > 4000
            || actor.is_empty()
            || actor.len() > 200
        {
            return Err(WenlanError::Validation(
                "source status requires a bounded reason and actor".into(),
            ));
        }
        let conn = self.conn.lock().await;
        conn.execute("BEGIN", ()).await.map_err(sql)?;
        let result=async{
            let name=exact_space_source(&conn,space_id,&request.source,&request.source_id).await?;
            if request.space!=name {return Err(WenlanError::Validation("source Space identity changed".into()));}
            let mut history=lifecycle_history(&conn,space_id,&request.source,&request.source_id).await?;
            let previous=history.records.last().cloned().unwrap_or_else(||default_record(space_id,&request.source,&request.source_id));
            if previous.revision!=request.expected_revision {return Err(WenlanError::Conflict("source status changed; reread before deciding".into()));}
            if history.records.len()>=1000 {return Err(WenlanError::Validation("source status history exceeds its bound; no change was applied".into()));}
            let record=SourceLifecycleRecord{space_id:space_id.into(),source:request.source.clone(),source_id:request.source_id.clone(),revision:previous.revision+1,state:request.state,reason:request.reason.trim().into(),actor:actor.into(),updated_at:chrono::Utc::now().timestamp()};
            let pages=affected_pages(&conn,&name,&request.source,&request.source_id).await?;
            let mut locators=vec![request.source_id.clone()];
            let mut chunks=conn.query("SELECT id FROM memories WHERE source=?1 AND source_id=?2",libsql::params![request.source.as_str(),request.source_id.as_str()]).await.map_err(sql)?;
            while let Some(row)=chunks.next().await.map_err(sql)? {locators.push(row.get(0).map_err(sql)?);} drop(chunks);
            for page in pages {
                capture_kind_on(&conn,&page,&locators,Some(record.state!=SourceLifecycleState::Active),&record.reason).await?;
                conn.execute("UPDATE pages SET stale_reason='source_updated',refresh_blocked_reason=NULL,sources_updated_count=COALESCE(sources_updated_count,0)+1,source_revision=COALESCE(source_revision,0)+1,citations=NULL WHERE id=?1",[page]).await.map_err(sql)?;
            }
            history.records.push(record);
            save(&conn,&key("source",&(space_id,&request.source,&request.source_id))?,&history).await?;
            response_on(&conn,space_id,&name,&request.source,&request.source_id).await
        }.await;
        match result {
            Ok(value) => {
                commit_or_rollback(&conn).await.map_err(sql)?;
                Ok(value)
            }
            Err(error) => {
                let _ = conn.execute("ROLLBACK", ()).await;
                Err(error)
            }
        }
    }
    pub async fn ensure_citation_sources_active(
        &self,
        snapshots: &[CitationSourceSnapshot],
    ) -> Result<(), WenlanError> {
        let conn = self.conn.lock().await;
        ensure_active_on(&conn, snapshots).await
    }

    pub(crate) async fn downgrade_inactive_citations(
        &self,
        body: &str,
        json: &str,
        snapshots: &[CitationSourceSnapshot],
    ) -> Result<String, WenlanError> {
        let conn = self.conn.lock().await;
        let inactive = inactive_locators_on(&conn, snapshots).await?;
        let citations: Vec<PageCitation> = serde_json::from_str(json)?;
        Ok(serde_json::to_string(&effective_map(
            body, &citations, &inactive, &inactive,
        ))?)
    }

    /// None: no valid historical map. Some(false): concurrent writer won;
    /// Some(true): references restored, while source_updated/review impacts remain.
    pub(crate) async fn restore_citation_provenance(
        &self,
        id: &str,
        expected_version: i64,
        fence: &PageFence,
    ) -> Result<Option<bool>, WenlanError> {
        let conn = self.conn.lock().await;
        conn.execute("BEGIN", ()).await.map_err(sql)?;
        let result=async{
            let Some((page,saved))=preserved_on(&conn,id).await? else{return Ok(None);};
            if page.incarnation!=fence.incarnation || page.version!=expected_version || page.source_revision!=fence.source_revision {return Ok(Some(false));}
            let citations=serde_json::to_string(&effective_map(&page.content,&saved.citations,&saved.affected_locators,&saved.historical_locators))?;
            let citations=normalize_on(&conn,&page.content,page.space.as_deref(),Some(&citations),&saved.historical_locators.iter().cloned().collect::<Vec<_>>()).await?.unwrap_or(citations);
            // Deliberately do not stamp current versions onto historical refs.
            let changed=conn.execute("UPDATE pages SET citations=?1 WHERE id=?2 AND version=?3 AND COALESCE(source_revision,0)=?4 AND incarnation=?5 AND citations IS NULL AND status='active'",libsql::params![citations.as_str(),id,page.version,page.source_revision,page.incarnation]).await.map_err(sql)?;
            if changed>0 {Self::dual_write_page_citations(&conn,id,Some(&citations)).await.map_err(sql)?;}
            Ok(Some(changed>0))
        }.await;
        match result {
            Ok(value) => {
                commit_or_rollback(&conn).await.map_err(sql)?;
                Ok(value)
            }
            Err(error) => {
                let _ = conn.execute("ROLLBACK", ()).await;
                Err(error)
            }
        }
    }
}

async fn response_on(
    conn: &libsql::Connection,
    space_id: &str,
    space: &str,
    source: &str,
    source_id: &str,
) -> Result<SourceLifecycleResponse, WenlanError> {
    let history = lifecycle_history(conn, space_id, source, source_id).await?;
    let record = history
        .records
        .last()
        .cloned()
        .unwrap_or_else(|| default_record(space_id, source, source_id));
    let mut impacts = Vec::new();
    for page in affected_pages(conn, space, source, source_id).await? {
        if let Some(saved) = load::<CitationProvenance>(conn, &key("page", &page)?).await? {
            // Impact records carry only authorized page identity and occurrences.
            if saved.space_id.as_deref() == Some(space_id) {
                impacts.extend(
                    saved
                        .impacts
                        .into_iter()
                        .filter(|impact| impact.source_ids.iter().any(|id| id == source_id)),
                );
            }
        }
    }
    Ok(SourceLifecycleResponse {
        record,
        history: history.records,
        impacts,
        preview_impacts: Vec::new(),
    })
}

#[cfg(test)]
#[path = "citation_provenance_tests.rs"]
mod tests;

/// Metadata-only lookup; callers retain their existing exact-source authorization.
pub(super) async fn summary_on(
    conn: &libsql::Connection,
    space: Option<&str>,
    source: &str,
    source_id: &str,
) -> Result<SourceLifecycleSummary, WenlanError> {
    Ok(identity_record_on(conn, space, source, source_id)
        .await?
        .map(|r| SourceLifecycleSummary {
            state: r.state,
            revision: r.revision,
            reason: r.reason,
        })
        .unwrap_or_default())
}
pub(super) async fn research_fence_on(
    conn: &libsql::Connection,
    space_id: &str,
    source: &str,
    source_id: &str,
    expected_revision: u64,
    require_active: bool,
) -> Result<(), WenlanError> {
    let current = lifecycle_history(conn, space_id, source, source_id)
        .await?
        .records
        .last()
        .cloned()
        .unwrap_or_else(|| default_record(space_id, source, source_id));
    if current.revision != expected_revision {
        return Err(WenlanError::Conflict(
            "source lifecycle changed; begin a fresh exact-scope run or proposal".into(),
        ));
    }
    if require_active && current.state != SourceLifecycleState::Active {
        return Err(WenlanError::Validation("source_requires_review: excluded, retracted or removed source is historical evidence; omit it from current organizing result inputs".into()));
    }
    Ok(())
}
impl MemoryDB {
    pub async fn source_lifecycle_summary(
        &self,
        space: Option<&str>,
        source: &str,
        source_id: &str,
    ) -> Result<SourceLifecycleSummary, WenlanError> {
        let conn = self.conn.lock().await;
        summary_on(&conn, space, source, source_id).await
    }
}

pub(super) async fn historical_aliases_on(
    conn: &libsql::Connection,
    page_id: &str,
    incarnation: &str,
    space_id: Option<&str>,
    space: Option<&str>,
) -> Result<Vec<String>, WenlanError> {
    let saved: Option<CitationProvenance> = load(conn, &key("page", &page_id)?).await?;
    Ok(saved
        .filter(|saved| {
            saved.incarnation == incarnation
                && saved.space_id.as_deref() == space_id
                && (space_id.is_some() || saved.space.as_deref() == space)
        })
        .map(|saved| saved.historical_locators.into_iter().collect())
        .unwrap_or_default())
}

/// Canonical source identity rename, inside the existing rebind transaction.
/// Prior keys remain historical evidence; frozen runs/proposals are never retargeted.
pub(super) async fn rebind_source_identity_on(
    conn: &libsql::Connection,
    source: &str,
    old_id: &str,
    new_id: &str,
) -> Result<(), WenlanError> {
    if old_id == new_id {
        return Ok(());
    }
    if matches!(source, "file" | "webpage") {
        let mut rows=conn.query("SELECT DISTINCT NULLIF(m.space,?3),s.id FROM memories m LEFT JOIN spaces s ON s.name=NULLIF(m.space,?3) WHERE m.source=?1 AND m.source_id=?2",libsql::params![source,old_id,UNFILED_SPACE_ID]).await.map_err(sql)?;
        let mut spaces = Vec::new();
        while let Some(row) = rows.next().await.map_err(sql)? {
            spaces.push((
                row.get::<Option<String>>(0).map_err(sql)?,
                row.get::<Option<String>>(1).map_err(sql)?,
            ));
        }
        drop(rows);
        if spaces.is_empty() {
            return Err(WenlanError::NotFound(format!(
                "rebind source not found: {old_id}"
            )));
        }
        if spaces.len() != 1 {
            return Err(WenlanError::Validation(
                "source identity rename requires one exact source Space; no state was moved".into(),
            ));
        }
        if let Some(space_id) = spaces[0].1.as_deref() {
            let old_key = key("source", &(space_id, source, old_id))?;
            let new_key = key("source", &(space_id, source, new_id))?;
            let old = metadata_value_on(conn, &old_key).await?;
            if metadata_value_on(conn, &new_key).await?.is_some() {
                return Err(WenlanError::Conflict("renamed source already has distinct lifecycle identity; no state was overwritten".into()));
            }
            if let Some(original) = old {
                let mut history: SourceHistory = serde_json::from_str(&original)?;
                let current = history.records.last().cloned().ok_or_else(|| {
                    WenlanError::Validation("source lifecycle history is empty".into())
                })?;
                if current.space_id != space_id
                    || current.source != source
                    || current.source_id != old_id
                    || history
                        .records
                        .iter()
                        .any(|record| record.space_id != space_id || record.source != source)
                {
                    return Err(WenlanError::Validation(
                        "source lifecycle identity does not match the renamed source".into(),
                    ));
                }
                if history.records.len() >= 1000 {
                    return Err(WenlanError::Validation(
                        "source lifecycle history exceeds its bound; no rename was applied".into(),
                    ));
                }
                let revision = current.revision.checked_add(1).ok_or_else(|| {
                    WenlanError::Validation("source lifecycle revision overflow".into())
                })?;
                history.records.push(SourceLifecycleRecord {
                    source_id: new_id.into(),
                    revision,
                    actor: "source_identity_rebind".into(),
                    updated_at: chrono::Utc::now().timestamp(),
                    ..current
                });
                copy_identity_metadata_on(
                    conn,
                    &old_key,
                    &original,
                    &new_key,
                    &serde_json::to_string(&history)?,
                )
                .await?;
            }
        }
    }
    // document_tags already moves in the canonical rebind. Its independent
    // decision record must move too, including human present=false tombstones.
    let old_key = super::research_runs::key("decisions", &(source, old_id))?;
    let new_key = super::research_runs::key("decisions", &(source, new_id))?;
    let original = metadata_value_on(conn, &old_key).await?;
    if metadata_value_on(conn, &new_key).await?.is_some() {
        return Err(WenlanError::Conflict("renamed source already has distinct classification decisions; no state was overwritten".into()));
    }
    if let Some(original) = original {
        let mut decisions: wenlan_types::research::SourceClassificationDecisions =
            serde_json::from_str(&original)?;
        decisions.revision = decisions.revision.checked_add(1).ok_or_else(|| {
            WenlanError::Validation("classification decision revision overflow".into())
        })?;
        copy_identity_metadata_on(
            conn,
            &old_key,
            &original,
            &new_key,
            &serde_json::to_string(&decisions)?,
        )
        .await?;
    }
    Ok(())
}
async fn metadata_value_on(
    conn: &libsql::Connection,
    key: &str,
) -> Result<Option<String>, WenlanError> {
    let mut rows = conn
        .query("SELECT value FROM app_metadata WHERE key=?1", [key])
        .await
        .map_err(sql)?;
    rows.next()
        .await
        .map_err(sql)?
        .map(|row| row.get::<String>(0).map_err(sql))
        .transpose()
}
async fn copy_identity_metadata_on(
    conn: &libsql::Connection,
    old_key: &str,
    old_value: &str,
    new_key: &str,
    new_value: &str,
) -> Result<(), WenlanError> {
    let changed=conn.execute("INSERT INTO app_metadata(key,value) SELECT ?1,?2 WHERE EXISTS(SELECT 1 FROM app_metadata WHERE key=?3 AND value=?4) ON CONFLICT(key) DO NOTHING",libsql::params![new_key,new_value,old_key,old_value]).await.map_err(sql)?;
    if changed != 1 {
        return Err(WenlanError::Conflict(
            "source identity metadata changed during rename; no state was overwritten".into(),
        ));
    }
    Ok(())
}
