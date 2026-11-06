// SPDX-License-Identifier: Apache-2.0
//! Durable organizing state; original content remains in Sources and Pages.
use crate::{
    error::ServerError,
    route_registry::{get, TrackedRouter},
    space_header::SpaceHeader,
    state::SharedState,
};
use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::Deserialize;
use std::sync::Arc;
use wenlan_core::db::MemoryDB;
use wenlan_types::research::*;

pub(crate) fn register(router: TrackedRouter<SharedState>) -> TrackedRouter<SharedState> {
    router
        .route(
            "/api/research/merge-tags",
            crate::route_registry::post(merge_tags),
        )
        .route("/api/research/rules", get(get_rules).post(save_rules))
        .route("/api/research/runs", get(list_runs).post(create_run))
        .route("/api/research/runs/{id}", get(get_run).post(update_run))
        .route(
            "/api/research/runs/{id}/read-source",
            crate::route_registry::post(read_source),
        )
        .route(
            "/api/research/classifications",
            get(get_decisions).post(propose),
        )
}
#[derive(Deserialize)]
pub struct ResearchQuery {
    pub space: String,
    pub source: Option<String>,
    pub source_id: Option<String>,
}
async fn scoped_database(
    db: Arc<MemoryDB>,
    space: &str,
    header: Option<&str>,
) -> Result<(Arc<MemoryDB>, String), ServerError> {
    if space.trim().is_empty() || header.is_some_and(|s| s != space) {
        return Err(ServerError::BadRequest(
            "explicit research Space and header must agree".into(),
        ));
    }
    let registered = db
        .get_space(space)
        .await?
        .ok_or_else(|| ServerError::NotFound("registered research Space not found".into()))?;
    Ok((db, registered.id))
}
async fn scoped(
    state: &SharedState,
    space: &str,
    header: Option<&str>,
) -> Result<(Arc<MemoryDB>, String), ServerError> {
    let db = {
        let s = state.read().await;
        s.db.clone().ok_or(ServerError::DbNotInitialized)?
    };
    scoped_database(db, space, header).await
}
async fn guarded_scoped(
    state: &SharedState,
    space: &str,
    header: Option<&str>,
) -> Result<
    (
        Arc<MemoryDB>,
        String,
        crate::maintenance_coordinator::BackgroundMaintenanceGuard,
    ),
    ServerError,
> {
    let (db, coordinator) = {
        let s = state.read().await;
        (
            s.db.clone().ok_or(ServerError::DbNotInitialized)?,
            s.maintenance_coordinator.clone(),
        )
    };
    let guard = coordinator
        .try_begin_background()
        .ok_or_else(|| ServerError::Conflict("repair_write_fence_conflict".into()))?;
    let (db, id) = scoped_database(db, space, header).await?;
    Ok((db, id, guard))
}
async fn get_rules(
    State(state): State<SharedState>,
    SpaceHeader(header): SpaceHeader,
    Query(q): Query<ResearchQuery>,
) -> Result<Json<OrganizeRules>, ServerError> {
    let (db, id) = scoped(&state, &q.space, header.as_deref()).await?;
    Ok(Json(db.research_rules(&id).await?))
}
async fn save_rules(
    State(state): State<SharedState>,
    SpaceHeader(header): SpaceHeader,
    Json(req): Json<SaveOrganizeRulesRequest>,
) -> Result<Json<OrganizeRules>, ServerError> {
    let (db, id) = scoped(&state, &req.space, header.as_deref()).await?;
    Ok(Json(db.save_research_rules(&id, req).await?))
}
async fn list_runs(
    State(state): State<SharedState>,
    SpaceHeader(header): SpaceHeader,
    Query(q): Query<ResearchQuery>,
) -> Result<Json<ResearchRunsResponse>, ServerError> {
    let (db, id) = scoped(&state, &q.space, header.as_deref()).await?;
    Ok(Json(db.research_runs(&id).await?))
}
async fn create_run(
    State(state): State<SharedState>,
    SpaceHeader(header): SpaceHeader,
    Json(req): Json<CreateOrganizeRunRequest>,
) -> Result<Json<OrganizeRun>, ServerError> {
    let (db, id) = scoped(&state, &req.space, header.as_deref()).await?;
    Ok(Json(db.create_research_run(&id, req).await?))
}
async fn get_run(
    State(state): State<SharedState>,
    SpaceHeader(header): SpaceHeader,
    Path(run): Path<String>,
    Query(q): Query<ResearchQuery>,
) -> Result<Json<OrganizeRun>, ServerError> {
    let (db, id) = scoped(&state, &q.space, header.as_deref()).await?;
    Ok(Json(db.research_run(&id, &run).await?))
}
async fn read_source(
    State(state): State<SharedState>,
    SpaceHeader(header): SpaceHeader,
    Path(run): Path<String>,
    Json(request): Json<ReadOrganizeSourceRequest>,
) -> Result<Json<ReadOrganizeSourceResponse>, ServerError> {
    let (db, id) = scoped(&state, &request.space, header.as_deref()).await?;
    Ok(Json(db.read_organize_source(&id, &run, request).await?))
}
async fn update_run(
    State(state): State<SharedState>,
    SpaceHeader(header): SpaceHeader,
    Path(run): Path<String>,
    Json(req): Json<UpdateOrganizeRunRequest>,
) -> Result<Json<OrganizeRun>, ServerError> {
    let (db, id) = scoped(&state, &req.space, header.as_deref()).await?;
    Ok(Json(db.update_research_run(&id, &run, req).await?))
}
async fn get_decisions(
    State(state): State<SharedState>,
    SpaceHeader(header): SpaceHeader,
    Query(q): Query<ResearchQuery>,
) -> Result<Json<SourceClassificationDecisions>, ServerError> {
    let (db, id) = scoped(&state, &q.space, header.as_deref()).await?;
    Ok(Json(
        db.research_classification_decisions(
            &id,
            q.source
                .as_deref()
                .ok_or_else(|| ServerError::BadRequest("source required".into()))?,
            q.source_id
                .as_deref()
                .ok_or_else(|| ServerError::BadRequest("source_id required".into()))?,
        )
        .await?,
    ))
}
async fn propose(
    State(state): State<SharedState>,
    SpaceHeader(header): SpaceHeader,
    Json(req): Json<ProposeSourceClassificationsRequest>,
) -> Result<Json<ClassificationProposalResult>, ServerError> {
    let (db, id, _maintenance_guard) =
        guarded_scoped(&state, &req.space, header.as_deref()).await?;
    Ok(Json(db.propose_research_classifications(&id, req).await?))
}

async fn merge_tags(
    State(state): State<SharedState>,
    SpaceHeader(header): SpaceHeader,
    Json(req): Json<MergeResearchTagsRequest>,
) -> Result<Json<MergeResearchTagsResponse>, ServerError> {
    let (db, id, _maintenance_guard) =
        guarded_scoped(&state, &req.space, header.as_deref()).await?;
    Ok(Json(db.merge_research_tags(&id, req).await?))
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;
    #[tokio::test]
    async fn research_routes_reject_conflicting_explicit_scope_before_storage() {
        let state = std::sync::Arc::new(tokio::sync::RwLock::new(
            crate::state::ServerState::default(),
        ));
        let app = crate::router::build_router(state);
        for path in [
            "/api/research/rules?space=work",
            "/api/research/runs?space=work",
            "/api/research/runs/run?space=work",
            "/api/research/classifications?space=work&source=file&source_id=paper",
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
    }
}

#[cfg(test)]
mod source_read_tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;
    #[tokio::test]
    async fn run_source_read_returns_exact_slice_and_persists_only_real_prefix() {
        let root = tempfile::tempdir().unwrap();
        let db = Arc::new(
            MemoryDB::new(root.path(), Arc::new(wenlan_core::events::NoopEmitter))
                .await
                .unwrap(),
        );
        let space = db.create_space("Research", None, false).await.unwrap();
        db.create_space("Other", None, false).await.unwrap();
        let fixture = libsql::Builder::new_local(root.path().join("origin_memory.db"))
            .build()
            .await
            .unwrap();
        fixture.connect().unwrap().execute("INSERT INTO memories(id,source,source_id,title,content,space,chunk_index,last_modified,chunk_type) VALUES('paper','file','paper','Paper','甲🙂ab終','Research',0,1,'text')",()).await.unwrap();
        let run = db
            .create_research_run(
                &space.id,
                CreateOrganizeRunRequest {
                    space: "Research".into(),
                    sources: vec![ResearchSourceRef {
                        source: "file".into(),
                        source_id: "paper".into(),
                        title: String::new(),
                    }],
                    instructions: None,
                    target_page_id: None,
                },
            )
            .await
            .unwrap();
        let state = Arc::new(tokio::sync::RwLock::new(crate::state::ServerState {
            db: Some(db.clone()),
            ..Default::default()
        }));
        let router = crate::router::build_router(state);
        let uri = format!("/api/research/runs/{}/read-source", run.id);
        for (header, offset, expected) in [
            ("Other", 0, StatusCode::BAD_REQUEST),
            ("Research", 3, StatusCode::UNPROCESSABLE_ENTITY),
            ("Research", 0, StatusCode::OK),
        ] {
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(&uri)
                        .header("x-wenlan-space", header)
                        .header("content-type", "application/json")
                        .body(Body::from(
                            serde_json::to_vec(&ReadOrganizeSourceRequest {
                                space: "Research".into(),
                                source: "file".into(),
                                source_id: "paper".into(),
                                offset,
                                limit: 2,
                            })
                            .unwrap(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), expected);
            if expected == StatusCode::OK {
                let bytes = axum::body::to_bytes(response.into_body(), 1048576)
                    .await
                    .unwrap();
                let read: ReadOrganizeSourceResponse = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(read.text, "甲🙂");
                assert_eq!(read.served_chars, 2);
                assert_eq!(read.total_chars, 5);
                assert_eq!(read.next_offset, Some(2));
                assert!(!read.complete);
            }
        }
        let stored = db.research_run(&space.id, &run.id).await.unwrap();
        assert_eq!(stored.items[0].served_chars, 2);
        assert_eq!(stored.items[0].coverage, ResearchCoverage::Reading);
        fixture
            .connect()
            .unwrap()
            .execute("UPDATE memories SET content='' WHERE id='paper'", ())
            .await
            .unwrap();
        let empty = db
            .create_research_run(
                &space.id,
                CreateOrganizeRunRequest {
                    space: "Research".into(),
                    sources: vec![ResearchSourceRef {
                        source: "file".into(),
                        source_id: "paper".into(),
                        title: String::new(),
                    }],
                    instructions: None,
                    target_page_id: None,
                },
            )
            .await
            .unwrap();
        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/research/runs/{}/read-source", empty.id))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&ReadOrganizeSourceRequest {
                            space: "Research".into(),
                            source: "file".into(),
                            source_id: "paper".into(),
                            offset: 0,
                            limit: 2,
                        })
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            db.research_run(&space.id, &empty.id).await.unwrap().items[0].served_total_chars,
            None
        );
    }
}

#[cfg(test)]
mod maintenance_tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;
    async fn post(
        router: &crate::route_registry::AppRouter,
        uri: &str,
        body: serde_json::Value,
    ) -> axum::response::Response {
        router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap()
    }
    #[tokio::test]
    async fn maintenance_fence_protects_research_queue_and_tag_mutations() {
        let root = tempfile::tempdir().unwrap();
        let db = Arc::new(
            MemoryDB::new(root.path(), Arc::new(wenlan_core::events::NoopEmitter))
                .await
                .unwrap(),
        );
        let space = db.create_space("Research", None, false).await.unwrap();
        let fixture = libsql::Builder::new_local(root.path().join("origin_memory.db"))
            .build()
            .await
            .unwrap();
        fixture.connect().unwrap().execute("INSERT INTO memories(id,source,source_id,title,content,space,chunk_index,last_modified,chunk_type) VALUES('paper','file','paper','Paper','actual complete text','Research',0,1,'text')",()).await.unwrap();
        db.set_document_tags("file", "paper", vec!["old".into()])
            .await
            .unwrap();
        let value = db
            .create_research_run(
                &space.id,
                CreateOrganizeRunRequest {
                    space: "Research".into(),
                    sources: vec![ResearchSourceRef {
                        source: "file".into(),
                        source_id: "paper".into(),
                        title: String::new(),
                    }],
                    instructions: None,
                    target_page_id: None,
                },
            )
            .await
            .unwrap();
        let read = db
            .read_organize_source(
                &space.id,
                &value.id,
                ReadOrganizeSourceRequest {
                    space: "Research".into(),
                    source: "file".into(),
                    source_id: "paper".into(),
                    offset: 0,
                    limit: 16000,
                },
            )
            .await
            .unwrap();
        let value = db
            .update_research_run(
                &space.id,
                &value.id,
                UpdateOrganizeRunRequest {
                    space: "Research".into(),
                    expected_revision: read.run_revision,
                    source: Some("file".into()),
                    source_id: Some("paper".into()),
                    coverage: Some(ResearchCoverage::Complete),
                    content_digest: Some(value.items[0].content_digest.clone()),
                    next_offset: None,
                    error: None,
                    cancel: false,
                    resume: false,
                },
            )
            .await
            .unwrap();
        let change = SourceClassificationChange {
            source: "file".into(),
            source_id: "paper".into(),
            source_content_digest: value.items[0].content_digest.clone(),
            expected_decision_revision: value.items[0].decision_revision,
            add_tags: vec!["new".into()],
            remove_tags: vec![],
            reason: "Actual read".into(),
            title: String::new(),
            before_tags: vec![],
        };
        let original = db
            .propose_research_classifications(
                &space.id,
                ProposeSourceClassificationsRequest {
                    space: "Research".into(),
                    run_id: value.id.clone(),
                    changes: vec![change.clone()],
                },
            )
            .await
            .unwrap();
        let another = ProposeSourceClassificationsRequest {
            space: "Research".into(),
            run_id: value.id,
            changes: vec![SourceClassificationChange {
                add_tags: vec!["proposal2".into()],
                ..change.clone()
            }],
        };
        let merge = serde_json::json!({"space":"Research","sources":[{"source":"file","source_id":"paper","title":""}],"from_tag":"old","into_tag":"merged"});
        let state = Arc::new(tokio::sync::RwLock::new(crate::state::ServerState {
            db: Some(db.clone()),
            ..Default::default()
        }));
        let router = crate::router::build_router(state.clone());
        let accept = format!("/api/refinery/queue/{}/accept?space=Research", original.id);
        for (uri, body) in [
            (accept.as_str(), serde_json::json!({})),
            (
                accept.as_str(),
                serde_json::json!({"action":"classification_edit","changes":[change]}),
            ),
            (
                "/api/research/classifications",
                serde_json::to_value(&another).unwrap(),
            ),
            ("/api/research/merge-tags", merge.clone()),
        ] {
            assert_eq!(
                post(&router, uri, body).await.status(),
                StatusCode::CONFLICT,
                "{uri}"
            );
            assert_eq!(
                db.get_document_tags("file", "paper").await.unwrap(),
                vec!["old".to_string()]
            );
            assert_eq!(
                db.get_refinement_proposal(&original.id)
                    .await
                    .unwrap()
                    .unwrap()
                    .status,
                "awaiting_review"
            );
            assert_eq!(db.get_pending_refinements().await.unwrap().len(), 1);
        }
        // Positive controls prove every sealed request was otherwise actionable.
        state.read().await.maintenance_coordinator.finish_recovery();
        assert_eq!(
            post(
                &router,
                "/api/research/classifications",
                serde_json::to_value(&another).unwrap()
            )
            .await
            .status(),
            StatusCode::OK
        );
        assert_eq!(db.get_pending_refinements().await.unwrap().len(), 2);
        assert_eq!(
            post(&router, &accept, serde_json::json!({})).await.status(),
            StatusCode::OK
        );
        assert_eq!(
            db.get_refinement_proposal(&original.id)
                .await
                .unwrap()
                .unwrap()
                .status,
            "resolved"
        );
        assert_eq!(
            post(&router, "/api/research/merge-tags", merge)
                .await
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            db.get_document_tags("file", "paper").await.unwrap(),
            vec!["merged".to_string(), "new".to_string()]
        );
    }
}
