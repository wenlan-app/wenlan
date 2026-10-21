// SPDX-License-Identifier: Apache-2.0
use crate::error::ServerError;
use crate::route_registry::{delete, post, TrackedRouter};
use crate::state::{ServerState, SharedState};
use crate::telemetry::TelemetryEvent;
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::Json,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use wenlan_types::sources::RawDocument;
use wenlan_types::WriteSpaceTarget;

pub(crate) fn register(router: TrackedRouter<SharedState>) -> TrackedRouter<SharedState> {
    router
        .route("/api/ingest/text", post(handle_ingest_text))
        .route("/api/ingest/webpage", post(handle_ingest_webpage))
        .route("/api/ingest/webpage/create", post(handle_create_webpage))
        .route("/api/ingest/memory", post(handle_ingest_memory))
        .route(
            "/api/documents/{source}/{source_id}",
            delete(handle_delete_document),
        )
}

// ===== Request/Response Types =====

#[derive(Debug, Deserialize)]
pub struct IngestTextRequest {
    pub source: String,
    pub source_id: String,
    pub title: String,
    pub content: String,
    pub url: Option<String>,
    pub metadata: Option<HashMap<String, String>>,
}

#[derive(Debug, Deserialize)]
pub struct IngestWebpageRequest {
    pub url: String,
    pub title: String,
    pub content: String,
    pub metadata: Option<HashMap<String, String>>,
    /// Omitted inherits the Space header, then the default Space.
    #[serde(default, alias = "domain")]
    pub space: WriteSpaceTarget,
}

#[derive(Debug, Deserialize)]
pub struct IngestMemoryRequest {
    pub source: String,
    pub source_id: String,
    pub title: String,
    pub content: String,
    pub url: Option<String>,
    pub tags: Option<Vec<String>>,
    pub metadata: Option<HashMap<String, String>>,
}

#[derive(Debug, Serialize)]
pub struct IngestResponse {
    pub chunks_created: usize,
    pub document_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub space: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DeleteResponse {
    pub deleted: bool,
}

// ===== Route Handlers =====

/// POST /api/ingest/text
pub async fn handle_ingest_text(
    State(state): State<Arc<RwLock<ServerState>>>,
    Json(req): Json<IngestTextRequest>,
) -> Result<Json<IngestResponse>, ServerError> {
    let telemetry = { state.read().await.telemetry.clone() };
    let result = handle_ingest_text_inner(State(state), Json(req)).await;
    telemetry.record(if result.is_ok() {
        TelemetryEvent::SaveSuccess
    } else {
        TelemetryEvent::SaveError
    });
    result
}

async fn handle_ingest_text_inner(
    State(state): State<Arc<RwLock<ServerState>>>,
    Json(req): Json<IngestTextRequest>,
) -> Result<Json<IngestResponse>, ServerError> {
    let document_id = req.source_id.clone();

    let doc = RawDocument {
        source: req.source,
        source_id: req.source_id,
        title: req.title,
        summary: None,
        content: req.content,
        url: req.url,
        last_modified: chrono::Utc::now().timestamp(),
        metadata: req.metadata.unwrap_or_default(),
        memory_type: None,
        source_agent: None,
        space: None,
        confidence: None,
        confirmed: None,
        supersedes: None,
        pending_revision: false,
        ..Default::default()
    };

    // Snapshot the DB Arc and drop the guard before the upsert: it embeds and
    // writes a transaction, the heaviest await in the daemon, and tokio's
    // write-preferring RwLock would queue every other handler behind it
    // (AGENTS.md: never hold a tokio RwLock guard across .await).
    let db = {
        let s = state.read().await;
        s.db.clone().ok_or(ServerError::DbNotInitialized)?
    };
    let chunks_created = db
        .upsert_documents(vec![doc])
        .await
        .map_err(|e| ServerError::IngestFailed(e.to_string()))?;

    Ok(Json(IngestResponse {
        chunks_created,
        document_id,
        space: None,
    }))
}

/// POST /api/ingest/webpage
pub async fn handle_ingest_webpage(
    State(state): State<Arc<RwLock<ServerState>>>,
    headers: HeaderMap,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    Json(req): Json<IngestWebpageRequest>,
) -> Result<Json<IngestResponse>, ServerError> {
    handle_webpage_with_telemetry(state, &headers, header_space, req, false).await
}

/// POST /api/ingest/webpage/create — create only, never replace an existing excerpt.
pub async fn handle_create_webpage(
    State(state): State<Arc<RwLock<ServerState>>>,
    headers: HeaderMap,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    Json(req): Json<IngestWebpageRequest>,
) -> Result<Json<IngestResponse>, ServerError> {
    handle_webpage_with_telemetry(state, &headers, header_space, req, true).await
}

async fn handle_webpage_with_telemetry(
    state: Arc<RwLock<ServerState>>,
    headers: &HeaderMap,
    header_space: Option<String>,
    req: IngestWebpageRequest,
    create_only: bool,
) -> Result<Json<IngestResponse>, ServerError> {
    let telemetry = { state.read().await.telemetry.clone() };
    let result = handle_ingest_webpage_inner(state, headers, header_space, req, create_only).await;
    telemetry.record(if result.is_ok() {
        TelemetryEvent::SaveSuccess
    } else {
        TelemetryEvent::SaveError
    });
    result
}

async fn handle_ingest_webpage_inner(
    state: Arc<RwLock<ServerState>>,
    headers: &HeaderMap,
    header_space: Option<String>,
    req: IngestWebpageRequest,
    create_only: bool,
) -> Result<Json<IngestResponse>, ServerError> {
    let document_id = req.url.clone();

    // Snapshot the DB Arc and drop the guard before any await on it: the
    // upsert embeds and writes a transaction, the heaviest await in the
    // daemon, and tokio's write-preferring RwLock would queue every other
    // handler behind it (AGENTS.md: never hold a tokio RwLock guard across
    // .await).
    let db = {
        let s = state.read().await;
        s.db.clone().ok_or(ServerError::DbNotInitialized)?
    };
    // Same Space precedence as /api/memory/store: body, then header, then
    // the default Space.
    let resolved_write_space = db
        .resolve_write_space(&req.space, header_space.as_deref())
        .await?;
    // An agent saving a source it read elsewhere is a write like capture: a
    // disabled agent is refused, and the source records who saved it.
    let agent = crate::memory_routes::extract_agent_name(headers, None);
    let source_agent = if agent == "unknown" {
        None
    } else {
        db.check_agent_for_write(&agent)
            .await
            .map_err(ServerError::from)?;
        Some(agent)
    };

    let mut metadata = req.metadata.unwrap_or_default();
    if let Some(domain) = req
        .url
        .split("://")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
    {
        // Metadata blob key kept as "domain" for downstream-reader back-compat.
        metadata.insert("domain".to_string(), domain.to_string());
    }

    let doc = RawDocument {
        source: "webpage".to_string(),
        source_id: req.url.clone(),
        title: req.title,
        summary: None,
        content: req.content,
        url: Some(req.url),
        last_modified: chrono::Utc::now().timestamp(),
        metadata,
        memory_type: None,
        source_agent,
        space: resolved_write_space.space_name.clone(),
        confidence: None,
        confirmed: None,
        supersedes: None,
        pending_revision: false,
        ..Default::default()
    };

    let space = resolved_write_space.space_name.clone();
    let docs = vec![(doc, Some(resolved_write_space))];
    let chunks_created = if create_only {
        db.create_documents_with_write_spaces(docs).await
    } else {
        db.upsert_documents_with_write_spaces(docs).await
    }
    .map_err(|e| match e {
        wenlan_core::WenlanError::Conflict(message) => ServerError::Conflict(message),
        wenlan_core::WenlanError::Validation(message) => ServerError::ValidationError(message),
        other => ServerError::IngestFailed(other.to_string()),
    })?;

    Ok(Json(IngestResponse {
        chunks_created,
        document_id,
        space,
    }))
}

/// POST /api/ingest/memory
pub async fn handle_ingest_memory(
    State(state): State<Arc<RwLock<ServerState>>>,
    Json(req): Json<IngestMemoryRequest>,
) -> Result<Json<IngestResponse>, ServerError> {
    let telemetry = { state.read().await.telemetry.clone() };
    let result = handle_ingest_memory_inner(State(state), Json(req)).await;
    telemetry.record(if result.is_ok() {
        TelemetryEvent::SaveSuccess
    } else {
        TelemetryEvent::SaveError
    });
    result
}

async fn handle_ingest_memory_inner(
    State(state): State<Arc<RwLock<ServerState>>>,
    Json(req): Json<IngestMemoryRequest>,
) -> Result<Json<IngestResponse>, ServerError> {
    let trimmed_content = req.content.trim();
    if trimmed_content.chars().count() < 10 {
        return Err(ServerError::ValidationError(
            "Memory content must be at least 10 characters".into(),
        ));
    }

    let document_id = req.source_id.clone();

    let mut metadata = req.metadata.unwrap_or_default();
    if let Some(tags) = req.tags {
        metadata.insert("tags".to_string(), tags.join(","));
    }

    let doc = RawDocument {
        source: req.source,
        source_id: req.source_id,
        title: req.title,
        summary: None,
        content: req.content,
        url: req.url,
        last_modified: chrono::Utc::now().timestamp(),
        metadata,
        memory_type: None,
        source_agent: None,
        space: None,
        confidence: None,
        confirmed: None,
        supersedes: None,
        pending_revision: false,
        ..Default::default()
    };

    // Snapshot the DB Arc and drop the guard before the upsert: it embeds and
    // writes a transaction, the heaviest await in the daemon, and tokio's
    // write-preferring RwLock would queue every other handler behind it
    // (AGENTS.md: never hold a tokio RwLock guard across .await).
    let db = {
        let s = state.read().await;
        s.db.clone().ok_or(ServerError::DbNotInitialized)?
    };
    let chunks_created = db
        .upsert_documents(vec![doc])
        .await
        .map_err(|e| ServerError::IngestFailed(e.to_string()))?;

    Ok(Json(IngestResponse {
        chunks_created,
        document_id,
        space: None,
    }))
}

/// DELETE /api/documents/:source/:source_id
pub async fn handle_delete_document(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path((source, source_id)): Path<(String, String)>,
) -> Result<Json<DeleteResponse>, ServerError> {
    let db = {
        let s = state.read().await;
        s.db.clone().ok_or(ServerError::DbNotInitialized)?
    };
    db.delete_by_source_id(&source, &source_id)
        .await
        .map_err(|e| ServerError::Internal(e.to_string()))?;
    Ok(Json(DeleteResponse { deleted: true }))
}

#[cfg(test)]
mod content_length_gate_tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    async fn empty_state() -> (Arc<RwLock<ServerState>>, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let db = Arc::new(
            wenlan_core::db::MemoryDB::new(tmp.path(), Arc::new(wenlan_core::events::NoopEmitter))
                .await
                .unwrap(),
        );
        let state = Arc::new(RwLock::new(ServerState {
            db: Some(db),
            ..ServerState::default()
        }));
        (state, tmp)
    }

    fn ingest_request(content: &str) -> IngestMemoryRequest {
        IngestMemoryRequest {
            source: "memory".to_string(),
            source_id: "mem_length_gate".to_string(),
            title: "length gate test".to_string(),
            content: content.to_string(),
            url: None,
            tags: None,
            metadata: None,
        }
    }

    /// 9 ASCII characters must still be rejected by the minimum-length gate.
    #[tokio::test]
    async fn nine_ascii_chars_are_rejected() {
        let (state, _tmp) = empty_state().await;
        let result =
            handle_ingest_memory_inner(State(state), Json(ingest_request("123456789"))).await;
        assert!(matches!(
            result,
            Err(ServerError::ValidationError(ref m)) if m == "Memory content must be at least 10 characters"
        ));
    }

    /// 10 Chinese characters is the same "10" floor as ASCII because the gate
    /// counts `chars()` instead of UTF-8 bytes (each CJK char is 3 bytes, so
    /// a byte-length gate would pass this at just 4 characters).
    #[tokio::test]
    async fn ten_chinese_chars_are_accepted() {
        let (state, _tmp) = empty_state().await;
        let result =
            handle_ingest_memory_inner(State(state), Json(ingest_request("今天天氣非常晴朗好啊")))
                .await;
        assert!(
            result.is_ok(),
            "10-char CJK content should clear the length floor: {result:?}"
        );
    }

    /// 4 Chinese characters must be rejected: a byte-length gate
    /// (`content.len() < 10`) would pass this at 12 UTF-8 bytes.
    #[tokio::test]
    async fn four_chinese_chars_are_rejected() {
        let (state, _tmp) = empty_state().await;
        let result =
            handle_ingest_memory_inner(State(state), Json(ingest_request("你好嗎呀"))).await;
        assert!(matches!(
            result,
            Err(ServerError::ValidationError(ref m)) if m == "Memory content must be at least 10 characters"
        ));
    }

    /// 9 Chinese characters is 27 UTF-8 bytes: a byte-length gate
    /// (`content.len() < 10`) would wrongly accept this at 27 >= 10.
    /// The char-counting gate must reject it at 9 < 10.
    #[tokio::test]
    async fn nine_chinese_chars_are_rejected() {
        let (state, _tmp) = empty_state().await;
        let result =
            handle_ingest_memory_inner(State(state), Json(ingest_request("今天天氣非常晴朗好")))
                .await;
        assert!(matches!(
            result,
            Err(ServerError::ValidationError(ref m)) if m == "Memory content must be at least 10 characters"
        ));
    }
}

#[cfg(test)]
mod webpage_create_tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    #[tokio::test]
    async fn concurrent_webpage_create_preserves_winner_and_explicit_replace_still_works() {
        let temp = tempfile::tempdir().unwrap();
        let db = Arc::new(
            wenlan_core::db::MemoryDB::new(temp.path(), Arc::new(wenlan_core::events::NoopEmitter))
                .await
                .unwrap(),
        );
        let state = Arc::new(RwLock::new(ServerState {
            db: Some(db.clone()),
            ..Default::default()
        }));
        let app = crate::router::build_router(state);
        let url = "https://example.com/concurrent-create";
        let request = |path: &str, content: &str| {
            Request::builder()
            .method("POST").uri(path).header("content-type", "application/json")
            .body(Body::from(serde_json::json!({ "url": url, "title": "Concurrent excerpt", "content": content }).to_string()))
            .unwrap()
        };
        let first = "First writer document must remain intact when it wins.";
        let second = "Second writer document must remain intact when it wins.";
        let (one, two) = tokio::join!(
            app.clone()
                .oneshot(request("/api/ingest/webpage/create", first)),
            app.clone()
                .oneshot(request("/api/ingest/webpage/create", second)),
        );
        let one = one.unwrap();
        let two = two.unwrap();
        let winner = match (one.status(), two.status()) {
            (StatusCode::OK, StatusCode::CONFLICT) => first,
            (StatusCode::CONFLICT, StatusCode::OK) => second,
            statuses => panic!("Expected exactly one create and one conflict: {statuses:?}"),
        };
        let chunks = db
            .get_webpage_chunks_scoped(url, &wenlan_core::read_scope::ReadScope::Global)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            chunks
                .iter()
                .map(|chunk| chunk.content.as_str())
                .collect::<Vec<_>>(),
            vec![winner]
        );
        let replacement = "Deliberately replaced after the user confirmed the existing excerpt.";
        let response = app
            .oneshot(request("/api/ingest/webpage", replacement))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let chunks = db
            .get_webpage_chunks_scoped(url, &wenlan_core::read_scope::ReadScope::Global)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            chunks
                .iter()
                .map(|chunk| chunk.content.as_str())
                .collect::<Vec<_>>(),
            vec![replacement]
        );
    }
}
