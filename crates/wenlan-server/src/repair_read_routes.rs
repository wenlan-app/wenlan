// SPDX-License-Identifier: Apache-2.0
//! Read-only recovery routes for durable repair state.
//!
//! These routes deliberately expose only the exact durable repair target. They
//! never fall back to the ordinary queue or arbitrary memory/page ids: the
//! durable manifest is the authority while the normal router is sealed.

use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::Deserialize;
use std::sync::Arc;
use wenlan_core::{
    db::{MemoryDB, RefinementProposal},
    repair::RepairArtifactStore,
};
use wenlan_types::{
    repair::{RepairManifest, RepairTarget},
    responses::{
        ListRefinementsResponse, ProposalAction, RefinementPayload, RefinementProposalSummary,
    },
    RepairRecovery,
};

use crate::{
    error::ServerError,
    route_registry::{get, TrackedRouter},
    space_header::SpaceHeader,
    state::SharedState,
    truth_guard::TruthView,
};

const REVIEW_CONFLICT: &str = "repair_recovery_review_conflict";
const TARGET_NOT_FOUND: &str = "repair_recovery_target_not_found";

pub(crate) fn register(router: TrackedRouter<SharedState>) -> TrackedRouter<SharedState> {
    router
        .route("/api/refinery/queue", get(handle_list_repair_queue))
        .route(
            "/api/memory/{id}/detail",
            get(handle_get_repair_memory_detail),
        )
        .route("/api/pages/{id}", get(handle_get_repair_page))
}

pub(crate) fn register_recovery(router: TrackedRouter<SharedState>) -> TrackedRouter<SharedState> {
    router.route(
        "/api/repairs/recovery/{review_id}",
        get(handle_get_repair_recovery),
    )
}

#[derive(Debug, Deserialize, Default)]
pub struct RepairQueueQuery {
    /// Kept for wire compatibility with the normal queue reader.
    pub action: Option<String>,
    /// A recovery row is never dropped because a caller supplied `limit=0`.
    pub limit: Option<usize>,
}

struct RepairContext {
    db: Arc<MemoryDB>,
    manifest: RepairManifest,
    store: RepairArtifactStore,
}

async fn repair_context(state: &SharedState) -> Result<Option<RepairContext>, ServerError> {
    let (db, root) = {
        let state = state.read().await;
        (
            state.db.clone().ok_or(ServerError::DbNotInitialized)?,
            state.repair_root.clone().ok_or_else(|| {
                ServerError::Internal("repair artifact root not configured".into())
            })?,
        )
    };
    let store = RepairArtifactStore::new(root);
    let mut pending = store
        .pending_verification_manifest_ids()
        .map_err(ServerError::from)?;
    if pending.is_empty() {
        return Ok(None);
    }
    if pending.len() != 1 {
        return Err(ServerError::Conflict(
            "repair_recovery_multiple_pending_manifests".to_string(),
        ));
    }
    let manifest = store
        .load_manifest(&pending.remove(0))
        .map_err(ServerError::from)?;
    Ok(Some(RepairContext {
        db,
        manifest,
        store,
    }))
}

fn review_conflict() -> ServerError {
    ServerError::Conflict(REVIEW_CONFLICT.to_string())
}

fn validate_bound_review(
    manifest: &RepairManifest,
    proposal: &RefinementProposal,
) -> Result<RefinementProposalSummary, ServerError> {
    validate_bound_review_with_status(manifest, proposal, false)
}

fn validate_bound_review_with_authenticated_receipt(
    manifest: &RepairManifest,
    proposal: &RefinementProposal,
) -> Result<RefinementProposalSummary, ServerError> {
    validate_bound_review_with_status(manifest, proposal, true)
}

fn validate_bound_review_with_status(
    manifest: &RepairManifest,
    proposal: &RefinementProposal,
    allow_resolved: bool,
) -> Result<RefinementProposalSummary, ServerError> {
    let binding = manifest
        .source()
        .review_binding()
        .ok_or_else(review_conflict)?;

    validate_bound_review_fields_with_status(
        binding.review_id(),
        manifest.source().check_id(),
        binding.occurrence_digest(),
        binding.owner_ids(),
        proposal,
        allow_resolved,
    )
}

#[cfg(test)]
fn validate_bound_review_fields(
    review_id: &str,
    check_id: &str,
    occurrence_digest: &wenlan_types::repair::RepairDigest,
    owner_ids: &[String],
    proposal: &RefinementProposal,
) -> Result<RefinementProposalSummary, ServerError> {
    validate_bound_review_fields_with_status(
        review_id,
        check_id,
        occurrence_digest,
        owner_ids,
        proposal,
        false,
    )
}

fn validate_bound_review_fields_with_status(
    review_id: &str,
    check_id: &str,
    occurrence_digest: &wenlan_types::repair::RepairDigest,
    owner_ids: &[String],
    proposal: &RefinementProposal,
    allow_resolved: bool,
) -> Result<RefinementProposalSummary, ServerError> {
    if proposal.id != review_id
        || proposal.action != "lint_repair_review"
        || !(proposal.status == "awaiting_review"
            || (allow_resolved && proposal.status == "resolved"))
        || proposal.source_ids != owner_ids
    {
        return Err(review_conflict());
    }

    let payload = proposal
        .payload
        .as_deref()
        .ok_or_else(review_conflict)
        .and_then(|raw| {
            wenlan_core::db::validate_lint_review_contract(&proposal.id, &proposal.source_ids, raw)
                .map_err(|_| review_conflict())
        })?;

    let RefinementPayload::LintRepairReview {
        check_id: payload_check_id,
        occurrence_digest: payload_occurrence_digest,
        ..
    } = &payload
    else {
        return Err(review_conflict());
    };
    if check_id != payload_check_id.as_str() || occurrence_digest != payload_occurrence_digest {
        return Err(review_conflict());
    }

    Ok(RefinementProposalSummary {
        id: proposal.id.clone(),
        action: ProposalAction::LintRepairReview,
        source_ids: proposal.source_ids.clone(),
        payload: Some(payload),
        confidence: proposal.confidence,
        created_at: proposal.created_at.clone(),
    })
}

pub async fn handle_list_repair_queue(
    State(state): State<SharedState>,
    SpaceHeader(header_space): SpaceHeader,
    Query(query): Query<RepairQueueQuery>,
) -> Result<Json<ListRefinementsResponse>, ServerError> {
    let Some(context) = repair_context(&state).await? else {
        return Ok(Json(ListRefinementsResponse { proposals: vec![] }));
    };
    validate_recovery_scope(
        header_space.as_deref(),
        context.manifest.source().lint_scope(),
        context.manifest.source().report_scope(),
    )?;
    let Some(binding) = context.manifest.source().review_binding() else {
        return Ok(Json(ListRefinementsResponse { proposals: vec![] }));
    };

    let proposal = context
        .db
        .get_refinement_proposal(binding.review_id())
        .await
        .map_err(ServerError::from)?
        .ok_or_else(review_conflict)?;
    let summary = if proposal.status == "resolved"
        && context
            .store
            .has_authenticated_review_verification_receipt(context.manifest.manifest_id())
            .map_err(ServerError::from)?
    {
        validate_bound_review_with_authenticated_receipt(&context.manifest, &proposal)?
    } else {
        validate_bound_review(&context.manifest, &proposal)?
    };

    if query
        .action
        .as_deref()
        .is_some_and(|action| action != "lint_repair_review")
    {
        return Ok(Json(ListRefinementsResponse { proposals: vec![] }));
    }

    // The exact bound row is the recovery authority. Preserve it even when a
    // stale client supplies zero or a smaller limit.
    let _ = query.limit;
    Ok(Json(ListRefinementsResponse {
        proposals: vec![summary],
    }))
}

pub async fn handle_get_repair_recovery(
    State(state): State<SharedState>,
    SpaceHeader(header_space): SpaceHeader,
    Path(review_id): Path<String>,
) -> Result<Json<Option<RepairRecovery>>, ServerError> {
    let Some(context) = repair_context(&state).await? else {
        return Ok(Json(None));
    };
    validate_recovery_scope(
        header_space.as_deref(),
        context.manifest.source().lint_scope(),
        context.manifest.source().report_scope(),
    )?;
    let binding = context
        .manifest
        .source()
        .review_binding()
        .ok_or_else(review_conflict)?;
    if binding.review_id() != review_id {
        return Err(ServerError::NotFound(
            "repair_recovery_review_not_found".to_string(),
        ));
    }

    let recovery = context
        .store
        .load_pending_recovery(context.manifest.manifest_id())
        .map_err(ServerError::from)?
        .ok_or_else(|| ServerError::Conflict("repair_recovery_artifact_missing".to_string()))?;
    if recovery
        .manifest
        .source()
        .review_binding()
        .is_none_or(|recovery_binding| recovery_binding.review_id() != review_id)
    {
        return Err(review_conflict());
    }
    let proposal = context
        .db
        .get_refinement_proposal(review_id.as_str())
        .await
        .map_err(ServerError::from)?
        .ok_or_else(review_conflict)?;
    if proposal.status == "resolved"
        && context
            .store
            .has_authenticated_review_verification_receipt(recovery.manifest.manifest_id())
            .map_err(ServerError::from)?
    {
        validate_bound_review_with_authenticated_receipt(&recovery.manifest, &proposal)?;
    } else {
        validate_bound_review(&recovery.manifest, &proposal)?;
    }
    Ok(Json(Some(recovery)))
}

fn validate_recovery_scope(
    header_space: Option<&str>,
    lint_scope: &wenlan_types::repair::RepairLintScope,
    report_scope: &wenlan_types::lint::LintScope,
) -> Result<(), ServerError> {
    crate::repair_routes::validate_repair_scope_binding(
        header_space,
        lint_scope,
        report_scope,
        None,
    )
}

fn target_allows_memory(manifest: &RepairManifest, id: &str) -> bool {
    target_allows_memory_target(manifest.target(), id)
}

fn target_allows_page(manifest: &RepairManifest, id: &str) -> bool {
    target_allows_page_target(manifest.target(), id)
}

fn target_allows_memory_target(target: &RepairTarget, id: &str) -> bool {
    match target {
        RepairTarget::Memory { source_id, .. } => source_id == id,
        RepairTarget::MemoryEntityExtraction { memory_id, .. } => memory_id == id,
        _ => false,
    }
}

fn target_allows_page_target(target: &RepairTarget, id: &str) -> bool {
    match target {
        RepairTarget::Page { page_id, .. } | RepairTarget::PageProjection { page_id, .. } => {
            page_id == id
        }
        _ => false,
    }
}

async fn require_target(
    state: &SharedState,
    id: &str,
    allowed: fn(&RepairManifest, &str) -> bool,
) -> Result<(), ServerError> {
    let Some(context) = repair_context(state).await? else {
        return Err(ServerError::NotFound(TARGET_NOT_FOUND.to_string()));
    };
    if allowed(&context.manifest, id) {
        Ok(())
    } else {
        Err(ServerError::NotFound(TARGET_NOT_FOUND.to_string()))
    }
}

pub async fn handle_get_repair_memory_detail(
    State(state): State<SharedState>,
    space: SpaceHeader,
    Path(id): Path<String>,
) -> Result<Json<wenlan_types::responses::MemoryDetailResponse>, ServerError> {
    require_target(&state, &id, target_allows_memory).await?;
    crate::memory_detail_routes::handle_get_memory_detail(State(state), space, Path(id)).await
}

pub async fn handle_get_repair_page(
    State(state): State<SharedState>,
    space: SpaceHeader,
    view: TruthView,
    Path(id): Path<String>,
) -> Result<Json<wenlan_types::responses::PageReadResponse>, ServerError> {
    require_target(&state, &id, target_allows_page).await?;
    // A repair read is not an AI session's lookup: no session header travels.
    crate::page_routes::handle_get_page(
        State(state),
        axum::http::HeaderMap::new(),
        space,
        view,
        Path(id),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use wenlan_types::repair::{
        RepairDigest, RepairScope, RepairTarget, REPAIR_CLASSIFICATION_CHECK_ID,
    };

    #[derive(serde::Serialize)]
    struct OwnerBinding<'a> {
        occurrence_digest: &'a RepairDigest,
        source_ids: &'a [String],
    }

    fn bound_review_proposal(
        occurrence_digest: &RepairDigest,
        source_ids: Vec<String>,
    ) -> RefinementProposal {
        let owner_binding_digest = hex::encode(Sha256::digest(
            serde_json::to_vec(&OwnerBinding {
                occurrence_digest,
                source_ids: &source_ids,
            })
            .unwrap(),
        ));
        let id = format!("lint_review_{}", occurrence_digest.as_str());
        let payload = serde_json::json!({
            "action": "lint_repair_review",
            "check_id": REPAIR_CLASSIFICATION_CHECK_ID,
            "occurrence_digest": occurrence_digest,
            "owner_binding_digest": owner_binding_digest,
            "issue": "A classification needs review.",
            "choices": ["reclassify_memory", "keep"],
            "suggested_research_queries": [],
        });
        RefinementProposal {
            id,
            action: "lint_repair_review".to_string(),
            source_ids,
            payload: Some(payload.to_string()),
            confidence: 1.0,
            status: "awaiting_review".to_string(),
            created_at: "2026-09-13 00:00:00".to_string(),
        }
    }

    #[test]
    fn exact_pending_review_row_is_accepted_and_unrelated_row_is_refused() {
        let occurrence = RepairDigest::parse(&"ab".repeat(32)).unwrap();
        let owner_ids = vec!["bound-memory".to_string()];
        let exact = bound_review_proposal(&occurrence, owner_ids.clone());
        let summary = validate_bound_review_fields(
            &exact.id,
            REPAIR_CLASSIFICATION_CHECK_ID,
            &occurrence,
            &owner_ids,
            &exact,
        )
        .unwrap();
        assert_eq!(summary.id, exact.id);
        assert!(matches!(summary.action, ProposalAction::LintRepairReview));

        let mut unrelated = exact.clone();
        unrelated.id = "lint_review_unrelated".to_string();
        assert!(matches!(
            validate_bound_review_fields(
                &exact.id,
                REPAIR_CLASSIFICATION_CHECK_ID,
                &occurrence,
                &owner_ids,
                &unrelated,
            ),
            Err(ServerError::Conflict(message)) if message == REVIEW_CONFLICT
        ));
    }

    #[test]
    fn altered_dismissed_or_rebound_review_rows_are_refused() {
        let occurrence = RepairDigest::parse(&"cd".repeat(32)).unwrap();
        let owner_ids = vec!["bound-memory".to_string()];
        let exact = bound_review_proposal(&occurrence, owner_ids.clone());

        let mut altered = exact.clone();
        altered.source_ids = vec!["other-memory".to_string()];
        assert!(validate_bound_review_fields(
            &exact.id,
            REPAIR_CLASSIFICATION_CHECK_ID,
            &occurrence,
            &owner_ids,
            &altered,
        )
        .is_err());

        let mut dismissed = exact.clone();
        dismissed.status = "dismissed".to_string();
        assert!(validate_bound_review_fields(
            &exact.id,
            REPAIR_CLASSIFICATION_CHECK_ID,
            &occurrence,
            &owner_ids,
            &dismissed,
        )
        .is_err());

        let rebound_occurrence = RepairDigest::parse(&"ef".repeat(32)).unwrap();
        let rebound = bound_review_proposal(&rebound_occurrence, owner_ids.clone());
        assert!(validate_bound_review_fields(
            &exact.id,
            REPAIR_CLASSIFICATION_CHECK_ID,
            &occurrence,
            &owner_ids,
            &rebound,
        )
        .is_err());
    }

    #[test]
    fn memory_recovery_gate_rejects_unrelated_and_non_memory_targets() {
        let target = RepairTarget::memory("bound-memory".into(), RepairScope::global()).unwrap();
        assert!(matches!(target, RepairTarget::Memory { .. }));
        assert!(target_allows_memory_target(&target, "bound-memory"));
        assert!(!target_allows_memory_target(&target, "other-memory"));
    }

    #[test]
    fn page_recovery_gate_rejects_unrelated_and_non_page_targets() {
        let target =
            RepairTarget::page_projection("bound-page".into(), RepairScope::global()).unwrap();
        assert!(matches!(target, RepairTarget::PageProjection { .. }));
        assert!(target_allows_page_target(&target, "bound-page"));
        assert!(!target_allows_page_target(&target, "other-page"));
    }

    #[test]
    fn recovery_scope_header_must_match_bound_registered_scope() {
        let report_scope = wenlan_types::lint::LintScope::registered(
            wenlan_types::lint::LintOpaqueId::from_sorted_position(0).unwrap(),
        );
        let lint_scope =
            wenlan_types::repair::RepairLintScope::registered("career".into()).unwrap();
        assert!(validate_recovery_scope(Some("career"), &lint_scope, &report_scope).is_ok());
        assert!(validate_recovery_scope(Some("health"), &lint_scope, &report_scope).is_err());
        assert!(validate_recovery_scope(None, &lint_scope, &report_scope).is_ok());
    }
}
