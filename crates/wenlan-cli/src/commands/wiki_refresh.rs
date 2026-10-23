// SPDX-License-Identifier: Apache-2.0
//! `wenlan wiki refresh` and `wenlan wiki workflow`: keep a wiki folder that
//! Wenlan put into a repository in step with the code, from a plain checkout
//! and without Wenlan running. This is what the GitHub Actions workflow runs.
//!
//! The pages whose code only moved get new line numbers. The pages whose code
//! changed go to the AI tool (Claude Code or Codex) with a prompt naming them;
//! anything else it touched is put back, and a page it left naming lines that
//! do not exist is restored and left for a person. `--pull-request` commits
//! the folder on `wenlan/wiki-refresh` and opens or updates the pull request.

use anyhow::{Context, Result};
use clap::{Args, ValueEnum};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use wenlan_core::repo_wiki::{self, find_agent, find_gh, github_slug, origin_url};
use wenlan_core::repo_wiki_ci::{
    self, Reanchored, WikiCheck, CODE_FILE, REFRESH_BRANCH, WORKFLOW_PATH,
};
use wenlan_types::repo_wiki::{RepoWikiAgent, DEFAULT_REPO_WIKI_FOLDER};

use crate::output::{print_json, ResolvedFormat};

#[derive(Clone, Copy, ValueEnum)]
pub enum RefreshAgentArg {
    Claude,
    Codex,
    Cursor,
    Copilot,
    /// Only fix line numbers and list the pages that need rewriting.
    None,
}

#[derive(Args)]
pub struct RefreshArgs {
    /// Any folder inside the repository. Default: the current folder.
    #[arg(long)]
    pub repo: Option<PathBuf>,
    /// The wiki folder inside the repository.
    #[arg(long, default_value = DEFAULT_REPO_WIKI_FOLDER)]
    pub folder: String,
    /// The AI tool that rewrites pages whose code changed.
    #[arg(long, value_enum, default_value = "claude")]
    pub agent: RefreshAgentArg,
    /// Commit the changed pages on `wenlan/wiki-refresh` and open or update
    /// the pull request.
    #[arg(long)]
    pub pull_request: bool,
}

#[derive(Args)]
pub struct WorkflowArgs {
    /// Any folder inside the repository. Default: the current folder.
    #[arg(long)]
    pub repo: Option<PathBuf>,
    /// The wiki folder inside the repository.
    #[arg(long, default_value = DEFAULT_REPO_WIKI_FOLDER)]
    pub folder: String,
    /// The AI tool the workflow runs.
    #[arg(long, value_enum, default_value = "claude")]
    pub agent: super::wiki::AgentArg,
    /// The hour (0-23, UTC) it runs every day.
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u8).range(0..=23))]
    pub utc_hour: u8,
    /// Write it into the repository's GitHub workflows folder, as
    /// wenlan-wiki.yml, instead of printing it.
    #[arg(long)]
    pub write: bool,
}

#[derive(Debug, Default, serde::Serialize)]
pub struct RefreshReport {
    /// Pages whose code is where it was.
    pub unchanged: usize,
    /// Pages that only got new line numbers.
    pub moved: Vec<String>,
    /// Pages the AI tool rewrote.
    pub rewritten: Vec<String>,
    /// Pages whose code changed and that are still as they were, with why.
    pub left: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pull_request: Option<String>,
}

fn git(dir: &Path, args: &[&str]) -> Result<String> {
    Ok(git_raw(dir, args)?.trim().to_owned())
}

/// Like [`git`], keeping leading spaces: `git status --porcelain` lines start
/// with a status column that may be blank.
fn git_raw(dir: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .with_context(|| format!("could not run git {}", args.join(" ")))?;
    if !output.status.success() {
        anyhow::bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Paths `git status` reports as changed or new, relative to the top level.
fn changed_paths(top: &Path) -> Result<BTreeSet<String>> {
    let out = git_raw(top, &["status", "--porcelain", "--untracked-files=all"])?;
    Ok(out
        .lines()
        .filter_map(|line| line.get(3..))
        .map(|path| match path.split_once(" -> ") {
            Some((_, to)) => to,
            None => path,
        })
        .map(|path| path.trim_matches('"').to_owned())
        .collect())
}

/// Put back a path the AI tool was not asked to change.
fn restore(top: &Path, path: &str) -> Result<()> {
    if git(top, &["ls-files", "--error-unmatch", "--", path]).is_ok() {
        git(top, &["checkout", "--", path])?;
    } else {
        let full = top.join(path);
        if full.is_file() {
            std::fs::remove_file(full)?;
        }
    }
    Ok(())
}

pub fn workflow(args: WorkflowArgs, quiet: bool) -> Result<()> {
    let folder = repo_wiki::normalize_folder(&args.folder)?;
    let text = repo_wiki_ci::workflow(&folder, args.agent.into(), args.utc_hour);
    if !args.write {
        print!("{text}");
        return Ok(());
    }
    let repo = match args.repo {
        Some(repo) => repo,
        None => std::env::current_dir()?,
    };
    let top = repo_wiki::repository_top_level(&repo)?;
    let path = top.join(WORKFLOW_PATH);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, text)?;
    if !quiet {
        println!(
            "Wrote {}. Commit it, add the secret it names, and allow GitHub Actions to create pull requests in the repository's settings.",
            path.display()
        );
    }
    Ok(())
}

/// Run the refresh; `Ok(false)` when the AI tool failed (what could be done
/// is still done and offered).
pub fn refresh(args: RefreshArgs, format: ResolvedFormat, quiet: bool) -> Result<bool> {
    let repo = match args.repo {
        Some(repo) => repo,
        None => std::env::current_dir()?,
    };
    let top = repo_wiki::repository_top_level(&repo)?;
    let folder = repo_wiki::normalize_folder(&args.folder)?;
    let found = repo_wiki_ci::check(&top, &folder)?;
    let mut report = RefreshReport {
        unchanged: found.unchanged,
        moved: found.moved().map(|page| page.file.clone()).collect(),
        ..RefreshReport::default()
    };
    repo_wiki_ci::apply_moves(&top, &folder, &found)?;

    let agent = match args.agent {
        RefreshAgentArg::Claude => Some(RepoWikiAgent::Claude),
        RefreshAgentArg::Codex => Some(RepoWikiAgent::Codex),
        RefreshAgentArg::Cursor => Some(RepoWikiAgent::Cursor),
        RefreshAgentArg::Copilot => Some(RepoWikiAgent::Copilot),
        RefreshAgentArg::None => None,
    };
    let to_rewrite: Vec<String> = found.to_rewrite().map(|page| page.file.clone()).collect();
    if !to_rewrite.is_empty() {
        match agent {
            None => {
                for file in &to_rewrite {
                    report
                        .left
                        .insert(file.clone(), "its code changed; no AI tool was run".into());
                }
            }
            Some(agent) => rewrite(&top, &folder, &found, agent, &to_rewrite, &mut report)?,
        }
    }

    if args.pull_request {
        report.pull_request = open_pull_request(&top, &folder, &report)?;
    }
    if !quiet {
        match format {
            ResolvedFormat::Json => print_json(&report)?,
            ResolvedFormat::Table => println!("{}", describe(&report)),
        }
    }
    Ok(report.agent_error.is_none())
}

fn rewrite(
    top: &Path,
    folder: &str,
    found: &WikiCheck,
    agent: RepoWikiAgent,
    files: &[String],
    report: &mut RefreshReport,
) -> Result<()> {
    let wiki_dir = top.join(folder);
    let before_text: BTreeMap<&String, String> = files
        .iter()
        .map(|file| {
            (
                file,
                std::fs::read_to_string(wiki_dir.join(file)).unwrap_or_default(),
            )
        })
        .collect();
    let before = changed_paths(top)?;
    match find_agent(agent) {
        None => {
            report.agent_error = Some(format!("{} was not found on PATH", agent.programs()[0]));
        }
        Some(program) => {
            let prompt = repo_wiki_ci::rewrite_prompt(folder, found);
            let mut command = Command::new(program);
            command
                .args(repo_wiki_ci::refresh_agent_args(agent, &prompt))
                .current_dir(top)
                .stdin(std::process::Stdio::null());
            // An unset repository secret arrives as an empty variable; a tool
            // that sees an empty key may try it instead of the one that is set.
            for key in [
                "ANTHROPIC_API_KEY",
                "CLAUDE_CODE_OAUTH_TOKEN",
                "CODEX_API_KEY",
                "CURSOR_API_KEY",
                "COPILOT_GITHUB_TOKEN",
            ] {
                if std::env::var_os(key).is_some_and(|value| value.is_empty()) {
                    command.env_remove(key);
                }
            }
            match command.status() {
                Ok(status) if status.success() => {}
                Ok(status) => {
                    report.agent_error = Some(format!("the AI tool exited with {status}"))
                }
                Err(error) => {
                    report.agent_error = Some(format!("could not start the AI tool: {error}"))
                }
            }
        }
    }

    // Only the listed pages may change; put back anything else it touched.
    let allowed: BTreeSet<String> = files
        .iter()
        .map(|file| format!("{folder}/{file}"))
        .chain([format!("{folder}/{CODE_FILE}")])
        .collect();
    for path in changed_paths(top)?.difference(&before) {
        if !allowed.contains(path) {
            restore(top, path)?;
        }
    }

    let edited: Vec<String> = files
        .iter()
        .filter(|file| {
            std::fs::read_to_string(wiki_dir.join(file)).unwrap_or_default() != before_text[file]
        })
        .cloned()
        .collect();
    for file in files.iter().filter(|file| !edited.contains(file)) {
        report.left.insert(
            file.clone(),
            "its code changed; the AI tool did not update it".into(),
        );
    }
    if edited.is_empty() {
        return Ok(());
    }
    // The manifest names this commit only once every page is right for it.
    let base = if edited.len() == files.len() {
        repo_wiki_ci::head_commit(top)
    } else {
        found.base.clone()
    };
    for outcome in repo_wiki_ci::reanchor(top, folder, &edited, base)? {
        match outcome {
            Reanchored::Updated { file } => report.rewritten.push(file),
            Reanchored::Broken { file, reason } => {
                std::fs::write(wiki_dir.join(&file), &before_text[&file])?;
                report.left.insert(
                    file,
                    format!("the AI tool's version names code that does not exist ({reason})"),
                );
            }
        }
    }
    Ok(())
}

fn pr_body(report: &RefreshReport) -> String {
    let mut body = String::from("Wenlan checked the code each wiki page relies on.\n");
    if !report.rewritten.is_empty() {
        body.push_str("\nRewritten because their code changed:\n");
        for file in &report.rewritten {
            body.push_str(&format!("- `{file}`\n"));
        }
    }
    if !report.moved.is_empty() {
        body.push_str(&format!(
            "\n{} page(s) only got new line numbers: the code they cite moved.\n",
            report.moved.len()
        ));
    }
    if !report.left.is_empty() {
        body.push_str("\nStill to look at:\n");
        for (file, why) in &report.left {
            body.push_str(&format!("- `{file}`: {why}\n"));
        }
    }
    body.push_str(&format!(
        "\nThis branch is rebuilt on every run. Merge it, and Wenlan brings the pages back into \
         the Space on its next update.\n\nGenerated by `wenlan wiki refresh` ({}).",
        wenlan_core::version()
    ));
    body
}

fn open_pull_request(top: &Path, folder: &str, report: &RefreshReport) -> Result<Option<String>> {
    if git(top, &["status", "--porcelain", "--", folder])?.is_empty() {
        return Ok(None);
    }
    let base = git(top, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    git(top, &["switch", "--quiet", "-C", REFRESH_BRANCH])?;
    git(top, &["add", "--all", "--", folder])?;
    let mut commit = vec![];
    if git(top, &["config", "user.email"])
        .unwrap_or_default()
        .is_empty()
    {
        commit.extend([
            "-c",
            "user.name=github-actions[bot]",
            "-c",
            "user.email=41898282+github-actions[bot]@users.noreply.github.com",
        ]);
    }
    commit.extend([
        "commit",
        "--quiet",
        "-m",
        "docs(wiki): keep the wiki in step with the code",
    ]);
    git(top, &commit)?;
    git(
        top,
        &[
            "push",
            "--quiet",
            "--force",
            "origin",
            &format!("HEAD:refs/heads/{REFRESH_BRANCH}"),
        ],
    )?;
    let Some(slug) = origin_url(top).as_deref().and_then(github_slug) else {
        return Ok(Some(format!("pushed {REFRESH_BRANCH}")));
    };
    let compare = format!("https://github.com/{slug}/compare/{base}...{REFRESH_BRANCH}?expand=1");
    let Some(gh) = find_gh() else {
        return Ok(Some(compare));
    };
    let gh_run = |args: &[&str]| -> Result<String> {
        let output = Command::new(&gh).current_dir(top).args(args).output()?;
        if !output.status.success() {
            anyhow::bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    };
    let open = gh_run(&[
        "pr",
        "list",
        "--repo",
        &slug,
        "--head",
        REFRESH_BRANCH,
        "--state",
        "open",
        "--json",
        "url",
        "--jq",
        ".[0].url // \"\"",
    ])
    .unwrap_or_default();
    if !open.is_empty() {
        return Ok(Some(open));
    }
    match gh_run(&[
        "pr",
        "create",
        "--repo",
        &slug,
        "--base",
        &base,
        "--head",
        REFRESH_BRANCH,
        "--title",
        "docs(wiki): keep the wiki in step with the code",
        "--body",
        &pr_body(report),
    ]) {
        Ok(out) => Ok(out
            .lines()
            .rev()
            .find(|line| line.starts_with("https://"))
            .map(str::to_owned)
            .or(Some(compare))),
        Err(error) => {
            eprintln!(
                "wenlan: could not open the pull request ({error}). If GitHub Actions may not create pull requests in this repository, allow it under Settings > Actions > General, or open it here: {compare}"
            );
            Ok(Some(compare))
        }
    }
}

fn describe(report: &RefreshReport) -> String {
    let mut lines = Vec::new();
    if report.moved.is_empty() && report.rewritten.is_empty() && report.left.is_empty() {
        lines.push(format!(
            "All {} page(s) still match their code. Nothing to do.",
            report.unchanged
        ));
    } else {
        if !report.moved.is_empty() {
            lines.push(format!(
                "{} page(s) got new line numbers: their code moved.",
                report.moved.len()
            ));
        }
        if !report.rewritten.is_empty() {
            lines.push(format!(
                "{} page(s) rewritten because their code changed: {}",
                report.rewritten.len(),
                report.rewritten.join(", ")
            ));
        }
        for (file, why) in &report.left {
            lines.push(format!("{file}: {why}"));
        }
    }
    if let Some(error) = &report.agent_error {
        lines.push(format!("The AI tool failed: {error}"));
    }
    if let Some(pr) = &report.pull_request {
        lines.push(format!("Pull request: {pr}"));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pull_request_names_rewritten_and_left_pages() {
        let report = RefreshReport {
            unchanged: 3,
            moved: vec!["pages/a.md".into()],
            rewritten: vec!["pages/b.md".into()],
            left: BTreeMap::from([("pages/c.md".into(), "why".into())]),
            ..RefreshReport::default()
        };
        let body = pr_body(&report);
        assert!(body.contains("- `pages/b.md`\n"), "{body}");
        assert!(
            body.contains("1 page(s) only got new line numbers"),
            "{body}"
        );
        assert!(body.contains("- `pages/c.md`: why\n"), "{body}");
        let text = describe(&report);
        assert!(
            text.contains("rewritten because their code changed: pages/b.md"),
            "{text}"
        );
    }
}
