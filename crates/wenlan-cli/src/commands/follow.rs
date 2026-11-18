// SPDX-License-Identifier: Apache-2.0
//! `wenlan follow`, `wenlan unfollow`, `wenlan follows`: one short door for
//! "this follows that page", whether "this" is another page or a file of the
//! repository the Space is bound to.
//!
//! A page that follows a page is the page_basis tool (`wenlan pages basis`);
//! a file that follows a page is the doc_fix tool's follow actions (`wenlan
//! repo follow`). Both go through the tools themselves, so the rules they
//! enforce for an AI tool hold here too.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use super::page_tools::{read_page, resolve_page, title_of, version_of};
use super::tool::{json_in, Caller, Session};
use crate::output::ResolvedFormat;

/// `wenlan follow <what> <page>`.
#[derive(clap::Args, Debug)]
pub struct FollowArgs {
    /// What follows: a page (its title, part of it, or its id) or a file of
    /// this repository such as README.md.
    pub what: String,
    /// The page it follows: its title, part of it, or its id.
    pub page: String,
}

/// `wenlan unfollow <what> [page]`.
#[derive(clap::Args, Debug)]
pub struct UnfollowArgs {
    /// The page or file that stops following.
    pub what: String,
    /// The page it follows. Needed only when it follows more than one.
    pub page: Option<String>,
}

/// `wenlan follows [page]`.
#[derive(clap::Args, Debug)]
pub struct FollowsArgs {
    /// Show what this page follows. Leave out to list the files that follow
    /// a page and the pages waiting to catch up.
    pub page: Option<String>,
}

/// A file of the repository this folder is in, as the repository names it
/// (relative to its root, `/` separators). `None` when `what` is not a file.
fn repo_file(what: &str) -> Result<Option<String>> {
    let path = Path::new(what);
    if !path.is_file() {
        return Ok(None);
    }
    let full = std::fs::canonicalize(path).with_context(|| format!("read {what}"))?;
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(full.parent().unwrap_or(Path::new(".")))
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .context("run git")?;
    if !output.status.success() {
        bail!("{what} is not inside a git repository, so it cannot follow a page.");
    }
    let top = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    let top = std::fs::canonicalize(&top).unwrap_or(top);
    let relative = full
        .strip_prefix(&top)
        .with_context(|| format!("{what} is not inside {}", top.display()))?;
    Ok(Some(
        relative
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/"),
    ))
}

fn with_space(mut arguments: Value, caller: &Caller) -> Value {
    if let Some(space) = &caller.space {
        arguments["space"] = json!(space);
    }
    arguments
}

/// The files that follow pages, as the doc_fix tool lists them.
fn file_follows(session: &mut Session, caller: &Caller) -> Result<Vec<(String, Vec<String>)>> {
    let listed = session.call_json(
        "doc_fix",
        with_space(json!({ "action": "follows" }), caller),
    )?;
    Ok(listed
        .get("follows")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|follow| {
            let page = follow.get("page_id")?.as_str()?.to_string();
            let files = follow
                .get("files")?
                .as_array()?
                .iter()
                .filter_map(|file| file.as_str().map(str::to_string))
                .collect();
            Some((page, files))
        })
        .collect())
}

fn set_files(
    session: &mut Session,
    caller: &Caller,
    page_id: &str,
    files: &[String],
) -> Result<String> {
    if files.is_empty() {
        return session.call_text(
            "doc_fix",
            with_space(json!({ "action": "unfollow", "page_id": page_id }), caller),
        );
    }
    session.call_text(
        "doc_fix",
        with_space(
            json!({ "action": "follow", "page_id": page_id, "files": files }),
            caller,
        ),
    )
}

fn say(format: ResolvedFormat, quiet: bool, line: &str, reply: &str) -> Result<()> {
    if quiet {
        return Ok(());
    }
    match format {
        ResolvedFormat::Table => println!("{line}"),
        ResolvedFormat::Json => println!("{}", json_in(reply).unwrap_or(json!(reply))),
    }
    Ok(())
}

pub fn run_follow(
    args: FollowArgs,
    caller: &Caller,
    format: ResolvedFormat,
    quiet: bool,
) -> Result<()> {
    let mut session = Session::start(caller)?;
    let lead_id = resolve_page(&mut session, caller, &args.page)?;
    let lead = read_page(&mut session, &lead_id)?;
    let lead_title = title_of(&lead).to_string();
    if let Some(file) = repo_file(&args.what)? {
        // Following again keeps the files that already follow this page.
        let mut files = file_follows(&mut session, caller)?
            .into_iter()
            .find(|(page, _)| *page == lead_id)
            .map(|(_, files)| files)
            .unwrap_or_default();
        if !files.contains(&file) {
            files.push(file.clone());
        }
        let reply = set_files(&mut session, caller, &lead_id, &files)?;
        return say(
            format,
            quiet,
            &format!(
                "{file} now follows \"{lead_title}\". When the page changes, your AI tool \
                 suggests fixes in the repository's pull request."
            ),
            &reply,
        );
    }
    let page_id = resolve_page(&mut session, caller, &args.what)?;
    let page = read_page(&mut session, &page_id)?;
    let reply = session.call_text(
        "page_basis",
        json!({
            "action": "mark",
            "page_id": page_id,
            "basis_page_id": lead_id,
            "version": version_of(&lead)?,
        }),
    )?;
    say(
        format,
        quiet,
        &format!(
            "\"{}\" now follows \"{lead_title}\". When it changes, steep brings this page in \
             line; a locked page waits for your review.",
            title_of(&page)
        ),
        &reply,
    )
}

pub fn run_unfollow(
    args: UnfollowArgs,
    caller: &Caller,
    format: ResolvedFormat,
    quiet: bool,
) -> Result<()> {
    let mut session = Session::start(caller)?;
    let lead_id = match &args.page {
        Some(page) => Some(resolve_page(&mut session, caller, page)?),
        None => None,
    };
    if let Some(file) = repo_file(&args.what)? {
        let follows: Vec<(String, Vec<String>)> = file_follows(&mut session, caller)?
            .into_iter()
            .filter(|(page, files)| {
                files.contains(&file) && lead_id.as_ref().is_none_or(|lead| lead == page)
            })
            .collect();
        if follows.is_empty() {
            bail!("{file} does not follow a page.");
        }
        let mut reply = String::new();
        for (page, files) in follows {
            let rest: Vec<String> = files.into_iter().filter(|f| *f != file).collect();
            reply = set_files(&mut session, caller, &page, &rest)?;
        }
        return say(
            format,
            quiet,
            &format!("{file} no longer follows a page."),
            &reply,
        );
    }
    let page_id = resolve_page(&mut session, caller, &args.what)?;
    let answer = session.call_json("read_page", json!({ "page_id": page_id }))?;
    let bases: Vec<(String, String)> = answer
        .get("basis")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|basis| {
            Some((
                basis.get("page_id")?.as_str()?.to_string(),
                basis
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            ))
        })
        .collect();
    let (lead, lead_title) = match lead_id {
        Some(lead) => {
            let title = bases
                .iter()
                .find(|(id, _)| *id == lead)
                .map(|(_, title)| title.clone())
                .unwrap_or_default();
            (lead, title)
        }
        None => match bases.as_slice() {
            [] => bail!("\"{}\" does not follow a page.", args.what),
            [one] => one.clone(),
            many => {
                let names: Vec<&str> = many.iter().map(|(_, title)| title.as_str()).collect();
                bail!(
                    "\"{}\" follows {} pages ({}). Name the one to stop following: \
                     wenlan unfollow \"{}\" <page>",
                    args.what,
                    many.len(),
                    names.join(", "),
                    args.what
                )
            }
        },
    };
    let reply = session.call_text(
        "page_basis",
        json!({ "action": "remove", "page_id": page_id, "basis_page_id": lead }),
    )?;
    say(
        format,
        quiet,
        &format!("\"{}\" no longer follows \"{lead_title}\".", args.what),
        &reply,
    )
}

pub fn run_follows(
    args: FollowsArgs,
    caller: &Caller,
    format: ResolvedFormat,
    quiet: bool,
) -> Result<()> {
    let mut session = Session::start(caller)?;
    if let Some(page) = args.page {
        let page_id = resolve_page(&mut session, caller, &page)?;
        let answer = session.call_json("read_page", json!({ "page_id": page_id }))?;
        let bases = answer.get("basis").cloned().unwrap_or(json!([]));
        if quiet {
            return Ok(());
        }
        if format == ResolvedFormat::Json {
            println!("{bases}");
            return Ok(());
        }
        let rows = bases.as_array().cloned().unwrap_or_default();
        if rows.is_empty() {
            println!("\"{page}\" does not follow a page.");
        }
        for basis in rows {
            let title = basis.get("title").and_then(Value::as_str).unwrap_or("");
            let state = match basis.get("state").and_then(Value::as_str) {
                Some("ok") => "up to date",
                Some("pending") => "changed; the next steep updates this page",
                Some("archived") => "that page is archived",
                Some("deleted") => "that page was deleted",
                _ => "not readable here",
            };
            println!("Follows \"{title}\": {state}");
        }
        return Ok(());
    }
    let files = file_follows(&mut session, caller).unwrap_or_default();
    let waiting = session
        .call_json(
            "steep",
            with_space(json!({ "action": "list", "limit": 100 }), caller),
        )
        .ok()
        .and_then(|list| list.get("items").and_then(Value::as_array).cloned())
        .unwrap_or_default()
        .into_iter()
        .filter(|item| item.get("stale_reason").and_then(Value::as_str) == Some("basis_changed"))
        .collect::<Vec<_>>();
    if quiet {
        return Ok(());
    }
    if format == ResolvedFormat::Json {
        let files: Vec<Value> = files
            .iter()
            .map(|(page, files)| json!({ "page_id": page, "files": files }))
            .collect();
        println!("{}", json!({ "files": files, "waiting": waiting }));
        return Ok(());
    }
    if files.is_empty() && waiting.is_empty() {
        println!(
            "No file here follows a page, and steep has no page to bring in line. A page \
             that follows another shows with: wenlan follows <page>"
        );
        println!("Set one up with: wenlan follow <page or file> <page it follows>");
        return Ok(());
    }
    for (page, list) in &files {
        let title = read_page(&mut session, page)
            .map(|page| title_of(&page).to_string())
            .unwrap_or_else(|_| page.clone());
        let verb = if list.len() == 1 { "follows" } else { "follow" };
        println!("{} {verb} \"{title}\"", list.join(", "));
    }
    for item in &waiting {
        let title = item.get("title").and_then(Value::as_str).unwrap_or("");
        println!("\"{title}\" is waiting to catch up; the next steep updates it");
    }
    Ok(())
}
