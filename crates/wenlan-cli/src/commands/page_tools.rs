// SPDX-License-Identifier: Apache-2.0
//! Page, memory and source commands that are the MCP tools under CLI names:
//! `write_page` is `wenlan pages write`, `forget` is `wenlan memories forget`.
//! Each one calls the tool itself (see `tool.rs`), so the rules a tool enforces
//! for an AI tool hold here too. What the CLI adds is what a person at a
//! terminal needs: a page named by its title, the current version looked up for
//! them, and a question before anything is deleted.

use std::io::{IsTerminal, Read, Write};
use std::path::PathBuf;

use anyhow::{anyhow, bail, Context, Result};
use clap::Subcommand;
use serde_json::{json, Value};

use super::tool::{Caller, Session};
use crate::output::ResolvedFormat;

#[derive(Subcommand, Debug)]
pub enum PageToolAction {
    /// Print a page: its title, text and sources (the read_page tool).
    Read {
        /// The page's title, part of it, or its id.
        page: String,
    },
    /// Create a page, or replace the text of an existing one (the write_page tool). Text comes from --file or stdin.
    Write {
        /// An existing page to replace: its title, part of it, or its id. Leave out to create a page.
        page: Option<String>,
        /// Title for a new page.
        #[arg(long)]
        title: Option<String>,
        /// Read the text from this file instead of stdin.
        #[arg(long, value_name = "PATH")]
        file: Option<PathBuf>,
        /// One-sentence summary.
        #[arg(long)]
        summary: Option<String>,
    },
    /// Change a page's title; links to it follow (the rename_page tool).
    Rename {
        /// The page's title, part of it, or its id.
        page: String,
        /// The new title.
        title: String,
    },
    /// Delete a page (the delete_page tool). Asks first.
    Delete {
        /// The page's title, part of it, or its id.
        page: String,
        /// Do not ask.
        #[arg(short = 'y', long)]
        yes: bool,
    },
    /// Move a page into a folder; "" is the top level (the move_page tool).
    Move {
        /// The page's title, part of it, or its id.
        page: String,
        /// Destination folder path.
        folder: String,
    },
    /// List a page's versions, newest first (the get_page_revisions tool).
    History {
        /// The page's title, part of it, or its id.
        page: String,
    },
    /// Put an earlier version of a page back (the restore_page_version tool).
    Restore {
        /// The page's title, part of it, or its id.
        page: String,
        /// The version to put back, as `wenlan pages history` lists it.
        version: i64,
    },
    /// Show the pages this page links to and the pages linking to it (the get_page_links tool).
    Links {
        /// The page's title, part of it, or its id.
        page: String,
    },
    /// Make a page follow another page, or check or stop it (the page_basis tool).
    /// When the other page changes, steep brings this one in line; a locked
    /// page waits for your review.
    Basis {
        /// mark: follow it; diff: what it said then and says now; confirm: this
        /// page still holds; remove: stop following.
        #[arg(value_enum)]
        action: BasisAction,
        /// The page that follows: its title, part of it, or its id.
        page: String,
        /// The page it follows: its title, part of it, or its id.
        basis: String,
    },
}

/// `wenlan pages basis` actions, the page_basis tool's own.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum BasisAction {
    Mark,
    Diff,
    Confirm,
    Remove,
}

#[derive(Subcommand, Debug)]
pub enum MemoryAction {
    /// List captures waiting for review (the list_pending tool).
    Pending {
        /// Max results.
        #[arg(short, long, default_value_t = 20)]
        limit: usize,
    },
    /// Confirm a pending memory (the confirm_memory tool).
    Confirm {
        /// The memory's id, as `wenlan memories` prints it.
        id: String,
    },
    /// Delete a memory (the forget tool). Asks first.
    Forget {
        /// The memory's id, as `wenlan memories` prints it.
        id: String,
        /// Do not ask.
        #[arg(short = 'y', long)]
        yes: bool,
    },
    /// Show how a memory changed over time (the get_memory_revisions tool).
    History {
        /// The memory's id.
        id: String,
    },
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadSourceKind {
    File,
    Webpage,
}

/// `wenlan sources read`: arguments of the read_source tool.
#[derive(clap::Args, Debug)]
pub struct ReadSourceArgs {
    /// The source's id.
    pub source_id: String,
    /// What the source is.
    #[arg(long, value_enum, default_value_t = ReadSourceKind::File)]
    pub kind: ReadSourceKind,
    /// Start at this character.
    #[arg(long, default_value_t = 0)]
    pub offset: usize,
    /// Max characters to print (1 to 16000).
    #[arg(long, default_value_t = 8000)]
    pub limit: usize,
}

fn looks_like_page_id(text: &str) -> bool {
    (text.starts_with("page_") || text.starts_with("concept_"))
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Pick the one page `wanted` names out of `pages` (`list_pages` rows): an
/// exact title first, else the only title containing it. Never a silent pick
/// between two.
pub fn pick_page(pages: &[Value], wanted: &str) -> Result<String> {
    let title = |page: &Value| {
        page.get("title")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let id = |page: &Value| {
        page.get("page_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let needle = wanted.to_lowercase();
    let exact: Vec<&Value> = pages
        .iter()
        .filter(|p| title(p).to_lowercase() == needle)
        .collect();
    let matches: Vec<&Value> = if exact.is_empty() {
        pages
            .iter()
            .filter(|p| title(p).to_lowercase().contains(&needle))
            .collect()
    } else {
        exact
    };
    match matches.as_slice() {
        [] => bail!("No page is titled '{wanted}'. List them with: wenlan pages"),
        [one] => Ok(id(one)),
        many => {
            let mut message = format!(
                "'{wanted}' matches {} pages. Use the full title or the id:\n",
                many.len()
            );
            for page in many.iter().take(10) {
                message.push_str(&format!("  {}  {}\n", id(page), title(page)));
            }
            bail!("{}", message.trim_end())
        }
    }
}

fn resolve_page(session: &mut Session, caller: &Caller, wanted: &str) -> Result<String> {
    if looks_like_page_id(wanted) {
        return Ok(wanted.to_string());
    }
    let mut pages = Vec::new();
    let mut offset = 0;
    loop {
        let mut arguments = json!({ "limit": 200, "offset": offset });
        if let Some(space) = &caller.space {
            arguments["space"] = json!(space);
        }
        let batch = session.call_json("list_pages", arguments)?;
        let rows = batch
            .get("pages")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let got = rows.len();
        pages.extend(rows);
        if got < 200 {
            break;
        }
        offset += got;
    }
    pick_page(&pages, wanted)
}

/// The folder paths `wanted` needs that `existing` lacks, outermost first.
pub fn missing_folders(existing: &[String], wanted: &str) -> Vec<(String, String)> {
    let mut missing = Vec::new();
    let mut parent = String::new();
    for name in wanted.split('/').filter(|part| !part.is_empty()) {
        let path = if parent.is_empty() {
            name.to_string()
        } else {
            format!("{parent}/{name}")
        };
        if !existing.iter().any(|have| have == &path) {
            missing.push((name.to_string(), parent.clone()));
        }
        parent = path;
    }
    missing
}

/// Make the destination folder when it is not there yet, as `mkdir -p` would.
fn ensure_folder(session: &mut Session, folder: &str) -> Result<()> {
    if folder.trim_matches('/').is_empty() {
        return Ok(());
    }
    let listed = session.call_json("list_folders", json!({}))?;
    let existing: Vec<String> = listed
        .get("folders")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|row| row.get("path").and_then(Value::as_str).map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    for (name, parent_path) in missing_folders(&existing, folder) {
        session.call_text(
            "create_folder",
            json!({ "name": name, "parent_path": parent_path }),
        )?;
    }
    Ok(())
}

fn read_page(session: &mut Session, page_id: &str) -> Result<Value> {
    let answer = session.call_json("read_page", json!({ "page_id": page_id }))?;
    answer
        .get("page")
        .cloned()
        .ok_or_else(|| anyhow!("read_page returned no page for {page_id}"))
}

fn version_of(page: &Value) -> Result<i64> {
    page.get("version")
        .and_then(Value::as_i64)
        .ok_or_else(|| anyhow!("the page has no version"))
}

fn title_of(page: &Value) -> &str {
    page.get("title").and_then(Value::as_str).unwrap_or("")
}

/// Ask before deleting. Without a terminal there is nobody to ask, so the
/// command refuses and names the flag.
fn confirm(question: &str, yes: bool) -> Result<bool> {
    if yes {
        return Ok(true);
    }
    if !std::io::stdin().is_terminal() {
        bail!("{question} Pass --yes to delete without being asked.");
    }
    eprint!("{question} [y/N] ");
    std::io::stderr().flush().ok();
    let mut answer = String::new();
    std::io::stdin()
        .read_line(&mut answer)
        .context("read the answer")?;
    Ok(matches!(answer.trim().to_lowercase().as_str(), "y" | "yes"))
}

fn text_input(file: Option<&PathBuf>) -> Result<String> {
    let text = match file {
        Some(path) => {
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?
        }
        None => {
            if std::io::stdin().is_terminal() {
                bail!("Give the page text with --file <PATH>, or pipe it in.");
            }
            let mut text = String::new();
            std::io::stdin()
                .read_to_string(&mut text)
                .context("read stdin")?;
            text
        }
    };
    if text.trim().is_empty() {
        bail!("The page text is empty.");
    }
    Ok(text)
}

/// One line for a person from the write_page tool's reply, which is written
/// for an AI tool (an id and `key: value` lines). A reply in any other shape
/// is shown as it is.
fn written_line(text: &str) -> String {
    let field = |key: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(key)?.strip_prefix(": "))
            .map(str::trim)
    };
    if text.starts_with("Page unchanged") {
        return "Nothing changed: the page already has this text.".to_owned();
    }
    let verb = if text.starts_with("Created page") {
        "Created"
    } else if text.starts_with("Refreshed page") || text.starts_with("Updated page") {
        "Updated"
    } else {
        return text.to_owned();
    };
    match field("space") {
        Some(space) if !space.is_empty() => format!("{verb} the page in {space}."),
        _ => format!("{verb} the page."),
    }
}

/// One line (or the two texts, for diff) for a person from the page_basis
/// tool's reply.
fn basis_as_text(action: BasisAction, page: &str, basis: &str, text: &str) -> String {
    let value = super::tool::json_in(text).ok();
    match action {
        BasisAction::Mark => format!(
            "\"{page}\" now follows \"{basis}\". When it changes, steep brings this page in line; \
             a locked page waits for your review.\n"
        ),
        BasisAction::Confirm => format!("\"{page}\" still holds after \"{basis}\" changed.\n"),
        BasisAction::Remove => format!("\"{page}\" no longer follows \"{basis}\".\n"),
        BasisAction::Diff => {
            let field = |key: &str| {
                value
                    .as_ref()
                    .and_then(|v| v.get(key))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            };
            match (field("from_content"), field("to_content")) {
                (Some(then), Some(now)) => format!(
                    "What \"{basis}\" said when \"{page}\" last agreed with it:\n\n{then}\n\n\
                     What it says now:\n\n{now}\n"
                ),
                _ => format!("{text}\n"),
            }
        }
    }
}

fn show(format: ResolvedFormat, quiet: bool, text: &str) -> Result<()> {
    if quiet {
        return Ok(());
    }
    match format {
        ResolvedFormat::Json => match super::tool::json_in(text) {
            Ok(value) => println!("{}", serde_json::to_string_pretty(&value)?),
            Err(_) => println!(
                "{}",
                serde_json::to_string_pretty(&json!({ "text": text }))?
            ),
        },
        ResolvedFormat::Table => println!("{text}"),
    }
    Ok(())
}

pub fn run_pages(
    action: PageToolAction,
    caller: &Caller,
    format: ResolvedFormat,
    quiet: bool,
) -> Result<()> {
    let mut session = Session::start(caller)?;
    match action {
        PageToolAction::Read { page } => {
            let id = resolve_page(&mut session, caller, &page)?;
            let text = session.call_text("read_page", json!({ "page_id": id }))?;
            if quiet {
                return Ok(());
            }
            match format {
                ResolvedFormat::Json => show(format, quiet, &text)?,
                ResolvedFormat::Table => print!("{}", page_as_text(&super::tool::json_in(&text)?)),
            }
        }
        PageToolAction::Write {
            page,
            title,
            file,
            summary,
        } => {
            let content = text_input(file.as_ref())?;
            let mut arguments = json!({ "content": content });
            if let Some(summary) = summary {
                arguments["summary"] = json!(summary);
            }
            match page {
                Some(page) => {
                    if title.is_some() {
                        bail!("--title names a new page. To change this page's title use: wenlan pages rename");
                    }
                    let id = resolve_page(&mut session, caller, &page)?;
                    let current = read_page(&mut session, &id)?;
                    arguments["page_id"] = json!(id);
                    arguments["expected_version"] = json!(version_of(&current)?);
                    // A page built from sources refuses a write that drops them.
                    arguments["source_memory_ids"] = current
                        .get("source_memory_ids")
                        .cloned()
                        .unwrap_or_else(|| json!([]));
                }
                None => {
                    let title = title.ok_or_else(|| anyhow!("A new page needs --title."))?;
                    arguments["title"] = json!(title);
                    if let Some(space) = &caller.space {
                        arguments["space"] = json!(space);
                    }
                }
            }
            let text = session.call_text("write_page", arguments)?;
            match format {
                ResolvedFormat::Table => show(format, quiet, &written_line(&text))?,
                ResolvedFormat::Json => show(format, quiet, &text)?,
            }
        }
        PageToolAction::Rename { page, title } => {
            let id = resolve_page(&mut session, caller, &page)?;
            let current = read_page(&mut session, &id)?;
            let text = session.call_text(
                "rename_page",
                json!({ "page_id": id, "title": title, "expected_version": version_of(&current)? }),
            )?;
            show(format, quiet, &text)?;
        }
        PageToolAction::Delete { page, yes } => {
            let id = resolve_page(&mut session, caller, &page)?;
            let current = read_page(&mut session, &id)?;
            if !confirm(&format!("Delete the page \"{}\"?", title_of(&current)), yes)? {
                if !quiet {
                    println!("Nothing deleted.");
                }
                return Ok(());
            }
            let text = session.call_text("delete_page", json!({ "page_id": id }))?;
            show(format, quiet, &text)?;
        }
        PageToolAction::Move { page, folder } => {
            let id = resolve_page(&mut session, caller, &page)?;
            let current = read_page(&mut session, &id)?;
            let storage_path = current
                .get("storage_path")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    anyhow!("This page is not a file page, so it cannot be moved into a folder.")
                })?;
            ensure_folder(&mut session, &folder)?;
            let operation_id = format!(
                "cli-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            );
            let text = session.call_text(
                "move_page",
                json!({
                    "page_id": id,
                    "folder_path": folder.trim_matches('/'),
                    "expected_storage_path": storage_path,
                    "operation_id": operation_id,
                }),
            )?;
            if !quiet && format == ResolvedFormat::Table {
                let now = super::tool::json_in(&text).ok().and_then(|v| {
                    v.get("storage_path")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                });
                match now {
                    Some(path) => println!("Moved \"{}\" to {path}", title_of(&current)),
                    None => println!("{text}"),
                }
            } else {
                show(format, quiet, &text)?;
            }
        }
        PageToolAction::History { page } => {
            let id = resolve_page(&mut session, caller, &page)?;
            let text = session.call_text("get_page_revisions", json!({ "page_id": id }))?;
            if quiet {
                return Ok(());
            }
            match format {
                ResolvedFormat::Json => show(format, quiet, &text)?,
                ResolvedFormat::Table => {
                    print!("{}", history_as_text(&super::tool::json_in(&text)?))
                }
            }
        }
        PageToolAction::Restore { page, version } => {
            let id = resolve_page(&mut session, caller, &page)?;
            let current = read_page(&mut session, &id)?;
            let text = session.call_text(
                "restore_page_version",
                json!({ "page_id": id, "version": version, "expected_version": version_of(&current)? }),
            )?;
            show(format, quiet, &text)?;
        }
        PageToolAction::Basis {
            action,
            page,
            basis,
        } => {
            let id = resolve_page(&mut session, caller, &page)?;
            let basis_id = resolve_page(&mut session, caller, &basis)?;
            let mut arguments = json!({
                "action": match action {
                    BasisAction::Mark => "mark",
                    BasisAction::Diff => "diff",
                    BasisAction::Confirm => "confirm",
                    BasisAction::Remove => "remove",
                },
                "page_id": id,
                "basis_page_id": basis_id,
            });
            // The version is the other page's as it reads now, looked up for
            // the person; an AI tool passes the one it read.
            if matches!(action, BasisAction::Mark | BasisAction::Confirm) {
                let current = read_page(&mut session, &basis_id)?;
                arguments["version"] = json!(version_of(&current)?);
            }
            let text = session.call_text("page_basis", arguments)?;
            if quiet {
                return Ok(());
            }
            match format {
                ResolvedFormat::Json => show(format, quiet, &text)?,
                ResolvedFormat::Table => {
                    print!("{}", basis_as_text(action, &page, &basis, &text))
                }
            }
        }
        PageToolAction::Links { page } => {
            let id = resolve_page(&mut session, caller, &page)?;
            let text = session.call_text("get_page_links", json!({ "page_id": id }))?;
            if quiet {
                return Ok(());
            }
            match format {
                ResolvedFormat::Json => show(format, quiet, &text)?,
                ResolvedFormat::Table => print!("{}", links_as_text(&super::tool::json_in(&text)?)),
            }
        }
    }
    Ok(())
}

pub fn run_memories(
    action: MemoryAction,
    caller: &Caller,
    format: ResolvedFormat,
    quiet: bool,
) -> Result<()> {
    let mut session = Session::start(caller)?;
    let text = match action {
        MemoryAction::Pending { limit } => {
            let mut arguments = json!({ "limit": limit });
            if let Some(space) = &caller.space {
                arguments["space"] = json!(space);
            }
            let text = session.call_text("list_pending", arguments)?;
            if !quiet && format == ResolvedFormat::Table {
                print!("{}", pending_as_text(&super::tool::json_in(&text)?));
                return Ok(());
            }
            text
        }
        MemoryAction::Confirm { id } => {
            session.call_text("confirm_memory", json!({ "memory_id": id }))?
        }
        MemoryAction::Forget { id, yes } => {
            if !confirm(&format!("Delete the memory {id}?"), yes)? {
                if !quiet {
                    println!("Nothing deleted.");
                }
                return Ok(());
            }
            session.call_text("forget", json!({ "memory_id": id }))?
        }
        MemoryAction::History { id } => {
            session.call_text("get_memory_revisions", json!({ "memory_id": id }))?
        }
    };
    show(format, quiet, &text)
}

pub fn run_read_source(
    args: ReadSourceArgs,
    caller: &Caller,
    format: ResolvedFormat,
    quiet: bool,
) -> Result<()> {
    let space = caller.space.clone().ok_or_else(|| {
        anyhow!("Say which Space the source is in: wenlan --space <SPACE> sources read <ID>")
    })?;
    let kind = match args.kind {
        ReadSourceKind::File => "file",
        ReadSourceKind::Webpage => "webpage",
    };
    let mut session = Session::start(caller)?;
    let text = session.call_text(
        "read_source",
        json!({
            "source": kind,
            "source_id": args.source_id,
            "space": space,
            "offset": args.offset,
            "limit": args.limit,
        }),
    )?;
    show(format, quiet, &text)
}

fn page_as_text(answer: &Value) -> String {
    let page = answer.get("page").unwrap_or(&Value::Null);
    let mut out = format!("# {}\n\n", title_of(page));
    out.push_str(
        page.get("content")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim_end(),
    );
    out.push('\n');
    let sources = answer
        .get("sources")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if !sources.is_empty() {
        out.push_str("\nSources:\n");
        for (index, source) in sources.iter().enumerate() {
            let name = ["title", "content", "source_id", "id"]
                .iter()
                .find_map(|key| source.get(*key).and_then(Value::as_str))
                .unwrap_or("");
            let line: String = name
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(100)
                .collect();
            out.push_str(&format!("  [{}] {line}\n", index + 1));
        }
    }
    out
}

fn history_as_text(answer: &Value) -> String {
    let rows = answer
        .get("history")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if rows.is_empty() {
        return "No earlier versions.\n".to_string();
    }
    let current = answer.get("current_version").and_then(Value::as_i64);
    let mut out = String::new();
    for row in &rows {
        let version = row.get("version").and_then(Value::as_i64).unwrap_or(0);
        let who = match row.get("kind").and_then(Value::as_str).unwrap_or("") {
            "ai_edit" => "AI",
            "user_edit" | "human_edit" => "you",
            other => other,
        };
        let changed = row.get("changed_sentences").and_then(Value::as_i64);
        out.push_str(&format!("  version {version:<4} {who:<6}"));
        if let Some(changed) = changed {
            out.push_str(&format!(
                " {changed} sentence{} changed",
                if changed == 1 { "" } else { "s" }
            ));
        }
        if Some(version) == current {
            out.push_str("  (current)");
        }
        out.push('\n');
    }
    out
}

fn links_as_text(answer: &Value) -> String {
    let list = |key: &str| -> Vec<String> {
        answer
            .get(key)
            .and_then(Value::as_array)
            .map(|rows| {
                rows.iter()
                    .map(|row| {
                        ["title", "target_title", "source_title", "page_id", "id"]
                            .iter()
                            .find_map(|k| row.get(*k).and_then(Value::as_str))
                            .map(str::to_string)
                            .unwrap_or_else(|| row.to_string())
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut out = String::new();
    for (label, key) in [("Links to", "outbound"), ("Linked from", "inbound")] {
        let rows = list(key);
        if rows.is_empty() {
            out.push_str(&format!("{label}: none\n"));
        } else {
            out.push_str(&format!("{label}:\n"));
            for row in rows {
                out.push_str(&format!("  {row}\n"));
            }
        }
    }
    out
}

fn pending_as_text(answer: &Value) -> String {
    let rows = answer.as_array().cloned().unwrap_or_default();
    if rows.is_empty() {
        return "Nothing is waiting for review.\n".to_string();
    }
    let mut out = String::new();
    for row in &rows {
        let id = row.get("source_id").and_then(Value::as_str).unwrap_or("");
        let text = row.get("content").and_then(Value::as_str).unwrap_or("");
        let line: String = text.lines().next().unwrap_or("").chars().take(90).collect();
        out.push_str(&format!("  {id}  {line}\n"));
    }
    out.push_str("\nKeep one with: wenlan memories confirm <ID>   Drop one with: wenlan memories forget <ID>\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_write_reply_becomes_one_line_for_a_person() {
        let created = "Created page page_1\nspace: Work\nspace_source: default\nwrite_outcome: created\nnext_step: Review the saved page.";
        assert_eq!(written_line(created), "Created the page in Work.");
        let updated = "Updated page page_1\nwrite_outcome: updated";
        assert_eq!(written_line(updated), "Updated the page.");
        let unchanged = "Page unchanged: page_1\nnext_step: nothing";
        assert_eq!(
            written_line(unchanged),
            "Nothing changed: the page already has this text."
        );
        assert_eq!(written_line("something else"), "something else");
    }

    fn pages() -> Vec<Value> {
        vec![
            json!({ "page_id": "page_a", "title": "Install notes" }),
            json!({ "page_id": "page_b", "title": "Install notes for Windows" }),
            json!({ "page_id": "page_c", "title": "Team plan" }),
        ]
    }

    #[test]
    fn an_exact_title_wins_over_a_longer_one_that_contains_it() {
        assert_eq!(pick_page(&pages(), "install notes").unwrap(), "page_a");
    }

    #[test]
    fn part_of_a_title_is_enough_when_only_one_page_has_it() {
        assert_eq!(pick_page(&pages(), "team").unwrap(), "page_c");
    }

    #[test]
    fn two_matches_are_listed_never_picked_between() {
        let error = pick_page(&pages(), "install").unwrap_err().to_string();
        assert!(error.contains("matches 2 pages"), "{error}");
        assert!(
            error.contains("page_a") && error.contains("page_b"),
            "{error}"
        );
    }

    #[test]
    fn only_the_folders_that_are_not_there_are_made_outermost_first() {
        let have = vec!["guides".to_string()];
        assert_eq!(
            missing_folders(&have, "guides/mac/arm"),
            vec![
                ("mac".to_string(), "guides".to_string()),
                ("arm".to_string(), "guides/mac".to_string())
            ]
        );
        assert!(missing_folders(&have, "guides").is_empty());
        assert!(missing_folders(&have, "/guides/").is_empty());
    }

    #[test]
    fn no_match_says_how_to_list_pages() {
        let error = pick_page(&pages(), "budget").unwrap_err().to_string();
        assert!(error.contains("wenlan pages"), "{error}");
    }

    #[test]
    fn an_id_is_recognised_and_a_title_starting_like_one_is_not() {
        assert!(looks_like_page_id("page_50c461cd-cf15"));
        assert!(!looks_like_page_id("page_ layout ideas"));
        assert!(!looks_like_page_id("Install notes"));
    }

    #[test]
    fn a_page_prints_as_title_text_and_numbered_sources() {
        let text = page_as_text(&json!({
            "page": { "title": "Install notes", "content": "One line.\n" },
            "sources": [{ "title": "Release checklist" }],
        }));
        assert_eq!(
            text,
            "# Install notes\n\nOne line.\n\nSources:\n  [1] Release checklist\n"
        );
    }

    #[test]
    fn history_marks_the_current_version() {
        let text = history_as_text(&json!({
            "current_version": 2,
            "history": [
                { "kind": "ai_edit", "version": 2, "changed_sentences": 1 },
                { "kind": "ai_edit", "version": 1, "changed_sentences": 3 },
            ],
        }));
        assert!(
            text.contains("version 2    AI     1 sentence changed  (current)"),
            "{text}"
        );
        assert!(
            text.contains("version 1    AI     3 sentences changed\n"),
            "{text}"
        );
    }

    #[test]
    fn pending_list_says_what_to_do_next() {
        let text = pending_as_text(&json!([{ "source_id": "mem_1", "content": "A fact." }]));
        assert!(text.contains("mem_1  A fact."), "{text}");
        assert!(text.contains("wenlan memories confirm"), "{text}");
        assert_eq!(
            pending_as_text(&json!([])),
            "Nothing is waiting for review.\n"
        );
    }

    #[test]
    fn deleting_without_a_terminal_needs_yes() {
        // Tests run with stdin not attached to a terminal.
        if !std::io::stdin().is_terminal() {
            let error = confirm("Delete the page \"X\"?", false)
                .unwrap_err()
                .to_string();
            assert!(error.contains("--yes"), "{error}");
        }
        assert!(confirm("Delete?", true).unwrap());
    }
}
