// SPDX-License-Identifier: Apache-2.0
//! The nightly trust scan's report, and the one repair it offers by hand.
//!
//! - `GET /api/trust/scan`: the latest report (`report: null` before the
//!   first nightly run). Read only; the scan itself runs in the scheduler.
//! - `POST /api/pages/{id}/citations/requeue`: put one page whose citation
//!   backfill gave up back in the backfill's queue. Never automatic: the
//!   person asks for it after reading the report.

use crate::error::ServerError;
use crate::route_registry::{get, post, TrackedRouter};
use crate::state::SharedState;
use axum::{
    extract::{Path, State},
    response::Json,
};
use wenlan_types::trust_scan::{CitationRequeueResponse, TrustScanResponse};

pub(crate) fn register(router: TrackedRouter<SharedState>) -> TrackedRouter<SharedState> {
    router
        .route("/api/trust/scan", get(handle_trust_scan))
        .route(
            "/api/pages/{id}/citations/requeue",
            post(handle_requeue_citations),
        )
}

/// GET /api/trust/scan
pub async fn handle_trust_scan(
    State(state): State<SharedState>,
) -> Result<Json<TrustScanResponse>, ServerError> {
    let db = {
        let s = state.read().await;
        s.db.clone().ok_or(ServerError::DbNotInitialized)?
    };
    Ok(Json(TrustScanResponse {
        report: wenlan_core::trust_scan::latest(&db).await?,
    }))
}

/// POST /api/pages/{id}/citations/requeue
pub async fn handle_requeue_citations(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<Json<CitationRequeueResponse>, ServerError> {
    let db = {
        let s = state.read().await;
        s.db.clone().ok_or(ServerError::DbNotInitialized)?
    };
    let requeued = db.requeue_citation_backfill(&id).await?;
    Ok(Json(CitationRequeueResponse {
        page_id: id,
        requeued,
    }))
}

#[cfg(test)]
#[path = "trust_scan_routes_test.rs"]
mod tests;
