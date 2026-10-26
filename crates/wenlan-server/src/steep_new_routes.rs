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

use wenlan_types::steep_new::{
    STEEP_NEW_ACTION_ATTACH, STEEP_NEW_ACTION_RELEASE, STEEP_NEW_ACTION_WRITE,
};
use wenlan_types::{
    SteepNewClaimRequest, SteepNewClaimResponse, SteepNewCommitRequest, SteepNewCommitResponse,
    SteepNewLinksRequest, SteepNewLinksResponse, SteepNewListResponse, SteepNewTasksRequest,
    SteepNewTasksResponse, SteepOrganizeCommitRequest, SteepOrganizeCommitResponse,
};

use crate::error::ServerError;
use crate::route_registry::{get, post, TrackedRouter};
use crate::state::{ServerState, SharedState};
use crate::steep_routes::steep_error;

pub(crate) fn register(router: TrackedRouter<SharedState>) -> TrackedRouter<SharedState> {
    router
        .route("/api/steep/new", get(handle_steep_new_list))
        .route("/api/steep/new/claim", post(handle_steep_new_claim))
        .route("/api/steep/new/organize", post(handle_steep_new_organize))
        .route("/api/steep/new/links", post(handle_steep_new_links))
        .route("/api/steep/new/tasks", post(handle_steep_new_tasks))
        .route("/api/steep/new/commit", post(handle_steep_new_commit))
}

/// What the claim tells the caller to do with the sources.
pub const NEW_PAGE_NOTE: &str = "Write one note from these sources with your main model. \
Give it a short title that names the topic. Open with one plain sentence that says what \
the note is about, then a few short paragraphs or lists. Use only what the numbered \
sources say and cite every claim with its number, like [2]. Write in the language of the \
sources. Then commit with action \"write\", the token, the title and the body. If the \
sources do not belong together, commit with action \"release\" instead.";

/// Added to the note when the claim offers existing pages to link.
pub const LINK_TARGETS_NOTE: &str = "Where the note talks about one of the pages in \
link_targets, link it once as [[Title]], with the title exactly as given. Link no other \
titles.";

/// Added to the note when a page on the subject already exists.
pub const EXISTING_PAGE_NOTE: &str = "A page on this subject already exists: existing_page. \
Do not write a second page. Commit with action \"attach\" and the token: the sources go \
into that page, which then waits to be rewritten with all its sources.";

const NEXT_LIST: &str =
    "Call steep with action \"claim_new\" for one coherent group of related item_ids.";
const NEXT_ORGANIZE_TASKS: &str = "Call steep with action \"organize_tasks\" and this token; answer the returned tasks, then call action \"organize\".";
const NEXT_LINKS: &str =
    "Call steep with action \"links\" and this token immediately before writing.";
const NEXT_GROUP: &str =
    "Call steep with action \"new\", claim the next coherent group with action \"claim_new\", and repeat.";

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
        page_titles: list.page_titles,
        next_step: NEXT_LIST.to_string(),
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
    let (db, organize) = {
        let s = state.read().await;
        let db = s.db.clone().ok_or(ServerError::DbNotInitialized)?;
        // The caller's AI tool organizes the sources only when no background
        // model would: the two never spend twice on one memory.
        let organize = (!crate::steep_routes::background_does_everyday(&s)).then(|| {
            wenlan_core::steep_organize::OrganizeOptions {
                prompts: s.prompts.clone(),
                entity_link_distance: s.tuning.refinery.entity_link_distance as f32,
            }
        });
        (db, organize)
    };
    let scope = crate::read_scope::effective_read_scope(&db, None, header_space.as_deref()).await?;
    let claim = wenlan_core::steep_new::claim_new_on(
        &db,
        &scope,
        &req.item_ids,
        organize.as_ref(),
        req.topic.as_deref(),
    )
    .await
    .map_err(steep_error)?;
    let mut note = NEW_PAGE_NOTE.to_string();
    if !claim.link_targets.is_empty() {
        note = format!("{note} {LINK_TARGETS_NOTE}");
    }
    if claim.existing_page.is_some() {
        note = format!("{EXISTING_PAGE_NOTE} {note}");
    }
    let warnings = claim.thin_group.then(|| {
        format!(
            "Thin group: only {} sources were claimed while plenty of material is still waiting. Continue if they form a real topic; otherwise release and regroup.",
            claim.sources.len()
        )
    });
    let next_step = if claim.organize.is_some() {
        NEXT_ORGANIZE_TASKS
    } else {
        NEXT_LINKS
    };
    Ok(Json(SteepNewClaimResponse {
        token: claim.token,
        held_until: claim.held_until,
        sources: claim.sources,
        skipped: claim.skipped,
        note,
        organize: claim.organize,
        link_targets: claim.link_targets,
        existing_page: claim.existing_page,
        warnings: warnings.into_iter().collect(),
        next_step: next_step.to_string(),
    }))
}

/// POST /api/steep/new/organize
///
/// Land the small model's answers to a claim's organize tasks through the
/// same parsers and version-checked writes Wenlan's own model's answers take.
pub async fn handle_steep_new_organize(
    State(state): State<Arc<RwLock<ServerState>>>,
    Json(req): Json<SteepOrganizeCommitRequest>,
) -> Result<Json<SteepOrganizeCommitResponse>, ServerError> {
    let db = state
        .read()
        .await
        .db
        .clone()
        .ok_or(ServerError::DbNotInitialized)?;
    let mut done = wenlan_core::steep_new::organize_new(&db, &req.token, &req.answers)
        .await
        .map_err(steep_error)?;
    done.next_step = if done.remaining > 0 {
        NEXT_ORGANIZE_TASKS
    } else {
        NEXT_LINKS
    }
    .to_string();
    Ok(Json(done))
}

/// POST /api/steep/new/links
///
/// A claim's `link_targets` and `existing_page` as they are now, read just
/// before its page is written: pages this run wrote since the claim count.
/// Needs no model.
pub async fn handle_steep_new_links(
    State(state): State<Arc<RwLock<ServerState>>>,
    Json(req): Json<SteepNewLinksRequest>,
) -> Result<Json<SteepNewLinksResponse>, ServerError> {
    let db = state
        .read()
        .await
        .db
        .clone()
        .ok_or(ServerError::DbNotInitialized)?;
    let mut links = wenlan_core::steep_new::links_new(&db, &req.token)
        .await
        .map_err(steep_error)?;
    links.next_step = if links.existing_page.is_some() {
        "Call steep with action \"attach\" and this token; do not write a duplicate page."
    } else {
        "Write one cited page, using relevant link_targets once, then call steep with action \"write\", this token, title, and content."
    }
    .to_string();
    Ok(Json(links))
}

/// POST /api/steep/new/tasks
///
/// A claim's organize tasks still without an answer, for the helper that
/// answers them: it needs only the token. A Space-locked caller reads only a
/// claim made in its Space. Needs no model.
pub async fn handle_steep_new_tasks(
    State(state): State<Arc<RwLock<ServerState>>>,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    Json(req): Json<SteepNewTasksRequest>,
) -> Result<Json<SteepNewTasksResponse>, ServerError> {
    let db = state
        .read()
        .await
        .db
        .clone()
        .ok_or(ServerError::DbNotInitialized)?;
    let scope = crate::read_scope::effective_read_scope(&db, None, header_space.as_deref()).await?;
    let organize =
        wenlan_core::steep_new::organize_tasks_new(&req.token, &scope).map_err(steep_error)?;
    let next_step = if organize.is_some() {
        "Answer these tasks, then call steep with action \"organize\", this token, and the answers."
    } else {
        NEXT_LINKS
    };
    Ok(Json(SteepNewTasksResponse {
        organize,
        next_step: next_step.to_string(),
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
        STEEP_NEW_ACTION_ATTACH => {
            let agent = headers
                .get("x-agent-name")
                .and_then(|v| v.to_str().ok())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("steep")
                .to_string();
            wenlan_core::steep_new::attach_new(&db, &req.token, &agent)
                .await
                .map_err(steep_error)?
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
            "steep_invalid: action must be \"write\", \"attach\" or \"release\", not \"{other}\""
        )))
        }
    };
    let warnings = if done.page_id.is_some() && done.outcome != "attached" {
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
        link_targets: done.link_targets,
        next_step: NEXT_GROUP.to_string(),
    }))
}
