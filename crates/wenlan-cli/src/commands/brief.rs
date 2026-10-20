// SPDX-License-Identifier: Apache-2.0
//! `wenlan brief [topic]`, typed handoff updates, and update history.

use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};
use std::path::{Path, PathBuf};
use wenlan_types::{
    Brief, BriefHistoryResponse, BriefItem, BriefReadRequest, BriefReadResponse, BriefReadState,
    BriefUpdateReceipt, BriefUpdateRequest, BriefVersionResponse, OutboxEnvelope, OutboxPayload,
    OUTBOX_SCHEMA,
};

use crate::client::WenlanClient;
use crate::outbox;
use crate::output::{print_json, ResolvedFormat};

#[derive(Debug, Args)]
pub struct BriefArgs {
    /// Optional topic for a separately-labelled same-Space recall.
    #[arg(value_name = "TOPIC")]
    pub topic: Option<String>,

    #[command(subcommand)]
    pub command: Option<BriefCommand>,
}

#[derive(Debug, Subcommand)]
pub enum BriefCommand {
    /// Apply a typed item-level handoff update from JSON.
    Update {
        /// JSON file containing a BriefUpdateRequest.
        #[arg(long, value_name = "JSON_FILE")]
        file: PathBuf,
    },
    /// List past Brief updates, newest first.
    History {
        /// Maximum number of updates to list.
        #[arg(short, long, default_value_t = 20)]
        limit: usize,
    },
    /// Show the Brief as it stood after one past update.
    Show {
        /// Brief version to show (see `wenlan brief history`).
        #[arg(long)]
        version: i64,
    },
}

impl BriefArgs {
    /// True for the subcommands that write, which resolve their Space from the
    /// update file rather than the read-side project mapping.
    pub fn is_update(&self) -> bool {
        matches!(self.command, Some(BriefCommand::Update { .. }))
    }
}

pub async fn run(
    client: &WenlanClient,
    format: ResolvedFormat,
    quiet: bool,
    args: BriefArgs,
    effective_space: Option<String>,
    agent_name: Option<&str>,
) -> Result<()> {
    match args.command {
        Some(BriefCommand::History { limit }) => {
            let response = client.brief_history(limit).await?;
            if !quiet {
                match format {
                    ResolvedFormat::Json => print_json(&response)?,
                    ResolvedFormat::Table => print!("{}", format_history(&response)),
                }
            }
        }
        Some(BriefCommand::Show { version }) => {
            let response = client.brief_version(version).await?;
            if !quiet {
                match format {
                    ResolvedFormat::Json => print_json(&response)?,
                    ResolvedFormat::Table => print!("{}", format_version(&response)),
                }
            }
        }
        Some(BriefCommand::Update { file }) => {
            let request = load_update(&file)?;
            if let Some(space) = effective_space.as_deref() {
                if request.space.trim() != space {
                    bail!(
                        "Brief update Space '{}' conflicts with resolved Space '{}'",
                        request.space,
                        space
                    );
                }
            }
            let receipt = match client.update_brief(&request).await {
                Ok(receipt) => receipt,
                Err(error) if client.is_local() && outbox::is_daemon_unreachable(&error) => {
                    let envelope = OutboxEnvelope {
                        schema: OUTBOX_SCHEMA,
                        created_at: outbox::now_rfc3339(),
                        caller_id: request.caller_id.clone(),
                        operation_id: request.operation_id.clone(),
                        space: Some(request.space.clone()),
                        agent_name: agent_name.map(str::to_owned),
                        reconcile_summary_version: true,
                        payload: OutboxPayload::BriefUpdate(request),
                    };
                    let path = outbox::enqueue(&envelope)?;
                    if !quiet {
                        print_queued(format, &path, &envelope.operation_id)?;
                    }
                    return Ok(());
                }
                Err(error) => return Err(error),
            };
            if !quiet {
                match format {
                    ResolvedFormat::Json => print_json(&receipt)?,
                    ResolvedFormat::Table => print!("{}", format_update_receipt(&receipt)),
                }
            }
        }
        None => {
            let response = client
                .brief(&BriefReadRequest {
                    topic: args.topic,
                    space: None,
                    legacy_context_limit: None,
                })
                .await?;
            if !quiet {
                match format {
                    ResolvedFormat::Json => print_json(&response)?,
                    ResolvedFormat::Table => print!("{}", format_brief(&response)),
                }
            }
        }
    }
    Ok(())
}

#[derive(serde::Serialize)]
struct QueuedOutput<'a> {
    status: &'static str,
    path: String,
    operation_id: &'a str,
}

fn print_queued(format: ResolvedFormat, path: &Path, operation_id: &str) -> Result<()> {
    match format {
        ResolvedFormat::Json => print_json(&QueuedOutput {
            status: "queued",
            path: path.display().to_string(),
            operation_id,
        }),
        ResolvedFormat::Table => {
            println!(
                "saved locally, syncs when Wenlan starts: {}",
                path.display()
            );
            Ok(())
        }
    }
}

pub fn load_update(path: &Path) -> Result<BriefUpdateRequest> {
    let body = std::fs::read_to_string(path)
        .with_context(|| format!("reading Brief update {}", path.display()))?;
    serde_json::from_str(&body)
        .with_context(|| format!("invalid Brief update JSON in {}", path.display()))
}

fn format_brief(response: &BriefReadResponse) -> String {
    match response.state {
        BriefReadState::SpaceNotResolved => {
            "No Space resolved. Use --space or configure a project mapping.\n".into()
        }
        BriefReadState::BriefNotCreated => format!(
            "No Brief exists for Space '{}'. Run `wenlan brief update --file <json>` \
             (or the /handoff plugin command) to create it.\n",
            response.space.as_deref().unwrap_or("unknown")
        ),
        BriefReadState::Ready => response
            .brief
            .as_ref()
            .map(|brief| format_ready_brief(brief, response))
            .unwrap_or_else(|| "Brief response was marked ready without Brief data.\n".into()),
    }
}

fn format_history(response: &BriefHistoryResponse) -> String {
    let space = response.space.as_deref().unwrap_or("(none)");
    if response.entries.is_empty() {
        return format!("No Brief updates recorded for Space '{space}'.\n");
    }
    let mut output = format!("Space: {space}\n\n");
    output.push_str(&format!(
        "{:<8} {:<17} {:<20} {:<7} {}\n",
        "VERSION", "TIME", "CALLER", "CHANGES", "SUMMARY"
    ));
    for entry in &response.entries {
        output.push_str(&format!(
            "{:<8} {:<17} {:<20} {:<7} {}\n",
            entry.version,
            local_time(entry.created_at),
            truncate(&entry.caller_id, 20),
            entry.mutation_count,
            truncate(entry.summary.lines().next().unwrap_or("").trim(), 60),
        ));
    }
    output
}

fn format_version(response: &BriefVersionResponse) -> String {
    let mut output = format!(
        "Version {} ({} by {})\n\n",
        response.entry.version,
        local_time(response.entry.created_at),
        response.entry.caller_id
    );
    output.push_str(&format_ready_brief_body(&response.brief, None));
    output
}

fn local_time(unix_seconds: i64) -> String {
    chrono::DateTime::from_timestamp(unix_seconds, 0)
        .map(|time| {
            time.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| unix_seconds.to_string())
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let kept: String = text.chars().take(max.saturating_sub(3)).collect();
    format!("{kept}...")
}

fn format_ready_brief(brief: &Brief, response: &BriefReadResponse) -> String {
    format_ready_brief_body(brief, response.related_context.as_ref())
}

fn format_ready_brief_body(
    brief: &Brief,
    related_context: Option<&wenlan_types::BriefRelatedContext>,
) -> String {
    let mut output = format!("Space: {}\n\nLast session\n", brief.space);
    if brief.last_session_summary.trim().is_empty() {
        output.push_str("  (none)\n");
    } else {
        for line in brief.last_session_summary.lines() {
            output.push_str("  ");
            output.push_str(line.trim());
            output.push('\n');
        }
    }
    format_items(&mut output, "Active", &brief.active);
    format_items(&mut output, "Backlog", &brief.backlog);

    if let Some(related) = related_context {
        output.push_str(&format!("\nRelated Context: {}\n", related.query));
        if related.results.is_empty() {
            output.push_str("  (no related memories)\n");
        } else {
            for result in related.results.iter().take(10) {
                let title = if result.title.is_empty() {
                    result.content.lines().next().unwrap_or("(no title)")
                } else {
                    &result.title
                };
                output.push_str(&format!("  [{:.3}] {}\n", result.score, title));
            }
        }
    }
    output
}

fn format_items(output: &mut String, heading: &str, items: &[BriefItem]) {
    output.push_str(&format!("\n{heading}\n"));
    if items.is_empty() {
        output.push_str("  (none)\n");
        return;
    }
    for item in items {
        output.push_str("  - ");
        output.push_str(&item.text);
        if let Some(gate) = item.gate.as_deref() {
            output.push_str(" [gate: ");
            output.push_str(gate);
            output.push(']');
        }
        output.push('\n');
    }
}

fn format_update_receipt(receipt: &BriefUpdateReceipt) -> String {
    let mut output = format!(
        "Updated Brief for '{}': {} applied, {} conflict(s).\n",
        receipt.space,
        receipt.applied.len(),
        receipt.conflicts.len()
    );
    if let Some(path) = &receipt.projection_path {
        output.push_str(&format!("Receipt: {path}\n"));
    }
    for warning in &receipt.warnings {
        output.push_str(&format!("Warning: {warning}\n"));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use wenlan_types::{BriefItemState, BriefRelatedContext, SearchResult};

    fn result() -> SearchResult {
        serde_json::from_value(serde_json::json!({
            "id": "memory-1",
            "content": "related memory",
            "source": "memory",
            "source_id": "memory-1",
            "title": "Related",
            "chunk_index": 0,
            "last_modified": 0,
            "score": 0.9
        }))
        .unwrap()
    }

    #[test]
    fn human_brief_omits_machine_ids_and_keeps_related_context_separate() {
        let response = BriefReadResponse {
            state: BriefReadState::Ready,
            space: Some("work".into()),
            brief: Some(Brief {
                space_id: "secret-space-id".into(),
                space: "work".into(),
                last_session_summary: "shipped storage".into(),
                last_handoff_at: Some(1),
                version: 3,
                active: vec![BriefItem {
                    id: "secret-item-id".into(),
                    text: "open PR".into(),
                    state: BriefItemState::Active,
                    added_at: 1,
                    gate: Some("CI green".into()),
                    version: 2,
                }],
                backlog: Vec::new(),
            }),
            related_context: Some(BriefRelatedContext {
                query: "release".into(),
                results: vec![result()],
            }),
            code_basis_pending: None,
        };

        let output = format_brief(&response);

        assert!(output.contains("open PR"));
        assert!(output.contains("Related Context: release"));
        assert!(!output.contains("secret-space-id"));
        assert!(!output.contains("secret-item-id"));
    }

    fn entry(version: i64, summary: &str) -> wenlan_types::BriefHistoryEntry {
        wenlan_types::BriefHistoryEntry {
            version,
            created_at: 1_700_000_000,
            caller_id: "handoff".into(),
            operation_id: format!("op-{version}"),
            summary: summary.into(),
            mutation_count: 2,
            mutations: Vec::new(),
        }
    }

    #[test]
    fn history_table_lists_version_caller_changes_and_short_summary() {
        let response = BriefHistoryResponse {
            space: Some("work".into()),
            entries: vec![
                entry(2, &format!("{}\nsecond line", "long summary ".repeat(20))),
                entry(1, "first"),
            ],
        };

        let output = format_history(&response);

        assert!(output.contains("VERSION"));
        assert!(output.contains("handoff"));
        assert!(output.contains("first"));
        assert!(output.contains("..."));
        assert!(!output.contains("second line"));
        assert!(!output.contains("op-2"));
        let first = output.lines().position(|l| l.starts_with("2 ")).unwrap();
        let second = output.lines().position(|l| l.starts_with("1 ")).unwrap();
        assert!(first < second, "newest first");
        assert!(format_history(&BriefHistoryResponse {
            space: Some("work".into()),
            entries: Vec::new()
        })
        .contains("No Brief updates recorded"));
    }

    #[test]
    fn show_renders_the_past_brief_in_the_same_layout_as_brief() {
        let brief = Brief {
            space_id: "secret-space-id".into(),
            space: "work".into(),
            last_session_summary: "old summary".into(),
            last_handoff_at: Some(1),
            version: 1,
            active: vec![BriefItem {
                id: "secret-item-id".into(),
                text: "old item".into(),
                state: wenlan_types::BriefItemState::Active,
                added_at: 1,
                gate: None,
                version: 1,
            }],
            backlog: Vec::new(),
        };
        let output = format_version(&BriefVersionResponse {
            entry: entry(1, "old summary"),
            brief: brief.clone(),
        });
        let current = format_brief(&BriefReadResponse {
            state: BriefReadState::Ready,
            space: Some("work".into()),
            brief: Some(brief),
            related_context: None,
            code_basis_pending: None,
        });

        assert!(output.starts_with("Version 1 ("));
        assert!(output.ends_with(&current));
        assert!(!output.contains("secret-"));
    }

    #[test]
    fn no_brief_hint_names_a_command_the_cli_actually_has() {
        let response = BriefReadResponse {
            state: BriefReadState::BriefNotCreated,
            space: Some("work".into()),
            brief: None,
            related_context: None,
            code_basis_pending: None,
        };

        let output = format_brief(&response);

        // The CLI has no `handoff` subcommand; the hint must point at
        // something a CLI user can actually run.
        assert!(!output.contains("The first handoff"));
        assert!(output.contains("wenlan brief update"));
    }
}
