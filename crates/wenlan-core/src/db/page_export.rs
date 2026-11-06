// SPDX-License-Identifier: Apache-2.0
//! Immutable export journal. All `_on` hooks use the caller's connection/transaction.
use super::{
    citation_provenance, citation_sources, commit_or_rollback, MemoryDB, UNFILED_SPACE_ID,
};
use crate::{pages::Page, read_scope::ReadScope, WenlanError};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use wenlan_types::page_export::*;

fn sql(error: libsql::Error) -> WenlanError {
    WenlanError::VectorDb(format!("page export: {error}"))
}
fn hash(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}
fn identity(page: &State) -> String {
    hash(&format!("{}\0{}", page.page.id, page.incarnation))
}
fn prefix(page: &State) -> String {
    format!("export_v1:bundle:{}:", identity(page))
}
fn head_key(page: &State) -> String {
    format!("export_v1:head:{}", identity(page))
}
fn bib_key(space: &str, source: &str) -> String {
    format!("export_v1:bib:{}", hash(&format!("{space}\0{source}")))
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
    conn.execute("INSERT INTO app_metadata(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",libsql::params![key,serde_json::to_string(value)?]).await.map_err(sql)?;
    Ok(())
}
async fn finish<T>(
    conn: &libsql::Connection,
    result: Result<T, WenlanError>,
) -> Result<T, WenlanError> {
    match result {
        Ok(value) => {
            commit_or_rollback(conn).await.map_err(sql)?;
            Ok(value)
        }
        Err(error) => {
            let _ = conn.execute("ROLLBACK", ()).await;
            Err(error)
        }
    }
}
fn missing() -> WenlanError {
    WenlanError::NotFound("page export not found".into())
}
fn public_url(value: Option<String>) -> Option<String> {
    value.filter(|v| {
        // URL fields never carry local paths, credentials, or relative destinations.
        (v.starts_with("https://") || v.starts_with("http://"))
            && !v.split('/').nth(2).unwrap_or("").contains('@')
    })
}
#[derive(Serialize, Deserialize)]
struct BibRecord {
    value: PublishExportBibliography,
    metadata_digest: String,
}
#[derive(Default, Serialize, Deserialize)]
struct Head {
    revision: u64,
    last_id: Option<String>,
}
struct State {
    page: Page,
    incarnation: String,
    source_revision: i64,
    space_id: Option<String>,
    raw_citations: Option<String>,
}
async fn state_on(conn: &libsql::Connection, id: &str) -> Result<Option<State>, WenlanError> {
    let mut rows=conn.query("SELECT id,title,summary,content,entity_id,space,source_memory_ids,version,status,created_at,last_compiled,last_modified,COALESCE(sources_updated_count,0),stale_reason,COALESCE(user_edited,0),COALESCE(changelog,'[]'),COALESCE(creation_kind,'distilled'),COALESCE(review_status,'confirmed'),workspace,citations,COALESCE(kind,'concept'),refresh_blocked_reason,COALESCE(review_required,0),incarnation,COALESCE(source_revision,0) FROM pages WHERE id=?1 AND status IN ('active','archived') AND COALESCE(kind,'concept')!='entity'",[id]).await.map_err(sql)?;
    let Some(row) = rows.next().await.map_err(sql)? else {
        return Ok(None);
    };
    let mut page = MemoryDB::row_to_page(&row)?;
    let incarnation = row.get(23).map_err(sql)?;
    let source_revision = row.get(24).map_err(sql)?;
    let raw_citations = row.get(19).map_err(sql)?;
    drop(rows);
    // Includes retained/detached maps; NULL must not silently erase saved references.
    citation_provenance::apply_on(conn, &mut page).await?;
    let space_id = match page.workspace.as_deref() {
        Some(name) => {
            let mut rows = conn
                .query("SELECT id FROM spaces WHERE name=?1", [name])
                .await
                .map_err(sql)?;
            rows.next()
                .await
                .map_err(sql)?
                .map(|r| r.get::<String>(0).map_err(sql))
                .transpose()?
        }
        None => None,
    };
    Ok(Some(State {
        page,
        incarnation,
        source_revision,
        space_id,
        raw_citations,
    }))
}
async fn authorize_on(
    conn: &libsql::Connection,
    page: &State,
    scope: &ReadScope,
) -> Result<(), WenlanError> {
    if !scope.matches(page.page.workspace.as_deref())
        || (page.page.workspace.is_some() && page.space_id.is_none())
    {
        return Err(missing());
    }
    // Same automatic, Full-only gate as existing exports, evaluated under this connection lock.
    let mut rows=conn.query("SELECT key,value FROM app_metadata WHERE key IN ('truth_cutover_generation','truth_cutover_fence')",()).await.map_err(sql)?;
    let mut generation = 0;
    while let Some(row) = rows.next().await.map_err(sql)? {
        let key: String = row.get(0).map_err(sql)?;
        let value: String = row.get(1).map_err(sql)?;
        if key == "truth_cutover_generation" {
            generation = value
                .trim()
                .parse::<i64>()
                .map_err(|_| WenlanError::Validation("invalid truth generation".into()))?;
        } else if super::CutoverFence::parse(&value)
            .is_none_or(|f| f.phase == super::CutoverPhase::Preparing)
        {
            return Err(missing());
        }
    }
    drop(rows);
    if generation != 0 {
        let truths =
            MemoryDB::page_truth_states_on_conn(conn, std::slice::from_ref(&page.page.id)).await?;
        let truth =
            truths
                .get(&page.page.id)
                .copied()
                .unwrap_or(super::truth_exposure::PageTruth {
                    support: crate::truth_contract::Support::Unevaluated,
                    human_reviewed: false,
                });
        if crate::truth_contract::visible_at(
            generation,
            &crate::truth_contract::TruthGrant::Automatic,
            &page.page.id,
            truth.support,
            truth.human_reviewed,
        ) != crate::truth_contract::Visibility::Full
        {
            return Err(missing());
        }
    }
    Ok(())
}
async fn bundles_on(
    conn: &libsql::Connection,
    page: &State,
) -> Result<Vec<PageExportBundle>, WenlanError> {
    let mut rows = conn
        .query(
            "SELECT value FROM app_metadata WHERE key LIKE ?1 ORDER BY key",
            [format!("{}%", prefix(page))],
        )
        .await
        .map_err(sql)?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.map_err(sql)? {
        let value: PageExportBundle = serde_json::from_str(&row.get::<String>(0).map_err(sql)?)?;
        if value.page_incarnation == page.incarnation && value.space_id == page.space_id {
            out.push(value)
        }
    }
    Ok(out)
}
fn content_digest(bundle: &PageExportBundle) -> Result<String, WenlanError> {
    let mut value = serde_json::to_value(bundle)?;
    for key in ["id", "digest", "export_revision", "captured_at"] {
        value.as_object_mut().unwrap().remove(key);
    }
    Ok(hash(&serde_json::to_string(&value)?))
}
async fn append_on(
    conn: &libsql::Connection,
    page: &State,
    mut bundle: PageExportBundle,
) -> Result<PageExportBundle, WenlanError> {
    let mut head: Head = load(conn, &head_key(page)).await?.unwrap_or_default();
    bundle.digest = content_digest(&bundle)?;
    for old in bundles_on(conn, page).await? {
        if old.digest == bundle.digest {
            return Ok(old);
        }
    }
    head.revision += 1;
    bundle.export_revision = head.revision;
    bundle.id = hash(&format!(
        "{}:{}:{}",
        identity(page),
        head.revision,
        bundle.digest
    ));
    conn.execute(
        "INSERT INTO app_metadata(key,value) VALUES(?1,?2)",
        libsql::params![
            format!("{}{}", prefix(page), bundle.id),
            serde_json::to_string(&bundle)?
        ],
    )
    .await
    .map_err(sql)?;
    head.last_id = Some(bundle.id.clone());
    save(conn, &head_key(page), &head).await?;
    Ok(bundle)
}
async fn source_presentation(
    conn: &libsql::Connection,
    page: &State,
    locator: &str,
    kind: &str,
    expected: Option<&str>,
    historical: bool,
    prior: &[PageExportBundle],
) -> Result<(PageExportSource, Option<PageExportBibliography>), WenlanError> {
    let previous = prior
        .iter()
        .rev()
        .flat_map(|b| b.sources.iter().map(move |s| (b, s)))
        .find(|(_, s)| {
            s.locator == locator
                && s.source_kind == kind
                && expected.is_none_or(|d| s.text_digest.as_deref() == Some(d))
        });
    let scope = page
        .page
        .workspace
        .as_ref()
        .map(|s| ReadScope::Space(s.clone()))
        .unwrap_or(ReadScope::Uncategorized);
    let current = if historical {
        None
    } else {
        match citation_sources::read_on(conn, locator, &scope).await {
            Ok(source) => source,
            Err(WenlanError::Validation(_)) => None,
            Err(error) => return Err(error),
        }
    };
    let matches = current
        .as_ref()
        .is_some_and(|s| expected.is_none_or(|d| d == s.snapshot.text_digest));
    if !matches {
        if let Some((bundle, source)) = previous {
            let mut source = source.clone();
            source.historical |= historical;
            let bib = bundle
                .bibliography
                .iter()
                .find(|b| b.source_id == source.source_id)
                .cloned();
            return Ok((source, bib));
        }
    }
    let Some(current) = current.filter(|_| matches) else {
        return Ok((
            PageExportSource {
                locator: locator.into(),
                source_kind: kind.into(),
                source_id: locator.into(),
                title: if kind == "external_url" {
                    public_url(Some(locator.into())).unwrap_or_else(|| "Saved webpage".into())
                } else {
                    "Saved source (metadata unavailable)".into()
                },
                source_agent: None,
                url: if kind == "external_url" {
                    public_url(Some(locator.into()))
                } else {
                    None
                },
                text_digest: expected.map(str::to_owned),
                historical,
            },
            None,
        ));
    };
    let mut rows=conn.query("SELECT title,source_agent,url,content_hash FROM memories WHERE source=?1 AND source_id=?2 AND COALESCE(pending_revision,0)=0 AND NULLIF(space,?3) IS ?4 ORDER BY chunk_index LIMIT 1",libsql::params![current.snapshot.source.as_str(),current.snapshot.source_id.as_str(),UNFILED_SPACE_ID,current.snapshot.space.as_deref()]).await.map_err(sql)?;
    let row = rows.next().await.map_err(sql)?.ok_or_else(missing)?;
    let title: String = row.get(0).map_err(sql)?;
    let agent: Option<String> = row.get(1).map_err(sql)?;
    let url: Option<String> = row.get(2).map_err(sql)?;
    let import_hash: Option<String> = row.get(3).map_err(sql)?;
    drop(rows);
    let mut source = PageExportSource {
        locator: locator.into(),
        source_kind: kind.into(),
        source_id: current.snapshot.source_id.clone(),
        title,
        source_agent: agent.clone(),
        url: public_url(url),
        text_digest: Some(current.snapshot.text_digest),
        historical,
    };
    let bibliography = if agent.as_deref() == Some("zotero") {
        let record = match &page.space_id {
            Some(id) => match load::<BibRecord>(conn, &bib_key(id, &source.source_id)).await {
                Ok(record) => record,
                Err(WenlanError::Json(_)) => None,
                Err(error) => return Err(error),
            },
            None => None,
        };
        let available = record
            .as_ref()
            .is_some_and(|r| Some(r.value.import_content_hash.as_str()) == import_hash.as_deref());
        if available {
            let value = &record.as_ref().unwrap().value;
            source.title = value.title.clone();
            source.url = public_url(value.url.clone());
        }
        Some(PageExportBibliography {
            source_id: source.source_id.clone(),
            title: source.title.clone(),
            bibtex: record
                .as_ref()
                .filter(|_| available)
                .map(|r| r.value.bibtex.clone()),
            availability: if available {
                BibliographyAvailability::Available
            } else if record.is_some() {
                BibliographyAvailability::GenerationMismatch
            } else {
                BibliographyAvailability::Missing
            },
        })
    } else {
        None
    };
    Ok((source, bibliography))
}
async fn build_on(
    conn: &libsql::Connection,
    page: &State,
    kind: PageExportKind,
    body: &str,
    citations: Option<Vec<wenlan_types::pages::PageCitation>>,
    selected: &[String],
    pending: Option<(String, String)>,
) -> Result<PageExportBundle, WenlanError> {
    // Drafts remain unadopted evidence. Membership review has not been resolved
    // merely because a freshly assembled draft found the same source text.
    let citations = if kind == PageExportKind::PendingDraft {
        let mut draft = page.page.clone();
        draft.content = body.into();
        draft.citations = citations.clone().unwrap_or_default();
        super::zotero_scope_impacts::apply_on(conn, &mut draft).await?;
        citations.map(|_| draft.citations)
    } else {
        citations
    };
    let mut prior = bundles_on(conn, page).await?;
    prior.sort_by_key(|b| b.export_revision);
    let mut sources = Vec::new();
    let mut bibliography = Vec::new();
    let mut seen = BTreeSet::new();
    for citation in citations
        .iter()
        .flatten()
        .filter(|c| c.source_kind != "authored")
    {
        if !seen.insert((citation.source_kind.clone(), citation.locator.clone())) {
            continue;
        }
        let (source, bib) = source_presentation(
            conn,
            page,
            &citation.locator,
            &citation.source_kind,
            citation.cited_digest.as_deref(),
            citation.historical,
            &prior,
        )
        .await?;
        sources.push(source);
        if let Some(bib) = bib {
            if !bibliography
                .iter()
                .any(|b: &PageExportBibliography| b.source_id == bib.source_id)
            {
                bibliography.push(bib)
            }
        }
    }
    for locator in selected {
        if sources
            .iter()
            .any(|s| s.locator == *locator || s.source_id == *locator)
        {
            continue;
        }
        let (source, bib) =
            source_presentation(conn, page, locator, "memory", None, false, &prior).await?;
        sources.push(source);
        if let Some(bib) = bib {
            if !bibliography.iter().any(|b| b.source_id == bib.source_id) {
                bibliography.push(bib)
            }
        }
    }
    let occurrences = crate::citations::marker_spans(body);
    let map_available = if occurrences.is_empty() {
        citations.as_ref().is_none_or(|map| map.is_empty())
    } else {
        citations.as_ref().is_some_and(|map| {
            map.len() == occurrences.len()
                && occurrences.iter().enumerate().all(|(index, (start, end))| {
                    let marker = body[start + 1..end - 1].parse::<u32>().ok();
                    map.iter()
                        .any(|c| c.occurrence == (index as u32 + 1) && Some(c.marker) == marker)
                })
        })
    };
    let metadata_available = citations.iter().flatten().all(|citation| {
        matches!(citation.source_kind.as_str(), "authored" | "external_file")
            || sources.iter().any(|source| {
                source.locator == citation.locator
                    && source.title != "Saved source (metadata unavailable)"
            })
    });
    let mut generations = std::collections::BTreeMap::new();
    let generation_consistent = citations.iter().flatten().all(|c| {
        let identity = (&c.source_kind, &c.locator);
        let generation = (&c.cited_digest, c.cited_version);
        generations
            .insert(identity, generation)
            .is_none_or(|old| old == generation)
    });
    let references_available = map_available && metadata_available && generation_consistent;
    Ok(PageExportBundle {
        id: String::new(),
        digest: String::new(),
        kind,
        page_id: page.page.id.clone(),
        page_incarnation: page.incarnation.clone(),
        space_id: page.space_id.clone(),
        version: page.page.version,
        export_revision: 0,
        source_revision: page.source_revision,
        title: page.page.title.clone(),
        content: body.into(),
        last_modified: page.page.last_modified.clone(),
        captured_at: chrono::Utc::now().timestamp_millis(),
        citations,
        sources,
        bibliography,
        references_available,
        pending_revision_id: pending.as_ref().map(|p| p.0.clone()),
        pending_card_digest: pending.map(|p| p.1),
    })
}
/// Hook after the final saved body/citation update, inside its transaction.
pub(super) async fn capture_on(
    conn: &libsql::Connection,
    id: &str,
) -> Result<Option<PageExportBundle>, WenlanError> {
    let Some(page) = state_on(conn, id).await? else {
        return Ok(None);
    };
    let citations = if page.raw_citations.is_some() || !page.page.citations.is_empty() {
        Some(page.page.citations.clone())
    } else {
        None
    };
    let value = build_on(
        conn,
        &page,
        PageExportKind::Saved,
        &page.page.content,
        citations,
        &page.page.source_memory_ids,
        None,
    )
    .await?;
    Ok(Some(append_on(conn, &page, value).await?))
}
pub(super) async fn capture_history_on(
    conn: &libsql::Connection,
    id: &str,
) -> Result<(), libsql::Error> {
    capture_on(conn, id)
        .await
        .map(|_| ())
        .map_err(|e| libsql::Error::Misuse(e.to_string()))
}

struct Pending {
    id: String,
    body: String,
    structured: serde_json::Value,
    digest: String,
}
async fn pending_on(
    conn: &libsql::Connection,
    page: &State,
    id: &str,
) -> Result<Option<Pending>, WenlanError> {
    // Match adoption: an edited single row uses content; multi-chunk cards
    // recover source_text only through the canonical privacy/time transforms.
    let mut rows=conn.query("SELECT content,source_text,structured_fields,COALESCE(created_at,last_modified),EXISTS(SELECT 1 FROM memories sibling WHERE sibling.source='memory' AND sibling.source_id=memories.source_id AND sibling.chunk_index>0) FROM memories WHERE source='memory' AND source_id=?1 AND supersedes=?2 AND pending_revision=1 AND chunk_index=0 AND NULLIF(space,?3) IS ?4",libsql::params![id,page.page.id.as_str(),UNFILED_SPACE_ID,page.page.workspace.as_deref()]).await.map_err(sql)?;
    let Some(row) = rows.next().await.map_err(sql)? else {
        return Ok(None);
    };
    let content: String = row.get(0).map_err(sql)?;
    let raw: Option<String> = row.get(2).map_err(sql)?;
    let Some(raw) = raw else { return Ok(None) };
    let body = if row.get::<i64>(4).map_err(sql)? != 0 {
        let Some(raw) = row.get::<Option<String>>(1).map_err(sql)? else {
            return Ok(None);
        };
        super::memory_point_reads::rehydrate_staged_body(&raw, row.get::<i64>(3).map_err(sql)?)
    } else {
        content
    };
    drop(rows);
    let value: serde_json::Value = serde_json::from_str(&raw)?;
    if value["revision_kind"] != "page_write"
        || value["target_kind"] != "page"
        || value["revises_page"] != page.page.id
        || value["page_version"].as_i64() != Some(page.page.version)
        || value["source_revision"].as_i64() != Some(page.source_revision)
        || value["citation_body_digest"].as_str() != Some(hash(&body).as_str())
    {
        return Ok(None);
    }
    // A card without an incarnation cannot distinguish a deleted/recreated target.
    if value.get("page_incarnation").and_then(|v| v.as_str()) != Some(page.incarnation.as_str()) {
        return Ok(None);
    }
    if let Some(raw) = value.get("citation_source_snapshots") {
        let snapshots: Vec<citation_sources::CitationSourceSnapshot> =
            serde_json::from_value(raw.clone())?;
        match citation_sources::guard_on(conn, &snapshots).await {
            Ok(()) => {}
            Err(WenlanError::Conflict(_)) | Err(WenlanError::Validation(_)) => return Ok(None),
            Err(error) => return Err(error),
        }
    }
    Ok(Some(Pending {
        id: id.into(),
        digest: hash(&format!("{body}\0{raw}")),
        body,
        structured: value,
    }))
}
pub(super) async fn capture_pending_on(
    conn: &libsql::Connection,
    page_id: &str,
    revision_id: &str,
) -> Result<(), WenlanError> {
    let Some(page) = state_on(conn, page_id).await? else {
        return Ok(());
    };
    let Some(pending) = pending_on(conn, &page, revision_id).await? else {
        return Ok(());
    };
    let citations = pending
        .structured
        .get("citations")
        .map(|v| serde_json::from_value(v.clone()))
        .transpose()?;
    let selected: Vec<String> = pending
        .structured
        .get("source_memory_ids")
        .map(|v| serde_json::from_value(v.clone()))
        .transpose()?
        .unwrap_or_default();
    let bundle = build_on(
        conn,
        &page,
        PageExportKind::PendingDraft,
        &pending.body,
        citations,
        &selected,
        Some((pending.id, pending.digest)),
    )
    .await?;
    append_on(conn, &page, bundle).await?;
    Ok(())
}
async fn valid_pending(
    conn: &libsql::Connection,
    page: &State,
    bundle: &PageExportBundle,
) -> Result<bool, WenlanError> {
    if bundle.kind != PageExportKind::PendingDraft {
        return Ok(true);
    }
    let Some(id) = bundle.pending_revision_id.as_deref() else {
        return Ok(false);
    };
    Ok(pending_on(conn, page, id)
        .await?
        .is_some_and(|p| Some(p.digest) == bundle.pending_card_digest))
}
impl MemoryDB {
    pub async fn capture_page_export(
        &self,
        id: &str,
        scope: &ReadScope,
    ) -> Result<PageExportBundle, WenlanError> {
        let conn = self.conn.lock().await;
        conn.execute("BEGIN", ()).await.map_err(sql)?;
        let result=async{
            let page=state_on(&conn,id).await?.ok_or_else(missing)?;authorize_on(&conn,&page,scope).await?;
            let saved=capture_on(&conn,id).await?.ok_or_else(missing)?;
            let mut rows=conn.query("SELECT DISTINCT source_id FROM memories WHERE source='memory' AND supersedes=?1 AND pending_revision=1 LIMIT 1000",[id]).await.map_err(sql)?;
            let mut ids=Vec::new();while let Some(row)=rows.next().await.map_err(sql)?{ids.push(row.get::<String>(0).map_err(sql)?)}drop(rows);
            for revision in ids{capture_pending_on(&conn,id,&revision).await?}Ok(saved)
        }.await;
        finish(&conn, result).await
    }
    pub async fn page_export_versions(
        &self,
        id: &str,
        scope: &ReadScope,
    ) -> Result<PageExportVersionsResponse, WenlanError> {
        let conn = self.conn.lock().await;
        let page = state_on(&conn, id).await?.ok_or_else(missing)?;
        authorize_on(&conn, &page, scope).await?;
        let mut versions = Vec::new();
        let mut saved = BTreeSet::new();
        for bundle in bundles_on(&conn, &page).await? {
            if !valid_pending(&conn, &page, &bundle).await? {
                continue;
            }
            if bundle.kind == PageExportKind::Saved {
                saved.insert(bundle.version);
            }
            let bib = !bundle.bibliography.is_empty()
                && bundle
                    .bibliography
                    .iter()
                    .all(|b| b.availability == BibliographyAvailability::Available);
            versions.push(PageExportVersion {
                bundle_id: Some(bundle.id),
                kind: bundle.kind,
                version: bundle.version,
                export_revision: Some(bundle.export_revision),
                title: Some(bundle.title),
                saved_at: bundle.last_modified,
                captured_at: Some(bundle.captured_at),
                references_available: bundle.references_available,
                bibliography_available: bib,
                unavailable_reason: (!bundle.references_available)
                    .then(|| "References were not retained for this version".into()),
            });
        }
        let mut rows=conn.query("SELECT version,title,created_at FROM page_history WHERE page_id=?1 ORDER BY version DESC",[id]).await.map_err(sql)?;
        while let Some(row) = rows.next().await.map_err(sql)? {
            let version = row.get(0).map_err(sql)?;
            if saved.contains(&version) {
                continue;
            }
            versions.push(PageExportVersion {
                bundle_id: None,
                kind: PageExportKind::LegacyUnavailable,
                version,
                export_revision: None,
                title: row.get(1).map_err(sql)?,
                saved_at: row.get::<i64>(2).map_err(sql)?.to_string(),
                captured_at: None,
                references_available: false,
                bibliography_available: false,
                unavailable_reason: Some("References were not retained for this version".into()),
            });
        }
        versions.sort_by(|a, b| {
            b.version
                .cmp(&a.version)
                .then(b.export_revision.cmp(&a.export_revision))
        });
        Ok(PageExportVersionsResponse { versions })
    }
    pub async fn page_export_bundle(
        &self,
        id: &str,
        bundle_id: &str,
        scope: &ReadScope,
    ) -> Result<PageExportBundle, WenlanError> {
        if bundle_id.len() != 64 || !bundle_id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(missing());
        }
        let conn = self.conn.lock().await;
        let page = state_on(&conn, id).await?.ok_or_else(missing)?;
        authorize_on(&conn, &page, scope).await?;
        let bundle: PageExportBundle = load(&conn, &format!("{}{bundle_id}", prefix(&page)))
            .await?
            .ok_or_else(missing)?;
        if bundle.page_id != id
            || bundle.page_incarnation != page.incarnation
            || bundle.space_id != page.space_id
            || bundle.id != bundle_id
            || bundle.digest != content_digest(&bundle)?
            || !valid_pending(&conn, &page, &bundle).await?
        {
            return Err(missing());
        }
        Ok(bundle)
    }
    pub async fn publish_export_bibliography(
        &self,
        mut value: PublishExportBibliography,
    ) -> Result<(), WenlanError> {
        if !regex::Regex::new(r"(?i)@[a-z]+\s*\{")
            .unwrap()
            .is_match(&value.bibtex)
            || value.bibtex.len() > 256_000
            || value.title.len() > 16_000
            || value.import_content_hash.is_empty()
        {
            return Err(WenlanError::Validation(
                "invalid bibliography record".into(),
            ));
        }
        value.url = public_url(value.url);
        let conn = self.conn.lock().await;
        conn.execute("BEGIN", ()).await.map_err(sql)?;
        let result=async{
            let mut rows=conn.query("SELECT name FROM spaces WHERE id=?1",[value.space_id.as_str()]).await.map_err(sql)?;
            let name=rows.next().await.map_err(sql)?.ok_or_else(missing)?.get::<String>(0).map_err(sql)?;drop(rows);
            let mut rows=conn.query("SELECT COUNT(*),SUM(CASE WHEN content_hash=?3 AND source_agent='zotero' AND space=?2 THEN 1 ELSE 0 END) FROM memories WHERE source='file' AND source_id=?1 AND COALESCE(pending_revision,0)=0",libsql::params![value.source_id.as_str(),name,value.import_content_hash.as_str()]).await.map_err(sql)?;
            let row=rows.next().await.map_err(sql)?.ok_or_else(missing)?;let count:i64=row.get(0).map_err(sql)?;let matched:Option<i64>=row.get(1).map_err(sql)?;drop(rows);
            if count==0 || matched!=Some(count){return Err(WenlanError::Conflict("bibliography import generation changed".into()))}
            let record=BibRecord{metadata_digest:hash(&serde_json::to_string(&value)?),value};
            save(&conn,&bib_key(&record.value.space_id,&record.value.source_id),&record).await?;Ok(())
        }.await;
        finish(&conn, result).await
    }
}
/// Called from page deletion, before the row is removed; old incarnations cannot leak after ID reuse.
pub(super) async fn purge_on(conn: &libsql::Connection, id: &str) -> Result<(), WenlanError> {
    let mut rows = conn
        .query("SELECT incarnation FROM pages WHERE id=?1", [id])
        .await
        .map_err(sql)?;
    let Some(row) = rows.next().await.map_err(sql)? else {
        return Ok(());
    };
    let incarnation: String = row.get(0).map_err(sql)?;
    drop(rows);
    let identity = hash(&format!("{id}\0{incarnation}"));
    conn.execute(
        "DELETE FROM app_metadata WHERE key LIKE ?1 OR key=?2",
        libsql::params![
            format!("export_v1:bundle:{identity}:%"),
            format!("export_v1:head:{identity}")
        ],
    )
    .await
    .map_err(sql)?;
    Ok(())
}
#[cfg(test)]
#[path = "page_export_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "page_export_guard_tests.rs"]
mod guard_tests;
