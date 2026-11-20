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
    adoption_inputs, adoption_plan, describe_change, discover_adoption, fingerprint, line_changes,
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
struct Delivery {
    kind: Kind,
    delivered: Snapshot,
    #[serde(default)]
    observed: Option<Snapshot>,
    at: i64,
}

#[derive(Debug, Serialize, Deserialize)]
struct Pending {
    path: PathBuf,
    seen: Snapshot,
    desired: Snapshot,
    kind: Option<Kind>,
    allow_link: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct Manifest {
    master: PersonalMaster,
    /// Every destination's original bytes, mode or exact link value. This
    /// whole map reaches disk before the first tool file changes.
    backup: BTreeMap<PathBuf, Snapshot>,
    delivered: BTreeMap<PathBuf, Delivery>,
    pending: Option<Pending>,
    adopted: bool,
    #[serde(default)]
    registry_skipped: BTreeMap<String, BTreeSet<PathBuf>>,
    #[serde(default)]
    scope_reviews: BTreeMap<String, ScopeReview>,
    #[serde(default)]
    delivery_issues: BTreeMap<PathBuf, RuleDeliveryIssue>,
}

#[derive(Debug, Serialize, Deserialize)]
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

#[derive(Debug, Serialize, Deserialize)]
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
                Ok(()) => {}
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

    fn load(&self) -> Result<Option<Manifest>> {
        let root = FileRoot::open(&self.data)?;
        match root.read(Path::new("personal.json"), false)? {
            Snapshot::File { bytes, .. } => Ok(Some(serde_json::from_slice(&bytes)?)),
            Snapshot::Absent => Ok(None),
            _ => bail!("unexpected personal rules master file type"),
        }
    }

    fn save(&self, manifest: &Manifest) -> Result<()> {
        let root = FileRoot::open(&self.data)?;
        let path = Path::new("personal.json");
        let seen = root.read(path, false)?;
        let next = Snapshot::file(serde_json::to_vec(manifest)?);
        ensure!(
            root.apply(path, &seen, &next, false)? == ApplyOutcome::Applied,
            "personal rules state changed during the operation"
        );
        Ok(())
    }

    fn adopt(&self, token: &str) -> Result<RulesSyncReport> {
        let adoption = discover_adoption(&self.home)?;
        ensure!(
            adoption_plan(&adoption)?.token == token,
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
            adoption_plan(&discover_adoption(&self.home)?)?.token == token,
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
        let mut manifest = Manifest {
            master,
            backup,
            delivered: BTreeMap::new(),
            pending: None,
            adopted: false,
            registry_skipped: BTreeMap::new(),
            scope_reviews: BTreeMap::new(),
            delivery_issues: BTreeMap::new(),
        };
        self.save(&manifest)?;
        let mut result = report("Adopted the local tool setup");
        for operation in operations {
            let path = operation.path.display().to_string();
            if !self.apply(root, &mut manifest, operation)? {
                bail!("{path} changed during adoption. Run rules restore to roll back completed files");
            }
            result.written.push(path);
        }
        manifest.adopted = true;
        self.save(&manifest)?;
        result.blocked.extend(
            adoption
                .skipped
                .into_iter()
                .map(|(path, reason)| format!("{}: {reason}", path.display())),
        );
        Ok(result)
    }

    fn recover(&self, root: &FileRoot, manifest: &mut Manifest) -> Result<()> {
        let Some(pending) = manifest.pending.take() else {
            return Ok(());
        };
        let actual = root.read(&pending.path, pending.allow_link)?;
        if actual == pending.desired {
            settle(manifest, pending);
        } else {
            ensure!(
                actual == pending.seen,
                "{} changed while Wenlan was applying it. The journal and backup are retained",
                pending.path.display()
            );
        }
        self.save(manifest)
    }

    fn apply(&self, root: &FileRoot, manifest: &mut Manifest, operation: Pending) -> Result<bool> {
        manifest.pending = Some(operation);
        self.save(manifest)?;
        let operation = manifest
            .pending
            .take()
            .context("missing personal rules journal entry")?;
        let status = root.apply(
            &operation.path,
            &operation.seen,
            &operation.desired,
            operation.allow_link,
        )?;
        if status == ApplyOutcome::Applied {
            settle(manifest, operation);
        }
        self.save(manifest)?;
        Ok(status == ApplyOutcome::Applied)
    }

    fn restore(&self) -> Result<RulesSyncReport> {
        let _lock = self.lock()?;
        let root = FileRoot::open(&self.home)?;
        let mut manifest = self.load()?.context("no local adoption backup")?;
        self.recover(&root, &mut manifest)?;
        manifest.adopted = false;
        self.save(&manifest)?;
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
            let operation = Pending {
                path: path.clone(),
                seen,
                desired: original,
                kind: None,
                allow_link: true,
            };
            if self.apply(&root, &mut manifest, operation)? {
                result.written.push(path.display().to_string());
            } else {
                result
                    .blocked
                    .push(format!("{} changed during restore", path.display()));
            }
        }
        if manifest.delivered.is_empty() {
            let data = FileRoot::open(&self.data)?;
            let path = Path::new("personal.json");
            let seen = data.read(path, false)?;
            ensure!(
                data.apply(path, &seen, &Snapshot::Absent, false)? == ApplyOutcome::Applied,
                "the restored state changed at the end of restore"
            );
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
        self.sync_locked(root, manifest, generated, |target, source| {
            self.render_member_rule(target, source, &layers)
        })
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
                    self.save(manifest)?;
                }
            }
            let record = &manifest.delivered[&path];
            if record.observed.as_ref().unwrap_or(&record.delivered) == &current {
                let remaining = rulesync_layers::remove_inaccessible(after, &keys)?;
                if remaining != after {
                    let operation = Pending {
                        path,
                        seen: current.clone(),
                        desired: with_bytes(&current, remaining.into_bytes()),
                        kind: Some(record.kind.clone()),
                        allow_link: false,
                    };
                    self.apply(root, manifest, operation)?;
                }
            }
        }
        Ok(())
    }

    fn sync_locked(
        &self,
        root: &FileRoot,
        manifest: &mut Manifest,
        generated: &[String],
        render: impl Fn(&str, &str) -> Result<BTreeMap<PathBuf, Vec<u8>>>,
    ) -> Result<RulesSyncReport> {
        ensure!(
            manifest.adopted,
            "the previous adoption stopped early. Restore it first"
        );
        let mut result = report("Checked the local tool setup");
        let blocked = self.pull_back(root, manifest, &mut result)?;
        manifest.delivery_issues.clear();
        let paths: Vec<_> = manifest.delivered.keys().cloned().collect();
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
            let output = match render(target, &source) {
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
            let operation = Pending {
                path: path.clone(),
                seen,
                desired,
                kind: Some(Kind::Rule {
                    target: target.clone(),
                    source,
                    base: manifest.master.base_rule_text(target, generated),
                }),
                allow_link: false,
            };
            if self.apply(root, manifest, operation)? {
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
        self.save(manifest)?;
        self.sync_skills(root, manifest, &blocked, &mut result)?;
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
        let pending_skills =
            rulesync_registry::collect_tool_proposals(&self.home, &self.data, &skills)?;
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
        self.save(manifest)?;
        for card in cards {
            blocked.extend(card.paths.iter().cloned());
            result.proposals.push(card.wire);
        }
        result.proposals.extend(
            manifest
                .scope_reviews
                .values()
                .map(|review| review.card.clone()),
        );
        result
            .blocked
            .extend(blocked.iter().map(|path| path.display().to_string()));
        Ok(blocked)
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
        manifest.registry_skipped.clear();
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
                if path_blocked(blocked, folder) {
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
                    if !self.apply(
                        root,
                        manifest,
                        Pending {
                            path: folder.to_owned(),
                            seen,
                            desired: Snapshot::Directory,
                            kind: Some(Kind::RegistryDirectory { name: name.clone() }),
                            allow_link: false,
                        },
                    )? {
                        continue;
                    }
                }
                let mut wanted = BTreeSet::from([folder.to_owned()]);
                let mut complete = true;
                for (emitted_file, bytes) in
                    output.iter().filter(|(path, _)| path.starts_with(emitted))
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
                        wanted.insert(parent.to_owned());
                        if matches!(root.read(parent, false)?, Snapshot::Absent) {
                            manifest
                                .backup
                                .entry(parent.to_owned())
                                .or_insert(Snapshot::Absent);
                            complete &= self.apply(
                                root,
                                manifest,
                                Pending {
                                    path: parent.to_owned(),
                                    seen: Snapshot::Absent,
                                    desired: Snapshot::Directory,
                                    kind: Some(Kind::RegistryAssetDirectory { name: name.clone() }),
                                    allow_link: false,
                                },
                            )?;
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
                    if self.apply(
                        root,
                        manifest,
                        Pending {
                            path: path.clone(),
                            seen,
                            desired,
                            kind: Some(Kind::RegistrySkill { name: name.clone() }),
                            allow_link: false,
                        },
                    )? {
                        result.written.push(path.display().to_string());
                    } else {
                        complete = false;
                        result
                            .blocked
                            .push(format!("{} changed during sync", path.display()));
                    }
                }
                if complete {
                    expected.insert(folder.to_owned(), wanted);
                }
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
            if !self.apply(
                root,
                manifest,
                Pending {
                    path: path.clone(),
                    seen,
                    desired,
                    kind: (!remove).then(|| record.kind.clone()),
                    allow_link,
                },
            )? {
                result
                    .blocked
                    .push(format!("{} changed since delivery", path.display()));
            }
        }
        self.save(manifest)
    }

    fn sync_skills(
        &self,
        root: &FileRoot,
        manifest: &mut Manifest,
        blocked: &BTreeSet<PathBuf>,
        result: &mut RulesSyncReport,
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
        for (target, paths) in by_target {
            let output = self.render_tree(&target, "skills", |input| {
                let mut names = BTreeSet::new();
                for path in &paths {
                    let skill = &manifest.master.skills[path.index];
                    if names.insert(&skill.name) {
                        write_skill(input, skill)?;
                    }
                }
                Ok(())
            });
            let output = match output {
                Ok(tree) => tree,
                Err(error) => {
                    result.blocked.push(format!("{target} skills: {error:#}"));
                    continue;
                }
            };
            for spec in paths {
                let delivery = &manifest.delivered[&spec.installed];
                let seen = delivery.observed.as_ref().unwrap_or(&delivery.delivered);
                let skill = &manifest.master.skills[spec.index];
                let desired = if !skill.files.contains_key(&spec.relative) {
                    Snapshot::Absent
                } else {
                    let Some(bytes) = rendered_skill_file(&output, &skill.name, &spec.relative)?
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
                if self.apply(
                    root,
                    manifest,
                    Pending {
                        path: spec.installed.clone(),
                        seen: seen.clone(),
                        desired,
                        kind: Some(Kind::Skill),
                        allow_link: false,
                    },
                )? {
                    result.written.push(spec.installed.display().to_string());
                } else {
                    result
                        .blocked
                        .push(format!("{} changed during sync", spec.installed.display()));
                }
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

fn rendered_skill_file<'a>(
    output: &'a BTreeMap<PathBuf, Vec<u8>>,
    skill: &str,
    file: &Path,
) -> Result<Option<&'a Vec<u8>>> {
    Ok(rendered_skill_root(output, skill)?.and_then(|root| output.get(&root.join(file))))
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
    blocked: BTreeSet<PathBuf>,
}

fn inspect(root: &FileRoot, manifest: &Manifest) -> Result<Pullback> {
    let mut drafts = Vec::new();
    let mut blocked = BTreeSet::new();
    for (path, delivery) in &manifest.delivered {
        if !matches!(delivery.kind, Kind::Rule { .. }) {
            continue;
        }
        let current = match root.read(path, false) {
            Ok(current) => current,
            Err(_) => {
                blocked.insert(path.clone());
                continue;
            }
        };
        let baseline = delivery.observed.as_ref().unwrap_or(&delivery.delivered);
        if *baseline == current {
            continue;
        }
        let (Ok(before), Ok(after)) = (text(baseline), text(&current)) else {
            blocked.insert(path.clone());
            continue;
        };
        if !rulesync_layers::edits(before, after).is_ok_and(|edits| edits.is_empty()) {
            blocked.insert(path.clone());
            continue;
        }
        let before = rulesync_layers::only_personal(before)?;
        let after = rulesync_layers::only_personal(after)?;
        let changes = match line_changes(&before, &after) {
            Ok(changes) => changes,
            Err(_) => {
                blocked.insert(path.clone());
                continue;
            }
        };
        if changes.is_empty() {
            blocked.insert(path.clone());
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
                .sync_locked(&root, &mut manifest, &[], |target, text| {
                    let path = if target == "codexcli" { CODEX } else { CLAUDE };
                    Ok(BTreeMap::from([(
                        PathBuf::from(path),
                        format!("{pin_banner}{text}").into_bytes(),
                    )]))
                })
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
                .sync_locked(&root, &mut manifest, &[], |target, text| {
                    if target == "codexcli" {
                        bail!("failed to render Codex");
                    }
                    Ok(BTreeMap::from([(
                        PathBuf::from(CLAUDE),
                        text.as_bytes().to_vec(),
                    )]))
                })
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
