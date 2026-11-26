// SPDX-License-Identifier: Apache-2.0
//! Project rules use the bound Space as the master and the wiki PR as the writer.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::{Deserialize, Serialize};
use wenlan_types::repo_wiki::{ProjectRuleProposal, RepoWikiBinding};

use super::{agents_block, git, run, sha256_hex, AGENTS_END, AGENTS_START, RULES_HEADING};
use crate::error::WenlanError;
use crate::rulesync_personal::{
    describe_change, fingerprint, reject_secret_lines, write_named_root_rule, write_root_rule,
    LEGACY_RULE_PRECEDENCE, RULE_PRECEDENCE,
};
use crate::skill_grow::tool_file_text;

const ROOT_AGENTS: &str = "AGENTS.md";
const ROOT_CLAUDE: &str = "CLAUDE.md";
const TARGETS: &[&str] = &["codexcli", "claudecode", "copilot"];
const TEAM_HEADING: &str = "## Team rules";
const MAX_RULE_FILE: u64 = 256 * 1024;

type Result<T> = std::result::Result<T, WenlanError>;
type StagedRules = BTreeMap<String, String>;

fn conflict(message: impl Into<String>) -> WenlanError {
    WenlanError::Conflict(message.into())
}

fn core_error(error: anyhow::Error) -> WenlanError {
    WenlanError::Generic(error.to_string())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Blob {
    mode: String,
    oid: String,
}

type Snapshot = BTreeMap<String, Option<Blob>>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Delivery {
    /// The default-branch revision the PR commit was made on.
    parent: String,
    commit: String,
    files: Snapshot,
    #[serde(default)]
    confirmed: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Review {
    proposal: ProjectRuleProposal,
    /// Remember a decision even when a wiki-only refresh sees the edit again.
    #[serde(default)]
    accepted: bool,
    #[serde(default)]
    dismissed: bool,
    /// Unreviewed changes in a managed region also block replacement.
    managed: bool,
    /// Identifies the edited text independently of a wiki-only block refresh.
    seen: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    landed: Option<Delivery>,
    offered: Vec<Delivery>,
    reviews: BTreeMap<String, Review>,
}

fn state_path(store: &Path, binding: &RepoWikiBinding) -> Result<PathBuf> {
    let identity = fingerprint(&(&binding.space, &binding.repo_root, &binding.remote_url))
        .map_err(core_error)?;
    Ok(store
        .with_file_name("repo-wiki-project-rules")
        .join(format!("{identity}.json")))
}

fn read_state(path: &Path) -> Result<State> {
    match fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
        Err(error) => Err(error.into()),
    }
}

pub(super) struct Project {
    path: PathBuf,
    _lock: File,
    state: State,
}

impl Project {
    pub(super) fn open(store: &Path, binding: &RepoWikiBinding) -> Result<Self> {
        let path = state_path(store, binding)?;
        fs::create_dir_all(path.parent().expect("project state directory"))?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path.with_extension("lock"))?;
        fs2::FileExt::lock_exclusive(&lock)?;
        let state = read_state(&path)?;
        Ok(Self {
            path,
            _lock: lock,
            state,
        })
    }

    fn save(&self) -> Result<()> {
        let mut staged =
            tempfile::NamedTempFile::new_in(self.path.parent().expect("project state directory"))?;
        serde_json::to_writer(staged.as_file_mut(), &self.state)?;
        staged.as_file().sync_all()?;
        staged.persist(&self.path).map_err(|error| error.error)?;
        Ok(())
    }

    /// A worktree lives under the data root. Check it before Git refreshes it
    /// so a stray link in a reused worktree is still visible to this publish.
    pub(super) fn check_links(&self, root: &Path) -> Result<()> {
        for path in self.paths() {
            check_worktree_path(root, &path)?;
        }
        Ok(())
    }

    fn paths(&self) -> BTreeSet<String> {
        [ROOT_AGENTS.to_owned(), ROOT_CLAUDE.to_owned()]
            .into_iter()
            .chain(
                self.state
                    .landed
                    .iter()
                    .chain(&self.state.offered)
                    .flat_map(|delivery| delivery.files.keys().cloned()),
            )
            .chain(
                self.state
                    .reviews
                    .values()
                    .map(|review| review.proposal.path.clone()),
            )
            .collect()
    }

    pub(super) fn inspect(
        &mut self,
        repo: &Path,
        base: &str,
        branch: Option<&str>,
        carried: &[String],
        staged_paths: impl IntoIterator<Item = String>,
    ) -> Result<Observed> {
        if let Some(branch) = branch {
            for delivery in &mut self.state.offered {
                delivery.confirmed |= super::is_ancestor(repo, &delivery.commit, branch);
            }
        }
        reconcile_landed(&mut self.state, repo, base)?;
        let branch = branch.filter(|branch| !super::is_ancestor(repo, branch, base));
        let fork = branch
            .map(|branch| {
                let mut command = git(repo);
                command.args(["merge-base", base, branch]);
                run(command, "git merge-base")
            })
            .transpose()?;
        let offered = branch.and_then(|branch| {
            self.state
                .offered
                .iter()
                .rev()
                .find(|delivery| super::is_ancestor(repo, &delivery.commit, branch))
        });
        let mut paths = self.paths();
        paths.extend(staged_paths);
        validate_paths(&paths)?;

        let mut files = BTreeMap::new();
        let mut reviews = BTreeMap::new();
        let mut collision = false;
        for path in paths {
            check_worktree_path(repo, &path)?;
            let on_base = blob_at(repo, base, &path)?;
            let tracked = self
                .state
                .landed
                .as_ref()
                .and_then(|delivery| delivery.files.get(&path));
            let base_text = blob_text(repo, &path, on_base.as_ref())?;
            if let Some(before) = tracked {
                if before != &on_base {
                    let before = blob_text(repo, &path, before.as_ref())?;
                    add_review(&self.state, &mut reviews, &path, base, &before, &base_text)?;
                }
            } else {
                // First contact with a handwritten file is a reviewable
                // adoption. Already managed text from the legacy publisher
                // stays eligible only when it is still approved in the Space.
                let hand = parts(&base_text)?.hand();
                if !hand.trim().is_empty() {
                    add_review(&self.state, &mut reviews, &path, base, "", &hand)?;
                }
            }
            carry_existing_reviews(&self.state, &mut reviews, &path, &base_text)?;

            let mut seen = on_base.clone();
            if let (Some(branch), Some(fork)) = (branch, fork.as_deref()) {
                let on_branch = blob_at(repo, branch, &path)?;
                let on_fork = blob_at(repo, fork, &path)?;
                let is_carried = carried.contains(&path);
                let previous = offered.and_then(|delivery| delivery.files.get(&path));
                if let Some(before) = previous.or_else(|| is_carried.then_some(&on_fork)) {
                    if before != &on_branch {
                        let before = blob_text(repo, &path, before.as_ref())?;
                        let branch_text = blob_text(repo, &path, on_branch.as_ref())?;
                        add_review(
                            &self.state,
                            &mut reviews,
                            &path,
                            branch,
                            &before,
                            &branch_text,
                        )?;
                        collision |= !is_carried && on_base != on_branch;
                    }
                }
                if is_carried {
                    let branch_text = blob_text(repo, &path, on_branch.as_ref())?;
                    carry_existing_reviews(&self.state, &mut reviews, &path, &branch_text)?;
                    seen = on_branch;
                }
            }
            let current = blob_text(repo, &path, seen.as_ref())?;
            files.insert(
                path,
                ObservedFile {
                    on_base,
                    seen,
                    current,
                    tracked: tracked.is_some(),
                },
            );
        }
        self.state.reviews = reviews;
        self.save()?;
        let pending_managed = self
            .state
            .reviews
            .values()
            .any(|review| review.managed && !review.accepted && !review.dismissed);
        if collision {
            return Err(conflict(
                "a project rules file changed on both the wiki branch and default branch; reconcile the Git edits before publishing",
            ));
        }
        if pending_managed {
            return Err(conflict(
                "a repository edit to managed project rules is awaiting review in its Space",
            ));
        }
        Ok(Observed { files })
    }

    pub(super) fn remember_delivery(
        &mut self,
        repo: &Path,
        parent: &str,
        commit: &str,
        plan: &Plan,
        confirmed: bool,
    ) -> Result<()> {
        let mut files = Snapshot::new();
        let mut differs = false;
        for (path, target) in &plan.files {
            let written = blob_at(repo, commit, path)?;
            let correct = match &target.change {
                Some(Some(wanted)) => {
                    let mode = target.seen.as_ref().map_or("100644", |file| &file.mode);
                    written.as_ref().is_some_and(|file| file.mode == mode)
                        && blob_text(repo, path, written.as_ref())? == *wanted
                }
                Some(None) => written.is_none(),
                None => written == target.seen,
            };
            if !correct {
                return Err(conflict(format!(
                    "committed project rules file {path:?} differs from its plan"
                )));
            }
            differs |= written != target.on_base;
            files.insert(path.clone(), written);
        }
        if differs
            && !self
                .state
                .offered
                .iter()
                .any(|offer| offer.commit == commit)
        {
            // Write before push to recover a landed commit after interruption.
            // Squash equivalence also needs proof this offer reached the branch.
            self.state.offered.push(Delivery {
                parent: parent.to_owned(),
                commit: commit.to_owned(),
                files,
                confirmed,
            });
            self.save()?;
        } else if confirmed {
            self.confirm_delivery(commit)?;
        }
        Ok(())
    }

    pub(super) fn confirm_delivery(&mut self, commit: &str) -> Result<()> {
        if let Some(offer) = self
            .state
            .offered
            .iter_mut()
            .find(|offer| offer.commit == commit && !offer.confirmed)
        {
            offer.confirmed = true;
            self.save()?;
        }
        Ok(())
    }
}

pub(super) fn proposals(store: &Path, space: &str) -> Result<Vec<ProjectRuleProposal>> {
    let binding = bound_space(store, space)?;
    let state = read_state(&state_path(store, &binding)?)?;
    Ok(state
        .reviews
        .into_values()
        .filter(|review| !review.accepted && !review.dismissed)
        .map(|review| review.proposal)
        .collect())
}

fn bound_space(store: &Path, space: &str) -> Result<RepoWikiBinding> {
    super::binding_at(store, space)?
        .ok_or_else(|| WenlanError::NotFound("this Space is not bound to a repository".into()))
}

pub(super) fn decide(
    store: &Path,
    space: &str,
    id: &str,
    approved_rules: Option<&[String]>,
) -> Result<()> {
    let mut project = Project::open(store, &bound_space(store, space)?)?;
    let review = project.state.reviews.get_mut(id).ok_or_else(|| {
        conflict("this repository rules proposal has changed or is no longer pending")
    })?;
    if review.accepted || review.dismissed {
        return Err(conflict(
            "this repository rules proposal was already reviewed",
        ));
    }
    if let Some(approved) = approved_rules {
        if let Some(reason) = &review.proposal.blocked {
            return Err(conflict(reason));
        }
        let approved = approved_texts(approved)?;
        if review
            .proposal
            .added_rules
            .iter()
            .any(|rule| !approved.contains(rule))
            || review
                .proposal
                .removed_rules
                .iter()
                .any(|rule| approved.contains(rule))
        {
            return Err(conflict("apply the reviewed additions and removals to this Space's approved Rules before accepting the proposal"));
        }
        review.accepted = true;
    } else {
        review.dismissed = true;
    }
    project.save()
}

/// Rulesync is responsible for emitting target paths and text. Project mode
/// asks only for root rule files, with no tool settings or credentials.
pub(super) fn render(store: &Path, rules: &[String], team: &[String]) -> Result<StagedRules> {
    let approved = approved_texts(rules)?;
    let team = approved_texts(team)?;
    if approved.is_empty() && team.is_empty() {
        return Ok(BTreeMap::new());
    }
    let body = format!(
        "{RULES_HEADING}\n\n{RULE_PRECEDENCE}\n\n{}\n",
        approved
            .iter()
            .map(|rule| format!("- {rule}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let filename = crate::rulesync_renderer::pinned_binary_name().map_err(core_error)?;
    let binary = std::env::var_os("WENLAN_RULESYNC_BINARY")
        .map(PathBuf::from)
        .unwrap_or_else(|| store.with_file_name("rulesync").join(filename));
    let stage =
        crate::rulesync_renderer::new_stage(store.parent().expect("repo wiki data directory"))
            .map_err(core_error)?;
    let input = stage.path().join("input");
    let output = stage.path().join("output");
    fs::create_dir(&input)?;
    fs::create_dir(&output)?;
    write_root_rule(&input, &body).map_err(core_error)?;
    let mut roots = Vec::new();
    if !team.is_empty() {
        let root = stage.path().join("team");
        let text = format!(
            "{TEAM_HEADING}\n\n{}\n",
            team.iter()
                .map(|line| format!("- {line}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        write_named_root_rule(&root, "team-rules", &text).map_err(core_error)?;
        roots.push(root);
    }
    roots.push(input);
    let targets = TARGETS
        .iter()
        .map(|target| (*target).to_owned())
        .collect::<Vec<_>>();
    crate::rulesync_renderer::render(
        &binary,
        stage.path(),
        &roots,
        &output,
        false,
        &targets,
        &["rules"],
    )
    .map_err(core_error)?;
    let mut rendered = BTreeMap::new();
    collect_staged(&output, &output, &mut rendered)?;
    if !rendered.contains_key(ROOT_AGENTS) {
        return Err(conflict("rulesync did not render the project AGENTS.md"));
    }
    Ok(rendered)
}

fn collect_staged(root: &Path, directory: &Path, files: &mut StagedRules) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let ty = entry.file_type()?;
        if ty.is_dir() {
            collect_staged(root, &path, files)?;
            continue;
        }
        let relative = path.strip_prefix(root).expect("renderer output path");
        let name = relative
            .iter()
            .map(|part| part.to_str().unwrap_or_default())
            .collect::<Vec<_>>()
            .join("/");
        validate_path(&name)?;
        if !ty.is_file() || entry.metadata()?.len() > MAX_RULE_FILE {
            return Err(conflict(format!(
                "unsupported project rules output {name:?}"
            )));
        }
        files.insert(name, fs::read_to_string(path)?);
    }
    Ok(())
}

fn approved_texts(rules: &[String]) -> Result<Vec<String>> {
    let mut texts = Vec::new();
    let mut seen = BTreeSet::new();
    for rule in rules {
        reject_secret_lines(rule).map_err(core_error)?;
        let escaped = tool_file_text(rule);
        if !escaped.is_empty() && seen.insert(escaped.clone()) {
            texts.push(escaped);
        }
    }
    Ok(texts)
}

fn validate_path(path: &str) -> Result<()> {
    if path.to_ascii_lowercase().ends_with(".md")
        && path.split('/').all(|part| {
            !part.is_empty()
                && !matches!(part, "." | "..")
                && !part.eq_ignore_ascii_case(".git")
                && part
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
        })
    {
        Ok(())
    } else {
        Err(conflict(
            "rulesync produced a path outside the project rules files",
        ))
    }
}

fn validate_paths(paths: &BTreeSet<String>) -> Result<()> {
    let mut folded = BTreeSet::new();
    for path in paths {
        validate_path(path)?;
        if !folded.insert(path.to_ascii_lowercase()) {
            return Err(conflict("project rules paths differ only by case"));
        }
    }
    Ok(())
}

fn revision_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn raw_tree_entry(repo: &Path, tree: &str, path: &str) -> Result<Option<Blob>> {
    if !revision_id(tree) {
        return Err(conflict("invalid Git project rules revision"));
    }
    let mut command = git(repo);
    command.args(["--literal-pathspecs", "ls-tree", "-z", tree, "--", path]);
    let output = run(command, "git ls-tree for project rules")?;
    if output.is_empty() {
        return Ok(None);
    }
    let entry = output.strip_suffix('\0').unwrap_or(&output);
    let (header, name) = entry
        .split_once('\t')
        .ok_or_else(|| conflict("Git returned an invalid project rules entry"))?;
    let fields: Vec<_> = header.split_ascii_whitespace().collect();
    if name != path || fields.len() != 3 || !revision_id(fields[2]) {
        return Err(conflict("Git returned an invalid project rules entry"));
    }
    Ok(Some(Blob {
        mode: fields[0].to_owned(),
        oid: fields[2].to_owned(),
    }))
}

fn blob_at(repo: &Path, tree: &str, path: &str) -> Result<Option<Blob>> {
    validate_path(path)?;
    for (offset, _) in path.match_indices('/') {
        let prefix = &path[..offset];
        match raw_tree_entry(repo, tree, prefix)? {
            Some(entry) if entry.mode == "040000" => {}
            None => return Ok(None),
            _ => return Err(conflict(format!("project rules ancestor {prefix:?} in Git is not a directory (symlinks and submodules are blocked)"))),
        }
    }
    match raw_tree_entry(repo, tree, path)? {
        Some(entry) if !matches!(entry.mode.as_str(), "100644" | "100755") => {
            Err(conflict(format!(
                "project rules file {path:?} in Git is not a regular file (symlinks are blocked)"
            )))
        }
        entry => Ok(entry),
    }
}

fn blob_text(repo: &Path, path: &str, blob: Option<&Blob>) -> Result<String> {
    let Some(blob) = blob else {
        return Ok(String::new());
    };
    if !revision_id(&blob.oid) {
        return Err(conflict("invalid Git project rules object"));
    }
    let mut command = git(repo);
    command.args(["cat-file", "-s", &blob.oid]);
    let size = run(command, "git cat-file size")?;
    if !size.parse::<u64>().is_ok_and(|size| size <= MAX_RULE_FILE) {
        return Err(conflict(format!(
            "project rules file {path:?} is too large to review"
        )));
    }
    let output = git(repo).args(["cat-file", "blob", &blob.oid]).output()?;
    if !output.status.success() {
        return Err(conflict(format!(
            "Git could not read project rules file {path:?}"
        )));
    }
    String::from_utf8(output.stdout)
        .map_err(|_| conflict(format!("project rules file {path:?} is not UTF-8")))
}

fn reconcile_landed(state: &mut State, repo: &Path, base: &str) -> Result<()> {
    if let Some(landed) = &state.landed {
        if !super::is_ancestor(repo, &landed.commit, base) {
            return Err(conflict(
                "the default branch no longer contains the last landed project rules delivery",
            ));
        }
    }
    for (index, offer) in state.offered.iter().enumerate().rev() {
        let landed_commit = if super::is_ancestor(repo, &offer.commit, base) {
            Some(offer.commit.clone())
        } else if offer.confirmed {
            find_squash(repo, base, offer)?
        } else {
            None
        };
        if let Some(commit) = landed_commit {
            state.landed = Some(Delivery {
                parent: offer.parent.clone(),
                commit,
                files: offer.files.clone(),
                confirmed: true,
            });
            state.offered.drain(..=index);
            break;
        }
    }
    Ok(())
}

/// A squash has no ancestry back to the PR commit. The exact set of rule
/// blobs must occur in a default-branch commit after the offer's parent.
fn find_squash(repo: &Path, base: &str, offer: &Delivery) -> Result<Option<String>> {
    if !super::is_ancestor(repo, &offer.parent, base) || offer.parent == base {
        return Ok(None);
    }
    let mut command = git(repo);
    command
        .args([
            "--literal-pathspecs",
            "log",
            "--first-parent",
            "--format=%H",
        ])
        .arg(format!("{}..{base}", offer.parent))
        .arg("--")
        .args(offer.files.keys());
    let history = run(command, "git log for project rules deliveries")?;
    Ok(history.lines().find_map(|commit| {
        offer
            .files
            .iter()
            .all(|(path, expected)| blob_at(repo, commit, path).is_ok_and(|seen| &seen == expected))
            .then(|| commit.to_owned())
    }))
}

fn check_worktree_path(root: &Path, relative: &str) -> Result<()> {
    validate_path(relative)?;
    let mut path = root.to_path_buf();
    let mut parts = std::iter::once(None)
        .chain(relative.split('/').map(Some))
        .peekable();
    while let Some(part) = parts.next() {
        if let Some(part) = part {
            path.push(part);
        }
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        let directory = parts.peek().is_some();
        if (directory && !metadata.is_dir()) || (!directory && !metadata.is_file()) {
            return Err(conflict(format!("project rules path {relative:?} contains a symlink or non-regular entry in the worktree")));
        }
    }
    Ok(())
}

struct Parts<'a> {
    before: &'a str,
    block: &'a str,
    after: &'a str,
}

impl Parts<'_> {
    fn hand(&self) -> String {
        format!("{}{}", self.before, self.after)
    }
}

fn parts(text: &str) -> Result<Parts<'_>> {
    let mut starts = marker_positions(text, AGENTS_START);
    let mut ends = marker_positions(text, AGENTS_END);
    match (starts.next(), ends.next()) {
        (None, None) => Ok(Parts {
            before: text,
            block: "",
            after: "",
        }),
        (Some(start), Some(end))
            if start < end && starts.next().is_none() && ends.next().is_none() =>
        {
            let end = end + AGENTS_END.len();
            Ok(Parts {
                before: &text[..start],
                block: &text[start..end],
                after: &text[end..],
            })
        }
        _ => Err(conflict(
            "project rules file has incomplete or repeated Wenlan markers",
        )),
    }
}

fn marker_positions<'a>(text: &'a str, marker: &'a str) -> impl Iterator<Item = usize> + 'a {
    text.split_inclusive('\n')
        .scan(0, |offset, line| {
            let position = *offset;
            *offset += line.len();
            Some((position, line.trim_end_matches(['\r', '\n'])))
        })
        .filter_map(move |(position, line)| (line == marker).then_some(position))
}

fn hand_rule(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let sentence = ["- ", "+ ", "* "]
        .iter()
        .find_map(|prefix| trimmed.strip_prefix(*prefix))
        .unwrap_or(trimmed);
    (!sentence.is_empty()
        && !sentence.starts_with(['#', '@', '<', '>', '|', '`', '~', '[', '-', '*', '+'])
        && !sentence.contains('\0'))
    .then(|| tool_file_text(sentence))
    .filter(|sentence| !sentence.is_empty())
}

fn candidate_rules(text: &str, managed: bool) -> Result<BTreeSet<String>> {
    let text = parts(text)?;
    let from_block = super::block_rule_lines(text.block);
    Ok(if managed {
        from_block
            .into_iter()
            .map(|line| tool_file_text(&line))
            .collect()
    } else {
        text.hand().lines().filter_map(hand_rule).collect()
    })
}

fn review_digest(text: &str, managed: bool) -> Result<String> {
    let text = parts(text)?;
    Ok(if managed {
        sha256_hex(text.block)
    } else {
        sha256_hex(text.hand().trim_matches(['\r', '\n']))
    })
}

fn carry_existing_reviews(
    state: &State,
    reviews: &mut BTreeMap<String, Review>,
    path: &str,
    current: &str,
) -> Result<()> {
    for (id, review) in &state.reviews {
        if review.proposal.path == path && review_digest(current, review.managed)? == review.seen {
            reviews.entry(id.clone()).or_insert_with(|| review.clone());
        }
    }
    Ok(())
}

fn add_review(
    state: &State,
    reviews: &mut BTreeMap<String, Review>,
    path: &str,
    commit: &str,
    before: &str,
    after: &str,
) -> Result<()> {
    let managed = parts(before)?.block != parts(after)?.block;
    let seen = review_digest(after, managed)?;
    let previous = review_digest(before, managed)?;
    let id = fingerprint(&(path, &previous, &seen, managed)).map_err(core_error)?;
    if let Some(review) = state.reviews.get(&id) {
        reviews.insert(id, review.clone());
        return Ok(());
    }
    let mut diff = String::new();
    let mut added_rules = Vec::new();
    let mut removed_rules = Vec::new();
    let blocked = if reject_secret_lines(before).is_err() || reject_secret_lines(after).is_err() {
        Some("the file may contain credentials; its text is omitted from this proposal".into())
    } else if managed && team_rules_in(before)? != team_rules_in(after)? {
        diff = describe_change(path, before, after).2;
        Some("the team section changed; review that edit in the team Rules master before reconciling Git".into())
    } else {
        diff = describe_change(path, before, after).2;
        let old = candidate_rules(before, managed)?;
        let new = candidate_rules(after, managed)?;
        added_rules.extend(new.difference(&old).cloned());
        removed_rules.extend(old.difference(&new).cloned());
        (added_rules.is_empty() && removed_rules.is_empty())
            .then(|| "review this edit directly in Git; it has no rule text to accept".into())
    };
    reviews.insert(
        id.clone(),
        Review {
            proposal: ProjectRuleProposal {
                id,
                path: path.to_owned(),
                revision: commit.to_owned(),
                diff,
                added_rules,
                removed_rules,
                blocked,
            },
            accepted: false,
            dismissed: false,
            managed,
            seen,
        },
    );
    Ok(())
}

fn team_rules_in(text: &str) -> Result<Vec<&str>> {
    Ok(parts(text)?
        .block
        .lines()
        .skip_while(|line| *line != TEAM_HEADING)
        .skip(1)
        .take_while(|line| !line.starts_with("## ") && *line != super::RULES_HINT)
        .filter(|line| !line.trim().is_empty() && *line != AGENTS_END)
        .collect())
}

struct ObservedFile {
    on_base: Option<Blob>,
    seen: Option<Blob>,
    current: String,
    tracked: bool,
}

pub(super) struct Observed {
    files: BTreeMap<String, ObservedFile>,
}

struct PlannedFile {
    on_base: Option<Blob>,
    seen: Option<Blob>,
    /// No edit, replace the file, or delete a file that only held the block.
    change: Option<Option<String>>,
}

pub(super) struct Plan {
    files: BTreeMap<String, PlannedFile>,
    pub(super) adopted: Vec<String>,
}

impl Plan {
    pub(super) fn changes(&self) -> bool {
        self.files.values().any(|file| file.change.is_some())
    }

    pub(super) fn paths(&self) -> Vec<String> {
        self.files
            .iter()
            .filter_map(|(path, file)| file.change.is_some().then_some(path.clone()))
            .collect()
    }

    /// Overlay only the private worktree's Git index, which is what the
    /// reviewed pull request will carry. The checkout files are never opened
    /// for writing, and checkout/smudge settings cannot rewrite these bytes.
    pub(super) fn stage(&self, worktree: &Path) -> Result<()> {
        let mut records = Vec::new();
        for (path, file) in &self.files {
            check_worktree_path(worktree, path)?;
            if index_blob(worktree, path)? != file.seen {
                return Err(conflict(format!(
                    "project rules file {path:?} changed while the pull request was being prepared"
                )));
            }
            let Some(change) = &file.change else {
                continue;
            };
            match change {
                Some(text) => {
                    let oid = hash_text(worktree, text)?;
                    let mode = file
                        .seen
                        .as_ref()
                        .map_or("100644", |file| file.mode.as_str());
                    write!(records, "{mode} {oid}\t{path}\0")?;
                }
                None => {
                    if let Some(seen) = &file.seen {
                        let zeros = "0".repeat(seen.oid.len());
                        write!(records, "0 {zeros}\t{path}\0")?;
                    }
                }
            }
        }
        if records.is_empty() {
            return Ok(());
        }
        let mut command = git(worktree);
        command.args(["update-index", "-z", "--index-info"]);
        run_with_input(command, "git update-index for project rules", &records)?;
        Ok(())
    }
}

impl Observed {
    pub(super) fn plan(
        self,
        staged: &StagedRules,
        folder: Option<&str>,
        publisher: &str,
        approved: &[String],
        take_over: bool,
        previously_published: bool,
    ) -> Result<Plan> {
        let approved = approved_texts(approved)?;
        let mut plan = Plan {
            files: BTreeMap::new(),
            adopted: Vec::new(),
        };
        let claude_imports_agents = self
            .files
            .get(ROOT_CLAUDE)
            .is_some_and(|file| file.current.lines().any(|line| line.trim() == "@AGENTS.md"));
        for (path, file) in self.files {
            let staged_text = staged.get(&path);
            let parsed = parts(&file.current)?;
            let root = path == ROOT_AGENTS;
            let legacy =
                !parsed.block.is_empty() && matches!(path.as_str(), ROOT_AGENTS | ROOT_CLAUDE);
            if !(staged_text.is_some() || file.tracked || legacy || (root && folder.is_some())) {
                continue;
            }
            if !parsed.block.is_empty() && !take_over {
                match super::block_publisher(parsed.block) {
                    Some(id) if id == publisher => {}
                    None if legacy && previously_published => {}
                    _ => {
                        return Err(conflict(format!(
                            "project rules file {path:?} is kept by another Wenlan"
                        )))
                    }
                }
            }
            if legacy
                && !file.tracked
                && super::block_publisher(parsed.block).as_deref() == Some(publisher)
                && super::block_rule_lines(parsed.block)
                    .into_iter()
                    .any(|rule| !approved.contains(&tool_file_text(&rule)))
            {
                return Err(conflict(format!("project rules file {path:?} has untracked managed rules; reconcile them with the Space before publishing")));
            }
            let rendered = if path == ROOT_CLAUDE && claude_imports_agents {
                None
            } else {
                staged_text
            };
            let wiki = root.then_some(folder).flatten();
            let keep_marker = root && !staged.is_empty();
            let body = block(rendered, &parsed.hand(), wiki, keep_marker, publisher)?;
            let next = merge_block(&file.current, &parsed, body.as_deref(), file.seen.is_some());
            let change = (next.as_deref() != file.seen.as_ref().map(|_| file.current.as_str()))
                .then_some(next);
            if change.is_some() && parsed.block.is_empty() && file.seen.is_some() && body.is_some()
            {
                plan.adopted.push(path.clone());
            }
            plan.files.insert(
                path,
                PlannedFile {
                    on_base: file.on_base,
                    seen: file.seen,
                    change,
                },
            );
        }
        Ok(plan)
    }
}

fn block(
    rendered: Option<&String>,
    hand: &str,
    folder: Option<&str>,
    keep_marker: bool,
    publisher: &str,
) -> Result<Option<String>> {
    let mut body = String::new();
    let mut team = String::new();
    if let Some(rendered) = rendered {
        reject_secret_lines(rendered).map_err(core_error)?;
        let hand = hand.lines().filter_map(hand_rule).collect::<BTreeSet<_>>();
        let mut seen = BTreeSet::new();
        let mut section = None;
        for line in rendered.lines().filter(|line| !line.trim().is_empty()) {
            if line == RULES_HEADING {
                section = Some(false);
                continue;
            }
            if line == TEAM_HEADING {
                section = Some(true);
                continue;
            }
            if line == RULE_PRECEDENCE || line == LEGACY_RULE_PRECEDENCE {
                continue;
            }
            let out = if section
                .ok_or_else(|| conflict("rulesync rendered an unexpected project rules section"))?
            {
                &mut team
            } else {
                &mut body
            };
            let Some(text) = line.strip_prefix("- ") else {
                return Err(conflict(
                    "rulesync rendered an unexpected project rules line",
                ));
            };
            if !hand.contains(text) && seen.insert(text) {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    if body.is_empty() && team.is_empty() && folder.is_none() && !keep_marker {
        return Ok(None);
    }
    let mut block = agents_block(folder, &[], Some(publisher));
    block.truncate(block.len() - AGENTS_END.len());
    if !body.is_empty() || !team.is_empty() {
        if folder.is_some() {
            block.push('\n');
        }
        block.push_str(RULE_PRECEDENCE);
        block.push_str("\n\n");
        for (heading, section) in [(TEAM_HEADING, team), (RULES_HEADING, body)] {
            if !section.is_empty() {
                block.push_str(heading);
                block.push_str("\n\n");
                block.push_str(&section);
                block.push('\n');
            }
        }
        block.push_str(super::RULES_HINT);
        block.push('\n');
    } else if rendered.is_some() && keep_marker {
        // The instructions are already in the hand-written part of AGENTS.md.
        block.push_str(super::RULES_HINT);
        block.push('\n');
    }
    block.push_str(AGENTS_END);
    Ok(Some(block))
}

fn merge_block(
    current: &str,
    parsed: &Parts<'_>,
    body: Option<&str>,
    exists: bool,
) -> Option<String> {
    let eol = super::line_ending(current);
    if let Some(body) = body {
        let body = body.replace('\n', eol);
        if !parsed.block.is_empty() {
            return Some(format!("{}{body}{}", parsed.before, parsed.after));
        }
        return if current.is_empty() || current.ends_with(eol) {
            Some(format!("{current}{body}{eol}"))
        } else {
            Some(format!("{current}{eol}{body}{eol}"))
        };
    }
    if parsed.block.is_empty() {
        return exists.then(|| current.to_owned());
    }
    let hand = parsed.hand();
    (!hand.trim().is_empty()).then_some(hand)
}

fn index_blob(repo: &Path, path: &str) -> Result<Option<Blob>> {
    let mut command = git(repo);
    command.args([
        "--literal-pathspecs",
        "ls-files",
        "--stage",
        "-z",
        "--",
        path,
    ]);
    let output = run(command, "git ls-files for project rules")?;
    if output.is_empty() {
        return Ok(None);
    }
    let entry = output.strip_suffix('\0').unwrap_or(&output);
    let (header, name) = entry
        .split_once('\t')
        .ok_or_else(|| conflict("Git returned an invalid project rules index entry"))?;
    let fields: Vec<_> = header.split_ascii_whitespace().collect();
    if name != path
        || fields.len() != 3
        || !matches!(fields[0], "100644" | "100755")
        || !revision_id(fields[1])
        || fields[2] != "0"
    {
        return Err(conflict(format!(
            "Git index has a non-regular or conflicted project rules file {path:?}"
        )));
    }
    Ok(Some(Blob {
        mode: fields[0].into(),
        oid: fields[1].into(),
    }))
}

fn hash_text(repo: &Path, text: &str) -> Result<String> {
    let mut command = git(repo);
    command.args(["hash-object", "-w", "--stdin"]);
    let oid = run_with_input(
        command,
        "git hash-object for project rules",
        text.as_bytes(),
    )?;
    if !revision_id(&oid) {
        return Err(conflict(
            "Git did not return the staged project rules object",
        ));
    }
    Ok(oid)
}

fn run_with_input(mut command: std::process::Command, what: &str, input: &[u8]) -> Result<String> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let written = child.stdin.take().expect("piped stdin").write_all(input);
    let output = child.wait_with_output()?;
    written?;
    if !output.status.success() {
        return Err(WenlanError::Generic(format!(
            "{what} failed ({})",
            output.status
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

#[cfg(test)]
#[path = "repo_wiki_project_rules_test.rs"]
mod tests;
