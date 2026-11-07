// SPDX-License-Identifier: Apache-2.0
//! Explicit status changes retain source bytes, tags and human prose.
use crate::{
    error::ServerError,
    route_registry::{get, TrackedRouter},
    space_header::SpaceHeader,
    state::SharedState,
};
use axum::{
    extract::{Query, State},
    Json,
};
use serde::Deserialize;
use std::sync::Arc;
use wenlan_core::db::MemoryDB;
use wenlan_types::source_lifecycle::{
    SetSourceLifecycleRequest, SourceLifecycleResponse, SourceReferenceIdentity,
};

pub(crate) fn register(router: TrackedRouter<SharedState>) -> TrackedRouter<SharedState> {
    router
        .route("/api/research/source-lifecycle", get(read).post(write))
        .route("/api/research/source-lifecycle/reference", get(reference))
}

#[derive(Deserialize)]
pub struct SourceLifecycleQuery {
    pub space: String,
    pub source: String,
    pub source_id: String,
}

#[derive(Deserialize)]
pub struct ReferenceQuery {
    pub space: String,
    pub locator: String,
}

async fn reference(
    State(state): State<SharedState>,
    SpaceHeader(header): SpaceHeader,
    Query(query): Query<ReferenceQuery>,
) -> Result<Json<SourceReferenceIdentity>, ServerError> {
    let (db, space_id) = scoped(&state, &query.space, header.as_deref()).await?;
    Ok(Json(
        db.resolve_source_reference(&space_id, &query.locator)
            .await?,
    ))
}

async fn scoped(
    state: &SharedState,
    space: &str,
    header: Option<&str>,
) -> Result<(Arc<MemoryDB>, String), ServerError> {
    crate::research_routes::scoped_space(state, space, header, "source").await
}

async fn read(
    State(state): State<SharedState>,
    SpaceHeader(header): SpaceHeader,
    Query(query): Query<SourceLifecycleQuery>,
) -> Result<Json<SourceLifecycleResponse>, ServerError> {
    let (db, space_id) = scoped(&state, &query.space, header.as_deref()).await?;
    Ok(Json(
        db.source_lifecycle(&space_id, &query.source, &query.source_id)
            .await?,
    ))
}

async fn write(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
    SpaceHeader(header): SpaceHeader,
    Json(request): Json<SetSourceLifecycleRequest>,
) -> Result<Json<SourceLifecycleResponse>, ServerError> {
    let (db, space_id) = scoped(&state, &request.space, header.as_deref()).await?;
    let coordinator = { state.read().await.maintenance_coordinator.clone() };
    let _guard = coordinator
        .try_begin_background()
        .ok_or_else(|| ServerError::Conflict("repair_write_fence_conflict".into()))?;
    let actor = crate::memory_routes::extract_agent_name(&headers, None);
    Ok(Json(
        db.set_source_lifecycle(&space_id, request, &actor).await?,
    ))
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    #[tokio::test]
    async fn lifecycle_routes_require_matching_explicit_scope_before_storage() {
        let state = std::sync::Arc::new(tokio::sync::RwLock::new(
            crate::state::ServerState::default(),
        ));
        let app = crate::router::build_router(state);
        for path in [
            "/api/research/source-lifecycle?space=work&source=file&source_id=paper",
            "/api/research/source-lifecycle/reference?space=work&locator=chunk",
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(path)
                        .header("x-wenlan-space", "private")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{path}");
        }
        let response=app.oneshot(Request::builder().method("POST").uri("/api/research/source-lifecycle").header("x-wenlan-space","private").header("content-type","application/json").body(Body::from(r#"{"space":"work","source":"file","source_id":"paper","expected_revision":0,"state":"retracted","reason":"publisher notice"}"#)).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn lifecycle_write_fence_refuses_sealed_mutation_and_allows_recovered_write() {
        use std::sync::Arc;
        use wenlan_types::source_lifecycle::SourceLifecycleState;

        let temp = tempfile::tempdir().unwrap();
        let db = Arc::new(
            wenlan_core::db::MemoryDB::new(temp.path(), Arc::new(wenlan_core::events::NoopEmitter))
                .await
                .unwrap(),
        );
        let space = db.create_space("Research", None, false).await.unwrap();
        let fixture = libsql::Builder::new_local(temp.path().join("origin_memory.db"))
            .build()
            .await
            .unwrap();
        let conn = fixture.connect().unwrap();
        conn.execute("INSERT INTO memories(id,source,source_id,title,content,space,chunk_index,last_modified,chunk_type) VALUES('paper','file','paper','Paper','Evidence retained.','Research',0,1,'text')",()).await.unwrap();
        conn.execute(
            "INSERT INTO pages(id,title,content,source_memory_ids,created_at,last_compiled,last_modified,space,workspace,kind) VALUES('review','Human review','Human judgment based on evidence.',?1,'2026-11-06','2026-11-06','2026-11-06','Research','Research','authored')",
            [r#"["paper"]"#],
        ).await.unwrap();
        let linked_page = db.get_page("review").await.unwrap().unwrap();
        assert_eq!(linked_page.source_memory_ids, vec!["paper"]);
        let before_source_updates = linked_page.sources_updated_count;
        let before = db
            .source_lifecycle(&space.id, "file", "paper")
            .await
            .unwrap();
        let before_page = serde_json::to_value(db.get_page("review").await.unwrap()).unwrap();
        let state = Arc::new(tokio::sync::RwLock::new(crate::state::ServerState {
            db: Some(db.clone()),
            ..Default::default()
        }));
        let coordinator = state.read().await.maintenance_coordinator.clone();
        let app = crate::router::build_router(state);
        let request = || {
            Request::builder().method("POST")
            .uri("/api/research/source-lifecycle")
            .header("x-wenlan-space", "Research")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"space":"Research","source":"file","source_id":"paper","expected_revision":0,"state":"retracted","reason":"publisher notice"}"#)).unwrap()
        };
        let sealed = app.clone().oneshot(request()).await.unwrap();
        assert_eq!(sealed.status(), StatusCode::CONFLICT);
        let body = axum::body::to_bytes(sealed.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(String::from_utf8_lossy(&body).contains("repair_write_fence_conflict"));
        assert_eq!(
            serde_json::to_value(
                db.source_lifecycle(&space.id, "file", "paper")
                    .await
                    .unwrap()
            )
            .unwrap(),
            serde_json::to_value(before).unwrap(),
        );
        assert_eq!(
            serde_json::to_value(db.get_page("review").await.unwrap()).unwrap(),
            before_page
        );
        let mut rows = conn
            .query("SELECT content FROM memories WHERE id='paper'", ())
            .await
            .unwrap();
        assert_eq!(
            rows.next()
                .await
                .unwrap()
                .unwrap()
                .get::<String>(0)
                .unwrap(),
            "Evidence retained."
        );
        drop(rows);

        coordinator.finish_recovery();
        let recovered = app.oneshot(request()).await.unwrap();
        assert_eq!(recovered.status(), StatusCode::OK);
        let after = db
            .source_lifecycle(&space.id, "file", "paper")
            .await
            .unwrap();
        assert_eq!(after.record.state, SourceLifecycleState::Retracted);
        assert_eq!(after.record.revision, 1);
        let after_page = db.get_page("review").await.unwrap().unwrap();
        assert_eq!(after_page.content, "Human judgment based on evidence.");
        assert_eq!(after_page.stale_reason.as_deref(), Some("source_updated"));
        assert_eq!(after_page.sources_updated_count, before_source_updates + 1);
    }
}
