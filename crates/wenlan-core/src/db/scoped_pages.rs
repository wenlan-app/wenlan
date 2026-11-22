// SPDX-License-Identifier: Apache-2.0

use super::MemoryDB;
use crate::pages::Page;
use crate::read_scope::ReadScope;
use crate::WenlanError;
use std::collections::{HashMap, HashSet};

const SHARED_PAGE_DELETION_PREFIX: &str = "shared_space.deleted:";

fn numbered_conditions(conditions: &[String], start: usize) -> String {
    let mut parameter = start;
    conditions
        .iter()
        .map(|condition| {
            let mut numbered = String::new();
            for character in condition.chars() {
                if character == '?' {
                    numbered.push_str(&format!("?{parameter}"));
                    parameter += 1;
                } else {
                    numbered.push(character);
                }
            }
            numbered
        })
        .collect::<Vec<_>>()
        .join(" AND ")
}

/// The one page-scoping predicate. `pub(super)` so the bulk graph read in
/// `scoped_entities` scopes its page half by the same rule rather than
/// spelling a second copy of it.
pub(super) fn page_scope_clause(
    scope: &ReadScope,
    column: &str,
    parameter: usize,
) -> (String, Option<libsql::Value>) {
    match scope {
        ReadScope::Global => (String::new(), None),
        ReadScope::Space(space) => (
            format!(" AND {column} = ?{parameter}"),
            Some(libsql::Value::Text(space.clone())),
        ),
        ReadScope::Uncategorized => (
            format!(
                " AND ({column} IS NULL OR {column} = '{}')",
                super::UNFILED_SPACE_ID
            ),
            None,
        ),
    }
}

/// When a `page_links` row (joined to its target as `t`) reads as unresolved
/// for `scope`. A link that resolved to a page outside the caller's scope
/// reads exactly like a dangling one, so a `[[Space/Title]]` written by a
/// Space-locked agent cannot tell it whether that title exists elsewhere.
/// `parameter` is the scope value's placeholder, shared with
/// [`page_scope_clause`].
fn unresolved_for_scope(scope: &ReadScope, parameter: usize) -> String {
    let (target_sql, _) = page_scope_clause(scope, "t.workspace", parameter);
    if target_sql.is_empty() {
        return "pl.target_page_id IS NULL".to_string();
    }
    format!("(pl.target_page_id IS NULL OR t.id IS NULL OR NOT COALESCE((1 = 1{target_sql}), 0))")
}

/// The wikilinks a scoped orphan read looks at, shaped like `page_links`.
/// `page_links` keeps only unresolved links; a resolved one lives only as a
/// `links` edge. A scoped reader must also see the resolved ones, so that a
/// link into another Space can read as unresolved (see
/// [`unresolved_for_scope`]); Global only ever needs the unresolved rows.
fn link_rows_for_scope(scope: &ReadScope) -> &'static str {
    if matches!(scope, ReadScope::Global) {
        return "page_links";
    }
    "(SELECT label_key, label, source_page_id, target_page_id FROM page_links \
      UNION ALL \
      SELECT lower(trim(json_extract(payload, '$.label'))), json_extract(payload, '$.label'), \
             src_id, dst_id \
      FROM edges \
      WHERE edge_type = 'links' AND src_kind = 'page' AND dst_kind = 'page' \
        AND valid_until IS NULL AND json_valid(payload) \
        AND json_extract(payload, '$.label') IS NOT NULL)"
}

fn page_not_found() -> WenlanError {
    WenlanError::NotFound("page not found".to_string())
}

fn page_link_label_key(label: &str) -> String {
    label.to_lowercase()
}

/// The page set a Space shares, applied before filtering, ranking or paging.
pub(super) fn shared_page_predicate(alias: &str) -> String {
    member_page_predicate(alias, MemberPageTarget::Published)
}

#[derive(Clone, Copy)]
pub enum MemberPageTarget<'a> {
    Published,
    OwnedDraft(&'a str),
    PublishedOrOwnedDraft(&'a str),
}

fn member_page_predicate(alias: &str, target: MemberPageTarget<'_>) -> String {
    let personal_rules = super::skill_grow::personal_rule_pages_sql();
    let master_prefix = crate::rulesync_team::ID_PREFIX;
    let published = format!("{alias}status IN ('active', 'archived')");
    let draft = || {
        format!(
            "({alias}status = 'draft' AND EXISTS (SELECT 1 FROM page_draft_create_requests r
           WHERE r.page_id = {alias}id AND r.owner_member_id = ?2))"
        )
    };
    let status = match target {
        MemberPageTarget::Published => published,
        MemberPageTarget::OwnedDraft(_) => draft(),
        MemberPageTarget::PublishedOrOwnedDraft(_) => format!("({published} OR {})", draft()),
    };
    format!(
        "{status} AND {alias}workspace IS NOT NULL AND {alias}workspace != '{unfiled}'
           AND COALESCE({alias}kind, 'concept') != 'entity'
           AND COALESCE({alias}creation_kind, 'distilled') NOT IN ('source', 'imported', 'entity')
           AND NOT (LENGTH({alias}id) = 20 AND {alias}id GLOB 'src_*'
                    AND SUBSTR({alias}id, 5) NOT GLOB '*[^0-9a-f]*')
           AND NOT (LENGTH({alias}id) = {master_length} AND SUBSTR({alias}id, 1, {prefix_length}) = '{master_prefix}'
                    AND SUBSTR({alias}id, {digest_start}) NOT GLOB '*[^0-9a-f]*')
           AND {alias}id NOT IN (SELECT page_id FROM ({personal_rules}) WHERE page_id IS NOT NULL)",
        unfiled = super::UNFILED_SPACE_ID,
        master_length = master_prefix.len() + 64,
        prefix_length = master_prefix.len(),
        digest_start = master_prefix.len() + 1,
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PageReader {
    Browse,
    Internal,
    Shared,
}

/// One unresolved wikilink, still attached to the page whose body it came from.
///
/// This is the row `list_orphan_link_labels_scoped` throws away:
/// `COUNT(DISTINCT pl.source_page_id)` folds the source page out inside SQL,
/// and the source page is the only thing the exposure contract has a verdict
/// about -- an orphan label is `[[wikilink]]` text lifted out of ITS body, not
/// a name for the page it fails to resolve to. A reader that must filter by
/// source page therefore needs the rows, not the aggregate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrphanLinkRow {
    pub label_key: String,
    pub label: String,
    pub source_page_id: String,
}

impl MemoryDB {
    /// A member may introduce a new page, or upload to an existing published
    /// page in their Space. A retired ID remains governed by sync's tombstone.
    pub async fn member_may_upload_page_version(
        &self,
        id: &str,
        scope: &ReadScope,
    ) -> Result<bool, WenlanError> {
        let conn = self.conn.lock().await;
        let error = |e: libsql::Error| WenlanError::VectorDb(e.to_string());
        let mut rows = conn
            .query("SELECT 1 FROM pages WHERE id=?1", [id])
            .await
            .map_err(error)?;
        let exists = rows.next().await.map_err(error)?.is_some();
        drop(rows);
        if !exists {
            return Ok(true);
        }
        Self::member_page_access_on(&conn, id, scope, MemberPageTarget::Published).await
    }

    pub async fn member_can_access_page(
        &self,
        id: &str,
        scope: &ReadScope,
        target: MemberPageTarget<'_>,
    ) -> Result<bool, WenlanError> {
        let conn = self.conn.lock().await;
        Self::member_page_access_on(&conn, id, scope, target).await
    }

    /// Run the same predicate as shared reads, inside the write transaction
    /// when authorizing a mutation. Unscoped use is only for Space discovery.
    pub(super) async fn member_page_access_on(
        conn: &libsql::Connection,
        id: &str,
        scope: &ReadScope,
        target: MemberPageTarget<'_>,
    ) -> Result<bool, WenlanError> {
        let mut params = vec![libsql::Value::Text(id.to_owned())];
        match target {
            MemberPageTarget::OwnedDraft(member)
            | MemberPageTarget::PublishedOrOwnedDraft(member) => {
                params.push(libsql::Value::Text(member.to_owned()))
            }
            MemberPageTarget::Published => {}
        }
        let (binding, value) = page_scope_clause(scope, "p.workspace", params.len() + 1);
        params.extend(value);
        let predicate = member_page_predicate("p.", target);
        let mut rows = conn
            .query(
                &format!("SELECT 1 FROM pages p WHERE p.id=?1{binding} AND {predicate}"),
                params,
            )
            .await
            .map_err(|error| WenlanError::VectorDb(error.to_string()))?;
        Ok(rows
            .next()
            .await
            .map_err(|error| WenlanError::VectorDb(error.to_string()))?
            .is_some())
    }

    /// Route-facing Page search. Selected scopes use a workspace predicate in
    /// the same query that ranks and limits candidates; Global preserves the
    /// existing indexed search path.
    ///
    /// Retrieval arm: `kind='entity'` dual-write shadow pages stay fenced
    /// out. Used by `search_memory_cross_rerank_cued`'s direct page channel,
    /// which bypasses `select_visible_pages`. For the browse-facing twin
    /// that shows stub pages (Q1), see `search_pages_scoped_browse`.
    pub async fn search_pages_scoped(
        &self,
        query: &str,
        limit: usize,
        page_type: Option<&str>,
        scope: &ReadScope,
    ) -> Result<Vec<Page>, WenlanError> {
        self.search_pages_scoped_inner(query, limit, page_type, scope, PageReader::Internal)
            .await
    }

    /// Browse-facing twin of `search_pages_scoped`: backs `/api/pages/search`
    /// (+ MCP `search_pages` via the same HTTP route), where Q1 rules
    /// `kind='entity'` dual-write shadow pages must appear. Never call this
    /// from a retrieval/context channel -- those must stay fenced
    /// (`search_pages_scoped`).
    pub async fn search_pages_scoped_browse(
        &self,
        query: &str,
        limit: usize,
        page_type: Option<&str>,
        scope: &ReadScope,
    ) -> Result<Vec<Page>, WenlanError> {
        self.search_pages_scoped_inner(query, limit, page_type, scope, PageReader::Browse)
            .await
    }

    pub async fn search_shared_pages_scoped(
        &self,
        query: &str,
        limit: usize,
        page_type: Option<&str>,
        scope: &ReadScope,
    ) -> Result<Vec<Page>, WenlanError> {
        self.search_pages_scoped_inner(query, limit, page_type, scope, PageReader::Shared)
            .await
    }

    async fn search_pages_scoped_inner(
        &self,
        query: &str,
        limit: usize,
        page_type: Option<&str>,
        scope: &ReadScope,
        reader: PageReader,
    ) -> Result<Vec<Page>, WenlanError> {
        if matches!(scope, ReadScope::Global) && reader != PageReader::Shared {
            return if reader == PageReader::Internal {
                self.search_pages(query, limit, page_type).await
            } else {
                self.search_pages_browse(query, limit, page_type).await
            };
        }

        let embedding = self.get_or_compute_embedding(query)?;
        let vec_str = Self::vec_to_sql(&embedding);
        let fetch_limit = (limit * 3) as i64;
        let conn = self.conn.lock().await;

        // #708: `entity_id` filled in from `entity_page_map` for the
        // `kind='entity'` shadow rows, whose own column is NULL.
        let entity_id_column = super::page_entity_id_column("c.");
        let select = format!(
            "c.id, c.title, c.summary, c.content, {entity_id_column}, c.space, \
             c.source_memory_ids, c.version, c.status, c.created_at, \
             c.last_compiled, c.last_modified, \
             COALESCE(c.sources_updated_count, 0), c.stale_reason, \
             COALESCE(c.user_edited, 0), COALESCE(c.changelog, '[]'), \
             COALESCE(c.creation_kind, 'distilled'), \
             COALESCE(c.review_status, 'confirmed'), c.workspace, c.citations, COALESCE(c.kind, 'concept'), \
             NULL, COALESCE(c.review_required, 0)"
        );
        // Same column layout as the scoped list/read twins: 21 stays NULL,
        // 22 carries the review lock. The vector distance sits at column 23.
        let mut conditions = vec!["c.status = 'active'".to_string()];
        // Fence (M3 PR-1 stage f; Q1 lift, stage c; #708): the retrieval arm
        // (`fence_entity=true`) excludes every `kind='entity'` shadow page --
        // this channel is used by `search_memory_cross_rerank_cued`'s direct
        // page channel, which bypasses `select_visible_pages`. The browse arm
        // (`search_pages_scoped_browse`) keeps established, active entity
        // rows and drops detected and archived ones, matching the Wiki list.
        // In SQL rather than a post-fusion Rust filter so the fence lands
        // before `LIMIT`.
        conditions.push(match reader {
            PageReader::Internal => "COALESCE(c.kind, 'concept') != 'entity'".to_string(),
            PageReader::Browse => super::entity_row_browse_predicate("c."),
            PageReader::Shared => shared_page_predicate("c."),
        });
        let mut values = Vec::new();
        super::push_read_scope_filter_folded(scope, "c.workspace", &mut conditions, &mut values);
        if let Some(page_type) = page_type {
            conditions.push("c.space = ?".to_string());
            values.push(libsql::Value::Text(page_type.to_string()));
        }
        let where_clause = numbered_conditions(&conditions, 3);

        let vector_sql = format!(
            "SELECT {select}, vector_distance_cos(c.embedding, vector32(?1)) AS dist \
             FROM pages c \
             WHERE c.embedding IS NOT NULL AND {where_clause} \
             ORDER BY dist ASC LIMIT ?2"
        );
        let mut vector_params = vec![
            libsql::Value::Text(vec_str),
            libsql::Value::Integer(fetch_limit),
        ];
        vector_params.extend(values.clone());
        let mut vector_results = Vec::new();
        let mut channel_errors = Vec::new();
        let mut vector_succeeded = false;
        match conn.query(&vector_sql, vector_params).await {
            Ok(mut rows) => {
                vector_succeeded = true;
                while let Some(row) = rows.next().await.map_err(|error| {
                    WenlanError::VectorDb(format!("search_pages_scoped vector row: {error}"))
                })? {
                    let page = Self::row_to_page(&row).map_err(|error| {
                        WenlanError::VectorDb(format!("search_pages_scoped vector decode: {error}"))
                    })?;
                    let distance = row.get::<f64>(23).map_err(|error| {
                        WenlanError::VectorDb(format!(
                            "search_pages_scoped vector distance: {error}"
                        ))
                    })?;
                    vector_results.push((page.id.clone(), distance, page));
                }
            }
            Err(error) => {
                log::warn!("[search_pages_scoped] vector search failed: {error}");
                channel_errors.push(format!("vector: {error}"));
            }
        }

        use crate::retrieval::fts_query::{
            fts_recall_hardening_enabled, hardened_page_fts_queries,
        };
        let fts_queries = if fts_recall_hardening_enabled() {
            hardened_page_fts_queries(query)
        } else {
            vec![query.to_string(), Self::fts_or_query(query)]
        };
        let fts_sql = format!(
            "SELECT {select} FROM pages c \
             JOIN pages_fts f ON c.rowid = f.rowid \
             WHERE pages_fts MATCH ?1 AND {where_clause} \
             ORDER BY rank LIMIT ?2"
        );
        let mut fts_results = Vec::new();
        let mut fts_succeeded = false;
        // Only the first query requires every word; the legacy OR fallback
        // matches any one of them.
        let mut fts_matched_all_terms = false;
        for (fts_index, fts_query) in fts_queries.into_iter().enumerate() {
            let mut params = vec![
                libsql::Value::Text(fts_query),
                libsql::Value::Integer(fetch_limit),
            ];
            params.extend(values.clone());
            match conn.query(&fts_sql, params).await {
                Ok(mut rows) => {
                    fts_succeeded = true;
                    while let Some(row) = rows.next().await.map_err(|error| {
                        WenlanError::VectorDb(format!("search_pages_scoped FTS row: {error}"))
                    })? {
                        let page = Self::row_to_page(&row).map_err(|error| {
                            WenlanError::VectorDb(format!(
                                "search_pages_scoped FTS decode: {error}"
                            ))
                        })?;
                        fts_results.push((page.id.clone(), page));
                    }
                }
                Err(error) => {
                    log::debug!("[search_pages_scoped] FTS search failed: {error}");
                    channel_errors.push(format!("fts: {error}"));
                }
            }
            if !fts_results.is_empty() {
                fts_matched_all_terms = fts_index == 0;
                break;
            }
        }
        if !vector_succeeded && !fts_succeeded {
            return Err(WenlanError::VectorDb(format!(
                "search_pages_scoped has no available search channel: {}",
                channel_errors.join("; ")
            )));
        }
        drop(conn);

        let vector_ranked: Vec<(String, f64)> = vector_results
            .iter()
            .map(|(id, distance, _)| (id.clone(), *distance))
            .collect();
        let fts_ranked: Vec<String> = fts_results.iter().map(|(id, _)| id.clone()).collect();
        let scores = crate::pages::fuse_page_channel_scores(
            &vector_ranked,
            &fts_ranked,
            fts_matched_all_terms,
        );
        let mut pages: HashMap<String, Page> = HashMap::new();
        for (id, _, page) in vector_results {
            pages.entry(id).or_insert(page);
        }
        for (id, page) in fts_results {
            pages.entry(id).or_insert(page);
        }

        // The entity fence is a SQL condition on both channels above (see
        // `conditions`), so nothing is filtered out here.
        let mut results: Vec<Page> = pages.into_values().collect();
        results.sort_by(|left, right| {
            scores
                .get(&right.id)
                .unwrap_or(&0.0)
                .partial_cmp(scores.get(&left.id).unwrap_or(&0.0))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.id.cmp(&right.id))
        });
        results.truncate(limit);
        for page in &mut results {
            page.relevance_score = *scores.get(&page.id).unwrap_or(&0.0);
        }
        Ok(results)
    }

    pub async fn list_recent_pages_with_badges_scoped(
        &self,
        limit: i64,
        since_ms: Option<i64>,
        scope: &ReadScope,
    ) -> Result<Vec<wenlan_types::RecentActivityItem>, WenlanError> {
        self.list_recent_page_activity(limit, since_ms, scope, true)
            .await
    }

    pub async fn list_recent_shared_pages(
        &self,
        limit: i64,
        since_ms: Option<i64>,
        scope: &ReadScope,
    ) -> Result<Vec<wenlan_types::RecentActivityItem>, WenlanError> {
        self.list_recent_page_activity(limit, since_ms, scope, false)
            .await
    }

    async fn list_recent_page_activity(
        &self,
        limit: i64,
        since_ms: Option<i64>,
        scope: &ReadScope,
        private: bool,
    ) -> Result<Vec<wenlan_types::RecentActivityItem>, WenlanError> {
        if private && matches!(scope, ReadScope::Global) {
            return self.list_recent_pages_with_badges(limit, since_ms).await;
        }

        use wenlan_types::{ActivityBadge, ActivityKind, RecentActivityItem};

        // Q1 (stage c): `kind='entity'` dual-write shadow pages appear in this
        // recent-activity listing -- a browse surface, so it reads the unfenced
        // `list_pages_scoped_browse` twin.
        let pages = if private {
            self.list_pages_scoped_browse("active", limit, 0, scope)
                .await?
        } else {
            self.list_shared_pages_scoped("active", limit, 0, scope)
                .await?
        };
        let (visible_source_ids, flagged) = if private {
            let all_source_ids = pages
                .iter()
                .flat_map(|page| page.source_memory_ids.iter().cloned())
                .collect::<Vec<_>>();
            let visible = self
                .get_memories_by_source_ids_scoped(&all_source_ids, scope)
                .await?
                .into_iter()
                .map(|memory| memory.source_id)
                .collect::<HashSet<_>>();
            let flagged = self
                .pending_review_memory_ids(&visible.iter().cloned().collect::<Vec<_>>())
                .await?;
            (visible, flagged)
        } else {
            (HashSet::new(), HashSet::new())
        };
        let since_rfc = since_ms.and_then(|ms| {
            chrono::DateTime::from_timestamp(ms / 1000, 0).map(|dt| dt.to_rfc3339())
        });
        let since_s = since_ms.map(|ms| ms / 1000);

        let mut items = Vec::with_capacity(pages.len());
        for page in pages {
            let visible_members = page
                .source_memory_ids
                .iter()
                .filter(|id| visible_source_ids.contains(id.as_str()))
                .cloned()
                .collect::<Vec<_>>();
            let needs_review = visible_members.iter().any(|id| flagged.contains(id));
            let badge = if !private {
                if since_rfc
                    .as_ref()
                    .is_some_and(|since| page.last_modified >= *since)
                {
                    let rows = self.list_shared_page_history(&page.id, scope, 2).await?;
                    if rows
                        .get(1)
                        .is_some_and(|previous| previous.version == page.version - 1)
                    {
                        ActivityBadge::Revised
                    } else {
                        ActivityBadge::New
                    }
                } else {
                    ActivityBadge::None
                }
            } else if needs_review {
                ActivityBadge::NeedsReview
            } else if let Some(ref since) = since_rfc {
                if page.created_at >= *since {
                    ActivityBadge::New
                } else {
                    let growing_count = if visible_members.is_empty() {
                        0
                    } else {
                        let placeholders = (2..visible_members.len() + 2)
                            .map(|index| format!("?{index}"))
                            .collect::<Vec<_>>()
                            .join(", ");
                        let scope_parameter = visible_members.len() + 2;
                        let (scope_sql, scope_value) =
                            page_scope_clause(scope, "space", scope_parameter);
                        let sql = format!(
                            "SELECT COUNT(*) FROM memories \
                             WHERE source != 'episode' AND source_id IN ({placeholders}) \
                               AND created_at >= ?1{scope_sql}"
                        );
                        let mut params = vec![libsql::Value::Integer(since_s.unwrap_or(0))];
                        params.extend(visible_members.iter().cloned().map(libsql::Value::Text));
                        if let Some(value) = scope_value {
                            params.push(value);
                        }
                        let conn = self.conn.lock().await;
                        let mut rows = conn.query(&sql, params).await.map_err(|error| {
                            WenlanError::VectorDb(format!(
                                "list_recent_pages_with_badges_scoped growth count: {error}"
                            ))
                        })?;
                        rows.next()
                            .await
                            .map_err(|error| WenlanError::VectorDb(error.to_string()))?
                            .map(|row| row.get::<i64>(0).unwrap_or(0))
                            .unwrap_or(0)
                    };
                    if growing_count > 0 {
                        ActivityBadge::Growing {
                            added: growing_count as u32,
                        }
                    } else if page.version > 1 && page.last_modified >= *since {
                        ActivityBadge::Revised
                    } else {
                        ActivityBadge::None
                    }
                }
            } else {
                ActivityBadge::None
            };
            let timestamp_ms = chrono::DateTime::parse_from_rfc3339(&page.last_modified)
                .map(|dt| dt.timestamp_millis() as u64)
                .unwrap_or(0);
            items.push(RecentActivityItem {
                kind: ActivityKind::Page,
                id: page.id,
                title: page.title,
                snippet: page.summary.filter(|summary| !summary.is_empty()),
                timestamp_ms,
                badge,
            });
        }
        Ok(items)
    }

    pub async fn list_recent_changes_scoped(
        &self,
        limit: i64,
        scope: &ReadScope,
    ) -> Result<Vec<wenlan_types::PageChange>, WenlanError> {
        self.list_recent_page_changes(limit, scope, true).await
    }

    pub async fn list_recent_shared_page_changes(
        &self,
        limit: i64,
        scope: &ReadScope,
    ) -> Result<Vec<crate::member_view::MemberChange>, WenlanError> {
        Ok(self
            .list_recent_page_changes(limit, scope, false)
            .await?
            .into_iter()
            .map(crate::member_view::MemberChange::new)
            .collect())
    }

    async fn list_recent_page_changes(
        &self,
        limit: i64,
        scope: &ReadScope,
        include_entities: bool,
    ) -> Result<Vec<wenlan_types::PageChange>, WenlanError> {
        if include_entities && matches!(scope, ReadScope::Global) {
            return self.list_recent_changes(limit).await;
        }
        let (scope_sql, scope_value) = page_scope_clause(scope, "c.workspace", 2);
        let shared_fence = if include_entities {
            String::new()
        } else {
            format!("AND {}", shared_page_predicate("c."))
        };
        let sql = format!(
            "SELECT c.id, c.title, c.version, c.created_at, c.last_modified, c.status \
             FROM pages c WHERE c.status IN ('active', 'archived'){scope_sql} {shared_fence} \
             ORDER BY c.last_modified DESC LIMIT ?1"
        );
        let mut params = vec![libsql::Value::Integer(limit)];
        if let Some(value) = scope_value {
            params.push(value);
        }
        let conn = self.conn.lock().await;
        let mut rows = conn.query(&sql, params).await.map_err(|error| {
            WenlanError::VectorDb(format!("list_recent_changes_scoped: {error}"))
        })?;
        let mut changes = Vec::new();
        let mut versions = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|error| WenlanError::VectorDb(error.to_string()))?
        {
            let version = row.get::<i64>(2).unwrap_or(1);
            let created_at = row.get::<String>(3).unwrap_or_default();
            let last_modified = row.get::<String>(4).unwrap_or_default();
            let change_kind = if row.get::<String>(5).unwrap_or_default() == "archived" {
                wenlan_types::PageChangeKind::Archived
            } else if include_entities && (version > 1 || created_at != last_modified) {
                wenlan_types::PageChangeKind::Revised
            } else {
                wenlan_types::PageChangeKind::Created
            };
            changes.push(wenlan_types::PageChange {
                page_id: row.get(0).unwrap_or_default(),
                title: row.get(1).unwrap_or_default(),
                change_kind,
                changed_at_ms: chrono::DateTime::parse_from_rfc3339(&last_modified)
                    .map(|dt| dt.timestamp_millis())
                    .unwrap_or(0),
                edited_by: None,
                changed_sentences: None,
                agent: None,
            });
            versions.push(version);
        }
        drop(rows);
        drop(conn);
        if include_entities {
            self.annotate_page_changes(&mut changes, &versions).await;
        } else {
            for (change, version) in changes.iter_mut().zip(versions) {
                let rows = self
                    .list_shared_page_history(&change.page_id, scope, 2)
                    .await?;
                let Some(current) = rows.first().filter(|row| row.version == version) else {
                    continue;
                };
                use wenlan_types::PageChangeKind;
                if let Some((marker, actor)) =
                    self.page_lifecycle_at(&change.page_id, version).await
                {
                    change.change_kind = if marker == "archive" {
                        PageChangeKind::Archived
                    } else {
                        PageChangeKind::Restored
                    };
                    change.edited_by = actor;
                } else {
                    change.change_kind = PageChangeKind::Created;
                    change.edited_by = Some(current.edited_by.clone());
                    if let Some(previous) = rows.get(1).filter(|row| row.version == version - 1) {
                        change.change_kind = PageChangeKind::Revised;
                        change.changed_sentences = Some(crate::authorship::changed_sentence_count(
                            &previous.content,
                            &current.content,
                        ));
                    }
                }
            }
        }
        Ok(changes)
    }

    /// Fenced: `kind='entity'` dual-write shadow pages stay excluded on every
    /// scope arm (Global delegates to the fenced `list_pages`). Export and
    /// other internal consumers that must never emit a stub call this, so the
    /// fence lands in SQL before `LIMIT` -- a burst of fresh stubs can never
    /// crowd real pages out of a bounded window. For the browse-facing twin
    /// (Q1 read surfaces), see `list_pages_scoped_browse`.
    pub async fn list_pages_scoped(
        &self,
        status: &str,
        limit: i64,
        offset: i64,
        scope: &ReadScope,
    ) -> Result<Vec<Page>, WenlanError> {
        self.list_pages_scoped_inner(status, limit, offset, scope, PageReader::Internal)
            .await
    }

    /// Browse-facing twin of `list_pages_scoped`: backs `GET /api/pages` and
    /// the recent-with-badges browse path, where Q1 rules `kind='entity'`
    /// dual-write shadow pages must appear on every scope arm (Global delegates
    /// to `list_pages_browse`). Never call this from export or any internal
    /// non-browse path -- those must stay fenced (`list_pages_scoped`).
    pub async fn list_pages_scoped_browse(
        &self,
        status: &str,
        limit: i64,
        offset: i64,
        scope: &ReadScope,
    ) -> Result<Vec<Page>, WenlanError> {
        self.list_pages_scoped_inner(status, limit, offset, scope, PageReader::Browse)
            .await
    }

    pub async fn list_shared_pages_scoped(
        &self,
        status: &str,
        limit: i64,
        offset: i64,
        scope: &ReadScope,
    ) -> Result<Vec<Page>, WenlanError> {
        self.list_pages_scoped_inner(status, limit, offset, scope, PageReader::Shared)
            .await
    }

    async fn list_pages_scoped_inner(
        &self,
        status: &str,
        limit: i64,
        offset: i64,
        scope: &ReadScope,
        reader: PageReader,
    ) -> Result<Vec<Page>, WenlanError> {
        if matches!(scope, ReadScope::Global) && reader != PageReader::Shared {
            return if reader == PageReader::Internal {
                self.list_pages(status, limit, offset).await
            } else {
                self.list_pages_browse(status, limit, offset).await
            };
        }
        // #708: `entity_id` is filled in from `entity_page_map` for the
        // `kind='entity'` shadow rows, whose own column is NULL.
        let entity_id_column = super::page_entity_id_column("c.");
        let select = format!(
            "c.id, c.title, c.summary, c.content, {entity_id_column}, c.space, \
             c.source_memory_ids, c.version, c.status, c.created_at, \
             c.last_compiled, c.last_modified, \
             COALESCE(c.sources_updated_count, 0), c.stale_reason, \
             COALESCE(c.user_edited, 0), COALESCE(c.changelog, '[]'), \
             COALESCE(c.creation_kind, 'distilled'), \
             COALESCE(c.review_status, 'confirmed'), c.workspace, c.citations, COALESCE(c.kind, 'concept'), \
             NULL, COALESCE(c.review_required, 0)"
        );
        // Column 21 (`refresh_blocked_reason`) stays NULL on the scoped browse
        // twins as before; column 22 carries the review lock so list_pages and
        // read_page can report it.
        // #708: the browse twin keeps established, active entity rows and
        // drops detected and archived ones; the fenced twin drops every
        // entity row as before. Both land in SQL before `LIMIT`, so a burst
        // of fresh detected entities can never crowd real pages out of a
        // bounded window.
        let fence_sql = match reader {
            PageReader::Internal => " AND COALESCE(c.kind, 'concept') != 'entity'".to_string(),
            PageReader::Browse => super::entity_row_browse_fence("c."),
            PageReader::Shared => format!(" AND {}", shared_page_predicate("c.")),
        };
        let (sql, params) = match scope {
            ReadScope::Space(workspace) => (
                format!(
                    "SELECT {select} FROM pages c \
                     WHERE c.status = ?1{fence_sql} AND c.workspace = ?2 \
                     ORDER BY c.last_modified DESC LIMIT ?3 OFFSET ?4"
                ),
                vec![
                    libsql::Value::Text(status.to_string()),
                    libsql::Value::Text(workspace.clone()),
                    libsql::Value::Integer(limit),
                    libsql::Value::Integer(offset),
                ],
            ),
            ReadScope::Uncategorized => (
                format!(
                    "SELECT {select} FROM pages c \
                     WHERE c.status = ?1{fence_sql} AND c.workspace = '00000000-0000-4000-8000-000000000001' \
                     ORDER BY c.last_modified DESC LIMIT ?2 OFFSET ?3"
                ),
                vec![
                    libsql::Value::Text(status.to_string()),
                    libsql::Value::Integer(limit),
                    libsql::Value::Integer(offset),
                ],
            ),
            ReadScope::Global => (
                format!(
                    "SELECT {select} FROM pages c WHERE c.status = ?1{fence_sql}
                     ORDER BY c.last_modified DESC LIMIT ?2 OFFSET ?3"
                ),
                vec![
                    libsql::Value::Text(status.to_string()),
                    libsql::Value::Integer(limit),
                    libsql::Value::Integer(offset),
                ],
            ),
        };
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(&sql, params)
            .await
            .map_err(|error| WenlanError::VectorDb(format!("list_pages_scoped: {error}")))?;
        let mut pages = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|error| WenlanError::VectorDb(error.to_string()))?
        {
            pages.push(Self::row_to_page(&row)?);
        }
        Ok(pages)
    }

    /// Every orphan link as a row, source page intact, for readers that have to
    /// filter before they fold.
    ///
    /// Same predicate as `list_orphan_link_labels_scoped` -- unresolved target,
    /// active source page, the scope clause -- with the aggregation removed.
    /// Splitting it that way is the whole point: the aggregate applies
    /// `HAVING n >= ?` and `LIMIT 100` inside SQL, so a caller that filters the
    /// aggregate's output reports counts and a truncation point computed over
    /// pages it may not see. Pair this with [`MemoryDB::fold_orphan_labels`],
    /// which reproduces the aggregate over whatever survives the filter.
    ///
    /// Global is handled here rather than delegating to a separate twin: with
    /// no `min_count` parameter the scope clause is the only difference between
    /// the arms, and `page_scope_clause` already answers empty for Global.
    ///
    /// ponytail: the flat `LIMIT 20000` is a ceiling, not a page. Orphan links
    /// are a per-page-body artifact, and a graph that produces more than 20k of
    /// them has a bigger problem than this feed. If one ever does, the upgrade
    /// is a keyset walk over `label_key` feeding the same fold, not a larger
    /// number here.
    pub async fn list_orphan_link_rows_scoped(
        &self,
        scope: &ReadScope,
    ) -> Result<Vec<OrphanLinkRow>, WenlanError> {
        let (scope_sql, scope_value) = page_scope_clause(scope, "p.workspace", 1);
        let unresolved = unresolved_for_scope(scope, 1);
        let links = link_rows_for_scope(scope);
        let sql = format!(
            "SELECT pl.label_key, pl.label, pl.source_page_id \
             FROM {links} pl INNER JOIN pages p ON p.id = pl.source_page_id \
             LEFT JOIN pages t ON t.id = pl.target_page_id \
             WHERE {unresolved} AND p.status = 'active'{scope_sql} \
             LIMIT 20000"
        );
        let mut params = Vec::new();
        if let Some(value) = scope_value {
            params.push(value);
        }
        let conn = self.conn.lock().await;
        let mut rows = conn.query(&sql, params).await.map_err(|error| {
            WenlanError::VectorDb(format!("list_orphan_link_rows_scoped: {error}"))
        })?;
        let mut out = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|error| WenlanError::VectorDb(error.to_string()))?
        {
            out.push(OrphanLinkRow {
                label_key: row.get(0).unwrap_or_default(),
                label: row.get(1).unwrap_or_default(),
                source_page_id: row.get(2).unwrap_or_default(),
            });
        }
        Ok(out)
    }

    /// The aggregate twin's `GROUP BY label_key HAVING n >= ? ORDER BY n DESC,
    /// display_label ASC LIMIT 100`, in Rust, over rows a caller has already
    /// filtered.
    ///
    /// It has to be a re-implementation rather than a second query, because the
    /// point is to count and cap over the pages *this* caller may see and SQL
    /// cannot know that. The semantics are copied deliberately: display label is
    /// the lexicographic minimum in the group (`MIN(pl.label)`, BINARY collation,
    /// which is `String::cmp`), and `n` counts DISTINCT source pages.
    /// `page_links` is keyed `(source_page_id, label_key)`, so a duplicate pair
    /// cannot exist today -- the DISTINCT logic is written out anyway rather
    /// than leaning on a primary key this function has no way to check.
    ///
    /// Pure and DB-free; it hangs off `MemoryDB` only because that is what makes
    /// it nameable outside `db`'s private module tree.
    pub fn fold_orphan_labels(rows: Vec<OrphanLinkRow>, min_count: i64) -> Vec<(String, i64)> {
        let mut groups: HashMap<String, (String, HashSet<String>)> = HashMap::new();
        for row in rows {
            let group = groups
                .entry(row.label_key)
                .or_insert_with(|| (row.label.clone(), HashSet::new()));
            if row.label < group.0 {
                group.0 = row.label;
            }
            group.1.insert(row.source_page_id);
        }
        let mut folded: Vec<(String, i64)> = groups
            .into_values()
            .map(|(label, sources)| (label, sources.len() as i64))
            .filter(|(_, n)| *n >= min_count)
            .collect();
        folded.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
        folded.truncate(100);
        folded
    }

    pub async fn list_orphan_link_labels_scoped(
        &self,
        min_count: i64,
        scope: &ReadScope,
    ) -> Result<Vec<(String, i64)>, WenlanError> {
        if matches!(scope, ReadScope::Global) {
            return self.list_orphan_link_labels(min_count).await;
        }
        let (scope_sql, scope_value) = page_scope_clause(scope, "p.workspace", 2);
        let unresolved = unresolved_for_scope(scope, 2);
        let links = link_rows_for_scope(scope);
        let sql = format!(
            "SELECT MIN(pl.label) AS display_label, \
                    COUNT(DISTINCT pl.source_page_id) AS n \
             FROM {links} pl INNER JOIN pages p ON p.id = pl.source_page_id \
             LEFT JOIN pages t ON t.id = pl.target_page_id \
             WHERE {unresolved} AND p.status = 'active'{scope_sql} \
             GROUP BY pl.label_key HAVING n >= ?1 \
             ORDER BY n DESC, display_label ASC LIMIT 100"
        );
        let mut params = vec![libsql::Value::Integer(min_count)];
        if let Some(value) = scope_value {
            params.push(value);
        }
        let conn = self.conn.lock().await;
        let mut rows = conn.query(&sql, params).await.map_err(|error| {
            WenlanError::VectorDb(format!("list_orphan_link_labels_scoped: {error}"))
        })?;
        let mut labels = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|error| WenlanError::VectorDb(error.to_string()))?
        {
            labels.push((row.get(0).unwrap_or_default(), row.get(1).unwrap_or(0)));
        }
        Ok(labels)
    }

    /// Fenced: `kind='entity'` dual-write shadow pages stay excluded, so a
    /// by-id read on a mutation/export path resolves a stub as absent (None).
    /// For the browse-facing twin that returns stub pages by id (Q1 read
    /// surfaces), see `get_page_scoped_browse`.
    pub async fn get_page_scoped(
        &self,
        id: &str,
        scope: &ReadScope,
    ) -> Result<Option<Page>, WenlanError> {
        self.get_page_scoped_inner(id, scope, PageReader::Internal)
            .await
    }

    /// Browse-facing twin of `get_page_scoped`: backs `GET /api/pages/{id}` and
    /// its advisory revisions read, where Q1 rules `kind='entity'` dual-write
    /// shadow pages must resolve. Never call this from a mutation/export path --
    /// those must stay fenced (`get_page_scoped`).
    pub async fn get_page_scoped_browse(
        &self,
        id: &str,
        scope: &ReadScope,
    ) -> Result<Option<Page>, WenlanError> {
        self.get_page_scoped_inner(id, scope, PageReader::Browse)
            .await
    }

    /// Eligibility for the Space's shared folder, including archive delivery
    /// for pages verified as shared when the host permanently deleted them.
    pub async fn shared_folder_pages(
        &self,
        space: &str,
    ) -> Result<crate::team::SharedFolderPages, WenlanError> {
        let shared = shared_page_predicate("p.");
        let id_start = SHARED_PAGE_DELETION_PREFIX.len() + 1;
        let sql = format!(
            "SELECT p.id, p.title, 0, p.incarnation, s.id,
                    COALESCE((SELECT MAX(h.version) FROM page_history h
                               WHERE h.page_id=p.id AND h.edited_by='space_move'), 0),
                    p.version,
                    CASE WHEN p.status = 'active'
                          AND COALESCE(p.stale_reason, '') NOT IN ('source_updated', 'source_removed')
                    THEN COALESCE(p.source_revision, 0) END,
                    NULL, NULL, NULL
               FROM pages p JOIN spaces s ON s.name=p.workspace
              WHERE p.workspace = ?1 AND {shared}
             UNION ALL
             SELECT substr(m.key, {id_start}), '', 1,
                    CASE WHEN json_valid(m.value) THEN json_extract(m.value, '$.stay') END,
                    NULL, NULL, NULL, NULL,
                    CASE WHEN json_valid(m.value) THEN json_extract(m.value, '$.event_id') END,
                    CASE WHEN json_valid(m.value) THEN json_extract(m.value, '$.parent_hash') END,
                    CASE WHEN json_valid(m.value) THEN json_extract(m.value, '$.archive_hash') END
               FROM app_metadata m JOIN spaces s ON s.id =
                    CASE WHEN json_valid(m.value) THEN json_extract(m.value, '$.space')
                         ELSE m.value END
              WHERE s.name = ?1 AND m.key GLOB '{SHARED_PAGE_DELETION_PREFIX}*'
                AND NOT EXISTS (SELECT 1 FROM pages p WHERE p.id = substr(m.key, {id_start}))"
        );
        let error = |e| WenlanError::VectorDb(format!("shared folder pages: {e}"));
        let conn = self.conn.lock().await;
        let receipts_key = format!("shared_space.member_view_receipts:{space}");
        let mut stored_receipts = conn
            .query(
                "SELECT value FROM app_metadata WHERE key=?1",
                [receipts_key.as_str()],
            )
            .await
            .map_err(error)?;
        let receipts = match stored_receipts.next().await.map_err(error)? {
            Some(row) => row
                .get::<String>(0)
                .ok()
                .and_then(|value| {
                    serde_json::from_str::<
                        std::collections::HashMap<String, crate::shared_space::MemberViewReceipt>,
                    >(&value)
                    .ok()
                })
                .unwrap_or_default(),
            None => std::collections::HashMap::new(),
        };
        drop(stored_receipts);
        let mut rows = conn.query(&sql, [space]).await.map_err(error)?;
        let mut pages = crate::team::SharedFolderPages::default();
        while let Some(row) = rows.next().await.map_err(error)? {
            let id = row.get::<String>(0).map_err(error)?;
            if row.get::<i64>(2).map_err(error)? == 1 {
                pages.deleted.insert(id.clone());
                let event_id = row.get::<Option<String>>(8).map_err(error)?;
                let parent_hash = row.get::<Option<String>>(9).map_err(error)?;
                let archive_hash = row.get::<Option<String>>(10).map_err(error)?;
                if let Some(event_id) = event_id.filter(|event_id| !event_id.is_empty()) {
                    pages.deletions.insert(
                        id.clone(),
                        crate::team::SharedDeletion {
                            event_id,
                            parent_hash,
                            archive_hash,
                        },
                    );
                }
                if let Some(stay) = row.get::<Option<String>>(3).map_err(error)? {
                    if let Some(receipt) = receipts.get(&id).filter(|receipt| receipt.stay == stay)
                    {
                        pages.authorized.insert(id.clone(), receipt.hashes.clone());
                        if let Some(archive) = pages
                            .deletions
                            .get(&id)
                            .and_then(|deletion| deletion.archive_hash.as_ref())
                            .filter(|archive| receipt.hashes.contains(*archive))
                        {
                            pages.final_archives.insert(id.clone(), archive.clone());
                        }
                    }
                    pages.stays.insert(id, stay);
                }
            } else {
                let incarnation = row.get::<String>(3).map_err(error)?;
                let space_id = row.get::<String>(4).map_err(error)?;
                let stay_start = row.get::<i64>(5).map_err(error)?;
                let stay =
                    crate::shared_space::version::stay_token(&incarnation, &space_id, stay_start);
                pages.stays.insert(id.clone(), stay.clone());
                if let Some(receipt) = receipts.get(&id).filter(|receipt| receipt.stay == stay) {
                    pages.authorized.insert(id.clone(), receipt.hashes.clone());
                }
                // Forget and permanent source deletion invalidate dependent
                // prose in the source transaction. It cannot back a migrated head.
                if let Some(source_revision) = row.get::<Option<i64>>(7).map_err(error)? {
                    pages.legacy_active_heads.insert(
                        id.clone(),
                        crate::team::LegacyHeadProof {
                            stay,
                            revision: row.get(6).map_err(error)?,
                            source_revision,
                        },
                    );
                }
                pages.shared.insert(id, row.get(1).map_err(error)?);
            }
        }
        pages.receipts = receipts;
        Ok(pages)
    }

    /// Current incarnation and continuous Space-stay witness for a shared
    /// page. Snapshot exports use this token to break lineage across moves.
    pub(crate) async fn shared_page_stay_token(
        &self,
        page: &Page,
        space: &str,
    ) -> Result<Option<String>, WenlanError> {
        let shared = shared_page_predicate("p.");
        let error = |e| WenlanError::VectorDb(format!("shared page stay: {e}"));
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                &format!(
                    "SELECT p.incarnation, s.id,
                            COALESCE((SELECT MAX(h.version) FROM page_history h
                                       WHERE h.page_id=p.id AND h.edited_by='space_move'), 0)
                       FROM pages p JOIN spaces s ON s.name=p.workspace
                      WHERE p.id=?1 AND p.workspace=?2 AND p.version=?3
                            AND p.title=?4 AND p.content=?5 AND {shared}"
                ),
                libsql::params![
                    page.id.as_str(),
                    space,
                    page.version,
                    page.title.as_str(),
                    page.content.as_str()
                ],
            )
            .await
            .map_err(error)?;
        let Some(row) = rows.next().await.map_err(error)? else {
            return Ok(None);
        };
        Ok(Some(crate::shared_space::version::stay_token(
            &row.get::<String>(0).map_err(error)?,
            &row.get::<String>(1).map_err(error)?,
            row.get::<i64>(2).map_err(error)?,
        )))
    }

    pub(super) async fn record_shared_page_deletion_on(
        conn: &libsql::Connection,
        id: &str,
    ) -> Result<(), WenlanError> {
        let key = format!("{SHARED_PAGE_DELETION_PREFIX}{id}");
        let error = |e| WenlanError::VectorDb(format!("shared page deletion: {e}"));
        let shared = shared_page_predicate("p.");
        let mut rows = conn.query(
            &format!(
                "SELECT p.version, p.incarnation, s.id,
                        COALESCE((SELECT MAX(v.version) FROM page_history v
                                  WHERE v.page_id=p.id AND v.edited_by='space_move'), 0),
                        h.value
                   FROM pages p
                   JOIN spaces s ON s.name = p.workspace
                   JOIN app_metadata h ON h.key = 'shared_space.held:' || s.name
                  WHERE p.id = ?1 AND {shared}
                    AND EXISTS (
                        SELECT 1 FROM json_each(CASE WHEN json_valid(h.value) THEN h.value ELSE '{{}}' END)
                         WHERE key = p.id
                    )"
            ),
            libsql::params![id],
        )
        .await
        .map_err(error)?;
        let mut shared_record = None;
        if let Some(row) = rows.next().await.map_err(error)? {
            let page_version: i64 = row.get(0).map_err(error)?;
            let incarnation: String = row.get(1).map_err(error)?;
            let space_id: String = row.get(2).map_err(error)?;
            let start: i64 = row.get(3).map_err(error)?;
            let held_json: String = row.get(4).map_err(error)?;
            let stay = crate::shared_space::version::stay_token(&incarnation, &space_id, start);
            let held = serde_json::from_str::<serde_json::Value>(&held_json).ok();
            let held_page = held.as_ref().and_then(|held| held.get(id));
            let parent_hash = held_page
                .filter(|held| {
                    held.get("version").and_then(serde_json::Value::as_i64) == Some(page_version)
                })
                .and_then(|held| held.get("sha"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned);
            shared_record = Some((space_id, stay, parent_hash));
        }
        drop(rows);
        // This private tombstone is written for every deletion, including IDs
        // that were never shared. Only the optional space/stay fields make it
        // visible to a member's shared-folder projection.
        let event_id = uuid::Uuid::new_v4().simple().to_string();
        let mut record = serde_json::json!({"event_id": event_id});
        if let Some((space, stay, parent_hash)) = shared_record {
            record["space"] = serde_json::Value::String(space);
            record["stay"] = serde_json::Value::String(stay);
            if let Some(parent_hash) = parent_hash {
                record["parent_hash"] = serde_json::Value::String(parent_hash);
            }
        }
        let value = record.to_string();
        conn.execute(
            "INSERT OR REPLACE INTO app_metadata(key, value) VALUES (?1, ?2)",
            libsql::params![key, value],
        )
        .await
        .map_err(error)?;
        Ok(())
    }

    /// Bind the first verified archive to this exact durable deletion event.
    /// A later folder winner or restored page cannot reassign the binding.
    pub async fn bind_shared_deletion_archive(
        &self,
        id: &str,
        deletion_id: &str,
        archive_hash: &str,
    ) -> Result<bool, WenlanError> {
        if archive_hash.len() != 64
            || !archive_hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Ok(false);
        }
        let key = format!("{SHARED_PAGE_DELETION_PREFIX}{id}");
        let conn = self.conn.lock().await;
        let error = |e| WenlanError::VectorDb(format!("bind shared deletion archive: {e}"));
        let mut rows = conn
            .query(
                "SELECT value FROM app_metadata WHERE key = ?1",
                [key.as_str()],
            )
            .await
            .map_err(error)?;
        let Some(row) = rows.next().await.map_err(error)? else {
            return Ok(false);
        };
        let old_value: String = row.get(0).map_err(error)?;
        drop(rows);
        let Ok(mut record) = serde_json::from_str::<serde_json::Value>(&old_value) else {
            return Ok(false);
        };
        let pinned_parent = record
            .get("parent_hash")
            .and_then(serde_json::Value::as_str);
        if record.get("event_id").and_then(serde_json::Value::as_str) != Some(deletion_id)
            || record
                .get("space")
                .and_then(serde_json::Value::as_str)
                .is_none_or(str::is_empty)
            || record
                .get("stay")
                .and_then(serde_json::Value::as_str)
                .is_none_or(str::is_empty)
            || pinned_parent.is_none_or(|hash| {
                hash.len() != 64
                    || !hash
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
            || record
                .get("archive_hash")
                .is_some_and(|value| !value.is_null())
        {
            return Ok(false);
        }
        record["archive_hash"] = serde_json::Value::String(archive_hash.to_owned());
        let changed = conn
            .execute(
                "UPDATE app_metadata SET value = ?2 WHERE key = ?1 AND value = ?3",
                libsql::params![key, record.to_string(), old_value],
            )
            .await
            .map_err(error)?;
        Ok(changed == 1)
    }

    /// Whether a page ID has any durable deletion event, regardless of its
    /// member visibility. Used to refuse a stale sync resurrection.
    pub(super) async fn shared_page_was_deleted_on(
        conn: &libsql::Connection,
        id: &str,
    ) -> Result<bool, WenlanError> {
        let key = format!("{SHARED_PAGE_DELETION_PREFIX}{id}");
        let mut rows = conn
            .query("SELECT 1 FROM app_metadata WHERE key = ?1", [key.as_str()])
            .await
            .map_err(|e| WenlanError::VectorDb(format!("shared page deletion lookup: {e}")))?;
        Ok(rows
            .next()
            .await
            .map_err(|e| WenlanError::VectorDb(format!("shared page deletion lookup: {e}")))?
            .is_some())
    }

    pub async fn get_shared_page_scoped(
        &self,
        id: &str,
        scope: &ReadScope,
    ) -> Result<Option<Page>, WenlanError> {
        self.get_page_scoped_inner(id, scope, PageReader::Shared)
            .await
    }

    /// History text the Space could read. A saved export records the stable Space
    /// and page incarnation at the time that version was active or archived.
    /// A current page also proves its own body. Older snapshots without either
    /// witness may date from a draft or another Space.
    pub(crate) async fn list_shared_page_history(
        &self,
        id: &str,
        scope: &ReadScope,
        limit: i64,
    ) -> Result<Vec<super::PageHistoryEntry>, WenlanError> {
        let ReadScope::Space(space) = scope else {
            return Ok(Vec::new());
        };
        let conn = self.conn.lock().await;
        let shared = shared_page_predicate("p.");
        let error = |e| WenlanError::VectorDb(format!("shared page history: {e}"));
        let mut page = conn
            .query(
                &format!(
                    "SELECT p.incarnation, s.id, p.version, p.content, p.citations
                       FROM pages p JOIN spaces s ON s.name=p.workspace
                      WHERE p.id=?1 AND p.workspace=?2 AND {shared}"
                ),
                libsql::params![id, space.as_str()],
            )
            .await
            .map_err(error)?;
        let Some(row) = page.next().await.map_err(error)? else {
            return Ok(Vec::new());
        };
        let incarnation = row.get::<String>(0).map_err(error)?;
        let space_id = row.get::<String>(1).map_err(error)?;
        let version = row.get::<i64>(2).map_err(error)?;
        let content = row.get::<String>(3).map_err(error)?;
        let citations = row.get::<Option<String>>(4).map_err(error)?;
        drop(page);
        let prefix = super::page_export::bundle_prefix(id, &incarnation);
        let mut rows = conn
            .query(
                "WITH saved AS (
                SELECT json_extract(value, '$.version') AS version,
                       json_extract(value, '$.title') AS title,
                       json_extract(value, '$.content') AS content,
                       json_extract(value, '$.citations') AS citations
                FROM app_metadata WHERE key GLOB ?2 AND json_valid(value)
                  AND json_extract(value, '$.kind') = 'saved'
                  AND json_extract(value, '$.space_id') = ?3
             )
             SELECT h.version, h.title, h.content, h.edited_by, h.created_at,
                    CASE WHEN h.version=?4 THEN ?7 ELSE (
                        SELECT citations FROM saved WHERE version=h.version AND title=h.title AND content=h.content
                        ORDER BY citations IS NULL LIMIT 1
                    ) END
               FROM page_history h
              WHERE h.page_id=?1 AND h.version >= COALESCE((
                  SELECT MAX(m.version) FROM page_history m
                   WHERE m.page_id=?1 AND m.edited_by='space_move'
              ), 1)
              AND (EXISTS (SELECT 1 FROM saved WHERE version=h.version AND title=h.title AND content=h.content)
                   OR (h.version=?4 AND h.content=?5))
              ORDER BY h.version DESC LIMIT ?6",
                libsql::params![
                    id,
                    format!("{prefix}*"),
                    space_id,
                    version,
                    content,
                    limit,
                    citations
                ],
            )
            .await
            .map_err(error)?;
        let mut history = Vec::new();
        while let Some(row) = rows.next().await.map_err(error)? {
            let content = row.get::<String>(2).map_err(error)?;
            let citations = row
                .get::<Option<String>>(5)
                .map_err(error)?
                .and_then(|json| {
                    serde_json::from_str::<Vec<wenlan_types::pages::PageCitation>>(&json).ok()
                })
                .unwrap_or_default();
            history.push(super::PageHistoryEntry {
                version: row.get(0).map_err(error)?,
                title: row.get(1).map_err(error)?,
                content: crate::member_view::published_body(&content, &citations),
                source_memory_ids: Vec::new(),
                edited_by: row.get(3).map_err(error)?,
                created_at: row.get(4).map_err(error)?,
            });
        }
        Ok(history)
    }

    /// A shared basis reference can name the current row or a version this
    /// Space witnessed after the note most recently entered it.
    pub async fn member_can_read_page_version(
        &self,
        id: &str,
        scope: &ReadScope,
        version: i64,
    ) -> Result<bool, WenlanError> {
        if self
            .get_shared_page_scoped(id, scope)
            .await?
            .is_some_and(|page| page.version == version)
        {
            return Ok(true);
        }
        Ok(self
            .list_shared_page_history(id, scope, 200)
            .await?
            .iter()
            .any(|row| row.version == version))
    }

    async fn get_page_scoped_inner(
        &self,
        id: &str,
        scope: &ReadScope,
        reader: PageReader,
    ) -> Result<Option<Page>, WenlanError> {
        if matches!(scope, ReadScope::Global) && reader != PageReader::Shared {
            return if reader == PageReader::Internal {
                self.get_page(id).await
            } else {
                self.get_page_browse(id).await
            };
        }
        // #708: `entity_id` filled in from `entity_page_map` for the
        // `kind='entity'` shadow rows, whose own column is NULL -- the same
        // projection the list twins use, so a by-id read classifies the row
        // the way the listing that linked to it did.
        let entity_id_column = super::page_entity_id_column("c.");
        let select = format!(
            "c.id, c.title, c.summary, c.content, {entity_id_column}, c.space, \
             c.source_memory_ids, c.version, c.status, c.created_at, \
             c.last_compiled, c.last_modified, \
             COALESCE(c.sources_updated_count, 0), c.stale_reason, \
             COALESCE(c.user_edited, 0), COALESCE(c.changelog, '[]'), \
             COALESCE(c.creation_kind, 'distilled'), \
             COALESCE(c.review_status, 'confirmed'), c.workspace, c.citations, COALESCE(c.kind, 'concept'), \
             NULL, COALESCE(c.review_required, 0)"
        );
        // Column 21 (`refresh_blocked_reason`) stays NULL on the scoped browse
        // twins as before; column 22 carries the review lock so list_pages and
        // read_page can report it.
        let (scope_sql, scope_value) = page_scope_clause(scope, "c.workspace", 2);
        let fence_sql = match reader {
            PageReader::Internal => " AND COALESCE(c.kind, 'concept') != 'entity'".to_string(),
            PageReader::Browse => String::new(),
            PageReader::Shared => format!(" AND {}", shared_page_predicate("c.")),
        };
        let sql = format!("SELECT {select} FROM pages c WHERE c.id = ?1{fence_sql}{scope_sql}");
        let mut params = vec![libsql::Value::Text(id.to_string())];
        if let Some(value) = scope_value {
            params.push(value);
        }
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(&sql, params)
            .await
            .map_err(|error| WenlanError::VectorDb(format!("get_page_scoped: {error}")))?;
        match rows
            .next()
            .await
            .map_err(|error| WenlanError::VectorDb(error.to_string()))?
        {
            Some(row) => {
                let mut page = Self::row_to_page(&row)?;
                // Hosted readers don't disclose source evidence. Resolving
                // each locator here would still touch the host's memories.
                if reader != PageReader::Shared {
                    super::citation_provenance::apply_on(&conn, &mut page).await?;
                }
                Ok(Some(page))
            }
            None => Ok(None),
        }
    }

    /// G6 Stage 1.3: the joined side reads `edges` (`cites`) instead of
    /// `page_sources`, spanning all `dst_kind`s (no `dst_kind` filter) -- see
    /// the unscoped `get_page_sources` doc comment for the ruling. `linked_at`
    /// is COALESCE-total over the payload-mirrored legacy value and
    /// `edges.created_at` (ordering trap, same discipline as the unscoped
    /// reader).
    pub async fn get_page_sources_scoped(
        &self,
        page_id: &str,
        scope: &ReadScope,
    ) -> Result<Vec<wenlan_types::PageSource>, WenlanError> {
        let (scope_sql, scope_value) = page_scope_clause(scope, "c.workspace", 2);
        let sql = format!(
            "SELECT c.id, ps.src_id, ps.dst_id, \
                    COALESCE(json_extract(ps.payload,'$.linked_at'), ps.created_at), \
                    json_extract(ps.payload,'$.link_reason') \
             FROM pages c LEFT JOIN edges ps ON ps.src_id = c.id AND ps.edge_type = 'cites' \
                    AND ps.valid_until IS NULL \
             WHERE c.id = ?1{scope_sql} \
             ORDER BY COALESCE(json_extract(ps.payload,'$.linked_at'), ps.created_at) ASC, ps.dst_id ASC"
        );
        let mut params = vec![libsql::Value::Text(page_id.to_string())];
        if let Some(value) = scope_value {
            params.push(value);
        }
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(&sql, params)
            .await
            .map_err(|error| WenlanError::VectorDb(format!("get_page_sources_scoped: {error}")))?;
        let mut found = false;
        let mut sources = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|error| WenlanError::VectorDb(error.to_string()))?
        {
            found = true;
            let Some(source_page_id) = row.get::<Option<String>>(1).unwrap_or(None) else {
                continue;
            };
            sources.push(wenlan_types::PageSource {
                page_id: source_page_id,
                memory_source_id: row.get(2).unwrap_or_default(),
                linked_at: row.get(3).unwrap_or(0),
                link_reason: row.get::<Option<String>>(4).unwrap_or(None),
            });
        }
        if !found {
            return Err(page_not_found());
        }
        Ok(sources)
    }

    pub async fn get_page_outbound_links_scoped(
        &self,
        source_page_id: &str,
        scope: &ReadScope,
    ) -> Result<Vec<wenlan_types::responses::PageLinkOutbound>, WenlanError> {
        let (scope_sql, scope_value) = page_scope_clause(scope, "c.workspace", 2);
        let conn = self.conn.lock().await;
        let existence_sql = format!("SELECT c.id FROM pages c WHERE c.id = ?1{scope_sql}");
        let mut existence_params = vec![libsql::Value::Text(source_page_id.to_string())];
        if let Some(value) = scope_value.clone() {
            existence_params.push(value);
        }
        let mut rows = conn
            .query(&existence_sql, existence_params)
            .await
            .map_err(|error| {
                WenlanError::VectorDb(format!("get_page_outbound_links_scoped: {error}"))
            })?;
        let found = rows
            .next()
            .await
            .map_err(|error| WenlanError::VectorDb(error.to_string()))?
            .is_some();
        drop(rows);
        if !found {
            return Err(page_not_found());
        }

        // A target in another Space shows its title only when the label
        // named it there (`[[Space/Title]]`), and stays out of sight of a
        // reader held to this page's Space.
        let edge_sql = format!(
            "SELECT e.dst_id, json_extract(e.payload, '$.label'), target.title, \
                    CASE WHEN target.id IS NOT NULL THEN \
                      target.space IS c.space AND target.workspace IS c.workspace END, \
                    target.workspace, target.space \
             FROM edges e INNER JOIN pages c ON c.id = e.src_id \
             LEFT JOIN pages target ON target.id = e.dst_id \
               AND target.status = 'active' \
             WHERE c.id = ?1 AND e.edge_type = 'links' \
               AND e.src_kind = 'page' AND e.dst_kind = 'page' \
               AND e.valid_until IS NULL{scope_sql}"
        );
        let mut edge_params = vec![libsql::Value::Text(source_page_id.to_string())];
        if let Some(value) = scope_value {
            edge_params.push(value);
        }
        let mut rows = conn.query(&edge_sql, edge_params).await.map_err(|error| {
            WenlanError::VectorDb(format!("get_page_outbound_links_scoped: {error}"))
        })?;
        let mut links = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|error| WenlanError::VectorDb(error.to_string()))?
        {
            let label = row
                .get::<Option<String>>(1)
                .unwrap_or(None)
                .unwrap_or_default();
            let mut target_page_id = row.get::<String>(0).ok();
            let mut target_title = row.get::<Option<String>>(2).unwrap_or(None);
            let same_space = row.get::<Option<i64>>(3).unwrap_or(None);
            if same_space == Some(0) {
                let workspace = row.get::<Option<String>>(4).unwrap_or(None);
                let space_name = row.get::<Option<String>>(5).unwrap_or(None);
                if !scope.matches(workspace.as_deref()) {
                    target_page_id = None;
                    target_title = None;
                } else if !matches!(
                    (space_name.as_deref(), target_title.as_deref()),
                    (Some(space_name), Some(title))
                        if crate::synthesis::wikilinks::label_names_space_note(
                            &label, space_name, title,
                        )
                ) {
                    target_title = None;
                }
            }
            links.push((
                page_link_label_key(&label),
                wenlan_types::responses::PageLinkOutbound {
                    target_page_id,
                    label,
                    target_title,
                },
            ));
        }
        drop(rows);

        let mut rows = conn
            .query(
                "SELECT target_page_id, label FROM page_links \
                 WHERE source_page_id = ?1 AND target_page_id IS NULL",
                libsql::params![source_page_id],
            )
            .await
            .map_err(|error| {
                WenlanError::VectorDb(format!("get_page_outbound_links_scoped: {error}"))
            })?;
        while let Some(row) = rows
            .next()
            .await
            .map_err(|error| WenlanError::VectorDb(error.to_string()))?
        {
            let Some(label) = row.get::<Option<String>>(1).unwrap_or(None) else {
                continue;
            };
            links.push((
                page_link_label_key(&label),
                wenlan_types::responses::PageLinkOutbound {
                    target_page_id: row.get::<Option<String>>(0).unwrap_or(None),
                    label,
                    target_title: None,
                },
            ));
        }
        links.sort_by(|left, right| left.0.cmp(&right.0));
        Ok(links.into_iter().map(|(_, link)| link).collect())
    }

    pub async fn get_page_inbound_links_scoped(
        &self,
        target_page_id: &str,
        scope: &ReadScope,
    ) -> Result<Vec<(String, String)>, WenlanError> {
        let (source_scope_sql, source_scope_value) =
            page_scope_clause(scope, "source.workspace", 2);
        let (target_scope_sql, target_scope_value) =
            page_scope_clause(scope, "target.workspace", 3);
        let sql = format!(
            "SELECT target.id, source.id, json_extract(e.payload, '$.label') \
             FROM pages target \
             LEFT JOIN edges e ON e.dst_id = target.id \
               AND e.edge_type = 'links' AND e.src_kind = 'page' \
               AND e.dst_kind = 'page' AND e.valid_until IS NULL \
             LEFT JOIN pages source ON source.id = e.src_id \
               AND source.status = 'active'{source_scope_sql} \
             WHERE target.id = ?1{target_scope_sql} \
             ORDER BY source.last_modified DESC, source.id ASC"
        );
        let mut params = vec![libsql::Value::Text(target_page_id.to_string())];
        if let Some(value) = source_scope_value {
            params.push(value);
        }
        if let Some(value) = target_scope_value {
            params.push(value);
        }
        let conn = self.conn.lock().await;
        let mut rows = conn.query(&sql, params).await.map_err(|error| {
            WenlanError::VectorDb(format!("get_page_inbound_links_scoped: {error}"))
        })?;
        let mut found = false;
        let mut links = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|error| WenlanError::VectorDb(error.to_string()))?
        {
            found = true;
            let Some(source_id) = row.get::<Option<String>>(1).unwrap_or(None) else {
                continue;
            };
            links.push((source_id, row.get(2).unwrap_or_default()));
        }
        if !found {
            return Err(page_not_found());
        }
        Ok(links)
    }

    pub async fn get_page_changelog_scoped(
        &self,
        page_id: &str,
        scope: &ReadScope,
    ) -> Result<String, WenlanError> {
        let (scope_sql, scope_value) = page_scope_clause(scope, "c.workspace", 2);
        let sql =
            format!("SELECT COALESCE(c.changelog, '[]') FROM pages c WHERE c.id = ?1{scope_sql}");
        let mut params = vec![libsql::Value::Text(page_id.to_string())];
        if let Some(value) = scope_value {
            params.push(value);
        }
        let conn = self.conn.lock().await;
        let mut rows = conn.query(&sql, params).await.map_err(|error| {
            WenlanError::VectorDb(format!("get_page_changelog_scoped: {error}"))
        })?;
        match rows
            .next()
            .await
            .map_err(|error| WenlanError::VectorDb(error.to_string()))?
        {
            Some(row) => Ok(row.get(0).unwrap_or_else(|_| "[]".to_string())),
            None => Err(page_not_found()),
        }
    }

    pub async fn select_visible_pages_scoped(
        &self,
        raw_pages: Vec<Page>,
        scope: &ReadScope,
        memory_source_ids: &HashSet<String>,
        caller_trust: &str,
        cap: usize,
    ) -> Vec<Page> {
        let scoped = raw_pages
            .into_iter()
            .filter(|page| scope.matches(page.workspace.as_deref()))
            .collect();
        self.select_visible_pages(scoped, None, memory_source_ids, caller_trust, cap)
            .await
    }
}

impl MemoryDB {
    /// Active pages whose title, summary or text contains every word of
    /// `query`, title matches first, then newest. Search by meaning cannot
    /// find a Chinese or Japanese word inside a longer run of characters, and
    /// full-text search reads such a run as one word, so the search box adds
    /// these literal matches first (UX-FIND-1).
    pub async fn pages_containing_text(
        &self,
        query: &str,
        limit: usize,
        scope: &ReadScope,
    ) -> Result<Vec<Page>, WenlanError> {
        self.pages_containing_text_inner(query, limit, scope, true)
            .await
    }

    pub async fn shared_pages_containing_text(
        &self,
        query: &str,
        limit: usize,
        scope: &ReadScope,
    ) -> Result<Vec<Page>, WenlanError> {
        self.pages_containing_text_inner(query, limit, scope, false)
            .await
    }

    async fn pages_containing_text_inner(
        &self,
        query: &str,
        limit: usize,
        scope: &ReadScope,
        include_entities: bool,
    ) -> Result<Vec<Page>, WenlanError> {
        let words: Vec<String> = query
            .split_whitespace()
            .map(str::to_lowercase)
            .filter(|w| !w.is_empty())
            .take(8)
            .collect();
        if words.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let entity_id_column = super::page_entity_id_column("c.");
        let select = format!(
            "c.id, c.title, c.summary, c.content, {entity_id_column}, c.space, \
             c.source_memory_ids, c.version, c.status, c.created_at, \
             c.last_compiled, c.last_modified, \
             COALESCE(c.sources_updated_count, 0), c.stale_reason, \
             COALESCE(c.user_edited, 0), COALESCE(c.changelog, '[]'), \
             COALESCE(c.creation_kind, 'distilled'), \
             COALESCE(c.review_status, 'confirmed'), c.workspace, c.citations, COALESCE(c.kind, 'concept'), \
             NULL, COALESCE(c.review_required, 0)"
        );
        let entity_fence = if include_entities {
            super::entity_row_browse_predicate("c.")
        } else {
            shared_page_predicate("c.")
        };
        let mut conditions = vec!["c.status = 'active'".to_string(), entity_fence];
        let mut values = Vec::new();
        for word in &words {
            conditions.push(
                "(instr(lower(COALESCE(c.title, '')), ?) > 0 \
                  OR instr(lower(COALESCE(c.summary, '')), ?) > 0 \
                  OR instr(lower(COALESCE(c.content, '')), ?) > 0)"
                    .to_string(),
            );
            for _ in 0..3 {
                values.push(libsql::Value::Text(word.clone()));
            }
        }
        if !matches!(scope, ReadScope::Global) {
            super::push_read_scope_filter_folded(
                scope,
                "c.workspace",
                &mut conditions,
                &mut values,
            );
        }
        let where_clause = numbered_conditions(&conditions, 1);
        let next = values.len() + 1;
        values.push(libsql::Value::Text(words[0].clone()));
        values.push(libsql::Value::Integer(limit as i64));
        let sql = format!(
            "SELECT {select} FROM pages c WHERE {where_clause} \
             ORDER BY instr(lower(COALESCE(c.title, '')), ?{next}) > 0 DESC, c.last_modified DESC \
             LIMIT ?{}",
            next + 1
        );
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(&sql, values)
            .await
            .map_err(|e| WenlanError::VectorDb(format!("pages containing text: {e}")))?;
        let mut pages = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| WenlanError::VectorDb(format!("pages containing text row: {e}")))?
        {
            pages.push(Self::row_to_page(&row)?);
        }
        Ok(pages)
    }
}
