// SPDX-License-Identifier: Apache-2.0
//! Save and review authored skill bundles before the personal applier delivers them.

use std::collections::{BTreeMap, BTreeSet};
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, ensure, Context, Result};
use axum::{http::HeaderMap, Json};
use cap_fs_ext::DirExt as _;
use cap_std::fs::Dir;
use serde::{Deserialize, Serialize};
use wenlan_core::rulesync_adopt::{
    skip_skill_asset, MAX_ADOPTED_SKILL_BYTES, MAX_ADOPTED_SKILL_DEPTH, MAX_ADOPTED_SKILL_ENTRIES,
};
use wenlan_core::rulesync_personal::{self, fingerprint};
use wenlan_core::rulesync_skill::{self as skills, SkillFiles};
use wenlan_core::skill_registry;
use wenlan_types::rulesync::RulesSyncReport;
use wenlan_types::rulesync_skill::{
    SaveSkillRequest, SaveSkillResult, SavedSkillScope, SavedSkillTool, SkillProposal,
    SkillProposalOrigin, SkillProposalsResponse, SkillReviewAction, SkillReviewRequest,
};
use wenlan_types::skill_registry::{InstalledSkill, InstalledSkillRow, Scope, Verdict};

use crate::error::ServerError;
use crate::route_registry::{get, post, TrackedRouter};
use crate::rulesync_apply::{ApplyOutcome, FileRoot, Snapshot};
use crate::rulesync_routes::{require_local_person, Setup};
use crate::state::SharedState;

const SKILL_STATE: &str = "skills.json";
const MAX_AGENT_PROPOSALS: usize = 40;

pub(crate) fn register(router: TrackedRouter<SharedState>) -> TrackedRouter<SharedState> {
    router
        .route("/api/skills/registry/save", post(save_skill))
        .route("/api/skills/registry/proposals", get(proposals))
        .route("/api/skills/registry/review", post(review_skill))
}

fn require_personal_scope(headers: &HeaderMap) -> Result<(), ServerError> {
    if !rulesync_personal::enabled() {
        return Err(ServerError::ValidationError(
            "WENLAN_RULESYNC is off".into(),
        ));
    }
    if !matches!(
        crate::security::bind_scope_from_env(),
        crate::security::BindScope::Unset | crate::security::BindScope::Loopback
    ) {
        return Err(ServerError::ValidationError(
            "change personal skills through the local daemon".into(),
        ));
    }
    if [
        crate::space_header::HEADER_NAME,
        crate::space_header::HEADER_NAME_LEGACY,
    ]
    .into_iter()
    .any(|name| headers.contains_key(name))
    {
        return Err(ServerError::ValidationError(
            "a Space-bound connection cannot change personal skills".into(),
        ));
    }
    Ok(())
}

enum Submitter {
    Person,
    Agent(String),
}

fn submitter(headers: &HeaderMap) -> Result<Submitter, ServerError> {
    if crate::memory_routes::caller_is_person(headers) {
        return Ok(Submitter::Person);
    }
    headers
        .get("x-agent-name")
        .and_then(|name| name.to_str().ok())
        .map(str::trim)
        .filter(|name| {
            !name.is_empty() && name.len() <= 128 && skill_registry::printable(name) == *name
        })
        .map(|name| Submitter::Agent(name.into()))
        .ok_or_else(|| {
            ServerError::ValidationError(
                "use a named AI tool or the person's terminal to save a skill".into(),
            )
        })
}

async fn save_skill(
    headers: HeaderMap,
    Json(request): Json<SaveSkillRequest>,
) -> Result<Json<SaveSkillResult>, ServerError> {
    require_personal_scope(&headers)?;
    let caller = submitter(&headers)?;
    work_on_disk(move || {
        let files = skills::uploaded_files(&request).map_err(validation)?;
        let setup = Setup::system().map_err(validation)?;
        let lock = setup.lock().map_err(failed_store)?;
        let mut store = SkillStore::open(&setup.data).map_err(failed_store)?;
        let result = match caller {
            Submitter::Person => {
                refuse_installed_name(&request.name).map_err(conflict)?;
                store.state.keep(&request.name, request.scope, files);
                store.save().map_err(failed_store)?;
                drop(lock);
                saved(request.name)
            }
            Submitter::Agent(agent) => {
                let draft = Draft {
                    base: store.state.revision(&request.name).map_err(failed_store)?,
                    name: request.name,
                    scope: request.scope,
                    source: Candidate::Agent { agent, files },
                };
                let proposal = store.state.propose_agent(draft).map_err(conflict)?;
                store.save().map_err(failed_store)?;
                SaveSkillResult::Proposed { proposal }
            }
        };
        Ok(result)
    })
    .await
}

async fn proposals(headers: HeaderMap) -> Result<Json<SkillProposalsResponse>, ServerError> {
    require_personal_scope(&headers)?;
    require_local_person(&headers)?;
    work_on_disk(move || {
        let setup = Setup::system().map_err(validation)?;
        if !setup
            .data
            .try_exists()
            .map_err(|error| failed_store(error.into()))?
        {
            return Ok(SkillProposalsResponse {
                proposals: Vec::new(),
            });
        }
        let _lock = setup.lock().map_err(failed_store)?;
        let store = SkillStore::open(&setup.data).map_err(failed_store)?;
        let proposals = store
            .state
            .proposals
            .iter()
            .map(|(id, draft)| draft.preview(id.clone()))
            .collect::<Result<_>>()
            .map_err(failed_store)?;
        Ok(SkillProposalsResponse { proposals })
    })
    .await
}

async fn review_skill(
    headers: HeaderMap,
    Json(request): Json<SkillReviewRequest>,
) -> Result<Json<SaveSkillResult>, ServerError> {
    require_personal_scope(&headers)?;
    require_local_person(&headers)?;
    work_on_disk(move || {
        let setup = Setup::system().map_err(validation)?;
        let lock = setup.lock().map_err(failed_store)?;
        let mut store = SkillStore::open(&setup.data).map_err(failed_store)?;
        let name = store
            .state
            .review(
                &setup.home,
                request.id,
                request.action,
                request.scope,
                refuse_installed_name,
            )
            .map_err(conflict)?;
        store.save().map_err(failed_store)?;
        drop(lock);
        Ok(match request.action {
            SkillReviewAction::Accept => saved(name),
            SkillReviewAction::Dismiss => SaveSkillResult::Dismissed { name },
        })
    })
    .await
}

async fn work_on_disk<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, ServerError> + Send + 'static,
) -> Result<Json<T>, ServerError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| failed_store(error.into()))?
        .map(Json)
}

fn refuse_installed_name(name: &str) -> Result<()> {
    super::with_sources_lock(|| {
        ensure!(
            skill_registry::read_record(&skill_registry::default_wenlan_root(), name).is_none(),
            "this name is already held by the registry. Choose a different name"
        );
        Ok(())
    })?
}

fn saved(name: String) -> SaveSkillResult {
    let delivery = match super::invoke_delivery() {
        Ok(report) => report,
        Err(error) => RulesSyncReport {
            written: Vec::new(),
            blocked: Vec::new(),
            proposals: Vec::new(),
            message: format!(
                "The skill is saved. Run `wenlan rules sync` to retry delivery. {error:#}"
            ),
        },
    };
    SaveSkillResult::Saved {
        name,
        delivery: Some(delivery),
    }
}

fn validation(error: anyhow::Error) -> ServerError {
    ServerError::ValidationError(format!("{error:#}"))
}

fn conflict(error: anyhow::Error) -> ServerError {
    ServerError::Conflict(format!("{error:#}"))
}

fn failed_store(error: anyhow::Error) -> ServerError {
    tracing::warn!(%error, "[skills] personal skill store unavailable");
    ServerError::Internal("the personal skill store is unavailable".into())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ManagedSkill {
    pub tool: SavedSkillTool,
    /// Package-relative leaf snapshots, including each file's mode.
    pub files: BTreeMap<PathBuf, Snapshot>,
}

impl ManagedSkill {
    fn files(&self) -> SkillFiles {
        self.files
            .iter()
            .filter_map(|(path, snapshot)| match snapshot {
                Snapshot::File { bytes, .. } => Some((path.clone(), bytes.clone())),
                _ => None,
            })
            .collect()
    }
}

#[derive(Debug, Serialize, Deserialize)]
enum Candidate {
    Agent {
        agent: String,
        files: SkillFiles,
    },
    Tool {
        folder: PathBuf,
        snapshot: ManagedSkill,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct Draft {
    name: String,
    scope: SavedSkillScope,
    base: String,
    source: Candidate,
}

impl Draft {
    fn preview(&self, id: String) -> Result<SkillProposal> {
        let (origin, files) = match &self.source {
            Candidate::Agent { agent, files } => (
                SkillProposalOrigin::Agent {
                    agent: agent.clone(),
                },
                skills::preview(&self.name, files)?,
            ),
            Candidate::Tool { snapshot, .. } => (
                SkillProposalOrigin::Tool {
                    tool: snapshot.tool,
                },
                skills::preview(&self.name, &snapshot.files())?,
            ),
        };
        Ok(SkillProposal {
            id,
            name: self.name.clone(),
            scope: self.scope,
            origin,
            files,
        })
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Variants {
    shared: Option<SkillFiles>,
    tools: BTreeMap<SavedSkillTool, SkillFiles>,
    installed_at: i64,
    updated_at: i64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct SkillState {
    proposals: BTreeMap<String, Draft>,
    kept: BTreeMap<String, Variants>,
    ignored: BTreeMap<PathBuf, String>,
    approved_local: BTreeMap<PathBuf, ManagedSkill>,
}

impl SkillState {
    fn revision(&self, name: &str) -> Result<String> {
        fingerprint(
            &self
                .kept
                .get(name)
                .map(|skill| (&skill.shared, &skill.tools)),
        )
    }

    fn keep(&mut self, name: &str, scope: SavedSkillScope, files: SkillFiles) {
        let now = chrono::Utc::now().timestamp();
        let variants = self
            .kept
            .entry(name.to_owned())
            .or_insert_with(|| Variants {
                installed_at: now,
                ..Default::default()
            });
        variants.updated_at = now;
        match scope {
            SavedSkillScope::Shared => {
                variants.shared = Some(files);
                variants.tools.clear();
            }
            SavedSkillScope::Tool { tool } => {
                variants.tools.insert(tool, files);
            }
        }
    }

    fn propose_agent(&mut self, draft: Draft) -> Result<SkillProposal> {
        let id = fingerprint(&draft)?;
        if let Some(current) = self.proposals.get(&id) {
            return current.preview(id);
        }
        ensure!(
            self.proposals
                .values()
                .filter(|draft| matches!(draft.source, Candidate::Agent { .. }))
                .count()
                < MAX_AGENT_PROPOSALS,
            "review some pending skill proposals before adding more"
        );
        let proposal = draft.preview(id.clone())?;
        self.proposals.insert(id, draft);
        Ok(proposal)
    }

    fn review(
        &mut self,
        home: &Path,
        id: String,
        action: SkillReviewAction,
        scope: Option<SavedSkillScope>,
        check_name: impl FnOnce(&str) -> Result<()>,
    ) -> Result<String> {
        let draft = self
            .proposals
            .get(&id)
            .context("this skill proposal is no longer current")?;
        let name = draft.name.clone();
        if action == SkillReviewAction::Dismiss {
            if let Candidate::Tool { folder, snapshot } = &draft.source {
                self.ignored.insert(folder.clone(), fingerprint(snapshot)?);
            }
            self.proposals.remove(&id);
            return Ok(name);
        }
        ensure!(
            draft.base == self.revision(&name)?,
            "the saved skill changed after this proposal. Review a current proposal"
        );
        let scope = scope.unwrap_or(draft.scope);
        let files = match &draft.source {
            Candidate::Agent { files, .. } => {
                check_name(&name)?;
                files.clone()
            }
            Candidate::Tool { folder, snapshot } => {
                ensure!(
                    matches!(scope, SavedSkillScope::Shared) || scope == draft.scope,
                    "accept for the tool that changed the skill or share across tools"
                );
                let scanning_root = SkillHome::open(home)?;
                ensure!(scanning_root.package(folder, snapshot.tool)?.as_ref() == Some(snapshot),
                    "the tool edited this skill again. Run `wenlan rules sync` to review the current files");
                self.approved_local.insert(folder.clone(), snapshot.clone());
                self.ignored.remove(folder);
                snapshot.files()
            }
        };
        skills::preview(&name, &files)?;
        self.keep(&name, scope, files);
        self.proposals.remove(&id);
        Ok(name)
    }

    fn sources(&self, tool: SavedSkillTool) -> BTreeMap<String, SkillFiles> {
        self.kept
            .iter()
            .filter_map(|(name, skill)| {
                skill
                    .tools
                    .get(&tool)
                    .or(skill.shared.as_ref())
                    .map(|files| (name.clone(), files.clone()))
            })
            .collect()
    }

    fn rows(&self) -> Result<Vec<InstalledSkillRow>> {
        self.kept
            .iter()
            .map(|(name, variants)| {
                let description = variants
                    .shared
                    .as_ref()
                    .or_else(|| variants.tools.values().next())
                    .and_then(|files| files.get(Path::new("SKILL.md")))
                    .and_then(|bytes| std::str::from_utf8(bytes).ok())
                    .and_then(|markup| skill_registry::front_matter_field(markup, "description"));
                Ok(InstalledSkillRow {
                    skill: InstalledSkill {
                        name: name.clone(),
                        source: skill_registry::LOCAL_SOURCE.into(),
                        skill_id: name.clone(),
                        repo_path: String::new(),
                        commit: String::new(),
                        hash: self.revision(name)?,
                        verdict: Verdict::Warn,
                        scope: Scope::All,
                        description,
                        installed_at: variants.installed_at,
                        updated_at: variants.updated_at,
                    },
                    additions: String::new(),
                })
            })
            .collect()
    }

    fn remove_kept(&mut self, name: &str) -> Result<bool> {
        if self.kept.remove(name).is_none() {
            return Ok(false);
        }
        let origins: Vec<_> = self
            .approved_local
            .keys()
            .filter(|path| path.file_name().is_some_and(|leaf| leaf == name))
            .cloned()
            .collect();
        for folder in origins {
            if let Some(snapshot) = self.approved_local.remove(&folder) {
                self.ignored.insert(folder, fingerprint(&snapshot)?);
            }
        }
        self.proposals.retain(|_, draft| draft.name != name);
        Ok(true)
    }
}

struct SkillStore {
    root: FileRoot,
    seen: Snapshot,
    state: SkillState,
}

impl SkillStore {
    fn open(data: &Path) -> Result<Self> {
        let root = FileRoot::open(data)?;
        let seen = root.read(Path::new(SKILL_STATE), false)?;
        let state = match &seen {
            Snapshot::File { bytes, .. } => serde_json::from_slice(bytes)?,
            Snapshot::Absent => SkillState::default(),
            _ => bail!("unexpected personal skill store file type"),
        };
        Ok(Self { root, seen, state })
    }

    fn save(self) -> Result<()> {
        let next = Snapshot::file(serde_json::to_vec(&self.state)?);
        if self.seen == next {
            return Ok(());
        }
        ensure!(
            self.root
                .apply(Path::new(SKILL_STATE), &self.seen, &next, false)?
                == ApplyOutcome::Applied,
            "personal skill state changed during the operation"
        );
        Ok(())
    }
}

/// Read under the same setup lock as the renderer and proposal operations.
pub(crate) fn approved_skill_sources(
    data: &Path,
    target: &str,
) -> Result<BTreeMap<String, SkillFiles>> {
    let Some(target) = rulesync_personal::personal_target(target) else {
        return Ok(BTreeMap::new());
    };
    Ok(SkillStore::open(data)?.state.sources(target.tool))
}

pub(crate) fn approved_local_folders(data: &Path) -> Result<BTreeMap<PathBuf, ManagedSkill>> {
    let mut folders = SkillStore::open(data)?.state.approved_local;
    folders.retain(|path, _| !rulesync_personal::is_harness_path(path));
    Ok(folders)
}

#[cfg(test)]
pub(crate) fn accept_only_tool_proposal(home: &Path, data: &Path) -> Result<SkillProposal> {
    let mut store = SkillStore::open(data)?;
    ensure!(
        store.state.proposals.len() == 1,
        "expected one tool skill proposal"
    );
    let (id, draft) = store
        .state
        .proposals
        .first_key_value()
        .context("missing tool proposal")?;
    ensure!(
        matches!(draft.source, Candidate::Tool { .. }),
        "expected a tool edit"
    );
    let proposal = draft.preview(id.clone())?;
    store.state.review(
        home,
        proposal.id.clone(),
        SkillReviewAction::Accept,
        Some(SavedSkillScope::Shared),
        |_| Ok(()),
    )?;
    store.save()?;
    Ok(proposal)
}

pub(crate) fn authored_rows() -> Result<Vec<InstalledSkillRow>> {
    let setup = Setup::system()?;
    if !setup.data.try_exists()? {
        return Ok(Vec::new());
    }
    SkillStore::open(&setup.data)?.state.rows()
}

/// Called only after the registry's person check. The applier reconciles
/// the removed source and preserves every destination's recorded original.
pub(crate) fn remove_saved_source(name: &str) -> Result<bool> {
    let setup = Setup::system()?;
    if !setup.data.try_exists()? {
        return Ok(false);
    }
    let _lock = setup.lock()?;
    let mut store = SkillStore::open(&setup.data)?;
    if !store.state.remove_kept(name)? {
        return Ok(false);
    }
    store.save()?;
    Ok(true)
}

struct SkillHome {
    dir: Dir,
    root: FileRoot,
}

impl SkillHome {
    fn open(home: &Path) -> Result<Self> {
        let root = FileRoot::open_home(home)?;
        let parent = Dir::open_ambient_dir(
            home.parent().context("tool home has no parent")?,
            cap_std::ambient_authority(),
        )?;
        let dir = parent.open_dir_nofollow(Path::new(
            home.file_name().context("tool home has no name")?,
        ))?;
        Ok(Self { dir, root })
    }

    fn below(&self, path: &Path) -> Result<Option<Dir>> {
        let mut dir = self.dir.try_clone()?;
        for component in path.components() {
            let Component::Normal(name) = component else {
                bail!("invalid tool skill path");
            };
            dir = match dir.open_dir_nofollow(Path::new(name)) {
                Ok(dir) => dir,
                Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error.into()),
            };
        }
        Ok(Some(dir))
    }

    fn candidates(&self) -> BTreeMap<PathBuf, SavedSkillTool> {
        let mut paths = BTreeMap::new();
        for target in rulesync_personal::PERSONAL_TARGETS {
            let path = Path::new(target.skills.destination);
            let Ok(Some(dir)) = self.below(path) else {
                continue;
            };
            let Ok(entries) = dir.entries() else {
                continue;
            };
            for entry in entries.flatten() {
                let name = entry.file_name();
                if name.to_str().is_some_and(skills::skill_name)
                    && entry.file_type().is_ok_and(|kind| kind.is_dir())
                {
                    paths.insert(path.join(name), target.tool);
                }
            }
        }
        paths
    }

    fn package(&self, folder: &Path, tool: SavedSkillTool) -> Result<Option<ManagedSkill>> {
        let Some(dir) = self.below(folder)? else {
            return Ok(None);
        };
        match dir.symlink_metadata("SKILL.md") {
            Ok(metadata) if metadata.is_file() => {}
            Ok(_) => bail!("skill entry is not a regular file"),
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        }
        let mut package = PackageRead {
            files: BTreeMap::new(),
            bytes: 0,
            entries: 0,
        };
        self.read_files(&dir, folder, Path::new(""), &mut package)?;
        ensure!(
            package.files.contains_key(Path::new("SKILL.md")),
            "the skill changed during discovery"
        );
        Ok(Some(ManagedSkill {
            tool,
            files: package.files,
        }))
    }

    fn read_files(
        &self,
        dir: &Dir,
        folder: &Path,
        prefix: &Path,
        result: &mut PackageRead,
    ) -> Result<()> {
        ensure!(
            prefix.components().count() <= MAX_ADOPTED_SKILL_DEPTH,
            "skill assets are too deeply nested"
        );
        for entry in dir.entries()? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_str().context("a skill asset name is not UTF-8")?;
            if skip_skill_asset(name) {
                continue;
            }
            result.entries += 1;
            ensure!(
                result.entries <= MAX_ADOPTED_SKILL_ENTRIES,
                "skill package exceeds the {MAX_ADOPTED_SKILL_ENTRIES} files and directories allowed for import"
            );
            let path = prefix.join(name);
            let kind = entry.file_type()?;
            if kind.is_dir() {
                self.read_files(&dir.open_dir_nofollow(name)?, folder, &path, result)?;
                continue;
            }
            ensure!(kind.is_file(), "skill package has an unsafe asset type");
            ensure!(
                result.bytes + entry.metadata()?.len() <= MAX_ADOPTED_SKILL_BYTES,
                "skill package exceeds the {} MiB import limit",
                MAX_ADOPTED_SKILL_BYTES / (1024 * 1024)
            );
            let snapshot = self.root.read(&folder.join(&path), false)?;
            let Snapshot::File { bytes, .. } = &snapshot else {
                bail!("a skill asset changed during discovery");
            };
            result.bytes += bytes.len() as u64;
            ensure!(
                result.bytes <= MAX_ADOPTED_SKILL_BYTES,
                "skill package exceeds the {} MiB import limit",
                MAX_ADOPTED_SKILL_BYTES / (1024 * 1024)
            );
            result.files.insert(path, snapshot);
        }
        Ok(())
    }
}

struct PackageRead {
    files: BTreeMap<PathBuf, Snapshot>,
    bytes: u64,
    entries: usize,
}

/// Capture unapproved skill bytes. Returned package roots must remain blocked
/// from delivery until the person accepts the observed bytes.
/// The caller holds the setup lock through both discovery and delivery.
pub(crate) fn collect_tool_proposals(
    home: &Path,
    data: &Path,
    managed: &BTreeMap<PathBuf, ManagedSkill>,
    unmanaged: &BTreeSet<PathBuf>,
) -> Result<BTreeMap<PathBuf, String>> {
    let home = SkillHome::open(home)?;
    let mut candidates = home.candidates();
    candidates.extend(
        managed
            .iter()
            .map(|(folder, skill)| (folder.clone(), skill.tool)),
    );
    let mut store = SkillStore::open(data)?;
    // Replace the tool drafts with the currently observed versions. Agent
    // drafts have their own immutable payload and stay until disposition.
    store
        .state
        .proposals
        .retain(|_, draft| matches!(draft.source, Candidate::Agent { .. }));
    let mut blocked = BTreeMap::new();
    for (folder, tool) in candidates {
        if folder.ancestors().any(|path| unmanaged.contains(path)) {
            continue;
        }
        let baseline = managed.get(&folder);
        let current = match home.package(&folder, tool) {
            Ok(Some(snapshot)) => snapshot,
            problem => {
                if baseline.is_some_and(|skill| skill.files.contains_key(Path::new("SKILL.md"))) {
                    let reason = match problem {
                        Err(error) => format!(
                            "Cannot read this managed skill ({error:#}). Fix the folder and run `wenlan rules sync`."
                        ),
                        _ => "The managed SKILL.md is missing. Put it back and run `wenlan rules sync`.".into(),
                    };
                    blocked.insert(folder, reason);
                }
                continue;
            }
        };
        let files = current.files();
        if baseline.is_some_and(|skill| skill.files() == files)
            || store.state.approved_local.get(&folder) == Some(&current)
            || (baseline.is_none() && files.contains_key(Path::new(skill_registry::MANAGED_MARKER)))
        {
            continue;
        }
        if store.state.ignored.get(&folder) == Some(&fingerprint(&current)?) {
            blocked.insert(
                folder,
                "You dismissed these skill changes. Edit the skill to propose a new version."
                    .into(),
            );
            continue;
        }
        let Some(name) = folder
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| skills::skill_name(name))
        else {
            blocked.insert(folder, "Rename the skill folder to use lowercase letters, digits, and hyphens, then run `wenlan rules sync`.".into());
            continue;
        };
        if let Err(error) = skills::preview(name, &files) {
            blocked.insert(folder, format!("Cannot propose this skill ({error:#}). Fix the files and run `wenlan rules sync`."));
            continue;
        }
        if store.state.proposals.len() >= MAX_AGENT_PROPOSALS * 2 {
            blocked.insert(folder, "The skill review queue is full. Review some entries with `wenlan skills proposals`, then sync again.".into());
            continue;
        }
        blocked.insert(
            folder.clone(),
            "Skill changes need your approval. Run `wenlan skills proposals` to review them."
                .into(),
        );
        let draft = Draft {
            name: name.into(),
            scope: SavedSkillScope::Tool { tool },
            base: store.state.revision(name)?,
            source: Candidate::Tool {
                folder,
                snapshot: current,
            },
        };
        store.state.proposals.insert(fingerprint(&draft)?, draft);
    }
    store.save()?;
    Ok(blocked)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(home: &Path, path: &str, text: &str) -> PathBuf {
        let full = home.join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(&full, text).unwrap();
        full
    }

    fn make_test_dirs(dir: &tempfile::TempDir) -> (PathBuf, PathBuf) {
        let data = dir.path().join("data");
        let home = dir.path().join("home");
        fs::create_dir(&data).unwrap();
        fs::create_dir(&home).unwrap();
        (data, home)
    }

    #[test]
    fn rulesync_skill_agent_proposal_survives_reopen_and_requires_review() {
        let scratch = tempfile::tempdir().unwrap();
        let (data, home) = make_test_dirs(&scratch);
        let mut store = SkillStore::open(&data).unwrap();
        let name = "my-skill";
        let files = BTreeMap::from([(PathBuf::from("SKILL.md"), b"Some body.\n".to_vec())]);
        let proposal = store
            .state
            .propose_agent(Draft {
                name: name.into(),
                scope: SavedSkillScope::Shared,
                base: store.state.revision(name).unwrap(),
                source: Candidate::Agent {
                    agent: "codex".into(),
                    files: files.clone(),
                },
            })
            .unwrap();
        store.save().unwrap();
        assert!(approved_skill_sources(&data, "codexcli")
            .unwrap()
            .is_empty());
        assert!(fs::read_dir(&home).unwrap().next().is_none());

        let mut store = SkillStore::open(&data).unwrap();
        assert_eq!(
            store.state.proposals[&proposal.id]
                .preview(proposal.id.clone())
                .unwrap(),
            proposal
        );
        store
            .state
            .review(&home, proposal.id, SkillReviewAction::Accept, None, |_| {
                Ok(())
            })
            .unwrap();
        store.save().unwrap();
        assert_eq!(
            approved_skill_sources(&data, "codexcli").unwrap()[name],
            files
        );
        assert_eq!(
            approved_skill_sources(&data, "claudecode").unwrap()[name],
            files
        );
        assert!(SkillStore::open(&data).unwrap().state.proposals.is_empty());
        assert!(fs::read_dir(&home).unwrap().next().is_none());
    }

    #[test]
    fn rulesync_skill_hand_added_folder_is_a_tool_only_proposal_until_accepted() {
        let scratch = tempfile::tempdir().unwrap();
        let (data, home) = make_test_dirs(&scratch);
        let file = write(&home, ".claude/skills/own/SKILL.md", "Original body.\n");
        let folder = PathBuf::from(".claude/skills/own");
        let pending =
            collect_tool_proposals(&home, &data, &BTreeMap::new(), &BTreeSet::new()).unwrap();
        assert_eq!(pending.len(), 1);
        assert!(pending.contains_key(&folder));
        let mut store = SkillStore::open(&data).unwrap();
        let (id, draft) = store.state.proposals.first_key_value().unwrap();
        let proposal = draft.preview(id.clone()).unwrap();
        assert_eq!(
            proposal.scope,
            SavedSkillScope::Tool {
                tool: SavedSkillTool::ClaudeCode
            }
        );
        assert_eq!(proposal.files[0].text.as_deref(), Some("Original body.\n"));

        fs::write(&file, "Changed during review.\n").unwrap();
        assert!(store
            .state
            .review(&home, proposal.id, SkillReviewAction::Accept, None, |_| Ok(
                ()
            ))
            .is_err());
        collect_tool_proposals(&home, &data, &BTreeMap::new(), &BTreeSet::new()).unwrap();
        let mut store = SkillStore::open(&data).unwrap();
        let id = store.state.proposals.first_key_value().unwrap().0.clone();
        store
            .state
            .review(&home, id, SkillReviewAction::Accept, None, |_| Ok(()))
            .unwrap();
        store.save().unwrap();
        assert!(approved_local_folders(&data).unwrap().contains_key(&folder));
        assert!(approved_skill_sources(&data, "codexcli")
            .unwrap()
            .is_empty());
        assert_eq!(
            approved_skill_sources(&data, "claudecode").unwrap()["own"][Path::new("SKILL.md")],
            b"Changed during review.\n"
        );
        assert!(
            collect_tool_proposals(&home, &data, &BTreeMap::new(), &BTreeSet::new())
                .unwrap()
                .is_empty()
        );

        let mut store = SkillStore::open(&data).unwrap();
        assert_eq!(store.state.rows().unwrap()[0].skill.name, "own");
        assert!(store.state.remove_kept("own").unwrap());
        assert!(!store.state.remove_kept("own").unwrap());
        store.save().unwrap();
        assert!(approved_skill_sources(&data, "claudecode")
            .unwrap()
            .is_empty());
        assert!(approved_local_folders(&data).unwrap().is_empty());
        assert_eq!(fs::read(&file).unwrap(), b"Changed during review.\n");
        collect_tool_proposals(&home, &data, &BTreeMap::new(), &BTreeSet::new()).unwrap();
        assert!(SkillStore::open(&data).unwrap().state.proposals.is_empty());
    }

    #[test]
    fn rulesync_skill_new_tool_can_keep_or_share_an_edit() {
        let scratch = tempfile::tempdir().unwrap();
        let (data, home) = make_test_dirs(&scratch);
        let gemini = SavedSkillTool::GeminiCli;
        let folder = PathBuf::from(".gemini/skills/own");
        let initial = "Gemini skill.\n";
        let file = write(&home, ".gemini/skills/own/SKILL.md", initial);
        for (scope, contents) in [
            (SavedSkillScope::Tool { tool: gemini }, initial),
            (SavedSkillScope::Shared, "Shared skill.\n"),
        ] {
            fs::write(&file, contents).unwrap();
            let blocked =
                collect_tool_proposals(&home, &data, &BTreeMap::new(), &BTreeSet::new()).unwrap();
            assert_eq!(blocked.len(), 1);
            assert!(blocked.contains_key(&folder));
            let mut store = SkillStore::open(&data).unwrap();
            let (id, draft) = store.state.proposals.first_key_value().unwrap();
            let proposal = draft.preview(id.clone()).unwrap();
            assert_eq!(proposal.origin, SkillProposalOrigin::Tool { tool: gemini });
            assert_eq!(proposal.scope, SavedSkillScope::Tool { tool: gemini });
            store
                .state
                .review(
                    &home,
                    proposal.id,
                    SkillReviewAction::Accept,
                    Some(scope),
                    |_| Ok(()),
                )
                .unwrap();
            store.save().unwrap();
            for target in rulesync_personal::PERSONAL_TARGETS {
                let sources = approved_skill_sources(&data, target.id).unwrap();
                if target.tool == gemini || scope == SavedSkillScope::Shared {
                    assert_eq!(sources["own"][Path::new("SKILL.md")], contents.as_bytes());
                } else {
                    assert!(sources.is_empty());
                }
            }
        }
    }

    #[test]
    fn rulesync_skill_managed_asset_edit_can_be_dismissed_or_shared() {
        let scratch = tempfile::tempdir().unwrap();
        let (data, home) = make_test_dirs(&scratch);
        write(&home, ".codex/skills/own/SKILL.md", "The body.\n");
        let asset = write(
            &home,
            ".codex/skills/own/assets/notes.md",
            "The old asset.\n",
        );
        let folder = PathBuf::from(".codex/skills/own");
        let current = SkillHome::open(&home)
            .unwrap()
            .package(&folder, SavedSkillTool::Codex)
            .unwrap()
            .unwrap();
        let managed = BTreeMap::from([(folder.clone(), current)]);
        write(
            &home,
            ".codex/skills/own/scripts/.claude/.cc-writes/log",
            "Local tool state.\n",
        );
        assert!(
            collect_tool_proposals(&home, &data, &managed, &BTreeSet::new())
                .unwrap()
                .is_empty()
        );

        fs::write(&asset, "First edit.\n").unwrap();
        let pending = collect_tool_proposals(&home, &data, &managed, &BTreeSet::new()).unwrap();
        assert!(pending.contains_key(&folder));
        let mut store = SkillStore::open(&data).unwrap();
        let id = store.state.proposals.first_key_value().unwrap().0.clone();
        store
            .state
            .review(&home, id, SkillReviewAction::Dismiss, None, |_| Ok(()))
            .unwrap();
        store.save().unwrap();
        assert!(
            collect_tool_proposals(&home, &data, &managed, &BTreeSet::new())
                .unwrap()
                .contains_key(&folder)
        );
        assert!(SkillStore::open(&data).unwrap().state.proposals.is_empty());

        fs::write(&asset, "Second edit.\n").unwrap();
        collect_tool_proposals(&home, &data, &managed, &BTreeSet::new()).unwrap();
        let mut store = SkillStore::open(&data).unwrap();
        let id = store.state.proposals.first_key_value().unwrap().0.clone();
        store
            .state
            .review(
                &home,
                id,
                SkillReviewAction::Accept,
                Some(SavedSkillScope::Shared),
                |_| Ok(()),
            )
            .unwrap();
        store.save().unwrap();
        let shared = approved_skill_sources(&data, "claudecode").unwrap();
        assert_eq!(
            shared["own"][Path::new("assets/notes.md")],
            b"Second edit.\n"
        );
        assert_eq!(approved_skill_sources(&data, "codexcli").unwrap(), shared);
        assert_eq!(fs::read(&asset).unwrap(), b"Second edit.\n");
    }
}
