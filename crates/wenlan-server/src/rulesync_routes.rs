// SPDX-License-Identifier: Apache-2.0
//! The local setup endpoints. The same file applier handles enrolment,
//! delivery and restoration.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    ops::Bound::{Excluded, Unbounded},
    path::{Path, PathBuf},
};

use anyhow::{bail, ensure, Context, Result};
use axum::{extract::State, http::HeaderMap, Json};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use wenlan_core::rulesync_adopt::{AdoptedSkill, Adoption, AdoptionEntry};
use wenlan_core::rulesync_personal::{
    additional_targets, adoption_plan, adoption_token, check_delivery_paths,
    check_skill_frontmatter, describe_change, discover_additional_adoption, discover_adoption,
    fingerprint, is_harness_path, line_changes, merge_rule_text, personal_target,
    stage_adopted_skill, system_home, verify_adoption_token, write_root_rule, DeliveryWorktrees,
    GlobalOutput, HarnessGuard, LineChange, LocalText, PersonalMaster, PersonalTarget,
    PERSONAL_TARGETS,
};
use wenlan_core::rulesync_renderer::{self, RenderSession};
use wenlan_core::rulesync_skill::SkillFiles;
use wenlan_core::rulesync_team::{self, TeamRuleLayer};
use wenlan_types::rulesync::{
    AdoptPreviewRequest, AdoptRequest, AdoptedSkillUpdate, AdoptionPlan, PullbackAction,
    PullbackCard, PullbackDecision, RulesSyncReport,
};
use wenlan_types::rulesync_skill::{
    SavedSkillTool, SkillDeliveryChange, SkillDeliveryFile, SkillFileDigest,
};

use crate::error::ServerError;
use crate::route_registry::{post, TrackedRouter};
use crate::rulesync_apply::{ApplyOptions, ApplyOutcome, FileMode, FileRoot, Snapshot};
use crate::rulesync_layers;
use crate::rulesync_registry::{
    self, ApprovedSkills, ManagedSkill, SavedSkillDeliveries, SkillHome, SkillModes,
};
use crate::state::SharedState;

mod displaced;
mod settings;
mod storage;

pub(crate) use settings::SETTINGS_RESUME_NOTICE;

pub(crate) fn register(router: TrackedRouter<SharedState>) -> TrackedRouter<SharedState> {
    router
        .route("/api/rules/setup/adopt", post(adopt))
        .route("/api/rules/setup/adopt/preview", post(adopt_preview))
        .route("/api/rules/setup/restore", post(restore))
        .route("/api/rules/setup/sync", post(sync))
        .route("/api/rules/setup/skill-updates", post(skill_updates))
        .route("/api/rules/setup/review", post(review))
}

pub(crate) fn require_local_person(headers: &HeaderMap) -> Result<(), ServerError> {
    if !matches!(
        crate::security::bind_scope_from_env(),
        crate::security::BindScope::Unset | crate::security::BindScope::Loopback
    ) || !crate::memory_routes::caller_is_person(headers)
    {
        return Err(ServerError::ValidationError(
            "only the person using the daemon on this computer can change the tool setup".into(),
        ));
    }
    Ok(())
}

fn require_personal_rules_writer(headers: &HeaderMap) -> Result<(), ServerError> {
    require_local_person(headers)?;
    if !wenlan_core::rulesync_personal::enabled() {
        return Err(ServerError::ValidationError(
            "WENLAN_RULESYNC is off".into(),
        ));
    }
    Ok(())
}

async fn adopt_preview(
    headers: HeaderMap,
    Json(request): Json<AdoptPreviewRequest>,
) -> Result<Json<AdoptionPlan>, ServerError> {
    require_personal_rules_writer(&headers)?;
    tokio::task::spawn_blocking(move || Setup::system()?.preview_adoption(&request.home))
        .await
        .map_err(|error| ServerError::Internal(error.to_string()))?
        .map(Json)
        .map_err(|error| ServerError::Conflict(format!("{error:#}")))
}

async fn adopt(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(request): Json<AdoptRequest>,
) -> Result<Json<RulesSyncReport>, ServerError> {
    execute(state, headers, Operation::Adopt(request)).await
}

async fn restore(
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> Result<Json<RulesSyncReport>, ServerError> {
    execute(state, headers, Operation::Restore).await
}

async fn sync(
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> Result<Json<RulesSyncReport>, ServerError> {
    execute(state, headers, Operation::Sync).await
}

async fn skill_updates(headers: HeaderMap) -> Result<Json<Vec<AdoptedSkillUpdate>>, ServerError> {
    use wenlan_core::skill_registry::{self, Endpoints, ReqwestFetch};

    require_personal_rules_writer(&headers)?;
    let (home, skills) = tokio::task::spawn_blocking(|| {
        let setup = Setup::system()?;
        let manifest = setup
            .load()?
            .filter(|manifest| manifest.adopted)
            .context("run wenlan rules adopt first")?;
        Ok::<_, anyhow::Error>((setup.home, manifest.master.skills))
    })
    .await
    .map_err(|error| ServerError::Internal(error.to_string()))?
    .map_err(|error| ServerError::Conflict(error.to_string()))?;
    let http = ReqwestFetch::new().map_err(ServerError::Internal)?;
    Ok(Json(
        skill_registry::adopted_updates(&http, &Endpoints::default(), &home, &skills).await,
    ))
}

async fn review(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(request): Json<PullbackDecision>,
) -> Result<Json<RulesSyncReport>, ServerError> {
    execute(state, headers, Operation::Review(request)).await
}

enum Operation {
    Adopt(AdoptRequest),
    Restore,
    Sync,
    Review(PullbackDecision),
}

async fn execute(
    state: SharedState,
    headers: HeaderMap,
    operation: Operation,
) -> Result<Json<RulesSyncReport>, ServerError> {
    require_personal_rules_writer(&headers)?;
    let generated = if matches!(operation, Operation::Sync | Operation::Review(_)) {
        crate::skill_grow::general_tool_rules(&state).await?
    } else {
        Vec::new()
    };
    let result = tokio::task::spawn_blocking(move || {
        let setup = Setup::system()?;
        match operation {
            Operation::Adopt(request) => setup.adopt(&request),
            Operation::Restore => setup.restore(),
            Operation::Sync => setup.sync(&generated),
            Operation::Review(request) => setup.review(&request, &generated),
        }
    })
    .await
    .map_err(|error| ServerError::Internal(error.to_string()))?;
    result
        .map(Json)
        .map_err(|error| ServerError::Conflict(format!("{error:#}")))
}

pub(crate) fn sync_from_pages(generated: &[String]) -> Result<()> {
    let setup = Setup::system()?;
    if setup.data.join("personal.json").is_file() {
        let report = setup.sync(generated)?;
        if !report.blocked.is_empty() {
            tracing::warn!(
                files = report.blocked.len(),
                "[rules setup] some deliveries are waiting for local review"
            );
        }
    }
    Ok(())
}

pub(crate) struct DeliveryStatus {
    pub path: PathBuf,
    pub at: Option<i64>,
    pub blocked: Option<&'static str>,
}

pub(crate) fn delivery_status() -> BTreeMap<String, DeliveryStatus> {
    let Ok(setup) = Setup::system() else {
        return BTreeMap::new();
    };
    setup.delivery_status()
}

fn recorded_delivery(
    home: &FileRoot,
    path: &Path,
    delivery: &Delivery,
) -> (Option<i64>, Option<&'static str>) {
    match home.read(path, true) {
        Ok(actual) if actual == delivery.delivered => (Some(delivery.at), None),
        Ok(Snapshot::Symlink { .. }) => (None, Some("symlink_requires_adoption")),
        Ok(Snapshot::Directory { .. }) => (None, Some("not_a_file")),
        Ok(Snapshot::Absent) => (None, Some("managed_file_missing")),
        Ok(Snapshot::File { .. }) => (None, Some("local_edit_pending")),
        Err(error) if error.kind() == std::io::ErrorKind::FileTooLarge => {
            (None, Some("file_too_large"))
        }
        Err(_) => (None, Some("not_readable")),
    }
}

fn agent_for_target(target: &str) -> &str {
    personal_target(target).map_or(target, |tool| tool.agent)
}

fn path_blocked(blocked: &BTreeSet<PathBuf>, path: &Path) -> bool {
    path.ancestors()
        .any(|candidate| blocked.contains(candidate))
}

const HARNESS_READ_ONLY: &str =
    "Wenlan leaves ~/.agents untouched. Use a tool path outside the harness.";

#[derive(Debug)]
pub(crate) struct UnsupportedRulesyncState;

impl std::fmt::Display for UnsupportedRulesyncState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            "Unsupported or damaged rulesync state. Run `wenlan rules restore` with the build that wrote it and the same tool home and data directory. Then archive the old rulesync data and adopt again.",
        )
    }
}

impl std::error::Error for UnsupportedRulesyncState {}

impl Setup {
    fn delivery_status(&self) -> BTreeMap<String, DeliveryStatus> {
        let Ok(home) = FileRoot::open_home(&self.home) else {
            return BTreeMap::new();
        };
        let mut tools = BTreeMap::new();
        for candidate in PERSONAL_TARGETS {
            let path = Path::new(candidate.rules.destination);
            if matches!(home.read(path, true), Ok(Snapshot::Symlink { .. })) {
                tools.insert(
                    candidate.agent.into(),
                    DeliveryStatus {
                        path: self.home.join(path),
                        at: None,
                        blocked: Some("symlink_requires_adoption"),
                    },
                );
            }
        }
        let Ok(Some(manifest)) = self.load() else {
            return tools;
        };
        for (path, delivery) in manifest.active_deliveries() {
            let Kind::Rule { target, .. } = &delivery.kind else {
                continue;
            };
            let (mut at, mut blocked) = recorded_delivery(&home, path, delivery);
            if let Some(issue) = manifest
                .delivery_issues
                .get(path)
                .filter(|_| blocked.is_none())
            {
                at = None;
                blocked = Some(issue.label());
            }
            tools.insert(
                agent_for_target(target).into(),
                DeliveryStatus {
                    path: self.home.join(path),
                    at,
                    blocked,
                },
            );
        }
        tools
    }
}

pub(crate) fn sync_registry_now() -> Result<RulesSyncReport> {
    let setup = Setup::system()?;
    let mut result = report("Adopt the tool setup before delivering installed skills");
    if !setup.data.join("personal.json").is_file() {
        return Ok(result);
    }
    let _lock = setup.lock()?;
    let Some(mut manifest) = setup.load()? else {
        return Ok(result);
    };
    rulesync_registry::check_skill_format(&setup.data)?;
    let root = FileRoot::open_home(&setup.home)?;
    setup.recover(&root, &mut manifest)?;
    ensure!(manifest.adopted, "finish restoring the local setup first");
    result.message = "Checked installed skills".into();
    let blocked = setup.pull_back(&root, &mut manifest, &mut result)?;
    let session = RenderSession::new(&setup.binary);
    let saved = rulesync_registry::saved_skill_deliveries(&setup.data)?;
    setup.sync_registry(
        &root,
        &mut manifest,
        &blocked,
        &mut result,
        &saved,
        |renderer, skills| setup.render_skills(&session, renderer, skills),
    )?;
    Ok(result)
}

pub(crate) fn registry_delivery_status(name: &str) -> Result<(Vec<PathBuf>, Vec<PathBuf>)> {
    let setup = Setup::system()?;
    if !setup.data.join("personal.json").is_file() {
        return Ok((Vec::new(), Vec::new()));
    }
    let Some(manifest) = setup.load()? else {
        return Ok((Vec::new(), Vec::new()));
    };
    let root = FileRoot::open_home(&setup.home)?;
    let current = manifest
        .delivered
        .iter()
        .filter_map(|(path, record)| {
            let Kind::RegistryDirectory { name: installed } = &record.kind else {
                return None;
            };
            let skill_md = path.join("SKILL.md");
            (installed == name
                && manifest.delivered.get(&skill_md).is_some_and(|record| {
                    root.read(&skill_md, false)
                        .is_ok_and(|snapshot| snapshot == record.delivered)
                }))
            .then(|| setup.home.join(path))
        })
        .collect();
    let skipped = manifest
        .registry_skipped
        .get(name)
        .into_iter()
        .flatten()
        .map(|path| setup.home.join(path))
        .collect();
    Ok((current, skipped))
}

pub(crate) fn registry_pending_paths(name: &str) -> Result<Vec<PathBuf>> {
    let setup = Setup::system()?;
    if !setup.data.join("personal.json").is_file() {
        return Ok(Vec::new());
    }
    let Some(manifest) = setup.load()? else {
        return Ok(Vec::new());
    };
    let root = FileRoot::open_home(&setup.home)?;
    Ok(manifest
        .delivered
        .iter()
        .filter_map(|(path, delivery)| {
            let (Kind::RegistryDirectory { name: installed }
            | Kind::RegistryAssetDirectory { name: installed }
            | Kind::RegistrySkill { name: installed }) = &delivery.kind
            else {
                return None;
            };
            (installed == name && !matches!(root.read(path, false), Ok(Snapshot::Absent)))
                .then(|| setup.home.join(path))
        })
        .collect())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum Kind {
    Rule {
        target: String,
        source: String,
        base: String,
    },
    Skill,
    Directory,
    Settings,
    RegistrySkill {
        name: String,
    },
    RegistryDirectory {
        name: String,
    },
    RegistryAssetDirectory {
        name: String,
    },
    RuleRevocation {
        accessible: BTreeSet<String>,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct Delivery<Content = Vec<u8>> {
    kind: Kind,
    delivered: Snapshot<Content>,
    observed: Option<Snapshot<Content>>,
    // A person's decision used this observed version. A newer edit needs a
    // fresh decision while delivery of the reviewed version is outstanding.
    reviewed: bool,
    at: i64,
}

#[derive(Debug, Serialize, Deserialize)]
struct Pending<Content = Vec<u8>> {
    path: PathBuf,
    seen: Snapshot<Content>,
    desired: Snapshot<Content>,
    kind: Option<Kind>,
    allow_link: bool,
}

enum BatchMode {
    Adopt,
    Extend,
    Continue,
    Restore,
    SealRestore { extension_only: bool },
    Review,
    Settings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PendingStage {
    id: String,
    displaced: Option<PathBuf>,
}

impl PendingStage {
    fn options<'a>(
        &'a self,
        data: &'a FileRoot,
        allow_symlink: bool,
        verify_source: Option<&'a dyn Fn() -> std::io::Result<bool>>,
    ) -> ApplyOptions<'a> {
        ApplyOptions {
            allow_symlink,
            displaced: self.displaced.as_deref().map(|path| (data, path)),
            stage_id: Some(&self.id),
            verify_source,
            #[cfg(windows)]
            restore_owner: None,
        }
    }
}

#[derive(Default)]
struct BatchDirectories {
    parents: BTreeSet<PathBuf>,
    replaced: BTreeSet<PathBuf>,
}

impl BatchDirectories {
    fn record(&mut self, operation: &Pending) {
        if operation.seen != operation.desired {
            self.parents
                .extend(operation.path.ancestors().skip(1).map(Path::to_owned));
            if operation.seen.is_directory() && !operation.desired.is_directory() {
                self.replaced.insert(operation.path.clone());
            }
        }
    }

    fn sync(self, root: &FileRoot) -> Result<()> {
        let mut parents: Vec<_> = self.parents.into_iter().collect();
        parents.sort_unstable_by_key(|path| std::cmp::Reverse(path.components().count()));
        for path in parents {
            if !path_blocked(&self.replaced, &path) {
                root.sync_directory(&path)?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Manifest<Content = Vec<u8>> {
    master: PersonalMaster<Content>,
    /// Every destination's original bytes, mode or exact link value. This
    /// whole map reaches disk before the first tool file changes.
    backup: BTreeMap<PathBuf, Snapshot<Content>>,
    /// The ordinary inherited Windows ACL is determined by the owner. An
    /// older receipt contains only modes, so it cannot authorize new bytes.
    #[cfg(windows)]
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    windows_backup_owners: BTreeMap<PathBuf, Vec<u8>>,
    delivered: BTreeMap<PathBuf, Delivery<Content>>,
    pending: Vec<Pending<Content>>,
    stages: BTreeMap<PathBuf, PendingStage>,
    recovery_issues: BTreeMap<PathBuf, String>,
    adopted: bool,
    /// Journaled destinations of an addition that has not enrolled yet.
    extension_paths: BTreeSet<PathBuf>,
    settings_only: bool,
    settings_batch: Option<settings::SettingsBatch>,
    registry_skipped: BTreeMap<String, BTreeSet<PathBuf>>,
    unmanaged_skills: BTreeSet<PathBuf>,
    worktree_writes: BTreeMap<PathBuf, PathBuf>,
    scope_reviews: BTreeMap<String, ScopeReview>,
    delivery_issues: BTreeMap<PathBuf, RuleDeliveryIssue>,
}

impl Manifest {
    fn active_deliveries(&self) -> impl Iterator<Item = (&PathBuf, &Delivery)> {
        self.delivered
            .iter()
            .filter(|(path, _)| !self.extension_paths.contains(*path))
    }

    fn new(master: PersonalMaster, backup: BTreeMap<PathBuf, Snapshot>) -> Self {
        Self {
            master,
            backup,
            #[cfg(windows)]
            windows_backup_owners: BTreeMap::new(),
            delivered: BTreeMap::new(),
            pending: Vec::new(),
            stages: BTreeMap::new(),
            recovery_issues: BTreeMap::new(),
            adopted: false,
            extension_paths: BTreeSet::new(),
            settings_only: false,
            settings_batch: None,
            registry_skipped: BTreeMap::new(),
            unmanaged_skills: BTreeSet::new(),
            worktree_writes: BTreeMap::new(),
            scope_reviews: BTreeMap::new(),
            delivery_issues: BTreeMap::new(),
        }
    }
}

#[cfg(test)]
thread_local! {
    static MANIFEST_SAVES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum RuleDeliveryIssue {
    MasterConflict,
    RendererUnavailable,
    MissingOutput,
    ConcurrentEdit,
}

impl RuleDeliveryIssue {
    fn label(&self) -> &'static str {
        match self {
            Self::MasterConflict => "master_conflict",
            Self::RendererUnavailable => "renderer_unavailable",
            Self::MissingOutput => "missing_renderer_output",
            Self::ConcurrentEdit => "changed_during_sync",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ScopeReview {
    path: PathBuf,
    before: String,
    after: String,
    card: PullbackCard,
}

struct ExpectedRegistrySkill {
    name: String,
    wanted: BTreeSet<PathBuf>,
    snapshot: Option<ManagedSkill>,
}

enum RegistryInputs<'a> {
    Sync(&'a SavedSkillDeliveries),
    Review {
        approved: &'a BTreeMap<PathBuf, ManagedSkill>,
        sources: &'a BTreeMap<SavedSkillTool, ApprovedSkills>,
        explicit_file_modes: &'a BTreeSet<PathBuf>,
    },
}

struct RegistryPlan {
    moves: BTreeMap<PathBuf, PathBuf>,
    move_out: Vec<Pending>,
    writes: Vec<Pending>,
    removals: Vec<Pending>,
    packages: BTreeSet<PathBuf>,
    previous_skips: BTreeMap<String, BTreeSet<PathBuf>>,
}

impl RegistryPlan {
    fn operations(&self) -> impl Iterator<Item = &Pending> {
        self.move_out
            .iter()
            .chain(&self.writes)
            .chain(&self.removals)
    }
}

struct ReviewedRegistryPlan {
    root: FileRoot,
    manifest: Manifest,
    plan: RegistryPlan,
}

pub(crate) struct SkillReviewDelivery {
    delivery: Option<ReviewedRegistryPlan>,
    pub(crate) report: RulesSyncReport,
    pub(crate) tools: BTreeSet<SavedSkillTool>,
}

impl SkillReviewDelivery {
    pub(crate) fn digest(&self) -> Result<String> {
        fingerprint(&self.delivery.as_ref().map(|delivery| {
            (
                &delivery.plan.moves,
                delivery.plan.operations().collect::<Vec<_>>(),
            )
        }))
    }

    pub(crate) fn changes(&self) -> Vec<SkillDeliveryChange> {
        let Some(delivery) = &self.delivery else {
            return Vec::new();
        };
        let mut changes: Vec<_> = delivery
            .plan
            .operations()
            .filter(|op| op.seen != op.desired)
            .filter(|op| !delivery.plan.moves.values().any(|path| *path == op.path))
            .map(|op| {
                let entry = |snapshot: &Snapshot| match snapshot {
                    Snapshot::File { bytes, mode } => Some(SkillDeliveryFile {
                        digest: SkillFileDigest {
                            bytes: bytes.len(),
                            sha256: format!("{:x}", Sha256::digest(bytes)),
                        },
                        permissions: mode.to_string(),
                        directory: false,
                    }),
                    Snapshot::Directory { mode } => Some(SkillDeliveryFile {
                        digest: SkillFileDigest {
                            bytes: 0,
                            sha256: format!("{:x}", Sha256::digest(b"directory")),
                        },
                        permissions: mode.to_string(),
                        directory: true,
                    }),
                    Snapshot::Absent => None,
                    Snapshot::Symlink { .. } => unreachable!("skill approval cannot change links"),
                };
                let previous = delivery.plan.moves.get(&op.path);
                let before = previous
                    .and_then(|path| delivery.plan.move_out.iter().find(|op| op.path == *path))
                    .map_or(&op.seen, |op| &op.seen);
                SkillDeliveryChange {
                    path: preview_path(&op.path),
                    renamed_from: previous.map(|path| preview_path(path)),
                    before: entry(before),
                    after: entry(&op.desired),
                }
            })
            .collect();
        changes.sort_by(|left, right| left.path.cmp(&right.path));
        changes
    }
}

fn preview_path(path: &Path) -> String {
    path.iter()
        .map(|part| part.to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

pub(crate) struct Setup {
    pub(crate) home: PathBuf,
    pub(crate) data: PathBuf,
    pub(crate) binary: PathBuf,
}

impl Setup {
    pub(crate) fn system() -> Result<Self> {
        let dev_state =
            std::env::var_os("WENLAN_DEV_STATE_DIR").is_some_and(|path| !path.is_empty());
        ensure!(
            if dev_state {
                wenlan_core::config::isolated_data_root().is_some()
            } else {
                wenlan_core::config::may_write_tool_files()
            },
            "this daemon has no isolated AI-tool home"
        );
        let root = wenlan_core::config::data_root().join("rulesync");
        let binary = std::env::var_os("WENLAN_RULESYNC_BINARY")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                root.join(rulesync_renderer::pinned_binary_name().unwrap_or("rulesync"))
            });
        Ok(Self {
            home: system_home()?,
            data: root,
            binary,
        })
    }

    pub(crate) fn data_files(&self) -> Result<FileRoot> {
        Ok(FileRoot::open_data(&self.home, &self.data)?)
    }

    pub(crate) fn lock(&self) -> Result<fs::File> {
        let guard = wenlan_core::rulesync_personal::HarnessGuard::new(&self.home)?;
        let data_parent = self.data.parent().context("rulesync has no data parent")?;
        guard.check_absolute(data_parent)?;
        if self.data.try_exists()? {
            guard.check_absolute(&self.data)?;
        }
        if !self.data.try_exists()? {
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&self.data) {
                Ok(()) => {
                    #[cfg(unix)]
                    fs::File::open(self.data.parent().context("rulesync has no data parent")?)?
                        .sync_all()?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error.into()),
            }
        }
        let lock = self.data_files()?.open_private_lock()?;
        FileExt::lock_exclusive(&lock)?;
        Ok(lock)
    }

    fn check_adoption_home(&self, expected: &Path) -> Result<()> {
        ensure!(
            expected.is_absolute() && self.home.canonicalize()? == expected,
            "The CLI and daemon use different tool homes. Check HOME and WENLAN_DEV_STATE_DIR."
        );
        Ok(())
    }

    fn adoptable_manifest(&self) -> Result<Option<Manifest>> {
        let manifest = self.load_if_present()?;
        rulesync_registry::check_skill_format(&self.data)?;
        if let Some(manifest) = manifest.as_ref().filter(|manifest| !manifest.settings_only) {
            ensure!(
                manifest.adopted,
                "The previous setup is incomplete. Run `wenlan rules restore` before adopting again."
            );
            ensure!(
                manifest.pending.is_empty() && manifest.extension_paths.is_empty(),
                "A setup write is pending. Run `wenlan rules sync` before adopting another tool."
            );
        }
        Ok(manifest)
    }

    fn preview_adoption(&self, expected_home: &Path) -> Result<AdoptionPlan> {
        self.check_adoption_home(expected_home)?;
        let session = RenderSession::new(&self.binary);
        self.preview_adoption_using(|renderer, skills| {
            self.render_skills(&session, renderer, skills)
        })
    }

    fn discover_for_adoption(&self, manifest: Option<&Manifest>) -> Result<Adoption> {
        if let Some(manifest) = manifest.filter(|manifest| manifest.adopted) {
            discover_additional_adoption(&self.home, &manifest.master)
        } else {
            discover_adoption(&self.home)
        }
    }

    fn preview_adoption_using(
        &self,
        render_skills: impl Fn(&str, &SkillPackages) -> Result<SkillFiles>,
    ) -> Result<AdoptionPlan> {
        let manifest = self.adoptable_manifest()?;
        let mut adoption = self.discover_for_adoption(manifest.as_ref())?;
        plan_skill_deliveries(&mut adoption, &render_skills)?;
        let mut plan = adoption_plan(&adoption)?;
        if adoption.overlays.is_empty()
            && plan.files.is_empty()
            && manifest.is_some_and(|manifest| manifest.adopted)
        {
            plan.notes.push("No additional tools found.".into());
        }
        Ok(plan)
    }

    fn adopt(&self, request: &AdoptRequest) -> Result<RulesSyncReport> {
        if let Some(home) = &request.home {
            self.check_adoption_home(home)?;
        }
        let session = RenderSession::new(&self.binary);
        self.adopt_using(
            &request.token,
            |target, source| self.render_rule(&session, target, source),
            |renderer, skills| self.render_skills(&session, renderer, skills),
        )
    }

    fn adopt_using(
        &self,
        token: &str,
        render_rule: impl Fn(&str, &str) -> Result<SkillFiles>,
        render_skills: impl Fn(&str, &SkillPackages) -> Result<SkillFiles>,
    ) -> Result<RulesSyncReport> {
        let _lock = self.lock()?;
        let manifest = self.adoptable_manifest()?;
        let mut adoption = self.discover_for_adoption(manifest.as_ref())?;
        let sources = adoption_token(&adoption)?;
        plan_skill_deliveries(&mut adoption, &render_skills)?;
        verify_adoption_token(&adoption, token)?;
        if adoption.files.is_empty() && adoption.overlays.is_empty() {
            let mut result = report(if manifest.is_some_and(|manifest| manifest.adopted) {
                "No additional tools found"
            } else {
                "No installed tool files to adopt"
            });
            append_skipped_tools(&mut result, &adoption.skipped_tools);
            return Ok(result);
        }
        let master = PersonalMaster::new(&adoption);
        for target in PERSONAL_TARGETS {
            let path = Path::new(target.rules.destination);
            let Some(AdoptionEntry::File(wanted)) =
                adoption.files.get(path).map(|file| &file.after)
            else {
                continue;
            };
            let output = render_rule(target.id, &master.rule_text(target.id, &[])?)?;
            let actual = output
                .get(path)
                .context("rulesync no longer renders the planned rules file")?;
            ensure!(
                same_rule_lines(actual, wanted)?,
                "rulesync changed the lines in {}; adoption needs a fresh preview",
                path.display()
            );
        }
        verify_adoption_token(&self.discover_for_adoption(manifest.as_ref())?, &sources)
            .context("the source changed during rendering")?;
        let root = FileRoot::open_home(&self.home)?;
        self.apply_adoption(&root, adoption, master)
    }

    fn apply_adoption(
        &self,
        root: &FileRoot,
        adoption: Adoption,
        master: PersonalMaster,
    ) -> Result<RulesSyncReport> {
        let mut operations = Vec::new();
        let rules: BTreeMap<_, _> = PERSONAL_TARGETS
            .iter()
            .map(|target| (Path::new(target.rules.destination), target.id))
            .collect();
        let links = adoption.links;
        for (path, change) in adoption.files {
            let linked = path
                .ancestors()
                .find_map(|ancestor| links.get(ancestor).map(|link| (ancestor, link)));
            let seen = if linked.is_some_and(|(ancestor, _)| ancestor != path.as_path()) {
                Snapshot::Absent
            } else {
                root.read(&path, true)?
            };
            ensure!(
                adoption_entry_matches(&change.before, &seen),
                "{} changed since the plan",
                path.display()
            );
            if matches!(seen, Snapshot::Symlink { .. }) {
                let link = links
                    .get(&path)
                    .context("missing resolved link in the adoption plan")?;
                ensure!(
                    link.matches(&self.home, &path)?,
                    "{} link source changed since the plan",
                    path.display()
                );
            }
            let desired = match change.after {
                AdoptionEntry::File(bytes) => match linked {
                    Some((ancestor, link)) => {
                        let mode = link
                            .file_mode(path.strip_prefix(ancestor)?)
                            .context("missing resolved file mode in the adoption plan")?;
                        Snapshot::file_with_mode(bytes, mode)
                    }
                    None => with_bytes(&seen, bytes),
                },
                AdoptionEntry::Directory => {
                    if let Some((ancestor, link)) = linked {
                        let mode = link
                            .directory_mode(path.strip_prefix(ancestor)?)
                            .context("missing resolved directory mode in the adoption plan")?;
                        Snapshot::directory_with_mode(mode)
                    } else {
                        seen.clone()
                    }
                }
                _ => bail!("unexpected adoption output at {}", path.display()),
            };
            let kind = match rules.get(path.as_path()) {
                Some(tool) => Kind::Rule {
                    target: (*tool).into(),
                    source: master.rule_text(tool, &[])?,
                    base: master.base_rule_text(tool, &[]),
                },
                None if desired.is_directory() => Kind::Directory,
                None => Kind::Skill,
            };
            operations.push(Pending {
                path,
                seen,
                desired,
                kind: Some(kind),
                allow_link: true,
            });
        }
        // Detach aliases before an operation changes their approved referents.
        // In a chain the most distant link must go first.
        operations.sort_by_key(|op| {
            std::cmp::Reverse(links.get(&op.path).map_or(0, |link| link.symlink_hops()))
        });
        let (mut manifest, extension) = if let Some(mut manifest) = self.adoptable_manifest()? {
            self.recover(root, &mut manifest)?;
            for op in &operations {
                ensure!(
                    !manifest.backup.contains_key(&op.path),
                    "{} already belongs to the local setup backup",
                    op.path.display()
                );
            }
            if manifest.settings_only {
                manifest.master = master;
                (manifest, None)
            } else {
                (manifest, Some(master))
            }
        } else {
            (Manifest::new(master, BTreeMap::new()), None)
        };
        let extending = extension.is_some();
        let mut skipped = adoption.skipped.into_keys().collect::<BTreeSet<_>>();
        let skipped_skills = !skipped.is_empty();
        let mut worktree_writes = adoption.worktree_writes;
        let mode = if extending {
            BatchMode::Extend
        } else {
            manifest.adopted = false;
            manifest.settings_only = false;
            manifest.unmanaged_skills.append(&mut skipped);
            manifest.worktree_writes.append(&mut worktree_writes);
            BatchMode::Adopt
        };
        let mut result = if adoption.added_tools.is_empty() {
            report("Adopted the local tool setup")
        } else {
            report(&format!(
                "Added {} to the local tool setup",
                adoption.added_tools.join(", ")
            ))
        };
        let outcomes = self
            .run_batch(root, &mut manifest, operations, mode, Some(&links))
            .with_context(|| {
                if extending {
                    "The tool addition stopped. Run `wenlan rules sync` to recover the addition"
                } else {
                    "Adoption stopped. Run `wenlan rules restore` to recover the setup"
                }
            })?;
        for (path, status) in outcomes {
            if status != ApplyOutcome::Applied {
                if extending {
                    bail!(
                        "{} changed during the tool addition. Run rules sync to recover the addition",
                        path.display()
                    );
                }
                bail!(
                    "{} changed during adoption. Run rules restore to roll back completed files",
                    path.display()
                );
            }
            result.written.push(path.display().to_string());
        }
        if let Some(master) = extension {
            extend_personal_master(&mut manifest.master, master);
            manifest.unmanaged_skills.append(&mut skipped);
            manifest.worktree_writes.append(&mut worktree_writes);
            manifest.extension_paths.clear();
            self.save(&manifest)?;
        }
        if skipped_skills {
            result
                .message
                .push_str(". Skill paths marked unmanaged in the plan stayed in place.");
        }
        append_skipped_tools(&mut result, &adoption.skipped_tools);
        Ok(result)
    }

    fn recover(&self, root: &FileRoot, manifest: &mut Manifest) -> Result<Option<RulesSyncReport>> {
        self.recover_pending(root, manifest)?;
        if manifest.extension_paths.is_empty() {
            return Ok(None);
        }
        let mut result =
            report("Rolled back the interrupted tool addition. Earlier tools are still managed");
        self.restore_deliveries(root, manifest, &mut result, true)?;
        let pending: BTreeSet<_> = manifest.pending.iter().map(|op| &op.path).collect();
        manifest.extension_paths.retain(|path| {
            if pending.contains(path) || manifest.delivered.contains_key(path) {
                true
            } else {
                manifest.backup.remove(path);
                #[cfg(windows)]
                manifest.windows_backup_owners.remove(path);
                false
            }
        });
        if !manifest.extension_paths.is_empty() {
            result.message =
                "Some files from the interrupted addition still need attention. Earlier tools are still managed".into();
        }
        self.save(manifest)?;
        Ok(Some(result))
    }

    fn recover_pending(&self, root: &FileRoot, manifest: &mut Manifest) -> Result<()> {
        if manifest.pending.is_empty() {
            return Ok(());
        }
        let data = self.data_files()?;
        let pending = std::mem::take(&mut manifest.pending);
        // Child entries are not reachable while a journaled directory link
        // is still in place. Decide them from the corresponding link state.
        let mut linked = BTreeMap::new();
        for op in &pending {
            let phase = match (&op.seen, &op.desired) {
                (Snapshot::Symlink { .. }, Snapshot::Directory { .. }) => Some((false, &op.seen)),
                (Snapshot::Directory { .. }, Snapshot::Symlink { .. }) => Some((true, &op.desired)),
                _ => None,
            };
            if let Some((restored, expected)) = phase {
                if root
                    .read(&op.path, true)
                    .is_ok_and(|actual| &actual == expected)
                {
                    linked.insert(op.path.clone(), restored);
                }
            }
        }
        let mut directories = BatchDirectories::default();
        manifest.recovery_issues.clear();
        for op in pending {
            let recovery = self.recover_entry(root, &data, manifest, &op, &linked);
            match recovery {
                Ok(completed) => {
                    manifest.stages.remove(&op.path);
                    if completed {
                        directories.record(&op);
                        settle(manifest, op)?;
                    }
                }
                Err(error) => {
                    manifest
                        .recovery_issues
                        .insert(op.path.clone(), format!("{error:#}"));
                    manifest.pending.push(op);
                }
            }
        }
        directories.sync(root)?;
        self.save(manifest)
    }

    fn recover_entry(
        &self,
        root: &FileRoot,
        data: &FileRoot,
        manifest: &Manifest,
        op: &Pending,
        linked: &BTreeMap<PathBuf, bool>,
    ) -> Result<bool> {
        if let Some(&restored) = op
            .path
            .ancestors()
            .skip(1)
            .find_map(|path| linked.get(path))
        {
            let expected = if restored { &op.desired } else { &op.seen };
            ensure!(
                *expected == Snapshot::Absent,
                "a directory link covers an entry in the backup. Check the link before retrying"
            );
            return Ok(restored);
        }
        let read_path = || {
            read_planned_path(
                root,
                &op.path,
                op.allow_link,
                op.seen == Snapshot::Absent || op.desired == Snapshot::Absent,
            )
        };
        let mut actual = read_path()?;
        if let Some(stage) = manifest.stages.get(&op.path) {
            root.recover_stage(
                &op.path,
                &op.seen,
                &op.desired,
                &actual,
                stage.options(data, op.allow_link, None),
            )?;
            if cfg!(windows) && actual == Snapshot::Absent {
                // Recovery may have put the file back at the vacant name.
                actual = read_path()?;
            }
        }
        ensure!(
            op.desired == actual || op.seen == actual,
            "changed during an interrupted write. Save your edit and compare it with the retained backup before retrying"
        );
        Ok(op.desired == actual)
    }

    fn apply_batch(
        &self,
        root: &FileRoot,
        manifest: &mut Manifest,
        operations: Vec<Pending>,
        mode: BatchMode,
    ) -> Result<Vec<(PathBuf, ApplyOutcome)>> {
        self.run_batch(root, manifest, operations, mode, None)
    }

    fn run_batch(
        &self,
        root: &FileRoot,
        manifest: &mut Manifest,
        operations: Vec<Pending>,
        mode: BatchMode,
        links: Option<&BTreeMap<PathBuf, wenlan_core::rulesync_adopt::AdoptionLink>>,
    ) -> Result<Vec<(PathBuf, ApplyOutcome)>> {
        let adopting = matches!(mode, BatchMode::Adopt);
        let extending = matches!(mode, BatchMode::Extend);
        let reviewing = matches!(mode, BatchMode::Review);
        let restoring = matches!(mode, BatchMode::Restore | BatchMode::SealRestore { .. });
        let stop_on_conflict = !restoring && !matches!(mode, BatchMode::Continue);
        if operations.is_empty() {
            if adopting {
                manifest.adopted = true;
                self.save(manifest)?;
            }
            return Ok(Vec::new());
        }
        // Skill approvals include their parents. Restore creates only the
        // directories explicitly recorded as removed by a managed delivery.
        let operations = if reviewing || restoring {
            operations
        } else {
            with_missing_parents(root, operations, extending.then_some(&manifest.backup))?
        };
        check_delivery_paths(&self.home, operations.iter().map(|op| op.path.as_path()))?;
        let mut outcomes = Vec::with_capacity(operations.len());
        let mut ready = Vec::with_capacity(operations.len());
        for op in operations {
            if (reviewing
                && !read_planned_path(
                    root,
                    &op.path,
                    op.allow_link,
                    op.seen == Snapshot::Absent || op.desired == Snapshot::Absent,
                )
                .is_ok_and(|actual| op.seen == actual))
                || manifest.pending.iter().any(|pending| {
                    op.path.starts_with(&pending.path) || pending.path.starts_with(&op.path)
                })
            {
                outcomes.push((op.path, ApplyOutcome::ChangedSincePlan));
            } else {
                ready.push(op);
            }
        }
        if ready.is_empty() || stop_on_conflict && !outcomes.is_empty() {
            return Ok(outcomes);
        }
        record_originals(root, manifest, &ready, adopting || extending)?;
        let data = self.data_files()?;
        let displaced = displaced::Batch::prepare(&data, &ready)?;
        let batch_paths = ready.iter().map(|op| op.path.clone()).collect::<Vec<_>>();
        manifest.stages.extend(
            ready
                .iter()
                .enumerate()
                .filter(|(_, op)| op.seen != op.desired)
                .map(|(index, op)| {
                    (
                        op.path.clone(),
                        PendingStage {
                            id: uuid::Uuid::new_v4().simple().to_string(),
                            displaced: displaced.as_ref().and_then(|batch| batch.location(index)),
                        },
                    )
                }),
        );
        if extending {
            manifest.extension_paths.extend(batch_paths.iter().cloned());
        }
        let first = manifest.pending.len();
        manifest.pending.append(&mut ready);
        self.save(manifest)?;
        let mut directories = BatchDirectories::default();
        let mut failed = BTreeSet::new();
        for operation in manifest.pending.split_off(first) {
            let path = operation.path.clone();
            // Keep the receipt if closing this directory would prevent a
            // later retry from restoring a descendant that is still managed.
            let unfinished_children = match (&mode, &operation.desired) {
                (BatchMode::SealRestore { extension_only }, Snapshot::Directory { mode })
                    if !mode.permits_child_writes() =>
                {
                    manifest
                        .delivered
                        .range::<PathBuf, _>((Excluded(&path), Unbounded))
                        .take_while(|(child, _)| child.starts_with(&path))
                        .any(|(child, _)| {
                            !extension_only || manifest.extension_paths.contains(child)
                        })
                }
                _ => false,
            };
            let wrong_spelling = restoring
                && operation.seen != operation.desired
                && !matches!(root.restore_leaf_alias(&path, |_| false), Ok(None));
            let status = if path_blocked(&failed, &path) || unfinished_children || wrong_spelling {
                ApplyOutcome::ChangedSincePlan
            } else {
                if let Some(stage) = manifest.stages.get(&path) {
                    let link = links.and_then(|links| links.get(&path));
                    let removal = operation.desired == Snapshot::Absent;
                    let verify = || {
                        if restoring {
                            return Ok(matches!(
                                root.restore_leaf_alias(&path, |_| false),
                                Ok(None)
                            ));
                        }
                        if removal && skill_case_alias(root, &path)?.is_some() {
                            return Ok(false);
                        }
                        link.map_or(Ok(true), |expected| expected.matches(&self.home, &path))
                            .map_err(std::io::Error::other)
                    };
                    let options = stage.options(
                        &data,
                        operation.allow_link,
                        (link.is_some() || removal || restoring).then_some(&verify as _),
                    );
                    #[cfg(windows)]
                    let options = ApplyOptions {
                        restore_owner: if restoring || operation.kind.is_none() {
                            manifest.windows_backup_owners.get(&path).map(Vec::as_slice)
                        } else {
                            None
                        },
                        ..options
                    };
                    root.apply_with(&path, &operation.seen, &operation.desired, options)
                        .with_context(|| format!("write {}", path.display()))?
                } else if operation.seen == Snapshot::Absent
                    && operation.desired == Snapshot::Absent
                {
                    if read_planned_path(root, &path, operation.allow_link, true)?
                        == Snapshot::Absent
                    {
                        ApplyOutcome::Applied
                    } else {
                        ApplyOutcome::ChangedSincePlan
                    }
                } else {
                    root.apply(
                        &path,
                        &operation.seen,
                        &operation.desired,
                        operation.allow_link,
                    )?
                }
            };
            outcomes.push((path.clone(), status));
            if status == ApplyOutcome::Applied {
                directories.record(&operation);
                settle(manifest, operation)?;
            } else {
                failed.insert(path);
                if stop_on_conflict {
                    break;
                }
            }
        }
        directories.sync(root)?;
        if let Some(batch) = displaced {
            batch.sync(&data)?;
        }
        if adopting {
            manifest.adopted = failed.is_empty();
        }
        for path in batch_paths {
            manifest.stages.remove(&path);
        }
        self.save(manifest)?;
        Ok(outcomes)
    }

    pub(crate) fn restore(&self) -> Result<RulesSyncReport> {
        let _lock = self.lock()?;
        let root = FileRoot::open_home(&self.home)?;
        let mut manifest = self.load()?.context("no local setup backup")?;
        if let Some(result) = self.recover(&root, &mut manifest)? {
            return Ok(result);
        }
        manifest.adopted = false;
        manifest.settings_batch = None;
        if !manifest.settings_only {
            crate::rulesync_checkout::refresh(
                self,
                &RenderSession::new(&self.binary),
                &wenlan_core::shared_space::settings_path(),
                None,
                &[],
            );
        }
        let mut result = report("Restored the managed files from the local setup backup");
        self.restore_deliveries(&root, &mut manifest, &mut result, false)?;
        if manifest.delivered.is_empty() && manifest.pending.is_empty() {
            if !manifest.settings_only {
                rulesync_registry::clear_delivery_state(&self.data)?;
            }
            self.retire_backup()?;
        } else {
            result.message =
                "Some paths could not be restored. The backup is still available".into();
        }
        Ok(result)
    }

    fn restore_deliveries(
        &self,
        root: &FileRoot,
        manifest: &mut Manifest,
        result: &mut RulesSyncReport,
        extension_only: bool,
    ) -> Result<()> {
        let selected = |path: &Path| !extension_only || manifest.extension_paths.contains(path);
        result.blocked.extend(
            manifest
                .recovery_issues
                .iter()
                .filter(|(path, _)| selected(path))
                .map(|(path, reason)| format!("{}: {reason}", path.display())),
        );
        let originals: Vec<_> = manifest
            .backup
            .keys()
            .filter(|path| selected(path))
            .cloned()
            .collect();
        ensure!(
            manifest
                .delivered
                .keys()
                .filter(|path| selected(path))
                .all(|path| manifest.backup.contains_key(path)),
            "the backup is incomplete"
        );
        let mut operations = Vec::new();
        for path in originals {
            if manifest.recovery_issues.contains_key(&path) {
                continue;
            }
            // A journaled write that did not land, or a path already restored
            // during recovery, has no active delivery to undo.
            let Some(delivered) = manifest.delivered.get(&path) else {
                continue;
            };
            if is_harness_path(&path) {
                result.blocked.push(format!(
                    "{}: {HARNESS_READ_ONLY} The retained backup is available for manual recovery.",
                    path.display()
                ));
                continue;
            }
            let original = manifest.backup[&path].clone();
            let expected = &delivered.delivered;
            let read_for_restore = || {
                let alias = root.restore_leaf_alias(&path, |parent| {
                    (original == Snapshot::Absent && *expected == Snapshot::Absent)
                        || (selected(parent)
                            && !manifest.recovery_issues.contains_key(parent)
                            && manifest
                                .backup
                                .get(parent)
                                .is_some_and(Snapshot::is_directory)
                            && manifest
                                .delivered
                                .get(parent)
                                .is_some_and(|entry| entry.delivered == Snapshot::Absent))
                })?;
                // A reviewed leaf rename can be moved back only after the
                // journaled alternate spelling has been removed.
                if alias.is_some() && *expected != Snapshot::Absent {
                    return Err(std::io::Error::other("the destination spelling changed"));
                }
                read_planned_path(root, &path, true, *expected == Snapshot::Absent)
            };
            let seen = match read_for_restore() {
                Ok(current) if extension_only && original == current => {
                    manifest.delivered.remove(&path);
                    continue;
                }
                Ok(current) if *expected == current => current,
                Err(error) => {
                    result
                        .blocked
                        .push(format!("{} could not be restored. {error}", path.display()));
                    continue;
                }
                _ => {
                    result
                        .blocked
                        .push(format!("{} changed since delivery", path.display()));
                    continue;
                }
            };
            #[cfg(windows)]
            if let Err(reason) = check_windows_restore(root, manifest, &path, &seen, &original) {
                result.blocked.push(format!(
                    "{} could not be restored. {reason}",
                    path.display()
                ));
                continue;
            }
            operations.push(Pending {
                path,
                seen,
                desired: original,
                kind: None,
                allow_link: true,
            });
        }
        // Descendants are restored before their parent's final mode or link.
        operations.sort_by_key(|op| std::cmp::Reverse(op.path.components().count()));
        let mut removing = BTreeSet::new();
        let mut retained = BTreeSet::new();
        for op in &mut operations {
            if op.desired != Snapshot::Absent {
                continue;
            }
            if op.seen.is_directory() {
                let children = root.children(&op.path)?;
                let unplanned = children.difference(&removing).collect::<Vec<_>>();
                if !unplanned.is_empty()
                    && unplanned.iter().all(|child| {
                        retained.contains(*child)
                            || !manifest
                                .delivered
                                .keys()
                                .any(|path| path.starts_with(child))
                    })
                {
                    // Relinquish the directory while keeping the person's
                    // unrecorded contents in place.
                    retained.insert(op.path.clone());
                    op.desired = op.seen.clone();
                    continue;
                }
            }
            removing.insert(op.path.clone());
        }
        // The alternate spelling must be out of the way before a path
        // can be restored under its original spelling.
        let mut blocked = BTreeSet::new();
        let mut moves = plan_case_moves(root, operations.iter(), |path, alias| {
            result.blocked.push(format!(
                "{} changed since delivery. The current spelling is {}.",
                path.display(),
                alias.display()
            ));
            blocked.insert(path.to_owned());
            Ok(())
        })?;
        operations.retain(|op| !path_blocked(&blocked, &op.path));
        moves.retain(|path, _| !path_blocked(&blocked, path));
        let move_out = take_case_sources(&moves, &mut operations);
        for (path, status) in self.apply_batch(root, manifest, move_out, BatchMode::Restore)? {
            if status == ApplyOutcome::Applied {
                result.written.push(path.display().to_string());
            } else {
                result
                    .blocked
                    .push(format!("{} changed during restore", path.display()));
                blocked.insert(path);
            }
        }
        operations.retain(|op| {
            !moves.get(&op.path).is_some_and(|from| {
                blocked
                    .iter()
                    .any(|path| from.starts_with(path) || path.starts_with(from))
            })
        });
        self.prepare_restore_directories(root, manifest, result, &mut operations)?;
        if !extension_only
            && operations.is_empty()
            && (!manifest.delivered.is_empty() || !manifest.pending.is_empty())
        {
            self.save(manifest)?;
        }
        let (directories, contents) = operations
            .into_iter()
            .partition(|op| op.desired.is_directory());
        for (operations, mode) in [
            (contents, BatchMode::Restore),
            (directories, BatchMode::SealRestore { extension_only }),
        ] {
            for (path, status) in self.apply_batch(root, manifest, operations, mode)? {
                if status == ApplyOutcome::Applied && !retained.contains(&path) {
                    result.written.push(path.display().to_string());
                } else if status != ApplyOutcome::Applied {
                    result
                        .blocked
                        .push(format!("{} could not be restored yet", path.display()));
                }
            }
        }
        Ok(())
    }

    fn prepare_restore_directories(
        &self,
        root: &FileRoot,
        manifest: &mut Manifest,
        result: &mut RulesSyncReport,
        operations: &mut Vec<Pending>,
    ) -> Result<()> {
        let parents = operations
            .iter()
            .filter(|op| op.seen != op.desired)
            .flat_map(|op| op.path.ancestors().skip(1))
            .collect::<BTreeSet<_>>();
        let modes: BTreeMap<_, _> = operations
            .iter()
            .filter(|op| op.desired.is_directory())
            .filter_map(|op| {
                let writable = match &op.seen {
                    Snapshot::Absent => Snapshot::private_directory(),
                    Snapshot::Directory { mode }
                        if !mode.permits_child_writes() && parents.contains(op.path.as_path()) =>
                    {
                        Snapshot::directory_with_permissions(mode.with_child_writes())
                    }
                    _ => return None,
                };
                Some((op.path.clone(), writable))
            })
            .collect();
        if modes.is_empty() {
            return Ok(());
        }
        // Keep an active receipt for each temporary mode. If restore is
        // interrupted, the original and this writable state are journaled.
        let mut prepare: Vec<_> = operations
            .iter()
            .filter_map(|op| {
                Some(Pending {
                    path: op.path.clone(),
                    seen: op.seen.clone(),
                    desired: modes.get(&op.path)?.clone(),
                    kind: Some(manifest.delivered.get(&op.path)?.kind.clone()),
                    allow_link: false,
                })
            })
            .collect();
        prepare.sort_by_key(|op| op.path.components().count());
        let mut blocked = BTreeSet::new();
        for (path, status) in self.apply_batch(root, manifest, prepare, BatchMode::Restore)? {
            if status != ApplyOutcome::Applied {
                result.blocked.push(format!(
                    "{} changed while preparing restore",
                    path.display()
                ));
                blocked.insert(path);
            }
        }
        operations.retain_mut(|op| {
            if path_blocked(&blocked, &op.path) {
                return false;
            }
            if let Some(mode) = modes.get(&op.path) {
                op.seen = mode.clone();
            }
            true
        });
        Ok(())
    }

    fn sync_batch(
        &self,
        root: &FileRoot,
        manifest: &mut Manifest,
        operations: Vec<Pending>,
        result: &mut RulesSyncReport,
    ) -> Result<Vec<(PathBuf, ApplyOutcome)>> {
        if operations.is_empty() {
            return Ok(Vec::new());
        }
        let mut worktrees = DeliveryWorktrees::new(&self.home)?;
        let mut approved = Vec::with_capacity(operations.len());
        for op in operations {
            if op.seen != op.desired {
                if let Some(reason) = worktrees.unreviewed(&op.path, &manifest.worktree_writes)? {
                    result
                        .blocked
                        .push(format!("{}: {reason}", op.path.display()));
                    continue;
                }
            }
            approved.push(op);
        }
        self.apply_batch(root, manifest, approved, BatchMode::Continue)
    }

    fn sync(&self, generated: &[String]) -> Result<RulesSyncReport> {
        let _lock = self.lock()?;
        let root = FileRoot::open_home(&self.home)?;
        let mut manifest = self.load()?.context("run wenlan rules adopt first")?;
        rulesync_registry::check_skill_format(&self.data)?;
        let recovered = self.recover(&root, &mut manifest)?;
        let mut result = self.sync_member(&root, &mut manifest, generated)?;
        if let Some(recovery) = recovered {
            result.message = format!("{}.\n{}", recovery.message, result.message);
            result.written.extend(recovery.written);
            result.blocked.extend(recovery.blocked);
            result.blocked.sort_unstable();
            result.blocked.dedup();
        }
        Ok(result)
    }

    fn sync_member(
        &self,
        root: &FileRoot,
        manifest: &mut Manifest,
        generated: &[String],
    ) -> Result<RulesSyncReport> {
        ensure!(manifest.adopted, "finish restoring the local setup first");
        let settings = wenlan_core::shared_space::settings_path();
        let layers = rulesync_team::load_layers(&settings)?;
        self.capture_team_edits(root, manifest, &settings, &layers)?;
        let session = RenderSession::new(&self.binary);
        crate::rulesync_checkout::refresh(
            self,
            &session,
            &settings,
            Some(&manifest.master),
            generated,
        );
        self.sync_locked(
            root,
            manifest,
            generated,
            |target, source| self.render_member_rule(&session, target, source, &layers),
            |renderer, skills| self.render_skills(&session, renderer, skills),
        )
    }

    pub(crate) fn checkout(
        &self,
        repo: &Path,
        generated: &[String],
    ) -> Result<wenlan_types::rulesync::CheckoutRulesReceipt> {
        let _lock = self.lock()?;
        let manifest = self
            .load()?
            .filter(|manifest| manifest.adopted)
            .context("run wenlan rules adopt first")?;
        crate::rulesync_checkout::open(self, repo, &manifest.master, generated)
    }

    fn capture_team_edits(
        &self,
        root: &FileRoot,
        manifest: &mut Manifest,
        settings: &Path,
        layers: &[TeamRuleLayer],
    ) -> Result<()> {
        let accessible: BTreeMap<_, _> = layers
            .iter()
            .map(|layer| Ok((rulesync_layers::key(layer)?, layer)))
            .collect::<Result<_>>()?;
        let keys: BTreeSet<_> = accessible.keys().cloned().collect();
        let paths: Vec<_> = manifest
            .active_deliveries()
            .filter_map(|(path, record)| {
                (matches!(record.kind, Kind::Rule { .. }) && !is_harness_path(path))
                    .then_some(path.clone())
            })
            .collect();
        let mut operations = Vec::new();
        for path in &paths {
            let Ok(current) = root.read(path, false) else {
                continue;
            };
            let Ok(desired) = without_revoked_rules(&current, &keys) else {
                continue;
            };
            if current != desired {
                operations.push(Pending {
                    path: path.clone(),
                    seen: current,
                    desired,
                    kind: Some(Kind::RuleRevocation {
                        accessible: keys.clone(),
                    }),
                    allow_link: false,
                });
            }
        }
        self.apply_batch(root, manifest, operations, BatchMode::Continue)?;

        let mut observed_edits = false;
        for path in paths {
            let record = &manifest.delivered[&path];
            let previous = record.observed.as_ref().unwrap_or(&record.delivered);
            let Ok(current) = root.read(&path, false) else {
                continue;
            };
            let (Ok(before), Ok(after)) = (text(previous), text(&current)) else {
                continue;
            };
            if previous != &current && !record.reviewed {
                let Ok(changes) = rulesync_layers::edits(before, after) else {
                    continue;
                };
                if !changes.is_empty() {
                    // Leave a mixed or stale edit in place for the member to separate.
                    if rulesync_layers::only_personal(before)?
                        != rulesync_layers::only_personal(after)?
                    {
                        continue;
                    }
                    for (key, change) in changes {
                        if let Some(layer) = accessible.get(key) {
                            rulesync_team::queue_proposal(
                                settings,
                                &layer.id,
                                change.revision,
                                &rulesync_layers::proposed_markdown(layer, &change)?,
                            )?;
                        }
                    }
                    manifest
                        .delivered
                        .get_mut(&path)
                        .expect("managed rule")
                        .observed = Some(current.clone());
                    observed_edits = true;
                }
            }
        }
        if observed_edits {
            self.save(manifest)?;
        }
        Ok(())
    }

    fn sync_locked(
        &self,
        root: &FileRoot,
        manifest: &mut Manifest,
        generated: &[String],
        render_rule: impl Fn(&str, &str) -> Result<BTreeMap<PathBuf, Vec<u8>>>,
        render_skills: impl Fn(&str, &SkillPackages) -> Result<SkillFiles>,
    ) -> Result<RulesSyncReport> {
        ensure!(
            manifest.adopted,
            "the previous adoption stopped early. Restore it first"
        );
        let mut result = report("Checked the local tool setup");
        let added = additional_targets(&self.home, &manifest.master)?;
        if !added.is_empty() && manifest.extension_paths.is_empty() {
            let tools = added
                .iter()
                .map(|target| target.tool.display_name())
                .collect::<Vec<_>>()
                .join(", ");
            result.message.push_str(&format!(
                "\nAdditional tools available: {tools}. Run `wenlan rules adopt` to review and add them."
            ));
        }
        let blocked = self.pull_back(root, manifest, &mut result)?;
        let had_issues = !std::mem::take(&mut manifest.delivery_issues).is_empty();
        let paths: Vec<_> = manifest
            .active_deliveries()
            .map(|(path, _)| path.clone())
            .collect();
        let mut operations = Vec::new();
        for path in paths {
            if blocked.contains(&path) {
                continue;
            }
            let delivery = &manifest.delivered[&path];
            let Kind::Rule { target, .. } = &delivery.kind else {
                continue;
            };
            let seen = delivery
                .observed
                .as_ref()
                .unwrap_or(&delivery.delivered)
                .clone();
            let source = match manifest.master.rule_text(target, generated) {
                Ok(text) => text,
                Err(error) => {
                    manifest
                        .delivery_issues
                        .insert(path.clone(), RuleDeliveryIssue::MasterConflict);
                    result.blocked.push(format!("{}: {error}", path.display()));
                    continue;
                }
            };
            let output = match render_rule(target, &source) {
                Ok(output) => output,
                Err(error) => {
                    manifest
                        .delivery_issues
                        .insert(path.clone(), RuleDeliveryIssue::RendererUnavailable);
                    result
                        .blocked
                        .push(format!("{}: {error:#}", path.display()));
                    continue;
                }
            };
            let Some(bytes) = output.get(&path) else {
                manifest
                    .delivery_issues
                    .insert(path.clone(), RuleDeliveryIssue::MissingOutput);
                result.blocked.push(format!(
                    "{}: rulesync did not render this file",
                    path.display()
                ));
                continue;
            };
            let desired = with_bytes(&seen, bytes.clone());
            if desired == seen && delivery.observed.is_none() {
                continue;
            }
            operations.push(Pending {
                path,
                seen,
                desired,
                kind: Some(Kind::Rule {
                    target: target.clone(),
                    source,
                    base: manifest.master.base_rule_text(target, generated),
                }),
                allow_link: false,
            });
        }
        for (path, status) in self.sync_batch(root, manifest, operations, &mut result)? {
            if status == ApplyOutcome::Applied {
                result.written.push(path.display().to_string());
            } else {
                manifest
                    .delivery_issues
                    .insert(path.clone(), RuleDeliveryIssue::ConcurrentEdit);
                result
                    .blocked
                    .push(format!("{} changed during sync", path.display()));
            }
        }
        if had_issues || !manifest.delivery_issues.is_empty() {
            self.save(manifest)?;
        }
        let saved = rulesync_registry::saved_skill_deliveries(&self.data)?;
        self.sync_skills(
            root,
            manifest,
            &blocked,
            &mut result,
            &saved,
            &render_skills,
        )?;
        self.sync_registry(root, manifest, &blocked, &mut result, &saved, render_skills)?;
        Ok(result)
    }

    fn pull_back(
        &self,
        root: &FileRoot,
        manifest: &mut Manifest,
        result: &mut RulesSyncReport,
    ) -> Result<BTreeSet<PathBuf>> {
        let skills = self.managed_skills(manifest);
        let mut unmanaged = manifest.unmanaged_skills.clone();
        unmanaged.extend(
            PERSONAL_TARGETS
                .iter()
                .filter(|target| !manifest.master.enrolls_target(target.id))
                .map(|target| PathBuf::from(target.skills.destination)),
        );
        let enrolled = PERSONAL_TARGETS
            .iter()
            .filter(|target| manifest.master.enrolls_target(target.id))
            .map(|target| target.tool)
            .collect();
        let pending_skills = rulesync_registry::collect_tool_proposals(
            &self.home, &self.data, &skills, &unmanaged, &enrolled,
        )?;
        let Pullback {
            drafts,
            cards,
            mut blocked,
        } = inspect(root, manifest)?;
        blocked.extend(pending_skills);
        if !drafts.is_empty() {
            manifest
                .scope_reviews
                .retain(|_, review| !drafts.iter().any(|draft| draft.path == review.path));
        }
        for draft in drafts
            .iter()
            .filter(|draft| !cards.iter().any(|card| card.paths.contains(&draft.path)))
        {
            let delivery = &manifest.delivered[&draft.path];
            let before = rulesync_layers::only_personal(text(
                delivery.observed.as_ref().unwrap_or(&delivery.delivered),
            )?)?;
            let after = rulesync_layers::only_personal(text(&draft.current)?)?;
            let id = fingerprint(&(&draft.path, &before, &after))?;
            accept_tool_edit(manifest, draft)?;
            manifest.scope_reviews.insert(
                id.clone(),
                ScopeReview {
                    path: draft.path.clone(),
                    before,
                    after,
                    card: PullbackCard {
                        id,
                        files: vec![draft.path.display().to_string()],
                        diff: draft.diff.clone(),
                        reason: "Kept for this tool. Share across tools?".into(),
                    },
                },
            );
        }
        if !drafts.is_empty() {
            self.save(manifest)?;
        }
        for card in cards {
            for path in card.paths {
                blocked.insert(path, "Rule changes need your approval. Use `wenlan rules review` with the proposal below.".into());
            }
            result.proposals.push(card.wire);
        }
        result.proposals.extend(
            manifest
                .scope_reviews
                .values()
                .map(|review| review.card.clone()),
        );
        result.blocked.extend(
            blocked
                .iter()
                .map(|(path, reason)| format!("{}: {reason}", path.display())),
        );
        Ok(blocked.into_keys().collect())
    }

    fn sync_registry(
        &self,
        root: &FileRoot,
        manifest: &mut Manifest,
        blocked: &BTreeSet<PathBuf>,
        result: &mut RulesSyncReport,
        saved: &SavedSkillDeliveries,
        render: impl Fn(&str, &SkillPackages) -> Result<SkillFiles>,
    ) -> Result<()> {
        let plan = self.plan_registry(
            root,
            manifest,
            blocked,
            result,
            RegistryInputs::Sync(saved),
            render,
        )?;
        self.apply_registry(root, manifest, result, plan)
    }

    pub(crate) fn plan_skill_review(
        &self,
        reviewed: &BTreeMap<SavedSkillTool, ApprovedSkills>,
        approved: &BTreeMap<PathBuf, ManagedSkill>,
        explicit_file_modes: &BTreeSet<PathBuf>,
        render: impl Fn(&str, &SkillPackages) -> Result<SkillFiles>,
    ) -> Result<SkillReviewDelivery> {
        let mut report = report("Checked the approved skill's delivery");
        let Some(mut manifest) = self.load()?.filter(|manifest| manifest.adopted) else {
            report
                .blocked
                .push("Run `wenlan rules adopt` before accepting delivery of this skill.".into());
            return Ok(SkillReviewDelivery {
                delivery: None,
                report,
                tools: BTreeSet::new(),
            });
        };
        ensure!(
            manifest.pending.is_empty() && manifest.extension_paths.is_empty(),
            "Run `wenlan rules sync` to recover the pending tool setup before reviewing this skill"
        );
        let tools: BTreeSet<_> = PERSONAL_TARGETS
            .iter()
            .filter(|target| {
                manifest.master.enrolls_target(target.id) && reviewed.contains_key(&target.tool)
            })
            .map(|target| target.tool)
            .collect();
        if tools.is_empty() {
            report
                .blocked
                .push("Run `wenlan rules adopt` to enroll a destination for this skill.".into());
        }
        let root = FileRoot::open_home(&self.home)?;
        let inputs = RegistryInputs::Review {
            approved,
            sources: reviewed,
            explicit_file_modes,
        };
        let plan = self.plan_registry(
            &root,
            &mut manifest,
            &BTreeSet::new(),
            &mut report,
            inputs,
            render,
        )?;
        Ok(SkillReviewDelivery {
            delivery: Some(ReviewedRegistryPlan {
                root,
                manifest,
                plan,
            }),
            report,
            tools,
        })
    }

    pub(crate) fn apply_skill_review(
        &self,
        mut review: SkillReviewDelivery,
    ) -> Result<RulesSyncReport> {
        ensure!(
            review.report.blocked.is_empty(),
            "Some tools could not be included in this skill approval. Resolve the blocked paths and review it again."
        );
        if let Some(ReviewedRegistryPlan {
            root,
            mut manifest,
            plan,
        }) = review.delivery
        {
            let mut worktrees = DeliveryWorktrees::new(&self.home)?;
            for op in plan.operations().filter(|op| op.seen != op.desired) {
                if let Some(reason) = worktrees.unreviewed(&op.path, &manifest.worktree_writes)? {
                    bail!("{}: {reason}", op.path.display());
                }
            }
            let changed: BTreeSet<_> = plan
                .operations()
                .filter(|op| op.seen != op.desired)
                .map(|op| op.path.clone())
                .collect();
            let operations = plan
                .move_out
                .into_iter()
                .chain(plan.writes)
                .chain(plan.removals)
                .collect();
            for (path, status) in
                self.apply_batch(&root, &mut manifest, operations, BatchMode::Review)?
            {
                ensure!(
                    status == ApplyOutcome::Applied,
                    "{} changed during skill delivery. Run `wenlan rules sync` and review a fresh plan.",
                    path.display()
                );
                if changed.contains(&path) {
                    review.report.written.push(path.display().to_string());
                }
            }
        }
        Ok(review.report)
    }

    fn plan_registry(
        &self,
        root: &FileRoot,
        manifest: &mut Manifest,
        blocked: &BTreeSet<PathBuf>,
        result: &mut RulesSyncReport,
        inputs: RegistryInputs<'_>,
        render: impl Fn(&str, &SkillPackages) -> Result<SkillFiles>,
    ) -> Result<RegistryPlan> {
        let (installed, reviewed, approved, explicit_file_modes) = match inputs {
            RegistryInputs::Sync(saved) => (
                rulesync_registry::installed_sources()?,
                None,
                &saved.folders,
                None,
            ),
            RegistryInputs::Review {
                sources,
                approved,
                explicit_file_modes,
            } => (
                BTreeMap::new(),
                Some(sources),
                approved,
                Some(explicit_file_modes),
            ),
        };
        let reviewing = reviewed.is_some();
        let scanner = SkillHome::open(&self.home)?;
        let adopted = self.managed_skills(manifest);
        let mut active = installed.keys().cloned().collect::<BTreeSet<_>>();
        if let RegistryInputs::Sync(saved) = inputs {
            active.extend(saved.names.values().flatten().cloned());
        }
        let mut expected = BTreeMap::new();
        let mut targets: BTreeSet<_> = manifest
            .active_deliveries()
            .filter_map(|(_, delivery)| match &delivery.kind {
                Kind::Rule { target, .. } => Some(target.clone()),
                _ => None,
            })
            .collect();
        targets.extend(manifest.master.overlays.keys().cloned());
        targets.extend(
            manifest
                .master
                .skills
                .iter()
                .flat_map(|skill| skill.targets.iter().cloned()),
        );
        let previous_skips = if reviewing {
            manifest.registry_skipped.clone()
        } else {
            std::mem::take(&mut manifest.registry_skipped)
        };
        let mut worktrees = DeliveryWorktrees::new(&self.home)?;
        let mut writes = Vec::new();
        for id in targets {
            let target = personal_target(&id).context("unknown personal skill target")?;
            let local = reviewed.and_then(|sources| sources.get(&target.tool));
            let sources: BTreeMap<_, _> = match inputs {
                RegistryInputs::Sync(saved) => installed
                    .iter()
                    .filter(|(name, _)| !saved.names[&target.tool].contains(*name))
                    .collect(),
                RegistryInputs::Review { .. } => {
                    let Some(local) = local else {
                        continue;
                    };
                    local.files.iter().collect()
                }
            };
            if sources.is_empty() {
                continue;
            }
            match root.read(target.folder(), false) {
                Ok(Snapshot::Directory { .. }) => {}
                #[cfg(windows)]
                Err(error) if error.kind() == std::io::ErrorKind::Unsupported => {
                    result
                        .blocked
                        .push(format!("{}: {error}", target.folder().display()));
                    continue;
                }
                _ => {
                    if reviewing {
                        result.blocked.push(format!(
                            "{}: The enrolled tool folder is unavailable.",
                            target.folder().display()
                        ));
                    }
                    continue;
                }
            }
            let deliveries = match render_personal_skills(
                target,
                sources.iter().map(|(name, files)| (name.as_str(), *files)),
                &render,
            ) {
                Ok(deliveries) => deliveries,
                Err(error) if reviewing => {
                    return Err(error.context(format!("stage skills for {id}")))
                }
                Err(error) => {
                    result
                        .blocked
                        .push(format!("{id} installed skills: {error:#}"));
                    continue;
                }
            };
            for (name, delivery) in sources.keys().copied().zip(deliveries) {
                let emitted = Path::new(target.skills.staged).join(name);
                let default_folder = personal_skill_destination(target.skills, &emitted)?;
                let matches = |path: &PathBuf, skill: &ManagedSkill| {
                    path.file_name() == emitted.file_name() && skill.tool == target.tool
                };
                let selected = approved.iter().find(|(path, skill)| matches(path, skill));
                let adopted_folder = local
                    .is_some_and(|local| local.files.contains_key(name))
                    .then(|| adopted.iter().find(|(path, skill)| matches(path, skill)))
                    .flatten();
                let folder =
                    if emitted == default_folder && manifest.delivered.contains_key(&emitted) {
                        emitted.as_path()
                    } else {
                        selected
                            .or(adopted_folder)
                            .map_or(default_folder.as_path(), |(path, _)| path.as_path())
                    };
                if path_blocked(blocked, folder) {
                    continue;
                }
                if path_blocked(&manifest.unmanaged_skills, folder) {
                    if reviewing {
                        result
                            .blocked
                            .push(format!("{}: This skill is unmanaged.", folder.display()));
                    }
                    continue;
                }
                if let Some(reason) = worktrees.unreviewed(folder, &manifest.worktree_writes)? {
                    result
                        .blocked
                        .push(format!("{}: {reason}", folder.display()));
                    manifest
                        .registry_skipped
                        .entry(name.clone())
                        .or_default()
                        .insert(folder.to_owned());
                    continue;
                }
                if folder == default_folder
                    && is_harness_path(&emitted)
                    && !manifest.delivered.contains_key(folder)
                    && !matches!(
                        FileRoot::open(&self.home)?.read(&emitted, true),
                        Ok(Snapshot::Absent)
                    )
                {
                    result.blocked.push(format!(
                        "{}: Codex can already read a skill at {}. Choose another name to avoid a duplicate.",
                        folder.display(), emitted.display()
                    ));
                    manifest
                        .registry_skipped
                        .entry(name.clone())
                        .or_default()
                        .insert(folder.to_owned());
                    continue;
                }
                let baseline = adopted.get(folder);
                let approved = approved.get(folder);
                let current = match scanner.checked_package(folder, target.tool, baseline, approved)
                {
                    Ok(current) => current,
                    Err(error) => {
                        result
                            .blocked
                            .push(format!("{}: {error:#}", folder.display()));
                        continue;
                    }
                };
                if baseline.is_none()
                    && !approved.is_some_and(|approved| {
                        current.as_ref().is_some_and(|actual| approved == actual)
                    })
                    && root.read(folder, false)? != Snapshot::Absent
                {
                    if reviewing {
                        result.blocked.push(format!("{}: This skill folder already exists. Review its local contents first.", folder.display()));
                    }
                    manifest
                        .registry_skipped
                        .entry(name.clone())
                        .or_default()
                        .insert(folder.to_owned());
                    continue;
                }
                let package = RegistryPackage {
                    folder,
                    name,
                    files: &delivery.files,
                    modes: local.and_then(|local| local.modes.get(name)),
                    snapshot: current.as_ref(),
                    explicit_file_modes,
                };
                let planned =
                    plan_registry_entries(root, manifest, blocked, result, package, reviewing);
                match planned {
                    Ok((entries, wanted)) => {
                        writes.extend(entries);
                        expected.insert(
                            folder.to_owned(),
                            ExpectedRegistrySkill {
                                name: name.clone(),
                                wanted,
                                snapshot: current,
                            },
                        );
                        if reviewing {
                            if let Some(paths) = manifest.registry_skipped.get_mut(name) {
                                paths.remove(folder);
                            }
                        }
                    }
                    Err(error) => result
                        .blocked
                        .push(format!("{}: {error:#}", folder.display())),
                }
            }
        }
        let mut removals = registry_removals(
            root, manifest, &expected, &active, blocked, result, reviewing,
        )?;
        if reviewing {
            writes = with_missing_parents(root, writes, None)?;
            check_delivery_paths(
                &self.home,
                writes.iter().chain(&removals).map(|op| op.path.as_path()),
            )?;
            for op in writes
                .iter()
                .chain(&removals)
                .filter(|op| op.seen != op.desired)
            {
                if let Some(reason) = worktrees.unreviewed(&op.path, &manifest.worktree_writes)? {
                    result
                        .blocked
                        .push(format!("{}: {reason}", op.path.display()));
                }
            }
        }
        manifest
            .registry_skipped
            .retain(|_, paths| !paths.is_empty());
        let moves = plan_case_moves(root, writes.iter().chain(&removals), |path, alias| {
            bail!(
                "{} already refers to {}. Review the existing path before changing its spelling.",
                path.display(),
                alias.display()
            )
        })?;
        let move_out = take_case_sources(&moves, &mut removals);
        Ok(RegistryPlan {
            moves,
            move_out,
            writes,
            removals,
            packages: expected.into_keys().collect(),
            previous_skips,
        })
    }

    fn apply_registry(
        &self,
        root: &FileRoot,
        manifest: &mut Manifest,
        result: &mut RulesSyncReport,
        mut plan: RegistryPlan,
    ) -> Result<()> {
        let written: BTreeSet<_> = plan
            .writes
            .iter()
            .filter(|op| op.seen != op.desired && matches!(op.desired, Snapshot::File { .. }))
            .map(|op| op.path.clone())
            .collect();
        let blocked_before = result.blocked.len();
        let mut failed = BTreeSet::new();
        for (path, status) in self.sync_batch(root, manifest, plan.move_out, result)? {
            if status == ApplyOutcome::Applied {
                result.written.push(path.display().to_string());
            } else {
                result
                    .blocked
                    .push(format!("{} changed during sync", path.display()));
            }
        }
        let outcomes = self.sync_batch(root, manifest, plan.writes, result)?;
        if blocked_before != result.blocked.len() {
            failed.extend(plan.packages.iter().cloned());
        }
        for (path, status) in outcomes {
            if status == ApplyOutcome::Applied {
                if written.contains(&path) {
                    result.written.push(path.display().to_string());
                }
            } else {
                failed.extend(
                    path.ancestors()
                        .find(|folder| plan.packages.contains(*folder))
                        .map(Path::to_owned),
                );
                result
                    .blocked
                    .push(format!("{} changed during sync", path.display()));
            }
        }
        plan.removals.retain(|op| !path_blocked(&failed, &op.path));
        for (path, status) in self.sync_batch(root, manifest, plan.removals, result)? {
            if status != ApplyOutcome::Applied {
                result
                    .blocked
                    .push(format!("{} changed since delivery", path.display()));
            }
        }
        if manifest.registry_skipped != plan.previous_skips {
            self.save(manifest)?;
        }
        Ok(())
    }

    fn sync_skills(
        &self,
        root: &FileRoot,
        manifest: &mut Manifest,
        blocked: &BTreeSet<PathBuf>,
        result: &mut RulesSyncReport,
        saved: &SavedSkillDeliveries,
        render: &impl Fn(&str, &SkillPackages) -> Result<SkillFiles>,
    ) -> Result<()> {
        let mut by_target: BTreeMap<String, Vec<SkillPath>> = BTreeMap::new();
        for (path, delivery) in manifest.active_deliveries() {
            if !matches!(delivery.kind, Kind::Skill)
                || path_blocked(blocked, path)
                || path
                    .ancestors()
                    .any(|folder| saved.folders.contains_key(folder))
            {
                continue;
            }
            let Some(spec) = skill_path(&manifest.master.skills, path) else {
                result
                    .blocked
                    .push(format!("{} is missing from the master", path.display()));
                continue;
            };
            let tool = personal_target(&spec.target)
                .context("unknown personal skill target")?
                .tool;
            if saved.names[&tool].contains(&manifest.master.skills[spec.index].name) {
                continue;
            }
            by_target.entry(spec.target.clone()).or_default().push(spec);
        }
        let mut operations = Vec::new();
        for (id, paths) in by_target {
            let target = personal_target(&id).context("unknown personal skill target")?;
            let indices: BTreeSet<_> = paths.iter().map(|path| path.index).collect();
            let skills = indices.iter().map(|&index| {
                let skill = &manifest.master.skills[index];
                (skill.name.as_str(), &skill.files)
            });
            let deliveries = match render_personal_skills(target, skills, render) {
                Ok(deliveries) => deliveries,
                Err(error) => {
                    result.blocked.push(format!("{id} skills: {error:#}"));
                    continue;
                }
            };
            let outputs: BTreeMap<_, _> = indices.into_iter().zip(deliveries).collect();
            for spec in paths {
                let delivery = &manifest.delivered[&spec.installed];
                let seen = delivery.observed.as_ref().unwrap_or(&delivery.delivered);
                let desired = outputs[&spec.index]
                    .files
                    .get(&spec.relative)
                    .map_or(Snapshot::Absent, |bytes| with_bytes(seen, bytes.clone()));
                if desired == *seen && delivery.observed.is_none() {
                    continue;
                }
                operations.push(Pending {
                    path: spec.installed,
                    seen: seen.clone(),
                    desired,
                    kind: Some(Kind::Skill),
                    allow_link: false,
                });
            }
        }
        for (path, status) in self.sync_batch(root, manifest, operations, result)? {
            if status == ApplyOutcome::Applied {
                result.written.push(path.display().to_string());
            } else {
                result
                    .blocked
                    .push(format!("{} changed during sync", path.display()));
            }
        }
        Ok(())
    }

    fn managed_skills(&self, manifest: &Manifest) -> BTreeMap<PathBuf, ManagedSkill> {
        let mut packages = BTreeMap::new();
        for adopted in &manifest.master.skills {
            for (target, folders) in &adopted.locations {
                if let Some(spec) = personal_target(target) {
                    packages.extend(folders.iter().filter(|path| !is_harness_path(path)).map(
                        |folder| {
                            (
                                folder.clone(),
                                ManagedSkill {
                                    tool: spec.tool,
                                    files: BTreeMap::new(),
                                    directories: BTreeMap::new(),
                                },
                            )
                        },
                    ));
                }
            }
        }
        for (path, record) in manifest.active_deliveries() {
            if matches!(record.kind, Kind::RegistryDirectory { .. }) {
                if let Some(target) = PERSONAL_TARGETS
                    .iter()
                    .find(|target| path.starts_with(target.skills.destination))
                {
                    packages.entry(path.clone()).or_insert(ManagedSkill {
                        tool: target.tool,
                        files: BTreeMap::new(),
                        directories: BTreeMap::new(),
                    });
                }
            }
        }
        for (folder, skill) in &mut packages {
            for (path, record) in manifest.active_deliveries() {
                if let Ok(relative) = path.strip_prefix(folder) {
                    let snapshot = record.observed.as_ref().unwrap_or(&record.delivered);
                    if let Snapshot::Directory { mode } = snapshot {
                        skill.directories.insert(relative.to_owned(), *mode);
                    }
                    if matches!(record.kind, Kind::Skill | Kind::RegistrySkill { .. })
                        && matches!(snapshot, Snapshot::File { .. })
                    {
                        skill.files.insert(relative.to_owned(), snapshot.clone());
                    }
                }
            }
        }
        packages
    }

    fn review(&self, request: &PullbackDecision, generated: &[String]) -> Result<RulesSyncReport> {
        self.review_using(request, |root, manifest| {
            self.sync_member(root, manifest, generated)
        })
    }

    fn review_using(
        &self,
        request: &PullbackDecision,
        sync: impl FnOnce(&FileRoot, &mut Manifest) -> Result<RulesSyncReport>,
    ) -> Result<RulesSyncReport> {
        let _lock = self.lock()?;
        let root = FileRoot::open_home(&self.home)?;
        let mut manifest = self.load()?.context("run wenlan rules adopt first")?;
        rulesync_registry::check_skill_format(&self.data)?;
        self.recover(&root, &mut manifest)?;
        if let Some(scope) = manifest.scope_reviews.remove(&request.id) {
            if matches!(request.action, PullbackAction::AllTools) {
                let delivery = manifest
                    .delivered
                    .get_mut(&scope.path)
                    .context("file is no longer managed")?;
                let expected = delivery.observed.as_ref().unwrap_or(&delivery.delivered);
                ensure!(
                    root.read(&scope.path, false)? == *expected,
                    "the tool edited this file again. Run rules sync"
                );
                let Kind::Rule {
                    target,
                    base,
                    source,
                } = &delivery.kind
                else {
                    bail!("the proposal no longer belongs to a managed rule");
                };
                if manifest
                    .master
                    .promote_tool_edit(target, &scope.before, &scope.after)?
                {
                    // Pull-back already kept this edit locally. Remove that
                    // copy from the override now that the shared master has it.
                    let local = current_rule_source(&manifest.master, target, base, source)?;
                    let previous = merge_rule_text(&scope.after, &scope.before, &local)?;
                    if previous == *base {
                        manifest.master.overrides.remove(target);
                    } else {
                        manifest.master.overrides.insert(
                            target.clone(),
                            LocalText {
                                base: base.clone(),
                                edited: previous,
                            },
                        );
                    }
                }
                delivery.observed = Some(expected.clone());
                delivery.reviewed = true;
            }
            self.save(&manifest)?;
            return sync(&root, &mut manifest);
        }
        let Pullback { drafts, cards, .. } = inspect(&root, &manifest)?;
        let card = cards
            .iter()
            .find(|card| card.wire.id == request.id)
            .context("the proposal is no longer current. Run rules sync")?;
        let path = match (&request.file, card.paths.len(), &request.action) {
            (_, _, PullbackAction::Dismiss) => None,
            (Some(file), _, _) => {
                let path = Path::new(file);
                ensure!(
                    card.paths.contains(path),
                    "this file does not belong to the proposal"
                );
                Some(path)
            }
            (None, 1, _) => card.paths.first().map(PathBuf::as_path),
            (None, _, _) => bail!("choose a file to resolve the conflicting edits"),
        };
        let reviewed = || {
            drafts
                .iter()
                .filter(|draft| card.paths.contains(&draft.path))
        };
        let all_tools = request.action == PullbackAction::AllTools;
        if all_tools {
            for draft in reviewed() {
                ensure!(
                    root.read(&draft.path, false)? == draft.current,
                    "the tool edited {} again. Run rules sync",
                    draft.path.display()
                );
            }
        }
        for draft in reviewed() {
            let selected = path == Some(draft.path.as_path());
            match request.action {
                PullbackAction::ToolOnly => {
                    if selected {
                        accept_tool_edit(&mut manifest, draft)?;
                    }
                    continue;
                }
                PullbackAction::AllTools if selected => {
                    let delivery = &manifest.delivered[&draft.path];
                    let Kind::Rule { target, .. } = &delivery.kind else {
                        bail!("the proposal no longer belongs to a managed rule");
                    };
                    let before = rulesync_layers::only_personal(text(
                        delivery.observed.as_ref().unwrap_or(&delivery.delivered),
                    )?)?;
                    let after = rulesync_layers::only_personal(text(&draft.current)?)?;
                    manifest.master.promote_tool_edit(target, &before, &after)?;
                }
                PullbackAction::AllTools | PullbackAction::Dismiss => {}
            }
            // Use the exact reviewed version as the expected input for each
            // delivery, including the alternatives the person did not choose.
            let delivery = manifest
                .delivered
                .get_mut(&draft.path)
                .context("file is no longer managed")?;
            delivery.observed = Some(draft.current.clone());
            delivery.reviewed |= all_tools;
        }
        self.save(&manifest)?;
        sync(&root, &mut manifest)
    }

    fn render_rule(
        &self,
        session: &RenderSession<'_>,
        target: &str,
        source: &str,
    ) -> Result<SkillFiles> {
        let rules = personal_target(target)
            .context("unknown personal rule target")?
            .rules;
        let rendered = self.render_tree(session, rules.renderer, &["rules"], |input| {
            write_root_rule(input, source)
        })?;
        personal_rule_destination(rules, rendered)
    }

    pub(crate) fn render_skills(
        &self,
        session: &RenderSession<'_>,
        renderer: &str,
        skills: &SkillPackages,
    ) -> Result<SkillFiles> {
        self.render_tree(session, renderer, &["skills"], |input| {
            for (name, files) in skills {
                write_skill_files(input, name, files)?;
            }
            Ok(())
        })
    }

    fn render_member_rule(
        &self,
        session: &RenderSession<'_>,
        target: &str,
        source: &str,
        layers: &[TeamRuleLayer],
    ) -> Result<BTreeMap<PathBuf, Vec<u8>>> {
        if layers.is_empty() {
            return self.render_rule(session, target, source);
        }
        let rules = personal_target(target)
            .context("unknown personal rule target")?
            .rules;
        let stage = rulesync_renderer::new_stage(&self.data)?;
        let team = stage.path().join("team");
        let personal = stage.path().join("personal");
        let output = stage.path().join("output");
        fs::create_dir(&output)?;
        rulesync_layers::stage(&team, "Team", layers)?;
        write_root_rule(&personal, source)?;
        session.render(
            stage.path(),
            &[team, personal],
            &output,
            true,
            &[rules.renderer.into()],
            &["rules"],
        )?;
        let mut rendered = BTreeMap::new();
        read_stage(&output, &output, &mut rendered)?;
        let rendered = personal_rule_destination(rules, rendered)?;
        let expected = layers
            .iter()
            .map(rulesync_layers::key)
            .collect::<Result<BTreeSet<_>>>()?;
        for bytes in rendered.values() {
            let blocks = rulesync_layers::blocks(std::str::from_utf8(bytes)?)?;
            ensure!(
                blocks
                    .keys()
                    .map(|key| (*key).to_owned())
                    .collect::<BTreeSet<_>>()
                    == expected,
                "rulesync did not preserve the authorized team layers"
            );
        }
        Ok(rendered)
    }

    fn render_tree(
        &self,
        session: &RenderSession<'_>,
        target: &str,
        features: &[&str],
        stage_input: impl FnOnce(&Path) -> Result<()>,
    ) -> Result<BTreeMap<PathBuf, Vec<u8>>> {
        // Preview may run before the rulesync data directory has been created.
        let parent = self.data.parent().context("rulesync has no data parent")?;
        HarnessGuard::new(&self.home)?.check_absolute(parent)?;
        let stage = rulesync_renderer::new_stage(parent)?;
        let input = stage.path().join("master");
        let output = stage.path().join("output");
        fs::create_dir(&input)?;
        fs::create_dir(&output)?;
        stage_input(&input)?;
        session.render(
            stage.path(),
            &[input],
            &output,
            true,
            &[target.into()],
            features,
        )?;
        let mut rendered = BTreeMap::new();
        read_stage(&output, &output, &mut rendered)?;
        Ok(rendered)
    }
}

struct SkillPath {
    index: usize,
    target: String,
    installed: PathBuf,
    relative: PathBuf,
}

struct RegistryPackage<'a> {
    folder: &'a Path,
    name: &'a str,
    files: &'a SkillFiles,
    modes: Option<&'a SkillModes>,
    snapshot: Option<&'a ManagedSkill>,
    explicit_file_modes: Option<&'a BTreeSet<PathBuf>>,
}

fn skill_case_alias(root: &FileRoot, path: &Path) -> std::io::Result<Option<PathBuf>> {
    let scope = PERSONAL_TARGETS
        .iter()
        .map(|tool| Path::new(tool.skills.destination))
        .find(|scope| path != *scope && path.starts_with(scope));
    match scope {
        Some(scope) => root.case_alias(path, scope),
        None => Ok(None),
    }
}

fn read_planned_path(
    root: &FileRoot,
    path: &Path,
    allow_link: bool,
    expects_absence: bool,
) -> std::io::Result<Snapshot> {
    let actual = root.read(path, allow_link)?;
    if expects_absence && actual != Snapshot::Absent && skill_case_alias(root, path)?.is_some() {
        Ok(Snapshot::Absent)
    } else {
        Ok(actual)
    }
}

fn package_entry(
    root: &FileRoot,
    folder: &Path,
    path: &Path,
    package: Option<&ManagedSkill>,
) -> Result<Snapshot> {
    let Some(package) = package else {
        return Ok(root.read(path, false)?);
    };
    let relative = path.strip_prefix(folder)?;
    let expected = package
        .files
        .get(relative)
        .cloned()
        .or_else(|| {
            package
                .directories
                .get(relative)
                .copied()
                .map(Snapshot::directory_with_permissions)
        })
        .unwrap_or(Snapshot::Absent);
    let actual = read_planned_path(root, path, false, expected == Snapshot::Absent)?;
    ensure!(
        actual == expected,
        "{} changed during planning",
        path.display()
    );
    Ok(actual)
}

/// Match each new spelling against the entry the destination filesystem
/// opens. Move only entries that are already part of the reviewed plan.
fn plan_case_moves<'a>(
    root: &FileRoot,
    operations: impl Iterator<Item = &'a Pending> + Clone,
    mut conflict: impl FnMut(&Path, &Path) -> Result<()>,
) -> Result<BTreeMap<PathBuf, PathBuf>> {
    let mut removals = operations
        .clone()
        .filter(|op| op.seen != Snapshot::Absent && op.desired == Snapshot::Absent)
        .map(|op| op.path.as_path())
        .collect::<BTreeSet<_>>();
    let mut moves = BTreeMap::new();
    for op in operations.filter(|op| op.seen == Snapshot::Absent && op.desired != Snapshot::Absent)
    {
        if let Some(alias) = skill_case_alias(root, &op.path)? {
            if removals.remove(alias.as_path()) {
                moves.insert(op.path.clone(), alias);
            } else {
                conflict(&op.path, &alias)?;
            }
        }
    }
    Ok(moves)
}

fn take_case_sources(
    moves: &BTreeMap<PathBuf, PathBuf>,
    operations: &mut Vec<Pending>,
) -> Vec<Pending> {
    if moves.is_empty() {
        return Vec::new();
    }
    let sources = moves
        .values()
        .map(PathBuf::as_path)
        .collect::<BTreeSet<_>>();
    let (early, rest) = std::mem::take(operations).into_iter().partition(|op| {
        op.seen != Snapshot::Absent
            && op.desired == Snapshot::Absent
            && op.path.ancestors().any(|path| sources.contains(path))
    });
    *operations = rest;
    early
}

fn plan_registry_entries(
    root: &FileRoot,
    manifest: &Manifest,
    blocked: &BTreeSet<PathBuf>,
    result: &mut RulesSyncReport,
    package: RegistryPackage<'_>,
    reviewing: bool,
) -> Result<(Vec<Pending>, BTreeSet<PathBuf>)> {
    let RegistryPackage {
        folder,
        name,
        files,
        modes,
        snapshot,
        explicit_file_modes,
    } = package;
    let mut directories = BTreeMap::from([(folder.to_owned(), None)]);
    if let Some(modes) = modes {
        directories.extend(
            modes
                .directories
                .iter()
                .map(|(path, mode)| (folder.join(path), Some(*mode))),
        );
    }
    for relative in files.keys() {
        let path = folder.join(relative);
        for parent in path
            .ancestors()
            .skip(1)
            .take_while(|parent| *parent != folder)
        {
            directories.entry(parent.to_owned()).or_insert(None);
        }
    }
    let mut wanted = BTreeSet::new();
    let mut operations = Vec::new();
    let mut entry = |path: PathBuf, seen: Snapshot, desired: Snapshot, kind: Kind| {
        wanted.insert(path.clone());
        // Even a no-op must settle an accepted local deletion, re-addition,
        // rename destination or metadata edit when its receipt differs.
        let up_to_date = manifest.delivered.get(&path).is_some_and(|record| {
            record.observed.is_none()
                && record.delivered == desired
                && match (&record.kind, &kind) {
                    (Kind::RegistryDirectory { name: old }, Kind::RegistryDirectory { name })
                    | (
                        Kind::RegistryAssetDirectory { name: old },
                        Kind::RegistryAssetDirectory { name },
                    )
                    | (Kind::RegistrySkill { name: old }, Kind::RegistrySkill { name }) => {
                        old == name
                    }
                    _ => false,
                }
        });
        if reviewing || seen != desired || !up_to_date {
            operations.push(Pending {
                path,
                seen,
                desired,
                kind: Some(kind),
                allow_link: false,
            });
        }
    };
    for (path, mode) in directories {
        ensure!(
            !path_blocked(blocked, &path),
            "{} is blocked",
            path.display()
        );
        let seen = package_entry(root, folder, &path, snapshot)?;
        if let (Snapshot::Directory { mode: current }, Some(mode)) = (&seen, mode) {
            if !reviewing && *current != mode {
                result.blocked.push(format!(
                    "{}: Destination permissions are {current}, but the approved copy uses {mode}. The existing directory was kept.",
                    path.display()
                ));
            }
        }
        let desired = match &seen {
            Snapshot::Directory { .. } if !reviewing || mode.is_none() => seen.clone(),
            Snapshot::Directory { .. } | Snapshot::Absent => {
                ensure!(
                    mode.is_none_or(FileMode::permits_child_writes)
                        || seen.is_directory()
                            && mode.map(Snapshot::directory_with_permissions) == Some(seen.clone()),
                    "{}: The source permissions would prevent managing the copied files.",
                    path.display()
                );
                mode.map_or_else(
                    Snapshot::private_directory,
                    Snapshot::directory_with_permissions,
                )
            }
            _ => bail!("{} is not a safe directory", path.display()),
        };
        let kind = if path == folder {
            Kind::RegistryDirectory { name: name.into() }
        } else {
            Kind::RegistryAssetDirectory { name: name.into() }
        };
        entry(path, seen, desired, kind);
    }
    for (relative, bytes) in files {
        let path = folder.join(relative);
        ensure!(
            !path_blocked(blocked, &path),
            "{} is blocked",
            path.display()
        );
        let seen = package_entry(root, folder, &path, snapshot)?;
        ensure!(
            matches!(seen, Snapshot::File { .. } | Snapshot::Absent),
            "{} is not a safe file",
            path.display()
        );
        let source_mode = modes.and_then(|modes| modes.files.get(relative)).copied();
        let peer_mode = match &seen {
            Snapshot::File { mode, .. } => Some(*mode),
            // A case-only rename may have an absent plan entry even though it
            // replaces a file at the alternate spelling.
            Snapshot::Absent if reviewing && skill_case_alias(root, &path)?.is_some() => {
                match root.read(&path, false)? {
                    Snapshot::File { mode, .. } => Some(mode),
                    _ => None,
                }
            }
            _ => None,
        };
        let mode = if explicit_file_modes.is_some_and(|paths| paths.contains(relative)) {
            source_mode.or(peer_mode)
        } else {
            peer_mode.or(source_mode)
        };
        let desired = match mode {
            Some(mode) => Snapshot::file_with_permissions(bytes.clone(), mode),
            None => Snapshot::file(bytes.clone()),
        };
        entry(
            path,
            seen,
            desired,
            Kind::RegistrySkill { name: name.into() },
        );
    }
    Ok((operations, wanted))
}

fn registry_removals(
    root: &FileRoot,
    manifest: &Manifest,
    expected: &BTreeMap<PathBuf, ExpectedRegistrySkill>,
    active: &BTreeSet<String>,
    blocked: &BTreeSet<PathBuf>,
    result: &mut RulesSyncReport,
    reviewing: bool,
) -> Result<Vec<Pending>> {
    let mut deliveries: Vec<_> = manifest.active_deliveries().collect();
    deliveries.sort_by_key(|(path, _)| std::cmp::Reverse(path.components().count()));
    let mut removals = BTreeSet::new();
    let mut operations = Vec::new();
    for (path, record) in deliveries {
        if path_blocked(blocked, path) || is_harness_path(path) {
            continue;
        }
        let stale = path
            .ancestors()
            .find_map(|folder| expected.get_key_value(folder))
            .filter(|(_, expected)| !expected.wanted.contains(path));
        let registry_name = match &record.kind {
            Kind::RegistryDirectory { name }
            | Kind::RegistryAssetDirectory { name }
            | Kind::RegistrySkill { name } => Some(name),
            _ => None,
        };
        let uninstall = !reviewing && registry_name.is_some_and(|name| !active.contains(name));
        let (seen, desired, kind) = if uninstall {
            let desired = manifest
                .backup
                .get(path)
                .cloned()
                .context("the backup is incomplete")?;
            let seen =
                match read_planned_path(root, path, true, record.delivered == Snapshot::Absent) {
                    Ok(seen) => seen,
                    Err(error) => {
                        result.blocked.push(format!("{}: {error}", path.display()));
                        continue;
                    }
                };
            if record.delivered != seen {
                result
                    .blocked
                    .push(format!("{} changed since delivery", path.display()));
                continue;
            }
            #[cfg(windows)]
            if let Err(reason) = check_windows_restore(root, manifest, path, &seen, &desired) {
                result.blocked.push(format!("{}: {reason}", path.display()));
                continue;
            }
            (seen, desired, None)
        } else if let Some((folder, package)) = stale {
            let kind = match &record.kind {
                Kind::Directory | Kind::RegistryAssetDirectory { .. } => {
                    Kind::RegistryAssetDirectory {
                        name: package.name.clone(),
                    }
                }
                Kind::Skill | Kind::RegistrySkill { .. } => Kind::RegistrySkill {
                    name: package.name.clone(),
                },
                _ => continue,
            };
            let seen = match package_entry(root, folder, path, package.snapshot.as_ref()) {
                Ok(seen) => seen,
                Err(error) => {
                    result
                        .blocked
                        .push(format!("{}: {error:#}", path.display()));
                    continue;
                }
            };
            if record.delivered == Snapshot::Absent && seen == Snapshot::Absent {
                continue;
            }
            (seen, Snapshot::Absent, Some(kind))
        } else {
            continue;
        };
        // Leave a directory containing anything outside the planned removals.
        if seen.is_directory()
            && !desired.is_directory()
            && !root.children(path)?.is_subset(&removals)
        {
            continue;
        }
        if desired == Snapshot::Absent {
            removals.insert(path.clone());
        }
        operations.push(Pending {
            path: path.clone(),
            allow_link: uninstall && matches!(desired, Snapshot::Symlink { .. }),
            seen,
            desired,
            kind,
        });
    }
    Ok(operations)
}

fn extend_personal_master(master: &mut PersonalMaster, added: PersonalMaster) {
    master.overlays.extend(added.overlays);
    master.codex_sync_replaced |= added.codex_sync_replaced;
    for skill in added.skills {
        let shared = master.skills.iter_mut().find(|current| {
            current.shared
                && skill.shared
                && current.name == skill.name
                && current.files == skill.files
                && current.upstream == skill.upstream
        });
        if let Some(shared) = shared {
            shared.targets.extend(skill.targets);
            shared.locations.extend(skill.locations);
        } else {
            master.skills.push(skill);
        }
    }
}

fn personal_rule_destination(
    rules: GlobalOutput,
    mut rendered: BTreeMap<PathBuf, Vec<u8>>,
) -> Result<BTreeMap<PathBuf, Vec<u8>>> {
    let bytes = rendered
        .remove(Path::new(rules.staged))
        .with_context(|| format!("rulesync did not render {}", rules.staged))?;
    Ok(BTreeMap::from([(rules.destination.into(), bytes)]))
}

fn personal_skill_destination(skills: GlobalOutput, emitted: &Path) -> Result<PathBuf> {
    ensure!(
        emitted.parent() == Some(Path::new(skills.staged)),
        "rulesync emitted a skill outside {}",
        skills.staged
    );
    let name = emitted.file_name().context("rendered skill has no name")?;
    Ok(Path::new(skills.destination).join(name))
}

fn skill_path(skills: &[AdoptedSkill], path: &Path) -> Option<SkillPath> {
    for (index, skill) in skills.iter().enumerate() {
        for (target, locations) in &skill.locations {
            for location in locations {
                if let Ok(relative) = path.strip_prefix(location) {
                    if !relative.as_os_str().is_empty() {
                        return Some(SkillPath {
                            index,
                            target: target.clone(),
                            installed: path.to_owned(),
                            relative: relative.to_owned(),
                        });
                    }
                }
            }
        }
    }
    None
}

pub(crate) type SkillPackages = BTreeMap<String, SkillFiles>;

struct SkillDelivery {
    files: SkillFiles,
    byte_copy_reason: Option<String>,
}

fn plan_skill_deliveries(
    adoption: &mut Adoption,
    render: &impl Fn(&str, &SkillPackages) -> Result<SkillFiles>,
) -> Result<()> {
    for target in PERSONAL_TARGETS {
        let skills: Vec<_> = adoption
            .skills
            .iter()
            .filter(|skill| skill.locations.contains_key(target.id))
            .collect();
        let deliveries = render_personal_skills(
            target,
            skills
                .iter()
                .map(|skill| (skill.name.as_str(), &skill.files)),
            render,
        )
        .with_context(|| format!("stage skills for {}", target.tool.display_name()))?;
        for (skill, delivery) in skills.into_iter().zip(deliveries) {
            for folder in &skill.locations[target.id] {
                if let Some(reason) = &delivery.byte_copy_reason {
                    adoption
                        .skill_byte_copies
                        .insert(folder.clone(), reason.clone());
                }
                for (relative, bytes) in &delivery.files {
                    let path = folder.join(relative);
                    let file = adoption.files.get_mut(&path).with_context(|| {
                        format!("missing planned skill file {}", path.display())
                    })?;
                    file.after = AdoptionEntry::File(bytes.clone());
                }
            }
        }
    }
    Ok(())
}

/// Prepare input for the pinned renderer and keep the whole source package
/// when that target cannot represent it. The returned files are the delivery
/// bytes for both adoption and subsequent updates.
fn render_personal_skills<'a>(
    target: &PersonalTarget,
    sources: impl IntoIterator<Item = (&'a str, &'a SkillFiles)>,
    render: impl Fn(&str, &SkillPackages) -> Result<SkillFiles>,
) -> Result<Vec<SkillDelivery>> {
    let sources: Vec<_> = sources.into_iter().collect();
    let mut names = BTreeSet::new();
    let duplicates: BTreeSet<_> = sources
        .iter()
        .map(|(name, _)| *name)
        .filter(|name| !names.insert(*name))
        .collect();
    let entry = Path::new("SKILL.md");
    let mut staged = BTreeMap::new();
    let mut candidates = Vec::new();
    for (name, files) in sources {
        let source = files
            .get(entry)
            .context("missing SKILL.md in the skill master")?;
        let stage = if duplicates.contains(name) {
            Err(anyhow::anyhow!(
                "multiple skill folders share the rulesync name {name}"
            ))
        } else {
            stage_adopted_skill(target.id, source)
        };
        let candidate = stage.map(|markup| {
            let mut input = files.clone();
            input.insert(entry.to_owned(), markup);
            staged.insert(name.to_owned(), input);
        });
        candidates.push((name, files, candidate));
    }
    let mut output = BTreeMap::new();
    let mut rejected = BTreeSet::new();
    if !staged.is_empty() {
        render_skill_batch(
            target.skills.renderer,
            staged,
            &render,
            &mut output,
            &mut rejected,
        )?;
    }
    Ok(candidates
        .into_iter()
        .map(|(name, files, candidate)| {
            let rendered = candidate.and_then(|_| {
                ensure!(!rejected.contains(name), rulesync_renderer::SkillRejected);
                let folder = Path::new(target.skills.staged).join(name);
                rendered_personal_skill(&folder, files, &output)
            });
            match rendered {
                Ok(files) => SkillDelivery {
                    files,
                    byte_copy_reason: None,
                },
                Err(error) => SkillDelivery {
                    files: files.clone(),
                    byte_copy_reason: Some(error.to_string()),
                },
            }
        })
        .collect())
}

fn render_skill_batch(
    renderer: &str,
    mut staged: SkillPackages,
    render: &impl Fn(&str, &SkillPackages) -> Result<SkillFiles>,
    output: &mut SkillFiles,
    rejected: &mut BTreeSet<String>,
) -> Result<()> {
    match render(renderer, &staged) {
        Ok(files) => output.extend(files),
        Err(error) if error.is::<rulesync_renderer::SkillRejected>() => {
            // Keep the normal batch. Retry with smaller groups only when
            // rulesync identified a bad package, discarding the partial output.
            if staged.len() == 1 {
                rejected.extend(staged.into_keys());
            } else {
                let pivot = staged.keys().nth(staged.len() / 2).unwrap().clone();
                let second = staged.split_off(&pivot);
                render_skill_batch(renderer, staged, render, output, rejected)?;
                render_skill_batch(renderer, second, render, output, rejected)?;
            }
        }
        Err(error) => return Err(error),
    }
    Ok(())
}

fn rendered_personal_skill(
    folder: &Path,
    source: &SkillFiles,
    output: &SkillFiles,
) -> Result<SkillFiles> {
    let entry = Path::new("SKILL.md");
    let markup = output
        .get(&folder.join(entry))
        .context("rulesync did not render SKILL.md")?;
    check_skill_frontmatter(&source[entry], markup)?;
    let mut files = source.clone();
    for (path, bytes) in output
        .range(folder.to_owned()..)
        .take_while(|(path, _)| path.starts_with(folder))
    {
        let relative = path.strip_prefix(folder)?;
        let file = files
            .get_mut(relative)
            .with_context(|| format!("rulesync added {}", relative.display()))?;
        file.clone_from(bytes);
    }
    Ok(files)
}

pub(crate) fn write_skill_files(
    input: &Path,
    name: &str,
    files: &BTreeMap<PathBuf, Vec<u8>>,
) -> Result<()> {
    let name = Path::new(name);
    ensure!(
        name.components().count() == 1 && safe_relative(name),
        "invalid staged skill name"
    );
    let root = input.join("skills").join(name);
    for (relative, bytes) in files {
        ensure!(safe_relative(relative), "invalid staged skill path");
        let path = root.join(relative);
        fs::create_dir_all(path.parent().context("staged skill file has no parent")?)?;
        fs::write(path, bytes)?;
    }
    Ok(())
}

fn safe_relative(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_)))
}

fn rendered_skill_root<'a>(
    output: &'a BTreeMap<PathBuf, Vec<u8>>,
    skill: &str,
) -> Result<Option<&'a Path>> {
    let marker = Path::new(skill).join("SKILL.md");
    let mut roots = output
        .keys()
        .filter(|path| path.ends_with(&marker))
        .filter_map(|path| path.parent());
    let root = roots.next();
    ensure!(roots.next().is_none(), "ambiguous staged skill destination");
    Ok(root)
}

/// Missing auxiliary files are still part of the approved package. A rendered
/// SKILL.md is required to establish that this target gets the package at all.
pub(crate) fn rendered_skill_with_assets<'a>(
    output: &mut BTreeMap<PathBuf, Vec<u8>>,
    name: &str,
    files: impl IntoIterator<Item = (&'a PathBuf, &'a Vec<u8>)>,
) -> Result<Option<PathBuf>> {
    let Some(folder) = rendered_skill_root(output, name)?.map(Path::to_owned) else {
        return Ok(None);
    };
    for (path, bytes) in files {
        output
            .entry(folder.join(path))
            .or_insert_with(|| bytes.clone());
    }
    Ok(Some(folder))
}

pub(crate) fn read_stage(
    root: &Path,
    dir: &Path,
    output: &mut BTreeMap<PathBuf, Vec<u8>>,
) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let leaf = entry.path();
        let kind = entry.file_type()?;
        if kind.is_dir() {
            read_stage(root, &leaf, output)?;
        } else if kind.is_file() {
            output.insert(leaf.strip_prefix(root)?.to_path_buf(), fs::read(leaf)?);
        } else {
            bail!("renderer produced a non-regular file");
        }
    }
    Ok(())
}

fn adoption_entry_matches(entry: &AdoptionEntry, snapshot: &Snapshot) -> bool {
    match (entry, snapshot) {
        (AdoptionEntry::Missing, Snapshot::Absent)
        | (AdoptionEntry::Directory, Snapshot::Directory { .. }) => true,
        (AdoptionEntry::File(seen), Snapshot::File { bytes, .. }) => seen == bytes,
        (AdoptionEntry::Symlink { .. }, Snapshot::Symlink { .. }) => true,
        _ => false,
    }
}

pub(crate) fn with_bytes(seen: &Snapshot, bytes: Vec<u8>) -> Snapshot {
    match seen {
        Snapshot::File { mode, .. } => Snapshot::File { bytes, mode: *mode },
        _ => Snapshot::file(bytes),
    }
}

pub(crate) fn prepare_checkout_displaced(root: &FileRoot, path: &Path) -> Result<PathBuf> {
    displaced::Batch::prepare_file(root, path)
}

fn same_rule_lines(actual: &[u8], expected: &[u8]) -> Result<bool> {
    let actual = std::str::from_utf8(actual)?;
    let expected = std::str::from_utf8(expected)?;
    Ok(actual
        .trim_matches(['\r', '\n'])
        .lines()
        .eq(expected.trim_matches(['\r', '\n']).lines()))
}

fn without_revoked_rules(snapshot: &Snapshot, accessible: &BTreeSet<String>) -> Result<Snapshot> {
    let before = text(snapshot)?;
    let after = rulesync_layers::remove_inaccessible(before, accessible)?;
    if before == after {
        return Ok(snapshot.clone());
    }
    Ok(with_bytes(snapshot, after.into_bytes()))
}

/// Treat missing parents like any other adoption destination. This records
/// their original absence before they are created and restores children first.
fn with_missing_parents(
    root: &FileRoot,
    operations: Vec<Pending>,
    existing_backup: Option<&BTreeMap<PathBuf, Snapshot>>,
) -> Result<Vec<Pending>> {
    let explicit: BTreeSet<_> = operations
        .iter()
        .filter(|op| op.desired.is_directory())
        .map(|op| op.path.clone())
        .collect();
    let replacing_links: BTreeSet<_> = operations
        .iter()
        .filter(|op| op.desired.is_directory() && matches!(op.seen, Snapshot::Symlink { .. }))
        .map(|op| op.path.clone())
        .collect();
    let mut checked = BTreeSet::new();
    let mut batch = Vec::with_capacity(operations.len());
    for op in operations {
        if op.desired != Snapshot::Absent {
            let ancestors: Vec<_> = op
                .path
                .ancestors()
                .skip(1)
                .filter(|path| !path.as_os_str().is_empty())
                .collect();
            for path in ancestors.into_iter().rev() {
                if explicit.contains(path) || !checked.insert(path.to_owned()) {
                    continue;
                }
                let absent = path_blocked(&replacing_links, path)
                    || root.read(path, false)? == Snapshot::Absent;
                if absent {
                    ensure!(
                        existing_backup.is_none_or(|backup| !backup.contains_key(path)),
                        "{} already belongs to the local setup backup",
                        path.display()
                    );
                    batch.push(Pending {
                        path: path.to_owned(),
                        seen: Snapshot::Absent,
                        desired: Snapshot::private_directory(),
                        kind: op.kind.as_ref().map(|_| Kind::Directory),
                        allow_link: false,
                    });
                }
            }
        }
        batch.push(op);
    }
    Ok(batch)
}

/// Journal pathname state before the first managed operation. Adoption
/// captures the whole managed skill tree, including empty directories;
/// later registry plans can only touch those paths or add new ones.
fn record_originals(
    root: &FileRoot,
    manifest: &mut Manifest,
    operations: &[Pending],
    adopting: bool,
) -> Result<()> {
    #[cfg(not(windows))]
    let _ = root;
    for op in operations {
        let Some(kind) = &op.kind else {
            continue;
        };
        let std::collections::btree_map::Entry::Vacant(entry) =
            manifest.backup.entry(op.path.clone())
        else {
            continue;
        };
        let original = if !adopting
            && matches!(
                kind,
                Kind::RegistryDirectory { .. }
                    | Kind::RegistryAssetDirectory { .. }
                    | Kind::RegistrySkill { .. }
            ) {
            Snapshot::Absent
        } else {
            op.seen.clone()
        };
        #[cfg(windows)]
        if matches!(original, Snapshot::File { .. } | Snapshot::Directory { .. }) {
            let owner = root.windows_home_owner()?;
            ensure!(
                root.read(&op.path, false)
                    .with_context(|| format!("back up {}", op.path.display()))?
                    == original
                    && owner == root.windows_home_owner()?,
                "{} changed before backup",
                op.path.display()
            );
            manifest
                .windows_backup_owners
                .insert(op.path.clone(), owner);
        }
        entry.insert(original);
    }
    Ok(())
}

#[cfg(windows)]
fn check_windows_restore(
    root: &FileRoot,
    manifest: &Manifest,
    path: &Path,
    current: &Snapshot,
    original: &Snapshot,
) -> Result<()> {
    if current == original || *original == Snapshot::Absent {
        return Ok(());
    }
    let owner = manifest.windows_backup_owners.get(path).context(
        "restore conflict: the backup has no verified Windows permissions. Keep it for manual recovery",
    )?;
    ensure!(
        *owner == root.windows_home_owner()?,
        "restore conflict: the Windows owner changed since backup"
    );
    Ok(())
}

fn settle(manifest: &mut Manifest, operation: Pending) -> Result<()> {
    // Withdraw shared content from the receipts without accepting the member's unseen edits.
    if let Some(Kind::RuleRevocation { accessible }) = &operation.kind {
        let record = manifest
            .delivered
            .get_mut(&operation.path)
            .context("missing rule delivery during revocation")?;
        record.delivered = without_revoked_rules(&record.delivered, accessible)?;
        if let Some(observed) = &mut record.observed {
            *observed = without_revoked_rules(observed, accessible)?;
        }
        return Ok(());
    }
    if let Some(kind) = operation.kind {
        if matches!(kind, Kind::Settings) {
            settings::settle(manifest, &operation.path);
        }
        manifest.delivered.insert(
            operation.path,
            Delivery {
                kind,
                delivered: operation.desired,
                observed: None,
                reviewed: false,
                at: chrono::Utc::now().timestamp(),
            },
        );
    } else {
        manifest.delivered.remove(&operation.path);
    }
    Ok(())
}

pub(crate) fn text(snapshot: &Snapshot) -> Result<&str> {
    match snapshot {
        Snapshot::Absent => Ok(""),
        Snapshot::File { bytes, .. } => Ok(std::str::from_utf8(bytes)?),
        _ => bail!("this file has no text to pull back"),
    }
}

struct Draft {
    path: PathBuf,
    current: Snapshot,
    changes: Vec<LineChange>,
    diff: String,
    needs_review: bool,
}

struct Card {
    wire: PullbackCard,
    paths: BTreeSet<PathBuf>,
}

struct Pullback {
    drafts: Vec<Draft>,
    cards: Vec<Card>,
    blocked: BTreeMap<PathBuf, String>,
}

fn inspect(root: &FileRoot, manifest: &Manifest) -> Result<Pullback> {
    let mut drafts = Vec::new();
    let mut blocked = manifest.recovery_issues.clone();
    for (path, delivery) in manifest.active_deliveries() {
        if blocked.contains_key(path) {
            continue;
        }
        if is_harness_path(path) {
            blocked.insert(path.clone(), HARNESS_READ_ONLY.into());
            continue;
        }
        if !matches!(delivery.kind, Kind::Rule { .. }) {
            continue;
        }
        let current = match root.read(path, false) {
            Ok(current) => current,
            Err(error) => {
                blocked.insert(path.clone(), format!("Cannot read this rules file ({error:#}). Check that it is a readable regular file and sync again."));
                continue;
            }
        };
        let baseline = delivery.observed.as_ref().unwrap_or(&delivery.delivered);
        if *baseline == current {
            continue;
        }
        let (Ok(before), Ok(after)) = (text(baseline), text(&current)) else {
            blocked.insert(path.clone(), "The rules file is missing or is not UTF-8. Restore it as a UTF-8 text file and sync again.".into());
            continue;
        };
        if !rulesync_layers::edits(before, after).is_ok_and(|edits| edits.is_empty()) {
            blocked.insert(path.clone(), "A shared section or its markers changed. Restore that section from the shared Rules page and sync again.".into());
            continue;
        }
        let before = rulesync_layers::only_personal(before)?;
        let after = rulesync_layers::only_personal(after)?;
        let changes = match line_changes(&before, &after) {
            Ok(changes) => changes,
            Err(error) => {
                blocked.insert(path.clone(), format!("Cannot compare the rules edits ({error:#}). Split large changes into smaller edits and sync again."));
                continue;
            }
        };
        if changes.is_empty() {
            blocked.insert(path.clone(), "The file metadata changed without a rules edit. Check its permissions and sync again.".into());
            continue;
        }
        let needs_review =
            delivery.reviewed || changes.iter().any(|change| change.removed.is_empty());
        let (_, _, diff) = describe_change(&path.display().to_string(), &before, &after);
        drafts.push(Draft {
            path: path.clone(),
            current,
            changes,
            diff,
            needs_review,
        });
    }
    let mut grouped: Vec<Vec<usize>> = Vec::new();
    for (index, draft) in drafts.iter().enumerate() {
        let mut group = vec![index];
        grouped.retain(|previous| {
            if previous
                .iter()
                .any(|other| competing_edits(draft, &drafts[*other]))
            {
                group.extend(previous);
                false
            } else {
                true
            }
        });
        grouped.push(group);
    }
    let mut cards = Vec::new();
    for group in grouped {
        if group.len() == 1 && !drafts[group[0]].needs_review {
            continue;
        }
        let paths: BTreeSet<_> = group.iter().map(|i| drafts[*i].path.clone()).collect();
        let mut entries = group
            .iter()
            .map(|i| (&drafts[*i].path, &drafts[*i].current, &drafts[*i].diff))
            .collect::<Vec<_>>();
        entries.sort_by_key(|(path, _, _)| *path);
        let reason = if paths.len() > 1 {
            "Several tools changed the same lines"
        } else {
            "These changes do not have a verified author"
        };
        cards.push(Card {
            wire: PullbackCard {
                id: fingerprint(&entries)?,
                files: paths
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect(),
                diff: entries.iter().map(|(_, _, diff)| diff.as_str()).collect(),
                reason: reason.into(),
            },
            paths,
        });
    }
    Ok(Pullback {
        drafts,
        cards,
        blocked,
    })
}

fn competing_edits(first: &Draft, second: &Draft) -> bool {
    first.changes.iter().any(|change| {
        second.changes.iter().any(|other| {
            change.added != other.added
                && change
                    .removed
                    .iter()
                    .any(|line| !line.trim().is_empty() && other.removed.contains(line))
        })
    })
}

fn accept_tool_edit(manifest: &mut Manifest, draft: &Draft) -> Result<()> {
    let delivery = manifest
        .delivered
        .get_mut(&draft.path)
        .context("missing managed file")?;
    let Kind::Rule {
        target,
        source,
        base,
    } = &delivery.kind
    else {
        bail!("a skill proposal must be resolved in the skill's source");
    };
    let baseline = delivery.observed.as_ref().unwrap_or(&delivery.delivered);
    let previous = current_rule_source(&manifest.master, target, base, source)?;
    let before = rulesync_layers::only_personal(text(baseline)?)?;
    let after = rulesync_layers::only_personal(text(&draft.current)?)?;
    let edited = merge_rule_text(&before, &after, &previous)?;
    manifest.master.overrides.insert(
        target.clone(),
        LocalText {
            base: base.clone(),
            edited,
        },
    );
    delivery.observed = Some(draft.current.clone());
    Ok(())
}

fn current_rule_source(
    master: &PersonalMaster,
    target: &str,
    base: &str,
    source: &str,
) -> Result<String> {
    match master.overrides.get(target) {
        Some(local) => merge_rule_text(&local.base, &local.edited, base),
        None => Ok(source.to_owned()),
    }
}

pub(crate) fn report(message: &str) -> RulesSyncReport {
    RulesSyncReport {
        written: Vec::new(),
        blocked: Vec::new(),
        proposals: Vec::new(),
        message: message.into(),
    }
}

fn append_skipped_tools(report: &mut RulesSyncReport, skipped: &[String]) {
    for note in skipped {
        report.message.push('\n');
        report.message.push_str(note);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHARED: &str = "# Shared\n\nOne rule.\nSecond rule.\n";
    const CLAUDE: &str = ".claude/CLAUDE.md";
    const CODEX: &str = ".codex/AGENTS.md";

    struct Fixture {
        _temp: tempfile::TempDir,
        setup: Setup,
    }

    impl Fixture {
        fn new() -> Self {
            let _temp = tempfile::tempdir().unwrap();
            let home = _temp.path().join("home");
            let data = _temp.path().join("data");
            fs::create_dir_all(home.join(".agents")).unwrap();
            fs::create_dir(home.join(".claude")).unwrap();
            fs::create_dir(home.join(".codex")).unwrap();
            fs::create_dir(&data).unwrap();
            fs::write(home.join(".agents/AGENTS.md"), SHARED).unwrap();
            fs::write(home.join(CLAUDE), "@~/.agents/AGENTS.md\nClaude only.\n").unwrap();
            fs::write(home.join(CODEX), format!("{SHARED}Codex only.\n")).unwrap();
            Self {
                setup: Setup {
                    home,
                    data,
                    binary: PathBuf::from("unused"),
                },
                _temp,
            }
        }

        fn adopt_from_plan(&self) {
            let adoption = discover_adoption(&self.setup.home).unwrap();
            let master = PersonalMaster::new(&adoption);
            let root = FileRoot::open_home(&self.setup.home).unwrap();
            let _lock = self.setup.lock().unwrap();
            self.setup.apply_adoption(&root, adoption, master).unwrap();
        }

        fn sync_with(&self, pin_banner: &str) -> RulesSyncReport {
            let root = FileRoot::open_home(&self.setup.home).unwrap();
            let _lock = self.setup.lock().unwrap();
            let mut manifest = self.setup.load().unwrap().unwrap();
            self.setup
                .sync_locked(
                    &root,
                    &mut manifest,
                    &[],
                    |target, text| {
                        let path = personal_target(target).unwrap().rules.destination;
                        Ok(BTreeMap::from([(
                            PathBuf::from(path),
                            format!("{pin_banner}{text}").into_bytes(),
                        )]))
                    },
                    fixture_skill_renderer,
                )
                .unwrap()
        }

        fn sync_review(&self, root: &FileRoot, manifest: &mut Manifest) -> Result<RulesSyncReport> {
            self.setup.sync_locked(
                root,
                manifest,
                &[],
                fixture_rule_renderer,
                fixture_skill_renderer,
            )
        }

        fn text(&self, path: &str) -> String {
            fs::read_to_string(self.setup.home.join(path)).unwrap()
        }

        fn write(&self, path: &str, text: &str) {
            fs::write(self.setup.home.join(path), text).unwrap();
        }
    }

    fn fixture_rule_renderer(target: &str, source: &str) -> Result<SkillFiles> {
        let path = personal_target(target).unwrap().rules.destination;
        Ok(BTreeMap::from([(path.into(), source.as_bytes().to_vec())]))
    }

    fn fixture_skill_renderer(renderer: &str, skills: &SkillPackages) -> Result<SkillFiles> {
        let output = PERSONAL_TARGETS
            .iter()
            .find(|target| target.skills.renderer == renderer)
            .unwrap()
            .skills
            .staged;
        Ok(skills
            .iter()
            .flat_map(|(name, files)| {
                let folder = Path::new(output).join(name);
                files
                    .iter()
                    .map(move |(path, bytes)| (folder.join(path), bytes.clone()))
            })
            .collect())
    }

    fn render_fixture_tree(
        data: &Path,
        binary: &Path,
        target: &str,
        feature: &str,
        stage_input: impl FnOnce(&Path) -> Result<()>,
    ) -> Result<BTreeMap<PathBuf, Vec<u8>>> {
        let stage = rulesync_renderer::new_stage(data)?;
        let input = stage.path().join("input");
        let output = stage.path().join("home");
        let runtime = stage.path().join("tmp");
        for path in [&input, &output, &runtime] {
            fs::create_dir(path)?;
        }
        stage_input(&input)?;
        let mut command = std::process::Command::new(binary);
        command
            .env_clear()
            .env("HOME", &output)
            .env("HOME_DIR", &output)
            .env("USERPROFILE", &output)
            .env("TMPDIR", &runtime)
            .env("DO_NOT_TRACK", "1")
            .current_dir(&output)
            .args([
                "generate",
                "--global",
                "--output-roots",
                ".",
                "--targets",
                target,
            ])
            .args(["--features", feature, "--input-roots"])
            .arg(input);
        #[cfg(windows)]
        {
            command
                .env("APPDATA", &output)
                .env("LOCALAPPDATA", &output)
                .env("TMP", &runtime)
                .env("TEMP", &runtime);
            if let Some(root) = std::env::var_os("SystemRoot") {
                command.env("SystemRoot", root);
            }
        }
        let result = command.output()?;
        if feature == "skills"
            && result.status.code() == Some(1)
            && result.stderr.starts_with(b"Invalid frontmatter in ")
        {
            return Err(rulesync_renderer::SkillRejected.into());
        }
        ensure!(
            result.status.success(),
            "{target} {feature}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let mut tree = BTreeMap::new();
        read_stage(&output, &output, &mut tree)?;
        Ok(tree)
    }

    fn render_fixture_skills(
        data: &Path,
        binary: &Path,
        target: &str,
        skills: &SkillPackages,
    ) -> Result<SkillFiles> {
        render_fixture_tree(data, binary, target, "skills", |input| {
            for (name, files) in skills {
                write_skill_files(input, name, files)?;
            }
            Ok(())
        })
    }

    fn personal_targets_round_trip(binary: Option<&Path>) {
        const DESCRIPTION: &str = "Use a headless CLI for agent-to-agent communication, including prompt and session workflows, session scoping, queueing, permissions, output formats, system-prompt overrides, and multi-agent flows with branching decisions and explicit boundaries.";
        const METADATA: &str = "---\r\nname: metadata-rich\r\ndescription: Native skill metadata\r\nversion: '1.2.3'\r\nargument-hint: '<topic>'\r\nallowed-tools:\r\n  - Read\r\n---\r\n\r\nOriginal body.\r\n";
        const ASSET: &str = "Original guide.  \r\n";
        let f = Fixture::new();
        let mut originals = BTreeMap::new();
        let install = |path: &Path, data: &str| {
            let path = f.setup.home.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, data).unwrap();
        };
        for target in PERSONAL_TARGETS {
            let rules = Path::new(target.rules.destination);
            let overlay = format!("{} only.\n", target.id);
            install(rules, &overlay);
            originals.insert(rules.to_owned(), overlay.into_bytes());
            let folder = Path::new(target.skills.destination).join("example");
            for (leaf, content) in [
                (
                    "SKILL.md",
                    format!("---\nname: example\ndescription: {DESCRIPTION}\n---\n\nBody.\n"),
                ),
                ("guide.txt", format!("{} guide.\n", target.id)),
            ] {
                let path = folder.join(leaf);
                install(&path, &content);
                originals.insert(path, content.into_bytes());
            }
            for (leaf, content) in [("SKILL.md", METADATA), ("guide.txt", ASSET)] {
                let path = Path::new(target.skills.destination)
                    .join("metadata-rich")
                    .join(leaf);
                install(&path, content);
                originals.insert(path, content.as_bytes().to_vec());
            }
        }
        let render_rules = |target: &str, source: &str| {
            let spec = personal_target(target).unwrap().rules;
            let staged = if let Some(binary) = binary {
                render_fixture_tree(&f.setup.data, binary, spec.renderer, "rules", |input| {
                    write_root_rule(input, source)
                })?
            } else {
                BTreeMap::from([(PathBuf::from(spec.staged), source.as_bytes().to_vec())])
            };
            personal_rule_destination(spec, staged)
        };
        let render_skills = |renderer: &str, skills: &SkillPackages| {
            if let Some(binary) = binary {
                render_fixture_skills(&f.setup.data, binary, renderer, skills)
            } else {
                fixture_skill_renderer(renderer, skills)
            }
        };
        let sync = |root: &FileRoot, manifest: &mut Manifest| {
            f.setup
                .sync_locked(root, manifest, &[], render_rules, render_skills)
        };
        let run_sync = || {
            let _lock = f.setup.lock().unwrap();
            let root = FileRoot::open_home(&f.setup.home).unwrap();
            let mut manifest = f.setup.load().unwrap().unwrap();
            sync(&root, &mut manifest).unwrap()
        };
        let plan = f.setup.preview_adoption_using(render_skills).unwrap();
        let adopted = f
            .setup
            .adopt_using(&plan.token, render_rules, render_skills)
            .unwrap();
        let written = adopted.written.iter().collect::<BTreeSet<_>>();
        assert_eq!(written.len(), adopted.written.len());
        assert_eq!(f.setup.delivery_status().len(), PERSONAL_TARGETS.len());
        for target in PERSONAL_TARGETS {
            let rules = f.text(target.rules.destination);
            assert!(rules.contains("One rule."));
            assert!(rules.contains(&format!("{} only.", target.id)));
            assert!(f.setup.delivery_status()[target.agent].at.is_some());
            let folder = Path::new(target.skills.destination).join("metadata-rich");
            let skill = plan
                .skills
                .iter()
                .find(|skill| Path::new(&skill.path) == folder)
                .unwrap();
            assert!(
                skill
                    .byte_copy_reason
                    .as_deref()
                    .is_some_and(|reason| reason.contains("version")),
                "{skill:?}"
            );
            assert_eq!(
                fs::read(f.setup.home.join(&folder).join("SKILL.md")).unwrap(),
                METADATA.as_bytes()
            );
            assert_eq!(
                fs::read(f.setup.home.join(&folder).join("guide.txt")).unwrap(),
                ASSET.as_bytes()
            );
            if let Some(binary) = binary {
                let installed = Path::new(target.skills.destination).join("example/SKILL.md");
                let skill = plan
                    .skills
                    .iter()
                    .find(|skill| Path::new(&skill.path) == installed.parent().unwrap())
                    .unwrap();
                assert_eq!(skill.byte_copy_reason, None, "{}", target.id);
                let original = &originals[&installed];
                let staged = render_fixture_tree(
                    &f.setup.data,
                    binary,
                    target.skills.renderer,
                    "skills",
                    |input| {
                        write_skill_files(
                            input,
                            "example",
                            &BTreeMap::from([("SKILL.md".into(), original.clone())]),
                        )
                    },
                )
                .unwrap();
                let expected = &staged[&Path::new(target.skills.staged).join("example/SKILL.md")];
                assert_eq!(
                    fs::read(f.setup.home.join(&installed)).unwrap(),
                    *expected,
                    "{}",
                    target.id
                );
                let shown = plan
                    .files
                    .iter()
                    .find(|file| Path::new(&file.path) == installed)
                    .unwrap();
                let (_, _, diff) = describe_change(
                    &shown.path,
                    std::str::from_utf8(original).unwrap(),
                    std::str::from_utf8(expected).unwrap(),
                );
                assert_eq!(shown.diff, diff, "{}", target.id);
            }
        }
        let fresh = run_sync();
        assert!(
            fresh.written.is_empty() && fresh.proposals.is_empty() && fresh.blocked.is_empty(),
            "{fresh:?}"
        );

        for (source, change, action) in [
            (
                ".gemini/config/GEMINI.md",
                "Antigravity local addition.\n",
                PullbackAction::ToolOnly,
            ),
            (
                ".config/muse/AGENTS.md",
                "Shared from Muse Code.\n",
                PullbackAction::AllTools,
            ),
        ] {
            f.write(source, &format!("{}{change}", f.text(source)));
            let pending = run_sync();
            let [card] = pending.proposals.as_slice() else {
                panic!("expected one proposal, got {pending:?}");
            };
            assert_eq!(Path::new(&card.files[0]), Path::new(source));
            assert!(card.diff.contains(change.trim()));
            let applied = f
                .setup
                .review_using(
                    &PullbackDecision {
                        id: card.id.clone(),
                        action,
                        file: None,
                    },
                    sync,
                )
                .unwrap();
            assert!(
                applied.blocked.is_empty() && applied.proposals.is_empty(),
                "{applied:?}"
            );
            for target in PERSONAL_TARGETS {
                let path = target.rules.destination;
                let copies = f.text(path).matches(change.trim()).count();
                assert_eq!(
                    copies,
                    usize::from(action == PullbackAction::AllTools || path == source),
                    "{path}"
                );
            }
        }
        assert!(run_sync().proposals.is_empty());

        let edited_skill = METADATA.replace("Original body.", "Reviewed change.");
        let edited_asset = "Reviewed guide.  \r\n";
        f.write(".cursor/skills/metadata-rich/SKILL.md", &edited_skill);
        f.write(".cursor/skills/metadata-rich/guide.txt", edited_asset);
        let pending = run_sync();
        assert!(
            pending
                .blocked
                .iter()
                .any(|path| path.contains("metadata-rich")),
            "{pending:?}"
        );
        {
            let proposal = rulesync_registry::accept_only_tool_proposal(
                &f.setup,
                Some(wenlan_types::rulesync_skill::SavedSkillScope::Shared),
                render_skills,
            )
            .unwrap();
            assert_eq!(proposal.name, "metadata-rich");
        }
        let approved = run_sync();
        assert!(approved.blocked.is_empty(), "{approved:?}");
        for target in PERSONAL_TARGETS {
            let folder = f
                .setup
                .home
                .join(target.skills.destination)
                .join("metadata-rich");
            assert_eq!(
                fs::read(folder.join("SKILL.md")).unwrap(),
                edited_skill.as_bytes(),
                "{}",
                target.id
            );
            assert_eq!(
                fs::read(folder.join("guide.txt")).unwrap(),
                edited_asset.as_bytes(),
                "{}",
                target.id
            );
        }
        let settled = run_sync();
        assert!(
            settled.written.is_empty() && settled.blocked.is_empty(),
            "{settled:?}"
        );

        let restored = f.setup.restore().unwrap();
        assert!(restored.blocked.is_empty(), "{restored:?}");
        for (path, before) in originals {
            assert_eq!(
                fs::read(f.setup.home.join(&path)).unwrap(),
                before,
                "{}",
                path.display()
            );
        }
        assert_eq!(f.text(".agents/AGENTS.md"), SHARED);
    }

    #[test]
    fn rulesync_more_personal_targets_round_trip() {
        personal_targets_round_trip(None);
    }

    #[test]
    #[ignore = "requires the local pinned rulesync binary and an isolated test home"]
    fn rulesync_more_personal_targets_round_trip_with_pinned_binary() {
        let binary = PathBuf::from(std::env::var_os("WENLAN_TEST_RULESYNC_BINARY").unwrap());
        personal_targets_round_trip(Some(&binary));
    }

    #[test]
    #[ignore = "requires the pinned binary and a disposable copied tool home under .scratch"]
    fn rulesync_copied_home_adopts_and_restores_with_pinned_binary() {
        use wenlan_types::rulesync::AdoptionSkillOutcome;

        let binary = PathBuf::from(std::env::var_os("WENLAN_TEST_RULESYNC_BINARY").unwrap());
        let home = PathBuf::from(std::env::var_os("WENLAN_TEST_RULESYNC_COPIED_HOME").unwrap())
            .canonicalize()
            .unwrap();
        let scratch = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.scratch")
            .canonicalize()
            .unwrap();
        assert!(
            home.starts_with(&scratch) && home != scratch,
            "use a disposable copy under .scratch"
        );
        let data = rulesync_renderer::new_stage(&scratch).unwrap();
        let setup = Setup {
            home,
            data: data.path().to_owned(),
            binary,
        };
        let render_rule = |id: &str, source: &str| {
            let target = personal_target(id).unwrap();
            let files = render_fixture_tree(
                &setup.data,
                &setup.binary,
                target.rules.renderer,
                "rules",
                |input| write_root_rule(input, source),
            )?;
            personal_rule_destination(target.rules, files)
        };
        let render_skills = |renderer: &str, skills: &SkillPackages| {
            render_fixture_skills(&setup.data, &setup.binary, renderer, skills)
        };
        let plan = setup.preview_adoption_using(render_skills).unwrap();
        assert!(!setup.data.join("personal.json").exists());
        let copies: BTreeSet<_> = plan
            .skills
            .iter()
            .filter(|item| item.byte_copy_reason.is_some())
            .map(|item| Path::new(&item.path))
            .collect();
        assert!(!copies.is_empty());
        for target in PERSONAL_TARGETS {
            let scope = Path::new(target.skills.destination);
            let skills = plan
                .skills
                .iter()
                .filter(|item| Path::new(&item.path).starts_with(scope));
            let (mut rendered, mut copied, mut skipped) = (0, 0, 0);
            for item in skills {
                if let AdoptionSkillOutcome::Skipped { reason } = &item.outcome {
                    println!("Skipped {} ({reason})", item.path);
                    skipped += 1;
                } else if item.byte_copy_reason.is_some() {
                    copied += 1;
                } else {
                    rendered += 1;
                }
            }
            println!(
                "{} rendered={rendered} byte_copied={copied} skipped={skipped}",
                target.id
            );
        }

        let applied = setup
            .adopt_using(&plan.token, render_rule, render_skills)
            .unwrap();
        assert!(applied.blocked.is_empty(), "{applied:?}");
        let mut manifest = setup.load().unwrap().unwrap();
        assert_eq!(manifest.master.overlays.len(), PERSONAL_TARGETS.len());
        let mut checked = 0;
        let mut names = BTreeSet::new();
        for skill in &manifest.master.skills {
            for folder in skill
                .locations
                .values()
                .flatten()
                .filter(|folder| copies.contains(folder.as_path()))
            {
                for (path, bytes) in &skill.files {
                    assert_eq!(
                        fs::read(setup.home.join(folder).join(path)).unwrap(),
                        *bytes,
                        "{folder:?} {path:?}"
                    );
                }
                names.insert(skill.name.as_str());
                checked += 1;
            }
        }
        assert_eq!(checked, copies.len());
        println!(
            "verified {checked} byte-copy destinations across {} skill names",
            names.len()
        );
        {
            let root = FileRoot::open_home(&setup.home).unwrap();
            let _lock = setup.lock().unwrap();
            let synced = setup
                .sync_locked(&root, &mut manifest, &[], render_rule, render_skills)
                .unwrap();
            assert!(synced.blocked.is_empty(), "{synced:?}");
        }
        let restored = setup.restore().unwrap();
        assert!(restored.blocked.is_empty(), "{restored:?}");
        assert!(!setup.data.join("personal.json").exists());
        println!(
            "restored {} managed files and directories",
            restored.written.len()
        );
    }

    #[test]
    fn rulesync_adds_cursor_to_the_existing_adoption_and_restores_every_tool() {
        let f = Fixture::new();
        let original_claude = f.text(CLAUDE);
        let original_codex = f.text(CODEX);
        let preview = || {
            f.setup
                .preview_adoption_using(fixture_skill_renderer)
                .unwrap()
        };
        let adopt = |plan: &AdoptionPlan| {
            f.setup
                .adopt_using(&plan.token, fixture_rule_renderer, fixture_skill_renderer)
        };
        adopt(&preview()).unwrap();

        // Change the current shared master through the normal pull-back review.
        f.write(CODEX, &format!("{}Later shared rule.\n", f.text(CODEX)));
        let pending = f.sync_with("");
        f.setup
            .review_using(
                &PullbackDecision {
                    id: pending.proposals[0].id.clone(),
                    action: PullbackAction::AllTools,
                    file: None,
                },
                |root, manifest| f.sync_review(root, manifest),
            )
            .unwrap();
        let old = f.setup.load().unwrap().unwrap();

        let cursor = ".cursor/AGENTS.md";
        let original_cursor = "Cursor only.\r\nNo final newline.";
        let skill = ".cursor/skills/local/SKILL.md";
        let original_skill =
            "---\nname: local\ndescription: Cursor skill\nversion: '2.0'\n---\n\nOriginal body.\n";
        fs::create_dir_all(f.setup.home.join(skill).parent().unwrap()).unwrap();
        f.write(cursor, original_cursor);
        f.write(skill, original_skill);
        let offer = f.sync_with("");
        assert!(offer
            .message
            .contains("Additional tools available: Cursor."));
        assert!(
            offer.written.is_empty() && offer.blocked.is_empty() && offer.proposals.is_empty(),
            "{offer:?}"
        );

        let plan = preview();
        assert!(plan
            .notes
            .iter()
            .any(|note| note == "Add to the existing setup: Cursor."));
        assert!(plan
            .files
            .iter()
            .all(|file| file.path.starts_with(".cursor/")));
        assert_eq!(plan.skills.len(), 1);
        assert!(plan.skills[0].byte_copy_reason.is_some());
        assert_eq!(f.text(cursor), original_cursor);

        f.write(cursor, "Changed during approval.\n");
        assert!(adopt(&plan).is_err());
        assert_eq!(f.text(cursor), "Changed during approval.\n");
        assert!(!f
            .setup
            .load()
            .unwrap()
            .unwrap()
            .master
            .overlays
            .contains_key("cursor"));
        f.write(cursor, original_cursor);

        let receipt = adopt(&preview()).unwrap();
        assert!(receipt
            .written
            .iter()
            .all(|path| Path::new(path).starts_with(".cursor")));
        assert!(receipt.message.contains("Added Cursor"));
        assert!(f.text(cursor).contains("Later shared rule."));
        assert!(f.text(cursor).contains("Cursor only."));
        assert_eq!(f.text(skill), original_skill);
        let extended = f.setup.load().unwrap().unwrap();
        assert_eq!(extended.master.overlays.len(), 3);
        assert_eq!(extended.master.overlays["cursor"], original_cursor);
        for (path, backup) in &old.backup {
            assert_eq!(&extended.backup[path], backup);
            assert_eq!(
                extended.delivered[path].delivered,
                old.delivered[path].delivered
            );
            assert_eq!(extended.delivered[path].at, old.delivered[path].at);
        }
        assert!(preview().files.is_empty());
        assert!(!f
            .sync_with("")
            .message
            .contains("Additional tools available"));

        f.write(
            cursor,
            &format!("{}\nCursor edit to keep.\n", f.text(cursor)),
        );
        let pending = f.sync_with("");
        let [card] = pending.proposals.as_slice() else {
            panic!("expected one Cursor proposal, got {pending:?}");
        };
        assert_eq!(
            card.files.iter().map(Path::new).collect::<Vec<_>>(),
            [Path::new(cursor)]
        );
        f.setup
            .review_using(
                &PullbackDecision {
                    id: card.id.clone(),
                    action: PullbackAction::ToolOnly,
                    file: None,
                },
                |root, manifest| f.sync_review(root, manifest),
            )
            .unwrap();
        assert!(f.text(cursor).contains("Cursor edit to keep."));
        for path in [CLAUDE, CODEX] {
            assert!(!f.text(path).contains("Cursor edit to keep."));
        }
        let restore = f.setup.restore().unwrap();
        assert!(restore.blocked.is_empty(), "{restore:?}");
        assert_eq!(f.text(CLAUDE), original_claude);
        assert_eq!(f.text(CODEX), original_codex);
        assert_eq!(f.text(cursor), original_cursor);
        assert_eq!(f.text(skill), original_skill);
    }

    #[cfg(unix)]
    #[test]
    fn rulesync_sharing_a_byte_copy_keeps_a_new_executable_asset() {
        use std::os::unix::fs::PermissionsExt;

        let f = Fixture::new();
        let folder = f.setup.home.join(".claude/skills/example");
        let codex = f.setup.home.join(".codex/skills/example");
        for skill in [&folder, &codex] {
            fs::create_dir_all(skill).unwrap();
            fs::write(
                skill.join("SKILL.md"),
                "---\nname: example\ndescription: Fixture\nversion: 1\n---\n\nBody.\n",
            )
            .unwrap();
        }
        f.adopt_from_plan();
        let scripts = folder.join("scripts");
        fs::create_dir(&scripts).unwrap();
        fs::set_permissions(&scripts, fs::Permissions::from_mode(0o750)).unwrap();
        let script = scripts.join("run.sh");
        fs::write(&script, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o751)).unwrap();
        f.sync_with("");
        {
            accept_fixture_tool_proposal(&f).unwrap();
        }
        let delivered = f.sync_with("");
        assert!(delivered.blocked.is_empty(), "{delivered:?}");
        let copied = codex.join("scripts/run.sh");
        assert_eq!(fs::read(&copied).unwrap(), fs::read(&script).unwrap());
        assert_eq!(
            fs::metadata(&copied).unwrap().permissions().mode() & 0o7777,
            0o751
        );
        assert_eq!(
            fs::metadata(copied.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o7777,
            0o750
        );
    }

    #[test]
    #[ignore = "requires the local pinned rulesync binary and an isolated test home"]
    fn rulesync_preview_byte_copies_a_package_rejected_by_the_pin() {
        let f = Fixture::new();
        let binary = PathBuf::from(std::env::var_os("WENLAN_TEST_RULESYNC_BINARY").unwrap());
        let rejected = "---\nname: rejected\ndescription: Fixture\npaths: 42\n---\n\nBody.\n";
        let valid = "---\nname: valid\ndescription: Fixture\n---\n\nBody.\n";
        for (name, text) in [("rejected", rejected), ("valid", valid)] {
            let folder = f.setup.home.join(".cursor/skills").join(name);
            fs::create_dir_all(&folder).unwrap();
            fs::write(folder.join("SKILL.md"), text).unwrap();
        }
        let renderer = |target: &str, skills: &SkillPackages| {
            render_fixture_skills(&f.setup.data, &binary, target, skills)
        };
        let calls = std::cell::Cell::new(0);
        let error = f
            .setup
            .preview_adoption_using(|_, _| {
                calls.set(calls.get() + 1);
                bail!("rulesync checksum mismatch")
            })
            .unwrap_err();
        assert!(format!("{error:#}").contains("checksum mismatch"));
        assert_eq!(calls.get(), 1);
        assert!(!f.setup.data.join("personal.json").exists());
        let plan = f.setup.preview_adoption_using(renderer).unwrap();
        let copies: Vec<_> = plan
            .skills
            .iter()
            .filter(|skill| skill.byte_copy_reason.is_some())
            .map(|skill| skill.path.as_str())
            .collect();
        assert_eq!(copies, [".cursor/skills/rejected"]);
        f.setup
            .adopt_using(&plan.token, fixture_rule_renderer, renderer)
            .unwrap();
        assert_eq!(f.text(".cursor/skills/rejected/SKILL.md"), rejected);
        assert_ne!(f.text(".cursor/skills/valid/SKILL.md"), valid);
    }

    #[cfg(unix)]
    #[test]
    fn rulesync_byte_copy_keeps_linked_skill_directory_and_file_modes() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let f = Fixture::new();
        let source = f.setup.home.join(".agents/skills/example");
        let skills = f.setup.home.join(".cursor/skills");
        fs::create_dir_all(source.join("scripts")).unwrap();
        fs::create_dir_all(&skills).unwrap();
        fs::write(
            source.join("SKILL.md"),
            "---\nname: example\ndescription: Fixture\nversion: 1\n---\n\nBody.\n",
        )
        .unwrap();
        fs::write(source.join("scripts/run.sh"), "#!/bin/sh\nexit 0\n").unwrap();
        for (path, mode) in [("", 0o755), ("scripts", 0o750), ("scripts/run.sh", 0o751)] {
            fs::set_permissions(source.join(path), fs::Permissions::from_mode(mode)).unwrap();
        }
        let destination = skills.join("example");
        symlink("../../.agents/skills/example", &destination).unwrap();
        let plan = f
            .setup
            .preview_adoption_using(fixture_skill_renderer)
            .unwrap();
        assert!(plan.skills.iter().any(
            |skill| skill.path == ".cursor/skills/example" && skill.byte_copy_reason.is_some()
        ));
        f.setup
            .adopt_using(&plan.token, fixture_rule_renderer, fixture_skill_renderer)
            .unwrap();
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o7777;
        for path in ["", "scripts", "scripts/run.sh"] {
            assert_eq!(
                mode(&destination.join(path)),
                mode(&source.join(path)),
                "{path}"
            );
        }
        let restored = f.setup.restore().unwrap();
        assert!(restored.blocked.is_empty(), "{restored:?}");
        assert_eq!(
            fs::read_link(destination).unwrap(),
            Path::new("../../.agents/skills/example")
        );
    }

    #[cfg(unix)]
    #[test]
    fn rulesync_linked_skill_with_internal_symlink_stays_unmanaged() {
        use std::os::unix::fs::symlink;

        let f = Fixture::new();
        let source = f.setup.home.join(".agents/skills/example");
        let skills = f.setup.home.join(".cursor/skills");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&skills).unwrap();
        fs::write(
            source.join("SKILL.md"),
            "---\nname: example\ndescription: Fixture\nversion: 1\n---\n\nBody.\n",
        )
        .unwrap();
        fs::write(source.join("guide.txt"), "Original guide.\n").unwrap();
        symlink("guide.txt", source.join("alias.txt")).unwrap();
        let destination = skills.join("example");
        symlink("../../.agents/skills/example", &destination).unwrap();

        let plan = f
            .setup
            .preview_adoption_using(fixture_skill_renderer)
            .unwrap();
        assert!(plan.skills.iter().any(|skill| {
            skill.path == ".cursor/skills/example"
                && matches!(&skill.outcome, wenlan_types::rulesync::AdoptionSkillOutcome::Skipped { reason } if reason.contains("linked assets"))
        }), "{plan:?}");
        f.setup
            .adopt_using(&plan.token, fixture_rule_renderer, fixture_skill_renderer)
            .unwrap();
        assert_eq!(
            fs::read_link(destination.join("alias.txt")).unwrap(),
            Path::new("guide.txt")
        );
    }

    #[test]
    fn rulesync_added_tool_keeps_rules_written_since_adoption() {
        let f = Fixture::new();
        f.adopt_from_plan();
        let cursor = ".cursor/AGENTS.md";
        let later_rule = "Read the rollout checklist before shipping.\n";
        let harness = format!("{SHARED}{later_rule}");
        f.write(".agents/AGENTS.md", &harness);
        fs::create_dir(f.setup.home.join(".cursor")).unwrap();
        f.write(cursor, &format!("{harness}Cursor only.\n"));

        let plan = f
            .setup
            .preview_adoption_using(fixture_skill_renderer)
            .unwrap();
        f.setup
            .adopt_using(&plan.token, fixture_rule_renderer, fixture_skill_renderer)
            .unwrap();
        assert_eq!(
            f.text(cursor).matches(later_rule).count(),
            1,
            "{}",
            f.text(cursor)
        );
        let report = f.sync_with("");
        assert!(
            report.written.is_empty() && report.blocked.is_empty(),
            "{report:?}"
        );
        assert_eq!(f.text(cursor).matches(later_rule).count(), 1);
        assert!(!f.text(CLAUDE).contains(later_rule));
        assert!(!f.text(CODEX).contains(later_rule));
        assert_eq!(f.text(".agents/AGENTS.md"), harness);
    }

    #[test]
    fn rulesync_large_adopted_skill_proposes_only_the_changed_file() {
        let f = Fixture::new();
        fs::remove_dir_all(f.setup.home.join(".claude")).unwrap();
        fs::remove_dir_all(f.setup.home.join(".codex")).unwrap();
        let folder = f.setup.home.join(".cursor/skills/large-skill");
        fs::create_dir_all(&folder).unwrap();
        let original =
            "---\nname: large-skill\ndescription: Fixture\nversion: 1\n---\n\nOriginal body.\n";
        let edited = original.replace("Original body.", "A reviewed change.");
        fs::write(folder.join("SKILL.md"), original).unwrap();
        for index in 0..502 {
            fs::write(
                folder.join(format!("asset-{index:03}.txt")),
                format!("Asset {index}\n"),
            )
            .unwrap();
        }
        let plan = f
            .setup
            .preview_adoption_using(fixture_skill_renderer)
            .unwrap();
        assert!(plan.skills[0].byte_copy_reason.is_some(), "{plan:?}");
        f.setup
            .adopt_using(&plan.token, fixture_rule_renderer, fixture_skill_renderer)
            .unwrap();

        fs::write(folder.join("SKILL.md"), &edited).unwrap();
        let pending = f.sync_with("");
        assert!(
            pending
                .blocked
                .iter()
                .any(|path| path.contains("need your approval")),
            "{pending:?}"
        );
        {
            let proposal = accept_fixture_tool_proposal(&f).unwrap();
            let [file] = proposal.files.as_slice() else {
                panic!("expected only SKILL.md in the proposal");
            };
            assert_eq!(file.path, "SKILL.md");
            assert!(file.text.as_deref().unwrap().contains("A reviewed change."));
        }
        let delivered = f.sync_with("");
        assert!(delivered.blocked.is_empty(), "{delivered:?}");
        let unchanged = f.sync_with("");
        assert!(
            unchanged.blocked.is_empty() && unchanged.written.is_empty(),
            "{unchanged:?}"
        );
        assert_eq!(fs::read_to_string(folder.join("SKILL.md")).unwrap(), edited);
        assert_eq!(
            fs::read_to_string(folder.join("asset-499.txt")).unwrap(),
            "Asset 499\n"
        );
        assert_eq!(fs::read_dir(&folder).unwrap().count(), 503);
        let restored = f.setup.restore().unwrap();
        assert!(restored.blocked.is_empty(), "{restored:?}");
        assert_eq!(
            fs::read_to_string(folder.join("SKILL.md")).unwrap(),
            original
        );
    }

    fn write_step1d_skill(f: &Fixture, tool_dir: &str, body: &str, asset: &str) -> PathBuf {
        let folder = f.setup.home.join(tool_dir).join("skills/example");
        fs::create_dir_all(&folder).unwrap();
        fs::write(
            folder.join("SKILL.md"),
            format!("---\nname: example\ndescription: Fixture\nversion: 1\n---\n\n{body}\n"),
        )
        .unwrap();
        fs::write(folder.join("helper.py"), asset).unwrap();
        folder
    }

    #[test]
    fn rulesync_step1d_reviews_an_edit_with_adopted_variation_selectors() {
        let f = Fixture::new();
        fs::remove_dir_all(f.setup.home.join(".claude")).unwrap();
        fs::remove_dir_all(f.setup.home.join(".codex")).unwrap();
        let body = "## ⚙\u{fe0f} Setup\nOriginal instructions.\n## ☀\u{fe0f} Daily workflow";
        let asset = format!("# {}\n", "a".repeat(82_782));
        let folder = write_step1d_skill(&f, ".cursor", body, &asset);
        f.adopt_from_plan();

        let entry = folder.join("SKILL.md");
        let original = fs::read_to_string(&entry).unwrap();
        let edited = original.replace("Original instructions.", "Edited instructions.");
        fs::write(&entry, &edited).unwrap();
        let pending = f.sync_with("");
        assert!(
            pending
                .blocked
                .iter()
                .any(|line| line.contains("need your approval")),
            "{pending:?}"
        );
        let proposal = accept_fixture_tool_proposal(&f).unwrap();
        assert_eq!(proposal.files.len(), 1);
        assert_eq!(proposal.files[0].path, "SKILL.md");
        assert!(proposal.files[0]
            .text
            .as_deref()
            .unwrap()
            .contains("Edited instructions."));
        assert!(f.sync_with("").blocked.is_empty());
        assert_eq!(fs::read_to_string(&entry).unwrap(), edited);
        assert_eq!(fs::read_to_string(folder.join("helper.py")).unwrap(), asset);

        fs::write(
            &entry,
            edited.replace("Edited instructions.", "Another edit.\u{fe0f}"),
        )
        .unwrap();
        let blocked = f.sync_with("");
        assert!(
            blocked
                .blocked
                .iter()
                .any(|line| line.contains("invisible")),
            "{blocked:?}"
        );
        assert!(accept_fixture_tool_proposal(&f).is_err());
    }

    #[test]
    fn rulesync_step1d_shows_large_changed_assets_alongside_small_edits() {
        let f = Fixture::new();
        fs::remove_dir_all(f.setup.home.join(".claude")).unwrap();
        fs::remove_dir_all(f.setup.home.join(".codex")).unwrap();
        let asset = format!("# {}\n", "a".repeat(82_782));
        let folder = write_step1d_skill(&f, ".cursor", "Original instructions.", &asset);
        f.adopt_from_plan();

        let entry = folder.join("SKILL.md");
        let edited = fs::read_to_string(&entry)
            .unwrap()
            .replace("Original instructions.", "Edited instructions.");
        fs::write(&entry, &edited).unwrap();
        let edited_asset = format!("{asset}# A change to the long script\n");
        fs::write(folder.join("helper.py"), &edited_asset).unwrap();
        let pending = f.sync_with("");
        assert!(
            pending
                .blocked
                .iter()
                .any(|line| line.contains("need your approval")),
            "{pending:?}"
        );
        let proposal = accept_fixture_tool_proposal(&f).unwrap();
        assert!(proposal
            .files
            .iter()
            .any(|file| file.path == "SKILL.md" && file.text.as_deref() == Some(&edited)));
        let changes = proposal.changes.unwrap();
        let [large] = changes.oversized.as_slice() else {
            panic!("expected one oversized asset: {changes:?}");
        };
        use sha2::{Digest, Sha256};
        assert_eq!(large.path, "helper.py");
        let before = large.before.as_ref().unwrap();
        assert_eq!(before.bytes, asset.len());
        assert_eq!(before.sha256, format!("{:x}", Sha256::digest(asset)));
        assert_eq!(large.after.bytes, edited_asset.len());
        assert_eq!(
            large.after.sha256,
            format!("{:x}", Sha256::digest(&edited_asset))
        );
        assert!(f.sync_with("").blocked.is_empty());
        assert_eq!(
            fs::read_to_string(folder.join("helper.py")).unwrap(),
            edited_asset
        );
    }

    #[cfg(unix)]
    #[test]
    fn rulesync_step1d_reports_an_existing_directory_mode_mismatch() {
        use std::os::unix::fs::PermissionsExt as _;

        let f = Fixture::new();
        let source = write_step1d_skill(&f, ".claude", "Original body.", "# Helper\n");
        let destination = write_step1d_skill(&f, ".codex", "Original body.", "# Helper\n");
        fs::set_permissions(&source, fs::Permissions::from_mode(0o750)).unwrap();
        fs::set_permissions(&destination, fs::Permissions::from_mode(0o755)).unwrap();
        f.adopt_from_plan();
        let file = source.join("SKILL.md");
        let edited = fs::read_to_string(&file)
            .unwrap()
            .replace("Original body.", "Edited body.");
        fs::write(file, &edited).unwrap();
        f.sync_with("");
        accept_fixture_tool_proposal(&f).unwrap();
        let synced = f.sync_with("");
        let mode = fs::metadata(&destination).unwrap().permissions().mode() & 0o7777;
        assert!(
            mode == 0o750
                || synced
                    .blocked
                    .iter()
                    .any(|line| line.contains(".codex/skills/example")
                        && line.contains("permissions")),
            "mode {mode:o}, {synced:?}"
        );
        assert_eq!(
            fs::read_to_string(destination.join("SKILL.md")).unwrap(),
            edited
        );
    }

    #[cfg(unix)]
    #[test]
    fn rulesync_step1g_shared_content_edit_keeps_the_peers_file_mode() {
        use std::os::unix::fs::PermissionsExt as _;

        let f = Fixture::new();
        let source = write_step1d_skill(&f, ".claude", "Original body.", "# Helper\n");
        let peer = write_step1d_skill(&f, ".codex", "Original body.", "# Helper\n");
        let source_asset = source.join("helper.py");
        let peer_asset = peer.join("helper.py");
        fs::set_permissions(&source_asset, fs::Permissions::from_mode(0o666)).unwrap();
        fs::set_permissions(&peer_asset, fs::Permissions::from_mode(0o600)).unwrap();
        for (folder, mode) in [(&source, 0o666), (&peer, 0o600)] {
            let untouched = folder.join("untouched.txt");
            fs::write(&untouched, b"Same original contents\n").unwrap();
            fs::set_permissions(untouched, fs::Permissions::from_mode(mode)).unwrap();
        }
        f.adopt_from_plan();
        allow_fixture_tool_folders(&f);
        let edited = b"# Updated helper\n";
        fs::write(&source_asset, edited).unwrap();
        let preview = share_step1e_skill(&f);
        let change = preview
            .files
            .iter()
            .find(|change| Path::new(&change.path) == Path::new(".codex/skills/example/helper.py"))
            .unwrap();
        assert_eq!(change.before.as_ref().unwrap().permissions, "0600");
        assert_eq!(change.after.as_ref().unwrap().permissions, "0600");
        assert!(!preview
            .files
            .iter()
            .any(|change| change.path.ends_with("untouched.txt")));
        assert!(f.sync_with("").blocked.is_empty());
        assert_eq!(fs::read(&peer_asset).unwrap(), edited);
        for path in [peer_asset, peer.join("untouched.txt")] {
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o7777,
                0o600
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn rulesync_step1d_shared_asset_mode_only_edit_is_reviewable() {
        use std::os::unix::fs::PermissionsExt as _;

        let f = Fixture::new();
        let source = write_step1d_skill(&f, ".claude", "Original body.", "# Helper\n");
        let destination = write_step1d_skill(&f, ".codex", "Original body.", "# Helper\n");
        let source_asset = source.join("helper.py");
        let destination_asset = destination.join("helper.py");
        for asset in [&source_asset, &destination_asset] {
            fs::set_permissions(asset, fs::Permissions::from_mode(0o644)).unwrap();
        }
        f.adopt_from_plan();
        fs::set_permissions(&source_asset, fs::Permissions::from_mode(0o755)).unwrap();

        let pending = f.sync_with("");
        assert!(
            pending
                .blocked
                .iter()
                .any(|line| line.contains("need your approval")),
            "{pending:?}"
        );
        let proposal = accept_fixture_tool_proposal(&f).unwrap();
        assert!(proposal.files.iter().any(|file| file.path == "helper.py"));
        let synced = f.sync_with("");
        assert!(synced.blocked.is_empty(), "{synced:?}");
        assert_eq!(
            fs::metadata(&destination_asset)
                .unwrap()
                .permissions()
                .mode()
                & 0o7777,
            0o755
        );
    }

    #[test]
    fn rulesync_step1d_restores_an_adopted_asset_after_an_approved_deletion() {
        let f = Fixture::new();
        fs::remove_dir_all(f.setup.home.join(".codex")).unwrap();
        let claude = write_step1d_skill(&f, ".claude", "Original body.", "# Claude helper\n");
        let cursor = write_step1d_skill(&f, ".cursor", "Original body.", "# Cursor helper\n");
        let claude_asset = claude.join("helper.py");
        let cursor_asset = cursor.join("helper.py");
        f.adopt_from_plan();
        fs::remove_file(&cursor_asset).unwrap();
        f.sync_with("");
        let proposal = accept_fixture_tool_proposal(&f).unwrap();
        assert_eq!(proposal.changes.unwrap().removed, ["helper.py"]);
        let delivered = f.sync_with("");
        assert!(delivered.blocked.is_empty(), "{delivered:?}");
        let deleted_in_both_tools = !claude_asset.exists() && !cursor_asset.exists();

        let restored = f.setup.restore().unwrap();
        assert!(restored.blocked.is_empty(), "{restored:?}");
        assert!(deleted_in_both_tools);
        assert_eq!(
            fs::read_to_string(claude_asset).unwrap(),
            "# Claude helper\n"
        );
        assert_eq!(
            fs::read_to_string(cursor_asset).unwrap(),
            "# Cursor helper\n"
        );
    }

    #[test]
    fn rulesync_step1d_shared_preview_names_a_differing_tool_variant_it_overwrites() {
        use wenlan_types::rulesync_skill::SaveSkillResult;

        let f = Fixture::new();
        write_step1d_skill(&f, ".codex", "Original body.", "# Original helper\n");
        let claude = write_step1d_skill(&f, ".claude", "Original body.", "# Original helper\n");
        let cursor = write_step1d_skill(&f, ".cursor", "Original body.", "# Original helper\n");
        f.adopt_from_plan();
        let other = claude.join("helper.py");
        fs::write(&other, "# Saved Claude-only helper\n").unwrap();
        f.sync_with("");
        rulesync_registry::accept_only_tool_proposal(&f.setup, None, fixture_skill_renderer)
            .unwrap();
        let initial = f.sync_with("");
        assert!(initial.blocked.is_empty(), "{initial:?}");
        assert_eq!(
            fs::read_to_string(&other).unwrap(),
            "# Saved Claude-only helper\n"
        );

        let entry = cursor.join("SKILL.md");
        let edited = fs::read_to_string(&entry)
            .unwrap()
            .replace("Original body.", "Cursor edit.");
        fs::write(&entry, &edited).unwrap();
        fs::write(cursor.join("new-notes.txt"), "New asset.\n").unwrap();
        f.sync_with("");
        let review = |token| {
            rulesync_registry::review_only_tool_proposal(&f.setup, token, fixture_skill_renderer)
        };
        let SaveSkillResult::ReviewPreview { proposal, delivery } = review(None).unwrap() else {
            panic!("expected a shared review preview");
        };
        assert!(!proposal.files.iter().any(|file| file.path == "helper.py"));
        assert!(delivery.blocked.is_empty(), "{delivery:?}");
        assert_eq!(delivery.cleared_overrides, [SavedSkillTool::ClaudeCode]);
        let planned: BTreeSet<_> = delivery
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect();
        assert_eq!(
            planned,
            BTreeSet::from([
                ".claude/skills/example/SKILL.md",
                ".claude/skills/example/helper.py",
                ".claude/skills/example/new-notes.txt",
                ".codex/skills/example/SKILL.md",
                ".codex/skills/example/new-notes.txt",
            ])
        );
        assert_eq!(
            fs::read_to_string(&other).unwrap(),
            "# Saved Claude-only helper\n"
        );
        assert!(!claude.join("new-notes.txt").exists());

        // The same proposal id must not authorize a changed set of destinations.
        fs::write(&other, "# Another Claude edit during review\n").unwrap();
        assert!(review(Some(&delivery.token)).is_err());
        assert_eq!(
            fs::read_to_string(&other).unwrap(),
            "# Another Claude edit during review\n"
        );
        assert!(!claude.join("new-notes.txt").exists());
        fs::write(&other, "# Saved Claude-only helper\n").unwrap();

        let SaveSkillResult::Saved {
            delivery: Some(delivered),
            ..
        } = review(Some(&delivery.token)).unwrap()
        else {
            panic!("expected a saved skill");
        };
        assert!(delivered.blocked.is_empty(), "{delivered:?}");
        let written: BTreeSet<_> = delivered
            .written
            .iter()
            .map(|path| path.replace(std::path::MAIN_SEPARATOR, "/"))
            .collect();
        assert_eq!(written, planned.into_iter().map(str::to_owned).collect());
        assert_eq!(fs::read_to_string(&other).unwrap(), "# Original helper\n");
        assert_eq!(
            fs::read_to_string(claude.join("new-notes.txt")).unwrap(),
            "New asset.\n"
        );
        assert_eq!(f.text(".codex/skills/example/SKILL.md"), edited);
    }

    fn accept_fixture_tool_proposal(
        f: &Fixture,
    ) -> Result<wenlan_types::rulesync_skill::SkillProposal> {
        rulesync_registry::accept_only_tool_proposal(
            &f.setup,
            Some(wenlan_types::rulesync_skill::SavedSkillScope::Shared),
            fixture_skill_renderer,
        )
    }

    fn preview_step1e_skill(f: &Fixture) -> wenlan_types::rulesync_skill::SkillDeliveryPreview {
        use wenlan_types::rulesync_skill::SaveSkillResult;

        let SaveSkillResult::ReviewPreview { delivery, .. } =
            rulesync_registry::review_only_tool_proposal(&f.setup, None, fixture_skill_renderer)
                .unwrap()
        else {
            panic!("expected a skill preview");
        };
        delivery
    }

    fn accept_step1e_skill(
        f: &Fixture,
        preview: &wenlan_types::rulesync_skill::SkillDeliveryPreview,
    ) -> RulesSyncReport {
        use wenlan_types::rulesync_skill::SaveSkillResult;

        let SaveSkillResult::Saved {
            delivery: Some(report),
            ..
        } = rulesync_registry::review_only_tool_proposal(
            &f.setup,
            Some(&preview.token),
            fixture_skill_renderer,
        )
        .unwrap()
        else {
            panic!("expected an approved delivery");
        };
        report
    }

    fn share_step1e_skill(f: &Fixture) -> wenlan_types::rulesync_skill::SkillDeliveryPreview {
        f.sync_with("");
        let preview = preview_step1e_skill(f);
        assert!(preview.blocked.is_empty(), "{preview:?}");
        let before = step1e_tree(&f.setup.home);
        let accepted = accept_step1e_skill(f, &preview);
        assert!(accepted.blocked.is_empty(), "{accepted:?}");
        let after = step1e_tree(&f.setup.home);
        let changed: BTreeSet<_> = before
            .keys()
            .chain(after.keys())
            .filter(|path| before.get(*path) != after.get(*path))
            .cloned()
            .collect();
        let planned: BTreeSet<_> = preview
            .files
            .iter()
            .flat_map(|change| std::iter::once(&change.path).chain(&change.renamed_from))
            .map(PathBuf::from)
            .collect();
        assert_eq!(changed, planned);
        preview
    }

    // Fake homes are inside the repository. Give these fixtures the
    // same destination access as a tool home outside a Git worktree.
    fn allow_fixture_tool_folders(f: &Fixture) {
        let _lock = f.setup.lock().unwrap();
        let mut manifest = f.setup.load().unwrap().unwrap();
        let mut worktrees = DeliveryWorktrees::new(&f.setup.home).unwrap();
        for target in PERSONAL_TARGETS {
            if let Some(worktree) = worktrees.at(target.folder()).unwrap() {
                assert!(f._temp.path().starts_with(&worktree));
                manifest
                    .worktree_writes
                    .insert(target.folder().to_owned(), worktree);
            }
        }
        f.setup.save(&manifest).unwrap();
    }

    #[test]
    fn rulesync_step1f_rejects_saved_approvals_without_recipients() {
        let f = Fixture::new();
        fs::remove_dir_all(f.setup.home.join(".codex")).unwrap();
        write_step1d_skill(&f, ".claude", "Original body.", "# Original helper\n");
        f.adopt_from_plan();
        allow_fixture_tool_folders(&f);
        f.write(".claude/skills/example/helper.py", "# Reviewed helper\n");
        share_step1e_skill(&f);

        fs::create_dir(f.setup.home.join(".cursor")).unwrap();
        f.write(".cursor/AGENTS.md", "Cursor's rules.\n");
        let plan = f
            .setup
            .preview_adoption_using(fixture_skill_renderer)
            .unwrap();
        f.setup
            .adopt_using(&plan.token, fixture_rule_renderer, fixture_skill_renderer)
            .unwrap();
        let state = f.setup.data.join("skills.json");
        let mut stored: serde_json::Value =
            serde_json::from_slice(&fs::read(&state).unwrap()).unwrap();
        stored["state"]["kept"]["example"]
            .as_object_mut()
            .unwrap()
            .remove("shared_reviewed");
        fs::write(&state, serde_json::to_vec(&stored).unwrap()).unwrap();
        let before = step1e_tree(&f.setup.home);

        let root = FileRoot::open_home(&f.setup.home).unwrap();
        let _lock = f.setup.lock().unwrap();
        let mut manifest = f.setup.load().unwrap().unwrap();
        let sync = f.sync_review(&root, &mut manifest);
        let received = f
            .setup
            .home
            .join(".cursor/skills/example/SKILL.md")
            .exists();
        assert!(
            !received,
            "sync delivered a skill from an older approval to Cursor: {sync:?}"
        );
        let message = format!("{:#}", sync.unwrap_err());
        assert!(message.contains("wenlan rules restore"), "{message}");
        assert!(message.contains("adopt again"), "{message}");
        assert_eq!(step1e_tree(&f.setup.home), before);
    }

    #[test]
    fn rulesync_step1f_restores_adoption_tree_after_a_new_skill_is_renamed() {
        let f = Fixture::new();
        fs::remove_dir_all(f.setup.home.join(".codex")).unwrap();
        fs::create_dir(f.setup.home.join(".claude/skills")).unwrap();
        let before = step1e_tree(&f.setup.home);
        f.adopt_from_plan();
        allow_fixture_tool_folders(&f);
        let skill = write_step1d_skill(&f, ".claude", "New skill.", "# New helper\n");
        share_step1e_skill(&f);
        fs::rename(skill.join("helper.py"), skill.join("renamed.py")).unwrap();
        share_step1e_skill(&f);

        let restored = f.setup.restore().unwrap();
        assert!(restored.blocked.is_empty(), "{restored:?}");
        assert_eq!(step1e_tree(&f.setup.home), before);
    }

    #[cfg(unix)]
    #[test]
    fn rulesync_step1f_restores_contents_before_read_only_directory_modes() {
        use std::os::unix::fs::PermissionsExt;

        let f = Fixture::new();
        fs::remove_dir_all(f.setup.home.join(".codex")).unwrap();
        let skill = write_step1d_skill(&f, ".claude", "Original body.", "# Helper\n");
        let docs = skill.join("docs");
        let nested = docs.join("nested");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("guide.md"), b"Original guide\n").unwrap();
        fs::set_permissions(&nested, fs::Permissions::from_mode(0o551)).unwrap();
        fs::set_permissions(&docs, fs::Permissions::from_mode(0o555)).unwrap();
        let before = step1e_tree(&f.setup.home);
        f.adopt_from_plan();
        allow_fixture_tool_folders(&f);
        for path in [&docs, &nested] {
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        fs::remove_dir_all(&docs).unwrap();
        share_step1e_skill(&f);

        let restored = f.setup.restore();
        let after = step1e_tree(&f.setup.home);
        for path in [&docs, &nested] {
            if path.exists() {
                fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        let restored = restored.unwrap();
        assert!(restored.blocked.is_empty(), "{restored:?}");
        assert_eq!(after, before);
    }

    #[cfg(unix)]
    #[test]
    fn rulesync_step1f_retries_restore_below_an_originally_read_only_directory() {
        use std::os::unix::fs::PermissionsExt;

        let f = Fixture::new();
        fs::remove_dir_all(f.setup.home.join(".codex")).unwrap();
        let skill = write_step1d_skill(&f, ".claude", "Original body.", "# Helper\n");
        let docs = skill.join("docs");
        let guide = docs.join("guide.md");
        fs::create_dir(&docs).unwrap();
        fs::write(&guide, b"Original guide\n").unwrap();
        fs::set_permissions(&docs, fs::Permissions::from_mode(0o555)).unwrap();
        let before = step1e_tree(&f.setup.home);
        f.adopt_from_plan();
        allow_fixture_tool_folders(&f);
        fs::set_permissions(&docs, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(&guide, b"Reviewed guide\n").unwrap();
        share_step1e_skill(&f);
        fs::write(&guide, b"An edit that still needs attention\n").unwrap();

        let first = f.setup.restore().unwrap();
        assert!(!first.blocked.is_empty(), "{first:?}");
        assert_eq!(
            fs::read(&guide).unwrap(),
            b"An edit that still needs attention\n"
        );

        fs::write(&guide, b"Reviewed guide\n").unwrap();
        let restored = f.setup.restore();
        let after = step1e_tree(&f.setup.home);
        fs::set_permissions(&docs, fs::Permissions::from_mode(0o755)).unwrap();
        let restored = restored.unwrap();
        assert!(restored.blocked.is_empty(), "{restored:?}");
        assert_eq!(after, before);
    }

    #[test]
    fn rulesync_step1g_restore_keeps_a_persons_directory_deletion() {
        let f = Fixture::new();
        fs::remove_dir_all(f.setup.home.join(".codex")).unwrap();
        let folder = write_step1d_skill(&f, ".claude", "Original body.", "# Helper\n");
        let docs = folder.join("docs");
        let guide = docs.join("guide.md");
        fs::create_dir(&docs).unwrap();
        fs::write(&guide, b"Original guide\n").unwrap();
        f.adopt_from_plan();
        allow_fixture_tool_folders(&f);
        fs::remove_file(&guide).unwrap();
        share_step1e_skill(&f);
        assert!(docs.is_dir(), "the approval removed the guide only");
        fs::remove_dir(&docs).unwrap();

        let restored = f.setup.restore().unwrap();

        assert!(
            !docs.exists(),
            "restore recreated the directory the person deleted"
        );
        for path in [&docs, &guide] {
            let relative = path.strip_prefix(&f.setup.home).unwrap();
            assert!(
                restored
                    .blocked
                    .iter()
                    .any(|message| message.starts_with(&relative.display().to_string())),
                "{restored:?}"
            );
        }
        let manifest = f.setup.load().unwrap().expect("backup retained");
        assert!(manifest
            .delivered
            .contains_key(Path::new(".claude/skills/example/docs")));
        assert!(manifest
            .delivered
            .contains_key(Path::new(".claude/skills/example/docs/guide.md")));
    }

    #[test]
    fn rulesync_step1f_readoption_reoffers_the_saved_skill() {
        let f = Fixture::new();
        fs::remove_dir_all(f.setup.home.join(".codex")).unwrap();
        fs::create_dir(f.setup.home.join(".cursor")).unwrap();
        f.write(".cursor/AGENTS.md", "Cursor's rules.\n");
        f.adopt_from_plan();
        allow_fixture_tool_folders(&f);
        write_step1d_skill(&f, ".claude", "Saved skill.", "# Saved helper\n");
        let peer = f.setup.home.join(".cursor/skills/example/SKILL.md");
        share_step1e_skill(&f);
        assert!(peer.exists());

        let restored = f.setup.restore().unwrap();
        assert!(restored.blocked.is_empty(), "{restored:?}");
        assert!(!peer.exists());
        // Re-enroll just the peer to show that the retained shared source
        // can produce another delivery even without its original tool.
        fs::remove_dir_all(f.setup.home.join(".claude")).unwrap();
        f.adopt_from_plan();
        allow_fixture_tool_folders(&f);
        let sync = f.sync_with("");
        assert!(
            !peer.exists(),
            "the saved skill needs a new delivery review"
        );
        assert!(
            sync.blocked.iter().any(|message| {
                message.contains(".cursor") && message.contains("wenlan skills pending")
            }),
            "The saved skill must be offered to Cursor again: {sync:?}"
        );
        let preview = preview_step1e_skill(&f);
        assert!(!preview.files.is_empty(), "{preview:?}");
        assert!(preview
            .files
            .iter()
            .all(|change| change.path.starts_with(".cursor/")));
        accept_step1e_skill(&f, &preview);
        assert!(fs::read_to_string(peer).unwrap().contains("Saved skill."));
        assert!(f.sync_with("").blocked.is_empty());
    }

    #[test]
    fn rulesync_step1h_restore_checks_renamed_parents_of_missing_files() {
        let f = Fixture::new();
        fs::remove_dir_all(f.setup.home.join(".codex")).unwrap();
        let skill = write_step1d_skill(&f, ".claude", "Original body.", "# Helper\n");
        let docs = skill.join("docs");
        let renamed = skill.join("Docs");
        let guide = docs.join("guide.md");
        fs::create_dir(&docs).unwrap();
        fs::write(&guide, b"Original guide\n").unwrap();
        f.adopt_from_plan();
        allow_fixture_tool_folders(&f);
        fs::remove_file(&guide).unwrap();
        share_step1e_skill(&f);
        fs::rename(&docs, &renamed).unwrap();
        let aliases = docs.is_dir();
        eprintln!("The old parent spelling resolves to the renamed directory: {aliases}");

        let restored = f.setup.restore().unwrap();

        assert!(renamed.is_dir());
        assert!(!renamed.join("guide.md").exists(), "{restored:?}");
        let relative = guide.strip_prefix(&f.setup.home).unwrap();
        let reason = if aliases { "spelling" } else { "missing" };
        assert!(
            restored.blocked.iter().any(|message| {
                message.starts_with(&relative.display().to_string()) && message.contains(reason)
            }),
            "{restored:?}"
        );
        let manifest = f.setup.load().unwrap().expect("backup retained");
        assert!(manifest.delivered.contains_key(relative));
        assert!(manifest.delivered.contains_key(relative.parent().unwrap()));
    }

    #[test]
    fn rulesync_step1g_restore_keeps_an_unreviewed_case_only_rename() {
        let f = Fixture::new();
        fs::remove_dir_all(f.setup.home.join(".codex")).unwrap();
        let skill = write_step1d_skill(&f, ".claude", "Original body.", "# Helper\n");
        let old = skill.join("guide.md");
        let renamed = skill.join("Guide.md");
        fs::write(&old, b"Original guide\n").unwrap();
        f.adopt_from_plan();
        allow_fixture_tool_folders(&f);
        fs::write(&old, b"Reviewed guide\n").unwrap();
        share_step1e_skill(&f);
        fs::rename(&old, &renamed).unwrap();
        eprintln!(
            "The old spelling resolves to the renamed entry: {}",
            old.is_file()
        );

        let restored = f.setup.restore().unwrap();

        assert_eq!(fs::read(&renamed).unwrap(), b"Reviewed guide\n");
        let entries = fs::read_dir(skill)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<BTreeSet<_>>();
        assert!(entries.contains(std::ffi::OsStr::new("Guide.md")));
        assert!(!entries.contains(std::ffi::OsStr::new("guide.md")));
        assert!(
            restored
                .blocked
                .iter()
                .any(|reason| reason.contains("guide.md")),
            "{restored:?}"
        );
        assert!(f.setup.load().unwrap().is_some());
    }

    #[test]
    fn rulesync_step1f_approves_case_only_asset_renames_and_restores_the_spelling() {
        let f = Fixture::new();
        fs::remove_dir_all(f.setup.home.join(".codex")).unwrap();
        let claude = write_step1d_skill(&f, ".claude", "Original body.", "# Helper\n");
        let cursor = write_step1d_skill(&f, ".cursor", "Original body.", "# Helper\n");
        // APFS also aliases the two lowercase Greek sigmas, which stay
        // distinct when Rust merely lowercases their spellings.
        let renames = [("guide.md", "Guide.md"), ("σ.md", "ς.md")];
        let mut aliases = BTreeMap::new();
        for (old, new) in renames {
            for folder in [&claude, &cursor] {
                fs::write(folder.join(old), b"Original guide\n").unwrap();
            }
            aliases.insert(old, claude.join(new).is_file());
            eprintln!("Fixture sees {old} and {new} as aliases: {}", aliases[old]);
        }
        let before = step1e_tree(&f.setup.home);
        f.adopt_from_plan();
        allow_fixture_tool_folders(&f);
        for (old, new) in renames {
            fs::rename(cursor.join(old), cursor.join(new)).unwrap();
            fs::write(cursor.join(new), b"Revised guide\n").unwrap();
        }
        f.sync_with("");
        let preview = preview_step1e_skill(&f);
        assert!(preview.blocked.is_empty(), "{preview:?}");
        let peer = Path::new(".claude/skills/example");
        for (old, new) in renames {
            let old_path = peer.join(old);
            let new_path = peer.join(new);
            let change = preview
                .files
                .iter()
                .find(|change| Path::new(&change.path) == new_path)
                .unwrap();
            assert_eq!(
                change.renamed_from.as_deref().map(Path::new),
                aliases[old].then_some(old_path.as_path())
            );
            assert_eq!(
                preview
                    .files
                    .iter()
                    .any(|change| Path::new(&change.path) == old_path),
                !aliases[old],
                "{preview:?}"
            );
        }
        let accepted = accept_step1e_skill(&f, &preview);
        assert!(accepted.blocked.is_empty(), "{accepted:?}");
        let after = step1e_tree(&f.setup.home);
        for (old, new) in renames {
            assert!(after.contains_key(&peer.join(new)), "{preview:?}");
            assert!(!after.contains_key(&peer.join(old)), "{preview:?}");
            assert_eq!(fs::read(claude.join(new)).unwrap(), b"Revised guide\n");
        }
        let synced = f.sync_with("");
        assert!(
            synced.blocked.is_empty() && synced.written.is_empty(),
            "{synced:?}"
        );

        let restored = f.setup.restore().unwrap();
        assert!(restored.blocked.is_empty(), "{restored:?}");
        assert_eq!(step1e_tree(&f.setup.home), before);
    }

    #[test]
    fn rulesync_step1e_reverting_a_blocked_peer_cannot_expand_an_approval() {
        let f = Fixture::new();
        fs::remove_dir_all(f.setup.home.join(".codex")).unwrap();
        let claude = write_step1d_skill(&f, ".claude", "Original body.", "# Original helper\n");
        let cursor = write_step1d_skill(&f, ".cursor", "Original body.", "# Original helper\n");
        f.adopt_from_plan();
        fs::write(claude.join("helper.py"), "# Claude-only helper\n").unwrap();
        f.sync_with("");
        rulesync_registry::accept_only_tool_proposal(&f.setup, None, fixture_skill_renderer)
            .unwrap();
        assert!(f.sync_with("").blocked.is_empty());

        fs::write(cursor.join("helper.py"), "# Cursor shared helper\n").unwrap();
        f.sync_with("");
        // An unrelated edit after proposal creation blocks all Claude delivery.
        let unrelated = claude.join("unrelated.txt");
        fs::write(&unrelated, "A local draft\n").unwrap();
        let preview = preview_step1e_skill(&f);
        assert!(!preview.blocked.is_empty(), "{preview:?}");
        assert!(preview
            .files
            .iter()
            .all(|change| !change.path.starts_with(".claude/")));
        let accepted = rulesync_registry::review_only_tool_proposal(
            &f.setup,
            Some(&preview.token),
            fixture_skill_renderer,
        );
        assert!(accepted.is_err(), "{accepted:?}");
        fs::remove_file(unrelated).unwrap();
        f.sync_with("");
        assert_eq!(
            fs::read_to_string(claude.join("helper.py")).unwrap(),
            "# Claude-only helper\n",
            "{accepted:?}"
        );
        let current = preview_step1e_skill(&f);
        assert_ne!(current.token, preview.token);
        assert!(rulesync_registry::review_only_tool_proposal(
            &f.setup,
            Some(&preview.token),
            fixture_skill_renderer,
        )
        .is_err());
        assert!(current
            .files
            .iter()
            .any(|change| change.path == ".claude/skills/example/helper.py"));
    }

    #[test]
    fn rulesync_step1e_restores_a_removed_populated_directory() {
        let f = Fixture::new();
        fs::remove_dir_all(f.setup.home.join(".codex")).unwrap();
        let claude = write_step1d_skill(&f, ".claude", "Original body.", "# Helper\n");
        let cursor = write_step1d_skill(&f, ".cursor", "Original body.", "# Helper\n");
        for folder in [&claude, &cursor] {
            fs::create_dir(folder.join("docs")).unwrap();
            fs::write(folder.join("docs/guide.md"), b"Original guide\n").unwrap();
        }
        let before = step1e_tree(&f.setup.home);
        f.adopt_from_plan();
        fs::remove_dir_all(cursor.join("docs")).unwrap();
        let preview = share_step1e_skill(&f);
        assert!(preview.files.iter().any(|change| {
            change.path == ".claude/skills/example/docs"
                && change.before.as_ref().is_some_and(|entry| entry.directory)
                && change.after.is_none()
        }));
        let peer_directory_was_removed = !claude.join("docs").exists();
        let restored = f.setup.restore().unwrap();
        assert!(restored.blocked.is_empty(), "{restored:?}");
        assert_eq!(step1e_tree(&f.setup.home), before);
        assert!(peer_directory_was_removed);
    }

    #[test]
    fn rulesync_step1e_restores_after_readding_a_deleted_asset() {
        let f = Fixture::new();
        fs::remove_dir_all(f.setup.home.join(".codex")).unwrap();
        write_step1d_skill(&f, ".claude", "Original body.", "# Claude helper\n");
        let cursor = write_step1d_skill(&f, ".cursor", "Original body.", "# Cursor helper\n");
        let before = step1e_tree(&f.setup.home);
        f.adopt_from_plan();
        let asset = cursor.join("helper.py");
        fs::remove_file(&asset).unwrap();
        share_step1e_skill(&f);
        fs::write(&asset, "# Re-added helper\n").unwrap();
        share_step1e_skill(&f);
        let restored = f.setup.restore().unwrap();
        assert!(restored.blocked.is_empty(), "{restored:?}");
        assert_eq!(step1e_tree(&f.setup.home), before);
    }

    #[test]
    fn rulesync_step1e_restores_the_old_name_after_an_approved_rename() {
        let f = Fixture::new();
        fs::remove_dir_all(f.setup.home.join(".codex")).unwrap();
        write_step1d_skill(&f, ".claude", "Original body.", "# Claude helper\n");
        let cursor = write_step1d_skill(&f, ".cursor", "Original body.", "# Cursor helper\n");
        let before = step1e_tree(&f.setup.home);
        f.adopt_from_plan();
        fs::rename(cursor.join("helper.py"), cursor.join("renamed.py")).unwrap();
        share_step1e_skill(&f);
        let restored = f.setup.restore().unwrap();
        assert!(restored.blocked.is_empty(), "{restored:?}");
        assert_eq!(
            step1e_tree(&f.setup.home).keys().collect::<Vec<_>>(),
            before.keys().collect::<Vec<_>>()
        );
        assert_eq!(step1e_tree(&f.setup.home), before);
    }

    #[test]
    fn rulesync_step1e_deletes_a_peer_asset_missing_from_its_old_snapshot() {
        let f = Fixture::new();
        fs::remove_dir_all(f.setup.home.join(".codex")).unwrap();
        let claude = write_step1d_skill(&f, ".claude", "Original body.", "# Original helper\n");
        let cursor = write_step1d_skill(&f, ".cursor", "Original body.", "# Original helper\n");
        f.adopt_from_plan();
        let common = "# Same approved helper\n";
        fs::write(claude.join("helper.py"), common).unwrap();
        f.sync_with("");
        rulesync_registry::accept_only_tool_proposal(&f.setup, None, fixture_skill_renderer)
            .unwrap();
        assert!(f.sync_with("").blocked.is_empty());
        fs::write(cursor.join("helper.py"), common).unwrap();
        fs::write(cursor.join("added.txt"), "Extra asset\n").unwrap();
        share_step1e_skill(&f);
        let peer_asset = claude.join("added.txt");
        assert!(peer_asset.is_file());

        fs::remove_file(cursor.join("added.txt")).unwrap();
        f.sync_with("");
        let preview = preview_step1e_skill(&f);
        let accepted = accept_step1e_skill(&f, &preview);
        assert!(accepted.blocked.is_empty(), "{preview:?}\n{accepted:?}");
        assert!(preview
            .files
            .iter()
            .any(|change| change.path == ".claude/skills/example/added.txt"));
        assert!(!peer_asset.exists());
        assert!(f.sync_with("").blocked.is_empty());
    }

    #[test]
    fn rulesync_step1e_sync_cannot_extend_a_review_to_a_new_tool() {
        let f = Fixture::new();
        fs::remove_dir_all(f.setup.home.join(".codex")).unwrap();
        write_step1d_skill(&f, ".claude", "Original body.", "# Claude helper\n");
        let cursor = write_step1d_skill(&f, ".cursor", "Original body.", "# Cursor helper\n");
        f.adopt_from_plan();
        fs::write(cursor.join("helper.py"), "# Reviewed helper\n").unwrap();
        let preview = share_step1e_skill(&f);
        assert!(preview
            .files
            .iter()
            .all(|path| !path.path.starts_with(".codex/")));

        let codex = write_step1d_skill(&f, ".codex", "Original body.", "# New tool's helper\n");
        let plan = f
            .setup
            .preview_adoption_using(fixture_skill_renderer)
            .unwrap();
        f.setup
            .adopt_using(&plan.token, fixture_rule_renderer, fixture_skill_renderer)
            .unwrap();
        let synced = f.sync_with("");
        assert!(synced.written.is_empty(), "{synced:?}");
        assert!(
            synced
                .blocked
                .iter()
                .any(|line| line.contains("wenlan skills pending")),
            "{synced:?}"
        );
        assert_eq!(
            fs::read(codex.join("helper.py")).unwrap(),
            b"# New tool's helper\n"
        );
        let next = preview_step1e_skill(&f);
        assert!(
            next.blocked.is_empty() && !next.files.is_empty(),
            "{next:?}"
        );
        assert!(next
            .files
            .iter()
            .all(|change| change.path.starts_with(".codex/")));
        accept_step1e_skill(&f, &next);
        assert_eq!(
            fs::read(codex.join("helper.py")).unwrap(),
            b"# Reviewed helper\n"
        );
        assert!(f.sync_with("").written.is_empty());
    }

    #[test]
    fn rulesync_step1e_crash_during_skill_review_helper() {
        let Some(root) = std::env::var_os("WENLAN_TEST_SKILL_REVIEW_ROOT").map(PathBuf::from)
        else {
            return;
        };
        let setup = Setup {
            home: root.join("home"),
            data: root.join("data"),
            binary: "unused".into(),
        };
        let token = std::env::var("WENLAN_TEST_SKILL_REVIEW_TOKEN").unwrap();
        rulesync_registry::review_only_tool_proposal(&setup, Some(&token), fixture_skill_renderer)
            .unwrap();
        panic!("the injected process exit was not reached");
    }

    #[test]
    fn rulesync_step1e_interrupted_skill_review_keeps_the_previous_source_frozen() {
        let f = Fixture::new();
        fs::remove_dir_all(f.setup.home.join(".codex")).unwrap();
        let claude = write_step1d_skill(&f, ".claude", "Original body.", "# Original helper\n");
        let cursor = write_step1d_skill(&f, ".cursor", "Original body.", "# Original helper\n");
        f.adopt_from_plan();
        fs::write(claude.join("helper.py"), "# Claude-only helper\n").unwrap();
        f.sync_with("");
        rulesync_registry::accept_only_tool_proposal(&f.setup, None, fixture_skill_renderer)
            .unwrap();
        assert!(f.sync_with("").blocked.is_empty());

        let edited = fs::read_to_string(cursor.join("SKILL.md"))
            .unwrap()
            .replace("Original body.", "Reviewed body.");
        fs::write(cursor.join("SKILL.md"), &edited).unwrap();
        fs::write(cursor.join("helper.py"), "# Reviewed helper\n").unwrap();
        f.sync_with("");
        let interrupt = |token: &str, point: &str| {
            let child = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "rulesync_routes::tests::rulesync_step1e_crash_during_skill_review_helper",
                ])
                .env("WENLAN_TEST_SKILL_REVIEW_ROOT", f._temp.path())
                .env("WENLAN_TEST_SKILL_REVIEW_TOKEN", token)
                .env("WENLAN_TEST_FILE_APPLY_CRASH", point)
                .output()
                .unwrap();
            assert_eq!(child.status.code(), Some(76), "{child:?}");
            let root = FileRoot::open_home(&f.setup.home).unwrap();
            let _lock = f.setup.lock().unwrap();
            let mut manifest = f.setup.load().unwrap().unwrap();
            f.setup.recover(&root, &mut manifest).unwrap();
            assert!(manifest.pending.is_empty());
        };
        let preview = preview_step1e_skill(&f);
        interrupt(&preview.token, "staged:helper.py");
        assert_eq!(fs::read_to_string(claude.join("SKILL.md")).unwrap(), edited);
        assert_eq!(
            fs::read(claude.join("helper.py")).unwrap(),
            b"# Claude-only helper\n"
        );
        assert!(f.sync_with("").written.is_empty());
        assert_eq!(fs::read_to_string(claude.join("SKILL.md")).unwrap(), edited);

        let resumed = preview_step1e_skill(&f);
        assert_ne!(resumed.token, preview.token);
        assert_eq!(resumed.files.len(), 1);
        assert_eq!(resumed.files[0].path, ".claude/skills/example/helper.py");
        interrupt(&resumed.token, "staged:skills.json");
        assert_eq!(
            fs::read(claude.join("helper.py")).unwrap(),
            b"# Reviewed helper\n"
        );
        assert!(f.sync_with("").written.is_empty());
        assert_eq!(
            fs::read(claude.join("helper.py")).unwrap(),
            b"# Reviewed helper\n"
        );
        let finalize = preview_step1e_skill(&f);
        assert!(finalize.files.is_empty());
        accept_step1e_skill(&f, &finalize);
        assert!(f.sync_with("").written.is_empty());
    }

    #[test]
    fn rulesync_step1e_restore_leaves_unrecorded_entries_in_a_created_directory() {
        let f = Fixture::new();
        fs::remove_dir_all(f.setup.home.join(".codex")).unwrap();
        let claude = write_step1d_skill(&f, ".claude", "Original body.", "# Claude helper\n");
        let cursor = write_step1d_skill(&f, ".cursor", "Original body.", "# Cursor helper\n");
        f.adopt_from_plan();
        fs::create_dir(cursor.join("docs")).unwrap();
        fs::write(cursor.join("docs/guide.md"), "An approved guide\n").unwrap();
        let preview = share_step1e_skill(&f);
        assert!(preview.files.iter().any(|change| {
            change.path == ".claude/skills/example/docs"
                && change.before.is_none()
                && change.after.as_ref().is_some_and(|entry| entry.directory)
        }));
        let local = claude.join("docs/.DS_Store");
        fs::write(&local, "Unrecorded local data\n").unwrap();

        let restored = f.setup.restore().unwrap();
        assert!(restored.blocked.is_empty(), "{restored:?}");
        assert_eq!(
            fs::read_to_string(local).unwrap(),
            "Unrecorded local data\n"
        );
        assert!(!claude.join("docs/guide.md").exists());
        assert!(!cursor.join("docs").exists());
        assert!(!f.setup.data.join("personal.json").exists());
    }

    #[test]
    fn rulesync_step1f_refuses_the_v2_journal_without_mutating_tool_paths() {
        let f = Fixture::new();
        write_step1d_skill(&f, ".claude", "Original body.", "# Helper\n");
        f.adopt_from_plan();
        let before = step1e_tree(&f.setup.home);
        let path = f.setup.data.join("personal.json");
        let mut journal: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        journal["format"] = 2.into();
        let bytes = serde_json::to_vec(&journal).unwrap();
        fs::write(&path, &bytes).unwrap();

        for result in [f.setup.sync(&[]), f.setup.restore()] {
            let message = format!("{:#}", result.unwrap_err());
            assert!(message.contains("wenlan rules restore"), "{message}");
            assert!(message.contains("adopt again"), "{message}");
        }
        assert_eq!(fs::read(path).unwrap(), bytes);
        assert_eq!(step1e_tree(&f.setup.home), before);
    }

    fn step1e_tree(home: &Path) -> BTreeMap<PathBuf, Snapshot> {
        fn visit(
            home: &Path,
            root: &FileRoot,
            path: &Path,
            tree: &mut BTreeMap<PathBuf, Snapshot>,
        ) {
            for entry in fs::read_dir(home.join(path)).unwrap() {
                let child = path.join(entry.unwrap().file_name());
                let snapshot = root.read(&child, true).unwrap();
                if snapshot.is_directory() {
                    visit(home, root, &child, tree);
                }
                tree.insert(child, snapshot);
            }
        }
        let mut tree = BTreeMap::new();
        visit(
            home,
            &FileRoot::open(home).unwrap(),
            Path::new(""),
            &mut tree,
        );
        tree
    }

    #[cfg(unix)]
    #[test]
    fn rulesync_step1e_restores_a_seeded_sequence_of_skill_approvals() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        const SEEDS: u64 = 32;
        fn choose(state: &mut u64, len: usize) -> usize {
            *state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            ((*state >> 32) as usize) % len
        }

        for seed in 0..SEEDS {
            let result = std::panic::catch_unwind(|| {
                let f = Fixture::new();
                let home = &f.setup.home;
                fs::rename(home.join(CLAUDE), home.join(".agents/claude.md")).unwrap();
                symlink("../.agents/claude.md", home.join(CLAUDE)).unwrap();
                let tools = [".claude", ".codex", ".cursor"];
                let folders: Vec<_> = tools[..2 + (seed % 2) as usize]
                    .iter()
                    .enumerate()
                    .map(|(i, tool)| {
                        let folder = write_step1d_skill(
                            &f,
                            tool,
                            "Original body.",
                            &format!("# Original {tool}\n"),
                        );
                        fs::create_dir(folder.join("docs")).unwrap();
                        fs::set_permissions(
                            folder.join("docs"),
                            fs::Permissions::from_mode(if i == 1 { 0o775 } else { 0o755 }),
                        )
                        .unwrap();
                        fs::write(folder.join("docs/guide.md"), format!("Guide for {tool}\n"))
                            .unwrap();
                        fs::write(folder.join("extra.txt"), "One more original asset\n").unwrap();
                        fs::set_permissions(
                            folder.join("helper.py"),
                            fs::Permissions::from_mode(if i == 1 { 0o640 } else { 0o644 }),
                        )
                        .unwrap();
                        folder
                    })
                    .collect();
                let picked = &folders[(seed as usize) % folders.len()];
                let locked = picked.join("locked");
                let inner = locked.join("manual");
                fs::create_dir_all(&inner).unwrap();
                fs::write(inner.join("guide.md"), b"Read-only original guide\n").unwrap();
                fs::set_permissions(&inner, fs::Permissions::from_mode(0o551)).unwrap();
                fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();

                let before = step1e_tree(home);
                f.adopt_from_plan();
                allow_fixture_tool_folders(&f);
                for path in [&locked, &inner] {
                    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
                }
                fs::remove_dir_all(&locked).unwrap();
                fs::rename(picked.join("extra.txt"), picked.join("Extra.txt")).unwrap();
                share_step1e_skill(&f);

                let created = picked.with_file_name("created-after-adoption");
                fs::create_dir(&created).unwrap();
                fs::write(created.join("SKILL.md"),
                    "---\nname: created-after-adoption\ndescription: Fixture\nversion: 1\n---\n\nNew skill.\n",
                ).unwrap();
                fs::write(created.join("helper.py"), b"# New helper\n").unwrap();
                for rename in [false, true] {
                    if rename {
                        fs::rename(created.join("helper.py"), created.join("renamed.py")).unwrap();
                    }
                    f.sync_with("");
                    rulesync_registry::accept_only_tool_proposal(
                        &f.setup,
                        None,
                        fixture_skill_renderer,
                    )
                    .unwrap();
                }

                let mut state = seed;
                // Two shuffled rounds guarantee edits, additions, deletions,
                // renames and file-mode changes for every seed.
                for round in 0..2 {
                    let mut actions = vec![0, 1, 2, 3, 4];
                    for i in 0..5 {
                        let action = actions.swap_remove(choose(&mut state, actions.len()));
                        let folder = &folders[choose(&mut state, folders.len())];
                        let fresh = folder.join(format!("asset-{round}-{i}.txt"));
                        let pick_asset = || {
                            if folder.join("docs/guide.md").is_file() {
                                return folder.join("docs/guide.md");
                            }
                            let mut assets: Vec<_> = fs::read_dir(folder)
                                .unwrap()
                                .map(|entry| entry.unwrap().path())
                                .filter(|path| path.is_file() && !path.ends_with("SKILL.md"))
                                .collect();
                            assets.sort();
                            assets.remove(0)
                        };
                        match action {
                            0 => {
                                fs::write(folder.join("helper.py"), format!("# Edit {round}-{i}\n"))
                                    .unwrap()
                            }
                            1 => fs::write(fresh, format!("New asset {round}-{i}\n")).unwrap(),
                            2 => {
                                let asset = pick_asset();
                                if asset.ends_with("guide.md") {
                                    fs::remove_dir_all(folder.join("docs")).unwrap();
                                } else {
                                    fs::remove_file(asset).unwrap();
                                }
                            }
                            3 => fs::rename(pick_asset(), fresh).unwrap(),
                            4 => {
                                let docs = folder.join("docs");
                                let path = if docs.is_dir() && choose(&mut state, 2) == 0 {
                                    docs
                                } else {
                                    folder.join("SKILL.md")
                                };
                                let mode =
                                    fs::metadata(&path).unwrap().permissions().mode() & 0o7777;
                                fs::set_permissions(
                                    &path,
                                    fs::Permissions::from_mode(mode ^ 0o020),
                                )
                                .unwrap();
                            }
                            _ => unreachable!(),
                        }
                        f.sync_with("");
                        let preview = preview_step1e_skill(&f);
                        let accepted = accept_step1e_skill(&f, &preview);
                        assert!(accepted.blocked.is_empty(), "{preview:?}\n{accepted:?}");
                    }
                }
                let restored = f.setup.restore().unwrap();
                assert!(restored.blocked.is_empty(), "{restored:?}");
                let after = step1e_tree(home);
                for path in [&locked, &inner] {
                    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
                }
                assert_eq!(
                    after.keys().collect::<Vec<_>>(),
                    before.keys().collect::<Vec<_>>()
                );
                assert_eq!(after, before);
            });
            assert!(result.is_ok(), "restore property failed for seed {seed}");
        }
    }

    #[test]
    fn rulesync_shares_the_edited_occurrence_of_a_repeated_rule() {
        let f = Fixture::new();
        let cursor = ".cursor/AGENTS.md";
        fs::create_dir(f.setup.home.join(".cursor")).unwrap();
        f.write(cursor, &format!("{SHARED}One rule.\nKeep this local.\n"));
        f.adopt_from_plan();
        f.write(
            cursor,
            &format!("{SHARED}Edited overlay rule.\nKeep this local.\n"),
        );
        let pending = f.sync_with("");
        let [card] = pending.proposals.as_slice() else {
            panic!("expected one Cursor review card, got {pending:?}");
        };
        let result = f
            .setup
            .review_using(
                &PullbackDecision {
                    id: card.id.clone(),
                    action: PullbackAction::AllTools,
                    file: None,
                },
                |root, manifest| f.sync_review(root, manifest),
            )
            .unwrap();
        assert!(result.blocked.is_empty(), "{result:?}");
        for path in [cursor, CLAUDE, CODEX] {
            let text = f.text(path);
            assert_eq!(text.matches("One rule.").count(), 1, "{path}: {text}");
            assert_eq!(
                text.matches("Edited overlay rule.").count(),
                1,
                "{path}: {text}"
            );
            assert_eq!(text.contains("Keep this local."), path == cursor);
        }
    }

    #[test]
    fn rulesync_more_tools_share_an_edited_overlay_rule() {
        let f = Fixture::new();
        let edited = ".config/opencode/AGENTS.md";
        let uninstalled = ".config/muse";
        fs::create_dir_all(f.setup.home.join(edited).parent().unwrap()).unwrap();
        f.write(edited, "OpenCode only.\nKeep this local.\n");
        f.adopt_from_plan();
        f.write(
            edited,
            &f.text(edited).replace("OpenCode only.", "Now shared."),
        );
        let pending = f.sync_with("");
        assert_eq!(pending.proposals.len(), 1, "{pending:?}");
        assert!(pending.proposals[0].reason.contains("Kept for this tool"));
        assert!(!f.text(CODEX).contains("Now shared."));

        let result = f
            .setup
            .review_using(
                &PullbackDecision {
                    id: pending.proposals[0].id.clone(),
                    action: PullbackAction::AllTools,
                    file: None,
                },
                |root, manifest| f.sync_review(root, manifest),
            )
            .unwrap();
        assert!(
            result.blocked.is_empty() && result.proposals.is_empty(),
            "{result:?}"
        );
        for path in [CLAUDE, CODEX, edited] {
            assert_eq!(f.text(path).matches("Now shared.").count(), 1, "{path}");
            assert!(!f.text(path).contains("OpenCode only."), "{path}");
            assert_eq!(f.text(path).contains("Keep this local."), path == edited);
        }
        assert!(!f.setup.home.join(uninstalled).exists());
        assert!(f.setup.restore().unwrap().blocked.is_empty());
        assert_eq!(f.text(edited), "OpenCode only.\nKeep this local.\n");
    }

    #[cfg(unix)]
    #[test]
    fn rulesync_more_tools_restore_a_link_to_another_adopted_rule() {
        let f = Fixture::new();
        let gemini = ".gemini/GEMINI.md";
        let antigravity = ".gemini/config/GEMINI.md";
        let original = "Local rule.\n";
        let link = "../GEMINI.md";
        let path = f.setup.home.join(antigravity);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        f.write(gemini, original);
        std::os::unix::fs::symlink(link, &path).unwrap();
        let plan = wenlan_core::rulesync_personal::plan_adoption(&f.setup.home).unwrap();
        let adopted = f
            .setup
            .adopt_using(&plan.token, fixture_rule_renderer, fixture_skill_renderer)
            .unwrap();
        for path in [gemini, antigravity] {
            assert_eq!(
                adopted
                    .written
                    .iter()
                    .filter(|value| value.as_str() == path)
                    .count(),
                1
            );
            assert_eq!(f.text(path), format!("{SHARED}{original}"));
        }
        assert!(!fs::symlink_metadata(&path).unwrap().is_symlink());
        assert!(f.setup.restore().unwrap().blocked.is_empty());
        assert_eq!(f.text(gemini), original);
        assert_eq!(fs::read_link(path).unwrap(), Path::new(link));
    }

    #[test]
    fn run9_settings_backup_survives_later_adoption() {
        let f = Fixture::new();
        let config = Path::new(".claude.json");
        let original = r#"{"sessionToken":"fixture-only"}"#;
        f.write(config.to_str().unwrap(), original);
        let root = FileRoot::open_home(&f.setup.home).unwrap();
        let seen = root.read(config, false).unwrap();
        let desired = with_bytes(
            &seen,
            br#"{"sessionToken":"fixture-only","mcpServers":{"new":{"command":"new"}}}"#.to_vec(),
        );
        {
            let _lock = f.setup.lock().unwrap();
            let applied = f
                .setup
                .apply_settings(
                    &root,
                    "first-source".into(),
                    vec![(config.into(), seen, desired.clone())],
                )
                .unwrap();
            assert_eq!(applied.written, [".claude.json"]);
            assert!(applied.blocked.is_empty(), "{applied:?}");
        }

        let home = f.setup.home.canonicalize().unwrap();
        f.setup.preview_adoption(&home).unwrap();
        f.adopt_from_plan();
        assert_eq!(root.read(config, false).unwrap(), desired);

        let restored = f.setup.restore().unwrap();
        assert!(restored.blocked.is_empty(), "{restored:?}");
        assert_eq!(f.text(config.to_str().unwrap()), original);
    }

    #[cfg(unix)]
    #[test]
    fn run9_displaced_file_remains_reachable_after_editor_saves_through_open_handle() {
        use std::io::{Seek as _, Write as _};

        let f = Fixture::new();
        let path = f.setup.home.join(CLAUDE);
        let before = f.text(CLAUDE);
        let mut editor = fs::OpenOptions::new().write(true).open(&path).unwrap();
        f.adopt_from_plan();
        assert_ne!(f.text(CLAUDE), before);
        editor.rewind().unwrap();
        editor.set_len(0).unwrap();
        editor
            .write_all(b"late save to the original inode\n")
            .unwrap();
        editor.sync_all().unwrap();
        drop(editor);

        fn contains_edit(dir: &Path) -> bool {
            fs::read_dir(dir).unwrap().flatten().any(|entry| {
                let kind = entry.file_type().unwrap();
                if kind.is_dir() {
                    contains_edit(&entry.path())
                } else {
                    kind.is_file()
                        && fs::read(entry.path()).unwrap() == b"late save to the original inode\n"
                }
            })
        }
        assert!(
            contains_edit(&f.setup.data),
            "the editor's successful write has no recovery file"
        );
        assert_ne!(
            fs::read(path).unwrap(),
            b"late save to the original inode\n"
        );
    }

    #[test]
    fn rulesync_revokes_only_the_inaccessible_section_before_personal_review() {
        let f = Fixture::new();
        f.adopt_from_plan();
        let retained = TeamRuleLayer {
            id: "space-still-accessible".into(),
            name: "Retained".into(),
            markdown: "Use the retained checklist.".into(),
            revision: "b".repeat(64),
            synced_at: String::new(),
            project: None,
            skills: Vec::new(),
        };
        let section = |key: &str, wording: &str| {
            format!(
                "<!-- WENLAN SHARED RULES {key} {} -->\n## Shared rules\n\nLast synced date.\n\n{wording}\n<!-- END WENLAN SHARED RULES -->\n",
                "b".repeat(64)
            )
        };
        let keep = section(
            &rulesync_layers::key(&retained).unwrap(),
            &retained.markdown,
        );
        let revoked = section(&"a".repeat(64), "Use the revoked checklist.");
        let delivered = f.sync_with(&format!("{revoked}{keep}"));
        assert!(delivered.blocked.is_empty(), "{delivered:?}");
        let personal = "Personal advice to review.\n";
        let edited = f.text(CODEX) + personal;
        f.write(CODEX, &edited);

        {
            let _lock = f.setup.lock().unwrap();
            let root = FileRoot::open(&f.setup.home).unwrap();
            let mut manifest = f.setup.load().unwrap().unwrap();
            f.setup
                .capture_team_edits(
                    &root,
                    &mut manifest,
                    &f.setup.data.join("settings.toml"),
                    &[retained],
                )
                .unwrap();
        }
        let report = f.sync_with(&keep);
        assert_eq!(f.text(CODEX), edited.replacen(&revoked, "", 1));
        assert!(report
            .proposals
            .iter()
            .any(|proposal| proposal.files == [CODEX] && proposal.diff.contains(personal.trim())));
    }

    #[cfg(unix)]
    #[test]
    fn rulesync_adoption_subprocess() {
        let Some(root) = std::env::var_os("WENLAN_ADOPTION_TEST_ROOT").map(PathBuf::from) else {
            return;
        };
        let setup = Setup {
            home: root.join("home"),
            data: root.join("data"),
            binary: PathBuf::from("unused"),
        };
        if let Ok(token) = std::env::var("WENLAN_ADOPTION_TEST_TOKEN") {
            let result = setup.adopt_using(&token, fixture_rule_renderer, fixture_skill_renderer);
            match result {
                Ok(report) => println!(
                    "\nadoption result {}",
                    serde_json::to_string(&report).unwrap()
                ),
                Err(error) => println!("\nadoption error {error:#}"),
            }
        } else {
            let plan = setup
                .preview_adoption(&setup.home.canonicalize().unwrap())
                .unwrap();
            println!(
                "\nadoption preview {}",
                serde_json::to_string(&plan).unwrap()
            );
        }
    }

    #[cfg(unix)]
    fn adoption_subprocess(fixture: &Fixture, with_git: bool, token: Option<&str>) -> String {
        let root = fixture._temp.path();
        let work = root.join(if with_git { "preview-cwd" } else { "apply-cwd" });
        let tmp = work.join("tmp");
        fs::create_dir_all(&tmp).unwrap();
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "rulesync_routes::tests::rulesync_adoption_subprocess",
                "--nocapture",
            ])
            .current_dir(&work)
            .env("TMPDIR", &tmp)
            .env("HOME", &fixture.setup.home)
            .env("CODEX_HOME", fixture.setup.home.join(".codex"))
            .env("WENLAN_DATA_DIR", &fixture.setup.data)
            .env("WENLAN_NO_AUTOSTART", "1")
            .env_remove("WENLAN_SESSION")
            .env("WENLAN_ADOPTION_TEST_ROOT", root)
            .env_remove("WENLAN_ADOPTION_TEST_TOKEN");
        if !with_git {
            command.env("PATH", &tmp);
        }
        if let Some(token) = token {
            command.env("WENLAN_ADOPTION_TEST_TOKEN", token);
        }
        let output = command.output().unwrap();
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(
            output.status.success(),
            "{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        stdout
            .lines()
            .find(|line| line.starts_with("adoption "))
            .expect(&stdout)
            .to_owned()
    }

    #[cfg(unix)]
    fn preview_in_subprocess(fixture: &Fixture, with_git: bool) -> AdoptionPlan {
        let stdout = adoption_subprocess(fixture, with_git, None);
        serde_json::from_str(stdout.strip_prefix("adoption preview ").unwrap()).unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn run9_adoption_rechecks_linked_text_and_skill_assets_before_detaching() {
        use std::os::unix::fs::{symlink, PermissionsExt as _};

        for changed in [".agents/codex.md", ".agents/skills/example/asset.txt"] {
            for change_mode in [false, true] {
                let f = Fixture::new();
                let home = &f.setup.home;
                let source = home.join(".agents/skills/example");
                fs::create_dir_all(&source).unwrap();
                fs::create_dir_all(home.join(".codex/skills")).unwrap();
                fs::write(
                    source.join("SKILL.md"),
                    "---\nname: example\ndescription: Fixture\n---\n",
                )
                .unwrap();
                fs::write(source.join("asset.txt"), b"old asset").unwrap();
                fs::remove_file(home.join(CODEX)).unwrap();
                f.write(".agents/codex.md", "Old codex rules\n");
                symlink("../.agents/codex.md", home.join(CODEX)).unwrap();
                symlink(
                    "../../.agents/skills/example",
                    home.join(".codex/skills/example"),
                )
                .unwrap();
                let adoption = discover_adoption(home).unwrap();
                let master = PersonalMaster::new(&adoption);
                if change_mode {
                    fs::set_permissions(home.join(changed), fs::Permissions::from_mode(0o400))
                        .unwrap();
                } else {
                    f.write(changed, "A newer edit\n");
                }
                let root = FileRoot::open(home).unwrap();
                let _lock = f.setup.lock().unwrap();
                assert!(
                    f.setup.apply_adoption(&root, adoption, master).is_err(),
                    "adopted a stale referent: {changed}, mode only: {change_mode}"
                );
                assert!(fs::symlink_metadata(home.join(CODEX)).unwrap().is_symlink());
                assert!(fs::symlink_metadata(home.join(".codex/skills/example"))
                    .unwrap()
                    .is_symlink());
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn run9_adoption_accepts_equivalent_links_and_restores_the_latest_spelling() {
        use std::os::unix::fs::{symlink, PermissionsExt as _};

        let f = Fixture::new();
        let home = &f.setup.home;
        let rule = home.join(CODEX);
        let rule_source = home.join(".agents/codex.md");
        fs::rename(&rule, &rule_source).unwrap();
        fs::set_permissions(&rule_source, fs::Permissions::from_mode(0o640)).unwrap();
        symlink("../.agents/codex.md", &rule).unwrap();

        let skill = home.join(".codex/skills/example");
        let skill_source = home.join(".agents/skills/example");
        fs::create_dir_all(skill.parent().unwrap()).unwrap();
        fs::create_dir_all(skill_source.join("nested")).unwrap();
        fs::write(
            skill_source.join("SKILL.md"),
            "---\nname: example\ndescription: Fixture\n---\n",
        )
        .unwrap();
        let asset = skill_source.join("nested/asset.txt");
        fs::write(&asset, b"an asset").unwrap();
        fs::set_permissions(&asset, fs::Permissions::from_mode(0o750)).unwrap();
        symlink("../../.agents/skills/example", &skill).unwrap();

        let adoption = discover_adoption(home).unwrap();
        let master = PersonalMaster::new(&adoption);
        let plan = wenlan_core::rulesync_personal::adoption_plan(&adoption).unwrap();

        let rule_target = "../.agents/./codex.md";
        let skill_target = "../../.agents/skills/./example";
        for (path, target) in [(&rule, rule_target), (&skill, skill_target)] {
            fs::remove_file(path).unwrap();
            symlink(target, path).unwrap();
        }

        let current = discover_adoption(home).unwrap();
        verify_adoption_token(&current, &plan.token).unwrap();
        {
            let root = FileRoot::open_home(home).unwrap();
            let _lock = f.setup.lock().unwrap();
            // The last discovery before applying may have seen older spellings.
            f.setup.apply_adoption(&root, adoption, master).unwrap();
        }
        assert_eq!(
            fs::read(skill.join("nested/asset.txt")).unwrap(),
            b"an asset"
        );
        for (path, mode) in [(&rule, 0o640), (&skill.join("nested/asset.txt"), 0o750)] {
            let metadata = fs::symlink_metadata(path).unwrap();
            assert!(metadata.is_file());
            assert_eq!(metadata.permissions().mode() & 0o7777, mode);
        }

        let restored = f.setup.restore().unwrap();
        assert!(restored.blocked.is_empty(), "{restored:?}");
        for (path, target) in [(&rule, rule_target), (&skill, skill_target)] {
            assert_eq!(fs::read_link(path).unwrap().as_os_str(), target);
        }
    }

    #[cfg(unix)]
    #[test]
    fn rulesync_adoption_accepts_the_same_content_across_process_environments() {
        let fixture = Fixture::new();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .env_clear()
                .env("PATH", std::env::var_os("PATH").unwrap())
                .env("HOME", &fixture.setup.home)
                .arg("-C")
                .arg(fixture.setup.home.join(".codex"))
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "--quiet"]);
        git(&[
            "remote",
            "add",
            "origin",
            "https://github.com/fixture/approved-tools.git",
        ]);

        let plan = preview_in_subprocess(&fixture, true);
        let without_git = preview_in_subprocess(&fixture, false);
        assert!(plan
            .notes
            .iter()
            .any(|note| note.contains("repository approved-tools")));
        assert_ne!(plan.notes, without_git.notes);
        assert_eq!(plan.files, without_git.files);
        assert_eq!(plan.skills, without_git.skills);
        assert_eq!(plan.token, without_git.token);
        assert!(!fixture.setup.data.join("personal.lock").exists());

        let stdout = adoption_subprocess(&fixture, false, Some(&plan.token));
        let report: RulesSyncReport =
            serde_json::from_str(stdout.strip_prefix("adoption result ").expect(&stdout)).unwrap();
        assert_eq!(report.written, [CLAUDE, CODEX]);
        assert_eq!(fixture.text(CLAUDE), format!("{SHARED}Claude only.\n"));
        let backup = fs::read(fixture.setup.data.join("personal.json")).unwrap();
        let duplicate = adoption_subprocess(&fixture, false, Some(&plan.token));
        assert!(
            duplicate.contains("The adoption plan changed in file entries"),
            "{duplicate}"
        );
        assert_eq!(
            fs::read(fixture.setup.data.join("personal.json")).unwrap(),
            backup
        );
    }

    #[cfg(unix)]
    #[test]
    fn rulesync_adoption_refuses_a_file_edit_between_processes() {
        let fixture = Fixture::new();
        let plan = preview_in_subprocess(&fixture, true);
        fixture.write(CODEX, "Saved after preview.\n");
        let stdout = adoption_subprocess(&fixture, false, Some(&plan.token));
        assert!(
            stdout.starts_with("adoption error The adoption plan changed in file entries."),
            "{stdout}"
        );
        assert_eq!(fixture.text(CODEX), "Saved after preview.\n");
        assert_eq!(fixture.text(CLAUDE), "@~/.agents/AGENTS.md\nClaude only.\n");
        assert!(!fixture.setup.data.join("personal.json").exists());
    }

    #[test]
    fn rulesync_adoption_refuses_a_different_cli_tool_home() {
        let fixture = Fixture::new();
        let other = fixture._temp.path().canonicalize().unwrap();
        let error = fixture.setup.preview_adoption(&other).unwrap_err();
        assert!(error.to_string().contains("different tool homes"));
        let error = fixture
            .setup
            .adopt(&AdoptRequest {
                token: String::new(),
                home: Some(other),
            })
            .unwrap_err();
        assert!(error.to_string().contains("different tool homes"));
        assert!(!fixture.setup.data.join("personal.lock").exists());
    }

    #[test]
    fn rulesync_blocked_paths_explain_how_to_review_the_change() {
        let f = Fixture::new();
        f.adopt_from_plan();
        f.write(CODEX, &format!("{}A new line.\n", f.text(CODEX)));
        let skill = ".claude/skills/new-skill/SKILL.md";
        fs::create_dir_all(f.setup.home.join(skill).parent().unwrap()).unwrap();
        f.write(skill, "New instructions.\n");

        let json = serde_json::to_value(f.sync_with("")).unwrap();
        assert_eq!(
            json["blocked"],
            serde_json::json!([
                ".claude/skills/new-skill: Skill changes need your approval. Run `wenlan skills pending` to review them.",
                ".codex/AGENTS.md: Rule changes need your approval. Use `wenlan rules review` with the proposal below."
            ])
        );
    }

    #[test]
    fn rulesync_holds_delivery_into_a_worktree_added_after_adoption() {
        let f = Fixture::new();
        f.adopt_from_plan();
        let checkout = f.setup.home.join(".claude");
        assert!(std::process::Command::new("git")
            .arg("-C")
            .arg(&checkout)
            .args(["init", "--quiet"])
            .status()
            .unwrap()
            .success());
        let old = f.text(CLAUDE);
        let sync = f.sync_with("New render\n");
        assert_eq!(sync.written, [CODEX]);
        assert_eq!(sync.blocked.len(), 1, "{sync:?}");
        assert!(sync.blocked[0].contains("inside the Git worktree at ~/.claude"));
        assert_eq!(f.text(CLAUDE), old);
    }

    #[test]
    fn rulesync_personal_codex_delivery_uses_the_tool_owned_folder() {
        let shared = Path::new(".agents/skills/public-skill");
        let claude = Path::new(".claude/skills/public-skill");
        assert_eq!(
            personal_skill_destination(personal_target("codexcli").unwrap().skills, shared)
                .unwrap(),
            Path::new(".codex/skills/public-skill")
        );
        assert_eq!(
            personal_skill_destination(personal_target("claudecode").unwrap().skills, claude)
                .unwrap(),
            claude
        );
        let gemini = personal_target("geminicli").unwrap();
        let opencode = Path::new(".config/opencode/skills/public-skill");
        assert_eq!(
            personal_skill_destination(gemini.skills, opencode).unwrap(),
            Path::new(".gemini/skills/public-skill")
        );
        assert!(personal_skill_destination(gemini.skills, shared).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rulesync_personal_lifecycle_keeps_the_harness_read_only() {
        use std::os::unix::fs::symlink;

        let f = Fixture::new();
        let home = &f.setup.home;
        let harness = home.join(".agents").canonicalize().unwrap();
        let generated = ".agents/generated/codex/AGENTS.md";
        let shared_skill = ".agents/skills/example/SKILL.md";
        let asset = ".agents/skills/example/agents/openai.yaml";
        let codex_folder = ".codex/skills/example";
        let claude_folder = ".claude/skills/example";
        let original_skill = "---\nname: example\ndescription: An example\n---\nBody.\n";
        for path in [generated, shared_skill, asset, codex_folder, claude_folder] {
            fs::create_dir_all(home.join(path).parent().unwrap()).unwrap();
        }
        fs::rename(home.join(CODEX), home.join(generated)).unwrap();
        f.write(shared_skill, original_skill);
        f.write(asset, "interface:\n  display_name: Example\n");
        let original_links = BTreeMap::from([
            (
                Path::new(CODEX),
                PathBuf::from("../.agents/./generated/codex/AGENTS.md"),
            ),
            (
                Path::new(codex_folder),
                PathBuf::from("../../.agents/./skills/example"),
            ),
            (
                Path::new(claude_folder),
                home.join(".agents/skills/example"),
            ),
        ]);
        for (path, target) in &original_links {
            symlink(target, home.join(path)).unwrap();
        }

        // Compare the whole source tree at each step, including original file
        // modes and mtimes. Reading its content may change atime.
        fn snapshot(
            home: &Path,
            root: &FileRoot,
            path: &Path,
            entries: &mut BTreeMap<PathBuf, (Snapshot, std::time::SystemTime)>,
        ) {
            let absolute = home.join(path);
            let state = root.read(path, true).unwrap();
            if state.is_directory() {
                for entry in fs::read_dir(&absolute).unwrap() {
                    snapshot(home, root, &path.join(entry.unwrap().file_name()), entries);
                }
            }
            let modified = fs::symlink_metadata(&absolute).unwrap().modified().unwrap();
            entries.insert(path.to_owned(), (state, modified));
        }
        let home_root = FileRoot::open(home).unwrap();
        let harness_state = || {
            let mut entries = BTreeMap::new();
            snapshot(home, &home_root, Path::new(".agents"), &mut entries);
            entries
        };
        let source_before = harness_state();
        let check_before_or_restored_path = |path: &Path| {
            // The batch installs a real directory before writing its
            // children, or removes the children before restoring its link.
            let leaf = original_links
                .keys()
                .find(|link| path.starts_with(link))
                .copied()
                .unwrap_or(path);
            let destination = home
                .join(leaf.parent().unwrap())
                .canonicalize()
                .unwrap()
                .join(leaf.file_name().unwrap());
            assert!(!destination.starts_with(&harness), "{destination:?}");
        };
        let assert_writes_outside = |report: &RulesSyncReport| {
            assert!(report.blocked.is_empty(), "{report:?}");
            for path in &report.written {
                let destination = home.join(path).canonicalize().unwrap();
                assert!(!destination.starts_with(&harness), "{destination:?}");
            }
        };

        let adoption = discover_adoption(home).unwrap();
        for path in adoption.files.keys() {
            check_before_or_restored_path(path);
        }
        let changed_render = std::cell::Cell::new(false);
        let render_skills = |target: &str, skills: &SkillPackages| {
            let mut output = fixture_skill_renderer(target, skills)?;
            if changed_render.get() && target == "codexcli" {
                output
                    .get_mut(Path::new(".agents/skills/example/SKILL.md"))
                    .unwrap()
                    .extend_from_slice(b"Updated rendered skill.\n");
            }
            Ok(output)
        };
        let plan = f.setup.preview_adoption_using(render_skills).unwrap();
        assert!(plan
            .notes
            .iter()
            .any(|note| note.contains("~/.agents is left untouched")));
        assert_eq!(harness_state(), source_before);
        let adopted = f
            .setup
            .adopt_using(&plan.token, fixture_rule_renderer, render_skills)
            .unwrap();
        assert_writes_outside(&adopted);
        assert_eq!(harness_state(), source_before);
        assert!(!fs::symlink_metadata(home.join(CODEX)).unwrap().is_symlink());
        for folder in [codex_folder, claude_folder] {
            assert!(!fs::symlink_metadata(home.join(folder))
                .unwrap()
                .is_symlink());
            assert_eq!(
                f.text(&format!("{folder}/agents/openai.yaml")),
                f.text(asset)
            );
        }

        // A newly added source in the old harness is outside normal tool
        // pull-back. The known tools still receive both sync and review writes.
        let owner_skill = ".agents/skills/owner-only/SKILL.md";
        fs::create_dir(home.join(owner_skill).parent().unwrap()).unwrap();
        f.write(owner_skill, "Owner's source.\n");
        let source_before = harness_state();
        changed_render.set(true);
        let generated_rules = vec!["Generated guidance.".into()];
        let sync = |root: &FileRoot, manifest: &mut Manifest| {
            f.setup.sync_locked(
                root,
                manifest,
                &generated_rules,
                fixture_rule_renderer,
                render_skills,
            )
        };
        let sync_with_lock = || {
            let _lock = f.setup.lock().unwrap();
            let mut manifest = f.setup.load().unwrap().unwrap();
            f.setup.recover(&home_root, &mut manifest).unwrap();
            sync(&home_root, &mut manifest).unwrap()
        };
        let synced = sync_with_lock();
        assert_eq!(
            synced.written,
            [CLAUDE, CODEX, ".codex/skills/example/SKILL.md"]
        );
        assert!(synced.proposals.is_empty(), "{synced:?}");
        assert_writes_outside(&synced);
        assert_eq!(harness_state(), source_before);

        let addition = "Reviewed rule.";
        f.write(CODEX, &format!("{}{addition}\n", f.text(CODEX)));
        let proposed = sync_with_lock();
        assert_eq!(proposed.proposals.len(), 1, "{proposed:?}");
        assert!(proposed.written.is_empty(), "{proposed:?}");
        assert_eq!(harness_state(), source_before);
        let decision = PullbackDecision {
            id: proposed.proposals[0].id.clone(),
            action: PullbackAction::AllTools,
            file: None,
        };
        let reviewed = f.setup.review_using(&decision, sync).unwrap();
        assert!(reviewed.proposals.is_empty(), "{reviewed:?}");
        assert!(reviewed.written.iter().any(|path| path == CLAUDE));
        assert_writes_outside(&reviewed);
        for path in [CLAUDE, CODEX] {
            assert_eq!(f.text(path).matches(addition).count(), 1);
        }
        assert_eq!(harness_state(), source_before);

        let restored = f.setup.restore().unwrap();
        assert!(restored.blocked.is_empty(), "{restored:?}");
        assert_eq!(restored.written.len(), adopted.written.len());
        for path in &restored.written {
            check_before_or_restored_path(Path::new(path));
        }
        for (path, target) in original_links {
            assert_eq!(
                fs::read_link(home.join(path)).unwrap().as_os_str(),
                target.as_os_str()
            );
        }
        assert_eq!(harness_state(), source_before);
    }

    #[test]
    fn rulesync_rejects_a_batch_with_a_harness_destination_before_saving() {
        let f = Fixture::new();
        let root = FileRoot::open_home(&f.setup.home).unwrap();
        let originals = FileRoot::open(&f.setup.home).unwrap();
        let _lock = f.setup.lock().unwrap();
        let mut manifest = Manifest::new(
            PersonalMaster::new(&discover_adoption(&f.setup.home).unwrap()),
            BTreeMap::new(),
        );
        let operations = [CODEX, ".agents/AGENTS.md"]
            .into_iter()
            .map(|path| Pending {
                path: path.into(),
                seen: originals.read(Path::new(path), true).unwrap(),
                desired: Snapshot::file(b"unexpected replacement\n".to_vec()),
                kind: None,
                allow_link: true,
            })
            .collect();
        let error = f
            .setup
            .apply_batch(&root, &mut manifest, operations, BatchMode::Adopt)
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("inside the read-only ~/.agents harness"));
        assert!(manifest.pending.is_empty());
        assert!(!f.setup.data.join("personal.json").exists());
        assert_eq!(f.text(CODEX), format!("{SHARED}Codex only.\n"));
        assert_eq!(f.text(".agents/AGENTS.md"), SHARED);
    }

    #[test]
    fn rulesync_refuses_to_write_to_the_harness_even_with_a_receipt() {
        let f = Fixture::new();
        f.adopt_from_plan();
        let protected = PathBuf::from(".agents/AGENTS.md");
        let root = FileRoot::open(&f.setup.home).unwrap();
        let mut manifest = f.setup.load().unwrap().unwrap();
        manifest.backup.insert(protected.clone(), Snapshot::Absent);
        manifest.delivered.insert(
            protected.clone(),
            Delivery {
                kind: Kind::RegistrySkill {
                    name: "legacy".into(),
                },
                delivered: root.read(&protected, true).unwrap(),
                observed: None,
                reviewed: false,
                at: 0,
            },
        );
        f.setup.save(&manifest).unwrap();

        let sync = f.sync_with("");
        assert_eq!(
            sync.blocked,
            [format!("{}: {HARNESS_READ_ONLY}", protected.display())]
        );
        assert!(sync.proposals.is_empty(), "{sync:?}");
        let restored = f.setup.restore().unwrap();
        assert_eq!(restored.blocked.len(), 1, "{restored:?}");
        assert!(restored.blocked[0].contains("retained backup"));
        assert_eq!(restored.written.len(), 2);
        assert_eq!(f.text(".agents/AGENTS.md"), SHARED);
        assert!(f
            .setup
            .load()
            .unwrap()
            .unwrap()
            .backup
            .contains_key(&protected));
    }

    #[cfg(unix)]
    #[test]
    fn rulesync_data_lock_cannot_create_files_under_the_harness() {
        let mut f = Fixture::new();
        let link = f.setup.home.join("data-alias");
        std::os::unix::fs::symlink(".agents", &link).unwrap();
        f.setup.data = link.join("rulesync");
        let error = f.setup.lock().unwrap_err();
        assert!(error.to_string().contains("read-only ~/.agents"));
        assert!(!f.setup.data.exists());
    }

    #[cfg(unix)]
    #[test]
    fn rulesync_preview_rejects_a_tool_directory_that_is_the_physical_harness() {
        let f = Fixture::new();
        let home = &f.setup.home;
        fs::rename(home.join(".agents"), home.join("original-source")).unwrap();
        std::os::unix::fs::symlink(".codex", home.join(".agents")).unwrap();
        for adapter in [
            ".codex/adapters/claude/CLAUDE.md",
            ".codex/adapters/codex/AGENTS.codex.md",
        ] {
            fs::create_dir_all(home.join(adapter).parent().unwrap()).unwrap();
            f.write(adapter, "");
        }
        f.write(CLAUDE, "Claude rule.\n");
        let error = wenlan_core::rulesync_personal::plan_adoption(home).unwrap_err();
        assert!(
            error
                .to_string()
                .contains(".codex/AGENTS.md is inside the read-only ~/.agents harness"),
            "{error:#}"
        );
        assert_eq!(f.text(CODEX), format!("{SHARED}Codex only.\n"));
        assert!(!f.setup.data.join("personal.json").exists());
    }

    #[test]
    fn rulesync_adopted_regular_and_large_skills_are_quiet_on_first_sync() {
        let f = Fixture::new();
        for path in [
            ".claude/skills/large/SKILL.md",
            ".codex/skills/local/SKILL.md",
            ".agents/skills/shared/SKILL.md",
            ".codex/skills/too-large/SKILL.md",
        ] {
            fs::create_dir_all(f.setup.home.join(path).parent().unwrap()).unwrap();
            let name = Path::new(path)
                .parent()
                .unwrap()
                .file_name()
                .unwrap()
                .to_str()
                .unwrap();
            f.write(
                path,
                &format!("---\nname: {name}\ndescription: Test skill\n---\nBody.\n"),
            );
        }
        for i in 0..101 {
            f.write(&format!(".claude/skills/large/asset-{i}"), "Asset\n");
        }
        let oversized = f.setup.home.join(".codex/skills/too-large/asset");
        fs::File::create(oversized)
            .unwrap()
            .set_len(33 * 1024 * 1024)
            .unwrap();
        f.adopt_from_plan();

        let root = FileRoot::open_home(&f.setup.home).unwrap();
        let mut manifest = f.setup.load().unwrap().unwrap();
        let report = f
            .setup
            .sync_locked(
                &root,
                &mut manifest,
                &[],
                |target, text| {
                    let path = if target == "codexcli" { CODEX } else { CLAUDE };
                    Ok(BTreeMap::from([(path.into(), text.as_bytes().to_vec())]))
                },
                fixture_skill_renderer,
            )
            .unwrap();
        assert!(report.blocked.is_empty(), "{report:?}");
        assert!(report.written.is_empty(), "{report:?}");
        assert!(manifest
            .delivered
            .contains_key(Path::new(".codex/skills/local/SKILL.md")));
        assert!(rulesync_registry::saved_skill_deliveries(&f.setup.data)
            .unwrap()
            .folders
            .is_empty());
    }

    #[test]
    fn rulesync_carries_an_unrendered_codex_sidecar_through_sync() {
        let f = Fixture::new();
        let folder = ".codex/skills/social";
        let sidecar = ".codex/skills/social/agents/openai.yaml";
        fs::create_dir_all(f.setup.home.join(folder).join("agents")).unwrap();
        let skill = "---\nname: social\ndescription: Writing workflow\n---\nBody.\n";
        f.write(&format!("{folder}/SKILL.md"), skill);
        f.write(sidecar, "interface:\n  display_name: Writing\n");
        f.adopt_from_plan();

        let root = FileRoot::open_home(&f.setup.home).unwrap();
        let mut manifest = f.setup.load().unwrap().unwrap();
        let updated = "interface:\n  display_name: Social writing\n";
        manifest.master.skills[0]
            .files
            .insert("agents/openai.yaml".into(), updated.as_bytes().to_vec());
        let report = f
            .setup
            .sync_locked(
                &root,
                &mut manifest,
                &[],
                |target, text| {
                    let path = if target == "codexcli" { CODEX } else { CLAUDE };
                    Ok(BTreeMap::from([(path.into(), text.as_bytes().to_vec())]))
                },
                |_, _| {
                    Ok(BTreeMap::from([(
                        ".agents/skills/social/SKILL.md".into(),
                        skill.as_bytes().to_vec(),
                    )]))
                },
            )
            .unwrap();

        assert!(report.blocked.is_empty(), "{report:?}");
        assert_eq!(report.written, [sidecar]);
        assert_eq!(f.text(sidecar), updated);
        assert!(!f.setup.home.join(".agents/skills/social").exists());
    }

    #[test]
    fn rulesync_adopts_the_rendered_skill_without_a_first_sync_rewrite() {
        let f = Fixture::new();
        let file = ".codex/skills/folding/SKILL.md";
        let staged = ".agents/skills/folding/SKILL.md";
        fs::create_dir_all(f.setup.home.join(file).parent().unwrap()).unwrap();
        let original = "---\nname: folding\ndescription: A long description with enough words to make the standard formatter wrap it over multiple lines while retaining every word.\n---\n\nBody.\n";
        let rendered = b"---\nname: folding\ndescription: >-\n  A long description with enough words to make the standard formatter wrap it\n  over multiple lines while retaining every word.\n---\nBody.\n";
        f.write(file, original);
        let preview =
            |_: &str, _: &SkillPackages| Ok(BTreeMap::from([(staged.into(), rendered.to_vec())]));
        let plan = f.setup.preview_adoption_using(preview).unwrap();
        let skill = plan
            .files
            .iter()
            .find(|change| change.path == file)
            .unwrap();
        assert!(skill.diff.contains("+description: >-"));
        let error = f
            .setup
            .adopt_using(&plan.token, fixture_rule_renderer, |_, _| {
                Ok(BTreeMap::from([(
                    staged.into(),
                    original.as_bytes().to_vec(),
                )]))
            })
            .unwrap_err();
        assert!(format!("{error:#}").contains("The adoption plan changed in file entries"));
        assert!(!f.setup.data.join("personal.json").exists());
        assert_eq!(f.text(file), original);

        f.setup
            .adopt_using(&plan.token, fixture_rule_renderer, preview)
            .unwrap();
        assert_eq!(f.text(file).as_bytes(), rendered);
        let root = FileRoot::open_home(&f.setup.home).unwrap();
        let mut manifest = f.setup.load().unwrap().unwrap();
        let sync = f
            .setup
            .sync_locked(&root, &mut manifest, &[], fixture_rule_renderer, preview)
            .unwrap();
        assert!(
            sync.written.is_empty() && sync.blocked.is_empty() && sync.proposals.is_empty(),
            "{sync:?}"
        );
        f.setup.restore().unwrap();
        assert_eq!(f.text(file), original);
    }

    #[cfg(unix)]
    #[test]
    fn rulesync_thousand_files_use_bounded_manifest_saves() {
        const SKILLS: usize = 20;
        const ASSETS: usize = 50;
        const FILES: usize = SKILLS * ASSETS;
        let f = Fixture::new();
        let home = &f.setup.home;
        let tool_skills = Path::new(".claude/skills");
        fs::create_dir_all(home.join(tool_skills)).unwrap();
        let names: Vec<_> = (0..SKILLS)
            .map(|index| format!("fixture-{index:02}"))
            .collect();
        for name in &names {
            let source = home.join("sources").join(name);
            fs::create_dir_all(&source).unwrap();
            fs::write(
                source.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: Fixture\n---\n"),
            )
            .unwrap();
            for index in 0..ASSETS {
                fs::write(source.join(format!("file-{index}.txt")), b"original\0\xff").unwrap();
            }
            std::os::unix::fs::symlink(
                format!("../../sources/{name}"),
                home.join(tool_skills).join(name),
            )
            .unwrap();
        }
        let before = MANIFEST_SAVES.get();
        let started = std::time::Instant::now();
        f.adopt_from_plan();
        let adoption_saves = MANIFEST_SAVES.get() - before;
        let adoption_time = started.elapsed();
        for name in &names {
            let folder = home.join(tool_skills).join(name);
            assert!(!folder.is_symlink());
            assert_eq!(fs::read_dir(folder).unwrap().count(), ASSETS + 1);
        }
        assert!(
            adoption_saves <= 4,
            "adopting {FILES} files saved the manifest {adoption_saves} times"
        );

        let manifest_file = f.setup.data.join("personal.json");
        let stored: serde_json::Value =
            serde_json::from_slice(&fs::read(&manifest_file).unwrap()).unwrap();
        assert_eq!(stored["format"], 3);
        let files = &stored["state"]["master"]["skills"][0]["files"];
        let hash = files["file-0.txt"].as_str().unwrap();
        assert_eq!(hash.len(), 64);
        assert_eq!(
            stored["state"]["master"]["skills"][SKILLS - 1]["files"]["file-49.txt"],
            hash
        );
        assert_eq!(
            stored["state"]["delivered"][".claude/skills/fixture-00/file-0.txt"]["delivered"]
                ["bytes"],
            hash
        );
        let blob = f.setup.data.join("personal-blobs").join(hash);
        assert_eq!(fs::read(&blob).unwrap(), b"original\0\xff");
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            fs::metadata(&blob).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(blob.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );

        let before = MANIFEST_SAVES.get();
        let started = std::time::Instant::now();
        let sync = {
            let root = FileRoot::open(home).unwrap();
            let _lock = f.setup.lock().unwrap();
            let mut manifest = f.setup.load().unwrap().unwrap();
            f.setup
                .sync_locked(
                    &root,
                    &mut manifest,
                    &[],
                    |target, source| {
                        let path = if target == "codexcli" { CODEX } else { CLAUDE };
                        Ok(BTreeMap::from([(
                            PathBuf::from(path),
                            source.as_bytes().to_vec(),
                        )]))
                    },
                    |target, skills| {
                        assert_eq!(target, "claudecode");
                        assert_eq!(skills.len(), SKILLS);
                        let mut output = BTreeMap::new();
                        for (name, files) in skills {
                            let folder = tool_skills.join(name);
                            for (path, original) in files {
                                let bytes = if path == Path::new("SKILL.md") {
                                    original.clone()
                                } else {
                                    b"renderer update\0\xfe".to_vec()
                                };
                                output.insert(folder.join(path), bytes);
                            }
                        }
                        Ok(output)
                    },
                )
                .unwrap()
        };
        let sync_saves = MANIFEST_SAVES.get() - before;
        let sync_time = started.elapsed();
        assert!(sync.blocked.is_empty(), "{:?}", sync.blocked);
        assert_eq!(sync.written.len(), FILES);
        assert!(
            sync_saves <= 8,
            "syncing {FILES} files saved the manifest {sync_saves} times"
        );
        for name in &names {
            for index in 0..ASSETS {
                assert_eq!(
                    fs::read(
                        home.join(tool_skills)
                            .join(name)
                            .join(format!("file-{index}.txt"))
                    )
                    .unwrap(),
                    b"renderer update\0\xfe"
                );
            }
        }

        let before = MANIFEST_SAVES.get();
        let started = std::time::Instant::now();
        let restore = f.setup.restore().unwrap();
        let restore_saves = MANIFEST_SAVES.get() - before;
        let restore_time = started.elapsed();
        assert!(restore.blocked.is_empty(), "{restore:?}");
        assert!(
            restore_saves <= 4,
            "restoring {FILES} files saved the manifest {restore_saves} times"
        );
        for name in &names {
            let folder = home.join(tool_skills).join(name);
            assert_eq!(
                fs::read_link(&folder).unwrap().as_os_str(),
                std::ffi::OsStr::new(&format!("../../sources/{name}"))
            );
            for index in 0..ASSETS {
                assert_eq!(
                    fs::read(folder.join(format!("file-{index}.txt"))).unwrap(),
                    b"original\0\xff"
                );
            }
        }
        assert!(!manifest_file.exists());
        assert!(!f.setup.data.join("personal-blobs").exists());
        eprintln!("1000 files: adopt {adoption_saves} saves in {adoption_time:?}, sync {sync_saves} saves in {sync_time:?}, restore {restore_saves} saves in {restore_time:?}");
    }

    #[cfg(unix)]
    #[test]
    fn run9_crash_during_file_apply_helper() {
        let Some(root) = std::env::var_os("WENLAN_TEST_FILE_APPLY_ROOT").map(PathBuf::from) else {
            return;
        };
        let setup = Setup {
            home: root.join("home"),
            data: root.join("data"),
            binary: "unused".into(),
        };
        let previous = setup.adoptable_manifest().unwrap();
        let adoption = setup.discover_for_adoption(previous.as_ref()).unwrap();
        let master = PersonalMaster::new(&adoption);
        let home = FileRoot::open_home(&setup.home).unwrap();
        let _lock = setup.lock().unwrap();
        setup.apply_adoption(&home, adoption, master).unwrap();
        panic!("the injected process exit was not reached");
    }

    #[cfg(unix)]
    fn run9_crash_during_file_apply(f: &Fixture, point: &str) {
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("rulesync_routes::tests::run9_crash_during_file_apply_helper")
            .env("WENLAN_TEST_FILE_APPLY_ROOT", f._temp.path())
            .env("WENLAN_TEST_FILE_APPLY_CRASH", point)
            .output()
            .unwrap();
        assert_eq!(child.status.code(), Some(76), "{child:?}");
    }

    #[cfg(unix)]
    #[test]
    fn rulesync_keeps_the_existing_setup_after_an_interrupted_extension() {
        let f = Fixture::new();
        let claude = f.text(CLAUDE);
        let codex = f.text(CODEX);
        f.adopt_from_plan();
        let before = f.setup.load().unwrap().unwrap();
        let cursor = ".cursor/AGENTS.md";
        let folder = f.setup.home.join(".cursor/skills/example");
        let source = f.setup.home.join(".agents/skills/example");
        let link = "../../.agents/skills/example";
        fs::create_dir_all(folder.parent().unwrap()).unwrap();
        fs::create_dir_all(&source).unwrap();
        fs::write(
            source.join("SKILL.md"),
            "---\nname: example\ndescription: Skill\nversion: 1\n---\n\nBody.\n",
        )
        .unwrap();
        fs::write(source.join("guide.txt"), b"original guide").unwrap();
        std::os::unix::fs::symlink(link, &folder).unwrap();
        f.write(cursor, "Cursor original.\r\n");
        run9_crash_during_file_apply(&f, "staged:guide.txt");

        let interrupted = f.setup.load().unwrap().unwrap();
        assert!(!interrupted.pending.is_empty());
        assert!(
            interrupted.adopted,
            "a staged addition disabled the existing setup"
        );
        for (path, backup) in &before.backup {
            assert_eq!(&interrupted.backup[path], backup);
            assert_eq!(
                interrupted.delivered[path].delivered,
                before.delivered[path].delivered
            );
        }
        let receipt = f.setup.restore().unwrap();
        assert!(receipt.blocked.is_empty(), "{receipt:?}");
        let recovered = f.setup.load().unwrap().unwrap();
        assert!(recovered.adopted);
        assert_eq!(recovered.master.overlays, before.master.overlays);
        assert_eq!(recovered.backup, before.backup);
        assert_eq!(f.text(cursor), "Cursor original.\r\n");
        assert_eq!(fs::read_link(folder).unwrap(), Path::new(link));
        let synced = f.sync_with("<!-- Updated renderer -->\n");
        assert!(synced.blocked.is_empty(), "{synced:?}");
        assert!(f.text(CLAUDE).starts_with("<!-- Updated renderer -->\n"));
        assert!(f.text(CODEX).starts_with("<!-- Updated renderer -->\n"));
        assert_eq!(f.text(cursor), "Cursor original.\r\n");
        let receipt = f.setup.restore().unwrap();
        assert!(receipt.blocked.is_empty(), "{receipt:?}");
        assert_eq!(f.text(CLAUDE), claude);
        assert_eq!(f.text(CODEX), codex);
        assert!(!f.setup.data.join("personal.json").exists());
    }

    #[test]
    fn rulesync_recovers_a_conflicted_addition_without_disabling_existing_tools() {
        let f = Fixture::new();
        f.adopt_from_plan();
        let _lock = f.setup.lock().unwrap();
        let root = FileRoot::open_home(&f.setup.home).unwrap();
        let mut manifest = f.setup.load().unwrap().unwrap();
        let original_paths = manifest.backup.keys().cloned().collect::<Vec<_>>();
        let cursor = ".cursor/AGENTS.md";
        let gemini = ".gemini/GEMINI.md";
        let muse = ".config/muse/AGENTS.md";
        let mut operations = Vec::new();
        for path in [cursor, gemini, muse] {
            fs::create_dir_all(f.setup.home.join(path).parent().unwrap()).unwrap();
            f.write(path, "Original overlay.\n");
            let seen = root.read(Path::new(path), false).unwrap();
            manifest.backup.insert(path.into(), seen.clone());
            operations.push(Pending {
                path: path.into(),
                seen: seen.clone(),
                desired: with_bytes(&seen, b"Adopted rules.\n".to_vec()),
                kind: Some(Kind::Rule {
                    target: "cursor".into(),
                    source: String::new(),
                    base: String::new(),
                }),
                allow_link: false,
            });
        }
        f.write(muse, "Saved while the addition was applying.\n");
        let outcomes = f
            .setup
            .run_batch(&root, &mut manifest, operations, BatchMode::Extend, None)
            .unwrap();
        assert_eq!(
            outcomes,
            [
                (cursor.into(), ApplyOutcome::Applied),
                (gemini.into(), ApplyOutcome::Applied),
                (muse.into(), ApplyOutcome::ChangedSincePlan),
            ]
        );
        assert_eq!(f.text(cursor), "Adopted rules.\n");
        assert_eq!(f.text(gemini), "Adopted rules.\n");
        f.write(gemini, "Original overlay.\n");

        let mut manifest = f.setup.load().unwrap().unwrap();
        let recovery = f.setup.recover(&root, &mut manifest).unwrap().unwrap();
        assert_eq!(recovery.written, [cursor]);
        assert!(recovery.blocked.is_empty(), "{recovery:?}");
        assert!(manifest.adopted && manifest.extension_paths.is_empty());
        assert_eq!(
            manifest.backup.into_keys().collect::<Vec<_>>(),
            original_paths
        );
        assert_eq!(f.text(cursor), "Original overlay.\n");
        assert_eq!(f.text(gemini), "Original overlay.\n");
        assert_eq!(f.text(muse), "Saved while the addition was applying.\n");
        drop(_lock);
        let synced = f.sync_with("<!-- Updated renderer -->\n");
        assert!(synced.blocked.is_empty(), "{synced:?}");
        assert_eq!(synced.written, [CLAUDE, CODEX]);
    }

    #[cfg(unix)]
    #[test]
    fn run9_restore_recovers_a_crash_with_a_staged_asset_in_an_adopted_skill() {
        let f = Fixture::new();
        let home = &f.setup.home;
        let folder = home.join(".claude/skills/example");
        let source = home.join(".agents/skills/example");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(folder.parent().unwrap()).unwrap();
        fs::write(
            source.join("SKILL.md"),
            "---\nname: example\ndescription: Fixture\n---\n",
        )
        .unwrap();
        fs::write(source.join("asset.txt"), b"source asset").unwrap();
        std::os::unix::fs::symlink("../../.agents/skills/example", &folder).unwrap();
        run9_crash_during_file_apply(&f, "staged:asset.txt");
        assert!(fs::read_dir(&folder).unwrap().flatten().any(|entry| {
            entry
                .file_name()
                .as_encoded_bytes()
                .starts_with(b".wenlan-rules-stage-")
        }));

        let receipt = f.setup.restore().unwrap();
        assert!(receipt.blocked.is_empty(), "{receipt:?}");
        assert_eq!(
            fs::read_link(folder).unwrap().as_os_str(),
            "../../.agents/skills/example"
        );
        assert_eq!(fs::read(source.join("asset.txt")).unwrap(), b"source asset");
    }

    #[cfg(unix)]
    #[test]
    fn run9_restore_removes_created_ancestors_after_a_crash_during_exchange() {
        let f = Fixture::new();
        let home = &f.setup.home;
        let folder = home.join(".claude/skills/example");
        let source = home.join(".agents/skills/example");
        let link = "../../.agents/skills/example";
        fs::create_dir_all(source.join("nested")).unwrap();
        fs::create_dir_all(folder.parent().unwrap()).unwrap();
        fs::write(
            source.join("SKILL.md"),
            "---\nname: example\ndescription: Fixture\n---\n",
        )
        .unwrap();
        fs::write(source.join("nested/asset.txt"), b"original asset").unwrap();
        std::os::unix::fs::symlink(link, &folder).unwrap();
        let codex_before = f.text(CODEX);
        // Force an exchange after the tool skill link has become directories
        // and files owned by the applier.
        f.write(CODEX, &format!("<!-- Generated by ~/.agents/scripts/sync-codex-agents.sh. Rerun. -->\n{codex_before}"));
        let codex_before = f.text(CODEX);
        run9_crash_during_file_apply(&f, "displaced:AGENTS.md");
        assert!(!fs::symlink_metadata(&folder).unwrap().is_symlink());
        assert_eq!(
            fs::read(folder.join("nested/asset.txt")).unwrap(),
            b"original asset"
        );
        assert_ne!(f.text(CODEX), codex_before);
        let receipt = f.setup.restore().unwrap();
        assert!(receipt.blocked.is_empty(), "{receipt:?}");
        assert_eq!(f.text(CODEX), codex_before);
        assert_eq!(fs::read_link(folder).unwrap(), Path::new(link));
        assert_eq!(
            fs::read(source.join("nested/asset.txt")).unwrap(),
            b"original asset"
        );
        assert_eq!(
            fs::read_dir(f.setup.home.join(".codex")).unwrap().count(),
            1
        );
    }

    #[cfg(unix)]
    #[test]
    fn run9_restore_preserves_special_permission_bits() {
        use std::os::unix::fs::PermissionsExt as _;
        for mode in [0o1750, 0o2750, 0o4750] {
            let f = Fixture::new();
            let path = f.setup.home.join(CLAUDE);
            fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
            let original = fs::metadata(&path).unwrap().permissions().mode() & 0o7777;
            if mode != 0o1750 && original != mode {
                // Some test sandboxes silently strip setuid and setgid.
                continue;
            }
            assert_eq!(original, mode);
            f.adopt_from_plan();
            assert!(f.setup.restore().unwrap().blocked.is_empty());
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o7777,
                mode
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn rulesync_restores_after_interruption_in_the_middle_of_adoption() {
        let f = Fixture::new();
        let home = &f.setup.home;
        let root = FileRoot::open(home).unwrap();
        let mut operations = Vec::new();
        for name in ["landed", "untouched"] {
            let source = home.join(".agents/skills").join(name);
            fs::create_dir_all(&source).unwrap();
            fs::write(source.join("SKILL.md"), format!("{name} source\n")).unwrap();
            fs::create_dir_all(home.join(".claude/skills")).unwrap();
            let folder = Path::new(".claude/skills").join(name);
            let link = format!("../../.agents/./skills/{name}");
            std::os::unix::fs::symlink(&link, home.join(&folder)).unwrap();
            operations.push(Pending {
                path: folder.clone(),
                seen: root.read(&folder, true).unwrap(),
                desired: Snapshot::private_directory(),
                kind: Some(Kind::Directory),
                allow_link: true,
            });
            for file in ["SKILL.md", "asset.txt"] {
                operations.push(Pending {
                    path: folder.join(file),
                    seen: Snapshot::Absent,
                    desired: Snapshot::file(b"new copy".to_vec()),
                    kind: Some(Kind::Skill),
                    allow_link: true,
                });
            }
        }
        {
            let _lock = f.setup.lock().unwrap();
            let adoption = discover_adoption(home).unwrap();
            let backup = operations
                .iter()
                .map(|op| (op.path.clone(), op.seen.clone()))
                .collect();
            let mut manifest = Manifest::new(PersonalMaster::new(&adoption), backup);
            manifest.pending = operations;
            f.setup.save(&manifest).unwrap();
            // Kill point: two renames reached the tool folder, now lose all
            // in-memory progress without checkpointing the completed entries.
            for op in &manifest.pending[..2] {
                assert_eq!(
                    root.apply(&op.path, &op.seen, &op.desired, op.allow_link)
                        .unwrap(),
                    ApplyOutcome::Applied
                );
            }
        }
        let receipt = f.setup.restore().unwrap();
        assert!(receipt.blocked.is_empty(), "{receipt:?}");
        assert_eq!(receipt.written.len(), 2);
        for name in ["landed", "untouched"] {
            let folder = home.join(".claude/skills").join(name);
            assert_eq!(
                fs::read_link(&folder).unwrap().as_os_str(),
                std::ffi::OsStr::new(&format!("../../.agents/./skills/{name}"))
            );
            assert_eq!(
                fs::read_to_string(folder.join("SKILL.md")).unwrap(),
                format!("{name} source\n")
            );
        }
        assert!(!f.setup.data.join("personal.json").exists());
    }

    #[cfg(unix)]
    #[test]
    fn rulesync_resumes_restore_after_the_directory_link_is_already_back() {
        let f = Fixture::new();
        let home = &f.setup.home;
        fs::create_dir_all(home.join(".agents/skills/shared")).unwrap();
        fs::create_dir_all(home.join(".claude/skills")).unwrap();
        let skill = ".agents/skills/shared/SKILL.md";
        let source = "---\nname: shared\ndescription: Fixture\n---\nsource\n";
        f.write(skill, source);
        let folder = ".claude/skills/shared";
        std::os::unix::fs::symlink("../../.agents/skills/shared", home.join(folder)).unwrap();
        let original_rule = f.text(CLAUDE);
        f.adopt_from_plan();
        {
            let root = FileRoot::open(home).unwrap();
            let _lock = f.setup.lock().unwrap();
            let mut manifest = f.setup.load().unwrap().unwrap();
            manifest.adopted = false;
            manifest.pending = [".claude/skills/shared/SKILL.md", folder, CLAUDE]
                .into_iter()
                .map(|name| {
                    let path = PathBuf::from(name);
                    Pending {
                        seen: manifest.delivered[&path].delivered.clone(),
                        desired: manifest.backup[&path].clone(),
                        path,
                        kind: None,
                        allow_link: true,
                    }
                })
                .collect();
            f.setup.save(&manifest).unwrap();
            for op in &manifest.pending[..2] {
                assert_eq!(
                    root.apply(&op.path, &op.seen, &op.desired, op.allow_link)
                        .unwrap(),
                    ApplyOutcome::Applied
                );
            }
        }
        let receipt = f.setup.restore().unwrap();
        assert!(receipt.blocked.is_empty(), "{receipt:?}");
        assert_eq!(f.text(CLAUDE), original_rule);
        assert_eq!(f.text(skill), source);
        assert_eq!(
            fs::read_link(home.join(folder)).unwrap().as_os_str(),
            "../../.agents/skills/shared"
        );
        assert!(!f.setup.data.join("personal.json").exists());
    }

    #[test]
    fn rulesync_refuses_the_old_format_and_keeps_its_backup() {
        let f = Fixture::new();
        let old = br#"{"master":{"shared":""},"backup":{},"delivered":{},"pending":null,"adopted":false}"#;
        let path = f.setup.data.join("personal.json");
        fs::write(&path, old).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let original = f.text(CLAUDE);
        let message = f.setup.restore().unwrap_err().to_string();
        assert!(message.contains("wenlan rules restore"), "{message}");
        assert!(message.contains("adopt again"), "{message}");
        assert_eq!(fs::read(path).unwrap(), old);
        assert_eq!(f.text(CLAUDE), original);
        assert!(!f.setup.data.join("personal-blobs").exists());
    }

    #[test]
    fn rulesync_refuses_a_damaged_backup_blob_before_restoring() {
        let f = Fixture::new();
        f.adopt_from_plan();
        let state = f.setup.data.join("personal.json");
        let before = fs::read(&state).unwrap();
        let stored: serde_json::Value = serde_json::from_slice(&before).unwrap();
        let hash = stored["state"]["backup"][CLAUDE]["bytes"].as_str().unwrap();
        fs::write(f.setup.data.join("personal-blobs").join(hash), b"damaged").unwrap();
        let delivered = f.text(CLAUDE);
        let error = f.setup.restore().unwrap_err();
        assert!(error.to_string().contains("damaged personal rules blob"));
        assert_eq!(f.text(CLAUDE), delivered);
        assert_eq!(fs::read(state).unwrap(), before);
    }

    #[cfg(unix)]
    #[test]
    fn run9_backup_blobs_stay_private_when_reused_and_require_private_directories() {
        use std::os::unix::fs::PermissionsExt as _;
        let f = Fixture::new();
        f.adopt_from_plan();
        let lock = f.setup.lock().unwrap();
        let manifest = f.setup.load().unwrap().unwrap();
        let state_path = f.setup.data.join("personal.json");
        let stored: serde_json::Value =
            serde_json::from_slice(&fs::read(&state_path).unwrap()).unwrap();
        let hash = stored["state"]["backup"][CLAUDE]["bytes"].as_str().unwrap();
        let blobs = f.setup.data.join("personal-blobs");
        let blob = blobs.join(hash);

        fs::set_permissions(&state_path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(f.setup.load().is_err());
        fs::set_permissions(&state_path, fs::Permissions::from_mode(0o600)).unwrap();

        fs::set_permissions(&blob, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(f.setup.load().is_err());

        let alias = f._temp.path().join("outside-blob");
        fs::hard_link(&blob, &alias).unwrap();
        let error = f.setup.save(&manifest).unwrap_err();
        assert!(error
            .to_string()
            .contains("hard-linked personal rules file"));
        assert_eq!(
            fs::metadata(&alias).unwrap().permissions().mode() & 0o777,
            0o644
        );
        fs::remove_file(alias).unwrap();

        f.setup.save(&manifest).unwrap();
        assert_eq!(
            fs::metadata(&blob).unwrap().permissions().mode() & 0o777,
            0o600
        );
        for dir in [&blobs, &f.setup.data] {
            fs::set_permissions(dir, fs::Permissions::from_mode(0o755)).unwrap();
            assert!(
                f.setup.load().is_err(),
                "read a backup through exposed directory {dir:?}"
            );
            assert_eq!(
                fs::metadata(dir).unwrap().permissions().mode() & 0o777,
                0o755
            );
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
        }
        drop(lock);
        fs::set_permissions(&f.setup.data, fs::Permissions::from_mode(0o755)).unwrap();
        let _lock = f.setup.lock().unwrap();
        assert_eq!(
            fs::metadata(&f.setup.data).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert!(f.setup.load().unwrap().is_some());
    }

    #[test]
    fn run9_status_rejects_an_oversized_managed_file_before_reading() {
        let f = Fixture::new();
        f.adopt_from_plan();
        fs::OpenOptions::new()
            .write(true)
            .open(f.setup.home.join(CODEX))
            .unwrap()
            .set_len(32 * 1024 * 1024 + 1)
            .unwrap();
        assert_eq!(
            f.setup.delivery_status()["codex"].blocked,
            Some("file_too_large")
        );
    }

    #[test]
    fn run9_recovery_reports_one_post_crash_edit_and_restores_the_other_paths() {
        let f = Fixture::new();
        let original = f.text(CLAUDE);
        f.adopt_from_plan();
        {
            let _lock = f.setup.lock().unwrap();
            let mut manifest = f.setup.load().unwrap().unwrap();
            for name in [CODEX, CLAUDE] {
                let path = PathBuf::from(name);
                let record = &manifest.delivered[&path];
                manifest.pending.push(Pending {
                    path,
                    seen: record.delivered.clone(),
                    desired: with_bytes(&record.delivered, b"next release\n".to_vec()),
                    kind: Some(record.kind.clone()),
                    allow_link: false,
                });
            }
            f.setup.save(&manifest).unwrap();
            f.write(CLAUDE, "next release\n");
        }
        f.write(CODEX, "editor saved while the daemon was down\n");
        let receipt = f.setup.restore().unwrap();
        assert_eq!(receipt.written, [CLAUDE]);
        assert_eq!(receipt.blocked.len(), 1, "{receipt:?}");
        assert!(receipt.blocked[0].starts_with(CODEX));
        assert_eq!(f.text(CLAUDE), original);
        assert_eq!(f.text(CODEX), "editor saved while the daemon was down\n");
        let manifest = f.setup.load().unwrap().unwrap();
        assert_eq!(manifest.pending.len(), 1);
        assert_eq!(manifest.pending[0].path, Path::new(CODEX));
    }

    #[test]
    fn adoption_replaces_the_codex_header_and_restore_keeps_the_script() {
        let f = Fixture::new();
        let script = ".agents/scripts/sync-codex-agents.sh";
        fs::create_dir(f.setup.home.join(".agents/scripts")).unwrap();
        f.write(script, "#!/bin/sh\n");
        let legacy = format!("<!-- Generated by ~/{script}. Rerun. -->\n{SHARED}Codex only.\n");
        f.write(CODEX, &legacy);
        f.adopt_from_plan();

        let adopted = f.text(CODEX);
        assert!(adopted.starts_with("<!-- Managed by Wenlan."));
        assert!(adopted.ends_with(&format!("{SHARED}Codex only.\n")));
        assert!(!adopted.contains("Generated by"));
        assert!(!f.text(CLAUDE).contains("Managed by Wenlan"));
        assert_eq!(f.text(script), "#!/bin/sh\n");
        assert!(f.sync_with("").proposals.is_empty());
        assert_eq!(f.text(CODEX), adopted);

        f.setup.restore().unwrap();
        assert_eq!(f.text(CODEX), legacy);
        assert_eq!(f.text(script), "#!/bin/sh\n");
    }

    #[test]
    #[cfg(unix)]
    fn codex_delivery_reports_its_link_and_managed_state() {
        let f = Fixture::new();
        let codex = f.setup.home.join(CODEX);
        fs::rename(&codex, f.setup.home.join(".agents/generated.md")).unwrap();
        std::os::unix::fs::symlink("../.agents/generated.md", &codex).unwrap();
        let before = f.setup.delivery_status();
        assert_eq!(before["codex"].path, codex);
        assert_eq!(before["codex"].blocked, Some("symlink_requires_adoption"));
        f.adopt_from_plan();
        let after = f.setup.delivery_status();
        assert!(after["codex"].at.is_some());
        assert_eq!(after["codex"].blocked, None);
        assert!(after["claude-code"].at.is_some());
        f.write(CODEX, "tool edit\n");
        assert_eq!(
            f.setup.delivery_status()["codex"].blocked,
            Some("local_edit_pending")
        );
    }

    #[test]
    fn rulesync_reports_codex_blocked_until_a_render_succeeds() {
        let f = Fixture::new();
        f.adopt_from_plan();
        {
            let root = FileRoot::open_home(&f.setup.home).unwrap();
            let _lock = f.setup.lock().unwrap();
            let mut manifest = f.setup.load().unwrap().unwrap();
            f.setup
                .sync_locked(
                    &root,
                    &mut manifest,
                    &[],
                    |target, text| {
                        if target == "codexcli" {
                            bail!("failed to render Codex");
                        }
                        Ok(BTreeMap::from([(
                            PathBuf::from(CLAUDE),
                            text.as_bytes().to_vec(),
                        )]))
                    },
                    fixture_skill_renderer,
                )
                .unwrap();
        }
        let status = f.setup.delivery_status();
        assert_eq!(status["codex"].blocked, Some("renderer_unavailable"));
        assert!(status["codex"].at.is_none());
        assert!(status["claude-code"].at.is_some());
        assert!(f.sync_with("").blocked.is_empty());
        assert!(f.setup.delivery_status()["codex"].at.is_some());
    }

    #[test]
    fn ux_skill_15_rulesync_adopt_and_restore_exact_bytes_and_links() {
        let f = Fixture::new();
        #[cfg(unix)]
        {
            let codex = f.setup.home.join(CODEX);
            fs::rename(&codex, f.setup.home.join(".agents/generated.md")).unwrap();
            std::os::unix::fs::symlink("../.agents/./generated.md", codex).unwrap();
        }
        let original_claude = f.text(CLAUDE);
        let original_codex = f.text(CODEX);
        let plan = wenlan_core::rulesync_personal::plan_adoption(&f.setup.home).unwrap();
        assert_eq!(plan.files.len(), 2);
        assert!(!f.setup.data.join("personal.json").exists());
        f.adopt_from_plan();
        assert_eq!(f.text(CLAUDE), format!("{SHARED}Claude only.\n"));
        assert_eq!(f.text(CODEX), original_codex);
        assert!(!fs::symlink_metadata(f.setup.home.join(CODEX))
            .unwrap()
            .is_symlink());
        let result = f.setup.restore().unwrap();
        assert!(result.blocked.is_empty(), "{result:?}");
        assert_eq!(f.text(CLAUDE), original_claude);
        assert_eq!(f.text(CODEX), original_codex);
        #[cfg(unix)]
        assert_eq!(
            fs::read_link(f.setup.home.join(CODEX)).unwrap().as_os_str(),
            "../.agents/./generated.md"
        );
    }

    #[test]
    fn rulesync_pulls_back_deletions_for_the_edited_tool_and_retains_appends_for_review() {
        let f = Fixture::new();
        f.adopt_from_plan();
        f.write(CODEX, &f.text(CODEX).replace("Second rule.\n", ""));
        let result = f.sync_with("");
        assert_eq!(result.proposals.len(), 1, "{result:?}");
        assert!(result.proposals[0].reason.contains("Kept for this tool"));
        assert!(f.text(CLAUDE).contains("Second rule."));
        assert!(!f.text(CODEX).contains("Second rule."));
        let mut changed = f.text(CODEX);
        changed.push_str("Tool appended this.\n");
        f.write(CODEX, &changed);
        let result = f.sync_with("");
        assert_eq!(result.proposals.len(), 1, "{result:?}");
        assert!(result.proposals[0].diff.contains("+Tool appended this."));
        assert!(!f.text(CLAUDE).contains("Tool appended this."));
        assert_eq!(f.text(CODEX), changed);
    }

    #[test]
    fn rulesync_competing_edits_to_the_same_line_form_one_card() {
        let f = Fixture::new();
        f.adopt_from_plan();
        f.write(
            CODEX,
            &f.text(CODEX).replace("One rule.", "Codex's wording."),
        );
        f.write(
            CLAUDE,
            &f.text(CLAUDE).replace("One rule.", "Claude's wording."),
        );
        let result = f.sync_with("");
        assert_eq!(result.proposals.len(), 1, "{result:?}");
        assert_eq!(result.proposals[0].files, [CLAUDE, CODEX]);
        assert!(f.text(CODEX).contains("Codex's wording."));
        assert!(f.text(CLAUDE).contains("Claude's wording."));
    }

    #[test]
    fn rulesync_all_tools_review_replaces_both_competing_edits() {
        let f = Fixture::new();
        f.adopt_from_plan();
        let codex = f.text(CODEX).replace("One rule.", "Codex's wording.");
        let claude = f.text(CLAUDE).replace("One rule.", "Claude's wording.");
        f.write(CODEX, &codex);
        f.write(CLAUDE, &claude);
        let proposed = f.sync_with("");
        assert_eq!(proposed.proposals.len(), 1, "{proposed:?}");
        assert_eq!(proposed.proposals[0].files, [CLAUDE, CODEX]);

        let mut decision = PullbackDecision {
            id: proposed.proposals[0].id.clone(),
            action: PullbackAction::AllTools,
            file: None,
        };
        let error = f
            .setup
            .review_using(&decision, |_, _| panic!("review had no chosen edit"))
            .unwrap_err();
        assert!(error.to_string().contains("choose a file"));
        assert_eq!(f.text(CODEX), codex);
        assert_eq!(f.text(CLAUDE), claude);

        decision.file = Some(CLAUDE.into());
        let reviewed = f
            .setup
            .review_using(&decision, |root, manifest| f.sync_review(root, manifest))
            .unwrap();
        assert!(reviewed.blocked.is_empty(), "{reviewed:?}");
        assert!(reviewed.proposals.is_empty(), "{reviewed:?}");
        assert_eq!(f.text(CLAUDE), claude);
        assert_eq!(
            f.text(CODEX),
            codex.replace("Codex's wording.", "Claude's wording.")
        );
        let state = f.setup.load().unwrap().unwrap();
        assert!(state.master.overrides.is_empty());
        assert!(state.scope_reviews.is_empty());
        assert!(f.sync_with("").written.is_empty());
    }

    #[test]
    fn rulesync_all_tools_review_preserves_an_edit_after_the_decision() {
        let f = Fixture::new();
        f.adopt_from_plan();
        let claude = f.text(CLAUDE).replace("One rule.", "Claude's wording.");
        let codex = f.text(CODEX).replace("One rule.", "Codex's wording.");
        f.write(CLAUDE, &claude);
        f.write(CODEX, &codex);
        let proposed = f.sync_with("");
        assert_eq!(proposed.proposals.len(), 1, "{proposed:?}");
        let decision = PullbackDecision {
            id: proposed.proposals[0].id.clone(),
            action: PullbackAction::AllTools,
            file: Some(CLAUDE.into()),
        };
        let newer = codex.replace("Codex's wording.", "Edited during review.");
        let reviewed = f
            .setup
            .review_using(&decision, |root, manifest| {
                f.write(CODEX, &newer);
                f.sync_review(root, manifest)
            })
            .unwrap();
        assert_eq!(f.text(CODEX), newer);
        assert_eq!(f.text(CLAUDE), claude);
        assert_eq!(reviewed.proposals.len(), 1, "{reviewed:?}");
        assert_eq!(reviewed.proposals[0].files, [CODEX]);
        assert_ne!(reviewed.proposals[0].id, decision.id);
        assert!(reviewed.proposals[0]
            .diff
            .contains("+Edited during review."));
        assert!(reviewed.blocked.iter().any(|line| line.starts_with(CODEX)));
        assert!(f.setup.load().unwrap().unwrap().master.overrides.is_empty());

        let next = f.sync_with("");
        assert_eq!(next.proposals, reviewed.proposals);
        assert_eq!(f.text(CODEX), newer);
        let dismiss = PullbackDecision {
            id: next.proposals[0].id.clone(),
            action: PullbackAction::Dismiss,
            file: None,
        };
        let dismissed = f
            .setup
            .review_using(&dismiss, |root, manifest| f.sync_review(root, manifest))
            .unwrap();
        assert!(dismissed.blocked.is_empty(), "{dismissed:?}");
        assert!(dismissed.proposals.is_empty(), "{dismissed:?}");
        assert_eq!(
            f.text(CODEX),
            codex.replace("Codex's wording.", "Claude's wording.")
        );
    }

    #[test]
    fn rulesync_tool_only_review_leaves_the_other_proposal_pending() {
        let f = Fixture::new();
        f.adopt_from_plan();
        let claude = f.text(CLAUDE).replace("One rule.", "Claude's wording.");
        let codex = format!(
            "{}New rule for Codex.\n",
            f.text(CODEX).replace("One rule.", "Codex's wording.")
        );
        f.write(CLAUDE, &claude);
        f.write(CODEX, &codex);
        let proposed = f.sync_with("");
        assert_eq!(proposed.proposals.len(), 1, "{proposed:?}");
        assert_eq!(proposed.proposals[0].files, [CLAUDE, CODEX]);
        let decision = PullbackDecision {
            id: proposed.proposals[0].id.clone(),
            action: PullbackAction::ToolOnly,
            file: Some(CLAUDE.into()),
        };
        let reviewed = f
            .setup
            .review_using(&decision, |root, manifest| f.sync_review(root, manifest))
            .unwrap();
        assert_eq!(reviewed.proposals.len(), 1, "{reviewed:?}");
        assert_eq!(reviewed.proposals[0].files, [CODEX]);
        assert_eq!(f.text(CLAUDE), claude);
        assert_eq!(f.text(CODEX), codex);
        let state = f.setup.load().unwrap().unwrap();
        assert_eq!(state.master.shared, SHARED);
        assert_eq!(state.master.overrides.len(), 1);
    }

    #[test]
    fn rulesync_pin_output_changes_are_never_pulled_back_as_user_edits() {
        let f = Fixture::new();
        f.adopt_from_plan();
        let result = f.sync_with("# New renderer preamble\n\n");
        assert_eq!(result.written.len(), 2);
        assert!(result.proposals.is_empty());
        let result = f.sync_with("# New renderer preamble\n\n");
        assert!(result.proposals.is_empty());
        assert!(result.written.is_empty());
        let state = f.setup.load().unwrap().unwrap();
        assert_eq!(state.master.shared, SHARED);
        assert!(state.master.overrides.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn rulesync_linked_skill_keeps_original_subset_and_restore_shape() {
        let f = Fixture::new();
        let home = &f.setup.home;
        for path in [
            ".agents/skills/shared",
            ".claude/skills",
            ".codex/skills/handmade",
        ] {
            fs::create_dir_all(home.join(path)).unwrap();
        }
        let original = "---\nname: shared\ndescription: fixture\n---\nMy skill.\n";
        let handmade = "---\nname: handmade\ndescription: Handmade skill\n---\nHandmade\n";
        fs::write(home.join(".agents/skills/shared/SKILL.md"), original).unwrap();
        fs::write(home.join(".codex/skills/handmade/SKILL.md"), handmade).unwrap();
        let link = "../../.agents/skills/shared";
        std::os::unix::fs::symlink(link, home.join(".claude/skills/shared")).unwrap();
        f.adopt_from_plan();
        assert!(home.join(".claude/skills/shared/SKILL.md").is_file());
        assert!(!fs::symlink_metadata(home.join(".claude/skills/shared"))
            .unwrap()
            .is_symlink());
        assert!(!home.join(".codex/skills/shared").exists());
        assert_eq!(f.text(".codex/skills/handmade/SKILL.md"), handmade);
        let restored = f.setup.restore().unwrap();
        assert!(restored.blocked.is_empty(), "{restored:?}");
        assert_eq!(
            fs::read_link(home.join(".claude/skills/shared"))
                .unwrap()
                .as_os_str(),
            link
        );
        assert_eq!(f.text(".agents/skills/shared/SKILL.md"), original);
        assert_eq!(f.text(".codex/skills/handmade/SKILL.md"), handmade);
    }

    #[test]
    fn rulesync_setup_rejects_unauthenticated_callers() {
        assert!(require_local_person(&HeaderMap::new()).is_err());
    }
}
