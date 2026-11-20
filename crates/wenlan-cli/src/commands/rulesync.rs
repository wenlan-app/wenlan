// SPDX-License-Identifier: Apache-2.0
//! Preview and manage the personal rules and skills on this computer.

use std::fmt::Write;
use std::io::{self, IsTerminal};
use std::path::PathBuf;

use anyhow::{ensure, Result};
use clap::{Subcommand, ValueEnum};
use serde::Serialize;
use wenlan_core::{rulesync_personal, skill_registry::printable};
use wenlan_types::rulesync::{
    AdoptRequest, AdoptionPlan, PullbackAction, PullbackDecision, RulesSyncReport,
};
use wenlan_types::rulesync_settings::{SettingsApplyRequest, SettingsPlan, SettingsPreviewRequest};

use super::skills::{one_line, person_says_yes};
use crate::{client::WenlanClient, output::ResolvedFormat};

#[derive(Debug, Subcommand)]
pub enum RulesSetupCmd {
    /// Bring your existing personal rules and skills under Wenlan management.
    Adopt {
        /// Show the files and changes without contacting Wenlan or writing to disk.
        #[arg(long)]
        dry_run: bool,
    },
    /// Restore the original files and links from the adoption backup.
    Restore,
    /// Collect tool edits for review and deliver current personal rules and skills.
    Sync,
    /// Preview local MCP servers, hooks and permissions and confirm before applying.
    Settings {
        /// A local rulesync source folder.
        source: PathBuf,
        /// Show only the proposed settings changes.
        #[arg(long)]
        dry_run: bool,
    },
    /// Answer a proposal shown by `wenlan rules sync`.
    Review {
        /// The proposal ID shown by `wenlan rules sync`.
        id: String,
        #[arg(value_enum)]
        action: ReviewAction,
        /// Choose which file's change to keep when several tools changed the same line.
        #[arg(long, value_name = "PATH")]
        file: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ReviewAction {
    /// Keep the change for the tool that made it.
    ToolOnly,
    /// Use the change in all tools.
    AllTools,
    /// Discard the proposal.
    Dismiss,
}

pub async fn run(
    command: &RulesSetupCmd,
    format: ResolvedFormat,
    quiet: bool,
    names_an_agent: bool,
    explicit_session: Option<&str>,
) -> Result<()> {
    run_inner(command, format, quiet, names_an_agent, explicit_session)
        .await
        .map_err(terminal_error)
}

pub(super) fn terminal_error(error: anyhow::Error) -> anyhow::Error {
    anyhow::Error::msg(printable(&format!("{error:#}")))
}

async fn run_inner(
    command: &RulesSetupCmd,
    format: ResolvedFormat,
    quiet: bool,
    names_an_agent: bool,
    explicit_session: Option<&str>,
) -> Result<()> {
    if matches!(command, RulesSetupCmd::Adopt { dry_run: true }) {
        return show(format, quiet, &adoption_plan()?, plan_for_a_person);
    }
    let session = wenlan_types::session_context::resolve_session_from_env(explicit_session);
    ensure!(
        !names_an_agent && session.session.is_none(),
        "Only you can change your personal tool setup. Run this from your own terminal outside an AI session."
    );
    ensure!(
        matches!(
            command,
            RulesSetupCmd::Sync | RulesSetupCmd::Settings { dry_run: true, .. }
        ) || (io::stdin().is_terminal() && io::stdout().is_terminal()),
        "Run this command from your own terminal and type yes when asked."
    );

    let client = WenlanClient::for_local_rules()?;
    let report = match command {
        RulesSetupCmd::Adopt { .. } => {
            let plan = adoption_plan()?;
            // The person must see the same plan even with --quiet or --format json.
            println!("{}", plan_for_a_person(&plan));
            if !confirm("Back up these files and links and let Wenlan manage them?")? {
                return Ok(());
            }
            client
                .adopt_rules_as_person(&AdoptRequest { token: plan.token })
                .await?
        }
        RulesSetupCmd::Restore => {
            if !confirm("Replace managed files and links with the original copies from the adoption backup?")? {
                return Ok(());
            }
            client.restore_rules_as_person().await?
        }
        RulesSetupCmd::Sync => client.sync_rules_as_person().await?,
        RulesSetupCmd::Settings { source, dry_run } => {
            let source = std::path::absolute(source)?;
            let plan = client
                .preview_settings_as_person(&SettingsPreviewRequest {
                    source: source.clone(),
                })
                .await?;
            if *dry_run {
                return show(format, quiet, &plan, settings_plan_for_a_person);
            }
            println!("{}", settings_plan_for_a_person(&plan));
            if plan.files.is_empty()
                || !confirm("Apply these settings to your AI tools on this computer?")?
            {
                return Ok(());
            }
            client
                .apply_settings_as_person(&SettingsApplyRequest {
                    source,
                    token: plan.token,
                })
                .await?
        }
        RulesSetupCmd::Review { id, action, file } => {
            let mut proposal = one_line(id);
            if let Some(file) = file {
                proposal.push_str(&format!(" from {}", file_label(file)));
            }
            let (action, question) = match action {
                ReviewAction::ToolOnly => (
                    PullbackAction::ToolOnly,
                    format!("Keep proposal {proposal} for this tool only?"),
                ),
                ReviewAction::AllTools => (
                    PullbackAction::AllTools,
                    format!("Use proposal {proposal} in all tools?"),
                ),
                ReviewAction::Dismiss => (
                    PullbackAction::Dismiss,
                    format!("Dismiss proposal {proposal}?"),
                ),
            };
            if !confirm(&question)? {
                return Ok(());
            }
            client
                .review_rules_as_person(&PullbackDecision {
                    id: id.clone(),
                    action,
                    file: file.clone(),
                })
                .await?
        }
    };
    show(format, quiet, &report, report_for_a_person)
}

fn adoption_plan() -> Result<AdoptionPlan> {
    rulesync_personal::plan_adoption(&rulesync_personal::system_home()?)
}

fn confirm(question: &str) -> Result<bool> {
    let answered = person_says_yes(&mut io::stdin().lock(), &mut io::stdout(), question)?;
    if !answered {
        println!("Cancelled.");
    }
    Ok(answered)
}

fn show<T: Serialize>(
    format: ResolvedFormat,
    quiet: bool,
    value: &T,
    for_a_person: impl FnOnce(&T) -> String,
) -> Result<()> {
    if quiet {
        return Ok(());
    }
    match format {
        ResolvedFormat::Json => println!("{}", terminal_json(value)?),
        ResolvedFormat::Table => print!("{}", for_a_person(value)),
    }
    Ok(())
}

pub(super) fn terminal_json(value: &impl Serialize) -> Result<String> {
    let json = serde_json::to_string_pretty(value)?;
    if printable(&json) == json {
        return Ok(json);
    }
    // Escape Unicode when the serialized JSON contains terminal controls, so
    // parsing the CLI output still recovers the exact file names and diffs.
    let mut safe = String::with_capacity(json.len());
    for c in json.chars() {
        if c.is_ascii() && c != '\u{007f}' {
            safe.push(c);
        } else {
            for unit in c.encode_utf16(&mut [0; 2]) {
                write!(&mut safe, "\\u{unit:04X}")?;
            }
        }
    }
    Ok(safe)
}

fn plan_for_a_person(plan: &AdoptionPlan) -> String {
    render_plan(
        "Personal rules and skills adoption plan",
        "No existing files to adopt.",
        &plan.files,
    )
}

fn settings_plan_for_a_person(plan: &SettingsPlan) -> String {
    render_plan(
        "Personal settings plan",
        "No settings changes.",
        &plan.files,
    )
}

fn render_plan(
    title: &str,
    empty: &str,
    files: &[wenlan_types::rulesync::AdoptionChange],
) -> String {
    let mut out = format!("{title}\n");
    if files.is_empty() {
        out.push_str(empty);
        out.push('\n');
    }
    for file in files {
        out.push_str(&format!(
            "\n{} (+{} / -{})\n",
            file_label(&file.path),
            file.added,
            file.removed
        ));
        append_diff(&mut out, &file.diff);
    }
    out
}

fn report_for_a_person(report: &RulesSyncReport) -> String {
    let mut out = format!("{}\n", printable(&report.message));
    for path in &report.written {
        out.push_str(&format!("Wrote {}\n", file_label(path)));
    }
    for path in &report.blocked {
        out.push_str(&format!("Blocked {}\n", file_label(path)));
    }
    for card in &report.proposals {
        out.push_str(&format!(
            "\nProposal {}\n{}\n",
            one_line(&card.id),
            printable(&card.reason)
        ));
        for file in &card.files {
            out.push_str(&format!("  {}\n", file_label(file)));
        }
        append_diff(&mut out, &card.diff);
    }
    if !report.proposals.is_empty() {
        out.push_str("\nAnswer with `wenlan rules review <id> <tool-only|all-tools|dismiss>`.\n");
    }
    if report.proposals.iter().any(|card| card.files.len() > 1) {
        out.push_str("Use `--file <path>` to choose a file when tools changed the same rule.\n");
    }
    out
}

fn append_diff(out: &mut String, diff: &str) {
    if diff.is_empty() {
        return;
    }
    out.push_str(&printable(diff));
    if !diff.ends_with('\n') {
        out.push('\n');
    }
}

pub(super) fn file_label(path: &str) -> String {
    printable(path)
        .replace('\n', "<U+000A>")
        .replace('\t', "<U+0009>")
}

#[cfg(test)]
mod tests {
    use super::*;
    use wenlan_types::rulesync::AdoptionChange;

    #[test]
    fn personal_preview_neutralizes_terminal_controls() {
        let plan = AdoptionPlan {
            token: "test".into(),
            files: vec![AdoptionChange {
                path: "tool\nsettings.md".into(),
                added: 2,
                removed: 1,
                diff: "-old\n+\u{1b}[2J\n+🐈\u{202e}\u{7f}".into(),
            }],
        };
        let text = plan_for_a_person(&plan);
        assert!(text.contains("tool<U+000A>settings.md (+2 / -1)"));
        assert!(text.contains("-old\n+<U+001B>[2J\n+🐈<U+202E><U+007F>\n"));

        let json = terminal_json(&plan).unwrap();
        assert_eq!(json, printable(&json));
        assert_eq!(serde_json::from_str::<AdoptionPlan>(&json).unwrap(), plan);
    }
}
