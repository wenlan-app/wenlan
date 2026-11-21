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
    AdoptPreviewRequest, AdoptRequest, AdoptedSkillUpdate, AdoptionPlan, AdoptionSkillOutcome,
    PullbackAction, PullbackDecision, RulesSyncReport, SkillUpdateStatus, SkillUpstream,
};
use wenlan_types::rulesync_settings::{SettingsApplyRequest, SettingsPlan, SettingsPreviewRequest};

use super::skills::{one_line, person_says_yes};
use crate::{client::WenlanClient, output::ResolvedFormat};

#[derive(Debug, Subcommand)]
pub enum RulesSetupCmd {
    /// Bring your personal rules, skills, commands and subagents under Wenlan management.
    Adopt {
        /// Preview the changes, or list the installed files while offline.
        #[arg(long)]
        dry_run: bool,
    },
    /// Restore the original files and links from the local setup backup.
    Restore,
    /// Review local changes and deliver the current personal tool setup.
    Sync,
    /// Check adopted skills for upstream releases and list offers for review.
    SkillUpdates,
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
        /// Choose the source file when a proposal contains competing edits.
        #[arg(long, value_name = "PATH")]
        file: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ReviewAction {
    /// Keep the change for the tool that made it.
    ToolOnly,
    /// Share the change with all compatible tools.
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
    let session = wenlan_types::session_context::resolve_session_from_env(explicit_session);
    if matches!(command, RulesSetupCmd::Adopt { dry_run: true }) {
        if rulesync_personal::enabled() && !names_an_agent && session.session.is_none() {
            if let Ok(client) = WenlanClient::for_local_rules() {
                let request = AdoptPreviewRequest {
                    home: rulesync_personal::system_home()?.canonicalize()?,
                };
                match client.preview_adoption_as_person(&request).await {
                    Ok(plan) => return show(format, quiet, &plan, plan_for_a_person),
                    Err(error) => {
                        let mut plan = adoption_plan()?;
                        plan.notes
                            .push(format!("Rendered preview unavailable ({error:#})."));
                        return show(format, quiet, &plan, plan_for_a_person);
                    }
                }
            }
        }
        return show(format, quiet, &adoption_plan()?, plan_for_a_person);
    }
    ensure!(
        !names_an_agent && session.session.is_none(),
        "Only you can change your personal tool setup. Run this from your own terminal outside an AI session."
    );
    ensure!(
        matches!(
            command,
            RulesSetupCmd::Sync
                | RulesSetupCmd::SkillUpdates
                | RulesSetupCmd::Settings { dry_run: true, .. }
        ) || (io::stdin().is_terminal() && io::stdout().is_terminal()),
        "Run this command from your own terminal and type yes when asked."
    );

    let client = WenlanClient::for_local_rules()?;
    let report = match command {
        RulesSetupCmd::Adopt { .. } => {
            let expected = AdoptPreviewRequest {
                home: rulesync_personal::system_home()?.canonicalize()?,
            };
            let plan = client.preview_adoption_as_person(&expected).await?;
            // The person must see the same plan even with --quiet or --format json.
            println!("{}", plan_for_a_person(&plan));
            if plan.files.is_empty()
                || !confirm("Back up these files and links and let Wenlan manage them?")?
            {
                return Ok(());
            }
            client
                .adopt_rules_as_person(&AdoptRequest {
                    token: plan.token,
                    home: Some(expected.home),
                })
                .await?
        }
        RulesSetupCmd::Restore => {
            if !confirm("Replace managed files and links with the original copies from the local setup backup?")? {
                return Ok(());
            }
            client.restore_rules_as_person().await?
        }
        RulesSetupCmd::Sync => client.sync_rules_as_person().await?,
        RulesSetupCmd::SkillUpdates => {
            let updates = client.adopted_skill_updates_as_person().await?;
            return show(format, quiet, &updates, |updates| {
                updates_for_a_person(updates)
            });
        }
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
            if (plan.files.is_empty() && plan.notice.is_none())
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
            let decision = PullbackDecision {
                id: id.clone(),
                action: match action {
                    ReviewAction::ToolOnly => PullbackAction::ToolOnly,
                    ReviewAction::AllTools => PullbackAction::AllTools,
                    ReviewAction::Dismiss => PullbackAction::Dismiss,
                },
                file: file.clone(),
            };
            let preview = client.preview_rules_review_as_person(&decision).await?;
            let mut text = printable(&preview.plan);
            text.push('\n');
            append_diff(&mut text, &preview.diff);
            if !review_confirmation(&mut io::stdin().lock(), &mut io::stdout(), &decision, &text)? {
                println!("Cancelled.");
                return Ok(());
            }
            client.review_rules_as_person(&decision).await?
        }
    };
    show(format, quiet, &report, report_for_a_person)
}

fn review_confirmation(
    input: &mut dyn io::BufRead,
    output: &mut dyn io::Write,
    decision: &PullbackDecision,
    preview: &str,
) -> Result<bool> {
    writeln!(output, "{}", preview.trim_end())?;
    let mut proposal = one_line(&decision.id);
    if let Some(file) = &decision.file {
        proposal.push_str(&format!(" from {}", file_label(file)));
    }
    let question = match decision.action {
        PullbackAction::ToolOnly => format!("Keep proposal {proposal} for this tool only?"),
        PullbackAction::AllTools => format!("Use proposal {proposal} in all compatible tools?"),
        PullbackAction::Dismiss => format!("Dismiss proposal {proposal}?"),
    };
    person_says_yes(input, output, &question)
}

fn adoption_plan() -> Result<AdoptionPlan> {
    rulesync_personal::plan_adoption_offline(&rulesync_personal::system_home()?)
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
    let mut out = String::from("Personal tool setup adoption plan\n");
    for note in &plan.notes {
        writeln!(&mut out, "\n{}", one_line(note)).unwrap();
    }
    if !plan.skills.is_empty() {
        out.push_str("\nSkill folders\n");
        for skill in &plan.skills {
            let mut status = match &skill.outcome {
                AdoptionSkillOutcome::TrackedUpstream { upstream } => {
                    format!("tracked upstream from {}", upstream_label(upstream))
                }
                AdoptionSkillOutcome::PlainCopy => "plain copy in the personal master".into(),
                AdoptionSkillOutcome::Skipped { reason } => {
                    format!(
                        "skipped ({}) — stays where it is, not managed",
                        one_line(reason)
                    )
                }
            };
            if let Some(reason) = &skill.byte_copy_reason {
                write!(
                    &mut status,
                    ". Deliver byte-for-byte from the master ({})",
                    one_line(reason)
                )
                .unwrap();
            }
            writeln!(&mut out, "  {} — {status}", file_label(&skill.path)).unwrap();
        }
    }
    out.push_str(&render_plan(
        "File changes",
        "No existing files to adopt.",
        &plan.files,
    ));
    out
}

fn upstream_label(upstream: &SkillUpstream) -> String {
    let (source, directory) = match upstream {
        SkillUpstream::GitHub {
            repository,
            directory,
            ..
        } => (format!("GitHub {repository}"), directory),
        SkillUpstream::Npm {
            package,
            version,
            directory,
        } => (format!("npm {package}@{version}"), directory),
    };
    if directory.is_empty() {
        one_line(&source)
    } else {
        one_line(&format!("{source} ({directory})"))
    }
}

fn updates_for_a_person(updates: &[AdoptedSkillUpdate]) -> String {
    if updates.is_empty() {
        return "No adopted skills track an upstream.\n".into();
    }
    let mut out = String::new();
    for update in updates {
        let status = match &update.status {
            SkillUpdateStatus::Current => "current".into(),
            SkillUpdateStatus::Available { revision } => {
                format!("{} available to review", one_line(revision))
            }
            SkillUpdateStatus::Unavailable { reason } => {
                format!("check unavailable ({})", one_line(reason))
            }
        };
        writeln!(
            &mut out,
            "{} — {} — {status}",
            one_line(&update.name),
            upstream_label(&update.upstream)
        )
        .unwrap();
    }
    out
}

fn settings_plan_for_a_person(plan: &SettingsPlan) -> String {
    let mut output = render_plan(
        "Personal settings plan",
        "No settings changes.",
        &plan.files,
    );
    if let Some(notice) = &plan.notice {
        writeln!(output, "\n{}", printable(notice)).unwrap();
    }
    output
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
    fn rulesync_review_displays_the_selected_plan_before_asking_for_yes() {
        let all_tools = "Create directory .codex/prompts with permissions 0700\n\
            Create .codex/prompts/check.md with permissions 0600\n\
            Replace .cursor/commands/check.md with permissions 0644\n\
            Remove .config/opencode/commands/check.md with permissions 0640\n\
            Skip Gemini CLI. Gemini CLI uses its own native TOML command format.\n";
        let tool_only = "Keep the existing edit in Claude Code.\n\
            Skip Codex CLI. This review is for Claude Code only.\n";
        let source_only = "All-tools stays with its tool (Claude Code).\n\
            Skip Codex CLI. Left unmanaged: '$' can introduce argument placeholders. The command stays in its originating tool.\n";
        for (action, preview) in [
            (PullbackAction::AllTools, all_tools),
            (PullbackAction::ToolOnly, tool_only),
            (PullbackAction::AllTools, source_only),
        ] {
            let decision = PullbackDecision {
                id: "proposal".into(),
                action,
                file: Some(".claude/commands/check.md".into()),
            };
            let mut output = Vec::new();
            assert!(
                review_confirmation(&mut &b"yes\n"[..], &mut output, &decision, preview).unwrap()
            );
            let output = String::from_utf8(output).unwrap();
            let (before, _) = output.split_once("Type yes to go on:").unwrap();
            assert!(before.contains(preview), "{before}");
            assert!(before.contains(".claude/commands/check.md"), "{before}");
        }
    }

    #[test]
    fn rules_sync_table_includes_the_block_reason() {
        let report = RulesSyncReport {
            written: Vec::new(),
            blocked: vec![".codex/AGENTS.md: Review your changes before syncing.".into()],
            proposals: Vec::new(),
            message: "Checked the local tool setup".into(),
        };
        assert!(report_for_a_person(&report)
            .contains("Blocked .codex/AGENTS.md: Review your changes before syncing.\n"));
    }

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
            skills: Vec::new(),
            notes: Vec::new(),
        };
        let text = plan_for_a_person(&plan);
        assert!(text.contains("tool<U+000A>settings.md (+2 / -1)"));
        assert!(text.contains("-old\n+<U+001B>[2J\n+🐈<U+202E><U+007F>\n"));

        let json = terminal_json(&plan).unwrap();
        assert_eq!(json, printable(&json));
        assert_eq!(serde_json::from_str::<AdoptionPlan>(&json).unwrap(), plan);
    }
}
