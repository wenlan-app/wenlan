// SPDX-License-Identifier: Apache-2.0
//! Native files scoped to a checkout that belongs to the member's team.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{bail, ensure, Context, Result};
use wenlan_core::rulesync_team::TeamRuleLayer;
use wenlan_types::rulesync::{RulesSetupCode as Code, RulesSyncReport};

use crate::{
    rulesync_apply::{ApplyOutcome, FileRoot, Snapshot},
    rulesync_layers::{self, ProposalReceipts, SharedReview},
    rulesync_routes::{text, with_bytes, Setup},
    rulesync_shared_answers,
};

const START: &str = "<!-- WENLAN CHECKOUT RULES -->\n";
const END: &str = "<!-- END WENLAN CHECKOUT RULES -->\n";
const CODEX_HEADER: &str =
    "# Written by Wenlan. Team rules for this repository; Wenlan rewrites this file.\n";
const EXCLUDE_START: &str = "# Wenlan local rule files\n";
const EXCLUDE_END: &str = "# End Wenlan local rule files\n";

fn command(repo: &Path, args: &[&str]) -> Result<std::process::Output> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()?;
    Ok(output)
}

fn managed_span(source: &str) -> Result<Option<std::ops::Range<usize>>> {
    let crlf_start = START.replace('\n', "\r\n");
    let crlf_end = END.replace('\n', "\r\n");
    let (start_marker, end_marker) = if source.contains(&crlf_start) {
        (crlf_start.as_str(), crlf_end.as_str())
    } else {
        (START, END)
    };
    let Some(start) = source.find(start_marker) else {
        ensure!(
            !source.contains("<!-- WENLAN CHECKOUT RULES")
                && !source.contains(END)
                && !source.contains(&crlf_end),
            "checkout rule markers changed"
        );
        return Ok(None);
    };
    ensure!(
        source.matches(start_marker).count() == 1,
        "checkout rule markers changed"
    );
    let end = source[start + start_marker.len()..]
        .find(end_marker)
        .map(|offset| start + start_marker.len() + offset + end_marker.len())
        .context("checkout rule markers changed")?;
    ensure!(
        !source[end..].contains(end_marker),
        "checkout rule markers changed"
    );
    Ok(Some(start..end))
}

fn local_text(teams: &[TeamRuleLayer], project: &[TeamRuleLayer]) -> Result<String> {
    let mut text = String::new();
    for layer in teams {
        text.push_str(&rulesync_layers::format_layer("Team", layer)?);
    }
    for layer in project {
        text.push_str(&rulesync_layers::format_layer("Project", layer)?);
    }
    Ok(text)
}

fn codex_instructions(source: &str) -> Result<String> {
    let value: toml::Value = toml::from_str(source)?;
    Ok(value
        .get("developer_instructions")
        .and_then(toml::Value::as_str)
        .context("Codex checkout instructions changed")?
        .to_owned())
}

// Use one TOML basic-string form so the managed value always has a known span.
fn codex_string(value: &str) -> String {
    let mut out = String::from("\"");
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn codex_unmanaged_changed(current: &str, recorded: &str) -> bool {
    let outside = |source: &str| -> Option<String> {
        let range = codex_value_range(source).ok()?;
        let mut outside = source.to_owned();
        outside.replace_range(range, "<Wenlan instructions>");
        Some(outside)
    };
    outside(current).is_none_or(|value| Some(value) != outside(recorded))
}

fn codex_value_range(source: &str) -> Result<std::ops::Range<usize>> {
    let original: toml::Value = toml::from_str(source)?;
    original
        .get("developer_instructions")
        .and_then(toml::Value::as_str)
        .context("Codex checkout instructions changed")?;
    // The key text can also appear inside a rule, so try each place it starts
    // a line and keep the one that proves to be the real top-level key.
    let prefix = "developer_instructions = ";
    let mut found = None;
    for (at, _) in source.match_indices(prefix) {
        if at != 0 && source.as_bytes()[at - 1] != b'\n' {
            continue;
        }
        if let Ok(range) = codex_value_range_at(source, &original, at + prefix.len()) {
            ensure!(
                found.is_none(),
                "Codex instruction key appears more than once"
            );
            found = Some(range);
        }
    }
    found.context("Codex instruction key changed")
}

fn codex_value_range_at(
    source: &str,
    original: &toml::Value,
    start: usize,
) -> Result<std::ops::Range<usize>> {
    let rest = &source[start..];
    let end = if let Some(after_open) = rest.strip_prefix("\"\"\"") {
        start
            + 3
            + after_open
                .find("\n\"\"\"")
                .context("Codex instruction format changed")?
            + 4
    } else {
        ensure!(rest.starts_with('"'), "Codex instruction format changed");
        let mut escaped = false;
        let mut end = None;
        for (offset, byte) in rest.bytes().enumerate().skip(1) {
            if escaped {
                escaped = false;
                continue;
            }
            if byte == b'\\' {
                escaped = true;
                continue;
            }
            if byte == b'"' {
                end = Some(start + offset + 1);
                break;
            }
        }
        end.context("Codex instruction format changed")?
    };
    // Prove the range is the real top-level key: emptying it must change
    // nothing else in the parsed file.
    let mut changed_source = source.to_owned();
    changed_source.replace_range(start..end, "\"\"");
    let changed: toml::Value = toml::from_str(&changed_source)
        .context("Codex instruction range is not the managed key")?;
    let mut expected = original.clone();
    expected
        .as_table_mut()
        .context("Codex config is not a table")?
        .insert(
            "developer_instructions".into(),
            toml::Value::String(String::new()),
        );
    ensure!(
        changed == expected,
        "Codex instruction range is not the managed key"
    );
    Ok(start..end)
}

fn remove_codex_instructions(source: &str) -> Result<String> {
    let range = codex_value_range(source)?;
    let start = range.start - "developer_instructions = ".len();
    let end = range.end;
    let mut next = source.to_owned();
    next.replace_range(
        start..end + usize::from(source.as_bytes().get(end) == Some(&b'\n')),
        "",
    );
    Ok(next)
}

fn replace_codex_instructions(source: &str, replacement: &str) -> Result<String> {
    let range = codex_value_range(source)?;
    let mut next = source.to_owned();
    next.replace_range(range, &codex_string(replacement));
    Ok(next)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn prune_inaccessible(
    setup: &Setup,
    repo: &Path,
    key: &str,
    authorized: &BTreeMap<String, &TeamRuleLayer>,
    claude_record: &mut Option<String>,
    codex_record: &mut Option<String>,
    receipts: &mut BTreeMap<PathBuf, ProposalReceipts>,
    result: &mut RulesSyncReport,
) -> Result<()> {
    let keys = authorized
        .keys()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    let root = FileRoot::open(repo)?;
    for name in ["CLAUDE.local.md", ".codex/config.toml"] {
        let record = if name == "CLAUDE.local.md" {
            &mut *claude_record
        } else {
            &mut *codex_record
        };
        let Some(prior) = record.as_deref() else {
            continue;
        };
        let old_rules = if name == "CLAUDE.local.md" {
            prior.to_owned()
        } else {
            codex_instructions(prior)?
        };
        let old_blocks = rulesync_layers::blocks(&old_rules)?;
        if old_blocks.keys().all(|key| keys.contains(*key)) {
            continue;
        }
        if tracked(repo, name)? {
            result.block(Code::CheckoutNeedsReview, format!("The revoked team rules are still in {}/{} because Git tracks that file; untrack it and sync again.", repo.display(), name));
            continue;
        }
        let path = Path::new(name);
        let current = match root.read(path, false) {
            Ok(value) => value,
            Err(_) => {
                result.block(Code::CheckoutNeedsReview, format!("The revoked team rules are still in {}/{} because it could not be read; repair that file and sync again.", repo.display(), name));
                continue;
            }
        };
        let current_text = match text(&current) {
            Ok(value) => value,
            Err(_) => {
                result.block(Code::CheckoutNeedsReview, format!("The revoked team rules are still in {}/{} because it could not be read as text; repair that file and sync again.", repo.display(), name));
                continue;
            }
        };
        if name == ".codex/config.toml"
            && (!current_text.starts_with(CODEX_HEADER) || tracked(repo, name)?)
        {
            result.block(Code::CheckoutNeedsReview, format!("The revoked team rules are still in {}/{}; remove their developer_instructions after saving your settings.", repo.display(), name));
            continue;
        }
        let current_rules = if name == "CLAUDE.local.md" {
            match managed_span(current_text) {
                Ok(Some(span)) => current_text[span].to_owned(),
                _ => {
                    result.block(Code::CheckoutNeedsReview, format!("The revoked team rules are still in {}/{} because its markers changed; repair that file and sync again.", repo.display(), name));
                    continue;
                }
            }
        } else {
            match codex_instructions(current_text) {
                Ok(value) => value,
                Err(_) => {
                    result.block(Code::CheckoutNeedsReview, format!("The revoked team rules are still in {}/{}; repair the instructions and sync again.", repo.display(), name));
                    continue;
                }
            }
        };
        let current_blocks = match rulesync_layers::blocks(&current_rules) {
            Ok(blocks) => blocks,
            Err(_) => {
                result.block(Code::CheckoutNeedsReview, format!("The revoked team rules are still in {}/{}; repair the section markers and sync again.", repo.display(), name));
                continue;
            }
        };
        let changed_revoked = old_blocks.iter().any(|(key, block)| {
            !keys.contains(*key)
                && current_blocks
                    .get(key)
                    .is_some_and(|current| current.body != block.body)
        });
        let copy = if changed_revoked {
            Some(keep_revoked_copy(setup, key, name, current_text)?)
        } else {
            None
        };
        let kept = rulesync_layers::remove_inaccessible(&current_rules, &keys)?;
        let next = if name == "CLAUDE.local.md" {
            let span = managed_span(current_text)?.context("missing checkout block")?;
            let mut next = current_text.to_owned();
            next.replace_range(span, &kept);
            next
        } else {
            replace_codex_instructions(current_text, &kept)?
        };
        write(&root, path, &current, Some(&next))?;
        let approved = rulesync_layers::remove_inaccessible(&old_rules, &keys)?;
        *record = Some(if name == "CLAUDE.local.md" {
            approved
        } else {
            replace_codex_instructions(prior, &approved)?
        });
        if let Some(receipts) = receipts.get_mut(&alias(key, name)) {
            receipts.retain(|layer_key, _| keys.contains(layer_key));
        }
        if let Some(copy) = copy {
            result.block(
                Code::CheckoutNeedsReview,
                format!(
                    "Revoked team rules were removed from {}/{}; your edited text is in {}.",
                    repo.display(),
                    name,
                    copy.display()
                ),
            );
        }
    }
    let mut answers = rulesync_shared_answers::read(&setup.data)?;
    answers.waiting.retain(|_, edit| {
        if wenlan_core::rulesync_personal::fingerprint(&edit.layer_id)
            .is_ok_and(|layer_key| !keys.contains(&layer_key))
        {
            edit.files.retain(|file| {
                file != &alias(key, "CLAUDE.local.md").display().to_string()
                    && file != &alias(key, ".codex/config.toml").display().to_string()
            });
        }
        !edit.files.is_empty()
    });
    rulesync_shared_answers::save(&setup.data, &answers)?;
    Ok(())
}

fn keep_revoked_copy(setup: &Setup, key: &str, name: &str, content: &str) -> Result<PathBuf> {
    let digest = wenlan_core::rulesync_personal::fingerprint(&(key, name, content))?;
    let safe_name = name.replace('/', "-");
    let relative = PathBuf::from(format!(
        "checkouts/revoked-local-{key}-{safe_name}-{digest}.txt"
    ));
    let root = setup.data_files()?;
    let seen = root.read(&relative, false)?;
    let desired = Snapshot::file(content.as_bytes().to_vec());
    ensure!(
        seen == Snapshot::Absent || seen == desired,
        "revoked rule copy changed"
    );
    if seen == Snapshot::Absent {
        ensure!(
            root.apply(&relative, &seen, &desired, false)? == ApplyOutcome::Applied,
            "revoked rule copy changed during save"
        );
    }
    Ok(setup.data.join(relative))
}

#[allow(clippy::too_many_arguments)]
fn revoke(
    setup: &Setup,
    repo: &Path,
    key: &str,
    claude_record: &mut Option<String>,
    codex_record: &mut Option<String>,
    codex_dir_created: &mut bool,
    receipts: &mut BTreeMap<PathBuf, ProposalReceipts>,
    result: &mut RulesSyncReport,
) -> Result<()> {
    let root = FileRoot::open(repo)?;
    let mut removed = Vec::new();
    let claude_path = Path::new("CLAUDE.local.md");
    let current = match root.read(claude_path, false) {
        Ok(value) => Some(value),
        Err(_) => {
            result.block(Code::CheckoutNeedsReview, format!("The team rules are still in {}/CLAUDE.local.md because it could not be read; repair that file and sync again.", repo.display()));
            None
        }
    };
    if let Some(current) = current {
        if let Snapshot::File { .. } = current {
            let content = match text(&current) {
                Ok(value) => Some(value),
                Err(_) => {
                    result.block(Code::CheckoutNeedsReview, format!("The team rules are still in {}/CLAUDE.local.md because it could not be read as text; repair that file and sync again.", repo.display()));
                    None
                }
            };
            if let Some(content) = content {
                let span = match managed_span(content) {
                    Ok(span) => span,
                    Err(_) => {
                        result.block(Code::CheckoutNeedsReview, format!("The team rules are still in {}/CLAUDE.local.md because its markers changed; repair or remove the Wenlan block.", repo.display()));
                        None
                    }
                };
                if let Some(span) = span {
                    if tracked(repo, "CLAUDE.local.md")? {
                        result.block(Code::CheckoutNeedsReview, format!("The team rules are still in {}/CLAUDE.local.md because Git tracks that file; untrack it and sync again.", repo.display()));
                    } else if let Some(prior) = claude_record.as_deref() {
                        if &content[span.clone()] != prior {
                            let copy = keep_revoked_copy(setup, key, "CLAUDE.local.md", content)?;
                            removed.push(copy);
                        }
                        let mut next = content.to_owned();
                        next.replace_range(span, "");
                        write(
                            &root,
                            claude_path,
                            &current,
                            (!next.is_empty()).then_some(&next),
                        )?;
                        *claude_record = None;
                        receipts.remove(&alias(key, "CLAUDE.local.md"));
                    }
                }
            }
        } else if current == Snapshot::Absent {
            *claude_record = None;
        }
    }
    let codex_path = Path::new(".codex/config.toml");
    let current = root.read(codex_path, false)?;
    if let Snapshot::File { .. } = current {
        let content = text(&current)?;
        if let Some(prior) = codex_record.as_deref() {
            if content.starts_with(CODEX_HEADER) && !tracked(repo, ".codex/config.toml")? {
                if let Ok(next) = remove_codex_instructions(content) {
                    if codex_instructions(content)? != codex_instructions(prior)? {
                        let copy = keep_revoked_copy(setup, key, ".codex/config.toml", content)?;
                        removed.push(copy);
                    }
                    write(
                        &root,
                        codex_path,
                        &current,
                        (next != CODEX_HEADER && !next.is_empty()).then_some(next.as_str()),
                    )?;
                    *codex_record = None;
                    receipts.remove(&alias(key, ".codex/config.toml"));
                    let directory = repo.join(".codex");
                    if *codex_dir_created && fs::read_dir(&directory)?.next().is_none() {
                        fs::remove_dir(directory)?;
                        *codex_dir_created = false;
                    }
                } else {
                    result.block(Code::CheckoutNeedsReview, format!("The team rules are still in {}/.codex/config.toml because its instructions could not be parsed; remove the developer_instructions key after saving your settings.", repo.display()));
                }
            } else {
                result.block(Code::CheckoutNeedsReview, format!("The team rules are still in {}/.codex/config.toml because it is tracked or no longer Wenlan's file; remove the developer_instructions key after saving your settings.", repo.display()));
            }
        }
    } else if current == Snapshot::Absent {
        *codex_record = None;
    }
    let mut answers = rulesync_shared_answers::read(&setup.data)?;
    answers.waiting.retain(|_, edit| {
        edit.files.retain(|file| {
            file != &alias(key, "CLAUDE.local.md").display().to_string()
                && file != &alias(key, ".codex/config.toml").display().to_string()
        });
        !edit.files.is_empty()
    });
    rulesync_shared_answers::save(&setup.data, &answers)?;
    if !removed.is_empty() {
        result.block(Code::CheckoutNeedsReview, format!("Team rules were removed from {} because access ended. A copy of your edits is in {}.", repo.display(), removed.iter().map(|path| path.display().to_string()).collect::<Vec<_>>().join(" and ")));
    }
    update_exclude(repo, claude_record.is_some(), codex_record.is_some())?;
    Ok(())
}

/// Carry a pending edit onto the current approved rendering, including newly
/// approved sections in the same file. The approved rendering stays the
/// baseline; the person's changed body remains a waiting edit.
#[allow(clippy::too_many_arguments)]
fn rebase_waiting_local(
    setup: &Setup,
    root: &FileRoot,
    codex_home: &Path,
    repo: &Path,
    key: &str,
    expected: &str,
    authorized: &BTreeMap<String, &TeamRuleLayer>,
    claude_record: &mut Option<String>,
    codex_record: &mut Option<String>,
    claude_safe: bool,
    codex_safe: bool,
) -> Result<()> {
    let mut answers = rulesync_shared_answers::read(&setup.data)?;
    for (id, edit) in answers.waiting.clone() {
        let Some(layer) = authorized.values().find(|layer| layer.id == edit.layer_id) else {
            continue;
        };
        let block_key = rulesync_layers::key(layer)?;
        let mut updates = Vec::new();
        for (name, baseline) in [
            ("CLAUDE.local.md", claude_record.as_deref()),
            (".codex/config.toml", codex_record.as_deref()),
        ] {
            if (name == "CLAUDE.local.md" && !claude_safe)
                || (name == ".codex/config.toml" && !codex_safe)
            {
                continue;
            }
            let file = alias(key, name).display().to_string();
            if !edit.files.contains(&file)
                || !rulesync_layers::waiting_in_file(
                    &edit,
                    &file,
                    Path::new(""),
                    &setup.data,
                    &setup.home,
                )
            {
                continue;
            }
            let Some(baseline) = baseline else { continue };
            if name == ".codex/config.toml"
                && (!codex_trusted(codex_home, repo)? || tracked(repo, name)?)
            {
                continue;
            }
            let path = Path::new(name);
            let Ok(current) = root.read(path, false) else {
                continue;
            };
            let Ok(local) = text(&current) else { continue };
            if name == ".codex/config.toml" && codex_unmanaged_changed(local, baseline) {
                continue;
            }
            if name == "CLAUDE.local.md" {
                // Merge in LF, then write back with the file's own line endings.
                let newline = if local.contains("\r\n") { "\r\n" } else { "\n" };
                let (baseline, local) =
                    (baseline.replace("\r\n", "\n"), local.replace("\r\n", "\n"));
                let (Ok(Some(old_span)), Ok(Some(local_span))) =
                    (managed_span(&baseline), managed_span(&local))
                else {
                    continue;
                };
                let approved = format!("{START}{expected}{END}");
                if baseline[old_span.clone()] == approved {
                    continue;
                }
                let Ok((rebased, new_id, markdown)) = rebase_rules(
                    &baseline[old_span],
                    &local[local_span.clone()],
                    &approved,
                    layer,
                    &block_key,
                ) else {
                    continue;
                };
                let mut next = local;
                next.replace_range(local_span, &rebased);
                updates.push((
                    name,
                    current,
                    next.replace('\n', newline),
                    approved.replace('\n', newline),
                    new_id,
                    markdown,
                ));
                continue;
            }
            // Owned strings are needed because TOML unescapes the value.
            let old = codex_instructions(baseline)?;
            let mine = codex_instructions(local)?;
            if old == expected {
                continue;
            }
            let Ok((rebased, new_id, markdown)) =
                rebase_rules(&old, &mine, expected, layer, &block_key)
            else {
                continue;
            };
            let next = format!(
                "{CODEX_HEADER}developer_instructions = {}\n",
                codex_string(&rebased)
            );
            let approved = format!(
                "{CODEX_HEADER}developer_instructions = {}\n",
                codex_string(expected)
            );
            updates.push((name, current, next, approved, new_id, markdown));
        }
        if updates.is_empty() {
            continue;
        }
        let (new_id, markdown) = (updates[0].4.clone(), updates[0].5.clone());
        ensure!(
            updates
                .iter()
                .all(|update| update.4 == new_id && update.5 == markdown),
            "checkout rule edits disagree across tools"
        );
        let updated_files = updates
            .iter()
            .map(|update| alias(key, update.0).display().to_string())
            .collect::<Vec<_>>();
        for (name, current, next, approved, _, _) in updates {
            write(root, Path::new(name), &current, Some(&next))?;
            if name == "CLAUDE.local.md" {
                *claude_record = Some(approved);
            } else {
                *codex_record = Some(approved);
            }
        }
        answers.waiting.remove(&id);
        let mut remaining = edit.clone();
        remaining.files.retain(|file| !updated_files.contains(file));
        if !remaining.files.is_empty() {
            answers.waiting.insert(id, remaining);
        }
        let mut moved = edit;
        moved.files = updated_files;
        moved.revision = layer.revision.clone();
        moved.markdown = markdown;
        if let Some(existing) = answers.waiting.get_mut(&new_id) {
            for file in moved.files {
                if !existing.files.contains(&file) {
                    existing.files.push(file);
                }
            }
        } else {
            answers.waiting.insert(new_id, moved);
        }
        rulesync_shared_answers::save(&setup.data, &answers)?;
    }
    Ok(())
}

fn rebase_rules(
    old: &str,
    mine: &str,
    approved: &str,
    layer: &TeamRuleLayer,
    key: &str,
) -> Result<(String, String, String)> {
    ensure!(
        rulesync_layers::only_personal(old)? == rulesync_layers::only_personal(mine)?,
        "checkout rule markers changed"
    );
    ensure!(
        rulesync_layers::edits(old, mine)?
            .iter()
            .all(|(changed, _)| *changed == key),
        "another shared section changed while this edit is waiting"
    );
    let old_blocks = rulesync_layers::blocks(old)?;
    let mine_blocks = rulesync_layers::blocks(mine)?;
    let approved_blocks = rulesync_layers::blocks(approved)?;
    let (base, changed, fresh) = (
        old_blocks
            .get(key)
            .context("old shared section is missing")?,
        mine_blocks
            .get(key)
            .context("edited shared section is missing")?,
        approved_blocks
            .get(key)
            .context("new shared section is missing")?,
    );
    let merged =
        wenlan_core::rulesync_personal::merge_rule_text(base.body, changed.body, fresh.body)
            .context(wenlan_types::rulesync::SHARED_EDIT_STALE_REVISION_MESSAGE)?;
    let rebased = rulesync_layers::replace_block_body(approved, key, &merged)?;
    let blocks = rulesync_layers::blocks(&rebased)?;
    let changed = blocks.get(key).context("rebased section is missing")?;
    let markdown = rulesync_layers::proposed_markdown(layer, changed)?;
    let id =
        wenlan_core::rulesync_personal::fingerprint(&(&layer.id, changed.revision, changed.body))?;
    Ok((rebased, id, markdown))
}

fn write(root: &FileRoot, path: &Path, current: &Snapshot, next: Option<&str>) -> Result<bool> {
    let desired = next.map_or(Snapshot::Absent, |value| {
        with_bytes(current, value.as_bytes().to_vec())
    });
    if current == &desired {
        return Ok(false);
    }
    ensure!(
        root.apply(path, current, &desired, false)? == ApplyOutcome::Applied,
        "checkout file changed during sync"
    );
    Ok(true)
}

fn alias(key: &str, file: &str) -> PathBuf {
    PathBuf::from("checkouts")
        .join(key)
        .join("local")
        .join(file)
}

#[allow(clippy::too_many_arguments)]
fn review(
    settings: &Path,
    setup: &Setup,
    key: &str,
    file: &str,
    before: &str,
    after: &str,
    authorized: &BTreeMap<String, &TeamRuleLayer>,
    receipts: &mut BTreeMap<PathBuf, ProposalReceipts>,
) -> Result<SharedReview> {
    let edits = rulesync_layers::edits(before, after)?;
    ensure!(
        !edits.is_empty()
            && rulesync_layers::only_personal(before)? == rulesync_layers::only_personal(after)?,
        "checkout rule markers changed"
    );
    let path = alias(key, file);
    rulesync_layers::review_edits(
        settings,
        authorized,
        edits,
        receipts.entry(path.clone()).or_default(),
        &path,
        &setup.data,
        &setup.home,
    )
}

fn git_common_dir(repo: &Path) -> Result<PathBuf> {
    let out = command(repo, &["rev-parse", "--git-common-dir"])?;
    ensure!(out.status.success(), "cannot find Git common directory");
    let value = String::from_utf8(out.stdout)?;
    let path = Path::new(value.trim());
    Ok(if path.is_absolute() {
        path.to_path_buf()
    } else {
        repo.join(path)
    })
}

fn excluded(repo: &Path, path: &str) -> Result<bool> {
    let out = command(repo, &["check-ignore", "-q", path])?;
    Ok(out.status.success())
}

fn tracked(repo: &Path, path: &str) -> Result<bool> {
    let out = command(repo, &["ls-files", "--error-unmatch", "--", path])?;
    Ok(out.status.success())
}

fn safe_local_write(repo: &Path, path: &str, result: &mut RulesSyncReport) -> Result<bool> {
    let reason = if tracked(repo, path)? {
        Some("Git tracks that file in this repository; untrack it to get them")
    } else if !excluded(repo, path)? {
        Some("Git would not ignore that file; fix its ignore rules to get them")
    } else {
        None
    };
    if let Some(reason) = reason {
        result.block(
            Code::CheckoutNeedsReview,
            format!("Wenlan didn't put team rules in {path} because {reason}."),
        );
    }
    Ok(reason.is_none())
}

fn update_exclude(repo: &Path, claude: bool, codex: bool) -> Result<()> {
    let root = FileRoot::open(&git_common_dir(repo)?)?;
    let path = Path::new("info/exclude");
    let seen = root.read(path, false)?;
    let original = match &seen {
        Snapshot::File { .. } => text(&seen)?.to_owned(),
        Snapshot::Absent => String::new(),
        _ => bail!("Git exclude is not a regular file"),
    };
    let mut base = original.clone();
    let mut owned_before = String::new();
    if let Some(start) = base.find(EXCLUDE_START) {
        let end = base[start..]
            .find(EXCLUDE_END)
            .map(|n| start + n + EXCLUDE_END.len())
            .context("Git exclude Wenlan markers changed")?;
        owned_before = base[start..end].to_owned();
        base.replace_range(start..end, "");
    }
    let mut entries = Vec::new();
    for (enabled, name) in [(claude, "CLAUDE.local.md"), (codex, ".codex/config.toml")] {
        if owned_before.contains(&format!("\n{name}\n")) || (enabled && !excluded(repo, name)?) {
            entries.push(name);
        }
    }
    if !entries.is_empty() {
        if !base.is_empty() && !base.ends_with('\n') {
            base.push('\n');
        }
        base.push_str(EXCLUDE_START);
        for entry in entries {
            base.push_str(entry);
            base.push('\n');
        }
        base.push_str(EXCLUDE_END);
    }
    if base != original {
        ensure!(
            root.apply(path, &seen, &with_bytes(&seen, base.into_bytes()), false)?
                == ApplyOutcome::Applied,
            "Git exclude changed during sync"
        );
    }
    Ok(())
}

/// Codex's folder under the tool home, the same place rule sync writes
/// `.codex/AGENTS.md`. Reading `CODEX_HOME` here would let a test or a
/// differently configured process read another Codex config.
pub(super) fn codex_home(home: &Path) -> PathBuf {
    home.join(".codex")
}

/// The person's Codex config, or `None` when they have none.
fn codex_config(codex_home: &Path) -> Result<Option<toml::Value>> {
    let source = match fs::read_to_string(codex_home.join("config.toml")) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    Ok(Some(
        toml::from_str(&source).context("invalid Codex config")?,
    ))
}

fn codex_trusted(codex_home: &Path, repo: &Path) -> Result<bool> {
    let Some(parsed) = codex_config(codex_home)? else {
        return Ok(false);
    };
    let canonical = repo.canonicalize()?;
    let common = command(repo, &["rev-parse", "--git-common-dir"])?;
    let common = String::from_utf8(common.stdout)?;
    let common = PathBuf::from(common.trim());
    let common = if common.is_absolute() {
        common
    } else {
        repo.join(common)
    };
    let main_checkout = common.parent().unwrap_or(&common).canonicalize().ok();
    Ok(config_trusts_path(
        &parsed,
        repo,
        &canonical,
        main_checkout.as_deref(),
    ))
}

fn config_trusts_path(
    parsed: &toml::Value,
    repo: &Path,
    canonical: &Path,
    main_checkout: Option<&Path>,
) -> bool {
    parsed
        .get("projects")
        .and_then(toml::Value::as_table)
        .is_some_and(|projects| {
            projects.iter().any(|(path, value)| {
                value.get("trust_level").and_then(toml::Value::as_str) == Some("trusted")
                    && (Path::new(path) == repo
                        || Path::new(path).canonicalize().ok().as_deref() == Some(canonical)
                        || main_checkout.is_some_and(|main| {
                            Path::new(path).canonicalize().ok().as_deref() == Some(main)
                        }))
            })
        })
}

fn codex_has_global_instructions(codex_home: &Path) -> Result<bool> {
    Ok(codex_config(codex_home)?
        .is_some_and(|parsed| parsed.get("developer_instructions").is_some()))
}

#[cfg(test)]
pub(crate) fn trust_config(path: &str) -> String {
    let mut project = toml::map::Map::new();
    project.insert("trust_level".into(), toml::Value::String("trusted".into()));
    let mut projects = toml::map::Map::new();
    projects.insert(path.into(), toml::Value::Table(project));
    let mut root = toml::map::Map::new();
    root.insert("projects".into(), toml::Value::Table(projects));
    toml::to_string(&toml::Value::Table(root)).unwrap()
}

#[allow(clippy::too_many_arguments)]
pub(super) fn sync(
    setup: &Setup,
    codex_home: &Path,
    settings: &Path,
    repo: &Path,
    project: &[TeamRuleLayer],
    teams: &[TeamRuleLayer],
    authorized: &BTreeMap<String, &TeamRuleLayer>,
    claude_record: &mut Option<String>,
    codex_record: &mut Option<String>,
    codex_dir_created: &mut bool,
    receipts: &mut BTreeMap<PathBuf, ProposalReceipts>,
    result: &mut RulesSyncReport,
    person: bool,
    key: &str,
) -> Result<()> {
    let root = FileRoot::open(repo)?;
    let expected = local_text(teams, project)?;
    let wanted = !project.is_empty();
    if !wanted {
        return revoke(
            setup,
            repo,
            key,
            claude_record,
            codex_record,
            codex_dir_created,
            receipts,
            result,
        );
    }
    // A broken personal Codex config must not keep the team rules out of
    // CLAUDE.local.md; this sync then leaves .codex/config.toml as it is.
    let codex_readable = codex_config(codex_home).is_ok();
    let trusted = codex_readable && codex_trusted(codex_home, repo)?;
    // Hide .codex/config.toml from Git only when Wenlan may write it: a
    // person's own untracked file must stay visible to `git status`.
    let codex_ours = match fs::read_to_string(repo.join(".codex/config.toml")) {
        Ok(existing) => existing.starts_with(CODEX_HEADER),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(_) => false,
    };
    update_exclude(repo, true, trusted && codex_ours)?;
    let claude_safe = safe_local_write(repo, "CLAUDE.local.md", result)?;
    // A file that isn't Wenlan's is never written; the check below says so.
    let codex_safe = codex_readable
        && (!trusted || !codex_ours || safe_local_write(repo, ".codex/config.toml", result)?);
    if person && wanted {
        rebase_waiting_local(
            setup,
            &root,
            codex_home,
            repo,
            key,
            &expected,
            authorized,
            claude_record,
            codex_record,
            claude_safe,
            codex_safe,
        )?;
    }
    if claude_safe {
        let claude_path = Path::new("CLAUDE.local.md");
        let current = root.read(claude_path, false)?;
        let content = match &current {
            Snapshot::File { .. } => Some(text(&current)?),
            Snapshot::Absent => None,
            _ => bail!("CLAUDE.local.md is not a regular file"),
        };
        let prior = claude_record.as_deref();
        let span = content.map(managed_span).transpose().map(Option::flatten);
        match span {
        Err(_) => result.block(Code::CheckoutNeedsReview, format!("Your edits to the team rules in {}/CLAUDE.local.md were kept; run `wenlan rules shared-edits` to send or discard them.", repo.display())),
        Ok(span) => {
            let existing = content.and_then(|content| span.as_ref().map(|range| &content[range.clone()]));
            let missing_block = span.is_none() && prior.is_some();
            let changed = existing.map(|text| text.replace("\r\n", "\n"))
                != prior.map(|text| text.replace("\r\n", "\n"));
            let mut replace = !changed || missing_block;
            let mut rejected = false;
            if changed && !missing_block && person && wanted {
                // A CRLF file gets a CRLF block; review the LF text the rules were rendered from.
                let before = prior.unwrap_or_default().replace("\r\n", "\n");
                let after = existing.unwrap_or_default().replace("\r\n", "\n");
                match review(settings, setup, key, "CLAUDE.local.md", &before, &after, authorized, receipts) {
                    Ok(SharedReview::Decided { rejected: true }) => { replace = true; rejected = true; },
                    Ok(SharedReview::Decided { rejected: false }) => replace = true,
                    Ok(_) | Err(_) => (),
                }
            }
            if !replace {
                result.block(Code::SharedRuleNeedsConfirm, format!("Your edits to the team rules in {}/CLAUDE.local.md were kept; run `wenlan rules shared-edits` to send or discard them.", repo.display()));
            } else {
                let mut next = content.unwrap_or_default().to_owned();
                let newline = if next.contains("\r\n") { "\r\n" } else { "\n" };
                let block = wanted.then(|| format!("{START}{expected}{END}").replace('\n', newline));
                if let Some(range) = span {
                    next.replace_range(range, block.as_deref().unwrap_or(""));
                } else if let Some(block) = &block {
                    if !next.is_empty() && !next.ends_with('\n') { next.push_str(newline); }
                    if !next.is_empty() && !next.ends_with(&format!("{newline}{newline}")) { next.push_str(newline); }
                    next.push_str(block);
                }
                let desired = if next.is_empty() { None } else { Some(next.as_str()) };
                if write(&root, claude_path, &current, desired)? { result.written.push(repo.join(claude_path).display().to_string()); }
                *claude_record = block;
                receipts.remove(&alias(key, "CLAUDE.local.md"));
                if missing_block { result.block(Code::CheckoutNeedsReview, format!("Wenlan put the team rules back in {}/CLAUDE.local.md.", repo.display())); }
                if rejected { rulesync_layers::report_rejection(result); }
            }
        }
    }
    }
    let codex_path = Path::new(".codex/config.toml");
    if !codex_readable {
        result.block(
            Code::CheckoutNeedsReview,
            format!(
                "Wenlan couldn't read your Codex settings in ~/.codex/config.toml, so Codex won't get team rules in {} until that file is fixed.",
                repo.file_name().unwrap_or_default().to_string_lossy()
            ),
        );
    }
    if !codex_safe {
        return Ok(());
    }
    if codex_has_global_instructions(codex_home)? {
        result.block(
            Code::CheckoutNeedsReview,
            "Wenlan didn't put team rules in .codex/config.toml because your personal Codex config already sets developer_instructions; remove that setting before syncing repository rules.",
        );
        return Ok(());
    }
    let current = root.read(codex_path, false)?;
    let current_text = match &current {
        Snapshot::File { .. } => Some(text(&current)?),
        Snapshot::Absent => None,
        _ => bail!("Codex checkout config is not a regular file"),
    };
    if wanted && !trusted {
        result.block(
            Code::CheckoutNeedsReview,
            format!(
                "Codex won't get the team rules in {} until you trust this repository in Codex: set [projects.{}] trust_level = \"trusted\" in ~/.codex/config.toml.",
                repo.file_name().unwrap_or_default().to_string_lossy(),
                codex_string(&repo.display().to_string())
            ),
        );
    }
    if current_text.is_some_and(|value| !value.starts_with(CODEX_HEADER))
        || (current_text.is_some() && tracked(repo, ".codex/config.toml")?)
    {
        result.block(
            Code::CheckoutNeedsReview,
            format!(
                "Wenlan didn't change {}/.codex/config.toml because it isn't Wenlan's file.",
                repo.display()
            ),
        );
    } else if current_text
        .zip(codex_record.as_deref())
        .is_some_and(|(current, prior)| codex_unmanaged_changed(current, prior))
    {
        result.block(Code::CheckoutNeedsReview, format!("Wenlan didn't change {}/.codex/config.toml because it has your own settings. Move those settings to your personal Codex config, then sync again.", repo.display()));
    } else {
        let missing_codex = codex_record.is_some() && current_text.is_none() && wanted && trusted;
        let mut held = missing_codex && !person;
        if let Some(after) = current_text {
            if codex_record.as_deref() != Some(after) {
                if let Some(before) = codex_record.as_deref() {
                    let decision = codex_instructions(before).and_then(|before| {
                        let after = codex_instructions(after)?;
                        review(
                            settings,
                            setup,
                            key,
                            ".codex/config.toml",
                            &before,
                            &after,
                            authorized,
                            receipts,
                        )
                    });
                    let decided = matches!(&decision, Ok(SharedReview::Decided { .. }));
                    if person && matches!(&decision, Ok(SharedReview::Decided { rejected: true })) {
                        rulesync_layers::report_rejection(result);
                    }
                    held = !person || !decided;
                } else {
                    held = true;
                }
            }
        }
        if held {
            if codex_record.is_some() && current_text.is_none() {
                result.block(Code::CheckoutNeedsReview, format!("Wenlan will put the team rules back in {}/.codex/config.toml on the next sync.", repo.display()));
            } else {
                result.block(Code::SharedRuleNeedsConfirm, format!("Your edits to the team rules in {}/.codex/config.toml were kept; run `wenlan rules shared-edits` to send or discard them.", repo.display()));
            }
        } else {
            let next = if wanted && trusted {
                Some(format!(
                    "{CODEX_HEADER}developer_instructions = {}\n",
                    codex_string(&expected)
                ))
            } else {
                None
            };
            if next.is_some() && current_text.is_none() && !repo.join(".codex").exists() {
                fs::create_dir_all(repo.join(".codex"))?;
                *codex_dir_created = true;
            }
            if write(&root, codex_path, &current, next.as_deref())? {
                result
                    .written
                    .push(repo.join(codex_path).display().to_string());
                if missing_codex {
                    result.block(
                        Code::CheckoutNeedsReview,
                        format!(
                            "Wenlan put the team rules back in {}/.codex/config.toml.",
                            repo.display()
                        ),
                    );
                }
            }
            *codex_record = next;
            if !wanted || !trusted {
                let directory = repo.join(".codex");
                if *codex_dir_created
                    && directory.is_dir()
                    && fs::read_dir(&directory)?.next().is_none()
                {
                    fs::remove_dir(directory)?;
                    *codex_dir_created = false;
                }
            }
        }
    }
    update_exclude(repo, claude_record.is_some(), codex_record.is_some())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rulesync_routes::report;
    use wenlan_core::rulesync_team;

    #[test]
    #[ignore = "creates persistent scratch repositories for a manual Codex run"]
    fn checkout_codex_files_for_a_real_codex_run() {
        let Some(root) = std::env::var_os("WENLAN_TEST_CODEX_CHECK_DIR").map(PathBuf::from) else {
            return;
        };
        assert!(root.is_absolute());
        fs::create_dir_all(&root).unwrap();
        assert!(
            fs::read_dir(&root).unwrap().next().is_none(),
            "use an empty check directory"
        );
        let codex_home = root.join("codex-home");
        let data = root.join("data");
        fs::create_dir(&codex_home).unwrap();
        fs::create_dir(&data).unwrap();
        let setup = Setup {
            home: codex_home.clone(),
            data: data.clone(),
            binary: PathBuf::from("unused"),
        };
        let remote = "https://github.com/fixture/codex-check.git";
        let repo_id = rulesync_team::repo_identity(remote).unwrap();
        let trusted = root.join("trusted-repo");
        let untrusted = root.join("untrusted-repo");
        for repo in [&trusted, &untrusted] {
            fs::create_dir(repo).unwrap();
            assert!(command(repo, &["init", "--quiet"])
                .unwrap()
                .status
                .success());
            assert!(command(repo, &["remote", "add", "origin", remote])
                .unwrap()
                .status
                .success());
        }
        fs::write(
            codex_home.join("config.toml"),
            trust_config(&trusted.display().to_string()),
        )
        .unwrap();
        let team = TeamRuleLayer {
            id: "team-codex-check".into(),
            name: "Codex check team".into(),
            markdown: "- WENLAN_TEAM_CHECK_TOKEN".into(),
            revision: "a".repeat(64),
            synced_at: "2026-11-26".into(),
            approved_by: None,
            approved_at: None,
            project: None,
            skills: Vec::new(),
            proposal_decisions: BTreeMap::new(),
        };
        let project = TeamRuleLayer {
            id: "project-codex-check".into(),
            name: "Codex check project".into(),
            markdown: "- WENLAN_PROJECT_CHECK_TOKEN".into(),
            revision: "b".repeat(64),
            synced_at: "2026-11-26".into(),
            approved_by: None,
            approved_at: None,
            project: Some(wenlan_types::team::TeamRulesProject {
                space: "Codex check".into(),
                space_id: "codex-check".into(),
                repo_id: Some(repo_id),
            }),
            skills: Vec::new(),
            proposal_decisions: BTreeMap::new(),
        };
        let authorized = [&team, &project]
            .into_iter()
            .map(|layer| (rulesync_layers::key(layer).unwrap(), layer))
            .collect();
        for repo in [&trusted, &untrusted] {
            let mut claude = None;
            let mut codex = None;
            let mut codex_dir_created = false;
            let mut receipts = BTreeMap::new();
            let mut result = report("checkout");
            sync(
                &setup,
                &codex_home,
                &data.join("shared-spaces.json"),
                repo,
                std::slice::from_ref(&project),
                std::slice::from_ref(&team),
                &authorized,
                &mut claude,
                &mut codex,
                &mut codex_dir_created,
                &mut receipts,
                &mut result,
                true,
                &wenlan_core::rulesync_personal::fingerprint(repo).unwrap(),
            )
            .unwrap();
            assert!(repo.join("CLAUDE.local.md").exists());
        }
        let content = fs::read_to_string(trusted.join(".codex/config.toml")).unwrap();
        assert!(content.contains("WENLAN_TEAM_CHECK_TOKEN"));
        assert!(content.contains("WENLAN_PROJECT_CHECK_TOKEN"));
        assert!(!untrusted.join(".codex/config.toml").exists());
    }

    struct Fixture {
        _temp: tempfile::TempDir,
        setup: Setup,
        repo: PathBuf,
        key: String,
        settings: PathBuf,
        writer: wenlan_core::team::Member,
        team: Vec<TeamRuleLayer>,
        project: Vec<TeamRuleLayer>,
        claude: Option<String>,
        codex: Option<String>,
        codex_dir_created: bool,
        receipts: BTreeMap<PathBuf, ProposalReceipts>,
    }

    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let setup = Setup {
                home: temp.path().join("home"),
                data: temp.path().join("data"),
                binary: PathBuf::from("unused"),
            };
            fs::create_dir_all(setup.home.join(".codex")).unwrap();
            fs::create_dir(&setup.data).unwrap();
            let repo = temp.path().join("repo");
            fs::create_dir(&repo).unwrap();
            assert!(command(&repo, &["init", "--quiet"])
                .unwrap()
                .status
                .success());
            assert!(command(
                &repo,
                &[
                    "remote",
                    "add",
                    "origin",
                    crate::rulesync_layers::tests::REMOTE
                ]
            )
            .unwrap()
            .status
            .success());
            let (settings, _, writer) = crate::rulesync_layers::tests::hosted_fixture(&setup.data);
            fs::create_dir(setup.data.join("checkouts")).unwrap();
            let key = wenlan_core::rulesync_personal::fingerprint(&repo).unwrap();
            fs::write(
                setup.data.join(format!("checkouts/{key}.json")),
                serde_json::to_vec(&serde_json::json!({"format": 1, "checkout": {
                    "repo": repo, "files": {}, "skills": {}, "pending": null, "unresolved": []
                }}))
                .unwrap(),
            )
            .unwrap();
            let team = rulesync_team::load_layers(&settings).unwrap();
            let project = rulesync_team::load_project_layers(&settings).unwrap();
            assert!(!team.is_empty() && !project.is_empty());
            Self {
                _temp: temp,
                setup,
                repo,
                key,
                settings,
                writer,
                team,
                project,
                claude: None,
                codex: None,
                codex_dir_created: false,
                receipts: BTreeMap::new(),
            }
        }

        fn trust(&self) {
            fs::write(
                self.setup.home.join(".codex/config.toml"),
                trust_config(&self.repo.display().to_string()),
            )
            .unwrap();
        }

        fn run(&mut self) -> RulesSyncReport {
            self.run_as(true)
        }

        fn run_as(&mut self, person: bool) -> RulesSyncReport {
            let mut result = report("checkout");
            let authorized = self
                .team
                .iter()
                .chain(&self.project)
                .map(|layer| (rulesync_layers::key(layer).unwrap(), layer))
                .collect();
            sync(
                &self.setup,
                &self.setup.home.join(".codex"),
                &self.settings,
                &self.repo,
                &self.project,
                &self.team,
                &authorized,
                &mut self.claude,
                &mut self.codex,
                &mut self.codex_dir_created,
                &mut self.receipts,
                &mut result,
                person,
                &self.key,
            )
            .unwrap();
            result
        }

        fn approve_rules(&mut self, space: &str, markdown: &str) {
            let proposal =
                rulesync_team::update_hosted_rules(&self.settings, space, markdown).unwrap();
            if space == rulesync_team::SPACE {
                rulesync_team::decide_local_proposal(&self.settings, &proposal, true).unwrap();
                self.team = rulesync_team::load_layers(&self.settings).unwrap();
            } else {
                rulesync_team::decide_local_space_proposal(&self.settings, space, &proposal, true)
                    .unwrap();
                self.project = rulesync_team::load_project_layers(&self.settings).unwrap();
            }
        }

        fn decide_edit(&self, id: String) {
            crate::rulesync_routes::decide_shared_edit_at(
                &self.setup,
                &self.settings,
                wenlan_types::rulesync::SharedEditDecisionRequest { id, send: true },
            )
            .unwrap();
        }

        fn pending_team_count(&self) -> usize {
            use wenlan_core::team::{self, CredentialKind};
            rulesync_team::pending_proposals(
                &self.settings.with_file_name(team::TEAM_FILE),
                &self.settings,
                rulesync_team::SPACE,
                &self.writer.id,
                CredentialKind::BrowserSession,
            )
            .unwrap()
            .len()
        }
    }

    #[test]
    fn checkout_local_files_get_team_and_project_rules_in_a_matching_repo() {
        let mut f = Fixture::new();
        f.trust();
        let before = fs::read(f.repo.join(".gitignore")).ok();
        f.run();
        let content = fs::read_to_string(f.repo.join("CLAUDE.local.md")).unwrap();
        assert!(content.find("## Team rules").unwrap() < content.find("## Project rules").unwrap());
        assert!(content.contains("Rules are listed from broadest to most specific"));
        assert_eq!(fs::read(f.repo.join(".gitignore")).ok(), before);
        let exclude =
            fs::read_to_string(git_common_dir(&f.repo).unwrap().join("info/exclude")).unwrap();
        assert!(exclude.contains("CLAUDE.local.md"));
        assert!(exclude.contains(".codex/config.toml"));
        f.run();
        let repeated =
            fs::read_to_string(git_common_dir(&f.repo).unwrap().join("info/exclude")).unwrap();
        assert_eq!(repeated.matches("CLAUDE.local.md").count(), 1);
        assert_eq!(repeated.matches(".codex/config.toml").count(), 1);
        assert!(command(&f.repo, &["status", "--porcelain"])
            .unwrap()
            .stdout
            .is_empty());
    }

    #[test]
    fn checkout_local_files_skip_a_repo_of_no_team_project() {
        let mut f = Fixture::new();
        f.project.clear();
        f.team.clear();
        f.run();
        assert!(!f.repo.join("CLAUDE.local.md").exists());
    }

    #[test]
    fn checkout_codex_config_written_only_when_trusted() {
        let mut f = Fixture::new();
        let report = f.run();
        assert!(report
            .blocked
            .iter()
            .any(|line| line.contains("until you trust")
                && line.contains(&format!(
                    "[projects.{}]",
                    codex_string(&f.repo.display().to_string())
                ))
                && line.contains("~/.codex/config.toml")));
        assert!(!f.repo.join(".codex/config.toml").exists());
        f.trust();
        f.run();
        let content = fs::read_to_string(f.repo.join(".codex/config.toml")).unwrap();
        assert!(content.starts_with(CODEX_HEADER));
        let value: toml::Value = toml::from_str(&content).unwrap();
        assert!(value
            .get("developer_instructions")
            .unwrap()
            .as_str()
            .unwrap()
            .contains("## Team rules"));
        fs::remove_file(f.setup.home.join(".codex/config.toml")).unwrap();
        let report = f.run();
        assert!(report
            .blocked
            .iter()
            .any(|line| line.contains("until you trust")));
        assert!(!f.repo.join(".codex/config.toml").exists());
    }

    #[test]
    fn codex_trust_reader_round_trips_windows_style_project_paths() {
        let path = r"C:\Users\me\repo";
        let encoded = trust_config(path);
        let parsed: toml::Value = toml::from_str(&encoded).unwrap();
        assert_eq!(
            parsed["projects"][path]["trust_level"].as_str(),
            Some("trusted")
        );
        assert!(config_trusts_path(
            &parsed,
            Path::new(path),
            Path::new(path),
            None
        ));
    }

    #[test]
    fn checkout_codex_trust_on_main_checkout_applies_to_linked_worktree() {
        let f = Fixture::new();
        let main = f._temp.path().join("main");
        fs::create_dir(&main).unwrap();
        assert!(command(&main, &["init", "--quiet"])
            .unwrap()
            .status
            .success());
        assert!(command(
            &main,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "--allow-empty",
                "-m",
                "init"
            ]
        )
        .unwrap()
        .status
        .success());
        let linked = f._temp.path().join("linked");
        assert!(command(
            &main,
            &["worktree", "add", "--detach", linked.to_str().unwrap()]
        )
        .unwrap()
        .status
        .success());
        fs::write(
            f.setup.home.join(".codex/config.toml"),
            trust_config(&main.display().to_string()),
        )
        .unwrap();
        assert!(codex_trusted(&f.setup.home.join(".codex"), &linked).unwrap());
    }

    #[test]
    fn checkout_codex_global_instructions_prevent_project_instructions() {
        let mut f = Fixture::new();
        fs::write(
            f.setup.home.join(".codex/config.toml"),
            format!(
                "developer_instructions = 'personal instructions'\n{}",
                trust_config(&f.repo.display().to_string())
            ),
        )
        .unwrap();
        assert!(codex_trusted(&f.setup.home.join(".codex"), &f.repo).unwrap());
        let report = f.run();
        assert!(!f.repo.join(".codex/config.toml").exists());
        assert!(report.blocked.iter().any(|line| {
            line.contains("personal Codex config") && line.contains("developer_instructions")
        }));
    }

    #[test]
    fn rulesync_checkout_codex_quoted_edit_stays_waiting_for_confirmation() {
        let mut f = Fixture::new();
        f.trust();
        f.run();
        let path = f.repo.join(".codex/config.toml");
        let value = codex_instructions(&fs::read_to_string(&path).unwrap())
            .unwrap()
            .replacen(
                "Use the approved checklist.",
                "Use the \"quoted\" checklist.",
                1,
            );
        let edited = format!(
            "{CODEX_HEADER}developer_instructions = {}\n",
            serde_json::to_string(&value).unwrap()
        );
        assert!(edited.contains("\\\"quoted\\\""));
        fs::write(&path, &edited).unwrap();
        f.run();
        let answers = rulesync_shared_answers::read(&f.setup.data).unwrap();
        assert_eq!(answers.waiting.len(), 1);
        let edit = answers.waiting.values().next().unwrap();
        assert_eq!(edit.files.len(), 1);
        assert!(rulesync_layers::waiting_in_file(
            edit,
            &edit.files[0],
            &f.settings,
            &f.setup.data,
            &f.setup.home
        ));
        assert_eq!(fs::read_to_string(path).unwrap(), edited);
    }

    #[test]
    fn checkout_codex_config_refuses_a_file_that_is_not_wenlans() {
        let mut f = Fixture::new();
        f.trust();
        fs::create_dir(f.repo.join(".codex")).unwrap();
        fs::write(
            f.repo.join(".codex/config.toml"),
            "developer_instructions = 'mine'\n",
        )
        .unwrap();
        let report = f.run();
        assert!(report
            .blocked
            .iter()
            .any(|line| line.contains("isn't Wenlan's file")));
        assert_eq!(
            fs::read_to_string(f.repo.join(".codex/config.toml")).unwrap(),
            "developer_instructions = 'mine'\n"
        );
        // The person's own untracked file stays visible to Git.
        assert!(!excluded(&f.repo, ".codex/config.toml").unwrap());
        assert!(excluded(&f.repo, "CLAUDE.local.md").unwrap());
    }

    #[test]
    fn checkout_codex_config_refuses_a_tracked_wenlan_header() {
        let mut f = Fixture::new();
        f.trust();
        fs::create_dir(f.repo.join(".codex")).unwrap();
        let existing = format!("{CODEX_HEADER}developer_instructions = 'keep'\n");
        fs::write(f.repo.join(".codex/config.toml"), &existing).unwrap();
        assert!(command(&f.repo, &["add", ".codex/config.toml"])
            .unwrap()
            .status
            .success());
        let report = f.run();
        assert!(report
            .blocked
            .iter()
            .any(|line| line.contains("Git tracks that file")));
        assert_eq!(
            fs::read_to_string(f.repo.join(".codex/config.toml")).unwrap(),
            existing
        );
    }

    #[test]
    fn checkout_local_tracked_files_are_refused_without_changes() {
        let mut f = Fixture::new();
        f.trust();
        fs::write(f.repo.join("CLAUDE.local.md"), "My tracked Claude text.\n").unwrap();
        fs::create_dir(f.repo.join(".codex")).unwrap();
        fs::write(f.repo.join(".codex/config.toml"), "model = 'mine'\n").unwrap();
        assert!(command(
            &f.repo,
            &["add", "-f", "CLAUDE.local.md", ".codex/config.toml"]
        )
        .unwrap()
        .status
        .success());
        let report = f.run();
        assert_eq!(
            fs::read_to_string(f.repo.join("CLAUDE.local.md")).unwrap(),
            "My tracked Claude text.\n"
        );
        assert_eq!(
            fs::read_to_string(f.repo.join(".codex/config.toml")).unwrap(),
            "model = 'mine'\n"
        );
        assert_eq!(
            report
                .blocked
                .iter()
                .filter(|line| line.contains("Git tracks"))
                .count(),
            1
        );
        // The tracked Codex file isn't Wenlan's, so it is refused as such.
        assert!(report
            .blocked
            .iter()
            .any(|line| line.contains("isn't Wenlan's file")));
    }

    #[test]
    fn checkout_local_negated_ignore_refuses_both_files() {
        let mut f = Fixture::new();
        f.trust();
        fs::write(
            f.repo.join(".gitignore"),
            "!CLAUDE.local.md\n!.codex/config.toml\n",
        )
        .unwrap();
        let report = f.run();
        assert!(!f.repo.join("CLAUDE.local.md").exists());
        assert!(!f.repo.join(".codex/config.toml").exists());
        assert_eq!(
            report
                .blocked
                .iter()
                .filter(|line| line.contains("Git would not ignore"))
                .count(),
            2
        );
    }

    #[test]
    fn checkout_local_written_files_are_not_staged_by_git_add_all() {
        let mut f = Fixture::new();
        f.trust();
        f.run();
        assert!(command(&f.repo, &["add", "-A"]).unwrap().status.success());
        let staged = command(&f.repo, &["diff", "--cached", "--name-only"]).unwrap();
        let staged = String::from_utf8(staged.stdout).unwrap();
        assert!(!staged.contains("CLAUDE.local.md"));
        assert!(!staged.contains(".codex/config.toml"));
    }

    #[test]
    fn checkout_codex_special_rule_text_round_trips_and_revokes() {
        for rule in [
            r"Use \\d+ here.",
            "Use \"\"\" here.",
            "Use ''' here.",
            "Use \\d+, \"\"\", and ''' here.",
            "Put developer_instructions = \"x\" in config.",
            "developer_instructions = \"x\" starts this rule.",
        ] {
            let mut f = Fixture::new();
            f.trust();
            f.approve_rules(rulesync_team::SPACE, rule);
            let first = f.run();
            assert!(
                !first
                    .blocked
                    .iter()
                    .any(|line| line.contains("your own settings")),
                "{first:?}"
            );
            let before = fs::read_to_string(f.repo.join(".codex/config.toml")).unwrap();
            assert!(codex_instructions(&before).unwrap().contains(rule));
            let second = f.run();
            assert!(
                !second
                    .blocked
                    .iter()
                    .any(|line| line.contains("shared-edits")),
                "{second:?}"
            );
            assert_eq!(
                fs::read_to_string(f.repo.join(".codex/config.toml")).unwrap(),
                before
            );
            f.project.clear();
            f.team.clear();
            f.run();
            assert!(!f.repo.join("CLAUDE.local.md").exists());
            assert!(!f.repo.join(".codex/config.toml").exists());
        }
    }

    #[test]
    fn checkout_local_files_keep_the_persons_own_lines() {
        let mut f = Fixture::new();
        fs::write(f.repo.join("CLAUDE.local.md"), "Own first.\nOwn last.\n").unwrap();
        f.run();
        f.run();
        let content = fs::read_to_string(f.repo.join("CLAUDE.local.md")).unwrap();
        assert!(content.starts_with("Own first.\nOwn last.\n\n"));
    }

    #[test]
    fn checkout_local_deleted_claude_rules_are_restored_without_a_shared_edit_notice() {
        for remove_file in [false, true] {
            let mut f = Fixture::new();
            f.run();
            let path = f.repo.join("CLAUDE.local.md");
            if remove_file {
                fs::remove_file(&path).unwrap();
            } else {
                fs::write(&path, "My own line.\n").unwrap();
            }
            let report = f.run();
            assert!(
                report
                    .blocked
                    .iter()
                    .any(|line| line.contains("put the team rules back in")
                        && line.contains("CLAUDE.local.md")),
                "{report:?}"
            );
            assert!(
                !report
                    .blocked
                    .iter()
                    .any(|line| line.contains("shared-edits")),
                "{report:?}"
            );
            let restored = fs::read_to_string(&path).unwrap();
            assert!(restored.contains("Use the approved checklist."));
            if !remove_file {
                assert!(restored.starts_with("My own line.\n"));
            }
        }
    }

    #[test]
    fn checkout_local_deleted_codex_file_is_restored_without_a_shared_edit_notice() {
        let mut f = Fixture::new();
        f.trust();
        f.run();
        let path = f.repo.join(".codex/config.toml");
        fs::remove_file(&path).unwrap();
        let report = f.run();
        assert!(
            report
                .blocked
                .iter()
                .any(|line| line.contains("put the team rules back in")
                    && line.contains(".codex/config.toml")),
            "{report:?}"
        );
        assert!(
            !report
                .blocked
                .iter()
                .any(|line| line.contains("shared-edits")),
            "{report:?}"
        );
        assert!(codex_instructions(&fs::read_to_string(path).unwrap())
            .unwrap()
            .contains("Use the approved checklist."));
    }

    #[test]
    fn checkout_local_crlf_claude_file_keeps_line_endings_and_one_block() {
        let mut f = Fixture::new();
        let path = f.repo.join("CLAUDE.local.md");
        fs::write(&path, "My own line.\r\n").unwrap();
        f.run();
        let first = fs::read_to_string(&path).unwrap();
        assert_eq!(first.matches(START.trim_end()).count(), 1);
        assert!(first.contains("My own line.\r\n"));
        assert!(!first.replace("\r\n", "").contains('\n'));
        let report = f.run();
        assert!(
            !report
                .blocked
                .iter()
                .any(|line| line.contains("shared-edits")),
            "{report:?}"
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), first);
    }

    #[test]
    fn checkout_local_crlf_claude_file_team_rule_edit_can_be_sent() {
        let mut f = Fixture::new();
        let path = f.repo.join("CLAUDE.local.md");
        fs::write(&path, "My own line.\r\n").unwrap();
        f.run();
        let delivered = fs::read_to_string(&path).unwrap();
        assert!(!delivered.replace("\r\n", "").contains('\n'));
        let edited = delivered.replacen(
            "Use the approved checklist.",
            "Use the revised checklist.",
            1,
        );
        fs::write(&path, &edited).unwrap();
        let report = f.run();
        assert!(report
            .blocked
            .iter()
            .any(|line| line.contains("shared-edits")));
        let answers = rulesync_shared_answers::read(&f.setup.data).unwrap();
        assert_eq!(answers.waiting.len(), 1);
        let (id, edit) = answers.waiting.iter().next().unwrap();
        assert_eq!(edit.markdown, "Use the revised checklist.\n");
        assert_eq!(edit.files.len(), 1);
        assert!(rulesync_layers::waiting_in_file(
            edit,
            &edit.files[0],
            &f.settings,
            &f.setup.data,
            &f.setup.home
        ));
        let pending = f.pending_team_count();
        f.decide_edit(id.clone());
        assert_eq!(f.pending_team_count(), pending + 1);
        f.run();
        let answers = rulesync_shared_answers::read(&f.setup.data).unwrap();
        assert!(answers.waiting.is_empty(), "{:?}", answers.waiting);
        assert_eq!(fs::read_to_string(&path).unwrap(), edited);
    }

    #[test]
    fn checkout_local_crlf_claude_edit_moves_to_a_new_team_revision() {
        let mut f = Fixture::new();
        let path = f.repo.join("CLAUDE.local.md");
        fs::write(&path, "My own line.\r\n").unwrap();
        f.run();
        let delivered = fs::read_to_string(&path).unwrap();
        fs::write(
            &path,
            delivered.replacen(
                "Use the approved checklist.",
                "Use the revised checklist.",
                1,
            ),
        )
        .unwrap();
        f.run();
        let old = rulesync_shared_answers::read(&f.setup.data).unwrap();
        let (old_id, old_edit) = old.waiting.iter().next().unwrap();
        f.approve_rules(
            rulesync_team::SPACE,
            "Use the approved checklist.\nNew approved line.\n",
        );
        f.run();
        let content = fs::read_to_string(&path).unwrap();
        assert!(!content.replace("\r\n", "").contains('\n'), "{content:?}");
        assert!(
            content.contains("Use the revised checklist.\r\nNew approved line."),
            "{content:?}"
        );
        let answers = rulesync_shared_answers::read(&f.setup.data).unwrap();
        assert_eq!(answers.waiting.len(), 1, "{:?}", answers.waiting);
        let (id, edit) = answers.waiting.iter().next().unwrap();
        assert_ne!(id, old_id);
        assert_ne!(edit.revision, old_edit.revision);
        assert_eq!(
            edit.markdown,
            "Use the revised checklist.\nNew approved line.\n"
        );
        let (id, revision) = (id.clone(), edit.revision.clone());
        f.run();
        let again = rulesync_shared_answers::read(&f.setup.data).unwrap();
        assert_eq!(again.waiting.len(), 1, "{:?}", again.waiting);
        assert_eq!(again.waiting[&id].revision, revision);
        let pending = f.pending_team_count();
        f.decide_edit(id.clone());
        assert_eq!(f.pending_team_count(), pending + 1);
    }

    #[test]
    fn checkout_malformed_personal_codex_config_still_delivers_claude_rules() {
        let mut f = Fixture::new();
        f.trust();
        f.run();
        let codex = f.repo.join(".codex/config.toml");
        let codex_before = fs::read(&codex).unwrap();
        fs::write(f.setup.home.join(".codex/config.toml"), "[projects\nbroken").unwrap();
        f.approve_rules(
            rulesync_team::SPACE,
            "Use the approved checklist.\nNew approved line.\n",
        );
        let report = f.run();
        assert!(fs::read_to_string(f.repo.join("CLAUDE.local.md"))
            .unwrap()
            .contains("New approved line."));
        let notice = format!(
            "Wenlan couldn't read your Codex settings in ~/.codex/config.toml, so Codex won't get team rules in {} until that file is fixed.",
            f.repo.file_name().unwrap().to_string_lossy()
        );
        assert_eq!(
            report
                .blocked
                .iter()
                .filter(|line| **line == notice)
                .count(),
            1,
            "{report:?}"
        );
        assert_eq!(fs::read(&codex).unwrap(), codex_before);
    }

    #[test]
    fn checkout_local_files_removed_after_access_is_revoked() {
        let mut f = Fixture::new();
        f.trust();
        f.run();
        f.project.clear();
        f.team.clear();
        f.run();
        assert!(!f.repo.join("CLAUDE.local.md").exists());
        assert!(!f.repo.join(".codex/config.toml").exists());
        let exclude =
            fs::read_to_string(git_common_dir(&f.repo).unwrap().join("info/exclude")).unwrap();
        assert!(exclude.contains(EXCLUDE_START));
    }

    #[test]
    fn rulesync_checkout_broken_claude_marker_does_not_block_codex_revocation() {
        let mut f = Fixture::new();
        f.trust();
        f.run();
        let path = f.repo.join("CLAUDE.local.md");
        let broken = fs::read_to_string(&path)
            .unwrap()
            .replace(END, "<!-- broken end -->\n");
        fs::write(&path, &broken).unwrap();
        f.project.clear();
        f.team.clear();
        let report = f.run();
        assert_eq!(fs::read_to_string(&path).unwrap(), broken);
        assert!(!f.repo.join(".codex/config.toml").exists());
        assert!(report
            .blocked
            .iter()
            .any(|line| line.contains("CLAUDE.local.md") && line.contains("still")));
    }

    #[test]
    fn rulesync_checkout_non_utf8_claude_does_not_block_codex_revocation() {
        let mut f = Fixture::new();
        f.trust();
        f.run();
        let bytes = vec![0xff, 0xfe, 0x00];
        fs::write(f.repo.join("CLAUDE.local.md"), &bytes).unwrap();
        f.project.clear();
        f.team.clear();
        let report = f.run();
        assert_eq!(fs::read(f.repo.join("CLAUDE.local.md")).unwrap(), bytes);
        assert!(!f.repo.join(".codex/config.toml").exists());
        assert!(report
            .blocked
            .iter()
            .any(|line| line.contains("CLAUDE.local.md") && line.contains("read as text")));
    }

    #[test]
    fn rulesync_checkout_revocation_saves_hand_edits_and_removes_local_rules() {
        let mut f = Fixture::new();
        fs::write(f.repo.join("CLAUDE.local.md"), "My own line.\n\n").unwrap();
        edit_both(&mut f);
        f.run();
        assert_eq!(pending_id(&f).len(), 64);
        f.project.clear();
        f.team.clear();
        let report = f.run();
        assert!(report
            .blocked
            .iter()
            .any(|line| line.contains("access ended") && line.contains("revoked-local-")));
        assert_eq!(
            fs::read_to_string(f.repo.join("CLAUDE.local.md")).unwrap(),
            "My own line.\n\n"
        );
        assert!(!f.repo.join(".codex/config.toml").exists());
        let copies = fs::read_dir(f.setup.data.join("checkouts"))
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("revoked-local-")
            })
            .collect::<Vec<_>>();
        assert_eq!(copies.len(), 2);
        for copy in copies {
            assert!(fs::read_to_string(copy.path())
                .unwrap()
                .contains("Use the revised checklist."));
        }
        assert!(rulesync_shared_answers::read(&f.setup.data)
            .unwrap()
            .waiting
            .is_empty());
    }

    #[test]
    fn rulesync_checkout_prunes_one_revoked_project_with_another_edit_waiting() {
        let mut f = Fixture::new();
        let mut second = f.project[0].clone();
        second.id = "second-project-layer".into();
        second.markdown = "Other project checklist.\n".into();
        let retained_key = rulesync_layers::key(&second).unwrap();
        let revoked_key = rulesync_layers::key(&f.project[0]).unwrap();
        f.project.push(second);
        edit_both(&mut f);
        f.run();
        let mut sample = f.project[0].clone();
        sample.id = "unrelated-personal-example".into();
        let example = format!(
            "\n```md\n{}```\n",
            rulesync_layers::format_layer("Example", &sample).unwrap()
        );
        let claude_path = f.repo.join("CLAUDE.local.md");
        fs::write(
            &claude_path,
            format!("{}{}", fs::read_to_string(&claude_path).unwrap(), example),
        )
        .unwrap();
        f.project.remove(0);
        let authorized = f
            .team
            .iter()
            .chain(&f.project)
            .map(|layer| (rulesync_layers::key(layer).unwrap(), layer))
            .collect();
        let mut result = report("checkout");
        prune_inaccessible(
            &f.setup,
            &f.repo,
            &f.key,
            &authorized,
            &mut f.claude,
            &mut f.codex,
            &mut f.receipts,
            &mut result,
        )
        .unwrap();
        for name in ["CLAUDE.local.md", ".codex/config.toml"] {
            let content = file_rules(&f.repo, name);
            assert!(!content.contains(&revoked_key));
            assert!(content.contains(&retained_key));
            assert!(content.contains("Use the revised checklist."));
        }
        assert!(fs::read_to_string(claude_path).unwrap().ends_with(&example));
    }

    #[test]
    fn rulesync_checkout_codex_own_settings_survive_a_settled_team_edit() {
        use wenlan_core::team::{self, CredentialKind};
        let mut f = Fixture::new();
        edit_both(&mut f);
        f.run();
        let id = pending_id(&f);
        let path = f.repo.join(".codex/config.toml");
        let own = format!("{}model = \"custom\"\n", fs::read_to_string(&path).unwrap());
        fs::write(&path, &own).unwrap();
        let report = f.run();
        assert!(report
            .blocked
            .iter()
            .any(|line| line.contains("your own settings")));
        assert_eq!(fs::read_to_string(&path).unwrap(), own);
        f.decide_edit(id);
        let team_path = f.settings.with_file_name(team::TEAM_FILE);
        let proposals = rulesync_team::pending_proposals(
            &team_path,
            &f.settings,
            rulesync_team::SPACE,
            &f.writer.id,
            CredentialKind::BrowserSession,
        )
        .unwrap();
        rulesync_team::decide_proposal(
            &team_path,
            &f.settings,
            rulesync_team::SPACE,
            &f.writer.id,
            CredentialKind::BrowserSession,
            &proposals[0].id,
            true,
        )
        .unwrap();
        f.team = rulesync_team::load_layers(&f.settings).unwrap();
        let report = f.run();
        assert!(report
            .blocked
            .iter()
            .any(|line| line.contains("your own settings")));
        assert_eq!(fs::read_to_string(&path).unwrap(), own);
        f.project.clear();
        f.team.clear();
        let report = f.run();
        let after = fs::read_to_string(&path).unwrap();
        assert!(after.contains("model = \"custom\""));
        assert!(!after.contains("developer_instructions"));
        assert!(report
            .blocked
            .iter()
            .any(|line| line.contains("access ended") && line.contains("revoked-local-")));
    }

    #[test]
    fn rulesync_checkout_revocation_finds_the_real_codex_key_and_keeps_own_lines() {
        // A commented copy of the key does not hide the real one: revocation
        // removes only the managed key and keeps the person's own lines.
        let mut f = Fixture::new();
        f.trust();
        f.run();
        let path = f.repo.join(".codex/config.toml");
        let original = fs::read_to_string(&path).unwrap();
        let own = "# developer_instructions = \"\"\"\nmodel = \"custom\"\n# keep this comment\n";
        let altered = original.replacen(
            "developer_instructions = ",
            &format!("{own}developer_instructions = "),
            1,
        );
        assert!(toml::from_str::<toml::Value>(&altered).is_ok());
        fs::write(&path, &altered).unwrap();
        f.project.clear();
        f.team.clear();
        f.run();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            format!("{CODEX_HEADER}{own}")
        );
    }

    #[test]
    fn rulesync_checkout_revocation_refuses_a_key_spelling_hidden_by_personal_text() {
        // The personal string first holds a different quoted value, then an exact
        // copy of the managed value: each must be refused by its own check.
        for exact_copy in [false, true] {
            let mut f = Fixture::new();
            f.trust();
            f.run();
            let path = f.repo.join(".codex/config.toml");
            let original = fs::read_to_string(&path).unwrap();
            let key = "developer_instructions = ";
            let value = &original[codex_value_range(&original).unwrap()];
            let hidden = if exact_copy {
                format!("'''{key}{value}'''")
            } else {
                format!("'{key}\"z\"'")
            };
            let altered = original.replacen(
                key,
                &format!("label = {hidden}\n\"developer_instructions\" = "),
                1,
            );
            assert!(toml::from_str::<toml::Value>(&altered).is_ok());
            fs::write(&path, &altered).unwrap();
            let receipt = f.codex.clone();
            f.project.clear();
            f.team.clear();
            let result = f.run();
            assert_eq!(fs::read_to_string(&path).unwrap(), altered);
            assert_eq!(f.codex, receipt);
            assert!(result
                .blocked
                .iter()
                .any(|line| line.contains("team rules are still")
                    && line.contains("remove the developer_instructions key")));
        }
    }

    #[test]
    fn checkout_local_block_hand_edit_is_kept_and_reported() {
        let mut f = Fixture::new();
        f.run();
        let path = f.repo.join("CLAUDE.local.md");
        let content = fs::read_to_string(&path)
            .unwrap()
            .replace("Use the approved checklist.", "Use my checklist.");
        fs::write(&path, &content).unwrap();
        let report = f.run();
        assert_eq!(fs::read_to_string(path).unwrap(), content);
        assert!(report
            .blocked
            .iter()
            .any(|line| line.contains("shared-edits")));
    }

    #[test]
    fn checkout_local_files_without_a_receipt_are_left_for_review() {
        let mut f = Fixture::new();
        f.trust();
        let rules = local_text(&f.team, &f.project).unwrap();
        let claude = format!("{START}{rules}{END}My later edit.\n");
        fs::write(f.repo.join("CLAUDE.local.md"), &claude).unwrap();
        fs::create_dir(f.repo.join(".codex")).unwrap();
        let codex = format!(
            "{CODEX_HEADER}developer_instructions = {}\n",
            toml::Value::String(rules)
        );
        fs::write(f.repo.join(".codex/config.toml"), &codex).unwrap();
        let report = f.run();
        assert_eq!(
            report
                .blocked
                .iter()
                .filter(|line| line.contains("shared-edits"))
                .count(),
            2
        );
        assert_eq!(
            fs::read_to_string(f.repo.join("CLAUDE.local.md")).unwrap(),
            claude
        );
        assert_eq!(
            fs::read_to_string(f.repo.join(".codex/config.toml")).unwrap(),
            codex
        );
    }

    #[test]
    fn checkout_local_exclude_uses_the_linked_worktree_common_dir() {
        let mut f = Fixture::new();
        assert!(command(
            &f.repo,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "--allow-empty",
                "-m",
                "init"
            ]
        )
        .unwrap()
        .status
        .success());
        let linked = f._temp.path().join("linked");
        assert!(command(
            &f.repo,
            &[
                "worktree",
                "add",
                "--quiet",
                "-b",
                "linked",
                linked.to_str().unwrap()
            ]
        )
        .unwrap()
        .status
        .success());
        f.repo = linked;
        f.trust();
        f.run();
        let common = git_common_dir(&f.repo).unwrap();
        assert!(fs::read_to_string(common.join("info/exclude"))
            .unwrap()
            .contains(".codex/config.toml"));
        let sibling = f._temp.path().join("sibling");
        assert!(command(
            &f.repo,
            &[
                "worktree",
                "add",
                "--quiet",
                "-b",
                "sibling",
                sibling.to_str().unwrap()
            ]
        )
        .unwrap()
        .status
        .success());
        f.repo = sibling;
        f.run();
        let exclude = fs::read_to_string(common.join("info/exclude")).unwrap();
        assert!(exclude.contains("CLAUDE.local.md"));
        assert!(exclude.contains(".codex/config.toml"));
    }

    fn pending_id(f: &Fixture) -> String {
        let answers = crate::rulesync_shared_answers::read(&f.setup.data).unwrap();
        assert_eq!(answers.waiting.len(), 1);
        assert_eq!(answers.waiting.values().next().unwrap().files.len(), 2);
        answers.waiting.keys().next().unwrap().clone()
    }

    fn refuse_sent(f: &Fixture, id: &str) {
        let proposal = rulesync_shared_answers::read(&f.setup.data)
            .unwrap()
            .sent_edits[id]
            .proposal
            .clone();
        crate::rulesync_routes::reopen_edits_after_old_host_refusal_at(
            &f.setup,
            &f.settings,
            &[proposal],
        )
        .unwrap();
    }

    fn edit_both(f: &mut Fixture) -> (String, String) {
        f.trust();
        f.run();
        let claude = f.repo.join("CLAUDE.local.md");
        let codex = f.repo.join(".codex/config.toml");
        let claude_edited = fs::read_to_string(&claude).unwrap().replacen(
            "Use the approved checklist.",
            "Use the revised checklist.",
            1,
        );
        let codex_edited = fs::read_to_string(&codex).unwrap().replacen(
            "Use the approved checklist.",
            "Use the revised checklist.",
            1,
        );
        fs::write(claude, &claude_edited).unwrap();
        fs::write(codex, &codex_edited).unwrap();
        assert!(!codex_unmanaged_changed(
            &codex_edited,
            f.codex.as_deref().unwrap()
        ));
        (claude_edited, codex_edited)
    }

    fn proposal_kind(
        folder: &Path,
        id: &str,
    ) -> Option<wenlan_core::shared_space::version::AuthorKind> {
        for entry in fs::read_dir(folder).ok()?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(kind) = proposal_kind(&path, id) {
                    return Some(kind);
                }
            } else if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(id))
            {
                let bytes = fs::read(&path).ok()?;
                let name = path.file_name()?.to_str()?;
                if let Ok(version) = wenlan_core::shared_space::version::parse(name, &bytes) {
                    return Some(version.meta.author.kind);
                }
            }
        }
        None
    }

    fn file_rules(repo: &Path, name: &str) -> String {
        let content = fs::read_to_string(repo.join(name)).unwrap();
        if name == ".codex/config.toml" {
            codex_instructions(&content).unwrap()
        } else {
            content
        }
    }

    #[test]
    fn rulesync_checkout_team_hand_edit_waits_for_a_write_holder_and_restores_on_rejection() {
        use wenlan_core::team::{self, CredentialKind};
        let mut f = Fixture::new();
        let (claude, codex) = edit_both(&mut f);
        assert!(f
            .run()
            .blocked
            .iter()
            .any(|line| line.contains("shared-edits")));
        assert_eq!(
            fs::read_to_string(f.repo.join("CLAUDE.local.md")).unwrap(),
            claude
        );
        assert_eq!(
            fs::read_to_string(f.repo.join(".codex/config.toml")).unwrap(),
            codex
        );
        let id = pending_id(&f);
        crate::rulesync_routes::decide_shared_edit_at(
            &f.setup,
            &f.settings,
            wenlan_types::rulesync::SharedEditDecisionRequest { id, send: true },
        )
        .unwrap();
        let team_path = f.settings.with_file_name(team::TEAM_FILE);
        let proposals = rulesync_team::pending_proposals(
            &team_path,
            &f.settings,
            rulesync_team::SPACE,
            &f.writer.id,
            CredentialKind::BrowserSession,
        )
        .unwrap();
        assert_eq!(proposals.len(), 1);
        assert_eq!(
            proposal_kind(&f.setup.data, &proposals[0].id),
            Some(wenlan_core::shared_space::version::AuthorKind::Unknown)
        );
        rulesync_team::decide_proposal(
            &team_path,
            &f.settings,
            rulesync_team::SPACE,
            &f.writer.id,
            CredentialKind::BrowserSession,
            &proposals[0].id,
            false,
        )
        .unwrap();
        f.team = rulesync_team::load_layers(&f.settings).unwrap();
        f.run_as(false);
        assert_eq!(
            fs::read_to_string(f.repo.join("CLAUDE.local.md")).unwrap(),
            claude
        );
        assert_eq!(
            fs::read_to_string(f.repo.join(".codex/config.toml")).unwrap(),
            codex
        );
        let report = f.run();
        assert!(report
            .notice_codes
            .contains(&Code::SharedRuleRejected.into()));
        assert!(!fs::read_to_string(f.repo.join("CLAUDE.local.md"))
            .unwrap()
            .contains("revised checklist"));
        assert!(!fs::read_to_string(f.repo.join(".codex/config.toml"))
            .unwrap()
            .contains("revised checklist"));
    }

    #[test]
    fn rulesync_checkout_write_holder_applies_their_team_hand_edit_after_person_and_write_holder_confirm(
    ) {
        use wenlan_core::team::{self, CredentialKind};
        let mut f = Fixture::new();
        edit_both(&mut f);
        f.run();
        let id = pending_id(&f);
        crate::rulesync_routes::decide_shared_edit_at(
            &f.setup,
            &f.settings,
            wenlan_types::rulesync::SharedEditDecisionRequest { id, send: true },
        )
        .unwrap();
        let team_path = f.settings.with_file_name(team::TEAM_FILE);
        let proposals = rulesync_team::pending_proposals(
            &team_path,
            &f.settings,
            rulesync_team::SPACE,
            &f.writer.id,
            CredentialKind::BrowserSession,
        )
        .unwrap();
        assert_eq!(proposals.len(), 1);
        rulesync_team::decide_proposal(
            &team_path,
            &f.settings,
            rulesync_team::SPACE,
            &f.writer.id,
            CredentialKind::BrowserSession,
            &proposals[0].id,
            true,
        )
        .unwrap();
        f.team = rulesync_team::load_layers(&f.settings).unwrap();
        f.approve_rules(
            rulesync_team::SPACE,
            "Use the revised checklist.\nNew approved line.\n",
        );
        let report = f.run();
        assert!(
            report
                .blocked
                .iter()
                .all(|line| !line.contains("Your edits")),
            "{report:?}"
        );
        assert!(fs::read_to_string(f.repo.join("CLAUDE.local.md"))
            .unwrap()
            .contains("New approved line."));
        assert!(fs::read_to_string(f.repo.join(".codex/config.toml"))
            .unwrap()
            .contains("New approved line."));
    }

    #[test]
    fn rulesync_checkout_old_host_refusal_returns_edit_to_waiting() {
        let mut f = Fixture::new();
        let (claude, codex) = edit_both(&mut f);
        f.run();
        let id = pending_id(&f);
        f.decide_edit(id.clone());
        f.run();
        let state_path = f.setup.data.join(format!("checkouts/{}.json", f.key));
        let mut state: serde_json::Value =
            serde_json::from_slice(&fs::read(&state_path).unwrap()).unwrap();
        state["checkout"]["shared_rule_proposals"] = serde_json::to_value(&f.receipts).unwrap();
        fs::write(&state_path, serde_json::to_vec(&state).unwrap()).unwrap();
        fs::write(f.setup.data.join("checkouts/incompatible.json"), b"{}").unwrap();
        assert_eq!(f.pending_team_count(), 1);
        let refused = rulesync_shared_answers::read(&f.setup.data)
            .unwrap()
            .sent_edits[&id]
            .proposal
            .clone();
        crate::rulesync_routes::reopen_edits_after_old_host_refusal_at(
            &f.setup,
            &f.settings,
            &[refused],
        )
        .unwrap();
        let answers = rulesync_shared_answers::read(&f.setup.data).unwrap();
        let state: serde_json::Value =
            serde_json::from_slice(&fs::read(&state_path).unwrap()).unwrap();
        assert!(state["checkout"]["shared_rule_proposals"]
            .as_object()
            .is_none_or(|map| map.is_empty()));
        f.receipts.clear();
        assert!(answers.waiting.contains_key(&id));
        assert!(answers.answered_edits.is_empty());
        assert!(answers.answers.is_empty());
        assert_eq!(f.pending_team_count(), 0);
        assert_eq!(
            fs::read_to_string(f.repo.join("CLAUDE.local.md")).unwrap(),
            claude
        );
        assert_eq!(
            fs::read_to_string(f.repo.join(".codex/config.toml")).unwrap(),
            codex
        );
        assert!(f
            .run()
            .blocked
            .iter()
            .any(|line| line.contains("shared-edits")));
        assert_eq!(f.pending_team_count(), 0);
        f.decide_edit(id);
        assert_eq!(f.pending_team_count(), 1);
        let team_path = f.settings.with_file_name(wenlan_core::team::TEAM_FILE);
        let proposals = rulesync_team::pending_proposals(
            &team_path,
            &f.settings,
            rulesync_team::SPACE,
            &f.writer.id,
            wenlan_core::team::CredentialKind::BrowserSession,
        )
        .unwrap();
        rulesync_team::decide_proposal(
            &team_path,
            &f.settings,
            rulesync_team::SPACE,
            &f.writer.id,
            wenlan_core::team::CredentialKind::BrowserSession,
            &proposals[0].id,
            true,
        )
        .unwrap();
        f.team = rulesync_team::load_layers(&f.settings).unwrap();
        f.run();
        assert!(fs::read_to_string(f.repo.join("CLAUDE.local.md"))
            .unwrap()
            .contains("revised checklist"));
    }

    #[test]
    fn rulesync_checkout_refused_edit_can_be_reconfirmed_then_rejected() {
        use wenlan_core::team::{self, CredentialKind};
        let mut f = Fixture::new();
        let (claude, codex) = edit_both(&mut f);
        f.run();
        let id = pending_id(&f);
        f.decide_edit(id.clone());
        refuse_sent(&f, &id);
        f.receipts.clear();
        f.decide_edit(id);
        let team_path = f.settings.with_file_name(team::TEAM_FILE);
        let proposals = rulesync_team::pending_proposals(
            &team_path,
            &f.settings,
            rulesync_team::SPACE,
            &f.writer.id,
            CredentialKind::BrowserSession,
        )
        .unwrap();
        assert_eq!(proposals.len(), 1);
        rulesync_team::decide_proposal(
            &team_path,
            &f.settings,
            rulesync_team::SPACE,
            &f.writer.id,
            CredentialKind::BrowserSession,
            &proposals[0].id,
            false,
        )
        .unwrap();
        f.team = rulesync_team::load_layers(&f.settings).unwrap();
        f.run_as(false);
        assert_eq!(
            fs::read_to_string(f.repo.join("CLAUDE.local.md")).unwrap(),
            claude
        );
        assert_eq!(
            fs::read_to_string(f.repo.join(".codex/config.toml")).unwrap(),
            codex
        );
        let report = f.run();
        assert!(report
            .notice_codes
            .contains(&Code::SharedRuleRejected.into()));
        assert!(!fs::read_to_string(f.repo.join("CLAUDE.local.md"))
            .unwrap()
            .contains("revised checklist"));
    }

    #[test]
    fn rulesync_checkout_changed_edit_is_offered_again_after_refusal() {
        let mut f = Fixture::new();
        edit_both(&mut f);
        f.run();
        let id = pending_id(&f);
        f.decide_edit(id.clone());
        f.run();
        assert!(!f.receipts.is_empty(), "sync records the confirmed edit");
        for name in ["CLAUDE.local.md", ".codex/config.toml"] {
            let path = f.repo.join(name);
            fs::write(
                &path,
                fs::read_to_string(&path)
                    .unwrap()
                    .replace("revised checklist", "newer checklist"),
            )
            .unwrap();
        }
        refuse_sent(&f, &id);
        f.run();
        let new_id = pending_id(&f);
        assert_ne!(new_id, id);
        assert!(rulesync_shared_answers::read(&f.setup.data)
            .unwrap()
            .waiting[&new_id]
            .markdown
            .contains("newer checklist"));
        f.decide_edit(new_id);
    }

    #[test]
    fn rulesync_checkout_refusal_reopens_only_live_files() {
        let mut f = Fixture::new();
        edit_both(&mut f);
        f.run();
        let id = pending_id(&f);
        f.decide_edit(id.clone());
        let proposal = rulesync_shared_answers::read(&f.setup.data)
            .unwrap()
            .sent_edits[&id]
            .proposal
            .clone();
        fs::remove_file(f.repo.join("CLAUDE.local.md")).unwrap();
        crate::rulesync_routes::reopen_edits_after_old_host_refusal_at(
            &f.setup,
            &f.settings,
            &[proposal],
        )
        .unwrap();
        let answers = rulesync_shared_answers::read(&f.setup.data).unwrap();
        assert_eq!(
            answers.waiting[&id].files,
            vec![alias(&f.key, ".codex/config.toml").display().to_string()]
        );
        f.decide_edit(id);
    }

    #[test]
    fn rulesync_checkout_failed_proposal_retirement_keeps_sent_edit_retryable() {
        let mut f = Fixture::new();
        edit_both(&mut f);
        f.run();
        let id = pending_id(&f);
        f.decide_edit(id.clone());
        let sent = rulesync_shared_answers::read(&f.setup.data)
            .unwrap()
            .sent_edits[&id]
            .clone();
        let original = fs::read(&sent.proposal_path).unwrap();
        fs::remove_file(&sent.proposal_path).unwrap();
        fs::create_dir(&sent.proposal_path).unwrap();
        assert!(
            crate::rulesync_routes::reopen_edits_after_old_host_refusal_at(
                &f.setup,
                &f.settings,
                std::slice::from_ref(&sent.proposal)
            )
            .is_err()
        );
        let answers = rulesync_shared_answers::read(&f.setup.data).unwrap();
        assert!(answers.sent_edits.contains_key(&id));
        assert!(!answers.waiting.contains_key(&id));
        fs::remove_dir(&sent.proposal_path).unwrap();
        fs::write(&sent.proposal_path, original).unwrap();
        crate::rulesync_routes::reopen_edits_after_old_host_refusal_at(
            &f.setup,
            &f.settings,
            std::slice::from_ref(&sent.proposal),
        )
        .unwrap();
        let answers = rulesync_shared_answers::read(&f.setup.data).unwrap();
        assert!(!answers.sent_edits.contains_key(&id));
        assert!(answers.waiting.contains_key(&id));
        assert!(!sent.proposal_path.exists());
    }

    #[test]
    fn rulesync_checkout_changed_sent_edit_blocks_only_its_file() {
        let mut f = Fixture::new();
        f.trust();
        f.run();
        let path = f.repo.join("CLAUDE.local.md");
        let first = fs::read_to_string(&path).unwrap().replacen(
            "Use the approved checklist.",
            "First edit.",
            1,
        );
        fs::write(&path, first).unwrap();
        f.run();
        let id = rulesync_shared_answers::read(&f.setup.data)
            .unwrap()
            .waiting
            .keys()
            .next()
            .unwrap()
            .clone();
        f.decide_edit(id);
        f.run();
        let latest = fs::read_to_string(&path)
            .unwrap()
            .replace("First edit.", "Second edit.");
        fs::write(&path, &latest).unwrap();
        f.approve_rules(
            rulesync_team::SPACE,
            "Use the approved checklist.\nNew approved line.\n",
        );
        let report = f.run();
        assert_eq!(
            report
                .blocked
                .iter()
                .filter(|line| line.contains("CLAUDE.local.md") && line.contains("shared-edits"))
                .count(),
            1,
            "{report:?}"
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), latest);
        assert!(codex_instructions(
            &fs::read_to_string(f.repo.join(".codex/config.toml")).unwrap()
        )
        .unwrap()
        .contains("New approved line."));
    }

    #[test]
    fn rulesync_checkout_rebase_error_in_one_file_allows_other_file_to_update() {
        let mut f = Fixture::new();
        f.trust();
        f.run();
        let path = f.repo.join("CLAUDE.local.md");
        fs::write(
            &path,
            fs::read_to_string(&path).unwrap().replacen(
                "Use the approved checklist.",
                "First team edit.",
                1,
            ),
        )
        .unwrap();
        f.run();
        let team_id = rulesync_shared_answers::read(&f.setup.data)
            .unwrap()
            .waiting
            .keys()
            .next()
            .unwrap()
            .clone();
        f.decide_edit(team_id);
        f.run();
        fs::write(
            &path,
            fs::read_to_string(&path)
                .unwrap()
                .replace("Use the approved checklist.", "Waiting project edit."),
        )
        .unwrap();
        f.run();
        assert_eq!(
            rulesync_shared_answers::read(&f.setup.data)
                .unwrap()
                .waiting
                .len(),
            1
        );
        let changed = fs::read_to_string(&path)
            .unwrap()
            .replace("First team edit.", "Changed sent edit.");
        fs::write(&path, &changed).unwrap();
        f.approve_rules(
            rulesync_team::SPACE,
            "Use the approved checklist.\nNew approved line.\n",
        );
        f.run();
        assert_eq!(fs::read_to_string(&path).unwrap(), changed);
        assert!(file_rules(&f.repo, ".codex/config.toml").contains("New approved line."));
    }

    #[test]
    fn rulesync_checkout_broken_wrapper_during_rebase_does_not_block_codex() {
        let mut f = Fixture::new();
        f.trust();
        f.run();
        let path = f.repo.join("CLAUDE.local.md");
        fs::write(
            &path,
            fs::read_to_string(&path).unwrap().replacen(
                "Use the approved checklist.",
                "Waiting team edit.",
                1,
            ),
        )
        .unwrap();
        f.run();
        let broken = fs::read_to_string(&path)
            .unwrap()
            .replace(END, "<!-- damaged checkout end -->\n");
        fs::write(&path, &broken).unwrap();
        f.approve_rules(
            rulesync_team::SPACE,
            "Use the approved checklist.\nNew approved line.\n",
        );
        f.run();
        assert_eq!(fs::read_to_string(&path).unwrap(), broken);
        assert!(file_rules(&f.repo, ".codex/config.toml").contains("New approved line."));
    }

    #[test]
    fn rulesync_checkout_sent_edit_survives_changed_or_missing_file() {
        let mut f = Fixture::new();
        f.run();
        let path = f.repo.join("CLAUDE.local.md");
        let edited = fs::read_to_string(&path).unwrap().replacen(
            "Use the approved checklist.",
            "Use my checklist.",
            1,
        );
        fs::write(&path, edited).unwrap();
        f.run();
        let id = rulesync_shared_answers::read(&f.setup.data)
            .unwrap()
            .waiting
            .keys()
            .next()
            .unwrap()
            .clone();
        f.decide_edit(id.clone());
        let proposal_path = rulesync_shared_answers::read(&f.setup.data)
            .unwrap()
            .sent_edits[&id]
            .proposal_path
            .clone();
        fs::write(
            &path,
            fs::read_to_string(&path)
                .unwrap()
                .replace("my checklist", "another checklist"),
        )
        .unwrap();
        rulesync_layers::refresh_answers(&f.settings, &f.setup.data, &f.setup.home).unwrap();
        assert!(rulesync_shared_answers::read(&f.setup.data)
            .unwrap()
            .sent_edits
            .contains_key(&id));
        assert!(proposal_path.exists());
        fs::remove_file(path).unwrap();
        rulesync_layers::refresh_answers(&f.settings, &f.setup.data, &f.setup.home).unwrap();
        assert!(rulesync_shared_answers::read(&f.setup.data)
            .unwrap()
            .sent_edits
            .contains_key(&id));
        assert!(proposal_path.exists());
    }

    #[test]
    fn rulesync_checkout_waiting_edit_rebases_only_on_person_sync() {
        let mut f = Fixture::new();
        let (claude, codex) = edit_both(&mut f);
        f.run();
        let id = pending_id(&f);
        f.approve_rules(
            rulesync_team::SPACE,
            "Use the approved checklist.\nNew approved line.\n",
        );
        f.run_as(false);
        assert_eq!(
            fs::read_to_string(f.repo.join("CLAUDE.local.md")).unwrap(),
            claude
        );
        assert_eq!(
            fs::read_to_string(f.repo.join(".codex/config.toml")).unwrap(),
            codex
        );
        assert_eq!(pending_id(&f), id);
        f.run();
        assert_ne!(pending_id(&f), id);
        assert!(fs::read_to_string(f.repo.join("CLAUDE.local.md"))
            .unwrap()
            .contains("New approved line."));
    }

    #[test]
    fn rulesync_checkout_rebase_merges_identical_waiting_edits_from_two_repos() {
        let mut f = Fixture::new();
        edit_both(&mut f);
        f.run();
        let first = (
            f.repo.clone(),
            f.key.clone(),
            f.claude.take(),
            f.codex.take(),
            std::mem::take(&mut f.receipts),
        );
        let second = f._temp.path().join("second-repo");
        fs::create_dir(&second).unwrap();
        assert!(command(&second, &["init", "--quiet"])
            .unwrap()
            .status
            .success());
        assert!(command(
            &second,
            &[
                "remote",
                "add",
                "origin",
                crate::rulesync_layers::tests::REMOTE
            ]
        )
        .unwrap()
        .status
        .success());
        f.key = wenlan_core::rulesync_personal::fingerprint(&second).unwrap();
        let mut state: serde_json::Value = serde_json::from_slice(
            &fs::read(f.setup.data.join(format!("checkouts/{}.json", first.1))).unwrap(),
        )
        .unwrap();
        state["checkout"]["repo"] = serde_json::to_value(&second).unwrap();
        fs::write(
            f.setup.data.join(format!("checkouts/{}.json", f.key)),
            serde_json::to_vec(&state).unwrap(),
        )
        .unwrap();
        f.repo = second;
        edit_both(&mut f);
        f.run();
        let answers = rulesync_shared_answers::read(&f.setup.data).unwrap();
        let old_id = answers.waiting.keys().next().unwrap().clone();
        assert_eq!(answers.waiting[&old_id].files.len(), 4);
        f.approve_rules(
            rulesync_team::SPACE,
            "Use the approved checklist.\nNew approved line.\n",
        );
        f.run();
        f.repo = first.0;
        f.key = first.1;
        f.claude = first.2;
        f.codex = first.3;
        f.receipts = first.4;
        f.trust();
        f.run();
        let answers = rulesync_shared_answers::read(&f.setup.data).unwrap();
        assert_eq!(answers.waiting.len(), 1);
        assert_eq!(answers.waiting.values().next().unwrap().files.len(), 4);
    }

    #[test]
    fn rulesync_checkout_codex_rebase_checks_trust_and_tracking_first() {
        for tracked_file in [false, true] {
            let mut f = Fixture::new();
            let (_, codex) = edit_both(&mut f);
            f.run();
            if tracked_file {
                assert!(command(&f.repo, &["add", "-f", ".codex/config.toml"])
                    .unwrap()
                    .status
                    .success());
            } else {
                fs::remove_file(f.setup.home.join(".codex/config.toml")).unwrap();
            }
            f.approve_rules(
                rulesync_team::SPACE,
                "Use the approved checklist.\nNew approved line.\n",
            );
            f.run();
            assert_eq!(
                fs::read_to_string(f.repo.join(".codex/config.toml")).unwrap(),
                codex
            );
        }
    }

    #[test]
    fn rulesync_checkout_new_team_revision_refuses_old_confirmation_and_keeps_edit() {
        let mut f = Fixture::new();
        let (claude, codex) = edit_both(&mut f);
        f.run();
        let id = pending_id(&f);
        f.approve_rules(
            rulesync_team::SPACE,
            "Use the approved checklist.\nAnother approved line.\n",
        );
        let error = crate::rulesync_routes::decide_shared_edit_at(
            &f.setup,
            &f.settings,
            wenlan_types::rulesync::SharedEditDecisionRequest {
                id: id.clone(),
                send: true,
            },
        )
        .unwrap_err();
        assert!(matches!(
            error,
            crate::error::ServerError::Structured { status, body }
                if status == axum::http::StatusCode::CONFLICT
                    && body["code"] == wenlan_types::rulesync::SHARED_EDIT_STALE_REVISION_CODE
        ));
        assert!(rulesync_shared_answers::read(&f.setup.data)
            .unwrap()
            .waiting
            .contains_key(&id));
        assert_eq!(
            fs::read_to_string(f.repo.join("CLAUDE.local.md")).unwrap(),
            claude
        );
        assert_eq!(
            fs::read_to_string(f.repo.join(".codex/config.toml")).unwrap(),
            codex
        );
        let report = f.run();
        assert!(report
            .blocked
            .iter()
            .any(|line| line.contains("shared-edits")));
        for name in ["CLAUDE.local.md", ".codex/config.toml"] {
            let content = file_rules(&f.repo, name);
            assert!(content.contains("Use the revised checklist.\nAnother approved line."));
        }
        let new_id = pending_id(&f);
        assert_ne!(new_id, id);
        assert_eq!(
            rulesync_shared_answers::read(&f.setup.data)
                .unwrap()
                .waiting[&new_id]
                .markdown,
            "Use the revised checklist.\nAnother approved line.\n"
        );
        f.decide_edit(new_id);
    }

    #[test]
    fn rulesync_checkout_waiting_team_edit_delivers_new_project_section_in_same_file() {
        let mut f = Fixture::new();
        edit_both(&mut f);
        f.run();
        let id = pending_id(&f);
        f.approve_rules(
            crate::rulesync_layers::tests::PROJECT,
            "Use the approved checklist.\nNew project instruction.\n",
        );
        let report = f.run();
        assert!(report
            .blocked
            .iter()
            .any(|line| line.contains("shared-edits")));
        for name in ["CLAUDE.local.md", ".codex/config.toml"] {
            let content = file_rules(&f.repo, name);
            assert!(content.contains("Use the revised checklist."));
            assert!(content.contains("New project instruction."));
        }
        let answers = rulesync_shared_answers::read(&f.setup.data).unwrap();
        assert!(answers.waiting.contains_key(&id));
        assert_eq!(
            answers.waiting[&id].markdown,
            "Use the revised checklist.\n"
        );
    }

    #[test]
    fn rulesync_checkout_declined_shared_edit_restores_approved_lines_on_person_sync() {
        let mut f = Fixture::new();
        let (claude, codex) = edit_both(&mut f);
        f.run();
        let id = pending_id(&f);
        crate::rulesync_routes::decide_shared_edit_at(
            &f.setup,
            &f.settings,
            wenlan_types::rulesync::SharedEditDecisionRequest { id, send: false },
        )
        .unwrap();
        f.run_as(false);
        assert_eq!(
            fs::read_to_string(f.repo.join("CLAUDE.local.md")).unwrap(),
            claude
        );
        assert_eq!(
            fs::read_to_string(f.repo.join(".codex/config.toml")).unwrap(),
            codex
        );
        let report = f.run();
        assert!(report
            .notice_codes
            .contains(&Code::SharedRuleRejected.into()));
        assert!(!fs::read_to_string(f.repo.join("CLAUDE.local.md"))
            .unwrap()
            .contains("revised checklist"));
        assert!(!fs::read_to_string(f.repo.join(".codex/config.toml"))
            .unwrap()
            .contains("revised checklist"));
    }

    #[test]
    fn rulesync_checkout_stale_shared_edit_cannot_be_sent() {
        let mut f = Fixture::new();
        edit_both(&mut f);
        f.run();
        let id = pending_id(&f);
        fs::write(f.repo.join("CLAUDE.local.md"), f.claude.as_deref().unwrap()).unwrap();
        let answer = crate::rulesync_routes::decide_shared_edit_at(
            &f.setup,
            &f.settings,
            wenlan_types::rulesync::SharedEditDecisionRequest { id, send: true },
        );
        assert!(answer.is_err());
    }
}
