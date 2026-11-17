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
use wenlan_types::repo_wiki::{
    BindRepoWikiRequest, RepoWikiAgent, RepoWikiBinding, RepoWikiDaily, RepoWikiRunner,
};

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

/// Whether the tool already has Wenlan through its plugin, which carries its
/// own MCP entry: connecting it again would list every Wenlan tool twice.
/// Claude Code names enabled plugins in `~/.claude/settings.json`, Codex in
/// `config.toml` under its home; both key them `wenlan@<marketplace>`. A file
/// that is missing or cannot be read counts as no plugin.
fn has_plugin(tool: Tool, home: Option<&Path>, codex_home: Option<&Path>) -> bool {
    let read = |path: std::path::PathBuf| std::fs::read_to_string(path).unwrap_or_default();
    match tool.client {
        McpClient::ClaudeCode => home.is_some_and(|home| {
            claude_plugin_enabled(&read(home.join(".claude").join("settings.json")))
        }),
        McpClient::Codex => {
            codex_home.is_some_and(|dir| codex_plugin_enabled(&read(dir.join("config.toml"))))
        }
        _ => false,
    }
}

fn claude_plugin_enabled(settings_json: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(settings_json)
        .ok()
        .and_then(|value| value.get("enabledPlugins")?.as_object().cloned())
        .is_some_and(|plugins| {
            plugins
                .iter()
                .any(|(key, on)| key.starts_with("wenlan@") && on.as_bool() == Some(true))
        })
}

/// `[plugins."wenlan@<marketplace>"]` with `enabled = true` before the next
/// table. Read line by line: the CLI carries no TOML reader.
fn codex_plugin_enabled(config_toml: &str) -> bool {
    let mut inside = false;
    for line in config_toml.lines().map(str::trim) {
        if line.starts_with('[') {
            inside =
                line.starts_with("[plugins.\"wenlan@") || line.starts_with("[plugins.'wenlan@");
        } else if inside {
            let compact: String = line
                .split('#')
                .next()
                .unwrap_or("")
                .split_whitespace()
                .collect();
            if compact == "enabled=true" {
                return true;
            }
        }
    }
    false
}

/// Where Codex keeps its settings: `CODEX_HOME`, or `.codex` in the home folder.
fn codex_home(home: Option<&Path>) -> Option<std::path::PathBuf> {
    std::env::var_os("CODEX_HOME")
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| home.map(|home| home.join(".codex")))
}

/// How the AI tools found on this computer get connected.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Connect {
    /// Ask about each one (`wenlan init` on a terminal).
    AskEach,
    /// Connect every one without asking (`wenlan setup`).
    All,
    /// Connect none; only report (nobody to ask).
    Report,
}

/// The AI tools on this computer after the connect step.
pub(super) struct Tools {
    /// Connected now: just connected, connected before, or through a plugin.
    connected: Vec<Tool>,
    /// Among `connected`, how many this run added to the tool's settings.
    pub(super) new: usize,
    /// Installed and not connected.
    waiting: Vec<Tool>,
}

impl Tools {
    pub(super) fn connect_names(&self) -> Vec<&'static str> {
        self.connected.iter().map(|tool| tool.connect_as).collect()
    }

    pub(super) fn waiting_names(&self) -> Vec<&'static str> {
        self.waiting.iter().map(|tool| tool.connect_as).collect()
    }

    /// The label of the first connected tool, for "open … and ask".
    pub(super) fn first_label(&self) -> Option<&'static str> {
        self.connected.first().map(|tool| tool.label)
    }

    /// What `init` and `setup` print about the tools.
    pub(super) fn print(&self) {
        if !self.connected.is_empty() {
            println!("Connected AI tools: {}.", labels(&self.connected));
        }
        for tool in &self.waiting {
            println!(
                "{} is not connected yet. `wenlan connect {}` connects it.",
                tool.label, tool.connect_as
            );
        }
        if self.connected.is_empty() && self.waiting.is_empty() {
            println!("No AI tool was found on this computer. `wenlan connect --help` lists the ones Wenlan can connect.");
        }
    }
}

/// Find the AI tools installed here and connect them as `how` says. A tool the
/// daemon already knows, or one that has the Wenlan plugin, is left as it is.
/// A daemon that cannot list them counts as none known.
pub(super) async fn connect_tools(client: &WenlanClient, how: Connect) -> Result<Tools> {
    let known: HashSet<String> = client
        .list_agents()
        .await
        .map(|agents| agents.into_iter().map(|agent| agent.name).collect())
        .unwrap_or_default();
    let home = dirs::home_dir();
    let codex = codex_home(home.as_deref());
    let installed = installed_tools(|binary| which::which(binary).is_ok(), home.as_deref());
    let mut tools = Tools {
        connected: Vec::new(),
        new: 0,
        waiting: Vec::new(),
    };
    for tool in installed {
        if known.contains(tool.agent) || has_plugin(tool, home.as_deref(), codex.as_deref()) {
            tools.connected.push(tool);
            continue;
        }
        let go = match how {
            Connect::All => true,
            Connect::AskEach => ask(&format!("Connect {}?", tool.label), true)?,
            Connect::Report => false,
        };
        if !go {
            tools.waiting.push(tool);
            continue;
        }
        let args = ConnectArgs {
            client: tool.client,
            dry_run: false,
            all_tools: false,
        };
        match super::mcp::run_connect(client, args, true).await {
            Ok(changed) => {
                tools.connected.push(tool);
                tools.new += usize::from(changed);
            }
            Err(error) => {
                eprintln!("wenlan: {} was not connected: {error:#}", tool.label);
                tools.waiting.push(tool);
            }
        }
    }
    Ok(tools)
}

/// The local hour after which the daily run `init` turns on starts.
const DAILY_HOUR: u8 = 9;

/// The AI tool a daily run would start: the first one installed here, in the
/// order `wenlan repo bind --agent` lists them.
fn daily_agent(on_path: impl Fn(&str) -> bool) -> Option<RepoWikiAgent> {
    RepoWikiAgent::ALL
        .into_iter()
        .find(|agent| agent.programs().iter().any(|program| on_path(program)))
}

/// Whether to open a pull request every day, asked once when the repository
/// is first tied. Only a yes on the terminal turns it on: with nobody to ask,
/// no pull request to open (no `origin`) or no AI tool to start, it stays off.
fn daily_choice(
    target: &PublishTarget,
    agent: Option<RepoWikiAgent>,
    interactive: bool,
) -> Result<Option<RepoWikiDaily>> {
    let (Some(_), Some(agent), true) = (target.name(), agent, interactive) else {
        return Ok(None);
    };
    let yes = ask(
        &format!(
            "Open a pull request every day when something changed? It starts {} on this computer once a day, after {DAILY_HOUR:02}:00.",
            agent.display_name()
        ),
        true,
    )?;
    Ok(yes.then_some(RepoWikiDaily {
        enabled: true,
        hour: DAILY_HOUR,
        agent,
        runs_on: RepoWikiRunner::Computer,
    }))
}

/// The words the daemon uses when another Wenlan keeps the repository.
const KEPT_BY_ANOTHER: &str = "already kept by another Wenlan";

/// How the Space and this repository stand after `init`.
enum Tie {
    /// Tied by this run.
    Made(RepoWikiBinding),
    /// Was tied to this repository already.
    Already(RepoWikiBinding),
    /// The Space is tied to another working copy; left as it is.
    Elsewhere(RepoWikiBinding),
    /// Another Wenlan (a teammate's, or another computer) already keeps this
    /// repository's pages and rules; a repository has one.
    KeptByAnother,
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
                let daily = daily_choice(
                    &target,
                    daily_agent(|program| which::which(program).is_ok()),
                    interactive,
                )?;
                let binding = client
                    .bind_repo_wiki(
                        &space,
                        &BindRepoWikiRequest {
                            repo_root: top.to_string_lossy().into_owned(),
                            folder: None,
                            daily,
                            take_over: false,
                        },
                    )
                    .await;
                match binding {
                    Ok(binding) => Tie::Made(binding),
                    // Another Wenlan keeps this repository's pages and rules.
                    Err(error) if error.to_string().contains(KEPT_BY_ANOTHER) => Tie::KeptByAnother,
                    Err(error) => return Err(error),
                }
            } else {
                Tie::NotAgreed {
                    name: target.name().unwrap_or_default().to_owned(),
                    owner: target.owner().map(str::to_owned),
                }
            }
        }
    };

    // 3. The AI tools.
    let how = if interactive {
        Connect::AskEach
    } else {
        Connect::Report
    };
    let tools = connect_tools(client, how).await?;

    if quiet {
        return Ok(());
    }
    if matches!(format, ResolvedFormat::Json) {
        let (repository, repository_state) = match &tie {
            Tie::Made(binding) => (Some(binding), "tied"),
            Tie::Already(binding) => (Some(binding), "already_tied"),
            Tie::Elsewhere(binding) => (Some(binding), "tied_elsewhere"),
            Tie::NotAgreed { .. } => (None, "not_agreed"),
            Tie::KeptByAnother => (None, "kept_by_another"),
        };
        return print_json(&serde_json::json!({
            "space": space,
            "created": created,
            "repository_state": repository_state,
            "repository": repository,
            "connected": tools.connect_names(),
            "not_connected": tools.waiting_names(),
        }));
    }

    if created {
        println!("Made Space '{space}' for this repository.");
    } else {
        println!("Space '{space}' is already set up for this folder.");
    }
    match &tie {
        Tie::Made(binding) => {
            println!(
                "Tied it to {}. `wenlan repo publish` puts its pages into the repository as a pull request.",
                binding.repo_root
            );
            println!(
                "The pages go in `{}/` there. `wenlan repo bind --folder <NAME>` picks another folder.",
                binding.folder
            );
            match &binding.daily {
                Some(daily) if daily.enabled => println!(
                    "A pull request opens every day when something changed, after {:02}:00, with {}. `wenlan repo bind --daily off` stops it.",
                    daily.hour,
                    daily.agent.display_name()
                ),
                _ => println!(
                    "No daily pull request. `wenlan repo bind --daily {DAILY_HOUR}` turns it on."
                ),
            }
        }
        Tie::Already(binding) => println!("It is already tied to {}.", binding.repo_root),
        Tie::Elsewhere(binding) => println!(
            "It is tied to another folder, {}, and was left as it is.",
            binding.repo_root
        ),
        Tie::KeptByAnother => println!(
            "Not tied to the repository: its pages and rules are already kept by another Wenlan \
             (a teammate's, or another computer of yours). Your AI tools read them from the \
             repository. `wenlan repo bind --take-over` publishes from this computer instead."
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
    tools.print();
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

/// The file, in Wenlan's data folder, listing the repositories whose "not set
/// up yet" line was already said (one path a line).
const SAID_FILE: &str = "init-hints";

/// Whether `cwd` is inside a git repository that has no Space of its own
/// among `registered`. Returns the repository's top folder when so.
fn unknown_repository(cwd: &Path, registered: &HashSet<String>) -> Option<std::path::PathBuf> {
    let top = repo_wiki::repository_top_level(cwd).ok()?;
    let known = folder_hint(cwd)
        .registered(|name| registered.contains(name))
        .is_some();
    (!known).then_some(top)
}

/// For a command that works without a Space of its own (`recall`, `capture`,
/// `rules`, …): in a repository Wenlan does not know, say once that
/// `wenlan init` sets it up. Only to a person at a terminal, and never again
/// for the same repository. Best effort: nothing here can fail the command.
pub fn say_once_not_set_up(cwd: &Path, registered: &HashSet<String>) {
    if !(std::io::stderr().is_terminal() && std::io::stdout().is_terminal()) {
        return;
    }
    let Some(top) = unknown_repository(cwd, registered) else {
        return;
    };
    let said = wenlan_core::config::data_root().join(SAID_FILE);
    if first_time(&said, &top.to_string_lossy()) {
        eprintln!(
            "wenlan: this repository is not set up in Wenlan yet. `wenlan init` gives it its own Space. (Said once.)"
        );
    }
}

/// Record `line` in the file at `path`; `true` when it was not there before
/// and is now. A file that cannot be written counts as "said", so a read-only
/// data folder does not repeat the line on every command.
fn first_time(path: &Path, line: &str) -> bool {
    let before = std::fs::read_to_string(path).unwrap_or_default();
    if before.lines().any(|said| said == line) {
        return false;
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| writeln!(file, "{line}"))
        .is_ok()
}

/// For `wenlan repo publish` where the folder is a repository that is not
/// tied yet: the command cannot do its work, so a person at a terminal is
/// asked once whether to set it up now, and `init` runs on a yes. With nobody
/// to ask the command fails and names `wenlan init`.
/// Returns the Space to publish, or `None` when there is nothing to publish
/// and the person has been told why: they said no, or `init` tied nothing.
pub async fn set_up_to_publish(
    client: &WenlanClient,
    format: ResolvedFormat,
    quiet: bool,
) -> Result<Option<String>> {
    let cwd = std::env::current_dir()?;
    if repo_wiki::repository_top_level(&cwd).is_err() {
        bail!(
            "this folder is not inside a git repository. Run `wenlan repo publish` inside the repository"
        );
    }
    let interactive = !quiet
        && matches!(format, ResolvedFormat::Table)
        && std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal();
    if !interactive {
        bail!(
            "this repository is not set up in Wenlan yet. `wenlan init` sets it up; then run `wenlan repo publish` again"
        );
    }
    if !ask(
        "This repository is not set up in Wenlan yet. Set it up now?",
        true,
    )? {
        println!("Nothing was changed. `wenlan init` sets it up when you want to.");
        return Ok(None);
    }
    run(client, format, quiet).await?;
    let registered: HashSet<String> = client
        .list_spaces()
        .await?
        .into_iter()
        .map(|space| space.name)
        .collect();
    let hint = folder_hint(&cwd);
    let Some(space) = hint.registered(|name| registered.contains(name)) else {
        return Ok(None);
    };
    let tied = client.repo_wiki_binding(space).await?.is_some();
    Ok(tied.then(|| space.to_owned()))
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
    fn the_not_set_up_line_is_said_once_for_each_repository() {
        let dir = tempfile::tempdir().unwrap();
        let said = dir.path().join("data").join(SAID_FILE);
        assert!(first_time(&said, "/work/a"));
        assert!(!first_time(&said, "/work/a"));
        assert!(first_time(&said, "/work/b"));
        assert!(!first_time(&said, "/work/a"));
        assert!(!first_time(&said, "/work/b"));
    }

    #[test]
    fn a_tool_with_the_wenlan_plugin_counts_as_connected() {
        assert!(claude_plugin_enabled(
            r#"{"enabledPlugins":{"wenlan@7xuanlu":true}}"#
        ));
        assert!(claude_plugin_enabled(
            r#"{"enabledPlugins":{"other@x":true,"wenlan@7xuanlu-wenlan":true}}"#
        ));
        // Switched off, another plugin, no key, or a file that is not JSON.
        assert!(!claude_plugin_enabled(
            r#"{"enabledPlugins":{"wenlan@7xuanlu":false}}"#
        ));
        assert!(!claude_plugin_enabled(
            r#"{"enabledPlugins":{"notwenlan@x":true}}"#
        ));
        assert!(!claude_plugin_enabled("{}"));
        assert!(!claude_plugin_enabled(""));

        assert!(codex_plugin_enabled(
            "model = \"x\"\n[plugins.\"wenlan@7xuanlu-wenlan\"]\nenabled = true # on\n"
        ));
        assert!(!codex_plugin_enabled(
            "[plugins.\"wenlan@7xuanlu-wenlan\"]\nenabled = false\n"
        ));
        // `enabled = true` under another table is not Wenlan's.
        assert!(!codex_plugin_enabled(
            "[plugins.\"wenlan@x\"]\n[plugins.\"other@x\"]\nenabled = true\n"
        ));
        assert!(!codex_plugin_enabled(""));
    }

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

    #[test]
    fn the_daily_question_is_not_asked_with_nobody_to_ask_no_origin_or_no_ai_tool() {
        let mine = PublishTarget {
            remote_url: Some("https://github.com/me/acme-api.git".to_owned()),
            slug: Some("me/acme-api".to_owned()),
            signed_in_as: Some("me".to_owned()),
        };
        let no_remote = PublishTarget {
            remote_url: None,
            slug: None,
            signed_in_as: None,
        };
        let claude = Some(RepoWikiAgent::Claude);
        // No terminal.
        assert_eq!(daily_choice(&mine, claude, false).unwrap(), None);
        // Nowhere to open a pull request.
        assert_eq!(daily_choice(&no_remote, claude, true).unwrap(), None);
        // No AI tool to start.
        assert_eq!(daily_choice(&mine, None, true).unwrap(), None);
    }

    #[test]
    fn the_daily_run_starts_the_first_ai_tool_installed() {
        assert_eq!(daily_agent(|_| false), None);
        assert_eq!(
            daily_agent(|program| program == "codex" || program == "copilot"),
            Some(RepoWikiAgent::Codex)
        );
        assert_eq!(
            daily_agent(|program| program == "cursor-agent"),
            Some(RepoWikiAgent::Cursor)
        );
    }
}
