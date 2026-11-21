// SPDX-License-Identifier: Apache-2.0
use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use output::OutputFormat;
use std::path::PathBuf;
use std::process::ExitCode;
use wenlan_cli::space_context::{
    resolve_agent_name, resolve_cli_space, resolve_cli_space_offline, resolve_native_read_space,
    CliSpaceOperation,
};
use wenlan_cli::{client, commands, output};
use wenlan_types::lint::LintProfile;

#[derive(Parser)]
#[command(
    name = "wenlan",
    version,
    about = "Wenlan: docs, skills and rules your AI tools keep current and work from. Capture, recall, and hand off across sessions."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Output format. Auto-detects JSON when piped, table on TTY.
    #[arg(long, value_enum, default_value_t = OutputFormat::Auto, global = true)]
    format: OutputFormat,

    /// Suppress all non-error output. Useful for scripts.
    #[arg(long, short, global = true)]
    quiet: bool,

    /// Identify the caller in X-Agent-Name audit metadata.
    #[arg(long, global = true)]
    agent_name: Option<String>,

    /// AI session as tool:id (for example claude-code:<session id>); links what you write to that conversation.
    /// Defaults to WENLAN_SESSION, then CODEX_THREAD_ID.
    #[arg(long, global = true, value_name = "TOOL:ID")]
    session: Option<String>,

    /// Use one registered Space for scope-aware reads and writes.
    #[arg(long, global = true, conflicts_with = "all_spaces")]
    space: Option<String>,

    /// Read across every Space. Invalid for writes and strict WENLAN_SPACE pins.
    #[arg(long, global = true)]
    all_spaces: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Check whether Wenlan is running, and which Space this folder uses.
    Status,
    /// Open Wenlan's personal browser UI.
    Open,
    /// Set Wenlan up and connect the AI tools on this computer; optionally a local model or an Anthropic key.
    Setup {
        /// Set up without a model or API key.
        #[arg(long)]
        basic: bool,
        /// Download and select a local model, for example qwen3.5-4b.
        #[arg(long, value_name = "MODEL_ID")]
        model: Option<String>,
        /// Read an Anthropic key from this environment variable.
        #[arg(long = "anthropic-api-key-env", value_name = "ENV_VAR")]
        anthropic_api_key_env: Option<String>,
        /// Skip confirmation prompts where possible.
        #[arg(short = 'y', long)]
        yes: bool,
        /// Write the settings only: connect no AI tool and ask nothing.
        #[arg(long)]
        config_only: bool,
        /// Only what follows the settings; the installer runs it once Wenlan has started.
        #[arg(long, hide = true, conflicts_with_all = ["basic", "model", "anthropic_api_key_env", "config_only"])]
        finish: bool,
    },
    /// Set this repository up: a Space named after it, tied to it, and your AI tools connected.
    Init,
    /// Start Wenlan now and keep it running in the background after login.
    Start,
    /// Stop Wenlan. It stays off until `wenlan start`.
    Stop,
    /// The earlier spelling of `start` (`background on`) and `stop` (`background off`).
    #[command(hide = true)]
    Background {
        #[command(subcommand)]
        command: commands::service::BackgroundCommand,
    },
    /// Restart the Wenlan background process. Required after an update.
    Restart,
    /// Diagnose runtime, model, and API key setup.
    Doctor {
        /// Point older ~/.wenlan/bin copies at the app bundle. The previous file is renamed aside, not deleted.
        #[arg(long)]
        repair: bool,
    },
    /// Check the health of memories, pages, and the running Wenlan; report, then resolve findings safely.
    Lint {
        #[arg(long)]
        profile: Option<LintProfile>,
        /// Permit a deep semantic pass to use an already configured external provider.
        #[arg(long)]
        allow_external: bool,
        /// Return bounded high-recall semantic candidates for the calling agent.
        #[arg(long)]
        agent_assist: bool,
        /// Submit typed agent verdicts produced from a prior prepare report.
        #[arg(long, value_name = "JSON_FILE")]
        agent_submission: Option<PathBuf>,
    },
    /// Manage local models.
    Models {
        #[command(subcommand)]
        command: commands::setup::ModelCommand,
    },
    /// Manage provider API keys.
    Keys {
        #[command(subcommand)]
        command: commands::setup::KeyCommand,
    },
    /// Configure, inspect, or disable Steep, the optional background upkeep (model-backed).
    Steep {
        #[command(subcommand)]
        command: commands::setup::EnrichmentCommand,
    },
    /// Connect Wenlan to a supported agent or editor.
    Connect(commands::mcp::ConnectArgs),
    /// Find the memories and pages that match a query.
    Recall {
        /// Query to recall memories for.
        query: String,
        /// Max memories to return.
        #[arg(short, long, default_value_t = 10)]
        limit: usize,
    },
    /// Read and save working rules for a Space, or manage your personal AI-tool setup.
    Rules {
        #[command(subcommand)]
        command: Option<commands::skills::RulesCmd>,
    },
    /// Find, save, review and install skills your AI tools can use.
    Skills {
        #[command(subcommand)]
        command: Option<commands::skills::SkillsCmd>,
    },
    /// Read the current Space Brief, optionally with related context.
    Brief(commands::brief::BriefArgs),
    /// Hand off work through the Space Brief; keeps a project file if Wenlan is down.
    Handoff(commands::handoff::HandoffArgs),
    /// List pages or open one by title; read, write, rename, move, delete or restore one (read, write, rename, delete, move, history, restore, links, lock, unlock).
    #[command(args_conflicts_with_subcommands = true)]
    Pages {
        #[command(subcommand)]
        action: Option<commands::pages::PagesAction>,
        /// Title/filename substring. Omit to list pages newest-first.
        query: Option<String>,
        /// Max pages to list (newest-first). 0 = all. Ignored when a query opens a page.
        #[arg(short, long, default_value_t = 20)]
        limit: usize,
        /// Print the matched page's stable internal id instead of opening it.
        #[arg(long)]
        resolve_id: bool,
        /// Print the matched page's text and its sources instead of opening it.
        #[arg(long, requires = "query", conflicts_with = "resolve_id")]
        print: bool,
    },
    /// Manage folders and files Wenlan should learn from (add, save, read).
    Sources {
        #[command(subcommand)]
        command: commands::ingest::SourcesCommand,
    },
    /// Capture a memory. Provide text positionally, or use --file, or pipe via stdin.
    Capture {
        /// Content text. If omitted and --file unset, read from stdin.
        #[arg(conflicts_with = "file")]
        text: Option<String>,
        /// Read content from a file.
        #[arg(short, long)]
        file: Option<std::path::PathBuf>,
        /// Memory type (e.g. fact, decision).
        #[arg(short = 't', long = "type")]
        memory_type: Option<String>,
    },
    /// List recent memories, or confirm, forget and review them (pending, confirm, forget, history).
    #[command(args_conflicts_with_subcommands = true)]
    Memories {
        #[command(subcommand)]
        action: Option<commands::page_tools::MemoryAction>,
        /// Max results.
        #[arg(short, long, default_value_t = 20)]
        limit: usize,
        /// Filter by memory type.
        #[arg(short = 't', long = "type")]
        memory_type: Option<String>,
        /// Only pending captures (not yet confirmed; what the MCP list_pending tool returns).
        #[arg(long)]
        pending: bool,
    },
    /// List changes waiting for review, or dismiss one. Accepting is done in the Wenlan app.
    Curate {
        #[command(subcommand)]
        action: Option<commands::curate::CurateAction>,
    },
    /// Manage registered agents (list / show / edit).
    #[command(hide = true)]
    Agents {
        #[command(subcommand)]
        cmd: commands::agents::AgentsCmd,
    },
    /// Manage Spaces, the separate sets of pages Wenlan keeps (list, add, default, show).
    Spaces {
        #[command(subcommand)]
        cmd: commands::space::SpaceCmd,
    },
    /// Connect an AI tool on this computer to a team with a short code you allow in a browser.
    Login(commands::login::LoginArgs),
    /// Work with a team: start one here, invite people, or join with an invite link (start, share, invite, join, status, leave, update, remove, new-key, stop).
    Team {
        #[command(subcommand)]
        cmd: commands::team::TeamCmd,
    },
    /// Show or send what was saved locally while Wenlan was not running.
    Outbox {
        #[command(subcommand)]
        command: commands::outbox::OutboxCommand,
    },
    /// List, archive, restore, merge, or alias knowledge-graph entities.
    Entities {
        #[command(subcommand)]
        cmd: commands::entities::EntitiesCmd,
    },
    /// Export pages to an external bundle format.
    Export {
        #[command(subcommand)]
        command: commands::export::ExportCommand,
    },
    /// Put this Space's code pages into its git repository as a pull request.
    /// `wenlan wiki` is the earlier name and still works.
    #[command(name = "repo", alias = "wiki")]
    Wiki {
        #[command(subcommand)]
        command: commands::wiki::WikiCommand,
    },
    /// Make a page or a repo file follow a page, so it keeps up when that page changes.
    Follow(commands::follow::FollowArgs),
    /// Stop a page or a repo file following a page.
    Unfollow(commands::follow::UnfollowArgs),
    /// Show what follows which page, and what is waiting to catch up.
    Follows(commands::follow::FollowsArgs),
    /// Call any Wenlan tool an AI tool can call, by the same name (list, show, call).
    Tool {
        #[command(subcommand)]
        cmd: commands::tool::ToolCmd,
    },
    /// Force one bounded pass over every due ambient job now, instead of
    /// waiting for a quiet turn.
    #[command(hide = true)]
    Sweep,
}

/// Help groups for `wenlan --help`, in display order. Only names live here;
/// each description stays on its `Commands` doc comment. Hidden subcommands
/// (`sweep`, `agents`) are deliberately absent. The first `FIRST_PAGE_GROUPS`
/// groups are the short help; `wenlan help all` writes out every group.
const HELP_GROUPS: &[(&str, &[&str])] = &[
    ("Start", &["setup", "init", "open", "status"]),
    (
        "Daily",
        &["capture", "recall", "brief", "handoff", "rules", "pages"],
    ),
    ("Team", &["team", "login"]),
    (
        "Running",
        &["connect", "doctor", "start", "stop", "restart"],
    ),
    (
        "More",
        &[
            "skills", "memories", "sources", "follow", "unfollow", "follows", "spaces", "repo",
            "curate", "lint", "models", "keys", "steep", "outbox", "export", "entities", "tool",
        ],
    ),
];

/// How many of `HELP_GROUPS` the short `wenlan --help` shows.
const FIRST_PAGE_GROUPS: usize = 3;

/// One short line per command on the short help, so the first page reads at a
/// glance. `wenlan help all` and `wenlan <command> --help` keep the full text.
const FIRST_PAGE_LINES: &[(&str, &str)] = &[
    ("setup", "Set Wenlan up (once per computer)"),
    ("init", "Set this repository up (once per repo)"),
    ("open", "Open Wenlan in the browser"),
    ("status", "Is Wenlan running, and which Space is this"),
    ("capture", "Save something"),
    ("recall", "Find memories and pages"),
    ("brief", "Read the Space Brief"),
    ("handoff", "Hand off work to the next session"),
    ("rules", "Read the working rules"),
    ("pages", "List, read or write pages"),
    ("team", "Start, invite, or join a team"),
];

/// Build a clap help template that lists the subcommands by `HELP_GROUPS`
/// instead of one flat `Commands:` block. Every line keeps clap's two-space
/// indent so help parsers that read the first word of indented lines still work.
fn grouped_help_template(cmd: &clap::Command, all: bool) -> String {
    let abouts: std::collections::BTreeMap<&str, String> = cmd
        .get_subcommands()
        .map(|sub| {
            (
                sub.get_name(),
                sub.get_about().map(ToString::to_string).unwrap_or_default(),
            )
        })
        .collect();
    let groups = if all {
        HELP_GROUPS
    } else {
        &HELP_GROUPS[..FIRST_PAGE_GROUPS]
    };
    let width = groups
        .iter()
        .flat_map(|(_, names)| names.iter())
        .map(|name| name.len())
        .max()
        .unwrap_or(0);
    let mut template =
        String::from("{before-help}{about-with-newline}\n{usage-heading} {usage}\n\n");
    for (heading, names) in groups {
        template.push_str(heading);
        template.push_str(":\n");
        for name in names.iter() {
            let short = FIRST_PAGE_LINES
                .iter()
                .find(|(command, _)| command == name)
                .map(|(_, line)| *line);
            let about = match short {
                Some(line) if !all => line,
                _ => abouts.get(name).map(String::as_str).unwrap_or(""),
            };
            template.push_str(&format!("  {name:<width$}  {about}\n"));
        }
        template.push('\n');
    }
    if all {
        template.push_str("Options:\n{options}{after-help}");
    } else {
        // Not indented: tools that read the first word of indented lines as a
        // command must not see this one.
        template.push_str("Everything else: wenlan help all\n{after-help}");
    }
    template
}

#[tokio::main]
async fn main() -> anyhow::Result<ExitCode> {
    let cmd = Cli::command();
    // `wenlan help all`: every group, full descriptions and the options.
    let wants_all = std::env::args()
        .skip(1)
        .eq(["help", "all"].map(String::from));
    if wants_all {
        let template = grouped_help_template(&cmd, true);
        cmd.help_template(template).print_help()?;
        return Ok(ExitCode::SUCCESS);
    }
    let template = grouped_help_template(&cmd, false);
    let matches = cmd.help_template(template).get_matches();
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|error| error.exit());
    if cli.all_spaces && matches!(&cli.command, Commands::Brief(_)) {
        anyhow::bail!("Brief is owned by one Space; --all-spaces is not supported");
    }
    if matches!(&cli.command, Commands::Wiki { command } if command.is_offline()) {
        let (format, quiet) = (cli.format.resolve(), cli.quiet);
        if cli.space.is_some() || cli.all_spaces {
            anyhow::bail!("--space/--all-spaces are not supported by this command");
        }
        return match cli.command {
            Commands::Wiki {
                command: commands::wiki::WikiCommand::Refresh(args),
            } => Ok(if commands::wiki_refresh::refresh(args, format, quiet)? {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }),
            Commands::Wiki {
                command: commands::wiki::WikiCommand::Workflow(args),
            } => {
                commands::wiki_refresh::workflow(args, quiet)?;
                Ok(ExitCode::SUCCESS)
            }
            _ => unreachable!("only offline wiki commands get here"),
        };
    }
    let environment_agent = std::env::var("WENLAN_AGENT_NAME").ok();
    let agent_name = resolve_agent_name(cli.agent_name.as_deref(), environment_agent.as_deref());
    // A personal adoption preview reads only the tool files. It does not look
    // up a Space, build a daemon client or recover a stopped daemon.
    if let Commands::Rules {
        command: Some(commands::skills::RulesCmd::Setup(command)),
    } = &cli.command
    {
        if cli.space.is_some() || cli.all_spaces {
            anyhow::bail!("Personal rules setup does not take --space or --all-spaces");
        }
        if std::env::var_os("WENLAN_SPACE")
            .is_some_and(|space| !space.to_string_lossy().trim().is_empty())
        {
            anyhow::bail!("WENLAN_SPACE limits this terminal to one Space. Run personal rules setup without WENLAN_SPACE.");
        }
        commands::rulesync::run(
            command,
            cli.format.resolve(),
            cli.quiet,
            agent_name.is_some(),
            cli.session.as_deref(),
        )
        .await?;
        return Ok(ExitCode::SUCCESS);
    }
    // Commands that are an MCP tool under a CLI name go through `wenlan-mcp`,
    // not the daemon client below, so they behave as they do in an AI tool.
    let tool_backed = matches!(
        &cli.command,
        Commands::Tool { .. }
            | Commands::Follow(_)
            | Commands::Unfollow(_)
            | Commands::Follows(_)
            | Commands::Pages {
                action: Some(commands::pages::PagesAction::Tool(_)),
                ..
            }
            | Commands::Memories {
                action: Some(_),
                ..
            }
            | Commands::Sources {
                command: commands::ingest::SourcesCommand::Read(_)
            }
    );
    if tool_backed {
        if cli.all_spaces {
            anyhow::bail!(
                "--all-spaces is not supported by this command; name one Space with --space"
            );
        }
        let caller = commands::tool::Caller {
            host: std::env::var("WENLAN_HOST")
                .ok()
                .filter(|host| !host.is_empty()),
            agent_name,
            space: cli.space.clone(),
        };
        let (format, quiet) = (cli.format.resolve(), cli.quiet);
        let ok = match cli.command {
            Commands::Tool { cmd } => commands::tool::run(cmd, &caller, format, quiet)?,
            Commands::Follow(args) => {
                commands::follow::run_follow(args, &caller, format, quiet)?;
                true
            }
            Commands::Unfollow(args) => {
                commands::follow::run_unfollow(args, &caller, format, quiet)?;
                true
            }
            Commands::Follows(args) => {
                commands::follow::run_follows(args, &caller, format, quiet)?;
                true
            }
            Commands::Pages {
                action: Some(commands::pages::PagesAction::Tool(action)),
                ..
            } => {
                commands::page_tools::run_pages(action, &caller, format, quiet)?;
                true
            }
            Commands::Memories {
                action: Some(action),
                ..
            } => {
                commands::page_tools::run_memories(action, &caller, format, quiet)?;
                true
            }
            Commands::Sources {
                command: commands::ingest::SourcesCommand::Read(args),
            } => {
                commands::page_tools::run_read_source(args, &caller, format, quiet)?;
                true
            }
            _ => unreachable!("matched above"),
        };
        return Ok(if ok {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        });
    }
    let recovery_enabled = !matches!(
        &cli.command,
        Commands::Status
            | Commands::Start
            | Commands::Stop
            | Commands::Open
            | Commands::Background { .. }
            | Commands::Restart
            | Commands::Connect(_)
            | Commands::Login(_)
    );
    let resolved_session =
        wenlan_types::session_context::resolve_session_from_env(cli.session.as_deref());
    for warning in &resolved_session.warnings {
        eprintln!("wenlan: {warning}");
    }
    let project_rules = match &cli.command {
        Commands::Wiki { command } if command.is_project_rules() => Some(command),
        _ => None,
    };
    let session = resolved_session.session;
    let rules_checkout = project_rules.is_some_and(commands::wiki::WikiCommand::is_rules_checkout);
    let base_client = if let Some(command) = project_rules {
        if cli.all_spaces {
            anyhow::bail!("Repository rule proposals belong to one Space. Use --space <Space>.");
        }
        if rules_checkout
            && (cli.space.is_some()
                || std::env::var_os("WENLAN_SPACE")
                    .is_some_and(|space| !space.to_string_lossy().trim().is_empty()))
        {
            anyhow::bail!("The checkout selects its own readable project layers. Run without --space or WENLAN_SPACE.");
        }
        command.require_project_rules_caller(agent_name.is_some() || session.is_some())?;
        client::WenlanClient::for_local_rules()?
    } else {
        client::WenlanClient::from_env_with_context(agent_name.as_deref(), None)?
            .with_session(session.clone())?
            .with_recovery(recovery_enabled)
    };
    let is_brief_update = matches!(
        &cli.command,
        Commands::Brief(args) if args.is_update()
    ) || matches!(&cli.command, Commands::Handoff(_));
    let is_outbox = matches!(&cli.command, Commands::Outbox { .. });
    // A rule for every Space, and the removal of a rule by its id, are not
    // for the Space this folder or the default points at: they carry a
    // Space only when `WENLAN_SPACE` limits this terminal to one, and then
    // the daemon holds them to it (UX-SKILL-14).
    let is_rule_beyond_one_space = matches!(
        &cli.command,
        Commands::Rules {
            command: Some(
                commands::skills::RulesCmd::Add {
                    every_space: true,
                    ..
                } | commands::skills::RulesCmd::Remove { .. }
            ),
        }
    );
    let operation = match &cli.command {
        Commands::Wiki { .. } if rules_checkout => None,
        Commands::Rules { .. } if is_rule_beyond_one_space => None,
        Commands::Recall { .. } | Commands::Memories { .. } | Commands::Rules { .. } => {
            Some(CliSpaceOperation::Read)
        }
        Commands::Brief(args) if !args.is_update() => Some(CliSpaceOperation::Read),
        Commands::Capture { .. } | Commands::Wiki { .. } => Some(CliSpaceOperation::Write),
        _ => None,
    };
    let is_lint = matches!(&cli.command, Commands::Lint { .. });
    let is_export = matches!(&cli.command, Commands::Export { .. });
    // `sources add` (a folder, a file or an OKF bundle) and `sources save`
    // each write into one Space.
    let writes_one_space = matches!(
        &cli.command,
        Commands::Sources {
            command: commands::ingest::SourcesCommand::Add { .. }
                | commands::ingest::SourcesCommand::Save { .. }
        }
    );
    let mut effective_cli_space = cli.space.clone();
    let client = if is_outbox {
        if cli.space.is_some() || cli.all_spaces {
            anyhow::bail!("--space/--all-spaces are not supported by outbox commands");
        }
        effective_cli_space = None;
        base_client
    } else if is_rule_beyond_one_space {
        let strict_space = std::env::var("WENLAN_SPACE")
            .ok()
            .map(|space| space.trim().to_string())
            .filter(|space| !space.is_empty());
        let every_space = matches!(
            &cli.command,
            Commands::Rules {
                command: Some(commands::skills::RulesCmd::Add { .. })
            }
        );
        commands::skills::refuse_rule_beyond_one_space(
            every_space,
            strict_space.as_deref(),
            cli.space.as_deref(),
            cli.all_spaces,
        )?;
        effective_cli_space = strict_space;
        client::WenlanClient::from_env_with_context(
            agent_name.as_deref(),
            effective_cli_space.as_deref(),
        )?
        .with_session(session.clone())?
        .with_recovery(recovery_enabled)
    } else if let Some(operation) = operation {
        let context = match base_client.list_spaces().await {
            Ok(spaces) => {
                let registered = spaces.into_iter().map(|space| space.name).collect();
                let cwd = std::env::current_dir().ok();
                let context = resolve_cli_space(
                    cli.space.clone(),
                    cli.all_spaces,
                    cwd.clone(),
                    operation,
                    &registered,
                )?;
                // In a repository Wenlan does not know, a person is told once
                // about `wenlan init`. `repo` commands say their own.
                let wants_hint = context.space.is_none()
                    && !cli.all_spaces
                    && !cli.quiet
                    && matches!(cli.format.resolve(), output::ResolvedFormat::Table)
                    && !matches!(&cli.command, Commands::Wiki { .. });
                if let (true, Some(cwd)) = (wants_hint, cwd.as_deref()) {
                    commands::init::say_once_not_set_up(cwd, &registered);
                }
                context
            }
            Err(error)
                if matches!(&cli.command, Commands::Capture { .. })
                    && base_client.is_local()
                    && wenlan_cli::outbox::is_daemon_unreachable(&error) =>
            {
                if cli.all_spaces {
                    anyhow::bail!("--all-spaces is valid only for read commands");
                }
                let context = resolve_cli_space_offline(
                    cli.space.clone(),
                    cli.all_spaces,
                    std::env::current_dir().ok(),
                    operation,
                )?;
                if let Some(space) = context.space.as_deref() {
                    eprintln!(
                        "wenlan: Wenlan is not running; saved locally for Space '{space}' and will sync when it starts"
                    );
                }
                context
            }
            Err(error) => return Err(error),
        };
        effective_cli_space = context.space.clone();
        if project_rules.is_some() {
            // These person-only requests identify the Space in the path. Keep
            // the direct local connection and leave daemon recovery disabled.
            base_client
        } else {
            client::WenlanClient::from_env_with_context(
                agent_name.as_deref(),
                context.space.as_deref(),
            )?
            .with_session(session.clone())?
            .with_recovery(recovery_enabled)
        }
    } else if is_brief_update {
        let strict_space = std::env::var("WENLAN_SPACE").ok();
        effective_cli_space =
            resolve_native_read_space(strict_space.as_deref(), cli.space.as_deref(), false)?;
        client::WenlanClient::from_env_with_context(
            agent_name.as_deref(),
            effective_cli_space.as_deref(),
        )?
        .with_session(session.clone())?
        .with_recovery(recovery_enabled)
    } else if is_export {
        // Export is a native read: no flag means every Space, `--space X`
        // scopes the bundle to X, and a strict `WENLAN_SPACE` pin wins
        // (the resolver refuses `--all-spaces` under a pin).
        let strict_space = std::env::var("WENLAN_SPACE").ok();
        effective_cli_space = resolve_native_read_space(
            strict_space.as_deref(),
            cli.space.as_deref(),
            cli.all_spaces,
        )?;
        client::WenlanClient::from_env_with_context(
            agent_name.as_deref(),
            effective_cli_space.as_deref(),
        )?
        .with_session(session.clone())?
        .with_recovery(recovery_enabled)
    } else if writes_one_space {
        // An added or saved source writes into one Space: `--space X` or a
        // strict `WENLAN_SPACE` pin picks it. Otherwise an OKF bundle or a
        // saved document goes to the daemon's default Space and a folder's
        // pages stay unfiled.
        if cli.all_spaces {
            anyhow::bail!("--all-spaces is valid only for read commands");
        }
        let strict_space = std::env::var("WENLAN_SPACE").ok();
        effective_cli_space =
            resolve_native_read_space(strict_space.as_deref(), cli.space.as_deref(), false)?;
        client::WenlanClient::from_env_with_context(
            agent_name.as_deref(),
            effective_cli_space.as_deref(),
        )?
        .with_session(session.clone())?
        .with_recovery(recovery_enabled)
    } else if is_lint {
        let strict_space = std::env::var("WENLAN_SPACE").ok();
        effective_cli_space = resolve_native_read_space(
            strict_space.as_deref(),
            cli.space.as_deref(),
            cli.all_spaces,
        )?;
        base_client
    } else {
        if cli.space.is_some() || cli.all_spaces {
            anyhow::bail!("--space/--all-spaces are not supported by this command");
        }
        base_client
    };
    // Resolve Auto once based on stdout TTY state. Subcommands receive Json or Table only.
    let format = cli.format.resolve();
    match cli.command {
        Commands::Status => commands::status::run(&client, format, cli.quiet).await?,
        Commands::Open => commands::web::run(&client).await?,
        Commands::Setup {
            basic,
            model,
            anthropic_api_key_env,
            yes,
            config_only,
            finish,
        } => {
            commands::setup::run_setup(
                &client,
                commands::setup::SetupArgs {
                    basic,
                    model,
                    anthropic_api_key_env,
                    yes,
                    config_only,
                    finish,
                },
            )
            .await?
        }
        Commands::Init => commands::init::run(&client, format, cli.quiet).await?,
        Commands::Start => {
            commands::service::run_background(commands::service::BackgroundCommand::On).await?
        }
        Commands::Stop => {
            commands::service::run_background(commands::service::BackgroundCommand::Off).await?
        }
        Commands::Background { command } => commands::service::run_background(command).await?,
        Commands::Restart => commands::service::restart().await?,
        Commands::Tool { .. }
        | Commands::Follow(_)
        | Commands::Unfollow(_)
        | Commands::Follows(_) => {
            unreachable!("tool commands return before a daemon client is made")
        }
        Commands::Doctor { repair } => commands::setup::run_doctor(repair).await?,
        Commands::Lint {
            profile,
            allow_external,
            agent_assist,
            agent_submission,
        } => {
            return Ok(commands::lint::run(
                &client,
                format,
                cli.quiet,
                profile,
                effective_cli_space,
                allow_external,
                agent_assist,
                agent_submission,
            )
            .await)
        }
        Commands::Models { command } => commands::setup::run_model(command).await?,
        Commands::Keys { command } => commands::setup::run_key(command).await?,
        Commands::Steep { command } => {
            commands::setup::run_enrichment(&client, command, format).await?
        }
        Commands::Connect(args) => {
            commands::mcp::run_connect(&client, args, cli.quiet).await?;
        }
        Commands::Recall { query, limit } => {
            commands::recall::run(&client, format, cli.quiet, query, limit).await?
        }
        Commands::Rules { command } => {
            commands::skills::run_rules(&client, format, cli.quiet, command).await?
        }
        Commands::Skills { command } => {
            commands::skills::run_skills(&client, format, cli.quiet, command).await?
        }
        Commands::Brief(args) => {
            commands::brief::run(
                &client,
                format,
                cli.quiet,
                args,
                effective_cli_space,
                agent_name.as_deref(),
            )
            .await?
        }
        Commands::Handoff(args) => {
            commands::handoff::run(
                &client,
                format,
                cli.quiet,
                args,
                effective_cli_space.as_deref(),
            )
            .await?
        }
        Commands::Pages {
            action: Some(action),
            ..
        } => commands::pages::run_action(&client, format, cli.quiet, action).await?,
        Commands::Pages {
            action: None,
            query,
            limit,
            resolve_id,
            print,
        } => {
            commands::pages::run(&client, format, cli.quiet, query, limit, resolve_id, print)
                .await?
        }
        Commands::Sources { command } => {
            commands::ingest::run_sources(
                &client,
                format,
                cli.quiet,
                command,
                effective_cli_space.as_deref(),
            )
            .await?
        }
        Commands::Capture {
            text,
            file,
            memory_type,
        } => {
            commands::store::run(
                &client,
                format,
                cli.quiet,
                text,
                file,
                memory_type,
                effective_cli_space,
                agent_name.as_deref(),
            )
            .await?
        }
        Commands::Memories {
            action: _,
            limit,
            memory_type,
            pending,
        } => commands::list::run(&client, format, cli.quiet, limit, memory_type, pending).await?,
        Commands::Curate { action } => {
            commands::curate::run(&client, format, cli.quiet, action).await?
        }
        Commands::Agents { cmd } => commands::agents::run(&client, format, cli.quiet, cmd).await?,
        Commands::Spaces { cmd } => commands::space::run(&client, format, cli.quiet, cmd).await?,
        Commands::Team { cmd } => commands::team::run(&client, format, cli.quiet, cmd).await?,
        Commands::Login(args) => commands::login::run(args, cli.quiet).await?,
        Commands::Outbox { command } => {
            commands::outbox::run(&client, format, cli.quiet, command).await?
        }
        Commands::Entities { cmd } => {
            commands::entities::run(&client, format, cli.quiet, cmd).await?
        }
        Commands::Wiki { command } => {
            commands::wiki::run(
                &client,
                format,
                cli.quiet,
                effective_cli_space.as_deref(),
                command,
            )
            .await?
        }
        Commands::Export { command } => {
            commands::export::run(&client, format, cli.quiet, command).await?
        }
        Commands::Sweep => commands::sweep::run(&client, format, cli.quiet).await?,
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod cli_reference;

#[cfg(test)]
mod catalog_tests {
    use super::Cli;
    use clap::CommandFactory;
    use std::collections::BTreeSet;

    #[test]
    fn ux_skill_25_rulesync_action_hints_parse_with_the_real_cli() {
        use super::commands::{rulesync::RulesSetupCmd, skills::RulesCmd};
        use clap::Parser;
        use wenlan_types::rulesync::{
            BRING_IN_COMMAND, RESTORE_COMMAND, STATUS_COMMAND, SYNC_COMMAND, TO_REVIEW_COMMAND,
        };
        use wenlan_types::rulesync_skill::PENDING_SKILLS_COMMAND;

        let parse = |hint: &str| {
            let cli = Cli::try_parse_from(hint.split_ascii_whitespace()).unwrap();
            let super::Commands::Rules {
                command: Some(RulesCmd::Setup(action)),
            } = cli.command
            else {
                panic!("invalid setup hint {hint}");
            };
            action
        };
        assert!(matches!(
            parse(BRING_IN_COMMAND),
            RulesSetupCmd::Adopt { dry_run: false }
        ));
        assert!(matches!(
            parse("wenlan rules adopt --dry-run"),
            RulesSetupCmd::Adopt { dry_run: true }
        ));
        assert!(matches!(
            parse(STATUS_COMMAND),
            RulesSetupCmd::Status { sync: false }
        ));
        assert!(matches!(
            parse(SYNC_COMMAND),
            RulesSetupCmd::Status { sync: true }
        ));
        assert!(matches!(parse(RESTORE_COMMAND), RulesSetupCmd::Restore));
        for hint in [TO_REVIEW_COMMAND, PENDING_SKILLS_COMMAND] {
            assert!(matches!(
                parse(hint),
                RulesSetupCmd::Review { id: None, .. }
            ));
        }
        for answer in ["tool-only", "all-tools", "dismiss", "revert"] {
            assert!(matches!(
                parse(&format!("{TO_REVIEW_COMMAND} card {answer} --file example")),
                RulesSetupCmd::Review {
                    id: Some(_),
                    answer: Some(_),
                    file: Some(_)
                },
            ));
        }
        assert!(matches!(parse("wenlan rules sync"), RulesSetupCmd::Sync));
        assert!(matches!(
            parse("wenlan rules skill-updates"),
            RulesSetupCmd::SkillUpdates
        ));
        assert!(matches!(
            parse("wenlan rules settings source --dry-run"),
            RulesSetupCmd::Settings { dry_run: true, .. }
        ));
        let command = Cli::command();
        let rules = command.find_subcommand("rules").unwrap();
        for alias in ["sync", "skill-updates", "settings"] {
            assert!(rules.find_subcommand(alias).unwrap().is_hide_set());
        }
        let skills = command.find_subcommand("skills").unwrap();
        for alias in ["pending", "review"] {
            assert!(skills.find_subcommand(alias).unwrap().is_hide_set());
        }
    }

    #[test]
    fn project_rule_commands_take_one_space_and_no_yes_flag() {
        use super::commands::wiki::{ProjectRulesAction, WikiCommand};
        use super::Commands;
        use clap::Parser;

        let list = Cli::try_parse_from(["wenlan", "--space", "Clinic", "wiki", "rules-proposals"])
            .unwrap();
        assert_eq!(list.space.as_deref(), Some("Clinic"));
        assert!(matches!(
            list.command,
            Commands::Wiki {
                command: WikiCommand::RulesProposals
            }
        ));

        let review = Cli::try_parse_from([
            "wenlan",
            "wiki",
            "rules-review",
            "card",
            "accept",
            "--space",
            "Clinic",
        ])
        .unwrap();
        assert_eq!(review.space.as_deref(), Some("Clinic"));
        assert!(matches!(
            review.command,
            Commands::Wiki {
                command: WikiCommand::RulesReview {
                    action: ProjectRulesAction::Accept,
                    ..
                }
            }
        ));
        assert!(Cli::try_parse_from([
            "wenlan",
            "wiki",
            "rules-review",
            "card",
            "dismiss",
            "--yes"
        ])
        .is_err());
    }

    /// Every top-level clap subcommand must have a row in the M5 truth
    /// manifest, and the manifest must not list a command that no longer
    /// exists. Clap is the source of truth; the manifest is the catalog.
    #[test]
    fn truth_manifest_cli_rows_match_clap_subcommands() {
        let clap_names: BTreeSet<String> = Cli::command()
            .get_subcommands()
            .map(|c| format!("wenlan {}", c.get_name()))
            .filter(|name| name != "wenlan help")
            .collect();
        let manifest: BTreeSet<String> = wenlan_core::truth_manifest::CLI_READERS
            .iter()
            .map(|r| r.subcommand.to_string())
            .collect();
        assert_eq!(
            clap_names, manifest,
            "CLI_READERS in wenlan-core/src/truth_manifest.rs drifted from the clap `Commands` enum"
        );
    }

    /// `wenlan --help` is rendered from `HELP_GROUPS`, so every visible
    /// subcommand must be listed exactly once, and no hidden one may leak in.
    #[test]
    fn help_groups_cover_every_visible_subcommand() {
        let mut grouped: Vec<&str> = super::HELP_GROUPS
            .iter()
            .flat_map(|(_, names)| names.iter().copied())
            .collect();
        let unique: BTreeSet<&str> = grouped.iter().copied().collect();
        assert_eq!(unique.len(), grouped.len(), "duplicate name in HELP_GROUPS");
        grouped.sort_unstable();
        let visible: BTreeSet<String> = Cli::command()
            .get_subcommands()
            .filter(|sub| !sub.is_hide_set())
            .map(|sub| sub.get_name().to_string())
            .filter(|name| name != "help")
            .collect();
        assert_eq!(
            unique
                .iter()
                .map(ToString::to_string)
                .collect::<BTreeSet<_>>(),
            visible,
            "HELP_GROUPS in main.rs drifted from the visible clap subcommands"
        );
    }

    /// UX-CLI-3: turning Wenlan on and off uses the plain words, each one
    /// listed in the short help, and the earlier `background on|off` still
    /// parses.
    #[test]
    fn ux_cli_3_start_stop_and_restart_are_plain_top_level_words() {
        use super::Commands;
        use clap::Parser;

        let start = Cli::try_parse_from(["wenlan", "start"]).expect("start");
        assert!(matches!(start.command, Commands::Start));
        let stop = Cli::try_parse_from(["wenlan", "stop"]).expect("stop");
        assert!(matches!(stop.command, Commands::Stop));
        let restart = Cli::try_parse_from(["wenlan", "restart"]).expect("restart");
        assert!(matches!(restart.command, Commands::Restart));

        let listed: BTreeSet<&str> = super::HELP_GROUPS
            .iter()
            .flat_map(|(_, names)| names.iter().copied())
            .collect();
        for word in ["start", "stop", "restart"] {
            assert!(listed.contains(word), "{word} left the short help");
        }

        Cli::try_parse_from(["wenlan", "background", "on"]).expect("background on");
        Cli::try_parse_from(["wenlan", "background", "off"]).expect("background off");
    }

    /// `wenlan pages lock|unlock <id>` parse as the review-lock actions, and
    /// a plain query (or no argument) still browses pages as before.
    #[test]
    fn pages_lock_unlock_and_query_parse() {
        use super::commands::pages::PagesAction;
        use super::Commands;
        use clap::Parser;

        let lock = Cli::try_parse_from(["wenlan", "pages", "lock", "page_abc"]).expect("lock");
        let Commands::Pages {
            action: Some(PagesAction::Lock { page_id }),
            query: None,
            ..
        } = lock.command
        else {
            panic!("expected pages lock");
        };
        assert_eq!(page_id, "page_abc");

        let unlock =
            Cli::try_parse_from(["wenlan", "pages", "unlock", "page_abc"]).expect("unlock");
        let Commands::Pages {
            action: Some(PagesAction::Unlock { page_id }),
            ..
        } = unlock.command
        else {
            panic!("expected pages unlock");
        };
        assert_eq!(page_id, "page_abc");

        let browse = Cli::try_parse_from(["wenlan", "pages", "daemon", "--print"]).expect("query");
        let Commands::Pages {
            action: None,
            query: Some(query),
            print: true,
            ..
        } = browse.command
        else {
            panic!("expected a page query");
        };
        assert_eq!(query, "daemon");

        let list = Cli::try_parse_from(["wenlan", "pages"]).expect("list");
        assert!(matches!(
            list.command,
            Commands::Pages {
                action: None,
                query: None,
                ..
            }
        ));

        assert!(
            Cli::try_parse_from(["wenlan", "pages", "lock"]).is_err(),
            "lock needs a page id"
        );
    }

    #[test]
    fn brief_history_and_show_parse_beside_topic_and_update() {
        use super::commands::brief::BriefCommand;
        use super::Commands;
        use clap::Parser;

        let parse = |args: &[&str]| match Cli::try_parse_from(args).unwrap().command {
            Commands::Brief(brief) => brief,
            _ => panic!("expected brief"),
        };

        let plain = parse(&["wenlan", "brief"]);
        assert!(plain.command.is_none() && plain.topic.is_none());
        let topic = parse(&["wenlan", "brief", "release gate"]);
        assert_eq!(topic.topic.as_deref(), Some("release gate"));
        assert!(!topic.is_update());

        let default_limit = parse(&["wenlan", "brief", "history"]);
        assert!(matches!(
            default_limit.command,
            Some(BriefCommand::History { limit: 20 })
        ));
        let short = parse(&["wenlan", "brief", "history", "-l", "5"]);
        assert!(matches!(
            short.command,
            Some(BriefCommand::History { limit: 5 })
        ));
        let long = parse(&["wenlan", "brief", "history", "--limit", "7"]);
        assert!(matches!(
            long.command,
            Some(BriefCommand::History { limit: 7 })
        ));
        let show = parse(&["wenlan", "brief", "show", "--version", "3"]);
        assert!(matches!(
            show.command,
            Some(BriefCommand::Show { version: 3 })
        ));
        assert!(Cli::try_parse_from(["wenlan", "brief", "show"]).is_err());
        assert!(parse(&["wenlan", "brief", "update", "--file", "x.json"]).is_update());
    }

    #[test]
    fn entities_list_archive_restore_parse() {
        use super::commands::entities::EntitiesCmd;
        use super::Commands;
        use clap::Parser;
        use wenlan_types::entities::EntityStatus;

        let list = Cli::try_parse_from([
            "wenlan", "entities", "list", "--type", "person", "--status", "archived", "--limit",
            "5", "--offset", "10", "--query", "ali",
        ])
        .expect("list parses");
        let Commands::Entities {
            cmd:
                EntitiesCmd::List {
                    entity_type,
                    status,
                    query,
                    limit,
                    offset,
                },
        } = list.command
        else {
            panic!("expected entities list");
        };
        assert_eq!(entity_type.as_deref(), Some("person"));
        assert_eq!(status, Some(EntityStatus::Archived));
        assert_eq!(query.as_deref(), Some("ali"));
        assert_eq!((limit, offset), (5, 10));

        let archive = Cli::try_parse_from(["wenlan", "entities", "archive", "a", "b", "--dry-run"])
            .expect("archive parses");
        let Commands::Entities {
            cmd: EntitiesCmd::Archive { ids, dry_run },
        } = archive.command
        else {
            panic!("expected entities archive");
        };
        assert_eq!(ids, ["a", "b"]);
        assert!(dry_run);

        assert!(Cli::try_parse_from(["wenlan", "entities", "restore"]).is_err());
        assert!(Cli::try_parse_from(["wenlan", "entities", "list", "--status", "x"]).is_err());
    }
}
