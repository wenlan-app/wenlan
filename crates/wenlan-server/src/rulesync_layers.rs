// SPDX-License-Identifier: Apache-2.0
//! Keep edits to shared instructions separate from the local rule master.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{bail, ensure, Context, Result};
use wenlan_core::rulesync_personal::{
    fingerprint, line_changes, write_named_root_rule, RULE_PRECEDENCE,
};
use wenlan_core::rulesync_team::TeamRuleLayer;
use wenlan_core::skill_grow::tool_file_text;

const START: &str = "<!-- WENLAN SHARED RULES ";
const END: &str = "<!-- END WENLAN SHARED RULES -->";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Block<'a> {
    pub(crate) revision: &'a str,
    pub(crate) body: &'a str,
    range: std::ops::Range<usize>,
}

pub(crate) fn key(layer: &TeamRuleLayer) -> Result<String> {
    fingerprint(&layer.id)
}

pub(crate) fn stage(input: &Path, prefix: &str, layers: &[TeamRuleLayer]) -> Result<()> {
    for layer in layers {
        let key = key(layer)?;
        let body = display_body(&layer.markdown);
        let name = tool_file_text(&layer.name);
        let date = tool_file_text(layer.synced_at.get(..10).unwrap_or(&layer.synced_at));
        let text = format!(
            "{START}{key} {} -->\n## {prefix} rules for {name}\n\nLast synced {date}. {RULE_PRECEDENCE}\n\n{body}\n{END}\n",
            layer.revision
        );
        write_named_root_rule(
            input,
            &format!("{}-{key}", prefix.to_ascii_lowercase()),
            &text,
        )?;
    }
    Ok(())
}

fn display_body(markdown: &str) -> String {
    markdown
        .lines()
        .map(tool_file_text)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Apply a change to the exact approved revision. Keep source wording and
/// formatting on every line that only changed during renderer escaping.
pub(crate) fn proposed_markdown(layer: &TeamRuleLayer, change: &Block<'_>) -> Result<String> {
    ensure!(
        layer.revision == change.revision,
        "this shared rule edit needs the current approved revision"
    );
    let mut source = layer
        .markdown
        .lines()
        .map(|line| format!("{line}\n"))
        .collect::<Vec<_>>();
    if source.is_empty() {
        source.push("\n".into());
    }
    let displayed = format!("{}\n", display_body(&layer.markdown));
    for edit in line_changes(&displayed, change.body)?.into_iter().rev() {
        source.splice(edit.start..edit.start + edit.removed.len(), edit.added);
    }
    Ok(source.concat())
}

fn hex(token: &str) -> bool {
    token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub(crate) fn blocks(text: &str) -> Result<BTreeMap<&str, Block<'_>>> {
    let mut result = BTreeMap::new();
    let mut lines = text.split_inclusive('\n').scan(0, |position, line| {
        let start = *position;
        *position += line.len();
        Some((start, line, *position))
    });
    while let Some((begin, line, _)) = lines.next() {
        let line = line.trim_end_matches(['\n', '\r']);
        ensure!(line != END, "unmatched shared rules end marker");
        let Some(start) = line.strip_prefix(START) else {
            continue;
        };
        let (key, revision) = start
            .strip_suffix(" -->")
            .and_then(|line| line.split_once(' '))
            .filter(|(key, revision)| hex(key) && hex(revision))
            .context("invalid shared rules marker")?;
        let mut metadata = lines.by_ref().take(4);
        for expected in ["## ", "\n", "Last synced ", "\n"] {
            ensure!(
                metadata
                    .next()
                    .is_some_and(|(_, line, _)| line.starts_with(expected)
                        || (expected == "\n" && line == "\r\n")),
                "invalid shared rules heading"
            );
        }
        let mut body_start = None;
        let mut end = None;
        for (position, body_line, next) in lines.by_ref() {
            let body_line = body_line.trim_end_matches(['\n', '\r']);
            ensure!(!body_line.starts_with(START), "nested shared rules markers");
            if body_line == END {
                end = Some((position, next));
                break;
            }
            body_start.get_or_insert(position);
        }
        let (body_end, end) = end.context("missing shared rules end marker")?;
        let body = &text[body_start.unwrap_or(body_end)..body_end];
        ensure!(
            result
                .insert(
                    key,
                    Block {
                        revision,
                        body,
                        range: begin..end
                    }
                )
                .is_none(),
            "duplicate shared rules markers"
        );
    }
    Ok(result)
}

pub(crate) fn only_personal(text: &str) -> Result<String> {
    let markers = blocks(text)?;
    Ok(without(text, markers.into_values()))
}

pub(crate) fn remove_inaccessible(text: &str, authorized: &BTreeSet<String>) -> Result<String> {
    let markers = blocks(text)?;
    Ok(without(
        text,
        markers
            .into_iter()
            .filter_map(|(key, block)| (!authorized.contains(key)).then_some(block)),
    ))
}

fn without<'a>(text: &str, blocks: impl Iterator<Item = Block<'a>>) -> String {
    let mut ranges = blocks.map(|block| block.range).collect::<Vec<_>>();
    ranges.sort_by_key(|range| range.start);
    let mut output = String::new();
    let mut start = 0;
    for range in ranges {
        output.push_str(&text[start..range.start]);
        start = range.end;
    }
    output.push_str(&text[start..]);
    output
}

pub(crate) fn edits<'a>(before: &'a str, after: &'a str) -> Result<Vec<(&'a str, Block<'a>)>> {
    let before = blocks(before)?;
    let after = blocks(after)?;
    ensure!(
        before.keys().eq(after.keys()),
        "shared rule markers changed"
    );
    let mut edits = Vec::new();
    for (key, new) in after {
        let old = &before[key];
        if old.revision != new.revision {
            bail!("a shared rules revision marker changed");
        }
        if old.body != new.body {
            edits.push((key, new));
        }
    }
    Ok(edits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isolates_edits_and_can_remove_inaccessible_layers() {
        let key = "a".repeat(64);
        let rev = "b".repeat(64);
        let before = format!("{START}{key} {rev} -->\n## Team rules\n\nLast synced date.\n\nUse A.\n{END}\nPersonal.\n");
        let after = before.replace("Use A.", "Use B.");
        let edits = edits(&before, &after).unwrap();
        assert_eq!((edits[0].0, edits[0].1.body), (key.as_str(), "Use B.\n"));
        assert_eq!(only_personal(&after).unwrap(), "Personal.\n");
        assert_eq!(
            remove_inaccessible(&before, &BTreeSet::new()).unwrap(),
            "Personal.\n"
        );
        assert!(blocks(&after.replace(END, "")).is_err());
    }
}
