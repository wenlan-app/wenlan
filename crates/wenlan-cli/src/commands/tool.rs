// SPDX-License-Identifier: Apache-2.0
//! `wenlan tool`: call any Wenlan MCP tool from a terminal.
//!
//! The CLI does not reimplement the tools. It starts the `wenlan-mcp` that ships
//! beside it, speaks MCP over its stdio, and prints what the tool returned, so a
//! tool behaves the same here as it does inside an AI tool.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use anyhow::{anyhow, bail, Context, Result};
use clap::Subcommand;
use serde_json::{json, Map, Value};

use crate::output::ResolvedFormat as Format;

#[derive(Subcommand, Debug)]
pub enum ToolCmd {
    /// List every tool, with one line on what it does.
    List,
    /// Show one tool's description and the arguments it takes.
    Show {
        /// Tool name, as `wenlan tool list` prints it.
        name: String,
    },
    /// Call a tool. Arguments are `key=value` pairs, or one JSON object with --json.
    Call {
        /// Tool name, as `wenlan tool list` prints it.
        name: String,
        /// Arguments as key=value. A value that parses as JSON (a number, true, a list) is sent as that; anything else is sent as text. `key=@file` reads the value from a file, `key=-` from stdin.
        args: Vec<String>,
        /// All arguments as one JSON object. Not with key=value pairs.
        #[arg(long, value_name = "OBJECT", conflicts_with = "args")]
        json: Option<String>,
    },
}

/// Who is calling, as `wenlan-mcp` should see it.
#[derive(Debug, Default, Clone)]
pub struct Caller {
    pub host: Option<String>,
    pub agent_name: Option<String>,
    pub space: Option<String>,
}

pub fn run(cmd: ToolCmd, caller: &Caller, format: Format, quiet: bool) -> Result<bool> {
    match cmd {
        ToolCmd::List => {
            let tools = Session::start(caller)?.list_tools()?;
            if matches!(format, Format::Json) {
                println!("{}", serde_json::to_string_pretty(&tools)?);
            } else if !quiet {
                let width = tools.iter().map(|t| name_of(t).len()).max().unwrap_or(0);
                for tool in &tools {
                    println!("{:width$}  {}", name_of(tool), first_sentence(tool));
                }
            }
            Ok(true)
        }
        ToolCmd::Show { name } => {
            let tools = Session::start(caller)?.list_tools()?;
            let tool = find_tool(&tools, &name)?;
            if matches!(format, Format::Json) {
                println!("{}", serde_json::to_string_pretty(tool)?);
            } else if !quiet {
                print!("{}", describe(tool));
            }
            Ok(true)
        }
        ToolCmd::Call { name, args, json } => {
            let arguments = match json {
                Some(text) => parse_object(&text)?,
                None => pairs_to_object(&args, &mut std::io::stdin())?,
            };
            call_and_print(caller, &name, arguments, format, quiet)
        }
    }
}

/// Call one tool and print its answer. `Ok(false)` when the tool itself
/// reported an error (the text is already on stderr).
pub fn call_and_print(
    caller: &Caller,
    name: &str,
    arguments: Map<String, Value>,
    format: Format,
    quiet: bool,
) -> Result<bool> {
    let result = Session::start(caller)?.call(name, arguments)?;
    let failed = result.get("isError").and_then(Value::as_bool) == Some(true);
    let text = result_text(&result);
    if failed {
        eprintln!(
            "{}",
            if text.is_empty() {
                "The tool reported an error.".to_string()
            } else {
                text
            }
        );
        return Ok(false);
    }
    if matches!(format, Format::Json) {
        // A tool that answers in JSON is passed through as JSON; prose is wrapped.
        match serde_json::from_str::<Value>(&text) {
            Ok(value) => println!("{}", serde_json::to_string_pretty(&value)?),
            Err(_) => println!(
                "{}",
                serde_json::to_string_pretty(&json!({ "text": text }))?
            ),
        }
    } else if !quiet {
        println!("{text}");
    }
    Ok(true)
}

fn name_of(tool: &Value) -> &str {
    tool.get("name").and_then(Value::as_str).unwrap_or("")
}

fn first_sentence(tool: &Value) -> String {
    let text = tool
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("");
    let line = text.lines().next().unwrap_or("").trim();
    let cut = line
        .char_indices()
        .find(|&(i, c)| c == '.' && line[i + 1..].starts_with(' '))
        .map(|(i, _)| i + 1)
        .unwrap_or(line.len());
    let sentence = &line[..cut];
    if sentence.chars().count() > 110 {
        format!("{}…", sentence.chars().take(109).collect::<String>())
    } else {
        sentence.to_string()
    }
}

fn find_tool<'a>(tools: &'a [Value], name: &str) -> Result<&'a Value> {
    tools
        .iter()
        .find(|tool| name_of(tool) == name)
        .ok_or_else(|| anyhow!("There is no tool named '{name}'. See them with: wenlan tool list"))
}

fn describe(tool: &Value) -> String {
    let mut out = format!(
        "{}\n\n{}\n",
        name_of(tool),
        tool.get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
    );
    let schema = tool.get("inputSchema");
    let required: Vec<&str> = schema
        .and_then(|s| s.get("required"))
        .and_then(Value::as_array)
        .map(|names| names.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let Some(properties) = schema
        .and_then(|s| s.get("properties"))
        .and_then(Value::as_object)
    else {
        out.push_str("\nTakes no arguments.\n");
        return out;
    };
    if properties.is_empty() {
        out.push_str("\nTakes no arguments.\n");
        return out;
    }
    out.push_str("\nArguments:\n");
    for (key, spec) in properties {
        let need = if required.contains(&key.as_str()) {
            "required"
        } else {
            "optional"
        };
        let about = spec
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("");
        out.push_str(&format!("  {key} ({need})"));
        if !about.is_empty() {
            out.push_str(&format!(": {about}"));
        }
        out.push('\n');
    }
    out
}

fn parse_object(text: &str) -> Result<Map<String, Value>> {
    match serde_json::from_str::<Value>(text).context("--json is not valid JSON")? {
        Value::Object(map) => Ok(map),
        _ => bail!("--json must be one JSON object, like '{{\"query\": \"release plan\"}}'"),
    }
}

/// `key=value` pairs to a JSON object. A value that is valid JSON keeps its
/// type; anything else is text. `@path` reads a file, `-` reads stdin (once).
pub fn pairs_to_object(pairs: &[String], stdin: &mut dyn Read) -> Result<Map<String, Value>> {
    let mut map = Map::new();
    let mut stdin_used = false;
    for pair in pairs {
        let (key, raw) = pair
            .split_once('=')
            .ok_or_else(|| anyhow!("'{pair}' is not key=value"))?;
        if key.is_empty() {
            bail!("'{pair}' has no name before '='");
        }
        let value = if raw == "-" {
            if stdin_used {
                bail!("only one argument can read from stdin");
            }
            stdin_used = true;
            let mut text = String::new();
            stdin.read_to_string(&mut text).context("read stdin")?;
            Value::String(text)
        } else if let Some(path) = raw.strip_prefix('@') {
            Value::String(std::fs::read_to_string(path).with_context(|| format!("read {path}"))?)
        } else {
            match serde_json::from_str::<Value>(raw) {
                Ok(parsed) if !parsed.is_string() => parsed,
                _ => Value::String(raw.to_string()),
            }
        };
        if map.insert(key.to_string(), value).is_some() {
            bail!("'{key}' is given twice");
        }
    }
    Ok(map)
}

fn result_text(result: &Value) -> String {
    result
        .get("content")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// The JSON value in a tool answer: the whole text, or what follows a prose
/// first line (`version 3 (2 entries)\n{…}`).
pub fn json_in(text: &str) -> Result<Value> {
    if let Ok(value) = serde_json::from_str::<Value>(text) {
        return Ok(value);
    }
    let start = text
        .find(['{', '['])
        .ok_or_else(|| anyhow!("no JSON in the answer"))?;
    serde_json::from_str::<Value>(&text[start..]).context("parse the answer")
}

fn mcp_binary() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("WENLAN_MCP_BIN") {
        return Ok(PathBuf::from(path));
    }
    let exe = std::env::current_exe().context("find this program")?;
    let dir = exe.parent().context("find this program's folder")?;
    let name = format!("wenlan-mcp{}", std::env::consts::EXE_SUFFIX);
    let sibling = dir.join(&name);
    if sibling.is_file() {
        return Ok(sibling);
    }
    bail!(
        "{name} is not beside {}. It ships with Wenlan; reinstall with: npx -y wenlan setup",
        exe.display()
    )
}

/// One `wenlan-mcp` process, initialized and ready for requests.
pub struct Session {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl Session {
    pub fn start(caller: &Caller) -> Result<Self> {
        let binary = mcp_binary()?;
        let mut command = Command::new(&binary);
        if let Some(host) = &caller.host {
            command.args(["--origin-url", host]);
        }
        if let Some(agent) = &caller.agent_name {
            command.args(["--agent-name", agent]);
        }
        if let Some(space) = &caller.space {
            command.env("WENLAN_DEFAULT_SPACE", space);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("start {}", binary.display()))?;
        let stdin = child.stdin.take().context("open wenlan-mcp input")?;
        let stdout = BufReader::new(child.stdout.take().context("open wenlan-mcp output")?);
        let mut session = Session {
            child,
            stdin,
            stdout,
            next_id: 1,
        };
        session.request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "wenlan-cli", "version": env!("CARGO_PKG_VERSION") },
            }),
        )?;
        session.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))?;
        Ok(session)
    }

    fn send(&mut self, message: &Value) -> Result<()> {
        writeln!(self.stdin, "{message}").context("write to wenlan-mcp")?;
        self.stdin.flush().context("write to wenlan-mcp")
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))?;
        let mut line = String::new();
        loop {
            line.clear();
            if self
                .stdout
                .read_line(&mut line)
                .context("read from wenlan-mcp")?
                == 0
            {
                bail!("wenlan-mcp stopped before answering. Check that Wenlan is running: wenlan status");
            }
            let Ok(message) = serde_json::from_str::<Value>(line.trim()) else {
                continue;
            };
            // Notifications and requests from the server carry no matching id.
            if message.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = message.get("error") {
                let text = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown error");
                bail!("{text}");
            }
            return Ok(message.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    fn list_tools(&mut self) -> Result<Vec<Value>> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let params = match &cursor {
                Some(cursor) => json!({ "cursor": cursor }),
                None => json!({}),
            };
            let page = self.request("tools/list", params)?;
            if let Some(items) = page.get("tools").and_then(Value::as_array) {
                tools.extend(items.iter().cloned());
            }
            cursor = page
                .get("nextCursor")
                .and_then(Value::as_str)
                .map(str::to_string);
            if cursor.is_none() {
                break;
            }
        }
        tools.sort_by(|a, b| name_of(a).cmp(name_of(b)));
        Ok(tools)
    }

    /// Call a tool and return its text. A tool that reports an error becomes
    /// an `Err` carrying the tool's own sentence.
    pub fn call_text(&mut self, name: &str, arguments: Value) -> Result<String> {
        let Value::Object(arguments) = arguments else {
            bail!("tool arguments must be an object");
        };
        let result = self.call(name, arguments)?;
        let text = result_text(&result);
        if result.get("isError").and_then(Value::as_bool) == Some(true) {
            bail!(
                "{}",
                if text.is_empty() {
                    "The tool reported an error.".to_string()
                } else {
                    text
                }
            );
        }
        Ok(text)
    }

    /// Call a tool whose answer is JSON, skipping any prose line before it.
    pub fn call_json(&mut self, name: &str, arguments: Value) -> Result<Value> {
        let text = self.call_text(name, arguments)?;
        json_in(&text).with_context(|| format!("{name} did not answer in JSON"))
    }

    fn call(&mut self, name: &str, arguments: Map<String, Value>) -> Result<Value> {
        self.request(
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        )
        .map_err(|error| {
            if error.to_string().to_lowercase().contains("not found") {
                anyhow!("There is no tool named '{name}'. See them with: wenlan tool list")
            } else {
                error
            }
        })
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(items: &[&str]) -> Result<Map<String, Value>> {
        let owned: Vec<String> = items.iter().map(|s| s.to_string()).collect();
        pairs_to_object(&owned, &mut std::io::empty())
    }

    #[test]
    fn values_keep_json_types_and_everything_else_is_text() {
        let map = pairs(&[
            "query=release plan",
            "limit=5",
            "all=true",
            "ids=[\"a\",\"b\"]",
            "title=\"quoted\"",
        ])
        .unwrap();
        assert_eq!(map["query"], json!("release plan"));
        assert_eq!(map["limit"], json!(5));
        assert_eq!(map["all"], json!(true));
        assert_eq!(map["ids"], json!(["a", "b"]));
        assert_eq!(map["title"], json!("\"quoted\""));
    }

    #[test]
    fn a_value_can_come_from_a_file_or_stdin() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("body.md");
        std::fs::write(&path, "# Title\n\n42\n").unwrap();
        let owned = vec![format!("content=@{}", path.display()), "note=-".to_string()];
        let map = pairs_to_object(&owned, &mut "from stdin".as_bytes()).unwrap();
        assert_eq!(map["content"], json!("# Title\n\n42\n"));
        assert_eq!(map["note"], json!("from stdin"));
    }

    #[test]
    fn bad_pairs_say_what_is_wrong() {
        assert!(pairs(&["query"])
            .unwrap_err()
            .to_string()
            .contains("not key=value"));
        assert!(pairs(&["=x"]).unwrap_err().to_string().contains("no name"));
        assert!(pairs(&["a=1", "a=2"])
            .unwrap_err()
            .to_string()
            .contains("given twice"));
        assert!(pairs(&["a=-", "b=-"])
            .unwrap_err()
            .to_string()
            .contains("only one"));
    }

    #[test]
    fn json_must_be_an_object() {
        assert!(parse_object("[1]")
            .unwrap_err()
            .to_string()
            .contains("one JSON object"));
        assert_eq!(parse_object("{\"a\":1}").unwrap()["a"], json!(1));
    }

    #[test]
    fn list_line_is_the_first_sentence_only() {
        let tool = json!({ "name": "recall", "description": "Search memories. Use it before answering.\nMore." });
        assert_eq!(first_sentence(&tool), "Search memories.");
    }

    #[test]
    fn show_marks_required_and_optional_arguments() {
        let tool = json!({
            "name": "rename_page",
            "description": "Rename a page.",
            "inputSchema": {
                "properties": { "page_id": { "description": "The page." }, "note": {} },
                "required": ["page_id"],
            },
        });
        let text = describe(&tool);
        assert!(text.contains("page_id (required): The page."), "{text}");
        assert!(text.contains("note (optional)"), "{text}");
    }
}
