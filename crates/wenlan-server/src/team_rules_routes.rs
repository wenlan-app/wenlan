// SPDX-License-Identifier: Apache-2.0
//! The team host's own review of the Rules pages it keeps: the team-wide
//! page (`everyone`) and each Space's page. These routes answer on the
//! host's loopback daemon only, to the person using it; the team listener
//! never serves them, and no MCP tool calls them.

use std::path::Path as FilePath;

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Json;
use wenlan_core::error::WenlanError;
use wenlan_core::rulesync_team;
use wenlan_core::shared_space;
use wenlan_types::team::{
    TeamRulesDecisionRequest, TeamRulesHostDecision, TeamRulesHostProposal,
    TeamRulesHostProposeRequest,
};

use crate::error::ServerError;
use crate::state::SharedState;

pub(super) const ON_THE_HOST: &str =
    "This team's Rules are kept on the team host. Propose and decide on the team host.";

async fn host_person(headers: &HeaderMap) -> Result<(), ServerError> {
    crate::repo_wiki_routes::require_project_review(headers)?;
    if super::managed_elsewhere().await?.is_some() {
        return Err(ServerError::ValidationError(ON_THE_HOST.into()));
    }
    Ok(())
}

/// POST /api/team/rules/proposals
pub(super) async fn proposals(
    headers: HeaderMap,
) -> Result<Json<Vec<TeamRulesHostProposal>>, ServerError> {
    host_person(&headers).await?;
    super::blocking(|| proposals_at(&shared_space::settings_path()))
        .await
        .map(Json)
}

/// POST /api/team/rules/propose
pub(super) async fn propose(
    headers: HeaderMap,
    Json(request): Json<TeamRulesHostProposeRequest>,
) -> Result<Json<String>, ServerError> {
    host_person(&headers).await?;
    super::blocking(move || propose_at(&shared_space::settings_path(), &request))
        .await
        .map(Json)
}

/// POST /api/team/rules/proposals/{id}/decision
pub(super) async fn decide(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<TeamRulesDecisionRequest>,
) -> Result<Json<TeamRulesHostDecision>, ServerError> {
    host_person(&headers).await?;
    let decision =
        super::blocking(move || decide_at(&shared_space::settings_path(), &id, request.accept))
            .await?;
    if decision.accepted {
        let db = state.read().await.db.clone();
        if let Some(db) = db {
            let root = crate::repo_wiki_routes::page_root_of(&state).await;
            crate::skill_grow::sync_rules_files_quietly(&db, root.as_deref()).await;
        }
    }
    Ok(Json(decision))
}

pub(super) fn proposals_at(settings: &FilePath) -> Result<Vec<TeamRulesHostProposal>, WenlanError> {
    rulesync_team::host_proposals(settings).map_err(crate::repo_wiki_routes::team_rule_error)
}

pub(super) fn propose_at(
    settings: &FilePath,
    request: &TeamRulesHostProposeRequest,
) -> Result<String, WenlanError> {
    rulesync_team::host_propose(settings, &request.space, &request.markdown)
        .map_err(crate::repo_wiki_routes::team_rule_error)
}

pub(super) fn decide_at(
    settings: &FilePath,
    id: &str,
    accept: bool,
) -> Result<TeamRulesHostDecision, WenlanError> {
    rulesync_team::host_decide(settings, id, accept)
        .map_err(crate::repo_wiki_routes::team_rule_error)
}
