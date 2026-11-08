// SPDX-License-Identifier: Apache-2.0
//! Where a page made from a kept original came from.
//!
//! An imported chat's artifacts and canvases are kept as files with their own
//! pages (`wenlan_core::chat_import::originals`). This route tells the app,
//! for one such page, the vendor, the vendor's conversation id and URL (so it
//! can open the conversation in the original AI tool) and the kept file. It
//! answers only for a page the caller can see in its Space and under its
//! truth grant; any other page, and a page not made from an original, reads
//! as absent.
//!
//! `POST /api/originals` keeps what an AI tool made in a live session (an
//! HTML page, an SVG, a Markdown document) the same way, as its own page that
//! names the session (UX-ART-1..3).

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Json;
use tokio::sync::RwLock;

use wenlan_types::originals::{KeepOriginalRequest, KeepOriginalResponse, PageOriginalResponse};

use crate::error::ServerError;
use crate::route_registry::{get, post, TrackedRouter};
use crate::state::{ServerState, SharedState};

pub(crate) fn register(router: TrackedRouter<SharedState>) -> TrackedRouter<SharedState> {
    router
        .route("/api/pages/{id}/original", get(handle_get_page_original))
        .route("/api/originals", post(handle_keep_original))
}

/// POST /api/originals
///
/// Keep an HTML page, SVG or Markdown document an AI tool made in this
/// session as it is, as its own page. The session comes from the
/// `x-wenlan-session` header and is stamped on the page, so the page shows
/// which conversation made it and when. Keeping the same content again in
/// the same session returns the page it already has.
pub async fn handle_keep_original(
    State(state): State<Arc<RwLock<ServerState>>>,
    headers: HeaderMap,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    Json(req): Json<KeepOriginalRequest>,
) -> Result<Json<KeepOriginalResponse>, ServerError> {
    use wenlan_core::chat_import::originals::{
        keep_session_original, SessionOriginal, SessionOriginalFormat,
    };
    let format = SessionOriginalFormat::from_name(&req.format)
        .ok_or_else(|| ServerError::ValidationError("original_format_unsupported".into()))?;
    let (db, page_root) = {
        let s = state.read().await;
        (
            s.db.clone().ok_or(ServerError::DbNotInitialized)?,
            s.lint_config.page_root().map(std::path::Path::to_path_buf),
        )
    };
    let session = crate::session_routes::extract_session_ref(&headers);
    let write_started_at = chrono::Utc::now().timestamp();
    let (kept, created, space) = {
        let _space_write_guard = db.lock_space_writes().await;
        let resolved = db
            .resolve_write_space(
                &wenlan_types::WriteSpaceTarget::Inherit,
                header_space.as_deref(),
            )
            .await?;
        let space = match resolved.space_name.as_ref() {
            Some(name) => wenlan_types::WriteSpaceTarget::Named(name.clone()),
            None => wenlan_types::WriteSpaceTarget::Uncategorized,
        };
        let data_root = wenlan_core::config::data_root();
        let (kept, created) = keep_session_original(
            &db,
            &data_root,
            page_root.as_deref(),
            SessionOriginal {
                session: session.as_ref(),
                title: &req.title,
                content: &req.content,
                format,
                space,
                workspace: resolved.space_name.clone(),
            },
        )
        .await?;
        (kept, created, resolved.space_name)
    };
    // Attribution is best effort: the page is already written.
    if let (Some(session), true) = (session.as_ref(), created) {
        if let Err(error) = db
            .stamp_page_history_session(&kept.page_id, write_started_at, session, space.as_deref())
            .await
        {
            tracing::warn!(
                "[session] failed to stamp original {} with {session}: {error}",
                kept.page_id
            );
        }
    }
    let title = db
        .get_page(&kept.page_id)
        .await?
        .map(|page| page.title)
        .unwrap_or_else(|| req.title.trim().to_string());
    Ok(Json(KeepOriginalResponse {
        page_id: kept.page_id,
        title,
        created,
    }))
}

/// GET /api/pages/{id}/original
pub async fn handle_get_page_original(
    State(state): State<Arc<RwLock<ServerState>>>,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    view: crate::truth_guard::TruthView,
    Path(id): Path<String>,
) -> Result<Json<PageOriginalResponse>, ServerError> {
    let db = state
        .read()
        .await
        .db
        .clone()
        .ok_or(ServerError::DbNotInitialized)?;
    let scope = crate::read_scope::effective_read_scope(&db, None, header_space.as_deref()).await?;
    let visible =
        crate::page_basis_routes::visible_ids(&db, &view.grant, &scope, std::slice::from_ref(&id))
            .await?;
    if !visible.contains(&id) {
        return Err(ServerError::NotFound(format!("page {id} not found")));
    }
    wenlan_core::chat_import::originals::page_original(&db, &id)
        .await?
        .map(Json)
        .ok_or_else(|| ServerError::NotFound(format!("page {id} was not made from an original")))
}
