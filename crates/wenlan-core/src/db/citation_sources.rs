//! Exact source generations used by citation validation and its write fence.
use super::{MemoryDB, UNFILED_SPACE_ID};
use crate::{read_scope::ReadScope, WenlanError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const MAX_CHUNKS: usize = 10_000;
const MAX_TEXT_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CitationSourceSnapshot {
    pub locator: String,
    pub source: String,
    pub source_id: String,
    pub space: Option<String>,
    pub version: i64,
    pub chunk_count: usize,
    pub fingerprint: String,
    pub text_digest: String,
}

pub struct CitationSource {
    pub snapshot: CitationSourceSnapshot,
    pub source_kind: String,
    pub text: String,
    pub full_text: String,
}

#[derive(Serialize)]
struct SourceRow {
    id: String,
    title: String,
    source: String,
    source_id: String,
    space: Option<String>,
    version: i64,
    content_hash: Option<String>,
    chunk_index: i64,
    content: String,
    source_agent: Option<String>,
    last_modified: i64,
}

fn error(error: libsql::Error) -> WenlanError {
    WenlanError::VectorDb(format!("citation source snapshot: {error}"))
}

pub(super) async fn read_on(
    conn: &libsql::Connection,
    locator: &str,
    scope: &ReadScope,
) -> Result<Option<CitationSource>, WenlanError> {
    let (mode, space) = match scope {
        ReadScope::Global => ("global", ""),
        ReadScope::Space(space) => ("space", space.as_str()),
        ReadScope::Uncategorized => ("unfiled", ""),
    };
    // The scope is applied in SQL. The count also detects a split document or
    // ambiguous locator without returning any out-of-Space text to the caller.
    let mut rows = conn
        .query(
            "WITH candidates AS (
           SELECT DISTINCT source,source_id FROM memories
           WHERE (source_id=?1 OR (id=?1 AND source!='episode'))
             AND source!='episode' AND COALESCE(pending_revision,0)=0
             AND source_id NOT IN (SELECT supersedes FROM memories WHERE supersedes IS NOT NULL
                 AND pending_revision=0 AND source='memory' AND supersede_mode='hide')
         ), all_chunks AS (
           SELECT m.* FROM memories m JOIN candidates c
             ON m.source=c.source AND m.source_id=c.source_id
           WHERE COALESCE(m.pending_revision,0)=0
         )
         SELECT id,source,source_id,space,COALESCE(version,1),content_hash,
                chunk_index,content,source_agent,last_modified,
                (SELECT COUNT(*) FROM all_chunks),title
         FROM all_chunks
         WHERE ?2='global' OR (?2='space' AND space=?3)
            OR (?2='unfiled' AND (space IS NULL OR space=?4))
         ORDER BY source,source_id,chunk_index LIMIT 10001",
            libsql::params![locator, mode, space, UNFILED_SPACE_ID],
        )
        .await
        .map_err(error)?;
    let mut chunks = Vec::new();
    let mut total = 0;
    let mut bytes = 0usize;
    while let Some(row) = rows.next().await.map_err(error)? {
        total = row.get::<i64>(10).map_err(error)? as usize;
        let content: String = row.get(7).map_err(error)?;
        bytes = bytes.saturating_add(content.len());
        if bytes > MAX_TEXT_BYTES || chunks.len() >= MAX_CHUNKS {
            return Err(WenlanError::Validation(
                "source_read_refused: source exceeds the complete-read limit; nothing was truncated".into(),
            ));
        }
        chunks.push(SourceRow {
            id: row.get(0).map_err(error)?,
            title: row.get(11).map_err(error)?,
            source: row.get(1).map_err(error)?,
            source_id: row.get(2).map_err(error)?,
            space: row
                .get::<Option<String>>(3)
                .map_err(error)?
                .filter(|s| s != UNFILED_SPACE_ID),
            version: row.get(4).map_err(error)?,
            content_hash: row.get(5).map_err(error)?,
            chunk_index: row.get(6).map_err(error)?,
            content,
            source_agent: row.get(8).map_err(error)?,
            last_modified: row.get(9).map_err(error)?,
        });
    }
    let Some(first) = chunks.first() else {
        return Ok(None);
    };
    let document = matches!(first.source.as_str(), "file" | "webpage");
    if total != chunks.len()
        || chunks.iter().enumerate().any(|(i, row)| {
            row.source != first.source
                || row.source_id != first.source_id
                || row.space != first.space
                || row.chunk_index != i as i64
                || (document
                    && (row.title != first.title
                        || row.source_agent != first.source_agent
                        || row.version != first.version
                        || row.content_hash != first.content_hash
                        || row.last_modified != first.last_modified))
        })
    {
        return Ok(None);
    }
    let joined_text = chunks
        .iter()
        .map(|row| row.content.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    let full_text = if first.source_id != locator {
        chunks
            .iter()
            .find(|row| row.id == locator)
            .map(|row| row.content.clone())
            .unwrap_or_default()
    } else {
        joined_text
    };
    let text = if document {
        full_text.clone()
    } else {
        chunks
            .iter()
            .find(|row| row.id == locator)
            .unwrap_or(first)
            .content
            .chars()
            .take(800)
            .collect()
    };
    let mut snapshot = CitationSourceSnapshot {
        locator: locator.into(),
        source: first.source.clone(),
        source_id: first.source_id.clone(),
        space: first.space.clone(),
        version: chunks.iter().map(|row| row.version).max().unwrap_or(1),
        chunk_count: chunks.len(),
        fingerprint: format!("{:x}", Sha256::digest(serde_json::to_vec(&chunks)?)),
        text_digest: super::sentence_history::cited_text_digest(&full_text),
    };
    super::citation_provenance::stamp_snapshot_on(conn, &mut snapshot).await?;
    Ok(Some(CitationSource {
        source_kind: crate::citations::resolve_page_evidence_source_kind(
            &first.source,
            first.source_agent.as_deref(),
            &first.source_id,
        )
        .into(),
        snapshot,
        text,
        full_text,
    }))
}

pub(super) async fn guard_on(
    conn: &libsql::Connection,
    snapshots: &[CitationSourceSnapshot],
) -> Result<(), WenlanError> {
    for expected in snapshots {
        let scope = expected
            .space
            .clone()
            .map(ReadScope::Space)
            .unwrap_or(ReadScope::Uncategorized);
        let current = if expected.source == "page" {
            read_page_on(conn, &expected.locator, &scope).await?
        } else {
            read_on(conn, &expected.locator, &scope).await?
        };
        if current.as_ref().map(|s| &s.snapshot) != Some(expected) {
            return Err(WenlanError::Conflict(
                "source_read_refused: citation source changed after validation; reread exact sources and retry".into(),
            ));
        }
    }
    Ok(())
}

impl MemoryDB {
    pub async fn citation_sources(
        &self,
        locators: &[String],
        scope: &ReadScope,
    ) -> Result<Vec<Option<CitationSource>>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut sources = Vec::with_capacity(locators.len());
        for locator in locators {
            sources.push(read_on(&conn, locator, scope).await?);
        }
        Ok(sources)
    }
}

impl MemoryDB {
    pub async fn citation_source_metadata(
        &self,
        locators: &[String],
        scope: &ReadScope,
    ) -> Result<Vec<wenlan_types::MemoryItem>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut items = Vec::new();
        let columns =
            Self::memory_item_columns("title", &super::superseder_archives(scope, "memories"));
        for locator in locators {
            let Some(source) = read_on(&conn, locator, scope).await? else {
                continue;
            };
            let snapshot = source.snapshot;
            let sql = format!(
                "SELECT {columns} FROM memories WHERE source=?1 AND source_id=?2
                AND COALESCE(pending_revision,0)=0 AND (space IS ?3 OR (?3 IS NULL AND space=?4))
                ORDER BY CASE WHEN id=?5 THEN 0 ELSE 1 END,chunk_index LIMIT 1"
            );
            let mut rows = conn
                .query(
                    &sql,
                    libsql::params![
                        snapshot.source,
                        snapshot.source_id,
                        snapshot.space,
                        UNFILED_SPACE_ID,
                        locator.as_str()
                    ],
                )
                .await
                .map_err(error)?;
            if let Some(row) = rows.next().await.map_err(error)? {
                let mut item = Self::memory_item_from_row(&row);
                item.source_id = locator.clone();
                items.push(item);
            }
        }
        Ok(items)
    }
}

async fn read_page_on(
    conn: &libsql::Connection,
    locator: &str,
    scope: &ReadScope,
) -> Result<Option<CitationSource>, WenlanError> {
    let mut rows = conn
        .query(
            "SELECT content,space,version,incarnation FROM pages WHERE id=?1 AND status='active'
         AND NOT EXISTS (SELECT 1 FROM memories WHERE source_id=?1 OR (id=?1 AND source!='episode'))",
            libsql::params![locator],
        )
        .await
        .map_err(error)?;
    let Some(row) = rows.next().await.map_err(error)? else {
        return Ok(None);
    };
    let space: Option<String> = row.get(1).map_err(error)?;
    if !scope.matches(space.as_deref()) {
        return Ok(None);
    }
    let space = space.filter(|space| space != UNFILED_SPACE_ID);
    let text: String = row.get(0).map_err(error)?;
    let version: i64 = row.get(2).map_err(error)?;
    let incarnation: String = row.get(3).map_err(error)?;
    let fingerprint = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(
            locator,
            &space,
            version,
            incarnation,
            &text
        ))?)
    );
    Ok(Some(CitationSource {
        snapshot: CitationSourceSnapshot {
            locator: locator.into(),
            source: "page".into(),
            source_id: locator.into(),
            space,
            version,
            chunk_count: 1,
            fingerprint,
            text_digest: super::sentence_history::cited_text_digest(&text),
        },
        source_kind: "memory".into(),
        full_text: text.clone(),
        text,
    }))
}

impl MemoryDB {
    pub(crate) async fn citation_page_source(
        &self,
        locator: &str,
        scope: &ReadScope,
    ) -> Result<Option<CitationSource>, WenlanError> {
        let conn = self.conn.lock().await;
        read_page_on(&conn, locator, scope).await
    }
}

#[cfg(test)]
#[path = "citation_sources_tests.rs"]
mod tests;
