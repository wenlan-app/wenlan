// SPDX-License-Identifier: Apache-2.0
//! Page chunks: a page cut into sections with one vector each, so page search
//! can find a page by any part of it, not only by the start its page vector
//! sees (`page_embedding_text`: title, summary and the first 1,500 chars).
//!
//! The table is a derived index only ([`TableClass::Cache`]): it is never a
//! place anything is written to by a person or an AI, nothing reads it back
//! into a page, and a page's rows are rebuilt from its current version. Rows
//! of an older version are invisible to search (it joins on `page_version =
//! pages.version`) until the background worker replaces them. `memory_ids`
//! is kept for the topic-file layout (D8), where a section's lines carry
//! their memory ids; it stays NULL until pages carry them.
//!
//! Measured on 316 real pages (2026-11-21, spec v2 §6d): a question about the
//! later part of a long page found it in the top 5 for 9/40 with the page
//! vector alone and 29/40 with section vectors.
//!
//! [`TableClass::Cache`]: super::table_classes::TableClass::Cache

use super::{commit_or_rollback, MemoryDB};
use crate::error::WenlanError;
use crate::pages::Page;

/// Pages chunked per background batch.
pub const PAGE_CHUNK_BATCH: usize = 16;

pub(super) const PAGE_CHUNKS_DDL: &str = "
CREATE TABLE IF NOT EXISTS page_chunks (
    page_id TEXT NOT NULL,
    page_version INTEGER NOT NULL,
    chunk_index INTEGER NOT NULL,
    heading TEXT,
    byte_start INTEGER,
    byte_end INTEGER,
    content TEXT NOT NULL,
    memory_ids TEXT,
    embedding F32_BLOB(768),
    PRIMARY KEY (page_id, chunk_index)
);
";

/// True unless `WENLAN_ENABLE_PAGE_CHUNK_VECTORS` is a falsy token (`0`,
/// `false`, `no`, `off`, case-insensitive). OPT-OUT, default ON. OFF searches
/// pages by their page vector alone, as before this table existed.
pub fn page_chunk_vectors_enabled() -> bool {
    match std::env::var("WENLAN_ENABLE_PAGE_CHUNK_VECTORS") {
        Ok(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        ),
        Err(_) => true,
    }
}

/// Pages that are chunked: active and not an entity shadow row.
const CHUNKED_PAGE_PREDICATE: &str =
    "p.status = 'active' AND COALESCE(p.kind, 'concept') != 'entity'";

/// What one background batch did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PageChunkBatch {
    /// Pages whose chunks were written.
    pub pages: usize,
    /// Chunk rows written.
    pub chunks: usize,
    /// Pages left for later (changed meanwhile, or no vector this time).
    pub skipped: usize,
}

struct PageChunkRow {
    index: usize,
    heading: Option<String>,
    byte_start: Option<i64>,
    byte_end: Option<i64>,
    content: String,
}

/// The last markdown heading at or before `byte_start`, the chunk's own first
/// line included.
fn heading_before(content: &str, byte_start: usize, chunk: &str) -> Option<String> {
    let as_heading = |line: &str| {
        let trimmed = line.trim_start();
        let hashes = trimmed.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&hashes) && trimmed[hashes..].starts_with(' ') {
            Some(trimmed[hashes..].trim().to_string()).filter(|text| !text.is_empty())
        } else {
            None
        }
    };
    if let Some(first) = chunk.lines().find(|line| !line.trim().is_empty()) {
        if let Some(heading) = as_heading(first) {
            return Some(heading);
        }
    }
    let end = byte_start.min(content.len());
    let end = (0..=end)
        .rev()
        .find(|i| content.is_char_boundary(*i))
        .unwrap_or(0);
    content[..end].lines().rev().find_map(as_heading)
}

/// The text a page chunk is embedded from: the page title, then the chunk.
pub(crate) fn page_chunk_embedding_text(title: &str, chunk: &str) -> String {
    if title.trim().is_empty() {
        chunk.to_string()
    } else {
        format!("{title}\n\n{chunk}")
    }
}

/// The page search vector arm of the chunk table: for each page that passes
/// `where_clause` (on alias `c`), the selected columns and the distance of its
/// closest chunk at column 23. `?1` is the query vector and `?2` the limit.
pub(crate) fn page_chunk_vector_sql(select: &str, where_clause: &str) -> String {
    format!(
        "SELECT {select}, MIN(vector_distance_cos(pc.embedding, vector32(?1))) AS dist \
         FROM page_chunks pc \
         JOIN pages c ON c.id = pc.page_id AND pc.page_version = c.version \
         WHERE pc.embedding IS NOT NULL AND {where_clause} \
         GROUP BY c.id ORDER BY dist ASC LIMIT ?2"
    )
}

/// Each page keeps its closer distance, page vector or best chunk; pages only
/// a chunk found are added. Re-ranked by distance and cut to `limit`.
pub(crate) fn merge_page_chunk_hits<T>(
    vector: &mut Vec<(String, f64, T)>,
    chunks: Vec<(String, f64, T)>,
    limit: usize,
) {
    for (id, distance, page) in chunks {
        match vector.iter_mut().find(|(existing, _, _)| *existing == id) {
            Some(entry) => entry.1 = entry.1.min(distance),
            None => vector.push((id, distance, page)),
        }
    }
    vector.sort_by(|left, right| {
        left.1
            .partial_cmp(&right.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.0.cmp(&right.0))
    });
    vector.truncate(limit);
}

/// The chunk arm of page search: run [`page_chunk_vector_sql`] and decode
/// its rows. A failure is logged and yields nothing, so page search falls
/// back to the page vector alone.
pub(super) async fn page_chunk_vector_hits(
    conn: &libsql::Connection,
    sql: &str,
    params: Vec<libsql::Value>,
) -> Vec<(String, f64, Page)> {
    let mut hits = Vec::new();
    let mut rows = match conn.query(sql, params).await {
        Ok(rows) => rows,
        Err(error) => {
            log::warn!("[search_pages] page chunk search failed: {error}");
            return hits;
        }
    };
    loop {
        match rows.next().await {
            Ok(Some(row)) => match (MemoryDB::row_to_page(&row), row.get::<f64>(23)) {
                (Ok(page), Ok(distance)) => hits.push((page.id.clone(), distance, page)),
                (Err(error), _) => log::warn!("[search_pages] page chunk decode: {error}"),
                (_, Err(error)) => log::warn!("[search_pages] page chunk distance: {error}"),
            },
            Ok(None) => break,
            Err(error) => {
                log::warn!("[search_pages] page chunk row scan stopped: {error}");
                break;
            }
        }
    }
    hits
}

impl MemoryDB {
    /// Migration 171 adds the `page_chunks` table. The background worker
    /// fills it ([`Self::refresh_page_chunks_batch`]).
    pub(super) async fn migrate_171_page_chunks(
        &self,
        prior_version: i64,
    ) -> Result<(), WenlanError> {
        self.backup_before_migration(171, prior_version).await?;
        let conn = self.conn.lock().await;
        conn.execute_batch(PAGE_CHUNKS_DDL)
            .await
            .map_err(|e| WenlanError::VectorDb(format!("m171 page_chunks DDL: {e}")))?;
        conn.execute("PRAGMA user_version = 171", ())
            .await
            .map_err(|e| WenlanError::VectorDb(format!("m171 bump: {e}")))?;
        log::info!("[migration] Migration 171 applied: page_chunks");
        Ok(())
    }

    /// Pages whose chunks are missing or of an older version.
    pub async fn count_pages_needing_chunks(&self) -> Result<usize, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                &format!(
                    "SELECT COUNT(*) FROM pages p WHERE {CHUNKED_PAGE_PREDICATE} \
                       AND NOT EXISTS (SELECT 1 FROM page_chunks pc \
                                        WHERE pc.page_id = p.id AND pc.page_version = p.version)"
                ),
                (),
            )
            .await
            .map_err(|e| WenlanError::VectorDb(format!("page chunk count: {e}")))?;
        let count = rows
            .next()
            .await
            .map_err(|e| WenlanError::VectorDb(format!("page chunk count row: {e}")))?
            .and_then(|row| row.get::<i64>(0).ok())
            .unwrap_or(0);
        Ok(usize::try_from(count).unwrap_or(0))
    }

    /// Chunk and embed up to `limit` pages whose chunks are missing or of an
    /// older version, and drop the rows of pages that are gone or no longer
    /// chunked. Pages are read under the connection lock, chunked and embedded
    /// with no lock held, then written in one transaction; a page whose
    /// version changed meanwhile is left for the next batch. A page with no
    /// text gets one row without a vector, so it is not picked again. Does
    /// nothing while no embedding model is loaded.
    pub async fn refresh_page_chunks_batch(
        &self,
        limit: usize,
    ) -> Result<PageChunkBatch, WenlanError> {
        if !self.embedder_ready() {
            return Ok(PageChunkBatch::default());
        }
        let limit = limit.clamp(1, PAGE_CHUNK_BATCH * 4) as i64;
        let pages: Vec<(String, i64, String, String)> = {
            let conn = self.conn.lock().await;
            conn.execute(
                &format!(
                    "DELETE FROM page_chunks WHERE page_id NOT IN \
                       (SELECT p.id FROM pages p WHERE {CHUNKED_PAGE_PREDICATE})"
                ),
                (),
            )
            .await
            .map_err(|e| WenlanError::VectorDb(format!("page chunk prune: {e}")))?;
            let mut rows = conn
                .query(
                    &format!(
                        "SELECT p.id, p.version, p.title, p.content FROM pages p \
                          WHERE {CHUNKED_PAGE_PREDICATE} \
                            AND NOT EXISTS (SELECT 1 FROM page_chunks pc \
                                             WHERE pc.page_id = p.id \
                                               AND pc.page_version = p.version) \
                          ORDER BY p.id LIMIT ?1"
                    ),
                    libsql::params![limit],
                )
                .await
                .map_err(|e| WenlanError::VectorDb(format!("page chunk select: {e}")))?;
            let mut pages = Vec::new();
            while let Some(row) = rows
                .next()
                .await
                .map_err(|e| WenlanError::VectorDb(format!("page chunk row: {e}")))?
            {
                let id: String = row
                    .get(0)
                    .map_err(|e| WenlanError::VectorDb(format!("page chunk id: {e}")))?;
                let version: i64 = row.get(1).unwrap_or(1);
                let title: String = row
                    .get::<Option<String>>(2)
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                let content: String = row
                    .get::<Option<String>>(3)
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                pages.push((id, version, title, content));
            }
            pages
        };
        if pages.is_empty() {
            return Ok(PageChunkBatch::default());
        }

        let metadata = std::collections::HashMap::new();
        let mut planned: Vec<(String, i64, Vec<PageChunkRow>)> = Vec::new();
        let mut texts: Vec<String> = Vec::new();
        for (id, version, title, content) in &pages {
            let mut rows = Vec::new();
            if !content.trim().is_empty() {
                // A `.md` path selects the markdown strategy, the same one
                // memory rows from Markdown files use.
                let chunks = self
                    .chunker
                    .chunk(content, title, &format!("{id}.md"), &metadata);
                for chunk in chunks {
                    if chunk.content.trim().is_empty() {
                        continue;
                    }
                    let (start, end) = chunk.byte_range.unwrap_or((0, content.len()));
                    rows.push(PageChunkRow {
                        index: rows.len(),
                        heading: heading_before(content, start, &chunk.content),
                        byte_start: i64::try_from(start).ok(),
                        byte_end: i64::try_from(end).ok(),
                        content: chunk.content,
                    });
                }
            }
            for row in &rows {
                texts.push(page_chunk_embedding_text(title, &row.content));
            }
            planned.push((id.clone(), *version, rows));
        }
        let embeddings = self.generate_embeddings_by_length(texts).await?;

        let conn = self.conn.lock().await;
        conn.execute("BEGIN", ())
            .await
            .map_err(|e| WenlanError::VectorDb(format!("page chunk begin: {e}")))?;
        let mut batch = PageChunkBatch::default();
        let mut next_vector = 0usize;
        let result: Result<(), WenlanError> = async {
            for (id, version, rows) in &planned {
                let vectors = &embeddings[next_vector..next_vector + rows.len()];
                next_vector += rows.len();
                // Every chunk needs its vector, or the page waits for a later
                // batch: a page found by only some of its sections would
                // rank as if the rest were not there.
                if vectors.iter().any(Option::is_none) {
                    batch.skipped += 1;
                    continue;
                }
                let mut current = conn
                    .query(
                        "SELECT version FROM pages WHERE id = ?1",
                        libsql::params![id.clone()],
                    )
                    .await
                    .map_err(|e| WenlanError::VectorDb(format!("page chunk recheck: {e}")))?;
                let still = current
                    .next()
                    .await
                    .map_err(|e| WenlanError::VectorDb(format!("page chunk recheck row: {e}")))?
                    .and_then(|row| row.get::<i64>(0).ok());
                drop(current);
                if still != Some(*version) {
                    batch.skipped += 1;
                    continue;
                }
                conn.execute(
                    "DELETE FROM page_chunks WHERE page_id = ?1",
                    libsql::params![id.clone()],
                )
                .await
                .map_err(|e| WenlanError::VectorDb(format!("page chunk clear: {e}")))?;
                if rows.is_empty() {
                    conn.execute(
                        "INSERT INTO page_chunks (page_id, page_version, chunk_index, content) \
                         VALUES (?1, ?2, 0, '')",
                        libsql::params![id.clone(), *version],
                    )
                    .await
                    .map_err(|e| WenlanError::VectorDb(format!("page chunk mark: {e}")))?;
                }
                for (row, vector) in rows.iter().zip(vectors) {
                    let vector = vector.as_ref().expect("checked above");
                    conn.execute(
                        "INSERT INTO page_chunks (page_id, page_version, chunk_index, heading, \
                                                  byte_start, byte_end, content, embedding) \
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, vector32(?8))",
                        libsql::params![
                            id.clone(),
                            *version,
                            row.index as i64,
                            row.heading.clone(),
                            row.byte_start,
                            row.byte_end,
                            row.content.clone(),
                            Self::vec_to_sql(vector)
                        ],
                    )
                    .await
                    .map_err(|e| WenlanError::VectorDb(format!("page chunk insert: {e}")))?;
                    batch.chunks += 1;
                }
                batch.pages += 1;
            }
            Ok(())
        }
        .await;
        if let Err(error) = result {
            let _ = conn.execute("ROLLBACK", ()).await;
            return Err(error);
        }
        commit_or_rollback(&conn)
            .await
            .map_err(|e| WenlanError::VectorDb(format!("page chunk commit: {e}")))?;
        Ok(batch)
    }
}

#[cfg(test)]
#[path = "page_chunks_test.rs"]
mod tests;
