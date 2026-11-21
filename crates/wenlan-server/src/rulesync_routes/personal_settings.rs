// SPDX-License-Identifier: Apache-2.0
//! Personal settings share the setup manifest and generic compare-and-swap writer.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use wenlan_core::rulesync_personal::{check_delivery_paths, fingerprint, DeliveryWorktrees};
use wenlan_core::rulesync_settings::personal::{
    enrolled, equivalent, project, reviewable, Adoption, Catalog, Document, Entry, Key, Location,
};
use wenlan_types::rulesync::{PullbackAction, PullbackCard, PullbackDecision, RulesSyncReport};

use super::{with_bytes, Kind, Manifest, Pending, Setup};
use crate::rulesync_apply::{ApplyOutcome, FileRoot, Snapshot};

const PROPOSAL_PREFIX: &str = "setting-";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Managed {
    key: Key,
    // A shared review can be durable before its write succeeds. Only this
    // approved previous entry (or absence) can be replaced on a retry.
    before: Option<Entry>,
    wanted: Option<Entry>,
}

impl Managed {
    fn kept(key: Key, entry: Option<Entry>) -> Self {
        Self {
            key,
            before: entry.clone(),
            wanted: entry,
        }
    }

    fn waiting_for_absent_entry(&self) -> bool {
        self.before.is_none() && self.wanted.is_some()
    }
}

pub(super) type Source = BTreeMap<PathBuf, Vec<Managed>>;

pub(super) fn settle(manifest: &mut Manifest, path: &Path, keys: &[Key]) {
    if let Some(managed) = manifest.personal_settings.get_mut(path) {
        for entry in managed.iter_mut().filter(|entry| keys.contains(&entry.key)) {
            entry.before.clone_from(&entry.wanted);
        }
    }
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
    manifest: &mut Manifest,
    source: Adoption,
    operations: &mut Vec<Pending>,
) -> Result<usize> {
    if source.sources.is_empty() {
        return Ok(0);
    }
    let catalog = Catalog::read(home)?;
    let mut count = 0;
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
        count += entries.len();
        let known = manifest.personal_settings.entry(path.clone()).or_default();
        known.retain(|known| !entries.iter().any(|entry| entry.key == known.key));
        known.extend(
            entries
                .into_iter()
                .map(|entry| Managed::kept(entry.key.clone(), Some(entry))),
        );
        // Settings already present in a tool are adopted in place. Record an
        // exact original only when the whole file is safe to back up.
        if file.safe && !manifest.delivered.contains_key(&path) && !reserved(manifest, &path) {
            operations.push(Pending {
                path,
                seen: file.snapshot.clone(),
                desired: file.snapshot,
                kind: Some(Kind::PersonalSettings {
                    entries: Vec::new(),
                }),
                allow_link: false,
            });
        }
    }
    Ok(count)
}

struct LiveFile {
    snapshot: Snapshot,
    doc: Document,
    safe: bool,
}

fn read(home: &Path, root: &FileRoot, location: &Location) -> Result<LiveFile, &'static str> {
    check_delivery_paths(home, [location.path.as_path()])
        .map_err(|_| "the file aliases the shared harness or its path is unsafe")?;
    let snapshot = root
        .read(&location.path, false)
        .map_err(|_| "the settings file is linked, unreadable, or too large")?;
    let bytes = match &snapshot {
        Snapshot::File { bytes, .. } => Some(bytes.as_slice()),
        Snapshot::Absent => None,
        _ => return Err("the settings path is not a regular file"),
    };
    let doc = location
        .document(bytes)
        .map_err(|_| "the settings format is invalid, duplicate, or unsupported")?;
    let safe = bytes.is_none_or(|bytes| location.snapshot_safe(bytes, &doc));
    Ok(LiveFile {
        snapshot,
        doc,
        safe,
    })
}

fn snapshot_bytes(snapshot: &Snapshot) -> Option<&[u8]> {
    match snapshot {
        Snapshot::File { bytes, .. } => Some(bytes),
        _ => None,
    }
}

struct View {
    files: Vec<(Location, Result<LiveFile, &'static str>)>,
    enrolled: BTreeSet<PathBuf>,
}

impl View {
    fn read(home: &Path, root: &FileRoot, manifest: &Manifest) -> Result<Self> {
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

#[derive(Serialize)]
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

fn managed_entry<'a>(manifest: &'a Manifest, path: &Path, key: &Key) -> Option<&'a Managed> {
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

fn reserved(manifest: &Manifest, path: &Path) -> bool {
    manifest.recovery_issues.contains_key(path)
        || manifest.pending.iter().any(|pending| pending.path == path)
        || manifest
            .settings_batch
            .as_ref()
            .is_some_and(|batch| batch.holds(path))
        || manifest
            .delivered
            .get(path)
            .is_some_and(|delivery| !matches!(delivery.kind, Kind::Settings))
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
            let Ok(file) = file else { continue };
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
            if !file.safe || reserved(manifest, &to.path) {
                blocked.push(format!("{display}: {name} cannot be delivered while the file contains possible credentials or has an unfinished write."));
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
        },
        observed: observed.cloned(),
        destinations,
        blocked,
    })
}

impl Setup {
    pub(super) fn sync_personal_settings(
        &self,
        root: &FileRoot,
        manifest: &mut Manifest,
        result: &mut RulesSyncReport,
    ) -> Result<()> {
        let view = View::read(&self.home, root, manifest)?;
        let mut worktrees = DeliveryWorktrees::new(&self.home)?;
        let mut operations = Vec::new();
        let mut advanced = Vec::new();
        for (location, file) in &view.files {
            let path = &location.path;
            let Some(managed) = manifest.personal_settings.get(path) else {
                continue;
            };
            let display = location.display();
            let file = match file {
                Ok(file) => file,
                Err(reason) => {
                    result.blocked.push(format!("{display}: {reason}."));
                    continue;
                }
            };
            if reserved(manifest, path) {
                result.blocked.push(format!(
                    "{display}: an earlier settings write needs review."
                ));
                continue;
            }
            let mut next = None;
            let mut entries = Vec::new();
            for state in managed {
                let label = label(state);
                let current = file.doc.entry(&state.key);
                if (current.is_none() && file.doc.has(&state.key))
                    || current
                        .into_iter()
                        .chain(state.before.iter())
                        .chain(state.wanted.iter())
                        .any(|entry| !reviewable(location.tool, entry))
                {
                    result.blocked.push(format!(
                        "{display}: {label} is ambiguous or contains possible credentials."
                    ));
                } else if current.is_some() && state.waiting_for_absent_entry() {
                    result.blocked.push(format!(
                        "{display}: {label} conflicts with a new, unowned entry."
                    ));
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
                    if !file.safe {
                        result.blocked.push(format!("{display}: {label} stayed in place because this file cannot be backed up without copying possible credentials."));
                        continue;
                    }
                    match location.edit(
                        next.as_deref().or_else(|| snapshot_bytes(&file.snapshot)),
                        wanted,
                    ) {
                        Ok(bytes) => {
                            next = Some(bytes);
                            entries.push(state.key.clone());
                        }
                        Err(_) => result.blocked.push(format!(
                            "{display}: {label} conflicts with a native entry or settings shape."
                        )),
                    }
                } else {
                    let proposal =
                        proposal(&view, manifest, &mut worktrees, location, state, current)?;
                    result.proposals.push(proposal.card);
                }
            }
            if let Some(bytes) = next {
                // Check the assembled snapshot too, before any part of it
                // can enter the journal, backup or displaced-file storage.
                if !location
                    .document(Some(&bytes))
                    .is_ok_and(|doc| location.snapshot_safe(&bytes, &doc))
                {
                    result.blocked.push(format!(
                        "{display}: the merged file could contain credentials."
                    ));
                    continue;
                }
                operations.push(Pending {
                    path: path.clone(),
                    seen: file.snapshot.clone(),
                    desired: with_bytes(&file.snapshot, bytes),
                    kind: Some(Kind::PersonalSettings { entries }),
                    allow_link: false,
                });
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
        for (path, outcome) in self.sync_batch(root, manifest, operations, result)? {
            if outcome == ApplyOutcome::Applied {
                result.written.push(path.display().to_string());
            } else {
                result.blocked.push(format!(
                    "{} changed while applying a personal setting.",
                    path.display()
                ));
            }
        }
        let additions = view.additions(&manifest.personal_settings);
        if additions != 0 {
            result.message.push_str(&format!(
                "\nFound {additions} additional personal setting(s). Run `wenlan rules adopt` to review and add them."
            ));
        }
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
                if current == state.before.as_ref()
                    || current == state.wanted.as_ref()
                    || (current.is_none() && file.doc.has(&state.key))
                    || state.waiting_for_absent_entry()
                    || current
                        .into_iter()
                        .chain(state.before.iter())
                        .chain(state.wanted.iter())
                        .any(|entry| !reviewable(location.tool, entry))
                {
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
                    root.read(path, false)? == file.snapshot,
                    "the tool edited this settings file again. Run rules sync."
                );
                let key = state.key.clone();
                let dismiss_write = request.action == PullbackAction::Dismiss
                    && state.wanted.as_ref().is_some_and(Entry::can_reapply);
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
                }
                if request.action != PullbackAction::Dismiss && file.safe {
                    if let (Some(delivery), Some(entry), Some(live)) = (
                        manifest.delivered.get_mut(path),
                        current,
                        snapshot_bytes(&file.snapshot),
                    ) {
                        // Approving one entry does not approve unowned edits.
                        // Mask the reviewed value on both versions and retain
                        // the receipt only if every other byte still matches.
                        if let Some(old) = snapshot_bytes(&delivery.delivered) {
                            if matches!((location.edit(Some(old), entry), location.edit(Some(live), entry)),
                                (Ok(before), Ok(after)) if before == after)
                            {
                                delivery.delivered = file.snapshot.clone();
                            }
                        }
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
                        let next = Managed {
                            key: key.clone(),
                            before: destination.before,
                            wanted: Some(destination.wanted),
                        };
                        if let Some(current) = entries.iter_mut().find(|entry| entry.key == key) {
                            *current = next;
                        } else {
                            entries.push(next);
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
        anyhow::bail!("the settings proposal or its destinations changed. Run rules sync for a fresh preview.")
    }
}
