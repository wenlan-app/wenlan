// SPDX-License-Identifier: Apache-2.0
//! Finding, looking at and holding other people's skills over HTTP, so the
//! terminal and AI tools reach what the desktop app does. Every action is
//! `wenlan_core::skill_registry`'s; this file picks the folders, checks who
//! is asking and frames the answer.
//!
//! Reading (search, preview, the list, the scan) is open to any local
//! caller. Installing, taking in and removing put files into every AI
//! tool's folder or take them away, so only the person may ask for them
//! (UX-SKILL-13): an AI tool is refused before anything is fetched or
//! written, and so is every caller while the daemon listens beyond this
//! computer, because the person header alone could then come from the
//! network.

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::Query;
use axum::http::HeaderMap;
use axum::response::Json;
use serde::Deserialize;
use wenlan_core::skill_registry::{
    self as registry, actions, Endpoints, Fetch, RegistryError, ReqwestFetch,
};
use wenlan_types::skill_registry::{
    InstalledSkillsResponse, LocalSkillsResponse, SkillImportRequest, SkillImportResult,
    SkillInstallRequest, SkillInstallResult, SkillPreview, SkillRemoveRequest, SkillRemoveResponse,
    SkillSearchResponse,
};

use crate::error::ServerError;
use crate::route_registry::{get, post, TrackedRouter};
use crate::state::SharedState;

/// The refusal an AI tool gets when it tries to change which skills the
/// person's tools hold.
pub(crate) const SKILLS_NEED_THE_PERSON: &str =
    "skills_need_the_person: only the person can install, take in or remove a skill. Ask them \
     to do it in the Wenlan app, or with `wenlan skills` in their own terminal; you may still \
     search, preview and list skills";

/// The refusal every caller gets while the daemon is bound to an address
/// other computers can reach.
pub(crate) const SKILLS_NEED_THIS_COMPUTER: &str =
    "skills_need_this_computer: this Wenlan listens beyond this computer (WENLAN_BIND_ADDR), \
     so skills cannot be installed, taken in or removed over it. Use the Wenlan app";

/// More names than any tool folder holds; a longer list is a mistake.
const MAX_IMPORT_NAMES: usize = 200;

/// One change to the skills folders at a time, so two installs of the same
/// name cannot interleave their file swaps.
static SKILLS_CHANGE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub(crate) fn register(router: TrackedRouter<SharedState>) -> TrackedRouter<SharedState> {
    router
        .route("/api/skills/registry/search", get(handle_registry_search))
        .route("/api/skills/registry/preview", get(handle_registry_preview))
        .route(
            "/api/skills/registry/installed",
            get(handle_registry_installed),
        )
        .route(
            "/api/skills/registry/local",
            get(handle_registry_scan_local),
        )
        .route(
            "/api/skills/registry/import",
            post(handle_registry_import_local),
        )
        .route(
            "/api/skills/registry/install",
            post(handle_registry_install),
        )
        .route("/api/skills/registry/remove", post(handle_registry_remove))
}

/// Where skills are kept and fetched from. Tests build one over scratch
/// folders and a fake network.
#[derive(Clone)]
struct World {
    /// The Wenlan folder that holds `skills/`.
    root: PathBuf,
    /// The skills folders of the AI tools on this computer.
    tool_dirs: Vec<PathBuf>,
    http: Arc<dyn Fetch>,
    endpoints: Endpoints,
    /// The daemon listens only on this computer, so a caller marked as the
    /// person is a program the person runs here.
    local_only: bool,
}

impl World {
    async fn system() -> Result<Self, ServerError> {
        // Looking for the AI tools' folders reads the disk.
        let tool_dirs = on_disk(system_tool_dirs).await?;
        Ok(Self {
            root: registry::default_wenlan_root(),
            tool_dirs,
            http: system_http()?,
            endpoints: system_endpoints(),
            local_only: matches!(
                crate::security::bind_scope_from_env(),
                crate::security::BindScope::Unset | crate::security::BindScope::Loopback
            ),
        })
    }
}

/// One HTTP client for the life of the daemon.
fn system_http() -> Result<Arc<dyn Fetch>, ServerError> {
    static HTTP: std::sync::OnceLock<Arc<ReqwestFetch>> = std::sync::OnceLock::new();
    if let Some(http) = HTTP.get() {
        return Ok(http.clone());
    }
    let http = Arc::new(ReqwestFetch::new().map_err(ServerError::Internal)?);
    Ok(HTTP.get_or_init(|| http).clone())
}

/// The AI tools' skills folders this daemon may read and write. A dev run
/// with its own state folder gets that folder's stand-in tools, as the app
/// does. Any other run on data that is not the installed app's (a test or a
/// trial daemon) gets none: it never reads or writes the person's real tool
/// folders.
fn system_tool_dirs() -> Vec<PathBuf> {
    tool_dirs_for(
        std::env::var_os("WENLAN_DEV_STATE_DIR")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from),
        wenlan_core::config::may_write_tool_files(),
        dirs::home_dir(),
        dirs::config_dir(),
    )
}

fn tool_dirs_for(
    dev_state: Option<PathBuf>,
    installed_app_data: bool,
    home: Option<PathBuf>,
    config: Option<PathBuf>,
) -> Vec<PathBuf> {
    if let Some(state) = dev_state {
        let tools = state.join("ai-tools");
        return actions::tool_skill_dirs(Some(&tools.join("home")), Some(&tools.join("config")));
    }
    if !installed_app_data {
        return Vec::new();
    }
    actions::tool_skill_dirs(home.as_deref(), config.as_deref())
}

/// skills.sh, wenlan.app and GitHub. A run on data that is not the
/// installed app's (a test or a trial daemon) may point all four at one
/// local stand-in, so it can be driven without the network. The installed
/// app's own run never reads the variable.
fn system_endpoints() -> Endpoints {
    endpoints_for(
        std::env::var("WENLAN_DEV_SKILLS_ENDPOINTS").ok(),
        wenlan_core::config::isolated_data_root().is_some(),
    )
}

fn endpoints_for(stand_in: Option<String>, isolated_data: bool) -> Endpoints {
    let Some(base) = stand_in.filter(|value| isolated_data && !value.is_empty()) else {
        return Endpoints::default();
    };
    let base = base.trim_end_matches('/');
    Endpoints {
        skills_sh: format!("{base}/skills-sh"),
        wenlan_site: format!("{base}/wenlan-site"),
        github_api: format!("{base}/github-api"),
        github_raw: format!("{base}/github-raw"),
    }
}

fn refusal(error: RegistryError) -> ServerError {
    match error {
        RegistryError::SearchUnavailable | RegistryError::Network(_) => {
            ServerError::Upstream(error.to_string())
        }
        RegistryError::NotFound => ServerError::NotFound(error.to_string()),
        RegistryError::FailedSafetyCheck
        | RegistryError::Ambiguous(_)
        | RegistryError::Refused(_) => ServerError::ValidationError(error.to_string()),
        RegistryError::Io { .. } => ServerError::Internal(error.to_string()),
    }
}

fn require_person(world: &World, headers: &HeaderMap) -> Result<(), ServerError> {
    if !world.local_only {
        return Err(ServerError::ValidationError(
            SKILLS_NEED_THIS_COMPUTER.into(),
        ));
    }
    if !crate::memory_routes::caller_is_person(headers) {
        return Err(ServerError::ValidationError(SKILLS_NEED_THE_PERSON.into()));
    }
    Ok(())
}

async fn on_disk<T, F>(work: F) -> Result<T, ServerError>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| ServerError::Internal(error.to_string()))
}

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    #[serde(default)]
    q: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewQuery {
    source: String,
    #[serde(alias = "skill_id")]
    skill_id: String,
}

async fn search_in(world: &World, query: &str) -> Result<SkillSearchResponse, ServerError> {
    actions::search_checked(world.http.as_ref(), &world.endpoints, query)
        .await
        .map_err(refusal)
}

async fn preview_in(world: &World, query: &PreviewQuery) -> Result<SkillPreview, ServerError> {
    let (mut preview, folder) = actions::preview_from_network(
        world.http.as_ref(),
        &world.endpoints,
        &query.source,
        &query.skill_id,
    )
    .await
    .map_err(refusal)?;
    let (root, source, skill_id) = (
        world.root.clone(),
        preview.source.clone(),
        preview.skill_id.clone(),
    );
    preview.installed_as =
        on_disk(move || actions::installed_as(&root, folder, &source, &skill_id)).await?;
    Ok(preview)
}

async fn installed_in(world: &World) -> Result<InstalledSkillsResponse, ServerError> {
    let root = world.root.clone();
    let skills = on_disk(move || actions::installed_rows(&root)).await?;
    Ok(InstalledSkillsResponse { skills })
}

async fn scan_local_in(world: &World) -> Result<LocalSkillsResponse, ServerError> {
    let (root, tools) = (world.root.clone(), world.tool_dirs.clone());
    let skills = on_disk(move || registry::scan_local(&root, &tools)).await?;
    Ok(LocalSkillsResponse { skills })
}

async fn import_local_in(
    world: &World,
    headers: &HeaderMap,
    request: SkillImportRequest,
) -> Result<SkillImportResult, ServerError> {
    require_person(world, headers)?;
    if request.names.len() > MAX_IMPORT_NAMES {
        return Err(ServerError::ValidationError(format!(
            "at most {MAX_IMPORT_NAMES} skills can be taken in at once"
        )));
    }
    let (root, tools) = (world.root.clone(), world.tool_dirs.clone());
    let now = chrono::Utc::now().timestamp();
    let _one_at_a_time = SKILLS_CHANGE_LOCK.lock().await;
    on_disk(move || actions::import_named(&root, &tools, request.names, now)).await
}

async fn install_in(
    world: &World,
    headers: &HeaderMap,
    request: SkillInstallRequest,
) -> Result<SkillInstallResult, ServerError> {
    require_person(world, headers)?;
    let (fetched, verdict) =
        actions::fetch_for_install(world.http.as_ref(), &world.endpoints, &request)
            .await
            .map_err(refusal)?;
    let (root, tools) = (world.root.clone(), world.tool_dirs.clone());
    let now = chrono::Utc::now().timestamp();
    let _one_at_a_time = SKILLS_CHANGE_LOCK.lock().await;
    on_disk(move || actions::store_and_deliver(&root, &tools, &fetched, verdict, &request, now))
        .await?
        .map_err(refusal)
}

async fn remove_in(
    world: &World,
    headers: &HeaderMap,
    request: SkillRemoveRequest,
) -> Result<SkillRemoveResponse, ServerError> {
    require_person(world, headers)?;
    let (root, tools) = (world.root.clone(), world.tool_dirs.clone());
    let name = request.name;
    let _one_at_a_time = SKILLS_CHANGE_LOCK.lock().await;
    on_disk(move || {
        registry::remove(&root, &name, &tools).map(|()| SkillRemoveResponse { removed: name })
    })
    .await?
    .map_err(|error| match error {
        RegistryError::NotFound => ServerError::NotFound("no skill by that name".into()),
        other => refusal(other),
    })
}

/// Search skills.sh. Results that fail a safety check are left out.
pub async fn handle_registry_search(
    Query(query): Query<SearchQuery>,
) -> Result<Json<SkillSearchResponse>, ServerError> {
    Ok(Json(search_in(&World::system().await?, &query.q).await?))
}

/// Every file of one skill and its safety checks, before anything is kept.
pub async fn handle_registry_preview(
    Query(query): Query<PreviewQuery>,
) -> Result<Json<SkillPreview>, ServerError> {
    Ok(Json(preview_in(&World::system().await?, &query).await?))
}

/// The skills Wenlan holds, each with its scope and the person's additions.
pub async fn handle_registry_installed() -> Result<Json<InstalledSkillsResponse>, ServerError> {
    Ok(Json(installed_in(&World::system().await?).await?))
}

/// Skills in this computer's AI tool folders that Wenlan does not hold yet.
pub async fn handle_registry_scan_local() -> Result<Json<LocalSkillsResponse>, ServerError> {
    Ok(Json(scan_local_in(&World::system().await?).await?))
}

/// Take in skills the scan found, by name. Only the person may ask.
pub async fn handle_registry_import_local(
    headers: HeaderMap,
    Json(request): Json<SkillImportRequest>,
) -> Result<Json<SkillImportResult>, ServerError> {
    Ok(Json(
        import_local_in(&World::system().await?, &headers, request).await?,
    ))
}

/// Install exactly the commit a preview showed. Only the person may ask.
pub async fn handle_registry_install(
    headers: HeaderMap,
    Json(request): Json<SkillInstallRequest>,
) -> Result<Json<SkillInstallResult>, ServerError> {
    Ok(Json(
        install_in(&World::system().await?, &headers, request).await?,
    ))
}

/// Remove a held skill and the copies Wenlan wrote. Only the person may ask.
pub async fn handle_registry_remove(
    headers: HeaderMap,
    Json(request): Json<SkillRemoveRequest>,
) -> Result<Json<SkillRemoveResponse>, ServerError> {
    Ok(Json(
        remove_in(&World::system().await?, &headers, request).await?,
    ))
}

#[cfg(test)]
#[path = "skill_registry_routes_test.rs"]
mod tests;
