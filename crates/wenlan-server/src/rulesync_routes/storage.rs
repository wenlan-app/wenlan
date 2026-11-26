// SPDX-License-Identifier: Apache-2.0
//! Owns the on-disk personal manifest and its immutable, private file payloads.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use wenlan_core::rulesync_personal::{reject_secret_lines, PERSONAL_TARGETS};

use super::{
    is_rule_or_skill, snapshot_has_credentials, AdoptedSkill, Delivery, Kind, Manifest, Pending,
    PersonalMaster, Setup, UnsupportedRulesyncState,
};
use crate::rulesync_apply::{ApplyOutcome, FileRoot, Snapshot};

pub(super) const MANIFEST: &str = "personal.json";
const BLOBS: &str = "personal-blobs";
const FORMAT: u32 = 3;

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(super) struct BlobHash(String);

impl BlobHash {
    pub(super) fn of(bytes: &[u8]) -> Self {
        Self(hex::encode(Sha256::digest(bytes)))
    }

    fn path(&self) -> Result<PathBuf> {
        ensure!(
            self.0.len() == 64
                && self
                    .0
                    .bytes()
                    .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f')),
            "invalid personal rules blob hash"
        );
        Ok(Path::new(BLOBS).join(&self.0))
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StoredSetup {
    format: u32,
    pub(super) state: Manifest<BlobHash>,
}

impl Setup {
    pub(super) fn load_if_present(&self) -> Result<Option<Manifest>> {
        match std::fs::symlink_metadata(self.data.join(MANIFEST)) {
            Ok(_) => self.load(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error).context("check for a local setup backup"),
        }
    }

    pub(super) fn load(&self) -> Result<Option<Manifest>> {
        let root = self.data_files()?.with_private_directories();
        let Some(stored) = StoredSetup::read(&root)? else {
            return Ok(None);
        };
        let loaded = stored.load_contents(&root);
        // An unlocked status reader can race a successful restore, which
        // removes the manifest first and then retires its blob directory.
        match loaded {
            Ok(manifest) => Ok(Some(manifest)),
            Err(_) if root.read_private(Path::new(MANIFEST), None)? == Snapshot::Absent => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub(super) fn save(&self, manifest: &Manifest) -> Result<()> {
        let root = self.data_files()?.with_private_directories();
        save_manifest(&root, manifest)
    }
}

impl StoredSetup {
    pub(super) fn read(root: &FileRoot) -> Result<Option<Self>> {
        let path = Path::new(MANIFEST);
        let bytes = match root.read_private(path, None)? {
            Snapshot::File { bytes, .. } => bytes,
            Snapshot::Absent => return Ok(None),
            _ => bail!("unexpected personal rules master file type"),
        };
        let mut stored: Self = serde_json::from_slice(&bytes).context(UnsupportedRulesyncState)?;
        ensure!(stored.format == FORMAT, UnsupportedRulesyncState);
        discard_legacy_source_receipts(&mut stored.state);
        Ok(Some(stored))
    }

    pub(super) fn load_contents(&self, root: &FileRoot) -> Result<Manifest> {
        let mut cache = BTreeMap::<String, Vec<u8>>::new();
        let manifest = map_contents(&self.state, |hash| {
            if let Some(bytes) = cache.get(&hash.0) {
                return Ok(bytes.clone());
            }
            let blob = hash.path()?;
            let Snapshot::File { bytes, .. } = root.read_private(&blob, None)? else {
                bail!("missing personal rules blob");
            };
            ensure!(
                BlobHash::of(&bytes).0 == hash.0,
                "damaged personal rules blob"
            );
            cache.insert(hash.0.clone(), bytes.clone());
            Ok(bytes)
        })?;
        // Older builds could already have saved credentials. Leave that
        // backup intact and refuse to use it for delivery, restore or review.
        check_saved_rules_and_skills(&manifest)?;
        Ok(manifest)
    }
}

// Retire journaled source writes using the recorded operation kind. Tool
// files still use the ordinary recovery journal.
fn discard_legacy_source_receipts<T>(manifest: &mut Manifest<T>) {
    let mut paths = manifest
        .delivered
        .iter()
        .filter(|(_, receipt)| matches!(receipt.kind, Kind::SharedRuleSource))
        .map(|(path, _)| path.clone())
        .chain(
            manifest
                .pending
                .iter()
                .filter(|op| matches!(op.kind, Some(Kind::SharedRuleSource)))
                .map(|op| op.path.clone()),
        )
        .collect::<BTreeSet<_>>();
    if manifest.legacy_source_following {
        // An interrupted legacy restore could already have cleared the
        // kind. Keep tool backups and their parents for restore recovery.
        paths.extend(
            manifest
                .backup
                .keys()
                .filter(|path| {
                    !manifest.delivered.contains_key(*path)
                        && !manifest.pending.iter().any(|op| op.path == **path)
                        && super::tool_for_path(path).is_none()
                        && !PERSONAL_TARGETS
                            .iter()
                            .any(|target| target.folder().starts_with(path))
                })
                .cloned(),
        );
    }
    if paths.is_empty() {
        return;
    }
    let is_old_source = |path: &Path| paths.contains(path);
    manifest.backup.retain(|path, _| !is_old_source(path));
    manifest.delivered.retain(|path, _| !is_old_source(path));
    manifest.stages.retain(|path, _| !is_old_source(path));
    manifest
        .recovery_issues
        .retain(|path, _| !is_old_source(path));
    if let Some(journal) = &mut manifest.rule_share {
        journal.attempted -= manifest
            .pending
            .iter()
            .take(journal.attempted)
            .filter(|op| is_old_source(&op.path))
            .count();
        journal.new_backups.retain(|path| !is_old_source(path));
        journal.undo_stages.retain(|path, _| !is_old_source(path));
    }
    manifest.pending.retain(|op| !is_old_source(&op.path));
}

fn check_saved_rules_and_skills(manifest: &Manifest) -> Result<()> {
    use wenlan_core::rulesync_personal::credentials::contains_credential_bytes;

    let master = &manifest.master;
    super::personal_rules::validate_personal_metadata(manifest)?;
    ensure!(
        master.unenrolled_overlays.iter().all(|tool| {
            master.overlays.contains_key(tool)
                && PERSONAL_TARGETS.iter().any(|target| target.id == tool)
                && !master
                    .skills
                    .iter()
                    .any(|skill| skill.targets.contains(tool))
        }),
        UnsupportedRulesyncState
    );
    let rule_text = std::iter::once(&master.shared)
        .chain(master.overlays.values())
        .chain(master.claude_imported_rules.iter().flatten())
        .chain(
            master
                .overrides
                .values()
                .flat_map(|local| [&local.base, &local.edited]),
        )
        .chain(&manifest.kept_share_rules)
        .chain(
            manifest
                .authored_rules
                .values()
                .flat_map(|authored| authored.text()),
        )
        .chain(
            manifest
                .scope_reviews
                .values()
                .flat_map(|review| [&review.before, &review.after]),
        )
        .chain(
            manifest
                .delivered
                .values()
                .filter_map(|record| match &record.kind {
                    Kind::Rule { base, source, .. } => Some([base, source]),
                    _ => None,
                })
                .flatten(),
        );
    let is_rules_or_skills_path = |path: &Path| {
        PERSONAL_TARGETS.iter().any(|target| {
            path == Path::new(target.rules.destination)
                || target.skill_roots().any(|root| path.starts_with(root))
        })
    };
    let snapshots = manifest
        .backup
        .iter()
        .filter(|(path, _)| is_rules_or_skills_path(path))
        .map(|(_, original)| original)
        .chain(manifest.restored_rules.values())
        .chain(
            manifest
                .delivered
                .values()
                .filter(|record| is_rule_or_skill(&record.kind))
                .flat_map(|record| {
                    std::iter::once(&record.delivered).chain(record.observed.iter())
                }),
        )
        .chain(
            manifest
                .pending
                .iter()
                .filter(|op| {
                    is_rules_or_skills_path(&op.path)
                        || op.kind.as_ref().is_some_and(is_rule_or_skill)
                })
                .flat_map(|op| [&op.seen, &op.desired]),
        );
    ensure!(
        !rule_text.into_iter().any(|text| reject_secret_lines(text).is_err())
            && !master.skills.iter().flat_map(|skill| skill.files.values())
                .any(|bytes| contains_credential_bytes(bytes))
            && !snapshots.into_iter().any(snapshot_has_credentials),
        "The saved rules or skills setup holds text that looks like a key. Wenlan is leaving the existing backup and tool files in place."
    );
    Ok(())
}

pub(super) fn save_manifest(root: &FileRoot, manifest: &Manifest) -> Result<()> {
    let mut saved = BTreeSet::new();
    let mut added_blob = false;
    let state = map_contents(manifest, |bytes| {
        let hash = BlobHash::of(bytes);
        if saved.insert(hash.0.clone()) {
            let path = hash.path()?;
            match root.read_private(&path, Some(bytes))? {
                Snapshot::Absent => {
                    let desired = Snapshot::file(bytes.clone());
                    ensure!(
                        root.apply(&path, &Snapshot::Absent, &desired, false)?
                            == ApplyOutcome::Applied,
                        "personal rules blob changed while saving"
                    );
                    added_blob = true;
                }
                Snapshot::File { bytes: current, .. } if current == *bytes => (),
                _ => bail!("damaged personal rules blob"),
            }
        }
        Ok(hash)
    })?;
    if added_blob {
        root.sync_directory(Path::new(BLOBS))?;
        root.sync_directory(Path::new(""))?;
    }
    let path = Path::new(MANIFEST);
    let seen = root.read(path, false)?;
    let next = Snapshot::file(serde_json::to_vec(&StoredSetup {
        format: FORMAT,
        state,
    })?);
    ensure!(
        root.apply(path, &seen, &next, false)? == ApplyOutcome::Applied,
        "personal rules state changed during the operation"
    );
    root.sync_directory(Path::new(""))?;
    #[cfg(test)]
    super::MANIFEST_SAVES.set(super::MANIFEST_SAVES.get() + 1);
    Ok(())
}

fn map_contents<From, To>(
    manifest: &Manifest<From>,
    mut map: impl FnMut(&From) -> Result<To>,
) -> Result<Manifest<To>> {
    let master = &manifest.master;
    let skills = master
        .skills
        .iter()
        .map(|skill| {
            let files = skill
                .files
                .iter()
                .map(|(path, bytes)| Ok((path.clone(), map(bytes)?)))
                .collect::<Result<_>>()?;
            Ok(AdoptedSkill {
                name: skill.name.clone(),
                shared: skill.shared,
                targets: skill.targets.clone(),
                locations: skill.locations.clone(),
                files,
                upstream: skill.upstream.clone(),
            })
        })
        .collect::<Result<_>>()?;
    let mut map_snapshot = |snapshot: &Snapshot<From>| {
        Ok::<_, anyhow::Error>(match snapshot {
            Snapshot::Absent => Snapshot::Absent,
            Snapshot::Directory { mode } => Snapshot::Directory { mode: *mode },
            Snapshot::File { bytes, mode } => Snapshot::File {
                bytes: map(bytes)?,
                mode: *mode,
            },
            Snapshot::Symlink { target, directory } => Snapshot::Symlink {
                target: target.clone(),
                directory: *directory,
            },
        })
    };
    Ok(Manifest {
        master: PersonalMaster {
            shared: master.shared.clone(),
            overlays: master.overlays.clone(),
            unenrolled_overlays: master.unenrolled_overlays.clone(),
            claude_imported_rules: master.claude_imported_rules.clone(),
            codex_sync_replaced: master.codex_sync_replaced,
            skills,
            overrides: master.overrides.clone(),
        },
        personal_rule_origins: manifest.personal_rule_origins.clone(),
        personal_rule_history: manifest.personal_rule_history.clone(),
        personal_rule_journal: manifest.personal_rule_journal.clone(),
        page_rules: manifest.page_rules,
        legacy_source_following: manifest.legacy_source_following,
        rule_import_notice_pending: manifest.rule_import_notice_pending,
        team_rules_moved_notice_pending: manifest.team_rules_moved_notice_pending,
        imported_rule_paths: manifest.imported_rule_paths.clone(),
        backup: manifest
            .backup
            .iter()
            .map(|(path, snap)| Ok((path.clone(), map_snapshot(snap)?)))
            .collect::<Result<_>>()?,
        authored_rules: manifest.authored_rules.clone(),
        rule_origins_tracked: manifest.rule_origins_tracked,
        restored_rules: manifest
            .restored_rules
            .iter()
            .map(|(path, snap)| Ok((path.clone(), map_snapshot(snap)?)))
            .collect::<Result<_>>()?,
        restored_tool_file_fingerprints: manifest.restored_tool_file_fingerprints.clone(),
        #[cfg(windows)]
        windows_backup_owners: manifest.windows_backup_owners.clone(),
        delivered: manifest
            .delivered
            .iter()
            .map(|(path, delivery)| {
                Ok((
                    path.clone(),
                    Delivery {
                        kind: delivery.kind.clone(),
                        delivered: map_snapshot(&delivery.delivered)?,
                        observed: delivery
                            .observed
                            .as_ref()
                            .map(&mut map_snapshot)
                            .transpose()?,
                        reviewed: delivery.reviewed,
                        at: delivery.at,
                        rule_revision: delivery.rule_revision.clone(),
                    },
                ))
            })
            .collect::<Result<_>>()?,
        pending: manifest
            .pending
            .iter()
            .map(|op| {
                Ok(Pending {
                    path: op.path.clone(),
                    seen: map_snapshot(&op.seen)?,
                    desired: map_snapshot(&op.desired)?,
                    kind: op.kind.clone(),
                    allow_link: op.allow_link,
                })
            })
            .collect::<Result<_>>()?,
        stages: manifest.stages.clone(),
        rule_share: manifest.rule_share.clone(),
        recovery_issues: manifest.recovery_issues.clone(),
        adopted: manifest.adopted,
        extension_paths: manifest.extension_paths.clone(),
        settings_only: manifest.settings_only,
        settings_batch: manifest.settings_batch.clone(),
        personal_settings: manifest.personal_settings.clone(),
        registry_skipped: manifest.registry_skipped.clone(),
        unmanaged_skills: manifest.unmanaged_skills.clone(),
        unmanaged_personal_files: manifest.unmanaged_personal_files.clone(),
        worktree_writes: manifest.worktree_writes.clone(),
        scope_reviews: manifest.scope_reviews.clone(),
        shared_rule_proposals: manifest.shared_rule_proposals.clone(),
        scope_review_heads: manifest.scope_review_heads.clone(),
        dismissed_skill_updates: manifest.dismissed_skill_updates.clone(),
        kept_share_rules: manifest.kept_share_rules.clone(),
        credentials: manifest.credentials.clone(),
        delivery_issues: manifest.delivery_issues.clone(),
        keys: manifest.keys.clone(),
    })
}
