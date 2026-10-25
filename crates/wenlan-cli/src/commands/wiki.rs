// SPDX-License-Identifier: Apache-2.0
//! `wenlan wiki` — put a Space's code pages into its git repository.
//!
//! `bind` ties the Space to the repository the command runs in (or
//! `--repo`), `publish` writes the pages into the repository's wiki folder on
//! the `wenlan/wiki` branch and opens or updates the pull request, `status`
//! shows the binding and the last publish, `unbind` removes the binding.

use anyhow::{bail, Result};
use clap::{Subcommand, ValueEnum};
use std::path::{Path, PathBuf};
use wenlan_core::repo_wiki::{self, PublishTarget};
use wenlan_types::repo_wiki::{
    BindRepoWikiRequest, RepoWikiAgent, RepoWikiBinding, RepoWikiDaily, RepoWikiPublishResult,
    RepoWikiPublishStatus, RepoWikiRunner,
};

use crate::client::WenlanClient;
use crate::output::{print_json, ResolvedFormat};

#[derive(Clone, Copy, ValueEnum)]
pub enum AgentArg {
    Claude,
    Codex,
    Cursor,
    Copilot,
}

impl From<AgentArg> for RepoWikiAgent {
    fn from(agent: AgentArg) -> Self {
        match agent {
            AgentArg::Claude => RepoWikiAgent::Claude,
            AgentArg::Codex => RepoWikiAgent::Codex,
            AgentArg::Cursor => RepoWikiAgent::Cursor,
            AgentArg::Copilot => RepoWikiAgent::Copilot,
        }
    }
}

#[derive(Subcommand)]
pub enum WikiCommand {
    /// Tie the Space to this git repository (or --repo), so its code pages
    /// can go into the repository as a pull request.
    Bind {
        /// Any folder inside the repository. Default: the current folder.
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Folder inside the repository for the pages. Default: wiki.
        #[arg(long)]
        folder: Option<String>,
        /// Bring the wiki up to date by itself every day after this local
        /// hour (0-23), or `off`.
        #[arg(long, value_name = "HOUR|off")]
        daily: Option<String>,
        /// The AI tool the daily run starts. Default: claude.
        #[arg(long, value_enum, requires = "daily")]
        agent: Option<AgentArg>,
        /// Run the daily update on GitHub Actions instead of this computer:
        /// the next publish adds the workflow to the pull request.
        #[arg(long, requires = "daily")]
        on_github: bool,
        /// The repository pull requests go to (`owner/name` on GitHub, else
        /// the remote URL). Needed when it is not yours.
        #[arg(long, value_name = "OWNER/NAME")]
        confirm_repo: Option<String>,
    },
    /// Write the Space's code pages into the repository and open or update
    /// the pull request.
    Publish,
    /// Show the repository the Space is bound to and the last publish.
    Status,
    /// Stop putting this Space's pages into its repository.
    Unbind,
    /// Check the wiki folder of this checkout against the code: fix line
    /// numbers that moved and have an AI tool rewrite pages whose code
    /// changed. Works without Wenlan running; the GitHub Actions workflow
    /// runs it.
    Refresh(super::wiki_refresh::RefreshArgs),
    /// Print, or `--write`, the GitHub Actions workflow that runs `refresh`
    /// every day and opens a pull request when a page changed.
    Workflow(super::wiki_refresh::WorkflowArgs),
}

impl WikiCommand {
    /// Runs from the checkout alone, without Wenlan's daemon or a Space.
    pub fn is_offline(&self) -> bool {
        matches!(self, WikiCommand::Refresh(_) | WikiCommand::Workflow(_))
    }
}

pub async fn run(
    client: &WenlanClient,
    format: ResolvedFormat,
    quiet: bool,
    space: Option<&str>,
    command: WikiCommand,
) -> Result<()> {
    let Some(space) = space else {
        bail!(
            "no Space for this folder; run it inside a repository matched to a Space, or pass --space"
        );
    };
    match command {
        WikiCommand::Bind {
            repo,
            folder,
            daily,
            agent,
            on_github,
            confirm_repo,
        } => {
            let repo = match repo {
                Some(repo) => repo,
                None => std::env::current_dir()?,
            };
            let repo = std::path::absolute(repo)?;
            let top = repo_wiki::repository_top_level(&repo)?;
            let target = repo_wiki::publish_target(&top, repo_wiki::find_gh().as_deref());
            let existing = client.repo_wiki_binding(space).await?;
            if !already_agreed(existing.as_ref(), &top, &target) {
                check_target(&target, confirm_repo.as_deref())?;
            }
            let daily = parse_daily(daily.as_deref(), agent, on_github)?;
            let binding = client
                .bind_repo_wiki(
                    space,
                    &BindRepoWikiRequest {
                        repo_root: repo.to_string_lossy().into_owned(),
                        folder,
                        daily,
                    },
                )
                .await?;
            print_binding(format, quiet, &binding)
        }
        WikiCommand::Publish => {
            let result = client.publish_repo_wiki(space).await?;
            if !quiet {
                match format {
                    ResolvedFormat::Json => print_json(&result)?,
                    ResolvedFormat::Table => println!("{}", describe_publish(&result)),
                }
            }
            Ok(())
        }
        WikiCommand::Status => match client.repo_wiki_binding(space).await? {
            Some(binding) => print_binding(format, quiet, &binding),
            None => {
                if !quiet {
                    match format {
                        ResolvedFormat::Json => print_json(&serde_json::Value::Null)?,
                        ResolvedFormat::Table => println!(
                            "Space '{space}' is not bound to a repository. `wenlan wiki bind` ties it to this one."
                        ),
                    }
                }
                Ok(())
            }
        },
        WikiCommand::Refresh(_) | WikiCommand::Workflow(_) => {
            unreachable!("offline wiki commands run before the daemon is reached")
        }
        WikiCommand::Unbind => {
            let removed = client.unbind_repo_wiki(space).await?;
            if !quiet {
                match format {
                    ResolvedFormat::Json => print_json(&serde_json::json!({ "removed": removed }))?,
                    ResolvedFormat::Table if removed => {
                        println!("Space '{space}' no longer puts its pages into a repository.")
                    }
                    ResolvedFormat::Table => println!("Space '{space}' was not bound."),
                }
            }
            Ok(())
        }
    }
}

/// Whether the Space is already bound to this repository with this `origin`:
/// changing its folder or daily setting needs no new agreement.
fn already_agreed(existing: Option<&RepoWikiBinding>, top: &Path, target: &PublishTarget) -> bool {
    existing.is_some_and(|binding| {
        Path::new(&binding.repo_root) == top
            && match (&binding.remote_url, &target.remote_url) {
                (Some(bound), Some(now)) => repo_wiki::same_remote(bound, now),
                (None, None) => true,
                _ => false,
            }
    })
}

/// Refuse to bind a repository the user did not name when it is not theirs:
/// a publish pushes a branch there and opens a pull request.
fn check_target(target: &PublishTarget, confirm: Option<&str>) -> Result<()> {
    let Some(name) = target.name() else {
        // No `origin`: a publish has nowhere to go and says so.
        return Ok(());
    };
    if let Some(confirm) = confirm {
        if target.confirmed_by(confirm) {
            return Ok(());
        }
        bail!(
            "--confirm-repo names {confirm}, but pull requests would go to {name}; nothing was bound"
        );
    }
    if target.owned() {
        return Ok(());
    }
    let whose = match (target.owner(), target.signed_in_as.as_deref()) {
        (Some(owner), Some(login)) => {
            format!("github.com/{name}, which belongs to {owner}, not to you (signed in to GitHub as {login})")
        }
        (Some(_), None) => format!(
            "github.com/{name}, and Wenlan cannot tell whether it is yours (the GitHub CLI is not signed in)"
        ),
        _ => name.to_owned(),
    };
    bail!(
        "pull requests from this Space would open on {whose}; nothing was bound. \
         If that is what you want, run it again with --confirm-repo {name}"
    )
}

fn parse_daily(
    daily: Option<&str>,
    agent: Option<AgentArg>,
    on_github: bool,
) -> Result<Option<RepoWikiDaily>> {
    let Some(daily) = daily else {
        return Ok(None);
    };
    let agent = agent.map(Into::into).unwrap_or(RepoWikiAgent::Claude);
    let runs_on = if on_github {
        RepoWikiRunner::Github
    } else {
        RepoWikiRunner::Computer
    };
    if daily.eq_ignore_ascii_case("off") {
        return Ok(Some(RepoWikiDaily {
            enabled: false,
            hour: 9,
            agent,
            runs_on,
        }));
    }
    let hour: u8 = daily
        .trim()
        .parse()
        .ok()
        .filter(|hour| *hour <= 23)
        .ok_or_else(|| anyhow::anyhow!("--daily takes an hour from 0 to 23, or off"))?;
    Ok(Some(RepoWikiDaily {
        enabled: true,
        hour,
        agent,
        runs_on,
    }))
}

fn print_binding(format: ResolvedFormat, quiet: bool, binding: &RepoWikiBinding) -> Result<()> {
    if quiet {
        return Ok(());
    }
    if matches!(format, ResolvedFormat::Json) {
        return print_json(binding);
    }
    println!(
        "Space '{}' → {} (folder {}/)",
        binding.space, binding.repo_root, binding.folder
    );
    if let Some(remote) = &binding.remote_url {
        println!("  remote: {remote}");
    }
    match &binding.daily {
        Some(daily) if daily.enabled => println!(
            "  daily: after {:02}:00, with {}, on {}",
            daily.hour,
            daily.agent.display_name(),
            match daily.runs_on {
                RepoWikiRunner::Computer => "this computer",
                RepoWikiRunner::Github => "GitHub",
            }
        ),
        _ => println!("  daily: off"),
    }
    if let Some(last) = &binding.last_publish {
        println!("  last publish: {}", describe_publish(last));
    }
    Ok(())
}

fn describe_publish(result: &RepoWikiPublishResult) -> String {
    let mut left_out = if result.left_out > 0 {
        format!(
            "; {} page(s) that rest on personal notes stay out",
            result.left_out
        )
    } else {
        String::new()
    };
    if result.waiting > 0 {
        left_out.push_str(&format!(
            "; {} page(s) cite code not pushed to the default branch yet and wait for it",
            result.waiting
        ));
    }
    match result.status {
        RepoWikiPublishStatus::NothingToPublish => {
            format!("no page of this Space rests on this repository's code yet{left_out}")
        }
        RepoWikiPublishStatus::NoChanges => {
            format!("{} page(s), already up to date{left_out}", result.pages)
        }
        RepoWikiPublishStatus::PrOpened => format!(
            "{} page(s), pull request opened: {}{left_out}",
            result.pages,
            result.pr_url.as_deref().unwrap_or("(no link)")
        ),
        RepoWikiPublishStatus::PrUpdated => format!(
            "{} page(s), pull request updated: {}{left_out}",
            result.pages,
            result.pr_url.as_deref().unwrap_or("(no link)")
        ),
        RepoWikiPublishStatus::Pushed => match &result.compare_url {
            Some(url) => format!(
                "{} page(s) pushed to {}; open the pull request here: {url}{left_out}",
                result.pages,
                result.branch.as_deref().unwrap_or("wenlan/wiki")
            ),
            None => format!(
                "{} page(s) pushed to {}{left_out}",
                result.pages,
                result.branch.as_deref().unwrap_or("wenlan/wiki")
            ),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(slug: Option<&str>, login: Option<&str>) -> PublishTarget {
        PublishTarget {
            remote_url: slug.map(|slug| format!("https://github.com/{slug}.git")),
            slug: slug.map(str::to_owned),
            signed_in_as: login.map(str::to_owned),
        }
    }

    #[test]
    fn binding_someone_elses_repo_needs_it_named() {
        let theirs = target(Some("pallets/itsdangerous"), Some("lucian"));
        let error = check_target(&theirs, None).unwrap_err().to_string();
        assert!(error.contains("github.com/pallets/itsdangerous"), "{error}");
        assert!(error.contains("belongs to pallets"), "{error}");
        assert!(
            error.contains("--confirm-repo pallets/itsdangerous"),
            "{error}"
        );
        check_target(&theirs, Some("pallets/itsdangerous")).unwrap();
        check_target(&theirs, Some("https://github.com/Pallets/itsdangerous")).unwrap();
        let wrong = check_target(&theirs, Some("lucian/itsdangerous"))
            .unwrap_err()
            .to_string();
        assert!(wrong.contains("nothing was bound"), "{wrong}");
    }

    #[test]
    fn binding_your_own_repo_or_one_without_origin_just_works() {
        check_target(&target(Some("Lucian/site"), Some("lucian")), None).unwrap();
        check_target(&target(None, None), None).unwrap();
        let unknown = check_target(&target(Some("lucian/site"), None), None)
            .unwrap_err()
            .to_string();
        assert!(unknown.contains("not signed in"), "{unknown}");
        let elsewhere = PublishTarget {
            remote_url: Some("git@gitlab.com:lucian/site.git".into()),
            slug: None,
            signed_in_as: None,
        };
        assert!(check_target(&elsewhere, None).is_err());
        check_target(&elsewhere, Some("git@gitlab.com:lucian/site.git")).unwrap();
    }

    #[test]
    fn rebinding_the_same_repo_needs_no_new_agreement() {
        let binding = |remote: Option<&str>| RepoWikiBinding {
            space: "s".into(),
            repo_root: "/r".into(),
            folder: "wiki".into(),
            remote_url: remote.map(str::to_owned),
            daily: None,
            last_publish: None,
            last_daily_run: None,
            doc_fixes: Vec::new(),
            doc_check: None,
            kept_edits: Vec::new(),
        };
        let now = target(Some("pallets/itsdangerous"), None);
        let ssh = binding(Some("git@github.com:pallets/itsdangerous.git"));
        assert!(already_agreed(Some(&ssh), Path::new("/r"), &now));
        assert!(!already_agreed(Some(&ssh), Path::new("/other"), &now));
        let moved = binding(Some("https://github.com/lucian/itsdangerous"));
        assert!(!already_agreed(Some(&moved), Path::new("/r"), &now));
        assert!(!already_agreed(None, Path::new("/r"), &now));
    }

    #[test]
    fn daily_takes_an_hour_or_off() {
        let on = parse_daily(Some("9"), None, false).unwrap().unwrap();
        assert!(on.enabled);
        assert_eq!(on.hour, 9);
        assert_eq!(on.agent, RepoWikiAgent::Claude);
        assert_eq!(on.runs_on, RepoWikiRunner::Computer);
        let off = parse_daily(Some("OFF"), Some(AgentArg::Codex), true)
            .unwrap()
            .unwrap();
        assert!(!off.enabled);
        assert_eq!(off.agent, RepoWikiAgent::Codex);
        assert_eq!(off.runs_on, RepoWikiRunner::Github);
        assert!(parse_daily(Some("24"), None, false).is_err());
        assert!(parse_daily(Some("nine"), None, false).is_err());
        assert!(parse_daily(None, None, false).unwrap().is_none());
    }
}
