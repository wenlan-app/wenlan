// SPDX-License-Identifier: Apache-2.0
//! Keep edits to shared instructions separate from the local rule master.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::rulesync_apply::{FileRoot, Snapshot};
use crate::rulesync_shared_answers::{self, Answer, Answers, Waiting};
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use wenlan_core::rulesync_personal::{
    fingerprint, line_changes, write_named_root_rule, RULE_PRECEDENCE,
};
use wenlan_core::rulesync_team::{self, TeamRuleLayer};
use wenlan_core::skill_grow::tool_file_text;

const START: &str = "<!-- WENLAN SHARED RULES ";
const END: &str = "<!-- END WENLAN SHARED RULES -->";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Block<'a> {
    pub(crate) revision: &'a str,
    pub(crate) body: &'a str,
    header: &'a str,
    footer: &'a str,
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
        let body_start = body_start.unwrap_or(body_end);
        let body = &text[body_start..body_end];
        ensure!(
            result
                .insert(
                    key,
                    Block {
                        revision,
                        body,
                        header: &text[begin..body_start],
                        footer: &text[body_end..end],
                        range: begin..end
                    }
                )
                .is_none(),
            "duplicate shared rules markers"
        );
    }
    Ok(result)
}

/// Collect only new lines whose origin inside a revoked section is unambiguous.
pub(crate) fn revoked_additions(
    before: &str,
    delivered: &str,
    after: &str,
    accessible: &BTreeSet<String>,
) -> Result<String> {
    let old = blocks(before)?;
    let delivered = blocks(delivered)?;
    let current = blocks(after)?;
    let mut additions = String::new();
    for (key, new) in current
        .iter()
        .filter(|(key, _)| !accessible.contains(**key))
    {
        if delivered.get(key).is_some_and(|block| {
            block.header == new.header && block.body == new.body && block.footer == new.footer
        }) {
            continue;
        }
        let old = old
            .get(key)
            .or_else(|| delivered.get(key))
            .context("revoked shared section has no matching delivery")?;
        ensure!(
            old.header == new.header && old.footer == new.footer,
            "revoked shared section markers or heading changed"
        );
        for change in line_changes(old.body, new.body)? {
            ensure!(
                change.removed.is_empty(),
                "revoked shared section has changes that cannot be separated safely"
            );
            for line in change.added {
                additions.push_str(&line);
            }
        }
    }
    Ok(additions)
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

pub(crate) fn replace_block_body(text: &str, key: &str, body: &str) -> Result<String> {
    let blocks = blocks(text)?;
    let block = blocks.get(key).context("shared rule section is missing")?;
    let start = block.range.start + block.header.len();
    let end = block.range.end - block.footer.len();
    Ok(format!("{}{}{}", &text[..start], body, &text[end..]))
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
        ensure!(
            old.header == new.header && old.footer == new.footer,
            "a shared rules heading or marker changed"
        );
        if old.body != new.body {
            edits.push((key, new));
        }
    }
    Ok(edits)
}

/// The local file's exact edits and their IDs in the Space's proposal history.
/// The approved delivery stays the baseline until the proposal is decided.
pub(crate) type ProposalReceipts = BTreeMap<String, ProposalReceipt>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ProposalReceipt {
    edit: String,
    pub(crate) proposal: String,
}

pub(crate) enum SharedReview {
    NeedsConfirm,
    Pending,
    Decided { rejected: bool },
}

fn recorded_path(
    data: &Path,
    home: &Path,
    file: &str,
) -> Option<(std::path::PathBuf, std::path::PathBuf)> {
    let relative = Path::new(file);
    if relative
        .components()
        .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return None;
    }
    let root = if relative.starts_with("checkouts") {
        data
    } else {
        home
    };
    Some((root.to_path_buf(), relative.to_path_buf()))
}

pub(crate) fn waiting_in_file(
    waiting: &Waiting,
    file: &str,
    _settings: &Path,
    data: &Path,
    home: &Path,
) -> bool {
    let Some((root_path, relative)) = recorded_path(data, home, file) else {
        return false;
    };
    let Ok(root) = FileRoot::open(&root_path) else {
        return false;
    };
    let Ok(Snapshot::File { bytes, .. }) = root.read(&relative, false) else {
        return false;
    };
    let Ok(source) = std::str::from_utf8(&bytes) else {
        return false;
    };
    let Ok(blocks) = blocks(source) else {
        return false;
    };
    let Ok(key) = fingerprint(&waiting.layer_id) else {
        return false;
    };
    let Some(block) = blocks.get(key.as_str()) else {
        return false;
    };
    block.revision == waiting.revision
        && block.body == format!("{}\n", display_body(&waiting.markdown))
}

pub(crate) fn refresh_answers(settings: &Path, data: &Path, home: &Path) -> Result<Answers> {
    let mut answers = rulesync_shared_answers::read(data)?;
    if !answers.sent_edits.is_empty() {
        let layers: Vec<_> = rulesync_team::load_layers(settings)?
            .into_iter()
            .chain(rulesync_team::load_project_layers(settings)?)
            .collect();
        let sent: Vec<_> = answers.sent_edits.keys().cloned().collect();
        for id in sent {
            let record = &answers.sent_edits[&id];
            let layer = layers.iter().find(|layer| layer.id == record.edit.layer_id);
            let decided =
                layer.is_some_and(|layer| layer.proposal_decisions.contains_key(&record.proposal));
            let queued = record.proposal_path.try_exists()?;
            if decided || layer.is_none() || !queued {
                if !decided && queued {
                    rulesync_team::retire_queued_proposal_at(
                        &record.proposal_path,
                        &record.edit.revision,
                        &record.proposal,
                    )?;
                }
                answers.sent_edits.remove(&id);
            }
        }
    }
    answers.waiting.retain(|_, waiting| {
        waiting
            .files
            .iter()
            .any(|file| waiting_in_file(waiting, file, settings, data, home))
    });
    let settled: Vec<_> = answers
        .answered_edits
        .iter()
        .filter(|(id, record)| {
            let live: Vec<_> = record
                .edit
                .files
                .iter()
                .filter(|file| waiting_in_file(&record.edit, file, settings, data, home))
                .collect();
            live.is_empty()
                || (matches!(answers.answers.get(*id), Some(Answer::Send { .. }))
                    && live.iter().all(|file| record.received.contains(file)))
        })
        .map(|(id, _)| id.clone())
        .collect();
    for id in settled {
        answers.answered_edits.remove(&id);
        answers.answers.remove(&id);
    }
    if data.exists() {
        rulesync_shared_answers::save(data, &answers)?;
    }
    Ok(answers)
}

pub(crate) fn review_edits(
    _settings: &Path,
    accessible: &BTreeMap<String, &TeamRuleLayer>,
    edits: Vec<(&str, Block<'_>)>,
    receipts: &mut ProposalReceipts,
    file: &Path,
    data: &Path,
    home: &Path,
) -> Result<SharedReview> {
    let mut answers = refresh_answers(_settings, data, home)?;
    receipts.retain(|key, receipt| {
        edits.iter().any(|(changed, _)| *changed == key)
            && (answers
                .sent_edits
                .values()
                .any(|sent| sent.proposal == receipt.proposal)
                || accessible
                    .get(key)
                    .is_some_and(|layer| layer.proposal_decisions.contains_key(&receipt.proposal)))
    });
    let mut pending = false;
    let mut rejected = false;
    let mut needs_confirm = false;
    for (key, change) in edits {
        let layer = accessible
            .get(key)
            .context("This shared layer is no longer accessible.")?;
        let id = fingerprint(&(&layer.id, change.revision, change.body))?;
        let edit = fingerprint(&(change.revision, change.body))?;
        if let Some(receipt) = receipts.get(key) {
            let decision = layer.proposal_decisions.get(&receipt.proposal);
            if receipt.edit == edit {
                pending |= decision.is_none();
                rejected |= decision == Some(&false);
                continue;
            }
            ensure!(decision.is_some(), "This section changed again while its proposal is open. Your latest edit is still in the file.");
        }
        let proposal = match answers.answers.get(&id) {
            Some(Answer::Send { proposal }) => proposal.clone(),
            Some(Answer::Decline) => {
                rejected = true;
                continue;
            }
            None => {
                if change.revision != layer.revision {
                    ensure!(
                        answers.waiting.contains_key(&id),
                        "this shared rule edit needs the current approved revision"
                    );
                    needs_confirm = true;
                    continue;
                }
                let markdown = proposed_markdown(layer, &change)?;
                let path = file.display().to_string();
                let waiting = answers.waiting.entry(id).or_insert_with(|| Waiting {
                    layer_id: layer.id.clone(),
                    layer_name: layer.name.clone(),
                    revision: change.revision.to_owned(),
                    markdown,
                    files: Vec::new(),
                    first_seen: chrono::Utc::now().to_rfc3339(),
                });
                if !waiting.files.contains(&path) {
                    waiting.files.push(path);
                }
                needs_confirm = true;
                continue;
            }
        };
        let decision = layer.proposal_decisions.get(&proposal);
        pending |= decision.is_none();
        rejected |= decision == Some(&false);
        receipts.insert(key.to_owned(), ProposalReceipt { edit, proposal });
        if let Some(record) = answers.answered_edits.get_mut(&id) {
            let path = file.display().to_string();
            if !record.received.contains(&path) {
                record.received.push(path);
            }
        }
    }
    let settled: Vec<_> = answers
        .answered_edits
        .iter()
        .filter(|(id, record)| {
            matches!(answers.answers.get(*id), Some(Answer::Send { .. }))
                && record
                    .edit
                    .files
                    .iter()
                    .filter(|file| waiting_in_file(&record.edit, file, _settings, data, home))
                    .all(|file| record.received.contains(file))
        })
        .map(|(id, _)| id.clone())
        .collect();
    for id in settled {
        answers.answered_edits.remove(&id);
        answers.answers.remove(&id);
    }
    rulesync_shared_answers::save(data, &answers)?;
    Ok(if needs_confirm {
        SharedReview::NeedsConfirm
    } else if pending {
        SharedReview::Pending
    } else {
        SharedReview::Decided { rejected }
    })
}

pub(crate) fn report_rejection(result: &mut wenlan_types::rulesync::RulesSyncReport) {
    use wenlan_types::rulesync::RulesSetupCode;
    let notice = RulesSetupCode::SharedRuleRejected.into();
    if !result.notice_codes.contains(&notice) {
        result
            .message
            .push_str("\nA shared rule edit was rejected. Wenlan restored the approved rules.");
        result.notice_codes.push(notice);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use wenlan_core::{rulesync_team, shared_space, team};
    use wenlan_types::team::{TeamAccess, TeamSpaceAccess};

    pub(crate) const PROJECT: &str = "B1 test project";
    pub(crate) const REMOTE: &str = "https://github.com/fixture/rules-test.git";
    pub(crate) const APPROVED: &str = "Use the approved checklist.\n";

    pub(crate) fn hosted_fixture(data: &Path) -> (std::path::PathBuf, team::Member, team::Member) {
        let settings = data.join("shared-spaces.json");
        let team_path = data.join(team::TEAM_FILE);
        let (owner, _) = team::owner_key(&team_path, "Owner").unwrap();
        std::fs::write(data.join("repo-wiki.json"), serde_json::to_vec(&serde_json::json!({
            "bindings": [{"space": PROJECT, "repo_root": data.join("repo"), "folder": "wiki", "remote_url": REMOTE}]
        })).unwrap()).unwrap();
        team::set_everyone(&team_path, rulesync_team::SPACE, Some(TeamAccess::Read)).unwrap();
        for name in [rulesync_team::SPACE, PROJECT] {
            let scope = shared_space::Scope::Space(name.into());
            let folder = team::new_hosted_folder(&settings);
            shared_space::join_at(&settings, &scope, &folder, "Owner").unwrap();
            shared_space::update_share(&settings, &scope, |share| share.hosted = true).unwrap();
            shared_space::ensure_space_file(
                &folder,
                &scope,
                name,
                &shared_space::SpaceMember {
                    id: owner.id.clone(),
                    name: owner.name.clone(),
                },
            )
            .unwrap();
            let proposal = rulesync_team::update_hosted_rules(&settings, name, APPROVED).unwrap();
            rulesync_team::decide_local_space_proposal(&settings, name, &proposal, true).unwrap();
        }
        let (writer, _) = team::add_member(
            &team_path,
            "Writer",
            &[rulesync_team::SPACE, PROJECT].map(|space| TeamSpaceAccess {
                space: space.into(),
                access: TeamAccess::Write,
                id: None,
            }),
        )
        .unwrap();
        (settings, owner, writer)
    }

    pub(crate) fn render_fixture_section(layer: &TeamRuleLayer) -> String {
        let stage_dir = tempfile::tempdir().unwrap();
        let label = if layer.project.is_some() {
            "Project"
        } else {
            "Team"
        };
        stage(stage_dir.path(), label, std::slice::from_ref(layer)).unwrap();
        let path = stage_dir.path().join("rules").join(format!(
            "{}-{}.md",
            label.to_ascii_lowercase(),
            key(layer).unwrap()
        ));
        std::fs::read_to_string(path)
            .unwrap()
            .strip_prefix("---\nroot: true\n---\n")
            .unwrap()
            .to_owned()
    }

    #[test]
    fn rulesync_shared_section_uses_current_precedence() {
        let scratch = tempfile::tempdir().unwrap();
        let (settings, _, _) = hosted_fixture(scratch.path());
        for layer in rulesync_team::load_layers(&settings).unwrap() {
            let section = render_fixture_section(&layer);
            assert!(section.contains(RULE_PRECEDENCE));
            assert!(!section.contains(wenlan_core::rulesync_personal::LEGACY_RULE_PRECEDENCE));
        }
    }

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

    #[test]
    fn rulesync_declined_shared_edit_restores_approved_lines() {
        let scratch = tempfile::tempdir().unwrap();
        let (settings, _, _) = hosted_fixture(scratch.path());
        let layer = rulesync_team::load_layers(&settings).unwrap().remove(0);
        let before = render_fixture_section(&layer);
        let after = before.replace(APPROVED, "Use the revised checklist.\n");
        let accessible = BTreeMap::from([(key(&layer).unwrap(), &layer)]);
        let mut receipts = ProposalReceipts::new();
        let file = Path::new("AGENTS.md");
        std::fs::write(scratch.path().join(file), &after).unwrap();
        let changed = edits(&before, &after).unwrap();
        assert!(matches!(
            review_edits(
                &settings,
                &accessible,
                changed,
                &mut receipts,
                file,
                scratch.path(),
                scratch.path()
            )
            .unwrap(),
            SharedReview::NeedsConfirm
        ));
        let mut answers = rulesync_shared_answers::read(scratch.path()).unwrap();
        assert_eq!(answers.waiting.len(), 1);
        let id = answers.waiting.keys().next().unwrap().clone();
        answers.waiting.remove(&id);
        answers.answers.insert(id, Answer::Decline);
        rulesync_shared_answers::save(scratch.path(), &answers).unwrap();
        let changed = edits(&before, &after).unwrap();
        assert!(matches!(
            review_edits(
                &settings,
                &accessible,
                changed,
                &mut receipts,
                file,
                scratch.path(),
                scratch.path()
            )
            .unwrap(),
            SharedReview::Decided { rejected: true }
        ));
        assert!(receipts.is_empty());
        assert!(rulesync_team::local_proposals(&settings)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn rulesync_waiting_edit_survives_a_new_team_revision() {
        let scratch = tempfile::tempdir().unwrap();
        let (settings, _, _) = hosted_fixture(scratch.path());
        let layer = rulesync_team::load_layers(&settings).unwrap().remove(0);
        let before = render_fixture_section(&layer);
        let after = before.replace(APPROVED, "Use the revised checklist.\n");
        let file = Path::new("AGENTS.md");
        std::fs::write(scratch.path().join(file), &after).unwrap();
        let mut receipts = ProposalReceipts::new();
        let accessible = BTreeMap::from([(key(&layer).unwrap(), &layer)]);
        review_edits(
            &settings,
            &accessible,
            edits(&before, &after).unwrap(),
            &mut receipts,
            file,
            scratch.path(),
            scratch.path(),
        )
        .unwrap();
        assert_eq!(
            rulesync_shared_answers::read(scratch.path())
                .unwrap()
                .waiting
                .len(),
            1
        );
        let mut newer = layer.clone();
        newer.revision = "a".repeat(64);
        let accessible = BTreeMap::from([(key(&newer).unwrap(), &newer)]);
        assert!(matches!(
            review_edits(
                &settings,
                &accessible,
                edits(&before, &after).unwrap(),
                &mut receipts,
                file,
                scratch.path(),
                scratch.path()
            )
            .unwrap(),
            SharedReview::NeedsConfirm
        ));
        let answers = rulesync_shared_answers::read(scratch.path()).unwrap();
        assert_eq!(answers.waiting.len(), 1);
        assert_eq!(
            answers.waiting.values().next().unwrap().revision,
            layer.revision
        );
    }

    #[test]
    fn rulesync_shared_edit_list_prunes_a_reverted_file() {
        let scratch = tempfile::tempdir().unwrap();
        let (settings, _, _) = hosted_fixture(scratch.path());
        let layer = rulesync_team::load_layers(&settings).unwrap().remove(0);
        let before = render_fixture_section(&layer);
        let after = before.replace(APPROVED, "Use the revised checklist.\n");
        let file = Path::new("AGENTS.md");
        std::fs::write(scratch.path().join(file), &after).unwrap();
        let accessible = BTreeMap::from([(key(&layer).unwrap(), &layer)]);
        review_edits(
            &settings,
            &accessible,
            edits(&before, &after).unwrap(),
            &mut ProposalReceipts::new(),
            file,
            scratch.path(),
            scratch.path(),
        )
        .unwrap();
        assert_eq!(
            refresh_answers(&settings, scratch.path(), scratch.path())
                .unwrap()
                .waiting
                .len(),
            1
        );
        std::fs::write(scratch.path().join(file), &before).unwrap();
        assert!(refresh_answers(&settings, scratch.path(), scratch.path())
            .unwrap()
            .waiting
            .is_empty());
    }
}
