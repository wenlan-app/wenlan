// SPDX-License-Identifier: Apache-2.0
//! Routes for a note that relies on lines of code (its **code basis**), and the
//! shared helpers the page routes use to put a note's code bases on the wire.
//!
//! The rules (what a path may be, how a row moves between `ok`, `moved`,
//! `pending` and `unavailable`) live in `wenlan_core::code_basis` and
//! `wenlan_core::db::code_basis`. This module frames them as HTTP. Reads of the
//! files happen in core on the blocking pool; no state lock is held while a
//! file is read.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::Json;
use serde::Deserialize;
use tokio::sync::RwLock;

use wenlan_core::code_basis::CodeBasisError;
use wenlan_core::db::MemoryDB;
use wenlan_types::{BasisRemoveResponse, CodeBasisLocator, CodeBasisRecheckEntry, CodeBasisRef};

use crate::error::ServerError;
use crate::page_basis_routes::{actor_from, visible_ids, MUTATION_GRANT};
use crate::route_registry::{delete, post, TrackedRouter};
use crate::state::{ServerState, SharedState};

pub(crate) fn register(router: TrackedRouter<SharedState>) -> TrackedRouter<SharedState> {
    router
        .route(
            "/api/pages/code-basis/recheck",
            post(handle_code_basis_recheck),
        )
        .route(
            "/api/pages/{id}/code-basis/confirm",
            post(handle_confirm_code_basis),
        )
        .route(
            "/api/pages/{id}/code-basis",
            delete(handle_remove_code_basis),
        )
}

/// Map a code-basis refusal onto the HTTP error its kind implies: 400 for a
/// request that can never name a code basis (path out of the repository, span
/// outside the file, a file that cannot be read when a note is written), 404
/// for a row or note that is not there, 409 for one that cannot be settled
/// right now.
pub(crate) fn code_basis_error(error: CodeBasisError) -> ServerError {
    match error {
        // The stable prefix lets an MCP client tell a bad `code_basis` entry from
        // any other 400 without the daemon's free text crossing to the agent.
        CodeBasisError::Invalid(message) => {
            ServerError::BadRequest(format!("code_basis_invalid: {message}"))
        }
        CodeBasisError::NotFound => ServerError::NotFound(error.to_string()),
        CodeBasisError::Conflict(message) => ServerError::Conflict(message),
        CodeBasisError::Db(inner) => ServerError::from(inner),
    }
}

/// The non-ok code bases of many notes (a list or a recall result), keyed by
/// note. Notes with nothing to look at are absent. A code basis names no other
/// note, so there is no title to withhold: visibility of the listed notes was
/// already decided by the caller.
pub(crate) async fn non_ok_code_basis_by_page(
    db: &MemoryDB,
    page_ids: &[String],
) -> Result<HashMap<String, Vec<CodeBasisRef>>, ServerError> {
    if page_ids.is_empty() {
        return Ok(HashMap::new());
    }
    Ok(db.list_non_ok_code_basis(page_ids).await?)
}

/// Bring the code bases of the listed scope up to date before a list shows
/// their states, so a note whose cited code changed is not listed as current.
/// Rows whose file still shows the stamp they were last read with are skipped,
/// so a quiet scope costs one query and a `stat` per file. A failed check
/// leaves the list readable with the states as last recorded.
pub(crate) async fn recheck_for_list(db: &MemoryDB, scope: &wenlan_core::read_scope::ReadScope) {
    if let Err(error) = wenlan_core::code_basis::recheck_code_basis_for_scope(db, scope).await {
        tracing::warn!("[pages] code basis check failed for the list: {error}");
    }
}

async fn open_db(state: &Arc<RwLock<ServerState>>) -> Result<Arc<MemoryDB>, ServerError> {
    state
        .read()
        .await
        .db
        .clone()
        .ok_or(ServerError::DbNotInitialized)
}

#[derive(Debug, Deserialize)]
pub struct CodeBasisRecheckQuery {
    #[serde(default)]
    space: Option<String>,
}

/// POST /api/pages/code-basis/recheck?space=
///
/// Re-read the files behind every code basis in the Space, move each row to
/// the state its file now justifies, and return the notes that have a
/// `pending` or `unavailable` one. Needs no model.
pub async fn handle_code_basis_recheck(
    State(state): State<Arc<RwLock<ServerState>>>,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    view: crate::truth_guard::TruthView,
    Query(query): Query<CodeBasisRecheckQuery>,
) -> Result<Json<Vec<CodeBasisRecheckEntry>>, ServerError> {
    let db = open_db(&state).await?;
    let scope = crate::read_scope::effective_read_scope(
        &db,
        query.space.as_deref(),
        header_space.as_deref(),
    )
    .await?;
    let entries = wenlan_core::code_basis::recheck_code_basis_in_scope(&db, &scope).await?;
    // The report names notes and carries their titles: a note the caller may
    // not read in full is left out.
    let ids: Vec<String> = entries.iter().map(|e| e.page_id.clone()).collect();
    let visible = visible_ids(&db, &view.grant, &scope, &ids).await?;
    Ok(Json(
        entries
            .into_iter()
            .filter(|entry| visible.contains(&entry.page_id))
            .collect(),
    ))
}

/// POST /api/pages/{id}/code-basis/confirm
///
/// "I have accounted for the code as it reads now." The file is read again,
/// the lines are found where they are now, the anchor is recomputed, and the
/// row returns to `ok`. 404 when the note has no such row; 409 when the file
/// cannot be read or the lines cannot be located.
pub async fn handle_confirm_code_basis(
    State(state): State<Arc<RwLock<ServerState>>>,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(locator): Json<CodeBasisLocator>,
) -> Result<Json<CodeBasisRef>, ServerError> {
    let db = open_db(&state).await?;
    let scope = crate::read_scope::effective_read_scope(&db, None, header_space.as_deref()).await?;
    require_page_visible(&db, &scope, &id).await?;
    let confirmed =
        wenlan_core::code_basis::confirm_code_basis(&db, &id, &locator, &actor_from(&headers))
            .await
            .map_err(code_basis_error)?;
    Ok(Json(confirmed))
}

/// DELETE /api/pages/{id}/code-basis
///
/// Stop relying on the lines the body names.
pub async fn handle_remove_code_basis(
    State(state): State<Arc<RwLock<ServerState>>>,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    Path(id): Path<String>,
    Json(locator): Json<CodeBasisLocator>,
) -> Result<Json<BasisRemoveResponse>, ServerError> {
    let db = open_db(&state).await?;
    let scope = crate::read_scope::effective_read_scope(&db, None, header_space.as_deref()).await?;
    require_page_visible(&db, &scope, &id).await?;
    let removed = db.remove_code_basis(&id, &locator).await?;
    Ok(Json(BasisRemoveResponse { removed }))
}

/// The note must be visible to the caller, or it reads as absent.
async fn require_page_visible(
    db: &MemoryDB,
    scope: &wenlan_core::read_scope::ReadScope,
    id: &str,
) -> Result<(), ServerError> {
    let visible = visible_ids(db, &MUTATION_GRANT, scope, &[id.to_string()]).await?;
    if visible.contains(id) {
        Ok(())
    } else {
        Err(ServerError::NotFound(format!("page {id} not found")))
    }
}
