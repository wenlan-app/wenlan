// SPDX-License-Identifier: Apache-2.0
//! The memory palace layout (UX-GRAPH-1..3): `GET`/`PUT
//! /api/palace/layout` and `POST /api/palace/layout/revert`.
//!
//! The layout lives in `<data root>/layout/palace.json`
//! (`wenlan_core::palace`). Every write names the version it was based on;
//! a stale one is refused with 409 and the stored version, so the app reloads
//! instead of overwriting. A write that carries `x-agent-name` is an AI
//! tool's: what it adds stays a dashed suggestion until the person accepts it.

use crate::error::ServerError;
use crate::route_registry::{get, post, TrackedRouter};
use crate::state::{ServerState, SharedState};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use std::sync::Arc;
use tokio::sync::RwLock;
use wenlan_core::palace::{PalaceError, PalaceStore, PalaceWriter};
use wenlan_types::palace::{
    PalaceLayoutResponse, PutPalaceLayoutRequest, RevertPalaceLayoutRequest,
};

pub(crate) fn register(router: TrackedRouter<SharedState>) -> TrackedRouter<SharedState> {
    router
        .route(
            "/api/palace/layout",
            get(handle_get_palace_layout).put(handle_put_palace_layout),
        )
        .route(
            "/api/palace/layout/revert",
            post(handle_revert_palace_layout),
        )
}

async fn store_of(state: &Arc<RwLock<ServerState>>) -> PalaceStore {
    let root = state.read().await.palace_root.clone();
    PalaceStore::new(root.unwrap_or_else(wenlan_core::palace::default_layout_dir))
}

fn writer_of(headers: &HeaderMap) -> PalaceWriter {
    match headers
        .get("x-agent-name")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|agent| !agent.is_empty())
    {
        Some(agent) => PalaceWriter::Ai {
            agent: agent.to_string(),
        },
        None => PalaceWriter::Person,
    }
}

fn into_server_error(error: PalaceError) -> ServerError {
    match error {
        PalaceError::VersionConflict { current } => ServerError::Structured {
            status: StatusCode::CONFLICT,
            body: serde_json::json!({
                "error": "palace_version_conflict",
                "current_version": current,
            }),
        },
        PalaceError::NoSuchVersion(version) => {
            ServerError::NotFound(format!("no kept palace layout version {version}"))
        }
        PalaceError::Invalid(message) => ServerError::ValidationError(message),
        PalaceError::Io(error) => ServerError::Internal(format!("palace layout: {error}")),
    }
}

/// GET /api/palace/layout
pub async fn handle_get_palace_layout(
    State(state): State<Arc<RwLock<ServerState>>>,
) -> Result<Json<PalaceLayoutResponse>, ServerError> {
    let store = store_of(&state).await;
    tokio::task::spawn_blocking(move || store.read())
        .await
        .map_err(|error| ServerError::Internal(error.to_string()))?
        .map(Json)
        .map_err(into_server_error)
}

/// PUT /api/palace/layout
pub async fn handle_put_palace_layout(
    State(state): State<Arc<RwLock<ServerState>>>,
    headers: HeaderMap,
    Json(request): Json<PutPalaceLayoutRequest>,
) -> Result<Json<PalaceLayoutResponse>, ServerError> {
    let store = store_of(&state).await;
    store
        .write(
            request.expected_version,
            request.layout,
            request.reason,
            &writer_of(&headers),
        )
        .await
        .map(Json)
        .map_err(into_server_error)
}

/// POST /api/palace/layout/revert
pub async fn handle_revert_palace_layout(
    State(state): State<Arc<RwLock<ServerState>>>,
    headers: HeaderMap,
    Json(request): Json<RevertPalaceLayoutRequest>,
) -> Result<Json<PalaceLayoutResponse>, ServerError> {
    if matches!(writer_of(&headers), PalaceWriter::Ai { .. }) {
        return Err(ServerError::AgentDisabled(
            "only the person can revert the palace layout".into(),
        ));
    }
    let store = store_of(&state).await;
    store
        .revert(request.expected_version, request.to_version)
        .await
        .map(Json)
        .map_err(into_server_error)
}

#[cfg(test)]
#[path = "palace_routes_test.rs"]
mod tests;
