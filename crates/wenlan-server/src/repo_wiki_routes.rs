// SPDX-License-Identifier: Apache-2.0
//! A Space bound to a git repository, and putting its code pages into that
//! repository as a pull request.
//!
//! - `GET /api/spaces/{name}/wiki/repo`: the binding, or `null`.
//! - `PUT /api/spaces/{name}/wiki/repo`: bind, or change folder and daily run.
//! - `DELETE /api/spaces/{name}/wiki/repo`: unbind.
//! - `GET /api/spaces/{name}/wiki/suggest`: repositories the Space's code
//!   pages could go into, for the app to offer binding.
//! - `POST /api/spaces/{name}/wiki/publish`: write the pages and open or
//!   update the pull request.
//! - `POST /api/spaces/{name}/wiki/run`: start the daily run now, in the
//!   background (the user's AI tool, then a publish).
//! - `POST /api/spaces/{name}/wiki/docs`: an AI tool suggests a fix to one of
//!   the repository's own docs, or reports how many it read; the user
//!   declines a fix.
//! - `GET /api/spaces/{name}/wiki/github`: what a daily update on GitHub
//!   Actions still needs in the repository, and its last run.
//! - `POST /api/spaces/{name}/wiki/github`: one of those settings: store the
//!   AI tool's secret (signing in to Claude in the browser, or a pasted
//!   OpenAI key), or let Actions open pull requests.
//!
//! The work lives in `wenlan_core::repo_wiki`; git and gh run on a blocking
//! thread.

use crate::error::ServerError;
use crate::route_registry::{get, post, TrackedRouter};
use crate::state::{ServerState, SharedState};
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::Json,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use tokio::sync::RwLock;
use wenlan_core::db::MemoryDB;
use wenlan_core::repo_wiki;
use wenlan_core::{rulesync_team, shared_space};
use wenlan_types::repo_wiki::{
    BindRepoWikiRequest, ProjectRuleProposal, RepoDocFixState, RepoDocsReply, RepoDocsRequest,
    RepoWikiAgent, RepoWikiBinding, RepoWikiGithubRequest, RepoWikiGithubStatus,
    RepoWikiPublishResult, RepoWikiSuggestion,
};
use wenlan_types::rulesync::{ProjectTeamRulesRequest, SharedRulesProposalRequest};
use wenlan_types::team::{TeamRulesDecisionRequest, TeamRulesProposal};

static PROJECT_RULES_TRANSACTION: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub(crate) fn register(router: TrackedRouter<SharedState>) -> TrackedRouter<SharedState> {
    router
        .route(
            "/api/spaces/{name}/wiki/repo",
            get(handle_get_binding)
                .put(handle_bind)
                .delete(handle_unbind),
        )
        .route("/api/spaces/{name}/wiki/suggest", get(handle_suggest))
        .route("/api/spaces/{name}/wiki/publish", post(handle_publish))
        .route("/api/spaces/{name}/wiki/run", post(handle_run_now))
        .route("/api/spaces/{name}/wiki/docs", post(handle_docs))
        .route(
            "/api/spaces/{name}/wiki/rules/proposals",
            post(handle_rule_proposals),
        )
        .route(
            "/api/spaces/{name}/wiki/rules/proposals/{id}/accept",
            post(handle_accept_rule_proposal),
        )
        .route(
            "/api/spaces/{name}/wiki/rules/proposals/{id}/dismiss",
            post(handle_dismiss_rule_proposal),
        )
        .route(
            "/api/spaces/{name}/wiki/rules/team",
            post(handle_project_team_rules),
        )
        .route(
            "/api/spaces/{name}/wiki/shared-rules/proposals",
            post(handle_shared_rule_proposals),
        )
        .route(
            "/api/spaces/{name}/wiki/shared-rules/propose",
            post(handle_propose_shared_rules),
        )
        .route(
            "/api/spaces/{name}/wiki/shared-rules/proposals/{id}/decision",
            post(handle_shared_rule_decision),
        )
        .route(
            "/api/spaces/{name}/wiki/github",
            get(handle_github_status).post(handle_github_setup),
        )
}

async fn project_rule_space(
    db: &MemoryDB,
    space: &str,
) -> Result<wenlan_types::Space, ServerError> {
    db.get_space(space)
        .await?
        .ok_or_else(|| ServerError::NotFound("Space not found".into()))
}

pub(crate) fn require_project_review(headers: &HeaderMap) -> Result<(), ServerError> {
    crate::rulesync_routes::require_local_person(headers)?;
    if !wenlan_core::rulesync_personal::enabled() {
        return Err(ServerError::Conflict("WENLAN_RULESYNC is off".into()));
    }
    Ok(())
}

fn team_rules_key(space: &wenlan_types::Space) -> String {
    format!("rulesync.project.team:{}", space.id)
}

async fn handle_project_team_rules(
    State(state): State<SharedState>,
    Path(name): Path<String>,
    headers: HeaderMap,
    Json(request): Json<ProjectTeamRulesRequest>,
) -> Result<Json<ProjectTeamRulesRequest>, ServerError> {
    require_project_review(&headers)?;
    let _serial = PROJECT_RULES_TRANSACTION.lock().await;
    let db = db_of(&state).await?;
    let space = project_rule_space(&db, &registered_space(&db, &name).await?).await?;
    let name = space.name.clone();
    let team = blocking(move || {
        if repo_wiki::binding_at(&repo_wiki::store_path(), &name)?.is_none() {
            return Err(wenlan_core::WenlanError::NotFound(
                "bind this Space to a repository first".into(),
            ));
        }
        let settings = shared_space::settings_path();
        if shared_space::share_settings_at(&settings, &shared_space::Scope::Space(name))?
            .is_some_and(|share| share.host.is_some())
        {
            return Err(wenlan_core::WenlanError::Validation(
                "change the team rules opt-in on the project host".into(),
            ));
        }
        let Some(team) = request.team else {
            return Ok(None);
        };
        let layers = rulesync_team::load_layers(&settings)?;
        if layers.iter().any(|layer| layer.id == team) {
            return Ok(Some(team));
        }
        let mut matches = layers.into_iter().filter(|layer| layer.name == team);
        let found = matches.next().ok_or_else(|| {
            wenlan_core::WenlanError::NotFound("no readable Rules layer for that team".into())
        })?;
        if matches.next().is_some() {
            return Err(wenlan_core::WenlanError::Conflict(
                "several teams use that name; give the team ID".into(),
            ));
        }
        Ok(Some(found.id))
    })
    .await?;
    db.set_app_metadata(&team_rules_key(&space), team.as_deref().unwrap_or(""))
        .await?;
    Ok(Json(ProjectTeamRulesRequest { team }))
}

async fn handle_shared_rule_proposals(
    State(state): State<SharedState>,
    Path(space): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Vec<TeamRulesProposal>>, ServerError> {
    require_project_review(&headers)?;
    let db = db_of(&state).await?;
    let space = registered_space(&db, &space).await?;
    blocking(move || {
        rulesync_team::local_space_proposals(&shared_space::settings_path(), &space)
            .map_err(team_rule_error)
    })
    .await
    .map(Json)
}

async fn handle_propose_shared_rules(
    State(state): State<SharedState>,
    Path(space): Path<String>,
    headers: HeaderMap,
    Json(request): Json<SharedRulesProposalRequest>,
) -> Result<Json<String>, ServerError> {
    require_project_review(&headers)?;
    let db = db_of(&state).await?;
    let space = registered_space(&db, &space).await?;
    blocking(move || {
        rulesync_team::update_hosted_rules(
            &shared_space::settings_path(),
            &space,
            &request.markdown,
        )
        .map_err(team_rule_error)
    })
    .await
    .map(Json)
}

async fn handle_shared_rule_decision(
    State(state): State<SharedState>,
    Path((space, id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(request): Json<TeamRulesDecisionRequest>,
) -> Result<Json<()>, ServerError> {
    require_project_review(&headers)?;
    let db = db_of(&state).await?;
    let space = registered_space(&db, &space).await?;
    blocking(move || {
        rulesync_team::decide_local_space_proposal(
            &shared_space::settings_path(),
            &space,
            &id,
            request.accept,
        )
        .map_err(team_rule_error)
    })
    .await?;
    crate::skill_grow::sync_rules_files_quietly(&db, page_root_of(&state).await.as_deref()).await;
    Ok(Json(()))
}

pub(crate) fn team_rule_error(error: rulesync_team::ReviewError) -> wenlan_core::WenlanError {
    match error {
        rulesync_team::ReviewError::Forbidden => {
            wenlan_core::WenlanError::Validation(error.to_string())
        }
        rulesync_team::ReviewError::NotConfigured => {
            wenlan_core::WenlanError::NotFound(error.to_string())
        }
        rulesync_team::ReviewError::Failed(error) => error,
    }
}

pub(crate) fn markdown_rules(markdown: &str) -> Vec<String> {
    markdown
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| line.strip_prefix("- ").unwrap_or(line).to_owned())
        .collect()
}

async fn handle_rule_proposals(
    State(state): State<SharedState>,
    Path(space): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Vec<ProjectRuleProposal>>, ServerError> {
    require_project_review(&headers)?;
    let db = db_of(&state).await?;
    let space = registered_space(&db, &space).await?;
    let proposals =
        blocking(move || repo_wiki::project_rule_proposals_at(&repo_wiki::store_path(), &space))
            .await?;
    Ok(Json(proposals))
}

async fn handle_accept_rule_proposal(
    State(state): State<SharedState>,
    Path((space, id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<ProjectRuleProposal>, ServerError> {
    review_project_rule(state, space, id, headers, true).await
}

async fn handle_dismiss_rule_proposal(
    State(state): State<SharedState>,
    Path((space, id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<ProjectRuleProposal>, ServerError> {
    review_project_rule(state, space, id, headers, false).await
}

#[derive(Default, Serialize, Deserialize)]
struct ProjectRuleEdits {
    added: BTreeMap<String, String>,
    removed: BTreeSet<String>,
}

impl ProjectRuleEdits {
    fn edit(&mut self, proposal: &ProjectRuleProposal) {
        for line in &proposal.removed_rules {
            self.added.remove(line);
            self.removed.insert(line.clone());
        }
        for line in &proposal.added_rules {
            self.removed.remove(line);
            self.added.insert(
                line.clone(),
                format!("{} @ {}", proposal.path, proposal.revision),
            );
        }
    }

    fn project(&self, rules: &mut Vec<String>) {
        rules.retain(|line| {
            !self
                .removed
                .contains(&wenlan_core::skill_grow::tool_file_text(line))
        });
        let base: BTreeSet<_> = rules
            .iter()
            .map(|line| wenlan_core::skill_grow::tool_file_text(line))
            .collect();
        rules.extend(
            self.added
                .keys()
                .filter(|line| !base.contains(*line))
                .cloned(),
        );
    }
}

fn edits_key(space: &wenlan_types::Space) -> String {
    format!("rulesync.project.edits:{}", space.id)
}

async fn project_edits(db: &MemoryDB, key: &str) -> Result<ProjectRuleEdits, ServerError> {
    match db.get_app_metadata(key).await? {
        Some(value) => {
            serde_json::from_str(&value).map_err(|error| ServerError::Internal(error.to_string()))
        }
        None => Ok(ProjectRuleEdits::default()),
    }
}

pub(crate) async fn exclude_reviewed_removals(
    db: &MemoryDB,
    space: &wenlan_types::Space,
    rules: &mut Vec<String>,
) -> Result<(), ServerError> {
    let edits = project_edits(db, &edits_key(space)).await?;
    rules.retain(|line| {
        !edits
            .removed
            .contains(&wenlan_core::skill_grow::tool_file_text(line))
    });
    Ok(())
}

pub(crate) async fn include_reviewed_project_rules(
    db: &MemoryDB,
    space: &wenlan_types::Space,
    lines: &mut Vec<wenlan_types::SkillLine>,
) -> Result<(), ServerError> {
    let edits = project_edits(db, &edits_key(space)).await?;
    lines.retain(|line| {
        line.general
            || line.kind != wenlan_types::SkillLineKind::Rule
            || !edits
                .removed
                .contains(&wenlan_core::skill_grow::tool_file_text(&line.text))
    });
    let existing: BTreeSet<_> = lines
        .iter()
        .map(|line| wenlan_core::skill_grow::tool_file_text(&line.text))
        .collect();
    for (text, origin) in edits.added {
        if existing.contains(&text) {
            continue;
        }
        let id = wenlan_core::rulesync_personal::fingerprint(&(&space.id, &origin, &text))
            .map_err(|error| ServerError::Internal(error.to_string()))?;
        lines.push(wenlan_types::SkillLine {
            id,
            kind: wenlan_types::SkillLineKind::Rule,
            text,
            source_page_id: String::new(),
            source_page_title: origin,
            source_revision: 0,
            source_revision_at: 0,
            source_quote: String::new(),
            person_edited: false,
            general: false,
            hand_written: false,
            saved_from: None,
            saved_by: None,
        });
    }
    Ok(())
}

async fn review_project_rule(
    state: SharedState,
    space: String,
    id: String,
    headers: HeaderMap,
    accept: bool,
) -> Result<Json<ProjectRuleProposal>, ServerError> {
    require_project_review(&headers)?;
    let _serial = PROJECT_RULES_TRANSACTION.lock().await;
    let (db, page_root) = {
        let state = state.read().await;
        (
            state.db.clone().ok_or(ServerError::DbNotInitialized)?,
            state
                .lint_config
                .page_root()
                .map(std::path::Path::to_path_buf),
        )
    };
    let space = project_rule_space(&db, &registered_space(&db, &space).await?).await?;
    if shared_space::share_settings_at(
        &shared_space::settings_path(),
        &shared_space::Scope::Space(space.name.clone()),
    )?
    .is_some_and(|share| share.host.is_some())
    {
        return Err(ServerError::ValidationError(
            "project rule review needs write access to this Space".into(),
        ));
    }
    let name = space.name.clone();
    let requested = id.clone();
    let proposal = blocking(move || {
        repo_wiki::project_rule_proposals_at(&repo_wiki::store_path(), &name)?
            .into_iter()
            .find(|proposal| proposal.id == requested)
            .ok_or_else(|| {
                wenlan_core::WenlanError::NotFound("no pending project rule proposal".into())
            })
    })
    .await?;
    if accept {
        if let Some(reason) = &proposal.blocked {
            return Err(ServerError::Conflict(reason.clone()));
        }
        let settings = shared_space::settings_path();
        let hosted = rulesync_team::hosted_project_layer(&settings, &space.name)?;
        if let Some(layer) = hosted {
            let mut rules = markdown_rules(&layer.markdown);
            let mut edits = ProjectRuleEdits::default();
            edits.edit(&proposal);
            edits.project(&mut rules);
            let markdown = rules
                .iter()
                .map(|line| format!("- {line}\n"))
                .collect::<String>();
            let name = space.name.clone();
            blocking(move || {
                let queued = rulesync_team::queue_proposal(
                    &settings,
                    &layer.id,
                    &layer.revision,
                    &markdown,
                    wenlan_core::shared_space::version::AuthorKind::Unknown,
                )?;
                rulesync_team::decide_local_space_proposal(&settings, &name, &queued, true)
                    .map_err(team_rule_error)?;
                repo_wiki::approve_project_rule_proposal_at(
                    &repo_wiki::store_path(),
                    &name,
                    &id,
                    &rules,
                )
            })
            .await?;
            return Ok(Json(proposal));
        }
        let mut rules = crate::skill_grow::project_rules(&db, page_root.as_deref(), &space.name)
            .await
            .ok_or_else(|| ServerError::Conflict("the Space rules could not be read".into()))?;
        let key = edits_key(&space);
        let mut edits = project_edits(&db, &key).await?;
        edits.edit(&proposal);
        edits.project(&mut rules);
        wenlan_core::rulesync_personal::reject_secret_lines(&rules.join("\n"))
            .map_err(|error| ServerError::ValidationError(error.to_string()))?;
        db.set_app_metadata(
            &key,
            &serde_json::to_string(&edits)
                .map_err(|error| ServerError::Internal(error.to_string()))?,
        )
        .await?;
        blocking(move || {
            repo_wiki::approve_project_rule_proposal_at(
                &repo_wiki::store_path(),
                &space.name,
                &id,
                &rules,
            )
        })
        .await?;
    } else {
        blocking(move || {
            repo_wiki::dismiss_project_rule_proposal_at(&repo_wiki::store_path(), &space.name, &id)
        })
        .await?;
    }
    Ok(Json(proposal))
}

async fn db_of(state: &Arc<RwLock<ServerState>>) -> Result<Arc<MemoryDB>, ServerError> {
    let s = state.read().await;
    s.db.clone().ok_or(ServerError::DbNotInitialized)
}

/// The registered Space named in the path.
async fn registered_space(db: &MemoryDB, space: &str) -> Result<String, ServerError> {
    db.registered_space_or_none(Some(space))
        .await
        .map_err(|e| ServerError::Internal(e.to_string()))?
        .ok_or_else(|| ServerError::NotFound(format!("no Space named {space:?}")))
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, wenlan_core::WenlanError> + Send + 'static,
) -> Result<T, ServerError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| ServerError::Internal(format!("repo wiki task failed: {e}")))?
        .map_err(ServerError::from)
}

/// GET /api/spaces/{name}/wiki/repo
async fn handle_get_binding(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(space): Path<String>,
) -> Result<Json<Option<RepoWikiBinding>>, ServerError> {
    let db = db_of(&state).await?;
    let space = registered_space(&db, &space).await?;
    let binding = blocking(move || repo_wiki::binding_at(&repo_wiki::store_path(), &space)).await?;
    Ok(Json(binding))
}

/// PUT /api/spaces/{name}/wiki/repo
async fn handle_bind(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(space): Path<String>,
    Json(req): Json<BindRepoWikiRequest>,
) -> Result<Json<RepoWikiBinding>, ServerError> {
    let db = db_of(&state).await?;
    let space = registered_space(&db, &space).await?;
    let repo_path = std::path::PathBuf::from(req.repo_root.trim());
    if !repo_path.is_absolute() {
        return Err(ServerError::ValidationError(format!(
            "repo_root must be an absolute path (got {:?})",
            req.repo_root
        )));
    }
    let binding = blocking(move || {
        let store = repo_wiki::store_path();
        // A repository has one Wenlan keeping its pages and rules. Tying a
        // second one is refused here, where it reads from the files on disk,
        // rather than at the first publish.
        if !req.take_over && repo_wiki::binding_at(&store, &space)?.is_none() {
            let top = repo_wiki::repository_top_level(&repo_path)?;
            let ours = repo_wiki::publisher_id_at(&store)?;
            if repo_wiki::working_copy_kept_by_another(&top, &ours) {
                let folder = req
                    .folder
                    .clone()
                    .unwrap_or_else(|| repo_wiki::default_folder(&top).to_owned());
                return Err(repo_wiki::kept_by_another_error(&folder));
            }
        }
        let binding =
            repo_wiki::bind_at(&store, &space, &repo_path, req.folder.as_deref(), req.daily)?;
        if req.take_over {
            if let Some(binding) = repo_wiki::set_take_over_at(&store, &space, true)? {
                return Ok(binding);
            }
        }
        Ok(binding)
    })
    .await?;
    Ok(Json(binding))
}

/// DELETE /api/spaces/{name}/wiki/repo
async fn handle_unbind(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(space): Path<String>,
) -> Result<Json<serde_json::Value>, ServerError> {
    let db = db_of(&state).await?;
    let space = registered_space(&db, &space).await?;
    let removed = blocking(move || {
        let store = repo_wiki::store_path();
        let binding = repo_wiki::binding_at(&store, &space)?;
        let removed = repo_wiki::unbind_at(&store, &space)?;
        if let Some(binding) = binding {
            repo_wiki::remove_worktree(&binding.repo_root, &repo_wiki::worktree_for(&space));
        }
        Ok(removed)
    })
    .await?;
    Ok(Json(serde_json::json!({ "removed": removed })))
}

/// GET /api/spaces/{name}/wiki/suggest
async fn handle_suggest(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(space): Path<String>,
) -> Result<Json<RepoWikiSuggestion>, ServerError> {
    let db = db_of(&state).await?;
    let space = registered_space(&db, &space).await?;
    let pages = exportable_pages(&db, &space).await?;
    let code_bases = db
        .code_basis_locators_by_page()
        .await
        .map_err(|e| ServerError::Internal(e.to_string()))?;
    let candidates = blocking(move || Ok(repo_wiki::candidates(&pages, &code_bases))).await?;
    Ok(Json(RepoWikiSuggestion { candidates }))
}

/// POST /api/spaces/{name}/wiki/publish
async fn handle_publish(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(space): Path<String>,
) -> Result<Json<RepoWikiPublishResult>, ServerError> {
    let db = db_of(&state).await?;
    let space = registered_space(&db, &space).await?;
    let page_root = page_root_of(&state).await;
    let result = publish_space(&db, &space, page_root.as_deref()).await?;
    Ok(Json(result))
}

/// The pages folder of this daemon, for re-projecting a page taken back from
/// the repository.
pub(crate) async fn page_root_of(state: &Arc<RwLock<ServerState>>) -> Option<std::path::PathBuf> {
    let s = state.read().await;
    s.lint_config.page_root().map(std::path::Path::to_path_buf)
}

/// POST /api/spaces/{name}/wiki/docs
async fn handle_docs(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(space): Path<String>,
    Json(req): Json<RepoDocsRequest>,
) -> Result<Json<RepoDocsReply>, ServerError> {
    let db = db_of(&state).await?;
    let space = registered_space(&db, &space).await?;
    let now = chrono::Utc::now().timestamp();
    // Refusals carry a code the MCP tool passes on to the AI tool, so it can
    // correct the fix: the words name only the file, counts and the rule.
    let refused = |error: wenlan_core::WenlanError| {
        use wenlan_core::WenlanError as E;
        match error {
            E::Validation(message) => E::Validation(format!("doc_fix_refused: {message}")),
            E::Conflict(message) => E::Conflict(format!("doc_fix_refused: {message}")),
            E::NotFound(message) => E::NotFound(format!("doc_fix_refused: {message}")),
            other => other,
        }
    };
    if let Err(error) = check_followed_page(&db, &space, &req).await? {
        return Err(refused(error).into());
    }
    let reply =
        blocking(move || docs_action(&repo_wiki::store_path(), &space, req, now).map_err(refused))
            .await?;
    Ok(Json(reply))
}

/// For a request about a followed page: the page is in `space` and active,
/// a fix's quote is in the version it names, and that version is the
/// page's current one (a version an AI tool says it followed may be older).
async fn check_followed_page(
    db: &MemoryDB,
    space: &str,
    req: &RepoDocsRequest,
) -> Result<Result<(), wenlan_core::WenlanError>, ServerError> {
    use wenlan_core::WenlanError as E;
    let (page_id, version) = match req {
        RepoDocsRequest::Suggest {
            page: Some(line), ..
        } => (line.page_id.as_str(), Some(line.version)),
        RepoDocsRequest::Follow { page_id, .. } => (page_id.as_str(), None),
        RepoDocsRequest::Followed { page_id, version } => (page_id.as_str(), Some(*version)),
        _ => return Ok(Ok(())),
    };
    let scope = crate::read_scope::effective_read_scope(db, Some(space), None).await?;
    let Some(page) = db
        .get_page_scoped(page_id, &scope)
        .await
        .map_err(|e| ServerError::Internal(e.to_string()))?
        .filter(|page| page.status == "active")
    else {
        return Ok(Err(E::NotFound(format!(
            "page {page_id} is not an active page of {space:?}"
        ))));
    };
    match req {
        RepoDocsRequest::Suggest {
            page: Some(line), ..
        } => {
            if line.version != page.version {
                return Ok(Err(E::Conflict(format!(
                    "page {page_id} is at version {}, not {}; read it again with read_page",
                    page.version, line.version
                ))));
            }
            if !repo_wiki::page_has_quote(&page.content, &line.quote) {
                return Ok(Err(E::Validation(format!(
                    "page.quote is not on page {page_id}; copy the line from the page exactly"
                ))));
            }
        }
        RepoDocsRequest::Followed { .. }
            if version.is_some_and(|version| version < 1 || version > page.version) =>
        {
            return Ok(Err(E::Validation(format!(
                "page {page_id} is at version {}; name the version you read",
                page.version
            ))));
        }
        _ => {}
    }
    Ok(Ok(()))
}

/// The binding of the registered Space `space`, or 404.
async fn bound(db: &MemoryDB, space: &str) -> Result<RepoWikiBinding, ServerError> {
    let space = registered_space(db, space).await?;
    let lookup = space.clone();
    blocking(move || repo_wiki::binding_at(&repo_wiki::store_path(), &lookup))
        .await?
        .ok_or_else(|| {
            ServerError::NotFound(format!(
                "Space {space:?} is not bound to a repository; bind it first"
            ))
        })
}

/// GET /api/spaces/{name}/wiki/github
async fn handle_github_status(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(space): Path<String>,
) -> Result<Json<RepoWikiGithubStatus>, ServerError> {
    let db = db_of(&state).await?;
    let binding = bound(&db, &space).await?;
    let status = blocking(move || {
        Ok(wenlan_core::repo_wiki_github::status(
            &binding,
            repo_wiki::find_gh().as_deref(),
        ))
    })
    .await?;
    Ok(Json(status))
}

/// POST /api/spaces/{name}/wiki/github
async fn handle_github_setup(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(space): Path<String>,
    Json(req): Json<RepoWikiGithubRequest>,
) -> Result<Json<RepoWikiGithubStatus>, ServerError> {
    use wenlan_core::repo_wiki_github as github;
    let db = db_of(&state).await?;
    let binding = bound(&db, &space).await?;
    let status = blocking(move || {
        let gh = repo_wiki::find_gh().ok_or_else(|| {
            wenlan_core::WenlanError::Validation(
                "install the GitHub CLI (gh) and sign in with `gh auth login`".to_owned(),
            )
        })?;
        match req {
            RepoWikiGithubRequest::ClaudeToken => {
                let claude = repo_wiki::find_agent(RepoWikiAgent::Claude).ok_or_else(|| {
                    wenlan_core::WenlanError::Validation(
                        "Claude Code is not installed on this computer".to_owned(),
                    )
                })?;
                github::store_claude_token(&binding, &gh, &claude)?;
            }
            RepoWikiGithubRequest::ApiKey { key } => {
                github::store_api_key(&binding, &gh, &key)?;
            }
            RepoWikiGithubRequest::AllowPullRequests => {
                github::allow_pull_requests(&binding, &gh)?;
            }
        }
        Ok(github::status(&binding, Some(&gh)))
    })
    .await?;
    Ok(Json(status))
}

/// POST /api/spaces/{name}/wiki/run
async fn handle_run_now(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(space): Path<String>,
) -> Result<Json<serde_json::Value>, ServerError> {
    let db = db_of(&state).await?;
    let space = registered_space(&db, &space).await?;
    let lookup = space.clone();
    let binding = blocking(move || repo_wiki::binding_at(&repo_wiki::store_path(), &lookup))
        .await?
        .ok_or_else(|| {
            ServerError::NotFound(format!(
                "Space {space:?} is not bound to a repository; bind it first"
            ))
        })?;
    if crate::repo_wiki_daily::is_running(&binding.space) {
        return Ok(Json(
            serde_json::json!({ "started": false, "reason": "already running" }),
        ));
    }
    let shared = state.clone();
    tokio::spawn(async move {
        match crate::repo_wiki_daily::run_once(&shared, &binding).await {
            Ok(result) => tracing::info!(
                "[repo_wiki] run for {} finished: {:?}",
                binding.space,
                result.status
            ),
            Err(error) => tracing::warn!("[repo_wiki] run for {} failed: {error}", binding.space),
        }
    });
    Ok(Json(serde_json::json!({ "started": true })))
}

/// One `POST .../wiki/docs` request against the store at `store`.
fn docs_action(
    store: &std::path::Path,
    space: &str,
    req: RepoDocsRequest,
    now: i64,
) -> Result<RepoDocsReply, wenlan_core::WenlanError> {
    Ok(match req {
        RepoDocsRequest::Suggest {
            path,
            old_text,
            new_text,
            reason,
            code,
            page,
        } => {
            let (fix, open) = repo_wiki::suggest_doc_fix_at(
                store,
                space,
                repo_wiki::DocFixRequest {
                    path,
                    old_text,
                    new_text,
                    reason,
                    code,
                    page,
                },
                now,
            )?;
            RepoDocsReply {
                fix: Some(fix),
                open,
                follow: None,
            }
        }
        RepoDocsRequest::Dismiss { id } => {
            let (fix, open) = repo_wiki::dismiss_doc_fix_at(store, space, &id)?;
            RepoDocsReply {
                fix: Some(fix),
                open,
                follow: None,
            }
        }
        RepoDocsRequest::Checked { docs } => RepoDocsReply {
            fix: None,
            open: repo_wiki::record_doc_check_at(store, space, docs, now)?,
            follow: None,
        },
        RepoDocsRequest::Follow { page_id, files } => {
            let follow = repo_wiki::follow_page_at(store, space, &page_id, files, now)?;
            RepoDocsReply {
                fix: None,
                open: open_fix_count(store, space)?,
                follow: Some(follow),
            }
        }
        RepoDocsRequest::Unfollow { page_id } => {
            if !repo_wiki::unfollow_page_at(store, space, &page_id)? {
                return Err(wenlan_core::WenlanError::NotFound(format!(
                    "no docs follow page {page_id} in {space:?}"
                )));
            }
            RepoDocsReply {
                fix: None,
                open: open_fix_count(store, space)?,
                follow: None,
            }
        }
        RepoDocsRequest::Followed { page_id, version } => {
            let follow = repo_wiki::mark_followed_at(store, space, &page_id, version)?;
            RepoDocsReply {
                fix: None,
                open: open_fix_count(store, space)?,
                follow: Some(follow),
            }
        }
    })
}

/// Fixes of `space` waiting for or in the pull request.
fn open_fix_count(store: &std::path::Path, space: &str) -> Result<usize, wenlan_core::WenlanError> {
    Ok(repo_wiki::binding_at(store, space)?
        .map(|binding| {
            binding
                .doc_fixes
                .iter()
                .filter(|fix| fix.state != RepoDocFixState::Dismissed)
                .count()
        })
        .unwrap_or(0))
}

/// What a publish of `space` writes from: its pages that may go into the
/// repository, with their code bases and anchors.
struct Gathered {
    pages: Vec<wenlan_core::pages::Page>,
    left_out: usize,
    code_bases: std::collections::HashMap<String, Vec<wenlan_types::code_basis::CodeBasisLocator>>,
    concept_paths: wenlan_core::export::okf::ConceptPathMap,
    anchors: repo_wiki::CodeAnchors,
}

async fn gather(
    db: &MemoryDB,
    space: &str,
    binding: &RepoWikiBinding,
) -> Result<Gathered, ServerError> {
    let mut pages = exportable_pages(db, space).await?;
    pages.retain(|page| !rulesync_team::is_master_page(&page.id));
    let code_bases = db
        .code_basis_locators_by_page()
        .await
        .map_err(|e| ServerError::Internal(e.to_string()))?;
    let concept_paths = wenlan_core::export::okf::concept_path_map(
        &db.okf_concept_paths()
            .await
            .map_err(|e| ServerError::Internal(e.to_string()))?,
    );
    let anchors: repo_wiki::CodeAnchors = db
        .list_code_basis_rows(Some(space))
        .await
        .map_err(|e| ServerError::Internal(e.to_string()))?
        .into_iter()
        .map(|row| {
            let locator = wenlan_types::code_basis::CodeBasisLocator::from(&row.reference);
            (repo_wiki::anchor_key(&row.page_id, &locator), row.anchor)
        })
        .collect();
    let (pages, left_out) = repo_wiki::select_pages(pages, &code_bases, &binding.repo_root);
    Ok(Gathered {
        pages,
        left_out,
        code_bases,
        concept_paths,
        anchors,
    })
}

/// The name a change from the repository carries on the page's history and
/// on a revision card.
const FROM_REPOSITORY: &str = "GitHub";

/// What [`take_back_edits`] did.
#[derive(Default)]
struct TakenBack {
    /// Paths Wenlan now holds (taken back, already matching the page, or
    /// decided on a card): the publish writes their files from the page.
    paths: std::collections::BTreeSet<String>,
    /// How many pages changed this time.
    wrote: usize,
    /// Edits a card could not be made for: not remembered as offered, so
    /// the next publish tries again.
    failed: std::collections::BTreeSet<String>,
}

/// Take wiki page files changed in the repository back into Wenlan.
///
/// - A file as a merged `wenlan wiki refresh` wrote it, for a page Wenlan
///   still owns and has not changed since: the page takes the AI tool's
///   rewrite as its own update, named "GitHub", and its code lines become
///   the ones the file names.
/// - A file a person edited, for a page Wenlan has not changed since: an
///   edit of the page by a person (so a review lock applies as for any edit
///   of theirs).
/// - Anything else changed on both sides: offered once as a revision card;
///   the repository keeps its text until the card is decided, and after
///   that the page goes out as Wenlan has it.
async fn take_back_edits(
    db: &MemoryDB,
    binding: &RepoWikiBinding,
    edits: &[repo_wiki::WikiFileEdit],
    pages: &[wenlan_core::pages::Page],
    page_root: Option<&std::path::Path>,
) -> Result<TakenBack, ServerError> {
    let pending: std::collections::HashSet<String> = db
        .list_pending_revisions(1000)
        .await
        .map_err(|e| ServerError::Internal(e.to_string()))?
        .into_iter()
        .filter(|card| card.source_agent.as_deref() == Some(FROM_REPOSITORY))
        .map(|card| card.target_source_id)
        .collect();
    let mut out = TakenBack::default();
    for edit in edits {
        let Some(page) = pages.iter().find(|page| page.id == edit.page_id) else {
            continue;
        };
        let content = wenlan_core::export::okf::wikilinks_from_repository_body(&edit.body, pages);
        if content.trim() == page.content.trim() {
            out.paths.insert(edit.path.clone());
            continue;
        }
        let key = repo_wiki::kept_edit_key(edit);
        if pending.contains(&page.id) {
            continue;
        }
        if binding.kept_edits.contains(&key) {
            // Offered before and decided: the page goes out as it is now.
            out.paths.insert(edit.path.clone());
            continue;
        }
        let owned = page.review_required || page.user_edited;
        let caught_up = edit.origin_version >= page.version;
        if !edit.by_refresh && caught_up {
            if take_back_person_edit(db, page, content, page_root).await {
                tracing::info!(
                    "[repo_wiki] took back the repository edit of {} ({})",
                    page.id,
                    edit.path
                );
                out.paths.insert(edit.path.clone());
                out.wrote += 1;
            }
            continue;
        }
        let source_revision = db
            .get_page_source_revision(&page.id)
            .await
            .map_err(|e| ServerError::Internal(e.to_string()))?;
        let direct = edit.by_refresh && caught_up && !owned;
        let written = wenlan_core::post_write::steep_update_page(
            db,
            &page.id,
            wenlan_types::requests::UpdatePageRequest {
                content,
                source_memory_ids: page.source_memory_ids.clone(),
                expected_version: Some(page.version),
                caller_id: None,
                operation_id: None,
            },
            Some(FROM_REPOSITORY),
            None,
            source_revision,
            Some(!direct),
            page_root,
            None,
        )
        .await;
        match written {
            Ok(result) if result.gated => {
                tracing::info!(
                    "[repo_wiki] {} changed on both sides; offered as a revision card",
                    page.id
                );
            }
            Ok(result) if result.wrote => {
                if let Err(error) = settle_code(db, binding, &page.id, &edit.spans).await {
                    tracing::warn!(
                        "[repo_wiki] {} came back from the repository; code lines not settled: {error}",
                        page.id
                    );
                }
                out.paths.insert(edit.path.clone());
                out.wrote += 1;
            }
            Ok(_) => {
                out.failed.insert(key);
            }
            Err(error) => {
                tracing::warn!(
                    "[repo_wiki] could not take back the repository change of {}: {error}",
                    page.id
                );
                out.failed.insert(key);
            }
        }
    }
    Ok(out)
}

/// A person's edit of a page file, written as their edit of the page.
async fn take_back_person_edit(
    db: &MemoryDB,
    page: &wenlan_core::pages::Page,
    content: String,
    page_root: Option<&std::path::Path>,
) -> bool {
    let written = wenlan_core::post_write::page_write(
        db,
        wenlan_core::post_write::PageWrite::UpdatePreservingSources {
            page_id: &page.id,
            req: wenlan_types::requests::UpdatePageRequest {
                content,
                source_memory_ids: page.source_memory_ids.clone(),
                expected_version: Some(page.version),
                caller_id: None,
                operation_id: None,
            },
            edited_by: "fs_edit",
            knowledge_path: page_root,
            basis: wenlan_core::db::BasisWrite::default(),
        },
    )
    .await;
    match written.map(|result| result.outcome) {
        Ok(wenlan_core::post_write::WriteOutcome::Wrote) => true,
        Ok(outcome) => {
            tracing::info!(
                "[repo_wiki] kept the repository edit of {} as it is: {outcome:?}",
                page.id
            );
            false
        }
        Err(error) => {
            tracing::warn!(
                "[repo_wiki] could not take back the repository edit of {}: {error}",
                page.id
            );
            false
        }
    }
}

/// After a page came back from the repository: make its code lines in this
/// repository the ones the repository's version names, when the working copy
/// holds those files as the default branch does. Otherwise the rows stay,
/// and the usual check settles them once the user pulls.
async fn settle_code(
    db: &MemoryDB,
    binding: &RepoWikiBinding,
    page_id: &str,
    spans: &[(String, u32, u32)],
) -> Result<(), String> {
    use wenlan_types::code_basis::{CodeBasisInput, CodeBasisLocator};
    let repo_root = binding.repo_root.clone();
    let paths: Vec<String> = spans.iter().map(|(path, _, _)| path.clone()).collect();
    let in_step = blocking(move || {
        Ok(paths
            .iter()
            .all(|path| repo_wiki::working_copy_matches_default_branch(&repo_root, path)))
    })
    .await
    .map_err(|e| format!("{e:?}"))?;
    if !in_step {
        return Ok(());
    }
    let inputs: Vec<CodeBasisInput> = spans
        .iter()
        .map(|(path, start, end)| CodeBasisInput {
            repo_root: binding.repo_root.clone(),
            path: path.clone(),
            line_start: *start,
            line_end: *end,
        })
        .collect();
    let prepared = wenlan_core::code_basis::prepare_code_basis(&inputs)
        .await
        .map_err(|e| e.to_string())?;
    let keep: std::collections::HashSet<(String, u32, u32)> = spans.iter().cloned().collect();
    for row in db
        .list_page_code_basis(page_id)
        .await
        .map_err(|e| e.to_string())?
    {
        let locator = CodeBasisLocator::from(&row);
        if locator.repo_root.trim_end_matches('/') == binding.repo_root.trim_end_matches('/')
            && !keep.contains(&(locator.path.clone(), locator.line_start, locator.line_end))
        {
            db.remove_code_basis(page_id, &locator)
                .await
                .map_err(|e| e.to_string())?;
        }
    }
    db.upsert_page_code_basis(page_id, &prepared, FROM_REPOSITORY)
        .await
        .map_err(|e| e.to_string())
}

/// Publish the bound Space `space`: used by the route and by the daily run.
/// Edits people made to its wiki pages in the repository are taken back
/// first; doc fixes go into the same pull request.
pub(crate) async fn publish_space(
    db: &MemoryDB,
    space: &str,
    page_root: Option<&std::path::Path>,
) -> Result<RepoWikiPublishResult, ServerError> {
    let rulesync = wenlan_core::rulesync_personal::enabled();
    let _project_serial = if rulesync {
        Some(PROJECT_RULES_TRANSACTION.lock().await)
    } else {
        None
    };
    let store = repo_wiki::store_path();
    let lookup_space = space.to_owned();
    let lookup_store = store.clone();
    let binding = blocking(move || repo_wiki::binding_at(&lookup_store, &lookup_space))
        .await?
        .ok_or_else(|| {
            ServerError::NotFound(format!(
                "Space {space:?} is not bound to a repository; bind it first"
            ))
        })?;

    let mut gathered = gather(db, space, &binding).await?;
    let open_fix = binding
        .doc_fixes
        .iter()
        .any(|fix| fix.state != RepoDocFixState::Dismissed);
    let worktree = repo_wiki::worktree_for(space);
    let mut prepared = None;
    let mut taken = TakenBack::default();
    if !gathered.pages.is_empty() || open_fix {
        let for_prepare = binding.clone();
        let at = worktree.clone();
        let found = blocking(move || repo_wiki::prepare(&for_prepare, &at)).await?;
        taken = take_back_edits(db, &binding, &found.edits, &gathered.pages, page_root).await?;
        if taken.wrote > 0 {
            gathered = gather(db, space, &binding).await?;
        }
        prepared = Some(found);
    }
    let TakenBack {
        paths: taken_back,
        wrote: took_back,
        failed,
    } = taken;
    let now = chrono::Utc::now().timestamp();
    let mut rules = crate::skill_grow::project_rules(db, page_root, space).await;
    let mut team_rules = Vec::new();
    if rulesync {
        let settings = shared_space::settings_path();
        if shared_space::share_settings_at(&settings, &shared_space::Scope::Space(space.into()))?
            .is_some_and(|share| share.host.is_some())
        {
            return Err(ServerError::Conflict(
                "publish the rules from the project host".into(),
            ));
        }
        if let Some(layer) = rulesync_team::hosted_project_layer(&settings, space)? {
            rules = Some(markdown_rules(&layer.markdown));
        }
        let space = project_rule_space(db, space).await?;
        if let Some(selected) = db
            .get_app_metadata(&team_rules_key(&space))
            .await?
            .filter(|value| !value.is_empty())
        {
            if let Some(layer) = rulesync_team::load_layers(&settings)?
                .into_iter()
                .find(|layer| layer.id == selected)
            {
                team_rules = markdown_rules(&layer.markdown);
            }
        }
    }
    let space = space.to_owned();
    let result = blocking(move || {
        let Gathered {
            pages,
            left_out,
            code_bases,
            concept_paths,
            anchors,
        } = gathered;
        let input = repo_wiki::PublishInput {
            binding: &binding,
            pages,
            left_out,
            code_bases: &code_bases,
            anchors: &anchors,
            concept_paths: &concept_paths,
            rules,
            publisher: repo_wiki::publisher_id_at(&store)?,
            worktree,
            gh: repo_wiki::find_gh(),
            now,
            prepared,
            taken_back,
            took_back,
        };
        let published = if rulesync {
            repo_wiki::publish_with_project_rules(input, &store, &team_rules)?
        } else {
            repo_wiki::publish(input)?
        };
        repo_wiki::settle_doc_fixes_at(&store, &space, &published.applied, &published.gone)?;
        let mut published = published;
        if let Some(kept) = published.kept_edits.as_mut() {
            kept.retain(|key| !failed.contains(key));
        }
        repo_wiki::record_publish_at(&store, &space, &published)?;
        Ok(published.result)
    })
    .await?;
    tracing::info!(
        "[repo_wiki] published {} page(s), {} doc fix(es): {:?} {}",
        result.pages,
        result.doc_fixes,
        result.status,
        result.pr_url.as_deref().unwrap_or("")
    );
    Ok(result)
}

/// Every active page of `space` the automatic reader may see, the same set
/// the OKF export writes. Refuses while a truth cutover is preparing, since
/// the exporter would otherwise delete every page file of the last publish.
async fn exportable_pages(
    db: &MemoryDB,
    space: &str,
) -> Result<Vec<wenlan_core::pages::Page>, ServerError> {
    let scope = crate::read_scope::effective_read_scope(db, Some(space), None).await?;
    let fence_before = db.cutover_fence().await?;
    let pages = db
        .list_pages_scoped("active", i64::MAX, 0, &scope)
        .await
        .map_err(|e| ServerError::Internal(e.to_string()))?;
    let mut exportable = Vec::with_capacity(pages.len());
    for page in pages {
        if wenlan_core::truth_adapter::page_write_permit(db, &page.id)
            .await
            .map_err(|e| ServerError::Internal(e.to_string()))?
            .is_some()
        {
            exportable.push(page);
        }
    }
    let fence_after = db.cutover_fence().await?;
    if fence_before.phase == wenlan_core::db::CutoverPhase::Preparing || fence_after != fence_before
    {
        return Err(wenlan_core::WenlanError::Conflict(
            "a truth cutover is in progress; publish the wiki after it finishes".to_string(),
        )
        .into());
    }
    Ok(exportable)
}
