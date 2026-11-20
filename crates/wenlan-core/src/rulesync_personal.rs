// SPDX-License-Identifier: Apache-2.0
//! The personal master and the read-only side of adopting an existing setup.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use wenlan_types::rulesync::{AdoptionChange, AdoptionPlan};

use crate::rulesync_adopt::{self, AdoptedSkill, Adoption, AdoptionEntry, AdoptionTarget};

pub const RULE_PRECEDENCE: &str =
    "When instructions disagree, project rules come first, followed by team rules and personal rules.";

/// One switch for every writer to the person's tool folders.
pub fn enabled() -> bool {
    std::env::var_os("WENLAN_RULESYNC").is_some_and(|value| value == "1")
}

/// The CLI and daemon resolve the same root, including the existing dev isolation.
pub fn system_home() -> Result<PathBuf> {
    if let Some(state) = std::env::var_os("WENLAN_DEV_STATE_DIR").filter(|value| !value.is_empty())
    {
        return Ok(PathBuf::from(state).join("ai-tools/home"));
    }
    dirs::home_dir().context("cannot find the home directory")
}

/// These are migration inputs from the existing harness, not output path
/// definitions. Renderer output paths come from the pinned rulesync process.
pub fn adoption_inputs() -> [AdoptionTarget; 2] {
    [
        AdoptionTarget {
            tool: "claudecode".into(),
            rules_path: Some(".claude/CLAUDE.md".into()),
            skills_path: Some(".claude/skills".into()),
        },
        AdoptionTarget {
            tool: "codexcli".into(),
            rules_path: Some(".codex/AGENTS.md".into()),
            skills_path: Some(".codex/skills".into()),
        },
    ]
}

pub fn discover_adoption(home: &Path) -> Result<Adoption> {
    rulesync_adopt::discover(home, &adoption_inputs())
}

pub fn fingerprint(value: &impl Serialize) -> Result<String> {
    let mut hash = Sha256::new();
    serde_json::to_writer(&mut hash, value)?;
    Ok(format!("{:x}", hash.finalize()))
}

/// No staging, renderer, daemon, database, or data-root lookup on this path.
pub fn plan_adoption(home: &Path) -> Result<AdoptionPlan> {
    adoption_plan(&discover_adoption(home)?)
}

pub fn adoption_plan(adoption: &Adoption) -> Result<AdoptionPlan> {
    let mut files = Vec::new();
    for (path, file) in &adoption.files {
        let path = path.to_string_lossy().into_owned();
        let entry_text = |entry: &AdoptionEntry| match entry {
            AdoptionEntry::File(bytes) => match std::str::from_utf8(bytes) {
                Ok(text) if !text.contains('\0') => text.to_owned(),
                _ => format!(
                    "{} binary bytes, SHA-256 {:x}\n",
                    bytes.len(),
                    Sha256::digest(bytes)
                ),
            },
            AdoptionEntry::Symlink { target, bytes } => {
                let body = bytes.as_deref().map(String::from_utf8_lossy);
                format!(
                    "symlink -> {}\n{}",
                    target.display(),
                    body.as_deref().unwrap_or("")
                )
            }
            AdoptionEntry::Directory => String::from("directory\n"),
            AdoptionEntry::Missing => String::new(),
        };
        let before = entry_text(&file.before);
        let after = entry_text(&file.after);
        line_changes(&before, &after).with_context(|| format!("cannot fully preview {path}"))?;
        let (added, removed, diff) = describe_change(&path, &before, &after);
        files.push(AdoptionChange {
            path,
            added,
            removed,
            diff,
        });
    }
    for (path, reason) in &adoption.skipped {
        files.push(AdoptionChange {
            path: path.to_string_lossy().into_owned(),
            added: 0,
            removed: 0,
            diff: format!("Skipped ({reason})\n"),
        });
    }
    Ok(AdoptionPlan {
        token: fingerprint(adoption)?,
        files,
    })
}

/// Wenlan keeps imported text as authored. Only Wenlan-generated additions
/// have already passed through the UX-SKILL-8 escaping in skill_grow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonalMaster {
    pub shared: String,
    pub overlays: BTreeMap<String, String>,
    pub skills: Vec<AdoptedSkill>,
    #[serde(default)]
    pub overrides: BTreeMap<String, LocalText>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalText {
    pub base: String,
    pub edited: String,
}

impl PersonalMaster {
    pub fn new(adoption: &Adoption) -> Self {
        Self {
            shared: adoption.shared.clone(),
            overlays: adoption.overlays.clone(),
            skills: adoption.skills.clone(),
            overrides: BTreeMap::new(),
        }
    }

    pub fn base_rule_text(&self, target: &str, generated: &[String]) -> String {
        let overlay = self.overlays.get(target).map(String::as_str).unwrap_or("");
        let mut base = join_text(&self.shared, overlay);
        if !generated.is_empty() {
            let mut addition = String::from("\n## How I work\n\n");
            for line in generated {
                addition.push_str("- ");
                addition.push_str(line);
                addition.push('\n');
            }
            base = join_text(&base, &addition);
        }
        base
    }

    pub fn rule_text(&self, target: &str, generated: &[String]) -> Result<String> {
        let base = self.base_rule_text(target, generated);
        match self.overrides.get(target) {
            Some(local) => merge_rule_text(&local.base, &local.edited, &base),
            None => Ok(base),
        }
    }
}

pub fn join_text(first: &str, second: &str) -> String {
    if first.is_empty() || second.is_empty() || first.ends_with('\n') {
        format!("{first}{second}")
    } else {
        format!("{first}\n{second}")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineChange {
    pub start: usize,
    pub removed: Vec<String>,
    pub added: Vec<String>,
}

fn bounded_lines(before: &[&str], after: &[&str]) -> bool {
    // Prevent a local tool's accidental huge file from allocating an unbounded
    // LCS table. Large files stay untouched and can be reviewed separately.
    before.len().saturating_mul(after.len()) <= 4_000_000
}

pub fn line_changes(before: &str, after: &str) -> Result<Vec<LineChange>> {
    let before: Vec<_> = before.split_inclusive('\n').collect();
    let after: Vec<_> = after.split_inclusive('\n').collect();
    let prefix = before
        .iter()
        .zip(&after)
        .take_while(|(a, b)| a == b)
        .count();
    let suffix = before[prefix..]
        .iter()
        .rev()
        .zip(after[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let old = &before[prefix..before.len() - suffix];
    let new = &after[prefix..after.len() - suffix];
    ensure!(
        bounded_lines(old, new),
        "too many changed lines to compare safely"
    );
    let pairs = crate::authorship::lcs_pairs(old, new);
    let mut changes = Vec::new();
    let (mut previous_old, mut previous_new) = (0, 0);
    for (i, j) in pairs
        .into_iter()
        .chain(std::iter::once((old.len(), new.len())))
    {
        if (i, j) != (previous_old, previous_new) {
            changes.push(LineChange {
                start: prefix + previous_old,
                removed: old[previous_old..i]
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
                added: new[previous_new..j]
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
            });
        }
        previous_old = i + 1;
        previous_new = j + 1;
    }
    Ok(changes)
}

pub fn describe_change(path: &str, before: &str, after: &str) -> (usize, usize, String) {
    let Ok(changes) = line_changes(before, after) else {
        return (0, 0, "Too many changed lines for a safe preview.\n".into());
    };
    if changes.is_empty() {
        return (0, 0, String::new());
    }
    let mut diff = format!("--- {path}\n+++ {path}\n");
    let (mut added, mut removed) = (0, 0);
    for change in changes {
        let old_line = change.start + usize::from(!change.removed.is_empty());
        let new_line = change.start + added - removed + usize::from(!change.added.is_empty());
        diff.push_str(&format!(
            "@@ -{old_line},{} +{new_line},{} @@\n",
            change.removed.len(),
            change.added.len()
        ));
        removed += change.removed.len();
        added += change.added.len();
        for (prefix, lines) in [('-', change.removed), ('+', change.added)] {
            for line in lines {
                diff.push(prefix);
                diff.push_str(&line);
                if !line.ends_with('\n') {
                    diff.push_str("\n\\ No newline at end of file\n");
                }
            }
        }
    }
    (added, removed, diff)
}

/// Apply both sets of changes to a common ancestor. Overlapping changes
/// require consent when they disagree rather than guessing how to combine them.
pub fn merge_rule_text(base: &str, left: &str, right: &str) -> Result<String> {
    if left == base || left == right {
        return Ok(right.into());
    }
    if right == base {
        return Ok(left.into());
    }
    let mut changes = line_changes(base, left)?;
    for change in line_changes(base, right)? {
        if changes.contains(&change) {
            continue;
        }
        let end = change.start + change.removed.len();
        if changes.iter().any(|earlier| {
            let earlier_end = earlier.start + earlier.removed.len();
            earlier.start < end && change.start < earlier_end || earlier.start == change.start
        }) {
            bail!("the master and this tool changed the same lines");
        }
        changes.push(change);
    }
    changes.sort_by_key(|change| change.start);
    let lines: Vec<_> = base.split_inclusive('\n').collect();
    let (mut result, mut at) = (String::new(), 0);
    for change in changes {
        result.extend(lines[at..change.start].iter().copied());
        result.extend(change.added);
        at = change.start + change.removed.len();
    }
    result.extend(lines[at..].iter().copied());
    Ok(result)
}

/// Accept a tool's edit into the shared rules only while its original lines
/// identify exactly one place there. Appended material is put at the end.
pub fn promote_changes(shared: &str, before: &str, edited: &str) -> Result<String> {
    let mut shared: Vec<String> = shared.split_inclusive('\n').map(str::to_owned).collect();
    for change in line_changes(before, edited)? {
        if change.removed.is_empty() {
            if let Some(last) = shared.last_mut() {
                if !last.ends_with('\n') {
                    last.push('\n');
                }
            }
            shared.extend(change.added);
            continue;
        }
        let mut matches = shared
            .windows(change.removed.len())
            .enumerate()
            .filter_map(|(at, lines)| (lines == change.removed).then_some(at));
        let at = matches
            .next()
            .context("this change has no matching shared rule")?;
        ensure!(
            matches.next().is_none(),
            "this change matches multiple shared rules"
        );
        shared.splice(at..at + change.removed.len(), change.added);
    }
    Ok(shared.concat())
}

pub fn write_root_rule(input: &Path, text: &str) -> Result<()> {
    write_named_root_rule(input, "wenlan", text)
}

pub fn write_named_root_rule(input: &Path, name: &str, text: &str) -> Result<()> {
    ensure!(
        !name.is_empty()
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'),
        "invalid staged rule name"
    );
    let rules = input.join("rules");
    fs::create_dir_all(&rules)?;
    fs::write(
        rules.join(format!("{name}.md")),
        format!("---\nroot: true\n---\n{text}"),
    )?;
    Ok(())
}

/// Shared setup must never carry credential-like text. Error messages
/// identify only the position so the suspected value is never echoed.
pub fn reject_secret_lines(text: &str) -> Result<()> {
    static TOKEN: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(concat!(
            r"-----BEGIN (?:[A-Z0-9]+ )*PRIVATE KEY-----",
            r"|\b(?:sk-[A-Za-z0-9_-]{20,}|gh[pousr]_[A-Za-z0-9]{20,}",
            r"|github_pat_[A-Za-z0-9_]{20,}|xox[baprs]-[A-Za-z0-9-]{16,}",
            r"|(?:AKIA|ASIA)[A-Z0-9]{16}|AIza[A-Za-z0-9_-]{35}|wlt_[a-f0-9]{64})\b",
            r"|(?i)\bbearer\s+[A-Za-z0-9_.~+/-]{20,}",
            r#"|(?i)\b(?:[a-z0-9_]*[_-])?(?:api[_-]?key|secret|token|password|access[_-]?key)\b["']?"#,
            r#"\s*[:=]\s*["']?[A-Za-z0-9_+/.-]{8,}"#,
        )).expect("credential detection pattern")
    });
    for (index, line) in text.lines().enumerate() {
        ensure!(
            !TOKEN.is_match(line),
            "possible credential on line {}",
            index + 1
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_changes_include_deletions_and_final_newline_changes() {
        let (added, removed, rendered) = describe_change("AGENTS.md", "a\nb\nc", "a\nc\n");
        assert_eq!((added, removed), (1, 2));
        assert!(rendered.contains("-b\n-c\n\\ No newline at end of file\n+c\n"));
    }

    #[test]
    fn tool_only_deletion_survives_an_unrelated_master_edit() {
        let base = "one\ntwo\nthree\n";
        let local = "one\nthree\n";
        assert_eq!(
            merge_rule_text(base, local, "one\ntwo\nnew\n").unwrap(),
            "one\nnew\n"
        );
        assert!(merge_rule_text(base, local, "one\nsecond\nthree\n").is_err());
    }

    #[test]
    fn a_shared_change_requires_an_unambiguous_anchor() {
        assert!(promote_changes("x\nx\n", "x\n", "y\n").is_err());
        assert_eq!(promote_changes("x\ny\n", "x\n", "z\n").unwrap(), "z\ny\n");
    }

    #[test]
    fn shared_masters_refuse_credentials_without_echoing_them() {
        let fixture = ["sk-", &"x".repeat(24)].concat();
        let error = reject_secret_lines(&format!("ordinary rule\nBearer {fixture}")).unwrap_err();
        assert_eq!(error.to_string(), "possible credential on line 2");
        assert!(reject_secret_lines("Read API keys from the system keychain.").is_ok());
        assert!(reject_secret_lines("SERVICE_TOKEN=fixture-value").is_err());
    }
}
