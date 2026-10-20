// SPDX-License-Identifier: Apache-2.0
use crate::error::ServerError;
use crate::memory_routes::extract_agent_name;
use crate::route_registry::{get, post, put, TrackedRouter};
use crate::state::{ServerState, SharedState};
use crate::telemetry::TelemetryEvent;
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::Json,
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use wenlan_types::requests::{
    CreateConceptRequest, CreatePageDraftRequest, ExportFormat, ExportPagesRequest,
    MovePageRequest, PageDraftVersionRequest, SearchPagesRequest, UpdatePageDraftRequest,
};
use wenlan_types::responses::{
    CreatePageResponse, MovePageResponse, OkfConceptRef, PageDraftResponse, PageInventoryEntry,
    PageInventoryResponse, PageProjectionStatus, PageReadResponse,
};
use wenlan_types::{WriteOutcome, WriteSpaceSource, WriteSpaceTarget};

pub(crate) fn register(router: TrackedRouter<SharedState>) -> TrackedRouter<SharedState> {
    router
        .route(
            "/api/pages",
            get(handle_list_pages).post(handle_create_page),
        )
        .route("/api/pages/search", post(handle_search_pages))
        .route("/api/pages/drafts", post(handle_create_page_draft))
        .route(
            "/api/pages/drafts/{id}",
            put(handle_update_page_draft).delete(handle_discard_page_draft),
        )
        .route(
            "/api/pages/drafts/{id}/publish",
            post(handle_publish_page_draft),
        )
        .route("/api/pages/export", post(handle_export_pages))
        .route(
            "/api/pages/recent-changes",
            get(crate::routes::handle_recent_page_changes),
        )
        .route("/api/pages/orphan-links", get(handle_list_orphan_links))
        .route("/api/pages/{id}/export", post(handle_export_page))
        .route(
            "/api/pages/{id}",
            get(handle_get_page)
                .put(handle_refresh_page)
                .delete(handle_delete_page),
        )
        .route("/api/pages/{id}/sources", get(handle_get_page_sources))
        .route("/api/pages/{id}/links", get(handle_get_page_links))
        .route("/api/pages/{id}/move", post(handle_move_page))
        .route("/api/pages/{id}/archive", post(handle_archive_page))
        .route("/api/pages/{id}/rename", post(handle_rename_page))
        .route("/api/pages/{id}/revisions", get(handle_get_page_revisions))
        .route("/api/pages/{id}/review", post(handle_review_page))
        .route(
            "/api/pages/{id}/review-required",
            put(handle_set_page_review_required),
        )
}

pub(crate) fn register_manual_edit(
    router: TrackedRouter<SharedState>,
) -> TrackedRouter<SharedState> {
    router.route("/api/memory/{id}/update-page", post(handle_update_page))
}

/// One `GET /api/pages` window, with `limit` and `offset` counted in pages this
/// caller may see.
///
/// The truth adapter can hide a page, and it can only run after SQL has
/// picked the rows. Applying `limit` and `offset` to raw rows made any window
/// holding a hidden page come back short, and every pager of this route (the
/// app's `listAllPages`, MCP `list_pages`) reads a short window as the end of
/// the list, so every page after it silently disappeared. Here the scan
/// starts at the first row, skips `offset` visible pages, and keeps reading
/// until it has `limit` of them or the rows run out. Rescanning from the top
/// costs O(offset) rows per call, which is cheap at library sizes and keeps
/// the wire unchanged for every client.
async fn list_visible_browse_pages(
    db: &wenlan_core::db::MemoryDB,
    grant: &wenlan_core::truth_contract::TruthGrant,
    status: &str,
    limit: usize,
    offset: usize,
    scope: &wenlan_core::read_scope::ReadScope,
) -> Result<Vec<wenlan_core::pages::Page>, wenlan_core::WenlanError> {
    const MIN_SCAN_BATCH: usize = 100;
    let batch = limit.max(MIN_SCAN_BATCH);
    let mut skipped = 0;
    let mut row_offset = 0;
    let mut window = Vec::with_capacity(limit.min(batch));
    while window.len() < limit {
        // Q1 browse surface: `kind='entity'` dual-write shadow pages appear
        // here, so read the unfenced `_browse` twin (export + internal callers
        // keep the fenced `list_pages_scoped`).
        let rows = db
            .list_pages_scoped_browse(status, batch as i64, row_offset as i64, scope)
            .await?;
        let read = rows.len();
        row_offset += read;
        for page in wenlan_core::truth_adapter::filter_pages(db, grant, rows).await? {
            if skipped < offset {
                skipped += 1;
            } else if window.len() < limit {
                window.push(page);
            }
        }
        if read < batch {
            break;
        }
    }
    Ok(window)
}

/// GET /api/pages
pub async fn handle_list_pages(
    State(state): State<Arc<RwLock<ServerState>>>,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    view: crate::truth_guard::TruthView,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<PageInventoryResponse>, ServerError> {
    let status = params.get("status").map(|s| s.as_str()).unwrap_or("active");
    let space = params
        .get("space")
        .or_else(|| params.get("domain"))
        .cloned();
    let limit: usize = params
        .get("limit")
        .and_then(|l| l.parse().ok())
        .unwrap_or(50);
    let offset: usize = params
        .get("offset")
        .and_then(|o| o.parse().ok())
        .unwrap_or(0);

    let (db, page_root) = {
        let s = state.read().await;
        (
            s.db.clone().ok_or(ServerError::DbNotInitialized)?,
            s.lint_config.page_root().map(std::path::Path::to_path_buf),
        )
    };
    let scope =
        crate::read_scope::effective_read_scope(&db, space.as_deref(), header_space.as_deref())
            .await?;
    let pages = list_visible_browse_pages(&db, &view.grant, status, limit, offset, &scope)
        .await
        .map_err(|e| ServerError::SearchFailed(e.to_string()))?;
    // Enrich only the final scoped, truth-filtered browse result. Entity shadow
    // pages and drafts are browse records, not Markdown projection claims.
    let page_ids: Vec<String> = pages
        .iter()
        .filter(|page| page.kind != "entity" && page.status != "draft")
        .map(|page| page.id.clone())
        .collect();
    // A note whose basis needs a look carries those rows; the quiet majority
    // carry none, so the list stays small.
    let mut basis_by_page =
        crate::page_basis_routes::non_ok_basis_by_citer(&db, &view.grant, &scope, &page_ids)
            .await?;
    let mut code_basis_by_page =
        crate::code_basis_routes::non_ok_code_basis_by_page(&db, &page_ids).await?;
    let mut folders = HashMap::new();
    for page in pages.iter().filter(|page| page.status == "draft") {
        folders.insert(
            page.id.clone(),
            db.page_initial_folder_path(&page.id).await?,
        );
    }
    let filenames = if let Some(root) = page_root {
        tokio::task::spawn_blocking(move || {
            let ids: Vec<&str> = page_ids.iter().map(String::as_str).collect();
            wenlan_core::export::knowledge::KnowledgeWriter::new(root, &db)
                .live_page_filenames(&ids)
        })
        .await
        .unwrap_or_default()
    } else {
        HashMap::new()
    };
    Ok(Json(PageInventoryResponse {
        pages: pages
            .into_iter()
            .map(|page| PageInventoryEntry {
                basis: basis_by_page.remove(&page.id).unwrap_or_default(),
                code_basis: code_basis_by_page.remove(&page.id).unwrap_or_default(),
                storage_path: filenames.get(&page.id).cloned(),
                folder_path: folders.get(&page.id).cloned(),
                page,
            })
            .collect(),
    }))
}

/// GET /api/pages/:id
pub async fn handle_get_page(
    State(state): State<Arc<RwLock<ServerState>>>,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    view: crate::truth_guard::TruthView,
    Path(id): Path<String>,
) -> Result<Json<PageReadResponse>, ServerError> {
    let (db, page_root) = {
        let s = state.read().await;
        (
            s.db.clone().ok_or(ServerError::DbNotInitialized)?,
            s.lint_config.page_root().map(std::path::Path::to_path_buf),
        )
    };
    let scope = crate::read_scope::effective_read_scope(&db, None, header_space.as_deref()).await?;
    // Q1 browse surface: a shadow id resolves 200 here, so read the unfenced
    // `_browse` twin (mutation/export by-id reads keep fenced `get_page_scoped`).
    let page = db
        .get_page_scoped_browse(&id, &scope)
        .await
        .map_err(|e| ServerError::SearchFailed(e.to_string()))?;
    match wenlan_core::truth_adapter::filter_page(&db, &view.grant, page)
        .await
        .map_err(|e| ServerError::SearchFailed(e.to_string()))?
    {
        Some(page) => {
            // An imported OKF concept carries its bundle frontmatter beside the
            // page, for display only. The key is omitted for every other page,
            // so a reader that does not know about OKF sees today's shape.
            // kind is intentionally skipped on the public Page wire; inspect the
            // authorized internal record before serialization.
            let is_draft = page.status == "draft";
            let has_live_file = wenlan_core::pages::is_active_file_page(&page);
            let mut folder_path = None;
            let mut storage_path = None;
            if is_draft {
                folder_path = Some(db.page_initial_folder_path(&id).await?);
            } else if has_live_file {
                // Resolve only after scope and truth authorization. A path is verified
                // physical metadata, never a guess from title or initial folder intent.
                if let Some(root) = page_root {
                    let db = db.clone();
                    let page_id = id.clone();
                    storage_path = tokio::task::spawn_blocking(move || {
                        wenlan_core::export::knowledge::KnowledgeWriter::new(root, &db)
                            .live_page_filenames(&[page_id.as_str()])
                            .remove(&page_id)
                    })
                    .await
                    .unwrap_or_default();
                }
            }
            let okf = match db.get_okf_concept(&id).await {
                Ok(Some(record)) => Some(OkfConceptRef {
                    source_id: record.source_id,
                    concept_id: record.concept_id,
                    frontmatter: record.frontmatter,
                    updated_at: record.updated_at,
                }),
                _ => None,
            };
            // Every basis of this note, in any state, plus (agents only, but
            // computed for every caller) the notes it links to that changed
            // after it did.
            let basis = crate::page_basis_routes::basis_refs(
                &db,
                &view.grant,
                &scope,
                db.list_page_basis(&id).await?,
            )
            .await?;
            let linked_ids: Vec<(String, String, i64)> = db.linked_changed(&id).await?;
            let linked_visible = crate::page_basis_routes::visible_ids(
                &db,
                &view.grant,
                &scope,
                &linked_ids
                    .iter()
                    .map(|(page_id, _, _)| page_id.clone())
                    .collect::<Vec<_>>(),
            )
            .await?;
            let linked_changed = linked_ids
                .into_iter()
                .filter(|(page_id, _, _)| linked_visible.contains(page_id))
                .map(|(page_id, title, version)| wenlan_types::LinkedChanged {
                    page_id,
                    title,
                    version,
                })
                .collect();
            Ok(Json(PageReadResponse {
                code_basis: db.list_page_code_basis(&id).await?,
                basis,
                linked_changed,
                // The full list is the top-level `basis`; the entry's own
                // non-ok list is for inventories.
                page: PageInventoryEntry {
                    basis: Vec::new(),
                    code_basis: Vec::new(),
                    page,
                    storage_path,
                    folder_path,
                },
                okf,
            }))
        }
        // A hidden page 404s rather than 200-ing empty: for this caller it is
        // not there, and the existing not-found answer is the honest one.
        None => Err(ServerError::NotFound("page not found".to_string())),
    }
}

/// GET /api/pages/{id}/sources
///
/// Returns all source memories linked to a concept via the concept_sources join table,
/// enriched with memory metadata for display.
pub async fn handle_get_page_sources(
    State(state): State<Arc<RwLock<ServerState>>>,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    view: crate::truth_guard::TruthView,
    Path(id): Path<String>,
) -> Result<Json<Vec<wenlan_types::PageSourceWithMemory>>, ServerError> {
    let db = {
        let s = state.read().await;
        s.db.clone().ok_or(ServerError::DbNotInitialized)?
    };

    let scope = crate::read_scope::effective_read_scope(&db, None, header_space.as_deref()).await?;
    let sources = db.get_page_sources_scoped(&id, &scope).await?;
    // The provenance trail hangs off the page rather than being it, so it has
    // no entry form: `reduce_to_entry` strips `source_memory_ids` precisely
    // because handing them out turns a listing into a retrieval plan, and this
    // route hands out the memories themselves on top. Full or nothing.
    let sources =
        wenlan_core::truth_adapter::filter_page_refs(&db, &view.grant, sources, |source| {
            source.page_id.as_str()
        })
        .await?;

    let source_id_strings: Vec<String> =
        sources.iter().map(|s| s.memory_source_id.clone()).collect();
    let memories = match &scope {
        wenlan_core::read_scope::ReadScope::Global => {
            db.get_memories_by_source_ids(&source_id_strings).await
        }
        _ => {
            db.get_memories_by_source_ids_scoped(&source_id_strings, &scope)
                .await
        }
    }
    .map_err(|e| ServerError::SearchFailed(e.to_string()))?;

    let result: Vec<wenlan_types::PageSourceWithMemory> = sources
        .iter()
        .map(|s| {
            let memory = memories
                .iter()
                .find(|m| m.source_id == s.memory_source_id)
                .cloned();
            wenlan_types::PageSourceWithMemory {
                source: s.clone(),
                memory,
            }
        })
        .collect();

    Ok(Json(result))
}

/// Refuse a page mutation that targets an entity's page, naming the entity and
/// carrying its id as a field (#708).
///
/// The core guard produces the same sentence, but its only channel is an error
/// string; the desktop app needs the id to offer "open in Entities", so the
/// HTTP surface sends it separately. Returns `Ok(())` for every page that is
/// not an entity's, including ids that do not exist -- archive and delete keep
/// their no-op-on-missing behavior.
async fn reject_entity_page_mutation(
    db: &std::sync::Arc<wenlan_core::db::MemoryDB>,
    page_id: &str,
) -> Result<(), ServerError> {
    let Some(owner) = db.entity_shadow_page_owner(page_id).await? else {
        return Ok(());
    };
    let mut body = serde_json::json!({
        "error": wenlan_core::db::entity_shadow_page_guard_message(&owner.entity_name),
        "entity_name": owner.entity_name,
    });
    if let Some(entity_id) = owner.entity_id {
        body["entity_id"] = serde_json::Value::String(entity_id);
    }
    Err(ServerError::Structured {
        status: axum::http::StatusCode::UNPROCESSABLE_ENTITY,
        body,
    })
}

/// PUT /api/pages/{id}/review-required
///
/// Set or clear a page's review lock. While it is set, every machine write
/// (an agent refresh with or without `expected_version`, and every pipeline
/// stage) stages a revision card instead of landing; human edits are never
/// gated by it. The flag is metadata: it does not bump the page version.
pub async fn handle_set_page_review_required(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(id): Path<String>,
    Json(req): Json<wenlan_types::requests::SetPageReviewRequiredRequest>,
) -> Result<Json<wenlan_types::responses::PageReviewRequiredResponse>, ServerError> {
    let db = {
        let s = state.read().await;
        s.db.clone().ok_or(ServerError::DbNotInitialized)?
    };
    reject_entity_page_mutation(&db, &id).await?;
    if !db.set_page_review_required(&id, req.required).await? {
        return Err(ServerError::NotFound(format!("page {id} not found")));
    }
    Ok(Json(wenlan_types::responses::PageReviewRequiredResponse {
        page_id: id,
        review_required: req.required,
    }))
}

/// POST /api/pages/{id}/archive
pub async fn handle_archive_page(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ServerError> {
    let db = {
        let s = state.read().await;
        s.db.clone().ok_or(ServerError::DbNotInitialized)?
    };
    // Checked here, not left to the core fence, so the refusal can carry the
    // entity id as a field. The core fence still stands behind it for every
    // caller that does not come through this route.
    reject_entity_page_mutation(&db, &id).await?;
    db.archive_page(&id).await.map_err(ServerError::from)?;
    Ok(Json(serde_json::json!({"status": "archived"})))
}

/// DELETE /api/pages/{id}
///
/// Removes both the DB row and the projected `<pages dir>/<slug>.md`
/// file. DB-first so a transient md removal failure leaves a stale file
/// (cheap to clean up) rather than a stranded DB row (invisible to the
/// user). The md side failing is logged but not surfaced as an error —
/// the caller's intent (delete the page) succeeded as far as queries
/// are concerned.
pub async fn handle_delete_page(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ServerError> {
    let db = {
        let s = state.read().await;
        s.db.as_ref()
            .cloned()
            .ok_or(ServerError::DbNotInitialized)?
    };

    let knowledge_path = wenlan_core::config::load_config().knowledge_path_or_default();
    let projection =
        wenlan_core::export::knowledge::KnowledgeProjectionWrite::new(knowledge_path, &db);
    // Same as archive: refuse an entity's page here so the response can name
    // the entity and carry its id.
    reject_entity_page_mutation(&db, &id).await?;
    db.delete_page(&id).await.map_err(ServerError::from)?;

    if let Err(e) = projection.remove_page(&id) {
        tracing::warn!(
            "[page] DB row deleted but md projection cleanup failed for {}: {}",
            id,
            e
        );
    }

    Ok(Json(serde_json::json!({"status": "deleted"})))
}

/// POST /api/pages/search
pub async fn handle_search_pages(
    State(state): State<Arc<RwLock<ServerState>>>,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    view: crate::truth_guard::TruthView,
    Json(req): Json<SearchPagesRequest>,
) -> Result<Json<serde_json::Value>, ServerError> {
    let db = {
        let s = state.read().await;
        s.db.clone().ok_or(ServerError::DbNotInitialized)?
    };
    let scope =
        crate::read_scope::effective_read_scope(&db, req.space.as_deref(), header_space.as_deref())
            .await?;
    let results = db
        .search_pages_scoped_browse(
            &req.query,
            req.limit.unwrap_or(20),
            req.page_type.as_deref(),
            &scope,
        )
        .await
        .map_err(|e| ServerError::SearchFailed(e.to_string()))?;
    let results = wenlan_core::truth_adapter::filter_pages(&db, &view.grant, results)
        .await
        .map_err(|e| ServerError::SearchFailed(e.to_string()))?;
    Ok(Json(serde_json::json!({ "pages": results })))
}

/// POST /api/pages
///
/// Atomic md-first + DB-index. The `<pages dir>/<slug>.md` file (default
/// `~/.wenlan/pages/`, slug made unique on collision) is the
/// human-readable canonical form; the DB row is the hybrid index over it.
/// If the DB insert fails after the md write succeeds, the md file is
/// removed so the two stores stay consistent.
pub async fn handle_create_page(
    State(state): State<Arc<RwLock<ServerState>>>,
    headers: HeaderMap,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    Json(wire): Json<wenlan_types::requests::CreatePageWithBasisRequest>,
) -> Result<Json<CreatePageResponse>, ServerError> {
    let wenlan_types::requests::CreatePageWithBasisRequest {
        create: req,
        basis,
        code_basis,
    } = wire;
    // An omitted creation_kind is resolved by the core writer from the
    // distinct source count against the distillation floor. Classify
    // telemetry with the same resolver so MCP's compact write_page payload is
    // counted as a wiki generation exactly when it lands as distilled.
    let (telemetry, page_min_cluster_size) = {
        let s = state.read().await;
        (
            s.telemetry.clone(),
            s.tuning.distillation.page_min_cluster_size,
        )
    };
    let is_distilled = wenlan_core::post_write::resolve_page_creation_kind(
        req.creation_kind.as_deref(),
        &req.source_memory_ids,
        page_min_cluster_size,
    ) == "distilled";
    let result = handle_create_page_inner(
        State(state),
        headers,
        crate::space_header::SpaceHeader(header_space),
        Json(req),
        basis,
        code_basis,
    )
    .await;
    if is_distilled {
        match &result {
            Ok(Json(response)) if response.attached_to.is_none() => {
                telemetry.record(TelemetryEvent::WikiGenerated)
            }
            Ok(_) => {}
            Err(_) => telemetry.record(TelemetryEvent::WikiError),
        }
    }
    result
}

async fn handle_create_page_inner(
    State(state): State<Arc<RwLock<ServerState>>>,
    headers: HeaderMap,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    Json(mut req): Json<CreateConceptRequest>,
    basis: Vec<wenlan_types::BasisInput>,
    code_basis: Vec<wenlan_types::CodeBasisInput>,
) -> Result<Json<CreatePageResponse>, ServerError> {
    let agent = extract_agent_name(&headers, None);
    let (db, page_root, page_min_cluster_size, page_match_threshold) = {
        let s = state.read().await;
        (
            s.db.as_ref()
                .cloned()
                .ok_or(ServerError::DbNotInitialized)?,
            s.lint_config.page_root().map(std::path::Path::to_path_buf),
            s.tuning.distillation.page_min_cluster_size,
            s.tuning.distillation.page_match_threshold,
        )
    };

    let _space_write_guard = db.lock_space_writes().await;
    let target = normalize_page_write_target(&req.space, req.workspace.as_deref())?;
    let resolved = db
        .resolve_write_space(&target, header_space.as_deref())
        .await?;
    req.space = match resolved.space_name.as_ref() {
        Some(name) => WriteSpaceTarget::Named(name.clone()),
        None => WriteSpaceTarget::Uncategorized,
    };
    req.workspace = resolved.space_name.clone();
    // Refuse a bad basis before anything is created, so the agent is told
    // instead of getting a note without the basis it said it relied on.
    crate::page_basis_routes::refuse_note_ids_in_sources(&db, &req.source_memory_ids).await?;
    let marks: Vec<(String, i64)> = basis
        .iter()
        .map(|b| (b.page_id.clone(), b.version))
        .collect();
    if !marks.is_empty() {
        let scope =
            crate::read_scope::effective_read_scope(&db, None, header_space.as_deref()).await?;
        crate::page_basis_routes::require_basis_visible(&db, &scope, &marks).await?;
    }
    db.check_basis_candidates(resolved.space_name.as_deref(), &marks)
        .await
        .map_err(crate::page_basis_routes::basis_error)?;
    // Lines of code the note relies on are read and fingerprinted before
    // anything is created, so a path out of the repository or a file that cannot
    // be read refuses the whole request instead of landing as a row that was
    // never true.
    let prepared_code_basis = wenlan_core::code_basis::prepare_code_basis(&code_basis)
        .await
        .map_err(crate::code_basis_routes::code_basis_error)?;
    let result = wenlan_core::post_write::create_page_with_tuning(
        &db,
        req,
        &agent,
        page_root.as_deref(),
        page_min_cluster_size,
        page_match_threshold,
    )
    .await?;
    let persisted_page_id = result.attached_to.as_deref().unwrap_or(&result.id);
    let persisted_space = db
        .get_page(persisted_page_id)
        .await?
        .ok_or_else(|| ServerError::Internal("Page disappeared after create".into()))?
        .space;
    let attached_existing = result.attached_to.is_some();
    // Marks go on the note this call created. A create that attached to an
    // existing note wrote nothing of its own, so it marks nothing.
    let mut written_basis = Vec::new();
    let mut written_code_basis = Vec::new();
    let mut warnings = result.warnings;
    if !attached_existing {
        for (basis_page_id, version) in &marks {
            if let Err(error) = db
                .mark_page_basis(&result.id, basis_page_id, *version, &agent)
                .await
            {
                tracing::warn!(
                    "[create_page] {} created; basis {basis_page_id} not recorded: {error}",
                    result.id
                );
                // The note stays; the agent is told which basis did not land
                // and can mark it with the basis route.
                warnings.push(format!("basis {basis_page_id} was not recorded: {error}"));
            }
        }
        written_basis = crate::page_basis_routes::written_basis(&db, &result.id).await?;
        if !prepared_code_basis.is_empty() {
            if let Err(error) = db
                .upsert_page_code_basis(&result.id, &prepared_code_basis, &agent)
                .await
            {
                tracing::warn!(
                    "[create_page] {} created; code basis not recorded: {error}",
                    result.id
                );
                warnings.push(format!("code_basis was not recorded: {error}"));
            }
        }
        written_code_basis = db.list_page_code_basis(&result.id).await?;
    } else if !prepared_code_basis.is_empty() {
        warnings.push(
            "code_basis was not recorded: this call attached to an existing note and wrote \
             nothing of its own"
                .to_string(),
        );
    }
    Ok(Json(CreatePageResponse {
        code_basis: written_code_basis,
        basis: written_basis,
        id: result.id,
        attached_to: result.attached_to,
        warnings,
        space: persisted_space.clone(),
        space_source: Some(if attached_existing {
            WriteSpaceSource::Existing
        } else if persisted_space.is_some() {
            resolved.source
        } else {
            WriteSpaceSource::Uncategorized
        }),
        write_outcome: Some(if attached_existing {
            WriteOutcome::AttachedExisting
        } else {
            WriteOutcome::Created
        }),
    }))
}

/// Map core draft errors onto the editor's wire contract.
///
/// The desktop editor (`src/lib/tauri.ts` `parsePageDraftError`) parses a JSON
/// object with a `code` field out of the error text; the codes and messages
/// here mirror `e2e/tauriMock/runtime.ts`, the executable spec the UI is
/// tested against. Anything uncoded (validation, internal) falls through to
/// the plain `ServerError` mapping.
fn page_draft_error(err: wenlan_core::WenlanError) -> ServerError {
    match err {
        wenlan_core::WenlanError::PageDraftIdConflict(_) => ServerError::Structured {
            status: axum::http::StatusCode::CONFLICT,
            body: serde_json::json!({
                "code": "page_draft_id_conflict",
                "error": "Page draft id already belongs to another Page",
            }),
        },
        wenlan_core::WenlanError::NotFound(_) => draft_not_found(),
        other => ServerError::from(other),
    }
}

fn draft_not_found() -> ServerError {
    ServerError::Structured {
        status: axum::http::StatusCode::NOT_FOUND,
        body: serde_json::json!({
            "code": "page_draft_not_found",
            "error": "Page draft not found",
        }),
    }
}

/// Pass a draft-chain page echo through the caller's truth grant, the same
/// contract as `handle_get_page`: a page this caller may not see is not there,
/// and the structured draft 404 is the honest wire answer. Fresh drafts and
/// publishes are `Unevaluated` and pass in full; this bites only when a
/// publish replay echoes a page since classified unsupported.
async fn filter_draft_echo(
    db: &wenlan_core::db::MemoryDB,
    view: &crate::truth_guard::TruthView,
    page: wenlan_core::pages::Page,
) -> Result<wenlan_core::pages::Page, ServerError> {
    wenlan_core::truth_adapter::filter_page(db, &view.grant, Some(page))
        .await
        .map_err(|e| ServerError::Internal(e.to_string()))?
        .ok_or_else(draft_not_found)
}

/// Decide whether a publish title-conflict 409 may disclose the conflicting
/// page's identity, and if so, which identity. Page ids are reusable, so the
/// pair captured inside the publish transaction can be stale by the time the
/// handler runs. `MemoryDB::conflict_identity_snapshot` makes the row reload,
/// the truth verdict, and the title-fold recheck under one connection guard,
/// so no concurrent rename, delete, or id reuse can split the visibility
/// decision from the disclosed identity. Any failure degrades to omission —
/// the 409 itself is already established and never turns into a 500 here.
async fn visible_conflict_identity(
    db: &wenlan_core::db::MemoryDB,
    view: &crate::truth_guard::TruthView,
    existing_page_id: &str,
    wanted_key: &str,
    expected_scope: &str,
) -> Option<(String, String)> {
    match db
        .conflict_identity_snapshot(&view.grant, existing_page_id, wanted_key, expected_scope)
        .await
    {
        Ok(identity) => identity,
        Err(e) => {
            tracing::warn!(
                "[page] conflict identity check failed for {existing_page_id}; omitting: {e}"
            );
            None
        }
    }
}

fn draft_version_conflict(current_version: i64) -> ServerError {
    ServerError::Structured {
        status: axum::http::StatusCode::CONFLICT,
        body: serde_json::json!({
            "code": "draft_version_conflict",
            "error": "Page draft changed since it was loaded",
            "current_version": current_version,
        }),
    }
}

async fn draft_db(
    state: &Arc<RwLock<ServerState>>,
) -> Result<Arc<wenlan_core::db::MemoryDB>, ServerError> {
    let s = state.read().await;
    s.db.clone().ok_or(ServerError::DbNotInitialized)
}

async fn draft_response(
    db: &wenlan_core::db::MemoryDB,
    page: wenlan_core::pages::Page,
) -> Result<PageDraftResponse, ServerError> {
    Ok(PageDraftResponse {
        folder_path: db.page_initial_folder_path(&page.id).await?,
        page,
        projection_status: None,
        storage_path: None,
        projection_error: None,
    })
}

/// Move only an authorized, visible active note. Scope/truth checks precede disk lookup.
pub async fn handle_move_page(
    State(state): State<SharedState>,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    view: crate::truth_guard::TruthView,
    Path(id): Path<String>,
    Json(req): Json<MovePageRequest>,
) -> Result<Json<MovePageResponse>, ServerError> {
    let (db, root) = {
        let s = state.read().await;
        (
            s.db.clone().ok_or(ServerError::DbNotInitialized)?,
            s.lint_config.page_root().map(std::path::Path::to_path_buf),
        )
    };
    let scope = crate::read_scope::effective_read_scope(&db, None, header_space.as_deref()).await?;
    let page = db.get_page_scoped(&id, &scope).await?;
    let page = wenlan_core::truth_adapter::filter_page(&db, &view.grant, page)
        .await?
        .filter(wenlan_core::pages::is_active_file_page)
        .ok_or_else(|| ServerError::NotFound("page not found".into()))?;
    let root = root
        .ok_or_else(|| ServerError::ValidationError("Wiki projection is not configured".into()))?;
    let projection_write = db.begin_page_projection_write();
    let storage_path = tokio::task::spawn_blocking(move || {
        // The blocking mutation may outlive cancellation of the request future.
        let _projection_write = projection_write;
        wenlan_core::export::knowledge::move_projected_page(
            &root,
            &page.id,
            &req.expected_storage_path,
            &req.folder_path,
            &req.operation_id,
        )
    })
    .await
    .map_err(|e| ServerError::Internal(e.to_string()))??;
    Ok(Json(MovePageResponse { storage_path }))
}

/// POST /api/pages/drafts
///
/// First durable snapshot of a human-authored Page draft. An omitted `space`
/// key inherits the `X-Wenlan-Space` header; an explicit `null` clears it.
pub async fn handle_create_page_draft(
    State(state): State<Arc<RwLock<ServerState>>>,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    view: crate::truth_guard::TruthView,
    Json(req): Json<CreatePageDraftRequest>,
) -> Result<Json<PageDraftResponse>, ServerError> {
    let db = draft_db(&state).await?;
    let space = if req.space_was_provided() {
        req.space.clone()
    } else {
        header_space
    };
    let page = db
        .create_page_draft_with_id_in_registered_space_and_folder(
            &req.draft_id,
            &req.title,
            &req.content,
            space.as_deref(),
            req.folder_path.as_deref().unwrap_or(""),
        )
        .await
        .map_err(page_draft_error)?;
    let page = filter_draft_echo(&db, &view, page).await?;
    Ok(Json(draft_response(&db, page).await?))
}

/// PUT /api/pages/drafts/{id}
pub async fn handle_update_page_draft(
    State(state): State<Arc<RwLock<ServerState>>>,
    view: crate::truth_guard::TruthView,
    Path(id): Path<String>,
    Json(req): Json<UpdatePageDraftRequest>,
) -> Result<Json<PageDraftResponse>, ServerError> {
    let db = draft_db(&state).await?;
    match db
        .update_page_draft_in_registered_space(
            &id,
            req.expected_version,
            &req.title,
            &req.content,
            req.space.as_deref(),
        )
        .await
        .map_err(page_draft_error)?
    {
        wenlan_core::pages::PageDraftUpdateOutcome::Updated(page) => {
            let page = filter_draft_echo(&db, &view, page).await?;
            Ok(Json(draft_response(&db, page).await?))
        }
        wenlan_core::pages::PageDraftUpdateOutcome::VersionConflict { current_version } => {
            Err(draft_version_conflict(current_version))
        }
    }
}

/// POST /api/pages/drafts/{id}/publish
pub async fn handle_publish_page_draft(
    State(state): State<Arc<RwLock<ServerState>>>,
    view: crate::truth_guard::TruthView,
    Path(id): Path<String>,
    Json(req): Json<PageDraftVersionRequest>,
) -> Result<Json<PageDraftResponse>, ServerError> {
    let (db, page_root) = {
        let s = state.read().await;
        (
            s.db.clone().ok_or(ServerError::DbNotInitialized)?,
            s.lint_config.page_root().map(std::path::Path::to_path_buf),
        )
    };
    // Resolve response intent before the DB commit; projection failures below remain pending.
    let folder_path = db.page_initial_folder_path(&id).await?;
    let page = match db
        .publish_page_draft(&id, req.expected_version)
        .await
        .map_err(page_draft_error)?
    {
        wenlan_core::pages::PageDraftPublishOutcome::Published(page) => page,
        wenlan_core::pages::PageDraftPublishOutcome::VersionConflict { current_version } => {
            return Err(draft_version_conflict(current_version))
        }
        wenlan_core::pages::PageDraftPublishOutcome::TitleConflict {
            existing_page_id,
            existing_page_title,
            scope,
        } => {
            let disclosed = visible_conflict_identity(
                &db,
                &view,
                &existing_page_id,
                &wenlan_core::db::MemoryDB::page_title_key(&existing_page_title),
                &scope,
            )
            .await;
            let mut body = serde_json::json!({
                "code": "page_title_conflict",
                "error": "A Page with this title already exists",
            });
            if let Some((current_id, current_title)) = disclosed {
                body["existing_page_id"] = current_id.into();
                body["existing_page_title"] = current_title.into();
            }
            return Err(ServerError::Structured {
                status: axum::http::StatusCode::CONFLICT,
                body,
            });
        }
    };
    let page = filter_draft_echo(&db, &view, page).await?;
    let mut response = PageDraftResponse {
        page,
        folder_path,
        projection_status: None,
        storage_path: None,
        projection_error: None,
    };
    response.projection_status = Some(PageProjectionStatus::NotConfigured);
    if let Some(page_root) = page_root {
        let projection =
            wenlan_core::export::knowledge::KnowledgeProjectionWrite::new(page_root.clone(), &db);
        match projection.write_page_gated(&db, &response.page).await {
            Ok(Some(path)) => {
                response.projection_status = Some(PageProjectionStatus::Synced);
                response.storage_path = std::path::Path::new(&path)
                    .strip_prefix(&page_root)
                    .ok()
                    .map(|relative| {
                        relative
                            .components()
                            .map(|c| c.as_os_str().to_string_lossy())
                            .collect::<Vec<_>>()
                            .join("/")
                    });
            }
            Ok(None) => response.projection_status = Some(PageProjectionStatus::Hidden),
            Err(error) => {
                tracing::warn!(
                    "draft {} published with pending projection: {error}",
                    response.page.id
                );
                response.projection_status = Some(PageProjectionStatus::Pending);
                response.projection_error = Some(error.to_string());
            }
        }
    }
    Ok(Json(response))
}

/// DELETE /api/pages/drafts/{id}
///
/// Drafts are never projected to md, so unlike `handle_delete_page` there is
/// no projection cleanup here.
pub async fn handle_discard_page_draft(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(id): Path<String>,
    Json(req): Json<PageDraftVersionRequest>,
) -> Result<Json<serde_json::Value>, ServerError> {
    let db = draft_db(&state).await?;
    match db
        .delete_page_draft(&id, req.expected_version)
        .await
        .map_err(page_draft_error)?
    {
        wenlan_core::pages::PageDraftDeleteOutcome::Deleted => {
            Ok(Json(serde_json::json!({"status": "deleted"})))
        }
        wenlan_core::pages::PageDraftDeleteOutcome::VersionConflict { current_version } => {
            Err(draft_version_conflict(current_version))
        }
    }
}

fn normalize_page_write_target(
    space: &WriteSpaceTarget,
    workspace: Option<&str>,
) -> Result<WriteSpaceTarget, ServerError> {
    let workspace = workspace.map(str::trim).filter(|value| !value.is_empty());
    match (space, workspace) {
        (WriteSpaceTarget::Inherit, Some(workspace)) => {
            Ok(WriteSpaceTarget::Named(workspace.to_string()))
        }
        (WriteSpaceTarget::Named(space), Some(workspace)) if space == workspace => {
            Ok(WriteSpaceTarget::Named(space.clone()))
        }
        (WriteSpaceTarget::Named(space), Some(workspace)) => Err(ServerError::ValidationError(
            format!("page space aliases conflict: space='{space}' and workspace='{workspace}'"),
        )),
        (WriteSpaceTarget::Uncategorized, Some(workspace)) => {
            Err(ServerError::ValidationError(format!(
                "page space aliases conflict: explicit Uncategorized and workspace='{workspace}'"
            )))
        }
        (target, None) => Ok(target.clone()),
    }
}

/// POST /api/pages/export
pub async fn handle_export_pages(
    State(state): State<Arc<RwLock<ServerState>>>,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    Json(req): Json<ExportPagesRequest>,
) -> Result<Json<wenlan_types::ExportStats>, ServerError> {
    let db = {
        let s = state.read().await;
        s.db.clone().ok_or(ServerError::DbNotInitialized)?
    };
    let scope = crate::read_scope::effective_read_scope(&db, None, header_space.as_deref()).await?;
    // Export must stay stub-free: `kind='entity'` dual-write shadow pages carry
    // no content and are not meant to leave the daemon. The fenced
    // `list_pages_scoped` excludes them in SQL *before* LIMIT, so a burst of
    // fresh stubs can never crowd real pages out of a bounded window (the
    // browse surfaces read the `_browse` twin instead).
    //
    // Windows differ by format: the Obsidian export keeps the historical
    // 1000-row window, while the OKF bundle reads every active page in one
    // query. The OKF writer deletes marker-listed files it does not plan
    // this run, so a windowed read would delete older pages' files on
    // re-export; and OFFSET paging over `last_modified DESC` could skip a
    // page modified mid-export, with the same destructive result.
    const OBSIDIAN_EXPORT_WINDOW: i64 = 1000;
    let is_okf = matches!(req.format, Some(ExportFormat::Okf));
    let read_window = if is_okf {
        i64::MAX
    } else {
        OBSIDIAN_EXPORT_WINDOW
    };
    // While a truth cutover is `preparing`, `page_write_permit` declines every
    // page. The OKF writer would then plan nothing and delete every page file
    // of a previous bundle as stale, so the OKF branch refuses instead. The
    // fence is read before the page read and again after the permit loop, and
    // any change of epoch in between also refuses.
    let okf_fence_before = if is_okf {
        Some(db.cutover_fence().await?)
    } else {
        None
    };
    let pages = db
        .list_pages_scoped("active", read_window, 0, &scope)
        .await
        .map_err(|e| ServerError::Internal(e.to_string()))?;
    // A write, not a read: this puts page prose into a vault the caller names,
    // and whoever opens that vault later declared no contract and made no
    // gesture -- so the write has to satisfy the automatic reader, which is
    // what `page_write_permit` asks. The projection writer takes its permit
    // through `write_page_gated`; this exporter has no such twin, so the permit
    // is taken here. A declined page is skipped rather than failing the export:
    // refusing to export an unsupported page is the contract working.
    let mut exportable = Vec::with_capacity(pages.len());
    let mut declined = 0usize;
    for page in pages {
        if wenlan_core::truth_adapter::page_write_permit(&db, &page.id)
            .await
            .map_err(|e| ServerError::Internal(e.to_string()))?
            .is_some()
        {
            exportable.push(page);
        } else {
            declined += 1;
        }
    }
    if declined > 0 {
        tracing::info!("[export] skipped {declined} page(s) the automatic reader may not see");
    }
    // Pure-OKF bundle twin of the Obsidian export below: same pages, same
    // permit filter, different writer. The obsidian branch underneath is
    // untouched so absent/`obsidian` keeps today's behavior byte for byte.
    if let Some(fence_before) = okf_fence_before {
        let fence_after = db.cutover_fence().await?;
        if fence_before.phase == wenlan_core::db::CutoverPhase::Preparing
            || fence_after != fence_before
        {
            return Err(wenlan_core::WenlanError::Conflict(
                "a truth cutover is in progress; retry the okf export after it finishes"
                    .to_string(),
            )
            .into());
        }
        let raw = match req.vault_path {
            Some(path) if !path.trim().is_empty() => path,
            _ => {
                return Err(wenlan_core::WenlanError::Validation(
                    "okf export requires an absolute vault_path (got none)".to_string(),
                )
                .into());
            }
        };
        let expanded = if let Some(rest) = raw.strip_prefix("~/") {
            match dirs::home_dir() {
                Some(home) if !home.as_os_str().is_empty() => {
                    format!("{}/{rest}", home.display())
                }
                _ => {
                    return Err(wenlan_core::WenlanError::Validation(
                        "okf export cannot expand ~/ without a home directory; pass an absolute vault_path"
                            .to_string(),
                    )
                    .into());
                }
            }
        } else {
            raw
        };
        if !std::path::Path::new(&expanded).is_absolute() {
            return Err(wenlan_core::WenlanError::Validation(format!(
                "okf export requires an absolute vault_path (got relative path {expanded:?})"
            ))
            .into());
        }
        // An imported concept's body still links in its ORIGINAL bundle's
        // terms (`/concepts/b.md`). The exporter resolves those to wherever
        // the bundle puts that concept's page now; without this map the link
        // would be written out pointing at a path the bundle does not carry.
        let concept_paths = wenlan_core::export::okf::concept_path_map(
            &db.okf_concept_paths()
                .await
                .map_err(|e| ServerError::Internal(e.to_string()))?,
        );
        let mut stats = wenlan_core::export::okf::export_okf_with_concept_paths(
            &exportable,
            std::path::Path::new(&expanded),
            &concept_paths,
        )?;
        // `export_okf` never sets `skipped`: pages the automatic reader may
        // not see are reported here, OKF-branch only. The Obsidian response
        // below is unchanged.
        stats.skipped = declined;
        return Ok(Json(stats));
    }
    let vault_path = req
        .vault_path
        .unwrap_or_else(|| "~/obsidian-vault/Wenlan/pages".to_string());
    let expanded = if let Some(rest) = vault_path.strip_prefix("~/") {
        let home = std::env::var("HOME").unwrap_or_default();
        format!("{}/{}", home, rest)
    } else {
        vault_path
    };
    let exporter =
        wenlan_core::export::obsidian::ObsidianExporter::new(std::path::PathBuf::from(expanded));
    let stats = exporter
        .export_all(&exportable)
        .map_err(|e| ServerError::Internal(e.to_string()))?;
    Ok(Json(stats))
}

/// POST /api/pages/{id}/export
pub async fn handle_export_page(
    State(state): State<Arc<RwLock<ServerState>>>,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    Path(page_id): Path<String>,
    Json(req): Json<wenlan_types::requests::ExportPageRequest>,
) -> Result<Json<wenlan_types::responses::ExportPageResponse>, ServerError> {
    // Clone Arc out of the guard so we don't hold the RwLock read guard
    // across the DB await. (AGENTS.md "Repository invariants": never hold
    // tokio::sync::RwLock guards across .await.)
    let db = {
        let s = state.read().await;
        s.db.clone().ok_or(ServerError::DbNotInitialized)?
    };

    let scope = crate::read_scope::effective_read_scope(&db, None, header_space.as_deref()).await?;
    // Export must stay stub-free (see handle_export_pages). The fenced
    // `get_page_scoped` resolves a `kind='entity'` shadow id as absent, so a
    // stub 404s here just as it did before the Q1 lift.
    let page = db
        .get_page_scoped(&page_id, &scope)
        .await
        .map_err(|e| ServerError::Internal(e.to_string()))?
        .ok_or_else(|| ServerError::NotFound("page not found".to_string()))?;

    // Same permit as the bulk export, and for the same reason: the vault this
    // writes into is read later by someone who declared no contract. A page the
    // automatic reader may not see is not there for that reader, so the route's
    // existing not-found answer is the honest one.
    if wenlan_core::truth_adapter::page_write_permit(&db, &page.id)
        .await
        .map_err(|e| ServerError::Internal(e.to_string()))?
        .is_none()
    {
        return Err(ServerError::NotFound("page not found".to_string()));
    }

    let expanded = if let Some(rest) = req.vault_path.strip_prefix("~/") {
        let home = std::env::var("HOME").unwrap_or_default();
        format!("{}/{}", home, rest)
    } else {
        req.vault_path
    };

    let exporter =
        wenlan_core::export::obsidian::ObsidianExporter::new(std::path::PathBuf::from(expanded));
    let result = exporter
        .export(&page)
        .map_err(|e| ServerError::Internal(e.to_string()))?;

    Ok(Json(wenlan_types::responses::ExportPageResponse {
        path: result.path,
    }))
}

/// Explicit title edit shared by the desktop app and local agent clients.
/// Scope and the automatic-reader write permit are rechecked by the core
/// capability; no content-refresh or delete/recreate path is involved.
pub async fn handle_rename_page(
    State(state): State<Arc<RwLock<ServerState>>>,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    Path(id): Path<String>,
    Json(req): Json<wenlan_types::requests::RenamePageRequest>,
) -> Result<Json<wenlan_types::responses::RenamePageResponse>, ServerError> {
    let (db, page_root) = {
        let state = state.read().await;
        (
            state.db.clone().ok_or(ServerError::DbNotInitialized)?,
            state
                .lint_config
                .page_root()
                .map(std::path::Path::to_path_buf),
        )
    };
    let scope = crate::read_scope::effective_read_scope(&db, None, header_space.as_deref()).await?;
    Ok(Json(
        wenlan_core::post_write::rename_page(
            &db,
            &id,
            &req.title,
            req.expected_version,
            &scope,
            page_root.as_deref(),
        )
        .await?,
    ))
}

/// POST /api/memory/{id}/update-page
/// GET /api/pages/{id}/links
///
/// Returns the wikilink graph centered on a page: outbound (labels parsed
/// out of this page's body, with resolved target_page_id when matched) and
/// inbound (every active page whose body links to this title). Used by
/// `/pages` to surface "3 inbound, 2 broken" without the caller having to
/// fetch + parse the full body.
pub async fn handle_get_page_links(
    State(state): State<Arc<RwLock<ServerState>>>,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    view: crate::truth_guard::TruthView,
    Path(id): Path<String>,
) -> Result<Json<wenlan_types::responses::PageLinksResponse>, ServerError> {
    let db = {
        let s = state.read().await;
        s.db.clone().ok_or(ServerError::DbNotInitialized)?
    };
    let scope = crate::read_scope::effective_read_scope(&db, None, header_space.as_deref()).await?;
    // The whole graph hangs off page `id`: outbound labels are text lifted out
    // of ITS body, and inbound rows exist only because they point at it. A
    // caller who may not see the page in full gets neither -- which is what
    // this route's `NamedPage` marker shape is for. Empty rather than 404,
    // matching the answer an absent page already gets here.
    let named_visible = !wenlan_core::truth_adapter::filter_page_refs(
        &db,
        &view.grant,
        vec![id.clone()],
        |page_id| page_id.as_str(),
    )
    .await?
    .is_empty();
    if !named_visible {
        return Ok(Json(wenlan_types::responses::PageLinksResponse {
            outbound: Vec::new(),
            inbound: Vec::new(),
        }));
    }
    let outbound_raw = db.get_page_outbound_links_scoped(&id, &scope).await?;
    let inbound_raw = db.get_page_inbound_links_scoped(&id, &scope).await?;
    // An inbound row names ANOTHER page and carries the label it links with; a
    // grant for `id` does not cover page B's title in A's links.
    let inbound_raw =
        wenlan_core::truth_adapter::filter_page_refs(&db, &view.grant, inbound_raw, |row| {
            row.0.as_str()
        })
        .await?;
    // Outbound rows are this page's own link text, so they stay -- but a
    // resolved target names another page. A target the caller may not see reads
    // as unresolved, the same answer a dangling link already gets, rather than
    // disclosing that the page exists.
    let resolved_targets: Vec<String> = outbound_raw
        .iter()
        .filter_map(|l| l.target_page_id.clone())
        .collect();
    let visible_targets: std::collections::HashSet<String> =
        wenlan_core::truth_adapter::filter_page_refs(
            &db,
            &view.grant,
            resolved_targets,
            |target| target.as_str(),
        )
        .await?
        .into_iter()
        .collect();
    let outbound = outbound_raw
        .into_iter()
        .map(|l| {
            let target_page_id = l
                .target_page_id
                .filter(|target_id| visible_targets.contains(target_id));
            wenlan_types::responses::PageLinkOutbound {
                label: l.label,
                target_title: if target_page_id.is_some() {
                    l.target_title
                } else {
                    None
                },
                target_page_id,
            }
        })
        .collect();
    let inbound = inbound_raw
        .into_iter()
        .map(|(src, label)| wenlan_types::responses::PageLinkInbound {
            source_page_id: src,
            label,
        })
        .collect();
    Ok(Json(wenlan_types::responses::PageLinksResponse {
        outbound,
        inbound,
    }))
}

#[derive(Debug, Default, serde::Deserialize)]
pub struct OrphanLinksQuery {
    /// Minimum number of distinct source pages reaching for the same label.
    /// Default 2 — single-source orphans are usually typos, not emergence
    /// signal.
    #[serde(default)]
    pub min_count: Option<i64>,
}

/// GET /api/pages/orphan-links
///
/// Group every unresolved wikilink label across the graph by hit count. A
/// label reached for by N independent pages is a topic-discovery signal:
/// emergence can prioritize building a page on it. Capped at 100 rows;
/// callers needing more should raise min_count and re-issue.
pub async fn handle_list_orphan_links(
    State(state): State<Arc<RwLock<ServerState>>>,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    view: crate::truth_guard::TruthView,
    axum::extract::Query(q): axum::extract::Query<OrphanLinksQuery>,
) -> Result<Json<wenlan_types::responses::OrphanLinksResponse>, ServerError> {
    let db = {
        let s = state.read().await;
        s.db.clone().ok_or(ServerError::DbNotInitialized)?
    };
    let scope = crate::read_scope::effective_read_scope(&db, None, header_space.as_deref()).await?;
    let min_count = q.min_count.unwrap_or(2).max(1);
    // The page an orphan label leaks is the SOURCE, not the target it fails to
    // resolve to: the label is `[[wikilink]]` text lifted out of some page's
    // body, so a caller who may not read that body may not read the text either.
    // Label prose has no entry form, hence `filter_page_refs` -- `Full` or gone.
    //
    // Filtering has to happen on ROWS, which is why this reads the row query
    // rather than the aggregate it replaced: `list_orphan_link_labels_scoped`
    // does the grouping, the `HAVING n >= ?` threshold and the `LIMIT 100`
    // inside SQL, so post-filtering its output would report counts and a
    // truncation point computed over pages this caller cannot see.
    // `fold_orphan_labels` is that same aggregate, applied to what survives.
    let rows = db
        .list_orphan_link_rows_scoped(&scope)
        .await
        .map_err(|e| ServerError::Internal(e.to_string()))?;
    let rows = wenlan_core::truth_adapter::filter_page_refs(&db, &view.grant, rows, |row| {
        row.source_page_id.as_str()
    })
    .await
    .map_err(|e| ServerError::Internal(e.to_string()))?;
    let orphan_labels = wenlan_core::db::MemoryDB::fold_orphan_labels(rows, min_count)
        .into_iter()
        .map(|(label, count)| wenlan_types::responses::OrphanLink { label, count })
        .collect();
    Ok(Json(wenlan_types::responses::OrphanLinksResponse {
        min_count: min_count as usize,
        orphan_labels,
    }))
}

/// POST /api/memory/{id}/update-page
///
/// Manual edit from the app. Goes through the one page-write gate, so a hand
/// edit is treated like every other write: version CAS, changelog entry, and
/// history row. Successful saves also repair Markdown from the latest DB row;
/// projection failures never turn a committed save into a failed write.
///
/// `expected_version` is optional. Sending it makes the edit a precondition
/// (refused outright if the page moved); omitting it guards on the version the
/// server itself loaded, so the write still cannot clobber an interleaved one.
pub async fn handle_update_page(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(id): Path<String>,
    Json(wire): Json<wenlan_types::requests::ManualPageEditRequest>,
) -> Result<Json<wenlan_types::responses::ManualPageWriteResponse>, ServerError> {
    let wenlan_types::requests::ManualPageEditRequest {
        edit: req,
        minor,
        confirm_basis,
    } = wire;
    let (db, page_root) = {
        let s = state.read().await;
        (
            s.db.clone().ok_or(ServerError::DbNotInitialized)?,
            s.lint_config.page_root().map(std::path::Path::to_path_buf),
        )
    };
    // Preserve the route's 404 shape, but do not copy sources from this
    // advisory read. PageWrite derives them again from the exact generation
    // its CAS updates, so a source attached in between cannot be deleted.
    db.get_page(&id)
        .await
        .map_err(|e| ServerError::Internal(e.to_string()))?
        .ok_or_else(|| ServerError::NotFound(format!("page {id} not found")))?;

    // Use captured server state, never global config (which would point an
    // isolated handler at the user's vault). Core commits DB + receipt first.
    // `minor` rides on the version's history row. `confirm_basis` (R11) is
    // confirmed in this save's own transaction and only if the basis is still
    // at that version, so a save made any other way never confirms. A person's
    // save marks nothing: marks are an agent's statement of what it read.
    let result = wenlan_core::post_write::page_write(
        &db,
        wenlan_core::post_write::PageWrite::UpdatePreservingSources {
            page_id: &id,
            req: wenlan_types::requests::UpdatePageRequest {
                content: req.content,
                source_memory_ids: vec![],
                expected_version: req.expected_version,
                caller_id: req.caller_id,
                operation_id: req.operation_id,
            },
            edited_by: "manual_edit",
            knowledge_path: page_root.as_deref(),
            basis: wenlan_core::db::BasisWrite {
                minor: minor.unwrap_or(false),
                actor: "human".to_string(),
                marks: Vec::new(),
                confirm: confirm_basis.map(|c| (c.basis_page_id, c.version)),
                confirm_snapshot: Vec::new(),
                code_basis: Vec::new(),
            },
        },
    )
    .await?;

    // A refused write is a conflict, not a success — reporting ok:true would
    // tell the editor its text was saved when the page still holds somebody
    // else's. But "nothing changed" is not a conflict, and this route sends the
    // page's own sources back, so saving a page without editing it lands on the
    // no-change path. Branching on `wrote` alone answered that routine save with
    // "somebody else edited this," which is both wrong and unactionable.
    use wenlan_core::post_write::WriteOutcome;
    match result.outcome {
        WriteOutcome::Wrote | WriteOutcome::Unchanged => {
            use wenlan_types::responses::{ManualPageWriteResponse, PageProjectionStatus};
            let (projection_status, projection_error) = match page_root {
                Some(root) => {
                    let projected =
                        wenlan_core::export::knowledge::KnowledgeProjectionWrite::new(root, &db);
                    match db.get_page(&id).await {
                        Ok(Some(page)) => {
                            match projected.write_page_gated(&db, &page).await {
                                Ok(Some(_)) => (PageProjectionStatus::Synced, None),
                                Ok(None) => (PageProjectionStatus::Hidden, None),
                                Err(error) => {
                                    tracing::warn!("[update_page] DB saved; projection pending for {id}: {error}");
                                    (PageProjectionStatus::Pending, Some(error.to_string()))
                                }
                            }
                        }
                        _ => (
                            PageProjectionStatus::Pending,
                            Some("saved page could not be loaded for projection".into()),
                        ),
                    }
                }
                None => (PageProjectionStatus::NotConfigured, None),
            };
            Ok(Json(ManualPageWriteResponse {
                basis_confirm: result.basis_confirm,
                ok: true,
                projection_status,
                projection_error,
            }))
        }
        WriteOutcome::Refused => Err(ServerError::Conflict(format!(
            "page {id} changed while this edit was open; reload and reapply"
        ))),
        WriteOutcome::Contended => Err(ServerError::Conflict(format!(
            "page {id} is being written too fast to edit safely; try again"
        ))),
        // Unreachable: gating is for machine writers, and this route is
        // `manual_edit`. Answered honestly rather than folded into a catch-all.
        WriteOutcome::Gated => Err(ServerError::Conflict(format!(
            "edit to page {id} was staged for review instead of applied"
        ))),
    }
}

/// PUT /api/pages/{id}
///
/// Agent-side refresh of a page from its current sources. Replaces content,
/// source list, optional summary; clears `stale_reason` so the refinery's
/// re-distill skips on its next tick (CAS pattern — see refinery
/// `re_distill_stale_pages`). Distinct from POST `/api/memory/{id}/update-page`
/// (manual edit, flips `user_edited`, preserves sources).
///
/// Commit through the canonical CAS gate before projecting. A rejected
/// candidate never reaches the vault, so a failed gate needs no filesystem
/// rollback that could overwrite an interleaved external edit.
pub async fn handle_refresh_page(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(req): Json<wenlan_types::requests::RefreshPageRequest>,
) -> Result<Json<wenlan_types::responses::PageWriteResponse>, ServerError> {
    let telemetry = { state.read().await.telemetry.clone() };
    // The agent named on a staged revision card. Only the header counts: an
    // unattributed write leaves the card unattributed.
    let proposer = headers
        .get("x-agent-name")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let result =
        handle_refresh_page_inner(State(state), Path(id), proposer.as_deref(), Json(req)).await;
    match &result {
        Ok((wenlan_core::post_write::WriteOutcome::Wrote, _)) => {
            telemetry.record(TelemetryEvent::WikiGenerated)
        }
        Ok(_) => {}
        Err(_) => telemetry.record(TelemetryEvent::WikiError),
    }
    result.map(|(_, response)| Json(response))
}

/// 409 for an agent refresh whose `expected_version` no longer matches. The
/// text is what the MCP `write_page` tool shows the agent, so it says exactly
/// how to recover.
fn page_read_required_conflict(id: &str, current_version: i64) -> ServerError {
    ServerError::Conflict(format!(
        "page {id} has the user's own writing, so a refresh must say which version it read; \
         read it with read_page and retry write_page with expected_version {current_version}"
    ))
}

fn page_moved_conflict(id: &str, current_version: i64) -> ServerError {
    ServerError::Conflict(format!(
        "page {id} moved to v{current_version}; re-read it with read_page and retry \
         write_page with expected_version {current_version}"
    ))
}

async fn handle_refresh_page_inner(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(id): Path<String>,
    proposer: Option<&str>,
    Json(req): Json<wenlan_types::requests::RefreshPageRequest>,
) -> Result<
    (
        wenlan_core::post_write::WriteOutcome,
        wenlan_types::responses::PageWriteResponse,
    ),
    ServerError,
> {
    // Validate the body before touching the filesystem. Empty content would
    // produce an empty md. An empty source list is checked against the page
    // below: it is accepted only for a page that has no sources to drop.
    if req.content.trim().is_empty() {
        return Err(ServerError::ValidationError(
            "content must not be empty".into(),
        ));
    }
    wenlan_core::export::provenance::validate_canonical_page_content(&req.content)
        .map_err(ServerError::from)?;

    let (db, page_root) = {
        let s = state.read().await;
        (
            s.db.clone().ok_or(ServerError::DbNotInitialized)?,
            s.lint_config.page_root().map(std::path::Path::to_path_buf),
        )
    };
    // A note id in the source list is a mistake the agent can fix right now:
    // say which field takes notes.
    crate::page_basis_routes::refuse_note_ids_in_sources(&db, &req.source_memory_ids).await?;
    // This route carries no Space header, so the visibility check is the
    // truth grant alone; the same-Space rule in core bounds the rest.
    let basis_marks: Vec<(String, i64)> = req
        .basis
        .iter()
        .map(|b| (b.page_id.clone(), b.version))
        .collect();
    crate::page_basis_routes::require_basis_visible(
        &db,
        &wenlan_core::read_scope::ReadScope::Global,
        &basis_marks,
    )
    .await?;
    // Read the staleness fence before the fields it protects (same
    // fence-first discipline as the refresh/growth path): a source attached
    // between this read and the page read below is then either fully
    // reflected in `existing` or fully caught by the card's CAS on accept.
    // This is an acceptance-time staging token, not a compile fence — the
    // request carries no counter of its own, so this only proves nothing
    // attached a new source between here and the write/staging below; it
    // cannot detect that the agent's own compile was already stale before
    // the request reached this route.
    let source_revision = db
        .try_get_page_source_revision(&id)
        .await
        .map_err(|e| ServerError::Internal(e.to_string()))?;

    let existing = db
        .get_page(&id)
        .await
        .map_err(|e| ServerError::Internal(e.to_string()))?
        .ok_or_else(|| ServerError::ValidationError(format!("page {} not found", id)))?;
    // `existing` just proved the page row exists, so the counter read a
    // moment earlier must also have found one; `None` here means the page
    // was deleted in the gap between the two reads — an internal race, not
    // the ordinary "page not found" the check above already handles.
    let source_revision = source_revision.ok_or_else(|| {
        ServerError::Internal(format!(
            "page {id} vanished between the source_revision read and the page read"
        ))
    })?;

    // A caller-supplied `expected_version` is a precondition checked before
    // any card is staged: a card built from content computed against an old
    // version would revert whatever landed since once accepted.
    if let Some(expected) = req.expected_version {
        if expected != existing.version {
            return Err(page_moved_conflict(&id, existing.version));
        }
    }
    // A refresh that does not say which version it read cannot prove it saw
    // the user's own writing, so it is refused the same way a stale read is:
    // the agent re-reads and retries, and its next write lands directly. This
    // used to stage a revision card, which turned an agent's omission into
    // review work for the human. A locked page still stages a card below,
    // because the lock asks for review of every agent change.
    if req.expected_version.is_none()
        && !existing.review_required
        && wenlan_core::post_write::page_is_human_owned(&existing)
    {
        return Err(page_read_required_conflict(&id, existing.version));
    }
    if req.source_memory_ids.is_empty() && !existing.source_memory_ids.is_empty() {
        return Err(ServerError::ValidationError(
            "source_memory_ids must not be empty for a page that has sources; \
             refresh keeps the page linked to its sources"
                .into(),
        ));
    }

    // Ownership gate (spec §5.1/§5.2): agent refresh is a machine write. A
    // review-locked page always stages a card. A human-owned page
    // (user_edited=1 OR creation_kind='authored') reaches here only with a
    // checked `expected_version` (refused above otherwise), so it lands
    // directly. The page prose stays byte-unchanged until the human accepts a
    // card. The core gate re-runs the same rule inside its CAS loop, which
    // still stages a card if the page turns human-owned mid-write.
    let requires_review = wenlan_core::post_write::machine_write_requires_review(
        wenlan_core::post_write::Writer::classify("agent_refresh"),
        &existing,
        req.expected_version.is_some(),
        false,
    );
    // Lines of code the write relies on: read and fingerprinted only now that
    // the cheap refusals (404, 409, ownership) are behind us, so those never
    // touch the disk. A bad path, span or unreadable file still refuses the
    // request before anything is staged or written.
    let prepared_code_basis = wenlan_core::code_basis::prepare_code_basis(&req.code_basis)
        .await
        .map_err(crate::code_basis_routes::code_basis_error)?;
    if requires_review {
        let result = wenlan_core::post_write::stage_page_revision_card_proposed_by(
            &db,
            &existing,
            &req.content,
            &req.source_memory_ids,
            source_revision,
            "agent_refresh",
            proposer,
            None,
        )
        .await
        .map_err(ServerError::from)?;
        return Ok((
            wenlan_core::post_write::WriteOutcome::Gated,
            wenlan_types::responses::PageWriteResponse {
                code_basis: db.list_page_code_basis(&id).await?,
                basis: crate::page_basis_routes::written_basis(&db, &id).await?,
                ok: true,
                revision_card_id: result.revision_card_id,
                gated: true,
                warnings: gated_code_basis_warning(&prepared_code_basis),
            },
        ));
    }

    // Summary omitted keeps the current value; an empty string clears it.
    let summary_update: Option<Option<String>> = match &req.summary {
        Some(s) if s.is_empty() => Some(None),
        Some(s) => Some(Some(s.clone())),
        None => None,
    };
    use wenlan_core::post_write::WriteOutcome;
    let result = wenlan_core::post_write::page_write(
        &db,
        wenlan_core::post_write::PageWrite::AgentUpdate {
            basis: wenlan_core::db::BasisWrite {
                minor: req.minor.unwrap_or(false),
                actor: proposer.unwrap_or("unknown").to_string(),
                marks: req
                    .basis
                    .iter()
                    .map(|b| (b.page_id.clone(), b.version))
                    .collect(),
                confirm: None,
                confirm_snapshot: Vec::new(),
                code_basis: prepared_code_basis.clone(),
            },
            page_id: &id,
            req: wenlan_types::requests::UpdatePageRequest {
                content: req.content.clone(),
                source_memory_ids: req.source_memory_ids.clone(),
                expected_version: req.expected_version,
                caller_id: None,
                operation_id: None,
            },
            agent: proposer,
            expected_source_revision: source_revision,
            knowledge_path: page_root.as_deref(),
        },
    )
    .await?;
    // The core gate refused because the page moved past `expected_version`
    // between the pre-check above and the write. Nothing landed.
    if result.outcome == WriteOutcome::Refused {
        let current_version = db
            .get_page(&id)
            .await
            .map_err(|e| ServerError::Internal(e.to_string()))?
            .map(|page| page.version)
            .unwrap_or(existing.version);
        return Err(page_moved_conflict(&id, current_version));
    }
    // A write that changed nothing wrote no transaction for the code bases to
    // ride on; the lines the agent says it relies on are recorded on their own.
    if result.outcome == WriteOutcome::Unchanged && !prepared_code_basis.is_empty() {
        db.upsert_page_code_basis(&id, &prepared_code_basis, proposer.unwrap_or("unknown"))
            .await
            .map_err(crate::code_basis_routes::code_basis_error)?;
    }
    // Metadata follows the canonical result, never a rejected/staged candidate.
    if matches!(
        result.outcome,
        WriteOutcome::Wrote | WriteOutcome::Unchanged
    ) {
        if let Some(opt) = &summary_update {
            db.update_page_summary(&id, opt.as_deref()).await?;
        }
        db.clear_page_staleness(&id).await?;
        if let Some(root) = page_root {
            // Re-read so a concurrent human edit cannot be replaced by the
            // original refresh snapshot. Projection failure is post-commit.
            if let Some(page) = db.get_page(&id).await? {
                let projection =
                    wenlan_core::export::knowledge::KnowledgeProjectionWrite::new(root, &db);
                if let Err(error) = projection.write_page_gated(&db, &page).await {
                    tracing::warn!("[page] refresh saved; projection pending for {id}: {error}");
                }
            }
        }
    }

    Ok((
        result.outcome,
        wenlan_types::responses::PageWriteResponse {
            code_basis: db.list_page_code_basis(&id).await?,
            basis: crate::page_basis_routes::written_basis(&db, &id).await?,
            ok: true,
            revision_card_id: result.revision_card_id,
            gated: result.gated,
            warnings: if result.gated {
                gated_code_basis_warning(&prepared_code_basis)
            } else {
                Vec::new()
            },
        },
    ))
}

/// A refresh staged as a revision card records no code bases: the prose the
/// lines were read for has not landed yet. Say so when the write carried any,
/// so the agent re-sends them after the card is accepted.
fn gated_code_basis_warning(
    prepared: &[wenlan_core::code_basis::PreparedCodeBasis],
) -> Vec<String> {
    if prepared.is_empty() {
        return Vec::new();
    }
    vec![
        "code_basis was not recorded: this write was staged as a revision card, so the card \
         must be accepted first and the write re-sent with code_basis"
            .to_string(),
    ]
}

// ===== Revision history endpoints =====

/// GET /api/pages/{id}/revisions
///
/// Return the changelog entries for a page, plus envelope metadata.
pub async fn handle_get_page_revisions(
    State(state): State<Arc<RwLock<ServerState>>>,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    view: crate::truth_guard::TruthView,
    Path(id): Path<String>,
) -> Result<Json<wenlan_types::responses::ListPageRevisionsResponse>, ServerError> {
    let db = {
        let s = state.read().await;
        s.db.clone().ok_or(ServerError::DbNotInitialized)?
    };
    let scope = crate::read_scope::effective_read_scope(&db, None, header_space.as_deref()).await?;
    // Q1 sub-resource: revisions is a read-only browse surface, so a shadow id
    // resolves 200 (empty changelog) via the unfenced `_browse` twin.
    let page = db.get_page_scoped_browse(&id, &scope).await?;
    // A hidden page 404s here for the same reason it does at `GET /api/pages/
    // {id}`: for this caller it is not there. An `EntryOnly` verdict arrives
    // already reduced, which is what clears the envelope's own `stale_reason`.
    let page = wenlan_core::truth_adapter::filter_page(&db, &view.grant, page)
        .await?
        .ok_or_else(|| ServerError::NotFound("page not found".to_string()))?;
    // A changelog entry is prose about the page -- a delta summary, a citation
    // verdict -- so it rides on `Full`, never on the `EntryOnly` carve-out that
    // lets a listing keep a title. `filter_page_refs` over the named id alone
    // answers "is this page Full for this caller", since the entries have no id
    // of their own to key on; short of Full the changelog is not even read.
    let full = !wenlan_core::truth_adapter::filter_page_refs(
        &db,
        &view.grant,
        vec![id.clone()],
        |page_id| page_id.as_str(),
    )
    .await?
    .is_empty();
    let entries: Vec<wenlan_types::responses::PageChangelogEntry> = if full {
        let changelog_str = db.get_page_changelog_scoped(&id, &scope).await?;
        serde_json::from_str(&changelog_str)
            .map_err(|e| ServerError::Internal(format!("parse changelog: {e}")))?
    } else {
        Vec::new()
    };
    Ok(Json(wenlan_types::responses::ListPageRevisionsResponse {
        page_id: id,
        current_version: page.version,
        user_edited: page.user_edited,
        stale_reason: page.stale_reason.clone(),
        entries,
    }))
}
/// POST /api/pages/{id}/review
///
/// Mark a page human-reviewed, on the strength of a capability the app's Tauri
/// backend minted for one gesture. Binding spec:
/// `docs/plans/2026-07-27-m5-presence-threat-model.md`.
///
/// This handler deliberately holds no protocol logic. It resolves the secret,
/// hands the whole request to `review_page_with_presence`, and turns the answer
/// into a status code — because the order the checks run in is the part that is
/// easy to get backwards, and it belongs in one place rather than in every
/// handler that ever carries presence.
///
/// Nothing here logs the request. The capability's `Debug` redacts its material
/// anyway, but the surest way not to leak it is to never format it (§7).
pub async fn handle_review_page(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(id): Path<String>,
    Json(request): Json<wenlan_types::requests::ReviewPageRequest>,
) -> Result<axum::response::Response, ServerError> {
    use axum::response::IntoResponse;

    let (db, presence_root) = {
        let s = state.read().await;
        (
            s.db.as_ref()
                .cloned()
                .ok_or(ServerError::DbNotInitialized)?,
            s.presence_root.clone(),
        )
    };

    // The root is handed over unresolved. Reading the secret here and refusing
    // early would put `presence_unavailable` in front of the replay lookup, so a
    // client retrying a dropped response after the secret was rotated would be
    // told its write failed when it had already succeeded — the exact T7 case
    // the ordering exists to prevent. No configured root is the same answer as
    // no secret in it, and both are reached at the one place that answers them.
    let now = chrono::Utc::now().timestamp();
    let outcome = db
        .review_page_with_presence(&request.presence, &id, presence_root.as_deref(), now)
        .await
        .map_err(ServerError::from)?;

    Ok(match outcome {
        // A replay returns the stored response verbatim and the same 200 the
        // first execution returned. A retry that succeeded looks like a success.
        wenlan_core::db::ReviewOutcome::Applied(receipt)
        | wenlan_core::db::ReviewOutcome::Replayed(receipt) => Json(receipt).into_response(),
        wenlan_core::db::ReviewOutcome::Refused(refusal) => refusal_response(refusal),
    })
}

/// The coarse wire answer. Carries the code and nothing else — no reason, no
/// path, and no part of the submitted capability (§7).
fn refusal_response(refusal: wenlan_core::presence::PresenceRefusal) -> axum::response::Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    use wenlan_core::presence::PresenceRefusal;

    let status = match refusal {
        PresenceRefusal::Invalid | PresenceRefusal::Expired => StatusCode::FORBIDDEN,
        PresenceRefusal::Replayed | PresenceRefusal::Conflict => StatusCode::CONFLICT,
        PresenceRefusal::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    };
    (status, Json(serde_json::json!({ "error": refusal.code() }))).into_response()
}

#[cfg(test)]
#[path = "page_routes/page_routes_test.rs"]
mod page_inventory_tests;

#[cfg(test)]
mod create_page_endpoint_tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use serde_json::json;
    use std::sync::Arc;
    use tokio::sync::RwLock;
    use tower::ServiceExt;

    use crate::state::ServerState;
    use wenlan_types::requests::CreateConceptRequest;

    struct DataDirGuard {
        previous: Option<std::ffi::OsString>,
        _tmp: tempfile::TempDir,
    }

    impl DataDirGuard {
        fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let pages = tmp.path().join("pages");
            std::fs::create_dir_all(&pages).unwrap();
            std::fs::write(
                tmp.path().join("config.json"),
                serde_json::json!({ "knowledge_path": pages.to_string_lossy() }).to_string(),
            )
            .unwrap();
            let previous = std::env::var_os("WENLAN_DATA_DIR");
            std::env::set_var("WENLAN_DATA_DIR", tmp.path());
            Self {
                previous,
                _tmp: tmp,
            }
        }
    }

    impl Drop for DataDirGuard {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => std::env::set_var("WENLAN_DATA_DIR", value),
                None => std::env::remove_var("WENLAN_DATA_DIR"),
            }
        }
    }

    async fn build_state_with_db(
        page_min_cluster_size: usize,
    ) -> (Arc<RwLock<ServerState>>, tempfile::TempDir) {
        build_state_with_db_and_page_match_threshold(
            page_min_cluster_size,
            wenlan_core::tuning::DistillationConfig::default().page_match_threshold,
        )
        .await
    }

    async fn build_state_with_db_and_page_match_threshold(
        page_min_cluster_size: usize,
        page_match_threshold: f64,
    ) -> (Arc<RwLock<ServerState>>, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("failed to create tempdir");
        let emitter: Arc<dyn wenlan_core::events::EventEmitter> =
            Arc::new(wenlan_core::events::NoopEmitter);
        let db = wenlan_core::db::MemoryDB::new(tmp.path(), emitter)
            .await
            .expect("MemoryDB::new should succeed");
        db.upsert_documents(vec![
            wenlan_core::sources::RawDocument {
                source: "memory".to_string(),
                source_id: "mem-page-floor-a".to_string(),
                title: "mem-page-floor-a".to_string(),
                content: "Rust ownership prevents memory safety bugs".to_string(),
                last_modified: chrono::Utc::now().timestamp(),
                memory_type: Some("fact".to_string()),
                source_agent: Some("test".to_string()),
                confidence: Some(0.9),
                ..Default::default()
            },
            wenlan_core::sources::RawDocument {
                source: "memory".to_string(),
                source_id: "mem-page-floor-b".to_string(),
                title: "mem-page-floor-b".to_string(),
                content: "Rust borrowing validates references at compile time".to_string(),
                last_modified: chrono::Utc::now().timestamp(),
                memory_type: Some("fact".to_string()),
                source_agent: Some("test".to_string()),
                confidence: Some(0.9),
                ..Default::default()
            },
            wenlan_core::sources::RawDocument {
                source: "memory".to_string(),
                source_id: "mem-page-floor-c".to_string(),
                title: "mem-page-floor-c".to_string(),
                content: "Rust lifetimes describe how long references remain valid".to_string(),
                last_modified: chrono::Utc::now().timestamp(),
                memory_type: Some("fact".to_string()),
                source_agent: Some("test".to_string()),
                confidence: Some(0.9),
                ..Default::default()
            },
        ])
        .await
        .unwrap();
        let mut tuning = wenlan_core::tuning::TuningConfig::default();
        tuning.distillation.page_min_cluster_size = page_min_cluster_size;
        tuning.distillation.page_match_threshold = page_match_threshold;
        let server_state = ServerState {
            db: Some(Arc::new(db)),
            tuning,
            ..Default::default()
        };
        (Arc::new(RwLock::new(server_state)), tmp)
    }

    async fn seed_distilled_page(
        db: &wenlan_core::db::MemoryDB,
        title: &str,
        summary: Option<&str>,
        content: &str,
        space: Option<&str>,
        source_ids: &[&str],
    ) -> String {
        db.upsert_documents(
            source_ids
                .iter()
                .map(|source_id| wenlan_core::sources::RawDocument {
                    source: "memory".to_string(),
                    source_id: (*source_id).to_string(),
                    title: (*source_id).to_string(),
                    content: content.to_string(),
                    last_modified: chrono::Utc::now().timestamp(),
                    memory_type: Some("fact".to_string()),
                    source_agent: Some("test".to_string()),
                    confidence: Some(0.9),
                    confirmed: Some(true),
                    ..Default::default()
                })
                .collect(),
        )
        .await
        .unwrap();
        let result = wenlan_core::post_write::create_page_with_floor(
            db,
            CreateConceptRequest {
                title: title.to_string(),
                content: content.to_string(),
                summary: summary.map(str::to_string),
                entity_id: None,
                space: (space.map(str::to_string)).into(),
                source_memory_ids: source_ids.iter().map(|id| (*id).to_string()).collect(),
                creation_kind: Some("distilled".to_string()),
                workspace: space.map(str::to_string),
            },
            "test",
            None,
            source_ids.len(),
        )
        .await
        .unwrap();
        db.set_page_review_status(&result.id, "confirmed")
            .await
            .unwrap();
        result.id
    }

    #[tokio::test]
    async fn create_distilled_page_with_two_sources_returns_422() {
        let _lock = crate::TEST_DATA_DIR_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        let _env = DataDirGuard::new();
        let (state, _tmp) = build_state_with_db(3).await;
        let app = crate::router::build_router(state);
        let body = json!({
            "title": "Rust Safety",
            "content": "Rust ownership and borrowing prevent memory safety bugs by validating references",
            "summary": "Rust safety",
            "source_memory_ids": ["mem-page-floor-a", "mem-page-floor-b"],
            "creation_kind": "distilled"
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/pages")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let bytes = axum::body::to_bytes(response.into_body(), 1_048_576)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            body["error"], "distilled page requires at least 3 distinct source memories (got 2)",
            "error should explain the exact source floor: {body}"
        );
    }

    #[tokio::test]
    async fn telemetry_counts_wiki_creation_without_explicit_creation_kind() {
        let _lock = crate::TEST_DATA_DIR_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        let _env = DataDirGuard::new();
        let (state, _tmp) = build_state_with_db(3).await;
        let telemetry_tmp = tempfile::tempdir().unwrap();
        let telemetry = Arc::new(crate::telemetry::Telemetry::for_test(
            telemetry_tmp.path(),
            "http://127.0.0.1:1/events",
            std::time::Duration::from_secs(3600),
        ));
        telemetry.set_enabled(true).unwrap();
        {
            state.write().await.telemetry = telemetry.clone();
        }
        let app = crate::router::build_router(state);
        let body = json!({
            "title": "Rust Safety Without An Explicit Kind",
            "content": "Rust ownership borrowing and lifetimes keep references memory safe by validating references",
            "summary": "Rust safety",
            "source_memory_ids": [
                "mem-page-floor-a",
                "mem-page-floor-b",
                "mem-page-floor-c"
            ]
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/pages")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(telemetry.status().pending_operations, 1);
        drop(response);
        drop(telemetry_tmp);
    }

    #[tokio::test]
    async fn telemetry_refresh_page_wrote_counts_one_wiki_generation() {
        let _lock = crate::TEST_DATA_DIR_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        let _env = DataDirGuard::new();
        let (state, _tmp) = build_state_with_db(3).await;
        let db = {
            let s = state.read().await;
            s.db.as_ref().cloned().unwrap()
        };
        let page_id = seed_distilled_page(
            &db,
            "Rust Refresh Target",
            Some("Rust refresh target"),
            "Rust ownership and borrowing prevent memory safety bugs by validating references",
            None,
            &["mem-page-floor-a", "mem-page-floor-b", "mem-page-floor-c"],
        )
        .await;
        let telemetry_tmp = tempfile::tempdir().unwrap();
        let telemetry = Arc::new(crate::telemetry::Telemetry::for_test(
            telemetry_tmp.path(),
            "http://127.0.0.1:1/events",
            std::time::Duration::from_secs(3600),
        ));
        telemetry.set_enabled(true).unwrap();
        {
            state.write().await.telemetry = telemetry.clone();
        }
        let app = crate::router::build_router(state);
        let body = json!({
            "content": "Rust ownership and lifetimes keep references safe by validating borrowing rules at compile time",
            "source_memory_ids": [
                "mem-page-floor-a",
                "mem-page-floor-b",
                "mem-page-floor-c"
            ]
        });

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/api/pages/{page_id}"))
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(telemetry.status().pending_operations, 1);

        // Repeating the same refresh is an authoritative Unchanged outcome,
        // not another generated page operation.
        let response = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/api/pages/{page_id}"))
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(telemetry.status().pending_operations, 1);
        drop(response);
        drop(telemetry_tmp);
    }

    #[tokio::test]
    async fn create_distilled_page_uses_configured_source_floor() {
        let _lock = crate::TEST_DATA_DIR_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        let _env = DataDirGuard::new();
        let (state, _tmp) = build_state_with_db(4).await;
        let app = crate::router::build_router(state);
        let body = json!({
            "title": "Rust References",
            "content": "Rust ownership, borrowing, and lifetimes keep references memory safe by validating references",
            "summary": "Rust references",
            "source_memory_ids": [
                "mem-page-floor-a",
                "mem-page-floor-b",
                "mem-page-floor-c"
            ],
            "creation_kind": "distilled"
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/pages")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let bytes = axum::body::to_bytes(response.into_body(), 1_048_576)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            body["error"], "distilled page requires at least 4 distinct source memories (got 3)",
            "error should reflect ServerState tuning: {body}"
        );
    }

    #[tokio::test]
    async fn create_distilled_near_duplicate_returns_attached_to() {
        let _lock = crate::TEST_DATA_DIR_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        let _env = DataDirGuard::new();
        let (state, _tmp) = build_state_with_db(3).await;
        let db = {
            let s = state.read().await;
            s.db.as_ref().cloned().unwrap()
        };
        db.create_space("work", None, false).await.unwrap();
        let existing_id = seed_distilled_page(
            &db,
            "Rust Workspace Operations",
            Some("Rust workspace operations"),
            "Rust workspaces share Cargo lockfiles, inherited metadata, and all-crate checks",
            Some("work"),
            &["mem-page-floor-a", "mem-page-floor-b", "mem-page-floor-c"],
        )
        .await;
        let app = crate::router::build_router(state);
        let body = json!({
            "title": "Rust Workspace Operations",
            "content": "Rust workspaces share Cargo lockfiles, inherited metadata, and all-crate checks",
            "summary": "Rust workspace operations",
            "space": "work",
            "source_memory_ids": [
                "mem-page-floor-a",
                "mem-page-floor-b",
                "mem-page-floor-c"
            ],
            "creation_kind": "distilled"
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/pages")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), 1_048_576)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["id"].as_str(), Some(existing_id.as_str()));
        assert_eq!(
            body["attached_to"].as_str(),
            Some(existing_id.as_str()),
            "create_page response must expose the attach target: {body}"
        );
        let pages = db.list_pages("active", 10, 0).await.unwrap();
        assert_eq!(pages.len(), 1, "HTTP near-duplicate attach must not mint");
    }

    #[tokio::test]
    async fn create_distilled_near_duplicate_uses_configured_page_match_threshold() {
        let _lock = crate::TEST_DATA_DIR_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        let _env = DataDirGuard::new();
        let (state, _tmp) = build_state_with_db_and_page_match_threshold(3, 1.1).await;
        let db = {
            let s = state.read().await;
            s.db.as_ref().cloned().unwrap()
        };
        db.create_space("work", None, false).await.unwrap();
        let existing_id = seed_distilled_page(
            &db,
            "Rust Workspace Operations",
            Some("Rust workspace operations"),
            "Rust workspaces share Cargo lockfiles, inherited metadata, and all-crate checks",
            Some("recap"),
            &["mem-page-floor-a", "mem-page-floor-b", "mem-page-floor-c"],
        )
        .await;
        let app = crate::router::build_router(state);
        let body = json!({
            "title": "Rust Workspace Operations",
            "content": "Rust workspaces share Cargo lockfiles, inherited metadata, and all-crate checks",
            "summary": "Rust workspace operations",
            "space": "work",
            "source_memory_ids": [
                "mem-page-floor-a",
                "mem-page-floor-b",
                "mem-page-floor-c"
            ],
            "creation_kind": "distilled"
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/pages")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), 1_048_576)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_ne!(
            body["id"].as_str(),
            Some(existing_id.as_str()),
            "configured threshold above possible cosine must force a new page: {body}"
        );
        assert!(
            body.get("attached_to").is_none(),
            "new page response must omit attached_to: {body}"
        );
        let pages = db.list_pages("active", 10, 0).await.unwrap();
        assert_eq!(
            pages.len(),
            2,
            "below configured threshold should mint a new page"
        );
    }

    // ── PR A: refresh ownership, expected_version, review lock ──

    const REFRESH_SOURCES: [&str; 3] = ["mem-page-floor-a", "mem-page-floor-b", "mem-page-floor-c"];
    const REFRESH_BODY: &str =
        "Rust ownership and borrowing prevent memory safety bugs by validating references";

    async fn send_json(
        app: &crate::router::AppRouter,
        method: &str,
        uri: String,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1_048_576)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        )
    }

    /// A sourced distilled page, then a human `fs_edit` so it is human-owned.
    async fn seed_user_edited_page(db: &wenlan_core::db::MemoryDB) -> wenlan_core::pages::Page {
        let page_id = seed_distilled_page(
            db,
            "Rust Owned Refresh",
            None,
            REFRESH_BODY,
            None,
            &REFRESH_SOURCES,
        )
        .await;
        wenlan_core::post_write::update_page(
            db,
            &page_id,
            wenlan_types::requests::UpdatePageRequest {
                content: format!("{REFRESH_BODY}, with human notes"),
                source_memory_ids: REFRESH_SOURCES.iter().map(|s| s.to_string()).collect(),
                expected_version: None,
                caller_id: None,
                operation_id: None,
            },
            "fs_edit",
            false,
            None,
            None,
        )
        .await
        .unwrap();
        let page = db.get_page(&page_id).await.unwrap().unwrap();
        assert!(
            page.user_edited,
            "precondition: fs_edit marks human ownership"
        );
        page
    }

    async fn state_db_app() -> (
        Arc<wenlan_core::db::MemoryDB>,
        crate::router::AppRouter,
        tempfile::TempDir,
    ) {
        let (state, tmp) = build_state_with_db(3).await;
        let db = { state.read().await.db.as_ref().cloned().unwrap() };
        (db, crate::router::build_router(state), tmp)
    }

    #[tokio::test]
    async fn refresh_with_stale_expected_version_returns_409_with_recovery_text() {
        let _lock = crate::TEST_DATA_DIR_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        let _env = DataDirGuard::new();
        let (db, app, _tmp) = state_db_app().await;
        let page = seed_user_edited_page(&db).await;
        let (status, body) = send_json(
            &app,
            "PUT",
            format!("/api/pages/{}", page.id),
            json!({
                "content": format!("{REFRESH_BODY}, agent rewrite"),
                "source_memory_ids": REFRESH_SOURCES,
                "expected_version": page.version - 1,
            }),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        let text = body["error"].as_str().unwrap_or_default();
        assert!(
            text.contains(&format!("moved to v{}", page.version)) && text.contains("read_page"),
            "409 must name the current version and how to recover: {body}"
        );
        assert!(db.list_pending_revisions(10).await.unwrap().is_empty());
        let after = db.get_page(&page.id).await.unwrap().unwrap();
        assert_eq!(after.content, page.content);
    }

    #[tokio::test]
    async fn refresh_with_matching_expected_version_writes_a_human_owned_page() {
        let _lock = crate::TEST_DATA_DIR_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        let _env = DataDirGuard::new();
        let (db, app, _tmp) = state_db_app().await;
        let page = seed_user_edited_page(&db).await;
        let content = format!(
            "{REFRESH_BODY}, agent rewrite after reading v{}",
            page.version
        );
        let (status, body) = send_json(
            &app,
            "PUT",
            format!("/api/pages/{}", page.id),
            json!({
                "content": content,
                "source_memory_ids": REFRESH_SOURCES,
                "expected_version": page.version,
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_ne!(body["gated"], json!(true), "{body}");
        let after = db.get_page(&page.id).await.unwrap().unwrap();
        assert_eq!(after.content, content);
        assert!(db.list_pending_revisions(10).await.unwrap().is_empty());

        // Without expected_version the same page is refused: nothing is
        // written and no card is staged, and the agent is told to re-read.
        let current = db.get_page(&page.id).await.unwrap().unwrap().version;
        let (status, body) = send_json(
            &app,
            "PUT",
            format!("/api/pages/{}", page.id),
            json!({
                "content": format!("{REFRESH_BODY}, unversioned agent rewrite"),
                "source_memory_ids": REFRESH_SOURCES,
            }),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        let text = body["error"].as_str().unwrap_or_default();
        assert!(
            text.contains(&format!("expected_version {current}")) && text.contains("read_page"),
            "409 must say how to recover: {body}"
        );
        assert!(db.list_pending_revisions(10).await.unwrap().is_empty());
        assert_eq!(
            db.get_page(&page.id).await.unwrap().unwrap().content,
            content
        );
    }

    #[tokio::test]
    async fn review_lock_route_sets_clears_and_gates_refresh() {
        let _lock = crate::TEST_DATA_DIR_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        let _env = DataDirGuard::new();
        let (db, app, _tmp) = state_db_app().await;
        let page_id = seed_distilled_page(
            &db,
            "Rust Locked Refresh",
            None,
            REFRESH_BODY,
            None,
            &REFRESH_SOURCES,
        )
        .await;
        let version = db.get_page(&page_id).await.unwrap().unwrap().version;

        let (status, body) = send_json(
            &app,
            "PUT",
            format!("/api/pages/{page_id}/review-required"),
            json!({ "required": true }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body, json!({ "page_id": page_id, "review_required": true }));
        assert!(
            db.get_page(&page_id)
                .await
                .unwrap()
                .unwrap()
                .review_required
        );

        let (status, body) = send_json(
            &app,
            "PUT",
            format!("/api/pages/{page_id}"),
            json!({
                "content": format!("{REFRESH_BODY}, agent rewrite under lock"),
                "source_memory_ids": REFRESH_SOURCES,
                "expected_version": version,
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            body["gated"],
            json!(true),
            "the lock stages even a versioned refresh: {body}"
        );
        assert_eq!(
            db.get_page(&page_id).await.unwrap().unwrap().content,
            REFRESH_BODY
        );

        let (status, body) = send_json(
            &app,
            "PUT",
            format!("/api/pages/{page_id}/review-required"),
            json!({ "required": false }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(
            !db.get_page(&page_id)
                .await
                .unwrap()
                .unwrap()
                .review_required
        );

        let (status, body) = send_json(
            &app,
            "PUT",
            "/api/pages/page_does_not_exist/review-required".to_string(),
            json!({ "required": true }),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

        let (status, _) = send_json(
            &app,
            "PUT",
            format!("/api/pages/{page_id}/review-required"),
            json!({ "required": true, "extra": 1 }),
        )
        .await;
        assert!(
            status.is_client_error(),
            "unknown fields are rejected, got {status}"
        );
    }

    #[tokio::test]
    async fn refresh_with_empty_sources_depends_on_whether_the_page_has_sources() {
        let _lock = crate::TEST_DATA_DIR_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        let _env = DataDirGuard::new();
        let (db, app, _tmp) = state_db_app().await;

        let sourced = seed_distilled_page(
            &db,
            "Rust Sourced Refresh",
            None,
            REFRESH_BODY,
            None,
            &REFRESH_SOURCES,
        )
        .await;
        let (status, body) = send_json(
            &app,
            "PUT",
            format!("/api/pages/{sourced}"),
            json!({ "content": format!("{REFRESH_BODY}, sources dropped"), "source_memory_ids": [] }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");

        let sourceless = wenlan_core::post_write::create_page(
            &db,
            CreateConceptRequest {
                title: "Rust Sourceless Notes".to_string(),
                content: REFRESH_BODY.to_string(),
                summary: None,
                entity_id: None,
                space: (None).into(),
                source_memory_ids: vec![],
                creation_kind: Some("research".to_string()),
                workspace: None,
            },
            "test",
            None,
        )
        .await
        .unwrap()
        .id;
        let content = format!("{REFRESH_BODY}, refreshed from the conversation");
        // `source_memory_ids` omitted entirely: serde default is an empty list.
        let (status, body) = send_json(
            &app,
            "PUT",
            format!("/api/pages/{sourceless}"),
            json!({ "content": content }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            db.get_page(&sourceless).await.unwrap().unwrap().content,
            content
        );
    }

    #[tokio::test]
    async fn create_without_kind_and_one_source_is_authored_and_not_counted_as_wiki() {
        let _lock = crate::TEST_DATA_DIR_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        let _env = DataDirGuard::new();
        let (state, _tmp) = build_state_with_db(3).await;
        let db = { state.read().await.db.as_ref().cloned().unwrap() };
        let telemetry_tmp = tempfile::tempdir().unwrap();
        let telemetry = Arc::new(crate::telemetry::Telemetry::for_test(
            telemetry_tmp.path(),
            "http://127.0.0.1:1/events",
            std::time::Duration::from_secs(3600),
        ));
        telemetry.set_enabled(true).unwrap();
        {
            state.write().await.telemetry = telemetry.clone();
        }
        let app = crate::router::build_router(state);
        let (status, body) = send_json(
            &app,
            "POST",
            "/api/pages".to_string(),
            json!({
                "title": "Rust Note From A Conversation",
                "content": "Rust ownership prevents memory safety bugs",
                "source_memory_ids": ["mem-page-floor-a"]
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let id = body["id"]
            .as_str()
            .expect("create returns an id")
            .to_string();
        let page = db.get_page(&id).await.unwrap().unwrap();
        assert_eq!(page.creation_kind, "authored");
        assert_eq!(
            telemetry.status().pending_operations,
            0,
            "an authored page is not a wiki generation"
        );
        drop(telemetry_tmp);
    }
}

#[cfg(test)]
mod conflict_identity_tests {
    use super::visible_conflict_identity;
    use crate::truth_guard::TruthView;
    use std::sync::Arc;
    use wenlan_core::db::MemoryDB;

    const PAGE: &str = "page_00000000-0000-4000-8000-0000000000c1";
    /// Stored scope of an unfiled publish: the M1 sentinel space id.
    const SCOPE: &str = "00000000-0000-4000-8000-000000000001";

    async fn test_db() -> (Arc<MemoryDB>, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let emitter: Arc<dyn wenlan_core::events::EventEmitter> =
            Arc::new(wenlan_core::events::NoopEmitter);
        let db = Arc::new(MemoryDB::new(tmp.path(), emitter).await.unwrap());
        (db, tmp)
    }

    async fn publish(db: &MemoryDB, id: &str, title: &str) -> wenlan_core::pages::Page {
        let draft = db
            .create_page_draft_with_id(id, title, "Body", None, None)
            .await
            .unwrap();
        match db.publish_page_draft(id, draft.version).await.unwrap() {
            wenlan_core::pages::PageDraftPublishOutcome::Published(page) => page,
            _ => panic!("expected a clean publish"),
        }
    }

    #[tokio::test]
    async fn a_visible_conflicting_page_discloses_its_current_identity() {
        let (db, _tmp) = test_db().await;
        let page = publish(&db, PAGE, "Secret Plan").await;
        let wanted = MemoryDB::page_title_key("  secret PLAN ");
        let got =
            visible_conflict_identity(&db, &TruthView::automatic(), PAGE, &wanted, SCOPE).await;
        assert_eq!(got, Some((page.id, page.title)));
    }

    #[tokio::test]
    async fn a_concurrently_deleted_page_degrades_to_omission() {
        let (db, _tmp) = test_db().await;
        publish(&db, PAGE, "Secret Plan").await;
        db.delete_page(PAGE).await.unwrap();
        let wanted = MemoryDB::page_title_key("Secret Plan");
        assert_eq!(
            visible_conflict_identity(&db, &TruthView::automatic(), PAGE, &wanted, SCOPE).await,
            None
        );
    }

    /// Replace the row's title out from under the open `MemoryDB`, the way a
    /// concurrent writer would between the publish transaction and the
    /// handler's reload. Same direct-file seam as the routes tests.
    async fn overwrite_title(tmp: &tempfile::TempDir, id: &str, title: &str) {
        let raw = libsql::Builder::new_local(tmp.path().join("origin_memory.db"))
            .build()
            .await
            .unwrap();
        let conn = raw.connect().unwrap();
        conn.execute(
            "UPDATE pages SET title = ?1 WHERE id = ?2",
            libsql::params![title, id],
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn a_replaced_row_with_an_unrelated_title_stays_omitted() {
        // Page ids are reusable and rows are mutable: between the publish
        // transaction and this reload the id can hold a different title. The
        // reloaded row no longer folds to the conflicting title, so neither
        // the stale captured title nor the new row's title may be disclosed.
        let (db, tmp) = test_db().await;
        publish(&db, PAGE, "Secret Plan").await;
        overwrite_title(&tmp, PAGE, "Innocuous Note").await;
        let wanted = MemoryDB::page_title_key("Secret Plan");
        assert_eq!(
            visible_conflict_identity(&db, &TruthView::automatic(), PAGE, &wanted, SCOPE).await,
            None
        );
    }

    #[tokio::test]
    async fn a_replaced_row_still_folding_to_the_conflict_discloses_the_reloaded_row() {
        let (db, tmp) = test_db().await;
        let page = publish(&db, PAGE, "Secret Plan").await;
        overwrite_title(&tmp, PAGE, "SECRET plan").await;
        let wanted = MemoryDB::page_title_key("Secret Plan");
        let got =
            visible_conflict_identity(&db, &TruthView::automatic(), PAGE, &wanted, SCOPE).await;
        // The identity comes from the reloaded row, never the stale capture.
        assert_eq!(got, Some((page.id, "SECRET plan".to_string())));
    }

    /// Same direct-file seam as `overwrite_title`, for an arbitrary column.
    async fn overwrite_column(tmp: &tempfile::TempDir, id: &str, column: &str, value: &str) {
        let raw = libsql::Builder::new_local(tmp.path().join("origin_memory.db"))
            .build()
            .await
            .unwrap();
        let conn = raw.connect().unwrap();
        conn.execute(
            &format!("UPDATE pages SET {column} = ?1 WHERE id = ?2"),
            libsql::params![value, id],
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn a_page_moved_to_another_space_degrades_to_omission() {
        // The publish found the conflict in one scope; if the page moves to a
        // different space before the handler reloads it, it is no longer that
        // conflict and its identity must not be disclosed.
        let (db, tmp) = test_db().await;
        publish(&db, PAGE, "Secret Plan").await;
        overwrite_column(&tmp, PAGE, "space", "another-space").await;
        let wanted = MemoryDB::page_title_key("Secret Plan");
        assert_eq!(
            visible_conflict_identity(&db, &TruthView::automatic(), PAGE, &wanted, SCOPE).await,
            None
        );
    }

    #[tokio::test]
    async fn an_entity_shadow_conflict_stays_omitted() {
        // Publish treats entity shadow pages as conflicts, but conflict
        // responses have never disclosed their identity; the snapshot must
        // keep that fence.
        let (db, tmp) = test_db().await;
        publish(&db, PAGE, "Secret Plan").await;
        overwrite_column(&tmp, PAGE, "kind", "entity").await;
        let wanted = MemoryDB::page_title_key("Secret Plan");
        assert_eq!(
            visible_conflict_identity(&db, &TruthView::automatic(), PAGE, &wanted, SCOPE).await,
            None
        );
    }
}

#[cfg(test)]
#[path = "page_routes/entity_page_guard_tests.rs"]
mod entity_page_guard_tests;

#[cfg(test)]
#[path = "page_routes/real_folder_page_routes_test.rs"]
mod real_folder_page_route_tests;

#[cfg(test)]
mod gated_code_basis_warning_tests {
    use super::*;
    use wenlan_core::code_anchor::{anchor, LineSpan};
    use wenlan_core::code_basis::PreparedCodeBasis;

    #[test]
    fn a_staged_write_warns_only_when_it_carried_code_basis() {
        assert!(gated_code_basis_warning(&[]).is_empty());
        let input = wenlan_types::CodeBasisInput {
            repo_root: "/repo".into(),
            path: "a.rs".into(),
            line_start: 1,
            line_end: 1,
        };
        let prepared = PreparedCodeBasis {
            anchor: anchor("one\n", LineSpan { start: 1, end: 1 }).unwrap(),
            input,
            stamp: None,
        };
        let warnings = gated_code_basis_warning(&[prepared]);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("code_basis was not recorded"));
        assert!(warnings[0].contains("accepted first"));
        assert!(warnings[0].contains("re-sent with code_basis"));
    }
}
