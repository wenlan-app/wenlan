// SPDX-License-Identifier: Apache-2.0
//! Poll delivered rule files for the scope pullback step of Sync.

use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    time::SystemTime,
};

use anyhow::{bail, ensure, Context, Result};
use fs2::FileExt;

use super::{
    group_drafts, inspect_rule, needs_conflict_card, record_scope_review,
    storage::{save_manifest, BlobHash, StoredSetup, MANIFEST},
    Code, Kind, Setup,
};
use crate::rulesync_apply::{FileMode, FileRoot, Snapshot};
use wenlan_core::rulesync_personal::{enabled, is_harness_path};

#[derive(Default)]
pub struct RulesWatch {
    rules: BTreeMap<PathBuf, WatchedRule>,
    #[cfg(test)]
    reads: usize,
}

struct WatchedRule {
    stamp: (SystemTime, u64),
    content: Snapshot,
    hash: BlobHash,
    mode: FileMode,
}

impl RulesWatch {
    /// Keep filesystem work off the Tokio executor and retain the cache after
    /// recoverable failures. Report only fixed operation names, without causes
    /// that can contain absolute paths or rule text.
    pub async fn check(mut self, data: PathBuf) -> Self {
        match tokio::task::spawn_blocking(move || {
            if let Err(error) = self.round(enabled(), &data, Setup::tool_home) {
                tracing::warn!(reason = %error, "rules watch failed; will retry");
            }
            self
        })
        .await
        {
            Ok(watch) => watch,
            Err(_) => {
                tracing::warn!("rules watch worker failed; will retry");
                Self::default()
            }
        }
    }

    fn round(
        &mut self,
        enabled: bool,
        data: &Path,
        resolve_home: impl FnOnce() -> Result<PathBuf>,
    ) -> Result<()> {
        if !enabled {
            return Ok(());
        }
        match fs::symlink_metadata(data.join(MANIFEST)) {
            Ok(_) => (),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.rules.clear();
                return Ok(());
            }
            Err(error) => return Err(error).context("check for rules setup"),
        }

        let data_root = FileRoot::open(data)
            .context("open rules setup directory")?
            .with_private_directories();
        let lock = data_root
            .open_existing_private_lock()
            .context("open rules setup lock")?;
        match FileExt::try_lock_exclusive(&lock) {
            Ok(()) => (),
            Err(error) if error.raw_os_error() == fs2::lock_contended_error().raw_os_error() => {
                return Ok(());
            }
            Err(error) => return Err(error).context("acquire rules setup lock"),
        }

        let Some(stored) = StoredSetup::read(&data_root)
            .context("read rules setup manifest")?
            .filter(|stored| stored.state.adopted && !stored.state.settings_only)
        else {
            self.rules.clear();
            return Ok(());
        };
        // Leave interrupted operations to the existing Sync/review recovery path.
        if !stored.state.pending.is_empty() || !stored.state.stages.is_empty() {
            return Ok(());
        }

        let home = resolve_home().context("resolve tool home")?;
        let data_root = data_root
            .guard_data(&home, data)
            .context("validate rules setup directory")?;
        let root = FileRoot::open_home(&home).context("open tool home")?;
        self.rules
            .retain(|path, _| stored.state.delivered.contains_key(path));
        let mut changed = Vec::new();
        for (path, delivery) in stored.state.active_deliveries() {
            if !matches!(delivery.kind, Kind::Rule { .. })
                || is_harness_path(path)
                || stored.state.recovery_issues.contains_key(path)
            {
                self.rules.remove(path);
                continue;
            }
            let Some(rule) = self.read_rule(&root, path)? else {
                continue;
            };
            let receipt = delivery.observed.as_ref().unwrap_or(&delivery.delivered);
            if !matches!(receipt, Snapshot::File { bytes, mode }
                if *bytes == rule.hash && *mode == rule.mode)
            {
                changed.push(path);
            }
        }
        if changed.is_empty() {
            return Ok(());
        }

        // Resolve private backup blobs only when a tool file differs from
        // the stored receipt. Conflicts may remain outstanding across rounds.
        let mut manifest = stored
            .load_contents(&data_root)
            .context("load rules setup receipts")?;
        let mut blocked = BTreeMap::new();
        let mut drafts = Vec::new();
        for path in changed {
            let current = self.rules[path].content.clone();
            if let Some(draft) =
                inspect_rule(path, &manifest.delivered[path], current, &mut blocked)
                    .context("inspect delivered rule edit")?
            {
                drafts.push(draft);
            }
        }
        if blocked
            .values()
            .any(|(code, _)| matches!(code, Code::RuleCompareFailed | Code::RuleEncoding))
        {
            bail!("compare delivered rule text");
        }

        let mut changed = false;
        for group in group_drafts(&drafts) {
            if !needs_conflict_card(&drafts, &group) {
                record_scope_review(&mut manifest, &drafts[group[0]])
                    .context("record scope review")?;
                changed = true;
            }
        }
        if changed {
            save_manifest(&data_root, &manifest).context("save scope reviews")?;
        }
        Ok(())
    }

    fn read_rule(&mut self, root: &FileRoot, path: &Path) -> Result<Option<&WatchedRule>> {
        let stamp = || {
            root.file_stamp(path)
                .context("check delivered rule metadata")
        };
        let Some(current_stamp) = stamp()? else {
            self.rules.remove(path);
            return Ok(None);
        };
        if self
            .rules
            .get(path)
            .is_none_or(|rule| rule.stamp != current_stamp)
        {
            #[cfg(test)]
            {
                self.reads += 1;
            }
            let content = root.read(path, false).context("read delivered rule")?;
            ensure!(
                stamp()? == Some(current_stamp),
                "delivered rule changed during read"
            );
            let Snapshot::File { bytes, mode } = &content else {
                bail!("delivered rule was replaced during read");
            };
            self.rules.insert(
                path.to_owned(),
                WatchedRule {
                    stamp: current_stamp,
                    hash: BlobHash::of(bytes),
                    mode: *mode,
                    content,
                },
            );
        }
        Ok(Some(&self.rules[path]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rulesync_routes::tests::{Fixture, CLAUDE, CODEX};
    use crate::rulesync_routes::{
        discover_adoption, Manifest, PersonalMaster, PullbackAction, PullbackDecision,
        PullbackKind, MANIFEST_SAVES,
    };
    use std::{collections::BTreeMap, fs, path::PathBuf, time::SystemTime};

    fn round(watch: &mut RulesWatch, f: &Fixture) -> Result<()> {
        watch.round(true, &f.setup.data, || Ok(f.setup.home.clone()))
    }

    fn edit(f: &Fixture) {
        f.write(
            CODEX,
            &f.text(CODEX).replace("Codex only.", "Edited in Codex."),
        );
    }

    fn files(root: &Path) -> BTreeMap<PathBuf, (Vec<u8>, SystemTime)> {
        fn walk(root: &Path, dir: &Path, files: &mut BTreeMap<PathBuf, (Vec<u8>, SystemTime)>) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                let metadata = fs::symlink_metadata(&path).unwrap();
                if metadata.is_dir() {
                    walk(root, &path, files);
                } else {
                    assert!(metadata.is_file());
                    files.insert(
                        path.strip_prefix(root).unwrap().into(),
                        (fs::read(&path).unwrap(), metadata.modified().unwrap()),
                    );
                }
            }
        }
        let mut files = BTreeMap::new();
        walk(root, root, &mut files);
        files
    }

    #[test]
    fn ux_skill_24_rulesync_watch_records_scope_without_home_writes() {
        let f = Fixture::new();
        f.adopt_from_plan();
        edit(&f);
        let before = files(&f.setup.home);
        assert!(f.setup.status().unwrap().proposals.is_empty());

        round(&mut RulesWatch::default(), &f).unwrap();

        let [card] = f
            .setup
            .status()
            .unwrap()
            .proposals
            .try_into()
            .unwrap_or_else(|cards: Vec<_>| panic!("expected one scope card, got {cards:?}"));
        assert_eq!(card.kind, PullbackKind::Scope);
        assert_eq!(card.files, [CODEX]);
        assert!(card.diff.contains("+Edited in Codex."));
        assert_eq!(files(&f.setup.home), before);
    }

    #[test]
    fn rulesync_watch_unchanged_round_does_not_save_or_duplicate() {
        let f = Fixture::new();
        f.adopt_from_plan();
        edit(&f);
        let mut watch = RulesWatch::default();
        let saves = MANIFEST_SAVES.get();
        round(&mut watch, &f).unwrap();
        assert_eq!(
            MANIFEST_SAVES.get(),
            saves + 1,
            "the first round records the card"
        );
        let manifest = f.setup.data.join("personal.json");
        let before = (
            fs::read(&manifest).unwrap(),
            fs::metadata(&manifest).unwrap().modified().unwrap(),
        );

        round(&mut watch, &f).unwrap();
        // A cold cache must also recognize the edit from the persisted receipt.
        round(&mut RulesWatch::default(), &f).unwrap();

        assert_eq!(MANIFEST_SAVES.get(), saves + 1);
        assert_eq!(f.setup.status().unwrap().proposals.len(), 1);
        assert_eq!(
            (
                fs::read(&manifest).unwrap(),
                fs::metadata(&manifest).unwrap().modified().unwrap()
            ),
            before
        );
    }

    #[test]
    fn rulesync_watch_skips_disabled_and_unadopted_before_resolving_home() {
        let f = Fixture::new();
        let mut watch = RulesWatch::default();
        let before = files(&f.setup.home);
        watch
            .round(true, &f.setup.data, || panic!("absent setup resolved home"))
            .unwrap();
        {
            let _lock = f.setup.lock().unwrap();
            let master = PersonalMaster::new(&discover_adoption(&f.setup.home).unwrap());
            f.setup
                .save(&Manifest::new(master, BTreeMap::new()))
                .unwrap();
        }
        let before_data = files(&f.setup.data);
        watch
            .round(true, &f.setup.data, || {
                panic!("unadopted setup resolved home")
            })
            .unwrap();
        assert_eq!(files(&f.setup.data), before_data);
        assert_eq!(files(&f.setup.home), before);
        assert_eq!(watch.reads, 0);

        let f = Fixture::new();
        f.adopt_from_plan();
        edit(&f);
        let before_data = files(&f.setup.data);
        watch
            .round(false, &f.setup.data, || {
                panic!("disabled setup resolved home")
            })
            .unwrap();
        assert_eq!(watch.reads, 0);
        assert_eq!(files(&f.setup.data), before_data);
        round(&mut watch, &f).unwrap();
        assert_eq!(
            f.setup.status().unwrap().proposals.len(),
            1,
            "the same edit is noticed only when enabled and adopted"
        );
    }

    #[test]
    fn rulesync_watch_unchanged_mtime_and_size_skip_content_reads() {
        let f = Fixture::new();
        f.adopt_from_plan();
        let mut watch = RulesWatch::default();
        round(&mut watch, &f).unwrap();
        let reads = watch.reads;
        assert_eq!(reads, 2, "the cold round reads both delivered rules once");
        round(&mut watch, &f).unwrap();
        assert_eq!(watch.reads, reads);

        edit(&f);
        round(&mut watch, &f).unwrap();
        assert_eq!(watch.reads, reads + 1);
    }

    #[test]
    fn rulesync_watch_skips_held_setup_lock_without_waiting() {
        let f = Fixture::new();
        f.adopt_from_plan();
        edit(&f);
        let lock = f.setup.lock().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let mut watch = RulesWatch::default();
                watch
                    .round(true, &f.setup.data, || panic!("a busy round resolved home"))
                    .unwrap();
                sender.send(watch).unwrap();
            });
            let skipped = receiver.recv_timeout(std::time::Duration::from_secs(2));
            // Release even if a regression blocks the worker, so the test can fail.
            drop(lock);
            let mut watch = skipped.expect("the round waited for the setup lock");
            assert_eq!(watch.reads, 0);
            assert!(f.setup.status().unwrap().proposals.is_empty());
            round(&mut watch, &f).unwrap();
            assert_eq!(
                f.setup.status().unwrap().proposals.len(),
                1,
                "the unlocked round records the waiting edit"
            );
        });
    }

    #[test]
    fn rulesync_watch_failed_round_preserves_cards_and_retries() {
        let f = Fixture::new();
        f.adopt_from_plan();
        edit(&f);
        let mut watch = RulesWatch::default();
        round(&mut watch, &f).unwrap();
        let pending = f.setup.status().unwrap().proposals;
        let [existing] = pending.as_slice() else {
            panic!("the first round should record a card");
        };
        f.write(
            CLAUDE,
            &f.text(CLAUDE).replace("Claude only.", "Edited in Claude."),
        );
        // A non-regular replacement cannot be inspected as a delivered rule.
        let claude = f.setup.home.join(CLAUDE);
        let original = f.text(CLAUDE);
        fs::remove_file(&claude).unwrap();
        fs::create_dir(&claude).unwrap();
        let before_data = files(&f.setup.data);
        assert!(round(&mut watch, &f).is_err());
        assert_eq!(files(&f.setup.data), before_data);
        assert_eq!(f.setup.status().unwrap().proposals[0].id, existing.id);

        fs::remove_dir(&claude).unwrap();
        f.write(CLAUDE, &original);
        round(&mut watch, &f).unwrap();
        let proposals = f.setup.status().unwrap().proposals;
        assert_eq!(proposals.len(), 2);
        assert!(proposals.iter().any(|card| card.id == existing.id));
    }

    #[test]
    fn rulesync_watch_newer_edit_preserves_existing_card_and_requires_current_id() {
        let f = Fixture::new();
        f.adopt_from_plan();
        let mut watch = RulesWatch::default();
        edit(&f);
        round(&mut watch, &f).unwrap();
        let first = f.setup.status().unwrap().proposals.remove(0);
        let saved = f.setup.load().unwrap().unwrap();
        let first_record = serde_json::to_value(&saved.scope_reviews[&first.id]).unwrap();

        f.write(
            CODEX,
            &f.text(CODEX)
                .replace("Edited in Codex.", "Edited a second time."),
        );
        round(&mut watch, &f).unwrap();
        let manifest = f.setup.load().unwrap().unwrap();
        assert_eq!(manifest.scope_reviews.len(), 2);
        assert_eq!(
            serde_json::to_value(&manifest.scope_reviews[&first.id]).unwrap(),
            first_record
        );
        let [current] = f
            .setup
            .status()
            .unwrap()
            .proposals
            .try_into()
            .unwrap_or_else(|cards: Vec<_>| {
                panic!("expected one current scope card, got {cards:?}")
            });
        assert!(current.diff.contains("+Edited a second time."));
        let mut decision = PullbackDecision {
            id: first.id,
            action: PullbackAction::ToolOnly,
            file: None,
        };
        assert!(f.setup.preview_review(&decision).is_err());
        assert!(f
            .setup
            .review_using(&decision, |_, _| panic!("stale review applied"))
            .is_err());

        decision.id = current.id;
        f.setup
            .review_using(&decision, |root, manifest| f.sync_review(root, manifest))
            .unwrap();
        assert!(f.setup.load().unwrap().unwrap().scope_reviews.is_empty());
    }

    #[test]
    fn rulesync_watch_leaves_competing_edits_for_live_status() {
        let f = Fixture::new();
        f.adopt_from_plan();
        f.write(CODEX, &f.text(CODEX).replace("One rule.", "Codex wording."));
        f.write(
            CLAUDE,
            &f.text(CLAUDE).replace("One rule.", "Claude wording."),
        );
        let before = files(&f.setup.home);
        let proposals = f.setup.status().unwrap().proposals;
        assert_eq!(proposals.len(), 1);
        assert_eq!(proposals[0].reason_code, Some(Code::CompetingRules));
        let saves = MANIFEST_SAVES.get();

        round(&mut RulesWatch::default(), &f).unwrap();

        assert_eq!(f.setup.status().unwrap().proposals, proposals);
        assert_eq!(MANIFEST_SAVES.get(), saves);
        assert_eq!(files(&f.setup.home), before);
    }
}
