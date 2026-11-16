// SPDX-License-Identifier: Apache-2.0
//! `wenlan init` — set a repository up in one step: a Space named after it,
//! the two tied together, and the AI tools on this computer connected. It
//! does what `wenlan spaces add`, `wenlan repo bind` and `wenlan connect` do
//! one by one, and running it again changes nothing that is already set.
//!
//! `print_folder` reads the same picture back for `wenlan status`.

use anyhow::{bail, Result};
use std::collections::HashSet;
use std::io::{IsTerminal, Write};
use std::path::Path;
use wenlan_core::repo_wiki::{self, PublishTarget};
use wenlan_types::folder_space::{self, FolderSpaceHint};
use wenlan_types::repo_wiki::{BindRepoWikiRequest, RepoWikiBinding};

use super::mcp::{ConnectArgs, McpClient};
use crate::client::WenlanClient;
use crate::output::{print_json, ResolvedFormat};

/// What this folder says about the Space: the `~/.wenlan/spaces.toml`
/// mapping and the repository's folder name, the chain every command uses.
fn folder_hint(cwd: &Path) -> FolderSpaceHint {
    let home = dirs::home_dir();
    let spaces_file = home.as_ref().map(|home| home.join(".wenlan/spaces.toml"));
    folder_space::folder_space_hint(cwd, spaces_file.as_deref(), home.as_deref())
}

/// An AI tool `wenlan connect` can set up, found on this computer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Tool {
    client: McpClient,
    /// The name its Wenlan entry reports (`agent_name_for_client`).
    agent: &'static str,
    label: &'static str,
    /// The word after `wenlan connect`.
    connect_as: &'static str,
}

const CLAUDE_CODE: Tool = Tool {
    client: McpClient::ClaudeCode,
    agent: "claude-code",
    label: "Claude Code",
    connect_as: "claude-code",
};
const CODEX: Tool = Tool {
    client: McpClient::Codex,
    agent: "codex",
    label: "Codex",
    connect_as: "codex",
};
const GEMINI: Tool = Tool {
    client: McpClient::Gemini,
    agent: "gemini-cli",
    label: "Gemini CLI",
    connect_as: "gemini",
};
const CURSOR: Tool = Tool {
    client: McpClient::Cursor,
    agent: "cursor",
    label: "Cursor",
    connect_as: "cursor",
};

/// The AI tools installed here: a command on `PATH`, or Cursor's folder in
/// the home folder.
fn installed_tools(on_path: impl Fn(&str) -> bool, home: Option<&Path>) -> Vec<Tool> {
    let mut tools = Vec::new();
    for (binary, tool) in [
        ("claude", CLAUDE_CODE),
        ("codex", CODEX),
        ("gemini", GEMINI),
    ] {
        if on_path(binary) {
            tools.push(tool);
        }
    }
    if home.is_some_and(|home| home.join(".cursor").is_dir()) {
        tools.push(CURSOR);
    }
    tools
}

/// How the Space and this repository stand after `init`.
enum Tie {
    /// Tied by this run.
    Made(RepoWikiBinding),
    /// Was tied to this repository already.
    Already(RepoWikiBinding),
    /// The Space is tied to another working copy; left as it is.
    Elsewhere(RepoWikiBinding),
    /// Pull requests would open on a repository that is not the user's, and
    /// nobody said yes. Holds the repository's name and its owner.
    NotAgreed { name: String, owner: Option<String> },
}

pub async fn run(client: &WenlanClient, format: ResolvedFormat, quiet: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let Ok(top) = repo_wiki::repository_top_level(&cwd) else {
        bail!(
            "this folder is not inside a git repository. `wenlan init` sets up a repository; \
             `wenlan spaces add <name>` makes a Space without one"
        );
    };
    let hint = folder_hint(&cwd);
    let interactive = !quiet
        && matches!(format, ResolvedFormat::Table)
        && std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal();

    // 1. The Space.
    let registered: HashSet<String> = client
        .list_spaces()
        .await?
        .into_iter()
        .map(|space| space.name)
        .collect();
    let (space, created) = match hint.registered(|name| registered.contains(name)) {
        Some(space) => (space.to_owned(), false),
        None => {
            // A folder mapping is read before the repository's name, so a
            // mapping that names a Space nobody made yet decides the name.
            let Some(name) = hint.mapped.clone().or_else(|| hint.repository.clone()) else {
                bail!("could not read this repository's folder name; nothing was changed");
            };
            client.create_space(&name).await?;
            (name, true)
        }
    };

    // 2. The repository.
    let tie = match client.repo_wiki_binding(&space).await? {
        Some(binding) if Path::new(&binding.repo_root) == top => Tie::Already(binding),
        Some(binding) => Tie::Elsewhere(binding),
        None => {
            let target = repo_wiki::publish_target(&top, repo_wiki::find_gh().as_deref());
            if agreed(&target, interactive)? {
                let binding = client
                    .bind_repo_wiki(
                        &space,
                        &BindRepoWikiRequest {
                            repo_root: top.to_string_lossy().into_owned(),
                            folder: None,
                            daily: None,
                        },
                    )
                    .await?;
                Tie::Made(binding)
            } else {
                Tie::NotAgreed {
                    name: target.name().unwrap_or_default().to_owned(),
                    owner: target.owner().map(str::to_owned),
                }
            }
        }
    };

    // 3. The AI tools. A daemon that cannot list them counts as none known.
    let known: HashSet<String> = client
        .list_agents()
        .await
        .map(|agents| agents.into_iter().map(|agent| agent.name).collect())
        .unwrap_or_default();
    let home = dirs::home_dir();
    let installed = installed_tools(|binary| which::which(binary).is_ok(), home.as_deref());
    let mut connected: Vec<Tool> = Vec::new();
    let mut waiting: Vec<Tool> = Vec::new();
    for tool in installed {
        if known.contains(tool.agent) {
            connected.push(tool);
        } else if interactive && ask(&format!("Connect {}?", tool.label), true)? {
            let args = ConnectArgs {
                client: tool.client,
                dry_run: false,
                all_tools: false,
            };
            match super::mcp::run_connect(client, args, quiet).await {
                Ok(()) => connected.push(tool),
                Err(error) => {
                    eprintln!("wenlan: {} was not connected: {error:#}", tool.label);
                    waiting.push(tool);
                }
            }
        } else {
            waiting.push(tool);
        }
    }

    if quiet {
        return Ok(());
    }
    if matches!(format, ResolvedFormat::Json) {
        let (repository, repository_state) = match &tie {
            Tie::Made(binding) => (Some(binding), "tied"),
            Tie::Already(binding) => (Some(binding), "already_tied"),
            Tie::Elsewhere(binding) => (Some(binding), "tied_elsewhere"),
            Tie::NotAgreed { .. } => (None, "not_agreed"),
        };
        return print_json(&serde_json::json!({
            "space": space,
            "created": created,
            "repository_state": repository_state,
            "repository": repository,
            "connected": connected.iter().map(|tool| tool.connect_as).collect::<Vec<_>>(),
            "not_connected": waiting.iter().map(|tool| tool.connect_as).collect::<Vec<_>>(),
        }));
    }

    if created {
        println!("Made Space '{space}' for this repository.");
    } else {
        println!("Space '{space}' is already set up for this folder.");
    }
    match &tie {
        Tie::Made(binding) => println!(
            "Tied it to {}. `wenlan repo publish` puts its pages into the repository as a pull request.",
            binding.repo_root
        ),
        Tie::Already(binding) => println!("It is already tied to {}.", binding.repo_root),
        Tie::Elsewhere(binding) => println!(
            "It is tied to another folder, {}, and was left as it is.",
            binding.repo_root
        ),
        Tie::NotAgreed { name, owner } => {
            let whose = match owner {
                Some(owner) => format!("github.com/{name} belongs to {owner}"),
                None => format!("{name} is not yours"),
            };
            println!(
                "Not tied to the repository yet: {whose}, and pull requests would open there. \
                 `wenlan repo bind --confirm-repo {name}` ties it."
            );
        }
    }
    if !connected.is_empty() {
        println!("Connected AI tools: {}.", labels(&connected));
    }
    for tool in &waiting {
        println!(
            "{} is not connected yet. `wenlan connect {}` connects it.",
            tool.label, tool.connect_as
        );
    }
    if connected.is_empty() && waiting.is_empty() {
        println!("No AI tool was found on this computer. `wenlan connect --help` lists the ones Wenlan can connect.");
    }
    println!(
        "From now on, the terminal and AI tools started in this folder use Space '{space}'. `wenlan status` shows it."
    );
    Ok(())
}

/// Whether pull requests may go to `target`: always when the repository has
/// no remote or is the user's own; otherwise only when they say yes here.
/// The same line `wenlan repo bind` draws with `--confirm-repo`.
fn agreed(target: &PublishTarget, interactive: bool) -> Result<bool> {
    let Some(name) = target.name() else {
        return Ok(true);
    };
    if target.owned() {
        return Ok(true);
    }
    if !interactive {
        return Ok(false);
    }
    let whose = match target.owner() {
        Some(owner) => format!("github.com/{name}, which belongs to {owner}"),
        None => name.to_owned(),
    };
    ask(
        &format!(
            "`wenlan repo publish` would open pull requests on {whose}. Tie this Space to it?"
        ),
        false,
    )
}

/// Ask a yes/no question on the terminal. Enter takes `default_yes`.
fn ask(question: &str, default_yes: bool) -> Result<bool> {
    let choices = if default_yes { "[Y/n]" } else { "[y/N]" };
    print!("{question} {choices} ");
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    Ok(parse_answer(&answer, default_yes))
}

fn parse_answer(answer: &str, default_yes: bool) -> bool {
    match answer.trim().to_ascii_lowercase().as_str() {
        "" => default_yes,
        "y" | "yes" => true,
        _ => false,
    }
}

fn labels(tools: &[Tool]) -> String {
    tools
        .iter()
        .map(|tool| tool.label)
        .collect::<Vec<_>>()
        .join(", ")
}

/// For `wenlan status`: which Space this folder uses and the repository it
/// is tied to, or the one command that sets it up. Says nothing outside a
/// git repository, and nothing when Wenlan is not running: the lines above
/// it already say that.
pub async fn print_folder(client: &WenlanClient) {
    let Ok(cwd) = std::env::current_dir() else {
        return;
    };
    let Ok(top) = repo_wiki::repository_top_level(&cwd) else {
        return;
    };
    let Ok(spaces) = client.list_spaces().await else {
        return;
    };
    let registered: HashSet<String> = spaces.into_iter().map(|space| space.name).collect();
    let hint = folder_hint(&cwd);
    println!();
    println!("This folder");
    let Some(space) = hint.registered(|name| registered.contains(name)) else {
        println!("  Not in Wenlan yet. `wenlan init` sets it up.");
        return;
    };
    println!("  Space: {space}");
    match client.repo_wiki_binding(space).await {
        Ok(Some(binding)) if Path::new(&binding.repo_root) == top => {
            let place = binding.remote_url.as_deref().unwrap_or(&binding.repo_root);
            println!("  Repository: {place}");
            if let Some(last) = &binding.last_publish {
                println!(
                    "  Last pull request: {}",
                    super::wiki::describe_publish(last)
                );
            }
        }
        Ok(Some(binding)) => println!(
            "  Repository: tied to another folder, {}",
            binding.repo_root
        ),
        Ok(None) => println!("  Repository: not tied. `wenlan init` ties it."),
        Err(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enter_takes_the_default_and_anything_else_but_yes_is_no() {
        assert!(parse_answer("\n", true));
        assert!(!parse_answer("\n", false));
        assert!(parse_answer(" Y \n", false));
        assert!(parse_answer("yes\n", false));
        assert!(!parse_answer("n\n", true));
        assert!(!parse_answer("maybe\n", true));
    }

    #[test]
    fn installed_tools_are_the_commands_on_path_plus_cursor_s_folder() {
        let home = tempfile::tempdir().unwrap();
        assert!(installed_tools(|_| false, Some(home.path())).is_empty());
        assert_eq!(
            installed_tools(|binary| binary == "codex", Some(home.path())),
            vec![CODEX]
        );
        std::fs::create_dir(home.path().join(".cursor")).unwrap();
        assert_eq!(
            installed_tools(|binary| binary == "claude", Some(home.path())),
            vec![CLAUDE_CODE, CURSOR]
        );
    }

    #[test]
    fn a_repository_with_no_remote_or_of_your_own_needs_no_question() {
        let no_remote = PublishTarget {
            remote_url: None,
            slug: None,
            signed_in_as: None,
        };
        assert!(agreed(&no_remote, false).unwrap());
    }
}
