// SPDX-License-Identifier: Apache-2.0
//! `wenlan repo` (earlier `wenlan wiki`) — put a Space's code pages into its git repository.
//!
//! `bind` ties the Space to the repository the command runs in (or
//! `--repo`), `publish` writes the pages into the repository's wiki folder on
//! the `wenlan/wiki` branch and opens or updates the pull request, `status`
//! shows the binding and the last publish, `unbind` removes the binding.

use anyhow::{bail, ensure, Result};
use clap::{Subcommand, ValueEnum};
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use wenlan_core::repo_wiki::{self, PublishTarget};
use wenlan_core::skill_registry::printable;
use wenlan_types::repo_wiki::{
    BindRepoWikiRequest, ProjectRuleProposal, RepoDocsRequest, RepoWikiAgent, RepoWikiBinding,
    RepoWikiDaily, RepoWikiPublishResult, RepoWikiPublishStatus, RepoWikiRunner,
};

use super::rulesync::{file_label, terminal_error, terminal_json};
use super::skills::{one_line, person_says_yes};
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

#[derive(Clone, Copy, ValueEnum)]
pub enum ProjectRulesAction {
    Accept,
    Dismiss,
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
        /// the remote URL). Needed when it is not yours and GitHub does not let
        /// you push to it.
        #[arg(long, value_name = "OWNER/NAME")]
        confirm_repo: Option<String>,
        /// Publish from this computer although another Wenlan (a teammate's,
        /// or another computer of yours) keeps this repository's pages and
        /// rules. The next publish replaces theirs.
        #[arg(long)]
        take_over: bool,
    },
    /// Write the Space's code pages into the repository and open or update
    /// the pull request.
    Publish,
    /// Show the repository the Space is bound to and the last publish.
    Status,
    /// Stop putting this Space's pages into its repository.
    Unbind,
    /// Show repository rule changes waiting for review in this Space.
    RulesProposals,
    /// Review a repository rule change and decide whether to keep it in this Space.
    RulesReview {
        /// The proposal ID from `wenlan wiki rules-proposals`.
        id: String,
        #[arg(value_enum)]
        action: ProjectRulesAction,
    },
    /// Prepare readable rules and project skills for this checkout in Wenlan's data directory.
    RulesCheckout {
        /// A directory inside the checkout. Defaults to the current directory.
        #[arg(long)]
        repo: Option<PathBuf>,
    },
    /// Include one team Rules layer in future wiki pull requests, or turn it off.
    RulesTeam {
        /// The team's name or ID, or `off`.
        team: String,
    },
    /// Propose the next Rules page for a Space hosted on this computer.
    SharedRulesPropose { file: PathBuf },
    /// List Rules proposals for a Space hosted on this computer.
    SharedRulesProposals,
    /// Review a Rules proposal in a Space hosted on this computer.
    SharedRulesReview {
        id: String,
        #[arg(value_enum)]
        action: ProjectRulesAction,
    },
    /// Make docs of the bound repository follow a page of the Space: when
    /// the page changes, the AI tool suggests fixes to them for the wiki
    /// pull request. `wenlan follow README.md <page>` is the short way.
    #[command(hide = true)]
    Follow {
        /// The page the docs follow (its id).
        #[arg(long)]
        page: String,
        /// Docs relative to the repository root, such as README.md.
        #[arg(required = true)]
        files: Vec<String>,
    },
    /// Stop the docs following a page.
    #[command(hide = true)]
    Unfollow {
        /// The page the docs follow (its id).
        #[arg(long)]
        page: String,
    },
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

    pub fn is_project_rules(&self) -> bool {
        matches!(
            self,
            Self::RulesProposals
                | Self::RulesReview { .. }
                | Self::RulesTeam { .. }
                | Self::RulesCheckout { .. }
                | Self::SharedRulesPropose { .. }
                | Self::SharedRulesProposals
                | Self::SharedRulesReview { .. }
        )
    }

    pub fn is_rules_checkout(&self) -> bool {
        matches!(self, Self::RulesCheckout { .. })
    }

    /// Preflight before the CLI contacts the daemon for the Space registry.
    pub fn require_project_rules_caller(&self, is_ai_caller: bool) -> Result<()> {
        ensure!(
            !is_ai_caller,
            "Only you can view and review repository rule proposals. Run this outside an AI session."
        );
        ensure!(
            !matches!(
                self,
                Self::RulesReview { .. } | Self::SharedRulesReview { .. } | Self::RulesTeam { .. }
            ) || (io::stdin().is_terminal() && io::stdout().is_terminal()),
            "Run this command in your own terminal and type yes when asked."
        );
        Ok(())
    }
}

pub async fn run(
    client: &WenlanClient,
    format: ResolvedFormat,
    quiet: bool,
    space: Option<&str>,
    command: WikiCommand,
) -> Result<()> {
    if let WikiCommand::RulesCheckout { repo } = command {
        let repo = std::path::absolute(repo.unwrap_or(std::env::current_dir()?))?;
        let receipt = client.checkout_rules(repo).await?;
        if !quiet {
            match format {
                ResolvedFormat::Json => print_json(&receipt)?,
                ResolvedFormat::Table => {
                    println!(
                        "Checkout files in {}",
                        file_label(&receipt.directory.display().to_string())
                    );
                    if !receipt.report.notice_codes.is_empty() {
                        println!("{}", printable(&receipt.report.message));
                    }
                    for blocked in receipt.report.blocked {
                        eprintln!("{}", terminal_error(anyhow::anyhow!(blocked)));
                    }
                }
            }
        }
        return Ok(());
    }
    // `publish` in a repository that is not set up cannot do its work: a
    // person is asked once whether to set it up now (see `set_up_to_publish`).
    let set_up;
    let space = match (space, &command) {
        (Some(space), WikiCommand::Publish)
            if client.repo_wiki_binding(space).await?.is_none() =>
        {
            match super::init::set_up_to_publish(client, format, quiet).await? {
                Some(space) => {
                    set_up = space;
                    set_up.as_str()
                }
                None => return Ok(()),
            }
        }
        (Some(space), _) => space,
        (None, WikiCommand::Publish) => {
            match super::init::set_up_to_publish(client, format, quiet).await? {
                Some(space) => {
                    set_up = space;
                    set_up.as_str()
                }
                None => return Ok(()),
            }
        }
        (None, _) => bail!(
            "no Space for this folder yet. Inside a git repository, `wenlan init` sets one up; or pass --space"
        ),
    };
    match command {
        WikiCommand::Bind {
            repo,
            folder,
            daily,
            agent,
            on_github,
            confirm_repo,
            take_over,
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
                        take_over,
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
                            "Space '{space}' is not bound to a repository. `wenlan repo bind` ties it to this one."
                        ),
                    }
                }
                Ok(())
            }
        },
        WikiCommand::RulesProposals => {
            list_project_rule_proposals(client, format, quiet, space).await
        }
        WikiCommand::RulesReview { id, action } => {
            review_project_rule_proposal(client, format, quiet, space, &id, action).await
        }
        WikiCommand::RulesTeam { team } => {
            let choice = (team != "off").then_some(team);
            if person_says_yes(
                &mut io::stdin().lock(),
                &mut io::stdout(),
                "Change which team Rules are published in this repository?",
            )? {
                let reply = client.use_team_rules_in_repo(space, choice).await?;
                if !quiet {
                    match format {
                        ResolvedFormat::Json => print_json(&reply)?,
                        ResolvedFormat::Table => {
                            println!("Updated this repository's team rules preference.")
                        }
                    }
                }
            }
            Ok(())
        }
        WikiCommand::SharedRulesPropose { file } => {
            let markdown = std::fs::read_to_string(file)?;
            let id = client.propose_shared_rules(space, markdown).await?;
            if !quiet {
                match format {
                    ResolvedFormat::Json => print_json(&id)?,
                    ResolvedFormat::Table => {
                        println!("Rules proposal {} is awaiting review.", one_line(&id))
                    }
                }
            }
            Ok(())
        }
        WikiCommand::SharedRulesProposals => {
            let pending = client.shared_rule_proposals(space).await?;
            if !quiet {
                match format {
                    ResolvedFormat::Json => print_json(&pending)?,
                    ResolvedFormat::Table => {
                        for proposal in pending {
                            println!(
                                "{}  {}",
                                one_line(&proposal.id),
                                one_line(&proposal.proposed_at)
                            );
                        }
                    }
                }
            }
            Ok(())
        }
        WikiCommand::SharedRulesReview { id, action } => {
            let proposal = client
                .shared_rule_proposals(space)
                .await?
                .into_iter()
                .find(|proposal| proposal.id == id)
                .ok_or_else(|| anyhow::anyhow!("No pending Rules proposal with that ID"))?;
            println!("{}", terminal_json(&proposal.markdown)?);
            if person_says_yes(
                &mut io::stdin().lock(),
                &mut io::stdout(),
                match action {
                    ProjectRulesAction::Accept => "Accept these shared rules?",
                    ProjectRulesAction::Dismiss => "Dismiss these shared rules?",
                },
            )? {
                client
                    .decide_shared_rules(space, &id, matches!(action, ProjectRulesAction::Accept))
                    .await?;
            }
            Ok(())
        }
        WikiCommand::RulesCheckout { .. } => {
            unreachable!("checkout rules run without a Space selector")
        }
        WikiCommand::Refresh(_) | WikiCommand::Workflow(_) => {
            unreachable!("offline wiki commands run before the daemon is reached")
        }
        WikiCommand::Follow { page, files } => {
            let reply = client
                .repo_wiki_docs(
                    space,
                    &RepoDocsRequest::Follow {
                        page_id: page,
                        files,
                    },
                )
                .await?;
            if !quiet {
                match format {
                    ResolvedFormat::Json => print_json(&reply)?,
                    ResolvedFormat::Table => {
                        if let Some(follow) = &reply.follow {
                            println!(
                                "{} now follow page {}.",
                                follow.files.join(", "),
                                follow.page_id
                            );
                        }
                    }
                }
            }
            Ok(())
        }
        WikiCommand::Unfollow { page } => {
            let reply = client
                .repo_wiki_docs(
                    space,
                    &RepoDocsRequest::Unfollow {
                        page_id: page.clone(),
                    },
                )
                .await?;
            if !quiet {
                match format {
                    ResolvedFormat::Json => print_json(&reply)?,
                    ResolvedFormat::Table => println!("No docs follow page {page} any more."),
                }
            }
            Ok(())
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

async fn list_project_rule_proposals(
    client: &WenlanClient,
    format: ResolvedFormat,
    quiet: bool,
    space: &str,
) -> Result<()> {
    let proposals = client
        .project_rule_proposals(space)
        .await
        .map_err(terminal_error)?;
    if !quiet {
        match format {
            ResolvedFormat::Json => println!("{}", terminal_json(&proposals)?),
            ResolvedFormat::Table if proposals.is_empty() => {
                println!("No repository rule proposals are waiting for review.");
            }
            ResolvedFormat::Table => {
                for proposal in proposals {
                    println!("{}", project_proposal_for_a_person(&proposal));
                }
            }
        }
    }
    Ok(())
}

async fn review_project_rule_proposal(
    client: &WenlanClient,
    format: ResolvedFormat,
    quiet: bool,
    space: &str,
    id: &str,
    action: ProjectRulesAction,
) -> Result<()> {
    let proposal = client
        .project_rule_proposals(space)
        .await
        .map_err(terminal_error)?
        .into_iter()
        .find(|proposal| proposal.id == id)
        .ok_or_else(|| {
            anyhow::anyhow!("This proposal is no longer waiting for review in this Space.")
        })?;
    print!("{}", project_proposal_for_a_person(&proposal));
    let accept = matches!(action, ProjectRulesAction::Accept);
    ensure!(
        !accept || proposal.blocked.is_none(),
        "Use `dismiss` to close this blocked proposal."
    );
    let question = if accept {
        format!(
            "Accept this change to the rules in Space {:?}?",
            one_line(space)
        )
    } else {
        "Dismiss this proposal?".to_owned()
    };
    if !person_says_yes(&mut io::stdin().lock(), &mut io::stdout(), &question)? {
        println!("Cancelled.");
        return Ok(());
    }
    let result = client
        .decide_project_rule_proposal(space, &proposal.id, accept)
        .await
        .map_err(terminal_error)?;
    if !quiet {
        match format {
            ResolvedFormat::Json => println!("{}", terminal_json(&result)?),
            ResolvedFormat::Table => {
                let word = if accept { "Accepted" } else { "Dismissed" };
                println!("{word} proposal {}.", one_line(&result.id));
            }
        }
    }
    Ok(())
}

fn project_proposal_for_a_person(proposal: &ProjectRuleProposal) -> String {
    let mut out = format!(
        "Proposal {} for {}\nRepository revision {}\n",
        one_line(&proposal.id),
        file_label(&proposal.path),
        one_line(&proposal.revision),
    );
    if let Some(reason) = &proposal.blocked {
        out.push_str(&format!("Blocked {}\n", one_line(reason)));
    }
    if !proposal.diff.is_empty() {
        out.push_str(&printable(&proposal.diff));
        if !proposal.diff.ends_with('\n') {
            out.push('\n');
        }
    }
    out
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
    if target.ours() {
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
    for follow in &binding.follows {
        println!(
            "  follows page {}: {}{}",
            follow.page_id,
            follow.files.join(", "),
            match follow.followed_version {
                Some(version) => format!(" (up to version {version})"),
                None => " (not run yet)".to_owned(),
            }
        );
    }
    if let Some(last) = &binding.last_publish {
        println!("  last publish: {}", describe_publish(last));
    }
    Ok(())
}

pub(crate) fn describe_publish(result: &RepoWikiPublishResult) -> String {
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
        // No pages went: the pull request carries the project rules for
        // `AGENTS.md`, fixes to the repository's own docs, or both.
        RepoWikiPublishStatus::PrOpened | RepoWikiPublishStatus::PrUpdated if result.pages == 0 => {
            format!(
                "no pages; pull request {} with the project rules or doc fixes: {}{left_out}",
                if result.status == RepoWikiPublishStatus::PrOpened {
                    "opened"
                } else {
                    "updated"
                },
                result.pr_url.as_deref().unwrap_or("(no link)")
            )
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

    #[test]
    fn project_proposal_shows_escaped_path_and_diff() {
        let proposal = ProjectRuleProposal {
            id: "card".into(),
            path: "folder\nAGENTS.md".into(),
            revision: "git-revision".into(),
            diff: "-before\n+after\u{1b}[2J".into(),
            added_rules: vec![],
            removed_rules: vec![],
            blocked: None,
        };
        let text = project_proposal_for_a_person(&proposal);
        assert!(text.contains("Proposal card for folder<U+000A>AGENTS.md"));
        assert!(text.contains("Repository revision git-revision"));
        assert!(text.contains("-before\n+after<U+001B>[2J\n"));
        assert_eq!(printable(&text), text);
    }

    fn target(slug: Option<&str>, login: Option<&str>) -> PublishTarget {
        PublishTarget {
            remote_url: slug.map(|slug| format!("https://github.com/{slug}.git")),
            slug: slug.map(str::to_owned),
            signed_in_as: login.map(str::to_owned),
            can_push: false,
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
    fn binding_your_teams_repo_just_works() {
        let team = PublishTarget {
            can_push: true,
            ..target(Some("acme/api"), Some("lucian"))
        };
        check_target(&team, None).unwrap();
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
            can_push: false,
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
            follows: Vec::new(),
            take_over: false,
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
