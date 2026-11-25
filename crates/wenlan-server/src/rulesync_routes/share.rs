// SPDX-License-Identifier: Apache-2.0
//! `wenlan rules share`: offer portable rule lines, skills, commands, subagents
//! and MCP servers to the other enrolled tools that support them. The writes use the
//! same journal, backup and restore path as adding a tool to the setup.

use super::key_shares::{plan_token, KeyDigests, KeyPreview, KeyShare, EXPIRED_KEY_PREVIEW};
use super::personal_files::{choose_destination, file_bytes};
use super::personal_settings::{enroll_shares, offered_unmanaged_sources, SettingAddition};
use super::shared_source::SharedSource;
use super::*;
use wenlan_core::rulesync_personal::adoption_plan_id;
use wenlan_core::rulesync_personal_files::{sharing_paths, NO_PERSONAL_DESTINATION};
use wenlan_types::rulesync::{SharingPlan, SharingStay, SharingTool};

const SHARE_TOKEN_VERSION: &str = "share-5";
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
    rules: share_rules::RuleShares,
    /// Servers that may hold a key; written only with the person's yes.
    keys: Vec<KeyShare>,
    key_digests: KeyDigests,
    worktree_writes: BTreeMap<PathBuf, PathBuf>,
    pub(super) plan: SharingPlan,
}

impl Share {
    fn is_empty(&self) -> bool {
        self.operations.is_empty() && self.settings.is_empty() && self.rules.lines.is_empty()
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
    let place = if count == 1 { "it is" } else { "they are" };
    let line = format!("Left {count} {servers} that may contain a key where {place}.");
    if answer == KeyAnswer::NotAsked {
        let object = if count == 1 { "it" } else { "those" };
        format!(
            "{line} Only the person can share {object}, by running `wenlan rules share` in their own terminal."
        )
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
    files.into_iter().any(contains_credential_bytes)
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

    pub(super) fn preview_share(
        &self,
        expected_home: &Path,
        keep_rule: Option<&[usize]>,
        key_consent: bool,
    ) -> Result<SharingPlan> {
        self.check_adoption_home(expected_home)?;
        let _lock = self.lock()?;
        let session = RenderSession::new(&self.binary);
        self.build_share_preview(keep_rule, key_consent, |renderer, skills| {
            self.render_skills(&session, renderer, skills)
        })
    }

    #[cfg(test)]
    pub(super) fn preview_share_using(
        &self,
        render_skills: impl Fn(&str, &SkillPackages) -> Result<SkillFiles>,
    ) -> Result<SharingPlan> {
        self.build_share_preview(None, true, render_skills)
    }

    pub(super) fn build_share_preview(
        &self,
        keep_rule: Option<&[usize]>,
        key_consent: bool,
        render_skills: impl Fn(&str, &SkillPackages) -> Result<SkillFiles>,
    ) -> Result<SharingPlan> {
        let manifest = self.share_manifest()?;
        let root = self.home_files()?;
        let source = SharedSource::read(&self.home, &manifest)?;
        let Share {
            mut plan,
            key_digests,
            ..
        } = self.plan_share(&root, &manifest, &source, keep_rule, &render_skills)?;
        if key_consent {
            plan.token = KeyPreview::remember(self, plan.token, key_digests);
        }
        Ok(plan)
    }

    /// `keys` is true only for the person's own second yes; the route
    /// decides it from the request (see `execute`).
    pub(super) fn share(
        &self,
        request: &ShareRequest,
        keys: KeyAnswer,
        generated: &[String],
        person_confirmed: bool,
    ) -> Result<RulesSyncReport> {
        if let Some(home) = &request.approval.home {
            self.check_adoption_home(home)?;
        }
        let session = RenderSession::new(&self.binary);
        let layers = std::cell::OnceCell::new();
        self.apply_share(
            &request.approval.token,
            keys,
            request.keep_rule.as_deref(),
            generated,
            person_confirmed,
            (
                |target: &str, source: &str| {
                    let layers = match layers.get() {
                        Some(layers) => layers,
                        None => {
                            let settings = wenlan_core::shared_space::settings_path();
                            let loaded = rulesync_team::load_layers(&settings)?;
                            layers.get_or_init(|| loaded)
                        }
                    };
                    self.render_member_rule(&session, target, source, layers)
                },
                |renderer: &str, skills: &SkillPackages| {
                    self.render_skills(&session, renderer, skills)
                },
            ),
        )
    }

    #[cfg(test)]
    pub(super) fn share_using(
        &self,
        token: &str,
        render_skills: impl Fn(&str, &SkillPackages) -> Result<SkillFiles>,
    ) -> Result<RulesSyncReport> {
        self.share_with_keys(token, KeyAnswer::No, render_skills)
    }

    #[cfg(test)]
    pub(super) fn share_with_keys(
        &self,
        token: &str,
        answer: KeyAnswer,
        render_skills: impl Fn(&str, &SkillPackages) -> Result<SkillFiles>,
    ) -> Result<RulesSyncReport> {
        self.apply_share(
            token,
            answer,
            None,
            &[],
            true,
            (super::tests::fixture_rule_renderer, render_skills),
        )
    }

    pub(super) fn apply_share(
        &self,
        token: &str,
        answer: KeyAnswer,
        keep_rule: Option<&[usize]>,
        generated: &[String],
        person_confirmed: bool,
        (render_rule, render_skills): (
            impl Fn(&str, &str) -> Result<SkillFiles>,
            impl Fn(&str, &SkillPackages) -> Result<SkillFiles>,
        ),
    ) -> Result<RulesSyncReport> {
        let _lock = self.lock()?;
        let mut manifest = self.share_manifest()?;
        let root = self.home_files()?;
        let source = SharedSource::read(&self.home, &manifest)?;
        self.claim_key_intents(&root, &mut manifest)?;
        let share = self.plan_share(&root, &manifest, &source, keep_rule, &render_skills)?;
        ensure!(
            share.plan.token == plan_token(token),
            "The setup changed after the preview. Run `wenlan rules share --dry-run` again."
        );
        let key_yes = answer == KeyAnswer::Yes && !share.keys.is_empty();
        let preview = key_yes.then(|| KeyPreview::take(self, token)).flatten();
        let left = (!key_yes && !share.plan.key_servers.is_empty())
            .then(|| keys_left_message(share.plan.key_servers.len(), answer));
        let empty = share.is_empty() && preview.is_none();
        let mut result = report(if empty {
            NOTHING_TO_SHARE
        } else if share.operations.is_empty()
            && share.settings.is_empty()
            && share.rules.moving().next().is_none()
            && preview.is_none()
        {
            "Kept the selected rule lines in their own tools"
        } else {
            "Shared existing items with the other tools"
        });
        if key_yes && preview.is_none() {
            result.block(Code::ReviewNeeded, EXPIRED_KEY_PREVIEW);
        }
        let source_changed = source.merged != manifest.master.shared
            || manifest.active_deliveries().any(|(_, delivery)| {
                let Kind::Rule {
                    target,
                    base,
                    authored_ends: Some([end, _]),
                    ..
                } = &delivery.kind
                else {
                    return false;
                };
                base.get(..*end)
                    .is_some_and(|shared| shared != manifest.master.shared_rule_text(target))
            });
        source.record_with_history(self, &mut manifest)?;
        if source_changed {
            // Deliver the reconciled rules before moving the approved lines.
            // Its receipt checks still protect local edits in each tool.
            let pullback = inspect(&self.home, &root, &manifest)?;
            let blocked = pullback
                .blocked
                .into_keys()
                .chain(pullback.drafts.into_iter().map(|draft| draft.path))
                .collect();
            self.deliver_rules(
                &root,
                &mut manifest,
                generated,
                &render_rule,
                &blocked,
                &mut result,
            )?;
        }
        if empty {
            if let Some(left) = left {
                result.message = format!("{NOTHING_TO_SHARE}.\n{left}");
            }
            return Ok(result);
        }
        let Share {
            mut operations,
            skills,
            mut settings,
            worktree_writes,
            keys,
            rules,
            ..
        } = share;
        let rule_change =
            self.stage_rule_shares(&root, &manifest, &rules, generated, &render_rule)?;
        // Import the sources through the ordinary adoption path before any
        // projected copies are enrolled. It re-reads and verifies each entry
        // and adds its normal backup operation to this same batch.
        let unmanaged = offered_unmanaged_sources(&settings);
        let source_paths: BTreeSet<_> = unmanaged.sources.keys().cloned().collect();
        let mut adopted =
            personal_settings::adopt(&self.home, &root, &manifest, unmanaged, &mut operations)?;
        if let Some(change) = rule_change {
            self.apply_rule_shares(&root, &mut manifest, change, &mut result, person_confirmed)?;
        }
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
            if !source_paths.contains(&path)
                && !manifest
                    .delivered
                    .get(&path)
                    .is_some_and(|delivery| matches!(delivery.kind, Kind::Directory))
            {
                result.written.push(path.display().to_string());
            }
        }
        #[cfg(test)]
        tests::before_share_enrollment();
        personal_settings::recheck_share_sources(
            &self.home,
            &root,
            &mut settings,
            &mut adopted,
            &mut result,
        )?;
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
        personal_settings::enroll(&mut manifest, adopted);
        let destinations = enroll_shares(&mut manifest, settings);
        // Files about to hold a key take their safe additions in place too.
        let at = chrono::Utc::now().timestamp();
        if preview.is_some() {
            manifest
                .keys
                .intents
                .extend(keys.iter().cloned().map(|mut share| {
                    share.at = at;
                    share
                }));
        }
        self.save(&manifest)?;
        if !destinations.is_empty() {
            let mut written = report("");
            self.sync_personal_settings(&root, &mut manifest, &mut written, Some(&destinations))?;
            result.written.extend(written.written);
            result.kept_in_place.extend(written.kept_in_place);
            result.blocked.extend(written.blocked);
            result.blocked_codes.extend(written.blocked_codes);
        }
        if let Some(preview) = preview {
            manifest.keys.intents.clear();
            manifest.keys.notes.clear();
            self.apply_key_shares(&root, &mut manifest, keys, &preview, &mut result)?;
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
        source: &SharedSource,
        keep_rule: Option<&[usize]>,
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
        let (rules, rule_additions) = match keep_rule {
            Some(keep_rule) => {
                share_rules::plan(&self.home, root, manifest, source, &enrolled, keep_rule)?
            }
            None => Default::default(),
        };
        for (target, count) in rule_additions {
            listing.tool(target).rules = count;
        }
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
        settings.unmanaged_refusals.sort_unstable();
        listing.stays.sort_unstable_by(|left, right| {
            (&left.tool, &left.item, &left.reason).cmp(&(&right.tool, &right.item, &right.reason))
        });
        // The key group enters the token by place and name only.
        let token = fingerprint(&(
            SHARE_TOKEN_VERSION,
            &operations,
            &skills,
            &settings.additions,
            &settings.unmanaged,
            &settings.unmanaged_refusals,
            &settings.keys.shares,
            &worktree_writes,
            &listing.stays,
            listing.already_there,
            (
                &source.read,
                &source.merged,
                &source.credential_lines,
                rules.approval(),
            ),
        ))?;
        let count = rules.moving().count();
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
                rules: rules.lines.clone(),
                rule_stays: rules.stays.clone(),
                rule_notes: rules.notes.clone(),
                shared_source_notice: (count != 0).then(|| {
                    if !manifest.shared_source_following {
                        "The shared source is off. These rules will be saved in Wenlan and delivered to managed tools.".into()
                    } else if source.credential_lines.is_empty() {
                        let unit = if count == 1 { "line" } else { "lines" };
                        format!("Also adds {count} {unit} to ~/.agents/AGENTS.md.")
                    } else {
                        "Move the possible credential out of ~/.agents/AGENTS.md and preview again if you want these rules added there too. This share updates the tool copies.".into()
                    }
                }),
            },
            rules,
            keys: settings.keys.shares,
            key_digests: settings.keys.digests,
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
    use crate::rulesync_routes::tests::{
        accept_local_rule_additions, fixture_rule_renderer, fixture_skill_renderer, Fixture,
    };
    use std::cell::RefCell;

    thread_local! {
        static BEFORE_SHARE_ENROLLMENT: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
    }

    pub(super) fn before_share_enrollment() {
        if let Some(hook) = BEFORE_SHARE_ENROLLMENT.with(|cell| cell.take()) {
            hook();
        }
    }

    fn edit_before_share_enrollment(edit: impl FnOnce() + 'static) {
        BEFORE_SHARE_ENROLLMENT.with(|cell| cell.replace(Some(Box::new(edit))));
    }

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

    const CLAUDE_RULES: &str = ".claude/CLAUDE.md";
    const CODEX_RULES: &str = ".codex/AGENTS.md";
    const GEMINI_RULES: &str = ".gemini/GEMINI.md";
    const LOCAL_IMPORT: &str = "@./local-rules.md";
    const CLAUDE_RULE: &str = "Use Claude Code for this workspace.";
    const COMMON_RULE: &str = "Run the release checklist.";
    const REVIEW_RULE: &str = "Read the test output.";

    fn rule_fixture(f: &Fixture) {
        write(
            f,
            &[
                (".agents/AGENTS.md", "Already shared.\n"),
                (
                    CLAUDE_RULES,
                    &format!("{COMMON_RULE}\n{CLAUDE_RULE}\n{LOCAL_IMPORT}\n"),
                ),
                (
                    ".claude/local-rules.md",
                    "These imported lines stay local.\n",
                ),
                (CODEX_RULES, &format!("{COMMON_RULE}\n{REVIEW_RULE}\n")),
            ],
        );
        fs::create_dir_all(f.setup.home.join(".gemini")).unwrap();
    }

    fn preview_rules(f: &Fixture, keep: &[usize]) -> SharingPlan {
        f.setup
            .build_share_preview(Some(keep), false, fixture_skill_renderer)
            .unwrap()
    }

    #[test]
    fn rulesync_share_preview_names_saved_store_when_source_is_off() {
        let f = Fixture::new();
        rule_fixture(&f);
        adopt(&f);
        let off = f.setup.preview_source_off().unwrap();
        f.setup
            .turn_source_off(&SharedSourceOffRequest {
                plan_id: off.plan_id,
                token: off.token,
            })
            .unwrap();
        let source_before = f.text(SHARED_RULES);
        let plan = preview_rules(&f, &[]);
        assert!(!plan.rules.is_empty());
        let notice = plan.shared_source_notice.as_deref().unwrap();
        assert!(notice.contains("source is off"), "{notice}");
        assert!(!notice.contains("adds") || !notice.contains("~/.agents/AGENTS.md"));
        apply_rules(&f, &plan, &[]).unwrap();
        assert_eq!(f.text(SHARED_RULES), source_before);
    }

    #[test]
    fn rulesync_share_offers_other_tools_rules_when_a_live_tool_file_holds_a_key() {
        const VALUE: &str = "sk-claudeRuleFixtureOnly123456789";
        const CHANGED: &str = "sk-claudeRuleFixtureOnly987654321";
        const PORTABLE: &str = "- Prefer one change per pull request.";
        let f = Fixture::new();
        f.write(
            CLAUDE_RULES,
            &format!("{}{COMMON_RULE}\n", f.text(CLAUDE_RULES)),
        );
        f.write(
            CODEX_RULES,
            &format!("{}{PORTABLE}\n{COMMON_RULE}\n", f.text(CODEX_RULES)),
        );
        adopt(&f);
        let claude = f.text(CLAUDE_RULES);
        let line = claude.lines().count() + 1;
        let with_key = |value| format!("{claude}- api_key = \"{value}\"\n");
        f.write(CLAUDE_RULES, &with_key(VALUE));

        let plan = preview_rules(&f, &[]);
        assert!(plan.rules.iter().any(|rule| rule.line == PORTABLE));
        assert!(plan.rules.iter().any(|rule| rule.line == COMMON_RULE));
        let preview = serde_json::to_value(&plan).unwrap();
        assert!(preview["rule_stays"]
            .as_array()
            .unwrap()
            .iter()
            .any(|stay| {
                stay["tool"] == "Claude Code"
                    && stay["line_number"] == line
                    && stay["line"].is_null()
            }));
        assert!(!preview.to_string().contains(VALUE));
        assert_eq!(plan.rule_notes.len(), 1);
        assert!(plan.rule_notes[0].contains(SYNC_COMMAND));
        f.write(CLAUDE_RULES, &with_key(CHANGED));
        let again = preview_rules(&f, &[]);
        assert_eq!(again.plan_id, plan.plan_id);
        assert!(!serde_json::to_string(&again).unwrap().contains(CHANGED));

        let report = apply_rules(&f, &plan, &[]).unwrap();
        assert!(report.blocked.is_empty(), "{report:?}");
        assert_eq!(f.text(CLAUDE_RULES), with_key(CHANGED));
        assert!(f.text(CODEX_RULES).contains(PORTABLE));
        assert!(f.text(SHARED_RULES).contains(PORTABLE));
        let saved = f.setup.load().unwrap().unwrap();
        assert!(saved.master.shared.contains(PORTABLE));
        for value in [VALUE, CHANGED] {
            assert!(!saved.master.shared.contains(value));
            assert!(!f.text(CODEX_RULES).contains(value));
            assert!(!holds(&f.setup.data, value.as_bytes()));
        }
        f.write(CLAUDE_RULES, &claude);
        f.sync_with("");
        assert!(f.text(CLAUDE_RULES).contains(PORTABLE));
        assert_eq!(f.text(CLAUDE_RULES).matches(COMMON_RULE).count(), 1);
    }

    #[test]
    fn rulesync_share_offers_rules_when_the_shared_source_holds_a_key() {
        const VALUE: &str = "sk-sourceRuleFixtureOnly123456789";
        const CHANGED: &str = "sk-sourceRuleFixtureOnly987654321";
        let f = Fixture::new();
        rule_fixture(&f);
        f.write(
            SHARED_RULES,
            "Already shared.\nPrior shared rule.\nSafe source tail.\n",
        );
        adopt(&f);
        let with_key = |value| format!("Already shared.\nAPI_KEY={value}\nSafe source tail.\n");
        f.write(SHARED_RULES, &with_key(VALUE));

        let plan = preview_rules(&f, &[]);
        assert_eq!(
            plan.rules
                .iter()
                .map(|rule| rule.line.as_str())
                .collect::<Vec<_>>(),
            [COMMON_RULE, REVIEW_RULE]
        );
        let preview = serde_json::to_value(&plan).unwrap();
        assert!(preview["rule_stays"]
            .as_array()
            .unwrap()
            .iter()
            .any(|stay| {
                stay["tool"] == SHARED_RULES && stay["line_number"] == 2 && stay["line"].is_null()
            }));
        assert!(!preview.to_string().contains(VALUE));
        assert!(plan
            .shared_source_notice
            .as_deref()
            .unwrap()
            .contains("tool copies"));
        f.write(SHARED_RULES, &with_key(CHANGED));
        let again = preview_rules(&f, &[]);
        assert_eq!(again.plan_id, plan.plan_id);
        assert!(!serde_json::to_string(&again).unwrap().contains(CHANGED));
        f.write(
            SHARED_RULES,
            &with_key(CHANGED).replace("Safe source tail.\n", ""),
        );
        assert_ne!(preview_rules(&f, &[]).plan_id, plan.plan_id);
        assert!(apply_rules(&f, &plan, &[]).is_err());
        f.write(SHARED_RULES, &with_key(CHANGED));

        let report = apply_rules(&f, &plan, &[]).unwrap();
        assert!(report.blocked.is_empty(), "{report:?}");
        assert_eq!(f.text(SHARED_RULES), with_key(CHANGED));
        let saved = f.setup.load().unwrap().unwrap();
        assert!(saved.master.shared.contains(REVIEW_RULE));
        for path in [CLAUDE_RULES, CODEX_RULES, GEMINI_RULES] {
            let tool = f.text(path);
            assert!(
                tool.contains(COMMON_RULE) && tool.contains(REVIEW_RULE),
                "{path}"
            );
            assert!(!tool.contains(VALUE) && !tool.contains(CHANGED), "{path}");
        }
        let sync = f.sync_with("");
        assert!(
            sync.blocked.is_empty() && sync.written.is_empty(),
            "{sync:?}"
        );
        for value in [VALUE, CHANGED] {
            assert!(!saved.master.shared.contains(value));
            assert!(!holds(&f.setup.data, value.as_bytes()));
        }
    }

    fn apply_rules(f: &Fixture, plan: &SharingPlan, keep: &[usize]) -> Result<RulesSyncReport> {
        f.setup.apply_share(
            &plan.token,
            KeyAnswer::No,
            Some(keep),
            &[],
            true,
            (fixture_rule_renderer, fixture_skill_renderer),
        )
    }

    #[test]
    fn rulesync_share_requires_a_client_that_can_preview_rule_lines() {
        let f = Fixture::new();
        rule_fixture(&f);
        write(&f, &[(".codex/skills/codex-only/SKILL.md", SKILL)]);
        adopt(&f);

        let legacy: SharePreviewRequest = serde_json::from_value(serde_json::json!({
            "home": f.setup.home,
        }))
        .unwrap();
        let legacy_plan = f
            .setup
            .build_share_preview(legacy.keep_rule.as_deref(), false, fixture_skill_renderer)
            .unwrap();
        assert!(legacy_plan.rules.is_empty());
        assert_eq!(tool(&legacy_plan, "Claude Code").skills, ["codex-only"]);
        let current = preview_rules(&f, &[]);
        assert_ne!(current.plan_id, legacy_plan.plan_id);
        assert!(f
            .setup
            .share_using(&current.token, fixture_skill_renderer)
            .is_err());
        assert!(apply_rules(&f, &legacy_plan, &[]).is_err());
        let before = f.text(CLAUDE_RULES);
        let report = f
            .setup
            .share_using(&legacy_plan.token, fixture_skill_renderer)
            .unwrap();
        assert!(report.blocked.is_empty(), "{report:?}");
        assert_eq!(f.text(CLAUDE_RULES), before);
        assert_eq!(f.text(".claude/skills/codex-only/SKILL.md"), SKILL);
    }

    #[test]
    fn ux_skill_26_rulesync_share_moves_common_rules_and_preserves_tool_only_files() {
        let f = Fixture::new();
        rule_fixture(&f);
        let owner_items = [
            (".codex/skills/codex-only/SKILL.md", SKILL),
            (
                ".claude/skills/style/SKILL.md",
                "---\nname: style\ndescription: A fixture.\n---\nClaude style.\n",
            ),
            (
                ".codex/skills/style/SKILL.md",
                "---\nname: style\ndescription: A fixture.\n---\nCodex style.\n",
            ),
            (".claude/commands/hello.md", COMMAND),
            (".codex/prompts/hello.md", "Different command in Codex.\n"),
            (
                ".claude/agents/review.md",
                "---\nname: review\ndescription: Review changes.\n---\nCheck the changes.\n",
            ),
        ];
        write(&f, &owner_items);
        let before = tree(&f.setup.home);
        adopt(&f);
        let plan = preview_rules(&f, &[]);
        assert_eq!(
            serde_json::to_value(&plan).unwrap()["shared_source_notice"],
            "Also adds 2 lines to ~/.agents/AGENTS.md."
        );
        assert_eq!(
            plan.rules
                .iter()
                .map(|rule| rule.line.as_str())
                .collect::<Vec<_>>(),
            [COMMON_RULE, REVIEW_RULE]
        );
        assert_eq!(plan.rules[0].tools, ["Claude Code", "Codex"]);
        assert_eq!(tool(&plan, "Claude Code").rules, 1);
        assert_eq!(tool(&plan, "Gemini CLI").rules, 2);
        assert!(plan
            .rule_stays
            .iter()
            .any(|stay| stay.line.as_deref() == Some(LOCAL_IMPORT)
                && stay.reason.contains("Only Claude Code")));
        assert!(plan
            .rule_stays
            .iter()
            .any(|stay| stay.line.as_deref() == Some(CLAUDE_RULE)
                && stay.reason.contains("names a tool")));

        let shared = apply_rules(&f, &plan, &[]).unwrap();
        assert!(shared.blocked.is_empty(), "{shared:?}");
        assert_eq!(
            shared
                .written
                .iter()
                .filter(|path| path.as_str() == GEMINI_RULES)
                .count(),
            1
        );
        let manifest = f.setup.load().unwrap().unwrap();
        assert_eq!(
            manifest.master.shared,
            format!("Already shared.\n{COMMON_RULE}\n{REVIEW_RULE}\n")
        );
        for overlay in manifest.master.overlays.values() {
            assert!(!overlay.contains(COMMON_RULE));
            assert!(!overlay.contains(REVIEW_RULE));
        }
        for path in [CLAUDE_RULES, CODEX_RULES, GEMINI_RULES] {
            let text = f.text(path);
            assert!(text.contains("Already shared."));
            assert_eq!(text.lines().filter(|line| *line == COMMON_RULE).count(), 1);
            assert!(text.contains(REVIEW_RULE));
            assert_eq!(text.contains(CLAUDE_RULE), path == CLAUDE_RULES);
            assert_eq!(text.contains(LOCAL_IMPORT), path == CLAUDE_RULES);
            assert!(!text.contains("These imported lines stay local."));
        }
        for (path, original) in owner_items {
            assert_eq!(f.text(path), original, "{path}");
        }
        assert_eq!(f.text(".agents/AGENTS.md"), manifest.master.shared);
        assert!(preview_rules(&f, &[]).rules.is_empty());

        let after = tree(&f.setup.home);
        let sync = f.sync_with("");
        assert!(
            sync.blocked.is_empty() && sync.proposals.is_empty(),
            "{sync:?}"
        );
        assert_eq!(tree(&f.setup.home), after);
        let restore = f.setup.restore().unwrap();
        assert!(restore.blocked.is_empty(), "{restore:?}");
        assert_eq!(tree(&f.setup.home), before);
    }

    #[test]
    fn rulesync_share_keeps_numbered_rules_and_redacts_credentials() {
        let f = Fixture::new();
        rule_fixture(&f);
        const VALUE: &str = "synthetic-rule-credential";
        let secret = format!("Claude PASSWORD={VALUE}");
        f.write(
            CLAUDE_RULES,
            &format!("{COMMON_RULE}\n{CLAUDE_RULE}\n{secret}\n"),
        );
        adopt(&f);

        let all = preview_rules(&f, &[]);
        let keep = preview_rules(&f, &[2]);
        assert_ne!(all.plan_id, keep.plan_id);
        assert_eq!(keep.rules[1].line, REVIEW_RULE);
        assert!(keep.rules[1].kept);
        assert!(keep
            .rule_stays
            .iter()
            .any(|stay| stay.line.is_none() && stay.reason == "This line may hold a credential."));
        assert!(!serde_json::to_string(&keep).unwrap().contains(VALUE));
        assert!(
            apply_rules(&f, &keep, &[]).is_err(),
            "keep choices must match the approval"
        );
        let report = apply_rules(&f, &keep, &[2]).unwrap();
        assert!(report.blocked.is_empty(), "{report:?}");
        assert!(f.text(CLAUDE_RULES).contains(VALUE));
        assert!(f.text(CODEX_RULES).contains(REVIEW_RULE));
        assert!(!f.text(CODEX_RULES).contains(VALUE));
        assert!(!f.text(GEMINI_RULES).contains(REVIEW_RULE));
        assert!(!f.text(GEMINI_RULES).contains(VALUE));

        let again = preview_rules(&f, &[]);
        assert!(again.rules.is_empty());
        assert!(again
            .rule_stays
            .iter()
            .any(|stay| stay.line.as_deref() == Some(REVIEW_RULE)
                && stay.reason.contains("You chose")));
    }

    #[test]
    fn rulesync_share_redacts_http_credentials_before_building_approval() {
        let f = Fixture::new();
        rule_fixture(&f);
        let credentials = [
            ("Authorization: Basic dGVzdDpwYXNz", "dGVzdDpwYXNz"),
            ("Authorization: Bearer demo7", "demo7"),
            ("Send Basic dXNlcjphYmM=", "dXNlcjphYmM="),
            (
                "Read https://synthetic:example@docs.invalid",
                "synthetic:example",
            ),
            ("Use https://docs.invalid/?token=t1", "token=t1"),
            (
                "Call the endpoint with api_key='w79wxza928af410e9c4f36xdpycb01h6'",
                "w79wxza928af410e9c4f36xdpycb01h6",
            ),
            (
                "Send secret=s5d2b8t055kc14mh1shs65e791v7mcyb",
                "s5d2b8t055kc14mh1shs65e791v7mcyb",
            ),
        ];
        let mut donor = f.text(CLAUDE_RULES);
        for (line, _) in credentials {
            donor.push_str(&format!("{line}\n"));
        }
        f.write(CLAUDE_RULES, &donor);
        adopt(&f);

        let manifest = f.setup.load().unwrap().unwrap();
        let root = FileRoot::open_home(&f.setup.home).unwrap();
        let share = f
            .setup
            .plan_share(
                &root,
                &manifest,
                &SharedSource::read(&f.setup.home, &manifest).unwrap(),
                Some(&[]),
                &fixture_skill_renderer,
            )
            .unwrap();
        let plan = share.plan;
        let preview_json = serde_json::to_string(&plan).unwrap();
        let approval_input = serde_json::to_string(&share.rules.approval()).unwrap();
        assert_eq!(
            plan.rule_stays
                .iter()
                .filter(|stay| {
                    stay.line.is_none() && stay.reason == "This line may hold a credential."
                })
                .count(),
            credentials.len()
        );
        apply_rules(&f, &plan, &[]).unwrap();
        let saved = f.setup.load().unwrap().unwrap();
        for (line, value) in credentials {
            assert!(
                f.text(CLAUDE_RULES).contains(line),
                "donor lost its local line"
            );
            assert!(!preview_json.contains(value), "credential in JSON");
            assert!(
                !approval_input.contains(value),
                "credential in approval input"
            );
            for path in [CODEX_RULES, GEMINI_RULES, ".agents/AGENTS.md"] {
                assert!(!f.text(path).contains(value), "credential reached {path}");
            }
            assert!(
                !saved.master.shared.contains(value),
                "credential in the shared master"
            );
        }
        assert!(f.text(GEMINI_RULES).contains(COMMON_RULE));
    }

    #[test]
    fn rulesync_share_can_keep_every_numbered_rule_without_writing_tool_files() {
        let f = Fixture::new();
        rule_fixture(&f);
        adopt(&f);
        let before = tree(&f.setup.home);
        let plan = preview_rules(&f, &[1, 2]);
        assert!(plan.tools.is_empty());
        let report = apply_rules(&f, &plan, &[1, 2]).unwrap();
        assert_eq!(
            report.message,
            "Kept the selected rule lines in their own tools"
        );
        assert!(
            report.written.is_empty() && report.blocked.is_empty(),
            "{report:?}"
        );
        assert_eq!(tree(&f.setup.home), before);
        assert!(preview_rules(&f, &[]).rules.is_empty());
    }

    #[test]
    fn rulesync_share_refuses_stale_rule_files_and_binds_the_line_set() {
        let f = Fixture::new();
        rule_fixture(&f);
        adopt(&f);
        let old = preview_rules(&f, &[]);
        f.write(CODEX_RULES, "Replaced after the preview.\n");
        let changed = tree(&f.setup.home);
        assert!(apply_rules(&f, &old, &[])
            .unwrap_err()
            .to_string()
            .contains("changed"));
        assert_eq!(tree(&f.setup.home), changed);

        // A later adopted setup has different offered content, not just a
        // different on-disk snapshot of the original overlay.
        let fresh = Fixture::new();
        rule_fixture(&fresh);
        fresh.write(CODEX_RULES, "Read the final test output.\n");
        adopt(&fresh);
        let new = preview_rules(&fresh, &[]);
        assert_ne!(old.plan_id, new.plan_id);
        assert!(apply_rules(&fresh, &old, &[])
            .unwrap_err()
            .to_string()
            .contains("--dry-run"));
    }

    #[test]
    fn rulesync_share_reclassifies_accepted_edits_and_binds_the_donor_revision() {
        let f = Fixture::new();
        rule_fixture(&f);
        adopt(&f);
        let accept = |edited: &str| {
            f.write(CODEX_RULES, edited);
            let report = f.sync_with("");
            let card = report
                .proposals
                .iter()
                .find(|card| card.files == [CODEX_RULES])
                .unwrap();
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
        };
        let old = preview_rules(&f, &[]);
        // The blank line changes the donor's approved revision without
        // changing the lines offered to the other tools.
        accept(&f.text(CODEX_RULES).replace(
            &format!("{COMMON_RULE}\n{REVIEW_RULE}"),
            &format!("{COMMON_RULE}\n\n{REVIEW_RULE}"),
        ));
        let same_listing = preview_rules(&f, &[]);
        assert_eq!(old.rules, same_listing.rules);
        assert_eq!(old.rule_stays, same_listing.rule_stays);
        assert_ne!(old.plan_id, same_listing.plan_id);
        let error = apply_rules(&f, &old, &[]).unwrap_err();
        assert!(
            error.to_string().contains("changed after the preview"),
            "{error:#}"
        );

        let edited = f
            .text(CODEX_RULES)
            .replace(REVIEW_RULE, &format!("```text\n{REVIEW_RULE}\n```"));
        accept(&format!("{edited}Check the final exit status.\n"));
        let current = preview_rules(&f, &[]);
        assert!(current
            .rule_stays
            .iter()
            .any(|stay| stay.line.as_deref() == Some(REVIEW_RULE)));
        assert!(current
            .rules
            .iter()
            .any(|rule| rule.line == "Check the final exit status."));
        assert!(current.rules.iter().all(|rule| rule.line != REVIEW_RULE));
        apply_rules(&f, &current, &[]).unwrap();
        assert!(f
            .text(CODEX_RULES)
            .contains(&format!("```text\n{REVIEW_RULE}\n```")));
        assert!(f
            .text(GEMINI_RULES)
            .contains("Check the final exit status."));
        assert!(!f.text(GEMINI_RULES).contains(REVIEW_RULE));
    }

    #[test]
    fn rulesync_share_mid_batch_conflict_restores_peers_and_keeps_the_old_receipts() {
        let f = Fixture::new();
        rule_fixture(&f);
        write(
            &f,
            &[
                (CLAUDE_RULES, CLAUDE_RULE),
                (CODEX_RULES, "Use Codex for local work."),
                (GEMINI_RULES, COMMON_RULE),
            ],
        );
        adopt(&f);
        let plan = preview_rules(&f, &[]);
        let master_path = f.setup.data.join("personal.json");
        let before = fs::read(&master_path).unwrap();
        let original = f.setup.load().unwrap().unwrap();
        let early_peer = f.setup.home.join(CLAUDE_RULES);
        let donor = f.setup.home.join(GEMINI_RULES);
        let newer = format!("{}Gemini CLI local edit.\n", f.text(GEMINI_RULES));
        let edit = newer.clone();
        share_rules::at_rule_share_write(GEMINI_RULES, move || {
            assert!(fs::read_to_string(&early_peer)
                .unwrap()
                .contains(COMMON_RULE));
            fs::write(donor, edit).unwrap();
        });
        let error = apply_rules(&f, &plan, &[]).unwrap_err();
        assert!(
            error.to_string().contains("changed while sharing rules"),
            "{error:#}"
        );
        assert_eq!(fs::read(&master_path).unwrap(), before);
        assert_eq!(f.text(GEMINI_RULES), newer);
        f.sync_with("");
        for peer in [CLAUDE_RULES, CODEX_RULES, ".agents/AGENTS.md"] {
            assert!(!f.text(peer).contains(COMMON_RULE), "{peer}");
        }
        let after = f.setup.load().unwrap().unwrap();
        assert_eq!(after.master.shared, original.master.shared);
        assert_eq!(after.master.overlays, original.master.overlays);
        assert_eq!(after.kept_share_rules, original.kept_share_rules);
    }

    #[test]
    fn rulesync_share_keeps_scoped_sections_and_preserves_repeated_lines_in_order() {
        let f = Fixture::new();
        rule_fixture(&f);
        const REPEATED: &str = "- Use the release checklist.";
        let text = format!(
            "For Claude Code preflight:\n- Check the local path.\n\n\
             # For Claude Code release sessions only\n{REPEATED}\n\n## During signing\n- Verify the certificate.\n\n\
             # For Claude Code packaging only\n{REPEATED}\n\n\
             # Portable checks\n- Review the artifacts.\n"
        );
        f.write(CLAUDE_RULES, &text);
        adopt(&f);
        let plan = preview_rules(&f, &[]);
        for scoped in [
            REPEATED,
            "## During signing",
            "- Verify the certificate.",
            "- Check the local path.",
        ] {
            assert!(
                plan.rules.iter().all(|rule| rule.line != scoped),
                "{scoped}"
            );
            assert!(
                plan.rule_stays.iter().any(|stay| {
                    stay.line.as_deref() == Some(scoped)
                        && stay.reason == "Belongs to a section that stays in this tool"
                }),
                "{scoped}"
            );
        }
        let heading = plan
            .rules
            .iter()
            .find(|rule| rule.line == "# Portable checks")
            .unwrap();
        let keep = preview_rules(&f, &[heading.number]);
        assert!(keep
            .rules
            .iter()
            .all(|rule| rule.line != "- Review the artifacts."));
        assert!(keep.rule_stays.iter().any(|stay| stay.line.as_deref()
            == Some("- Review the artifacts.")
            && stay.reason == "Belongs to a section that stays in this tool"));

        let render = |target: &str, source: &str| {
            fixture_rule_renderer(target, &source.replacen(REPEATED, "", 1))
        };
        let error = f
            .setup
            .apply_share(
                &plan.token,
                KeyAnswer::No,
                Some(&[]),
                &[],
                true,
                (render, fixture_skill_renderer),
            )
            .unwrap_err();
        assert!(
            error.to_string().contains("would change other rules"),
            "{error:#}"
        );
        apply_rules(&f, &plan, &[]).unwrap();
        assert_eq!(f.text(CLAUDE_RULES).matches(REPEATED).count(), 2);
        assert!(!f.text(CODEX_RULES).contains(REPEATED));
        assert!(f.text(CODEX_RULES).contains("- Review the artifacts."));
    }

    #[test]
    fn rulesync_share_keeps_flattened_imports_local_with_or_without_saved_provenance() {
        let f = Fixture::new();
        rule_fixture(&f);
        const ADAPTER: &str = ".agents/adapters/claude/CLAUDE.md";
        const IMPORT: &str = ".agents/adapters/claude/release.md";
        const IMPORTED: &str = "Use the internal release checklist.";
        const NATIVE: &str = "Review the final changes.";
        write(
            &f,
            &[
                (ADAPTER, "@./release.md\n"),
                (IMPORT, &format!("{IMPORTED}\n{NATIVE}\n")),
                (CLAUDE_RULES, NATIVE),
            ],
        );
        adopt(&f);
        assert!(f.text(CLAUDE_RULES).contains(IMPORTED));
        let check = |plan: &SharingPlan, line| {
            assert!(plan.rules.iter().all(|rule| rule.line != line), "{line}");
            assert!(
                plan.rule_stays
                    .iter()
                    .any(|stay| stay.line.as_deref() == Some(line)
                        && stay.reason == "Comes from a file Claude Code imports"),
                "{line}"
            );
        };
        let mut manifest = f.setup.load().unwrap().unwrap();
        let provenance = manifest.master.claude_imported_rules.take().unwrap();
        assert!(provenance.contains(IMPORTED));
        f.setup.save(&manifest).unwrap(); // Load a pre-provenance manifest.
        let legacy = preview_rules(&f, &[]);
        check(&legacy, IMPORTED);
        check(&legacy, NATIVE);
        fs::remove_file(f.setup.home.join(IMPORT)).unwrap();
        let unknown = preview_rules(&f, &[]);
        assert!(unknown.rules.iter().all(|rule| rule.line != IMPORTED));
        assert!(unknown.rule_stays.iter().any(|stay| {
            stay.line.as_deref() == Some(IMPORTED) && stay.reason.starts_with("Could not verify")
        }));
        assert!(unknown.rules.iter().any(|rule| rule.line == NATIVE));

        manifest.master.claude_imported_rules = Some(provenance);
        f.setup.save(&manifest).unwrap();
        let persisted = preview_rules(&f, &[]);
        check(&persisted, IMPORTED);
        check(&persisted, NATIVE);
        apply_rules(&f, &persisted, &[]).unwrap();
        assert!(!f.text(GEMINI_RULES).contains(IMPORTED));
    }

    #[test]
    fn rulesync_share_updates_and_restores_the_shared_source() {
        const SOURCE: &str = ".agents/AGENTS.md";
        for existing in [true, false] {
            let f = Fixture::new();
            rule_fixture(&f);
            if !existing {
                fs::remove_file(f.setup.home.join(SOURCE)).unwrap();
                fs::remove_dir(f.setup.home.join(".agents")).unwrap();
            }
            let before = tree(&f.setup.home);
            adopt(&f);
            let plan = preview_rules(&f, &[]);
            let result = apply_rules(&f, &plan, &[]).unwrap();
            let saved = f.setup.load().unwrap().unwrap();
            if existing {
                assert!(
                    result.written.iter().any(|path| path == SOURCE),
                    "{result:?}"
                );
                assert_eq!(f.text(SOURCE), saved.master.shared);
                assert!(f.text(SOURCE).contains(COMMON_RULE));
            } else {
                assert!(!result.written.iter().any(|path| path == SOURCE));
                assert!(!f.setup.home.join(SOURCE).exists());
                assert!(saved.master.shared.contains(COMMON_RULE));
            }
            let result = f.setup.restore().unwrap();
            assert!(result.blocked.is_empty(), "{result:?}");
            assert_eq!(tree(&f.setup.home), before);
        }
    }

    #[test]
    fn rulesync_restore_keeps_a_shared_addition_only_in_its_donor() {
        let f = Fixture::new();
        rule_fixture(&f);
        let original = f.text(CODEX_RULES);
        let claude = f.text(CLAUDE_RULES);
        let shared = f.text(".agents/AGENTS.md");
        let addition = "New rule from the source tool.\n";
        let first = format!("{addition}Use Codex to check this workspace.\r\n{addition}");
        let later = "A rule added after sharing.\n";
        adopt(&f);
        for (local, shared_line) in [(&*first, addition), (later, later)] {
            accept_local_rule_additions(&f, local);
            let plan = preview_rules(&f, &[]);
            let shared_report = apply_rules(&f, &plan, &[]).unwrap();
            assert!(shared_report.blocked.is_empty(), "{shared_report:?}");
            assert!(f.text(CLAUDE_RULES).contains(shared_line));
            assert!(f.text(GEMINI_RULES).contains(shared_line));
        }

        let restored = f.setup.restore().unwrap();
        assert!(restored.blocked.is_empty(), "{restored:?}");
        assert_eq!(f.text(CODEX_RULES), format!("{original}{first}{later}"));
        assert_eq!(f.text(CLAUDE_RULES), claude);
        assert_eq!(f.text(".agents/AGENTS.md"), shared);
        assert!(!f.setup.home.join(GEMINI_RULES).exists());
    }

    #[test]
    fn rulesync_restore_blocks_an_old_share_without_donor_information() {
        let f = Fixture::new();
        rule_fixture(&f);
        adopt(&f);
        accept_local_rule_additions(&f, "An addition with no old receipt for its donor.\n");
        apply_rules(&f, &preview_rules(&f, &[]), &[]).unwrap();
        let mut legacy = f.setup.load().unwrap().unwrap();
        legacy.rule_origins_tracked = false;
        legacy.authored_rules.clear();
        f.setup.save(&legacy).unwrap();
        let before = tree(&f.setup.home);

        let restored = f.setup.restore().unwrap();
        assert!(
            restored
                .blocked
                .iter()
                .any(|reason| reason.contains("does not say which tool")),
            "{restored:?}"
        );
        assert_eq!(tree(&f.setup.home), before);
        assert!(f.setup.load().unwrap().is_some());
    }

    #[test]
    fn rulesync_sync_follows_shared_source_edits_without_review() {
        let f = Fixture::new();
        rule_fixture(&f);
        adopt(&f);
        let edited = "Edited directly in the shared source.\n";
        let before = [CLAUDE_RULES, CODEX_RULES, GEMINI_RULES].map(|path| f.text(path));
        f.write(".agents/AGENTS.md", edited);
        let sync = f.sync_with("");
        assert!(
            sync.blocked.is_empty() && sync.proposals.is_empty(),
            "{sync:?}"
        );
        assert_eq!(f.setup.load().unwrap().unwrap().master.shared, edited);
        assert_eq!(f.text(".agents/AGENTS.md"), edited);
        for (path, before) in [CLAUDE_RULES, CODEX_RULES, GEMINI_RULES]
            .into_iter()
            .zip(before)
        {
            assert_eq!(
                f.text(path),
                before.replacen("Already shared.\n", edited, 1)
            );
        }
        assert!(f.setup.status().unwrap().proposals.is_empty());
    }

    #[test]
    fn rulesync_sync_removes_deleted_shared_lines_and_keeps_overlays() {
        let f = Fixture::new();
        rule_fixture(&f);
        let removed = "Remove this rule from every tool.\n";
        let edited = f.text(".agents/AGENTS.md");
        f.write(".agents/AGENTS.md", &format!("{removed}{edited}"));
        adopt(&f);
        let before = [CLAUDE_RULES, CODEX_RULES, GEMINI_RULES].map(|path| f.text(path));

        f.write(".agents/AGENTS.md", &edited);
        let sync = f.sync_with("");
        assert!(
            sync.blocked.is_empty() && sync.proposals.is_empty(),
            "{sync:?}"
        );
        assert_eq!(f.setup.load().unwrap().unwrap().master.shared, edited);
        for (path, before) in [CLAUDE_RULES, CODEX_RULES, GEMINI_RULES]
            .into_iter()
            .zip(before)
        {
            assert_eq!(f.text(path), before.replacen(removed, "", 1));
        }
    }

    #[test]
    fn rulesync_sync_keeps_shared_moves_and_follows_later_source_edits() {
        let f = Fixture::new();
        rule_fixture(&f);
        adopt(&f);
        apply_rules(&f, &preview_rules(&f, &[]), &[]).unwrap();
        let edited = f
            .text(".agents/AGENTS.md")
            .replace("Already shared.", "Edited in the source.");
        f.write(".agents/AGENTS.md", &edited);

        let sync = f.sync_with("");
        assert!(
            sync.blocked.is_empty() && sync.proposals.is_empty(),
            "{sync:?}"
        );
        assert_eq!(f.setup.load().unwrap().unwrap().master.shared, edited);
        for path in [".agents/AGENTS.md", CLAUDE_RULES, CODEX_RULES, GEMINI_RULES] {
            assert!(f.text(path).contains("Edited in the source."), "{path}");
            assert!(f.text(path).contains(COMMON_RULE), "{path}");
        }
    }

    fn approve_shared_edit(f: &Fixture, before: &str, after: &str) {
        f.write(CODEX_RULES, &f.text(CODEX_RULES).replace(before, after));
        let report = f.sync_with("");
        let card = report
            .proposals
            .iter()
            .find(|card| card.files == [CODEX_RULES])
            .unwrap();
        let report = f
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
        assert!(report.blocked.is_empty(), "{report:?}");
        assert!(f.text(CLAUDE_RULES).contains(after));
        assert!(f.text(".agents/AGENTS.md").contains(before));
    }

    #[test]
    fn rulesync_sync_keeps_approved_shared_edits_without_source_history() {
        let f = Fixture::new();
        rule_fixture(&f);
        adopt(&f);
        let approved = "Approved for all tools.";
        approve_shared_edit(&f, "Already shared.", approved);
        let mut legacy = f.setup.load().unwrap().unwrap();
        legacy.shared_source_base = None;
        assert!(!is_shared_source_receipt(&legacy, Path::new(SHARED_RULES)));
        f.setup.save(&legacy).unwrap();

        let sync = f.sync_with("");
        assert!(
            sync.blocked.is_empty() && sync.proposals.is_empty(),
            "{sync:?}"
        );
        let saved = f.setup.load().unwrap().unwrap();
        assert_eq!(saved.master.shared, format!("{approved}\n"));
        assert_eq!(
            saved.shared_source_base.as_deref(),
            Some("Already shared.\n")
        );
        for path in [CLAUDE_RULES, CODEX_RULES, GEMINI_RULES] {
            assert!(f.text(path).contains(approved), "{path}");
        }
        assert!(f.sync_with("").written.is_empty());
    }

    #[test]
    fn rulesync_sync_retains_shared_rules_under_credentials_and_accepts_version_tags() {
        const VALUE: &str = "sk-redactedSourceFixtureOnly123456789";
        let f = Fixture::new();
        rule_fixture(&f);
        let original = "Already shared.\nKeep this shared rule.\nA shared anchor.";
        f.write(SHARED_RULES, original);
        adopt(&f);
        let safe = "Use node:20@sha256 and react:18@latest.\nCheck the result.\n";
        let revised = format!(
            "An edited rule.\napi_key={VALUE}\nA shared anchor.\n-----BEGIN OPENSSH PRIVATE KEY-----\n{safe}"
        );
        f.write(SHARED_RULES, &revised);

        let sync = f.sync_with("");
        assert!(
            sync.blocked.is_empty() && sync.proposals.is_empty(),
            "{sync:?}"
        );
        let saved = f.setup.load().unwrap().unwrap();
        assert_eq!(
            saved.master.shared,
            format!("An edited rule.\nKeep this shared rule.\nA shared anchor.\n{safe}")
        );
        assert_eq!(
            saved.credentials.rules[Path::new(SHARED_RULES)].lines,
            [2, 4]
        );
        for path in [CLAUDE_RULES, CODEX_RULES, GEMINI_RULES] {
            let tool = f.text(path);
            assert!(tool.contains(&saved.master.shared), "{path}");
            assert!(!tool.contains(VALUE), "{path}");
        }
        assert_eq!(f.text(SHARED_RULES), revised);
        assert!(f.sync_with("").written.is_empty());
        assert!(!holds(&f.setup.data, VALUE.as_bytes()));
    }

    #[test]
    fn rulesync_sync_pauses_for_missing_or_empty_shared_rules_source() {
        let f = Fixture::new();
        rule_fixture(&f);
        adopt(&f);
        let source_path = f.setup.home.join(SHARED_RULES);
        let original = f.text(SHARED_RULES);
        let saved = tree(&f.setup.data);
        for empty in [false, true] {
            if empty {
                f.write(SHARED_RULES, "\n \n");
            } else {
                fs::remove_file(&source_path).unwrap();
            }
            let before = tree(&f.setup.home);
            let sync = f.sync_with("");
            assert!(
                sync.written.is_empty() && sync.proposals.is_empty(),
                "{sync:?}"
            );
            assert_eq!(sync.blocked.len(), 1, "{sync:?}");
            assert!(sync.blocked[0].contains("missing or empty"), "{sync:?}");
            assert!(sync.blocked[0].contains(SYNC_COMMAND), "{sync:?}");
            assert_eq!(tree(&f.setup.home), before);
            assert_eq!(tree(&f.setup.data), saved);
            let status = f.setup.status().unwrap();
            assert!(!status.in_sync);
            assert_eq!(status.notes, sync.blocked);
        }
        f.write(SHARED_RULES, &original);
        assert!(f.sync_with("").blocked.is_empty());
    }

    #[test]
    fn rulesync_adoption_does_not_duplicate_shared_rules_via_a_protected_claude_import() {
        const VALUE: &str = "sk-claudeImportFixtureOnly123456789";
        let f = Fixture::new();
        let shared = "Read the release checklist.\n";
        let protected = format!("{shared}API_KEY={VALUE}\n");
        f.write(SHARED_RULES, &protected);
        adopt(&f);

        assert_eq!(f.text(CLAUDE_RULES), format!("{shared}Claude only.\n"));
        assert_eq!(f.text(SHARED_RULES), protected);
        let saved = f.setup.load().unwrap().unwrap();
        assert_eq!(saved.credentials.rules[Path::new(SHARED_RULES)].lines, [2]);
        assert!(!holds(&f.setup.data, VALUE.as_bytes()));
    }

    #[test]
    fn rulesync_sync_merges_a_shared_line_inserted_on_both_sides_only_once() {
        let f = Fixture::new();
        rule_fixture(&f);
        let base = "First rule.\nSecond rule.\nThird rule.\n";
        let line = "A shared addition.\n";
        f.write(SHARED_RULES, base);
        adopt(&f);
        let mut manifest = f.setup.load().unwrap().unwrap();
        manifest.master.shared = format!("{line}{base}");
        f.setup.save(&manifest).unwrap();
        let source = format!("{base}{line}");
        f.write(SHARED_RULES, &source);

        let sync = f.sync_with("");
        assert!(sync.blocked.is_empty(), "{sync:?}");
        let saved = f.setup.load().unwrap().unwrap();
        assert_eq!(saved.master.shared, source);
        for path in [CLAUDE_RULES, CODEX_RULES, GEMINI_RULES] {
            assert_eq!(f.text(path).matches(line).count(), 1, "{path}");
        }
        assert!(f.sync_with("").written.is_empty());
    }

    #[test]
    fn rulesync_sync_merges_independent_edits_on_both_sides() {
        let f = Fixture::new();
        rule_fixture(&f);
        adopt(&f);
        apply_rules(&f, &preview_rules(&f, &[]), &[]).unwrap();
        approve_shared_edit(&f, COMMON_RULE, "Reviewed in Wenlan.");
        // A setup created before source tracking can still use its share receipt.
        let mut legacy = f.setup.load().unwrap().unwrap();
        legacy.shared_source_base = None;
        f.setup.save(&legacy).unwrap();
        let source = f
            .text(".agents/AGENTS.md")
            .replace("Already shared.", "Edited in the source.");
        f.write(".agents/AGENTS.md", &source);

        let sync = f.sync_with("");
        assert!(
            sync.blocked.is_empty() && sync.proposals.is_empty(),
            "{sync:?}"
        );
        let expected = source.replace(COMMON_RULE, "Reviewed in Wenlan.");
        assert_eq!(f.setup.load().unwrap().unwrap().master.shared, expected);
        assert_eq!(f.text(".agents/AGENTS.md"), source);
        for path in [CLAUDE_RULES, CODEX_RULES, GEMINI_RULES] {
            assert!(f.text(path).contains("Edited in the source."), "{path}");
            assert!(f.text(path).contains("Reviewed in Wenlan."), "{path}");
        }
        assert!(f.sync_with("").written.is_empty());
    }

    #[test]
    fn rulesync_shared_source_conflict_reports_once_and_leaves_files_intact() {
        let f = Fixture::new();
        rule_fixture(&f);
        adopt(&f);
        apply_rules(&f, &preview_rules(&f, &[]), &[]).unwrap();
        approve_shared_edit(&f, COMMON_RULE, "Reviewed in Wenlan.");
        let prior_plan = preview_rules(&f, &[]);
        let source = f.text(".agents/AGENTS.md");
        f.write(
            ".agents/AGENTS.md",
            &source.replace(COMMON_RULE, "Changed in the source."),
        );
        let before = tree(&f.setup.home);
        let saved = tree(&f.setup.data);

        let sync = f.sync_with("");
        assert!(
            sync.written.is_empty() && sync.proposals.is_empty(),
            "{sync:?}"
        );
        assert_eq!(sync.blocked.len(), 1, "{sync:?}");
        assert!(sync.blocked[0].contains("same shared lines"), "{sync:?}");
        assert!(sync.blocked[0].contains(SYNC_COMMAND), "{sync:?}");
        let share_error = f
            .setup
            .build_share_preview(Some(&[]), false, fixture_skill_renderer)
            .unwrap_err();
        assert_eq!(share_error.to_string(), sync.blocked[0]);
        assert_eq!(
            apply_rules(&f, &prior_plan, &[]).unwrap_err().to_string(),
            sync.blocked[0]
        );
        assert_eq!(tree(&f.setup.home), before);
        assert_eq!(tree(&f.setup.data), saved);

        f.write(
            ".agents/AGENTS.md",
            &source.replace("Already shared.", "A separate source edit."),
        );
        let resolved = f.sync_with("");
        assert!(
            resolved.blocked.is_empty() && resolved.proposals.is_empty(),
            "{resolved:?}"
        );
        assert!(f.text(GEMINI_RULES).contains("A separate source edit."));
        assert!(f.text(GEMINI_RULES).contains("Reviewed in Wenlan."));
    }

    #[test]
    fn rulesync_sync_keeps_shared_source_credentials_out_of_every_copy() {
        const FIXTURE_VALUE: &str = "sk-sharedSourceFixtureOnly123456789";
        let f = Fixture::new();
        rule_fixture(&f);
        adopt(&f);
        // Also exercise a source already owned by a share receipt.
        apply_rules(&f, &preview_rules(&f, &[]), &[]).unwrap();
        let before = f.text(".agents/AGENTS.md");
        let source = format!("{before}A safe new rule.\nAPI_KEY={FIXTURE_VALUE}\n");
        f.write(".agents/AGENTS.md", &source);

        let sync = f.sync_with("");
        assert!(
            sync.blocked.is_empty() && sync.proposals.is_empty(),
            "{sync:?}"
        );
        let saved = f.setup.load().unwrap().unwrap();
        assert_eq!(saved.master.shared, format!("{before}A safe new rule.\n"));
        assert_eq!(
            saved.credentials.rules[Path::new(".agents/AGENTS.md")].lines,
            [source.lines().count()],
        );
        assert_eq!(f.text(".agents/AGENTS.md"), source);
        for path in [CLAUDE_RULES, CODEX_RULES, GEMINI_RULES] {
            assert!(f.text(path).contains("A safe new rule."), "{path}");
            assert!(!f.text(path).contains(FIXTURE_VALUE), "{path}");
        }
        assert!(!holds(&f.setup.data, FIXTURE_VALUE.as_bytes()));
    }

    #[test]
    fn rulesync_share_plans_against_an_edited_source_and_appends_only_approved_lines() {
        let f = Fixture::new();
        rule_fixture(&f);
        let original_wording = "Keep this wording in the source.";
        let revised_wording = "An edit approved in Wenlan.";
        f.write(
            ".agents/AGENTS.md",
            &format!("{original_wording}\r\nA line to edit in the source.\r\n"),
        );
        write(
            &f,
            &[
                (".agents/config.toml", "# preserve settings\r\n"),
                (
                    ".agents/rules/local.md",
                    "Preserve this file without a newline.",
                ),
            ],
        );
        adopt(&f);
        approve_shared_edit(&f, original_wording, revised_wording);
        let old_plan = preview_rules(&f, &[]);
        let original = format!("{original_wording}\r\n{COMMON_RULE}");
        f.write(".agents/AGENTS.md", &original);
        let harness = f.setup.home.join(".agents");
        let before_preview = tree(&f.setup.home);
        let mut before = tree(&harness);
        let plan = preview_rules(&f, &[]);

        assert_eq!(tree(&f.setup.home), before_preview);
        assert_eq!(plan.rules.len(), 1);
        assert_eq!(plan.rules[0].line, REVIEW_RULE);
        assert_eq!(
            serde_json::to_value(&plan).unwrap()["shared_source_notice"],
            "Also adds 1 line to ~/.agents/AGENTS.md."
        );
        assert_ne!(plan.plan_id, old_plan.plan_id);
        assert!(apply_rules(&f, &old_plan, &[]).is_err());
        assert_eq!(tree(&f.setup.home), before_preview);
        let result = apply_rules(&f, &plan, &[]).unwrap();
        assert!(
            result.blocked.is_empty() && result.proposals.is_empty(),
            "{result:?}"
        );
        let expected = format!("{original}\r\n{REVIEW_RULE}");
        before.get_mut(Path::new("AGENTS.md")).unwrap().0 = Some(expected.as_bytes().to_vec());
        assert_eq!(tree(&harness), before);
        assert_eq!(
            f.setup.load().unwrap().unwrap().master.shared,
            expected.replace(original_wording, revised_wording),
        );
        assert!(f.text(".agents/AGENTS.md").ends_with(REVIEW_RULE));
        for path in [CLAUDE_RULES, CODEX_RULES, GEMINI_RULES] {
            assert!(f.text(path).contains(revised_wording), "{path}");
            assert!(f.text(path).contains(REVIEW_RULE), "{path}");
        }
    }

    #[test]
    fn rulesync_share_retries_a_source_edit_that_has_not_reached_tools() {
        let f = Fixture::new();
        rule_fixture(&f);
        let retained = "Keep this shared rule.\n";
        let deleted = f.text(".agents/AGENTS.md");
        f.write(SHARED_RULES, &format!("{deleted}{retained}"));
        adopt(&f);
        f.write(SHARED_RULES, retained);
        // Record the edit without writing the tool files, as an interrupted
        // delivery can do. The old shared line is not a local share candidate.
        let mut manifest = f.setup.load().unwrap().unwrap();
        f.setup.follow_shared_source(&mut manifest).unwrap();
        let plan = preview_rules(&f, &[]);
        assert!(plan.rules.iter().all(|rule| rule.line != deleted.trim()));
        let result = apply_rules(&f, &plan, &[]).unwrap();
        assert!(result.blocked.is_empty(), "{result:?}");
        for path in [".agents/AGENTS.md", CLAUDE_RULES, CODEX_RULES, GEMINI_RULES] {
            assert!(!f.text(path).contains(&deleted), "{path}");
            assert!(f.text(path).contains(COMMON_RULE), "{path}");
        }
    }

    #[test]
    fn rulesync_restore_keeps_source_edits_made_between_shares() {
        let f = Fixture::new();
        rule_fixture(&f);
        adopt(&f);
        apply_rules(&f, &preview_rules(&f, &[]), &[]).unwrap();
        accept_local_rule_additions(&f, "Another rule for every tool.\n");
        let source = f
            .text(".agents/AGENTS.md")
            .replace("Already shared.", "An edit between shares.");
        f.write(".agents/AGENTS.md", &source);
        apply_rules(&f, &preview_rules(&f, &[]), &[]).unwrap();
        let before_restore = f.text(".agents/AGENTS.md");

        let result = f.setup.restore().unwrap();
        assert!(result
            .blocked
            .iter()
            .any(|line| line.contains("edits you made between shares")));
        assert_eq!(f.text(".agents/AGENTS.md"), before_restore);
    }

    #[test]
    fn rulesync_adding_a_tool_uses_the_latest_shared_source() {
        let f = Fixture::new();
        f.adopt_from_plan();
        let source = "A new version of the shared source.\n";
        f.write(".agents/AGENTS.md", source);
        fs::create_dir(f.setup.home.join(".gemini")).unwrap();

        adopt(&f);
        assert_eq!(f.text(GEMINI_RULES), source);
        assert_eq!(f.setup.load().unwrap().unwrap().master.shared, source);
        let sync = f.sync_with("");
        assert!(
            sync.blocked.is_empty() && sync.proposals.is_empty(),
            "{sync:?}"
        );
        for path in [CLAUDE_RULES, CODEX_RULES, GEMINI_RULES] {
            assert!(f.text(path).contains(source), "{path}");
        }
    }

    #[test]
    fn rulesync_share_refuses_a_projection_that_drops_a_local_line() {
        let f = Fixture::new();
        rule_fixture(&f);
        adopt(&f);
        let plan = preview_rules(&f, &[]);
        let before = tree(&f.setup.home);
        let render = |target: &str, source: &str| {
            fixture_rule_renderer(target, &source.replace(CLAUDE_RULE, ""))
        };
        let error = f
            .setup
            .apply_share(
                &plan.token,
                KeyAnswer::No,
                Some(&[]),
                &[],
                true,
                (render, fixture_skill_renderer),
            )
            .unwrap_err();
        assert!(
            error.to_string().contains("would change other rules"),
            "{error:#}"
        );
        assert_eq!(tree(&f.setup.home), before);
        assert_eq!(preview_rules(&f, &[]).plan_id, plan.plan_id);
    }

    /// Enroll three tools before adding the settings under test, so those
    /// entries are genuinely unmanaged when share discovers them.
    fn unmanaged_fixture() -> Fixture {
        let f = Fixture::new();
        write(
            &f,
            &[
                (".codex/config.toml", ""),
                (".claude.json", EMPTY_CLAUDE),
                (".gemini/GEMINI.md", "Gemini rules.\n"),
                (".gemini/settings.json", "{\"mcpServers\":{}}\n"),
            ],
        );
        adopt(&f);
        f
    }

    #[test]
    fn ux_skill_26_rulesync_share_adopts_unmanaged_mcp_before_projecting() {
        const SOURCE: &str = "[mcp_servers.context7]\ncommand = 'npx'\nargs = ['-y', '@upstash/context7-mcp']\n\n[mcp_servers.wenlan]\ncommand = 'wenlan-mcp'\nargs = ['--agent-name', 'codex']\n";
        const CLAUDE_PROJECT: &str = "{\"mcpServers\":{},\"projects\":{\"/fixture\":{\"mcpServers\":{\"project-only\":{\"command\":\"project-only-server\"}}}}}\n";
        let f = unmanaged_fixture();
        write(
            &f,
            &[
                (".codex/config.toml", SOURCE),
                (".claude.json", CLAUDE_PROJECT),
            ],
        );
        let before = f.setup.preview_share_using(fixture_skill_renderer).unwrap();
        for destination in ["Claude Code", "Gemini CLI"] {
            assert_eq!(tool(&before, destination).mcp_servers, ["context7"]);
        }
        assert!(!serde_json::to_string(&before)
            .unwrap()
            .contains("wenlan-mcp"));

        let report = f
            .setup
            .share_using(&before.token, fixture_skill_renderer)
            .unwrap();
        assert!(report.blocked.is_empty(), "{report:?}");
        for path in [".claude.json", ".gemini/settings.json"] {
            let text = fs::read_to_string(f.setup.home.join(path)).unwrap();
            assert!(
                text.contains("context7") && text.contains("@upstash/context7-mcp"),
                "{path}: {text}"
            );
            assert!(!text.contains("wenlan-mcp"), "{path}: {text}");
            if path == ".gemini/settings.json" {
                assert!(!text.contains("project-only-server"), "{text}");
            }
        }
        let manifest = f.setup.load().unwrap().unwrap();
        let entries = &manifest.personal_settings[Path::new(".codex/config.toml")];
        assert!(entries.iter().any(|entry| entry
            .key
            .name
            .last()
            .is_some_and(|name| name == "context7")
            && entry.receipt.is_some()));
        assert!(manifest
            .backup
            .contains_key(Path::new(".codex/config.toml")));
        let again = f.setup.preview_share_using(fixture_skill_renderer).unwrap();
        assert!(again.tools.is_empty(), "{again:?}");
        assert_eq!(
            f.setup
                .share_using(&again.token, fixture_skill_renderer)
                .unwrap()
                .message,
            NOTHING_TO_SHARE
        );

        let restored = f.setup.restore().unwrap();
        assert!(restored.blocked.is_empty(), "{restored:?}");
        assert_eq!(
            fs::read_to_string(f.setup.home.join(".codex/config.toml")).unwrap(),
            SOURCE
        );
        for path in [".claude.json", ".gemini/settings.json"] {
            assert!(!fs::read_to_string(f.setup.home.join(path))
                .unwrap()
                .contains("context7"));
        }
        assert!(fs::read_to_string(f.setup.home.join(".claude.json"))
            .unwrap()
            .contains("project-only-server"));
    }

    #[test]
    fn rulesync_share_unmanaged_set_changes_plan_even_when_already_everywhere() {
        let f = unmanaged_fixture();
        write(
            &f,
            &[(
                ".codex/config.toml",
                "[mcp_servers.one]\ncommand = 'server-one'\n",
            )],
        );
        let first = f.setup.preview_share_using(fixture_skill_renderer).unwrap();
        write(&f, &[
            (".codex/config.toml", "[mcp_servers.one]\ncommand = 'server-one'\n[mcp_servers.two]\ncommand = 'server-two'\n"),
            (".claude.json", "{\"mcpServers\":{\"two\":{\"command\":\"server-two\"}}}\n"),
            (".gemini/settings.json", "{\"mcpServers\":{\"two\":{\"command\":\"server-two\"}}}\n"),
        ]);
        let second = f.setup.preview_share_using(fixture_skill_renderer).unwrap();
        assert_ne!(first.plan_id, second.plan_id);
        assert!(f
            .setup
            .share_using(&first.token, fixture_skill_renderer)
            .unwrap_err()
            .to_string()
            .contains("--dry-run"));
        for destination in ["Claude Code", "Gemini CLI"] {
            assert_eq!(tool(&second, destination).mcp_servers, ["one"]);
        }
    }

    #[test]
    fn rulesync_share_unmanaged_source_uses_adoption_stale_check() {
        let f = unmanaged_fixture();
        write(
            &f,
            &[(
                ".codex/config.toml",
                "[mcp_servers.one]\ncommand = 'server-one'\n",
            )],
        );
        let root = FileRoot::open_home(&f.setup.home).unwrap();
        let manifest = f.setup.load().unwrap().unwrap();
        let plan = f
            .setup
            .plan_share(
                &root,
                &manifest,
                &SharedSource::read(&f.setup.home, &manifest).unwrap(),
                None,
                &fixture_skill_renderer,
            )
            .unwrap();
        let source = offered_unmanaged_sources(&plan.settings);
        assert!(!source.sources.is_empty());
        write(
            &f,
            &[(
                ".codex/config.toml",
                "[mcp_servers.one]\ncommand = 'changed'\n",
            )],
        );
        let error =
            personal_settings::adopt(&f.setup.home, &root, &manifest, source, &mut Vec::new())
                .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("changed after the settings preview"),
            "{error:#}"
        );
    }

    #[test]
    fn rulesync_share_rereads_unmanaged_donor_before_enrolling_copies() {
        let f = unmanaged_fixture();
        let donor = f.setup.home.join(".codex/config.toml");
        fs::write(&donor, keyed_codex()).unwrap();
        let plan = f.setup.preview_share_using(fixture_skill_renderer).unwrap();
        assert_eq!(tool(&plan, "Claude Code").mcp_servers, ["safe"]);
        edit_before_share_enrollment(move || {
            fs::write(donor, keyed_codex().replace("safe-server", "edited-server")).unwrap();
        });

        let result = f
            .setup
            .share_using(&plan.token, fixture_skill_renderer)
            .unwrap();
        assert!(result.written.is_empty(), "{result:?}");
        assert!(
            result.blocked.iter().any(|line| {
                line.contains(".codex/config.toml")
                    && line.contains("changed after the settings preview")
            }),
            "{result:?}"
        );
        assert_eq!(f.text(".claude.json"), EMPTY_CLAUDE);
        assert_eq!(f.text(".gemini/settings.json"), "{\"mcpServers\":{}}\n");
        let manifest = f.setup.load().unwrap().unwrap();
        assert!(!manifest
            .personal_settings
            .contains_key(Path::new(".codex/config.toml")));
    }

    #[test]
    fn rulesync_share_confirm_writes_only_approved_entries_in_the_destination() {
        const A: &str = "[mcp_servers.alpha]\ncommand = 'alpha-server'\n";
        const B: &str = "[mcp_servers.bravo]\ncommand = 'bravo-server'\n";
        let f = Fixture::new();
        write(
            &f,
            &[(".codex/config.toml", ""), (".claude.json", EMPTY_CLAUDE)],
        );
        adopt(&f);
        f.write(".codex/config.toml", A);
        let first = f.setup.preview_share_using(fixture_skill_renderer).unwrap();
        let target = f.setup.home.join(".claude.json");
        edit_before_share_enrollment(move || {
            fs::write(target, r#"{"mcpServers":{"alpha":{"command":"occupied"}}}"#).unwrap();
        });
        f.setup
            .share_using(&first.token, fixture_skill_renderer)
            .unwrap();
        assert!(f.text(".claude.json").contains("occupied"));
        let alpha_pending = || {
            let manifest = f.setup.load().unwrap().unwrap();
            manifest.personal_settings[Path::new(".claude.json")]
                .iter()
                .any(|state| {
                    state.key.name.last().is_some_and(|name| name == "alpha")
                        && state.before.is_none()
                })
        };
        assert!(alpha_pending());

        write(
            &f,
            &[
                (".claude.json", EMPTY_CLAUDE),
                (".codex/config.toml", &format!("{A}\n{B}")),
            ],
        );
        let second = f.setup.preview_share_using(fixture_skill_renderer).unwrap();
        assert_eq!(tool(&second, "Claude Code").mcp_servers, ["bravo"]);
        let request = AdoptRequest {
            confirmed_plan_id: Some(second.plan_id.clone()),
            token: second.token.clone(),
            home: Some(f.setup.home.clone()),
            include_keys: false,
        };
        super::super::validate_confirmed_share(&request, &second).unwrap();
        let result = f
            .setup
            .share_with_keys(
                &request.token,
                KeyAnswer::of(true, &request),
                fixture_skill_renderer,
            )
            .unwrap();
        assert!(result.blocked.is_empty(), "{result:?}");
        let contents = f.text(".claude.json");
        assert!(
            contents.contains("bravo-server") && !contents.contains("alpha-server"),
            "{contents}"
        );
        assert!(alpha_pending());

        let synced = f.sync_with("");
        assert!(synced.blocked.is_empty(), "{synced:?}");
        let contents = f.text(".claude.json");
        assert!(
            contents.contains("alpha-server") && contents.contains("bravo-server"),
            "{contents}"
        );
    }

    #[test]
    fn rulesync_share_jsonc_comment_never_enters_a_restore_receipt() {
        const COMMENT: &str = "sk-commentFixtureSentinel0123456789abcdef";
        let f = unmanaged_fixture();
        let refused = format!(
            "{{\"mcpServers\":{{\"wenlan\":{{\"command\":\"wenlan-mcp\"}} // {COMMENT}\n}}}}\n"
        );
        // An existing separator lets the insertion leave this comment in place
        // without spanning it.
        let allowed = refused.replace("} //", "}, //");
        write(
            &f,
            &[
                (
                    ".codex/config.toml",
                    "[mcp_servers.safe]\ncommand = 'safe-server'\n",
                ),
                (".claude.json", &refused),
                (".gemini/settings.json", &allowed),
            ],
        );
        let plan = f.setup.preview_share_using(fixture_skill_renderer).unwrap();
        let request = AdoptRequest {
            confirmed_plan_id: Some(plan.plan_id.clone()),
            token: plan.token.clone(),
            home: Some(f.setup.home.clone()),
            include_keys: false,
        };
        super::super::validate_confirmed_share(&request, &plan).unwrap();
        let result = f
            .setup
            .share_with_keys(
                &request.token,
                KeyAnswer::of(true, &request),
                fixture_skill_renderer,
            )
            .unwrap();
        assert!(result.blocked.is_empty(), "{result:?}");
        let manifest = f.setup.load().unwrap().unwrap();
        for path in [".gemini/settings.json", ".claude.json"] {
            let receipt = manifest
                .personal_settings
                .get(Path::new(path))
                .and_then(|states| states.iter().find_map(|state| state.receipt.as_ref()));
            if path == ".gemini/settings.json" {
                assert!(
                    receipt.is_some(),
                    "the safe insertion should have a receipt"
                );
            }
            if let Some(receipt) = receipt {
                let encoded = serde_json::to_value(receipt).unwrap();
                for fragment in [
                    &encoded["delivered"],
                    &encoded["original"]["Inserted"]["prefix"],
                    &encoded["original"]["Inserted"]["suffix"],
                    &encoded["original"]["Inserted"]["replaced"],
                ] {
                    let decoded: Vec<u8> = serde_json::from_value(fragment.clone()).unwrap();
                    assert!(
                        !decoded
                            .windows(COMMENT.len())
                            .any(|part| part == COMMENT.as_bytes()),
                        "{path} kept comment bytes in its restore receipt"
                    );
                }
            }
        }
        assert!(
            plan.stays.iter().any(|stay| {
                stay.item == "MCP server safe"
                    && stay.reason.contains(".claude.json")
                    && stay.reason.contains("comments")
            }),
            "{plan:?}"
        );
        assert_eq!(
            fs::read(f.setup.home.join(".claude.json")).unwrap(),
            refused.as_bytes()
        );
        assert!(f.text(".gemini/settings.json").contains(COMMENT));
        assert!(f.text(".gemini/settings.json").contains("safe-server"));

        let restored = f.setup.restore().unwrap();
        assert!(restored.blocked.is_empty(), "{restored:?}");
        assert_eq!(f.text(".gemini/settings.json"), allowed);
    }

    #[test]
    fn ux_skill_26_rulesync_share_names_an_unmanaged_unsafe_server() {
        let f = unmanaged_fixture();
        write(
            &f,
            &[(
                ".codex/config.toml",
                "[mcp_servers.unsafe_server]\ncommand = 'server'\nunexpected = true\n",
            )],
        );
        let plan = f.setup.preview_share_using(fixture_skill_renderer).unwrap();
        assert!(
            plan.stays
                .iter()
                .any(|stay| stay.item == "MCP server unsafe_server"
                    && stay.reason.contains("cannot read")),
            "{plan:?}"
        );
        assert!(!serde_json::to_string(&plan).unwrap().contains("entry "));
    }

    #[test]
    fn rulesync_share_binds_the_count_of_unmanaged_redacted_servers_without_hashing_names() {
        const FIRST: &str = "sk-proj-abcdefghijklmnopqrstuvwxyz012345";
        const SECOND: &str = "sk-proj-zyxwvutsrqponmlkjihgfedcba543210";
        let f = unmanaged_fixture();
        let server = |name: &str| format!("[mcp_servers.{name}]\ncommand = 'server'\n");
        write(&f, &[(".codex/config.toml", &server(FIRST))]);
        let first = f.setup.preview_share_using(fixture_skill_renderer).unwrap();
        assert!(first
            .stays
            .iter()
            .any(|stay| stay.item == "an MCP server in .codex/config.toml"));
        write(&f, &[(".codex/config.toml", &server(SECOND))]);
        let renamed = f.setup.preview_share_using(fixture_skill_renderer).unwrap();
        assert_eq!(
            first.plan_id, renamed.plan_id,
            "a redacted name must not enter the hash"
        );
        write(
            &f,
            &[(
                ".codex/config.toml",
                &format!("{}{}", server(FIRST), server(SECOND)),
            )],
        );
        let added = f.setup.preview_share_using(fixture_skill_renderer).unwrap();
        assert_ne!(renamed.plan_id, added.plan_id);
        let shown = serde_json::to_string(&added).unwrap();
        assert!(!shown.contains(FIRST) && !shown.contains(SECOND));
        assert!(f
            .setup
            .share_using(&renamed.token, fixture_skill_renderer)
            .unwrap_err()
            .to_string()
            .contains("--dry-run"));
    }

    #[test]
    fn rulesync_share_sorts_refusals_after_redacting_names() {
        let f = unmanaged_fixture();
        let settings = |name: &str| {
            format!(
                "[mcp_servers.{name}]\ncommand = 'server'\n\n\
             [mcp_servers.refused]\ncommand = 'server'\nunexpected = true\n"
            )
        };
        f.write(
            ".codex/config.toml",
            &settings("ghp_redactedFixture0123456789abcdef"),
        );
        let first = f.setup.preview_share_using(fixture_skill_renderer).unwrap();
        assert_eq!(first.stays.len(), 2);
        f.write(
            ".codex/config.toml",
            &settings("sk-redactedFixture0123456789abcdef"),
        );
        let second = f.setup.preview_share_using(fixture_skill_renderer).unwrap();
        assert_eq!(first.plan_id, second.plan_id);
        assert_eq!(first.stays, second.stays);
    }

    #[test]
    fn ux_skill_26_rulesync_share_offers_each_item_to_every_supporting_tool_and_restores() {
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
        assert_eq!(plan.plan_id, adoption_plan_id(plan_token(&plan.token)));
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

    #[test]
    fn ux_skill_26_rulesync_share_names_refused_servers_without_exposing_unsafe_names() {
        const SECRET_NAME: &str = "sk-proj-abcdefghijklmnopqrstuvwxyz012345";
        let f = Fixture::new();
        let codex = format!(
            "[mcp_servers.unique]\ncommand = 'server'\nunexpected = true\n\n\
             [mcp_servers.duplicate]\ncommand = 'server'\nunexpected = true\n\n\
             [mcp_servers.{SECRET_NAME}]\ncommand = 'server'\n"
        );
        let claude = r#"{"mcpServers":{"duplicate":{"command":"server","unexpected":true}}}"#;
        write(
            &f,
            &[(".codex/config.toml", &codex), (".claude.json", claude)],
        );
        adopt(&f);
        let plan = f.setup.preview_share_using(fixture_skill_renderer).unwrap();
        let labels: Vec<_> = plan.stays.iter().map(|stay| stay.item.as_str()).collect();
        assert!(labels.contains(&"MCP server unique"), "{labels:?}");
        assert!(
            labels
                .iter()
                .any(|label| label.contains("MCP server duplicate")
                    && label.contains(".codex/config.toml")),
            "{labels:?}"
        );
        assert!(
            labels
                .iter()
                .any(|label| label.contains("MCP server duplicate")
                    && label.contains(".claude.json")),
            "{labels:?}"
        );
        assert!(
            labels
                .iter()
                .any(|label| label.contains("an MCP server in .codex/config.toml")),
            "{labels:?}"
        );
        let shown = serde_json::to_string(&plan).unwrap();
        assert!(!shown.contains(SECRET_NAME), "{shown}");
        assert!(!shown.contains("entry "), "{shown}");
    }

    #[test]
    fn ux_skill_26_rulesync_share_disambiguates_conflicting_server_names() {
        let f = Fixture::new();
        write(
            &f,
            &[
                (
                    ".codex/config.toml",
                    "[mcp_servers.same]\ncommand = 'codex-server'\n\n[mcp_servers.keyed]\ncommand = 'codex-key'\nenv = { API_KEY = 'fixture-key-one' }\n",
                ),
                (
                    ".claude.json",
                    r#"{"mcpServers":{"same":{"command":"claude-server"},"keyed":{"command":"claude-key","env":{"API_KEY":"fixture-key-two"}}}}"#,
                ),
            ],
        );
        adopt(&f);
        let plan = f.setup.preview_share_using(fixture_skill_renderer).unwrap();
        for name in ["same", "keyed"] {
            for file in [".codex/config.toml", ".claude.json"] {
                assert!(
                    plan.stays
                        .iter()
                        .any(|stay| stay.item == format!("MCP server {name} in {file}")),
                    "{name} in {file}: {:?}",
                    plan.stays
                );
            }
        }
    }

    #[test]
    fn rulesync_adopt_uses_singular_and_plural_personal_settings() {
        for (config, expected) in [
            ("[mcp_servers.one]\ncommand = 'server-one'\n", "Imported 1 personal setting."),
            ("[mcp_servers.one]\ncommand = 'server-one'\n[mcp_servers.two]\ncommand = 'server-two'\n", "Imported 2 personal settings."),
        ] {
            let f = Fixture::new();
            write(&f, &[(".codex/config.toml", config)]);
            let preview = f.setup.preview_adoption_using(fixture_rule_renderer, fixture_skill_renderer).unwrap();
            let report = f.setup.adopt_using(&preview.token, fixture_rule_renderer, fixture_skill_renderer).unwrap();
            assert!(report.message.contains(expected), "{report:?}");
        }
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
    fn ux_skill_27_rulesync_share_unmanaged_key_requires_person_and_stores_no_value_or_hash() {
        use sha2::{Digest, Sha256};

        let f = unmanaged_fixture();
        write(&f, &[(".codex/config.toml", &keyed_codex())]);
        let root = FileRoot::open_home(&f.setup.home).unwrap();
        let manifest = f.setup.load().unwrap().unwrap();
        let view = personal_settings::View::read(&f.setup.home, &root, &manifest).unwrap();
        let (location, file) = view
            .files
            .iter()
            .find(|(location, _)| location.path == Path::new(".codex/config.toml"))
            .unwrap();
        let key_entry = file
            .as_ref()
            .unwrap()
            .doc
            .refused_servers(location.tool)
            .find(|(_, entry, _)| entry.key.name.last().is_some_and(|name| name == "keyed"))
            .unwrap()
            .1;
        let entry_hash = fingerprint(key_entry).unwrap();
        let secret_hash = format!("{:x}", Sha256::digest(KEY_SECRET.as_bytes()));
        let plan = f.setup.preview_share_using(fixture_skill_renderer).unwrap();
        assert_eq!(plan.key_servers, ["keyed"]);
        assert_eq!(plan.key_tools, ["Claude Code", "Gemini CLI"]);
        assert!(!serde_json::to_string(&plan).unwrap().contains(KEY_SECRET));

        // Changing only the key material must not enter even the plan hash.
        let changed = keyed_codex().replace(KEY_SECRET, "sk-aDifferentSentinel0123456789abcdef");
        write(&f, &[(".codex/config.toml", &changed)]);
        assert_eq!(
            plan.plan_id,
            f.setup
                .preview_share_using(fixture_skill_renderer)
                .unwrap()
                .plan_id
        );
        write(&f, &[(".codex/config.toml", &keyed_codex())]);

        let request = AdoptRequest {
            confirmed_plan_id: Some(plan.plan_id.clone()),
            token: plan.token.clone(),
            home: Some(f.setup.home.clone()),
            include_keys: true,
        };
        super::super::validate_confirmed_share(&request, &plan).unwrap();
        let answer = KeyAnswer::of(true, &request);
        assert_eq!(answer, KeyAnswer::NotAsked);
        let report = f
            .setup
            .share_with_keys(&request.token, answer, fixture_skill_renderer)
            .unwrap();
        assert!(
            report.message.contains("Only the person can share"),
            "{report:?}"
        );
        for path in [".claude.json", ".gemini/settings.json"] {
            let text = fs::read_to_string(f.setup.home.join(path)).unwrap();
            assert!(
                text.contains("safe-server")
                    && !text.contains("keyed-server")
                    && !text.contains(KEY_SECRET),
                "{path}: {text}"
            );
        }
        for needle in [
            KEY_SECRET.as_bytes(),
            entry_hash.as_bytes(),
            secret_hash.as_bytes(),
        ] {
            assert!(
                !holds(&f.setup.data, needle),
                "key value or hash reached the data folder"
            );
        }

        let report = share_keys(&f, KeyAnswer::Yes);
        assert!(report.blocked.is_empty(), "{report:?}");
        for path in [".claude.json", ".gemini/settings.json"] {
            assert!(fs::read_to_string(f.setup.home.join(path))
                .unwrap()
                .contains(KEY_SECRET));
        }
        for needle in [
            KEY_SECRET.as_bytes(),
            entry_hash.as_bytes(),
            secret_hash.as_bytes(),
        ] {
            assert!(
                !holds(&f.setup.data, needle),
                "key value or hash reached the data folder"
            );
        }
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
    fn rulesync_share_refuses_a_keyed_endpoint_changed_after_the_person_preview() {
        const BEFORE: &str = "https://one.example.invalid/mcp";
        const AFTER: &str = "https://two.example.invalid/mcp";
        let f = unmanaged_fixture();
        let donor = |endpoint: &str| {
            format!(
                "[mcp_servers.keyed]\nurl = '{endpoint}'\nhttp_headers = {{ 'Authorization' = 'Bearer {KEY_SECRET}' }}\n"
            )
        };
        let preview = || f.setup.preview_share_using(fixture_skill_renderer).unwrap();
        let entry_hash = || {
            let root = FileRoot::open_home(&f.setup.home).unwrap();
            let manifest = f.setup.load().unwrap().unwrap();
            let view = personal_settings::View::read(&f.setup.home, &root, &manifest).unwrap();
            let (location, file) = view
                .files
                .iter()
                .find(|(location, _)| location.path == Path::new(".codex/config.toml"))
                .unwrap();
            let (_, entry, _) = file
                .as_ref()
                .unwrap()
                .doc
                .refused_servers(location.tool)
                .next()
                .unwrap();
            fingerprint(entry).unwrap()
        };

        f.write(".codex/config.toml", &donor(BEFORE));
        let original = preview();
        let first_hash = entry_hash();
        assert_eq!(original.key_servers, ["keyed"]);
        f.write(".codex/config.toml", &donor(AFTER));
        let fresh = preview();
        let second_hash = entry_hash();
        assert_eq!(original.plan_id, fresh.plan_id);

        let result = f
            .setup
            .share_with_keys(&original.token, KeyAnswer::Yes, fixture_skill_renderer)
            .unwrap();
        assert!(result.written.is_empty(), "{result:?}");
        assert!(
            result
                .blocked
                .iter()
                .any(|line| line.contains("changed after the preview")),
            "{result:?}"
        );
        assert_eq!(f.text(".claude.json"), EMPTY_CLAUDE);
        assert_eq!(f.text(".gemini/settings.json"), "{\"mcpServers\":{}}\n");
        let shown = serde_json::to_string(&[original, fresh]).unwrap();
        let secret_hash = fingerprint(&KEY_SECRET).unwrap();
        for needle in [
            KEY_SECRET,
            BEFORE,
            AFTER,
            &first_hash,
            &second_hash,
            &secret_hash,
        ] {
            assert!(!shown.contains(needle));
            assert!(!holds(&f.setup.data, needle.as_bytes()));
        }
    }

    #[test]
    fn ux_skill_27_rulesync_share_copies_a_key_server_on_yes_and_restores_without_storing_it() {
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
                .ends_with("Left 1 server that may contain a key where it is."),
            "{shared:?}"
        );
        assert!(
            shared
                .kept_in_place
                .iter()
                .any(|kept| kept.tool == "codexcli" && kept.count == 1),
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
    fn rulesync_status_checks_key_share_receipt_against_live_source_and_destination() {
        let (f, _) = key_fixture();
        share_keys(&f, KeyAnswer::Yes);

        // The destination still contains the recorded copy, but its live
        // source has changed since the receipt was written.
        let source = f.setup.home.join(".codex/config.toml");
        let original = fs::read_to_string(&source).unwrap();
        fs::write(&source, original.replace("keyed-server", "changed-server")).unwrap();

        let status = f.setup.status().unwrap();
        assert!(
            status.notes.iter().any(|note| {
                note.starts_with("keyed in Claude Code was removed or changed since sharing")
                    && note.contains("Claude Code")
                    && note.contains("Review both tools before syncing")
            }),
            "{status:?}"
        );
        assert!(!status.in_sync, "{status:?}");
        assert!(
            !status.notes.iter().any(|note| note.contains(KEY_SECRET)),
            "status must not expose key material: {status:?}"
        );

        fs::write(&source, original).unwrap();
        let destination = f.setup.home.join(".claude.json");
        fs::remove_file(&destination).unwrap();
        let status = f.setup.status().unwrap();
        assert!(
            status.notes.iter().any(|note| {
                note.starts_with("keyed in Claude Code was removed or changed since sharing")
            }),
            "{status:?}"
        );
        assert!(!status.in_sync, "{status:?}");
        assert_eq!(
            status
                .tools
                .iter()
                .find(|tool| tool.tool.id == "claude-code")
                .map(|tool| tool.state),
            Some(wenlan_types::rulesync::RulesToolState::NeedsReview),
            "{status:?}"
        );
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
                "Left 1 server that may contain a key where it is. Only the person can share it, by running `wenlan rules share` in their own terminal."
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
    fn rulesync_key_share_post_rename_error_retains_intent_then_claims_receipt() {
        let (f, _) = key_fixture();
        let mut root = FileRoot::open_home(&f.setup.home).unwrap();
        root.inject_key_file_error_after_rename();
        let mut manifest = f.setup.load().unwrap().unwrap();
        let share = f
            .setup
            .plan_share(
                &root,
                &manifest,
                &SharedSource::read(&f.setup.home, &manifest).unwrap(),
                None,
                &fixture_skill_renderer,
            )
            .unwrap();
        assert_eq!(share.keys.len(), 1);

        let mut result = report("share key fixture");
        let token = KeyPreview::remember(&f.setup, share.plan.token, share.key_digests);
        let preview = KeyPreview::take(&f.setup, &token).unwrap();
        f.setup
            .apply_key_shares(&root, &mut manifest, share.keys, &preview, &mut result)
            .unwrap();
        assert!(
            result
                .blocked
                .iter()
                .any(|line| line.contains("injected post-rename error")),
            "{result:?}"
        );

        let mut manifest = f.setup.load().unwrap().unwrap();
        assert_eq!(manifest.keys.intents.len(), 1, "{manifest:?}");
        assert!(manifest.keys.shares.is_empty(), "{manifest:?}");
        assert!(f.text(".claude.json").contains("keyed-server"));
        assert!(!data_holds_key(&f));

        f.setup.claim_key_intents(&root, &mut manifest).unwrap();
        assert!(manifest.keys.intents.is_empty(), "{manifest:?}");
        assert_eq!(manifest.keys.shares.len(), 1, "{manifest:?}");
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
            .plan_share(
                &root,
                &manifest,
                &SharedSource::read(&f.setup.home, &manifest).unwrap(),
                None,
                &fixture_skill_renderer,
            )
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
        // The refused server stays in place and no longer blocks review.
        assert!(
            reviewed.written.iter().any(|path| path == ".claude.json")
                && reviewed.blocked.is_empty()
                && reviewed
                    .kept_in_place
                    .iter()
                    .any(|kept| { kept.tool == "claudecode" && kept.count == 1 }),
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
