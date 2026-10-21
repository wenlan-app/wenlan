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
    about = "Wenlan: a personal wiki your AI tools keep up to date. Capture, recall, and hand off across sessions."
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
    /// Check whether Wenlan is running.
    Status,
    /// Guided setup: get Wenlan running, optionally with a local model or an Anthropic key.
    Setup {
        /// Set up without a model or API key.
        #[arg(long)]
        basic: bool,
        /// Download and select a local model, for example qwen3-4b.
        #[arg(long, value_name = "MODEL_ID")]
        model: Option<String>,
        /// Read an Anthropic key from this environment variable.
        #[arg(long = "anthropic-api-key-env", value_name = "ENV_VAR")]
        anthropic_api_key_env: Option<String>,
        /// Skip confirmation prompts where possible.
        #[arg(short = 'y', long)]
        yes: bool,
    },
    /// Control whether Wenlan keeps running in the background.
    Background {
        #[command(subcommand)]
        command: commands::service::BackgroundCommand,
    },
    /// Restart the Wenlan background process. Required after an update.
    Restart,
    /// Diagnose runtime, model, and API key setup.
    Doctor,
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
    #[command(alias = "enrichment")]
    Steep {
        #[command(subcommand)]
        command: commands::setup::EnrichmentCommand,
    },
    /// Connect Wenlan to a supported agent or editor.
    Connect(commands::mcp::ConnectArgs),
    /// Search memories and pages by query (hybrid vector + keyword); --limit caps the primary results, plus up to 3 supplemental pages.
    #[command(hide = true)]
    Search {
        /// Search query.
        query: String,
        /// Max results (default 10).
        #[arg(short, long, default_value_t = 10)]
        limit: usize,
    },
    /// Recall scored memories for a query, plus the compiled pages that match it.
    Recall {
        /// Query to recall memories for.
        query: String,
        /// Max memories to return.
        #[arg(short, long, default_value_t = 10)]
        limit: usize,
    },
    /// Read the current Space Brief, optionally with related context.
    Brief(commands::brief::BriefArgs),
    /// Browse distilled pages, open one in your editor by title query, or
    /// lock a page so machine updates wait for your review.
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
    /// Manage folders and files Wenlan should learn from.
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
    /// List recent memories.
    Memories {
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
    /// Walk pending revisions (conflicts / merges) awaiting your accept or dismiss.
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
    /// Manage Spaces, the separate wikis Wenlan keeps (list, add, default, move, show).
    Spaces {
        #[command(subcommand)]
        cmd: commands::space::SpaceCmd,
    },
    /// Inspect or sync notes saved locally while Wenlan was not running.
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
    /// Force one bounded pass over every due ambient job now, instead of
    /// waiting for a quiet turn.
    #[command(hide = true)]
    Sweep,
}

/// Help groups for `wenlan --help`, in display order. Only names live here;
/// each description stays on its `Commands` doc comment. Hidden subcommands
/// (`search`, `sweep`, `agents`) are deliberately absent.
const HELP_GROUPS: &[(&str, &[&str])] = &[
    (
        "Daily",
        &[
            "brief", "capture", "recall", "pages", "memories", "curate", "lint", "spaces",
        ],
    ),
    (
        "Setup",
        &[
            "setup",
            "status",
            "doctor",
            "background",
            "restart",
            "connect",
            "models",
            "keys",
            "steep",
        ],
    ),
    ("Maintenance", &["sources", "outbox", "export", "entities"]),
];

/// Build a clap help template that lists the subcommands by `HELP_GROUPS`
/// instead of one flat `Commands:` block. Every line keeps clap's two-space
/// indent so help parsers that read the first word of indented lines still work.
fn grouped_help_template(cmd: &clap::Command) -> String {
    let abouts: std::collections::BTreeMap<&str, String> = cmd
        .get_subcommands()
        .map(|sub| {
            (
                sub.get_name(),
                sub.get_about().map(ToString::to_string).unwrap_or_default(),
            )
        })
        .collect();
    let width = HELP_GROUPS
        .iter()
        .flat_map(|(_, names)| names.iter())
        .map(|name| name.len())
        .max()
        .unwrap_or(0);
    let mut template =
        String::from("{before-help}{about-with-newline}\n{usage-heading} {usage}\n\n");
    for (heading, names) in HELP_GROUPS {
        template.push_str(heading);
        template.push_str(":\n");
        for name in names.iter() {
            let about = abouts.get(name).map(String::as_str).unwrap_or("");
            template.push_str(&format!("  {name:<width$}  {about}\n"));
        }
        template.push('\n');
    }
    template.push_str("Options:\n{options}{after-help}");
    template
}

#[tokio::main]
async fn main() -> anyhow::Result<ExitCode> {
    let cmd = Cli::command();
    let template = grouped_help_template(&cmd);
    let matches = cmd.help_template(template).get_matches();
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|error| error.exit());
    if cli.all_spaces && matches!(&cli.command, Commands::Brief(_)) {
        anyhow::bail!("Brief is owned by one Space; --all-spaces is not supported");
    }
    let environment_agent = std::env::var("WENLAN_AGENT_NAME").ok();
    let agent_name = resolve_agent_name(cli.agent_name.as_deref(), environment_agent.as_deref());
    let recovery_enabled = !matches!(
        &cli.command,
        Commands::Status | Commands::Background { .. } | Commands::Restart
    );
    let resolved_session =
        wenlan_types::session_context::resolve_session_from_env(cli.session.as_deref());
    for warning in &resolved_session.warnings {
        eprintln!("wenlan: {warning}");
    }
    let session = resolved_session.session;
    let base_client = client::WenlanClient::from_env_with_context(agent_name.as_deref(), None)?
        .with_session(session.clone())?
        .with_recovery(recovery_enabled);
    let is_brief_update = matches!(
        &cli.command,
        Commands::Brief(args) if args.is_update()
    );
    let is_outbox = matches!(&cli.command, Commands::Outbox { .. });
    let operation = match &cli.command {
        Commands::Search { .. } | Commands::Recall { .. } | Commands::Memories { .. } => {
            Some(CliSpaceOperation::Read)
        }
        Commands::Brief(args) if !args.is_update() => Some(CliSpaceOperation::Read),
        Commands::Capture { .. } => Some(CliSpaceOperation::Write),
        _ => None,
    };
    let is_lint = matches!(&cli.command, Commands::Lint { .. });
    let is_export = matches!(&cli.command, Commands::Export { .. });
    // An OKF import and `sources save` each write into one Space.
    let is_okf_import = matches!(
        &cli.command,
        Commands::Sources {
            command: commands::ingest::SourcesCommand::Add {
                source_type: commands::ingest::SourceKind::Okf,
                ..
            } | commands::ingest::SourcesCommand::Save { .. }
        }
    );
    let mut effective_cli_space = cli.space.clone();
    let client = if is_outbox {
        if cli.space.is_some() || cli.all_spaces {
            anyhow::bail!("--space/--all-spaces are not supported by outbox commands");
        }
        effective_cli_space = None;
        base_client
    } else if let Some(operation) = operation {
        let context = match base_client.list_spaces().await {
            Ok(spaces) => resolve_cli_space(
                cli.space.clone(),
                cli.all_spaces,
                std::env::current_dir().ok(),
                operation,
                &spaces.into_iter().map(|space| space.name).collect(),
            )?,
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
        client::WenlanClient::from_env_with_context(
            agent_name.as_deref(),
            context.space.as_deref(),
        )?
        .with_session(session.clone())?
        .with_recovery(recovery_enabled)
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
    } else if is_okf_import {
        // An OKF import or a saved source writes into one Space: `--space X`
        // or a strict `WENLAN_SPACE` pin picks it, otherwise the daemon's
        // default Space.
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
        Commands::Setup {
            basic,
            model,
            anthropic_api_key_env,
            yes,
        } => {
            commands::setup::run_setup(commands::setup::SetupArgs {
                basic,
                model,
                anthropic_api_key_env,
                yes,
            })
            .await?
        }
        Commands::Background { command } => commands::service::run_background(command).await?,
        Commands::Restart => commands::service::restart().await?,
        Commands::Doctor => commands::setup::run_doctor().await?,
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
        Commands::Steep { command } => commands::setup::run_enrichment(command).await?,
        Commands::Connect(args) => commands::mcp::run_connect(args, cli.quiet)?,
        Commands::Search { query, limit } => {
            commands::search::run(&client, format, cli.quiet, query, limit).await?
        }
        Commands::Recall { query, limit } => {
            commands::recall::run(&client, format, cli.quiet, query, limit).await?
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
            commands::ingest::run_sources(&client, format, cli.quiet, command).await?
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
            limit,
            memory_type,
            pending,
        } => commands::list::run(&client, format, cli.quiet, limit, memory_type, pending).await?,
        Commands::Curate { action } => {
            commands::curate::run(&client, format, cli.quiet, action).await?
        }
        Commands::Agents { cmd } => commands::agents::run(&client, format, cli.quiet, cmd).await?,
        Commands::Spaces { cmd } => commands::space::run(&client, format, cli.quiet, cmd).await?,
        Commands::Outbox { command } => {
            commands::outbox::run(&client, format, cli.quiet, command).await?
        }
        Commands::Entities { cmd } => {
            commands::entities::run(&client, format, cli.quiet, cmd).await?
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
