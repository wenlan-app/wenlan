// SPDX-License-Identifier: Apache-2.0
//! A Space bound to a git repository, and putting its code pages into that
//! repository as a pull request.
//!
//! - `GET /api/spaces/{name}/wiki/repo`: the binding, or `null`.
//! - `PUT /api/spaces/{name}/wiki/repo`: bind, or change folder and daily run.
//! - `DELETE /api/spaces/{name}/wiki/repo`: unbind.
//! - `GET /api/spaces/{name}/wiki/suggest`: repositories the Space's code
//!   pages could go into, for the app to offer binding.
//! - `POST /api/spaces/{name}/wiki/publish`: write the pages and open or
//!   update the pull request.
//! - `POST /api/spaces/{name}/wiki/run`: start the daily run now, in the
//!   background (the user's AI tool, then a publish).
//!
//! The work lives in `wenlan_core::repo_wiki`; git and gh run on a blocking
//! thread.

use crate::error::ServerError;
use crate::route_registry::{get, post, TrackedRouter};
use crate::state::{ServerState, SharedState};
use axum::{
    extract::{Path, State},
    response::Json,
};
use std::sync::Arc;
use tokio::sync::RwLock;
use wenlan_core::db::MemoryDB;
use wenlan_core::repo_wiki;
use wenlan_types::repo_wiki::{
    BindRepoWikiRequest, RepoWikiBinding, RepoWikiPublishResult, RepoWikiSuggestion,
};

pub(crate) fn register(router: TrackedRouter<SharedState>) -> TrackedRouter<SharedState> {
    router
        .route(
            "/api/spaces/{name}/wiki/repo",
            get(handle_get_binding)
                .put(handle_bind)
                .delete(handle_unbind),
        )
        .route("/api/spaces/{name}/wiki/suggest", get(handle_suggest))
        .route("/api/spaces/{name}/wiki/publish", post(handle_publish))
        .route("/api/spaces/{name}/wiki/run", post(handle_run_now))
}

async fn db_of(state: &Arc<RwLock<ServerState>>) -> Result<Arc<MemoryDB>, ServerError> {
    let s = state.read().await;
    s.db.clone().ok_or(ServerError::DbNotInitialized)
}

/// The registered Space named in the path.
async fn registered_space(db: &MemoryDB, space: &str) -> Result<String, ServerError> {
    db.registered_space_or_none(Some(space))
        .await
        .map_err(|e| ServerError::Internal(e.to_string()))?
        .ok_or_else(|| ServerError::NotFound(format!("no Space named {space:?}")))
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, wenlan_core::WenlanError> + Send + 'static,
) -> Result<T, ServerError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| ServerError::Internal(format!("repo wiki task failed: {e}")))?
        .map_err(ServerError::from)
}

/// GET /api/spaces/{name}/wiki/repo
async fn handle_get_binding(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(space): Path<String>,
) -> Result<Json<Option<RepoWikiBinding>>, ServerError> {
    let db = db_of(&state).await?;
    let space = registered_space(&db, &space).await?;
    let binding = blocking(move || repo_wiki::binding_at(&repo_wiki::store_path(), &space)).await?;
    Ok(Json(binding))
}

/// PUT /api/spaces/{name}/wiki/repo
async fn handle_bind(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(space): Path<String>,
    Json(req): Json<BindRepoWikiRequest>,
) -> Result<Json<RepoWikiBinding>, ServerError> {
    let db = db_of(&state).await?;
    let space = registered_space(&db, &space).await?;
    let repo_path = std::path::PathBuf::from(req.repo_root.trim());
    if !repo_path.is_absolute() {
        return Err(ServerError::ValidationError(format!(
            "repo_root must be an absolute path (got {:?})",
            req.repo_root
        )));
    }
    let binding = blocking(move || {
        repo_wiki::bind_at(
            &repo_wiki::store_path(),
            &space,
            &repo_path,
            req.folder.as_deref(),
            req.daily,
        )
    })
    .await?;
    Ok(Json(binding))
}

/// DELETE /api/spaces/{name}/wiki/repo
async fn handle_unbind(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(space): Path<String>,
) -> Result<Json<serde_json::Value>, ServerError> {
    let db = db_of(&state).await?;
    let space = registered_space(&db, &space).await?;
    let removed = blocking(move || {
        let store = repo_wiki::store_path();
        let binding = repo_wiki::binding_at(&store, &space)?;
        let removed = repo_wiki::unbind_at(&store, &space)?;
        if let Some(binding) = binding {
            repo_wiki::remove_worktree(&binding.repo_root, &repo_wiki::worktree_for(&space));
        }
        Ok(removed)
    })
    .await?;
    Ok(Json(serde_json::json!({ "removed": removed })))
}

/// GET /api/spaces/{name}/wiki/suggest
async fn handle_suggest(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(space): Path<String>,
) -> Result<Json<RepoWikiSuggestion>, ServerError> {
    let db = db_of(&state).await?;
    let space = registered_space(&db, &space).await?;
    let pages = exportable_pages(&db, &space).await?;
    let code_bases = db
        .code_basis_locators_by_page()
        .await
        .map_err(|e| ServerError::Internal(e.to_string()))?;
    let candidates = blocking(move || Ok(repo_wiki::candidates(&pages, &code_bases))).await?;
    Ok(Json(RepoWikiSuggestion { candidates }))
}

/// POST /api/spaces/{name}/wiki/publish
async fn handle_publish(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(space): Path<String>,
) -> Result<Json<RepoWikiPublishResult>, ServerError> {
    let db = db_of(&state).await?;
    let space = registered_space(&db, &space).await?;
    let page_root = page_root_of(&state).await;
    let result = publish_space(&db, page_root.as_deref(), &space).await?;
    Ok(Json(result))
}

/// POST /api/spaces/{name}/wiki/run
async fn handle_run_now(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(space): Path<String>,
) -> Result<Json<serde_json::Value>, ServerError> {
    let db = db_of(&state).await?;
    let space = registered_space(&db, &space).await?;
    let lookup = space.clone();
    let binding = blocking(move || repo_wiki::binding_at(&repo_wiki::store_path(), &lookup))
        .await?
        .ok_or_else(|| {
            ServerError::NotFound(format!(
                "Space {space:?} is not bound to a repository; bind it first"
            ))
        })?;
    if crate::repo_wiki_daily::is_running(&binding.space) {
        return Ok(Json(
            serde_json::json!({ "started": false, "reason": "already running" }),
        ));
    }
    let shared = state.clone();
    tokio::spawn(async move {
        match crate::repo_wiki_daily::run_once(&shared, &binding).await {
            Ok(result) => tracing::info!(
                "[repo_wiki] run for {} finished: {:?}",
                binding.space,
                result.status
            ),
            Err(error) => tracing::warn!("[repo_wiki] run for {} failed: {error}", binding.space),
        }
    });
    Ok(Json(serde_json::json!({ "started": true })))
}

/// The folder the Space's page files are mirrored in, for page writes.
pub(crate) async fn page_root_of(state: &Arc<RwLock<ServerState>>) -> Option<std::path::PathBuf> {
    let s = state.read().await;
    s.lint_config.page_root().map(std::path::Path::to_path_buf)
}

/// What [`bring_back`] did, for the publish that follows.
#[derive(Debug, Default)]
pub(crate) struct BroughtBack {
    /// Page files the Space now matches: the publish may replace them.
    pub adopt: Vec<String>,
    /// Page files that wait for the user: the publish leaves them as the
    /// repository has them.
    pub keep_remote: Vec<String>,
    pub brought_in: usize,
    pub to_review: usize,
}

/// The name a change from the repository carries on the page's history and
/// on a revision card.
const FROM_REPOSITORY: &str = "GitHub";

/// Take into the Space the pages changed in the repository since Wenlan last
/// wrote them: a merged `wenlan wiki refresh` pull request, or an edit made
/// on GitHub. A page Wenlan wrote takes the change directly; a page the user
/// edited or locked gets it as a revision card, once, and the repository
/// keeps its version until the card is decided. After that the Space's page
/// is what goes out again, whichever way the card went.
pub(crate) async fn bring_back(
    db: &MemoryDB,
    page_root: Option<&std::path::Path>,
    binding: &RepoWikiBinding,
) -> Result<BroughtBack, ServerError> {
    let lookup = binding.clone();
    let worktree = repo_wiki::worktree_for(&binding.space);
    let edits = blocking(move || repo_wiki::remote_edits(&lookup, &worktree)).await?;
    let mut out = BroughtBack::default();
    if edits.is_empty() {
        return Ok(out);
    }
    let pending: std::collections::HashSet<String> = db
        .list_pending_revisions(1000)
        .await
        .map_err(|e| ServerError::Internal(e.to_string()))?
        .into_iter()
        .filter(|card| card.source_agent.as_deref() == Some(FROM_REPOSITORY))
        .map(|card| card.target_source_id)
        .collect();
    let offered_path = repo_wiki::offered_path(&binding.space);
    let previously_offered = {
        let path = offered_path.clone();
        blocking(move || Ok(repo_wiki::load_offered(&path))).await?
    };
    let mut offered = std::collections::BTreeMap::new();
    for edit in edits {
        let Some(page) = db
            .get_page(&edit.page_id)
            .await
            .map_err(|e| ServerError::Internal(e.to_string()))?
        else {
            continue;
        };
        if page.content.trim() == edit.body.trim() {
            out.adopt.push(edit.file);
            continue;
        }
        if pending.contains(&page.id) {
            offered.insert(edit.file.clone(), edit.digest.clone());
            out.keep_remote.push(edit.file);
            continue;
        }
        if previously_offered.get(&edit.file) == Some(&edit.digest) {
            // Offered before and decided: the Space's page goes out.
            out.adopt.push(edit.file);
            continue;
        }
        let owned = page.review_required || page.user_edited;
        let source_revision = db
            .get_page_source_revision(&page.id)
            .await
            .map_err(|e| ServerError::Internal(e.to_string()))?;
        let written = wenlan_core::post_write::steep_update_page(
            db,
            &page.id,
            wenlan_types::requests::UpdatePageRequest {
                content: edit.body.clone(),
                source_memory_ids: page.source_memory_ids.clone(),
                expected_version: Some(page.version),
                caller_id: None,
                operation_id: None,
            },
            Some(FROM_REPOSITORY),
            None,
            source_revision,
            Some(owned),
            page_root,
            None,
        )
        .await;
        match written {
            Ok(result) if result.gated => {
                offered.insert(edit.file.clone(), edit.digest.clone());
                out.keep_remote.push(edit.file);
                out.to_review += 1;
            }
            Ok(result) if result.wrote => {
                if let Err(error) = settle_code(db, binding, &page.id, &edit.spans).await {
                    tracing::warn!(
                        "[repo_wiki] {} came back from the repository; code lines not settled: {error}",
                        page.id
                    );
                }
                out.adopt.push(edit.file);
                out.brought_in += 1;
            }
            Ok(_) => out.adopt.push(edit.file),
            Err(error) => {
                tracing::warn!(
                    "[repo_wiki] could not bring {} back from the repository: {error}",
                    page.id
                );
                out.keep_remote.push(edit.file);
            }
        }
    }
    blocking(move || repo_wiki::save_offered(&offered_path, &offered)).await?;
    Ok(out)
}

/// After a page came back from the repository: make its code lines in this
/// repository the ones the repository's version names, when the working copy
/// holds those files as the default branch does. Otherwise the rows stay,
/// and the usual check settles them once the user pulls.
async fn settle_code(
    db: &MemoryDB,
    binding: &RepoWikiBinding,
    page_id: &str,
    spans: &[(String, u32, u32)],
) -> Result<(), String> {
    use wenlan_types::code_basis::{CodeBasisInput, CodeBasisLocator};
    let repo_root = binding.repo_root.clone();
    let paths: Vec<String> = spans.iter().map(|(path, _, _)| path.clone()).collect();
    let in_step = blocking(move || {
        Ok(paths
            .iter()
            .all(|path| repo_wiki::working_copy_matches_default_branch(&repo_root, path)))
    })
    .await
    .map_err(|e| format!("{e:?}"))?;
    if !in_step {
        return Ok(());
    }
    let inputs: Vec<CodeBasisInput> = spans
        .iter()
        .map(|(path, start, end)| CodeBasisInput {
            repo_root: binding.repo_root.clone(),
            path: path.clone(),
            line_start: *start,
            line_end: *end,
        })
        .collect();
    let prepared = wenlan_core::code_basis::prepare_code_basis(&inputs)
        .await
        .map_err(|e| e.to_string())?;
    let keep: std::collections::HashSet<(String, u32, u32)> = spans.iter().cloned().collect();
    for row in db
        .list_page_code_basis(page_id)
        .await
        .map_err(|e| e.to_string())?
    {
        let locator = CodeBasisLocator::from(&row);
        if locator.repo_root.trim_end_matches('/') == binding.repo_root.trim_end_matches('/')
            && !keep.contains(&(locator.path.clone(), locator.line_start, locator.line_end))
        {
            db.remove_code_basis(page_id, &locator)
                .await
                .map_err(|e| e.to_string())?;
        }
    }
    db.upsert_page_code_basis(page_id, &prepared, FROM_REPOSITORY)
        .await
        .map_err(|e| e.to_string())
}

/// Publish the bound Space `space`: used by the route and by the daily run.
/// Pages changed in the repository come back into the Space first.
pub(crate) async fn publish_space(
    db: &MemoryDB,
    page_root: Option<&std::path::Path>,
    space: &str,
) -> Result<RepoWikiPublishResult, ServerError> {
    let store = repo_wiki::store_path();
    let lookup_space = space.to_owned();
    let lookup_store = store.clone();
    let binding = blocking(move || repo_wiki::binding_at(&lookup_store, &lookup_space))
        .await?
        .ok_or_else(|| {
            ServerError::NotFound(format!(
                "Space {space:?} is not bound to a repository; bind it first"
            ))
        })?;

    let brought = bring_back(db, page_root, &binding).await?;
    let pages = exportable_pages(db, space).await?;
    let code_bases = db
        .code_basis_locators_by_page()
        .await
        .map_err(|e| ServerError::Internal(e.to_string()))?;
    let concept_paths = wenlan_core::export::okf::concept_path_map(
        &db.okf_concept_paths()
            .await
            .map_err(|e| ServerError::Internal(e.to_string()))?,
    );
    let anchors: repo_wiki::CodeAnchors = db
        .list_code_basis_rows(Some(space))
        .await
        .map_err(|e| ServerError::Internal(e.to_string()))?
        .into_iter()
        .map(|row| {
            let locator = wenlan_types::code_basis::CodeBasisLocator::from(&row.reference);
            (repo_wiki::anchor_key(&row.page_id, &locator), row.anchor)
        })
        .collect();
    let (pages, left_out) = repo_wiki::select_pages(pages, &code_bases, &binding.repo_root);
    let now = chrono::Utc::now().timestamp();
    let space = space.to_owned();
    let (brought_in, to_review) = (brought.brought_in, brought.to_review);
    let result = blocking(move || {
        let mut result = repo_wiki::publish(repo_wiki::PublishInput {
            binding: &binding,
            pages,
            left_out,
            code_bases: &code_bases,
            anchors: &anchors,
            concept_paths: &concept_paths,
            worktree: repo_wiki::worktree_for(&space),
            gh: repo_wiki::find_gh(),
            now,
            adopt: brought.adopt,
            keep_remote: brought.keep_remote,
        })?;
        result.brought_in = brought_in;
        result.to_review = to_review;
        repo_wiki::record_publish_at(&store, &space, &result)?;
        Ok(result)
    })
    .await?;
    tracing::info!(
        "[repo_wiki] published {} page(s): {:?} {}",
        result.pages,
        result.status,
        result.pr_url.as_deref().unwrap_or("")
    );
    Ok(result)
}

/// Every active page of `space` the automatic reader may see, the same set
/// the OKF export writes. Refuses while a truth cutover is preparing, since
/// the exporter would otherwise delete every page file of the last publish.
async fn exportable_pages(
    db: &MemoryDB,
    space: &str,
) -> Result<Vec<wenlan_core::pages::Page>, ServerError> {
    let scope = crate::read_scope::effective_read_scope(db, Some(space), None).await?;
    let fence_before = db.cutover_fence().await?;
    let pages = db
        .list_pages_scoped("active", i64::MAX, 0, &scope)
        .await
        .map_err(|e| ServerError::Internal(e.to_string()))?;
    let mut exportable = Vec::with_capacity(pages.len());
    for page in pages {
        if wenlan_core::truth_adapter::page_write_permit(db, &page.id)
            .await
            .map_err(|e| ServerError::Internal(e.to_string()))?
            .is_some()
        {
            exportable.push(page);
        }
    }
    let fence_after = db.cutover_fence().await?;
    if fence_before.phase == wenlan_core::db::CutoverPhase::Preparing || fence_after != fence_before
    {
        return Err(wenlan_core::WenlanError::Conflict(
            "a truth cutover is in progress; publish the wiki after it finishes".to_string(),
        )
        .into());
    }
    Ok(exportable)
}
