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

/// The refusal to an AI tool, or to any caller but the person.
const ONLY_THE_PERSON: &str =
    "only the person using the daemon on this computer can review the team's Rules";

async fn host_person(headers: &HeaderMap) -> Result<(), ServerError> {
    if crate::rulesync_routes::require_local_person(headers).is_err() {
        return Err(ServerError::ValidationError(ONLY_THE_PERSON.into()));
    }
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
    let db = state.read().await.db.clone();
    let root = crate::repo_wiki_routes::page_root_of(&state).await;
    decide_and_deliver(
        shared_space::settings_path(),
        id,
        request.accept,
        db.as_deref(),
        root.as_deref(),
    )
    .await
    .map(Json)
}

/// The start of a decision's note when the accepted text did not reach the
/// repositories' tool files; the reason follows.
const NOT_DELIVERED: &str = "accepted; the repositories' tool files were not updated: ";

/// The reason given when the daemon has no database to read the rules from.
const NO_DATABASE: &str = "Wenlan's database is not open";

/// The note when the tool files were written except some that need the
/// person, wherever they are.
const NEEDS_ATTENTION: &str =
    "accepted; some tool files were not updated and need your attention: see `wenlan rules status`";

/// The note when nothing on this computer receives the accepted text.
const NO_CHECKOUT: &str = "accepted; this computer has no checkout of that repository yet";

/// The reason given when the checkout receipts could not be read.
const RECEIPTS_UNREAD: &str = "the checkout records on this computer could not be read";

/// Decides, then after an accept updates the tool files right away. When
/// they were not updated, or no checkout here receives the text, the reply
/// says so, unless it already says the text reaches no repository.
pub(super) async fn decide_and_deliver(
    settings: std::path::PathBuf,
    id: String,
    accept: bool,
    db: Option<&wenlan_core::db::MemoryDB>,
    page_root: Option<&FilePath>,
) -> Result<TeamRulesHostDecision, ServerError> {
    let mut decision = super::blocking({
        let settings = settings.clone();
        move || decide_at(&settings, &id, accept)
    })
    .await?;
    if decision.accepted {
        let tool_files = match db {
            Some(db) => crate::skill_grow::sync_rules_files_reporting(db, page_root).await,
            None => crate::skill_grow::ToolFiles::NotUpdated(NO_DATABASE.into()),
        };
        if decision.note.is_none() {
            decision.note = delivery_note(tool_files, settings, decision.space.clone()).await;
        }
    }
    Ok(decision)
}

/// What an accept's reply says about the tool files: nothing when they were
/// updated and a checkout on this computer receives the Space's Rules.
pub(super) async fn delivery_note(
    tool_files: crate::skill_grow::ToolFiles,
    settings: std::path::PathBuf,
    space: String,
) -> Option<String> {
    use crate::skill_grow::ToolFiles;
    match tool_files {
        ToolFiles::Updated => reach_note(settings, space).await,
        ToolFiles::NeedsAttention => Some(NEEDS_ATTENTION.into()),
        ToolFiles::NotUpdated(reason) => Some(format!("{NOT_DELIVERED}{reason}")),
    }
}

/// `None` when a checkout on this computer receives the Space's Rules.
async fn reach_note(settings: std::path::PathBuf, space: String) -> Option<String> {
    let reaches = tokio::task::spawn_blocking(move || {
        let setup = crate::rulesync_routes::Setup::system()?;
        crate::rulesync_checkout::space_reaches_a_checkout(&setup, &settings, &space)
    })
    .await;
    match reaches {
        Ok(Ok(true)) => None,
        Ok(Ok(false)) => Some(NO_CHECKOUT.into()),
        Ok(Err(error)) => {
            tracing::warn!(error = %format_args!("{error:#}"), "[team rules] checkout receipts not read");
            Some(format!("{NOT_DELIVERED}{RECEIPTS_UNREAD}"))
        }
        Err(error) => {
            tracing::warn!(%error, "[team rules] checkout receipt check failed");
            Some(format!("{NOT_DELIVERED}{RECEIPTS_UNREAD}"))
        }
    }
}

pub(super) fn proposals_at(settings: &FilePath) -> Result<Vec<TeamRulesHostProposal>, WenlanError> {
    rulesync_team::host_proposals(settings).map_err(crate::repo_wiki_routes::team_rule_error)
}

pub(super) fn propose_at(
    settings: &FilePath,
    request: &TeamRulesHostProposeRequest,
) -> Result<String, WenlanError> {
    rulesync_team::host_propose(settings, &request.space, &request.markdown).map_err(|error| {
        match error {
            rulesync_team::ReviewError::NotConfigured => {
                WenlanError::NotFound(no_rules_for(settings, &request.space))
            }
            error => crate::repo_wiki_routes::team_rule_error(error),
        }
    })
}

/// The answer for a Space name this host keeps no Rules for: a hosted
/// Space waits for a repository, any other name is not a team Space.
fn no_rules_for(settings: &FilePath, space: &str) -> String {
    if rulesync_team::name_looks_secret(space) {
        // A typed key stays out of the reply.
        "That name is not a team Space.".to_string()
    } else if wenlan_core::team::hosted_folder(settings, space).is_ok_and(|folder| folder.is_some())
    {
        format!("\"{space}\" has no Rules page yet: bind a repository to it first.")
    } else {
        format!("\"{space}\" is not a team Space.")
    }
}

pub(super) fn decide_at(
    settings: &FilePath,
    id: &str,
    accept: bool,
) -> Result<TeamRulesHostDecision, WenlanError> {
    rulesync_team::host_decide(settings, id, accept)
        .map_err(crate::repo_wiki_routes::team_rule_error)
}
