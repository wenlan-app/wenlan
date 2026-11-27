// SPDX-License-Identifier: Apache-2.0
//! Private, checkout-specific renders. The repository and its Git index are never the destination.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{ensure, Context, Result};
use axum::{extract::State, http::HeaderMap, Json};
use serde::{Deserialize, Serialize};
use wenlan_core::rulesync_personal::{self, fingerprint, write_root_rule, PersonalMaster};
use wenlan_core::rulesync_team::{self, TeamRuleLayer};
use wenlan_core::{repo_wiki, rulesync_renderer, shared_space};
use wenlan_types::rulesync::RulesSetupCode as Code;
use wenlan_types::rulesync::{CheckoutRulesReceipt, CheckoutRulesRequest, RulesSyncReport};

use crate::error::ServerError;
use crate::route_registry::{post, TrackedRouter};
use crate::rulesync_apply::{ApplyOptions, ApplyOutcome, FileRoot, Snapshot};
use crate::rulesync_layers;
use crate::rulesync_routes::{
    prepare_checkout_displaced, read_stage, rendered_skill_with_assets, report,
    require_local_person, snapshot_has_credentials, text, with_bytes, write_skill_files, Setup,
    UnsupportedRulesyncState,
};
use crate::state::SharedState;

pub(crate) fn register(router: TrackedRouter<SharedState>) -> TrackedRouter<SharedState> {
    router.route("/api/rules/setup/checkout", post(checkout))
}

async fn checkout(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(request): Json<CheckoutRulesRequest>,
) -> Result<Json<CheckoutRulesReceipt>, ServerError> {
    require_local_person(&headers)?;
    if !rulesync_personal::enabled() {
        return Err(ServerError::Conflict("WENLAN_RULESYNC is off".into()));
    }
    let personal = crate::skill_grow::general_tool_rules(&state).await?;
    tokio::task::spawn_blocking(move || Setup::system()?.checkout(&request.repo, &personal))
        .await
        .map_err(|error| ServerError::Internal(error.to_string()))?
        .map(Json)
        .map_err(|error| ServerError::Conflict(format!("{error:#}")))
}

#[derive(Default, Serialize, Deserialize)]
struct Checkout {
    repo: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    local_claude: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    local_codex: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    local_codex_dir_created: bool,
    files: BTreeMap<PathBuf, Snapshot>,
    skills: BTreeMap<PathBuf, SkillDelivery>,
    pending: Option<PendingDelivery>,
    unresolved: Vec<PendingDelivery>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    shared_rule_proposals: BTreeMap<PathBuf, rulesync_layers::ProposalReceipts>,
}

const CHECKOUT_FORMAT: u32 = 1;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckoutRecord<State> {
    format: u32,
    checkout: State,
}

fn read_checkout(bytes: &[u8]) -> Result<Checkout> {
    let stored: CheckoutRecord<Checkout> =
        serde_json::from_slice(bytes).context(UnsupportedRulesyncState)?;
    ensure!(stored.format == CHECKOUT_FORMAT, UnsupportedRulesyncState);
    ensure!(
        stored
            .checkout
            .pending
            .iter()
            .chain(&stored.checkout.unresolved)
            .all(|pending| pending.stage.is_some()),
        UnsupportedRulesyncState
    );
    Ok(stored.checkout)
}

pub(crate) fn clear_refused_receipts(data: &Path, proposal_ids: &[String]) -> Result<()> {
    let directory = data.join("checkouts");
    if !directory.is_dir() {
        return Ok(());
    }
    let root = FileRoot::open(data)?;
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        if entry
            .path()
            .extension()
            .is_none_or(|extension| extension != "json")
        {
            continue;
        }
        let path = Path::new("checkouts").join(entry.file_name());
        let seen = match root.read(&path, false) {
            Ok(value) => value,
            Err(error) => {
                tracing::warn!("[rulesync] skipping unreadable checkout receipt during proposal refusal: {}: {error:#}", path.display());
                continue;
            }
        };
        let mut checkout = match text(&seen).and_then(|source| read_checkout(source.as_bytes())) {
            Ok(checkout) => checkout,
            Err(error) => {
                tracing::warn!("[rulesync] skipping incompatible checkout receipt during proposal refusal: {}: {error:#}", path.display());
                continue;
            }
        };
        let before = checkout.shared_rule_proposals.clone();
        for receipts in checkout.shared_rule_proposals.values_mut() {
            receipts.retain(|_, receipt| !proposal_ids.contains(&receipt.proposal));
        }
        checkout
            .shared_rule_proposals
            .retain(|_, receipts| !receipts.is_empty());
        if checkout.shared_rule_proposals != before {
            let next = with_bytes(
                &seen,
                serde_json::to_vec(&CheckoutRecord {
                    format: CHECKOUT_FORMAT,
                    checkout,
                })?,
            );
            ensure!(
                root.apply(&path, &seen, &next, false)? == ApplyOutcome::Applied,
                "checkout receipt changed during refusal recovery"
            );
        }
    }
    Ok(())
}

pub(crate) fn local_recorded_path(data: &Path, relative: &Path) -> Option<(PathBuf, PathBuf)> {
    let mut parts = relative.components();
    if parts.next()?.as_os_str() != "checkouts" {
        return None;
    }
    let key = parts.next()?.as_os_str().to_str()?;
    if key.len() != 64 || !key.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    if parts.next()?.as_os_str() != "local" {
        return None;
    }
    let file = parts.as_path();
    if file != Path::new("CLAUDE.local.md") && file != Path::new(".codex/config.toml") {
        return None;
    }
    let root = FileRoot::open(data).ok()?;
    let state = root
        .read(&Path::new("checkouts").join(format!("{key}.json")), false)
        .ok()?;
    let Snapshot::File { bytes, .. } = state else {
        return None;
    };
    let checkout = read_checkout(&bytes).ok()?;
    if fingerprint(&checkout.repo).ok()?.as_str() != key {
        return None;
    }
    let repo = checkout.repo.canonicalize().ok()?;
    Some((repo, file.to_path_buf()))
}

#[derive(Serialize, Deserialize)]
struct SkillDelivery {
    source: String,
    digest: String,
}

#[derive(Serialize, Deserialize)]
struct PendingDelivery {
    path: PathBuf,
    digest: String,
    skill_source: Option<String>,
    stage: Option<CheckoutStage>,
    rules_baseline: Option<Snapshot>,
}

#[derive(Serialize, Deserialize)]
struct CheckoutStage {
    id: String,
    displaced: Option<PathBuf>,
    seen: Snapshot,
    desired: Snapshot,
}

enum DeliveryOutcome {
    Applied,
    Changed,
    Refused(io::Error),
}

impl DeliveryOutcome {
    fn conflict(self, changed: &str) -> Option<String> {
        match self {
            Self::Applied => None,
            Self::Changed => Some(changed.into()),
            Self::Refused(error) => Some(error.to_string()),
        }
    }
}

impl CheckoutStage {
    fn options<'a>(&'a self, root: &'a FileRoot) -> ApplyOptions<'a> {
        ApplyOptions {
            allow_symlink: false,
            displaced: self.displaced.as_deref().map(|path| (root, path)),
            stage_id: Some(&self.id),
            verify_source: None,
        }
    }
}

impl Checkout {
    fn settle(&mut self, pending: PendingDelivery, snapshot: Snapshot) {
        if let Some(source) = pending.skill_source {
            if snapshot == Snapshot::Absent {
                self.skills.remove(&pending.path);
            } else {
                self.skills.insert(
                    pending.path,
                    SkillDelivery {
                        source,
                        digest: pending.digest,
                    },
                );
            }
        } else {
            if pending.rules_baseline.is_none() {
                self.shared_rule_proposals.remove(&pending.path);
            }
            self.files
                .insert(pending.path, pending.rules_baseline.unwrap_or(snapshot));
        }
    }
}

#[derive(Default)]
struct RenderedCheckout {
    rules: BTreeMap<PathBuf, Vec<u8>>,
    skills: BTreeMap<PathBuf, ProjectSkillFile>,
}

fn ordered_rule_roots(
    team: Option<PathBuf>,
    personal: PathBuf,
    project: Option<PathBuf>,
) -> Vec<PathBuf> {
    team.into_iter()
        .chain(std::iter::once(personal))
        .chain(project)
        .collect()
}

struct ProjectSkillFile {
    source: String,
    bytes: Vec<u8>,
}

// A checkout can match several readable Spaces. A colliding skill has no
// unambiguous source, so neither copy participates in the render.
#[derive(Default)]
struct ProjectSkills {
    packages: BTreeMap<String, BTreeMap<PathBuf, ProjectSkillFile>>,
    conflicts: BTreeSet<String>,
}

impl ProjectSkills {
    fn from_layers(layers: &[TeamRuleLayer]) -> Result<Self> {
        let mut result = Self::default();
        for layer in layers {
            for skill in &layer.skills {
                if result.conflicts.contains(&skill.name) {
                    continue;
                }
                if result.packages.remove(&skill.name).is_some() {
                    result.conflicts.insert(skill.name.clone());
                    continue;
                }
                let files = skill
                    .files
                    .iter()
                    .map(|file| {
                        let source = fingerprint(&(&layer.id, &skill.name, &file.path))?;
                        Ok((
                            PathBuf::from(&file.path),
                            ProjectSkillFile {
                                source,
                                bytes: file.text.as_bytes().to_vec(),
                            },
                        ))
                    })
                    .collect::<Result<_>>()?;
                result.packages.insert(skill.name.clone(), files);
            }
        }
        Ok(result)
    }

    fn stage(&self, input: &Path) -> Result<()> {
        for (name, files) in &self.packages {
            let files = files
                .iter()
                .map(|(path, file)| (path.clone(), file.bytes.clone()))
                .collect();
            write_skill_files(input, name, &files)?;
        }
        Ok(())
    }

    fn take_rendered(
        &self,
        output: &mut BTreeMap<PathBuf, Vec<u8>>,
        rendered: &mut RenderedCheckout,
    ) -> Result<()> {
        for (name, files) in &self.packages {
            let assets = files.iter().map(|(path, file)| (path, &file.bytes));
            let root = rendered_skill_with_assets(output, name, assets)?
                .with_context(|| format!("rulesync did not render project skill {name}"))?;
            for (relative, file) in files {
                let path = root.join(relative);
                let bytes = output.remove(&path).with_context(|| {
                    format!("rulesync omitted {} from {name}", relative.display())
                })?;
                ensure!(
                    rendered
                        .skills
                        .insert(
                            path,
                            ProjectSkillFile {
                                source: file.source.clone(),
                                bytes
                            }
                        )
                        .is_none(),
                    "rulesync produced the same skill path for two tools"
                );
            }
            ensure!(
                !output.keys().any(|path| path.starts_with(&root)),
                "rulesync returned unexpected files for project skill {name}"
            );
        }
        Ok(())
    }
}

pub(crate) fn open(
    setup: &Setup,
    repo: &Path,
    master: &PersonalMaster,
    generated: &[String],
) -> Result<CheckoutRulesReceipt> {
    ensure!(repo.is_absolute(), "use an absolute checkout path");
    let repo = repo_wiki::repository_top_level(repo)?;
    let key = fingerprint(&repo)?;
    let state_path = PathBuf::from(format!("checkouts/{key}.json"));
    let root = setup.data_files()?;
    let state = root.read(&state_path, false)?;
    let checkout = match &state {
        Snapshot::Absent => Checkout {
            repo,
            ..Checkout::default()
        },
        Snapshot::File { bytes, .. } => read_checkout(bytes)?,
        _ => anyhow::bail!("checkout record must be a regular file"),
    };
    let directory = PathBuf::from(format!("checkouts/{key}/rules"));
    let job = Job {
        setup,
        session: &rulesync_renderer::RenderSession::new(&setup.binary),
        settings: shared_space::settings_path(),
        root: &root,
        directory: &directory,
        can_restore_rejection: true,
    };
    let result = job.run(&state_path, state, checkout, Some(master), generated)?;
    Ok(CheckoutRulesReceipt {
        directory: setup.data.join(directory),
        report: result,
    })
}

pub(crate) fn refresh(
    setup: &Setup,
    session: &rulesync_renderer::RenderSession<'_>,
    settings: &Path,
    master: Option<&PersonalMaster>,
    generated: &[String],
    can_restore_rejection: bool,
) -> RulesSyncReport {
    let result = (|| {
        let entries = match std::fs::read_dir(setup.data.join("checkouts")) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(report("Checked checkout rules"))
            }
            Err(error) => return Err(error.into()),
        };
        refresh_entries(
            setup,
            session,
            settings,
            master,
            generated,
            can_restore_rejection,
            entries,
        )
    })();
    match result {
        Ok(receipt) => receipt,
        Err(error) => {
            tracing::warn!(%error, "[rules setup] checkout receipts could not be read");
            report("Checked checkout rules")
        }
    }
}

fn team_layers_for_projects(
    teams: Vec<TeamRuleLayer>,
    projects: &[TeamRuleLayer],
) -> Vec<TeamRuleLayer> {
    teams
        .into_iter()
        .filter(|team| {
            projects
                .iter()
                .any(|project| rulesync_team::project_layer_belongs_to_team(team, project))
        })
        .collect()
}

fn refresh_entries(
    setup: &Setup,
    session: &rulesync_renderer::RenderSession<'_>,
    settings: &Path,
    master: Option<&PersonalMaster>,
    generated: &[String],
    can_restore_rejection: bool,
    entries: impl IntoIterator<Item = std::io::Result<std::fs::DirEntry>>,
) -> Result<RulesSyncReport> {
    let root = setup.data_files()?;
    let mut result = report("Checked checkout rules");
    for entry in entries {
        let refresh_entry = || -> Result<()> {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                return Ok(());
            }
            let leaf = PathBuf::from(entry.file_name());
            let Some(key) = leaf
                .file_stem()
                .and_then(|key| key.to_str())
                .filter(|key| key.len() == 64 && key.bytes().all(|byte| byte.is_ascii_hexdigit()))
            else {
                return Ok(());
            };
            if leaf.extension().and_then(|ext| ext.to_str()) != Some("json") {
                return Ok(());
            }
            let state_path = Path::new("checkouts").join(&leaf);
            let read_context = || format!("read checkout receipt {}", state_path.display());
            let state = root.read(&state_path, false).with_context(read_context)?;
            let Snapshot::File { bytes, .. } = &state else {
                return Ok(());
            };
            let checkout = read_checkout(bytes).with_context(read_context)?;
            let directory = PathBuf::from(format!("checkouts/{key}/rules"));
            let job = Job {
                setup,
                session,
                settings: settings.to_path_buf(),
                root: &root,
                directory: &directory,
                can_restore_rejection,
            };
            let checkout_dir = setup.data.join(&directory);
            match job.run(&state_path, state, checkout, master, generated) {
                Ok(report) => {
                    result.written.extend(
                        report
                            .written
                            .into_iter()
                            .map(|path| checkout_dir.join(path).display().to_string()),
                    );
                    for (problem, code) in report.blocked.into_iter().zip(report.blocked_codes) {
                        let problem = format!("Checkout {} — {problem}", checkout_dir.display());
                        tracing::warn!(%problem, "[rules setup] checkout delivery needs attention");
                        result.block(code, problem);
                    }
                    if !report.notice_codes.is_empty() {
                        rulesync_layers::report_rejection(&mut result);
                    }
                }
                Err(error) => result.block(
                    Code::CheckoutNeedsReview,
                    format!("Checkout {} — {error:#}", checkout_dir.display()),
                ),
            }
            Ok(())
        };
        if let Err(error) = refresh_entry() {
            tracing::warn!(error = %format_args!("{error:#}"), "[rules setup] checkout rules refresh failed");
        }
    }
    Ok(result)
}

struct Job<'a> {
    setup: &'a Setup,
    session: &'a rulesync_renderer::RenderSession<'a>,
    settings: PathBuf,
    root: &'a FileRoot,
    directory: &'a Path,
    can_restore_rejection: bool,
}

fn report_blocks(result: &mut RulesSyncReport, blocked: BTreeMap<PathBuf, String>) {
    result.extend_blocks(
        Code::CheckoutNeedsReview,
        blocked
            .into_iter()
            .map(|(path, reason)| format!("{}: {reason}", path.display())),
    );
}

impl Job<'_> {
    fn run(
        &self,
        state_path: &Path,
        state: Snapshot,
        checkout: Checkout,
        master: Option<&PersonalMaster>,
        generated: &[String],
    ) -> Result<RulesSyncReport> {
        self.run_using(
            state_path,
            state,
            checkout,
            master,
            |project, teams, skills, master| self.render(project, teams, skills, master, generated),
        )
    }

    fn run_using(
        &self,
        state_path: &Path,
        state: Snapshot,
        mut checkout: Checkout,
        master: Option<&PersonalMaster>,
        render: impl FnOnce(
            &[TeamRuleLayer],
            &[TeamRuleLayer],
            &ProjectSkills,
            &PersonalMaster,
        ) -> Result<RenderedCheckout>,
    ) -> Result<RulesSyncReport> {
        let repo_id = master
            .and_then(|_| repo_wiki::origin_url(&checkout.repo))
            .as_deref()
            .and_then(rulesync_team::repo_identity);
        let project = match repo_id {
            Some(id) => rulesync_team::load_project_layers(&self.settings)?
                .into_iter()
                .filter(|layer| {
                    layer
                        .project
                        .as_ref()
                        .and_then(|project| project.repo_id.as_ref())
                        == Some(&id)
                })
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        };
        let teams = match master {
            Some(_) if !project.is_empty() => {
                team_layers_for_projects(rulesync_team::load_layers(&self.settings)?, &project)
            }
            _ => Vec::new(),
        };
        let skills = ProjectSkills::from_layers(&project)?;
        let authorized: BTreeMap<_, _> = project
            .iter()
            .chain(&teams)
            .map(|layer| Ok((rulesync_layers::key(layer)?, layer)))
            .collect::<Result<_>>()?;
        let keys: BTreeSet<_> = authorized.keys().cloned().collect();
        let mut result = report("Checked the rules for this checkout");
        result.extend_blocks(
            Code::CheckoutNeedsReview,
            skills
                .conflicts
                .iter()
                .map(|name| format!("project skill {name} is approved in more than one Space")),
        );
        let mut blocked = BTreeMap::new();
        self.recover_pending(&mut checkout, &mut blocked)?;
        let skill_sources: BTreeSet<_> = skills
            .packages
            .values()
            .flat_map(|files| files.values().map(|file| file.source.as_str()))
            .collect();
        let mut record = state;
        let mut observed_skills = self.prune_skills(
            state_path,
            &mut record,
            &mut checkout,
            &skill_sources,
            &mut blocked,
        )?;
        // Remove a refused layer from the last delivery even if a later render fails.
        let mut withdrawals = Vec::new();
        for (path, delivered) in &mut checkout.files {
            if blocked.contains_key(path) {
                continue;
            }
            let before = text(delivered)?;
            let remaining = rulesync_layers::remove_inaccessible(before, &keys)?;
            if remaining != before {
                let current = match self.root.read(&self.directory.join(path), false) {
                    Ok(current) => current,
                    Err(error) => {
                        blocked.insert(
                            path.clone(),
                            format!("Could not read the file to remove revoked rules. {error}"),
                        );
                        continue;
                    }
                };
                if current == Snapshot::Absent {
                    *delivered = current;
                    continue;
                }
                let after = text(&current).and_then(|current| {
                    rulesync_layers::edits(before, current)?;
                    rulesync_layers::remove_inaccessible(current, &keys)
                });
                let Ok(after) = after else {
                    blocked.insert(path.clone(), "Could not remove revoked rules because the section markers changed. Reconcile the checkout rules and sync again.".into());
                    continue;
                };
                let desired = with_bytes(&current, after.into_bytes());
                let pending = PendingDelivery {
                    path: path.clone(),
                    digest: fingerprint(&desired)?,
                    skill_source: None,
                    stage: None,
                    rules_baseline: Some(with_bytes(delivered, remaining.into_bytes())),
                };
                withdrawals.push((pending, current, desired));
            }
        }
        record = self.save(state_path, &record, &checkout)?;
        for (pending, current, desired) in withdrawals {
            let path = pending.path.clone();
            let outcome = self
                .deliver(
                    state_path,
                    &mut record,
                    &mut checkout,
                    pending,
                    &current,
                    desired,
                )
                .with_context(|| {
                    format!("Could not remove revoked rules from {}", path.display())
                })?;
            if let Some(reason) =
                outcome.conflict("The file changed during withdrawal. Sync again.")
            {
                blocked.insert(path, format!("Could not remove revoked rules. {reason}"));
            } else {
                result.written.push(path.display().to_string());
            }
        }
        let local_key = state_path
            .file_stem()
            .and_then(|s| s.to_str())
            .context("invalid checkout key")?;
        let has_repo = !checkout.repo.as_os_str().is_empty();
        let local_revoked = project.is_empty() && has_repo;
        if has_repo {
            let cleanup = if local_revoked {
                crate::rulesync_checkout_local::sync(
                    self.setup,
                    &crate::rulesync_checkout_local::codex_home(&self.setup.home),
                    &self.settings,
                    &checkout.repo,
                    &project,
                    &teams,
                    &authorized,
                    &mut checkout.local_claude,
                    &mut checkout.local_codex,
                    &mut checkout.local_codex_dir_created,
                    &mut checkout.shared_rule_proposals,
                    &mut result,
                    self.can_restore_rejection,
                    local_key,
                )
            } else {
                crate::rulesync_checkout_local::prune_inaccessible(
                    self.setup,
                    &checkout.repo,
                    local_key,
                    &authorized,
                    &mut checkout.local_claude,
                    &mut checkout.local_codex,
                    &mut checkout.shared_rule_proposals,
                    &mut result,
                )
            };
            record = self.save(state_path, &record, &checkout)?;
            if let Err(error) = cleanup {
                result.block(
                    Code::CheckoutNeedsReview,
                    format!("Checkout local revocation: {error:#}"),
                );
            }
        }
        let Some(master) = master else {
            report_blocks(&mut result, blocked);
            return Ok(result);
        };
        let mut observed = BTreeMap::new();
        let mut rejected = BTreeSet::new();
        let mut pending_review = BTreeSet::new();
        for (path, delivered) in &checkout.files {
            if blocked.contains_key(path) {
                continue;
            }
            let current = self.root.read(&self.directory.join(path), false)?;
            if snapshot_has_credentials(&current) {
                blocked.insert(
                    path.clone(),
                    "The checkout rules may contain a credential. The file was left in place."
                        .into(),
                );
                continue;
            }
            if current != *delivered {
                let before = text(delivered)?;
                let after = text(&current)?;
                match rulesync_layers::edits(before, after) {
                    Ok(edits)
                        if !edits.is_empty()
                            && rulesync_layers::only_personal(before)?
                                == rulesync_layers::only_personal(after)? =>
                    {
                        let receipts = checkout
                            .shared_rule_proposals
                            .entry(path.clone())
                            .or_default();
                        match rulesync_layers::review_edits(
                            &self.settings,
                            &authorized,
                            edits,
                            receipts,
                            &self.directory.join(path),
                            &self.setup.data,
                            &self.setup.home,
                        ) {
                            Ok(rulesync_layers::SharedReview::NeedsConfirm) => {
                                pending_review.insert(path.clone());
                                result.block(Code::SharedRuleNeedsConfirm, format!("{}: This edit is waiting for you to confirm it. Run `wenlan rules shared-edits` in your own terminal to send or decline it.", path.display()));
                            }
                            Ok(rulesync_layers::SharedReview::Pending) => {
                                pending_review.insert(path.clone());
                                result.block(Code::SharedRulePending, format!("{}: A shared rule edit is still in this file while its proposal is reviewed.", path.display()));
                            }
                            Ok(rulesync_layers::SharedReview::Decided { rejected: true }) => {
                                if self.can_restore_rejection {
                                    rejected.insert(path.clone());
                                } else {
                                    pending_review.insert(path.clone());
                                    result.block(Code::SharedRulePending, format!("{}: A shared rule edit was rejected. Run Sync now to restore the approved rules.", path.display()));
                                }
                            }
                            Ok(rulesync_layers::SharedReview::Decided { rejected: false }) => (),
                            Err(error) => {
                                blocked.insert(path.clone(), error.to_string());
                            }
                        }
                    }
                    _ => {
                        blocked.insert(path.clone(), "The checkout rules or their section markers changed. Reconcile the edits with the Rules page and sync again.".into());
                    }
                }
            } else {
                checkout.shared_rule_proposals.remove(path);
            }
            observed.insert(path.clone(), current);
        }
        checkout
            .shared_rule_proposals
            .retain(|_, receipts| !receipts.is_empty());
        record = self.save(state_path, &record, &checkout)?;
        // Capture exact bytes before invoking the renderer, which may be slow.
        let rendered = match render(&project, &teams, &skills, master) {
            Ok(files) => files,
            Err(error) => {
                result.block(
                    Code::CheckoutNeedsReview,
                    format!("Checkout render: {error:#}"),
                );
                report_blocks(&mut result, blocked);
                return Ok(result);
            }
        };
        for (path, bytes) in rendered.rules {
            let current = observed.get(&path).unwrap_or(&Snapshot::Absent);
            if self.can_restore_rejection
                && pending_review.contains(&path)
                && !blocked.contains_key(&path)
            {
                let mut answers = crate::rulesync_shared_answers::read(&self.setup.data)?;
                let full_path = self.directory.join(&path);
                let old = checkout
                    .files
                    .get(&path)
                    .context("managed checkout file is missing")?;
                let old_text = text(old)?;
                let local = text(current)?;
                let new_text = std::str::from_utf8(&bytes)?;
                for (id, waiting) in answers.waiting.clone() {
                    if !waiting.files.contains(&full_path.display().to_string()) {
                        continue;
                    }
                    let Some((key, layer)) = authorized.iter().find(|(_, layer)| {
                        layer.id == waiting.layer_id && layer.revision != waiting.revision
                    }) else {
                        continue;
                    };
                    if !rulesync_layers::waiting_in_file(
                        &waiting,
                        &full_path.display().to_string(),
                        &self.settings,
                        &self.setup.data,
                        &self.setup.home,
                    ) {
                        continue;
                    }
                    if rulesync_layers::only_personal(old_text)?
                        != rulesync_layers::only_personal(local)?
                    {
                        continue;
                    }
                    let old_blocks = rulesync_layers::blocks(old_text)?;
                    let local_blocks = rulesync_layers::blocks(local)?;
                    let new_blocks = rulesync_layers::blocks(new_text)?;
                    let (Some(base), Some(mine), Some(theirs)) = (
                        old_blocks.get(key.as_str()),
                        local_blocks.get(key.as_str()),
                        new_blocks.get(key.as_str()),
                    ) else {
                        continue;
                    };
                    let merged = wenlan_core::rulesync_personal::merge_rule_text(
                        base.body,
                        mine.body,
                        theirs.body,
                    )
                    .context(wenlan_types::rulesync::SHARED_EDIT_STALE_REVISION_MESSAGE)?;
                    let rebased = rulesync_layers::replace_block_body(new_text, key, &merged)?;
                    let updated = with_bytes(current, rebased.into_bytes());
                    ensure!(
                        self.root.apply(&full_path, current, &updated, false)?
                            == ApplyOutcome::Applied,
                        "checkout rules changed during rebase"
                    );
                    let changed_text = text(&updated)?;
                    let changed_blocks = rulesync_layers::blocks(changed_text)?;
                    let changed = changed_blocks
                        .get(key.as_str())
                        .context("rebased checkout section is missing")?;
                    let new_id = fingerprint(&(&layer.id, changed.revision, changed.body))?;
                    let markdown = rulesync_layers::proposed_markdown(layer, changed)?;
                    answers.waiting.remove(&id);
                    let mut next_waiting = waiting;
                    next_waiting.revision = layer.revision.clone();
                    next_waiting.markdown = markdown;
                    answers.waiting.insert(new_id, next_waiting);
                    crate::rulesync_shared_answers::save(&self.setup.data, &answers)?;
                    checkout
                        .files
                        .insert(path.clone(), with_bytes(old, bytes.clone()));
                    record = self.save(state_path, &record, &checkout)?;
                    break;
                }
            }
            if blocked.contains_key(&path) || pending_review.contains(&path) {
                continue;
            }
            let found = rulesync_layers::blocks(std::str::from_utf8(&bytes)?)?;
            ensure!(
                found
                    .keys()
                    .map(|key| (*key).to_owned())
                    .collect::<BTreeSet<_>>()
                    == keys,
                "rulesync did not preserve the authorized checkout layers"
            );
            let desired = with_bytes(current, bytes);
            let changed = current != &desired;
            let pending = PendingDelivery {
                path: path.clone(),
                digest: fingerprint(&desired)?,
                skill_source: None,
                stage: None,
                rules_baseline: None,
            };
            let outcome = self.deliver(
                state_path,
                &mut record,
                &mut checkout,
                pending,
                current,
                desired,
            )?;
            if let Some(reason) =
                outcome.conflict("Changed during delivery. Sync again to collect the latest edits.")
            {
                blocked.insert(path, reason);
            } else if changed {
                if rejected.contains(&path) {
                    rulesync_layers::report_rejection(&mut result);
                }
                result.written.push(path.display().to_string());
            }
        }
        for (path, file) in rendered.skills {
            if blocked.contains_key(&path) {
                continue;
            }
            let current = observed_skills.remove(&path).unwrap_or(Snapshot::Absent);
            let desired = with_bytes(&current, file.bytes);
            let changed = current != desired;
            let pending = PendingDelivery {
                path: path.clone(),
                digest: fingerprint(&desired)?,
                skill_source: Some(file.source),
                stage: None,
                rules_baseline: None,
            };
            let outcome = self.deliver(
                state_path,
                &mut record,
                &mut checkout,
                pending,
                &current,
                desired,
            )?;
            if let Some(reason) =
                outcome.conflict("Changed during delivery. Check the skill files and sync again.")
            {
                blocked.insert(path, reason);
            } else if changed {
                result.written.push(path.display().to_string());
            }
        }
        let local = (!local_revoked).then(|| {
            crate::rulesync_checkout_local::sync(
                self.setup,
                &crate::rulesync_checkout_local::codex_home(&self.setup.home),
                &self.settings,
                &checkout.repo,
                &project,
                &teams,
                &authorized,
                &mut checkout.local_claude,
                &mut checkout.local_codex,
                &mut checkout.local_codex_dir_created,
                &mut checkout.shared_rule_proposals,
                &mut result,
                self.can_restore_rejection,
                local_key,
            )
        });
        self.save(state_path, &record, &checkout)?;
        if let Some(Err(error)) = local {
            result.block(
                Code::CheckoutNeedsReview,
                format!("Checkout local files: {error:#}"),
            );
        }
        report_blocks(&mut result, blocked);
        rulesync_layers::refresh_answers(&self.settings, &self.setup.data, &self.setup.home)?;
        Ok(result)
    }

    fn recover_pending(
        &self,
        checkout: &mut Checkout,
        blocked: &mut BTreeMap<PathBuf, String>,
    ) -> Result<()> {
        if let Some(pending) = checkout.pending.take() {
            checkout.unresolved.push(pending);
        }
        for pending in std::mem::take(&mut checkout.unresolved) {
            let path = &pending.path;
            let destination = self.directory.join(path);
            let stage = pending.stage.as_ref().context(UnsupportedRulesyncState)?;
            let recovered = self.root.read(&destination, false).and_then(|current| {
                self.root.recover_stage(
                    &destination,
                    &stage.seen,
                    &stage.desired,
                    &current,
                    stage.options(self.root),
                )?;
                self.sync_parents(&destination)?;
                Ok(current)
            });
            if let Ok(current) = recovered {
                let digest = fingerprint(&current)?;
                if digest == pending.digest {
                    checkout.settle(pending, current);
                    continue;
                }
                if current == Snapshot::Absent || current == stage.seen {
                    continue;
                }
            }
            blocked.insert(path.clone(), "Interrupted checkout delivery needs review. Reconcile the local file and sync again.".into());
            checkout.unresolved.push(pending);
        }
        Ok(())
    }

    fn deliver(
        &self,
        state_path: &Path,
        record: &mut Snapshot,
        checkout: &mut Checkout,
        mut pending: PendingDelivery,
        current: &Snapshot,
        desired: Snapshot,
    ) -> Result<DeliveryOutcome> {
        let destination = self.directory.join(&pending.path);
        if current == &desired {
            if self.root.apply(&destination, current, &desired, false)?
                == ApplyOutcome::ChangedSincePlan
            {
                return Ok(DeliveryOutcome::Changed);
            }
            checkout.settle(pending, desired);
            *record = self.save(state_path, record, checkout)?;
            return Ok(DeliveryOutcome::Applied);
        }
        // Persist the exact plan and recovery names before modifying the
        // destination. The archive keeps the displaced inode reachable to
        // account for saves through a handle opened before delivery.
        let displaced = matches!(current, Snapshot::File { .. })
            .then(|| prepare_checkout_displaced(self.root, &destination))
            .transpose()?;
        pending.stage = Some(CheckoutStage {
            id: uuid::Uuid::new_v4().simple().to_string(),
            displaced,
            seen: current.clone(),
            desired: desired.clone(),
        });
        checkout.pending = Some(pending);
        *record = self.save(state_path, record, checkout)?;
        let pending = checkout
            .pending
            .take()
            .context("missing checkout journal")?;
        let stage = pending.stage.as_ref().context("missing checkout stage")?;
        let applied =
            match self
                .root
                .apply_with(&destination, current, &desired, stage.options(self.root))
            {
                Ok(outcome) => outcome == ApplyOutcome::Applied,
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::PermissionDenied | io::ErrorKind::Unsupported
                    ) =>
                {
                    // Preserve the journal in case the stage still holds an entry.
                    checkout.unresolved.push(pending);
                    *record = self.save(state_path, record, checkout)?;
                    return Ok(DeliveryOutcome::Refused(error));
                }
                Err(error) => return Err(error.into()),
            };
        if let Some(directory) = stage.displaced.as_deref().and_then(Path::parent) {
            self.root.sync_directory(directory)?;
        }
        self.sync_parents(&destination)?;
        if applied {
            checkout.settle(pending, desired);
        }
        *record = self.save(state_path, record, checkout)?;
        Ok(if applied {
            DeliveryOutcome::Applied
        } else {
            DeliveryOutcome::Changed
        })
    }

    fn prune_skills(
        &self,
        state_path: &Path,
        record: &mut Snapshot,
        checkout: &mut Checkout,
        allowed: &BTreeSet<&str>,
        blocked: &mut BTreeMap<PathBuf, String>,
    ) -> Result<BTreeMap<PathBuf, Snapshot>> {
        let mut retired = Vec::new();
        let mut observed = BTreeMap::new();
        for (path, delivery) in &checkout.skills {
            if blocked.contains_key(path) {
                continue;
            }
            let current = self.root.read(&self.directory.join(path), false)?;
            let matches_delivery =
                current == Snapshot::Absent || fingerprint(&current)? == delivery.digest;
            if !allowed.contains(delivery.source.as_str()) {
                if !matches_delivery {
                    blocked.insert(path.clone(), "The removed skill was edited. Reconcile the local file and sync again to finish removing it.".into());
                } else {
                    retired.push((path.clone(), delivery.source.clone(), current));
                }
            } else if matches_delivery {
                observed.insert(path.clone(), current);
            } else {
                blocked.insert(path.clone(), "The installed skill was edited. Reconcile the change with the Space's approved skill and sync again.".into());
            }
        }
        for (path, source, current) in retired {
            let desired = Snapshot::Absent;
            let pending = PendingDelivery {
                path: path.clone(),
                digest: fingerprint(&desired)?,
                skill_source: Some(source),
                stage: None,
                rules_baseline: None,
            };
            let outcome = self.deliver(state_path, record, checkout, pending, &current, desired)?;
            if let Some(reason) = outcome.conflict(
                "Changed while Wenlan was removing a skill. Sync again to finish removing it.",
            ) {
                blocked.insert(path, reason);
            }
        }
        Ok(observed)
    }

    fn save(&self, path: &Path, seen: &Snapshot, checkout: &Checkout) -> Result<Snapshot> {
        let desired = with_bytes(
            seen,
            serde_json::to_vec(&CheckoutRecord {
                format: CHECKOUT_FORMAT,
                checkout,
            })?,
        );
        ensure!(
            self.root.apply(path, seen, &desired, false)? == ApplyOutcome::Applied,
            "checkout receipt changed during sync"
        );
        if desired != *seen {
            self.sync_parents(path)?;
        }
        Ok(desired)
    }

    fn sync_parents(&self, path: &Path) -> std::io::Result<()> {
        for parent in path.ancestors().skip(1) {
            self.root.sync_directory(parent)?;
        }
        Ok(())
    }

    #[cfg(test)]
    fn replace(&self, path: &Path, seen: &Snapshot, desired: &Snapshot) -> Result<bool> {
        Ok(self
            .root
            .apply(&self.directory.join(path), seen, desired, false)?
            == ApplyOutcome::Applied)
    }

    fn render(
        &self,
        project: &[TeamRuleLayer],
        teams: &[TeamRuleLayer],
        skills: &ProjectSkills,
        master: &PersonalMaster,
        generated: &[String],
    ) -> Result<RenderedCheckout> {
        let mut rendered = RenderedCheckout::default();
        let features: &[&str] = if skills.packages.is_empty() {
            &["rules"]
        } else {
            &["rules", "skills"]
        };
        for target in ["claudecode", "codexcli"] {
            let stage = rulesync_renderer::new_stage(&self.setup.data)?;
            let output = stage.path().join("output");
            // Broadest first: team, then personal, then project.
            let team_root = if !teams.is_empty() {
                let input = stage.path().join("team");
                rulesync_layers::stage(&input, "Team", teams)?;
                Some(input)
            } else {
                None
            };
            let project_root = stage.path().join("project");
            if !project.is_empty() {
                rulesync_layers::stage(&project_root, "Project", project)?;
            }
            skills.stage(&project_root)?;
            let personal = stage.path().join("personal");
            let mut source = master.rule_text(target, generated)?;
            if target == "claudecode" {
                let native = wenlan_core::rulesync_adopt::without_shared(&master.shared, &source);
                source = wenlan_core::rulesync_adopt::checkout_claude_imports(
                    &self.setup.home,
                    &native,
                    &source,
                )?;
            }
            write_root_rule(&personal, &source)?;
            let roots = ordered_rule_roots(
                team_root,
                personal,
                (!project.is_empty()).then_some(project_root),
            );
            std::fs::create_dir(&output)?;
            self.session.render(
                stage.path(),
                &roots,
                &output,
                false,
                &[target.into()],
                features,
            )?;
            let mut files = BTreeMap::new();
            read_stage(&output, &output, &mut files)?;
            skills.take_rendered(&mut files, &mut rendered)?;
            rendered.rules.extend(files);
        }
        Ok(rendered)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wenlan_types::team::{TeamProjectSkill, TeamProjectSkillFile};

    #[test]
    fn rulesync_checkout_team_rules_are_scoped_to_matching_server() {
        use std::{fs, process::Command};

        let scratch = tempfile::tempdir().unwrap();
        let setup = Setup {
            home: scratch.path().join("home"),
            data: scratch.path().join("data"),
            binary: PathBuf::from("unused"),
        };
        fs::create_dir_all(setup.home.join(".codex")).unwrap();
        fs::create_dir_all(&setup.data).unwrap();
        let repo = scratch.path().join("checkout");
        fs::create_dir(&repo).unwrap();
        assert!(Command::new("git")
            .args(["-C", repo.to_str().unwrap(), "init", "--quiet"])
            .status()
            .unwrap()
            .success());
        fs::write(
            setup.home.join(".codex/config.toml"),
            crate::rulesync_checkout_local::trust_config(&repo.display().to_string()),
        )
        .unwrap();
        let make_team = |id: &str, markdown: &str| TeamRuleLayer {
            id: id.into(),
            name: id.into(),
            markdown: markdown.into(),
            revision: "a".repeat(64),
            synced_at: String::new(),
            project: None,
            skills: Vec::new(),
            proposal_decisions: BTreeMap::new(),
        };
        let make_project = |team_id: &str, space_id: &str, repo_id: &str| TeamRuleLayer {
            id: format!(
                "{}{}",
                "team-rules-",
                wenlan_core::shared_space::version::sha256_hex(
                    format!("{team_id}\n{space_id}").as_bytes()
                )
            ),
            name: format!("{team_id} project"),
            markdown: "Project rules".into(),
            revision: "b".repeat(64),
            synced_at: String::new(),
            project: Some(wenlan_types::team::TeamRulesProject {
                space: space_id.into(),
                space_id: space_id.into(),
                repo_id: Some(repo_id.into()),
            }),
            skills: Vec::new(),
            proposal_decisions: BTreeMap::new(),
        };
        let first = make_team("first-server", "FIRST_TEAM_RULE");
        let second = make_team("second-server", "SECOND_TEAM_RULE");
        let matching = make_project("first-server", "first-space", "repo-identity");
        let matching_projects = vec![matching];
        let selected = team_layers_for_projects(vec![first, second], &matching_projects);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].markdown, "FIRST_TEAM_RULE");
        assert!(!selected
            .iter()
            .any(|layer| layer.markdown.contains("SECOND_TEAM_RULE")));

        let authorized: BTreeMap<_, _> = selected
            .iter()
            .chain(&matching_projects)
            .map(|layer| (rulesync_layers::key(layer).unwrap(), layer))
            .collect();
        let mut claude = None;
        let mut codex = None;
        let mut codex_dir_created = false;
        let mut receipts = BTreeMap::new();
        let key = fingerprint(&repo).unwrap();
        let mut result = report("checkout");
        crate::rulesync_checkout_local::sync(
            &setup,
            &setup.home.join(".codex"),
            &scratch.path().join("settings.json"),
            &repo,
            &matching_projects,
            &selected,
            &authorized,
            &mut claude,
            &mut codex,
            &mut codex_dir_created,
            &mut receipts,
            &mut result,
            true,
            &key,
        )
        .unwrap();
        let claude = fs::read_to_string(repo.join("CLAUDE.local.md")).unwrap();
        let codex = fs::read_to_string(repo.join(".codex/config.toml")).unwrap();
        for content in [&claude, &codex] {
            assert!(content.contains("FIRST_TEAM_RULE"), "{content}");
            assert!(!content.contains("SECOND_TEAM_RULE"), "{content}");
        }
    }

    #[test]
    fn rulesync_checkout_does_not_load_team_for_an_unmatched_origin() {
        use crate::rulesync_layers::tests::hosted_fixture;
        use std::{fs, process::Command};

        let scratch = tempfile::tempdir().unwrap();
        let setup = Setup {
            home: scratch.path().join("home"),
            data: scratch.path().join("data"),
            binary: PathBuf::from("unused"),
        };
        fs::create_dir(&setup.home).unwrap();
        fs::create_dir(&setup.data).unwrap();
        let (settings, _, _) = hosted_fixture(&setup.data);
        let repo = scratch.path().join("unmatched");
        fs::create_dir(&repo).unwrap();
        for args in [
            &["init", "--quiet"][..],
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/other/repo.git",
            ],
        ] {
            assert!(Command::new("git")
                .current_dir(&repo)
                .args(args)
                .status()
                .unwrap()
                .success());
        }
        let key = fingerprint(&repo).unwrap();
        let state_path = PathBuf::from(format!("checkouts/{key}.json"));
        let directory = PathBuf::from(format!("checkouts/{key}/rules"));
        let root = setup.data_files().unwrap();
        let session = rulesync_renderer::RenderSession::new(&setup.binary);
        let job = Job {
            setup: &setup,
            session: &session,
            settings,
            root: &root,
            directory: &directory,
            can_restore_rejection: true,
        };
        let master =
            PersonalMaster::new(&rulesync_personal::discover_adoption(&setup.home).unwrap());
        let report = job
            .run_using(
                &state_path,
                Snapshot::Absent,
                Checkout {
                    repo: repo.clone(),
                    ..Checkout::default()
                },
                Some(&master),
                |project, team, _, _| {
                    assert!(project.is_empty() && team.is_empty());
                    Ok(RenderedCheckout::default())
                },
            )
            .unwrap();
        assert!(report.blocked.is_empty(), "{report:?}");
        assert!(!repo.join("CLAUDE.local.md").exists());
        assert!(!repo.join(".codex/config.toml").exists());
    }

    #[test]
    fn rulesync_ordered_rule_roots_are_team_personal_project() {
        let roots = ordered_rule_roots(
            Some(PathBuf::from("team")),
            PathBuf::from("personal"),
            Some(PathBuf::from("project")),
        );
        assert_eq!(roots, ["team", "personal", "project"].map(PathBuf::from));
    }

    #[test]
    #[ignore = "requires WENLAN_TEST_RULESYNC_BINARY pointing to the pinned renderer"]
    fn rulesync_checkout_claude_and_codex_roots_follow_team_personal_project_order() {
        use crate::rulesync_layers::tests::{hosted_fixture, REMOTE};
        use std::{fs, process::Command};
        assert_eq!(std::env::var("WENLAN_NO_AUTOSTART").as_deref(), Ok("1"));
        let binary = PathBuf::from(std::env::var_os("WENLAN_TEST_RULESYNC_BINARY").unwrap());
        let scratch = tempfile::tempdir().unwrap();
        let setup = Setup {
            home: scratch.path().join("home"),
            data: scratch.path().join("data"),
            binary,
        };
        fs::create_dir(&setup.home).unwrap();
        fs::create_dir(&setup.data).unwrap();
        let (settings, _, _) = hosted_fixture(&setup.data);
        let repo = scratch.path().join("checkout");
        fs::create_dir(&repo).unwrap();
        for args in [
            &["init", "--quiet"][..],
            &["remote", "add", "origin", REMOTE],
        ] {
            assert!(Command::new("git")
                .current_dir(&repo)
                .args(args)
                .status()
                .unwrap()
                .success());
        }
        let root = setup.data_files().unwrap();
        let session = rulesync_renderer::RenderSession::new(&setup.binary);
        let directory = Path::new("checkouts/render/rules");
        let job = Job {
            setup: &setup,
            session: &session,
            settings,
            root: &root,
            directory,
            can_restore_rejection: true,
        };
        let master =
            PersonalMaster::new(&rulesync_personal::discover_adoption(&setup.home).unwrap());
        let report = job
            .run_using(
                Path::new("checkouts/render.json"),
                Snapshot::Absent,
                Checkout {
                    repo,
                    ..Checkout::default()
                },
                Some(&master),
                |project, teams, _, master| {
                    assert_eq!(project.len(), 1);
                    assert_eq!(teams.len(), 1);
                    let mut project = project.to_vec();
                    project[0].skills.push(TeamProjectSkill {
                        name: "checked-procedure".into(),
                        files: vec![TeamProjectSkillFile {
                            path: "SKILL.md".into(),
                            text: "---\nname: checked-procedure\ndescription: Checked procedure.\n---\nExpected skill\n".into(),
                        }],
                    });
                    let skills = ProjectSkills::from_layers(&project)?;
                    job.render(
                        &project,
                        teams,
                        &skills,
                        master,
                        &["Personal instruction.".into()],
                    )
                },
            )
            .unwrap();
        assert!(
            report
                .blocked
                .iter()
                .all(|line| line.contains("until you trust this repository in Codex")),
            "{report:?}"
        );
        for file in ["CLAUDE.md", "AGENTS.md"] {
            let output = fs::read_to_string(setup.data.join(directory).join(file)).unwrap();
            println!("----- rendered {file} -----\n{output}----- end {file} -----");
            let team = output.find("Team rules for").unwrap();
            let personal = output.find("Personal instruction.").unwrap();
            let project = output.find("Project rules for").unwrap();
            assert!(team < personal && personal < project, "{file}: {output}");
            assert!(output.contains("Rules are listed from broadest to most specific. When two disagree, follow the later one."));
        }
        assert!(setup
            .data
            .join(directory)
            .join(".agents/skills/checked-procedure/SKILL.md")
            .is_file());
    }

    #[cfg(not(windows))]
    #[test]
    fn rulesync_step1j_first_checkout_delivery_skips_the_windows_replacement_check() {
        check_checkout_windows_acl_refusal(true);
    }

    #[cfg(not(windows))]
    #[test]
    fn rulesync_step1j_refused_checkout_withdrawal_names_the_live_file() {
        check_checkout_windows_acl_refusal(false);
    }

    #[cfg(not(windows))]
    fn check_checkout_windows_acl_refusal(first_delivery: bool) {
        let scratch = tempfile::tempdir().unwrap();
        let setup = Setup {
            home: scratch.path().join("home"),
            data: scratch.path().to_owned(),
            binary: scratch.path().join("unavailable-renderer"),
        };
        let mut root = FileRoot::open(&setup.data).unwrap();
        root.refuse_windows_acl_in_test = true;
        let job = Job {
            setup: &setup,
            session: &rulesync_renderer::RenderSession::new(&setup.binary),
            settings: scratch.path().join("settings.toml"),
            root: &root,
            directory: Path::new("checkouts/example/rules"),
            can_restore_rejection: true,
        };
        let path = PathBuf::from("AGENTS.md");
        let destination = job.directory.join(&path);
        let state_path = Path::new("checkouts/example.json");
        let approved = Snapshot::file(
            format!(
                "<!-- WENLAN SHARED RULES {} {} -->\n## Project rules\n\nLast synced date.\n\nRevoked advice.\n<!-- END WENLAN SHARED RULES -->\nPersonal advice.\n",
                "a".repeat(64),
                "b".repeat(64),
            )
            .into_bytes(),
        );
        let mut checkout = Checkout::default();
        if first_delivery {
            let pending = PendingDelivery {
                path: path.clone(),
                digest: fingerprint(&approved).unwrap(),
                skill_source: None,
                rules_baseline: None,
                stage: None,
            };
            let mut state = Snapshot::Absent;
            let outcome = job
                .deliver(
                    state_path,
                    &mut state,
                    &mut checkout,
                    pending,
                    &Snapshot::Absent,
                    approved,
                )
                .unwrap();
            assert!(outcome.conflict("Unexpected file change").is_none());
            assert_eq!(
                root.read(&destination, false).unwrap(),
                checkout.files[&path]
            );
        } else {
            assert!(job.replace(&path, &Snapshot::Absent, &approved).unwrap());
            checkout.files.insert(path.clone(), approved.clone());
            let state = job.save(state_path, &Snapshot::Absent, &checkout).unwrap();
            let report = job.run(state_path, state, checkout, None, &[]).unwrap();
            assert!(report.written.is_empty(), "{report:?}");
            assert!(
                report.blocked.iter().any(|reason| {
                    reason.contains("AGENTS.md")
                        && reason.contains("revoked")
                        && reason.contains(
                            "the Windows owner is not the process user or one of its owner groups",
                        )
                }),
                "{report:?}"
            );
            assert_eq!(root.read(&destination, false).unwrap(), approved);
            let stored = root.read(state_path, false).unwrap();
            let saved = read_checkout(text(&stored).unwrap().as_bytes()).unwrap();
            assert_eq!(saved.files[&path], approved);
            let default_access = FileRoot::open(&setup.data).unwrap();
            let retry = Job {
                root: &default_access,
                ..job
            };
            let report = retry.run(state_path, stored, saved, None, &[]).unwrap();
            assert!(report.blocked.is_empty(), "{report:?}");
            let current = default_access.read(&destination, false).unwrap();
            assert_eq!(text(&current).unwrap(), "Personal advice.\n");
        }
    }

    #[test]
    fn rulesync_step1g_refresh_continues_after_an_incompatible_checkout_receipt() {
        use std::fs;

        let scratch = tempfile::tempdir().unwrap();
        let setup = Setup {
            home: scratch.path().join("home"),
            data: scratch.path().join("data"),
            binary: scratch.path().join("unavailable-renderer"),
        };
        let checkouts = setup.data.join("checkouts");
        fs::create_dir(&setup.home).unwrap();
        fs::create_dir_all(&checkouts).unwrap();
        let legacy = checkouts.join(format!("{}.json", "a".repeat(64)));
        let key = "b".repeat(64);
        let valid = checkouts.join(format!("{key}.json"));
        fs::write(&legacy, b"{}").unwrap();
        let directory = PathBuf::from(format!("checkouts/{key}/rules"));
        let session = rulesync_renderer::RenderSession::new(&setup.binary);
        let settings = scratch.path().join("settings.toml");
        let root = setup.data_files().unwrap();
        let job = Job {
            setup: &setup,
            session: &session,
            settings: settings.clone(),
            root: &root,
            directory: &directory,
            can_restore_rejection: true,
        };
        let rule = PathBuf::from("AGENTS.md");
        let original = "Personal advice.\n";
        let delivered = Snapshot::file(
            format!(
                "<!-- WENLAN SHARED RULES {} {} -->\n## Project rules\n\nLast synced date.\n\nRevoked advice.\n<!-- END WENLAN SHARED RULES -->\n{original}",
                "a".repeat(64),
                "b".repeat(64),
            )
            .into_bytes(),
        );
        assert!(job.replace(&rule, &Snapshot::Absent, &delivered).unwrap());
        fs::write(
            &valid,
            serde_json::to_vec(&CheckoutRecord {
                format: CHECKOUT_FORMAT,
                checkout: Checkout {
                    files: BTreeMap::from([(rule.clone(), delivered)]),
                    ..Checkout::default()
                },
            })
            .unwrap(),
        )
        .unwrap();

        let mut entries = fs::read_dir(&checkouts)
            .unwrap()
            .collect::<std::io::Result<Vec<_>>>()
            .unwrap();
        entries.retain(|entry| entry.path() == legacy || entry.path() == valid);
        entries.sort_by_key(|entry| entry.file_name());
        assert_eq!(entries[0].path(), legacy, "the bad receipt must be first");
        assert_eq!(entries[1].path(), valid);
        let receipt = refresh_entries(
            &setup,
            &session,
            &settings,
            None,
            &[],
            true,
            entries.into_iter().map(Ok),
        )
        .unwrap();
        assert!(
            receipt
                .written
                .iter()
                .any(|path| path.ends_with("AGENTS.md")),
            "checkout write missing: {receipt:?}"
        );

        assert_eq!(fs::read(legacy).unwrap(), b"{}");
        let actual = fs::read_to_string(setup.data.join(directory).join(rule)).unwrap();
        assert!(!actual.contains("Revoked advice"), "{actual}");
        assert!(actual.contains(original), "{actual}");
    }

    #[test]
    fn rulesync_checkout_project_hand_edit_survives_sync_until_a_writer_rejects_it() {
        checkout_hand_edit(false, false, false);
    }

    #[test]
    fn rulesync_checkout_waiting_edit_survives_new_revision() {
        checkout_hand_edit(true, false, false);
    }

    #[test]
    fn rulesync_checkout_local_revocation_survives_a_render_failure() {
        checkout_hand_edit(false, true, false);
    }

    #[test]
    fn rulesync_checkout_refused_edit_can_be_declined_after_receipt() {
        checkout_hand_edit(false, false, true);
    }

    #[test]
    fn rulesync_checkout_one_revoked_project_is_removed_before_render_failure() {
        use crate::rulesync_layers::tests::{hosted_fixture, PROJECT, REMOTE};
        use std::{fs, process::Command};
        use wenlan_core::{shared_space, team};
        let scratch = tempfile::tempdir().unwrap();
        let setup = Setup {
            home: scratch.path().join("home"),
            data: scratch.path().join("data"),
            binary: PathBuf::from("unused"),
        };
        fs::create_dir(&setup.home).unwrap();
        fs::create_dir(&setup.data).unwrap();
        let (settings, owner, _) = hosted_fixture(&setup.data);
        let other = "Another project";
        let scope = shared_space::Scope::Space(other.into());
        let folder = team::new_hosted_folder(&settings);
        shared_space::join_at(&settings, &scope, &folder, "Owner").unwrap();
        shared_space::update_share(&settings, &scope, |share| share.hosted = true).unwrap();
        shared_space::ensure_space_file(
            &folder,
            &scope,
            other,
            &shared_space::SpaceMember {
                id: owner.id,
                name: owner.name,
            },
        )
        .unwrap();
        fs::write(setup.data.join("repo-wiki.json"), serde_json::to_vec(&serde_json::json!({"bindings": [
            {"space": PROJECT, "repo_root": scratch.path().join("repo"), "folder": "wiki", "remote_url": REMOTE},
            {"space": other, "repo_root": scratch.path().join("repo"), "folder": "other-wiki", "remote_url": REMOTE}
        ]})).unwrap()).unwrap();
        let proposal =
            rulesync_team::update_hosted_rules(&settings, other, "Other project instruction.\n")
                .unwrap();
        rulesync_team::decide_local_space_proposal(&settings, other, &proposal, true).unwrap();
        let repo = scratch.path().join("repo");
        fs::create_dir(&repo).unwrap();
        for args in [
            &["init", "--quiet"][..],
            &["remote", "add", "origin", REMOTE],
        ] {
            assert!(Command::new("git")
                .current_dir(&repo)
                .args(args)
                .status()
                .unwrap()
                .success());
        }
        fs::create_dir(setup.home.join(".codex")).unwrap();
        fs::write(
            setup.home.join(".codex/config.toml"),
            crate::rulesync_checkout_local::trust_config(&repo.display().to_string()),
        )
        .unwrap();
        let root = setup.data_files().unwrap();
        let session = rulesync_renderer::RenderSession::new(&setup.binary);
        let job = Job {
            setup: &setup,
            session: &session,
            settings: settings.clone(),
            root: &root,
            directory: Path::new("checkouts/example/rules"),
            can_restore_rejection: true,
        };
        let state_path = Path::new("checkouts/example.json");
        let master =
            PersonalMaster::new(&rulesync_personal::discover_adoption(&setup.home).unwrap());
        let state = Snapshot::Absent;
        let checkout = Checkout {
            repo: repo.clone(),
            ..Checkout::default()
        };
        job.run_using(
            state_path,
            state,
            checkout,
            Some(&master),
            |projects, _, _, _| {
                assert_eq!(projects.len(), 2);
                Ok(RenderedCheckout::default())
            },
        )
        .unwrap();
        let revoked = rulesync_team::load_project_layers(&settings)
            .unwrap()
            .into_iter()
            .find(|layer| layer.project.as_ref().is_some_and(|p| p.space == other))
            .unwrap();
        let revoked_key = rulesync_layers::key(&revoked).unwrap();
        let retained = rulesync_team::load_project_layers(&settings)
            .unwrap()
            .into_iter()
            .find(|layer| layer.project.as_ref().is_some_and(|p| p.space == PROJECT))
            .unwrap();
        let retained_key = rulesync_layers::key(&retained).unwrap();
        shared_space::leave_at(&settings, &scope).unwrap();
        let state = root.read(state_path, false).unwrap();
        let checkout = read_checkout(text(&state).unwrap().as_bytes()).unwrap();
        let result = job
            .run_using(state_path, state, checkout, Some(&master), |_, _, _, _| {
                anyhow::bail!("deliberate render failure")
            })
            .unwrap();
        assert!(result
            .blocked
            .iter()
            .any(|line| line.contains("deliberate render failure")));
        for name in ["CLAUDE.local.md", ".codex/config.toml"] {
            let content = fs::read_to_string(repo.join(name)).unwrap();
            assert!(!content.contains(&revoked_key));
            assert!(content.contains(&retained_key));
        }
    }

    fn checkout_hand_edit(new_revision: bool, fail_revoked_render: bool, refusal: bool) {
        use crate::rulesync_layers::tests::{
            hosted_fixture, render_fixture_section, APPROVED, PROJECT, REMOTE,
        };
        use std::{fs, process::Command};
        use wenlan_core::team::{self, CredentialKind};

        let scratch = tempfile::tempdir().unwrap();
        let setup = Setup {
            home: scratch.path().join("home"),
            data: scratch.path().join("data"),
            binary: scratch.path().join("unused-renderer"),
        };
        fs::create_dir(&setup.home).unwrap();
        fs::create_dir(&setup.data).unwrap();
        let (settings, _, writer) = hosted_fixture(&setup.data);
        let repo = scratch.path().join("checkout");
        fs::create_dir(&repo).unwrap();
        for args in [
            &["init", "--quiet"][..],
            &["remote", "add", "origin", REMOTE],
        ] {
            assert!(Command::new("git")
                .current_dir(&repo)
                .args(args)
                .status()
                .unwrap()
                .success());
        }
        fs::create_dir(setup.home.join(".codex")).unwrap();
        fs::write(
            setup.home.join(".codex/config.toml"),
            crate::rulesync_checkout_local::trust_config(&repo.display().to_string()),
        )
        .unwrap();
        let root = setup.data_files().unwrap();
        let session = rulesync_renderer::RenderSession::new(&setup.binary);
        let directory = Path::new("checkouts/example/rules");
        let job = |can_restore_rejection| Job {
            setup: &setup,
            session: &session,
            settings: settings.clone(),
            root: &root,
            directory,
            can_restore_rejection,
        };
        let state_path = Path::new("checkouts/example.json");
        let rule = Path::new("AGENTS.md");
        let master =
            PersonalMaster::new(&rulesync_personal::discover_adoption(&setup.home).unwrap());
        let fail_render = std::cell::Cell::new(false);
        let sync = |can_restore_rejection| {
            let state = root.read(state_path, false).unwrap();
            let checkout = if state == Snapshot::Absent {
                Checkout {
                    repo: repo.clone(),
                    ..Checkout::default()
                }
            } else {
                read_checkout(text(&state).unwrap().as_bytes()).unwrap()
            };
            job(can_restore_rejection)
                .run_using(
                    state_path,
                    state,
                    checkout,
                    Some(&master),
                    |projects, teams, _, _| {
                        if fail_render.get() {
                            anyhow::bail!("deliberate render failure");
                        }
                        assert_eq!(projects.len(), 1);
                        let body = format!(
                            "{}{}Personal advice.\n",
                            render_fixture_section(&projects[0]),
                            render_fixture_section(&teams[0])
                        );
                        Ok(RenderedCheckout {
                            rules: BTreeMap::from([(rule.into(), body.into_bytes())]),
                            ..RenderedCheckout::default()
                        })
                    },
                )
                .unwrap()
        };
        assert!(sync(true).blocked.is_empty());
        let file = setup.data.join(directory).join(rule);
        let approved = fs::read_to_string(&file).unwrap();
        let edited = approved.replacen(APPROVED, "Use the checkout checklist.\n", 1);
        assert_ne!(approved, edited);
        fs::write(&file, &edited).unwrap();
        let members = settings.with_file_name(team::TEAM_FILE);
        let pending = || {
            rulesync_team::pending_proposals(
                &members,
                &settings,
                PROJECT,
                &writer.id,
                CredentialKind::BrowserSession,
            )
            .unwrap()
        };
        for _ in 0..2 {
            let report = sync(true);
            assert_eq!(fs::read_to_string(&file).unwrap(), edited);
            assert!(report.proposals.is_empty());
            assert_eq!(report.blocked_codes, [Code::SharedRuleNeedsConfirm.into()]);
            assert!(
                report
                    .blocked
                    .iter()
                    .any(|line| line.contains("This edit is waiting for you to confirm it.")),
                "{report:?}"
            );
            assert!(pending().is_empty());
        }
        let answers = crate::rulesync_shared_answers::read(&setup.data).unwrap();
        assert_eq!(answers.waiting.len(), 1);
        let mut id = answers.waiting.keys().next().unwrap().clone();
        let mut final_approved = approved.clone();
        let mut final_edited = edited.clone();
        if new_revision {
            let proposal = rulesync_team::update_hosted_rules(
                &settings,
                PROJECT,
                "Use the approved checklist.\nAnother approved line.\n",
            )
            .unwrap();
            rulesync_team::decide_local_space_proposal(&settings, PROJECT, &proposal, true)
                .unwrap();
            let error = crate::rulesync_routes::decide_shared_edit_at(
                &setup,
                &settings,
                wenlan_types::rulesync::SharedEditDecisionRequest {
                    id: id.clone(),
                    send: true,
                },
            )
            .unwrap_err();
            assert!(
                matches!(error, crate::error::ServerError::Structured { status, body } if status == axum::http::StatusCode::CONFLICT && body["code"] == wenlan_types::rulesync::SHARED_EDIT_STALE_REVISION_CODE)
            );
            assert_eq!(fs::read_to_string(&file).unwrap(), edited);
            let background = sync(false);
            assert_eq!(fs::read_to_string(&file).unwrap(), edited);
            assert!(background
                .blocked_codes
                .contains(&Code::SharedRuleNeedsConfirm.into()));
            let report = sync(true);
            assert_eq!(report.blocked_codes, [Code::SharedRuleNeedsConfirm.into()]);
            let project = rulesync_team::load_project_layers(&settings)
                .unwrap()
                .remove(0);
            let team = rulesync_team::load_layers(&settings).unwrap().remove(0);
            final_approved = format!(
                "{}{}Personal advice.\n",
                render_fixture_section(&project),
                render_fixture_section(&team)
            );
            final_edited = final_approved.replacen(APPROVED, "Use the checkout checklist.\n", 1);
            assert_eq!(fs::read_to_string(&file).unwrap(), final_edited);
            let answers = crate::rulesync_shared_answers::read(&setup.data).unwrap();
            assert_eq!(answers.waiting.len(), 1);
            let next_id = answers.waiting.keys().next().unwrap().clone();
            assert_ne!(next_id, id);
            id = next_id;
        }
        crate::rulesync_routes::decide_shared_edit_at(
            &setup,
            &settings,
            wenlan_types::rulesync::SharedEditDecisionRequest {
                id: id.clone(),
                send: true,
            },
        )
        .unwrap();
        let [proposal]: [_; 1] = pending().try_into().unwrap();
        if refusal {
            let background = sync(false);
            assert!(background
                .blocked_codes
                .contains(&Code::SharedRulePending.into()));
            let malformed = setup
                .data
                .join("checkouts")
                .join(format!("{}.json", "0".repeat(64)));
            fs::write(&malformed, b"{}").unwrap();
            let queued = crate::rulesync_shared_answers::read(&setup.data)
                .unwrap()
                .sent_edits[&id]
                .proposal_path
                .clone();
            let state = root.read(state_path, false).unwrap();
            let receipt = read_checkout(text(&state).unwrap().as_bytes()).unwrap();
            assert!(receipt
                .shared_rule_proposals
                .values()
                .any(|items| !items.is_empty()));
            crate::rulesync_routes::reopen_edits_after_old_host_refusal_at(
                &setup,
                &settings,
                std::slice::from_ref(&proposal.id),
            )
            .unwrap();
            assert_eq!(fs::read(&malformed).unwrap(), b"{}");
            assert!(!queued.exists());
            assert!(crate::rulesync_shared_answers::read(&setup.data)
                .unwrap()
                .waiting
                .contains_key(&id));
            let state = root.read(state_path, false).unwrap();
            assert!(read_checkout(text(&state).unwrap().as_bytes())
                .unwrap()
                .shared_rule_proposals
                .is_empty());
            crate::rulesync_routes::decide_shared_edit_at(
                &setup,
                &settings,
                wenlan_types::rulesync::SharedEditDecisionRequest { id, send: false },
            )
            .unwrap();
            let report = sync(true);
            assert_eq!(
                fs::read_to_string(&file).unwrap(),
                final_approved,
                "{report:?}"
            );
            return;
        }
        assert_eq!(
            proposal.markdown,
            if new_revision {
                "Use the checkout checklist.\nAnother approved line.\n"
            } else {
                "Use the checkout checklist.\n"
            }
        );
        assert!(rulesync_team::local_proposals(&settings)
            .unwrap()
            .is_empty());
        rulesync_team::decide_proposal(
            &members,
            &settings,
            PROJECT,
            &writer.id,
            CredentialKind::BrowserSession,
            &proposal.id,
            false,
        )
        .unwrap();
        assert!(sync(false).notice_codes.is_empty());
        assert_eq!(fs::read_to_string(&file).unwrap(), final_edited);
        let report = sync(true);
        assert_eq!(fs::read_to_string(&file).unwrap(), final_approved);
        assert!(
            report.blocked.is_empty() && pending().is_empty(),
            "{report:?}"
        );
        assert_eq!(report.notice_codes, [Code::SharedRuleRejected.into()]);
        assert!(sync(true).notice_codes.is_empty());
        if fail_revoked_render {
            assert!(repo.join("CLAUDE.local.md").exists());
            assert!(repo.join(".codex/config.toml").exists());
            assert!(Command::new("git")
                .current_dir(&repo)
                .args([
                    "remote",
                    "set-url",
                    "origin",
                    "https://github.com/fixture/other.git"
                ])
                .status()
                .unwrap()
                .success());
            fail_render.set(true);
            let report = sync(true);
            assert!(report
                .blocked
                .iter()
                .any(|line| line.contains("deliberate render failure")));
            assert!(!repo.join("CLAUDE.local.md").exists());
            assert!(!repo.join(".codex/config.toml").exists());
        }
    }

    #[test]
    fn rulesync_checkout_splits_skill_output_and_blocks_colliding_sources() {
        let layer = |id: &str| TeamRuleLayer {
            id: id.to_owned(),
            name: id.to_owned(),
            markdown: String::new(),
            revision: String::new(),
            synced_at: String::new(),
            project: None,
            skills: vec![TeamProjectSkill {
                name: "checked-procedure".into(),
                files: vec![TeamProjectSkillFile {
                    path: "SKILL.md".into(),
                    text: "Expected skill".into(),
                }],
            }],
            proposal_decisions: BTreeMap::new(),
        };
        let skills = ProjectSkills::from_layers(&[layer("one")]).unwrap();
        let rule = PathBuf::from("AGENTS.md");
        let skill = PathBuf::from(".agents/skills/checked-procedure/SKILL.md");
        let mut stage = BTreeMap::from([
            (rule.clone(), b"rules".to_vec()),
            (skill.clone(), b"a rendered skill".to_vec()),
        ]);
        let mut rendered = RenderedCheckout::default();
        skills.take_rendered(&mut stage, &mut rendered).unwrap();
        assert!(stage.contains_key(&rule));
        assert_eq!(rendered.skills[&skill].bytes, b"a rendered skill");

        let skills = ProjectSkills::from_layers(&[layer("one"), layer("two")]).unwrap();
        assert!(skills.packages.is_empty());
        assert!(skills.conflicts.contains("checked-procedure"));
    }

    #[test]
    fn rulesync_checkout_preserves_edited_files_and_receipts_on_withdrawal() {
        let scratch = tempfile::tempdir().unwrap();
        let setup = Setup {
            home: scratch.path().join("home"),
            data: scratch.path().to_owned(),
            binary: scratch.path().join("unavailable-renderer"),
        };
        std::fs::create_dir(&setup.home).unwrap();
        let repo = scratch.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        let root = FileRoot::open(&setup.data).unwrap();
        let job = Job {
            setup: &setup,
            session: &rulesync_renderer::RenderSession::new(&setup.binary),
            settings: scratch.path().join("settings.toml"),
            root: &root,
            directory: Path::new("checkouts/example/rules"),
            can_restore_rejection: true,
        };
        let rule_path = PathBuf::from("AGENTS.md");
        let approved_rule = format!(
            "<!-- WENLAN SHARED RULES {} {} -->\n## Project rules\n\nLast synced date.\n\nUse the project checklist.\n<!-- END WENLAN SHARED RULES -->\nOriginal advice.\n",
            "a".repeat(64),
            "b".repeat(64),
        );
        let edited_rule = approved_rule.replace("<!-- END", "<!-- EDITED END")
            + "Personal note added after delivery.\n";
        let skill_path = PathBuf::from(".agents/skills/project-only/SKILL.md");
        let approved_skill = Snapshot::file(b"approved project-only skill\n".to_vec());
        let edited_skill =
            Snapshot::file(b"approved project-only skill\nPersonal edit.\n".to_vec());
        let approved_rule = Snapshot::file(approved_rule.into_bytes());
        let edited_rule = Snapshot::file(edited_rule.into_bytes());
        let edited = [(&rule_path, &edited_rule), (&skill_path, &edited_skill)];
        for (path, snapshot) in edited {
            assert!(job.replace(path, &Snapshot::Absent, snapshot).unwrap());
        }
        let checkout = Checkout {
            repo,
            files: BTreeMap::from([(rule_path.clone(), approved_rule.clone())]),
            skills: BTreeMap::from([(
                skill_path.clone(),
                SkillDelivery {
                    source: "lost-access".into(),
                    digest: fingerprint(&approved_skill).unwrap(),
                },
            )]),
            ..Checkout::default()
        };
        let state_path = Path::new("checkouts/example.json");
        let state = job.save(state_path, &Snapshot::Absent, &checkout).unwrap();
        let master =
            PersonalMaster::new(&rulesync_personal::discover_adoption(&setup.home).unwrap());
        let report = job
            .run(state_path, state, checkout, Some(&master), &[])
            .unwrap();

        let changed: Vec<_> = edited
            .into_iter()
            .filter(|(path, snapshot)| {
                &root.read(&job.directory.join(path), false).unwrap() != *snapshot
            })
            .map(|(path, _)| path)
            .collect();
        assert!(
            changed.is_empty(),
            "withdrawal overwrote member edits at {changed:?}"
        );
        for (path, _) in edited {
            assert!(report
                .blocked
                .iter()
                .any(|item| item.starts_with(path.to_str().unwrap())));
        }
        let record = root.read(state_path, false).unwrap();
        let saved = read_checkout(text(&record).unwrap().as_bytes()).unwrap();
        assert_eq!(saved.files[&rule_path], approved_rule);
        assert_eq!(
            saved.skills[&skill_path].digest,
            fingerprint(&approved_skill).unwrap()
        );
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn rulesync_checkout_recovers_an_interrupted_rule_withdrawal() {
        use std::io::Write;

        let child_data =
            std::env::var_os("WENLAN_TEST_CHECKOUT_WITHDRAWAL_DATA").map(PathBuf::from);
        let scratch = child_data.is_none().then(|| tempfile::tempdir().unwrap());
        let data = child_data
            .as_deref()
            .or_else(|| scratch.as_ref().map(tempfile::TempDir::path))
            .unwrap();
        let setup = Setup {
            home: data.join("home"),
            data: data.to_owned(),
            binary: data.join("unavailable-renderer"),
        };
        let root = FileRoot::open(data).unwrap();
        let job = Job {
            setup: &setup,
            session: &rulesync_renderer::RenderSession::new(&setup.binary),
            settings: data.join("settings.toml"),
            root: &root,
            directory: Path::new("checkouts/example/rules"),
            can_restore_rejection: true,
        };
        let path = PathBuf::from("AGENTS.md");
        let state_path = Path::new("checkouts/example.json");
        let personal = "Original advice.\n";
        let personal_edit = "Personal note added after delivery.\n";
        let held = if child_data.is_none() {
            let approved = Snapshot::file(
                format!(
                    "<!-- WENLAN SHARED RULES {} {} -->\n## Project rules\n\nLast synced date.\n\nUse the project checklist.\n<!-- END WENLAN SHARED RULES -->\n{personal}",
                    "a".repeat(64),
                    "b".repeat(64),
                )
                .into_bytes(),
            );
            let edited = with_bytes(
                &approved,
                (text(&approved).unwrap().to_owned() + personal_edit).into_bytes(),
            );
            let checkout = Checkout {
                files: BTreeMap::from([(path.clone(), approved)]),
                ..Checkout::default()
            };
            job.save(state_path, &Snapshot::Absent, &checkout).unwrap();
            assert!(job.replace(&path, &Snapshot::Absent, &edited).unwrap());
            let held = std::fs::OpenOptions::new()
                .append(true)
                .open(data.join(job.directory).join(&path))
                .unwrap();
            let child = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "rulesync_checkout::tests::rulesync_checkout_recovers_an_interrupted_rule_withdrawal",
                ])
                .env("WENLAN_TEST_CHECKOUT_WITHDRAWAL_DATA", data)
                .env("WENLAN_TEST_FILE_APPLY_CRASH", "displaced:AGENTS.md")
                .output()
                .unwrap();
            assert_eq!(
                child.status.code(),
                Some(76),
                "withdrawal did not reach the crash point: {}",
                String::from_utf8_lossy(&child.stdout),
            );
            Some((held, edited))
        } else {
            None
        };

        let state = root.read(state_path, false).unwrap();
        let checkout = read_checkout(text(&state).unwrap().as_bytes()).unwrap();
        let report = job.run(state_path, state, checkout, None, &[]).unwrap();
        assert!(report.blocked.is_empty(), "{report:?}");
        let record = root.read(state_path, false).unwrap();
        let saved = read_checkout(text(&record).unwrap().as_bytes()).unwrap();
        assert_eq!(text(&saved.files[&path]).unwrap(), personal);
        let current = root.read(&job.directory.join(&path), false).unwrap();
        assert_eq!(
            text(&current).unwrap(),
            format!("{personal}{personal_edit}")
        );
        assert!(
            std::fs::read_dir(data.join(job.directory))
                .unwrap()
                .all(|entry| !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".wenlan-rules-stage-")),
            "recovery left the checkout's interrupted stage in the output directory"
        );
        if let Some((mut held, edited)) = held {
            let late_edit = "Personal note saved through the original handle.\n";
            held.write_all(late_edit.as_bytes()).unwrap();
            let mut batches = std::fs::read_dir(data.join("personal-displaced")).unwrap();
            let batch = batches.next().unwrap().unwrap().path();
            assert!(batches.next().is_none());
            let receipt: BTreeMap<String, PathBuf> =
                serde_json::from_slice(&std::fs::read(batch.join("paths.json")).unwrap()).unwrap();
            let (name, saved_path) = receipt.into_iter().next().unwrap();
            assert_eq!(saved_path, job.directory.join(&path));
            assert_eq!(
                std::fs::read_to_string(batch.join(name)).unwrap(),
                format!("{}{late_edit}", text(&edited).unwrap())
            );
        }
    }

    #[test]
    fn rulesync_checkout_recovers_then_prunes_revoked_skill_before_a_render_failure() {
        check_interrupted_revoked_skill_delivery(false);
    }

    #[test]
    fn rulesync_checkout_retains_receipt_for_an_edited_interrupted_delivery() {
        check_interrupted_revoked_skill_delivery(true);
    }

    fn check_interrupted_revoked_skill_delivery(edited_after_crash: bool) {
        let scratch = tempfile::tempdir().unwrap();
        let data = scratch.path().join("data");
        let source = scratch.path().join("repo");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir(&source).unwrap();
        let setup = Setup {
            home: scratch.path().join("home"),
            data,
            binary: scratch.path().join("unavailable-renderer"),
        };
        let root = FileRoot::open(&setup.data).unwrap();
        let job = Job {
            setup: &setup,
            session: &rulesync_renderer::RenderSession::new(&setup.binary),
            settings: scratch.path().join("settings.toml"),
            root: &root,
            directory: Path::new("checkouts/example/rules"),
            can_restore_rejection: true,
        };
        let path = PathBuf::from(".agents/skills/project-only/SKILL.md");
        let approved = Snapshot::file(b"approved project-only skill".to_vec());
        let digest = fingerprint(&approved).unwrap();
        let checkout = Checkout {
            repo: source,
            pending: Some(PendingDelivery {
                path: path.clone(),
                digest: digest.clone(),
                skill_source: Some("lost-access".to_owned()),
                stage: Some(CheckoutStage {
                    id: uuid::Uuid::new_v4().simple().to_string(),
                    displaced: None,
                    seen: Snapshot::Absent,
                    desired: approved.clone(),
                }),
                rules_baseline: None,
            }),
            ..Checkout::default()
        };
        let state_path = Path::new("checkouts/example.json");
        let state = job.save(state_path, &Snapshot::Absent, &checkout).unwrap();
        assert!(job.replace(&path, &Snapshot::Absent, &approved).unwrap());
        let destination = setup.data.join(job.directory).join(&path);
        let edited = b"approved project-only skill with a local edit";
        if edited_after_crash {
            std::fs::write(&destination, edited).unwrap();
        }
        std::fs::create_dir(&setup.home).unwrap();
        let master =
            PersonalMaster::new(&rulesync_personal::discover_adoption(&setup.home).unwrap());
        let report = job
            .run(state_path, state, checkout, Some(&master), &[])
            .unwrap();
        assert!(!report.blocked.is_empty());
        if edited_after_crash {
            assert_eq!(std::fs::read(&destination).unwrap().as_slice(), edited);
            let record = root.read(state_path, false).unwrap();
            let recorded = text(&record).unwrap();
            assert!(
                recorded.contains(&digest),
                "recovery lost the only receipt for the installed skill"
            );
            assert!(report
                .blocked
                .iter()
                .any(|warning| warning.starts_with(path.to_str().unwrap())));
            let saved = read_checkout(recorded.as_bytes()).unwrap();
            std::fs::write(&destination, text(&approved).unwrap()).unwrap();
            job.run(state_path, record, saved, Some(&master), &[])
                .unwrap();
        }
        assert_eq!(
            root.read(&job.directory.join(path), false).unwrap(),
            Snapshot::Absent
        );
        let record = root.read(state_path, false).unwrap();
        assert!(!text(&record).unwrap().contains(&digest));
        let saved = read_checkout(text(&record).unwrap().as_bytes()).unwrap();
        assert!(saved.skills.is_empty());
        assert!(saved.pending.is_none());
    }
}
