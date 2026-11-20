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
use sha2::{Digest, Sha256};
use wenlan_core::rulesync_adopt::{
    skip_skill_asset, MAX_ADOPTED_SKILL_BYTES, MAX_ADOPTED_SKILL_DEPTH, MAX_ADOPTED_SKILL_ENTRIES,
};
use wenlan_core::rulesync_personal::{self, fingerprint};
use wenlan_core::rulesync_skill::{self as skills, SkillFiles};
use wenlan_core::skill_registry;
use wenlan_types::rulesync::RulesSyncReport;
use wenlan_types::rulesync_skill::{
    SaveSkillRequest, SaveSkillResult, SavedSkillScope, SavedSkillTool, SkillProposal,
    SkillProposalChanges, SkillProposalOrigin, SkillProposalsResponse, SkillReviewAction,
    SkillReviewRequest,
};
use wenlan_types::skill_registry::{InstalledSkill, InstalledSkillRow, Scope, Verdict};

use crate::error::ServerError;
use crate::route_registry::{get, post, TrackedRouter};
use crate::rulesync_apply::{ApplyOutcome, FileMode, FileRoot, Snapshot};
use crate::rulesync_routes::{require_local_person, Setup};
use crate::state::SharedState;

const SKILL_STATE: &str = "skills.json";
const SKILL_BLOBS: &str = "skills-blobs";
const SKILL_STATE_FORMAT: u32 = 1;
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
                store
                    .state
                    .keep(&request.name, request.scope, files, SkillModes::default());
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
pub(crate) struct ManagedSkill<Content = Vec<u8>> {
    pub tool: SavedSkillTool,
    /// Package-relative leaf snapshots, including each file's mode.
    pub files: BTreeMap<PathBuf, Snapshot<Content>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub directories: BTreeMap<PathBuf, FileMode>,
}

impl ManagedSkill<Vec<u8>> {
    fn files(&self) -> SkillFiles {
        self.files
            .iter()
            .filter_map(|(path, snapshot)| match snapshot {
                Snapshot::File { bytes, .. } => Some((path.clone(), bytes.clone())),
                _ => None,
            })
            .collect()
    }

    fn modes(&self) -> SkillModes {
        SkillModes {
            files: self
                .files
                .iter()
                .filter_map(|(path, snapshot)| match snapshot {
                    Snapshot::File { mode, .. } => Some((path.clone(), *mode)),
                    _ => None,
                })
                .collect(),
            directories: self.directories.clone(),
        }
    }

    fn matches(&self, current: &Self) -> bool {
        self.tool == current.tool
            && self.files == current.files
            && (self.directories.is_empty() || self.directories == current.directories)
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(bound(deserialize = "Content: Deserialize<'de>"))]
enum Candidate<Content = Vec<u8>> {
    Agent {
        agent: String,
        files: BTreeMap<PathBuf, Content>,
    },
    Tool {
        folder: PathBuf,
        snapshot: ManagedSkill<Content>,
        #[serde(default)]
        changes: Option<ToolSkillChanges<Content>>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(bound(deserialize = "Content: Deserialize<'de>"))]
struct ToolSkillChanges<Content = Vec<u8>> {
    changed_paths: BTreeSet<PathBuf>,
    removed_paths: BTreeSet<PathBuf>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    previous: BTreeMap<PathBuf, Content>,
}

impl ToolSkillChanges<Vec<u8>> {
    fn between(before: &SkillFiles, after: &SkillFiles) -> Option<Self> {
        let changed_paths: BTreeSet<_> = after
            .iter()
            .filter(|(path, bytes)| before.get(*path) != Some(*bytes))
            .map(|(path, _)| path.clone())
            .collect();
        let removed_paths: BTreeSet<_> = before
            .keys()
            .filter(|path| !after.contains_key(*path))
            .cloned()
            .collect();
        let previous = changed_paths
            .iter()
            .filter_map(|path| before.get(path).map(|bytes| (path.clone(), bytes.clone())))
            .collect();
        (!changed_paths.is_empty() || !removed_paths.is_empty()).then_some(Self {
            changed_paths,
            removed_paths,
            previous,
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Draft<Content = Vec<u8>> {
    name: String,
    scope: SavedSkillScope,
    base: String,
    source: Candidate<Content>,
}

impl Draft {
    fn preview(&self, id: String) -> Result<SkillProposal> {
        let (origin, files, changes) = match &self.source {
            Candidate::Agent { agent, files } => (
                SkillProposalOrigin::Agent {
                    agent: agent.clone(),
                },
                skills::preview(&self.name, files)?,
                None,
            ),
            Candidate::Tool {
                snapshot, changes, ..
            } => {
                let full_files = snapshot.files();
                let (files, changes) = match changes {
                    Some(changes) => {
                        let preview = skills::preview_changes(
                            &self.name,
                            &full_files,
                            &changes.previous,
                            &changes.changed_paths,
                            &changes.removed_paths,
                        )?;
                        (
                            preview.files,
                            Some(SkillProposalChanges {
                                removed: changes
                                    .removed_paths
                                    .iter()
                                    .map(|path| {
                                        path.to_str()
                                            .context("a removed skill path is not UTF-8")
                                            .map(str::to_owned)
                                    })
                                    .collect::<Result<_>>()?,
                                oversized: preview.oversized,
                            }),
                        )
                    }
                    None => (skills::preview(&self.name, &full_files)?, None),
                };
                (
                    SkillProposalOrigin::Tool {
                        tool: snapshot.tool,
                    },
                    files,
                    changes,
                )
            }
        };
        Ok(SkillProposal {
            id,
            name: self.name.clone(),
            scope: self.scope,
            origin,
            files,
            changes,
        })
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct SkillModes {
    pub files: BTreeMap<PathBuf, FileMode>,
    pub directories: BTreeMap<PathBuf, FileMode>,
}

impl SkillModes {
    fn is_empty(&self) -> bool {
        self.files.is_empty() && self.directories.is_empty()
    }
}

#[derive(Default)]
pub(crate) struct ApprovedSkills {
    pub files: BTreeMap<String, SkillFiles>,
    pub modes: BTreeMap<String, SkillModes>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
struct Variants<Content = Vec<u8>> {
    shared: Option<BTreeMap<PathBuf, Content>>,
    tools: BTreeMap<SavedSkillTool, BTreeMap<PathBuf, Content>>,
    #[serde(skip_serializing_if = "SkillModes::is_empty")]
    shared_modes: SkillModes,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    tool_modes: BTreeMap<SavedSkillTool, SkillModes>,
    installed_at: i64,
    updated_at: i64,
}

impl<Content> Default for Variants<Content> {
    fn default() -> Self {
        Self {
            shared: None,
            tools: BTreeMap::new(),
            shared_modes: SkillModes::default(),
            tool_modes: BTreeMap::new(),
            installed_at: 0,
            updated_at: 0,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
struct SkillState<Content = Vec<u8>> {
    proposals: BTreeMap<String, Draft<Content>>,
    kept: BTreeMap<String, Variants<Content>>,
    ignored: BTreeMap<PathBuf, String>,
    approved_local: BTreeMap<PathBuf, ManagedSkill<Content>>,
}

impl<Content> Default for SkillState<Content> {
    fn default() -> Self {
        Self {
            proposals: BTreeMap::new(),
            kept: BTreeMap::new(),
            ignored: BTreeMap::new(),
            approved_local: BTreeMap::new(),
        }
    }
}

impl SkillState<Vec<u8>> {
    fn revision(&self, name: &str) -> Result<String> {
        let skill = self.kept.get(name);
        if let Some(skill) =
            skill.filter(|skill| !skill.shared_modes.is_empty() || !skill.tool_modes.is_empty())
        {
            fingerprint(&(
                &skill.shared,
                &skill.tools,
                &skill.shared_modes,
                &skill.tool_modes,
            ))
        } else {
            fingerprint(&skill.map(|skill| (&skill.shared, &skill.tools)))
        }
    }

    fn keep(&mut self, name: &str, scope: SavedSkillScope, files: SkillFiles, modes: SkillModes) {
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
                variants.shared_modes = modes;
                variants.tools.clear();
                variants.tool_modes.clear();
            }
            SavedSkillScope::Tool { tool } => {
                variants.tools.insert(tool, files);
                if modes.is_empty() {
                    variants.tool_modes.remove(&tool);
                } else {
                    variants.tool_modes.insert(tool, modes);
                }
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
            if let Candidate::Tool {
                folder, snapshot, ..
            } = &draft.source
            {
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
        let (files, modes) = match &draft.source {
            Candidate::Agent { files, .. } => {
                check_name(&name)?;
                skills::preview(&name, files)?;
                (files.clone(), SkillModes::default())
            }
            Candidate::Tool {
                folder,
                snapshot,
                changes,
            } => {
                ensure!(
                    matches!(scope, SavedSkillScope::Shared) || scope == draft.scope,
                    "accept for the tool that changed the skill or share across tools"
                );
                let scanning_root = SkillHome::open(home)?;
                let current = scanning_root.package(folder, snapshot.tool)?;
                ensure!(
                    current.as_ref().is_some_and(|current| snapshot.matches(current)),
                    "the tool edited this skill again. Run `wenlan rules sync` to review the current files"
                );
                self.approved_local.insert(folder.clone(), snapshot.clone());
                self.ignored.remove(folder);
                let files = snapshot.files();
                match changes {
                    Some(changes) => {
                        skills::preview_changes(
                            &name,
                            &files,
                            &changes.previous,
                            &changes.changed_paths,
                            &changes.removed_paths,
                        )?;
                    }
                    None => {
                        skills::preview(&name, &files)?;
                    }
                }
                (files, snapshot.modes())
            }
        };
        self.keep(&name, scope, files, modes);
        self.proposals.remove(&id);
        Ok(name)
    }

    fn sources(&self, tool: SavedSkillTool) -> ApprovedSkills {
        let mut sources = ApprovedSkills::default();
        for (name, skill) in &self.kept {
            let (files, modes) = match (skill.tools.get(&tool), &skill.shared) {
                (Some(files), _) => (files, skill.tool_modes.get(&tool)),
                (None, Some(files)) => (files, Some(&skill.shared_modes)),
                (None, None) => continue,
            };
            sources.files.insert(name.clone(), files.clone());
            if let Some(modes) = modes.filter(|modes| !modes.is_empty()) {
                sources.modes.insert(name.clone(), modes.clone());
            }
        }
        sources
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

#[derive(Debug, Serialize, Deserialize)]
#[serde(transparent)]
struct SkillBlobHash(String);

impl SkillBlobHash {
    fn of(bytes: &[u8]) -> Self {
        Self(hex::encode(Sha256::digest(bytes)))
    }

    fn path(&self) -> Result<PathBuf> {
        ensure!(
            self.0.len() == 64
                && self
                    .0
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')),
            "invalid skill blob hash"
        );
        Ok(Path::new(SKILL_BLOBS).join(&self.0))
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredSkillState<Content> {
    format: u32,
    state: SkillState<Content>,
}

fn map_skill_state<From, To>(
    state: SkillState<From>,
    map: &mut impl FnMut(From) -> Result<To>,
) -> Result<SkillState<To>> {
    let proposals = state
        .proposals
        .into_iter()
        .map(|(id, draft)| {
            let source = match draft.source {
                Candidate::Agent { agent, files } => Candidate::Agent {
                    agent,
                    files: map_skill_files(files, map)?,
                },
                Candidate::Tool {
                    folder,
                    snapshot,
                    changes,
                } => Candidate::Tool {
                    folder,
                    snapshot: map_managed_skill(snapshot, map)?,
                    changes: changes
                        .map(|changes| {
                            Ok::<_, anyhow::Error>(ToolSkillChanges {
                                changed_paths: changes.changed_paths,
                                removed_paths: changes.removed_paths,
                                previous: map_skill_files(changes.previous, map)?,
                            })
                        })
                        .transpose()?,
                },
            };
            Ok((
                id,
                Draft {
                    name: draft.name,
                    scope: draft.scope,
                    base: draft.base,
                    source,
                },
            ))
        })
        .collect::<Result<_>>()?;
    let kept = state
        .kept
        .into_iter()
        .map(|(name, variants)| {
            Ok((
                name,
                Variants {
                    shared: variants
                        .shared
                        .map(|files| map_skill_files(files, map))
                        .transpose()?,
                    tools: variants
                        .tools
                        .into_iter()
                        .map(|(tool, files)| Ok((tool, map_skill_files(files, map)?)))
                        .collect::<Result<_>>()?,
                    shared_modes: variants.shared_modes,
                    tool_modes: variants.tool_modes,
                    installed_at: variants.installed_at,
                    updated_at: variants.updated_at,
                },
            ))
        })
        .collect::<Result<_>>()?;
    let approved_local = state
        .approved_local
        .into_iter()
        .map(|(path, skill)| Ok((path, map_managed_skill(skill, map)?)))
        .collect::<Result<_>>()?;
    Ok(SkillState {
        proposals,
        kept,
        ignored: state.ignored,
        approved_local,
    })
}

fn map_skill_files<From, To>(
    files: BTreeMap<PathBuf, From>,
    map: &mut impl FnMut(From) -> Result<To>,
) -> Result<BTreeMap<PathBuf, To>> {
    files
        .into_iter()
        .map(|(path, content)| Ok((path, map(content)?)))
        .collect()
}

fn map_managed_skill<From, To>(
    skill: ManagedSkill<From>,
    map: &mut impl FnMut(From) -> Result<To>,
) -> Result<ManagedSkill<To>> {
    Ok(ManagedSkill {
        tool: skill.tool,
        directories: skill.directories,
        files: skill
            .files
            .into_iter()
            .map(|(path, snapshot)| {
                let Snapshot::File { bytes, mode } = snapshot else {
                    bail!("a managed skill asset is not a regular file");
                };
                Ok((
                    path,
                    Snapshot::File {
                        bytes: map(bytes)?,
                        mode,
                    },
                ))
            })
            .collect::<Result<_>>()?,
    })
}

struct SkillStore {
    root: FileRoot,
    seen: Snapshot,
    state: SkillState,
}

impl SkillStore {
    fn open(data: &Path) -> Result<Self> {
        let root = FileRoot::open(data)?.with_private_directories();
        let seen = root.read(Path::new(SKILL_STATE), false)?;
        let state = match &seen {
            Snapshot::File { bytes, .. } => {
                let value: serde_json::Value = serde_json::from_slice(bytes)?;
                if value.get("format").is_some() {
                    let stored: StoredSkillState<SkillBlobHash> = serde_json::from_value(value)?;
                    ensure!(
                        stored.format == SKILL_STATE_FORMAT,
                        "unsupported personal skill state format {}",
                        stored.format
                    );
                    let mut cache = BTreeMap::<String, Vec<u8>>::new();
                    map_skill_state(stored.state, &mut |hash| {
                        if let Some(bytes) = cache.get(&hash.0) {
                            return Ok(bytes.clone());
                        }
                        let blob = hash.path()?;
                        let Snapshot::File { bytes, .. } = root.read_private(&blob, None)? else {
                            bail!("missing personal skill blob");
                        };
                        ensure!(
                            SkillBlobHash::of(&bytes).0 == hash.0,
                            "damaged personal skill blob"
                        );
                        cache.insert(hash.0.clone(), bytes.clone());
                        Ok(bytes)
                    })?
                } else {
                    serde_json::from_value(value)?
                }
            }
            Snapshot::Absent => SkillState::default(),
            _ => bail!("unexpected personal skill store file type"),
        };
        Ok(Self { root, seen, state })
    }

    fn save(self) -> Result<()> {
        let mut saved = BTreeSet::new();
        let mut added_blob = false;
        let state = StoredSkillState {
            format: SKILL_STATE_FORMAT,
            state: map_skill_state(self.state, &mut |bytes| {
                let hash = SkillBlobHash::of(&bytes);
                if saved.insert(hash.0.clone()) {
                    let path = hash.path()?;
                    match self.root.read_private(&path, Some(&bytes))? {
                        Snapshot::Absent => {
                            let desired = Snapshot::file(bytes.clone());
                            ensure!(
                                self.root.apply(&path, &Snapshot::Absent, &desired, false)?
                                    == ApplyOutcome::Applied,
                                "personal skill blob changed while saving"
                            );
                            added_blob = true;
                        }
                        Snapshot::File { bytes: current, .. } if current == bytes => (),
                        _ => bail!("damaged personal skill blob"),
                    }
                }
                Ok(hash)
            })?,
        };
        if added_blob {
            self.root.sync_directory(Path::new(SKILL_BLOBS))?;
            self.root.sync_directory(Path::new(""))?;
        }
        let next = Snapshot::file(serde_json::to_vec(&state)?);
        if self.seen == next {
            return Ok(());
        }
        ensure!(
            self.root
                .apply(Path::new(SKILL_STATE), &self.seen, &next, false)?
                == ApplyOutcome::Applied,
            "personal skill state changed during the operation"
        );
        self.root.sync_directory(Path::new(""))?;
        Ok(())
    }
}

/// Read under the same setup lock as the renderer and proposal operations.
pub(crate) fn approved_skill_sources(
    data: &Path,
    target: &str,
) -> Result<BTreeMap<String, SkillFiles>> {
    Ok(approved_skills(data, target)?.files)
}

pub(crate) fn approved_skills(data: &Path, target: &str) -> Result<ApprovedSkills> {
    let Some(target) = rulesync_personal::personal_target(target) else {
        return Ok(ApprovedSkills::default());
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
            directories: BTreeMap::from([(PathBuf::new(), FileMode::of(&dir.dir_metadata()?))]),
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
            directories: package.directories,
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
                let child = dir.open_dir_nofollow(name)?;
                result
                    .directories
                    .insert(path.clone(), FileMode::of(&child.dir_metadata()?));
                self.read_files(&child, folder, &path, result)?;
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
    directories: BTreeMap<PathBuf, FileMode>,
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
            || store
                .state
                .approved_local
                .get(&folder)
                .is_some_and(|approved| approved.matches(&current))
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
        let changes =
            baseline.and_then(|baseline| ToolSkillChanges::between(&baseline.files(), &files));
        let validation = match &changes {
            Some(changes) => skills::preview_changes(
                name,
                &files,
                &changes.previous,
                &changes.changed_paths,
                &changes.removed_paths,
            )
            .map(|_| ()),
            None => skills::preview(name, &files).map(|_| ()),
        };
        if let Err(error) = validation {
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
                changes,
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
        create_test_data_directory(&data);
        fs::create_dir(&home).unwrap();
        (data, home)
    }

    fn create_test_data_directory(path: &Path) {
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt as _;
            builder.mode(0o700);
        }
        builder.recursive(false).create(path).unwrap();
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

    #[test]
    fn skill_store_blobs_keep_large_packages_and_read_legacy_state() {
        let scratch = tempfile::tempdir().unwrap();
        let data = scratch.path().join("data");
        create_test_data_directory(&data);
        let payload = vec![u8::MAX; 8 * 1024 * 1024];
        let path = PathBuf::from("SKILL.md");
        let mut state = SkillState::default();
        state.keep(
            "large-skill",
            SavedSkillScope::Shared,
            BTreeMap::from([(path.clone(), payload.clone())]),
            SkillModes::default(),
        );
        state.approved_local.insert(
            PathBuf::from(".cursor/skills/large-skill"),
            ManagedSkill {
                tool: SavedSkillTool::Cursor,
                files: BTreeMap::from([(path, Snapshot::file(payload.clone()))]),
                directories: BTreeMap::new(),
            },
        );
        assert!(serde_json::to_vec(&state).unwrap().len() > 32 * 1024 * 1024);

        SkillStore {
            root: FileRoot::open(&data).unwrap().with_private_directories(),
            seen: Snapshot::Absent,
            state,
        }
        .save()
        .unwrap();
        assert!(fs::metadata(data.join(SKILL_STATE)).unwrap().len() < 32 * 1024 * 1024);
        assert_eq!(fs::read_dir(data.join(SKILL_BLOBS)).unwrap().count(), 1);
        let reopened = SkillStore::open(&data).unwrap();
        assert_eq!(
            reopened.state.kept["large-skill"].shared.as_ref().unwrap()[Path::new("SKILL.md")],
            payload
        );
        assert_eq!(
            reopened.state.approved_local[Path::new(".cursor/skills/large-skill")].files
                [Path::new("SKILL.md")],
            Snapshot::file(vec![u8::MAX; 8 * 1024 * 1024])
        );

        let legacy_data = scratch.path().join("legacy");
        create_test_data_directory(&legacy_data);
        let legacy_state = SkillState {
            kept: BTreeMap::from([(
                "legacy-skill".into(),
                Variants {
                    shared: Some(BTreeMap::from([(
                        PathBuf::from("SKILL.md"),
                        b"Legacy skill body.\n".to_vec(),
                    )])),
                    ..Variants::default()
                },
            )]),
            ..SkillState::default()
        };
        fs::write(
            legacy_data.join(SKILL_STATE),
            serde_json::to_vec(&legacy_state).unwrap(),
        )
        .unwrap();
        let legacy = SkillStore::open(&legacy_data).unwrap();
        assert_eq!(
            legacy.state.kept["legacy-skill"].shared,
            legacy_state.kept["legacy-skill"].shared
        );
        legacy.save().unwrap();
        let migrated: serde_json::Value =
            serde_json::from_slice(&fs::read(legacy_data.join(SKILL_STATE)).unwrap()).unwrap();
        assert_eq!(migrated["format"], SKILL_STATE_FORMAT);
    }
}
