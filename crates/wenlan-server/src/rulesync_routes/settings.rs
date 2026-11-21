// SPDX-License-Identifier: Apache-2.0
//! Resume and restore person-confirmed settings batches through the setup journal.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use wenlan_core::rulesync_personal::PersonalMaster;
use wenlan_types::rulesync::{RulesSetupCode as Code, RulesSyncReport};

use super::{report, BatchMode, Kind, Manifest, Pending, Setup};
use crate::rulesync_apply::{ApplyOutcome, FileRoot, Snapshot};

pub(crate) const SETTINGS_RESUME_NOTICE: &str = "A settings apply was interrupted. Open `wenlan rules review` to review and resume, or run `wenlan rules restore` to undo applied changes.";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct SettingsBatch {
    source_digest: String,
    remaining: BTreeSet<PathBuf>,
}

impl SettingsBatch {
    pub(super) fn holds(&self, path: &Path) -> bool {
        self.remaining.contains(path)
    }

    fn check_source(&self, digest: &str) -> Result<()> {
        ensure!(
            self.source_digest == digest,
            "An earlier settings apply was interrupted. Use the same settings source to resume, or run `wenlan rules restore` to undo applied changes."
        );
        Ok(())
    }
}

impl Setup {
    pub(crate) fn settings_remaining(&self, digest: &str) -> Result<Option<BTreeSet<PathBuf>>> {
        let Some(batch) = self
            .load_if_present()?
            .and_then(|state| state.settings_batch)
        else {
            return Ok(None);
        };
        batch.check_source(digest)?;
        Ok(Some(batch.remaining))
    }

    /// The caller holds the setup lock and has checked the confirmation token.
    pub(crate) fn apply_settings(
        &self,
        root: &FileRoot,
        digest: String,
        writes: Vec<(PathBuf, Snapshot, Snapshot)>,
    ) -> Result<RulesSyncReport> {
        let mut manifest = self.load_if_present()?.unwrap_or_else(settings_only);
        self.recover(root, &mut manifest)?;

        let mut result = report("Settings are already up to date");
        let paths: BTreeSet<_> = writes.iter().map(|(path, _, _)| path.clone()).collect();
        for op in &manifest.pending {
            if matches!(op.kind, Some(Kind::Settings))
                || paths
                    .iter()
                    .any(|path| path.starts_with(&op.path) || op.path.starts_with(path))
            {
                result.block(Code::ChangedDuringSync, format!(
                    "{} changed while Wenlan was applying settings. Review the file before resuming.",
                    op.path.display()
                ));
            }
        }
        if !result.blocked.is_empty() {
            result.message = SETTINGS_RESUME_NOTICE.into();
            return Ok(result);
        }

        if let Some(batch) = &manifest.settings_batch {
            batch.check_source(&digest)?;
            ensure!(
                paths.is_subset(&batch.remaining),
                "Settings progress changed since preview. Run the preview again."
            );
        }
        for (path, seen, _) in &writes {
            if let Some(delivery) = manifest.delivered.get(path) {
                ensure!(
                    matches!(delivery.kind, Kind::Settings) && *seen == delivery.delivered,
                    "{} changed since its last delivery. Review this file and the retained backup before applying more settings.",
                    path.display()
                );
            }
        }
        if writes.is_empty() {
            // The approved source is already present on all remaining paths.
            if manifest.settings_batch.take().is_some() {
                self.save(&manifest)?;
            }
            return Ok(result);
        }

        manifest.settings_batch = Some(SettingsBatch {
            source_digest: digest,
            remaining: paths.clone(),
        });
        let operations = writes
            .into_iter()
            .map(|(path, seen, desired)| Pending {
                path,
                seen,
                desired,
                kind: Some(Kind::Settings),
                allow_link: false,
            })
            .collect();
        let applied = self
            .apply_batch(root, &mut manifest, operations, BatchMode::Settings)
            .context(SETTINGS_RESUME_NOTICE)?;
        result.written = applied
            .into_iter()
            .filter(|(path, outcome)| *outcome == ApplyOutcome::Applied && paths.contains(path))
            .map(|(path, _)| path.display().to_string())
            .collect();
        if let Some(batch) = &manifest.settings_batch {
            result.message = SETTINGS_RESUME_NOTICE.into();
            result.extend_blocks(
                Code::RecoveryNeeded,
                batch
                    .remaining
                    .iter()
                    .map(|path| path.display().to_string()),
            );
        } else {
            result.message = "Applied the approved local settings".into();
        }
        Ok(result)
    }
}

pub(super) fn settle(manifest: &mut Manifest, path: &Path) {
    if let Some(batch) = &mut manifest.settings_batch {
        batch.remaining.remove(path);
        if batch.remaining.is_empty() {
            manifest.settings_batch = None;
        }
    }
}

fn settings_only() -> Manifest {
    let master = PersonalMaster {
        shared: String::new(),
        overlays: BTreeMap::new(),
        codex_sync_replaced: false,
        skills: Vec::new(),
        overrides: BTreeMap::new(),
    };
    let mut manifest = Manifest::new(master, BTreeMap::new());
    manifest.settings_only = true;
    manifest
}
