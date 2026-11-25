// SPDX-License-Identifier: Apache-2.0
//! Recover the native rule text while undoing Wenlan's projections.

use super::*;
use std::ops::Range;

pub(super) fn unknown_shared_origins(manifest: &Manifest) -> bool {
    if manifest.rule_origins_tracked || manifest.master.shared.trim().is_empty() {
        return false;
    }
    // Old backups may still preserve an original copy. Use saved rule files
    // and known donors so restore does not need the imported source.
    let recoverable: BTreeSet<_> = manifest
        .backup
        .iter()
        .filter(|(path, _)| {
            manifest
                .delivered
                .get(*path)
                .is_some_and(|receipt| matches!(receipt.kind, Kind::Rule { .. }))
        })
        .filter_map(|(_, snapshot)| text(snapshot).ok())
        .flat_map(str::lines)
        .chain(manifest.authored_rules.values().flat_map(|authored| {
            authored
                .lines
                .iter()
                .filter(|line| line.shared)
                .map(|line| line_key(&line.text))
        }))
        .collect();
    manifest
        .master
        .shared
        .lines()
        .any(|line| !line.trim().is_empty() && !recoverable.contains(line))
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(super) struct AuthoredRules {
    lines: Vec<AuthoredLine>,
    original_edited: bool,
    // The personal text against which the unshared line positions were saved.
    // Delivery receipts advance as overlays change, so they cannot serve as
    // the ownership baseline for a reviewed addition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reference: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct AuthoredLine {
    text: String,
    shared: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    position: Option<usize>,
}

type PositionedLine = (usize, AuthoredLine);

impl AuthoredRules {
    pub(super) fn text(&self) -> impl Iterator<Item = &String> {
        self.lines
            .iter()
            .map(|line| &line.text)
            .chain(self.reference.iter())
    }

    pub(super) fn line_keys(&self) -> impl Iterator<Item = &str> {
        self.lines.iter().map(|line| line_key(&line.text))
    }

    fn check_shared_known(&self, shared: &str) -> Result<()> {
        ensure!(
            self.lines
                .iter()
                .filter(|line| line.shared)
                .all(|line| shared.lines().any(|item| item == line_key(&line.text))),
            "A previously shared rule has an unclear origin. The current file was left in place."
        );
        Ok(())
    }

    pub(super) fn remove_deleted_shared(&mut self, before: &str, after: &str) -> Result<()> {
        self.check_shared_known(before)?;
        for line in self.lines.iter().filter(|line| line.shared) {
            let key = line_key(&line.text);
            let old_count = before.lines().filter(|item| *item == key).count();
            let new_count = after.lines().filter(|item| *item == key).count();
            ensure!(
                new_count == 0 || old_count <= 1 || old_count == new_count,
                "A shared rule appears more than once and its origin is unclear. The rules were left unchanged."
            );
        }
        self.lines.retain(|line| {
            !line.shared
                || !before.lines().any(|item| item == line_key(&line.text))
                || after.lines().any(|item| item == line_key(&line.text))
        });
        Ok(())
    }

    pub(super) fn for_tool_edit(
        manifest: &Manifest,
        path: &Path,
        seen: &Snapshot,
    ) -> Result<Option<Self>> {
        Self::capture_promotion(manifest, path, seen, |_, _| Ok(()))
    }

    // A confirmed plan can delete a previously accepted addition. Rebase its
    // occurrence onto the planned source before any destination is written.
    pub(super) fn for_plan(
        &self,
        source: &str,
        shared_before: &str,
        shared_after: &str,
        overlay_before: &str,
        overlay_after: &str,
    ) -> Result<Self> {
        let mut local: BTreeMap<_, _> = retained_local_lines(self, source)?.into_iter().collect();
        let mut previous = self.clone();
        previous.remove_deleted_shared(shared_before, shared_after)?;
        let mut reconciled = Vec::new();
        for line in &self.lines {
            if !line.shared {
                reconciled.push(line.clone());
                continue;
            }
            let key = line_key(&line.text);
            let removed_from_shared = shared_before.lines().any(|item| item == key)
                && !shared_after.lines().any(|item| item == key);
            if !removed_from_shared {
                reconciled.push(line.clone());
                continue;
            }
            let old_overlay = overlay_before.lines().filter(|item| *item == key).count();
            let new_overlay = overlay_after.lines().filter(|item| *item == key).count();
            if new_overlay <= old_overlay {
                ensure!(
                    new_overlay == 0,
                    "A shared rule and a tool rule have an unclear origin. The rules were left unchanged."
                );
                continue;
            }
            let positions: Vec<_> = source
                .split_inclusive('\n')
                .enumerate()
                .filter_map(|(index, item)| (line_key(item) == key).then_some(index))
                .collect();
            ensure!(
                old_overlay == 0
                    && new_overlay == 1
                    && positions.len() == 1
                    && self
                        .lines
                        .iter()
                        .filter(|item| item.shared && line_key(&item.text) == key)
                        .count()
                        == 1,
                "A moved shared rule has an unclear origin. The rules were left unchanged."
            );
            let mut moved = line.clone();
            moved.shared = false;
            moved.position = Some(positions[0]);
            local.insert(positions[0], moved.clone());
            reconciled.push(moved);
        }
        previous.lines = reconciled;
        let local = local.into_values().collect();
        let lines = keep_shared_in_order(previous.lines, local)?;
        Ok(Self {
            lines,
            original_edited: previous.original_edited,
            reference: Some(source.to_owned()),
        })
    }

    // Promotion changes the master and moves the donor's lines out of its
    // overlay. Save their ownership and order along with that transaction.
    pub(super) fn for_promotion(
        manifest: &Manifest,
        path: &Path,
        seen: &Snapshot,
        moving: &BTreeSet<&str>,
    ) -> Result<Option<Self>> {
        Self::capture_promotion(manifest, path, seen, |_, additions| {
            for (_, line) in additions {
                line.shared = moving.contains(line_key(&line.text));
            }
            Ok(())
        })
    }

    pub(super) fn for_review(
        manifest: &Manifest,
        path: &Path,
        seen: &Snapshot,
        before: &str,
        after: &str,
    ) -> Result<Option<Self>> {
        let added: BTreeSet<_> = edit_ranges(before, after)?
            .into_iter()
            .flat_map(|(_, added)| added)
            .collect();
        Self::capture_promotion(manifest, path, seen, |live, additions| {
            // Review shares changed occurrences, even when another copy of
            // the same text was previously kept in the tool.
            let mut changes = edit_ranges(after, live)?.into_iter().peekable();
            let (mut old_end, mut live_end) = (0, 0);
            for (index, line) in additions {
                while let Some((old, new)) = changes.next_if(|(_, new)| new.end <= *index) {
                    old_end = old.end;
                    live_end = new.end;
                }
                if changes.peek().is_none_or(|(_, new)| *index < new.start) {
                    line.shared = added.contains(&(old_end + *index - live_end));
                }
            }
            Ok(())
        })
    }

    fn capture_promotion(
        manifest: &Manifest,
        path: &Path,
        seen: &Snapshot,
        mark_shared: impl FnOnce(&str, &mut [PositionedLine]) -> Result<()>,
    ) -> Result<Option<Self>> {
        let authored = Self::from_file(manifest, path, seen, mark_shared)?;
        Ok((authored.original_edited || !authored.lines.is_empty()).then_some(authored))
    }

    fn from_file(
        manifest: &Manifest,
        path: &Path,
        seen: &Snapshot,
        mark_shared: impl FnOnce(&str, &mut [PositionedLine]) -> Result<()>,
    ) -> Result<Self> {
        let receipt = &manifest.delivered[path];
        let Kind::Rule { base, source, .. } = &receipt.kind else {
            unreachable!("only rule files have authored rule text");
        };
        let live = if *seen == receipt.delivered {
            source.clone()
        } else {
            let delivered = text(&receipt.delivered)?;
            let current = text(seen)?;
            ensure!(
                rulesync_layers::edits(delivered, current)?.is_empty(),
                "A shared rules section was edited. Copy the edits you want to keep out of that section before restoring."
            );
            ensure!(
                same_rule_lines(&rulesync_layers::only_personal(delivered)?, source),
                "The current file cannot be compared safely with the delivered rules."
            );
            rulesync_layers::only_personal(current)?
        };
        let previous = manifest
            .authored_rules
            .get(path)
            .cloned()
            .unwrap_or_default();
        previous.check_shared_known(&manifest.master.shared)?;
        let original = match &manifest.backup[path] {
            Snapshot::Symlink { .. } => base,
            snapshot => text(snapshot)?,
        };
        let protected: BTreeSet<_> = original
            .lines()
            .chain(
                previous
                    .lines
                    .iter()
                    .filter(|line| line.shared)
                    .map(|line| line_key(&line.text)),
            )
            .collect();
        let (additions, original_edited) = local_additions(base, &live, &protected)?;
        if previous.reference.is_none() {
            let mut current = additions.iter();
            for old in previous.lines.iter().filter(|line| !line.shared) {
                ensure!(
                    current.any(|(_, line)| line_key(&line.text) == line_key(&old.text)),
                    "An older saved rule has an unclear origin. The current file was left in place."
                );
            }
        }
        let mut combined: BTreeMap<_, _> = additions.into_iter().collect();
        // Prefer the originally accepted bytes to renderer-normalized output.
        combined.extend(retained_local_lines(&previous, &live)?);
        let mut additions: Vec<_> = combined.into_iter().collect();
        mark_shared(&live, &mut additions)?;
        for (_, line) in &mut additions {
            if line.shared {
                line.position = None;
            }
        }
        let lines = keep_shared_in_order(
            previous.lines,
            additions.into_iter().map(|(_, line)| line).collect(),
        )?;
        Ok(Self {
            lines,
            original_edited: original_edited || previous.original_edited,
            reference: Some(live),
        })
    }
}

pub(super) fn restored_rule(manifest: &Manifest, path: &Path, seen: &Snapshot) -> Result<Snapshot> {
    let original = &manifest.backup[path];
    if seen == original {
        return Ok(original.clone());
    }
    let latest = &manifest.delivered[path].delivered;
    ensure!(
        match (latest, seen) {
            (Snapshot::File { mode: before, .. }, Snapshot::File { mode: after, .. }) =>
                before == after,
            (Snapshot::Absent, Snapshot::File { .. } | Snapshot::Absent) => true,
            _ => false,
        },
        "The file type or permissions changed since delivery."
    );
    let authored = AuthoredRules::from_file(manifest, path, seen, |_, _| Ok(()))?;
    ensure!(
        !authored.original_edited,
        "An original rule or a rule you previously shared was edited or removed. The current file has been left in place."
    );
    if authored.lines.is_empty() {
        return Ok(original.clone());
    }
    ensure!(
        !matches!(original, Snapshot::Symlink { .. }),
        "Restoring the original link would remove rules added to this file."
    );
    let mut restored = text(original)?.to_owned();
    for line in authored.lines {
        if !restored.is_empty() && !restored.ends_with('\n') {
            if line.text.ends_with("\r\n") {
                restored.push('\r');
            }
            restored.push('\n');
        }
        restored.push_str(&line.text);
    }
    let mode_from = if *original == Snapshot::Absent {
        seen
    } else {
        original
    };
    let restored = with_bytes(mode_from, restored.into_bytes());
    ensure!(
        !snapshot_has_credentials(&restored),
        "{CREDENTIAL_FILE_REASON}"
    );
    Ok(restored)
}

fn line_key(raw: &str) -> &str {
    raw.strip_suffix("\r\n")
        .or_else(|| raw.strip_suffix('\n'))
        .unwrap_or(raw)
}

fn normalized<'a>(lines: impl Iterator<Item = &'a str>) -> String {
    lines.flat_map(|line| [line_key(line), "\n"]).collect()
}

fn edit_ranges(before: &str, after: &str) -> Result<Vec<(Range<usize>, Range<usize>)>> {
    let (mut old_end, mut new_end) = (0, 0);
    let changes = line_changes(
        &normalized(before.split_inclusive('\n')),
        &normalized(after.split_inclusive('\n')),
    )?;
    Ok(changes
        .into_iter()
        .map(|change| {
            let new_start = new_end + change.start - old_end;
            old_end = change.start + change.removed.len();
            new_end = new_start + change.added.len();
            (change.start..old_end, new_start..new_end)
        })
        .collect())
}

fn local_additions(
    base: &str,
    live: &str,
    original: &BTreeSet<&str>,
) -> Result<(Vec<PositionedLine>, bool)> {
    // Align line contents through renderer newline normalization. Take the
    // added bytes from the source, including whitespace and line endings.
    let before: Vec<_> = base.split_inclusive('\n').collect();
    let after: Vec<_> = live.split_inclusive('\n').collect();
    let mut additions = Vec::new();
    let mut removed_lines = BTreeSet::new();
    let mut original_edited = false;
    for (removed, added) in edit_ranges(base, live)? {
        let removed = before[removed].iter().map(|line| line_key(line));
        original_edited |= removed
            .clone()
            .any(|line| !line.trim().is_empty() && original.contains(line));
        removed_lines.extend(removed.filter(|line| !line.trim().is_empty()));
        additions.extend(added.map(|at| {
            (
                at,
                AuthoredLine {
                    text: after[at].to_owned(),
                    shared: false,
                    position: Some(at),
                },
            )
        }));
    }
    ensure!(
        !additions.iter().any(|(_, line)| removed_lines.contains(line_key(&line.text))),
        "Rules were rearranged and their origin is unclear. The current file has been left in place."
    );
    Ok((additions, original_edited))
}

fn retained_local_lines(previous: &AuthoredRules, live: &str) -> Result<Vec<PositionedLine>> {
    let Some(reference) = &previous.reference else {
        // Old receipts did not persist local positions. Their local lines are
        // still recovered from the delivery baseline by local_additions.
        return Ok(Vec::new());
    };
    let before: Vec<_> = reference.split_inclusive('\n').collect();
    let after: Vec<_> = live.split_inclusive('\n').collect();
    let changes = edit_ranges(reference, live)?;
    let mut retained = Vec::new();
    for line in previous.lines.iter().filter(|line| !line.shared) {
        let Some(position) = line.position else {
            continue;
        };
        ensure!(
            before
                .get(position)
                .is_some_and(|item| line_key(item) == line_key(&line.text)),
            "The saved rule origin cannot be verified. The current file was left in place."
        );
        let key = line_key(&line.text);
        let count = before.iter().filter(|item| line_key(item) == key).count();
        let matching_after: Vec<_> = after
            .iter()
            .enumerate()
            .filter_map(|(index, item)| (line_key(item) == key).then_some(index))
            .collect();
        let changed_occurrence = changes.iter().any(|(old, new)| {
            before[old.clone()].iter().any(|item| line_key(item) == key)
                || after[new.clone()].iter().any(|item| line_key(item) == key)
        });
        ensure!(
            !changed_occurrence
                || matching_after.is_empty()
                || (count == 1 && matching_after.len() == 1),
            "A repeated rule has an unclear origin. The rules were left unchanged."
        );
        let mut old_end = 0;
        let mut new_end = 0;
        let mut mapped = None;
        let mut removed = false;
        for (old, new) in &changes {
            if old.contains(&position) {
                // Sharing another line can move this unique occurrence in
                // the projection. Keep it if it is still present exactly once.
                mapped = matching_after.first().copied();
                removed = mapped.is_none();
                break;
            }
            if position < old.start {
                mapped = Some(new_end + position - old_end);
                break;
            }
            old_end = old.end;
            new_end = new.end;
        }
        if removed {
            continue;
        }
        let position = mapped.unwrap_or(new_end + position - old_end);
        ensure!(
            after
                .get(position)
                .is_some_and(|item| line_key(item) == key),
            "The saved rule origin cannot be verified. The current file was left in place."
        );
        let mut line = line.clone();
        line.position = Some(position);
        retained.push((position, line));
    }
    Ok(retained)
}

fn keep_shared_in_order(
    previous: Vec<AuthoredLine>,
    local: Vec<AuthoredLine>,
) -> Result<Vec<AuthoredLine>> {
    if previous.is_empty() {
        return Ok(local);
    }
    let mut edits = line_changes(
        &normalized(
            previous
                .iter()
                .filter(|line| !line.shared)
                .map(|line| line.text.as_str()),
        ),
        &normalized(local.iter().map(|line| line.text.as_str())),
    )?
    .into_iter()
    .peekable();
    let mut current = local.into_iter();
    let mut ordered = Vec::new();
    let (mut slot, mut old_end, mut new_end, mut consumed) = (0, 0, 0, 0);
    for line in previous {
        if !line.shared {
            slot += 1;
            continue;
        }
        while let Some(edit) = edits.next_if(|edit| edit.start + edit.removed.len() < slot) {
            new_end += edit.start - old_end + edit.added.len();
            old_end = edit.start + edit.removed.len();
        }
        // If an edit spans the old slot, keep the already-authored shared line
        // ahead of the new text at that slot.
        let anchor = edits.peek().map_or(slot, |edit| slot.min(edit.start));
        let next = new_end + anchor - old_end;
        ordered.extend(current.by_ref().take(next - consumed));
        ordered.push(line);
        consumed = next;
    }
    ordered.extend(current);
    Ok(ordered)
}
