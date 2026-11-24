// SPDX-License-Identifier: Apache-2.0
//! Personal settings share the setup manifest and generic compare-and-swap writer.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use wenlan_core::rulesync_personal::personal_target;
use wenlan_core::rulesync_personal::{check_delivery_paths, fingerprint, DeliveryWorktrees};
use wenlan_core::rulesync_settings::personal::{
    enrolled, equivalent, project, reviewable, Adoption, Catalog, Document, Entry, Feature, Key,
    Location, RecordedEntry, Refusal,
};
use wenlan_types::rulesync::{
    KeptInPlace, PullbackAction, PullbackCard, PullbackDecision, PullbackKind,
    RulesSetupCode as Code, RulesSyncReport,
};

use super::{with_bytes, Kind, Manifest, Pending, Setup};
use crate::rulesync_apply::{ApplyOutcome, FileRoot, Snapshot};

const PROPOSAL_PREFIX: &str = "setting-";

/// Whether a review id names a personal-settings proposal.
pub(super) fn is_proposal(id: &str) -> bool {
    id.starts_with(PROPOSAL_PREFIX)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Managed {
    pub(super) key: Key,
    // A shared review can be durable before its write succeeds. Only this
    // approved previous entry (or absence) can be replaced on a retry.
    pub(super) before: Option<Entry>,
    pub(super) wanted: Option<Entry>,
    pub(super) receipt: Option<RecordedEntry>,
}

impl Managed {
    fn kept(entry: Entry, receipt: Option<RecordedEntry>) -> Self {
        Self {
            key: entry.key.clone(),
            before: Some(entry.clone()),
            wanted: Some(entry),
            receipt,
        }
    }

    fn waiting_for_absent_entry(&self) -> bool {
        self.before.is_none() && self.wanted.is_some()
    }
}

pub(super) type Source = BTreeMap<PathBuf, Vec<Managed>>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Update {
    pub(super) key: Key,
    pub(super) receipt: RecordedEntry,
}

pub(super) fn settle(manifest: &mut Manifest, path: &Path, updates: Vec<Update>) {
    if let Some(managed) = manifest.personal_settings.get_mut(path) {
        for Update { key, receipt } in updates {
            let Some(index) = managed.iter().position(|entry| entry.key == key) else {
                continue;
            };
            // Preserve delivery order for reversing nested or adjacent insertions.
            let mut entry = managed.remove(index);
            entry.before.clone_from(&entry.wanted);
            entry.receipt = Some(receipt);
            managed.push(entry);
        }
    }
}

pub(super) fn forget_restore(manifest: &mut Manifest, path: &Path) {
    manifest.delivered.remove(path);
    manifest.backup.remove(path);
    manifest.personal_settings.remove(path);
    #[cfg(windows)]
    manifest.windows_backup_owners.remove(path);
}

pub(super) fn prepare_adoption(
    home: &Path,
    adoption: &mut wenlan_core::rulesync_adopt::Adoption,
    manifest: Option<&Manifest>,
) -> Result<()> {
    if let Some(manifest) = manifest {
        adoption.settings.sources.retain(|path, entries| {
            entries.retain(|entry| {
                !manifest.personal_settings.get(path).is_some_and(|known| {
                    known
                        .iter()
                        .any(|known| known.key == entry.key && !known.waiting_for_absent_entry())
                })
            });
            !entries.is_empty()
        });
    }
    let mut worktrees = DeliveryWorktrees::new(home)?;
    for path in adoption.settings.sources.keys() {
        if let Some(root) = worktrees.at(path)? {
            adoption.settings.notes.push(format!(
                "{} is inside the Git worktree at {}. Personal settings delivery can change this checkout.",
                path.display(), worktrees.display(&root),
            ));
            adoption.worktree_writes.insert(path.clone(), root);
        }
    }
    Ok(())
}

pub(super) fn adopt(
    home: &Path,
    root: &FileRoot,
    manifest: &Manifest,
    source: Adoption,
    operations: &mut Vec<Pending>,
) -> Result<Source> {
    if source.sources.is_empty() {
        return Ok(BTreeMap::new());
    }
    let catalog = Catalog::read(home)?;
    let mut prepared = BTreeMap::new();
    for (path, entries) in source.sources {
        let location = catalog
            .locations
            .iter()
            .find(|location| location.path == path)
            .context("a settings source disappeared after preview")?;
        let file = read(home, root, location).map_err(anyhow::Error::msg)?;
        for entry in &entries {
            ensure!(
                file.doc.entry(&entry.key) == Some(entry) && reviewable(location.tool, entry),
                "{} changed after the settings preview. Review the plan again.",
                location.display()
            );
        }
        let claimed = entries
            .into_iter()
            .map(|entry| {
                let receipt = snapshot_bytes(&file.snapshot)
                    .filter(|_| file.safe && entry.can_reapply())
                    .and_then(|bytes| location.record_original(bytes, &file.doc, &entry).ok());
                Managed::kept(entry, receipt)
            })
            .collect::<Vec<_>>();
        let has_receipt = claimed.iter().any(|entry| entry.receipt.is_some());
        prepared.insert(path.clone(), claimed);
        // Settings already present in a tool are adopted in place. Record an
        // exact original only when the whole file is safe to back up.
        if has_receipt && !manifest.delivered.contains_key(&path) && !reserved(manifest, &path) {
            operations.push(Pending {
                path,
                seen: file.snapshot.clone(),
                desired: file.snapshot,
                kind: Some(Kind::PersonalSettings {
                    updates: Vec::new(),
                }),
                allow_link: false,
            });
        }
    }
    Ok(prepared)
}

pub(super) fn enroll(manifest: &mut Manifest, source: Source) {
    for (path, entries) in source {
        let known = manifest.personal_settings.entry(path).or_default();
        known.retain(|known| !entries.iter().any(|entry| entry.key == known.key));
        known.extend(entries);
    }
}

pub(super) struct LiveFile {
    pub(super) snapshot: Snapshot,
    pub(super) doc: Document,
    pub(super) safe: bool,
}

pub(super) fn read(
    home: &Path,
    root: &FileRoot,
    location: &Location,
) -> Result<LiveFile, &'static str> {
    check_delivery_paths(home, [location.path.as_path()])
        .map_err(|_| "the file aliases the shared harness or its path is unsafe")?;
    let snapshot = root
        .read_personal_file(&location.path, false)
        .map_err(|_| "the file is linked (including hard links), unreadable, too large, or its permissions cannot be preserved")?;
    let bytes = match &snapshot {
        Snapshot::File { bytes, .. } => Some(bytes.as_slice()),
        Snapshot::Absent => None,
        _ => return Err("the settings path is not a regular file"),
    };
    let doc = location.document(bytes).map_err(|_| {
        if bytes.is_none() {
            "no native settings file establishes the configuration shape"
        } else {
            location.unsupported_shape()
        }
    })?;
    let safe = bytes.is_none_or(|bytes| location.snapshot_safe(bytes, &doc));
    Ok(LiveFile {
        snapshot,
        doc,
        safe,
    })
}

pub(super) fn snapshot_bytes(snapshot: &Snapshot) -> Option<&[u8]> {
    match snapshot {
        Snapshot::File { bytes, .. } => Some(bytes),
        _ => None,
    }
}

/// The entries Wenlan changed in a file, newest first, with their receipts.
pub(super) fn restore_changes(manifest: &Manifest, path: &Path) -> Vec<(Key, RecordedEntry)> {
    manifest
        .personal_settings
        .get(path)
        .into_iter()
        .flatten()
        .rev()
        .filter_map(|entry| {
            Some((
                entry.key.clone(),
                entry.receipt.clone().filter(|record| record.changed())?,
            ))
        })
        .collect()
}

pub(super) fn restore_operation(
    home: &Path,
    root: &FileRoot,
    manifest: &Manifest,
    path: &Path,
    original: &Snapshot,
) -> Result<Option<Pending>> {
    let changes = restore_changes(manifest, path);
    if changes.is_empty() {
        return Ok(None);
    }
    let mut catalog = Catalog::read(home)?;
    catalog.retain_managed_locations([path]);
    let location = catalog
        .locations
        .iter()
        .find(|location| location.path == path)
        .context("the settings path is no longer recognized")?;
    ensure!(
        root.restore_leaf_alias(path, |_| false)?.is_none(),
        "the settings path changed spelling"
    );
    let live = read(home, root, location).map_err(anyhow::Error::msg)?;
    // A file that holds a key is restored entry by entry before this
    // journaled restore runs (`restore_key_files`), so it never gets here.
    ensure!(live.safe, "the settings file contains possible credentials");
    let mut bytes = snapshot_bytes(&live.snapshot)
        .context("the settings file is missing")?
        .to_vec();
    for (key, receipt) in &changes {
        location.restore_entry(&mut bytes, key, receipt)?;
    }
    let doc = location.document(Some(&bytes))?;
    ensure!(
        location.snapshot_safe(&bytes, &doc),
        "the restored settings would contain possible credentials"
    );
    let desired = if *original == Snapshot::Absent && location.is_initial(&bytes) {
        Snapshot::Absent
    } else {
        with_bytes(&live.snapshot, bytes)
    };
    Ok(Some(Pending {
        path: path.to_owned(),
        seen: live.snapshot,
        desired,
        kind: None,
        allow_link: false,
    }))
}

pub(super) struct View {
    pub(super) files: Vec<(Location, Result<LiveFile, &'static str>)>,
    pub(super) enrolled: BTreeSet<PathBuf>,
}

impl View {
    pub(super) fn mcp_label(&self, location: &Location, name: Option<&str>) -> String {
        match name {
            Some(name) => {
                let duplicate = self.files.iter().any(|(other, file)| {
                    other.path != location.path
                        && file.as_ref().is_ok_and(|file| {
                            file.doc
                                .mcp_server_names(other.tool)
                                .any(|other| other == name)
                        })
                });
                if duplicate {
                    format!("MCP server {name} in {}", location.display())
                } else {
                    format!("MCP server {name}")
                }
            }
            None => format!("an MCP server in {}", location.display()),
        }
    }

    pub(super) fn read(home: &Path, root: &FileRoot, manifest: &Manifest) -> Result<Self> {
        let mut catalog = Catalog::read(home)?;
        catalog.retain_managed_locations(manifest.personal_settings.keys().map(PathBuf::as_path));
        let mut tools: BTreeSet<_> = manifest
            .master
            .overlays
            .keys()
            .map(String::as_str)
            .collect();
        tools.extend(
            catalog
                .locations
                .iter()
                .filter(|location| manifest.personal_settings.contains_key(&location.path))
                .map(|location| location.tool),
        );
        let enrolled: BTreeSet<_> = catalog
            .locations
            .iter()
            .filter(|location| enrolled(home, location, &tools))
            .map(|location| location.path.clone())
            .collect();
        let files = catalog
            .locations
            .into_iter()
            .filter(|location| {
                manifest.personal_settings.contains_key(&location.path)
                    || enrolled.contains(&location.path)
                    || home.join(&location.path).is_file()
            })
            .map(|location| {
                let file = read(home, root, &location);
                (location, file)
            })
            .collect();
        Ok(Self { files, enrolled })
    }

    fn additions(&self, source: &Source) -> usize {
        self.files
            .iter()
            .filter_map(|(location, file)| {
                let file = file.as_ref().ok()?;
                Some(
                    file.doc
                        .reviewable_entries(location.tool)
                        .filter(|entry| {
                            !source.get(&location.path).is_some_and(|known| {
                                known.iter().any(|known| {
                                    known.key == entry.key && !known.waiting_for_absent_entry()
                                })
                            })
                        })
                        .count(),
                )
            })
            .sum()
    }
}

#[derive(Clone, Serialize)]
struct Destination {
    path: PathBuf,
    before: Option<Entry>,
    wanted: Entry,
    worktree: Option<PathBuf>,
}

struct Proposal {
    card: PullbackCard,
    observed: Option<Entry>,
    destinations: Vec<Destination>,
    blocked: Vec<String>,
}

pub(super) fn managed_entry<'a>(
    manifest: &'a Manifest,
    path: &Path,
    key: &Key,
) -> Option<&'a Managed> {
    manifest
        .personal_settings
        .get(path)?
        .iter()
        .find(|entry| &entry.key == key)
}

fn label(state: &Managed) -> String {
    state
        .wanted
        .as_ref()
        .or(state.before.as_ref())
        .map_or_else(|| "personal setting".into(), Entry::label)
}

pub(super) fn reserved(manifest: &Manifest, path: &Path) -> bool {
    manifest
        .keys
        .writes
        .iter()
        .any(|write| write.destination == path)
        || manifest.recovery_issues.contains_key(path)
        || manifest.pending.iter().any(|pending| pending.path == path)
        || manifest
            .settings_batch
            .as_ref()
            .is_some_and(|batch| batch.holds(path))
        || manifest
            .delivered
            .get(path)
            .is_some_and(|delivery| !matches!(delivery.kind, Kind::PersonalSettings { .. }))
}

fn proposal(
    view: &View,
    manifest: &Manifest,
    worktrees: &mut DeliveryWorktrees,
    location: &Location,
    state: &Managed,
    observed: Option<&Entry>,
) -> Result<Proposal> {
    let mut destinations = Vec::new();
    let mut blocked = Vec::new();
    let name = label(state);
    if let Some(observed) = observed {
        // A tool can have multiple physical config files. Select an existing
        // owner first, and refuse competing native definitions for one tool.
        let mut candidates = BTreeMap::<&str, Vec<_>>::new();
        for (to, file) in &view.files {
            if !view.enrolled.contains(&to.path) {
                continue;
            }
            let file = match file {
                Ok(file) => file,
                Err(reason) => {
                    if to.tool == "opencode" && *reason == to.unsupported_shape() {
                        blocked.push(format!("{}: {reason}.", to.display()));
                    }
                    continue;
                }
            };
            let Some(wanted) = project(location, to, observed, &file.doc) else {
                continue;
            };
            candidates
                .entry(to.tool)
                .or_default()
                .push((to, file, wanted));
        }
        for files in candidates.into_values() {
            let named = files
                .iter()
                .filter(|(to, file, wanted)| {
                    file.doc.has(&wanted.key)
                        || managed_entry(manifest, &to.path, &wanted.key).is_some()
                })
                .collect::<Vec<_>>();
            let (to, file, wanted) = match named.as_slice() {
                [only] => *only,
                [] => files
                    .iter()
                    .find(|(_, file, _)| file.snapshot != Snapshot::Absent)
                    .unwrap_or(&files[0]),
                _ => {
                    blocked.push(format!(
                        "{}: {name} has competing settings files.",
                        files[0].0.display()
                    ));
                    continue;
                }
            };
            let display = to.display();
            // A file that also holds a key gets this entry on its own, so
            // only an unfinished write holds the delivery back.
            if reserved(manifest, &to.path) {
                blocked.push(format!(
                    "{display}: {name} cannot be delivered while the file has an unfinished write."
                ));
                continue;
            }
            let current = file.doc.entry(&wanted.key);
            if current.is_some_and(|entry| equivalent(to, entry, wanted)) {
                continue;
            }
            let managed = managed_entry(manifest, &to.path, &wanted.key);
            let replaceable = match (managed, current) {
                (None, None) => !file.doc.has(&wanted.key),
                (Some(managed), current) => {
                    let previous = state
                        .wanted
                        .as_ref()
                        .and_then(|entry| project(location, to, entry, &file.doc));
                    matches!((managed.wanted.as_ref(), current, previous),
                        (Some(owned), Some(seen), Some(previous))
                            if owned == seen && equivalent(to, owned, &previous))
                }
                _ => false,
            };
            if !replaceable {
                blocked.push(format!(
                    "{display}: {name} conflicts with a separate or unreviewed entry."
                ));
                continue;
            }
            destinations.push(Destination {
                path: to.path.clone(),
                before: current.cloned(),
                wanted: wanted.clone(),
                worktree: worktrees.at(&to.path)?,
            });
        }
    }
    let id = format!(
        "{PROPOSAL_PREFIX}{}",
        fingerprint(&(&location.path, state, observed, &destinations, &blocked,))?
    );
    let mut diff = String::new();
    if let Some(entry) = &state.wanted {
        diff.push_str(&format!("-{}\n", entry.preview()?));
    }
    if let Some(entry) = observed {
        diff.push_str(&format!("+{}\n", entry.preview()?));
    }
    if destinations.is_empty() {
        diff.push_str("No other enrolled tool can receive this change safely.\n");
    } else {
        diff.push_str("All-tools delivery\n");
        for destination in &destinations {
            diff.push_str(&format!(
                "  {}\n+{}\n",
                destination.path.display(),
                destination.wanted.preview()?
            ));
            if let Some(worktree) = &destination.worktree {
                diff.push_str(&format!(
                    "  Inside the Git worktree at {}.\n",
                    worktrees.display(worktree)
                ));
            }
        }
    }
    for reason in &blocked {
        diff.push_str(&format!("  {reason}\n"));
    }
    Ok(Proposal {
        card: PullbackCard {
            id,
            files: vec![location.display()],
            diff,
            reason: "A managed personal setting changed. Use wenlan rules review to keep it or share it.".into(),
            reason_code: Some(Code::PersonalFileEdit),
            diff_codes: Vec::new(),
            kind: PullbackKind::Conflict,
        },
        observed: observed.cloned(),
        destinations,
        blocked,
    })
}

/// Whether a managed entry was edited by hand and waits for a review card,
/// the same test sync uses before it offers a settings proposal.
fn awaits_review(
    location: &Location,
    file: &LiveFile,
    state: &Managed,
    current: Option<&Entry>,
) -> bool {
    !(current == state.before.as_ref()
        || current == state.wanted.as_ref()
        || (current.is_none() && file.doc.has(&state.key))
        || state.waiting_for_absent_entry()
        || current
            .into_iter()
            .chain(state.before.iter())
            .chain(state.wanted.iter())
            .any(|entry| !reviewable(location.tool, entry)))
}

/// Personal MCP servers Wenlan manages, counted per tool id. An entry
/// adopted in place counts even when its file has no delivery record.
pub(super) fn managed_mcp_counts(
    home: &Path,
    manifest: &Manifest,
) -> Result<BTreeMap<&'static str, usize>> {
    let mut catalog = Catalog::read(home)?;
    catalog.retain_managed_locations(manifest.personal_settings.keys().map(PathBuf::as_path));
    let mut counts = BTreeMap::new();
    for (path, entries) in &manifest.personal_settings {
        if manifest
            .delivered
            .get(path)
            .is_some_and(|delivery| matches!(delivery.kind, Kind::Settings))
        {
            continue;
        }
        let Some(location) = catalog
            .locations
            .iter()
            .find(|location| &location.path == path)
        else {
            continue;
        };
        let count = entries
            .iter()
            .filter(|entry| entry.key.feature == Feature::Mcp && entry.wanted.is_some())
            .count();
        if count != 0 {
            *counts.entry(location.tool).or_default() += count;
        }
    }
    Ok(counts)
}

/// A managed MCP server another enrolled tool can receive unchanged.
#[derive(Clone, Serialize)]
pub(super) struct SettingAddition {
    pub(super) from_tool: &'static str,
    pub(super) to_tool: &'static str,
    pub(super) name: String,
    item: String,
    destination: Destination,
}

#[derive(Default)]
pub(super) struct SettingShares {
    pub(super) additions: Vec<SettingAddition>,
    /// (tool id, item, reason)
    pub(super) stays: Vec<(&'static str, String, String)>,
    pub(super) already_there: usize,
    /// Servers that may hold a key, shared only with the person's second yes.
    pub(super) keys: super::key_shares::KeyGroup,
}

pub(super) fn tool_name(id: &str) -> &'static str {
    personal_target(id).map_or("this tool", |target| target.tool.display_name())
}

/// Record approved additions as entries waiting for their first write, the
/// same state an all-tools review leaves for `sync_personal_settings`.
pub(super) fn enroll_shares(manifest: &mut Manifest, additions: Vec<SettingAddition>) {
    for addition in additions {
        let Destination {
            path,
            before,
            wanted,
            worktree,
        } = addition.destination;
        let entries = manifest.personal_settings.entry(path.clone()).or_default();
        let key = wanted.key.clone();
        if let Some(current) = entries.iter_mut().find(|entry| entry.key == key) {
            current.before = before;
            current.wanted = Some(wanted);
        } else {
            entries.push(Managed {
                key,
                before,
                wanted: Some(wanted),
                receipt: None,
            });
        }
        if let Some(worktree) = worktree {
            manifest.worktree_writes.insert(path, worktree);
        }
    }
}

impl SettingAddition {
    pub(super) fn path(&self) -> &Path {
        &self.destination.path
    }
}

impl Setup {
    /// Managed MCP servers that are in sync, offered to every other enrolled
    /// tool through the same projection an all-tools review uses.
    pub(super) fn plan_setting_shares(
        &self,
        root: &FileRoot,
        manifest: &Manifest,
    ) -> Result<SettingShares> {
        let view = View::read(&self.home, root, manifest)?;
        let mut worktrees = DeliveryWorktrees::new(&self.home)?;
        let mut shares = SettingShares::default();
        let mut wanted = BTreeMap::<(PathBuf, Key), Vec<SettingAddition>>::new();
        for (location, file) in &view.files {
            let Ok(file) = file else {
                continue;
            };
            let enrolled = view.enrolled.contains(&location.path);
            for (name, entry, refusal) in file.doc.refused_servers(location.tool) {
                // Servers that may hold a key in a managed tool form the key
                // group; a copy Wenlan made is not listed again.
                let reason = match refusal {
                    Refusal::MayHoldKey if enrolled => continue,
                    Refusal::MayHoldKey => "it may contain a key and its tool is not managed",
                    Refusal::Unsafe(reason) => reason,
                };
                if manifest.keys.covers(&location.path, &entry.key) {
                    continue;
                }
                let item = view.mcp_label(location, name);
                shares.stays.push((location.tool, item, reason.into()));
            }
            let Some(managed) = manifest.personal_settings.get(&location.path) else {
                continue;
            };
            if reserved(manifest, &location.path) {
                continue;
            }
            for state in managed {
                if state.key.feature != Feature::Mcp || state.before != state.wanted {
                    continue;
                }
                let Some(current) = file
                    .doc
                    .entry(&state.key)
                    .filter(|current| Some(*current) == state.wanted.as_ref())
                else {
                    continue;
                };
                if !reviewable(location.tool, current) {
                    continue;
                }
                // The server's own name, without the format's table prefix.
                let Some(name) = state.key.name.last().cloned() else {
                    continue;
                };
                let item = view.mcp_label(location, Some(&name));
                let mut unrepresentable = BTreeSet::new();
                for (to, other) in &view.files {
                    if to.tool == location.tool || !view.enrolled.contains(&to.path) {
                        continue;
                    }
                    // A file with no MCP section (Claude's settings.json) is
                    // not where that tool keeps its servers.
                    let Ok(other) = other else {
                        continue;
                    };
                    if !other.doc.holds(Feature::Mcp) {
                        continue;
                    }
                    match project(location, to, current, &other.doc) {
                        Some(projected) => {
                            if other
                                .doc
                                .entry(&projected.key)
                                .is_some_and(|entry| equivalent(to, entry, &projected))
                            {
                                shares.already_there += 1;
                            }
                        }
                        None => {
                            unrepresentable.insert(tool_name(to.tool));
                        }
                    }
                }
                if !unrepresentable.is_empty() {
                    shares.stays.push((
                        location.tool,
                        item.clone(),
                        format!(
                            "it uses settings {} cannot represent",
                            unrepresentable.into_iter().collect::<Vec<_>>().join(", ")
                        ),
                    ));
                }
                let proposal = proposal(
                    &view,
                    manifest,
                    &mut worktrees,
                    location,
                    state,
                    Some(current),
                )?;
                for reason in proposal.blocked {
                    shares.stays.push((location.tool, item.clone(), reason));
                }
                for destination in proposal.destinations {
                    let to_tool = view
                        .files
                        .iter()
                        .find(|(to, _)| to.path == destination.path)
                        .map(|(to, _)| to.tool)
                        .context("a settings destination disappeared")?;
                    wanted
                        .entry((destination.path.clone(), destination.wanted.key.clone()))
                        .or_default()
                        .push(SettingAddition {
                            from_tool: location.tool,
                            to_tool,
                            name: name.clone(),
                            item: item.clone(),
                            destination,
                        });
                }
            }
        }
        let taken = wanted.keys().cloned().collect();
        shares.keys = super::key_shares::plan(&view, manifest, &mut worktrees, &taken)?;
        for (_, mut candidates) in wanted {
            let first = candidates.remove(0);
            if candidates
                .iter()
                .all(|other| other.destination.wanted == first.destination.wanted)
            {
                shares.additions.push(first);
                continue;
            }
            for addition in std::iter::once(first).chain(candidates) {
                shares.stays.push((
                    addition.from_tool,
                    addition.item,
                    format!(
                        "another tool has a different MCP server with this name for {}",
                        tool_name(addition.to_tool)
                    ),
                ));
            }
        }
        Ok(shares)
    }
}

impl Setup {
    /// Settings proposals waiting in To review, read without writing.
    pub(super) fn personal_setting_proposals(
        &self,
        root: &FileRoot,
        manifest: &Manifest,
    ) -> Result<Vec<PullbackCard>> {
        let view = View::read(&self.home, root, manifest)?;
        let mut worktrees = DeliveryWorktrees::new(&self.home)?;
        let mut cards = Vec::new();
        for (location, file) in &view.files {
            let path = &location.path;
            let (Ok(file), Some(managed)) = (file, manifest.personal_settings.get(path)) else {
                continue;
            };
            if reserved(manifest, path) {
                continue;
            }
            for state in managed {
                let current = file.doc.entry(&state.key);
                if awaits_review(location, file, state, current) {
                    cards.push(
                        proposal(&view, manifest, &mut worktrees, location, state, current)?.card,
                    );
                }
            }
        }
        Ok(cards)
    }

    pub(super) fn sync_personal_settings(
        &self,
        root: &FileRoot,
        manifest: &mut Manifest,
        result: &mut RulesSyncReport,
    ) -> Result<()> {
        let view = View::read(&self.home, root, manifest)?;
        let mut worktrees = DeliveryWorktrees::new(&self.home)?;
        let mut operations = Vec::new();
        let mut direct = Vec::new();
        let mut advanced = Vec::new();
        let mut kept_in_place = BTreeMap::<String, usize>::new();
        for (location, file) in &view.files {
            let path = &location.path;
            let display = location.display();
            let file = match file {
                Ok(file) => file,
                Err(reason) => {
                    if manifest.personal_settings.contains_key(path) {
                        result.block(Code::ReviewNeeded, format!("{display}: {reason}."));
                    }
                    continue;
                }
            };
            // A server the person shared, and its copies, are not skipped.
            let shared = file
                .doc
                .refused_servers(location.tool)
                .filter(|(_, entry, _)| manifest.keys.covers(path, &entry.key))
                .count();
            let refused = file
                .doc
                .refused_entries(location.tool)
                .count()
                .saturating_sub(shared);
            if refused > 0 {
                *kept_in_place.entry(location.tool.to_owned()).or_default() += refused;
            }
            // A file that holds, or will hold, a key is never copied whole:
            // each entry is written in place with its own receipt.
            let in_place = !file.safe || manifest.keys.targets(path);
            let Some(managed) = manifest.personal_settings.get(path) else {
                continue;
            };
            if reserved(manifest, path) {
                result.block(
                    Code::ReviewNeeded,
                    format!("{display}: an earlier settings write needs review."),
                );
                continue;
            }
            let mut next = None;
            let mut updates = Vec::new();
            for state in managed {
                let current = file.doc.entry(&state.key);
                if current.is_some_and(|entry| !reviewable(location.tool, entry)) {
                    continue;
                }
                let label = label(state);
                if (current.is_none() && file.doc.has(&state.key))
                    || state
                        .before
                        .iter()
                        .chain(state.wanted.iter())
                        .any(|entry| !reviewable(location.tool, entry))
                {
                    result.block(
                        Code::ReviewNeeded,
                        format!(
                        "{display}: a recorded entry is ambiguous or contains possible credentials."
                    ),
                    );
                } else if current.is_some() && state.waiting_for_absent_entry() {
                    result.block(
                        Code::ReviewNeeded,
                        format!("{display}: {label} conflicts with a new, unowned entry."),
                    );
                } else if current == state.wanted.as_ref() {
                    if state.before != state.wanted {
                        advanced.push((path.clone(), state.key.clone()));
                    }
                } else if current == state.before.as_ref() {
                    let Some(wanted) = &state.wanted else {
                        continue;
                    };
                    if !wanted.can_reapply() {
                        continue;
                    }
                    if state.before.is_some() && state.receipt.is_none() {
                        result.block(
                            Code::ReviewNeeded,
                            format!("{display}: {label} has no exact native restore fragment."),
                        );
                        continue;
                    }
                    let previous = next.as_deref().or_else(|| snapshot_bytes(&file.snapshot));
                    match location.edit(previous, wanted).and_then(|bytes| {
                        let receipt = location.record_write(
                            state.receipt.as_ref(),
                            previous,
                            &bytes,
                            wanted,
                        )?;
                        Ok((bytes, receipt))
                    }) {
                        Ok((bytes, receipt)) => {
                            next = Some(bytes);
                            updates.push(Update {
                                key: state.key.clone(),
                                receipt,
                            });
                        }
                        Err(_) => result.block(
                            Code::ReviewNeeded,
                            format!(
                            "{display}: {label} conflicts with a native entry or settings shape."
                        ),
                        ),
                    }
                } else {
                    let proposal =
                        proposal(&view, manifest, &mut worktrees, location, state, current)?;
                    result.proposals.push(proposal.card);
                }
            }
            if let (Some(bytes), true) = (&next, in_place) {
                let clear = location.document(Some(bytes)).is_ok_and(|doc| {
                    updates
                        .iter()
                        .all(|update| doc.receipt_clear(location.tool, &update.receipt))
                });
                if !clear {
                    result.block(
                        Code::ReviewNeeded,
                        format!("{display}: a setting stayed in place because its restore record could include text from a server that may hold a key."),
                    );
                    continue;
                }
                if let Some(reason) = worktrees.unreviewed(path, &manifest.worktree_writes)? {
                    result.block(Code::GitUnreviewed, format!("{display}: {reason}"));
                    continue;
                }
                direct.push((path.clone(), file.snapshot.clone(), bytes.clone(), updates));
                continue;
            }
            if let Some(bytes) = next {
                // Check the assembled snapshot too, before any part of it
                // can enter the journal, backup or displaced-file storage.
                if !location
                    .document(Some(&bytes))
                    .is_ok_and(|doc| location.snapshot_safe(&bytes, &doc))
                {
                    result.block(
                        Code::ReviewNeeded,
                        format!("{display}: the merged file could contain credentials."),
                    );
                    continue;
                }
                let op = Pending {
                    path: path.clone(),
                    seen: file.snapshot.clone(),
                    desired: with_bytes(&file.snapshot, bytes),
                    kind: Some(Kind::PersonalSettings { updates }),
                    allow_link: false,
                };
                if let Err(reason) = super::displaced::preflight(root, &self.data_files()?, &op) {
                    result.block(Code::ReviewNeeded, format!("{display}: {reason}."));
                    continue;
                }
                operations.push(op);
            }
        }
        for (path, key) in &advanced {
            let entry = manifest
                .personal_settings
                .get_mut(path)
                .unwrap()
                .iter_mut()
                .find(|entry| entry.key == *key)
                .unwrap();
            entry.before.clone_from(&entry.wanted);
        }
        if !advanced.is_empty() {
            self.save(manifest)?;
        }
        for (path, seen, bytes, updates) in direct {
            let desired = with_bytes(&seen, bytes);
            let temp = super::key_shares::begin_write(self, manifest, &path, updates.clone())?;
            let outcome = root.apply_key_file(&path, &temp, &seen, &desired)?;
            if outcome == ApplyOutcome::Applied {
                settle(manifest, &path, updates);
                self.save(manifest)?;
                super::key_shares::finish_write(self, root, manifest, &temp)?;
                result.written.push(path.display().to_string());
            } else {
                super::key_shares::finish_write(self, root, manifest, &temp)?;
                result.block(
                    Code::ReviewNeeded,
                    format!(
                        "{} changed while applying a personal setting.",
                        path.display()
                    ),
                );
            }
        }
        for (path, outcome) in self.sync_batch(root, manifest, operations, result)? {
            if outcome == ApplyOutcome::Applied {
                result.written.push(path.display().to_string());
            } else {
                result.block(
                    Code::ReviewNeeded,
                    format!(
                        "{} changed while applying a personal setting.",
                        path.display()
                    ),
                );
            }
        }
        let additions = view.additions(&manifest.personal_settings);
        if additions != 0 {
            result.message.push_str(&format!(
                "\nFound {additions} additional personal setting(s). Run `wenlan rules adopt` to review and add them."
            ));
        }
        result.kept_in_place = kept_in_place
            .into_iter()
            .map(|(tool, count)| KeptInPlace { tool, count })
            .collect();
        Ok(())
    }

    pub(super) fn review_personal_settings(
        &self,
        root: &FileRoot,
        manifest: &mut Manifest,
        request: &PullbackDecision,
    ) -> Result<Option<Vec<String>>> {
        if !request.id.starts_with(PROPOSAL_PREFIX) {
            return Ok(None);
        }
        let view = View::read(&self.home, root, manifest)?;
        let mut worktrees = DeliveryWorktrees::new(&self.home)?;
        for (location, file) in &view.files {
            let path = &location.path;
            let (Ok(file), Some(managed)) = (file, manifest.personal_settings.get(path)) else {
                continue;
            };
            if reserved(manifest, path) {
                continue;
            }
            for state in managed {
                let current = file.doc.entry(&state.key);
                if !awaits_review(location, file, state, current) {
                    continue;
                }
                let proposed = proposal(&view, manifest, &mut worktrees, location, state, current)?;
                if request.id != proposed.card.id {
                    continue;
                }
                ensure!(
                    request
                        .file
                        .as_deref()
                        .is_none_or(|file| Path::new(file) == path),
                    "this file does not belong to the settings proposal"
                );
                ensure!(
                    root.read_personal_file(path, false)? == file.snapshot,
                    "the tool edited this settings file again. Run rules sync."
                );
                let key = state.key.clone();
                let dismiss_write = matches!(
                    request.action,
                    PullbackAction::Dismiss | PullbackAction::Revert
                ) && state.wanted.as_ref().is_some_and(Entry::can_reapply);
                let wanted = if dismiss_write {
                    state.wanted.clone()
                } else {
                    proposed.observed.clone()
                };
                {
                    let entry = manifest
                        .personal_settings
                        .get_mut(path)
                        .unwrap()
                        .iter_mut()
                        .find(|entry| entry.key == key)
                        .unwrap();
                    entry.before = proposed.observed.clone();
                    entry.wanted = wanted;
                    if !dismiss_write {
                        entry.receipt = entry.receipt.as_ref().and_then(|receipt| {
                            let current = current.filter(|entry| entry.can_reapply())?;
                            let live = snapshot_bytes(&file.snapshot).filter(|_| file.safe)?;
                            location
                                .record_review(receipt, live, &file.doc, current)
                                .ok()
                        });
                    }
                }
                let mut blocked = Vec::new();
                if request.action == PullbackAction::AllTools {
                    blocked = proposed.blocked;
                    for destination in proposed.destinations {
                        let entries = manifest
                            .personal_settings
                            .entry(destination.path.clone())
                            .or_default();
                        let key = destination.wanted.key.clone();
                        if let Some(current) = entries.iter_mut().find(|entry| entry.key == key) {
                            current.before = destination.before;
                            current.wanted = Some(destination.wanted);
                        } else {
                            entries.push(Managed {
                                key,
                                before: destination.before,
                                wanted: Some(destination.wanted),
                                receipt: None,
                            });
                        }
                        if let Some(worktree) = destination.worktree {
                            manifest.worktree_writes.insert(destination.path, worktree);
                        }
                    }
                }
                self.save(manifest)?;
                return Ok(Some(blocked));
            }
        }
        anyhow::bail!(
            "the settings proposal or its destinations changed. Run rules sync for a fresh preview."
        )
    }
}
