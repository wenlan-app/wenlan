use std::net::IpAddr;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use rmcp::ServiceExt;
use wenlan_mcp::client::{discover_origin_url, AgentIdentity, WenlanClient};
use wenlan_mcp::stdio::stdio;
use wenlan_mcp::tools::{ToolProfile, TransportMode, WenlanMcpServer};
use wenlan_mcp::{serve, token};

#[derive(Parser)]
#[command(
    name = "wenlan-mcp",
    about = "MCP server for Wenlan, the personal wiki your AI tools keep up to date",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Wenlan server URL (e.g. http://127.0.0.1:7878). Auto-discovers if not set.
    #[arg(long, global = true)]
    origin_url: Option<String>,

    /// Agent name for source_agent on writes.
    #[arg(long, global = true)]
    agent_name: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start Streamable HTTP MCP server for remote clients (claude.ai, ChatGPT)
    Serve(ServeArgs),
    /// Manage bearer tokens for authentication
    Token(TokenArgs),
}

#[derive(Parser)]
struct ServeArgs {
    /// Tool exposure profile
    #[arg(long, value_enum, default_value_t = ToolProfile::Standard)]
    tool_profile: ToolProfile,

    /// Port to listen on
    #[arg(long, default_value = "8080")]
    port: u16,

    /// Host to bind to
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    /// Bearer token for authentication
    #[arg(long)]
    token: Option<String>,

    /// Path to file containing the bearer token
    #[arg(long)]
    token_file: Option<PathBuf>,

    /// Read a bearer token from this environment variable, not process arguments
    #[arg(long, conflicts_with_all = ["token", "token_file", "no_auth"])]
    token_env: Option<String>,

    /// Disable authentication (only allowed on loopback)
    #[arg(long)]
    no_auth: bool,

    /// User ID for multi-user/cross-device support (optional)
    #[arg(long)]
    user_id: Option<String>,

    /// Comma-separated list of allowed Origin header values
    #[arg(long, default_value = "https://claude.ai,https://chatgpt.com")]
    allowed_origins: String,

    /// Query-only and web-write: serve the whole library (every Space and
    /// Uncategorized) instead of one pinned Space
    #[arg(long)]
    whole_library: bool,
}

#[derive(Parser)]
struct TokenArgs {
    #[command(subcommand)]
    action: TokenAction,
}

#[derive(Subcommand)]
enum TokenAction {
    /// Generate a new bearer token
    Generate {
        /// Output file path
        #[arg(long, default_value = "~/.config/wenlan-mcp/token")]
        output: String,
    },
}

fn default_token_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join("wenlan-mcp")
        .join("token")
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("wenlan_mcp=info".parse()?),
        )
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    let cli = Cli::parse();

    wenlan_mcp::lock_state::init_from_env();
    let whole_library = matches!(&cli.command, Some(Commands::Serve(args)) if args.whole_library);
    if whole_library && wenlan_mcp::lock_state::locked_space().is_none() {
        eprintln!("wenlan-mcp: whole-library scope, WENLAN_DEFAULT_SPACE ignored");
    } else if let Some(space) = wenlan_mcp::lock_state::locked_space() {
        eprintln!(
            "wenlan-mcp: WENLAN_SPACE strict pin active, space=\"{}\"",
            space
        );
    } else if let Some(space) = wenlan_mcp::lock_state::default_space() {
        eprintln!(
            "wenlan-mcp: WENLAN_DEFAULT_SPACE fallback active, space=\"{}\"",
            space
        );
    } else {
        eprintln!("wenlan-mcp: no process Space context");
    }

    match cli.command {
        None => run_stdio(cli.origin_url, effective_agent_name(cli.agent_name, None)).await,
        Some(Commands::Serve(args)) => {
            let agent_name = effective_agent_name(cli.agent_name, Some(&args));
            run_serve(args, cli.origin_url, agent_name).await
        }
        Some(Commands::Token(args)) => run_token(args),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EffectiveAgentName {
    name: String,
    explicit: bool,
}

impl EffectiveAgentName {
    fn into_identity(self) -> AgentIdentity {
        AgentIdentity::new(self.name, self.explicit)
    }
}

fn effective_agent_name(
    agent_name: Option<String>,
    serve_args: Option<&ServeArgs>,
) -> EffectiveAgentName {
    match agent_name {
        Some(name) => EffectiveAgentName {
            name,
            explicit: true,
        },
        None => EffectiveAgentName {
            name: if serve_args.is_some() {
                "remote-mcp".into()
            } else {
                "claude-code".into()
            },
            explicit: false,
        },
    }
}

async fn run_stdio(
    origin_url: Option<String>,
    agent_name: EffectiveAgentName,
) -> anyhow::Result<()> {
    let base_url = discover_origin_url(origin_url);
    tracing::info!("Connecting to Wenlan at {}", base_url);

    let client = WenlanClient::new(base_url);

    if let Some(msg) = client.version_handshake().await {
        tracing::warn!("{msg}");
        eprintln!("Warning: {msg}");
    }

    tokio::spawn(async {
        if let Some(msg) = wenlan_mcp::self_update_check::check().await {
            tracing::info!("{msg}");
            eprintln!("Note: {msg}");
        }
    });

    let server = WenlanMcpServer::new_with_identity(
        client,
        TransportMode::Stdio,
        agent_name.into_identity(),
        None,
    );
    let service = server
        .serve(stdio())
        .await
        .inspect_err(|e| tracing::error!("Failed to start MCP server: {}", e))?;

    tracing::info!("wenlan-mcp server running on stdio");
    service.waiting().await?;
    Ok(())
}

async fn run_serve(
    args: ServeArgs,
    origin_url: Option<String>,
    agent_name: EffectiveAgentName,
) -> anyhow::Result<()> {
    let connector_profile = serve::is_connector_profile(args.tool_profile);
    if connector_profile && args.no_auth {
        anyhow::bail!("{}", serve::connector_auth_error(args.tool_profile));
    }
    let resolved_token = resolve_token(&args)?;

    if connector_profile {
        if resolved_token
            .as_deref()
            .is_none_or(|token| token.trim().is_empty())
        {
            anyhow::bail!("{}", serve::connector_auth_error(args.tool_profile));
        }
        serve::connector_scope(
            args.tool_profile,
            wenlan_mcp::lock_state::locked_space(),
            args.whole_library,
        )?;
    } else if args.whole_library {
        anyhow::bail!("{}", serve::WHOLE_LIBRARY_PROFILE_ERROR);
    }

    if resolved_token.is_none() && !args.no_auth {
        anyhow::bail!(
            "Authentication required. Use --token, --token-file, --token-env, or --no-auth.\n\
             Generate a token with: wenlan-mcp token generate"
        );
    }
    if args.no_auth {
        let host_addr: IpAddr = args
            .host
            .parse()
            .map_err(|_| anyhow::anyhow!("Invalid host address: {}", args.host))?;
        if !host_addr.is_loopback() {
            anyhow::bail!(
                "--no-auth is only allowed on loopback addresses (127.0.0.1 or ::1), not {}",
                args.host
            );
        }
    }

    let base_url = discover_origin_url(origin_url);
    tracing::info!("Connecting to Wenlan at {}", base_url);

    if let Some(msg) = WenlanClient::new(base_url.clone())
        .version_handshake()
        .await
    {
        tracing::warn!("{msg}");
        eprintln!("Warning: {msg}");
    }

    tokio::spawn(async {
        if let Some(msg) = wenlan_mcp::self_update_check::check().await {
            tracing::info!("{msg}");
            eprintln!("Note: {msg}");
        }
    });

    let allowed_origins: Vec<String> = args
        .allowed_origins
        .split(',')
        .map(|s| s.trim().to_string())
        .collect();

    let config = serve::ServeConfig {
        port: args.port,
        host: args.host,
        origin_url: base_url,
        token: resolved_token,
        agent_name: agent_name.name,
        agent_name_explicit: agent_name.explicit,
        user_id: args.user_id,
        allowed_origins,
        whole_library: args.whole_library,
    };

    serve::run_serve_with_profile(config, args.tool_profile).await
}

fn resolve_token(args: &ServeArgs) -> anyhow::Result<Option<String>> {
    resolve_token_with_env(args, |name| std::env::var(name).ok())
}

fn resolve_token_with_env(
    args: &ServeArgs,
    read_env: impl FnOnce(&str) -> Option<String>,
) -> anyhow::Result<Option<String>> {
    if let Some(ref t) = args.token {
        return Ok(Some(t.clone()));
    }
    if let Some(ref path) = args.token_file {
        let t = token::read_token(path)?;
        return Ok(Some(t));
    }
    if let Some(ref name) = args.token_env {
        if name.is_empty()
            || name.len() > 128
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            anyhow::bail!("Invalid bearer token environment variable name");
        }
        let value = read_env(name)
            .filter(|value| {
                (32..=128).contains(&value.len())
                    && value
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
            })
            .ok_or_else(|| {
                anyhow::anyhow!("Bearer token environment variable is missing or invalid")
            })?;
        return Ok(Some(value));
    }
    if args.no_auth {
        return Ok(None);
    }
    let default_path = default_token_path();
    if default_path.exists() {
        let t = token::read_token(&default_path)?;
        tracing::info!("Using token from {}", default_path.display());
        return Ok(Some(t));
    }
    Ok(None)
}

fn run_token(args: TokenArgs) -> anyhow::Result<()> {
    match args.action {
        TokenAction::Generate { output } => {
            let path = if let Some(rest) = output.strip_prefix("~/") {
                let home = dirs::home_dir()
                    .ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?;
                home.join(rest)
            } else {
                PathBuf::from(&output)
            };

            let new_token = token::generate_token();
            token::write_token(&path, &new_token)?;

            eprintln!("Token saved to {}", path.display());
            eprintln!("Token: {}", new_token);
            eprintln!();
            eprintln!("Use with: wenlan-mcp serve --token-file {}", path.display());
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn explicit_agent(name: &str) -> EffectiveAgentName {
        EffectiveAgentName {
            name: name.to_string(),
            explicit: true,
        }
    }

    #[test]
    fn parses_stdio_agent_name_override() {
        let cli = Cli::try_parse_from(["wenlan-mcp", "--agent-name", "codex"])
            .expect("parse stdio agent override");

        assert_eq!(cli.agent_name.as_deref(), Some("codex"));
        assert!(cli.command.is_none());
    }

    #[test]
    fn stdio_default_agent_name_is_implicit() {
        assert_eq!(
            effective_agent_name(None, None),
            EffectiveAgentName {
                name: "claude-code".into(),
                explicit: false,
            }
        );
    }

    #[test]
    fn serve_default_agent_name_stays_remote_mcp() {
        let cli = Cli::try_parse_from(["wenlan-mcp", "serve", "--no-auth"])
            .expect("parse serve defaults");

        let Some(Commands::Serve(args)) = cli.command else {
            panic!("expected serve command");
        };

        assert_eq!(
            effective_agent_name(cli.agent_name, Some(&args)),
            EffectiveAgentName {
                name: "remote-mcp".into(),
                explicit: false,
            }
        );
    }

    #[test]
    fn serve_accepts_global_agent_name_override() {
        let cli = Cli::try_parse_from([
            "wenlan-mcp",
            "serve",
            "--no-auth",
            "--agent-name",
            "chatgpt",
        ])
        .expect("parse serve agent override");

        let Some(Commands::Serve(args)) = cli.command else {
            panic!("expected serve command");
        };

        assert_eq!(
            effective_agent_name(cli.agent_name, Some(&args)),
            explicit_agent("chatgpt")
        );
    }

    #[test]
    fn serve_tool_profile_defaults_to_standard() {
        let cli = Cli::try_parse_from(["wenlan-mcp", "serve", "--no-auth"])
            .expect("parse serve default tool profile");

        let Some(Commands::Serve(args)) = cli.command else {
            panic!("expected serve command");
        };

        assert_eq!(args.tool_profile, ToolProfile::Standard);
    }

    #[test]
    fn serve_accepts_query_only_tool_profile() {
        let cli = Cli::try_parse_from([
            "wenlan-mcp",
            "serve",
            "--tool-profile",
            "query-only",
            "--token",
            "secret",
        ])
        .expect("parse query-only tool profile");

        let Some(Commands::Serve(args)) = cli.command else {
            panic!("expected serve command");
        };

        assert_eq!(args.tool_profile, ToolProfile::QueryOnly);
    }

    #[test]
    fn serve_accepts_web_write_tool_profile() {
        let cli = Cli::try_parse_from([
            "wenlan-mcp",
            "serve",
            "--tool-profile",
            "web-write",
            "--token-env",
            "WENLAN_TEST_TOKEN",
            "--agent-name",
            "web-ai",
        ])
        .expect("parse web-write tool profile");

        let Some(Commands::Serve(args)) = cli.command else {
            panic!("expected serve command");
        };

        assert_eq!(args.tool_profile, ToolProfile::WebWrite);
        assert_eq!(
            effective_agent_name(cli.agent_name, Some(&args)),
            explicit_agent("web-ai")
        );
    }

    #[test]
    fn serve_rejects_invalid_tool_profile() {
        let result = Cli::try_parse_from(["wenlan-mcp", "serve", "--tool-profile", "invalid"]);

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn query_only_rejects_no_auth_before_startup() {
        let args = ServeArgs {
            tool_profile: ToolProfile::QueryOnly,
            port: 0,
            host: "127.0.0.1".into(),
            token: None,
            token_file: None,
            token_env: None,
            no_auth: true,
            user_id: None,
            allowed_origins: "*".into(),
            whole_library: false,
        };

        let error = run_serve(
            args,
            Some("http://127.0.0.1:19999".into()),
            explicit_agent("test"),
        )
        .await
        .expect_err("query-only --no-auth must be rejected");
        assert_eq!(error.to_string(), serve::QUERY_ONLY_AUTH_ERROR);
    }

    #[tokio::test]
    async fn web_write_rejects_no_auth_and_missing_token_before_startup() {
        for (no_auth, token) in [(true, None), (false, None), (false, Some(" ".to_string()))] {
            let args = ServeArgs {
                tool_profile: ToolProfile::WebWrite,
                port: 0,
                host: "127.0.0.1".into(),
                token,
                token_file: None,
                token_env: None,
                no_auth,
                user_id: None,
                allowed_origins: "*".into(),
                whole_library: false,
            };

            let error = run_serve(
                args,
                Some("http://127.0.0.1:19999".into()),
                explicit_agent("test"),
            )
            .await
            .expect_err("web-write without a bearer token must be rejected");
            assert_eq!(error.to_string(), serve::WEB_WRITE_AUTH_ERROR);
        }
    }

    #[tokio::test]
    async fn web_write_rejects_missing_space_pin_before_startup() {
        let args = ServeArgs {
            tool_profile: ToolProfile::WebWrite,
            port: 0,
            host: "127.0.0.1".into(),
            token: Some("secret".into()),
            token_file: None,
            token_env: None,
            no_auth: false,
            user_id: None,
            allowed_origins: "*".into(),
            whole_library: false,
        };

        let error = run_serve(
            args,
            Some("http://127.0.0.1:19999".into()),
            explicit_agent("test"),
        )
        .await
        .expect_err("web-write without a strict Space pin must be rejected");
        assert_eq!(error.to_string(), serve::WEB_WRITE_SPACE_ERROR);
    }

    #[tokio::test]
    async fn query_only_rejects_missing_space_pin_before_startup() {
        let args = ServeArgs {
            tool_profile: ToolProfile::QueryOnly,
            port: 0,
            host: "127.0.0.1".into(),
            token: Some("secret".into()),
            token_file: None,
            token_env: None,
            no_auth: false,
            user_id: None,
            allowed_origins: "*".into(),
            whole_library: false,
        };

        let error = run_serve(
            args,
            Some("http://127.0.0.1:19999".into()),
            explicit_agent("test"),
        )
        .await
        .expect_err("query-only without a strict Space pin must be rejected");
        assert_eq!(error.to_string(), serve::QUERY_ONLY_SPACE_ERROR);
    }

    #[test]
    fn explicit_environment_token_is_resolved_without_an_argv_secret() {
        let cli = Cli::try_parse_from([
            "wenlan-mcp",
            "serve",
            "--tool-profile",
            "query-only",
            "--token-env",
            "WENLAN_REMOTE_MCP_TOKEN",
        ])
        .unwrap();
        let Some(Commands::Serve(args)) = cli.command else {
            panic!("expected serve");
        };
        let expected = "s".repeat(64);
        let resolved = resolve_token_with_env(&args, |name| {
            assert_eq!(name, "WENLAN_REMOTE_MCP_TOKEN");
            Some(expected.clone())
        })
        .unwrap();
        assert_eq!(resolved.as_deref(), Some(expected.as_str()));
        assert!(args.token.is_none());
        assert!(args.token_file.is_none());
        assert!(!args.no_auth);
    }

    #[test]
    fn environment_token_conflicts_with_other_auth_sources_and_no_auth() {
        for extra in [
            vec!["--no-auth"],
            vec!["--token", "other"],
            vec!["--token-file", "/tmp/other"],
        ] {
            let mut argv = vec![
                "wenlan-mcp",
                "serve",
                "--token-env",
                "WENLAN_REMOTE_MCP_TOKEN",
            ];
            argv.extend(extra);
            assert!(Cli::try_parse_from(argv).is_err());
        }
    }

    #[test]
    fn missing_or_invalid_environment_token_never_falls_back_or_leaks_its_value() {
        let cli = Cli::try_parse_from([
            "wenlan-mcp",
            "serve",
            "--token-env",
            "WENLAN_REMOTE_MCP_TOKEN",
        ])
        .unwrap();
        let Some(Commands::Serve(args)) = cli.command else {
            panic!("expected serve");
        };
        for value in [
            None,
            Some(String::new()),
            Some("short".into()),
            Some("PRIVATE_VALUE\n".repeat(5)),
            Some("x".repeat(129)),
        ] {
            let error = resolve_token_with_env(&args, |_| value).unwrap_err();
            assert_eq!(
                error.to_string(),
                "Bearer token environment variable is missing or invalid"
            );
            assert!(!error.to_string().contains("PRIVATE_VALUE"));
        }
    }

    #[test]
    fn invalid_environment_name_is_rejected_before_lookup() {
        let cli =
            Cli::try_parse_from(["wenlan-mcp", "serve", "--token-env", "INVALID=NAME"]).unwrap();
        let Some(Commands::Serve(args)) = cli.command else {
            panic!("expected serve");
        };
        assert!(
            resolve_token_with_env(&args, |_| panic!("must not look up an invalid name")).is_err()
        );
    }

    #[test]
    fn legacy_explicit_token_and_no_auth_do_not_read_environment() {
        for extra in [vec!["--no-auth"], vec!["--token", "existing-token"]] {
            let mut argv = vec!["wenlan-mcp", "serve"];
            argv.extend(extra);
            let cli = Cli::try_parse_from(argv).unwrap();
            let Some(Commands::Serve(args)) = cli.command else {
                panic!("expected serve");
            };
            let result =
                resolve_token_with_env(&args, |_| panic!("no implicit environment fallback"))
                    .unwrap();
            assert_eq!(result, args.token);
        }
    }
}
