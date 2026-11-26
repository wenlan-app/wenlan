// SPDX-License-Identifier: Apache-2.0
//! Plan and apply rule moves from the reviewed, live tool documents.

use super::*;
use wenlan_core::rulesync_adopt::without_shared;
use wenlan_core::rulesync_personal::join_text;
use wenlan_core::rulesync_personal::sharing::{overlay_lines_with, LocalReason};
use wenlan_types::rulesync::{SharingRule, SharingRuleStay};

#[derive(Default)]
pub(super) struct RuleShares {
    pub lines: Vec<SharingRule>,
    pub stays: Vec<SharingRuleStay>,
    pub notes: Vec<String>,
    targets: Vec<(&'static PersonalTarget, bool)>,
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
        (&self.lines, &self.stays, &self.notes, &self.versions)
    }

    fn keep_credential_lines(&mut self, tool: &str, lines: &[usize]) {
        self.stays
            .extend(lines.iter().map(|&line_number| SharingRuleStay {
                tool: tool.into(),
                line: None,
                line_number: Some(line_number),
                reason: LocalReason::Credential.reason().into(),
            }));
    }
}

struct RuleDocument {
    target: &'static PersonalTarget,
    writable: bool,
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
    root: &FileRoot,
    manifest: &Manifest,
    enrolled: &[&'static PersonalTarget],
    keep: &[usize],
) -> Result<(RuleShares, RuleAdditions)> {
    let mut plan = RuleShares::default();
    let mut documents = Vec::new();
    for &target in enrolled {
        let path = Path::new(target.rules.destination);
        let receipt = manifest.delivered.get(path);
        let marker = manifest.credentials.rules.get(path);
        if manifest.imported_rule_paths.contains(path) {
            if let Some(marker) = marker {
                plan.keep_credential_lines(target.tool.display_name(), &marker.lines);
            }
            continue;
        }
        if receipt.is_none() && marker.is_none() {
            continue;
        }
        let current = root.read(path, false);
        let credential_lines = current.as_ref().map_or_else(
            |_| marker.map_or_else(Vec::new, |marker| marker.lines.clone()),
            snapshot_credential_lines,
        );
        let writable = credential_lines.is_empty();
        if !writable {
            let tool = target.tool.display_name();
            plan.keep_credential_lines(tool, &credential_lines);
            if receipt.is_some() {
                plan.notes.push(format!(
                    "{tool}'s other rules are waiting because its rule file may hold a credential. Move the credential out, then run `{SYNC_COMMAND}` to review the file."
                ));
            }
        }
        let Some(receipt) = receipt else {
            continue;
        };
        let Kind::Rule {
            target: id,
            base,
            authored_ends,
            ..
        } = &receipt.kind
        else {
            continue;
        };
        if id != target.id {
            continue;
        }
        let expected = receipt.observed.as_ref().unwrap_or(&receipt.delivered);
        if writable {
            verify_rule_snapshot(path, current, expected)?;
        }
        // For a protected tool only its safe, previously reviewed rules
        // can affect classification. Its live file cannot become a donor.
        let personal = rulesync_layers::only_personal(text(expected)?)?;
        let overlay = if let Some([shared, authored]) = authored_ends {
            // An interrupted sync may have recorded the source edit before
            // this tool could receive it. Strip the layers it actually got.
            let shared = base
                .get(..*shared)
                .context("invalid shared rule boundary")?;
            let generated = base
                .get(*authored..)
                .context("invalid authored rule boundary")?;
            without_shared(generated, &without_shared(shared, &personal))
        } else {
            manifest.master.authored_overlay(target.id, base, &personal)
        };
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
            (BTreeSet::new(), Some(native))
        };
        documents.push(RuleDocument {
            target,
            writable,
            personal,
            overlay,
            imported,
            native,
        });
        plan.targets.push((target, writable));
        plan.versions
            .push((target.id, receipt.rule_revision.clone()));
    }
    let previously_kept = |line: &str| manifest.kept_share_rules.contains(line);
    let local = classified_local(&documents, previously_kept, BTreeMap::new());
    let shared: BTreeSet<_> = manifest.master.shared.lines().collect();
    let mut numbers = BTreeMap::new();
    let donors = documents.iter().filter(|doc| doc.writable);
    for doc in donors.clone() {
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
        for doc in donors.clone() {
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
    let additions = donors
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
        line_number: None,
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
    history: Option<personal_rules::RulePlanUpdate>,
}

impl RuleShareUpdate {
    pub(super) fn commit(self, manifest: &mut Manifest) {
        if let Some(history) = self.history {
            history.commit(manifest);
        }
        manifest.master.shared = self.shared;
        manifest.master.overlays = self.overlays;
        manifest.master.overrides = self.overrides;
        manifest.kept_share_rules.extend(self.kept);
        manifest.authored_rules.extend(self.authored);
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
        let approved: Vec<_> = plan.moving().collect();
        let moving: BTreeSet<_> = approved.iter().copied().collect();
        let mut master = PersonalMaster {
            shared: manifest.master.shared.clone(),
            overlays: manifest.master.overlays.clone(),
            unenrolled_overlays: manifest.master.unenrolled_overlays.clone(),
            claude_imported_rules: manifest.master.claude_imported_rules.clone(),
            codex_sync_replaced: manifest.master.codex_sync_replaced,
            skills: Vec::new(),
            overrides: manifest.master.overrides.clone(),
        };
        let mut operations = Vec::new();
        let mut authored = BTreeMap::new();
        if !moving.is_empty() {
            master.shared = append_rule_lines(&master.shared, &approved);
            let mut worktrees = DeliveryWorktrees::new(&self.home)?;
            for &(target, writable) in &plan.targets {
                let overlay = master
                    .overlays
                    .get_mut(target.id)
                    .expect("enrolled rule overlay");
                *overlay = without_moves(overlay, &moving);
                let path = PathBuf::from(target.rules.destination);
                let receipt = &manifest.delivered[&path];
                let seen = if writable {
                    rule_snapshot(root, &path, receipt)?
                } else {
                    receipt.observed.as_ref().unwrap_or(&receipt.delivered)
                };
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
                // Bookkeep promotions against the last reviewed snapshot.
                // Once delivery resumes the shared line appears only once.
                if !writable {
                    continue;
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
                    kind: Some(Kind::rule(&master, target.id, generated, source)),
                    allow_link: false,
                });
            }
        }
        Ok(Some(RuleShareChange {
            update: RuleShareUpdate {
                shared: master.shared,
                overlays: master.overlays,
                overrides: master.overrides,
                kept,
                authored,
                history: None,
            },
            operations,
        }))
    }

    pub(super) fn apply_rule_shares(
        &self,
        root: &FileRoot,
        manifest: &mut Manifest,
        mut change: RuleShareChange,
        result: &mut RulesSyncReport,
        person_confirmed: bool,
    ) -> Result<()> {
        let mut next = manifest.master.clone();
        next.shared.clone_from(&change.update.shared);
        next.overlays.clone_from(&change.update.overlays);
        next.overrides.clone_from(&change.update.overrides);
        change.update.history =
            personal_rules::stage_master_history(self, manifest, next, person_confirmed, false)?;
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
        let expected = change.operations.len();
        let applied = self.apply_batch(
            root,
            manifest,
            change.operations,
            BatchMode::ShareRules(change.update),
        );
        if !applied.as_ref().is_ok_and(|outcomes| {
            outcomes.len() == expected
                && outcomes
                    .iter()
                    .all(|(_, outcome)| *outcome == ApplyOutcome::Applied)
        }) {
            self.recover_pending(root, manifest)?;
            return match applied {
                Ok(_) => Err(anyhow::anyhow!(
                    "A tool file changed while sharing rules. Run `wenlan rules sync`."
                )),
                Err(error) => Err(error),
            };
        }
        for (path, outcome) in applied? {
            ensure!(
                outcome == ApplyOutcome::Applied,
                "{} changed while sharing rules. Run `wenlan rules sync`.",
                path.display()
            );
            if changed.contains(&path) {
                let path = path.display().to_string();
                if !result.written.contains(&path) {
                    result.written.push(path);
                }
            }
        }
        Ok(())
    }
}

fn append_rule_lines(original: &str, approved: &[&str]) -> String {
    let ending = if original
        .rfind('\n')
        .is_some_and(|index| original[..index].ends_with('\r'))
    {
        "\r\n"
    } else {
        "\n"
    };
    let ends_in_newline = original.is_empty() || original.ends_with('\n');
    let separator = if ends_in_newline { "" } else { ending };
    let trailing = if ends_in_newline { ending } else { "" };
    format!("{original}{separator}{}{trailing}", approved.join(ending))
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
    static FAIL_FINAL_CHECKPOINT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static FAIL_AFTER_FINAL_CHECKPOINT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
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

#[cfg(test)]
pub(super) fn fail_next_rule_final_checkpoint() {
    FAIL_FINAL_CHECKPOINT.with(|flag| assert!(!flag.replace(true)));
}

#[cfg(test)]
pub(super) fn before_rule_final_checkpoint() -> Result<()> {
    ensure!(
        !FAIL_FINAL_CHECKPOINT.with(|flag| flag.replace(false)),
        "Synthetic final checkpoint failure."
    );
    Ok(())
}

#[cfg(test)]
pub(super) fn fail_after_next_rule_final_checkpoint() {
    FAIL_AFTER_FINAL_CHECKPOINT.with(|flag| assert!(!flag.replace(true)));
}

#[cfg(test)]
pub(super) fn after_rule_final_checkpoint() -> Result<()> {
    ensure!(
        !FAIL_AFTER_FINAL_CHECKPOINT.with(|flag| flag.replace(false)),
        "Synthetic post-checkpoint failure."
    );
    Ok(())
}
