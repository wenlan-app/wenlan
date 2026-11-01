// SPDX-License-Identifier: Apache-2.0
//! Things (一件事): the lines of work conversations belong to (UX-THING-1).
//!
//! The rules live in `wenlan_core::db::things`; this module frames them as
//! HTTP. Reads answer only for what the caller's Space may see, and page
//! titles pass the caller's truth grant. Renaming a thing and moving a
//! conversation are the person's corrections: an agent (a request with
//! `x-agent-name`) is refused, since the daemon already sorts for the AI.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Json;
use wenlan_core::db::things::MoveTarget;
use wenlan_core::db::MemoryDB;
use wenlan_types::{
    ThingDetailResponse, ThingMoveRequest, ThingRenameRequest, ThingSummary, ThingsResponse,
};

use crate::error::ServerError;
use crate::memory_routes::extract_agent_name;
use crate::route_registry::{get, post, TrackedRouter};
use crate::space_header::SpaceHeader;
use crate::state::SharedState;

pub(crate) fn register(router: TrackedRouter<SharedState>) -> TrackedRouter<SharedState> {
    router
        .route("/api/things", get(handle_list_things))
        .route(
            "/api/things/{id}",
            get(handle_get_thing).patch(handle_rename_thing),
        )
        .route(
            "/api/sessions/{id}/things/move",
            post(handle_move_session_thing),
        )
}

async fn open_db(state: &SharedState) -> Result<Arc<MemoryDB>, ServerError> {
    let state = state.read().await;
    state.db.clone().ok_or(ServerError::DbNotInitialized)
}

fn require_person(headers: &HeaderMap) -> Result<(), ServerError> {
    if extract_agent_name(headers, None) != "unknown" {
        return Err(ServerError::ValidationError(
            "thing_needs_the_person: only the person renames things and moves conversations".into(),
        ));
    }
    Ok(())
}

/// Sort new conversations into things, logging rather than failing: sorting
/// is the daemon's own upkeep and never holds up a request.
pub async fn sort_once(db: &MemoryDB) {
    match db.sort_sessions_into_things().await {
        Ok(report) if !report.sorted.is_empty() || report.brief_things > 0 || report.named > 0 => {
            tracing::info!(
                "[things] sorted {} conversation(s), {} Brief thing(s), named {}",
                report.sorted.len(),
                report.brief_things,
                report.named
            );
        }
        Ok(_) => {}
        Err(error) => tracing::warn!("[things] sorting failed: {error}"),
    }
}

/// GET /api/things
///
/// What you're working on, newest activity first.
pub async fn handle_list_things(
    State(state): State<SharedState>,
    SpaceHeader(header_space): SpaceHeader,
) -> Result<Json<ThingsResponse>, ServerError> {
    let db = open_db(&state).await?;
    let scope = crate::read_scope::effective_read_scope(&db, None, header_space.as_deref()).await?;
    Ok(Json(ThingsResponse {
        things: db.list_things(&scope, None).await?,
    }))
}

/// GET /api/things/{id}
///
/// The thing, its conversations, and the pages and memories they wrote or
/// read. 404 when the thing is unknown or out of the caller's scope.
pub async fn handle_get_thing(
    State(state): State<SharedState>,
    SpaceHeader(header_space): SpaceHeader,
    view: crate::truth_guard::TruthView,
    Path(id): Path<String>,
) -> Result<Json<ThingDetailResponse>, ServerError> {
    let db = open_db(&state).await?;
    let scope = crate::read_scope::effective_read_scope(&db, None, header_space.as_deref()).await?;
    let mut detail = db
        .get_thing_detail(&id, &scope)
        .await?
        .ok_or_else(|| ServerError::NotFound("thing not found".to_string()))?;
    // Page titles are page-bearing: list only pages this caller may see.
    detail.pages = wenlan_core::truth_adapter::filter_page_refs(
        &db,
        &view.grant,
        std::mem::take(&mut detail.pages),
        |page| page.id.as_str(),
    )
    .await?;
    Ok(Json(detail))
}

async fn visible_thing(
    db: &MemoryDB,
    header_space: Option<&str>,
    id: &str,
) -> Result<wenlan_core::read_scope::ReadScope, ServerError> {
    let scope = crate::read_scope::effective_read_scope(db, None, header_space).await?;
    if db.get_thing(id, &scope).await?.is_none() {
        return Err(ServerError::NotFound("thing not found".to_string()));
    }
    Ok(scope)
}

async fn summary(
    db: &MemoryDB,
    scope: &wenlan_core::read_scope::ReadScope,
    id: &str,
) -> Result<ThingSummary, ServerError> {
    let found = match db.get_thing(id, scope).await? {
        Some(thing) => Some(thing),
        None => db
            .get_thing(id, &wenlan_core::read_scope::ReadScope::Global)
            .await?
            .map(|mut thing| {
                thing.spaces.clear();
                thing
            }),
    };
    found.ok_or_else(|| ServerError::NotFound("thing not found".to_string()))
}

/// PATCH /api/things/{id}
///
/// The person renames a thing; from then on the AI leaves its name alone.
pub async fn handle_rename_thing(
    State(state): State<SharedState>,
    SpaceHeader(header_space): SpaceHeader,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<ThingRenameRequest>,
) -> Result<Json<ThingSummary>, ServerError> {
    require_person(&headers)?;
    let db = open_db(&state).await?;
    let scope = visible_thing(&db, header_space.as_deref(), &id).await?;
    db.rename_thing(&id, &request.title).await?;
    Ok(Json(summary(&db, &scope, &id).await?))
}

/// POST /api/sessions/{id}/things/move
///
/// The person moves a conversation from one thing to another, or to a new
/// thing they name; its other things stay. Returns the target thing.
pub async fn handle_move_session_thing(
    State(state): State<SharedState>,
    SpaceHeader(header_space): SpaceHeader,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Json(request): Json<ThingMoveRequest>,
) -> Result<Json<ThingSummary>, ServerError> {
    require_person(&headers)?;
    let target = match (request.to_thing_id, request.new_title) {
        (Some(id), None) => MoveTarget::Thing(id),
        (None, Some(title)) => MoveTarget::NewThing(title),
        _ => {
            return Err(ServerError::ValidationError(
                "thing_move_target: give exactly one of to_thing_id and new_title".into(),
            ))
        }
    };
    let db = open_db(&state).await?;
    let scope = visible_thing(&db, header_space.as_deref(), &request.from_thing_id).await?;
    if let MoveTarget::Thing(id) = &target {
        visible_thing(&db, header_space.as_deref(), id).await?;
    }
    let moved = db
        .move_session_thing(&session_id, &request.from_thing_id, &target)
        .await?;
    Ok(Json(summary(&db, &scope, &moved).await?))
}
