// SPDX-License-Identifier: Apache-2.0
//! Follow the person's shared rules without writing back to their source.

use super::*;
use wenlan_core::rulesync_adopt::{read_shared_rule_source, SHARED_RULES};
use wenlan_core::rulesync_personal::{reject_secret_lines, RuleTextConflict};

#[derive(Debug)]
pub(super) struct SharedSourceConflict;

impl std::fmt::Display for SharedSourceConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "~/.agents/AGENTS.md and Wenlan changed the same shared lines. Undo the edits to those lines in ~/.agents/AGENTS.md, then run `{SYNC_COMMAND}`."
        )
    }
}

impl std::error::Error for SharedSourceConflict {}

pub(super) struct SharedSource {
    pub read: String,
    pub merged: String,
    pub credential_lines: Vec<usize>,
}

impl SharedSource {
    pub(super) fn read(home: &Path, manifest: &Manifest) -> Result<Self> {
        let (read, credential_lines) = read_shared_rule_source(home).with_context(|| {
            format!("Cannot read ~/.agents/AGENTS.md. Check the file and run `{SYNC_COMMAND}`")
        })?;
        let shared = &manifest.master.shared;
        let path = Path::new(SHARED_RULES);
        let base = match manifest.shared_source_base.as_deref() {
            Some(base) => base,
            None if is_shared_source_receipt(manifest, path) => {
                text(&manifest.delivered[path].delivered)?
            }
            // Setups without a source receipt predate tracking a separate baseline.
            None => shared,
        };
        let merged = merge_rule_text(base, &read, shared).map_err(|error| {
            if error.is::<RuleTextConflict>() {
                SharedSourceConflict.into()
            } else {
                error.context(format!(
                    "Cannot compare the changes in ~/.agents/AGENTS.md. Split the edit into smaller changes and run `{SYNC_COMMAND}`"
                ))
            }
        })?;
        reject_secret_lines(&merged)?;
        Ok(Self {
            read,
            merged,
            credential_lines,
        })
    }

    pub(super) fn record(self, manifest: &mut Manifest) -> bool {
        let path = Path::new(SHARED_RULES);
        let credential = (!self.credential_lines.is_empty()).then_some(RuleCredentialLocation {
            tool: None,
            lines: self.credential_lines,
        });
        if manifest.shared_source_base.as_ref() == Some(&self.read)
            && manifest.master.shared == self.merged
            && manifest.credentials.rules.get(path) == credential.as_ref()
        {
            return false;
        }
        manifest.shared_source_base = Some(self.read);
        manifest.master.shared = self.merged;
        if let Some(credential) = credential {
            manifest.credentials.rules.insert(path.into(), credential);
        } else {
            manifest.credentials.rules.remove(path);
        }
        true
    }
}

impl Setup {
    pub(super) fn follow_shared_source(&self, manifest: &mut Manifest) -> Result<()> {
        if SharedSource::read(&self.home, manifest)?.record(manifest) {
            self.save(manifest)?;
        }
        Ok(())
    }

    pub(super) fn follow_source_for_sync(
        &self,
        manifest: &mut Manifest,
        result: &mut RulesSyncReport,
    ) -> Result<bool> {
        match self.follow_shared_source(manifest) {
            Ok(()) => Ok(true),
            Err(error) if error.is::<SharedSourceConflict>() => {
                result.block(Code::RuleConflict, error.to_string());
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }
}
