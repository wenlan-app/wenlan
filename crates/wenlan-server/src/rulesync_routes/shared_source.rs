// SPDX-License-Identifier: Apache-2.0
//! Follow the person's shared rules without writing back to their source.

use super::*;
use wenlan_core::rulesync_adopt::{read_shared_rule_source, SHARED_RULES};
use wenlan_core::rulesync_personal::{
    merge_shared_rule_text, preserve_rules_at_credential_lines, reject_secret_lines,
    RuleTextConflict,
};

#[derive(Debug)]
pub(super) enum SharedSourceBlocked {
    Conflict,
    MissingOrEmpty,
}

impl std::fmt::Display for SharedSourceBlocked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Conflict => write!(
                f,
                "Rule delivery is paused because changes in ~/.agents/AGENTS.md and Wenlan touch the same shared lines. Edit either side so the changes no longer touch, then run `{SYNC_COMMAND}`."
            ),
            Self::MissingOrEmpty => write!(
                f,
                "Rule delivery is paused because ~/.agents/AGENTS.md is missing or empty. Put your shared rules back there, then run `{SYNC_COMMAND}`."
            ),
        }
    }
}

impl std::error::Error for SharedSourceBlocked {}

pub(super) struct SharedSource {
    /// Safe live text, used with the credential positions to bind approval.
    pub read: String,
    baseline: String,
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
            // Without history a difference could be an approved master edit.
            // Start tracking the source as it is and preserve the saved rules.
            None if read.trim().is_empty() && credential_lines.is_empty() => shared,
            None => &read,
        };
        if read.trim().is_empty() && credential_lines.is_empty() && !base.trim().is_empty() {
            return Err(SharedSourceBlocked::MissingOrEmpty.into());
        }
        let comparison_error = |error: anyhow::Error| {
            if error.is::<RuleTextConflict>() {
                SharedSourceBlocked::Conflict.into()
            } else {
                error.context(format!(
                    "Cannot compare the changes in ~/.agents/AGENTS.md. Split the edit into smaller changes and run `{SYNC_COMMAND}`"
                ))
            }
        };
        let baseline = preserve_rules_at_credential_lines(base, &read, &credential_lines)
            .map_err(comparison_error)?;
        let merged = merge_shared_rule_text(base, &baseline, shared).map_err(comparison_error)?;
        reject_secret_lines(&baseline)?;
        reject_secret_lines(&merged)?;
        Ok(Self {
            read,
            baseline,
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
        if manifest.shared_source_base.as_ref() == Some(&self.baseline)
            && manifest.master.shared == self.merged
            && manifest.credentials.rules.get(path) == credential.as_ref()
        {
            return false;
        }
        manifest.shared_source_base = Some(self.baseline);
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
            Err(error) => match error.downcast_ref::<SharedSourceBlocked>() {
                Some(blocked) => {
                    let code = match blocked {
                        SharedSourceBlocked::Conflict => Code::RuleConflict,
                        SharedSourceBlocked::MissingOrEmpty => Code::RuleUnavailable,
                    };
                    result.block(code, blocked.to_string());
                    Ok(false)
                }
                None => Err(error),
            },
        }
    }
}
