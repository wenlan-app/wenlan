// SPDX-License-Identifier: Apache-2.0
//! A bound Space's daily update run on GitHub Actions instead of this
//! computer: the workflow file the wiki pull request carries, and the two
//! one-time settings it needs in the repository (a secret for the AI tool,
//! and Actions being allowed to open pull requests). Uses the GitHub CLI the
//! user is signed in to; a token handled here is never kept or logged.

use crate::error::WenlanError;
use crate::repo_wiki::{self, git, github_slug, origin_url, run};
use crate::repo_wiki_ci::{self, WORKFLOW_PATH};
use std::io::{Read as _, Write as _};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use wenlan_types::repo_wiki::{
    RepoWikiAgent, RepoWikiBinding, RepoWikiDaily, RepoWikiGithubRun, RepoWikiGithubStatus,
    RepoWikiRunner, RepoWikiWorkflowState, REPO_WIKI_BRANCH,
};

/// How the workflow file Wenlan writes starts; one that does not is the
/// user's, and is never removed.
const WORKFLOW_MARK: &str = "# Written by Wenlan";
/// The secret `claude setup-token` fills.
pub const CLAUDE_SECRET: &str = "CLAUDE_CODE_OAUTH_TOKEN";
/// The secret a Codex run reads (passed to it as `CODEX_API_KEY`).
pub const OPENAI_SECRET: &str = "OPENAI_API_KEY";
/// Longest wait for the user to confirm the Claude sign-in in the browser.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// This computer's offset from UTC now, in seconds.
pub fn local_offset_seconds() -> i32 {
    chrono::Local::now().offset().local_minus_utc()
}

/// The UTC hour a run at local `hour` starts in, for a zone `offset_seconds`
/// ahead of UTC. GitHub's schedule is in UTC.
pub fn utc_hour(hour: u8, offset_seconds: i32) -> u8 {
    let minutes = i32::from(hour) * 60 - offset_seconds / 60;
    minutes.div_euclid(60).rem_euclid(24) as u8
}

/// Write the daily workflow into `worktree` when the daily update runs on
/// GitHub, and take Wenlan's own out when it no longer does, so the
/// repository never runs a daily update the user turned off or moved back
/// to this computer.
pub fn maintain_workflow(
    worktree: &Path,
    folder: &str,
    daily: Option<&RepoWikiDaily>,
    offset_seconds: i32,
) -> Result<(), WenlanError> {
    let path = worktree.join(WORKFLOW_PATH);
    let current = std::fs::read_to_string(&path).ok();
    match daily {
        Some(daily) if daily.enabled && daily.runs_on == RepoWikiRunner::Github => {
            let text =
                repo_wiki_ci::workflow(folder, daily.agent, utc_hour(daily.hour, offset_seconds));
            if current.as_deref() != Some(text.as_str()) {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&path, text)?;
            }
        }
        _ => {
            if current.is_some_and(|text| text.starts_with(WORKFLOW_MARK)) {
                std::fs::remove_file(&path)?;
            }
        }
    }
    Ok(())
}

/// `owner/name` of the bound repository on GitHub.
fn slug_of(binding: &RepoWikiBinding) -> Result<String, WenlanError> {
    origin_url(Path::new(&binding.repo_root))
        .as_deref()
        .and_then(github_slug)
        .ok_or_else(|| {
            WenlanError::Validation(
                "the repository's `origin` is not on GitHub, so it cannot run there".to_owned(),
            )
        })
}

fn gh_output(gh: &Path, args: &[&str]) -> Result<String, WenlanError> {
    let mut command = Command::new(gh);
    command.args(args).stdin(Stdio::null());
    run(
        command,
        &format!("gh {}", args.first().copied().unwrap_or_default()),
    )
}

fn gh_json(gh: &Path, args: &[&str]) -> Option<serde_json::Value> {
    gh_output(gh, args)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
}

/// Whether the GitHub CLI at `gh` is signed in.
fn gh_signed_in(gh: &Path) -> bool {
    Command::new(gh)
        .args(["auth", "status", "--hostname", "github.com"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// The secrets the chosen tool reads; any one of them is enough.
fn secrets_for(agent: RepoWikiAgent) -> &'static [&'static str] {
    match agent {
        RepoWikiAgent::Claude => &[CLAUDE_SECRET, "ANTHROPIC_API_KEY"],
        RepoWikiAgent::Codex => &[OPENAI_SECRET],
    }
}

/// Where the workflow file stands: on the default branch, in the open wiki
/// pull request, or not written yet. Fetches first, so a merge on GitHub
/// shows.
fn workflow_state(repo: &Path) -> RepoWikiWorkflowState {
    let mut command = git(repo);
    command.args(["fetch", "--quiet", "origin"]);
    if let Err(error) = run(command, "git fetch origin") {
        log::warn!("[repo_wiki] {error}");
    }
    let exists = |tree: &str| {
        let mut command = git(repo);
        command.args(["cat-file", "-e", &format!("{tree}:{WORKFLOW_PATH}")]);
        command.stdout(Stdio::null()).stderr(Stdio::null());
        command.status().is_ok_and(|status| status.success())
    };
    let base = repo_wiki::remote_default_branch(repo);
    if exists(&format!("origin/{base}")) {
        RepoWikiWorkflowState::InRepo
    } else if exists(&format!("origin/{REPO_WIKI_BRANCH}")) {
        RepoWikiWorkflowState::InPr
    } else {
        RepoWikiWorkflowState::Missing
    }
}

/// What a daily update on GitHub still needs, and its last run. Blocking:
/// runs git and gh. A setting gh cannot read is `None`, not `false`.
pub fn status(binding: &RepoWikiBinding, gh: Option<&Path>) -> RepoWikiGithubStatus {
    let repo = slug_of(binding).ok();
    let workflow = workflow_state(Path::new(&binding.repo_root));
    let gh = gh.filter(|gh| gh_signed_in(gh));
    let mut out = RepoWikiGithubStatus {
        actions_url: repo
            .as_ref()
            .map(|slug| format!("https://github.com/{slug}/actions/workflows/wenlan-wiki.yml")),
        repo: repo.clone(),
        gh_ready: gh.is_some(),
        workflow,
        secret: None,
        can_open_prs: None,
        last_run: None,
    };
    let (Some(gh), Some(slug)) = (gh, repo.as_deref()) else {
        return out;
    };
    let agent = binding
        .daily
        .as_ref()
        .map_or(RepoWikiAgent::Claude, |daily| daily.agent);
    out.secret =
        gh_json(gh, &["secret", "list", "--repo", slug, "--json", "name"]).and_then(|list| {
            let names: Vec<String> = list
                .as_array()?
                .iter()
                .filter_map(|secret| secret.get("name")?.as_str().map(str::to_owned))
                .collect();
            Some(
                secrets_for(agent)
                    .iter()
                    .any(|name| names.iter().any(|have| have == name)),
            )
        });
    out.can_open_prs = gh_json(
        gh,
        &["api", &format!("repos/{slug}/actions/permissions/workflow")],
    )
    .and_then(|settings| settings.get("can_approve_pull_request_reviews")?.as_bool());
    if workflow != RepoWikiWorkflowState::Missing {
        out.last_run = last_run(gh, slug);
    }
    out
}

fn last_run(gh: &Path, slug: &str) -> Option<RepoWikiGithubRun> {
    let runs = gh_json(
        gh,
        &[
            "run",
            "list",
            "--repo",
            slug,
            "--workflow",
            "wenlan-wiki.yml",
            "--limit",
            "1",
            "--json",
            "createdAt,url,status,conclusion",
        ],
    )?;
    let run = runs.as_array()?.first()?;
    let text = |key: &str| {
        run.get(key)
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let pr_url = gh_json(
        gh,
        &[
            "pr",
            "list",
            "--repo",
            slug,
            "--head",
            repo_wiki_ci::REFRESH_BRANCH,
            "--state",
            "all",
            "--limit",
            "1",
            "--json",
            "url",
        ],
    )
    .and_then(|prs| {
        prs.as_array()?
            .first()?
            .get("url")?
            .as_str()
            .map(str::to_owned)
    });
    Some(RepoWikiGithubRun {
        at: text("createdAt")?,
        url: text("url")?,
        status: text("status").unwrap_or_else(|| "completed".to_owned()),
        conclusion: text("conclusion"),
        pr_url,
    })
}

/// Let GitHub Actions open pull requests in the bound repository, keeping
/// its default token permission as it is.
pub fn allow_pull_requests(binding: &RepoWikiBinding, gh: &Path) -> Result<(), WenlanError> {
    let slug = slug_of(binding)?;
    let path = format!("repos/{slug}/actions/permissions/workflow");
    let current = gh_json(gh, &["api", &path])
        .and_then(|settings| {
            settings
                .get("default_workflow_permissions")?
                .as_str()
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "read".to_owned());
    gh_output(
        gh,
        &[
            "api",
            "--method",
            "PUT",
            &path,
            "-f",
            &format!("default_workflow_permissions={current}"),
            "-F",
            "can_approve_pull_request_reviews=true",
        ],
    )?;
    Ok(())
}

/// Store `value` as the secret `name` of the bound repository, passing it to
/// gh on stdin so it never appears in a process list.
fn set_secret(
    binding: &RepoWikiBinding,
    gh: &Path,
    name: &str,
    value: &str,
) -> Result<(), WenlanError> {
    let slug = slug_of(binding)?;
    let mut child = Command::new(gh)
        .args(["secret", "set", name, "--repo", &slug])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| WenlanError::Generic(format!("gh secret set: {error}")))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(value.as_bytes())?;
    }
    let output = child
        .wait_with_output()
        .map_err(|error| WenlanError::Generic(format!("gh secret set: {error}")))?;
    if !output.status.success() {
        return Err(WenlanError::Generic(format!(
            "gh secret set {name} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

/// Store an OpenAI API key for a Codex run.
pub fn store_openai_key(
    binding: &RepoWikiBinding,
    gh: &Path,
    key: &str,
) -> Result<(), WenlanError> {
    let key = key.trim();
    if key.is_empty() || key.chars().any(char::is_whitespace) {
        return Err(WenlanError::Validation(
            "paste the whole OpenAI API key".to_owned(),
        ));
    }
    set_secret(binding, gh, OPENAI_SECRET, key)
}

static TOKEN_RE: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"sk-ant-oat[0-9A-Za-z_-]+").expect("valid"));
static ANSI_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"\x1b(?:\[[0-?]*[ -/]*[@-~]|\][^\x07\x1b]*(?:\x07|\x1b\\)|[@-Z\\-_])")
        .expect("valid")
});

/// The complete token in what `claude setup-token` printed so far: the
/// match must be followed by something, so a token still being written is
/// not taken half way.
pub fn token_in(output: &str) -> Option<String> {
    let text = ANSI_RE.replace_all(output, "");
    let found = TOKEN_RE.find(&text)?;
    (found.end() < text.len()).then(|| found.as_str().to_owned())
}

/// Sign in to Claude in the browser with `claude setup-token` and store the
/// token it prints as the repository secret [`CLAUDE_SECRET`]. The command
/// prints only to a terminal, so it runs under `script`, on a wide one so
/// the token is not wrapped; it is stopped once the token is read.
#[cfg(unix)]
pub fn store_claude_token(
    binding: &RepoWikiBinding,
    gh: &Path,
    claude: &Path,
) -> Result<(), WenlanError> {
    slug_of(binding)?;
    let inner = "stty cols 1000 rows 50 2>/dev/null; exec \"$WENLAN_CLAUDE_BIN\" setup-token";
    let mut command = Command::new("script");
    if cfg!(target_os = "macos") {
        command.args(["-q", "/dev/null", "/bin/sh", "-c", inner]);
    } else {
        command.args(["-qfec", inner, "/dev/null"]);
    }
    let mut child = command
        .env("WENLAN_CLAUDE_BIN", claude)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| WenlanError::Generic(format!("could not start `script`: {error}")))?;
    let stdin = child.stdin.take();
    let mut stdout = child.stdout.take().expect("piped stdout");
    let (sender, receiver) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        while let Ok(read) = stdout.read(&mut buffer) {
            if read == 0 || sender.send(buffer[..read].to_vec()).is_err() {
                break;
            }
        }
    });
    let started = Instant::now();
    let mut printed = Vec::new();
    let token = loop {
        match receiver.recv_timeout(Duration::from_millis(250)) {
            Ok(bytes) => printed.extend(bytes),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                break token_in(&String::from_utf8_lossy(&printed));
            }
        }
        if let Some(token) = token_in(&String::from_utf8_lossy(&printed)) {
            break Some(token);
        }
        if started.elapsed() > SIGN_IN_TIMEOUT {
            break None;
        }
    };
    // It waits for input after printing the token; nothing else is needed.
    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();
    printed.fill(0);
    let Some(token) = token else {
        return Err(WenlanError::Validation(
            "the Claude sign-in was not finished; try again and confirm in the browser".to_owned(),
        ));
    };
    set_secret(binding, gh, CLAUDE_SECRET, &token)
}

/// Windows has no `script`: the user runs `claude setup-token` in a
/// terminal and adds the secret on GitHub.
#[cfg(not(unix))]
pub fn store_claude_token(
    _binding: &RepoWikiBinding,
    _gh: &Path,
    _claude: &Path,
) -> Result<(), WenlanError> {
    Err(WenlanError::Validation(format!(
        "run `claude setup-token` in a terminal, then add what it prints as the repository \
         secret {CLAUDE_SECRET} on GitHub"
    )))
}

#[cfg(test)]
#[path = "repo_wiki_github_test.rs"]
mod tests;
