// SPDX-License-Identifier: Apache-2.0
//! The local setup endpoints. The same file applier handles enrolment,
//! delivery and restoration.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, ensure, Context, Result};
use axum::{extract::State, http::HeaderMap, Json};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use wenlan_core::rulesync_adopt::{AdoptedSkill, Adoption, AdoptionEntry};
use wenlan_core::rulesync_personal::{
    adoption_inputs, describe_change, discover_adoption, fingerprint, line_changes,
    merge_rule_text, promote_changes, system_home, write_root_rule, LineChange, LocalText,
    PersonalMaster,
};
use wenlan_core::rulesync_renderer;
use wenlan_core::rulesync_team::{self, TeamRuleLayer};
use wenlan_types::rulesync::{
    AdoptRequest, AdoptedSkillUpdate, PullbackAction, PullbackCard, PullbackDecision,
    RulesSyncReport,
};
use wenlan_types::rulesync_skill::SavedSkillTool;

use crate::error::ServerError;
use crate::route_registry::{post, TrackedRouter};
use crate::rulesync_apply::{ApplyOutcome, FileRoot, Snapshot};
use crate::rulesync_layers;
use crate::rulesync_registry::{self, ManagedSkill};
use crate::state::SharedState;

mod storage;

pub(crate) fn register(router: TrackedRouter<SharedState>) -> TrackedRouter<SharedState> {
    router
        .route("/api/rules/setup/adopt", post(adopt))
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

    require_local_person(&headers)?;
    if !wenlan_core::rulesync_personal::enabled() {
        return Err(ServerError::ValidationError(
            "WENLAN_RULESYNC is off".into(),
        ));
    }
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
    require_local_person(&headers)?;
    if !wenlan_core::rulesync_personal::enabled() {
        return Err(ServerError::ValidationError(
            "WENLAN_RULESYNC is off".into(),
        ));
    }
    let generated = if matches!(operation, Operation::Sync | Operation::Review(_)) {
        crate::skill_grow::general_tool_rules(&state).await?
    } else {
        Vec::new()
    };
    let result = tokio::task::spawn_blocking(move || {
        let setup = Setup::system()?;
        match operation {
            Operation::Adopt(request) => setup.adopt(&request.token),
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
        Ok(Snapshot::Directory) => (None, Some("not_a_file")),
        Ok(Snapshot::Absent) => (None, Some("managed_file_missing")),
        Ok(Snapshot::File { .. }) => (None, Some("local_edit_pending")),
        Err(_) => (None, Some("not_readable")),
    }
}

fn agent_for_target(target: &str) -> &str {
    match target {
        "claudecode" => "claude-code",
        "codexcli" => "codex",
        _ => target,
    }
}

fn renderer_skill_tool(target: &str) -> Option<SavedSkillTool> {
    match target {
        "claudecode" => Some(SavedSkillTool::ClaudeCode),
        "codexcli" => Some(SavedSkillTool::Codex),
        _ => None,
    }
}

fn path_blocked(blocked: &BTreeSet<PathBuf>, path: &Path) -> bool {
    path.ancestors()
        .any(|candidate| blocked.contains(candidate))
}

impl Setup {
    fn delivery_status(&self) -> BTreeMap<String, DeliveryStatus> {
        let Ok(home) = FileRoot::open(&self.home) else {
            return BTreeMap::new();
        };
        let mut tools = BTreeMap::new();
        for candidate in adoption_inputs() {
            let Some(path) = candidate.rules_path else {
                continue;
            };
            if matches!(home.read(&path, true), Ok(Snapshot::Symlink { .. })) {
                tools.insert(
                    agent_for_target(&candidate.tool).into(),
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
        for (path, delivery) in &manifest.delivered {
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
    let root = FileRoot::open(&setup.home)?;
    setup.recover(&root, &mut manifest)?;
    ensure!(manifest.adopted, "finish restoring the local setup first");
    result.message = "Checked installed skills".into();
    let blocked = setup.pull_back(&root, &mut manifest, &mut result)?;
    setup.sync_registry(&root, &mut manifest, &blocked, &mut result)?;
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
    let root = FileRoot::open(&setup.home)?;
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
    let root = FileRoot::open(&setup.home)?;
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
    RegistrySkill {
        name: String,
    },
    RegistryDirectory {
        name: String,
    },
    RegistryAssetDirectory {
        name: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct Delivery<Content = Vec<u8>> {
    kind: Kind,
    delivered: Snapshot<Content>,
    observed: Option<Snapshot<Content>>,
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
    Continue,
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
            if operation.seen == Snapshot::Directory && operation.desired != Snapshot::Directory {
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
    delivered: BTreeMap<PathBuf, Delivery<Content>>,
    pending: Vec<Pending<Content>>,
    adopted: bool,
    #[serde(default)]
    registry_skipped: BTreeMap<String, BTreeSet<PathBuf>>,
    #[serde(default)]
    unmanaged_skills: BTreeSet<PathBuf>,
    #[serde(default)]
    scope_reviews: BTreeMap<String, ScopeReview>,
    #[serde(default)]
    delivery_issues: BTreeMap<PathBuf, RuleDeliveryIssue>,
}

impl Manifest {
    fn new(master: PersonalMaster, backup: BTreeMap<PathBuf, Snapshot>) -> Self {
        Self {
            master,
            backup,
            delivered: BTreeMap::new(),
            pending: Vec::new(),
            adopted: false,
            registry_skipped: BTreeMap::new(),
            unmanaged_skills: BTreeSet::new(),
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

    pub(crate) fn lock(&self) -> Result<fs::File> {
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
        let metadata = fs::symlink_metadata(&self.data)?;
        ensure!(
            metadata.is_dir(),
            "rulesync data path must be a regular directory"
        );
        let directory =
            cap_std::fs::Dir::open_ambient_dir(&self.data, cap_std::ambient_authority())?;
        use cap_fs_ext::OpenOptionsFollowExt;
        let lock = directory
            .open_with(
                "personal.lock",
                cap_std::fs::OpenOptions::new()
                    .write(true)
                    .read(true)
                    .create(true)
                    .follow(cap_fs_ext::FollowSymlinks::No),
            )?
            .into_std();
        FileExt::lock_exclusive(&lock)?;
        Ok(lock)
    }

    fn adopt(&self, token: &str) -> Result<RulesSyncReport> {
        let adoption = discover_adoption(&self.home)?;
        ensure!(
            fingerprint(&adoption)? == token,
            "the adoption plan changed. Review it again"
        );
        let root = FileRoot::open(&self.home)?;
        let _lock = self.lock()?;
        ensure!(
            self.load()?.is_none(),
            "an adoption is already recorded. Restore it before adopting again"
        );
        let master = PersonalMaster::new(&adoption);
        let inputs = adoption_inputs();
        // Match the plan against the pinned renderer before putting anything
        // in a tool folder. No configuration features are requested.
        for input in &inputs {
            let Some(path) = input.rules_path.as_deref() else {
                continue;
            };
            let Some(file) = adoption.files.get(path) else {
                continue;
            };
            let AdoptionEntry::File(wanted) = &file.after else {
                continue;
            };
            let source = master.rule_text(&input.tool, &[])?;
            let output = self.render_rule(&input.tool, &source)?;
            let actual = output
                .get(path)
                .context("rulesync no longer renders the planned rules file")?;
            ensure!(
                same_rule_lines(actual, wanted)?,
                "rulesync changed the lines in {}; adoption needs a fresh preview",
                path.display()
            );
        }
        ensure!(
            fingerprint(&discover_adoption(&self.home)?)? == token,
            "the source changed during rendering. Review the plan again"
        );
        self.apply_adoption(&root, adoption, master)
    }

    fn apply_adoption(
        &self,
        root: &FileRoot,
        adoption: Adoption,
        master: PersonalMaster,
    ) -> Result<RulesSyncReport> {
        let mut operations = Vec::new();
        let mut backup = BTreeMap::new();
        let rules: BTreeMap<_, _> = adoption_inputs()
            .into_iter()
            .filter_map(|input| input.rules_path.map(|path| (path, input.tool)))
            .collect();
        let mut links = BTreeSet::new();
        for (path, change) in adoption.files {
            let inside_adopted_link = links.iter().any(|link: &PathBuf| path.starts_with(link));
            let seen = if inside_adopted_link {
                Snapshot::Absent
            } else {
                root.read(&path, true)?
            };
            ensure!(
                adoption_entry_matches(&change.before, &seen),
                "{} changed since the plan",
                path.display()
            );
            let desired = match change.after {
                AdoptionEntry::File(bytes) => {
                    if matches!(seen, Snapshot::Symlink { .. }) || inside_adopted_link {
                        // Discovery has already checked that the resolved
                        // content stays within this home. Copy only the plan
                        // bytes. Use original executable/read permissions.
                        source_mode(&self.home.join(&path), bytes)?
                    } else {
                        with_bytes(&seen, bytes)
                    }
                }
                AdoptionEntry::Directory => Snapshot::Directory,
                _ => bail!("unexpected adoption output at {}", path.display()),
            };
            if matches!(seen, Snapshot::Symlink { .. }) && desired == Snapshot::Directory {
                links.insert(path.clone());
            }
            let kind = match rules.get(&path) {
                Some(tool) => Kind::Rule {
                    target: tool.clone(),
                    source: master.rule_text(tool, &[])?,
                    base: master.base_rule_text(tool, &[]),
                },
                None if desired == Snapshot::Directory => Kind::Directory,
                None => Kind::Skill,
            };
            backup.insert(path.clone(), seen.clone());
            operations.push(Pending {
                path,
                seen,
                desired,
                kind: Some(kind),
                allow_link: true,
            });
        }
        let mut manifest = Manifest::new(master, backup);
        manifest.unmanaged_skills = adoption.skipped.into_keys().collect();
        let mut result = report("Adopted the local tool setup");
        for (path, status) in self.apply_batch(root, &mut manifest, operations, BatchMode::Adopt)? {
            if status != ApplyOutcome::Applied {
                bail!(
                    "{} changed during adoption. Run rules restore to roll back completed files",
                    path.display()
                );
            }
            result.written.push(path.display().to_string());
        }
        if !manifest.unmanaged_skills.is_empty() {
            result
                .message
                .push_str(". Skill paths marked unmanaged in the plan stayed in place.");
        }
        Ok(result)
    }

    fn recover(&self, root: &FileRoot, manifest: &mut Manifest) -> Result<()> {
        if manifest.pending.is_empty() {
            return Ok(());
        }
        let pending = std::mem::take(&mut manifest.pending);
        // A directory link may conceal paths in a partially adopted/restored
        // tree. Inspect the link itself, then reason about its journaled
        // children without reading through it.
        let mut linked = BTreeMap::new();
        for op in &pending {
            if matches!(op.seen, Snapshot::Symlink { .. }) && op.desired == Snapshot::Directory {
                if root.read(&op.path, true)? == op.seen {
                    linked.insert(op.path.clone(), false);
                }
            } else if op.seen == Snapshot::Directory
                && matches!(op.desired, Snapshot::Symlink { .. })
                && root.read(&op.path, true)? == op.desired
            {
                linked.insert(op.path.clone(), true);
            }
        }
        let mut directories = BatchDirectories::default();
        for op in pending {
            let completed = match op
                .path
                .ancestors()
                .skip(1)
                .find_map(|path| linked.get(path))
            {
                Some(&restored) => {
                    let expected = if restored { &op.desired } else { &op.seen };
                    ensure!(
                        expected == &Snapshot::Absent,
                        "inconsistent personal rules journal beneath a directory link"
                    );
                    restored
                }
                None => {
                    let actual = root.read(&op.path, op.allow_link)?;
                    let link_still_matches = linked.get(&op.path).is_none_or(|restored| {
                        let expected = if *restored { &op.desired } else { &op.seen };
                        &actual == expected
                    });
                    ensure!(
                        link_still_matches && (actual == op.desired || actual == op.seen),
                        "{} changed while Wenlan was applying it. The journal and backup are retained",
                        op.path.display()
                    );
                    actual == op.desired
                }
            };
            if completed {
                directories.record(&op);
                settle(manifest, op);
            }
        }
        directories.sync(root)?;
        self.save(manifest)
    }

    fn apply_batch(
        &self,
        root: &FileRoot,
        manifest: &mut Manifest,
        operations: Vec<Pending>,
        mode: BatchMode,
    ) -> Result<Vec<(PathBuf, ApplyOutcome)>> {
        let adopting = matches!(mode, BatchMode::Adopt);
        if operations.is_empty() {
            if adopting {
                manifest.adopted = true;
                self.save(manifest)?;
            }
            return Ok(Vec::new());
        }
        ensure!(
            manifest.pending.is_empty(),
            "recover the personal rules journal before applying"
        );
        manifest.pending = operations;
        self.save(manifest)?;
        let mut directories = BatchDirectories::default();
        let mut failed = BTreeSet::new();
        let mut outcomes = Vec::with_capacity(manifest.pending.len());
        for operation in std::mem::take(&mut manifest.pending) {
            let path = operation.path.clone();
            let status = if path_blocked(&failed, &path) {
                ApplyOutcome::ChangedSincePlan
            } else {
                root.apply(
                    &path,
                    &operation.seen,
                    &operation.desired,
                    operation.allow_link,
                )?
            };
            outcomes.push((path.clone(), status));
            if status == ApplyOutcome::Applied {
                directories.record(&operation);
                settle(manifest, operation);
            } else {
                failed.insert(path);
                if adopting {
                    break;
                }
            }
        }
        directories.sync(root)?;
        if adopting {
            manifest.adopted = failed.is_empty();
        }
        self.save(manifest)?;
        Ok(outcomes)
    }

    fn restore(&self) -> Result<RulesSyncReport> {
        let _lock = self.lock()?;
        let root = FileRoot::open(&self.home)?;
        let mut manifest = self.load()?.context("no local adoption backup")?;
        self.recover(&root, &mut manifest)?;
        manifest.adopted = false;
        crate::rulesync_checkout::refresh(
            self,
            &wenlan_core::shared_space::settings_path(),
            None,
            &[],
        );
        let mut result = report("Restored the local setup from its adoption backup");
        let mut owned: Vec<_> = manifest.delivered.keys().cloned().collect();
        // Children go before their parent that may become a symlink again.
        owned.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
        let mut operations = Vec::new();
        for path in owned {
            let original = manifest
                .backup
                .get(&path)
                .context("the backup is incomplete")?
                .clone();
            let delivered = &manifest.delivered[&path];
            let seen = match root.read(&path, true) {
                Ok(current) if current == delivered.delivered => current,
                _ => {
                    result
                        .blocked
                        .push(format!("{} changed since delivery", path.display()));
                    continue;
                }
            };
            operations.push(Pending {
                path,
                seen,
                desired: original,
                kind: None,
                allow_link: true,
            });
        }
        if operations.is_empty() && !manifest.delivered.is_empty() {
            self.save(&manifest)?;
        }
        for (path, status) in
            self.apply_batch(&root, &mut manifest, operations, BatchMode::Continue)?
        {
            if status == ApplyOutcome::Applied {
                result.written.push(path.display().to_string());
            } else {
                result
                    .blocked
                    .push(format!("{} changed during restore", path.display()));
            }
        }
        if manifest.delivered.is_empty() {
            self.retire_backup()?;
        } else {
            result.message =
                "Restore is waiting for changed files. The backup is still available".into();
        }
        Ok(result)
    }

    fn sync(&self, generated: &[String]) -> Result<RulesSyncReport> {
        let _lock = self.lock()?;
        let root = FileRoot::open(&self.home)?;
        let mut manifest = self.load()?.context("run wenlan rules adopt first")?;
        self.recover(&root, &mut manifest)?;
        self.sync_member(&root, &mut manifest, generated)
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
        crate::rulesync_checkout::refresh(self, &settings, Some(&manifest.master), generated);
        self.sync_locked(
            root,
            manifest,
            generated,
            |target, source| self.render_member_rule(target, source, &layers),
            |target, skills| self.render_adopted_skills(target, skills),
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
        let keys = accessible.keys().cloned().collect();
        let paths: Vec<_> = manifest
            .delivered
            .iter()
            .filter_map(|(path, record)| {
                matches!(record.kind, Kind::Rule { .. }).then_some(path.clone())
            })
            .collect();
        let mut operations = Vec::new();
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
            if previous != &current {
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
            let record = &manifest.delivered[&path];
            if record.observed.as_ref().unwrap_or(&record.delivered) == &current {
                let remaining = rulesync_layers::remove_inaccessible(after, &keys)?;
                if remaining != after {
                    operations.push(Pending {
                        path,
                        seen: current.clone(),
                        desired: with_bytes(&current, remaining.into_bytes()),
                        kind: Some(record.kind.clone()),
                        allow_link: false,
                    });
                }
            }
        }
        if operations.is_empty() && observed_edits {
            self.save(manifest)?;
        }
        self.apply_batch(root, manifest, operations, BatchMode::Continue)?;
        Ok(())
    }

    fn sync_locked(
        &self,
        root: &FileRoot,
        manifest: &mut Manifest,
        generated: &[String],
        render_rule: impl Fn(&str, &str) -> Result<BTreeMap<PathBuf, Vec<u8>>>,
        render_skills: impl Fn(&str, &[&AdoptedSkill]) -> Result<BTreeMap<PathBuf, Vec<u8>>>,
    ) -> Result<RulesSyncReport> {
        ensure!(
            manifest.adopted,
            "the previous adoption stopped early. Restore it first"
        );
        let mut result = report("Checked the local tool setup");
        let blocked = self.pull_back(root, manifest, &mut result)?;
        let had_issues = !std::mem::take(&mut manifest.delivery_issues).is_empty();
        let paths: Vec<_> = manifest.delivered.keys().cloned().collect();
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
        for (path, status) in self.apply_batch(root, manifest, operations, BatchMode::Continue)? {
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
        self.sync_skills(root, manifest, &blocked, &mut result, render_skills)?;
        self.sync_registry(root, manifest, &blocked, &mut result)?;
        Ok(result)
    }

    fn pull_back(
        &self,
        root: &FileRoot,
        manifest: &mut Manifest,
        result: &mut RulesSyncReport,
    ) -> Result<BTreeSet<PathBuf>> {
        let skills = self.managed_skills(manifest);
        let pending_skills = rulesync_registry::collect_tool_proposals(
            &self.home,
            &self.data,
            &skills,
            &manifest.unmanaged_skills,
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
    ) -> Result<()> {
        let installed = rulesync_registry::installed_sources()?;
        let approved = rulesync_registry::approved_local_folders(&self.data)?;
        let adopted = self.managed_skills(manifest);
        let mut active = installed.keys().cloned().collect::<BTreeSet<_>>();
        let mut expected = BTreeMap::<PathBuf, BTreeSet<PathBuf>>::new();
        let mut targets: BTreeSet<_> = manifest
            .delivered
            .values()
            .filter_map(|delivery| match &delivery.kind {
                Kind::Rule { target, .. } => Some(target.clone()),
                _ => None,
            })
            .collect();
        targets.extend(
            manifest
                .master
                .skills
                .iter()
                .flat_map(|skill| skill.targets.iter().cloned()),
        );
        let previous_skips = std::mem::take(&mut manifest.registry_skipped);
        let mut operations = Vec::new();
        for target in targets {
            let local = rulesync_registry::approved_skill_sources(&self.data, &target)?;
            let mut sources = installed.clone();
            sources.extend(local.clone());
            active.extend(sources.keys().cloned());
            if sources.is_empty() {
                continue;
            }
            let output = self.render_tree(&target, "skills", |input| {
                for (name, files) in &sources {
                    write_skill_files(input, name, files)?;
                }
                Ok(())
            });
            let output = match output {
                Ok(tree) => tree,
                Err(error) => {
                    result
                        .blocked
                        .push(format!("{target} installed skills: {error:#}"));
                    continue;
                }
            };
            for name in sources.keys() {
                let Some(emitted) = rendered_skill_root(&output, name)? else {
                    continue;
                };
                let selected = approved.iter().find(|(path, skill)| {
                    path.file_name() == emitted.file_name()
                        && Some(skill.tool) == renderer_skill_tool(&target)
                });
                let adopted_folder = local
                    .contains_key(name)
                    .then(|| {
                        adopted.iter().find(|(path, skill)| {
                            path.file_name() == emitted.file_name()
                                && Some(skill.tool) == renderer_skill_tool(&target)
                        })
                    })
                    .flatten();
                let folder = if manifest.delivered.contains_key(emitted) {
                    emitted
                } else {
                    selected
                        .or(adopted_folder)
                        .map_or(emitted, |(path, _)| path.as_path())
                };
                if path_blocked(blocked, folder) || path_blocked(&manifest.unmanaged_skills, folder)
                {
                    continue;
                }
                let owned = manifest.delivered.get(folder).is_some_and(|record| {
                    matches!(&record.kind, Kind::RegistryDirectory { name: installed } if installed == name)
                });
                let seen = root.read(folder, false);
                let snapshot = approved.get(folder).filter(|skill| {
                    skill.files.iter().all(|(path, expected)| {
                        root.read(&folder.join(path), false)
                            .is_ok_and(|current| current == *expected)
                    })
                });
                let adopted_owned = local.contains_key(name)
                    && adopted.contains_key(folder)
                    && manifest.delivered.contains_key(folder);
                if !owned
                    && !adopted_owned
                    && snapshot.is_none()
                    && !matches!(seen, Ok(Snapshot::Absent))
                {
                    manifest
                        .registry_skipped
                        .entry(name.clone())
                        .or_default()
                        .insert(folder.to_owned());
                    continue;
                }
                if !owned {
                    let seen = seen?;
                    manifest
                        .backup
                        .entry(folder.to_owned())
                        .or_insert_with(|| seen.clone());
                    operations.push(Pending {
                        path: folder.to_owned(),
                        seen,
                        desired: Snapshot::Directory,
                        kind: Some(Kind::RegistryDirectory { name: name.clone() }),
                        allow_link: false,
                    });
                }
                let mut wanted = BTreeSet::from([folder.to_owned()]);
                let mut complete = true;
                for (emitted_file, bytes) in output
                    .range(emitted.to_owned()..)
                    .take_while(|(path, _)| path.starts_with(emitted))
                {
                    let relative = emitted_file.strip_prefix(emitted)?;
                    let path = folder.join(relative);
                    if path_blocked(blocked, &path) {
                        complete = false;
                        continue;
                    }
                    wanted.insert(path.clone());
                    let mut parents = path
                        .ancestors()
                        .skip(1)
                        .take_while(|parent| *parent != folder)
                        .collect::<Vec<_>>();
                    parents.reverse();
                    for parent in parents {
                        if wanted.insert(parent.to_owned())
                            && matches!(root.read(parent, false)?, Snapshot::Absent)
                        {
                            manifest
                                .backup
                                .entry(parent.to_owned())
                                .or_insert(Snapshot::Absent);
                            operations.push(Pending {
                                path: parent.to_owned(),
                                seen: Snapshot::Absent,
                                desired: Snapshot::Directory,
                                kind: Some(Kind::RegistryAssetDirectory { name: name.clone() }),
                                allow_link: false,
                            });
                        }
                    }
                    let seen = if let Some(approved) =
                        snapshot.and_then(|skill| skill.files.get(relative))
                    {
                        approved.clone()
                    } else if let Some(record) = manifest.delivered.get(&path) {
                        record
                            .observed
                            .as_ref()
                            .unwrap_or(&record.delivered)
                            .clone()
                    } else {
                        Snapshot::Absent
                    };
                    let desired = with_bytes(&seen, bytes.clone());
                    if seen == desired
                        && manifest.delivered.get(&path).is_some_and(|record| {
                            matches!(record.kind, Kind::RegistrySkill { .. })
                                && record.observed.is_none()
                        })
                    {
                        continue;
                    }
                    manifest
                        .backup
                        .entry(path.clone())
                        .or_insert_with(|| seen.clone());
                    operations.push(Pending {
                        path,
                        seen,
                        desired,
                        kind: Some(Kind::RegistrySkill { name: name.clone() }),
                        allow_link: false,
                    });
                }
                if complete {
                    expected.insert(folder.to_owned(), wanted);
                }
            }
        }
        for (path, status) in self.apply_batch(root, manifest, operations, BatchMode::Continue)? {
            if status == ApplyOutcome::Applied {
                if matches!(manifest.delivered[&path].kind, Kind::RegistrySkill { .. }) {
                    result.written.push(path.display().to_string());
                }
            } else {
                if let Some(folder) = path
                    .ancestors()
                    .find(|folder| expected.contains_key(*folder))
                {
                    expected.remove(folder);
                }
                result
                    .blocked
                    .push(format!("{} changed during sync", path.display()));
            }
        }
        let mut obsolete: Vec<_> = manifest
            .delivered
            .iter()
            .filter_map(|(path, delivery)| {
                let (Kind::RegistryDirectory { name }
                | Kind::RegistryAssetDirectory { name }
                | Kind::RegistrySkill { name }) = &delivery.kind
                else {
                    return None;
                };
                let remove = !active.contains(name);
                let stale = path
                    .ancestors()
                    .find_map(|parent| expected.get(parent))
                    .is_some_and(|wanted| !wanted.contains(path));
                ((remove || stale) && !path_blocked(blocked, path)).then(|| (path.clone(), remove))
            })
            .collect();
        obsolete.sort_by_key(|(path, _)| std::cmp::Reverse(path.components().count()));
        let mut operations = Vec::new();
        for (path, remove) in obsolete {
            let record = &manifest.delivered[&path];
            let seen = record
                .observed
                .as_ref()
                .unwrap_or(&record.delivered)
                .clone();
            let desired = if remove {
                manifest
                    .backup
                    .get(&path)
                    .cloned()
                    .unwrap_or(Snapshot::Absent)
            } else {
                Snapshot::Absent
            };
            let allow_link = remove && matches!(desired, Snapshot::Symlink { .. });
            operations.push(Pending {
                path,
                seen,
                desired,
                kind: (!remove).then(|| record.kind.clone()),
                allow_link,
            });
        }
        for (path, status) in self.apply_batch(root, manifest, operations, BatchMode::Continue)? {
            if status != ApplyOutcome::Applied {
                result
                    .blocked
                    .push(format!("{} changed since delivery", path.display()));
            }
        }
        if manifest.registry_skipped != previous_skips {
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
        render: impl Fn(&str, &[&AdoptedSkill]) -> Result<BTreeMap<PathBuf, Vec<u8>>>,
    ) -> Result<()> {
        let approved = rulesync_registry::approved_local_folders(&self.data)?;
        let mut replacements = BTreeMap::new();
        for target in adoption_inputs() {
            replacements.insert(
                target.tool.clone(),
                rulesync_registry::approved_skill_sources(&self.data, &target.tool)?,
            );
        }
        let mut by_target: BTreeMap<String, Vec<SkillPath>> = BTreeMap::new();
        for (path, delivery) in &manifest.delivered {
            if !matches!(delivery.kind, Kind::Skill)
                || path_blocked(blocked, path)
                || approved.keys().any(|folder| path.starts_with(folder))
            {
                continue;
            }
            let Some(spec) = skill_path(&manifest.master.skills, path) else {
                result
                    .blocked
                    .push(format!("{} is missing from the master", path.display()));
                continue;
            };
            if replacements.get(&spec.target).is_some_and(|sources| {
                sources.contains_key(&manifest.master.skills[spec.index].name)
            }) {
                continue;
            }
            by_target.entry(spec.target.clone()).or_default().push(spec);
        }
        let mut operations = Vec::new();
        for (target, paths) in by_target {
            let mut names = BTreeSet::new();
            let skills = paths
                .iter()
                .map(|path| &manifest.master.skills[path.index])
                .filter(|skill| names.insert(&skill.name))
                .collect::<Vec<_>>();
            let output = match render(&target, &skills) {
                Ok(tree) => tree,
                Err(error) => {
                    result.blocked.push(format!("{target} skills: {error:#}"));
                    continue;
                }
            };
            let roots: BTreeMap<_, _> = skills
                .iter()
                .map(|skill| {
                    Ok((
                        skill.name.as_str(),
                        rendered_skill_root(&output, &skill.name)?,
                    ))
                })
                .collect::<Result<_>>()?;
            for spec in paths {
                let delivery = &manifest.delivered[&spec.installed];
                let seen = delivery.observed.as_ref().unwrap_or(&delivery.delivered);
                let skill = &manifest.master.skills[spec.index];
                let desired = if !skill.files.contains_key(&spec.relative) {
                    Snapshot::Absent
                } else {
                    let Some(bytes) = roots[skill.name.as_str()]
                        .and_then(|root| output.get(&root.join(&spec.relative)))
                    else {
                        result.blocked.push(format!(
                            "{} was absent from rulesync output",
                            spec.installed.display()
                        ));
                        continue;
                    };
                    with_bytes(seen, bytes.clone())
                };
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
        for (path, status) in self.apply_batch(root, manifest, operations, BatchMode::Continue)? {
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
                if let Some(tool) = renderer_skill_tool(target) {
                    packages.extend(folders.iter().map(|folder| {
                        (
                            folder.clone(),
                            ManagedSkill {
                                tool,
                                files: BTreeMap::new(),
                            },
                        )
                    }));
                }
            }
        }
        for (path, record) in &manifest.delivered {
            if matches!(record.kind, Kind::RegistryDirectory { .. }) {
                let tool = if path.starts_with(".claude") {
                    SavedSkillTool::ClaudeCode
                } else {
                    SavedSkillTool::Codex
                };
                packages.entry(path.clone()).or_insert(ManagedSkill {
                    tool,
                    files: BTreeMap::new(),
                });
            }
        }
        for (folder, skill) in &mut packages {
            for (path, record) in &manifest.delivered {
                if let Ok(relative) = path.strip_prefix(folder) {
                    if matches!(record.kind, Kind::Skill | Kind::RegistrySkill { .. }) {
                        skill.files.insert(
                            relative.to_owned(),
                            record
                                .observed
                                .as_ref()
                                .unwrap_or(&record.delivered)
                                .clone(),
                        );
                    }
                }
            }
        }
        packages
    }

    fn review(&self, request: &PullbackDecision, generated: &[String]) -> Result<RulesSyncReport> {
        let _lock = self.lock()?;
        let root = FileRoot::open(&self.home)?;
        let mut manifest = self.load()?.context("run wenlan rules adopt first")?;
        self.recover(&root, &mut manifest)?;
        if let Some(scope) = manifest.scope_reviews.remove(&request.id) {
            if matches!(request.action, PullbackAction::AllTools) {
                let delivery = manifest
                    .delivered
                    .get(&scope.path)
                    .context("file is no longer managed")?;
                let expected = delivery.observed.as_ref().unwrap_or(&delivery.delivered);
                ensure!(
                    root.read(&scope.path, false)? == *expected,
                    "the tool edited this file again. Run rules sync"
                );
                manifest.master.shared =
                    promote_changes(&manifest.master.shared, &scope.before, &scope.after)?;
            }
            self.save(&manifest)?;
            return self.sync_member(&root, &mut manifest, generated);
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
        for draft in drafts
            .iter()
            .filter(|draft| card.paths.contains(&draft.path))
        {
            if matches!(request.action, PullbackAction::Dismiss) {
                let delivery = manifest
                    .delivered
                    .get_mut(&draft.path)
                    .context("file is no longer managed")?;
                delivery.observed = Some(draft.current.clone());
            } else if path == Some(draft.path.as_path()) {
                match request.action {
                    PullbackAction::ToolOnly => accept_tool_edit(&mut manifest, draft)?,
                    PullbackAction::AllTools => {
                        let delivery = &manifest.delivered[&draft.path];
                        let before = rulesync_layers::only_personal(text(
                            delivery.observed.as_ref().unwrap_or(&delivery.delivered),
                        )?)?;
                        let after = rulesync_layers::only_personal(text(&draft.current)?)?;
                        manifest.master.shared =
                            promote_changes(&manifest.master.shared, &before, &after)?;
                        manifest
                            .delivered
                            .get_mut(&draft.path)
                            .context("file is no longer managed")?
                            .observed = Some(draft.current.clone());
                    }
                    PullbackAction::Dismiss => unreachable!(),
                }
            }
        }
        self.save(&manifest)?;
        self.sync_member(&root, &mut manifest, generated)
    }

    fn render_rule(&self, target: &str, source: &str) -> Result<BTreeMap<PathBuf, Vec<u8>>> {
        self.render_tree(target, "rules", |input| write_root_rule(input, source))
    }

    fn render_adopted_skills(
        &self,
        target: &str,
        skills: &[&AdoptedSkill],
    ) -> Result<BTreeMap<PathBuf, Vec<u8>>> {
        self.render_tree(target, "skills", |input| {
            for skill in skills {
                write_skill(input, skill)?;
            }
            Ok(())
        })
    }

    fn render_member_rule(
        &self,
        target: &str,
        source: &str,
        layers: &[TeamRuleLayer],
    ) -> Result<BTreeMap<PathBuf, Vec<u8>>> {
        if layers.is_empty() {
            return self.render_rule(target, source);
        }
        let stage = rulesync_renderer::new_stage(&self.data)?;
        let team = stage.path().join("team");
        let personal = stage.path().join("personal");
        let output = stage.path().join("output");
        fs::create_dir(&output)?;
        rulesync_layers::stage(&team, "Team", layers)?;
        write_root_rule(&personal, source)?;
        rulesync_renderer::render(
            &self.binary,
            stage.path(),
            &[team, personal],
            &output,
            true,
            &[target.into()],
            &["rules"],
        )?;
        let mut rendered = BTreeMap::new();
        read_stage(&output, &output, &mut rendered)?;
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
        target: &str,
        feature: &str,
        stage_input: impl FnOnce(&Path) -> Result<()>,
    ) -> Result<BTreeMap<PathBuf, Vec<u8>>> {
        let stage = rulesync_renderer::new_stage(&self.data)?;
        let input = stage.path().join("master");
        let output = stage.path().join("output");
        fs::create_dir(&input)?;
        fs::create_dir(&output)?;
        stage_input(&input)?;
        rulesync_renderer::render(
            &self.binary,
            stage.path(),
            &[input],
            &output,
            true,
            &[target.into()],
            &[feature],
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

fn write_skill(input: &Path, skill: &AdoptedSkill) -> Result<()> {
    write_skill_files(input, &skill.name, &skill.files)
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

pub(crate) fn rendered_skill_root<'a>(
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
        | (AdoptionEntry::Directory, Snapshot::Directory) => true,
        (AdoptionEntry::File(seen), Snapshot::File { bytes, .. }) => seen == bytes,
        (AdoptionEntry::Symlink { target, .. }, Snapshot::Symlink { target: actual, .. }) => {
            target.as_os_str() == actual.as_os_str()
        }
        _ => false,
    }
}

fn source_mode(path: &Path, bytes: Vec<u8>) -> Result<Snapshot> {
    let metadata = fs::metadata(path)?;
    ensure!(metadata.is_file(), "{} is no longer a file", path.display());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Ok(Snapshot::file_with_mode(
            bytes,
            metadata.permissions().mode(),
        ))
    }
    #[cfg(not(unix))]
    Ok(Snapshot::file(bytes))
}

pub(crate) fn with_bytes(seen: &Snapshot, bytes: Vec<u8>) -> Snapshot {
    match seen {
        Snapshot::File { mode, .. } => Snapshot::File { bytes, mode: *mode },
        _ => Snapshot::file(bytes),
    }
}

fn same_rule_lines(actual: &[u8], expected: &[u8]) -> Result<bool> {
    let actual = std::str::from_utf8(actual)?;
    let expected = std::str::from_utf8(expected)?;
    Ok(actual
        .trim_matches(['\r', '\n'])
        .lines()
        .eq(expected.trim_matches(['\r', '\n']).lines()))
}

fn settle(manifest: &mut Manifest, operation: Pending) {
    if let Some(kind) = operation.kind {
        manifest.delivered.insert(
            operation.path,
            Delivery {
                kind,
                delivered: operation.desired,
                observed: None,
                at: chrono::Utc::now().timestamp(),
            },
        );
    } else {
        manifest.delivered.remove(&operation.path);
    }
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
    let mut blocked = BTreeMap::new();
    for (path, delivery) in &manifest.delivered {
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
        let needs_review = changes.iter().any(|change| change.removed.is_empty());
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
    let previous = match manifest.master.overrides.get(target) {
        Some(local) => merge_rule_text(&local.base, &local.edited, base)?,
        None => source.clone(),
    };
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

pub(crate) fn report(message: &str) -> RulesSyncReport {
    RulesSyncReport {
        written: Vec::new(),
        blocked: Vec::new(),
        proposals: Vec::new(),
        message: message.into(),
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
            let root = FileRoot::open(&self.setup.home).unwrap();
            let _lock = self.setup.lock().unwrap();
            self.setup.apply_adoption(&root, adoption, master).unwrap();
        }

        fn sync_with(&self, pin_banner: &str) -> RulesSyncReport {
            let root = FileRoot::open(&self.setup.home).unwrap();
            let _lock = self.setup.lock().unwrap();
            let mut manifest = self.setup.load().unwrap().unwrap();
            self.setup
                .sync_locked(
                    &root,
                    &mut manifest,
                    &[],
                    |target, text| {
                        let path = if target == "codexcli" { CODEX } else { CLAUDE };
                        Ok(BTreeMap::from([(
                            PathBuf::from(path),
                            format!("{pin_banner}{text}").into_bytes(),
                        )]))
                    },
                    |target, skills| self.setup.render_adopted_skills(target, skills),
                )
                .unwrap()
        }

        fn text(&self, path: &str) -> String {
            fs::read_to_string(self.setup.home.join(path)).unwrap()
        }

        fn write(&self, path: &str, text: &str) {
            fs::write(self.setup.home.join(path), text).unwrap();
        }
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
                ".claude/skills/new-skill: Skill changes need your approval. Run `wenlan skills proposals` to review them.",
                ".codex/AGENTS.md: Rule changes need your approval. Use `wenlan rules review` with the proposal below."
            ])
        );
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

        let root = FileRoot::open(&f.setup.home).unwrap();
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
                |_target, skills| {
                    Ok(skills
                        .iter()
                        .flat_map(|skill| {
                            skill.files.iter().map(|(path, bytes)| {
                                (
                                    Path::new("skills").join(&skill.name).join(path),
                                    bytes.clone(),
                                )
                            })
                        })
                        .collect())
                },
            )
            .unwrap();
        assert!(report.blocked.is_empty(), "{report:?}");
        assert!(report.written.is_empty(), "{report:?}");
        assert!(manifest
            .delivered
            .contains_key(Path::new(".codex/skills/local/SKILL.md")));
        assert!(rulesync_registry::approved_local_folders(&f.setup.data)
            .unwrap()
            .is_empty());
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
        assert_eq!(stored["format"], 2);
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
                        for skill in skills {
                            let folder = tool_skills.join(&skill.name);
                            for (path, original) in &skill.files {
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
                desired: Snapshot::Directory,
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
        f.write(skill, "source\n");
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
        assert_eq!(f.text(skill), "source\n");
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
        let original = f.text(CLAUDE);
        let error = f.setup.restore().unwrap_err();
        assert!(error
            .to_string()
            .contains("run wenlan rules restore with the earlier build"));
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

    #[test]
    fn rulesync_recovery_retains_the_journal_when_a_file_has_a_third_version() {
        let f = Fixture::new();
        f.adopt_from_plan();
        {
            let _lock = f.setup.lock().unwrap();
            let mut manifest = f.setup.load().unwrap().unwrap();
            let path = PathBuf::from(CODEX);
            let record = &manifest.delivered[&path];
            manifest.pending.push(Pending {
                path,
                seen: record.delivered.clone(),
                desired: with_bytes(&record.delivered, b"next release\n".to_vec()),
                kind: Some(record.kind.clone()),
                allow_link: false,
            });
            f.setup.save(&manifest).unwrap();
        }
        let state = f.setup.data.join("personal.json");
        let before = fs::read(&state).unwrap();
        f.write(CODEX, "editor saved while the daemon was down\n");
        let error = f.setup.restore().unwrap_err();
        assert!(error
            .to_string()
            .contains("journal and backup are retained"));
        assert_eq!(f.text(CODEX), "editor saved while the daemon was down\n");
        assert_eq!(fs::read(state).unwrap(), before);
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
            let root = FileRoot::open(&f.setup.home).unwrap();
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
                    |target, skills| f.setup.render_adopted_skills(target, skills),
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
    fn rulesync_adopt_and_restore_exact_bytes_and_links() {
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
        fs::write(home.join(".agents/skills/shared/SKILL.md"), original).unwrap();
        fs::write(home.join(".codex/skills/handmade/SKILL.md"), "Handmade").unwrap();
        let link = "../../.agents/skills/shared";
        std::os::unix::fs::symlink(link, home.join(".claude/skills/shared")).unwrap();
        f.adopt_from_plan();
        assert!(home.join(".claude/skills/shared/SKILL.md").is_file());
        assert!(!fs::symlink_metadata(home.join(".claude/skills/shared"))
            .unwrap()
            .is_symlink());
        assert!(!home.join(".codex/skills/shared").exists());
        assert_eq!(f.text(".codex/skills/handmade/SKILL.md"), "Handmade");
        let restored = f.setup.restore().unwrap();
        assert!(restored.blocked.is_empty(), "{restored:?}");
        assert_eq!(
            fs::read_link(home.join(".claude/skills/shared"))
                .unwrap()
                .as_os_str(),
            link
        );
        assert_eq!(f.text(".agents/skills/shared/SKILL.md"), original);
        assert_eq!(f.text(".codex/skills/handmade/SKILL.md"), "Handmade");
    }

    #[test]
    fn rulesync_setup_rejects_unauthenticated_callers() {
        assert!(require_local_person(&HeaderMap::new()).is_err());
    }
}
