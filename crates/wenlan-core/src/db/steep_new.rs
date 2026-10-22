// SPDX-License-Identifier: Apache-2.0
//! Reads behind steep's new pages (`crate::steep_new`): the memory rows no
//! page cites yet, and the full text of the ones a claim takes.

use super::MemoryDB;
use crate::error::WenlanError;

/// One memory row no page cites yet, with only the start of its text.
#[derive(Debug, Clone)]
pub(crate) struct UnpagedRow {
    pub source_id: String,
    pub chunk_index: i64,
    pub title: Option<String>,
    /// The first characters of the row's text.
    pub head: String,
    pub chars: i64,
    pub source_agent: Option<String>,
    pub has_file_hash: bool,
    pub space: Option<String>,
    pub last_modified: i64,
}

/// The filter every new-material read shares: live, reviewed memory rows that
/// no note cites. Same exclusions as the distillation staging pool, except
/// that every chunk counts and no embedding is needed: the caller's AI reads
/// the text itself.
fn unpaged_where() -> String {
    let live = super::not_self_archived("m");
    let not_superseded = super::distillation_not_superseded("m");
    let reviewed = super::distillation_row_is_reviewed("m");
    // Only a note organizes material. A document's own source page (an
    // imported file shown as itself) and an entity page do not.
    let organizing = "p.status = 'active' AND COALESCE(p.kind, 'concept') <> 'entity' \
                      AND COALESCE(p.creation_kind, 'distilled') <> 'source'";
    format!(
        "m.source = 'memory' \
           AND (m.pinned = 0 OR m.pinned IS NULL) \
           AND {live} \
           AND {not_superseded} \
           AND {reviewed} \
           AND m.source_id NOT LIKE 'merged_%' \
           AND m.source_id NOT LIKE 'recap_%' \
           AND m.is_recap = 0 \
           AND NOT EXISTS ( \
               SELECT 1 FROM page_evidence pe JOIN pages p ON p.id = pe.page_id \
               WHERE pe.locator = m.source_id AND {organizing} \
           ) \
           AND NOT EXISTS ( \
               SELECT 1 FROM edges e JOIN pages p ON p.id = e.src_id \
               WHERE e.edge_type = 'cites' AND e.dst_kind = 'memory' \
                 AND e.dst_id = m.source_id AND e.valid_until IS NULL AND {organizing} \
           )"
    )
}

impl MemoryDB {
    /// Every memory row no page cites yet, newest first. Only the first
    /// `head_chars` characters of each row's text are read.
    pub(crate) async fn list_unpaged_memory_rows(
        &self,
        head_chars: usize,
    ) -> Result<Vec<UnpagedRow>, WenlanError> {
        let sql = format!(
            "SELECT m.source_id, m.chunk_index, m.title, substr(m.content, 1, {head_chars}), \
                    length(m.content), m.source_agent, m.content_hash IS NOT NULL, m.space, \
                    m.last_modified \
             FROM memories m WHERE {} \
             ORDER BY m.last_modified DESC, m.source_id ASC, m.chunk_index ASC",
            unpaged_where()
        );
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(&sql, ())
            .await
            .map_err(|e| WenlanError::VectorDb(format!("list_unpaged_memory_rows: {e}")))?;
        let mut out = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| WenlanError::VectorDb(format!("list_unpaged_memory_rows row: {e}")))?
        {
            let space: Option<String> = row
                .get::<Option<String>>(7)
                .unwrap_or(None)
                .filter(|s| s != super::UNFILED_SPACE_ID);
            out.push(UnpagedRow {
                source_id: row
                    .get(0)
                    .map_err(|e| WenlanError::VectorDb(e.to_string()))?,
                chunk_index: row.get(1).unwrap_or(0),
                title: row.get(2).unwrap_or(None),
                head: row
                    .get::<Option<String>>(3)
                    .unwrap_or(None)
                    .unwrap_or_default(),
                chars: row.get(4).unwrap_or(0),
                source_agent: row.get(5).unwrap_or(None),
                has_file_hash: row.get::<i64>(6).unwrap_or(0) != 0,
                space,
                last_modified: row.get(8).unwrap_or(0),
            });
        }
        Ok(out)
    }

    /// The full text of the uncited rows of `source_ids`, as
    /// `(source id, chunk index, text)` in source then chunk order. A source
    /// a page started citing, or one that is gone, has no rows here.
    pub(crate) async fn unpaged_texts(
        &self,
        source_ids: &[String],
    ) -> Result<Vec<(String, i64, String)>, WenlanError> {
        if source_ids.is_empty() {
            return Ok(Vec::new());
        }
        let marks = (1..=source_ids.len())
            .map(|i| format!("?{i}"))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT m.source_id, m.chunk_index, m.content FROM memories m \
             WHERE m.source_id IN ({marks}) AND {} \
             ORDER BY m.source_id ASC, m.chunk_index ASC",
            unpaged_where()
        );
        let params: Vec<libsql::Value> = source_ids
            .iter()
            .map(|id| libsql::Value::Text(id.clone()))
            .collect();
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(&sql, params)
            .await
            .map_err(|e| WenlanError::VectorDb(format!("unpaged_texts: {e}")))?;
        let mut out = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| WenlanError::VectorDb(format!("unpaged_texts row: {e}")))?
        {
            out.push((
                row.get(0)
                    .map_err(|e| WenlanError::VectorDb(e.to_string()))?,
                row.get(1).unwrap_or(0),
                row.get::<Option<String>>(2)
                    .unwrap_or(None)
                    .unwrap_or_default(),
            ));
        }
        Ok(out)
    }
}
