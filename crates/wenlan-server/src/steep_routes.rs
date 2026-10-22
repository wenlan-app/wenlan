// SPDX-License-Identifier: Apache-2.0
//! Routes for **steep on demand**: the caller's AI tool lists the notes
//! waiting for steep, claims one, and commits its rewrite.
//!
//! The protocol and its checks live in `wenlan_core::steep`. This module frames
//! them as HTTP and decides what the caller may see: a note outside its Space
//! or truth grant is left out of the list and answers a claim as missing.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;
use tokio::sync::RwLock;

use wenlan_core::db::MemoryDB;
use wenlan_core::steep::SteepError;
use wenlan_types::{
    SteepClaimRequest, SteepClaimResponse, SteepCommitRequest, SteepCommitResponse,
    SteepWorklistResponse,
};

use crate::error::ServerError;
use crate::page_basis_routes::visible_ids;
use crate::route_registry::{get, post, TrackedRouter};
use crate::state::{ServerState, SharedState};

pub(crate) fn register(router: TrackedRouter<SharedState>) -> TrackedRouter<SharedState> {
    router
        .route("/api/steep/worklist", get(handle_steep_worklist))
        .route("/api/steep/claim", post(handle_steep_claim))
        .route("/api/steep/commit", post(handle_steep_commit))
}

/// 404 for a note the caller cannot have, 409 for one it cannot have right
/// now, 422 for a rewrite the citation check refused (the hold is kept, so
/// the caller fixes the text and commits again), 400 for a malformed ask.
/// Steep's own refusals lead with their code (`steep_changed: ...`) so the
/// MCP client can hand the reason to the agent, which acts on it.
fn steep_error(error: SteepError) -> ServerError {
    let message = match error.code() {
        Some(code) => format!("{code}: {error}"),
        None => error.to_string(),
    };
    match error {
        SteepError::NotFound => ServerError::NotFound("page not found".into()),
        SteepError::NotWaiting(_)
        | SteepError::Held(_)
        | SteepError::Changed(_)
        | SteepError::TokenGone => ServerError::Conflict(message),
        SteepError::CitationGate(_) => ServerError::ValidationError(message),
        SteepError::Invalid(_) => ServerError::BadRequest(message),
        SteepError::Db(inner) => ServerError::from(inner),
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
pub struct SteepWorklistQuery {
    #[serde(default)]
    space: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    cursor: Option<String>,
}

/// GET /api/steep/worklist?space=&limit=&cursor=
///
/// The notes waiting for steep that the caller may see, newest change first,
/// with the full count. Needs no model.
pub async fn handle_steep_worklist(
    State(state): State<Arc<RwLock<ServerState>>>,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    view: crate::truth_guard::TruthView,
    Query(query): Query<SteepWorklistQuery>,
) -> Result<Json<SteepWorklistResponse>, ServerError> {
    let db = open_db(&state).await?;
    let scope = crate::read_scope::effective_read_scope(
        &db,
        query.space.as_deref(),
        header_space.as_deref(),
    )
    .await?;
    let waiting = wenlan_core::steep::waiting_page_ids(&db).await?;
    let visible = visible_ids(&db, &view.grant, &scope, &waiting).await?;
    let list = wenlan_core::steep::list_work(
        &db,
        &scope,
        |id| visible.contains(id),
        query.limit.unwrap_or(20),
        query.cursor.as_deref(),
    )
    .await?;
    Ok(Json(SteepWorklistResponse {
        items: list.items,
        total: list.total,
        next_cursor: list.next_cursor,
    }))
}

/// POST /api/steep/claim
///
/// Snapshot one note and its surviving sources, numbered, and hold it for
/// this caller. The plan says whether to rewrite it, stage the rewrite as a
/// revision card, or leave it to the user.
pub async fn handle_steep_claim(
    State(state): State<Arc<RwLock<ServerState>>>,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    view: crate::truth_guard::TruthView,
    Json(req): Json<SteepClaimRequest>,
) -> Result<Json<SteepClaimResponse>, ServerError> {
    let db = open_db(&state).await?;
    let scope = crate::read_scope::effective_read_scope(&db, None, header_space.as_deref()).await?;
    // The claim hands out the note and its sources in full: a note the caller
    // may not read in full is not there for it.
    let visible = visible_ids(&db, &view.grant, &scope, std::slice::from_ref(&req.page_id)).await?;
    if !visible.contains(&req.page_id) {
        return Err(ServerError::NotFound("page not found".into()));
    }
    let claim = wenlan_core::steep::claim(&db, &req.page_id, &scope)
        .await
        .map_err(steep_error)?;
    Ok(Json(SteepClaimResponse {
        page_id: claim.page_id,
        title: claim.title,
        plan: claim.plan.to_string(),
        token: claim.token,
        held_until: claim.held_until,
        version: claim.version,
        stale_reason: claim.stale_reason,
        current_content: claim.current_content,
        sources: claim.sources,
        note: claim.note,
    }))
}

/// POST /api/steep/commit
///
/// Settle a claimed note with the claim's token: write the rewrite (or stage
/// it as a revision card), record that the note still holds, or give it back.
/// Refused with nothing written when the note or its sources moved since the
/// claim. The same token twice returns the first result.
pub async fn handle_steep_commit(
    State(state): State<Arc<RwLock<ServerState>>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<SteepCommitRequest>,
) -> Result<Json<SteepCommitResponse>, ServerError> {
    let (db, page_root) = {
        let s = state.read().await;
        (
            s.db.clone().ok_or(ServerError::DbNotInitialized)?,
            s.lint_config.page_root().map(std::path::Path::to_path_buf),
        )
    };
    // The tool committing is named on the page's history and on a revision
    // card, as for write_page. Only the header counts.
    let agent = headers
        .get("x-agent-name")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let session = crate::session_routes::extract_session_ref(&headers);
    let write_started_at = chrono::Utc::now().timestamp();
    let done = wenlan_core::steep::commit(
        &db,
        &req.token,
        &req.action,
        req.content.as_deref(),
        page_root.as_deref(),
        agent.as_deref(),
    )
    .await
    .map_err(steep_error)?;
    // The conversation behind the rewrite, recorded on the version it wrote.
    // Best effort, as for write_page: the page is already written.
    if let (Some(session), "written", false) = (&session, done.outcome, done.replayed) {
        let space = db
            .get_page(&done.page_id)
            .await
            .ok()
            .flatten()
            .and_then(|p| p.space);
        if let Err(error) = db
            .stamp_page_history_session(&done.page_id, write_started_at, session, space.as_deref())
            .await
        {
            tracing::warn!(
                "[session] failed to stamp page {} with {session}: {error}",
                done.page_id
            );
        }
    }
    Ok(Json(SteepCommitResponse {
        page_id: done.page_id,
        outcome: done.outcome.to_string(),
        revision_card_id: done.revision_card_id,
        citations: done.citations,
        replayed: done.replayed,
    }))
}
