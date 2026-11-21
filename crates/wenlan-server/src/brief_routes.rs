// SPDX-License-Identifier: Apache-2.0

use crate::route_registry::{get, post, TrackedRouter};
use crate::{
    brief_files::project_brief_receipt, error::ServerError, space_header::SpaceHeader,
    state::SharedState, telemetry::TelemetryEvent,
};
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue},
    response::{IntoResponse, Json, Response},
};
use std::{future::Future, sync::Arc};
use wenlan_core::{
    db::{BriefUpdateOutcome, MemoryDB},
    read_scope::ReadScope,
};
use wenlan_types::{
    BriefHistoryResponse, BriefReadRequest, BriefReadResponse, BriefReadState, BriefRelatedContext,
    BriefUpdateRequest, BriefVersionResponse, SearchResult,
};

pub(crate) fn register(router: TrackedRouter<SharedState>) -> TrackedRouter<SharedState> {
    router
        .route(
            "/api/brief",
            post(handle_read_brief).patch(handle_update_brief),
        )
        .route("/api/brief/history", get(handle_brief_history))
        .route("/api/brief/plain", get(handle_brief_plain))
        .route("/api/brief/history/{version}", get(handle_brief_version))
        .route("/api/handoff", post(handle_handoff))
}

const HISTORY_DEFAULT_LIMIT: usize = 20;
const HISTORY_MAX_LIMIT: usize = 200;
pub(crate) const OPERATION_OUTCOME_HEADER: &str = "x-wenlan-operation-outcome";

#[derive(Debug, serde::Deserialize)]
pub struct BriefHistoryQuery {
    #[serde(default, alias = "domain")]
    pub space: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

const RELATED_CONTEXT_LIMIT: usize = 20;

async fn resolve_space(
    db: &MemoryDB,
    explicit: Option<&str>,
    header: Option<&str>,
) -> Result<Option<String>, ServerError> {
    let selected = explicit
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| header.map(str::trim).filter(|value| !value.is_empty()));

    if let Some(name) = selected {
        return db
            .registered_space_or_none(Some(name))
            .await?
            .map(Some)
            .ok_or_else(|| ServerError::ValidationError(format!("unknown Space: {name}")));
    }

    Ok(db.get_default_space().await?.map(|space| space.name))
}

async fn related_context_if_requested<F, Fut>(
    topic: Option<&str>,
    load: F,
) -> Result<Option<BriefRelatedContext>, ServerError>
where
    F: FnOnce(String) -> Fut,
    Fut: Future<Output = Result<Vec<SearchResult>, ServerError>>,
{
    let Some(query) = topic
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
    else {
        return Ok(None);
    };
    let results = load(query.clone()).await?;
    Ok(Some(BriefRelatedContext { query, results }))
}

/// POST /api/brief — read the complete Space Brief, optionally composed with
/// separately-labelled recall results for the same Space.
///
/// Page-bearing through `related_context`: `search_memory` merges the page
/// channel inline, so a page's prose can arrive as a `SearchResult` with
/// `source == "page"` and `source_id` the page id (`search_result_from_page`,
/// db.rs). The channel is default-OFF behind `WENLAN_ENABLE_PAGE_CHANNEL`, but
/// an exposure contract that only holds while a flag is off is not a contract,
/// so the gate is unconditional. `/api/context` reaches its pages through this
/// same handler, which is why the view is a parameter rather than an extractor
/// read here.
pub async fn handle_read_brief(
    State(state): State<SharedState>,
    headers: HeaderMap,
    SpaceHeader(header_space): SpaceHeader,
    view: crate::truth_guard::TruthView,
    Json(request): Json<BriefReadRequest>,
) -> Result<Json<BriefReadResponse>, ServerError> {
    // A topic turns the Brief read into one authoritative related-context
    // search. The legacy `/api/context` adapter delegates here, so recording
    // at this shared boundary covers both APIs exactly once.
    let has_topic = request
        .topic
        .as_deref()
        .map(str::trim)
        .is_some_and(|topic| !topic.is_empty());
    let telemetry = { state.read().await.telemetry.clone() };
    let db = {
        let state = state.read().await;
        state.db.clone().ok_or(ServerError::DbNotInitialized)?
    };
    let private = crate::team_actor::can_read_private_memories(&headers);
    let getting_started = if private {
        match wenlan_core::steep_new::getting_started(&db).await {
            Ok(getting_started) => getting_started,
            Err(error) => {
                tracing::warn!("[brief] getting started: {error}");
                None
            }
        }
    } else {
        None
    };
    let selected = if private {
        request.space.as_deref()
    } else {
        Some(crate::team_actor::required_space(header_space.as_deref())?)
    };
    let Some(space) = resolve_space(&db, selected, header_space.as_deref()).await? else {
        return Ok(Json(BriefReadResponse {
            state: BriefReadState::SpaceNotResolved,
            space: None,
            brief: None,
            related_context: None,
            code_basis_pending: None,
            code_basis_pending_explained: false,
            getting_started,
        }));
    };
    let Some(brief) = db.get_brief_by_space_name(&space).await? else {
        let (code_basis_pending, code_basis_pending_explained) = if private {
            code_basis_pending_for(&db, &space).await
        } else {
            (None, false)
        };
        return Ok(Json(BriefReadResponse {
            state: BriefReadState::BriefNotCreated,
            space: Some(space),
            brief: None,
            related_context: None,
            code_basis_pending,
            code_basis_pending_explained,
            getting_started,
        }));
    };

    let legacy_context_limit = request
        .legacy_context_limit
        .unwrap_or(RELATED_CONTEXT_LIMIT);
    let recall_db = Arc::clone(&db);
    let recall_space = space.clone();
    let related_context = related_context_if_requested(request.topic.as_deref(), move |query| {
        let scope = ReadScope::Space(recall_space);
        async move {
            if !private {
                return crate::page_routes::search_hosted_page_hits(
                    &recall_db,
                    &query,
                    legacy_context_limit,
                    &scope,
                    &view.grant,
                )
                .await;
            }
            let results = recall_db
                .search_memory(
                    &query,
                    legacy_context_limit,
                    None,
                    &scope,
                    None,
                    None,
                    None,
                    None,
                )
                .await
                .map_err(|error| ServerError::SearchFailed(error.to_string()))?;
            // Only the page-channel rows are page-bearing. Handing the whole
            // batch to the adapter would key memory rows by a memory id, which
            // no page grant covers, and drop legitimate results.
            let page_ids: Vec<String> = results
                .iter()
                .filter(|row| row.source == "page")
                .map(|row| row.source_id.clone())
                .collect();
            if page_ids.is_empty() {
                return Ok(results);
            }
            let visible: std::collections::HashSet<String> =
                wenlan_core::truth_adapter::filter_page_refs(
                    &recall_db,
                    &view.grant,
                    page_ids,
                    |id| id.as_str(),
                )
                .await
                .map_err(|error| ServerError::SearchFailed(error.to_string()))?
                .into_iter()
                .collect();
            // Retain rather than partition-and-append: `search_memory` has
            // already merged the page rows into RRF order, and a surviving page
            // belongs where that merge put it.
            let mut results = results;
            results.retain(|row| row.source != "page" || visible.contains(&row.source_id));
            Ok(results)
        }
    })
    .await;
    let related_context = match related_context {
        Ok(related_context) => {
            if has_topic {
                telemetry.record(
                    if related_context
                        .as_ref()
                        .is_some_and(|related| !related.results.is_empty())
                    {
                        TelemetryEvent::SearchNonempty
                    } else {
                        TelemetryEvent::SearchEmpty
                    },
                );
            }
            related_context
        }
        Err(error) => {
            if has_topic {
                telemetry.record(TelemetryEvent::SearchError);
            }
            return Err(error);
        }
    };

    let (code_basis_pending, code_basis_pending_explained) = if private {
        code_basis_pending_for(&db, &space).await
    } else {
        (None, false)
    };

    // A Brief that was actually read is listed on the session card when the
    // caller names its session. This is bookkeeping beside the read: it never
    // creates or changes a Brief or a Space, and nothing is recorded for the
    // unresolved and not-created answers above.
    crate::session_routes::note_session_read(
        db.clone(),
        &headers,
        wenlan_types::SessionReadKind::Brief,
        &space,
        wenlan_core::db::sessions::session_read_space_of(Some(&space)).into(),
    );

    Ok(Json(BriefReadResponse {
        state: BriefReadState::Ready,
        space: Some(space),
        brief: Some(brief),
        related_context,
        code_basis_pending,
        code_basis_pending_explained,
        getting_started,
    }))
}

/// Notes that rely on lines of code are checked against the files as they read
/// now (no model, one read per file; nothing at all for a Space without code
/// bases), and the notes whose code changed are counted. This holds whether or
/// not the Space has a Brief yet. A failed check leaves the Brief readable
/// without the count. The flag says the everyday model has already looked at
/// every one of those code bases.
async fn code_basis_pending_for(db: &MemoryDB, space: &str) -> (Option<u32>, bool) {
    let count = match wenlan_core::code_basis::brief_code_basis_pending(db, space).await {
        Ok(count) => count,
        Err(error) => {
            tracing::warn!("[brief] code basis check failed for Space {space}: {error}");
            None
        }
    };
    if count.is_none_or(|count| count == 0) {
        return (count, false);
    }
    let explained = db
        .space_code_basis_all_explained(space)
        .await
        .unwrap_or_else(|error| {
            tracing::warn!(
                "[brief] code basis explanation check failed for Space {space}: {error}"
            );
            false
        });
    (count, explained)
}

/// PATCH /api/brief — apply item-level handoff deltas, then best-effort
/// project the committed Brief as a human-readable Markdown receipt.
pub async fn handle_update_brief(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(request): Json<BriefUpdateRequest>,
) -> Result<Response, ServerError> {
    let (db, status_root, projection_lock) = {
        let state = state.read().await;
        (
            state.db.clone().ok_or(ServerError::DbNotInitialized)?,
            state.brief_status_root.clone(),
            Arc::clone(&state.brief_projection_lock),
        )
    };
    let session = crate::session_routes::extract_session_ref(&headers);
    let outcome = db
        .apply_brief_update_in_session_outcome(&request, session.as_ref())
        .await?;
    let mut receipt = match outcome {
        BriefUpdateOutcome::Applied(receipt) => receipt,
        BriefUpdateOutcome::Duplicate(receipt) => {
            let mut response = Json(receipt).into_response();
            response.headers_mut().insert(
                OPERATION_OUTCOME_HEADER,
                HeaderValue::from_static("duplicate"),
            );
            return Ok(response);
        }
    };
    let _projection_guard = projection_lock.lock().await;

    match (
        status_root,
        db.get_brief_by_space_name(&receipt.space).await?,
    ) {
        (Some(root), Some(brief)) => {
            match project_brief_receipt(&root, &brief, chrono::Utc::now().timestamp()) {
                Ok(path) => receipt.projection_path = Some(path.display().to_string()),
                Err(error) => receipt
                    .warnings
                    .push(format!("Brief receipt projection failed: {error}")),
            }
        }
        (None, _) => receipt
            .warnings
            .push("Brief receipt projection is not configured".to_string()),
        (Some(_), None) => receipt.warnings.push(
            "Brief receipt projection skipped because the committed Brief was not found".into(),
        ),
    }

    Ok(Json(receipt).into_response())
}

/// POST /api/handoff — the canonical Brief write verb. `PATCH /api/brief`
/// remains a compatibility route and calls the same implementation.
pub async fn handle_handoff(
    state: State<SharedState>,
    headers: HeaderMap,
    request: Json<wenlan_types::HandoffRequest>,
) -> Result<Response, ServerError> {
    handle_update_brief(state, headers, request).await
}

/// GET /api/brief/history — successful Brief updates for the resolved Space,
/// newest first. Brief text only; no page prose is returned.
pub async fn handle_brief_history(
    State(state): State<SharedState>,
    SpaceHeader(header_space): SpaceHeader,
    Query(query): Query<BriefHistoryQuery>,
) -> Result<Json<BriefHistoryResponse>, ServerError> {
    let db = {
        let state = state.read().await;
        state.db.clone().ok_or(ServerError::DbNotInitialized)?
    };
    let Some(space) = resolve_space(&db, query.space.as_deref(), header_space.as_deref()).await?
    else {
        return Ok(Json(BriefHistoryResponse {
            space: None,
            entries: Vec::new(),
        }));
    };
    let limit = query
        .limit
        .unwrap_or(HISTORY_DEFAULT_LIMIT)
        .clamp(1, HISTORY_MAX_LIMIT);
    let entries = db.list_brief_history(&space, limit).await?;
    Ok(Json(BriefHistoryResponse {
        space: Some(space),
        entries,
    }))
}

/// GET /api/brief/history/{version} — the full Brief as it stood right after
/// that update. 404 when the Space has no such recorded version.
pub async fn handle_brief_version(
    State(state): State<SharedState>,
    SpaceHeader(header_space): SpaceHeader,
    Path(version): Path<i64>,
    Query(query): Query<BriefHistoryQuery>,
) -> Result<Json<BriefVersionResponse>, ServerError> {
    let db = {
        let state = state.read().await;
        state.db.clone().ok_or(ServerError::DbNotInitialized)?
    };
    let space = resolve_space(&db, query.space.as_deref(), header_space.as_deref())
        .await?
        .ok_or_else(|| ServerError::NotFound("no Space resolved".into()))?;
    db.get_brief_version(&space, version)
        .await?
        .map(Json)
        .ok_or_else(|| {
            ServerError::NotFound(format!("no Brief version {version} in Space '{space}'"))
        })
}

/// GET /api/brief/plain — the plain lines the night wrote for the resolved
/// Space's Brief (UX-SPACE-2), for the Space's page. Lines written for an
/// older Brief version are withheld so the page falls back to the current
/// Brief's own words until the next night writes the new version.
pub async fn handle_brief_plain(
    State(state): State<SharedState>,
    SpaceHeader(header_space): SpaceHeader,
    Query(query): Query<BriefHistoryQuery>,
) -> Result<Json<wenlan_types::BriefPlainView>, ServerError> {
    let db = {
        let state = state.read().await;
        state.db.clone().ok_or(ServerError::DbNotInitialized)?
    };
    let Some(space) = resolve_space(&db, query.space.as_deref(), header_space.as_deref()).await?
    else {
        return Ok(Json(wenlan_types::BriefPlainView::default()));
    };
    let Some(record) = db.get_space(&space).await? else {
        return Ok(Json(wenlan_types::BriefPlainView::default()));
    };
    let current_version = db
        .get_brief_by_space_name(&space)
        .await?
        .map(|brief| brief.version);
    let plain = crate::brief_plain_night::load(&db, &record.id)
        .await
        .filter(|plain| Some(plain.brief_version) == current_version);
    Ok(Json(match plain {
        Some(plain) => wenlan_types::BriefPlainView {
            space: Some(space),
            brief_version: Some(plain.brief_version),
            summary: plain.summary,
            lines: plain.lines,
        },
        None => wenlan_types::BriefPlainView {
            space: Some(space),
            ..Default::default()
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn no_topic_never_invokes_related_context_loader() {
        let calls = AtomicUsize::new(0);
        let result = related_context_if_requested(None, |_| async {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(Vec::new())
        })
        .await
        .unwrap();

        assert!(result.is_none());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn topic_invokes_related_context_loader_once_without_rewriting_it() {
        let calls = AtomicUsize::new(0);
        let result = related_context_if_requested(Some("  release gate  "), |query| {
            calls.fetch_add(1, Ordering::SeqCst);
            async move {
                assert_eq!(query, "release gate");
                Ok(Vec::new())
            }
        })
        .await
        .unwrap()
        .unwrap();

        assert_eq!(result.query, "release gate");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
