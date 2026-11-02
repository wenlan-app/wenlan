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
    /// The page's own `entity_id`, else the entity `entity_page_map` maps
    /// to it.
    pub entity_id: Option<String>,
    /// Whether `pages.entity_id` itself is set (not only the map).
    pub entity_id_on_page: bool,
    pub entity_type: Option<String>,
    /// As stored: NULL, 0 and 1 are kept apart.
    pub entity_confirmed: Option<i64>,
    /// The `aliases` JSON as stored.
    pub aliases: Option<String>,
    pub established_by: Option<String>,
    /// The page's one-line summary; the person can edit it.
    pub summary: Option<String>,
    pub entity_created_at: Option<i64>,
    pub entity_updated_at: Option<i64>,
}

/// One `memories` row (one chunk), every column a memory file carries.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ExportMemoryRow {
    pub source: String,
    pub source_id: String,
    pub chunk_index: i64,
    pub content: String,
    pub title: Option<String>,
    pub url: Option<String>,
    pub memory_type: Option<String>,
    pub space: Option<String>,
    pub source_agent: Option<String>,
    pub confirmed: bool,
    pub pinned: bool,
    pub supersedes: Option<String>,
    pub supersede_mode: Option<String>,
    pub pending_revision: bool,
    pub structured_fields: Option<String>,
    pub source_text: Option<String>,
    pub created_at: Option<i64>,
    pub event_date: Option<i64>,
    pub event_end: Option<i64>,
    pub importance: Option<i64>,
    pub episode_of: Option<String>,
    pub origin_class: Option<String>,
    pub session_id: Option<String>,
    pub authored_content: Option<String>,
    pub entity_id: Option<String>,
    pub is_recap: bool,
    /// From `enrichment_origin`; a memory without a row there counts as
    /// explicit, as enrichment treats it.
    pub memory_type_explicit: bool,
    pub structured_fields_explicit: bool,
    pub last_modified: Option<i64>,
    /// Extra text the memory is found by; part of what is embedded.
    pub retrieval_cue: Option<String>,
    /// The person turned down the space enrichment suggested.
    pub space_rejected: bool,
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
                        p.entity_type, p.entity_confirmed, p.aliases, \
                        p.established_by, p.summary, p.entity_created_at, \
                        p.entity_updated_at, p.entity_id IS NOT NULL \
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
                entity_confirmed: row.get::<Option<i64>>(12).map_err(decode)?,
                aliases: row.get(13).map_err(decode)?,
                established_by: row.get(14).map_err(decode)?,
                summary: row.get(15).map_err(decode)?,
                entity_created_at: row.get(16).map_err(decode)?,
                entity_updated_at: row.get(17).map_err(decode)?,
                entity_id_on_page: row.get::<i64>(18).map_err(decode)? != 0,
            });
        }
        Ok(out)
    }

    /// The text of each memory in `ids` that exists, for citation labels.
    pub(crate) async fn export_memory_texts(
        &self,
        ids: &[String],
    ) -> Result<std::collections::HashMap<String, String>, WenlanError> {
        let mut out = std::collections::HashMap::new();
        let conn = self.conn.lock().await;
        for chunk in ids.chunks(200) {
            let marks = vec!["?"; chunk.len()].join(", ");
            // A memory's id is its `source_id`; its text is its chunks in
            // order (chunk boundaries are cache, so a label never depends on
            // where one falls). Only `memory` rows, as the app's label
            // reader reads them.
            // Document chunks are left out: their connected folder is their
            // truth, so a rebuilt database has none to label them by.
            let sql = format!(
                "SELECT source_id, content, origin_class, source_agent FROM memories \
                 WHERE source = 'memory' AND source_id IN ({marks}) \
                 ORDER BY source_id, chunk_index"
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
                let origin: Option<String> = row.get(2).unwrap_or_default();
                let agent: Option<String> = row.get(3).unwrap_or_default();
                if origin.as_deref() == Some("document_ingest")
                    || agent
                        .as_deref()
                        .is_some_and(crate::origin::is_document_ingest_source_agent)
                {
                    continue;
                }
                out.entry(id)
                    .and_modify(|joined: &mut String| {
                        joined.push('\n');
                        joined.push_str(&text);
                    })
                    .or_insert(text);
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

    /// Every `memories` row, ordered by memory then chunk.
    pub(crate) async fn export_memory_rows(&self) -> Result<Vec<ExportMemoryRow>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT m.source, m.source_id, m.chunk_index, m.content, m.title, m.url, \
                        m.memory_type, m.space, m.source_agent, COALESCE(m.confirmed, 0), \
                        COALESCE(m.pinned, 0), m.supersedes, m.supersede_mode, \
                        COALESCE(m.pending_revision, 0), m.structured_fields, m.source_text, \
                        m.created_at, m.event_date, m.event_end, m.importance, m.episode_of, \
                        m.origin_class, m.session_id, m.authored_content, m.entity_id, \
                        COALESCE(m.is_recap, 0), \
                        COALESCE(o.memory_type_explicit, 1), \
                        COALESCE(o.structured_fields_explicit, 1), m.last_modified, \
                        m.retrieval_cue, COALESCE(o.space_rejected, 0) \
                 FROM memories m LEFT JOIN enrichment_origin o ON o.source_id = m.source_id \
                 ORDER BY m.source_id, m.source, m.chunk_index, m.id",
                (),
            )
            .await
            .map_err(|e| WenlanError::VectorDb(format!("export_memory_rows: {e}")))?;
        let decode =
            |e: libsql::Error| WenlanError::VectorDb(format!("export_memory_rows row: {e}"));
        let flag = |value: i64| value != 0;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.map_err(decode)? {
            out.push(ExportMemoryRow {
                source: row.get(0).map_err(decode)?,
                source_id: row.get(1).map_err(decode)?,
                chunk_index: row.get(2).map_err(decode)?,
                content: row.get(3).map_err(decode)?,
                title: row.get(4).map_err(decode)?,
                url: row.get(5).map_err(decode)?,
                memory_type: row.get(6).map_err(decode)?,
                space: row.get(7).map_err(decode)?,
                source_agent: row.get(8).map_err(decode)?,
                confirmed: flag(row.get(9).map_err(decode)?),
                pinned: flag(row.get(10).map_err(decode)?),
                supersedes: row.get(11).map_err(decode)?,
                supersede_mode: row.get(12).map_err(decode)?,
                pending_revision: flag(row.get(13).map_err(decode)?),
                structured_fields: row.get(14).map_err(decode)?,
                source_text: row.get(15).map_err(decode)?,
                created_at: row.get(16).map_err(decode)?,
                event_date: row.get(17).map_err(decode)?,
                event_end: row.get(18).map_err(decode)?,
                importance: row.get(19).map_err(decode)?,
                episode_of: row.get(20).map_err(decode)?,
                origin_class: row.get(21).map_err(decode)?,
                session_id: row.get(22).map_err(decode)?,
                authored_content: row.get(23).map_err(decode)?,
                entity_id: row.get(24).map_err(decode)?,
                is_recap: flag(row.get(25).map_err(decode)?),
                memory_type_explicit: flag(row.get(26).map_err(decode)?),
                structured_fields_explicit: flag(row.get(27).map_err(decode)?),
                last_modified: row.get(28).map_err(decode)?,
                retrieval_cue: row.get(29).map_err(decode)?,
                space_rejected: flag(row.get(30).map_err(decode)?),
            });
        }
        Ok(out)
    }

    /// `(memory id, entity id)` links, ordered.
    pub(crate) async fn export_memory_entities(
        &self,
    ) -> Result<Vec<(String, String)>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT memory_id, entity_id FROM memory_entities ORDER BY memory_id, entity_id",
                (),
            )
            .await
            .map_err(|e| WenlanError::VectorDb(format!("export_memory_entities: {e}")))?;
        let mut out = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| WenlanError::VectorDb(format!("export_memory_entities row: {e}")))?
        {
            let decode =
                |e: libsql::Error| WenlanError::VectorDb(format!("export_memory_entities: {e}"));
            out.push((row.get(0).map_err(decode)?, row.get(1).map_err(decode)?));
        }
        Ok(out)
    }

    /// Every Brief with its space's name, and every item, in list order.
    pub(crate) async fn export_briefs(
        &self,
    ) -> Result<(Vec<ExportBriefRow>, Vec<ExportBriefItemRow>), WenlanError> {
        let conn = self.conn.lock().await;
        let decode = |e: libsql::Error| WenlanError::VectorDb(format!("export_briefs: {e}"));
        let mut briefs = Vec::new();
        let mut rows = conn
            .query(
                "SELECT b.space_id, s.name, b.last_session_summary, b.last_handoff_at, \
                        b.version, b.updated_at \
                 FROM briefs b LEFT JOIN spaces s ON s.id = b.space_id ORDER BY b.space_id",
                (),
            )
            .await
            .map_err(decode)?;
        while let Some(row) = rows.next().await.map_err(decode)? {
            briefs.push(ExportBriefRow {
                space_id: row.get(0).map_err(decode)?,
                space_name: row.get(1).map_err(decode)?,
                summary: row.get(2).map_err(decode)?,
                last_handoff_at: row.get(3).map_err(decode)?,
                version: row.get(4).map_err(decode)?,
                updated_at: row.get(5).map_err(decode)?,
            });
        }
        let mut items = Vec::new();
        let mut rows = conn
            .query(
                "SELECT id, space_id, text, state, added_at, gate, version \
                 FROM brief_items ORDER BY space_id, state, added_at, id",
                (),
            )
            .await
            .map_err(decode)?;
        while let Some(row) = rows.next().await.map_err(decode)? {
            items.push(ExportBriefItemRow {
                id: row.get(0).map_err(decode)?,
                space_id: row.get(1).map_err(decode)?,
                text: row.get(2).map_err(decode)?,
                state: row.get(3).map_err(decode)?,
                added_at: row.get(4).map_err(decode)?,
                gate: row.get(5).map_err(decode)?,
                version: row.get(6).map_err(decode)?,
            });
        }
        Ok((briefs, items))
    }

    /// Every canvas, node and edge, ordered.
    pub(crate) async fn export_canvases(
        &self,
    ) -> Result<
        (
            Vec<ExportCanvasRow>,
            Vec<ExportCanvasNodeRow>,
            Vec<ExportCanvasEdgeRow>,
        ),
        WenlanError,
    > {
        let conn = self.conn.lock().await;
        let decode = |e: libsql::Error| WenlanError::VectorDb(format!("export_canvases: {e}"));
        let mut maps = Vec::new();
        let mut rows = conn
            .query(
                "SELECT page_id, revision, map_schema, viewport, generated_at, created_at, \
                        updated_at FROM page_maps ORDER BY page_id",
                (),
            )
            .await
            .map_err(decode)?;
        while let Some(row) = rows.next().await.map_err(decode)? {
            maps.push(ExportCanvasRow {
                page_id: row.get(0).map_err(decode)?,
                revision: row.get(1).map_err(decode)?,
                map_schema: row.get(2).map_err(decode)?,
                viewport: row.get(3).map_err(decode)?,
                generated_at: row.get(4).map_err(decode)?,
                created_at: row.get(5).map_err(decode)?,
                updated_at: row.get(6).map_err(decode)?,
            });
        }
        let mut nodes = Vec::new();
        let mut rows = conn
            .query(
                "SELECT id, page_id, parent_id, rank, ref_kind, ref_id, label, status, pinned, \
                        placed, collapsed, x, y, width, height, fingerprint, provenance, \
                        created_at, updated_at \
                 FROM page_map_nodes ORDER BY page_id, rank, id",
                (),
            )
            .await
            .map_err(decode)?;
        while let Some(row) = rows.next().await.map_err(decode)? {
            nodes.push(ExportCanvasNodeRow {
                id: row.get(0).map_err(decode)?,
                page_id: row.get(1).map_err(decode)?,
                parent_id: row.get(2).map_err(decode)?,
                rank: row.get(3).map_err(decode)?,
                ref_kind: row.get(4).map_err(decode)?,
                ref_id: row.get(5).map_err(decode)?,
                label: row.get(6).map_err(decode)?,
                status: row.get(7).map_err(decode)?,
                pinned: row.get::<i64>(8).map_err(decode)? != 0,
                placed: row.get::<i64>(9).map_err(decode)? != 0,
                collapsed: row.get::<i64>(10).map_err(decode)? != 0,
                x: row.get(11).map_err(decode)?,
                y: row.get(12).map_err(decode)?,
                width: row.get(13).map_err(decode)?,
                height: row.get(14).map_err(decode)?,
                fingerprint: row.get(15).map_err(decode)?,
                provenance: row.get(16).map_err(decode)?,
                created_at: row.get(17).map_err(decode)?,
                updated_at: row.get(18).map_err(decode)?,
            });
        }
        let mut edges = Vec::new();
        let mut rows = conn
            .query(
                "SELECT id, page_id, from_node, to_node, kind, label, status, provenance, \
                        created_at FROM page_map_edges ORDER BY page_id, id",
                (),
            )
            .await
            .map_err(decode)?;
        while let Some(row) = rows.next().await.map_err(decode)? {
            edges.push(ExportCanvasEdgeRow {
                id: row.get(0).map_err(decode)?,
                page_id: row.get(1).map_err(decode)?,
                from_node: row.get(2).map_err(decode)?,
                to_node: row.get(3).map_err(decode)?,
                kind: row.get(4).map_err(decode)?,
                label: row.get(5).map_err(decode)?,
                status: row.get(6).map_err(decode)?,
                provenance: row.get(7).map_err(decode)?,
                created_at: row.get(8).map_err(decode)?,
            });
        }
        Ok((maps, nodes, edges))
    }

    /// Each page's `creation_kind` and `user_edited`, which say who made a
    /// first version recorded as plain `create`.
    pub(crate) async fn export_page_births(
        &self,
    ) -> Result<std::collections::HashMap<String, (String, bool)>, WenlanError> {
        let conn = self.conn.lock().await;
        let decode = |e: libsql::Error| WenlanError::VectorDb(format!("export_page_births: {e}"));
        let mut rows = conn
            .query(
                "SELECT id, COALESCE(creation_kind, ''), COALESCE(user_edited, 0) FROM pages",
                (),
            )
            .await
            .map_err(decode)?;
        let mut out = std::collections::HashMap::new();
        while let Some(row) = rows.next().await.map_err(decode)? {
            let id: String = row.get(0).map_err(decode)?;
            let kind: String = row.get(1).map_err(decode)?;
            let edited: i64 = row.get(2).map_err(decode)?;
            out.insert(id, (kind, edited != 0));
        }
        Ok(out)
    }

    /// Every kept artifact original, by page id.
    pub(crate) async fn export_original_records(
        &self,
    ) -> Result<Vec<super::originals::OriginalRecord>, WenlanError> {
        let conn = self.conn.lock().await;
        let decode = |e: libsql::Error| WenlanError::VectorDb(format!("export_originals: {e}"));
        let mut rows = conn
            .query(
                "SELECT page_id, vendor, conversation_id, artifact_id, kind, title, file_path, \
                        conversation_url, written_at, imported_at \
                 FROM originals ORDER BY page_id",
                (),
            )
            .await
            .map_err(decode)?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.map_err(decode)? {
            out.push(super::originals::OriginalRecord {
                page_id: row.get(0).map_err(decode)?,
                vendor: row.get(1).map_err(decode)?,
                conversation_id: row.get(2).map_err(decode)?,
                artifact_id: row.get(3).map_err(decode)?,
                kind: row.get(4).map_err(decode)?,
                title: row.get(5).map_err(decode)?,
                file_path: row.get(6).map_err(decode)?,
                conversation_url: row.get(7).map_err(decode)?,
                written_at: row.get(8).map_err(decode)?,
                imported_at: row.get(9).map_err(decode)?,
            });
        }
        Ok(out)
    }

    /// Every row of `table` as stored, in primary key order, or `None` when
    /// the table does not exist. `table` is one of the export's fixed names,
    /// never input.
    pub(crate) async fn export_table(
        &self,
        table: &'static str,
    ) -> Result<Option<ExportTable>, WenlanError> {
        let conn = self.conn.lock().await;
        let decode = |e: libsql::Error| WenlanError::VectorDb(format!("export {table}: {e}"));
        let mut info = conn
            .query(
                // Key columns first, in key order, then the rest by name, not
                // by position: a database upgraded with ALTER TABLE and a
                // fresh one list the same columns in different orders.
                "SELECT name, pk FROM pragma_table_info(?1) \
                 ORDER BY pk = 0, NULLIF(pk, 0), name",
                [table],
            )
            .await
            .map_err(decode)?;
        let mut columns = Vec::new();
        let mut keys: Vec<(i64, String)> = Vec::new();
        while let Some(row) = info.next().await.map_err(decode)? {
            let name: String = row.get(0).map_err(decode)?;
            let pk: i64 = row.get(1).map_err(decode)?;
            if pk > 0 {
                keys.push((pk, name.clone()));
            }
            columns.push(name);
        }
        if columns.is_empty() {
            return Ok(None);
        }
        keys.sort();
        let quote = |name: &str| format!("\"{}\"", name.replace('"', "\"\""));
        let order = if keys.is_empty() {
            "rowid".to_string()
        } else {
            keys.iter()
                .map(|(_, name)| quote(name))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let select = columns
            .iter()
            .map(|name| quote(name))
            .collect::<Vec<_>>()
            .join(", ");
        let mut rows = conn
            .query(
                &format!("SELECT {select} FROM {} ORDER BY {order}", quote(table)),
                (),
            )
            .await
            .map_err(decode)?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.map_err(decode)? {
            let mut values = Vec::with_capacity(columns.len());
            for index in 0..columns.len() {
                values.push(row.get_value(index as i32).map_err(decode)?);
            }
            out.push(values);
        }
        Ok(Some(ExportTable { columns, rows: out }))
    }
}

impl MemoryDB {
    /// The named columns of `table` that exist, every row, ordered by
    /// `order` (column names). `None` when the table does not exist.
    pub(crate) async fn export_columns(
        &self,
        table: &'static str,
        wanted: &[&str],
        order: &[&str],
    ) -> Result<Option<ExportTable>, WenlanError> {
        let conn = self.conn.lock().await;
        let decode = |e: libsql::Error| WenlanError::VectorDb(format!("export {table}: {e}"));
        let mut info = conn
            .query(
                "SELECT name FROM pragma_table_info(?1) ORDER BY cid",
                [table],
            )
            .await
            .map_err(decode)?;
        let mut present = Vec::new();
        while let Some(row) = info.next().await.map_err(decode)? {
            present.push(row.get::<String>(0).map_err(decode)?);
        }
        if present.is_empty() {
            return Ok(None);
        }
        let columns: Vec<String> = wanted
            .iter()
            .filter(|name| present.iter().any(|p| p == *name))
            .map(|name| name.to_string())
            .collect();
        let quote = |name: &str| format!("\"{}\"", name.replace('"', "\"\""));
        let select = columns
            .iter()
            .map(|c| quote(c))
            .collect::<Vec<_>>()
            .join(", ");
        let order = order
            .iter()
            .map(|c| quote(c))
            .collect::<Vec<_>>()
            .join(", ");
        let mut rows = conn
            .query(
                &format!("SELECT {select} FROM {} ORDER BY {order}", quote(table)),
                (),
            )
            .await
            .map_err(decode)?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.map_err(decode)? {
            let mut values = Vec::with_capacity(columns.len());
            for index in 0..columns.len() {
                values.push(row.get_value(index as i32).map_err(decode)?);
            }
            out.push(values);
        }
        Ok(Some(ExportTable { columns, rows: out }))
    }
}

/// A table copied whole: its column names and every row's values.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ExportTable {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<libsql::Value>>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ExportBriefRow {
    pub space_id: String,
    pub space_name: Option<String>,
    pub summary: String,
    pub last_handoff_at: Option<i64>,
    pub version: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ExportBriefItemRow {
    pub id: String,
    pub space_id: String,
    pub text: String,
    pub state: String,
    pub added_at: i64,
    pub gate: Option<String>,
    pub version: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ExportCanvasRow {
    pub page_id: String,
    pub revision: i64,
    pub map_schema: i64,
    pub viewport: Option<String>,
    pub generated_at: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ExportCanvasNodeRow {
    pub id: String,
    pub page_id: String,
    pub parent_id: Option<String>,
    pub rank: f64,
    pub ref_kind: String,
    pub ref_id: String,
    pub label: Option<String>,
    pub status: String,
    pub pinned: bool,
    pub placed: bool,
    pub collapsed: bool,
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub width: Option<f64>,
    pub height: Option<f64>,
    pub fingerprint: String,
    pub provenance: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ExportCanvasEdgeRow {
    pub id: String,
    pub page_id: String,
    pub from_node: String,
    pub to_node: String,
    pub kind: String,
    pub label: Option<String>,
    pub status: String,
    pub provenance: Option<String>,
    pub created_at: Option<String>,
}
