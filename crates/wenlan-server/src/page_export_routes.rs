// SPDX-License-Identifier: Apache-2.0
//! Portable immutable versions. These routes carry Page prose and retain export's Full-only gate.
use crate::{
    error::ServerError,
    route_registry::{get, post, TrackedRouter},
    space_header::SpaceHeader,
    state::SharedState,
};
use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::Deserialize;
use wenlan_types::page_export::*;
#[derive(Default, Deserialize)]
pub struct ExportQuery {
    pub space: Option<String>,
}
pub(crate) fn register(router: TrackedRouter<SharedState>) -> TrackedRouter<SharedState> {
    router
        .route("/api/pages/{id}/export-capture", post(capture))
        .route("/api/pages/{id}/export-versions", get(versions))
        .route("/api/pages/{id}/export-bundles/{bundle_id}", get(bundle))
}
async fn context(
    state: &SharedState,
    space: Option<&str>,
    header: Option<&str>,
) -> Result<
    (
        std::sync::Arc<wenlan_core::db::MemoryDB>,
        wenlan_core::read_scope::ReadScope,
    ),
    ServerError,
> {
    if space.is_some_and(|s| s.trim().is_empty()) || space.zip(header).is_some_and(|(a, b)| a != b)
    {
        return Err(ServerError::BadRequest(
            "export Space and header must agree".into(),
        ));
    }
    let db = {
        let state = state.read().await;
        state.db.clone().ok_or(ServerError::DbNotInitialized)?
    };
    let scope = crate::read_scope::effective_read_scope(&db, space, header).await?;
    Ok((db, scope))
}
async fn capture(
    State(state): State<SharedState>,
    SpaceHeader(header): SpaceHeader,
    Path(id): Path<String>,
    Json(req): Json<CapturePageExportRequest>,
) -> Result<Json<PageExportBundle>, ServerError> {
    let (db, scope) = context(&state, req.space.as_deref(), header.as_deref()).await?;
    Ok(Json(db.capture_page_export(&id, &scope).await?))
}
async fn versions(
    State(state): State<SharedState>,
    SpaceHeader(header): SpaceHeader,
    Path(id): Path<String>,
    Query(query): Query<ExportQuery>,
) -> Result<Json<PageExportVersionsResponse>, ServerError> {
    let (db, scope) = context(&state, query.space.as_deref(), header.as_deref()).await?;
    Ok(Json(db.page_export_versions(&id, &scope).await?))
}
async fn bundle(
    State(state): State<SharedState>,
    SpaceHeader(header): SpaceHeader,
    Path((id, bundle_id)): Path<(String, String)>,
    Query(query): Query<ExportQuery>,
) -> Result<Json<PageExportBundle>, ServerError> {
    let (db, scope) = context(&state, query.space.as_deref(), header.as_deref()).await?;
    Ok(Json(db.page_export_bundle(&id, &bundle_id, &scope).await?))
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;
    #[tokio::test]
    async fn page_export_routes_reject_conflicting_scope_before_storage() {
        let state = std::sync::Arc::new(tokio::sync::RwLock::new(
            crate::state::ServerState::default(),
        ));
        let app = crate::router::build_router(state);
        for (method, path, body) in [
            ("GET", "/api/pages/page/export-versions?space=Research", ""),
            (
                "GET",
                "/api/pages/page/export-bundles/bundle?space=Research",
                "",
            ),
            (
                "POST",
                "/api/pages/page/export-capture",
                r#"{"space":"Research"}"#,
            ),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(path)
                        .header("x-wenlan-space", "Other")
                        .header("content-type", "application/json")
                        .body(Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{path}");
        }
    }
}
