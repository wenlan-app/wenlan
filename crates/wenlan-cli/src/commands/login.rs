// SPDX-License-Identifier: Apache-2.0
//! `wenlan login <team address> --tool <tool>`: connect an AI tool on this
//! computer to a team with a short code, and write its MCP settings.
//!
//! The server gives a long code only this program knows and a short one
//! for the person to type in a browser (any device). Once they allow it,
//! this program collects a tool key: it reaches the team's `/mcp` and
//! nothing else. The key is kept apart from the desktop app's member key
//! (never replacing it), in a file only this user can read, and written
//! into the tool's own MCP settings. It never goes in a URL, a command
//! line or a log.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use clap::Args;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use wenlan_types::team::{TeamDeviceGrant, TeamDeviceRequest, TeamDeviceStart, TeamSpaceAccess};

/// Where tool keys are kept, under the data root.
pub const TOOL_KEYS_FILE: &str = "team-tool-keys.json";

#[derive(Debug, Args)]
pub struct LoginArgs {
    /// The team's address, such as `https://team.example.com`.
    pub server: String,
    /// The AI tool to connect: `claude-code`, `codex` or `cursor`.
    #[arg(long, value_name = "TOOL")]
    pub tool: String,
}

/// The AI tools `wenlan login` writes settings for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    ClaudeCode,
    Codex,
    Cursor,
}

impl Tool {
    pub fn parse(raw: &str) -> Option<Self> {
        let plain: String = raw
            .trim()
            .to_ascii_lowercase()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect();
        match plain.as_str() {
            "claudecode" | "claude" => Some(Self::ClaudeCode),
            "codex" => Some(Self::Codex),
            "cursor" => Some(Self::Cursor),
            _ => None,
        }
    }

    /// The name the person sees on the consent screen and in their list.
    pub fn display(self) -> &'static str {
        match self {
            Self::ClaudeCode => "Claude Code",
            Self::Codex => "Codex",
            Self::Cursor => "Cursor",
        }
    }
}

/// One kept tool key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeptToolKey {
    pub server: String,
    pub key_id: String,
    pub tool: String,
    pub key: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct KeptToolKeys {
    #[serde(default)]
    keys: Vec<KeptToolKey>,
}

pub async fn run(args: LoginArgs, quiet: bool) -> Result<()> {
    let tool = Tool::parse(&args.tool).ok_or_else(|| {
        anyhow!(
            "unknown tool `{}`: use claude-code, codex or cursor",
            args.tool
        )
    })?;
    let server = wenlan_core::shared_space::remote::check_url(args.server.trim())
        .map_err(|error| anyhow!(error))?;
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .context("starting an HTTP client")?;
    let started: TeamDeviceStart = {
        let response = http
            .post(format!("{server}/auth/device"))
            .json(&TeamDeviceRequest {
                tool: tool.display().to_string(),
            })
            .send()
            .await
            .with_context(|| format!("{server} cannot be reached"))?;
        match response.status().as_u16() {
            200 => response
                .json()
                .await
                .with_context(|| format!("{server} is not a Wenlan team server"))?,
            404 => bail!(
                "{server} cannot connect AI tools this way: it is an older server, or it has no address set"
            ),
            429 => bail!("{server} is busy. Try again in a minute."),
            status => bail!("{server} refused ({status})"),
        }
    };
    println!(
        "To let {} use this team, open\n{}\nand enter this code:\n\n  {}\n",
        tool.display(),
        started.verification_uri,
        started.user_code
    );
    println!(
        "Waiting for you to allow it… (the code works for {} minutes)",
        started.expires_in / 60
    );
    let grant = wait_for_grant(&http, &server, &started).await?;
    let path = wenlan_core::config::data_root().join(TOOL_KEYS_FILE);
    keep_key(
        &path,
        KeptToolKey {
            server: server.clone(),
            key_id: grant.key_id.clone(),
            tool: tool.display().to_string(),
            key: grant.key.clone(),
        },
    )?;
    let team = grant
        .team
        .as_ref()
        .and_then(|team| team.name.clone())
        .unwrap_or_else(|| host_of(&server));
    let entries = mcp_entries(&server, &team, &grant.spaces);
    if entries.is_empty() {
        println!(
            "Allowed, but this team has no Space open to you yet. Run this again once it does."
        );
        return Ok(());
    }
    let file = write_tool_settings(tool, &entries, &grant.key)?;
    if !quiet {
        println!(
            "✓ {} can now use {team}. Its MCP settings are written ({}).",
            tool.display(),
            file.display()
        );
        if tool != Tool::ClaudeCode {
            println!("Restart {} to load it.", tool.display());
        }
    }
    Ok(())
}

async fn wait_for_grant(
    http: &reqwest::Client,
    server: &str,
    started: &TeamDeviceStart,
) -> Result<TeamDeviceGrant> {
    let mut interval = Duration::from_secs(started.interval.max(1));
    let deadline = std::time::Instant::now() + Duration::from_secs(started.expires_in);
    loop {
        tokio::time::sleep(interval).await;
        if std::time::Instant::now() > deadline {
            bail!("The code expired. Run `wenlan login` again.");
        }
        let response = match http
            .post(format!("{server}/auth/device/token"))
            .json(&json!({ "device_code": started.device_code }))
            .send()
            .await
        {
            Ok(response) => response,
            // A dropped connection: try again at the next tick.
            Err(_) => continue,
        };
        if response.status().is_success() {
            return response
                .json()
                .await
                .with_context(|| format!("{server} answered something unexpected"));
        }
        if response.status().as_u16() == 429 {
            interval += Duration::from_secs(5);
            continue;
        }
        let said: Value = response.json().await.unwrap_or(Value::Null);
        match said["error"].as_str() {
            Some("authorization_pending") => {}
            Some("slow_down") => interval += Duration::from_secs(5),
            Some("access_denied") => bail!("It was not allowed. Nothing was written."),
            Some("expired_token") => bail!("The code expired. Run `wenlan login` again."),
            _ => bail!("{server} answered something unexpected"),
        }
    }
}

fn host_of(server: &str) -> String {
    reqwest::Url::parse(server)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .unwrap_or_else(|| server.to_string())
}

/// `Acme Design` → `acme-design`.
fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.trim().chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() {
        "team".into()
    } else {
        out
    }
}

/// One MCP server per Space the member can use: `(name, url)`.
pub fn mcp_entries(server: &str, team: &str, spaces: &[TeamSpaceAccess]) -> Vec<(String, String)> {
    let base = format!("wenlan-{}", slug(team));
    spaces
        .iter()
        .map(|space| {
            let name = if spaces.len() == 1 {
                base.clone()
            } else {
                format!("{base}-{}", slug(&space.space))
            };
            let mut url = reqwest::Url::parse(server).expect("a checked team address");
            url.path_segments_mut()
                .expect("an http address takes a path")
                .pop_if_empty()
                .push("mcp")
                .push(&space.space);
            (name, url.to_string())
        })
        .collect()
}

/// Keeps a tool key, by server and key id, beside any others. The file is
/// readable by this user only.
pub fn keep_key(path: &Path, kept: KeptToolKey) -> Result<()> {
    let mut all: KeptToolKeys = match fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text)
            .with_context(|| format!("{} is not a tool key list", path.display()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => KeptToolKeys::default(),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    all.keys
        .retain(|key| !(key.server == kept.server && key.key_id == kept.key_id));
    all.keys.push(kept);
    write_private(path, &format!("{}\n", serde_json::to_string_pretty(&all)?))
}

/// Writes `text` to `path` through a temporary file in the same folder
/// that only this user can read (on Unix, mode 0600; on Windows the file
/// is in the user's own profile).
fn write_private(path: &Path, text: &str) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("{} has no folder", path.display()))?;
    fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    let name = path
        .file_name()
        .ok_or_else(|| anyhow!("{} has no file name", path.display()))?
        .to_string_lossy();
    let tmp = parent.join(format!(".{name}.tmp.{}", std::process::id()));
    {
        use std::io::Write;
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&tmp)
            .with_context(|| format!("writing {}", tmp.display()))?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
    }
    let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    fs::rename(&tmp, &target).map_err(|error| {
        let _ = fs::remove_file(&tmp);
        anyhow!("replacing {}: {error}", target.display())
    })
}

/// The settings file a tool reads its user-wide MCP servers from.
pub fn settings_file(tool: Tool, home: &Path) -> PathBuf {
    match tool {
        Tool::ClaudeCode => home.join(".claude.json"),
        Tool::Codex => home.join(".codex").join("config.toml"),
        Tool::Cursor => home.join(".cursor").join("mcp.json"),
    }
}

fn write_tool_settings(tool: Tool, entries: &[(String, String)], key: &str) -> Result<PathBuf> {
    let home = super::mcp::home_dir_path()?;
    let path = settings_file(tool, &home);
    let old = match fs::read_to_string(&path) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    let new = match tool {
        Tool::Codex => codex_settings(old.as_deref(), entries, key)?,
        Tool::ClaudeCode | Tool::Cursor => json_settings(tool, old.as_deref(), entries, key)?,
    };
    if old.as_deref() == Some(new.as_str()) {
        return Ok(path);
    }
    write_private(&path, &new)?;
    Ok(path)
}

/// `mcpServers.<name> = { url, headers }` in a JSON settings file, the
/// rest of it untouched.
pub fn json_settings(
    tool: Tool,
    old: Option<&str>,
    entries: &[(String, String)],
    key: &str,
) -> Result<String> {
    let mut config: Value = match old {
        Some(text) if !text.trim().is_empty() => serde_json::from_str(text)
            .context("the settings file is not JSON; nothing was changed")?,
        _ => json!({}),
    };
    let servers = config
        .as_object_mut()
        .ok_or_else(|| anyhow!("the settings file is not a JSON object; nothing was changed"))?
        .entry("mcpServers")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| anyhow!("`mcpServers` is not a JSON object; nothing was changed"))?;
    for (name, url) in entries {
        let mut server = json!({
            "url": url,
            "headers": { "Authorization": format!("Bearer {key}") },
        });
        if tool == Tool::ClaudeCode {
            server["type"] = json!("http");
        }
        servers.insert(name.clone(), server);
    }
    Ok(format!("{}\n", serde_json::to_string_pretty(&config)?))
}

/// `[mcp_servers.<name>]` with `url` and `http_headers` in Codex's
/// `config.toml`, its comments and the rest kept.
pub fn codex_settings(
    old: Option<&str>,
    entries: &[(String, String)],
    key: &str,
) -> Result<String> {
    let mut doc: toml_edit::DocumentMut = old
        .unwrap_or_default()
        .parse()
        .context("config.toml is not TOML; nothing was changed")?;
    let servers = doc
        .entry("mcp_servers")
        .or_insert_with(|| {
            let mut table = toml_edit::Table::new();
            table.set_implicit(true);
            toml_edit::Item::Table(table)
        })
        .as_table_mut()
        .ok_or_else(|| anyhow!("`mcp_servers` is not a table; nothing was changed"))?;
    for (name, url) in entries {
        let mut table = toml_edit::Table::new();
        table.insert("url", toml_edit::value(url.as_str()));
        let mut headers = toml_edit::InlineTable::new();
        headers.insert("Authorization", format!("Bearer {key}").into());
        table.insert("http_headers", toml_edit::value(headers));
        servers.insert(name, toml_edit::Item::Table(table));
    }
    Ok(doc.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spaces(names: &[&str]) -> Vec<TeamSpaceAccess> {
        names
            .iter()
            .map(|name| TeamSpaceAccess {
                space: (*name).into(),
                access: wenlan_types::team::TeamAccess::Write,
                id: None,
            })
            .collect()
    }

    #[test]
    fn tools_are_named_loosely_and_shown_plainly() {
        for raw in ["claude-code", "Claude Code", "claude_code", "claude"] {
            assert_eq!(Tool::parse(raw), Some(Tool::ClaudeCode), "{raw}");
        }
        assert_eq!(Tool::parse("CODEX"), Some(Tool::Codex));
        assert_eq!(Tool::parse("cursor"), Some(Tool::Cursor));
        assert_eq!(Tool::parse("vim"), None);
        assert_eq!(Tool::ClaudeCode.display(), "Claude Code");
    }

    #[test]
    fn one_server_per_space_each_at_its_own_address() {
        assert_eq!(
            mcp_entries("https://team.acme.com", "Acme", &spaces(&["Product"])),
            [(
                "wenlan-acme".to_string(),
                "https://team.acme.com/mcp/Product".to_string()
            )]
        );
        assert_eq!(
            mcp_entries(
                "https://team.acme.com/",
                "Acme Co.",
                &spaces(&["Product", "Q4 plan"])
            ),
            [
                (
                    "wenlan-acme-co-product".to_string(),
                    "https://team.acme.com/mcp/Product".to_string()
                ),
                (
                    "wenlan-acme-co-q4-plan".to_string(),
                    "https://team.acme.com/mcp/Q4%20plan".to_string()
                ),
            ]
        );
    }

    #[test]
    fn json_settings_keep_everything_else() {
        let old = r#"{"theme":"dark","mcpServers":{"other":{"command":"x"}}}"#;
        let entries = [("wenlan-acme".to_string(), "https://t/mcp/P".to_string())];
        let new: Value = serde_json::from_str(
            &json_settings(Tool::ClaudeCode, Some(old), &entries, "wlt_k").unwrap(),
        )
        .unwrap();
        assert_eq!(new["theme"], "dark");
        assert_eq!(new["mcpServers"]["other"]["command"], "x");
        assert_eq!(
            new["mcpServers"]["wenlan-acme"],
            json!({"type": "http", "url": "https://t/mcp/P", "headers": {"Authorization": "Bearer wlt_k"}})
        );
        let cursor: Value =
            serde_json::from_str(&json_settings(Tool::Cursor, None, &entries, "wlt_k").unwrap())
                .unwrap();
        assert!(cursor["mcpServers"]["wenlan-acme"].get("type").is_none());
        assert!(json_settings(Tool::Cursor, Some("not json"), &entries, "k").is_err());
    }

    #[test]
    fn codex_settings_keep_comments_and_other_servers() {
        let old = "# mine\nmodel = \"o4\"\n\n[mcp_servers.other]\ncommand = \"x\"\n";
        let entries = [("wenlan-acme".to_string(), "https://t/mcp/P".to_string())];
        let new = codex_settings(Some(old), &entries, "wlt_k").unwrap();
        assert!(new.starts_with("# mine\nmodel = \"o4\""));
        assert!(new.contains("[mcp_servers.other]\ncommand = \"x\""));
        let parsed: toml_edit::DocumentMut = new.parse().unwrap();
        assert_eq!(
            parsed["mcp_servers"]["wenlan-acme"]["url"].as_str(),
            Some("https://t/mcp/P")
        );
        assert_eq!(
            parsed["mcp_servers"]["wenlan-acme"]["http_headers"]["Authorization"].as_str(),
            Some("Bearer wlt_k")
        );
        // Writing it again changes nothing.
        assert_eq!(codex_settings(Some(&new), &entries, "wlt_k").unwrap(), new);
    }

    #[test]
    fn kept_keys_are_private_and_one_per_server_and_id() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(TOOL_KEYS_FILE);
        let key = |id: &str, key: &str| KeptToolKey {
            server: "https://t".into(),
            key_id: id.into(),
            tool: "Codex".into(),
            key: key.into(),
        };
        keep_key(&path, key("a", "k1")).unwrap();
        keep_key(&path, key("b", "k2")).unwrap();
        keep_key(&path, key("a", "k3")).unwrap();
        let all: KeptToolKeys = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            all.keys.iter().map(|k| k.key.as_str()).collect::<Vec<_>>(),
            ["k2", "k3"]
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
