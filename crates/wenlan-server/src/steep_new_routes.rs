// SPDX-License-Identifier: Apache-2.0
//! Routes for **steep's new pages**: the caller's AI tool lists the material
//! no note uses yet, claims one topic's items, and commits the note it wrote.
//!
//! The protocol and its checks live in `wenlan_core::steep_new`. This module
//! frames them as HTTP. A Space-locked caller sees and claims only material in
//! its Space, and its note is born there.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;
use tokio::sync::RwLock;

use wenlan_types::steep_new::{STEEP_NEW_ACTION_RELEASE, STEEP_NEW_ACTION_WRITE};
use wenlan_types::{
    SteepNewClaimRequest, SteepNewClaimResponse, SteepNewCommitRequest, SteepNewCommitResponse,
    SteepNewListResponse,
};

use crate::error::ServerError;
use crate::route_registry::{get, post, TrackedRouter};
use crate::state::{ServerState, SharedState};
use crate::steep_routes::steep_error;

pub(crate) fn register(router: TrackedRouter<SharedState>) -> TrackedRouter<SharedState> {
    router
        .route("/api/steep/new", get(handle_steep_new_list))
        .route("/api/steep/new/claim", post(handle_steep_new_claim))
        .route("/api/steep/new/commit", post(handle_steep_new_commit))
}

/// What the claim tells the caller to do with the sources.
pub const NEW_PAGE_NOTE: &str = "Write one note from these sources with your main model. \
Give it a short title that names the topic. Open with one plain sentence that says what \
the note is about, then a few short paragraphs or lists. Use only what the numbered \
sources say and cite every claim with its number, like [2]. Write in the language of the \
sources. Then commit with action \"write\", the token, the title and the body. If the \
sources do not belong together, commit with action \"release\" instead.";

#[derive(Debug, Deserialize)]
pub struct SteepNewListQuery {
    #[serde(default)]
    space: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    cursor: Option<String>,
}

/// GET /api/steep/new?space=&limit=&cursor=
///
/// Material no note uses yet, one item per note, chat, file or capture,
/// newest first, with the full count. Needs no model.
pub async fn handle_steep_new_list(
    State(state): State<Arc<RwLock<ServerState>>>,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    Query(query): Query<SteepNewListQuery>,
) -> Result<Json<SteepNewListResponse>, ServerError> {
    let db = state
        .read()
        .await
        .db
        .clone()
        .ok_or(ServerError::DbNotInitialized)?;
    let scope = crate::read_scope::effective_read_scope(
        &db,
        query.space.as_deref(),
        header_space.as_deref(),
    )
    .await?;
    let list = wenlan_core::steep_new::list_new(
        &db,
        &scope,
        query.limit.unwrap_or(100),
        query.cursor.as_deref(),
    )
    .await?;
    Ok(Json(SteepNewListResponse {
        items: list.items,
        total: u32::try_from(list.total).unwrap_or(u32::MAX),
        next_cursor: list.next_cursor,
        kinds: list.kinds,
        held: u32::try_from(list.held).unwrap_or(u32::MAX),
    }))
}

/// POST /api/steep/new/claim
///
/// Hold one topic's items for one note and hand out their text, numbered.
pub async fn handle_steep_new_claim(
    State(state): State<Arc<RwLock<ServerState>>>,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    Json(req): Json<SteepNewClaimRequest>,
) -> Result<Json<SteepNewClaimResponse>, ServerError> {
    let db = state
        .read()
        .await
        .db
        .clone()
        .ok_or(ServerError::DbNotInitialized)?;
    let scope = crate::read_scope::effective_read_scope(&db, None, header_space.as_deref()).await?;
    let claim = wenlan_core::steep_new::claim_new(&db, &scope, &req.item_ids)
        .await
        .map_err(steep_error)?;
    Ok(Json(SteepNewClaimResponse {
        token: claim.token,
        held_until: claim.held_until,
        sources: claim.sources,
        skipped: claim.skipped,
        note: NEW_PAGE_NOTE.to_string(),
    }))
}

/// POST /api/steep/new/commit
///
/// Write the note a claim was for, or give the items back.
pub async fn handle_steep_new_commit(
    State(state): State<Arc<RwLock<ServerState>>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<SteepNewCommitRequest>,
) -> Result<Json<SteepNewCommitResponse>, ServerError> {
    let (db, page_root) = {
        let s = state.read().await;
        (
            s.db.clone().ok_or(ServerError::DbNotInitialized)?,
            s.lint_config.page_root().map(std::path::Path::to_path_buf),
        )
    };
    let done = match req.action.as_str() {
        STEEP_NEW_ACTION_RELEASE => {
            wenlan_core::steep_new::release_new(&req.token).map_err(steep_error)?
        }
        STEEP_NEW_ACTION_WRITE => {
            let named = headers
                .get("x-agent-name")
                .and_then(|v| v.to_str().ok())
                .map(str::trim)
                .filter(|s| !s.is_empty());
            // The tool writing the note is registered like a memory writer,
            // so its recall sees the notes it wrote.
            crate::agent_presence::note_page_writer(&db, named).await?;
            let agent = named.unwrap_or("steep").to_string();
            let session = crate::session_routes::extract_session_ref(&headers);
            let write_started_at = chrono::Utc::now().timestamp();
            let done = wenlan_core::steep_new::commit_new(
                &db,
                &req.token,
                req.title.as_deref().unwrap_or(""),
                req.content.as_deref().unwrap_or(""),
                &agent,
                page_root.as_deref(),
            )
            .await
            .map_err(steep_error)?;
            // The first note an AI tool writes from the user's material is
            // the first-page moment, as when the local model writes it.
            if done.outcome == "created" {
                let evaluator = wenlan_core::onboarding::MilestoneEvaluator::new(
                    &db,
                    Arc::new(wenlan_core::events::NoopEmitter),
                );
                if let Err(error) = evaluator.check_first_page().await {
                    tracing::warn!(%error, "onboarding: first page not recorded");
                }
            }
            // The conversation behind the note, as for write_page. Best
            // effort: the note is already written.
            if let (Some(session), Some(page_id), "created") =
                (&session, done.page_id.as_deref(), done.outcome)
            {
                let space = db
                    .get_page(page_id)
                    .await
                    .ok()
                    .flatten()
                    .and_then(|p| p.space);
                if let Err(error) = db
                    .stamp_page_history_session(
                        page_id,
                        write_started_at,
                        session,
                        space.as_deref(),
                    )
                    .await
                {
                    tracing::warn!(
                        "[session] failed to stamp page {page_id} with {session}: {error}"
                    );
                }
            }
            done
        }
        other => {
            return Err(ServerError::BadRequest(format!(
                "steep_invalid: action must be \"write\" or \"release\", not \"{other}\""
            )))
        }
    };
    let warnings = if done.page_id.is_some() {
        wenlan_core::diagrams::diagram_warnings(req.content.as_deref().unwrap_or(""))
    } else {
        Vec::new()
    };
    Ok(Json(SteepNewCommitResponse {
        outcome: done.outcome.to_string(),
        page_id: done.page_id,
        title: done.title,
        citations: done.citations,
        warnings,
    }))
}
