// SPDX-License-Identifier: Apache-2.0
//! Open whole-page revision cards: the one a new proposal for the same page
//! replaces, and the read-only scan for pages that already have more than one.
//!
//! A page has at most one open card of a given kind. A new proposal of the
//! same kind is staged as a new card under a new id, and the open card it
//! replaces is then retired ([`MemoryDB::retire_replaced_page_card`]): it
//! leaves the queue but is not deleted, and records the card that replaced
//! it. A card's id therefore always names the one body it was staged with, so
//! an Accept the person sent for the card they saw can never land a newer
//! body they did not see; an Accept of a retired id is refused
//! ([`MemoryDB::replaced_page_card`]). Pages that already have several open
//! cards (from before this rule, or from two proposals racing) are only
//! reported, by [`MemoryDB::list_duplicate_open_revision_cards`]; nothing here
//! deletes a card.
//!
//! Two kinds of card are never replaced this way. A link card (writer
//! `link_rename`) has its own per-renamed-page lifecycle in
//! `post_write::page_link_rename`. And a card whose body no longer matches
//! the `proposal_digest` it was staged with was changed after staging (the
//! person reworded it), or predates the digest: retiring it could hide the
//! person's words, so a new proposal is staged beside it instead.

use super::MemoryDB;
use crate::error::WenlanError;

/// The `revision_kind` of a whole-page card staged by a page write.
pub(crate) const PAGE_WRITE_CARD_KIND: &str = "page_write";

/// The changelog `retirement` written on a card a newer proposal replaced.
/// `restore_forgotten_memory` restores only its own `forget` retirement, so a
/// replaced card is never brought back into the queue by a memory restore.
pub(crate) const REPLACED_RETIREMENT: &str = "replaced";

/// A retired card, and the card that replaced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplacedPageCard {
    pub page_id: String,
    pub replaced_by: String,
}

/// The body a card staged with `content` at `staged_at` is stored as: the
/// document upsert redacts PII and grounds relative dates against the row's
/// `last_modified` before it writes, so a card's `proposal_digest` must be
/// taken over this form for an untouched card to match it later.
pub(crate) fn stored_card_body(content: &str, staged_at: i64) -> String {
    super::memory_point_reads::rehydrate_staged_body(content, staged_at)
}

fn db_err(context: &str) -> impl Fn(libsql::Error) -> WenlanError + '_ {
    move |e| WenlanError::VectorDb(format!("{context}: {e}"))
}

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

/// An open whole-page card, as a shared Space's review step reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenPageCard {
    pub card_id: String,
    pub page_id: String,
    pub edited_by: String,
    /// The AI tool the card names.
    pub proposer: Option<String>,
    /// The page's version the card was staged over.
    pub page_version: Option<i64>,
    /// Bound to a research write, or a link card: it stays on this computer.
    pub local_only: bool,
}

/// Where a page card stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageCardState {
    Open,
    /// A newer proposal for the page took its place; the row is kept.
    Replaced,
    /// Accepted or dismissed (the row is gone or no longer waits).
    Closed,
}

impl MemoryDB {
    /// The newest open whole-page card of `page_id` a new proposal replaces,
    /// if any. A card bound to a research write is left alone: its id is
    /// bound into that write's adoption fence, so it is never retired by
    /// another proposal. Nor is a link card, or a card whose body is no longer
    /// the proposal it was staged with (see the module doc).
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

    /// Retire open card `old_card_id` because `new_card_id`, a newer proposal
    /// for the same page, now stands in its place. Its rows leave the queue
    /// (`pending_revision = 0`) and every reader (`supersede_mode =
    /// 'evicted'`), keep their content, and record `replaced_by` and a
    /// changelog entry with the state they had; one `page_revision_replaced`
    /// row naming `agent` goes to `agent_activity`. Nothing is deleted.
    ///
    /// Checked and written in one write transaction: a card that is no longer
    /// open, or whose body no longer matches its `proposal_digest` (the person
    /// changed it since it was picked), is left as it is. Returns whether the
    /// card was retired.
    pub(crate) async fn retire_replaced_page_card(
        &self,
        old_card_id: &str,
        new_card_id: &str,
        agent: &str,
    ) -> Result<bool, WenlanError> {
        let conn = self.conn.lock().await;
        conn.execute("BEGIN IMMEDIATE", ())
            .await
            .map_err(db_err("retire replaced card begin"))?;
        let result = async {
            let staged_digest = {
                let mut rows = conn
                    .query(
                        "SELECT json_extract(structured_fields, '$.proposal_digest')
                           FROM memories
                          WHERE source = 'memory' AND source_id = ?1
                            AND pending_revision = 1 AND chunk_index = 0
                          LIMIT 1",
                        libsql::params![old_card_id],
                    )
                    .await
                    .map_err(db_err("retire replaced card digest"))?;
                match rows
                    .next()
                    .await
                    .map_err(db_err("retire replaced card digest row"))?
                {
                    Some(row) => row.get::<Option<String>>(0).ok().flatten(),
                    None => None,
                }
            };
            let Some(staged_digest) = staged_digest else {
                return Ok(false);
            };
            let untouched = super::page_rename::staged_card_body_on(&conn, old_card_id)
                .await?
                .is_some_and(|body| {
                    crate::provenance::revision_content_digest(&body) == staged_digest
                });
            if !untouched {
                return Ok(false);
            }
            let mut rows = conn
                .query(
                    "SELECT id, COALESCE(version, 1), changelog, supersede_mode
                       FROM memories
                      WHERE source = 'memory' AND source_id = ?1 AND pending_revision = 1",
                    libsql::params![old_card_id],
                )
                .await
                .map_err(db_err("retire replaced card rows"))?;
            let mut retired = Vec::new();
            while let Some(row) = rows
                .next()
                .await
                .map_err(db_err("retire replaced card row"))?
            {
                let changelog: Vec<serde_json::Value> = row
                    .get::<Option<String>>(2)
                    .ok()
                    .flatten()
                    .filter(|value| !value.trim().is_empty())
                    .and_then(|value| serde_json::from_str(&value).ok())
                    .unwrap_or_default();
                retired.push((
                    row.get::<String>(0)
                        .map_err(db_err("retire replaced card id"))?,
                    row.get::<i64>(1)
                        .map_err(db_err("retire replaced card version"))?,
                    changelog,
                    row.get::<Option<String>>(3).ok().flatten(),
                ));
            }
            drop(rows);
            let now = chrono::Utc::now().timestamp();
            for (id, version, mut changelog, mode) in retired {
                let version = version + 1;
                changelog.push(serde_json::json!({
                    "version": version,
                    "at": now,
                    "delta": "Replaced by a newer proposal for the same page",
                    "source_agent": agent,
                    "previous_supersede_mode": mode,
                    "previous_pending_revision": true,
                    "retirement": REPLACED_RETIREMENT,
                    "replaced_by": new_card_id,
                }));
                let changelog = serde_json::to_string(&changelog).map_err(|e| {
                    WenlanError::VectorDb(format!("retire replaced card changelog: {e}"))
                })?;
                conn.execute(
                    "UPDATE memories
                        SET pending_revision = 0,
                            supersede_mode = 'evicted',
                            version = ?1,
                            changelog = ?2,
                            structured_fields = CASE
                                WHEN structured_fields IS NULL THEN NULL
                                ELSE json_set(structured_fields, '$.replaced_by', ?3)
                            END
                      WHERE id = ?4",
                    libsql::params![version, changelog, new_card_id, id.as_str()],
                )
                .await
                .map_err(db_err("retire replaced card update"))?;
            }
            Self::log_agent_activity_on_connection(
                &conn,
                agent,
                "page_revision_replaced",
                &[old_card_id.to_string(), new_card_id.to_string()],
                None,
                "",
                now,
            )
            .await
            .map_err(db_err("retire replaced card activity"))?;
            Ok(true)
        }
        .await;
        match result {
            Ok(value) => {
                super::commit_or_rollback(&conn)
                    .await
                    .map_err(db_err("retire replaced card commit"))?;
                Ok(value)
            }
            Err(error) => {
                let _ = conn.execute("ROLLBACK", ()).await;
                Err(error)
            }
        }
    }

    /// The page and newer card of `card_id` when a newer proposal replaced
    /// it ([`Self::retire_replaced_page_card`]), so an Accept or Dismiss of
    /// the old id can say so instead of acting on a card the person did not
    /// see. `None` for an open card, an unknown id, or a card retired any
    /// other way.
    pub async fn replaced_page_card(
        &self,
        card_id: &str,
    ) -> Result<Option<ReplacedPageCard>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT supersedes, json_extract(structured_fields, '$.replaced_by')
                   FROM memories
                  WHERE source = 'memory' AND source_id = ?1 AND chunk_index = 0
                    AND pending_revision = 0
                    AND supersede_mode = 'evicted'
                    AND supersedes IS NOT NULL
                    AND json_extract(structured_fields, '$.replaced_by') IS NOT NULL
                  LIMIT 1",
                libsql::params![card_id],
            )
            .await
            .map_err(db_err("replaced page card"))?;
        let Some(row) = rows
            .next()
            .await
            .map_err(db_err("replaced page card row"))?
        else {
            return Ok(None);
        };
        Ok(Some(ReplacedPageCard {
            page_id: row.get::<String>(0).map_err(db_err("replaced card page"))?,
            replaced_by: row
                .get::<String>(1)
                .map_err(db_err("replaced card successor"))?,
        }))
    }

    /// Every open whole-page card, oldest first.
    pub async fn open_page_cards(&self) -> Result<Vec<OpenPageCard>, WenlanError> {
        let sql = format!(
            "SELECT source_id, supersedes,
                    COALESCE(json_extract(structured_fields, '$.edited_by'), ''),
                    source_agent,
                    json_extract(structured_fields, '$.page_version'),
                    json_extract(structured_fields, '$.research_write') IS NOT NULL
             FROM memories
             WHERE {OPEN_CARD_SQL}
               AND json_extract(structured_fields, '$.revision_kind') = ?1
             ORDER BY last_modified, source_id"
        );
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(&sql, libsql::params![PAGE_WRITE_CARD_KIND])
            .await
            .map_err(db_err("open page cards"))?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.map_err(db_err("open page cards row"))? {
            let edited_by: String = row.get(2).map_err(db_err("open page card writer"))?;
            out.push(OpenPageCard {
                card_id: row.get(0).map_err(db_err("open page card id"))?,
                page_id: row.get(1).map_err(db_err("open page card page"))?,
                local_only: row.get::<i64>(5).unwrap_or(0) != 0
                    || edited_by == crate::post_write::LINK_RENAME_WRITER,
                edited_by,
                proposer: row.get::<Option<String>>(3).ok().flatten(),
                page_version: row.get::<Option<i64>>(4).ok().flatten(),
            });
        }
        Ok(out)
    }

    /// Whether page card `card_id` still waits, was replaced by a newer
    /// proposal, or was accepted or dismissed.
    pub async fn page_card_state(&self, card_id: &str) -> Result<PageCardState, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT pending_revision,
                        json_extract(structured_fields, '$.replaced_by') IS NOT NULL
                   FROM memories
                  WHERE source = 'memory' AND source_id = ?1 AND chunk_index = 0
                  LIMIT 1",
                libsql::params![card_id],
            )
            .await
            .map_err(db_err("page card state"))?;
        let Some(row) = rows.next().await.map_err(db_err("page card state row"))? else {
            return Ok(PageCardState::Closed);
        };
        let pending = row.get::<i64>(0).unwrap_or(0) != 0;
        let replaced = row.get::<i64>(1).unwrap_or(0) != 0;
        Ok(match (pending, replaced) {
            (true, _) => PageCardState::Open,
            (false, true) => PageCardState::Replaced,
            (false, false) => PageCardState::Closed,
        })
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
            "a new proposal replaces the newest open card"
        );
        assert_eq!(
            db.list_duplicate_open_revision_cards().await.unwrap(),
            duplicates,
            "the scan is read-only"
        );
        assert_eq!(db.list_pending_revisions(50).await.unwrap().len(), 4);
    }
}
