// SPDX-License-Identifier: Apache-2.0
//! Open whole-page revision cards: the one a new proposal for the same page
//! replaces, and the read-only scan for pages that already have more than one.
//!
//! A page has at most one open card of a given kind. A new proposal of the
//! same kind takes the open card's id, so the replacing upsert keeps the card
//! and puts the newest content on it, instead of adding a second card. Pages
//! that already have several open cards (from before this rule) are only
//! reported, by [`MemoryDB::list_duplicate_open_revision_cards`]; nothing here
//! deletes a card.
//!
//! Two kinds of card are never replaced this way. A link card (writer
//! `link_rename`) has its own per-renamed-page lifecycle in
//! `post_write::page_link_rename`. And a card whose body no longer matches
//! the `proposal_digest` it was staged with was changed after staging (the
//! person reworded it), or predates the digest: overwriting it could lose the
//! person's words, so a new proposal gets its own card instead.

use super::MemoryDB;
use crate::error::WenlanError;

/// The `revision_kind` of a whole-page card staged by a page write.
pub(crate) const PAGE_WRITE_CARD_KIND: &str = "page_write";

/// One page with more than one open card of the same kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateOpenCards {
    pub page_id: String,
    pub revision_kind: String,
    /// The open cards' ids, newest first. The first is the one a new
    /// proposal would now update.
    pub card_ids: Vec<String>,
}

/// An open card of `page_id`: pending, a `memory` row's first chunk, and
/// pointing at the page. Shared by both reads so they agree on "open".
const OPEN_CARD_SQL: &str = "pending_revision = 1
       AND source = 'memory'
       AND chunk_index = 0
       AND supersedes IS NOT NULL
       AND EXISTS (SELECT 1 FROM pages page WHERE page.id = memories.supersedes)";

impl MemoryDB {
    /// The newest open whole-page card of `page_id` a new proposal may update
    /// in place, if any. A card bound to a research write is left alone: its
    /// id is bound into that write's adoption fence, so it is never reused.
    /// Nor is a link card, or a card whose body is no longer the proposal it
    /// was staged with (see the module doc).
    pub(crate) async fn open_page_write_card_id(
        &self,
        page_id: &str,
    ) -> Result<Option<String>, WenlanError> {
        let sql = format!(
            "SELECT source_id, json_extract(structured_fields, '$.proposal_digest')
             FROM memories
             WHERE supersedes = ?1 AND {OPEN_CARD_SQL}
               AND json_extract(structured_fields, '$.revision_kind') = ?2
               AND json_extract(structured_fields, '$.research_write') IS NULL
               AND COALESCE(json_extract(structured_fields, '$.edited_by'), '') != ?3
             ORDER BY last_modified DESC, source_id DESC
             LIMIT 1"
        );
        let candidate =
            {
                let conn = self.conn.lock().await;
                let mut rows = conn
                    .query(
                        &sql,
                        libsql::params![
                            page_id.to_string(),
                            PAGE_WRITE_CARD_KIND,
                            crate::post_write::LINK_RENAME_WRITER
                        ],
                    )
                    .await
                    .map_err(|e| WenlanError::VectorDb(format!("open_page_write_card_id: {e}")))?;
                match rows.next().await.map_err(|e| {
                    WenlanError::VectorDb(format!("open_page_write_card_id row: {e}"))
                })? {
                    Some(row) => Some((
                        row.get::<String>(0)
                            .map_err(|e| WenlanError::VectorDb(e.to_string()))?,
                        row.get::<Option<String>>(1)
                            .map_err(|e| WenlanError::VectorDb(e.to_string()))?,
                    )),
                    None => None,
                }
            };
        let Some((card_id, Some(staged_digest))) = candidate else {
            return Ok(None);
        };
        let untouched = self
            .staged_card_body(&card_id)
            .await?
            .is_some_and(|body| crate::provenance::revision_content_digest(&body) == staged_digest);
        Ok(untouched.then_some(card_id))
    }

    /// Read-only: every page with more than one open card of the same kind,
    /// for the nightly scan to report. Cards with no recorded kind count as
    /// one kind, `""`. Link cards are left out: a page may rightly hold one
    /// for each page that was renamed. Changes nothing.
    pub async fn list_duplicate_open_revision_cards(
        &self,
    ) -> Result<Vec<DuplicateOpenCards>, WenlanError> {
        let sql = format!(
            "SELECT supersedes,
                    COALESCE(json_extract(structured_fields, '$.revision_kind'), ''),
                    source_id
             FROM memories
             WHERE {OPEN_CARD_SQL}
               AND COALESCE(json_extract(structured_fields, '$.edited_by'), '') != ?1
             ORDER BY supersedes, 2, last_modified DESC, source_id DESC"
        );
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(&sql, libsql::params![crate::post_write::LINK_RENAME_WRITER])
            .await
            .map_err(|e| {
                WenlanError::VectorDb(format!("list_duplicate_open_revision_cards: {e}"))
            })?;
        let mut groups: Vec<DuplicateOpenCards> = Vec::new();
        while let Some(row) = rows.next().await.map_err(|e| {
            WenlanError::VectorDb(format!("list_duplicate_open_revision_cards row: {e}"))
        })? {
            let page_id = row
                .get::<String>(0)
                .map_err(|e| WenlanError::VectorDb(e.to_string()))?;
            let kind = row
                .get::<String>(1)
                .map_err(|e| WenlanError::VectorDb(e.to_string()))?;
            let card = row
                .get::<String>(2)
                .map_err(|e| WenlanError::VectorDb(e.to_string()))?;
            match groups.last_mut() {
                Some(group) if group.page_id == page_id && group.revision_kind == kind => {
                    group.card_ids.push(card);
                }
                _ => groups.push(DuplicateOpenCards {
                    page_id,
                    revision_kind: kind,
                    card_ids: vec![card],
                }),
            }
        }
        groups.retain(|group| group.card_ids.len() > 1);
        Ok(groups)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::NoopEmitter;
    use std::sync::Arc;

    fn card(id: &str, page: &str, kind: Option<&str>, at: i64) -> crate::sources::RawDocument {
        crate::sources::RawDocument {
            source: "memory".to_string(),
            source_id: id.to_string(),
            title: format!("Revision: {page}"),
            content: format!("proposal {id}"),
            last_modified: at,
            memory_type: Some("fact".to_string()),
            confirmed: Some(false),
            supersedes: Some(page.to_string()),
            pending_revision: true,
            structured_fields: kind.map(|kind| {
                serde_json::json!({
                    "revision_kind": kind,
                    "proposal_digest": crate::provenance::revision_content_digest(
                        &format!("proposal {id}")
                    ),
                })
                .to_string()
            }),
            ..Default::default()
        }
    }

    /// The nightly scan finds pages that already hold several open cards of
    /// one kind (newest first), and changes nothing.
    #[tokio::test]
    async fn the_duplicate_scan_lists_pages_with_several_open_cards_and_deletes_none() {
        let dir = tempfile::tempdir().unwrap();
        let db = MemoryDB::new(&dir.path().join("test.db"), Arc::new(NoopEmitter))
            .await
            .unwrap();
        for page in ["p", "q"] {
            db.insert_page_with_kind(
                page,
                page,
                None,
                "A page.",
                None,
                None,
                &[],
                &chrono::Utc::now().to_rfc3339(),
                "distilled",
                "confirmed",
                None,
                None,
            )
            .await
            .unwrap();
        }
        db.upsert_documents(vec![
            card("mem_old", "p", Some(PAGE_WRITE_CARD_KIND), 100),
            card("mem_new", "p", Some(PAGE_WRITE_CARD_KIND), 200),
            card("mem_other_kind", "p", Some("source_selection"), 150),
            card("mem_alone", "q", Some(PAGE_WRITE_CARD_KIND), 100),
        ])
        .await
        .unwrap();

        let duplicates = db.list_duplicate_open_revision_cards().await.unwrap();
        assert_eq!(
            duplicates,
            vec![DuplicateOpenCards {
                page_id: "p".into(),
                revision_kind: PAGE_WRITE_CARD_KIND.into(),
                card_ids: vec!["mem_new".into(), "mem_old".into()],
            }]
        );
        assert_eq!(
            db.open_page_write_card_id("p").await.unwrap().as_deref(),
            Some("mem_new"),
            "a new proposal updates the newest open card"
        );
        assert_eq!(
            db.list_duplicate_open_revision_cards().await.unwrap(),
            duplicates,
            "the scan is read-only"
        );
        assert_eq!(db.list_pending_revisions(50).await.unwrap().len(), 4);
    }
}
