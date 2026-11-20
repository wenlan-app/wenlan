// SPDX-License-Identifier: Apache-2.0
//! Owns the on-disk personal manifest and its immutable, private file payloads.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{AdoptedSkill, Delivery, Manifest, Pending, PersonalMaster, Setup};
use crate::rulesync_apply::{ApplyOutcome, Snapshot};

const MANIFEST: &str = "personal.json";
const BLOBS: &str = "personal-blobs";
const FORMAT: u32 = 2;

#[derive(Debug, Serialize, Deserialize)]
#[serde(transparent)]
struct BlobHash(String);

impl BlobHash {
    fn of(bytes: &[u8]) -> Self {
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
struct StoredSetup {
    format: u32,
    state: Manifest<BlobHash>,
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
        let path = Path::new(MANIFEST);
        let bytes = match root.read_private(path, None)? {
            Snapshot::File { bytes, .. } => bytes,
            Snapshot::Absent => return Ok(None),
            _ => bail!("unexpected personal rules master file type"),
        };
        let stored: StoredSetup = serde_json::from_slice(&bytes).context(
            "unsupported or damaged personal rules state. For an earlier format, run wenlan rules restore with the earlier build and the same tool home and data directory",
        )?;
        ensure!(
            stored.format == FORMAT,
            "unsupported personal rules state format {}",
            stored.format
        );

        let mut cache = BTreeMap::<String, Vec<u8>>::new();
        let loaded = map_contents(&stored.state, |hash| {
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
        });
        // An unlocked status reader can race a successful restore, which
        // removes the manifest first and then retires its blob directory.
        match loaded {
            Ok(manifest) => Ok(Some(manifest)),
            Err(_) if root.read_private(path, None)? == Snapshot::Absent => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub(super) fn save(&self, manifest: &Manifest) -> Result<()> {
        let root = self.data_files()?.with_private_directories();
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

    pub(super) fn retire_backup(&self) -> Result<()> {
        let root = self.data_files()?.with_private_directories();
        let path = Path::new(MANIFEST);
        let seen = root.read(path, false)?;
        ensure!(
            root.apply(path, &seen, &Snapshot::Absent, false)? == ApplyOutcome::Applied,
            "the restored state changed at the end of restore"
        );
        root.sync_directory(Path::new(""))?;
        if let Err(error) = root.remove_private_directory(Path::new(BLOBS)) {
            tracing::warn!(%error, "could not remove the restored personal rules blobs");
        }
        Ok(())
    }
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
            Snapshot::Directory => Snapshot::Directory,
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
            codex_sync_replaced: master.codex_sync_replaced,
            skills,
            overrides: master.overrides.clone(),
        },
        backup: manifest
            .backup
            .iter()
            .map(|(path, snap)| Ok((path.clone(), map_snapshot(snap)?)))
            .collect::<Result<_>>()?,
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
        recovery_issues: manifest.recovery_issues.clone(),
        adopted: manifest.adopted,
        settings_only: manifest.settings_only,
        settings_batch: manifest.settings_batch.clone(),
        registry_skipped: manifest.registry_skipped.clone(),
        unmanaged_skills: manifest.unmanaged_skills.clone(),
        worktree_writes: manifest.worktree_writes.clone(),
        scope_reviews: manifest.scope_reviews.clone(),
        delivery_issues: manifest.delivery_issues.clone(),
    })
}
