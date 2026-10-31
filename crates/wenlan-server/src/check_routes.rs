// SPDX-License-Identifier: Apache-2.0
//! The in-chat check (UX-CHAT-1/2/6).
//!
//! `POST /api/check` is what the Claude Code Stop hook (and the MCP
//! `check_reply` tool) send the AI's latest reply to; `POST /api/check/context`
//! is the UserPromptSubmit side. The rules are in `wenlan_core::chat_check`;
//! this file only frames HTTP and enforces the one-second budget: a check
//! that cannot answer in time returns an empty answer marked `timeout`, so
//! the caller skips and the chat is never held up.

use crate::error::ServerError;
use crate::route_registry::{post, TrackedRouter};
use crate::state::{ServerState, SharedState};
use axum::extract::State;
use axum::http::HeaderMap;
use axum::Json;
use std::sync::Arc;
use tokio::sync::RwLock;
use wenlan_core::chat_check::CHECK_BUDGET;
use wenlan_types::chat_check::{
    ChatCheckRequest, ChatCheckResponse, ChatCheckSkipped, ChatContextRequest, ChatContextResponse,
    DecisionSource,
};
use wenlan_types::sessions::SessionRef;

pub(crate) fn register(router: TrackedRouter<SharedState>) -> TrackedRouter<SharedState> {
    router
        .route("/api/check", post(handle_check))
        .route("/api/check/context", post(handle_check_context))
}

/// The in-chat check is opt-in: it runs only after the person turns it on
/// (`chat_check_enabled` in the config, `PUT /api/config`).
fn chat_check_enabled() -> bool {
    wenlan_core::config::load_config().chat_check_enabled
}

/// The session the check runs for: the body's, else the session header's.
/// A value that is not `<tool>:<id>` is ignored, like the header.
fn session_of(body: Option<&str>, headers: &HeaderMap) -> Option<String> {
    body.map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| SessionRef::parse(s).ok())
        .or_else(|| crate::session_routes::extract_session_ref(headers))
        .map(|s| s.to_string())
}

fn primary_space<'a>(space: Option<&'a str>, workspace: Option<&'a str>) -> Option<&'a str> {
    space
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or(workspace.map(str::trim).filter(|s| !s.is_empty()))
}

async fn db_of(
    state: &Arc<RwLock<ServerState>>,
) -> Result<Arc<wenlan_core::db::MemoryDB>, ServerError> {
    let s = state.read().await;
    s.db.clone().ok_or(ServerError::DbNotInitialized)
}

/// A source page the truth gate hides from this caller is dropped, and the
/// link falls back to the memory itself.
async fn hide_unservable_pages(
    db: &wenlan_core::db::MemoryDB,
    view: &crate::truth_guard::TruthView,
    sources: &mut [&mut DecisionSource],
) -> Result<(), ServerError> {
    let ids: Vec<String> = sources
        .iter()
        .filter_map(|s| s.page.as_ref().map(|p| p.page_id.clone()))
        .collect();
    if ids.is_empty() {
        return Ok(());
    }
    let visible: std::collections::HashSet<String> =
        wenlan_core::truth_adapter::filter_page_refs(db, &view.grant, ids, |id| id.as_str())
            .await?
            .into_iter()
            .collect();
    for source in sources.iter_mut() {
        if source
            .page
            .as_ref()
            .is_some_and(|p| !visible.contains(&p.page_id))
        {
            source.page = None;
            source.link = wenlan_types::chat_check::memory_link(&source.memory_id);
        }
    }
    Ok(())
}

/// POST /api/check
pub async fn handle_check(
    State(state): State<Arc<RwLock<ServerState>>>,
    headers: HeaderMap,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    view: crate::truth_guard::TruthView,
    Json(req): Json<ChatCheckRequest>,
) -> Result<Json<ChatCheckResponse>, ServerError> {
    if !chat_check_enabled() {
        return Ok(Json(ChatCheckResponse {
            skipped: Some(ChatCheckSkipped::Off),
            ..Default::default()
        }));
    }
    let db = db_of(&state).await?;
    let session = session_of(req.session.as_deref(), &headers);
    let work = async {
        let scope = crate::read_scope::effective_read_scope(
            &db,
            primary_space(req.space.as_deref(), req.workspace.as_deref()),
            header_space.as_deref(),
        )
        .await?;
        let mut response = wenlan_core::chat_check::check_against_decisions(
            &db,
            &req.reply,
            session.as_deref(),
            &scope,
            chrono::Local::now(),
        )
        .await?;
        let mut sources: Vec<&mut DecisionSource> = response
            .reminders
            .iter_mut()
            .map(|r| &mut r.source)
            .collect();
        hide_unservable_pages(&db, &view, &mut sources).await?;
        Ok::<_, ServerError>(response)
    };
    match tokio::time::timeout(CHECK_BUDGET, work).await {
        Ok(result) => result.map(Json),
        Err(_) => {
            tracing::debug!("[check] reply check passed its budget; returning nothing");
            Ok(Json(ChatCheckResponse {
                skipped: Some(ChatCheckSkipped::Timeout),
                ..Default::default()
            }))
        }
    }
}

/// POST /api/check/context
pub async fn handle_check_context(
    State(state): State<Arc<RwLock<ServerState>>>,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    view: crate::truth_guard::TruthView,
    Json(req): Json<ChatContextRequest>,
) -> Result<Json<ChatContextResponse>, ServerError> {
    if !chat_check_enabled() {
        return Ok(Json(ChatContextResponse {
            skipped: Some(ChatCheckSkipped::Off),
            ..Default::default()
        }));
    }
    let db = db_of(&state).await?;
    if req.prompt.trim().is_empty() {
        return Ok(Json(ChatContextResponse {
            skipped: Some(ChatCheckSkipped::Empty),
            ..Default::default()
        }));
    }
    let work = async {
        let scope = crate::read_scope::effective_read_scope(
            &db,
            primary_space(req.space.as_deref(), req.workspace.as_deref()),
            header_space.as_deref(),
        )
        .await?;
        let mut decisions =
            wenlan_core::chat_check::decision_context(&db, &req.prompt, &scope).await?;
        let mut sources: Vec<&mut DecisionSource> =
            decisions.iter_mut().map(|d| &mut d.source).collect();
        hide_unservable_pages(&db, &view, &mut sources).await?;
        Ok::<_, ServerError>(decisions)
    };
    match tokio::time::timeout(CHECK_BUDGET, work).await {
        Ok(result) => result.map(|decisions| {
            Json(ChatContextResponse {
                context: wenlan_core::chat_check::context_block(&decisions),
                decisions,
                skipped: None,
            })
        }),
        Err(_) => Ok(Json(ChatContextResponse {
            skipped: Some(ChatCheckSkipped::Timeout),
            ..Default::default()
        })),
    }
}

#[cfg(test)]
#[path = "check_routes_test.rs"]
mod tests;
