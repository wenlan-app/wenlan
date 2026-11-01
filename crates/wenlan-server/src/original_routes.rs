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

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::Json;
use tokio::sync::RwLock;

use wenlan_types::originals::PageOriginalResponse;

use crate::error::ServerError;
use crate::route_registry::{get, TrackedRouter};
use crate::state::{ServerState, SharedState};

pub(crate) fn register(router: TrackedRouter<SharedState>) -> TrackedRouter<SharedState> {
    router.route("/api/pages/{id}/original", get(handle_get_page_original))
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
