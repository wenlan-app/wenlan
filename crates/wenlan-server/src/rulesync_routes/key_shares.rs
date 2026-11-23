// SPDX-License-Identifier: Apache-2.0
//! MCP servers that may hold a key. Only the person can share them, after a
//! second yes. Wenlan keeps no copy of such a server anywhere: no value, no
//! hash and no whole-file backup of a file that holds one. It records only
//! where a copy went, by name, and reads the live source again whenever it
//! writes, claims or takes a copy out.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use wenlan_core::rulesync_personal::DeliveryWorktrees;
use wenlan_core::rulesync_settings::personal::{
    mcp_refusal, project_key, same_key_server, Catalog, Entry, Feature, Key, Location, Refusal,
};
use wenlan_types::rulesync::{RulesSetupCode as Code, RulesSyncReport};

use super::personal_settings::{
    forget_restore, managed_entry, read, reserved, restore_changes, snapshot_bytes, tool_name,
    LiveFile, View,
};
use super::{with_bytes, Kind, Manifest, Setup};
use crate::rulesync_apply::{ApplyOutcome, FileRoot, Snapshot};

/// Where one shared copy went. An intent is saved before the write and
/// becomes a receipt once the write is known to have landed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct KeyShare {
    pub(super) destination: PathBuf,
    pub(super) key: Key,
    /// The source tool id, file and server key.
    pub(super) tool: String,
    pub(super) source: PathBuf,
    pub(super) source_key: Key,
    /// When it was shared; zero in a plan.
    pub(super) at: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct KeyState {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) shares: Vec<KeyShare>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) intents: Vec<KeyShare>,
    /// Plain lines for status about copies left as they are.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) notes: Vec<String>,
}

impl KeyState {
    pub(super) fn is_empty(&self) -> bool {
        self.shares.is_empty() && self.intents.is_empty() && self.notes.is_empty()
    }

    /// Whether a file holds, or is about to hold, a shared copy.
    pub(super) fn targets(&self, path: &Path) -> bool {
        self.shares
            .iter()
            .chain(&self.intents)
            .any(|share| share.destination == path)
    }

    /// Whether an entry is a shared copy or the source of one.
    pub(super) fn covers(&self, path: &Path, key: &Key) -> bool {
        self.shares.iter().any(|share| {
            (share.destination == path && share.key == *key)
                || (share.source == path && share.source_key == *key)
        })
    }

    fn note(&mut self, line: String) {
        if !self.notes.contains(&line) {
            self.notes.push(line);
        }
    }
}

/// The servers that may hold a key and where each would be copied.
#[derive(Default)]
pub(super) struct KeyGroup {
    pub(super) shares: Vec<KeyShare>,
    /// Display names of the tools that would receive a copy.
    pub(super) tools: Vec<String>,
    /// (tool id, item, reason)
    pub(super) stays: Vec<(&'static str, String, String)>,
    pub(super) already_there: usize,
}

impl KeyGroup {
    pub(super) fn servers(&self) -> Vec<String> {
        let names: BTreeSet<_> = self.shares.iter().map(server_name).collect();
        names.into_iter().collect()
    }
}

fn server_name(share: &KeyShare) -> String {
    share.key.name.last().cloned().unwrap_or_default()
}

/// Plan the key group from the files a share preview already read.
/// `taken` holds the destinations safe additions will use.
pub(super) fn plan(
    view: &View,
    manifest: &Manifest,
    worktrees: &mut DeliveryWorktrees,
    taken: &BTreeSet<(PathBuf, Key)>,
) -> Result<KeyGroup> {
    let mut group = KeyGroup::default();
    type Candidate<'a> = (KeyShare, &'static str, &'static str, &'a Entry);
    let mut wanted = BTreeMap::<(PathBuf, Key), Vec<Candidate>>::new();
    for (location, file) in &view.files {
        let Ok(file) = file else {
            continue;
        };
        if !view.enrolled.contains(&location.path) {
            continue;
        }
        for (_, source, refusal) in file.doc.refused_servers(location.tool) {
            // A copy Wenlan made is not offered back as a new source.
            if refusal != Refusal::MayHoldKey
                || manifest
                    .keys
                    .shares
                    .iter()
                    .any(|share| share.destination == location.path && share.key == source.key)
            {
                continue;
            }
            let Some(name) = source.key.name.last().cloned() else {
                continue;
            };
            let item = format!("MCP server {name}");
            let mut candidates = BTreeMap::<&str, Vec<_>>::new();
            for (to, other) in &view.files {
                if to.tool == location.tool || !view.enrolled.contains(&to.path) {
                    continue;
                }
                let Ok(other) = other else {
                    continue;
                };
                if !other.doc.holds(Feature::Mcp) {
                    continue;
                }
                candidates.entry(to.tool).or_default().push((to, other));
            }
            for files in candidates.into_values() {
                let target = tool_name(files[0].0.tool);
                let named = files
                    .iter()
                    .filter(|(to, other)| {
                        project_key(location, to, source, &other.doc).is_some_and(|copy| {
                            other.doc.has(&copy.key)
                                || managed_entry(manifest, &to.path, &copy.key).is_some()
                        })
                    })
                    .collect::<Vec<_>>();
                let (to, other) = match named.as_slice() {
                    [only] => **only,
                    [] => match files
                        .iter()
                        .find(|(_, other)| other.snapshot != Snapshot::Absent)
                    {
                        Some(found) => *found,
                        None => {
                            group.stays.push((
                                location.tool,
                                item.clone(),
                                format!("{target} has no settings file yet"),
                            ));
                            continue;
                        }
                    },
                    _ => {
                        group.stays.push((
                            location.tool,
                            item.clone(),
                            format!("{target} has competing settings files"),
                        ));
                        continue;
                    }
                };
                let Some(copy) = project_key(location, to, source, &other.doc) else {
                    group.stays.push((
                        location.tool,
                        item.clone(),
                        format!("it uses settings {target} cannot represent"),
                    ));
                    continue;
                };
                if let Some(existing) = other.doc.entry(&copy.key) {
                    if same_key_server(location.tool, source, to.tool, existing) {
                        group.already_there += 1;
                        continue;
                    }
                }
                let refusal = if other.doc.has(&copy.key)
                    || managed_entry(manifest, &to.path, &copy.key).is_some()
                    || taken.contains(&(to.path.clone(), copy.key.clone()))
                {
                    Some(format!(
                        "{target} already has a different server with this name"
                    ))
                } else if reserved(manifest, &to.path)
                    || manifest
                        .keys
                        .intents
                        .iter()
                        .any(|intent| intent.destination == to.path)
                {
                    Some(format!("{target}'s settings file has an unfinished write"))
                } else if worktrees.at(&to.path)?.is_some() {
                    Some(format!("{target}'s settings file is inside a Git worktree"))
                } else if !snapshot_bytes(&other.snapshot)
                    .is_some_and(|bytes| to.key_round_trip(bytes, &copy))
                {
                    Some(format!(
                        "{target}'s settings file is laid out in a way Wenlan could not put back exactly"
                    ))
                } else {
                    None
                };
                if let Some(reason) = refusal {
                    group.stays.push((location.tool, item.clone(), reason));
                    continue;
                }
                let share = KeyShare {
                    destination: to.path.clone(),
                    key: copy.key.clone(),
                    tool: location.tool.to_owned(),
                    source: location.path.clone(),
                    source_key: source.key.clone(),
                    at: 0,
                };
                wanted
                    .entry((to.path.clone(), copy.key))
                    .or_default()
                    .push((share, location.tool, to.tool, source));
            }
        }
    }
    for (_, mut candidates) in wanted {
        let (first, from, to, source) = candidates.remove(0);
        if candidates
            .iter()
            .all(|(_, other, _, entry)| same_key_server(from, source, other, entry))
        {
            let name = tool_name(to).to_owned();
            if !group.tools.contains(&name) {
                group.tools.push(name);
            }
            group.shares.push(first);
            continue;
        }
        for (share, from, ..) in std::iter::once((first, from, to, source)).chain(candidates) {
            group.stays.push((
                from,
                format!("MCP server {}", server_name(&share)),
                format!(
                    "another tool has a different MCP server with this name for {}",
                    tool_name(to)
                ),
            ));
        }
    }
    Ok(group)
}

/// The live source, its location and the destination location of a share.
struct Live {
    from: Location,
    to: Location,
    source: Option<Entry>,
    destination: Result<LiveFile, &'static str>,
}

impl Live {
    fn read(home: &Path, root: &FileRoot, share: &KeyShare) -> Result<Self> {
        let catalog = Catalog::read(home)?;
        let find = |path: &Path| {
            catalog
                .locations
                .iter()
                .find(|location| location.path == path)
                .cloned()
                .context("the settings path is no longer recognized")
        };
        let (from, to) = (find(&share.source)?, find(&share.destination)?);
        let source = read(home, root, &from).ok().and_then(|file| {
            file.doc
                .entry(&share.source_key)
                .filter(|entry| mcp_refusal(from.tool, entry) == Some(Refusal::MayHoldKey))
                .cloned()
        });
        let destination = read(home, root, &to);
        Ok(Self {
            from,
            to,
            source,
            destination,
        })
    }

    /// Whether the destination holds the server exactly as the live source.
    fn matches(&self, share: &KeyShare) -> bool {
        let (Some(source), Ok(destination)) = (&self.source, &self.destination) else {
            return false;
        };
        destination
            .doc
            .entry(&share.key)
            .is_some_and(|copy| same_key_server(self.from.tool, source, self.to.tool, copy))
    }
}

impl Setup {
    /// Turn interrupted writes into receipts when the copy is there and the
    /// same as the live source. Anything else is left as it is and noted.
    pub(super) fn claim_key_intents(&self, root: &FileRoot, manifest: &mut Manifest) -> Result<()> {
        if manifest.keys.intents.is_empty() {
            return Ok(());
        }
        for intent in std::mem::take(&mut manifest.keys.intents) {
            let live = Live::read(&self.home, root, &intent)?;
            if live.matches(&intent) {
                manifest.keys.shares.push(intent);
            } else {
                manifest.keys.note(format!(
                    "Left {} in {} as it is: an earlier share was interrupted.",
                    server_name(&intent),
                    tool_name(live.to.tool)
                ));
            }
        }
        self.save(manifest)
    }

    /// Copy each consented server into its destination, in place, one entry
    /// at a time. The intent reaches disk before the write.
    pub(super) fn apply_key_shares(
        &self,
        root: &FileRoot,
        manifest: &mut Manifest,
        shares: Vec<KeyShare>,
        result: &mut RulesSyncReport,
    ) -> Result<()> {
        let at = chrono::Utc::now().timestamp();
        for mut share in shares {
            share.at = at;
            manifest.keys.intents.push(share.clone());
            self.save(manifest)?;
            let outcome = self.write_key_share(root, &share);
            manifest.keys.intents.retain(|intent| *intent != share);
            let display = format!("~/{}", share.destination.display());
            match outcome {
                Ok(true) => {
                    result.written.push(share.destination.display().to_string());
                    manifest.keys.shares.push(share);
                }
                Ok(false) => result.block(
                    Code::ReviewNeeded,
                    format!(
                        "{display}: {} was not shared because a file changed while sharing.",
                        server_name(&share)
                    ),
                ),
                Err(error) => result.block(
                    Code::ReviewNeeded,
                    format!("{display}: {} was not shared. {error}", server_name(&share)),
                ),
            }
            self.save(manifest)?;
        }
        Ok(())
    }

    fn write_key_share(&self, root: &FileRoot, share: &KeyShare) -> Result<bool> {
        let live = Live::read(&self.home, root, share)?;
        let source = live.source.as_ref().context("the source server changed")?;
        let file = live
            .destination
            .as_ref()
            .map_err(|reason| anyhow::anyhow!("{reason}"))?;
        let copy = project_key(&live.from, &live.to, source, &file.doc)
            .filter(|copy| copy.key == share.key && !file.doc.has(&copy.key))
            .context("the destination changed")?;
        let bytes = snapshot_bytes(&file.snapshot).context("the settings file is missing")?;
        anyhow::ensure!(
            live.to.key_round_trip(bytes, &copy),
            "the settings file could not be put back exactly after removing it"
        );
        let next = live.to.insert_key(bytes, &copy)?;
        let desired = with_bytes(&file.snapshot, next);
        Ok(
            root.apply(&share.destination, &file.snapshot, &desired, false)?
                == ApplyOutcome::Applied,
        )
    }

    /// Take shared copies out, and restore safe entries in files that hold a
    /// key, one entry at a time and without a whole-file copy. Returns the
    /// paths handled here so the journaled restore leaves them alone, and a
    /// line for each copy kept because it changed since sharing.
    pub(super) fn restore_key_files(
        &self,
        root: &FileRoot,
        manifest: &mut Manifest,
        result: &mut RulesSyncReport,
    ) -> Result<(BTreeSet<PathBuf>, Vec<String>)> {
        let mut paths: BTreeSet<PathBuf> = manifest
            .keys
            .shares
            .iter()
            .map(|share| share.destination.clone())
            .collect();
        let mut catalog = Catalog::read(&self.home)?;
        catalog.retain_managed_locations(manifest.personal_settings.keys().map(PathBuf::as_path));
        let other_delivery = |path: &Path| {
            manifest
                .delivered
                .get(path)
                .is_some_and(|delivery| !matches!(delivery.kind, Kind::PersonalSettings { .. }))
        };
        paths.retain(|path| !other_delivery(path));
        for path in manifest.personal_settings.keys() {
            if other_delivery(path) {
                continue;
            }
            let Some(location) = catalog
                .locations
                .iter()
                .find(|location| location.path == *path)
            else {
                continue;
            };
            if !manifest.backup.contains_key(path)
                || read(&self.home, root, location).is_ok_and(|file| !file.safe)
            {
                paths.insert(path.clone());
            }
        }
        let mut handled = BTreeSet::new();
        let mut kept = Vec::new();
        for path in paths {
            let Some(location) = catalog
                .locations
                .iter()
                .find(|location| location.path == path)
                .cloned()
            else {
                continue;
            };
            handled.insert(path.clone());
            self.restore_key_file(root, manifest, &location, result, &mut kept)?;
            self.save(manifest)?;
        }
        Ok((handled, kept))
    }

    fn restore_key_file(
        &self,
        root: &FileRoot,
        manifest: &mut Manifest,
        location: &Location,
        result: &mut RulesSyncReport,
        kept: &mut Vec<String>,
    ) -> Result<()> {
        let path = location.path.clone();
        let display = location.display();
        let shares: Vec<_> = manifest
            .keys
            .shares
            .iter()
            .filter(|share| share.destination == path)
            .cloned()
            .collect();
        let changes = restore_changes(manifest, &path);
        if shares.is_empty() && changes.is_empty() {
            forget_restore(manifest, &path);
            return Ok(());
        }
        if root.restore_leaf_alias(&path, |_| false)?.is_some() {
            result.block(
                Code::RestoreBlocked,
                format!("{display} could not be restored. The settings path changed spelling."),
            );
            return Ok(());
        }
        let live = match read(&self.home, root, location) {
            Ok(live) => live,
            Err(reason) => {
                result.block(
                    Code::RestoreBlocked,
                    format!("{display} could not be restored. {reason}."),
                );
                return Ok(());
            }
        };
        let Some(bytes) = snapshot_bytes(&live.snapshot) else {
            // The person removed the file; nothing of Wenlan's is left in it.
            manifest
                .keys
                .shares
                .retain(|share| share.destination != path);
            forget_restore(manifest, &path);
            return Ok(());
        };
        let mut next = bytes.to_vec();
        let kept_before = kept.len();
        // Shared copies come out first, newest first, and only while each
        // still matches the live source.
        for share in shares.iter().rev() {
            let current = Live::read(&self.home, root, share)?;
            let present = live.doc.has(&share.key);
            if !present {
                continue;
            }
            let removed = current
                .matches(share)
                .then(|| location.remove_key(&next, &share.key).ok())
                .flatten();
            match removed {
                Some(bytes) => next = bytes,
                None => kept.push(format!(
                    "kept {} in {}: changed since sharing; remove it by hand if you don't want it",
                    server_name(share),
                    tool_name(location.tool)
                )),
            }
        }
        let keys_only = next.clone();
        let mut safe_restored = true;
        for (key, receipt) in &changes {
            if let Err(error) = location.restore_entry(&mut next, key, receipt) {
                // A kept copy can sit next to an entry Wenlan added; its
                // recorded surroundings then no longer match.
                let hint = if kept.len() > kept_before {
                    " Remove the kept server, then run `wenlan rules restore` again."
                } else {
                    ""
                };
                result.block(
                    Code::RestoreBlocked,
                    format!("{display} could not be restored. {error}.{hint}"),
                );
                safe_restored = false;
                break;
            }
        }
        if !safe_restored {
            next = keys_only;
        }
        let original = manifest.backup.get(&path).cloned();
        let desired =
            if safe_restored && original == Some(Snapshot::Absent) && location.is_initial(&next) {
                Snapshot::Absent
            } else {
                with_bytes(&live.snapshot, next)
            };
        match root.apply(&path, &live.snapshot, &desired, false)? {
            ApplyOutcome::Applied => {
                if desired != live.snapshot {
                    result.written.push(path.display().to_string());
                }
                manifest
                    .keys
                    .shares
                    .retain(|share| share.destination != path);
                if safe_restored {
                    forget_restore(manifest, &path);
                }
            }
            _ => result.block(
                Code::RestoreBlocked,
                format!("{display} changed while restoring."),
            ),
        }
        Ok(())
    }
}
