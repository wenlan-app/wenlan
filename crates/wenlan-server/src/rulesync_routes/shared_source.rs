// SPDX-License-Identifier: Apache-2.0
//! Follow the person's shared rules without writing back to their source.

use super::*;
use anyhow::{bail, ensure, Context, Result};
use serde::Serialize;
use sha2::Digest;
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use wenlan_core::rulesync_adopt::{read_shared_rule_source, SHARED_RULES};
use wenlan_core::rulesync_personal::credentials::credential_line_numbers;
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
        if !manifest.shared_source_following {
            let saved = manifest.master.shared.clone();
            return Ok(Self {
                read: saved.clone(),
                baseline: saved.clone(),
                merged: saved,
                credential_lines: Vec::new(),
            });
        }
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

    pub(super) fn record(self, manifest: &mut Manifest) -> Result<bool> {
        if !manifest.shared_source_following {
            return Ok(false);
        }
        let path = Path::new(SHARED_RULES);
        let credential = (!self.credential_lines.is_empty()).then_some(RuleCredentialLocation {
            tool: None,
            lines: self.credential_lines,
        });
        if manifest.shared_source_base.as_ref() == Some(&self.baseline)
            && manifest.master.shared == self.merged
            && manifest.credentials.rules.get(path) == credential.as_ref()
        {
            return Ok(false);
        }
        let mut authored = manifest.authored_rules.clone();
        for (path, origin) in &mut authored {
            if let Some(Delivery {
                kind: Kind::Rule { target, .. },
                ..
            }) = manifest.delivered.get(path)
            {
                let overlay = manifest
                    .master
                    .overlays
                    .get(target)
                    .map_or("", String::as_str);
                origin.check_shared_removal_against_overlay(
                    &manifest.master.shared,
                    &self.merged,
                    overlay,
                )?;
            }
            origin.remove_deleted_shared(&manifest.master.shared, &self.merged)?;
        }
        manifest.authored_rules = authored;
        manifest.shared_source_base = Some(self.baseline);
        manifest.master.shared = self.merged;
        if let Some(credential) = credential {
            manifest.credentials.rules.insert(path.into(), credential);
        } else {
            manifest.credentials.rules.remove(path);
        }
        Ok(true)
    }

    pub(super) fn record_with_history(
        self,
        setup: &Setup,
        manifest: &mut Manifest,
    ) -> Result<bool> {
        let before = manifest.clone();
        if !self.record(manifest)? {
            return Ok(false);
        }
        personal_rules::checkpoint_master_change(setup, manifest, before, false)?;
        Ok(true)
    }
}

pub(super) fn deliveries_match_master(manifest: &Manifest) -> bool {
    manifest.active_deliveries().all(|(_, delivery)| {
        let Kind::Rule {
            target,
            base,
            authored_ends,
            ..
        } = &delivery.kind
        else {
            return true;
        };
        let Some([shared_end, _]) = authored_ends else {
            return manifest.master.shared_rule_text(target).is_empty();
        };
        base.get(..*shared_end) == Some(manifest.master.shared_rule_text(target).as_str())
    })
}

impl Setup {
    pub(super) fn follow_shared_source(&self, manifest: &mut Manifest) -> Result<()> {
        if !manifest.shared_source_following {
            return Ok(());
        }
        SharedSource::read(&self.home, manifest)?.record_with_history(self, manifest)?;
        Ok(())
    }

    pub(super) fn follow_source_for_sync(
        &self,
        manifest: &mut Manifest,
        result: &mut RulesSyncReport,
    ) -> Result<bool> {
        if !manifest.shared_source_following {
            return Ok(true);
        }
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

    pub(super) fn preview_source_off(&self) -> Result<SharedSourceOffPreview> {
        let _lock = self.lock()?;
        let manifest = self.load()?.context("run wenlan rules adopt first")?;
        ensure!(
            manifest.shared_source_following,
            "The shared source is already off."
        );
        ensure_no_unsettled_journal(&manifest)?;
        ensure!(
            manifest.rule_origins_tracked,
            "The backup does not record rule donor provenance. Resolve provenance before turning source following off."
        );
        ensure!(
            deliveries_match_master(&manifest),
            "The saved shared rules are not delivered to every managed tool. Run `{SYNC_COMMAND}` and resolve any delivery problems before turning source following off."
        );
        verify_live_rule_deliveries(&self.home, &manifest)?;
        let source = source_off_observation(&self.home, &manifest)?;
        let archive = archive_payload(&manifest, &source)?;
        let binding = fingerprint(&(&manifest.master, manifest.shared_source_following, &archive))?;
        let token = remember_preview(&self.data, &binding);
        let diff = if source.read.is_empty() {
            format!(
                "The shared source is missing or empty. Saved master:\n{}",
                manifest.master.shared
            )
        } else if source.merged == manifest.master.shared {
            "The source matches the saved shared rules. No pending changes.".to_owned()
        } else {
            let (_, _, diff) =
                describe_change(SHARED_RULES, &manifest.master.shared, &source.merged);
            wenlan_core::skill_registry::printable(&diff).replace('\t', "<U+0009>")
        };
        Ok(SharedSourceOffPreview {
            plan_id: format!("source-off-{}", &token[..16]),
            token,
            source_missing: source.read.is_empty(),
            diff,
        })
    }

    pub(super) fn turn_source_off(&self, request: &SharedSourceOffRequest) -> Result<()> {
        self.turn_source_off_inner(request, || {})
    }

    #[cfg(test)]
    pub(super) fn turn_source_off_after_archive_for_test(
        &self,
        request: &SharedSourceOffRequest,
        before_final_source_check: impl FnOnce(),
    ) -> Result<()> {
        self.turn_source_off_inner(request, before_final_source_check)
    }

    fn turn_source_off_inner(
        &self,
        request: &SharedSourceOffRequest,
        before_final_source_check: impl FnOnce(),
    ) -> Result<()> {
        let _lock = self.lock()?;
        let mut manifest = self.load()?.context("run wenlan rules adopt first")?;
        ensure!(
            manifest.shared_source_following,
            "The shared source is already off."
        );
        ensure_no_unsettled_journal(&manifest)?;
        ensure!(
            request
                .token
                .get(..16)
                .is_some_and(|prefix| { request.plan_id == format!("source-off-{prefix}") }),
            "Run the source-off preview again and confirm that exact plan."
        );
        ensure!(
            manifest.rule_origins_tracked,
            "The backup does not record rule donor provenance. Resolve provenance before turning source following off."
        );
        let source = source_off_observation(&self.home, &manifest)?;
        ensure!(
            source.merged == manifest.master.shared,
            "The shared source has changes not yet synced into the Wenlan master. Run `{SYNC_COMMAND}`, resolve any conflicts, then preview source-off again."
        );
        ensure!(
            deliveries_match_master(&manifest),
            "The saved shared rules are not delivered to every managed tool. Run `{SYNC_COMMAND}` and resolve any delivery problems before turning source following off."
        );
        verify_live_rule_deliveries(&self.home, &manifest)?;
        let archive = archive_payload(&manifest, &source)?;
        let binding = fingerprint(&(&manifest.master, manifest.shared_source_following, &archive))?;
        consume_preview(&self.data, &request.token, &binding)?;
        let baseline = personal_rules::stage_baseline(self, &mut manifest)?;
        let archive_path = match write_archive(self, &archive) {
            Ok(path) => path,
            Err(error) => {
                personal_rules::discard_history_journal(self, &mut manifest)?;
                return Err(error);
            }
        };
        if let Err(error) = verify_live_rule_deliveries(&self.home, &manifest) {
            personal_rules::discard_history_journal(self, &mut manifest)?;
            return Err(error);
        }
        before_final_source_check();
        let latest_source = match source_off_observation(&self.home, &manifest) {
            Ok(source) => source,
            Err(error) => {
                personal_rules::discard_history_journal(self, &mut manifest)?;
                return Err(error).context(
                    "the shared source could not be rechecked before source-off; preview again after resolving it",
                );
            }
        };
        if !same_source_observation(&source, &latest_source) {
            personal_rules::discard_history_journal(self, &mut manifest)?;
            bail!(
                "The shared source changed while source-off was being prepared. The source was left on; preview again."
            );
        }
        let source_path = Path::new(SHARED_RULES);
        manifest.backup.remove(source_path);
        manifest.delivered.remove(source_path);
        manifest.credentials.rules.remove(source_path);
        manifest.shared_source_following = false;
        manifest.source_off_archive = Some(archive_path);
        if let Some(baseline) = baseline {
            manifest.personal_rule_history.push(baseline);
            manifest.personal_rule_journal = None;
        }
        self.save(&manifest)?;
        Ok(())
    }
}

fn source_off_observation(home: &Path, manifest: &Manifest) -> Result<SharedSource> {
    match SharedSource::read(home, manifest) {
        Ok(source) => Ok(source),
        Err(error)
            if error
                .downcast_ref::<SharedSourceBlocked>()
                .is_some_and(|blocked| matches!(blocked, SharedSourceBlocked::MissingOrEmpty)) =>
        {
            Ok(SharedSource {
                read: String::new(),
                baseline: manifest.shared_source_base.clone().unwrap_or_default(),
                merged: manifest.master.shared.clone(),
                credential_lines: Vec::new(),
            })
        }
        Err(error) => Err(error),
    }
}

fn same_source_observation(left: &SharedSource, right: &SharedSource) -> bool {
    left.read == right.read
        && left.baseline == right.baseline
        && left.merged == right.merged
        && left.credential_lines == right.credential_lines
}

const SOURCE_OFF_CONFIRM_TTL: Duration = Duration::from_secs(5 * 60);

#[derive(Serialize, PartialEq)]
struct SharedSourceArchive {
    format: u32,
    source_path: String,
    was_following: bool,
    safe_source: String,
    merged_shared: String,
    credential_lines: Vec<usize>,
    shared_source_base: Option<String>,
    backup: Option<Snapshot>,
    receipt_delivered: Option<Snapshot>,
    receipt_observed: Option<Snapshot>,
    rule_origins_tracked: bool,
    donor_provenance: BTreeMap<PathBuf, restore_rules::AuthoredRules>,
}

fn archive_payload(manifest: &Manifest, source: &SharedSource) -> Result<SharedSourceArchive> {
    let path = Path::new(SHARED_RULES);
    let receipt = manifest.delivered.get(path);
    Ok(SharedSourceArchive {
        format: 1,
        source_path: SHARED_RULES.to_owned(),
        was_following: manifest.shared_source_following,
        safe_source: source.baseline.clone(),
        merged_shared: source.merged.clone(),
        credential_lines: source.credential_lines.clone(),
        shared_source_base: manifest.shared_source_base.clone(),
        backup: manifest.backup.get(path).map(safe_snapshot).transpose()?,
        receipt_delivered: receipt
            .map(|receipt| safe_snapshot(&receipt.delivered))
            .transpose()?,
        receipt_observed: receipt
            .and_then(|receipt| receipt.observed.as_ref())
            .map(safe_snapshot)
            .transpose()?,
        rule_origins_tracked: manifest.rule_origins_tracked,
        donor_provenance: manifest.authored_rules.clone(),
    })
}

fn safe_snapshot(snapshot: &Snapshot) -> Result<Snapshot> {
    let Snapshot::File { bytes, mode } = snapshot else {
        return Ok(snapshot.clone());
    };
    let text = std::str::from_utf8(bytes).context("source backup is not UTF-8")?;
    let secret_lines: BTreeSet<_> = credential_line_numbers(text).into_iter().collect();
    let safe = text
        .split_inclusive('\n')
        .enumerate()
        .filter_map(|(index, line)| (!secret_lines.contains(&(index + 1))).then_some(line))
        .collect::<String>();
    Ok(Snapshot::file_with_permissions(safe.into_bytes(), *mode))
}

fn ensure_no_unsettled_journal(manifest: &Manifest) -> Result<()> {
    ensure!(
        manifest.pending.is_empty()
            && manifest.stages.is_empty()
            && manifest.rule_share.is_none()
            && manifest.personal_rule_journal.is_none()
            && manifest.extension_paths.is_empty()
            && manifest.settings_batch.is_none()
            && manifest.recovery_issues.is_empty(),
        "A setup write or recovery is pending. Run `{SYNC_COMMAND}` to settle it before turning source following off."
    );
    Ok(())
}

fn fingerprint<T: Serialize>(value: &T) -> Result<String> {
    Ok(hex::encode(sha2::Sha256::digest(serde_json::to_vec(
        value,
    )?)))
}

type SourceOffPreviews = HashMap<String, (PathBuf, String, Instant)>;

fn preview_cache() -> &'static Mutex<SourceOffPreviews> {
    static CACHE: OnceLock<Mutex<SourceOffPreviews>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn remember_preview(data: &Path, binding: &str) -> String {
    let mut hasher = sha2::Sha256::new();
    hasher.update(binding.as_bytes());
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    hasher.update(nonce.to_le_bytes());
    let token = hex::encode(hasher.finalize());
    let mut cache = preview_cache()
        .lock()
        .expect("source-off preview cache poisoned");
    cache.retain(|_, (_, _, at)| at.elapsed() < SOURCE_OFF_CONFIRM_TTL);
    cache.insert(
        token.clone(),
        (data.to_owned(), binding.to_owned(), Instant::now()),
    );
    token
}

fn consume_preview(data: &Path, token: &str, binding: &str) -> Result<()> {
    let mut cache = preview_cache()
        .lock()
        .expect("source-off preview cache poisoned");
    let valid = cache.remove(token).is_some_and(|(path, saved, at)| {
        path == data && saved == binding && at.elapsed() < SOURCE_OFF_CONFIRM_TTL
    });
    ensure!(
        valid,
        "That source-off preview expired or the setup changed. Preview again and confirm."
    );
    Ok(())
}

fn verify_live_rule_deliveries(home: &Path, manifest: &Manifest) -> Result<()> {
    let root = FileRoot::open_home(home)
        .context("cannot inspect managed rule deliveries")?
        .guard_source_aliases();
    for (path, delivery) in manifest.active_deliveries() {
        if path == Path::new(SHARED_RULES) || !matches!(&delivery.kind, Kind::Rule { .. }) {
            continue;
        }
        let current = if crate::rulesync_apply::is_personal_entry(path) {
            root.read_personal_file(path, true)
        } else {
            root.read(path, true)
        }
        .with_context(|| format!("cannot verify managed rule delivery {}", path.display()))?;
        ensure!(
            current == delivery.delivered,
            "Managed rule delivery {} is missing or changed. Run `{SYNC_COMMAND}` and resolve the delivery before turning source following off.",
            path.display()
        );
    }
    Ok(())
}

fn write_archive(setup: &Setup, archive: &SharedSourceArchive) -> Result<PathBuf> {
    let root = setup.data_files()?.with_private_directories();
    let directory = Path::new("rule-history");
    match root.read_private(directory, None)? {
        Snapshot::Absent => ensure!(
            root.apply(
                directory,
                &Snapshot::Absent,
                &Snapshot::private_directory(),
                false
            )? == ApplyOutcome::Applied,
            "the source archive directory changed"
        ),
        Snapshot::Directory { .. } => (),
        _ => bail!("the private source archive directory is not a directory"),
    }
    let path = PathBuf::from(format!(
        "rule-history/shared-source-off-{}.json",
        uuid::Uuid::new_v4().simple()
    ));
    let bytes = serde_json::to_vec(archive)?;
    let seen = root.read_private(&path, None)?;
    ensure!(
        seen == Snapshot::Absent,
        "the unique source archive name already exists; retry source-off"
    );
    ensure!(
        root.apply(&path, &seen, &Snapshot::file(bytes), false)? == ApplyOutcome::Applied,
        "the unique source archive changed while saving"
    );
    root.sync_directory(directory)?;
    root.sync_directory(Path::new(""))?;
    Ok(path)
}
