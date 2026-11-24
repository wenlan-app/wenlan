// SPDX-License-Identifier: Apache-2.0
//! Plan and apply rule moves from the reviewed, live tool documents.

use super::*;
use wenlan_core::rulesync_adopt::{legacy_claude_imported_rules, SHARED_RULES};
use wenlan_core::rulesync_personal::join_text;
use wenlan_core::rulesync_personal::sharing::{overlay_lines_with, LocalReason};
use wenlan_types::rulesync::{SharingRule, SharingRuleStay};

#[derive(Default)]
pub(super) struct RuleShares {
    pub lines: Vec<SharingRule>,
    pub stays: Vec<SharingRuleStay>,
    targets: Vec<&'static PersonalTarget>,
    versions: Vec<(&'static str, Option<String>)>,
}

impl RuleShares {
    pub fn moving(&self) -> impl Iterator<Item = &str> {
        self.lines
            .iter()
            .filter(|rule| !rule.kept)
            .map(|rule| rule.line.as_str())
    }

    pub(super) fn approval(&self) -> impl Serialize + '_ {
        (&self.lines, &self.stays, &self.versions)
    }
}

struct RuleDocument {
    target: &'static PersonalTarget,
    personal: String,
    overlay: String,
    imported: BTreeSet<String>,
    /// For legacy Claude setups, lines proven to be native authored text.
    native: Option<BTreeSet<String>>,
}

fn classified_local<'a>(
    documents: &'a [RuleDocument],
    kept: impl Fn(&str) -> bool,
    mut local: BTreeMap<&'a str, LocalReason>,
) -> BTreeMap<&'a str, LocalReason> {
    // A local occurrence keeps the same text local in every document. A
    // repeated heading can therefore keep another section local as well.
    loop {
        let mut changed = false;
        for doc in documents {
            let classified = overlay_lines_with(&doc.overlay, |line| {
                if doc.imported.contains(line) {
                    Some(LocalReason::Imported)
                } else if doc
                    .native
                    .as_ref()
                    .is_some_and(|native| !native.contains(line))
                {
                    Some(LocalReason::UnverifiedOrigin)
                } else if kept(line) {
                    Some(LocalReason::KeptByPerson)
                } else {
                    local.get(line).copied()
                }
            });
            for (line, reason) in classified {
                let Some(reason) = reason else {
                    continue;
                };
                let entry = local.entry(line).or_insert_with(|| {
                    changed = true;
                    reason
                });
                if reason < *entry {
                    *entry = reason;
                    changed = true;
                }
            }
        }
        if !changed {
            return local;
        }
    }
}

type RuleAdditions = Vec<(&'static str, usize)>;

/// One stable number per portable line, independent of the keep selection.
/// A line cannot travel if any copy has to stay local, including within a
/// private key or a Markdown block.
pub(super) fn plan(
    home: &Path,
    root: &FileRoot,
    manifest: &Manifest,
    enrolled: &[&'static PersonalTarget],
    keep: &[usize],
) -> Result<(RuleShares, RuleAdditions)> {
    let mut plan = RuleShares::default();
    let mut documents = Vec::new();
    for &target in enrolled {
        let path = Path::new(target.rules.destination);
        let Some(receipt) = manifest.delivered.get(path) else {
            if let Some(credential) = manifest.credentials.rules.get(path) {
                let lines =
                    read_credential_lines(root, path).unwrap_or_else(|| credential.lines.clone());
                plan.stays.extend(
                    lines
                        .iter()
                        .map(|_| stay(target.tool.display_name(), "", LocalReason::Credential)),
                );
            }
            continue;
        };
        let Kind::Rule {
            target: id, base, ..
        } = &receipt.kind
        else {
            continue;
        };
        if id != target.id {
            continue;
        }
        let current = root.read(path, false);
        if let Ok(snapshot) = &current {
            let credential_lines = snapshot_credential_lines(snapshot);
            if !credential_lines.is_empty() {
                plan.stays.extend(
                    credential_lines
                        .iter()
                        .map(|_| stay(target.tool.display_name(), "", LocalReason::Credential)),
                );
                continue;
            }
        }
        let expected = receipt.observed.as_ref().unwrap_or(&receipt.delivered);
        verify_rule_snapshot(path, current, expected)?;
        let personal = rulesync_layers::only_personal(text(expected)?)?;
        let overlay = manifest.master.authored_overlay(target.id, base, &personal);
        let (imported, native) = if target.id != "claudecode" {
            (BTreeSet::new(), None)
        } else if let Some(imported) = &manifest.master.claude_imported_rules {
            (imported.clone(), None)
        } else {
            let original = manifest
                .backup
                .get(path)
                .and_then(|backup| text(backup).ok())
                .unwrap_or("");
            let imported =
                legacy_claude_imported_rules(home, path, original, &manifest.master.shared);
            let mut native: BTreeSet<_> = original.lines().map(str::to_owned).collect();
            // The current tool file can contain bytes Wenlan previously
            // flattened into it. Only later authored additions are new
            // evidence when the original native file lacks this line.
            let saved: BTreeSet<_> = manifest.master.overlays[target.id].lines().collect();
            native.extend(
                overlay
                    .lines()
                    .filter(|line| !saved.contains(line))
                    .map(str::to_owned),
            );
            (imported, Some(native))
        };
        documents.push(RuleDocument {
            target,
            personal,
            overlay,
            imported,
            native,
        });
        plan.targets.push(target);
        plan.versions
            .push((target.id, receipt.rule_revision.clone()));
    }
    let shared_path = Path::new(SHARED_RULES);
    if read_credential_lines(root, shared_path).map_or_else(
        || manifest.credentials.rules.contains_key(shared_path),
        |lines| !lines.is_empty(),
    ) {
        plan.stays.push(SharingRuleStay {
            tool: SHARED_RULES.into(),
            line: None,
            reason: CREDENTIAL_FILE_REASON.into(),
        });
        return Ok((plan, Vec::new()));
    }
    let previously_kept = |line: &str| manifest.kept_share_rules.contains(line);
    let local = classified_local(&documents, previously_kept, BTreeMap::new());
    let shared: BTreeSet<_> = manifest.master.shared.lines().collect();
    let mut numbers = BTreeMap::new();
    for doc in &documents {
        let name = doc.target.tool.display_name();
        for line in doc.overlay.lines().filter(|line| !line.trim().is_empty()) {
            if let Some(reason) = local.get(line) {
                plan.stays.push(stay(name, line, *reason));
                continue;
            }
            if plan.targets.len() < 2 || shared.contains(line) {
                continue;
            }
            let index = *numbers.entry(line).or_insert_with(|| {
                let index = plan.lines.len();
                plan.lines.push(SharingRule {
                    number: index + 1,
                    line: line.into(),
                    tools: Vec::new(),
                    kept: false,
                });
                index
            });
            if plan.lines[index]
                .tools
                .last()
                .is_none_or(|tool| tool != name)
            {
                plan.lines[index].tools.push(name.into());
            }
        }
    }
    for &number in keep {
        ensure!(
            (1..=plan.lines.len()).contains(&number),
            "Rule line {number} is not in the current preview. Run `wenlan rules share --dry-run --details`."
        );
        plan.lines[number - 1].kept = true;
    }
    if !keep.is_empty() {
        let selected: BTreeSet<_> = plan
            .lines
            .iter()
            .filter(|rule| rule.kept)
            .map(|rule| rule.line.clone())
            .collect();
        let with_keeps = classified_local(
            &documents,
            |line| previously_kept(line) || selected.contains(line),
            local.clone(),
        );
        for doc in &documents {
            for line in doc.overlay.lines() {
                if !local.contains_key(line) && !selected.contains(line) {
                    if let Some(reason) = with_keeps.get(line) {
                        plan.stays
                            .push(stay(doc.target.tool.display_name(), line, *reason));
                    }
                }
            }
        }
        plan.lines
            .retain(|rule| rule.kept || !with_keeps.contains_key(rule.line.as_str()));
    }
    let additions = documents
        .iter()
        .filter_map(|doc| {
            let live: BTreeSet<_> = doc.personal.lines().collect();
            let count = plan.moving().filter(|line| !live.contains(line)).count();
            (count != 0).then_some((doc.target.id, count))
        })
        .collect();
    Ok((plan, additions))
}

fn stay(tool: &str, line: &str, reason: LocalReason) -> SharingRuleStay {
    SharingRuleStay {
        tool: tool.into(),
        line: (reason != LocalReason::Credential).then(|| line.to_owned()),
        reason: reason.reason().into(),
    }
}

fn rule_snapshot<'a>(root: &FileRoot, path: &Path, receipt: &'a Delivery) -> Result<&'a Snapshot> {
    let expected = receipt.observed.as_ref().unwrap_or(&receipt.delivered);
    verify_rule_snapshot(path, root.read(path, false), expected)?;
    Ok(expected)
}

fn verify_rule_snapshot(
    path: &Path,
    current: std::io::Result<Snapshot>,
    expected: &Snapshot,
) -> Result<()> {
    ensure!(
        current.is_ok_and(|actual| actual == *expected),
        "{} changed. Run `wenlan rules sync` and review the rule file before sharing.",
        path.display()
    );
    Ok(())
}

pub(super) struct RuleShareChange {
    update: RuleShareUpdate,
    operations: Vec<Pending>,
}

pub(super) struct RuleShareUpdate {
    shared: String,
    overlays: BTreeMap<String, String>,
    overrides: BTreeMap<String, LocalText>,
    kept: BTreeSet<String>,
    authored: BTreeMap<PathBuf, restore_rules::AuthoredRules>,
    moved: bool,
}

impl RuleShareUpdate {
    pub(super) fn commit(self, manifest: &mut Manifest) {
        manifest.master.shared = self.shared;
        manifest.master.overlays = self.overlays;
        manifest.master.overrides = self.overrides;
        manifest.kept_share_rules.extend(self.kept);
        manifest.authored_rules.extend(self.authored);
        manifest.rule_origins_tracked |= self.moved;
    }
}

impl Setup {
    pub(super) fn stage_rule_shares(
        &self,
        root: &FileRoot,
        manifest: &Manifest,
        plan: &RuleShares,
        generated: &[String],
        render: &impl Fn(&str, &str) -> Result<SkillFiles>,
    ) -> Result<Option<RuleShareChange>> {
        if plan.lines.is_empty() {
            return Ok(None);
        }
        let kept = plan
            .lines
            .iter()
            .filter(|rule| rule.kept)
            .map(|rule| rule.line.clone())
            .collect();
        let moving: BTreeSet<_> = plan.moving().collect();
        let mut master = PersonalMaster {
            shared: manifest.master.shared.clone(),
            overlays: manifest.master.overlays.clone(),
            claude_imported_rules: manifest.master.claude_imported_rules.clone(),
            codex_sync_replaced: manifest.master.codex_sync_replaced,
            skills: Vec::new(),
            overrides: manifest.master.overrides.clone(),
        };
        let mut operations = Vec::new();
        let mut authored = BTreeMap::new();
        if !moving.is_empty() {
            ensure!(
                manifest.rule_origins_tracked || !is_shared_source_receipt(manifest, Path::new(SHARED_RULES)),
                "The backup does not say which tool added the previously shared rules. Review the tool and shared files before sharing more rules."
            );
            let mut append = String::new();
            for line in plan.moving() {
                append.push_str(line);
                append.push('\n');
            }
            master.shared = join_text(&master.shared, &append);
            let mut worktrees = DeliveryWorktrees::new(&self.home)?;
            for target in &plan.targets {
                let overlay = master
                    .overlays
                    .get_mut(target.id)
                    .expect("enrolled rule overlay");
                *overlay = without_moves(overlay, &moving);
                let path = PathBuf::from(target.rules.destination);
                let receipt = &manifest.delivered[&path];
                let seen = rule_snapshot(root, &path, receipt)?;
                if let Some(origin) =
                    restore_rules::AuthoredRules::for_promotion(manifest, &path, seen, &moving)?
                {
                    authored.insert(path.clone(), origin);
                }
                let base = master.base_rule_text(target.id, generated);
                let source = rebase_tool_rules(
                    &manifest.master,
                    &master,
                    target.id,
                    generated,
                    &base,
                    &moving,
                )
                .with_context(|| {
                    format!(
                        "{} has a local rule edit that could not be shared safely.",
                        path.display()
                    )
                })?;
                if source == base {
                    master.overrides.remove(target.id);
                } else {
                    master.overrides.insert(
                        target.id.into(),
                        LocalText {
                            base: base.clone(),
                            edited: source.clone(),
                        },
                    );
                }
                let mut outputs = render(target.id, &source).map_err(|_| {
                    anyhow::anyhow!("Could not render rules for {}.", target.tool.display_name())
                })?;
                let bytes = outputs
                    .remove(&path)
                    .with_context(|| format!("Could not render {}.", path.display()))?;
                verify_projection(&path, text(seen)?, std::str::from_utf8(&bytes)?, &moving)?;
                let desired = with_bytes(seen, bytes);
                if desired != *seen {
                    if let Some(reason) = worktrees.unreviewed(&path, &manifest.worktree_writes)? {
                        bail!("{}: {reason}", path.display());
                    }
                }
                operations.push(Pending {
                    path,
                    seen: seen.clone(),
                    desired,
                    kind: Some(Kind::Rule {
                        target: target.id.into(),
                        source,
                        base,
                    }),
                    allow_link: false,
                });
            }
            let source_root = root.with_shared_rule_source()?;
            let path = PathBuf::from(SHARED_RULES);
            let directory = path.parent().expect("shared rules directory");
            let seen = source_root
                .read(&path, false)
                .context("Read ~/.agents/AGENTS.md before sharing rules")?;
            ensure!(
                !snapshot_has_credentials(&seen),
                "{}",
                credential_note(&path)
            );
            ensure!(
                text(&seen)? == manifest.master.shared,
                "~/.agents/AGENTS.md differs from the shared rules saved in Wenlan. Resolve the difference before sharing rules."
            );
            if source_root.read(directory, false)? == Snapshot::Absent {
                operations.push(Pending {
                    path: directory.to_owned(),
                    seen: Snapshot::Absent,
                    desired: Snapshot::private_directory(),
                    kind: Some(Kind::SharedRuleSource),
                    allow_link: false,
                });
            }
            operations.push(Pending {
                path,
                desired: with_bytes(&seen, master.shared.as_bytes().to_vec()),
                seen,
                kind: Some(Kind::SharedRuleSource),
                allow_link: false,
            });
        }
        Ok(Some(RuleShareChange {
            update: RuleShareUpdate {
                shared: master.shared,
                overlays: master.overlays,
                overrides: master.overrides,
                kept,
                authored,
                moved: !moving.is_empty(),
            },
            operations,
        }))
    }

    pub(super) fn apply_rule_shares(
        &self,
        root: &FileRoot,
        manifest: &mut Manifest,
        change: RuleShareChange,
        result: &mut RulesSyncReport,
    ) -> Result<()> {
        let changed: BTreeSet<_> = change
            .operations
            .iter()
            .filter(|op| op.seen != op.desired)
            .map(|op| op.path.clone())
            .collect();
        if change.operations.is_empty() {
            change.update.commit(manifest);
            return self.save(manifest);
        }
        for (path, outcome) in self.apply_batch(
            root,
            manifest,
            change.operations,
            BatchMode::ShareRules(change.update),
        )? {
            ensure!(
                outcome == ApplyOutcome::Applied,
                "{} changed while sharing rules. Run `wenlan rules sync`.",
                path.display()
            );
            if changed.contains(&path) {
                result.written.push(path.display().to_string());
            }
        }
        Ok(())
    }
}

fn without_moves(text: &str, moving: &BTreeSet<&str>) -> String {
    text.split_inclusive('\n')
        .filter(|raw| !moving.contains(raw.lines().next().unwrap_or(raw)))
        .collect()
}

fn rebase_tool_rules(
    old: &PersonalMaster,
    new: &PersonalMaster,
    target: &str,
    generated: &[String],
    base: &str,
    moving: &BTreeSet<&str>,
) -> Result<String> {
    if !old.overrides.contains_key(target) {
        return Ok(base.to_owned());
    }
    let local = without_moves(&old.rule_text(target, generated)?, moving);
    let previous = old.shared_rule_text(target);
    if let Some(body) = local.strip_prefix(&previous) {
        let body = if !previous.is_empty() && !previous.ends_with('\n') {
            body.strip_prefix('\n').unwrap_or(body)
        } else {
            body
        };
        Ok(join_text(&new.shared_rule_text(target), body))
    } else {
        let previous_base = without_moves(&old.base_rule_text(target, generated), moving);
        merge_rule_text(&previous_base, &local, base)
    }
}

fn verify_projection(
    path: &Path,
    before: &str,
    after: &str,
    moving: &BTreeSet<&str>,
) -> Result<()> {
    let surviving = |line: &&str| !line.trim().is_empty() && !moving.contains(line);
    let mut additions = BTreeMap::new();
    for line in after.lines().filter(|line| moving.contains(line)) {
        *additions.entry(line).or_insert(0) += 1;
    }
    ensure!(
        before
            .lines()
            .filter(surviving)
            .eq(after.lines().filter(surviving))
            && moving.iter().all(|line| additions.get(line) == Some(&1)),
        "Sharing would change other rules in {}. Run `wenlan rules sync` and review the rule file.",
        path.display()
    );
    Ok(())
}

#[cfg(test)]
type BeforeWrite = (PathBuf, Box<dyn FnOnce()>);

#[cfg(test)]
thread_local! {
    static BEFORE_WRITE: std::cell::RefCell<Option<BeforeWrite>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(super) fn before_rule_share_write(path: &Path) {
    let hook = BEFORE_WRITE.with(|hook| hook.borrow_mut().take_if(|(target, _)| target == path));
    if let Some((_, hook)) = hook {
        hook();
    }
}

#[cfg(test)]
pub(super) fn at_rule_share_write(path: &str, hook: impl FnOnce() + 'static) {
    BEFORE_WRITE.with(|slot| assert!(slot.replace(Some((path.into(), Box::new(hook)))).is_none()));
}
