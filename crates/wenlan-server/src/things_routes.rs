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
    SavedIn, SavedInRequest, SavedInResponse, SavedPageChange, ThingDetailResponse, ThingMapAuthor,
    ThingMapOpsRequest, ThingMapOpsResponse, ThingMapResponse, ThingMergeRequest, ThingMoveRequest,
    ThingNightPointsResponse, ThingRenameRequest, ThingSummary, ThingsResponse,
};

use crate::error::ServerError;
use crate::memory_routes::extract_agent_name;
use crate::route_registry::{get, post, TrackedRouter};
use crate::space_header::SpaceHeader;
use crate::state::SharedState;

pub(crate) fn register(router: TrackedRouter<SharedState>) -> TrackedRouter<SharedState> {
    router
        .route("/api/things", get(handle_list_things))
        .route("/api/things/points", get(handle_thing_night_points))
        .route(
            "/api/things/points/connect",
            post(handle_connect_night_points),
        )
        .route(
            "/api/things/points/connect/undo",
            post(handle_undo_connect_night_points),
        )
        .route(
            "/api/things/{id}",
            get(handle_get_thing).patch(handle_rename_thing),
        )
        .route("/api/things/{id}/hide", post(handle_hide_thing))
        .route("/api/things/{id}/merge", post(handle_merge_thing))
        .route("/api/things/{id}/keep-apart", post(handle_keep_thing_apart))
        .route("/api/things/{id}/dream/undo", post(handle_undo_thing_dream))
        .route(
            "/api/things/{id}/tidy/{memory_id}/undo",
            post(handle_undo_thing_tidy),
        )
        .route("/api/things/nights/undo", post(handle_undo_nights))
        .route("/api/things/{id}/map", get(handle_get_thing_map))
        .route("/api/things/{id}/map/ops", post(handle_thing_map_ops))
        .route(
            "/api/sessions/{id}/things/move",
            post(handle_move_session_thing),
        )
        .route("/api/sessions/{id}/saved-in", post(handle_saved_in))
}

async fn open_db(state: &SharedState) -> Result<Arc<MemoryDB>, ServerError> {
    let state = state.read().await;
    state.db.clone().ok_or(ServerError::DbNotInitialized)
}

fn require_person(headers: &HeaderMap) -> Result<(), ServerError> {
    if extract_agent_name(headers, None) != "unknown" {
        return Err(ServerError::ValidationError(
            "thing_needs_the_person: only the person renames or hides things and moves conversations".into(),
        ));
    }
    Ok(())
}

/// Sort new conversations into things, logging rather than failing: sorting
/// is the daemon's own upkeep and never holds up a request.
pub async fn sort_once(db: &MemoryDB) {
    match db.sort_sessions_into_things().await {
        Ok(report) if !report.sorted.is_empty() || report.named > 0 => {
            tracing::info!(
                "[things] sorted {} conversation(s), named {}",
                report.sorted.len(),
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

/// GET /api/things/points
///
/// Every live thing the caller may see with points its nights wrote that
/// still stand, so the big map can draw each thing with its points around it
/// (UX-CHAT-5). Points of a memory outside the caller's Space are left out.
pub async fn handle_thing_night_points(
    State(state): State<SharedState>,
    SpaceHeader(header_space): SpaceHeader,
) -> Result<Json<ThingNightPointsResponse>, ServerError> {
    let db = open_db(&state).await?;
    let scope = crate::read_scope::effective_read_scope(&db, None, header_space.as_deref()).await?;
    Ok(Json(ThingNightPointsResponse {
        things: db.thing_night_points(&scope).await?,
    }))
}

/// POST /api/things/points/connect
///
/// The person connects the night points kept before nights named their topics
/// (UX-CHAT-5). Once only, and nothing runs it for them: a model reads each
/// thing's points.
pub async fn handle_connect_night_points(
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> Result<Json<wenlan_types::NightLinksBackfill>, ServerError> {
    require_person(&headers)?;
    let db = open_db(&state).await?;
    let models = crate::things_intake::models(&state)
        .await
        .ok_or_else(|| ServerError::ValidationError("no model to read the points with".into()))?;
    let report = crate::things_intake::connect_old_night_points(&db, models.group.as_ref())
        .await
        .map_err(ServerError::ValidationError)?;
    Ok(Json(report))
}

/// POST /api/things/points/connect/undo
///
/// The person takes back what connecting the old points linked (UX-CHAT-5).
pub async fn handle_undo_connect_night_points(
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> Result<Json<wenlan_types::NightLinksBackfillUndo>, ServerError> {
    require_person(&headers)?;
    let db = open_db(&state).await?;
    let removed = db.undo_night_links_backfill().await?;
    Ok(Json(wenlan_types::NightLinksBackfillUndo { removed }))
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
    // Only things this caller can see are offered to merge (UX-THING-3).
    for other in db.thing_merge_proposals(&id).await? {
        if db.get_thing(&other.id, &scope).await?.is_some() {
            detail.merge_proposals.push(other);
        }
    }
    Ok(Json(detail))
}

/// GET /api/things/{id}/map
///
/// The thing's map of points, which its recap and mind map read, with the
/// changes waiting for the person. A thing nothing wrote a map for answers an
/// empty map at revision 0. 404 when the thing is unknown or out of scope.
pub async fn handle_get_thing_map(
    State(state): State<SharedState>,
    SpaceHeader(header_space): SpaceHeader,
    Path(id): Path<String>,
) -> Result<Json<ThingMapResponse>, ServerError> {
    let db = open_db(&state).await?;
    visible_thing(&db, header_space.as_deref(), &id).await?;
    Ok(Json(db.get_thing_map(&id).await?))
}

/// POST /api/things/{id}/map/ops
///
/// Change the map: ops apply in order, all or none, against the revision the
/// caller read (409 otherwise). An agent (a request with `x-agent-name`)
/// writes as the AI, and its changes to points the person wrote wait as
/// proposals (UX-WHO-2); anyone else is the person, who also answers
/// proposals. 404 when the thing is unknown or out of scope, 422 for a bad op.
pub async fn handle_thing_map_ops(
    State(state): State<SharedState>,
    SpaceHeader(header_space): SpaceHeader,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<ThingMapOpsRequest>,
) -> Result<Json<ThingMapOpsResponse>, ServerError> {
    let by = if extract_agent_name(&headers, None) == "unknown" {
        ThingMapAuthor::You
    } else {
        ThingMapAuthor::Ai
    };
    let db = open_db(&state).await?;
    visible_thing(&db, header_space.as_deref(), &id).await?;
    let (map, proposed) = db
        .apply_thing_map_ops(&id, by, request.expected_revision, &request.ops)
        .await?;
    Ok(Json(ThingMapOpsResponse { map, proposed }))
}

/// POST /api/sessions/{id}/saved-in
///
/// What the AI tool's one closing line names (UX-CHAT-7). `saved_in` is set
/// only the first time the conversation lands in a thing, or when it moves
/// to another, and only once that thing has a name. A conversation not yet
/// sorted is sorted now, so the line can come with the first save. A thing
/// the caller's Space cannot see is never named. After a page write the body
/// names the page and what happened to it; `report_page` is true for a new
/// page, and for an update or a suggestion the first time in this
/// conversation. `{id}` is the conversation's row id or its
/// `tool:external-id` ref.
pub async fn handle_saved_in(
    State(state): State<SharedState>,
    SpaceHeader(header_space): SpaceHeader,
    Path(id): Path<String>,
    body: Option<Json<SavedInRequest>>,
) -> Result<Json<SavedInResponse>, ServerError> {
    let request = body.map(|Json(request)| request).unwrap_or_default();
    let (db, reported, reported_pages) = {
        let state = state.read().await;
        let db = state.db.clone().ok_or(ServerError::DbNotInitialized)?;
        (
            db,
            state.saved_in_reported.clone(),
            state.saved_in_pages.clone(),
        )
    };
    // An AI tool knows its conversation by its ref (`tool:external-id`, as it
    // sends in `x-wenlan-session`); the app knows the row id.
    let id = if id.starts_with("ses_") {
        id
    } else {
        let session = wenlan_types::SessionRef::parse(&id)
            .map_err(|reason| ServerError::ValidationError(format!("bad session: {reason}")))?;
        wenlan_core::db::sessions::session_row_id(&session)
    };
    let report_page = match (request.page_id.as_deref(), request.change) {
        (Some(page_id), Some(change)) if !page_id.trim().is_empty() => {
            let key = |change: SavedPageChange| format!("{id}|{page_id}|{change:?}");
            let mut seen = reported_pages
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match change {
                // A page the AI has just written is not news again when it
                // revises it in the same conversation.
                SavedPageChange::New => {
                    seen.insert(key(SavedPageChange::Updated));
                    true
                }
                SavedPageChange::Updated | SavedPageChange::Proposal => seen.insert(key(change)),
            }
        }
        _ => false,
    };
    Ok(Json(SavedInResponse {
        saved_in: thing_to_report(&db, header_space.as_deref(), &id, &reported).await?,
        report_page,
    }))
}

/// The thing to name for conversation `id`, if it changed since the last
/// report and has a name the caller may see.
async fn thing_to_report(
    db: &MemoryDB,
    header_space: Option<&str>,
    id: &str,
    reported: &std::sync::Mutex<std::collections::HashMap<String, String>>,
) -> Result<Option<SavedIn>, ServerError> {
    // Sorting runs in the background. When this conversation is in no thing
    // yet, or its thing still waits for a name (the tool may write the
    // conversation's title after the first save), sort now.
    let mut things = db.session_things(id).await?;
    let named = things
        .last()
        .and_then(|thing| thing.title.as_deref())
        .is_some_and(|title| !title.trim().is_empty());
    if !named {
        sort_once(db).await;
        things = db.session_things(id).await?;
    }
    // Newest assignment last: a moved conversation reports where it is now.
    let Some(thing) = things.pop() else {
        return Ok(None);
    };
    let Some(title) = thing.title.filter(|title| !title.trim().is_empty()) else {
        return Ok(None);
    };
    if visible_thing(db, header_space, &thing.id).await.is_err() {
        return Ok(None);
    }
    {
        let mut reported = reported
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if reported.get(id) == Some(&thing.id) {
            return Ok(None);
        }
        reported.insert(id.to_string(), thing.id.clone());
    }
    Ok(Some(SavedIn {
        link: format!("wenlan://thing/{}", thing.id),
        thing_id: thing.id,
        title,
    }))
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

/// POST /api/things/{id}/hide
///
/// The person takes a thing off "what you're working on"; it comes back with
/// its next conversation (UX-HOME-5).
pub async fn handle_hide_thing(
    State(state): State<SharedState>,
    SpaceHeader(header_space): SpaceHeader,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<ThingSummary>, ServerError> {
    require_person(&headers)?;
    let db = open_db(&state).await?;
    let scope = visible_thing(&db, header_space.as_deref(), &id).await?;
    db.hide_thing(&id).await?;
    Ok(Json(summary(&db, &scope, &id).await?))
}

/// POST /api/things/{id}/merge
///
/// The person presses "merge" on a thing's page: the other thing folds into
/// this one (UX-THING-3). Nothing else merges things. Returns this thing.
pub async fn handle_merge_thing(
    State(state): State<SharedState>,
    SpaceHeader(header_space): SpaceHeader,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<ThingMergeRequest>,
) -> Result<Json<ThingSummary>, ServerError> {
    require_person(&headers)?;
    let db = open_db(&state).await?;
    let scope = visible_thing(&db, header_space.as_deref(), &id).await?;
    visible_thing(&db, header_space.as_deref(), &request.other_thing_id).await?;
    db.merge_things(&id, &request.other_thing_id).await?;
    Ok(Json(summary(&db, &scope, &id).await?))
}

/// POST /api/things/{id}/keep-apart
///
/// The person keeps two things apart: the pair is not proposed again.
pub async fn handle_keep_thing_apart(
    State(state): State<SharedState>,
    SpaceHeader(header_space): SpaceHeader,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<ThingMergeRequest>,
) -> Result<Json<ThingSummary>, ServerError> {
    require_person(&headers)?;
    let db = open_db(&state).await?;
    let scope = visible_thing(&db, header_space.as_deref(), &id).await?;
    visible_thing(&db, header_space.as_deref(), &request.other_thing_id).await?;
    db.keep_things_apart(&id, &request.other_thing_id).await?;
    Ok(Json(summary(&db, &scope, &id).await?))
}

/// POST /api/things/{id}/dream/undo
///
/// The person takes back the newest night that put this thing's points into
/// long-term memory (UX-CHAT-5): its memories are forgotten. Returns the
/// thing.
pub async fn handle_undo_thing_dream(
    State(state): State<SharedState>,
    SpaceHeader(header_space): SpaceHeader,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<ThingSummary>, ServerError> {
    require_person(&headers)?;
    let db = open_db(&state).await?;
    let scope = visible_thing(&db, header_space.as_deref(), &id).await?;
    db.undo_thing_dream(&id).await?;
    Ok(Json(summary(&db, &scope, &id).await?))
}

/// POST /api/things/{id}/tidy/{memory_id}/undo
///
/// The person takes back one tidy a night made, from the memory that stands
/// now (UX-GRAPH-7): what it hid comes back, what it wrote is forgotten.
/// Returns the thing.
pub async fn handle_undo_thing_tidy(
    State(state): State<SharedState>,
    SpaceHeader(header_space): SpaceHeader,
    headers: HeaderMap,
    Path((id, memory_id)): Path<(String, String)>,
) -> Result<Json<ThingSummary>, ServerError> {
    require_person(&headers)?;
    let db = open_db(&state).await?;
    let scope = visible_thing(&db, header_space.as_deref(), &id).await?;
    if db.undo_tidy(&id, &memory_id).await?.is_none() {
        return Err(ServerError::NotFound(format!(
            "no tidy of {memory_id} on this thing to take back"
        )));
    }
    Ok(Json(summary(&db, &scope, &id).await?))
}

/// POST /api/things/nights/undo
///
/// The person takes back the whole night on the big map (UX-GRAPH-5): every
/// thing in their scope whose newest night ran at or after `since`, and the
/// points those nights wrote. One thing's page still takes back only its own.
pub async fn handle_undo_nights(
    State(state): State<SharedState>,
    SpaceHeader(header_space): SpaceHeader,
    headers: HeaderMap,
    Json(request): Json<wenlan_types::NightsUndoRequest>,
) -> Result<Json<wenlan_types::NightsUndo>, ServerError> {
    require_person(&headers)?;
    let db = open_db(&state).await?;
    let scope = crate::read_scope::effective_read_scope(&db, None, header_space.as_deref()).await?;
    let (things, memories) = db.undo_nights_since(&scope, request.since).await?;
    Ok(Json(wenlan_types::NightsUndo {
        things,
        points: memories.len(),
    }))
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
