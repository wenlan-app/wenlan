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
    let result = publish_space(&db, &space).await?;
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

/// Publish the bound Space `space`: used by the route and by the daily run.
pub(crate) async fn publish_space(
    db: &MemoryDB,
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
    let result = blocking(move || {
        let result = repo_wiki::publish(repo_wiki::PublishInput {
            binding: &binding,
            pages,
            left_out,
            code_bases: &code_bases,
            anchors: &anchors,
            concept_paths: &concept_paths,
            worktree: repo_wiki::worktree_for(&space),
            gh: repo_wiki::find_gh(),
            now,
        })?;
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
