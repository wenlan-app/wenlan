// SPDX-License-Identifier: Apache-2.0
//! Pages that cite a set of memories (UX-FIND-7).
//!
//! A recall that finds a memory also brings the pages built from it, so an AI
//! tool does not have to think of asking which pages use the memory. This is
//! one read; the caller still runs every page through the same Space, tier and
//! grant gates as a ranked page hit.

use super::MemoryDB;
use crate::error::WenlanError;
use crate::pages::Page;
use crate::read_scope::ReadScope;

impl MemoryDB {
    /// Active, non-entity pages in `scope` that cite any of `memory_ids`
    /// through a live `cites` edge, newest edit first, at most `limit`. The
    /// Space filter runs in SQL so pages in other Spaces cannot use up the
    /// limit. No ids or a zero limit read nothing.
    pub async fn pages_citing_memories(
        &self,
        memory_ids: &[String],
        scope: &ReadScope,
        limit: usize,
    ) -> Result<Vec<Page>, WenlanError> {
        if memory_ids.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let marks = vec!["?"; memory_ids.len()].join(", ");
        let mut params: Vec<libsql::Value> = memory_ids
            .iter()
            .map(|id| libsql::Value::Text(id.clone()))
            .collect();
        let mut conditions = Vec::new();
        super::push_read_scope_filter_folded(scope, "c.workspace", &mut conditions, &mut params);
        let scope_clause: String = conditions
            .iter()
            .map(|condition| format!(" AND {condition}"))
            .collect();
        params.push(libsql::Value::Integer(
            i64::try_from(limit).unwrap_or(i64::MAX),
        ));
        let sql = format!(
            "SELECT DISTINCT c.id, c.title, c.summary, c.content, c.entity_id, c.space,
                    c.source_memory_ids, c.version, c.status, c.created_at, c.last_compiled, c.last_modified,
                    COALESCE(c.sources_updated_count, 0), c.stale_reason, COALESCE(c.user_edited, 0),
                    COALESCE(c.changelog, '[]'), COALESCE(c.creation_kind, 'distilled'), COALESCE(c.review_status, 'confirmed'),
                    c.workspace, c.citations, COALESCE(c.kind, 'concept'), c.refresh_blocked_reason, COALESCE(c.review_required, 0)
             FROM pages c
             INNER JOIN edges cs ON cs.src_id = c.id AND cs.edge_type = 'cites'
                    AND cs.valid_until IS NULL AND cs.dst_kind = 'memory'
             WHERE cs.dst_id IN ({marks}) AND c.status = 'active'
               AND COALESCE(c.kind, 'concept') != 'entity'{scope_clause}
             ORDER BY c.last_modified DESC, c.id ASC LIMIT ?"
        );
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(&sql, params)
            .await
            .map_err(|e| WenlanError::VectorDb(format!("pages_citing_memories: {e}")))?;
        let mut pages = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| WenlanError::VectorDb(format!("pages_citing_memories row: {e}")))?
        {
            pages.push(Self::row_to_page(&row)?);
        }
        Ok(pages)
    }
}

#[cfg(test)]
mod tests {
    use crate::db::tests::test_db;
    use crate::read_scope::ReadScope;

    #[tokio::test]
    async fn ux_find_7_pages_citing_memories_returns_active_citing_pages_only() {
        let (db, _dir) = test_db().await;
        let now = chrono::Utc::now().to_rfc3339();
        let docs = ["mem_a", "mem_b", "mem_c"]
            .into_iter()
            .map(|id| crate::sources::RawDocument {
                source: "memory".to_string(),
                source_id: id.to_string(),
                title: id.to_string(),
                content: format!("content of {id}"),
                memory_type: Some("knowledge".to_string()),
                space: Some("work".to_string()),
                ..Default::default()
            })
            .collect();
        db.upsert_documents(docs).await.unwrap();
        for id in ["p_both", "p_archived", "p_other"] {
            db.insert_page(id, id, None, "content", None, Some("work"), &[], &now)
                .await
                .unwrap();
        }
        db.insert_page(
            "p_home",
            "p_home",
            None,
            "content",
            None,
            Some("home"),
            &[],
            &now,
        )
        .await
        .unwrap();
        db.link_page_source("p_home", "mem_a", "r").await.unwrap();
        db.archive_page("p_archived").await.unwrap();
        db.link_page_source("p_both", "mem_a", "r").await.unwrap();
        db.link_page_source("p_both", "mem_b", "r").await.unwrap();
        db.link_page_source("p_archived", "mem_a", "r")
            .await
            .unwrap();
        db.link_page_source("p_other", "mem_c", "r").await.unwrap();

        let ids = vec!["mem_a".to_string(), "mem_b".to_string()];
        let work = ReadScope::Space("work".to_string());
        let pages = db.pages_citing_memories(&ids, &work, 10).await.unwrap();
        let found: Vec<&str> = pages.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(
            found,
            vec!["p_both"],
            "once, active, in the asked Space, and only for these memories"
        );
        let everywhere = db
            .pages_citing_memories(&ids, &ReadScope::Global, 10)
            .await
            .unwrap();
        assert_eq!(everywhere.len(), 2, "a global read sees the home page too");

        assert!(db
            .pages_citing_memories(&[], &work, 10)
            .await
            .unwrap()
            .is_empty());
        assert!(db
            .pages_citing_memories(&ids, &work, 0)
            .await
            .unwrap()
            .is_empty());
    }
}
