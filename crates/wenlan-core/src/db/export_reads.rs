// SPDX-License-Identifier: Apache-2.0
//! Reads for the lossless export (`store::export`): every row as stored, with
//! no visibility filter and no parsing that could drop a value.

use super::MemoryDB;
use crate::error::WenlanError;

/// One `pages` row, every column the page and entity files carry, as stored.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ExportPageRow {
    pub id: String,
    pub title: String,
    pub content: String,
    pub kind: String,
    pub creation_kind: String,
    pub status: String,
    pub space: Option<String>,
    pub created_at: String,
    pub last_modified: String,
    /// The `citations` JSON as stored.
    pub citations: Option<String>,
    /// The entity this row is the shadow page of, from the row or
    /// `entity_page_map`.
    pub entity_id: Option<String>,
    pub entity_type: Option<String>,
    pub entity_confirmed: bool,
    /// The `aliases` JSON as stored.
    pub aliases: Option<String>,
    pub established_by: Option<String>,
}

impl MemoryDB {
    /// Every page row, ordered by id, archived and entity rows included.
    pub(crate) async fn export_page_rows(&self) -> Result<Vec<ExportPageRow>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT p.id, p.title, p.content, COALESCE(p.kind, ''), \
                        COALESCE(p.creation_kind, ''), p.status, p.space, \
                        p.created_at, p.last_modified, p.citations, \
                        COALESCE(p.entity_id, \
                                 (SELECT m.entity_id FROM entity_page_map m \
                                  WHERE m.page_id = p.id ORDER BY m.entity_id LIMIT 1)), \
                        p.entity_type, COALESCE(p.entity_confirmed, 0), p.aliases, \
                        p.established_by \
                 FROM pages p ORDER BY p.id",
                (),
            )
            .await
            .map_err(|e| WenlanError::VectorDb(format!("export_page_rows: {e}")))?;
        let mut out = Vec::new();
        let decode = |e: libsql::Error| WenlanError::VectorDb(format!("export_page_rows row: {e}"));
        while let Some(row) = rows.next().await.map_err(decode)? {
            out.push(ExportPageRow {
                id: row.get(0).map_err(decode)?,
                title: row.get(1).map_err(decode)?,
                content: row.get(2).map_err(decode)?,
                kind: row.get(3).map_err(decode)?,
                creation_kind: row.get(4).map_err(decode)?,
                status: row.get(5).map_err(decode)?,
                space: row.get(6).map_err(decode)?,
                created_at: row.get(7).map_err(decode)?,
                last_modified: row.get(8).map_err(decode)?,
                citations: row.get(9).map_err(decode)?,
                entity_id: row.get(10).map_err(decode)?,
                entity_type: row.get(11).map_err(decode)?,
                entity_confirmed: row.get::<i64>(12).map_err(decode)? != 0,
                aliases: row.get(13).map_err(decode)?,
                established_by: row.get(14).map_err(decode)?,
            });
        }
        Ok(out)
    }

    /// The start of each memory in `ids` that exists, for citation labels.
    pub(crate) async fn export_memory_texts(
        &self,
        ids: &[String],
    ) -> Result<std::collections::HashMap<String, String>, WenlanError> {
        let mut out = std::collections::HashMap::new();
        let conn = self.conn.lock().await;
        for chunk in ids.chunks(200) {
            let marks = vec!["?"; chunk.len()].join(", ");
            // A memory's id is its `source_id`; its first chunk starts its text.
            let sql = format!(
                "SELECT source_id, content FROM memories WHERE source_id IN ({marks}) \
                 ORDER BY source_id, chunk_index DESC"
            );
            let params: Vec<libsql::Value> = chunk
                .iter()
                .map(|id| libsql::Value::Text(id.clone()))
                .collect();
            let mut rows = conn
                .query(&sql, params)
                .await
                .map_err(|e| WenlanError::VectorDb(format!("export_memory_texts: {e}")))?;
            while let Some(row) = rows
                .next()
                .await
                .map_err(|e| WenlanError::VectorDb(format!("export_memory_texts row: {e}")))?
            {
                let id: String = row
                    .get(0)
                    .map_err(|e| WenlanError::VectorDb(format!("export_memory_texts id: {e}")))?;
                let text: String = row.get(1).unwrap_or_default();
                out.insert(id, text);
            }
        }
        Ok(out)
    }

    /// Runs one statement with `?1` bound to `id`, for export fixtures.
    #[cfg(test)]
    pub(crate) async fn export_fixture_sql(&self, statement: &str, id: &str) {
        let conn = self.conn.lock().await;
        conn.execute(statement, libsql::params![id]).await.unwrap();
    }
}
