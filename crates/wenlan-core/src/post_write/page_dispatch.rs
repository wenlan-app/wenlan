use super::{
    page_create::{
        attach_page_sources_impl, create_page_impl, replace_source_page_impl,
        write_document_source_page_impl, CreatePageInput,
    },
    page_update::update_page_impl,
    WriteResult,
};
use crate::{
    db::{BasisWrite, MemoryDB},
    error::WenlanError,
};
use std::path::Path;
use wenlan_types::requests::{CreateConceptRequest, UpdatePageRequest};

pub enum PageWrite<'a> {
    Attach {
        page_id: &'a str,
        source_memory_ids: &'a [String],
        link_reason: &'a str,
        agent: &'a str,
    },
    Create {
        page_id: Option<&'a str>,
        req: CreateConceptRequest,
        agent: &'a str,
        knowledge_path: Option<&'a Path>,
        page_min_cluster_size: usize,
        page_match_threshold: f64,
        citations_json: Option<String>,
        /// R7: the notes the new page relies on, `(basis_page_id,
        /// read_version)`, recorded in the create transaction (a refused mark
        /// creates nothing). Ignored when the write attaches to an existing
        /// page instead of creating one.
        basis_marks: Vec<(String, i64)>,
        /// A Space-locked caller's Space: each basis must still be in it
        /// inside the create transaction. `None` for a Global caller.
        basis_space_guard: Option<crate::read_scope::ReadScope>,
    },
    Update {
        page_id: &'a str,
        req: UpdatePageRequest,
        edited_by: &'a str,
        require_stale: bool,
        expected_source_revision: Option<i64>,
        knowledge_path: Option<&'a Path>,
        citations: Option<(String, String)>,
    },
    /// An agent's `agent_refresh` of a Page it names (`agent`, from the
    /// `x-agent-name` header), under a source-revision fence. Identical to
    /// `Update` for `agent_refresh` except the changelog entry records `agent`.
    AgentUpdate {
        page_id: &'a str,
        req: UpdatePageRequest,
        agent: Option<&'a str>,
        expected_source_revision: i64,
        knowledge_path: Option<&'a Path>,
        /// The notes this write relied on (R7) and the `minor` flag. Applied
        /// in the write's own transaction, only if the write lands.
        basis: BasisWrite,
    },
    /// Explicit user-forced rebuild of a Page body (Re-distill). Like `Update`
    /// under a source-revision fence with no stale requirement, except a
    /// `user_edited` Page is rewritten rather than staged, and that flag and
    /// the Page's staleness are released by the same UPDATE that lands the
    /// body. Authored Pages still stage a revision card. `expected_fence` is
    /// the page fence captured before the page and its sources were read;
    /// every terminal action (body write, card, acknowledgement) matches its
    /// source revision AND incarnation, so a page deleted and recreated under
    /// the same id during generation is never touched.
    UserForcedUpdate {
        page_id: &'a str,
        req: UpdatePageRequest,
        edited_by: &'a str,
        expected_fence: &'a crate::db::PageFence,
        knowledge_path: Option<&'a Path>,
        citations: Option<(String, String)>,
    },
    /// Human content edit that preserves the source set from the exact Page
    /// generation selected inside the update CAS. HTTP callers do not own the
    /// source list, so it must not be snapshotted outside the gate.
    UpdatePreservingSources {
        page_id: &'a str,
        req: UpdatePageRequest,
        edited_by: &'a str,
        knowledge_path: Option<&'a Path>,
        /// `minor`, and a `confirm` (R11) that lands only in this save's
        /// transaction.
        basis: BasisWrite,
    },
    ReplaceSource {
        page_id: &'a str,
        title: &'a str,
        summary: Option<&'a str>,
        content: &'a str,
        source_memory_ids: &'a [String],
        agent: &'a str,
    },
    DocumentSource {
        page_id: &'a str,
        title: &'a str,
        summary: Option<&'a str>,
        content: &'a str,
        source_memory_ids: &'a [String],
        queue_source_id: &'a str,
        file_path: &'a str,
        expected_content_hash: Option<&'a str>,
        expected_page_version: Option<i64>,
        /// Space for a newly created page; an existing page keeps its Space.
        space: Option<&'a str>,
        agent: &'a str,
    },
}

#[derive(Clone, Copy)]
pub(super) struct PageGrowthCommit<'a> {
    pub(super) source_id: &'a str,
    pub(super) expected_memory_version: i64,
    pub(super) expected_page_version: i64,
    pub(super) expected_source_revision: i64,
}

pub async fn page_write(db: &MemoryDB, write: PageWrite<'_>) -> Result<WriteResult, WenlanError> {
    match write {
        PageWrite::Attach {
            page_id,
            source_memory_ids,
            link_reason,
            agent,
        } => attach_page_sources_impl(db, page_id, source_memory_ids, link_reason, agent).await,
        PageWrite::Create {
            page_id,
            req,
            agent,
            knowledge_path,
            page_min_cluster_size,
            page_match_threshold,
            citations_json,
            basis_marks,
            basis_space_guard,
        } => {
            create_page_impl(
                db,
                CreatePageInput {
                    page_id,
                    req,
                    agent,
                    knowledge_path,
                    page_min_cluster_size,
                    page_match_threshold,
                    citations_json: citations_json.as_deref(),
                    basis_marks: &basis_marks,
                    basis_space_guard: basis_space_guard.as_ref(),
                },
            )
            .await
        }
        PageWrite::Update {
            page_id,
            req,
            edited_by,
            require_stale,
            expected_source_revision,
            knowledge_path,
            citations,
        } => {
            update_page_impl(
                db,
                page_id,
                req,
                edited_by,
                require_stale,
                expected_source_revision,
                knowledge_path,
                citations,
                None,
                false,
                false,
                None,
                None,
                &BasisWrite::default(),
            )
            .await
        }
        PageWrite::AgentUpdate {
            page_id,
            req,
            agent,
            expected_source_revision,
            knowledge_path,
            basis,
        } => {
            update_page_impl(
                db,
                page_id,
                req,
                "agent_refresh",
                false,
                Some(expected_source_revision),
                knowledge_path,
                None,
                None,
                false,
                false,
                None,
                agent,
                &basis,
            )
            .await
        }
        PageWrite::UserForcedUpdate {
            page_id,
            req,
            edited_by,
            expected_fence,
            knowledge_path,
            citations,
        } => {
            update_page_impl(
                db,
                page_id,
                req,
                edited_by,
                false,
                Some(expected_fence.source_revision),
                knowledge_path,
                citations,
                None,
                false,
                true,
                Some(expected_fence),
                None,
                &BasisWrite::default(),
            )
            .await
        }
        PageWrite::UpdatePreservingSources {
            page_id,
            req,
            edited_by,
            knowledge_path,
            basis,
        } => {
            update_page_impl(
                db,
                page_id,
                req,
                edited_by,
                false,
                None,
                knowledge_path,
                None,
                None,
                true,
                false,
                None,
                None,
                &basis,
            )
            .await
        }
        PageWrite::ReplaceSource {
            page_id,
            title,
            summary,
            content,
            source_memory_ids,
            agent,
        } => {
            replace_source_page_impl(
                db,
                page_id,
                title,
                summary,
                content,
                source_memory_ids,
                agent,
            )
            .await
        }
        PageWrite::DocumentSource {
            page_id,
            title,
            summary,
            content,
            source_memory_ids,
            queue_source_id,
            file_path,
            expected_content_hash,
            expected_page_version,
            space,
            agent,
        } => {
            write_document_source_page_impl(
                db,
                page_id,
                title,
                summary,
                content,
                source_memory_ids,
                queue_source_id,
                file_path,
                expected_content_hash,
                expected_page_version,
                space,
                agent,
            )
            .await
        }
    }
}

/// Create a distilled wiki page. Canonical entry for both agent-triggered
/// (`/api/pages`) and daemon-internal distillation callers.
pub async fn create_page(
    db: &MemoryDB,
    req: CreateConceptRequest,
    agent: &str,
    knowledge_path: Option<&Path>,
) -> Result<WriteResult, WenlanError> {
    let distillation = crate::tuning::DistillationConfig::default();
    create_page_with_tuning(
        db,
        req,
        agent,
        knowledge_path,
        distillation.page_min_cluster_size,
        distillation.page_match_threshold,
    )
    .await
}

pub async fn create_page_with_floor(
    db: &MemoryDB,
    req: CreateConceptRequest,
    agent: &str,
    knowledge_path: Option<&Path>,
    page_min_cluster_size: usize,
) -> Result<WriteResult, WenlanError> {
    create_page_with_tuning(
        db,
        req,
        agent,
        knowledge_path,
        page_min_cluster_size,
        crate::tuning::DistillationConfig::default().page_match_threshold,
    )
    .await
}

pub async fn create_page_with_tuning(
    db: &MemoryDB,
    req: CreateConceptRequest,
    agent: &str,
    knowledge_path: Option<&Path>,
    page_min_cluster_size: usize,
    page_match_threshold: f64,
) -> Result<WriteResult, WenlanError> {
    page_write(
        db,
        PageWrite::Create {
            page_id: None,
            req,
            agent,
            knowledge_path,
            page_min_cluster_size,
            page_match_threshold,
            citations_json: None,
            basis_marks: Vec::new(),
            basis_space_guard: None,
        },
    )
    .await
}

/// Update a distilled wiki page. Canonical entry for all page-update paths:
/// daemon-internal distillation, refinery re-distill, fs watcher, and
/// future agent-HTTP routes.
///
/// Two write modes via `require_stale`:
/// - `false` — unconditional write (post-ingest, distill, page_growth callers)
/// - `true`  — CAS: only writes when `stale_reason IS NOT NULL` (refinery callers).
///   Returns `Ok(WriteResult { warnings: vec![] })` without writing when not stale.
///
/// Hallucination guard runs for human writers (`manual_edit`, `fs_edit`) and
/// for any writer the gate does not recognize. `agent_refresh` and the LLM
/// rewrite stages (`distill`, `re_distill`, `page_growth`, `refinery_merge`)
/// skip it — incremental updates may push aggregate cosine sim below 0.6 and
/// would silently drop legitimate writes. See `Writer::skips_hallucination_guard`,
/// which is the single source of truth; `WRITER_TABLE` pins it per writer.
///
/// `citations`: `Some((citations_json, stats_summary))` when the caller has
/// freshly verified [N] markers against a numbered source list for this
/// exact `req.content` — persisted atomically with the content, and
/// `stats_summary` is recorded on the changelog entry. `None` always resets
/// `citations` to SQL `NULL` (a stale marker-to-source map must not survive a
/// content change without fresh verification, and the new body must remain
/// eligible for bounded annotation).
#[allow(clippy::too_many_arguments)]
pub async fn update_page(
    db: &MemoryDB,
    page_id: &str,
    req: UpdatePageRequest,
    edited_by: &str,
    require_stale: bool,
    knowledge_path: Option<&Path>,
    citations: Option<(String, String)>,
) -> Result<WriteResult, WenlanError> {
    page_write(
        db,
        PageWrite::Update {
            page_id,
            req,
            edited_by,
            require_stale,
            expected_source_revision: None,
            knowledge_path,
            citations,
        },
    )
    .await
}

/// Update only the Page body while preserving the sources from the exact
/// generation loaded inside PageWrite's CAS. This is the manual-editor seam:
/// the HTTP request does not own `source_memory_ids`, so a source attached
/// between request parsing and the write gate must survive.
pub async fn update_page_preserving_sources(
    db: &MemoryDB,
    page_id: &str,
    req: UpdatePageRequest,
    edited_by: &str,
    knowledge_path: Option<&Path>,
) -> Result<WriteResult, WenlanError> {
    page_write(
        db,
        PageWrite::UpdatePreservingSources {
            page_id,
            req,
            edited_by,
            knowledge_path,
            basis: BasisWrite::default(),
        },
    )
    .await
}

/// Update a Page under an explicit source-revision fence. Unlike the generic
/// `update_page` (which never fences on the source counter at all), every
/// caller here supplies a real `expected_source_revision`. `pub`, not
/// `pub(crate)`: wenlan-server's agent-refresh route writes through this
/// wrapper too, not just wenlan-core's own page-growth callers.
#[allow(clippy::too_many_arguments)]
pub async fn update_page_at_source_revision(
    db: &MemoryDB,
    page_id: &str,
    req: UpdatePageRequest,
    edited_by: &str,
    require_stale: bool,
    expected_source_revision: i64,
    knowledge_path: Option<&Path>,
    citations: Option<(String, String)>,
) -> Result<WriteResult, WenlanError> {
    page_write(
        db,
        PageWrite::Update {
            page_id,
            req,
            edited_by,
            require_stale,
            expected_source_revision: Some(expected_source_revision),
            knowledge_path,
            citations,
        },
    )
    .await
}

/// Page-growth-only update path. Unlike the generic update helper, the CAS
/// token comes from the pre-inference match and is not refreshed after the
/// model returns. The memory receipt shares the DB transaction with the Page
/// write, so neither can claim a stale inference landed.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn update_page_growth_at_versions(
    db: &MemoryDB,
    page_id: &str,
    req: UpdatePageRequest,
    expected_page_version: i64,
    expected_source_revision: i64,
    source_id: &str,
    expected_memory_version: i64,
    knowledge_path: Option<&Path>,
    citations: Option<(String, String)>,
) -> Result<WriteResult, WenlanError> {
    update_page_impl(
        db,
        page_id,
        req,
        "page_growth",
        false,
        None,
        knowledge_path,
        citations,
        Some(PageGrowthCommit {
            source_id,
            expected_memory_version,
            expected_page_version,
            expected_source_revision,
        }),
        false,
        false,
        None,
        None,
        &BasisWrite::default(),
    )
    .await
}
