// SPDX-License-Identifier: Apache-2.0
//! `wenlan rules share`: offer each managed skill, command, subagent and MCP
//! server to the other enrolled tools that support it. The writes use the
//! same journal, backup and restore path as adding a tool to the setup.

use super::key_shares::KeyShare;
use super::personal_files::{choose_destination, file_bytes};
use super::personal_settings::{enroll_shares, SettingAddition};
use super::*;
use wenlan_core::rulesync_personal::{adoption_plan_id, reject_secret_lines};
use wenlan_core::rulesync_personal_files::{sharing_paths, NO_PERSONAL_DESTINATION};
use wenlan_types::rulesync::{SharingPlan, SharingStay, SharingTool};

const SHARE_TOKEN_VERSION: &str = "share-1";
const NOTHING_TO_SHARE: &str = "Nothing to share";

/// A skill folder a tool receives, recorded in the master after the write.
#[derive(Serialize)]
struct SkillAddition {
    index: usize,
    target: &'static str,
    folder: PathBuf,
}

pub(super) struct Share {
    operations: Vec<Pending>,
    skills: Vec<SkillAddition>,
    settings: Vec<SettingAddition>,
    /// Servers that may hold a key; written only with the person's yes.
    keys: Vec<KeyShare>,
    worktree_writes: BTreeMap<PathBuf, PathBuf>,
    pub(super) plan: SharingPlan,
}

impl Share {
    fn is_empty(&self) -> bool {
        self.operations.is_empty() && self.settings.is_empty()
    }
}

/// The person's answer to the second question, about servers that may
/// hold a key. An AI caller is never asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum KeyAnswer {
    Yes,
    No,
    NotAsked,
}

impl KeyAnswer {
    /// Servers that may hold a key travel only on the person's own second
    /// yes: never on the AI confirm route, never for a request that names
    /// a session or an agent.
    pub(super) fn of(ai_caller: bool, request: &AdoptRequest) -> Self {
        if ai_caller || request.confirmed_plan_id.is_some() {
            Self::NotAsked
        } else if request.include_keys {
            Self::Yes
        } else {
            Self::No
        }
    }
}

fn keys_left_message(count: usize, answer: KeyAnswer) -> String {
    let servers = if count == 1 { "server" } else { "servers" };
    let line = format!("Left {count} {servers} that may contain a key where they are.");
    if answer == KeyAnswer::NotAsked {
        format!("{line} Only the person can share those, by running `wenlan rules share` in their own terminal.")
    } else {
        line
    }
}

#[derive(Default)]
struct Listing {
    tools: BTreeMap<usize, SharingTool>,
    stays: Vec<SharingStay>,
    already_there: usize,
}

fn display(id: &str) -> &'static str {
    personal_target(id).map_or("this tool", |target| target.tool.display_name())
}

impl Listing {
    fn tool(&mut self, id: &str) -> &mut SharingTool {
        let index = PERSONAL_TARGETS
            .iter()
            .position(|target| target.id == id)
            .unwrap_or(usize::MAX);
        self.tools.entry(index).or_insert_with(|| SharingTool {
            tool: display(id).into(),
            ..Default::default()
        })
    }

    fn stay(&mut self, tool: &str, item: impl Into<String>, reason: impl Into<String>) {
        let stay = SharingStay {
            tool: display(tool).into(),
            item: item.into(),
            reason: reason.into(),
        };
        if !self.stays.contains(&stay) {
            self.stays.push(stay);
        }
    }
}

/// The credential check shared setup already applies to skills and rules.
fn possible_credentials<'a>(files: impl IntoIterator<Item = &'a [u8]>) -> bool {
    files
        .into_iter()
        .any(|bytes| reject_secret_lines(&String::from_utf8_lossy(bytes)).is_err())
}

fn plain_folder_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

/// The regular files under a folder, or None when it holds anything else.
fn disk_tree(root: &FileRoot, folder: &Path) -> Result<Option<BTreeMap<PathBuf, Vec<u8>>>> {
    let mut files = BTreeMap::new();
    let mut folders = vec![folder.to_owned()];
    while let Some(next) = folders.pop() {
        for child in root.children(&next)? {
            match root.read(&child, false)? {
                Snapshot::File { bytes, .. } => {
                    files.insert(child.strip_prefix(folder)?.to_owned(), bytes);
                }
                Snapshot::Directory { .. } => folders.push(child),
                _ => return Ok(None),
            }
        }
    }
    Ok(Some(files))
}

/// Whether a restore copy already claims this path, something inside it, or
/// a missing folder above it. The batch refuses to recreate such a path.
fn owned_by_setup(root: &FileRoot, manifest: &Manifest, path: &Path) -> Result<bool> {
    for known in manifest.backup.keys().chain(manifest.delivered.keys()) {
        if known.starts_with(path)
            || path.starts_with(known)
                && manifest.backup.contains_key(known)
                && root.read(known, false)? == Snapshot::Absent
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn write_pending(manifest: &Manifest, path: &Path) -> bool {
    path_blocked(&manifest.extension_paths, path)
        || manifest
            .pending
            .iter()
            .any(|pending| path.starts_with(&pending.path) || pending.path.starts_with(path))
}

impl Setup {
    fn share_manifest(&self) -> Result<Manifest> {
        let manifest = self
            .adoptable_manifest()?
            .filter(|manifest| manifest.adopted && !manifest.settings_only)
            .context("Run `wenlan rules adopt` before sharing items between tools.")?;
        Ok(manifest)
    }

    pub(super) fn preview_share(&self, expected_home: &Path) -> Result<SharingPlan> {
        self.check_adoption_home(expected_home)?;
        let session = RenderSession::new(&self.binary);
        self.preview_share_using(|renderer, skills| self.render_skills(&session, renderer, skills))
    }

    pub(super) fn preview_share_using(
        &self,
        render_skills: impl Fn(&str, &SkillPackages) -> Result<SkillFiles>,
    ) -> Result<SharingPlan> {
        let manifest = self.share_manifest()?;
        let root = FileRoot::open_home(&self.home)?;
        Ok(self.plan_share(&root, &manifest, &render_skills)?.plan)
    }

    /// `keys` is true only for the person's own second yes; the route
    /// decides it from the request (see `execute`).
    pub(super) fn share(&self, request: &AdoptRequest, keys: KeyAnswer) -> Result<RulesSyncReport> {
        if let Some(home) = &request.home {
            self.check_adoption_home(home)?;
        }
        let session = RenderSession::new(&self.binary);
        self.share_with_keys(&request.token, keys, |renderer, skills| {
            self.render_skills(&session, renderer, skills)
        })
    }

    #[cfg(test)]
    pub(super) fn share_using(
        &self,
        token: &str,
        render_skills: impl Fn(&str, &SkillPackages) -> Result<SkillFiles>,
    ) -> Result<RulesSyncReport> {
        self.share_with_keys(token, KeyAnswer::No, render_skills)
    }

    pub(super) fn share_with_keys(
        &self,
        token: &str,
        answer: KeyAnswer,
        render_skills: impl Fn(&str, &SkillPackages) -> Result<SkillFiles>,
    ) -> Result<RulesSyncReport> {
        let _lock = self.lock()?;
        let mut manifest = self.share_manifest()?;
        let root = FileRoot::open_home(&self.home)?;
        self.claim_key_intents(&root, &mut manifest)?;
        let share = self.plan_share(&root, &manifest, &render_skills)?;
        ensure!(
            share.plan.token == token,
            "The setup changed since that preview. Run `wenlan rules share --dry-run` again."
        );
        let (keys, left) = if answer == KeyAnswer::Yes {
            (share.keys.clone(), 0)
        } else {
            (Vec::new(), share.plan.key_servers.len())
        };
        let left = (left != 0).then(|| keys_left_message(left, answer));
        if share.is_empty() && keys.is_empty() {
            let mut result = report(NOTHING_TO_SHARE);
            if let Some(left) = left {
                result.message = format!("{NOTHING_TO_SHARE}.\n{left}");
            }
            return Ok(result);
        }
        let Share {
            operations,
            skills,
            settings,
            worktree_writes,
            ..
        } = share;
        let mut result = report("Shared existing items with the other tools");
        let outcomes = if operations.is_empty() {
            Vec::new()
        } else {
            self.run_batch(&root, &mut manifest, operations, BatchMode::Extend, None)
                .with_context(|| {
                    format!("Sharing stopped. Run `{SYNC_COMMAND}` to roll back the copies")
                })?
        };
        for (path, status) in outcomes {
            ensure!(
                status == ApplyOutcome::Applied,
                "{} changed while sharing. Run `{SYNC_COMMAND}` to roll back the copies",
                path.display()
            );
            if !manifest
                .delivered
                .get(&path)
                .is_some_and(|delivery| matches!(delivery.kind, Kind::Directory))
            {
                result.written.push(path.display().to_string());
            }
        }
        // One checkpoint enrolls the copies, as for a tool addition. Recovery
        // before this point rolls back the journaled copies.
        for addition in skills {
            let skill = &mut manifest.master.skills[addition.index];
            skill.targets.insert(addition.target.to_owned());
            skill
                .locations
                .entry(addition.target.to_owned())
                .or_default()
                .insert(addition.folder);
        }
        manifest.extension_paths.clear();
        manifest.worktree_writes.extend(worktree_writes);
        let destinations: BTreeSet<_> = settings
            .iter()
            .map(|addition| addition.path().to_owned())
            .collect();
        enroll_shares(&mut manifest, settings);
        // Files about to hold a key take their safe additions in place too.
        let at = chrono::Utc::now().timestamp();
        manifest
            .keys
            .intents
            .extend(keys.iter().cloned().map(|mut share| {
                share.at = at;
                share
            }));
        self.save(&manifest)?;
        if !destinations.is_empty() {
            let mut written = report("");
            self.sync_personal_settings(&root, &mut manifest, &mut written)?;
            result.written.extend(written.written);
            for (line, code) in written.blocked.into_iter().zip(written.blocked_codes) {
                if destinations
                    .iter()
                    .any(|path| line.starts_with(&format!("~/{}", path.display())))
                {
                    result.blocked.push(line);
                    result.blocked_codes.push(code);
                }
            }
        }
        if !keys.is_empty() {
            manifest.keys.intents.clear();
            manifest.keys.notes.clear();
            self.apply_key_shares(&root, &mut manifest, keys, &mut result)?;
        }
        if let Some(left) = left {
            result.message = format!("{}.\n{left}", result.message);
        }
        Ok(result)
    }

    fn plan_share(
        &self,
        root: &FileRoot,
        manifest: &Manifest,
        render_skills: &impl Fn(&str, &SkillPackages) -> Result<SkillFiles>,
    ) -> Result<Share> {
        let enrolled: Vec<&'static PersonalTarget> = PERSONAL_TARGETS
            .iter()
            .filter(|target| {
                manifest.master.overlays.contains_key(target.id)
                    && self.home.join(target.folder()).is_dir()
            })
            .collect();
        let mut listing = Listing::default();
        let mut operations = Vec::new();
        let skills = self.plan_skill_shares(
            root,
            manifest,
            &enrolled,
            render_skills,
            &mut listing,
            &mut operations,
        )?;
        self.plan_file_shares(root, manifest, &enrolled, &mut listing, &mut operations)?;
        let mut settings = self.plan_setting_shares(root, manifest)?;
        listing.already_there += settings.already_there + settings.keys.already_there;
        for (tool, item, reason) in settings
            .stays
            .into_iter()
            .chain(std::mem::take(&mut settings.keys.stays))
        {
            listing.stay(tool, item, reason);
        }
        for addition in &settings.additions {
            listing
                .tool(addition.to_tool)
                .mcp_servers
                .push(addition.name.clone());
        }
        let mut worktrees = DeliveryWorktrees::new(&self.home)?;
        let mut worktree_writes = BTreeMap::new();
        for op in &operations {
            if let Some(git) = worktrees.at(&op.path)? {
                worktree_writes.insert(op.path.clone(), git);
            }
        }
        for tool in listing.tools.values_mut() {
            for names in [
                &mut tool.skills,
                &mut tool.commands,
                &mut tool.subagents,
                &mut tool.mcp_servers,
            ] {
                names.sort();
                names.dedup();
            }
        }
        // The key group enters the token by place and name only.
        let token = fingerprint(&(
            SHARE_TOKEN_VERSION,
            &operations,
            &skills,
            &settings.additions,
            &settings.keys.shares,
            &worktree_writes,
            &listing.stays,
            listing.already_there,
        ))?;
        Ok(Share {
            operations,
            skills,
            settings: settings.additions,
            worktree_writes,
            plan: SharingPlan {
                plan_id: adoption_plan_id(&token),
                token,
                tools: listing.tools.into_values().collect(),
                stays: listing.stays,
                already_there: listing.already_there,
                key_servers: settings.keys.servers(),
                key_tools: settings.keys.tools.clone(),
            },
            keys: settings.keys.shares,
        })
    }

    fn plan_skill_shares(
        &self,
        root: &FileRoot,
        manifest: &Manifest,
        enrolled: &[&'static PersonalTarget],
        render_skills: &impl Fn(&str, &SkillPackages) -> Result<SkillFiles>,
        listing: &mut Listing,
        operations: &mut Vec<Pending>,
    ) -> Result<Vec<SkillAddition>> {
        let saved = rulesync_registry::saved_skill_deliveries(&self.data)?;
        let saved_name = |target: &PersonalTarget, name: &str| {
            saved
                .names
                .get(&target.tool)
                .is_some_and(|names| names.contains(name))
        };
        // (target, destination folder) -> candidate source skills
        let mut wanted = BTreeMap::<(&'static str, PathBuf), Vec<(usize, &str, PathBuf)>>::new();
        for (index, skill) in manifest.master.skills.iter().enumerate() {
            let Some((source_target, source)) =
                skill.locations.iter().find_map(|(target, folders)| {
                    folders
                        .iter()
                        .find(|folder| !is_harness_path(folder))
                        .map(|folder| (target.as_str(), folder.clone()))
                })
            else {
                continue;
            };
            let Some(source_spec) = personal_target(source_target) else {
                continue;
            };
            if saved_name(source_spec, &skill.name)
                || source
                    .ancestors()
                    .any(|folder| saved.folders.contains_key(folder))
            {
                continue;
            }
            let Some(name) = source.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let missing: Vec<_> = enrolled
                .iter()
                .filter(|target| !skill.locations.contains_key(target.id))
                .collect();
            if missing.is_empty() {
                continue;
            }
            let item = format!("skill {name}");
            if !plain_folder_name(name) {
                listing.stay(
                    source_target,
                    item,
                    "its folder name is not plain ASCII, so its case cannot be checked safely",
                );
                continue;
            }
            let in_sync = manifest.active_deliveries().all(|(path, delivery)| {
                !path.starts_with(&source)
                    || delivery.observed.is_none()
                        && root
                            .read(path, false)
                            .is_ok_and(|current| current == delivery.delivered)
            });
            if !in_sync {
                listing.stay(
                    source_target,
                    item,
                    format!("it changed since the last sync. Run `{SYNC_COMMAND}` first"),
                );
                continue;
            }
            if possible_credentials(skill.files.values().map(Vec::as_slice)) {
                listing.stay(source_target, item, "a file in it may contain credentials");
                continue;
            }
            for target in missing {
                wanted
                    .entry((target.id, Path::new(target.skills.destination).join(name)))
                    .or_default()
                    .push((index, source_target, source.clone()));
            }
        }
        let mut rendered = BTreeMap::<(usize, &str), Result<SkillFiles, String>>::new();
        for target in enrolled {
            let indices: BTreeSet<_> = wanted
                .iter()
                .filter(|((id, _), _)| *id == target.id)
                .flat_map(|(_, sources)| sources.iter().map(|(index, ..)| *index))
                .collect();
            if indices.is_empty() {
                continue;
            }
            let sources = indices.iter().map(|&index| {
                let skill = &manifest.master.skills[index];
                (skill.name.as_str(), &skill.files)
            });
            match render_personal_skills(target, sources, render_skills) {
                Ok(deliveries) => {
                    for (index, delivery) in indices.into_iter().zip(deliveries) {
                        rendered.insert((index, target.id), Ok(delivery.files));
                    }
                }
                Err(error) => {
                    for index in indices {
                        rendered.insert((index, target.id), Err(format!("{error:#}")));
                    }
                }
            }
        }
        let mut additions = Vec::new();
        for ((target_id, folder), sources) in wanted {
            let target = personal_target(target_id).context("unknown personal skill target")?;
            let name = folder
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_owned();
            let item = format!("skill {name}");
            let tool = display(target_id);
            let mut files = None;
            let mut clash = false;
            for (index, source_target, _) in &sources {
                match &rendered[&(*index, target_id)] {
                    Ok(output) => match &files {
                        None => files = Some(output.clone()),
                        Some(first) if first == output => (),
                        Some(_) => clash = true,
                    },
                    Err(_) => listing.stay(
                        source_target,
                        item.clone(),
                        format!("it could not be converted for {tool}"),
                    ),
                }
            }
            let Some(files) = files else {
                continue;
            };
            let stays_for = |listing: &mut Listing, reason: String| {
                for (_, source_target, _) in &sources {
                    listing.stay(source_target, item.clone(), reason.clone());
                }
            };
            if clash {
                stays_for(
                    listing,
                    format!("another tool has a different skill with this name for {tool}"),
                );
                continue;
            }
            if path_blocked(&manifest.unmanaged_skills, &folder)
                || saved_name(target, &name)
                || folder
                    .ancestors()
                    .any(|ancestor| saved.folders.contains_key(ancestor))
            {
                stays_for(
                    listing,
                    format!("{tool} has its own skill with this name that Wenlan does not manage"),
                );
                continue;
            }
            if write_pending(manifest, &folder) {
                stays_for(listing, format!("a setup write is pending in {tool}"));
                continue;
            }
            if !matches!(manifest.case_alias(root, &folder), Ok(None)) {
                stays_for(
                    listing,
                    format!("its name in {tool} matches another path in the setup by case"),
                );
                continue;
            }
            let existing = root.read(&folder, false)?;
            if existing != Snapshot::Absent {
                if existing.is_directory() && disk_tree(root, &folder)?.as_ref() == Some(&files) {
                    listing.already_there += 1;
                } else {
                    stays_for(
                        listing,
                        format!("{tool} already has a different skill with this name"),
                    );
                }
                continue;
            }
            if owned_by_setup(root, manifest, &folder)? {
                stays_for(
                    listing,
                    format!("Wenlan still keeps a restore copy for this name in {tool}"),
                );
                continue;
            }
            let (index, _, source) = &sources[0];
            let source_mode = |relative: &Path| {
                manifest
                    .delivered
                    .get(&source.join(relative))
                    .or_else(|| manifest.delivered.get(&source.join("SKILL.md")))
                    .and_then(|delivery| delivery.delivered.file_mode())
            };
            for (relative, bytes) in files {
                let desired = match source_mode(&relative) {
                    Some(mode) => Snapshot::file_with_permissions(bytes, mode),
                    None => Snapshot::file(bytes),
                };
                operations.push(Pending {
                    path: folder.join(&relative),
                    seen: Snapshot::Absent,
                    desired,
                    kind: Some(Kind::Skill),
                    allow_link: false,
                });
            }
            listing.tool(target_id).skills.push(name);
            additions.push(SkillAddition {
                index: *index,
                target: target.id,
                folder,
            });
        }
        Ok(additions)
    }

    fn plan_file_shares(
        &self,
        root: &FileRoot,
        manifest: &Manifest,
        enrolled: &[&'static PersonalTarget],
        listing: &mut Listing,
        operations: &mut Vec<Pending>,
    ) -> Result<()> {
        // destination -> (source tool, item, bytes, mode)
        let mut wanted = BTreeMap::<PathBuf, Vec<(&'static str, String, Snapshot)>>::new();
        for (path, delivery) in manifest.active_deliveries() {
            if !matches!(delivery.kind, Kind::PersonalFile) {
                continue;
            }
            let Some(spec) = location_of(path) else {
                continue;
            };
            let Some(name) = spec.name_at(path) else {
                continue;
            };
            let noun = match spec.feature {
                PersonalFeature::Commands => "command",
                PersonalFeature::Subagents => "subagent",
            };
            let item = format!("{noun} {}", name.display());
            let current = root.read_personal_file(path, false);
            let (Ok(current), None) = (current, &delivery.observed) else {
                listing.stay(
                    spec.tool,
                    item,
                    format!("it changed since the last sync. Run `{SYNC_COMMAND}` first"),
                );
                continue;
            };
            if current != delivery.delivered {
                listing.stay(
                    spec.tool,
                    item,
                    format!("it changed since the last sync. Run `{SYNC_COMMAND}` first"),
                );
                continue;
            }
            let Some(bytes) = file_bytes(&current) else {
                continue;
            };
            let (destinations, skipped) = sharing_paths(path, bytes, &manifest.master.overlays);
            let mut reasons = BTreeMap::<&str, Vec<&str>>::new();
            for (tool, reason) in &skipped {
                if *reason != NO_PERSONAL_DESTINATION {
                    reasons
                        .entry(reason.trim_start_matches("Left unmanaged: "))
                        .or_default()
                        .push(display(tool));
                }
            }
            for (reason, tools) in reasons {
                listing.stay(
                    spec.tool,
                    item.clone(),
                    format!("{reason} Not copied to {}.", tools.join(", ")),
                );
            }
            let destinations: Vec<_> = destinations
                .into_iter()
                .filter(|destination| destination != path)
                .filter(|destination| {
                    location_of(destination).is_some_and(|target| {
                        enrolled.iter().any(|enrolled| enrolled.id == target.tool)
                    })
                })
                .collect();
            if destinations.is_empty() {
                continue;
            }
            if possible_credentials([bytes]) {
                listing.stay(spec.tool, item, "the file may contain credentials");
                continue;
            }
            for preferred in destinations {
                let target_tool = location_of(&preferred).map_or("", |target| target.tool);
                match choose_destination(root, manifest, &preferred) {
                    Ok(destination) => wanted.entry(destination).or_default().push((
                        spec.tool,
                        item.clone(),
                        current.clone(),
                    )),
                    Err(reason) => listing.stay(
                        spec.tool,
                        item.clone(),
                        format!("{reason} Not copied to {}.", display(target_tool)),
                    ),
                }
            }
        }
        for (destination, sources) in wanted {
            let Some(target) = location_of(&destination) else {
                continue;
            };
            let tool = display(target.tool);
            let stays_for = |listing: &mut Listing, reason: String| {
                for (source_tool, item, _) in &sources {
                    listing.stay(source_tool, item.clone(), reason.clone());
                }
            };
            let (_, _, first) = &sources[0];
            let bytes = file_bytes(first).unwrap_or_default();
            if sources
                .iter()
                .any(|(_, _, other)| file_bytes(other) != Some(bytes))
            {
                stays_for(
                    listing,
                    format!("another tool has a different file with this name for {tool}"),
                );
                continue;
            }
            if let Some(reason) = destination
                .ancestors()
                .find_map(|ancestor| manifest.unmanaged_personal_files.get(ancestor))
            {
                stays_for(
                    listing,
                    format!(
                        "{tool} has its own copy that adoption left unmanaged because {reason}"
                    ),
                );
                continue;
            }
            if write_pending(manifest, &destination) {
                stays_for(listing, format!("a setup write is pending in {tool}"));
                continue;
            }
            let existing = match root.read_personal_file(&destination, false) {
                Ok(existing) => existing,
                Err(error) => {
                    stays_for(
                        listing,
                        format!("{tool} has a path Wenlan cannot use: {error}"),
                    );
                    continue;
                }
            };
            if let Some(delivery) = manifest.delivered.get(&destination) {
                if matches!(delivery.kind, Kind::PersonalFile)
                    && file_bytes(&delivery.delivered) == Some(bytes)
                    && existing == delivery.delivered
                {
                    listing.already_there += 1;
                } else {
                    stays_for(listing, format!("{tool} has its own version of this name"));
                }
                continue;
            }
            if existing != Snapshot::Absent {
                if file_bytes(&existing) == Some(bytes) {
                    listing.already_there += 1;
                } else {
                    stays_for(
                        listing,
                        format!("{tool} already has a different file with this name"),
                    );
                }
                continue;
            }
            if owned_by_setup(root, manifest, &destination)? {
                stays_for(
                    listing,
                    format!("Wenlan still keeps a restore copy for this name in {tool}"),
                );
                continue;
            }
            let name = target
                .name_at(&destination)
                .map(|name| name.display().to_string())
                .unwrap_or_default();
            match target.feature {
                PersonalFeature::Commands => listing.tool(target.tool).commands.push(name),
                PersonalFeature::Subagents => listing.tool(target.tool).subagents.push(name),
            }
            operations.push(Pending {
                path: destination,
                seen: Snapshot::Absent,
                desired: first.clone(),
                kind: Some(Kind::PersonalFile),
                allow_link: false,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rulesync_routes::tests::{fixture_rule_renderer, fixture_skill_renderer, Fixture};

    const SKILL: &str = "---\nname: codex-only\ndescription: A fixture skill.\n---\nUse it.\n";
    const COMMAND: &str = "---\ndescription: Greet.\n---\nSay hello.\n";
    const CODEX_CONFIG: &str = "# keep comment\n[mcp_servers.safe]\ncommand = 'safe-server'\n\n[mcp_servers.held]\nurl = 'https://example.invalid/mcp'\nhttp_headers = { 'X-Custom' = 'synthetic-sentinel-header' }\n";

    /// Paths with their bytes (none for a folder) and permissions.
    type Tree = BTreeMap<PathBuf, (Option<Vec<u8>>, u32)>;

    /// Every path under the home with its bytes and permissions.
    fn tree(home: &Path) -> Tree {
        fn walk(home: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, (Option<Vec<u8>>, u32)>) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                let metadata = fs::symlink_metadata(&path).unwrap();
                #[cfg(unix)]
                let mode = std::os::unix::fs::PermissionsExt::mode(&metadata.permissions());
                #[cfg(not(unix))]
                let mode = u32::from(metadata.permissions().readonly());
                let relative = path.strip_prefix(home).unwrap().to_owned();
                if metadata.is_dir() {
                    out.insert(relative, (None, mode));
                    walk(home, &path, out);
                } else {
                    out.insert(relative, (Some(fs::read(&path).unwrap()), mode));
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(home, home, &mut out);
        out
    }

    fn write(f: &Fixture, files: &[(&str, &str)]) {
        for (path, text) in files {
            let path = f.setup.home.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
    }

    fn adopt(f: &Fixture) {
        let preview = f
            .setup
            .preview_adoption_using(fixture_rule_renderer, fixture_skill_renderer)
            .unwrap();
        f.setup
            .adopt_using(
                &preview.token,
                fixture_rule_renderer,
                fixture_skill_renderer,
            )
            .unwrap();
    }

    fn tool<'a>(plan: &'a SharingPlan, name: &str) -> &'a SharingTool {
        plan.tools
            .iter()
            .find(|tool| tool.tool == name)
            .unwrap_or_else(|| panic!("{name}: {plan:?}"))
    }

    #[test]
    fn rulesync_share_offers_each_item_to_every_supporting_tool_and_restores() {
        let f = Fixture::new();
        let home = f.setup.home.clone();
        write(
            &f,
            &[
                (".codex/skills/codex-only/SKILL.md", SKILL),
                (".claude/commands/hello.md", COMMAND),
                (".codex/config.toml", CODEX_CONFIG),
                (".claude.json", "{ \"mcpServers\": {} }\n"),
            ],
        );
        let before = tree(&home);
        adopt(&f);
        // The held server's refusal notice is the same before and after sharing.
        let baseline = f.sync_with("");
        let adopted = tree(&home);

        let plan = f.setup.preview_share_using(fixture_skill_renderer).unwrap();
        assert_eq!(plan.plan_id, adoption_plan_id(&plan.token));
        let claude = tool(&plan, "Claude Code");
        assert_eq!(claude.skills, ["codex-only"]);
        assert_eq!(claude.mcp_servers, ["safe"]);
        assert!(claude.commands.is_empty() && claude.subagents.is_empty());
        let codex = tool(&plan, "Codex");
        assert_eq!(codex.commands, ["hello"]);
        assert!(codex.skills.is_empty() && codex.mcp_servers.is_empty());
        assert_eq!(plan.tools.len(), 2, "{plan:?}");
        // The held server may contain a key: the preview names it and the tools it
        // would reach, never its value.
        assert_eq!(plan.key_servers, ["held"]);
        assert_eq!(plan.key_tools, ["Claude Code"]);
        assert!(plan.stays.is_empty(), "{plan:?}");
        let shown = serde_json::to_string(&plan).unwrap();
        assert!(!shown.contains("synthetic-sentinel"));
        assert_eq!(tree(&home), adopted, "a preview writes nothing");

        let stale = f.setup.share_using("stale", fixture_skill_renderer);
        assert!(stale.unwrap_err().to_string().contains("--dry-run"));
        assert_eq!(tree(&home), adopted);

        let shared = f
            .setup
            .share_using(&plan.token, fixture_skill_renderer)
            .unwrap();
        assert!(shared.blocked.is_empty(), "{shared:?}");
        assert_eq!(
            fs::read_to_string(home.join(".claude/skills/codex-only/SKILL.md")).unwrap(),
            SKILL
        );
        assert_eq!(
            fs::read_to_string(home.join(".codex/prompts/hello.md")).unwrap(),
            COMMAND
        );
        let claude_json = fs::read_to_string(home.join(".claude.json")).unwrap();
        assert!(claude_json.contains("safe-server"), "{claude_json}");
        assert!(!claude_json.contains("synthetic-sentinel"));
        assert_eq!(
            fs::read_to_string(home.join(".codex/config.toml")).unwrap(),
            CODEX_CONFIG
        );

        let after = tree(&home);
        let synced = f.sync_with("");
        assert!(
            synced.proposals.is_empty()
                && synced.written.is_empty()
                && synced.blocked == baseline.blocked,
            "{synced:?}"
        );
        assert_eq!(tree(&home), after, "a sync keeps the shared copies");
        let again = f.setup.preview_share_using(fixture_skill_renderer).unwrap();
        assert!(again.tools.is_empty(), "{again:?}");
        assert!(again.already_there >= 3, "{again:?}");
        let rerun = f
            .setup
            .share_using(&again.token, fixture_skill_renderer)
            .unwrap();
        assert!(
            rerun.message.starts_with(NOTHING_TO_SHARE)
                && rerun
                    .message
                    .contains("Left 1 server that may contain a key"),
            "{rerun:?}"
        );
        assert_eq!(tree(&home), after);

        let status = f.setup.status().unwrap();
        let counts = |id: &str| {
            status
                .tools
                .iter()
                .find(|row| row.tool.id == id)
                .unwrap()
                .counts
                .clone()
        };
        assert_eq!(counts("claude-code").skills, 1);
        assert_eq!(counts("claude-code").mcp, 1);
        assert_eq!(counts("codex").commands, 1);

        let restored = f.setup.restore().unwrap();
        assert!(restored.blocked.is_empty(), "{restored:?}");
        assert_eq!(tree(&home), before);
    }

    #[test]
    fn rulesync_share_leaves_a_different_item_with_the_same_name() {
        let f = Fixture::new();
        let home = f.setup.home.clone();
        let codex_own = "---\ndescription: Codex.\n---\nSomething else.\n";
        write(
            &f,
            &[
                (".claude/commands/hello.md", COMMAND),
                (".codex/prompts/hello.md", codex_own),
                (".claude/commands/args.md", "Run with $ARGUMENTS.\n"),
            ],
        );
        adopt(&f);
        let adopted = tree(&home);
        let plan = f.setup.preview_share_using(fixture_skill_renderer).unwrap();
        assert!(plan.tools.is_empty(), "{plan:?}");
        assert!(
            plan.stays
                .iter()
                .any(|stay| stay.item == "command hello" && stay.reason.contains("Codex")),
            "{plan:?}"
        );
        assert!(
            plan.stays.iter().any(|stay| stay.tool == "Claude Code"
                && stay.item == "command args"
                && stay.reason.contains("'$'")),
            "{plan:?}"
        );
        let report = f
            .setup
            .share_using(&plan.token, fixture_skill_renderer)
            .unwrap();
        assert_eq!(report.message, NOTHING_TO_SHARE);
        assert_eq!(tree(&home), adopted);
        assert_eq!(
            fs::read_to_string(home.join(".codex/prompts/hello.md")).unwrap(),
            codex_own
        );
    }

    /// Whether any file under a folder holds these bytes.
    fn holds(dir: &Path, needle: &[u8]) -> bool {
        tree(dir).into_iter().any(|(_, (bytes, _))| {
            bytes.is_some_and(|bytes| bytes.windows(needle.len()).any(|part| part == needle))
        })
    }

    #[test]
    fn rulesync_share_keeps_a_codex_server_with_a_key_in_a_name_field() {
        const SECRET: &str = "ghp_shareFixtureSentinel0123456789";
        let f = Fixture::new();
        let home = f.setup.home.clone();
        let codex = format!(
            "[mcp_servers.named]\nurl = 'https://example.invalid/mcp'\nbearer_token_env_var = '{SECRET}'\n\n\
             [mcp_servers.header]\nurl = 'https://example.invalid/mcp'\nenv_http_headers = {{ 'X-Team' = '{SECRET}' }}\n"
        );
        let claude = "{ \"mcpServers\": {} }\n";
        write(
            &f,
            &[(".codex/config.toml", &codex), (".claude.json", claude)],
        );
        adopt(&f);
        f.sync_with("");
        let plan = f.setup.preview_share_using(fixture_skill_renderer).unwrap();
        assert!(plan.tools.is_empty(), "{plan:?}");
        assert!(!serde_json::to_string(&plan).unwrap().contains(SECRET));
        f.setup
            .share_using(&plan.token, fixture_skill_renderer)
            .unwrap();
        f.sync_with("");
        assert_eq!(
            fs::read_to_string(home.join(".claude.json")).unwrap(),
            claude
        );
        assert_eq!(
            fs::read_to_string(home.join(".codex/config.toml")).unwrap(),
            codex
        );
        assert!(!holds(&f.setup.data, SECRET.as_bytes()));
    }

    /// Adopt and share everything, then check that no file under the home
    /// changed and that nothing under the data folder holds `secret`.
    fn share_keeps_out(f: &Fixture, files: &[(&str, &str)], secret: &str) -> SharingPlan {
        write(f, files);
        adopt(f);
        f.sync_with("");
        let plan = f.setup.preview_share_using(fixture_skill_renderer).unwrap();
        assert!(!serde_json::to_string(&plan).unwrap().contains(secret));
        if !plan.tools.is_empty() {
            f.setup
                .share_using(&plan.token, fixture_skill_renderer)
                .unwrap();
            f.sync_with("");
        }
        assert!(!holds(&f.setup.data, secret.as_bytes()));
        plan
    }

    #[test]
    fn rulesync_share_never_copies_a_server_url_with_a_key_in_its_path() {
        const SECRET: &str = "3f9a8b7c6d5e4f3a2b1c0d9e8f7a6b5c";
        let f = Fixture::new();
        let home = f.setup.home.clone();
        let codex =
            format!("[mcp_servers.remote]\nurl = 'https://mcp.example.invalid/s/{SECRET}/mcp'\n");
        let claude = "{ \"mcpServers\": {} }\n";
        let plan = share_keeps_out(
            &f,
            &[(".codex/config.toml", &codex), (".claude.json", claude)],
            SECRET,
        );
        assert!(plan.tools.is_empty(), "{plan:?}");
        assert_eq!(
            fs::read_to_string(home.join(".claude.json")).unwrap(),
            claude
        );
        assert_eq!(
            fs::read_to_string(home.join(".codex/config.toml")).unwrap(),
            codex
        );
    }

    #[test]
    fn rulesync_share_stores_no_password_from_a_permission_rule() {
        let f = Fixture::new();
        let home = f.setup.home.clone();
        let claude = "{\"permissions\":{\"allow\":[\"Bash(mysql -uroot -phunter2permpw db)\",\"Bash(git status)\"]}}\n";
        let rules = "prefix_rule(pattern = [\"sshpass\", \"-p\", \"hunter2rulepw\"], decision = \"allow\")\nprefix_rule(pattern = [\"git\", \"status\"], decision = \"allow\")\n";
        write(&f, &[(".codex/rules/default.rules", rules)]);
        share_keeps_out(&f, &[(".claude/settings.json", claude)], "hunter2permpw");
        assert!(!holds(&f.setup.data, b"hunter2rulepw"));
        for (path, bytes) in [
            (".claude/settings.json", claude),
            (".codex/rules/default.rules", rules),
        ] {
            let now = fs::read_to_string(home.join(path)).unwrap();
            assert!(now.contains(bytes.trim_end()), "{path}: {now}");
        }
    }

    #[test]
    fn rulesync_share_backs_up_no_codex_file_with_a_literal_password() {
        const SECRET: &str = "hunter2codexpw";
        let f = Fixture::new();
        let home = f.setup.home.clone();
        let codex = format!(
            "[mcp_servers.safe]\ncommand = 'safe-server'\n\n[shell_environment_policy]\nset = {{ DB_PASS = '{SECRET}' }}\n"
        );
        share_keeps_out(
            &f,
            &[(".codex/config.toml", &codex), (".claude.json", "{}\n")],
            SECRET,
        );
        assert_eq!(
            fs::read_to_string(home.join(".codex/config.toml")).unwrap(),
            codex
        );
        let blobs = f.setup.data.join("personal-blobs");
        assert!(!blobs.exists() || !holds(&blobs, SECRET.as_bytes()));
    }

    #[test]
    fn rulesync_status_counts_mcp_servers_adopted_in_place() {
        let f = Fixture::new();
        write(
            &f,
            &[(
                ".codex/config.toml",
                "[mcp_servers.safe]\ncommand = 'safe-server'\n",
            )],
        );
        adopt(&f);
        let status = f.setup.status().unwrap();
        let codex = status
            .tools
            .iter()
            .find(|row| row.tool.id == "codex")
            .unwrap();
        assert_eq!(codex.counts.mcp, 1, "{status:?}");
        assert_ne!(codex.state, RulesToolState::NotManaged);
    }

    const KEY_SECRET: &str = "sk-keyFixtureSentinel0123456789abcdef";
    const EMPTY_CLAUDE: &str = "{ \"mcpServers\": {} }\n";

    fn keyed_codex() -> String {
        format!(
            "[mcp_servers.safe]\ncommand = 'safe-server'\n\n\
             [mcp_servers.keyed]\ncommand = 'keyed-server'\nenv = {{ 'API_TOKEN' = '{KEY_SECRET}' }}\n"
        )
    }

    /// A Codex file with one safe server and one that may hold a key, and an
    /// empty Claude Code file. Returns the home before adoption.
    fn key_fixture() -> (Fixture, Tree) {
        let f = Fixture::new();
        write(
            &f,
            &[
                (".codex/config.toml", &keyed_codex()),
                (".claude.json", EMPTY_CLAUDE),
            ],
        );
        let before = tree(&f.setup.home);
        adopt(&f);
        (f, before)
    }

    fn share_keys(f: &Fixture, answer: KeyAnswer) -> RulesSyncReport {
        let plan = f.setup.preview_share_using(fixture_skill_renderer).unwrap();
        f.setup
            .share_with_keys(&plan.token, answer, fixture_skill_renderer)
            .unwrap()
    }

    fn data_holds_key(f: &Fixture) -> bool {
        f.setup.data.exists() && holds(&f.setup.data, KEY_SECRET.as_bytes())
    }

    #[test]
    fn rulesync_share_previews_key_servers_by_name_and_tool_only() {
        let (f, _) = key_fixture();
        let plan = f.setup.preview_share_using(fixture_skill_renderer).unwrap();
        assert_eq!(plan.key_servers, ["keyed"]);
        assert_eq!(plan.key_tools, ["Claude Code"]);
        assert_eq!(tool(&plan, "Claude Code").mcp_servers, ["safe"]);
        let shown = serde_json::to_string(&plan).unwrap();
        assert!(
            !shown.contains(KEY_SECRET) && !shown.contains("API_TOKEN"),
            "{shown}"
        );
        assert!(!data_holds_key(&f));
    }

    #[test]
    fn rulesync_share_copies_a_key_server_on_yes_and_restores_without_storing_it() {
        let (f, before) = key_fixture();
        let claude = f.setup.home.join(".claude.json");
        let shared = share_keys(&f, KeyAnswer::Yes);
        assert!(shared.blocked.is_empty(), "{shared:?}");
        let text = fs::read_to_string(&claude).unwrap();
        assert!(
            text.contains("safe-server") && text.contains("keyed-server"),
            "{text}"
        );
        assert!(text.contains(KEY_SECRET));
        assert!(!data_holds_key(&f), "the key reached the data folder");

        let after = tree(&f.setup.home);
        let synced = f.sync_with("");
        assert!(
            !synced
                .blocked
                .iter()
                .any(|line| line.contains(".claude.json")),
            "{synced:?}"
        );
        assert_eq!(tree(&f.setup.home), after, "a sync keeps the copy");
        assert!(!data_holds_key(&f));
        // Nothing is left to share, and the key server is not offered again.
        let again = f.setup.preview_share_using(fixture_skill_renderer).unwrap();
        assert!(
            again.key_servers.is_empty() && again.tools.is_empty(),
            "{again:?}"
        );

        let restored = f.setup.restore().unwrap();
        assert!(restored.blocked.is_empty(), "{restored:?}");
        assert_eq!(tree(&f.setup.home), before);
        assert!(!data_holds_key(&f));
    }

    #[test]
    fn rulesync_share_leaves_key_servers_on_no_and_shares_the_rest() {
        let (f, _) = key_fixture();
        let claude = f.setup.home.join(".claude.json");
        let shared = share_keys(&f, KeyAnswer::No);
        assert!(shared.blocked.is_empty(), "{shared:?}");
        assert!(
            shared
                .message
                .ends_with("Left 1 server that may contain a key where they are."),
            "{shared:?}"
        );
        let text = fs::read_to_string(&claude).unwrap();
        assert!(text.contains("safe-server"), "{text}");
        assert!(!text.contains("keyed-server") && !text.contains(KEY_SECRET));
        assert_eq!(
            fs::read_to_string(f.setup.home.join(".codex/config.toml")).unwrap(),
            keyed_codex()
        );
        // The key server is still there to share later.
        let again = f.setup.preview_share_using(fixture_skill_renderer).unwrap();
        assert_eq!(again.key_servers, ["keyed"]);
    }

    #[test]
    fn rulesync_share_refuses_key_servers_for_an_ai_caller() {
        use axum::http::{HeaderMap, HeaderValue};
        let request = |confirmed: Option<&str>| AdoptRequest {
            confirmed_plan_id: confirmed.map(str::to_owned),
            token: "token".into(),
            home: None,
            include_keys: true,
        };
        let mut session = HeaderMap::new();
        session.insert(wenlan_types::SESSION_HEADER, HeaderValue::from_static("s1"));
        let mut agent = HeaderMap::new();
        agent.insert("x-agent-name", HeaderValue::from_static("codex"));
        assert!(super::super::ai_caller(&session) && super::super::ai_caller(&agent));
        assert!(!super::super::ai_caller(&HeaderMap::new()));
        assert_eq!(KeyAnswer::of(true, &request(None)), KeyAnswer::NotAsked);
        assert_eq!(
            KeyAnswer::of(false, &request(Some("0123456789ab"))),
            KeyAnswer::NotAsked
        );
        assert_eq!(KeyAnswer::of(false, &request(None)), KeyAnswer::Yes);

        let (f, _) = key_fixture();
        let shared = share_keys(&f, KeyAnswer::NotAsked);
        assert!(
            shared.message.contains(
                "Left 1 server that may contain a key where they are. Only the person can share those, by running `wenlan rules share` in their own terminal."
            ),
            "{shared:?}"
        );
        let text = fs::read_to_string(f.setup.home.join(".claude.json")).unwrap();
        assert!(text.contains("safe-server"));
        assert!(!text.contains("keyed-server") && !text.contains(KEY_SECRET));
    }

    #[test]
    fn rulesync_restore_keeps_a_key_copy_the_person_changed() {
        let (f, before) = key_fixture();
        let claude = f.setup.home.join(".claude.json");
        let plan = f.setup.preview_share_using(fixture_skill_renderer).unwrap();
        // Share the safe server alone first to know the bytes without the copy.
        f.setup
            .share_with_keys(&plan.token, KeyAnswer::No, fixture_skill_renderer)
            .unwrap();
        let without_copy = fs::read(&claude).unwrap();
        share_keys(&f, KeyAnswer::Yes);
        let edited = fs::read_to_string(&claude)
            .unwrap()
            .replace("\"keyed-server\"", "\"keyed-server-edited\"");
        assert!(edited.contains("keyed-server-edited"));
        fs::write(&claude, &edited).unwrap();

        let restored = f.setup.restore().unwrap();
        assert!(
            restored.message.contains(
                "kept keyed in Claude Code: changed since sharing; remove it by hand if you don't want it"
            ),
            "{restored:?}"
        );
        // The kept copy sits next to the safe server Wenlan added, so that
        // entry's recorded surroundings no longer match: it stays, with a
        // plain way out, and nothing else in the file changes.
        assert!(
            restored
                .blocked
                .iter()
                .any(|line| line.starts_with(".claude.json")
                    && line.contains(
                        "Remove the kept server, then run `wenlan rules restore` again."
                    )),
            "{restored:?}"
        );
        assert_eq!(fs::read_to_string(&claude).unwrap(), edited);
        assert!(!data_holds_key(&f));

        // Once the person removes the kept copy, restore finishes.
        fs::write(&claude, &without_copy).unwrap();
        let restored = f.setup.restore().unwrap();
        assert!(restored.blocked.is_empty(), "{restored:?}");
        assert_eq!(tree(&f.setup.home), before);
        assert!(!data_holds_key(&f));
    }

    #[test]
    fn rulesync_interrupted_key_share_is_claimed_when_the_copy_landed() {
        let (f, before) = key_fixture();
        share_keys(&f, KeyAnswer::Yes);
        // A crash after the write and before the receipt was saved.
        let mut manifest = f.setup.load().unwrap().unwrap();
        manifest.keys.intents = std::mem::take(&mut manifest.keys.shares);
        f.setup.save(&manifest).unwrap();
        assert!(f
            .setup
            .status()
            .unwrap()
            .notes
            .iter()
            .any(|note| note.contains("was interrupted")));

        // Restore claims the copy first, so it can take it out.
        let restored = f.setup.restore().unwrap();
        assert!(restored.blocked.is_empty(), "{restored:?}");
        assert_eq!(tree(&f.setup.home), before);
        assert!(!data_holds_key(&f));
    }

    #[test]
    fn rulesync_interrupted_key_share_is_dropped_when_the_copy_is_not_there() {
        let (f, _) = key_fixture();
        let claude = f.setup.home.join(".claude.json");
        let root = FileRoot::open_home(&f.setup.home).unwrap();
        let mut manifest = f.setup.load().unwrap().unwrap();
        let keys = f
            .setup
            .plan_share(&root, &manifest, &fixture_skill_renderer)
            .unwrap()
            .keys;
        assert_eq!(keys.len(), 1);
        // A crash after the intent was saved and before the write; then the
        // person added their own server with that name.
        manifest.keys.intents = keys;
        f.setup.save(&manifest).unwrap();
        let own = "{ \"mcpServers\": {\"keyed\":{\"command\":\"my-own\"}} }\n";
        fs::write(&claude, own).unwrap();

        let mut manifest = f.setup.load().unwrap().unwrap();
        f.setup.claim_key_intents(&root, &mut manifest).unwrap();
        assert!(manifest.keys.intents.is_empty() && manifest.keys.shares.is_empty());
        let status = f.setup.status().unwrap();
        assert!(
            status.notes.contains(
                &"Left keyed in Claude Code as it is: an earlier share was interrupted.".to_owned()
            ),
            "{status:?}"
        );
        assert_eq!(fs::read_to_string(&claude).unwrap(), own);
        f.setup.restore().unwrap();
        assert_eq!(fs::read_to_string(&claude).unwrap(), own, "nothing deleted");
    }

    #[test]
    fn rulesync_safe_server_shares_syncs_and_restores_in_a_file_with_a_key() {
        let f = Fixture::new();
        let home = f.setup.home.clone();
        let claude = home.join(".claude.json");
        let codex = home.join(".codex/config.toml");
        let own = "{ \"mcpServers\": {\"mine\":{\"command\":\"mine\",\"env\":{\"API_TOKEN\":\"sk-otherFixture0123456789abcdef\"}}} }\n";
        write(
            &f,
            &[
                (
                    ".codex/config.toml",
                    "[mcp_servers.safe]\ncommand = 'safe-server'\n",
                ),
                (".claude.json", own),
            ],
        );
        let before = tree(&home);
        adopt(&f);
        let shared = share_keys(&f, KeyAnswer::No);
        assert!(shared.blocked.is_empty(), "{shared:?}");
        let text = fs::read_to_string(&claude).unwrap();
        assert!(
            text.contains("safe-server") && text.contains("sk-otherFixture"),
            "{text}"
        );

        // An edit to the shared server reaches the file that holds a key.
        fs::write(&codex, "[mcp_servers.safe]\ncommand = 'safe-server-2'\n").unwrap();
        let pull = f.sync_with("");
        let card = pull
            .proposals
            .iter()
            .find(|card| card.id.starts_with("setting-"))
            .unwrap_or_else(|| panic!("{pull:?}"));
        let reviewed = f
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
        // Only the standing notice about Claude Code's own server remains.
        assert!(
            reviewed.written.iter().any(|path| path == ".claude.json")
                && reviewed
                    .blocked
                    .iter()
                    .all(|line| line.contains("skipped MCP server (entry 1)")),
            "{reviewed:?}"
        );
        let text = fs::read_to_string(&claude).unwrap();
        assert!(text.contains("safe-server-2"), "{text}");
        assert!(!holds(&f.setup.data, b"sk-otherFixture"));

        let restored = f.setup.restore().unwrap();
        assert!(restored.blocked.is_empty(), "{restored:?}");
        assert_eq!(fs::read(&claude).unwrap(), own.as_bytes());
        assert_eq!(tree(&home), before);
    }
}
