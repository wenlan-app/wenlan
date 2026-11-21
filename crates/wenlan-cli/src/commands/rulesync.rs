// SPDX-License-Identifier: Apache-2.0
//! Preview and manage the personal rules and skills on this computer.

use std::fmt::Write;
use std::io::{self, IsTerminal};
use std::path::PathBuf;

use anyhow::{bail, ensure, Context, Result};
use clap::{Subcommand, ValueEnum};
use serde::Serialize;
use wenlan_core::{rulesync_personal, skill_registry::printable};
use wenlan_types::rulesync::{
    AdoptPreviewRequest, AdoptRequest, AdoptionPlan, AdoptionSkillOutcome, PullbackAction,
    PullbackDecision, RulesReviewContent, RulesReviewItem, RulesReviewList, RulesSetupStatus,
    RulesSyncReport, RulesToolState, SkillUpdateStatus, SkillUpstream, TO_REVIEW_COMMAND,
};
use wenlan_types::rulesync_settings::{SettingsApplyRequest, SettingsPlan, SettingsPreviewRequest};

use super::skills::{
    one_line, person_says_yes, review_saved_skill, skill_proposal_summary, SkillReviewChoice,
};
use crate::{client::WenlanClient, output::ResolvedFormat};
use wenlan_types::rulesync_skill::{SavedSkillScope, SkillProposalOrigin};

#[derive(Debug, Subcommand)]
pub enum RulesSetupCmd {
    /// Bring in your personal AI-tool setup and preview the changes.
    Adopt {
        /// Preview the changes, or list the installed files while offline.
        #[arg(long)]
        dry_run: bool,
    },
    /// Status of the personal setup in each tool.
    #[command(hide = !rulesync_personal::enabled())]
    Status {
        /// Check for edits and sync approved content now.
        #[arg(long)]
        sync: bool,
    },
    /// To review: list changes, or answer one item.
    Review {
        /// The ID shown by `wenlan rules review`.
        #[arg(requires = "answer")]
        id: Option<String>,
        #[arg(value_enum, requires = "id")]
        answer: Option<ReviewAction>,
        /// Choose the source file when a proposal contains competing edits.
        #[arg(long, value_name = "PATH", requires = "id")]
        file: Option<String>,
    },
    /// Restore the original files and links from the local setup backup.
    Restore,
    #[command(hide = true)]
    Sync,
    #[command(hide = true)]
    SkillUpdates,
    #[command(hide = true)]
    Settings {
        /// A local rulesync source folder.
        source: PathBuf,
        /// Show only the proposed settings changes.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ReviewAction {
    /// Keep the change in that tool only.
    ToolOnly,
    /// Use the change in all tools.
    AllTools,
    /// Clear the review without changing a tool.
    Dismiss,
    /// Put the tool back to the earlier version.
    Revert,
}

impl From<ReviewAction> for PullbackAction {
    fn from(action: ReviewAction) -> Self {
        match action {
            ReviewAction::ToolOnly => Self::ToolOnly,
            ReviewAction::AllTools => Self::AllTools,
            ReviewAction::Dismiss => Self::Dismiss,
            ReviewAction::Revert => Self::Revert,
        }
    }
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
                | RulesSetupCmd::Status { .. }
                | RulesSetupCmd::Review { id: None, .. }
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
            let question = if plan.settings.is_some() {
                "Back up this setup, let Wenlan manage it and apply the shown settings?"
            } else {
                "Back up these files and links and let Wenlan manage them?"
            };
            if (plan.files.is_empty() && plan.settings.is_none()) || !confirm(question)? {
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
        RulesSetupCmd::Status { sync } => {
            if *sync {
                client.sync_rules_as_person().await?;
            }
            let status = client.rules_setup_status().await?;
            return show(format, quiet, &status, status_for_a_person);
        }
        RulesSetupCmd::Sync => client.sync_rules_as_person().await?,
        RulesSetupCmd::SkillUpdates => {
            let mut review = client.rules_setup_review().await?;
            review
                .items
                .retain(|item| matches!(item.content, RulesReviewContent::SkillUpdate { .. }));
            return show(format, quiet, &review, review_for_a_person);
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
                    source: Some(source),
                    token: plan.token,
                })
                .await?
        }
        RulesSetupCmd::Review { id, answer, file } => {
            let review = client.rules_setup_review().await?;
            let (Some(id), Some(answer)) = (id, answer) else {
                return show(format, quiet, &review, review_for_a_person);
            };
            let item = review
                .items
                .into_iter()
                .find(|item| &item.id == id)
                .context("This item is no longer waiting for review. Run wenlan rules review.")?;
            return answer_item(&client, format, quiet, item, (*answer).into(), file.clone()).await;
        }
    };
    show(format, quiet, &report, report_for_a_person)
}

async fn answer_item(
    client: &WenlanClient,
    format: ResolvedFormat,
    quiet: bool,
    item: RulesReviewItem,
    action: PullbackAction,
    file: Option<String>,
) -> Result<()> {
    if !item.answers.contains(&action) {
        if action == PullbackAction::Dismiss && item.answers.contains(&PullbackAction::Revert) {
            bail!("This review puts the tool back to the earlier version. Use `revert`.");
        }
        if action == PullbackAction::Revert && item.answers.contains(&PullbackAction::Dismiss) {
            bail!("This review has nothing to put back. Use `dismiss`.");
        }
        bail!(
            "Available answers: {}",
            item.answers
                .iter()
                .map(|&answer| answer_word(answer))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    ensure!(
        file.is_none() || matches!(item.content, RulesReviewContent::ToolEdit { .. }),
        "--file only selects a tool edit."
    );
    let report = match item.content {
        RulesReviewContent::ToolEdit { .. } => {
            let decision = PullbackDecision {
                id: item.id,
                action,
                file,
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
        RulesReviewContent::SkillProposal { proposal } => {
            let (choice, scope) = match action {
                PullbackAction::AllTools => {
                    (SkillReviewChoice::Accept, Some(SavedSkillScope::Shared))
                }
                PullbackAction::ToolOnly => {
                    let scope = match proposal.origin {
                        SkillProposalOrigin::Agent { .. } => proposal.scope,
                        SkillProposalOrigin::Tool { tool }
                        | SkillProposalOrigin::Delivery { tool } => SavedSkillScope::Tool { tool },
                    };
                    (SkillReviewChoice::Accept, Some(scope))
                }
                PullbackAction::Dismiss => (SkillReviewChoice::Dismiss, None),
                PullbackAction::Revert => unreachable!("checked answers"),
            };
            return review_saved_skill(client, format, quiet, &proposal.id, choice, scope).await;
        }
        RulesReviewContent::SkillUpdate { update } => {
            if confirm(&format!(
                "Dismiss the update for {}?",
                one_line(&update.name)
            ))? {
                client.dismiss_rules_skill_update(&item.id).await?;
                return show(format, quiet, &item.id, |_| {
                    "Dismissed the skill update.\n".into()
                });
            }
            return Ok(());
        }
        RulesReviewContent::Settings { plan } => {
            println!("{}", settings_plan_for_a_person(&plan));
            if !confirm("Apply these settings to your AI tools on this computer?")? {
                return Ok(());
            }
            client
                .apply_settings_as_person(&SettingsApplyRequest {
                    source: None,
                    token: plan.token,
                })
                .await?
        }
    };
    show(format, quiet, &report, report_for_a_person)
}

fn answer_word(answer: PullbackAction) -> &'static str {
    match answer {
        PullbackAction::AllTools => "all-tools",
        PullbackAction::ToolOnly => "tool-only",
        PullbackAction::Dismiss => "dismiss",
        PullbackAction::Revert => "revert",
    }
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
        PullbackAction::Revert => {
            format!("Put the tool back to the earlier version for proposal {proposal}?")
        }
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
    let mut out = String::from("Bring in\n");
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
    if let Some(settings) = &plan.settings {
        out.push('\n');
        out.push_str(&settings_plan_for_a_person(settings));
    }
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

fn review_for_a_person(review: &RulesReviewList) -> String {
    if review.items.is_empty() {
        return "Nothing to review.\n".into();
    }
    let mut out = String::from("To review\n");
    for item in &review.items {
        match &item.content {
            RulesReviewContent::ToolEdit { card } => {
                writeln!(out, "\n{}  {}", one_line(&item.id), one_line(&card.reason)).unwrap();
                append_diff(&mut out, &card.diff);
            }
            RulesReviewContent::SkillProposal { proposal } => {
                out.push_str(&format!("\n{}", skill_proposal_summary(proposal)))
            }
            RulesReviewContent::SkillUpdate { update } => {
                if let SkillUpdateStatus::Available { revision } = &update.status {
                    writeln!(
                        out,
                        "\n{}  New version of {} ({})",
                        one_line(&item.id),
                        one_line(&update.name),
                        one_line(revision)
                    )
                    .unwrap();
                }
            }
            RulesReviewContent::Settings { plan } => {
                writeln!(
                    out,
                    "\n{}  MCP servers, hooks and permissions",
                    one_line(&item.id)
                )
                .unwrap();
                if let Some(notice) = &plan.notice {
                    writeln!(out, "  {}", one_line(notice)).unwrap();
                }
            }
        }
        writeln!(
            out,
            "  {}",
            item.answers
                .iter()
                .map(|&answer| answer_word(answer))
                .collect::<Vec<_>>()
                .join(" | ")
        )
        .unwrap();
    }
    writeln!(out, "\nAnswer with `{TO_REVIEW_COMMAND} <id> <answer>`.").unwrap();
    out
}

fn status_for_a_person(status: &RulesSetupStatus) -> String {
    if !status.enabled {
        return "WENLAN_RULESYNC is off.\n".into();
    }
    let mut out = String::from("Status\n");
    if let Some(at) = status
        .last_checked_at
        .and_then(|at| chrono::DateTime::from_timestamp(at, 0))
    {
        writeln!(
            out,
            "Last background check {}",
            at.format("%Y-%m-%d %H:%M:%S UTC")
        )
        .unwrap();
    } else {
        out.push_str("No background check yet.\n");
    }
    for row in &status.tools {
        let state = match row.state {
            RulesToolState::UpToDate => "up to date",
            RulesToolState::NeedsReview => "needs review",
            RulesToolState::NotManaged => "not managed",
        };
        let counts = &row.counts;
        writeln!(
            out,
            "{} — {state} — {} rules, {} skills, {} commands, {} subagents, {} MCP",
            one_line(&row.tool.name),
            counts.rules,
            counts.skills,
            counts.commands,
            counts.subagents,
            counts.mcp
        )
        .unwrap();
    }
    out
}

fn settings_plan_for_a_person(plan: &SettingsPlan) -> String {
    let mut output = render_plan(
        "MCP servers, hooks and permissions",
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
        writeln!(out, "\nSee answers with `{TO_REVIEW_COMMAND}`.").unwrap();
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
            blocked_codes: Vec::new(),
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
            settings: None,
            files: vec![AdoptionChange {
                path: "tool\nsettings.md".into(),
                added: 2,
                removed: 1,
                diff: "-old\n+\u{1b}[2J\n+🐈\u{202e}\u{7f}".into(),
                tool: None,
                notice_codes: Vec::new(),
                text_diff: None,
            }],
            skills: Vec::new(),
            notes: Vec::new(),
            tools: Vec::new(),
            skipped_files: Vec::new(),
            skipped_tools: Vec::new(),
            skipped_tool_codes: Vec::new(),
            note_codes: Vec::new(),
        };
        let text = plan_for_a_person(&plan);
        assert!(text.contains("tool<U+000A>settings.md (+2 / -1)"));
        assert!(text.contains("-old\n+<U+001B>[2J\n+🐈<U+202E><U+007F>\n"));

        let json = terminal_json(&plan).unwrap();
        assert_eq!(json, printable(&json));
        assert_eq!(serde_json::from_str::<AdoptionPlan>(&json).unwrap(), plan);
    }
}
