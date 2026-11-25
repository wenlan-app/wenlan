// SPDX-License-Identifier: Apache-2.0

use super::{rulesync_layers, text, Delivery, Kind, LineChange, Snapshot};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use wenlan_core::rulesync_personal::line_changes;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Layer {
    Shared,
    Overlay(usize),
    Other,
}

pub(super) fn overlay_source(base: &str, shared: usize, authored: usize) -> Option<&str> {
    let overlay = base.get(shared..authored)?;
    // A separator after a shared file without a final newline is not overlay text.
    Some(if shared > 0 && !base[..shared].ends_with('\n') {
        overlay.strip_prefix('\n').unwrap_or(overlay)
    } else {
        overlay
    })
}

pub(super) fn record(kind: &mut Kind, delivered: &Snapshot) {
    let Kind::Rule {
        base,
        source,
        authored_ends: Some([shared, authored]),
        line_layers,
        ..
    } = kind
    else {
        return;
    };
    *line_layers = tags(base, source, *shared, *authored, delivered).unwrap_or_default();
}

fn tags(
    base: &str,
    source: &str,
    shared: usize,
    authored: usize,
    delivered: &Snapshot,
) -> Option<Vec<Layer>> {
    let start = base.get(..shared)?.lines().count();
    let overlay = overlay_source(base, shared, authored)?.lines().count();
    let mut tags = vec![Layer::Shared; start];
    tags.extend((0..overlay).map(Layer::Overlay));
    tags.resize(base.lines().count(), Layer::Other);
    // Unchanged source lines retain their layers across local overrides.
    for change in line_changes(base, source).ok()?.into_iter().rev() {
        tags.splice(
            change.start..change.start + change.removed.len(),
            vec![Layer::Other; change.added.len()],
        );
    }
    let personal = rulesync_layers::only_personal(text(delivered).ok()?).ok()?;
    let lines: Vec<_> = personal.lines().collect();
    let source: Vec<_> = source.lines().collect();
    let first = source.iter().position(|line| !line.is_empty())?;
    let end = source.iter().rposition(|line| !line.is_empty())? + 1;
    // The renderer may add a heading or trim edge newlines. Require a unique
    // matching run before attributing any output line to the source.
    let mut matches = lines
        .windows(end - first)
        .enumerate()
        .filter(|(_, run)| *run == &source[first..end]);
    let (at, _) = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    let mut delivered_tags = vec![Layer::Other; lines.len()];
    delivered_tags[at..at + end - first].copy_from_slice(&tags[first..end]);
    let outside_overlay: BTreeSet<_> = lines
        .iter()
        .zip(&delivered_tags)
        .filter_map(|(line, tag)| (!matches!(tag, Layer::Overlay(_))).then_some(*line))
        .collect();
    for (line, tag) in lines.iter().zip(&mut delivered_tags) {
        if matches!(tag, Layer::Overlay(_))
            && !line.trim().is_empty()
            && outside_overlay.contains(line)
        {
            *tag = Layer::Other;
        }
    }
    Some(delivered_tags)
}

pub(super) enum OverlayEdit {
    ToolOnly(String),
    Mixed,
}

pub(super) fn overlay_edit(
    delivery: &Delivery,
    before: &str,
    after: &str,
    changes: &[LineChange],
) -> Option<OverlayEdit> {
    let Kind::Rule {
        base,
        authored_ends: Some([shared, authored]),
        line_layers,
        ..
    } = &delivery.kind
    else {
        return None;
    };
    if line_layers.is_empty()
        || delivery.observed.is_some()
            && rulesync_layers::only_personal(text(&delivery.delivered).ok()?).ok()? != before
    {
        return None;
    }
    // Adding a final newline can make an end-of-file insertion look like a
    // replacement of the previous line. The inserted text has no proven layer.
    if !before.ends_with('\n')
        && after
            .strip_prefix(before)
            .is_some_and(|tail| tail.starts_with(['\n', '\r']))
    {
        return None;
    }
    let source = overlay_source(base, *shared, *authored)?;
    let mut edited: Vec<_> = source.split_inclusive('\n').map(str::to_owned).collect();
    let mut touches_overlay = false;
    let mut touches_other = false;
    for change in changes.iter().rev() {
        let end = change.start + change.removed.len();
        let Some(Layer::Overlay(at)) = line_layers.get(change.start).copied() else {
            touches_other = true;
            touches_overlay |= line_layers
                .get(change.start..end)?
                .iter()
                .any(|tag| matches!(tag, Layer::Overlay(_)));
            continue;
        };
        if change.removed.is_empty() {
            // An insertion at a boundary has no proven layer.
            let previous = change.start.checked_sub(1).and_then(|i| line_layers.get(i));
            if !matches!(previous, Some(Layer::Overlay(i)) if i + 1 == at) {
                touches_other = true;
                continue;
            }
        } else {
            touches_overlay = true;
            if !line_layers
                .get(change.start..end)?
                .iter()
                .copied()
                .eq((at..at + change.removed.len()).map(Layer::Overlay))
            {
                touches_other = true;
                continue;
            }
        }
        let until = at + change.removed.len();
        if !edited
            .get(at..until)?
            .iter()
            .map(|line| line.trim_end_matches(['\r', '\n']))
            .eq(change
                .removed
                .iter()
                .map(|line| line.trim_end_matches(['\r', '\n'])))
        {
            return None;
        }
        edited.splice(at..until, change.added.iter().cloned());
        touches_overlay = true;
    }
    match (touches_overlay, touches_other) {
        (true, false) => Some(OverlayEdit::ToolOnly(edited.concat())),
        (true, true) => Some(OverlayEdit::Mixed),
        _ => None,
    }
}
