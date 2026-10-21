// SPDX-License-Identifier: Apache-2.0
//! `wenlan sources add <path> [--type directory|okf]` — register a source and
//! sync it now. `wenlan sources save --url <link> --title <title>` saves one
//! document read elsewhere (an email, a Notion page) under its link.
//!
//! Thin HTTP client: POST /api/sources to register the path as a Directory or
//! OKF bundle source (idempotent — an already-registered path is treated as
//! success), then POST /api/sources/{id}/sync, then render the returned stats.
//! An OKF bundle lands in the Space sent by the client's Space header, else
//! the default Space. No DB access, no new endpoints (per AGENTS.md crate
//! boundaries).

use anyhow::{Context, Result};
use std::io::{IsTerminal, Read};
use std::path::PathBuf;

use crate::client::{SyncStats, WenlanClient};
use crate::output::{print_json, ResolvedFormat};

#[derive(clap::Subcommand)]
pub enum SourcesCommand {
    /// Add a folder or file source and sync it now.
    Add {
        /// Path to a directory or file to add as a source.
        path: PathBuf,
        /// Kind of source. `okf` imports an OKF bundle (for example an
        /// OpenWiki folder) into one Space; pass --space to choose it.
        #[arg(long = "type", value_enum, default_value_t = SourceKind::Directory)]
        source_type: SourceKind,
    },
    /// Save one document read elsewhere (an email, a Notion or Drive page, a
    /// Slack thread, an article) with its link, so notes cite the original.
    /// Text comes from --file or stdin. Saving the same link again replaces
    /// the saved copy; Wenlan does not re-fetch the original.
    Save {
        /// Permanent http(s) link to the original.
        #[arg(long)]
        url: String,
        /// Title of the original.
        #[arg(long)]
        title: String,
        /// Where it came from, e.g. gmail, notion, drive, slack, github, web.
        #[arg(long)]
        label: Option<String>,
        /// Read the text from this file instead of stdin.
        #[arg(long)]
        file: Option<PathBuf>,
        /// Refuse if this link is already saved instead of replacing it.
        #[arg(long)]
        create_only: bool,
    },
}

/// The source kinds `sources add` registers.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    /// A folder or single file of documents.
    Directory,
    /// An OKF knowledge bundle.
    Okf,
}

impl SourceKind {
    fn as_str(self) -> &'static str {
        match self {
            SourceKind::Directory => "directory",
            SourceKind::Okf => "okf",
        }
    }
}

pub async fn run_sources(
    client: &WenlanClient,
    format: ResolvedFormat,
    quiet: bool,
    command: SourcesCommand,
) -> Result<()> {
    match command {
        SourcesCommand::Add { path, source_type } => {
            run(client, format, quiet, path, source_type).await
        }
        SourcesCommand::Save {
            url,
            title,
            label,
            file,
            create_only,
        } => {
            let content = match file {
                Some(path) => std::fs::read_to_string(&path)
                    .with_context(|| format!("reading {}", path.display()))?,
                None => {
                    // An interactive TTY would block waiting for EOF.
                    if std::io::stdin().is_terminal() {
                        anyhow::bail!(
                            "No input. Provide --file <path> or pipe the text via stdin."
                        );
                    }
                    let mut text = String::new();
                    std::io::stdin()
                        .read_to_string(&mut text)
                        .context("reading stdin")?;
                    text
                }
            };
            let req = save_request(url, title, label, content, create_only)?;
            let resp = client.save_source(&req).await.map_err(|error| {
                if create_only && is_conflict(&error) {
                    anyhow::anyhow!(
                        "Already saved: {}. Nothing was changed; drop --create-only to replace it.",
                        req.url
                    )
                } else {
                    error
                }
            })?;
            if quiet {
                return Ok(());
            }
            match format {
                ResolvedFormat::Json => print_json(&resp)?,
                ResolvedFormat::Table => println!(
                    "Saved source {} (space: {})",
                    resp.document_id,
                    resp.space.as_deref().unwrap_or("uncategorized")
                ),
            }
            Ok(())
        }
    }
}

/// Validate `sources save` input. The link becomes the document's identity,
/// and only an http(s) link lets a note's citation open the original.
fn save_request(
    url: String,
    title: String,
    label: Option<String>,
    content: String,
    create_only: bool,
) -> Result<wenlan_types::requests::IngestWebpageRequest> {
    let url = url.trim().to_string();
    let scheme_ok = url
        .split_once("://")
        .map(|(scheme, rest)| {
            matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") && !rest.is_empty()
        })
        .unwrap_or(false);
    if !scheme_ok {
        anyhow::bail!("--url must be the original's http(s) link; got `{url}`");
    }
    let title = title.trim().to_string();
    if title.is_empty() {
        anyhow::bail!("--title is empty");
    }
    if content.trim().is_empty() {
        anyhow::bail!("Empty text. Provide --file or pipe the text via stdin.");
    }
    let metadata = label
        .map(|label| label.trim().to_ascii_lowercase())
        .filter(|label| !label.is_empty())
        .map(|label| std::collections::HashMap::from([("source_label".to_string(), label)]));
    // The Space travels in the client's Space header, like `capture`.
    Ok(wenlan_types::requests::IngestWebpageRequest {
        url,
        title,
        content,
        metadata,
        create_only,
        space: Default::default(),
    })
}

fn is_conflict(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<reqwest::Error>()
            .and_then(reqwest::Error::status)
            == Some(reqwest::StatusCode::CONFLICT)
    })
}

pub async fn run(
    client: &WenlanClient,
    format: ResolvedFormat,
    quiet: bool,
    path: PathBuf,
    source_type: SourceKind,
) -> Result<()> {
    // Resolve to an absolute, canonical path so the daemon stores a stable key
    // and an idempotent re-run matches the existing registration.
    let abs = std::fs::canonicalize(&path)
        .with_context(|| format!("resolving path: {}", path.display()))?;
    let abs_str = abs.to_string_lossy().to_string();

    let id = register_source(client, source_type, &abs_str).await?;
    let stats = client.sync_source(&id).await?;

    if quiet {
        return Ok(());
    }
    match format {
        ResolvedFormat::Json => print_json(&stats)?,
        ResolvedFormat::Table => print!("{}", format_stats(&id, &stats)),
    }
    Ok(())
}

/// Register `abs_path` as a source of `source_type`, idempotently, returning
/// its id.
///
/// The happy path is a single POST /api/sources. If the path is already
/// registered the POST fails (the daemon rejects a duplicate), so we recover
/// the existing id from the source list instead of erroring. A genuine failure
/// (path missing, reserved root, daemon down) surfaces the original POST error
/// because the list will not contain the path.
async fn register_source(
    client: &WenlanClient,
    source_type: SourceKind,
    abs_path: &str,
) -> Result<String> {
    match client.add_source(source_type.as_str(), abs_path).await {
        Ok(source) => Ok(source.id),
        Err(add_err) => {
            let existing = client.list_sources().await.ok().and_then(|sources| {
                sources
                    .into_iter()
                    .find(|s| s.path.to_string_lossy() == abs_path)
            });
            match existing {
                Some(s) => Ok(s.id),
                None => Err(add_err),
            }
        }
    }
}

fn format_stats(id: &str, stats: &SyncStats) -> String {
    let mut out = format!(
        "Synced {}: {} file(s) found, {} ingested, {} skipped, {} error(s)\n",
        id, stats.files_found, stats.ingested, stats.skipped, stats.errors,
    );
    if let Some(detail) = &stats.error_detail {
        out.push_str(&format!("  error detail: {}\n", detail));
    }
    if let Some(paused) = &stats.paused {
        out.push_str(&format!("  steep paused: {}\n", paused));
    }
    if let (Some(queued), Some(waiting)) = (stats.queued_files, stats.waiting_files) {
        out.push_str(&format!(
            "  {} file(s) queued for preparation, {} waiting for the next batch\n",
            queued, waiting
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(found: usize, ingested: usize, skipped: usize, errors: usize) -> SyncStats {
        SyncStats {
            files_found: found,
            ingested,
            skipped,
            errors,
            error_detail: None,
            paused: None,
            queued_files: None,
            waiting_files: None,
        }
    }

    #[test]
    fn save_request_keeps_link_label_and_text() {
        let req = save_request(
            " https://www.notion.so/team/Plan-abc ".into(),
            " Plan ".into(),
            Some(" Notion ".into()),
            "Body".into(),
            true,
        )
        .unwrap();
        assert_eq!(req.url, "https://www.notion.so/team/Plan-abc");
        assert_eq!(req.title, "Plan");
        assert_eq!(
            req.metadata
                .unwrap()
                .get("source_label")
                .map(String::as_str),
            Some("notion")
        );
        assert!(req.create_only);
        assert!(req.space.is_inherit());
    }

    #[test]
    fn save_request_refuses_non_http_links_and_empty_input() {
        for url in ["notion://abc", "example.com/a", "https://"] {
            assert!(save_request(url.into(), "T".into(), None, "Body".into(), false).is_err());
        }
        assert!(save_request(
            "https://a.b/c".into(),
            " ".into(),
            None,
            "Body".into(),
            false
        )
        .is_err());
        assert!(
            save_request("https://a.b/c".into(), "T".into(), None, "\n".into(), false).is_err()
        );
    }

    #[test]
    fn format_stats_renders_counts() {
        let out = format_stats("directory-notes", &stats(3, 2, 1, 0));
        assert!(out.contains("directory-notes"));
        assert!(out.contains("3 file(s) found"));
        assert!(out.contains("2 ingested"));
        assert!(out.contains("1 skipped"));
        assert!(out.contains("0 error(s)"));
    }

    #[test]
    fn format_stats_surfaces_error_detail_and_pause() {
        let s = SyncStats {
            error_detail: Some("file_read_errors".to_string()),
            paused: Some("llm backoff".to_string()),
            ..stats(4, 1, 0, 3)
        };
        let out = format_stats("directory-docs", &s);
        assert!(out.contains("file_read_errors"));
        assert!(out.contains("llm backoff"));
        assert!(!out.contains("queued"), "no batch line for a folder: {out}");
    }

    #[test]
    fn format_stats_renders_okf_batch_position() {
        let s = SyncStats {
            queued_files: Some(1000),
            waiting_files: Some(200),
            ..stats(1200, 1000, 0, 0)
        };
        let out = format_stats("okf-wiki", &s);
        assert!(out.contains("1000 file(s) queued for preparation, 200 waiting"));
    }
}
