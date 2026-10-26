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
/// no note cites. A note's citation of a file's chunk is stored as an
/// `external` edge (the source resolves as an external file) with the chunk's
/// source id as its destination, so both kinds count. Same exclusions as the distillation staging pool, except
/// that every chunk counts and no embedding is needed: the caller's AI reads
/// the text itself.
/// Only a note organizes material. A document's own source page (an imported
/// file shown as itself) and an entity page do not.
const ORGANIZING: &str = "p.status = 'active' AND COALESCE(p.kind, 'concept') <> 'entity' \
                          AND COALESCE(p.creation_kind, 'distilled') <> 'source'";

fn unpaged_where() -> String {
    let live = super::not_self_archived("m");
    let not_superseded = super::distillation_not_superseded("m");
    let reviewed = super::distillation_row_is_reviewed("m");
    let organizing = ORGANIZING;
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
               WHERE e.edge_type = 'cites' AND e.dst_kind IN ('memory', 'external') \
                 AND e.dst_id = m.source_id AND e.valid_until IS NULL AND {organizing} \
           )"
    )
}

fn unpaged_columns(head_chars: usize) -> String {
    format!(
        "m.source_id, m.chunk_index, m.title, substr(m.content, 1, {head_chars}), \
         length(m.content), m.source_agent, m.content_hash IS NOT NULL, m.space, \
         m.last_modified"
    )
}

async fn read_unpaged_rows(
    rows: &mut libsql::Rows,
    out: &mut Vec<UnpagedRow>,
) -> Result<(), WenlanError> {
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
    Ok(())
}

impl MemoryDB {
    /// The memory source ids some note cites. A note citing part of a file or
    /// a chat has taken that item in, even where it skipped a chunk.
    pub(crate) async fn list_paged_memory_sources(&self) -> Result<Vec<String>, WenlanError> {
        let sql = format!(
            "SELECT pe.locator FROM page_evidence pe JOIN pages p ON p.id = pe.page_id \
             WHERE pe.locator IS NOT NULL AND {ORGANIZING} \
             UNION \
             SELECT e.dst_id FROM edges e JOIN pages p ON p.id = e.src_id \
             WHERE e.edge_type = 'cites' AND e.dst_kind IN ('memory', 'external') \
               AND e.valid_until IS NULL AND {ORGANIZING}"
        );
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(&sql, ())
            .await
            .map_err(|e| WenlanError::VectorDb(format!("list_paged_memory_sources: {e}")))?;
        let mut out = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| WenlanError::VectorDb(format!("list_paged_memory_sources row: {e}")))?
        {
            if let Ok(id) = row.get::<String>(0) {
                out.push(id);
            }
        }
        Ok(out)
    }

    /// Every memory row no page cites yet, newest first. Only the first
    /// `head_chars` characters of each row's text are read.
    pub(crate) async fn list_unpaged_memory_rows(
        &self,
        head_chars: usize,
    ) -> Result<Vec<UnpagedRow>, WenlanError> {
        let sql = format!(
            "SELECT {} FROM memories m WHERE {} \
             ORDER BY m.last_modified DESC, m.source_id ASC, m.chunk_index ASC",
            unpaged_columns(head_chars),
            unpaged_where()
        );
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(&sql, ())
            .await
            .map_err(|e| WenlanError::VectorDb(format!("list_unpaged_memory_rows: {e}")))?;
        let mut out = Vec::new();
        read_unpaged_rows(&mut rows, &mut out).await?;
        Ok(out)
    }

    /// [`Self::list_unpaged_memory_rows`] for the items `keys` only: the rows
    /// whose source id is a key or starts with it (a note's chunks are
    /// `<key>::<n>`), read through the source id index. The caller keeps the
    /// rows that belong to a key; a few others sharing its start may come
    /// back too. Newest first.
    pub(crate) async fn unpaged_memory_rows_of(
        &self,
        head_chars: usize,
        keys: &[String],
    ) -> Result<Vec<UnpagedRow>, WenlanError> {
        // `[key, key;)`: the key, then anything after it that starts with a
        // character up to ':', so every `<key>::<n>`.
        let sql = format!(
            "SELECT {} FROM memories m \
             WHERE m.source_id >= ?1 AND m.source_id < ?2 AND {}",
            unpaged_columns(head_chars),
            unpaged_where()
        );
        let conn = self.conn.lock().await;
        let mut out = Vec::new();
        for key in keys {
            let mut rows = conn
                .query(&sql, libsql::params![key.as_str(), format!("{key};")])
                .await
                .map_err(|e| WenlanError::VectorDb(format!("unpaged_memory_rows_of: {e}")))?;
            read_unpaged_rows(&mut rows, &mut out).await?;
        }
        out.sort_by(|a, b| {
            b.last_modified
                .cmp(&a.last_modified)
                .then_with(|| a.source_id.cmp(&b.source_id))
                .then_with(|| a.chunk_index.cmp(&b.chunk_index))
        });
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

    /// The entities each of `source_ids` is linked to that have a live
    /// entity page, as `(memory source id, entity id, confirmed)`. Reads the
    /// junction table and the legacy one-to-one mirror, as the graph does.
    pub(crate) async fn source_entity_links(
        &self,
        source_ids: &[String],
    ) -> Result<Vec<(String, String, bool)>, WenlanError> {
        if source_ids.is_empty() {
            return Ok(Vec::new());
        }
        let marks = (1..=source_ids.len())
            .map(|i| format!("?{i}"))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT x.memory_id, x.entity_id, MAX(COALESCE(p.entity_confirmed, 0)) \
             FROM (SELECT memory_id, entity_id FROM memory_entities \
                   UNION \
                   SELECT source_id AS memory_id, entity_id FROM memories \
                    WHERE entity_id IS NOT NULL AND source = 'memory' AND chunk_index = 0) x \
             JOIN entity_page_map epm ON epm.entity_id = x.entity_id \
             JOIN pages p ON p.id = epm.page_id AND p.kind = 'entity' AND p.status = 'active' \
             WHERE x.memory_id IN ({marks}) \
             GROUP BY x.memory_id, x.entity_id \
             ORDER BY x.memory_id ASC, x.entity_id ASC"
        );
        let params: Vec<libsql::Value> = source_ids
            .iter()
            .map(|id| libsql::Value::Text(id.clone()))
            .collect();
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(&sql, params)
            .await
            .map_err(|e| WenlanError::VectorDb(format!("source_entity_links: {e}")))?;
        let mut out = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| WenlanError::VectorDb(format!("source_entity_links row: {e}")))?
        {
            out.push((
                row.get(0)
                    .map_err(|e| WenlanError::VectorDb(e.to_string()))?,
                row.get(1)
                    .map_err(|e| WenlanError::VectorDb(e.to_string()))?,
                row.get::<i64>(2).unwrap_or(0) != 0,
            ));
        }
        Ok(out)
    }

    /// Notes (not entity or source pages, not the Overview) whose title is
    /// `title` (case aside), newest first, as `(page id, title, space)`, as
    /// in [`Self::notes_about_entity`]. Reads every note, however old.
    pub(crate) async fn notes_titled(
        &self,
        title: &str,
    ) -> Result<Vec<(String, String, Option<String>)>, WenlanError> {
        self.note_rows(
            &format!(
                "SELECT p.id, p.title, COALESCE(p.workspace, p.space) FROM pages p \
                 WHERE LOWER(TRIM(p.title)) = LOWER(TRIM(?1)) AND {ORGANIZING} \
                   AND LOWER(p.title) <> 'overview' \
                 ORDER BY p.last_modified DESC, p.id ASC LIMIT 20"
            ),
            vec![libsql::Value::Text(title.to_string())],
        )
        .await
    }

    /// Notes (not entity or source pages, not the Overview) about
    /// `entity_id`, newest first, as `(page id, title, space)`. The space is
    /// the page's workspace, else its space; unfiled reads as `None`.
    pub(crate) async fn notes_about_entity(
        &self,
        entity_id: &str,
    ) -> Result<Vec<(String, String, Option<String>)>, WenlanError> {
        self.note_rows(
            &format!(
                "SELECT p.id, p.title, COALESCE(p.workspace, p.space) FROM pages p \
                 WHERE p.entity_id = ?1 AND {ORGANIZING} AND LOWER(p.title) <> 'overview' \
                 ORDER BY p.last_modified DESC, p.id ASC LIMIT 20"
            ),
            vec![libsql::Value::Text(entity_id.to_string())],
        )
        .await
    }

    /// The newest `limit` notes (not entity or source pages, not the
    /// Overview), as `(page id, title, space)`, as in
    /// [`Self::notes_about_entity`].
    pub(crate) async fn recent_notes(
        &self,
        limit: usize,
    ) -> Result<Vec<(String, String, Option<String>)>, WenlanError> {
        self.note_rows(
            &format!(
                "SELECT p.id, p.title, COALESCE(p.workspace, p.space) FROM pages p \
                 WHERE {ORGANIZING} AND LOWER(p.title) <> 'overview' \
                 ORDER BY p.last_modified DESC, p.id ASC LIMIT {limit}"
            ),
            Vec::new(),
        )
        .await
    }

    async fn note_rows(
        &self,
        sql: &str,
        params: Vec<libsql::Value>,
    ) -> Result<Vec<(String, String, Option<String>)>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(sql, params)
            .await
            .map_err(|e| WenlanError::VectorDb(format!("steep notes: {e}")))?;
        let mut out = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| WenlanError::VectorDb(format!("steep notes row: {e}")))?
        {
            let title = row
                .get::<Option<String>>(1)
                .unwrap_or(None)
                .unwrap_or_default();
            if title.trim().is_empty() {
                continue;
            }
            let space: Option<String> = row
                .get::<Option<String>>(2)
                .unwrap_or(None)
                .filter(|s| s != super::UNFILED_SPACE_ID);
            out.push((
                row.get(0)
                    .map_err(|e| WenlanError::VectorDb(e.to_string()))?,
                title,
                space,
            ));
        }
        Ok(out)
    }

    /// Notes (not entity or source pages, not the Overview) citing a memory
    /// whose source id, lowercased, holds one of `probes`, newest first, as
    /// `(page id, title, space, cited source id)`: one row per citation.
    pub(crate) async fn notes_citing_sources_with(
        &self,
        probes: &[String],
    ) -> Result<Vec<(String, String, Option<String>, String)>, WenlanError> {
        self.notes_citing_where(probes, "c.source_id", "").await
    }

    /// Notes (not entity or source pages, not the Overview) citing a memory
    /// whose text, lowercased (ASCII), holds one of `probes`, newest first,
    /// as `(page id, title, space, that memory's text)`: one row per cited
    /// memory row.
    pub(crate) async fn notes_citing_text_with(
        &self,
        probes: &[String],
    ) -> Result<Vec<(String, String, Option<String>, String)>, WenlanError> {
        self.notes_citing_where(
            probes,
            "m.content",
            "CROSS JOIN memories m ON m.source = 'memory' AND m.source_id = c.source_id",
        )
        .await
    }

    /// The rows of [`Self::notes_citing_sources_with`] and
    /// [`Self::notes_citing_text_with`]: `column` holds one of `probes`.
    async fn notes_citing_where(
        &self,
        probes: &[String],
        column: &str,
        join: &str,
    ) -> Result<Vec<(String, String, Option<String>, String)>, WenlanError> {
        if probes.is_empty() {
            return Ok(Vec::new());
        }
        // The citations lead (CROSS JOIN keeps that order): read through
        // every memory instead, the text test would cost seconds.
        let holds = (1..=probes.len())
            .map(|i| format!("instr(lower({column}), ?{i}) > 0"))
            .collect::<Vec<_>>()
            .join(" OR ");
        let sql = format!(
            "SELECT p.id, p.title, COALESCE(p.workspace, p.space), {column} FROM ( \
               SELECT pe.page_id AS page_id, pe.locator AS source_id FROM page_evidence pe \
               WHERE pe.locator IS NOT NULL \
               UNION \
               SELECT e.src_id, e.dst_id FROM edges e \
               WHERE e.edge_type = 'cites' AND e.dst_kind IN ('memory', 'external') \
                 AND e.valid_until IS NULL \
             ) c CROSS JOIN pages p ON p.id = c.page_id {join} \
             WHERE {ORGANIZING} AND LOWER(p.title) <> 'overview' AND ({holds}) \
             ORDER BY p.last_modified DESC, p.id ASC LIMIT 500"
        );
        let params: Vec<libsql::Value> = probes
            .iter()
            .map(|probe| libsql::Value::Text(probe.clone()))
            .collect();
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(&sql, params)
            .await
            .map_err(|e| WenlanError::VectorDb(format!("notes_citing_where: {e}")))?;
        let mut out = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| WenlanError::VectorDb(format!("notes_citing_where row: {e}")))?
        {
            let title = row
                .get::<Option<String>>(1)
                .unwrap_or(None)
                .unwrap_or_default();
            if title.trim().is_empty() {
                continue;
            }
            let space: Option<String> = row
                .get::<Option<String>>(2)
                .unwrap_or(None)
                .filter(|s| s != super::UNFILED_SPACE_ID);
            out.push((
                row.get(0)
                    .map_err(|e| WenlanError::VectorDb(e.to_string()))?,
                title,
                space,
                row.get::<Option<String>>(3)
                    .unwrap_or(None)
                    .unwrap_or_default(),
            ));
        }
        Ok(out)
    }

    /// The titles of the live entity pages of `entity_ids`, as
    /// `(entity id, title)`.
    pub(crate) async fn entity_page_titles(
        &self,
        entity_ids: &[String],
    ) -> Result<Vec<(String, String)>, WenlanError> {
        if entity_ids.is_empty() {
            return Ok(Vec::new());
        }
        let marks = (1..=entity_ids.len())
            .map(|i| format!("?{i}"))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT epm.entity_id, p.title FROM entity_page_map epm \
             JOIN pages p ON p.id = epm.page_id AND p.kind = 'entity' AND p.status = 'active' \
             WHERE epm.entity_id IN ({marks}) ORDER BY epm.entity_id ASC"
        );
        let params: Vec<libsql::Value> = entity_ids
            .iter()
            .map(|id| libsql::Value::Text(id.clone()))
            .collect();
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(&sql, params)
            .await
            .map_err(|e| WenlanError::VectorDb(format!("entity_page_titles: {e}")))?;
        let mut out = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| WenlanError::VectorDb(format!("entity_page_titles row: {e}")))?
        {
            let title = row
                .get::<Option<String>>(1)
                .unwrap_or(None)
                .unwrap_or_default();
            if !title.trim().is_empty() {
                out.push((
                    row.get(0)
                        .map_err(|e| WenlanError::VectorDb(e.to_string()))?,
                    title,
                ));
            }
        }
        Ok(out)
    }

    /// Notes that share entities with `entity_ids`, most shared first, as
    /// `(page id, title, space, shared)`: a note about one of them counts one,
    /// and each of them a memory the note cites mentions counts one more.
    pub(crate) async fn notes_sharing_entities(
        &self,
        entity_ids: &[String],
        limit: usize,
    ) -> Result<Vec<(String, String, Option<String>, i64)>, WenlanError> {
        if entity_ids.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let marks = (1..=entity_ids.len())
            .map(|i| format!("?{i}"))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT id, title, sp, SUM(w) AS shared, MAX(lm) AS newest FROM ( \
               SELECT p.id, p.title, COALESCE(p.workspace, p.space) AS sp, \
                      COUNT(DISTINCT me.entity_id) AS w, p.last_modified AS lm \
               FROM pages p \
               JOIN edges e ON e.src_id = p.id AND e.edge_type = 'cites' \
                AND e.dst_kind = 'memory' AND e.valid_until IS NULL \
               JOIN memory_entities me ON me.memory_id = e.dst_id \
               WHERE {ORGANIZING} AND LOWER(p.title) <> 'overview' AND me.entity_id IN ({marks}) \
               GROUP BY p.id \
               UNION ALL \
               SELECT p.id, p.title, COALESCE(p.workspace, p.space), 1, p.last_modified \
               FROM pages p \
               WHERE {ORGANIZING} AND LOWER(p.title) <> 'overview' AND p.entity_id IN ({marks}) \
             ) GROUP BY id ORDER BY shared DESC, newest DESC, id ASC LIMIT {limit}"
        );
        let params: Vec<libsql::Value> = entity_ids
            .iter()
            .map(|id| libsql::Value::Text(id.clone()))
            .collect();
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(&sql, params)
            .await
            .map_err(|e| WenlanError::VectorDb(format!("notes_sharing_entities: {e}")))?;
        let mut out = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| WenlanError::VectorDb(format!("notes_sharing_entities row: {e}")))?
        {
            let title = row
                .get::<Option<String>>(1)
                .unwrap_or(None)
                .unwrap_or_default();
            if title.trim().is_empty() {
                continue;
            }
            let space: Option<String> = row
                .get::<Option<String>>(2)
                .unwrap_or(None)
                .filter(|s| s != super::UNFILED_SPACE_ID);
            out.push((
                row.get(0)
                    .map_err(|e| WenlanError::VectorDb(e.to_string()))?,
                title,
                space,
                row.get::<i64>(3).unwrap_or(0),
            ));
        }
        Ok(out)
    }

    /// Every entity whose Wiki page is confirmed. An organize run compares
    /// it before and after, so it reports only the entities it confirmed.
    pub(crate) async fn confirmed_entity_ids(
        &self,
    ) -> Result<std::collections::HashSet<String>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT DISTINCT epm.entity_id FROM entity_page_map epm \
                 JOIN pages p ON p.id = epm.page_id AND p.kind = 'entity' \
                  AND p.status = 'active' \
                 WHERE COALESCE(p.entity_confirmed, 0) <> 0",
                (),
            )
            .await
            .map_err(|e| WenlanError::VectorDb(format!("confirmed_entity_ids: {e}")))?;
        let mut out = std::collections::HashSet::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| WenlanError::VectorDb(format!("confirmed_entity_ids row: {e}")))?
        {
            out.insert(
                row.get::<String>(0)
                    .map_err(|e| WenlanError::VectorDb(e.to_string()))?,
            );
        }
        Ok(out)
    }

    /// Whether a note (not an entity or source page) about `entity_id`
    /// already exists, in any Space.
    pub(crate) async fn entity_has_note(&self, entity_id: &str) -> Result<bool, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                &format!("SELECT 1 FROM pages p WHERE p.entity_id = ?1 AND {ORGANIZING} LIMIT 1"),
                libsql::params![entity_id],
            )
            .await
            .map_err(|e| WenlanError::VectorDb(format!("entity_has_note: {e}")))?;
        Ok(rows
            .next()
            .await
            .map_err(|e| WenlanError::VectorDb(format!("entity_has_note row: {e}")))?
            .is_some())
    }
}
