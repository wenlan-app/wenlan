// SPDX-License-Identifier: Apache-2.0
//! Recover the native rule text while undoing Wenlan's projections.

use super::*;
use std::ops::Range;
use wenlan_core::rulesync_adopt::SHARED_RULES;

pub(super) fn unknown_shared_origins(root: &FileRoot, manifest: &Manifest) -> bool {
    if manifest.rule_origins_tracked {
        return false;
    }
    let shared_path = Path::new(SHARED_RULES);
    if is_shared_source_receipt(manifest, shared_path) {
        // Before origin tracking a share could move a rule out of its only
        // originating overlay, leaving identical tool delivery receipts.
        return true;
    }
    if manifest.master.shared.trim().is_empty() {
        return false;
    }
    // Older review approvals could also change the shared master in memory.
    // Check whether its lines still have a source we can restore or leave in place.
    let source = root
        .with_shared_rule_source()
        .and_then(|root| root.read(shared_path, false));
    let source = source
        .as_ref()
        .ok()
        .and_then(|snapshot| text(snapshot).ok())
        .unwrap_or("");
    let recoverable: BTreeSet<_> = source
        .lines()
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

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(super) struct AuthoredRules {
    lines: Vec<AuthoredLine>,
    original_edited: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct AuthoredLine {
    text: String,
    shared: bool,
}

type PositionedLine = (usize, AuthoredLine);

impl AuthoredRules {
    pub(super) fn text(&self) -> impl Iterator<Item = &String> {
        self.lines.iter().map(|line| &line.text)
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
        Ok(
            (authored.original_edited || authored.lines.iter().any(|line| line.shared))
                .then_some(authored),
        )
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
        let (mut additions, original_edited) = local_additions(base, &live, &protected)?;
        mark_shared(&live, &mut additions)?;
        let lines = keep_shared_in_order(
            previous.lines,
            additions.into_iter().map(|(_, line)| line).collect(),
        )?;
        Ok(Self {
            lines,
            original_edited: original_edited || previous.original_edited,
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
