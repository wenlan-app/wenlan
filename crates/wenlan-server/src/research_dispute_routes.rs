// SPDX-License-Identifier: Apache-2.0
//! Exact-Space dispute staging and explicit human Review decisions.
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
use wenlan_types::research_disputes::*;

pub(crate) fn register(router: TrackedRouter<SharedState>) -> TrackedRouter<SharedState> {
    router
        .route("/api/research/disputes", post(propose))
        .route("/api/research/disputes/{id}", get(read))
        .route("/api/research/disputes/{id}/decision", post(decide))
}
#[derive(Deserialize)]
struct DisputeQuery {
    space: String,
}
async fn scoped(
    state: &SharedState,
    space: &str,
    header: Option<&str>,
) -> Result<(std::sync::Arc<wenlan_core::db::MemoryDB>, String), ServerError> {
    if space.trim().is_empty() || header.is_some_and(|header| header.trim() != space.trim()) {
        return Err(ServerError::BadRequest(
            "explicit dispute Space and header must agree".into(),
        ));
    }
    let db = {
        state
            .read()
            .await
            .db
            .clone()
            .ok_or(ServerError::DbNotInitialized)?
    };
    let scope = crate::read_scope::effective_read_scope(&db, Some(space), header).await?;
    let wenlan_core::read_scope::ReadScope::Space(name) = scope else {
        return Err(ServerError::BadRequest(
            "disputes require a registered named Space".into(),
        ));
    };
    let registered = db
        .get_space(&name)
        .await?
        .ok_or_else(|| ServerError::NotFound("registered dispute Space not found".into()))?;
    Ok((db, registered.id))
}
async fn propose(
    State(state): State<SharedState>,
    SpaceHeader(header): SpaceHeader,
    Json(request): Json<ProposeResearchDisputeRequest>,
) -> Result<Json<ResearchDisputeProposalResult>, ServerError> {
    let (db, space_id) = scoped(&state, &request.space, header.as_deref()).await?;
    let coordinator = { state.read().await.maintenance_coordinator.clone() };
    let _guard = coordinator
        .try_begin_background()
        .ok_or_else(|| ServerError::Conflict("repair_write_fence_conflict".into()))?;
    Ok(Json(db.propose_research_dispute(&space_id, request).await?))
}
async fn read(
    State(state): State<SharedState>,
    SpaceHeader(header): SpaceHeader,
    Path(id): Path<String>,
    Query(query): Query<DisputeQuery>,
) -> Result<Json<ResearchDisputeView>, ServerError> {
    let (db, space_id) = scoped(&state, &query.space, header.as_deref()).await?;
    Ok(Json(db.research_dispute(&space_id, &id).await?))
}
async fn decide(
    State(state): State<SharedState>,
    SpaceHeader(header): SpaceHeader,
    Path(id): Path<String>,
    Json(request): Json<DecideResearchDisputeRequest>,
) -> Result<Json<ResearchDisputeView>, ServerError> {
    let (db, space_id) = scoped(&state, &request.space, header.as_deref()).await?;
    let coordinator = { state.read().await.maintenance_coordinator.clone() };
    let _guard = coordinator
        .try_begin_background()
        .ok_or_else(|| ServerError::Conflict("repair_write_fence_conflict".into()))?;
    // This route is the human Review surface, never an MCP decision tool.
    Ok(Json(
        db.decide_research_dispute(&space_id, &id, request, "user")
            .await?,
    ))
}

/// Only research payloads participate in this narrow scope boundary. Existing global
/// Review proposal kinds retain their established policy.
pub(crate) fn scope_research_payload(
    action: &str,
    payload: &mut Option<wenlan_types::responses::RefinementPayload>,
    registered: &std::collections::HashMap<String, String>,
) -> bool {
    use wenlan_types::responses::RefinementPayload;
    match action {
        "research_dispute" => match payload {
            Some(RefinementPayload::ResearchDispute { dispute, space, .. }) => {
                let Some(name) = registered.get(&dispute.space_id) else {
                    return false;
                };
                *space = name.clone();
                true
            }
            _ => false,
        },
        "source_classification" => {
            matches!(payload,Some(RefinementPayload::SourceClassification{space_id,..}) if registered.contains_key(space_id))
        }
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn queue_filters_private_quotes_and_resolves_current_registered_name() {
        use wenlan_types::responses::RefinementPayload;
        let mut payload = Some(RefinementPayload::ResearchDispute {
            dispute: ResearchDispute {
                space_id: "immutable-id".into(),
                kind: ResearchDisputeKind::DifferentDefinitions,
                conditions: "context".into(),
                reason: "private quote rationale".into(),
                evidence: vec![],
            },
            payload_digest: "digest".into(),
            space: String::new(),
        });
        assert!(!scope_research_payload(
            "research_dispute",
            &mut payload,
            &std::collections::HashMap::new()
        ));
        let registered =
            std::collections::HashMap::from([("immutable-id".into(), "Renamed".into())]);
        assert!(scope_research_payload(
            "research_dispute",
            &mut payload,
            &registered
        ));
        assert!(
            matches!(payload,Some(RefinementPayload::ResearchDispute{space,..}) if space=="Renamed")
        );
        let mut classification = Some(RefinementPayload::SourceClassification {
            run_id: "run".into(),
            space_id: "other-id".into(),
            rules_digest: "digest".into(),
            changes: vec![],
        });
        assert!(!scope_research_payload(
            "source_classification",
            &mut classification,
            &registered
        ));
        assert!(!scope_research_payload(
            "research_dispute",
            &mut None,
            &registered
        ));
    }
    #[tokio::test]
    async fn queue_scope_filters_before_serialization_and_generic_mutations_refuse_disputes() {
        use crate::refinery_routes::{handle_list_refinements, ListRefinementsQuery};
        use std::sync::Arc;
        let temp = tempfile::tempdir().unwrap();
        let db = Arc::new(
            wenlan_core::db::MemoryDB::new(temp.path(), Arc::new(wenlan_core::events::NoopEmitter))
                .await
                .unwrap(),
        );
        let research = db.create_space("Research", None, false).await.unwrap();
        let other = db.create_space("Other", None, false).await.unwrap();
        for (id, space_id) in [
            ("case-a", research.id.as_str()),
            ("case-b", other.id.as_str()),
        ] {
            let payload=serde_json::json!({"action":"research_dispute","dispute":{"space_id":space_id,"kind":"opposing_claims","conditions":"exact conditions","reason":format!("private-{id}"),"evidence":[]},"payload_digest":"digest"}).to_string();
            db.insert_refinement_proposal(id, "research_dispute", &[], Some(&payload), 0.0)
                .await
                .unwrap();
        }
        let classification=serde_json::json!({"action":"source_classification","run_id":"run","space_id":other.id,"rules_digest":"digest","changes":[]}).to_string();
        db.insert_refinement_proposal(
            "classification",
            "source_classification",
            &[],
            Some(&classification),
            0.0,
        )
        .await
        .unwrap();
        let fixture = libsql::Builder::new_local(temp.path().join("origin_memory.db"))
            .build()
            .await
            .unwrap();
        fixture
            .connect()
            .unwrap()
            .execute("UPDATE refinement_queue SET status='awaiting_review'", ())
            .await
            .unwrap();
        let state = Arc::new(tokio::sync::RwLock::new(crate::state::ServerState {
            db: Some(db.clone()),
            ..Default::default()
        }));
        let result = handle_list_refinements(
            State(state.clone()),
            SpaceHeader(Some("Research".into())),
            Query(ListRefinementsQuery {
                space: None,
                action: None,
                limit: None,
            }),
        )
        .await
        .unwrap()
        .0;
        let serialized = serde_json::to_string(&result).unwrap();
        assert!(serialized.contains("private-case-a"));
        assert!(!serialized.contains("private-case-b"));
        assert_eq!(result.proposals.len(), 1);
        assert!(matches!(
            handle_list_refinements(
                State(state.clone()),
                SpaceHeader(Some("Research".into())),
                Query(ListRefinementsQuery {
                    space: Some("Other".into()),
                    action: None,
                    limit: None
                })
            )
            .await,
            Err(ServerError::BadRequest(_))
        ));
        db.update_space("Research", "Renamed", None).await.unwrap();
        let result = handle_list_refinements(
            State(state),
            SpaceHeader(Some("Renamed".into())),
            Query(ListRefinementsQuery {
                space: None,
                action: None,
                limit: None,
            }),
        )
        .await
        .unwrap()
        .0;
        assert!(
            matches!(result.proposals[0].payload.as_ref(),Some(wenlan_types::responses::RefinementPayload::ResearchDispute{space,..}) if space=="Renamed")
        );
        assert!(db
            .resolve_refinement_if_open("case-a", "dismissed")
            .await
            .is_err());
        assert!(
            wenlan_core::synthesis::refinement_queue::apply_refinement_with_decision(
                &db,
                "case-a",
                "user",
                wenlan_core::synthesis::refinement_queue::RefinementDecision::Accept {
                    notes: None
                }
            )
            .await
            .is_err()
        );
        assert_eq!(
            db.get_refinement_proposal("case-a")
                .await
                .unwrap()
                .unwrap()
                .status,
            "awaiting_review"
        );
    }
    #[tokio::test]
    async fn dispute_scope_conflict_fails_before_store_access() {
        let state = std::sync::Arc::new(tokio::sync::RwLock::new(
            crate::state::ServerState::default(),
        ));
        assert!(matches!(
            read(
                State(state),
                SpaceHeader(Some("Other".into())),
                Path("case".into()),
                Query(DisputeQuery {
                    space: "Research".into()
                })
            )
            .await,
            Err(ServerError::BadRequest(_))
        ));
    }
}
