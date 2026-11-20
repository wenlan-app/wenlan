// SPDX-License-Identifier: Apache-2.0
//! Finding, looking at and holding other people's skills over HTTP, so the
//! terminal and AI tools reach what the desktop app does. The core registry
//! validates and stores each skill. The opt-in rulesync applier handles
//! tool-file delivery for confirmed changes.
//!
//! Reading (search, preview, the list, the scan) is open to any local
//! caller. Installing, taking in and removing put files into every AI
//! tool's folder or take them away, so only the person may ask for them
//! (UX-SKILL-13): an AI tool is refused before anything is fetched or
//! written, and so is every caller while the daemon listens beyond this
//! computer, because the person header alone could then come from the
//! network.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::Query;
use axum::http::HeaderMap;
use axum::response::Json;
use serde::Deserialize;
use wenlan_core::rulesync_personal;
use wenlan_core::skill_registry::{
    self as registry, actions, Endpoints, Fetch, RegistryError, ReqwestFetch,
};
use wenlan_types::skill_registry::{
    InstalledSkillsResponse, LocalSkill, LocalSkillsResponse, SkillImportRequest,
    SkillImportResult, SkillInstallRequest, SkillInstallResult, SkillPreview, SkillRemoveRequest,
    SkillRemoveResponse, SkillSearchResponse,
};

use crate::error::ServerError;
use crate::route_registry::{get, post, TrackedRouter};
use crate::rulesync_registry;
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
    delivery: Delivery,
    http: Arc<dyn Fetch>,
    endpoints: Endpoints,
    /// The daemon listens only on this computer, so a caller marked as the
    /// person is a program the person runs here.
    local_only: bool,
}

#[derive(Clone)]
enum Delivery {
    Legacy(Vec<PathBuf>),
    /// A selected home for reading import sources. Only the personal setup
    /// applier decides where to deliver copies.
    Rulesync(Option<PathBuf>),
}

impl World {
    async fn system() -> Result<Self, ServerError> {
        let delivery = on_disk(system_delivery).await??;
        Ok(Self {
            root: registry::default_wenlan_root(),
            delivery,
            http: system_http()?,
            endpoints: system_endpoints(),
            local_only: matches!(
                crate::security::bind_scope_from_env(),
                crate::security::BindScope::Unset | crate::security::BindScope::Loopback
            ),
        })
    }
}

fn system_delivery() -> Result<Delivery, ServerError> {
    let dev_state = std::env::var_os("WENLAN_DEV_STATE_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let installed_app_data = wenlan_core::config::may_write_tool_files();
    if rulesync_personal::enabled() {
        let home = if dev_state.is_some() || installed_app_data {
            Some(rulesync_personal::system_home().map_err(internal)?)
        } else {
            None
        };
        return Ok(Delivery::Rulesync(home));
    }
    Ok(Delivery::Legacy(tool_dirs_for(
        dev_state,
        installed_app_data,
        dirs::home_dir(),
        dirs::config_dir(),
    )))
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

/// The legacy tool folders this daemon may read and write. A dev run
/// with its own state folder gets that folder's stand-in tools, as the app
/// does. Any other run on data that is not the installed app's (a test or a
/// trial daemon) gets none: it never reads or writes the person's real tool
/// folders.
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

fn internal(error: anyhow::Error) -> ServerError {
    ServerError::Internal(format!("{error:#}"))
}

fn remove_error(error: RegistryError) -> ServerError {
    match error {
        RegistryError::NotFound => ServerError::NotFound("no skill by that name".into()),
        other => refusal(other),
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
    let include_authored = matches!(&world.delivery, Delivery::Rulesync(Some(_)));
    let skills = on_disk(move || {
        let installed = actions::installed_rows(&root);
        if !include_authored {
            return Ok(installed);
        }
        let mut by_name: BTreeMap<_, _> = installed
            .into_iter()
            .map(|row| (row.skill.name.clone(), row))
            .collect();
        for row in rulesync_registry::authored_rows().map_err(internal)? {
            by_name.insert(row.skill.name.clone(), row);
        }
        Ok::<_, ServerError>(by_name.into_values().collect())
    })
    .await??;
    Ok(InstalledSkillsResponse { skills })
}

async fn scan_local_in(world: &World) -> Result<LocalSkillsResponse, ServerError> {
    let (root, delivery) = (world.root.clone(), world.delivery.clone());
    let skills = on_disk(move || match delivery {
        Delivery::Legacy(tools) => registry::scan_local(&root, &tools),
        Delivery::Rulesync(home) => scan_rulesync_sources(&root, home.as_deref()),
    })
    .await?;
    Ok(LocalSkillsResponse { skills })
}

fn scan_rulesync_sources(root: &Path, home: Option<&Path>) -> Vec<LocalSkill> {
    home.map_or_else(Vec::new, |home| rulesync_registry::scan_sources(root, home))
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
    let (root, delivery) = (world.root.clone(), world.delivery.clone());
    let now = chrono::Utc::now().timestamp();
    let _one_at_a_time = SKILLS_CHANGE_LOCK.lock().await;
    on_disk(move || match delivery {
        Delivery::Legacy(tools) => Ok(actions::import_named(&root, &tools, request.names, now)),
        Delivery::Rulesync(home) => {
            let result = rulesync_registry::with_sources_lock(|| {
                let found = scan_rulesync_sources(&root, home.as_deref());
                actions::store_local_named(&root, &found, request.names, now)
            })
            .map_err(internal)?;
            if home.is_some() && !result.imported.is_empty() {
                match rulesync_registry::invoke_delivery() {
                    Ok(report) if !report.blocked.is_empty() => {
                        tracing::warn!(
                            files = report.blocked.len(),
                            "[skills] tool delivery needs attention"
                        );
                    }
                    Err(error) => {
                        tracing::warn!(%error, "[skills] imported skills could not be delivered")
                    }
                    _ => {}
                }
            }
            Ok(result)
        }
    })
    .await?
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
    let (root, delivery) = (world.root.clone(), world.delivery.clone());
    let now = chrono::Utc::now().timestamp();
    let _one_at_a_time = SKILLS_CHANGE_LOCK.lock().await;
    on_disk(move || match delivery {
        Delivery::Legacy(tools) => {
            actions::store_and_deliver(&root, &tools, &fetched, verdict, &request, now)
                .map_err(refusal)
        }
        Delivery::Rulesync(home) => {
            let skill = rulesync_registry::with_sources_lock(|| {
                actions::store_checked(&root, &fetched, verdict, &request, now)
            })
            .map_err(internal)?
            .map_err(refusal)?;
            let mut result = SkillInstallResult {
                skill,
                tools: Vec::new(),
                skipped: Vec::new(),
            };
            if home.is_some() {
                rulesync_registry::invoke_delivery().map_err(kept_skill_error)?;
                let (tools, skipped) =
                    crate::rulesync_routes::registry_delivery_status(&result.skill.name)
                        .map_err(kept_skill_error)?;
                result.tools = tools
                    .into_iter()
                    .map(|path| path.display().to_string())
                    .collect();
                result.skipped = skipped
                    .into_iter()
                    .map(|path| path.display().to_string())
                    .collect();
            }
            Ok(result)
        }
    })
    .await?
}

fn kept_skill_error(error: anyhow::Error) -> ServerError {
    ServerError::Conflict(format!(
        "The skill is kept in Wenlan. Tool delivery needs attention. {error:#}"
    ))
}

async fn remove_in(
    world: &World,
    headers: &HeaderMap,
    request: SkillRemoveRequest,
) -> Result<SkillRemoveResponse, ServerError> {
    require_person(world, headers)?;
    let (root, delivery) = (world.root.clone(), world.delivery.clone());
    let name = request.name;
    let _one_at_a_time = SKILLS_CHANGE_LOCK.lock().await;
    on_disk(move || {
        match delivery {
            Delivery::Legacy(tools) => {
                registry::remove(&root, &name, &tools).map_err(remove_error)?
            }
            Delivery::Rulesync(home) => {
                let legacy_removed = match rulesync_registry::with_sources_lock(|| {
                    registry::remove(&root, &name, &[])
                })
                .map_err(internal)?
                {
                    Ok(()) => true,
                    Err(RegistryError::NotFound) => false,
                    Err(error) => return Err(remove_error(error)),
                };
                let authored_removed = home.is_some()
                    && rulesync_registry::remove_saved_source(&name).map_err(internal)?;
                if !legacy_removed && !authored_removed {
                    return Err(remove_error(RegistryError::NotFound));
                }
                if home.is_some() {
                    rulesync_registry::invoke_delivery()
                        .map_err(|error| removed_skill_error(&name, &format!("{error:#}")))?;
                    let remaining = crate::rulesync_routes::registry_pending_paths(&name)
                        .map_err(|error| removed_skill_error(&name, &format!("{error:#}")))?;
                    if !remaining.is_empty() {
                        return Err(removed_skill_error(
                            &name,
                            "Run `wenlan rules sync` to review the remaining copies.",
                        ));
                    }
                }
            }
        }
        Ok(SkillRemoveResponse { removed: name })
    })
    .await?
}

fn removed_skill_error(name: &str, detail: &str) -> ServerError {
    ServerError::Conflict(format!(
        "Removed {name} from Wenlan. Tool-copy removal needs attention. {detail}"
    ))
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
