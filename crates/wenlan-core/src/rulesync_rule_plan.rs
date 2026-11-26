// SPDX-License-Identifier: Apache-2.0
//! Safe previews and line-level origins for personal rule edits.
//!
//! This module does not write anything or grant permission to apply a plan.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{ensure, Context, Result};
use sha2::{Digest, Sha256};
use wenlan_types::personal_rules::{
    PersonalRuleEdit, PersonalRuleLayer, PersonalRuleLayerOrigins, PersonalRuleOrigin,
    PersonalRulePlanChange,
};

use crate::rulesync_personal::{
    credentials::credential_line_numbers, describe_change, line_changes, personal_target,
    PersonalMaster, PERSONAL_TARGETS,
};

fn label(layer: &PersonalRuleLayer) -> String {
    match layer {
        PersonalRuleLayer::Shared => "shared rules".into(),
        PersonalRuleLayer::Tool { tool } => format!("{tool} rules"),
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{label}: possible credential on line {line}")]
pub struct PersonalRuleCredential {
    pub layer: PersonalRuleLayer,
    pub line: usize,
    label: String,
}

fn check_safe_text(layer: &PersonalRuleLayer, text: &str) -> Result<()> {
    let lines = credential_line_numbers(text);
    if let Some(line) = lines.first() {
        return Err(PersonalRuleCredential {
            layer: layer.clone(),
            line: *line,
            label: label(layer),
        }
        .into());
    }
    Ok(())
}

fn layer_text<'a>(master: &'a PersonalMaster, layer: &PersonalRuleLayer) -> Result<&'a str> {
    match layer {
        PersonalRuleLayer::Shared => Ok(&master.shared),
        PersonalRuleLayer::Tool { tool } => {
            ensure!(personal_target(tool).is_some(), "Unknown AI tool.");
            Ok(master.overlays.get(tool).map_or("", String::as_str))
        }
    }
}

fn affected_tools(master: &PersonalMaster, layer: &PersonalRuleLayer) -> Vec<String> {
    match layer {
        PersonalRuleLayer::Shared => PERSONAL_TARGETS
            .iter()
            .filter(|target| master.enrolls_target(target.id))
            .map(|target| target.id.to_owned())
            .collect(),
        PersonalRuleLayer::Tool { tool } if master.enrolls_target(tool) => vec![tool.clone()],
        PersonalRuleLayer::Tool { .. } => Vec::new(),
    }
}

/// Preview complete replacement text for each requested layer. Every layer is
/// explicit and may occur only once. Neither the master nor tool files change.
pub fn preview_personal_rule_edits(
    master: &PersonalMaster,
    edits: &[PersonalRuleEdit],
) -> Result<Vec<PersonalRulePlanChange>> {
    ensure!(
        !edits.is_empty(),
        "Choose at least one rule layer to change."
    );
    let mut layers = BTreeSet::new();
    let mut changes = Vec::new();
    for edit in edits {
        ensure!(
            layers.insert(&edit.layer),
            "Each layer can appear only once in a plan."
        );
        let before = layer_text(master, &edit.layer)?;
        check_safe_text(&edit.layer, before)?;
        check_safe_text(&edit.layer, &edit.text)?;
        // A bounded, complete diff is required; do not rely on describe_change's
        // fallback message when it cannot compare the requested text.
        line_changes(before, &edit.text).context("Cannot show the complete rule change")?;
        if before == edit.text {
            continue;
        }
        let (_, _, diff) = describe_change(&label(&edit.layer), before, &edit.text);
        let diff = crate::skill_registry::printable(&diff).replace('\t', "<U+0009>");
        changes.push(PersonalRulePlanChange {
            layer: edit.layer.clone(),
            before: before.to_owned(),
            after: edit.text.clone(),
            diff,
            affected_tools: affected_tools(master, &edit.layer),
        });
    }
    ensure!(!changes.is_empty(), "There are no rule changes to propose.");
    Ok(changes)
}

/// A hash is safe to persist only after the content has passed the credential
/// check. It identifies exact bytes, including line endings and final newline.
pub fn personal_rule_content_digest(layer: &PersonalRuleLayer, text: &str) -> Result<String> {
    check_safe_text(layer, text)?;
    Ok(hex::encode(Sha256::digest(text.as_bytes())))
}

/// Produce the line-origin record for an accepted layer change. A stale record
/// is treated as unknown, and unchanged duplicate lines with distinct origins
/// are also unknown when their identity cannot be established.
pub fn confirmed_personal_rule_origins(
    layer: &PersonalRuleLayer,
    before: &str,
    after: &str,
    previous: Option<&PersonalRuleLayerOrigins>,
    confirmed: &PersonalRuleOrigin,
) -> Result<PersonalRuleLayerOrigins> {
    ensure!(
        &confirmed.layer == layer,
        "Rule origin has the wrong layer."
    );
    let before_digest = personal_rule_content_digest(layer, before)?;
    let after_digest = personal_rule_content_digest(layer, after)?;
    let before_lines: Vec<_> = before.split_inclusive('\n').collect();
    let after_lines: Vec<_> = after.split_inclusive('\n').collect();
    let old_origins = previous
        .filter(|record| {
            record.layer == *layer
                && record.content_digest == before_digest
                && record.lines.len() == before_lines.len()
                && record
                    .lines
                    .iter()
                    .flatten()
                    .all(|origin| origin.layer == *layer)
        })
        .map(|record| record.lines.clone())
        .unwrap_or_else(|| vec![None; before_lines.len()]);

    let mut by_text: BTreeMap<&str, Vec<Option<PersonalRuleOrigin>>> = BTreeMap::new();
    for (line, origin) in before_lines.iter().zip(&old_origins) {
        by_text.entry(line).or_default().push(origin.clone());
    }
    let same_origin = |index: usize| {
        let candidates = &by_text[before_lines[index]];
        candidates
            .iter()
            .all(|candidate| candidate == &candidates[0])
            .then(|| old_origins[index].clone())
            .flatten()
    };

    let mut origins = Vec::with_capacity(after_lines.len());
    let mut old_pos = 0;
    for change in line_changes(before, after)? {
        origins.extend((old_pos..change.start).map(same_origin));
        origins.extend(change.added.iter().map(|_| Some(confirmed.clone())));
        old_pos = change.start + change.removed.len();
    }
    origins.extend((old_pos..before_lines.len()).map(same_origin));
    ensure!(
        origins.len() == after_lines.len(),
        "Cannot match rule origins safely."
    );
    Ok(PersonalRuleLayerOrigins {
        layer: layer.clone(),
        content_digest: after_digest,
        lines: origins,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn master() -> PersonalMaster {
        PersonalMaster {
            shared: "Shared rule.\n".into(),
            overlays: BTreeMap::from([
                ("cursor".into(), "Cursor rule.\n".into()),
                ("codexcli".into(), "Codex rule.\n".into()),
            ]),
            unenrolled_overlays: BTreeSet::new(),
            claude_imported_rules: None,
            codex_sync_replaced: false,
            skills: Vec::new(),
            overrides: BTreeMap::new(),
        }
    }

    fn origin(layer: PersonalRuleLayer, proposer: &str) -> PersonalRuleOrigin {
        PersonalRuleOrigin {
            author: Some("person".into()),
            proposer: Some(proposer.into()),
            confirmer: Some("person".into()),
            layer,
            time: Some(42),
        }
    }

    #[test]
    fn rulesync_rule_plan_previews_multiple_layers_without_changing_the_master() {
        let master = master();
        let edits = [
            PersonalRuleEdit {
                layer: PersonalRuleLayer::Shared,
                text: "Changed shared rule.\n".into(),
            },
            PersonalRuleEdit {
                layer: PersonalRuleLayer::Tool {
                    tool: "cursor".into(),
                },
                text: "Changed cursor rule.\n".into(),
            },
        ];
        let plan = preview_personal_rule_edits(&master, &edits).unwrap();
        assert_eq!(plan.len(), 2);
        assert_eq!(plan[0].before, "Shared rule.\n");
        assert!(plan[0].diff.contains("-Shared rule."));
        assert!(plan[0].diff.contains("+Changed shared rule."));
        assert!(plan[0].affected_tools.contains(&"codexcli".into()));
        assert_eq!(plan[1].affected_tools, vec!["cursor"]);
        assert_eq!(master.shared, "Shared rule.\n");
        assert_eq!(master.overlays["cursor"], "Cursor rule.\n");
        assert_eq!(master.overlays["codexcli"], "Codex rule.\n");
    }

    #[test]
    fn rulesync_rule_plan_refuses_secrets_and_repeated_layers_before_showing_them() {
        let master = master();
        let edit = PersonalRuleEdit {
            layer: PersonalRuleLayer::Shared,
            text: "A rule.\npassword=not-a-real-secret-9384\n".into(),
        };
        let error = preview_personal_rule_edits(&master, &[edit])
            .unwrap_err()
            .to_string();
        assert_eq!(error, "shared rules: possible credential on line 2");
        assert!(!error.contains("not-a-real-secret"));

        let edits = [
            PersonalRuleEdit {
                layer: PersonalRuleLayer::Shared,
                text: "One.\n".into(),
            },
            PersonalRuleEdit {
                layer: PersonalRuleLayer::Shared,
                text: "Two.\n".into(),
            },
        ];
        assert!(preview_personal_rule_edits(&master, &edits).is_err());
    }

    #[test]
    fn rulesync_rule_plan_marks_invisible_characters_in_the_diff() {
        let edit = PersonalRuleEdit {
            layer: PersonalRuleLayer::Shared,
            text: "A\tword\u{200b}.\r\n".into(),
        };
        let plan = preview_personal_rule_edits(&master(), &[edit]).unwrap();
        assert!(plan[0].diff.contains("<U+0009>"));
        assert!(plan[0].diff.contains("<U+200B>"));
        assert!(plan[0].diff.contains("<U+000D>"));
    }

    #[test]
    fn rulesync_rule_plan_preserves_known_origins_and_does_not_invent_old_ones() {
        let layer = PersonalRuleLayer::Shared;
        let old = "Known.\nUnknown.\n";
        let known = origin(layer.clone(), "earlier tool");
        let confirmed = origin(layer.clone(), "new tool");
        let previous = PersonalRuleLayerOrigins {
            layer: layer.clone(),
            content_digest: personal_rule_content_digest(&layer, old).unwrap(),
            lines: vec![Some(known.clone()), None],
        };
        let result = confirmed_personal_rule_origins(
            &layer,
            old,
            "Known.\nUnknown.\nAdded.\n",
            Some(&previous),
            &confirmed,
        )
        .unwrap();
        assert_eq!(
            result.lines,
            vec![Some(known), None, Some(confirmed.clone())]
        );

        let stale = confirmed_personal_rule_origins(
            &layer,
            "Known.\nChanged elsewhere.\n",
            "Known.\nChanged elsewhere.\nAdded.\n",
            Some(&previous),
            &confirmed,
        )
        .unwrap();
        assert_eq!(stale.lines, vec![None, None, Some(confirmed)]);
    }

    #[test]
    fn rulesync_rule_plan_does_not_guess_between_identical_lines_with_distinct_origins() {
        let layer = PersonalRuleLayer::Shared;
        let text = "Repeated.\nRepeated.\n";
        let previous = PersonalRuleLayerOrigins {
            layer: layer.clone(),
            content_digest: personal_rule_content_digest(&layer, text).unwrap(),
            lines: vec![
                Some(origin(layer.clone(), "first")),
                Some(origin(layer.clone(), "second")),
            ],
        };
        let result = confirmed_personal_rule_origins(
            &layer,
            text,
            "Repeated.\n",
            Some(&previous),
            &origin(layer.clone(), "third"),
        )
        .unwrap();
        assert_eq!(result.lines, vec![None]);
    }
}
