// SPDX-License-Identifier: Apache-2.0
//! Preview and manage the personal tool setup on this computer.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;
use std::io::{self, IsTerminal};
use std::path::PathBuf;

use anyhow::{bail, ensure, Context, Result};
use clap::{Subcommand, ValueEnum};
use serde::Serialize;
use wenlan_core::{rulesync_personal, skill_registry::printable};
use wenlan_types::rulesync::{
    AdoptPreviewRequest, AdoptRequest, AdoptionPlan, AdoptionSkillOutcome, PullbackAction,
    PullbackDecision, PullbackKind, RulesReviewContent, RulesReviewItem, RulesReviewList,
    RulesSetupCode, RulesSetupStatus, RulesSyncReport, RulesToolState, SharePreviewRequest,
    ShareRequest, SharingPlan, SkillUpdateStatus, SkillUpstream, TO_REVIEW_COMMAND,
};
use wenlan_types::rulesync_settings::{SettingsApplyRequest, SettingsPlan, SettingsPreviewRequest};

use super::skills::{
    one_line, person_says_yes, review_saved_skill, skill_proposal_summary, SkillReviewChoice,
};
use crate::{client::WenlanClient, output::ResolvedFormat};
use wenlan_types::rulesync_skill::{SavedSkillScope, SkillProposalOrigin};

/// Read and propose edits to the one personal rules store. Each `--edit`
/// explicitly names its layer, so a proposal can span several layers without
/// the CLI guessing where any text belongs.
#[derive(Debug, Subcommand)]
pub enum PersonalRulesCmd {
    /// Preview moving page rules into shared rules and confirm at a terminal.
    MergePages,
    /// The shared personal rules that apply across tools.
    Shared {
        #[command(subcommand)]
        command: PersonalRulesAction,
    },
    /// Rules that belong to one named tool.
    Tool {
        /// Tool name, for example `claude` or `codex`.
        name: String,
        #[command(subcommand)]
        command: PersonalRulesAction,
    },
    /// Browse and restore confirmed personal rule versions.
    History {
        #[command(subcommand)]
        command: PersonalRulesHistoryAction,
    },
}

#[derive(Debug, Subcommand)]
pub enum PersonalRulesAction {
    /// Show the complete current text for this layer.
    Show,
    /// Propose one atomic plan. Repeat `--edit LAYER=FILE` for every layer;
    /// LAYER is `shared` or `tool:NAME`. Every layer must be named explicitly.
    Propose {
        /// Explicit layer and UTF-8 text file, e.g. `shared=rules.md` or
        /// `tool:claude=claude.md`. Repeat to make one multi-layer plan.
        #[arg(long = "edit", required = true, value_name = "LAYER=FILE")]
        edits: Vec<String>,
    },
    /// Review a fresh copy of the full plan and apply it after typing `yes`.
    Apply {
        /// Plan ID printed by a propose command.
        plan_id: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum PersonalRulesHistoryAction {
    /// List confirmed personal rule versions.
    List,
    /// Show all rule text in a confirmed version.
    Show {
        /// Version ID from `wenlan rules history list`.
        version_id: String,
    },
    /// Propose restoring a version, then review and confirm the new plan.
    Restore {
        /// Version ID from `wenlan rules history list`.
        version_id: String,
    },
}

pub async fn run_personal_rules(
    command: &PersonalRulesCmd,
    agent_name: Option<&str>,
    explicit_session: Option<&str>,
) -> Result<()> {
    let session = wenlan_types::session_context::resolve_session_from_env(explicit_session);
    let has_ai_identity = agent_name.is_some() || session.session.is_some();
    let client = WenlanClient::for_local_rules_with_identity(agent_name, session.session.as_ref())?;
    match command {
        PersonalRulesCmd::MergePages => {
            let Some(plan) = client.preview_merge_page_rules().await? else {
                println!("There are no page rules waiting to move.");
                return Ok(());
            };
            print!("{}", personal_rule_plan(&plan));
            require_personal_rule_confirmation(
                has_ai_identity,
                io::stdin().is_terminal(),
                io::stdout().is_terminal(),
            )?;
            confirm_and_apply_personal_plan(&client, &plan).await
        }
        PersonalRulesCmd::Shared { command } => {
            run_personal_layer(
                &client,
                command,
                wenlan_types::personal_rules::PersonalRuleLayer::Shared,
                has_ai_identity,
            )
            .await
        }
        PersonalRulesCmd::Tool { name, command } => {
            run_personal_layer(
                &client,
                command,
                wenlan_types::personal_rules::PersonalRuleLayer::Tool { tool: name.clone() },
                has_ai_identity,
            )
            .await
        }
        PersonalRulesCmd::History { command } => {
            run_personal_history(&client, command, has_ai_identity).await
        }
    }
}

async fn run_personal_layer(
    client: &WenlanClient,
    command: &PersonalRulesAction,
    layer: wenlan_types::personal_rules::PersonalRuleLayer,
    has_ai_identity: bool,
) -> Result<()> {
    use wenlan_types::personal_rules::PersonalRuleShowRequest;
    match command {
        PersonalRulesAction::Show => {
            let shown = client
                .show_personal_rule(&PersonalRuleShowRequest { layer })
                .await?;
            println!("{}", personal_rule_show(&shown));
            Ok(())
        }
        PersonalRulesAction::Propose { edits } => {
            let edits = read_personal_rule_edits(edits)?;
            let plan = client
                .propose_personal_rules(&wenlan_types::personal_rules::PersonalRuleProposeRequest {
                    edits,
                })
                .await?;
            println!("{}", personal_rule_plan(&plan));
            Ok(())
        }
        PersonalRulesAction::Apply { plan_id } => {
            require_personal_rule_confirmation(
                has_ai_identity,
                io::stdin().is_terminal(),
                io::stdout().is_terminal(),
            )?;
            let plan = client.personal_rule_plan(plan_id).await?;
            print!("{}", personal_rule_plan(&plan));
            confirm_and_apply_personal_plan(client, &plan).await
        }
    }
}

async fn run_personal_history(
    client: &WenlanClient,
    command: &PersonalRulesHistoryAction,
    has_ai_identity: bool,
) -> Result<()> {
    match command {
        PersonalRulesHistoryAction::List => {
            let history = client.personal_rule_history().await?;
            if history.is_empty() {
                println!("No confirmed personal rule versions yet.");
            } else {
                for item in history {
                    println!(
                        "{}  {}  {}  layers: {}",
                        item.id,
                        item.time
                            .map_or_else(|| "time unknown".into(), |t| format!("{t}")),
                        item.cause,
                        item.layers
                            .iter()
                            .map(personal_layer_label)
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
            }
            Ok(())
        }
        PersonalRulesHistoryAction::Show { version_id } => {
            let version = client.personal_rule_version(version_id).await?;
            println!("{}", personal_rule_version(&version));
            Ok(())
        }
        PersonalRulesHistoryAction::Restore { version_id } => {
            require_personal_rule_confirmation(
                has_ai_identity,
                io::stdin().is_terminal(),
                io::stdout().is_terminal(),
            )?;
            let proposed = client.propose_personal_rule_restore(version_id).await?;
            let plan = client.personal_rule_plan(&proposed.id).await?;
            print!("{}", personal_rule_plan(&plan));
            confirm_and_apply_personal_plan(client, &plan).await
        }
    }
}

async fn confirm_and_apply_personal_plan(
    client: &WenlanClient,
    plan: &wenlan_types::personal_rules::PersonalRulePlan,
) -> Result<()> {
    let request = wenlan_types::personal_rules::PersonalRuleApplyRequest {
        plan_id: plan.id.clone(),
    };
    let accepted = if plan.page_merge.is_some() {
        let answer =
            person_rule_answer(&mut io::stdin().lock(), &mut io::stdout(), &plan.id, true)?;
        if answer == Some(false) {
            client.decline_merge_page_rules(&request).await?;
            println!("Kept the current page rules delivery.");
            return Ok(());
        }
        answer == Some(true)
    } else {
        person_types_yes(&mut io::stdin().lock(), &mut io::stdout(), &plan.id)?
    };
    if !accepted {
        println!("Nothing was changed.");
        return Ok(());
    }
    let version = client.apply_personal_rule_plan(&request).await?;
    println!("Applied as personal rule version {}.", version.summary.id);
    if !plan.unmanaged_tools.is_empty() {
        println!(
            "Not delivered to unmanaged tools: {}.",
            plan.unmanaged_tools.join(", ")
        );
    }
    Ok(())
}

fn require_personal_rule_confirmation(
    has_ai_identity: bool,
    stdin_is_terminal: bool,
    stdout_is_terminal: bool,
) -> Result<()> {
    ensure!(
        !has_ai_identity,
        "Only you can confirm personal rule changes. Run this from your own terminal outside an AI session."
    );
    ensure!(
        stdin_is_terminal && stdout_is_terminal,
        "Only you can confirm personal rule changes: run this in a terminal and type yes yourself. There is no flag or piped approval."
    );
    Ok(())
}

fn after_shared_edit_gate<T>(
    has_ai_identity: bool,
    stdin_is_terminal: bool,
    stdout_is_terminal: bool,
    request: impl FnOnce() -> Result<T>,
) -> Result<T> {
    require_shared_edit_confirmation(has_ai_identity, stdin_is_terminal, stdout_is_terminal)?;
    request()
}

fn require_shared_edit_confirmation(
    has_ai_identity: bool,
    stdin_is_terminal: bool,
    stdout_is_terminal: bool,
) -> Result<()> {
    ensure!(
        !has_ai_identity,
        "Only you can confirm team or project rule edits. Run this from your own terminal outside an AI session."
    );
    ensure!(
        stdin_is_terminal && stdout_is_terminal,
        "Only you can confirm team or project rule edits: run this in a terminal and type yes yourself. There is no flag or piped approval."
    );
    Ok(())
}

fn person_types_yes(
    input: &mut dyn io::BufRead,
    output: &mut dyn io::Write,
    plan_id: &str,
) -> Result<bool> {
    Ok(person_rule_answer(input, output, plan_id, false)? == Some(true))
}

fn person_rule_answer(
    input: &mut dyn io::BufRead,
    output: &mut dyn io::Write,
    plan_id: &str,
    can_decline: bool,
) -> Result<Option<bool>> {
    if can_decline {
        write!(
            output,
            "Apply plan {plan_id}? Type yes to move the rules or no to keep the current delivery: "
        )?;
    } else {
        write!(output, "Apply plan {plan_id}? Type yes to go on: ")?;
    }
    output.flush()?;
    let mut answer = String::new();
    input.read_line(&mut answer)?;
    Ok(match answer.trim().to_ascii_lowercase().as_str() {
        "yes" => Some(true),
        "no" if can_decline => Some(false),
        _ => None,
    })
}

fn shared_edit_answer(
    input: &mut dyn io::BufRead,
    output: &mut dyn io::Write,
    layer_name: &str,
) -> Result<Option<bool>> {
    write!(output, "Send this edit to {layer_name} for review? Type yes to send, no to discard it (the next sync puts the approved lines back), or press Enter to decide later: ")?;
    output.flush()?;
    let mut answer = String::new();
    input.read_line(&mut answer)?;
    Ok(match answer.trim().to_ascii_lowercase().as_str() {
        "yes" => Some(true),
        "no" => Some(false),
        _ => None,
    })
}

async fn run_shared_edits(client: &WenlanClient) -> Result<()> {
    let waiting = client.shared_rule_edits().await?;
    if waiting.is_empty() {
        println!("No team or project rule edits are waiting.");
        return Ok(());
    }
    for edit in waiting {
        println!("\n{}", edit.layer_name);
        for file in &edit.files {
            println!("  {file}");
        }
        println!("\n{}", edit.markdown);
        let Some(send) =
            shared_edit_answer(&mut io::stdin().lock(), &mut io::stdout(), &edit.layer_name)?
        else {
            continue;
        };
        let decision = client
            .decide_shared_rule_edit(&wenlan_types::rulesync::SharedEditDecisionRequest {
                id: edit.id,
                send,
            })
            .await;
        if shared_edit_decision_or_continue(decision)?.is_none() {
            continue;
        }
        println!(
            "{}",
            if send {
                "Sent for review."
            } else {
                "Declined. The next sync you start puts the approved lines back."
            }
        );
    }
    Ok(())
}

const SHARED_EDIT_REBASE_MESSAGE: &str = "The team's rules changed since this edit. Run `wenlan rules sync`, then confirm it again with `wenlan rules shared-edits`.";

fn shared_edit_decision_or_continue<T>(decision: Result<T>) -> Result<Option<T>> {
    match decision {
        Ok(value) => Ok(Some(value)),
        Err(error)
            if error
                .to_string()
                .contains("The team's rules changed since this edit.") =>
        {
            println!("{SHARED_EDIT_REBASE_MESSAGE}");
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

fn read_personal_rule_edits(
    specs: &[String],
) -> Result<Vec<wenlan_types::personal_rules::PersonalRuleEdit>> {
    use wenlan_types::personal_rules::{PersonalRuleEdit, PersonalRuleLayer};
    ensure!(
        !specs.is_empty(),
        "name each edited layer with --edit LAYER=FILE"
    );
    specs
        .iter()
        .map(|spec| {
            let (layer_name, path) = spec.split_once('=')
                .ok_or_else(|| anyhow::anyhow!("each --edit must name a layer and file, such as `shared=rules.md` or `tool:claude=claude.md`"))?;
            ensure!(!path.is_empty(), "each --edit needs a text file path after `=`");
            let layer = if layer_name == "shared" {
                PersonalRuleLayer::Shared
            } else if let Some(tool) = layer_name.strip_prefix("tool:") {
                ensure!(!tool.trim().is_empty(), "name a tool after `tool:` in --edit");
                PersonalRuleLayer::Tool { tool: tool.to_owned() }
            } else {
                bail!("unknown rule layer `{layer_name}`; use `shared` or `tool:NAME`");
            };
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("reading personal rule text from {path}"))?;
            Ok(PersonalRuleEdit { layer, text })
        })
        .collect()
}

fn personal_layer_label(layer: &wenlan_types::personal_rules::PersonalRuleLayer) -> String {
    match layer {
        wenlan_types::personal_rules::PersonalRuleLayer::Shared => "shared".into(),
        wenlan_types::personal_rules::PersonalRuleLayer::Tool { tool } => format!("tool:{tool}"),
    }
}

fn personal_rule_show(shown: &wenlan_types::personal_rules::PersonalRuleShow) -> String {
    let mut output = format!("Layer: {}\n", personal_layer_label(&shown.layer));
    if shown.text.is_empty() {
        output.push_str("(no rules in this layer)\n");
    } else {
        output.push_str(&printable(&shown.text));
        if !shown.text.ends_with('\n') {
            output.push('\n');
        }
    }
    output.push_str(&format!(
        "Affected tools: {}\n",
        if shown.affected_tools.is_empty() {
            "none".into()
        } else {
            shown.affected_tools.join(", ")
        }
    ));
    for (index, origin) in shown.origins.iter().enumerate() {
        if let Some(origin) = origin {
            output.push_str(&format!(
                "Line {} origin: author {}; proposer {}; confirmer {}; time {}\n",
                index + 1,
                printable(origin.author.as_deref().unwrap_or("unknown")),
                printable(origin.proposer.as_deref().unwrap_or("none")),
                printable(origin.confirmer.as_deref().unwrap_or("unknown")),
                origin
                    .time
                    .map_or_else(|| "unknown".into(), |time| time.to_string())
            ));
            for page in &origin.pages {
                writeln!(
                    output,
                    "  From {}, line {} · author {} · proposer {} · confirmer {}",
                    page.location
                        .page_id
                        .as_deref()
                        .map_or_else(|| "conversation preferences".into(), one_line),
                    page.location.line,
                    page.author
                        .as_deref()
                        .map_or_else(|| "unknown".into(), one_line),
                    one_line(&page.proposer),
                    one_line(&page.confirmer)
                )
                .unwrap();
            }
        }
    }
    output
}

fn personal_rule_plan(plan: &wenlan_types::personal_rules::PersonalRulePlan) -> String {
    let mut output = format!("Plan {} (expires {})\n", plan.id, plan.expires_at);
    output.push_str(&format!(
        "Proposed by: {}\n",
        printable(plan.proposer.as_deref().unwrap_or("person or unknown")),
    ));
    if let Some(merge) = &plan.page_merge {
        writeln!(
            output,
            "Move {} rules from your pages into your rules.",
            merge.rule_count
        )
        .unwrap();
        if merge.excludes_space_only {
            writeln!(output, "Rules for one Space stay on their pages.").unwrap();
        }
        for excluded in &merge.excluded {
            writeln!(
                output,
                "Possible credential, skipped: {}, line {}",
                excluded
                    .page_id
                    .as_deref()
                    .map_or_else(|| "conversation preferences".into(), one_line),
                excluded.line
            )
            .unwrap();
        }
    } else {
        output.push_str("Author on confirmation: person\n");
    }
    for change in &plan.changes {
        output.push_str(&format!(
            "\n{}\nAffected tools: {}\n",
            personal_layer_label(&change.layer),
            if change.affected_tools.is_empty() {
                "none".into()
            } else {
                change.affected_tools.join(", ")
            }
        ));
        output.push_str(&printable(&change.diff));
        if !change.diff.ends_with('\n') {
            output.push('\n');
        }
    }
    if !plan.blocked_tools.is_empty() {
        output.push_str(&format!(
            "\nBlocked tools: {}\n",
            plan.blocked_tools.join(", ")
        ));
    }
    if !plan.unmanaged_tools.is_empty() {
        output.push_str(&format!(
            "\nNot delivered to unmanaged tools: {}\n",
            plan.unmanaged_tools.join(", ")
        ));
    }
    output
}

fn personal_rule_version(version: &wenlan_types::personal_rules::PersonalRuleVersion) -> String {
    let mut output = format!("Version {}\n", version.summary.id);
    for rule in &version.rules {
        output.push('\n');
        output.push_str(&personal_rule_show(rule));
    }
    output
}

#[derive(Debug, Subcommand)]
pub enum RulesSetupCmd {
    /// Confirm or decline edits to team and project rule sections.
    SharedEdits,
    /// Turn on personal tool sync for this data root.
    On,
    /// Turn off personal tool sync without restoring files.
    Off,
    /// Bring in your personal AI-tool setup and preview the changes.
    Adopt {
        /// Preview the changes, or list the installed files while offline.
        #[arg(long)]
        dry_run: bool,
        /// Confirm the plan shown to the person.
        #[arg(long, conflicts_with = "dry_run")]
        confirm: Option<String>,
    },
    /// Share your rule lines, skills, commands, subagents and MCP servers with
    /// every managed tool that supports them.
    Share {
        /// Preview what each tool would get.
        #[arg(long)]
        dry_run: bool,
        /// Show every item in the preview.
        #[arg(long)]
        details: bool,
        /// Keep numbered rule lines in their own tools.
        #[arg(long, value_delimiter = ',', value_name = "N[,N]")]
        keep_rule: Vec<usize>,
        /// Confirm the plan shown to the person.
        #[arg(long, conflicts_with = "dry_run")]
        confirm: Option<String>,
        /// Also offer the MCP servers that may contain a key. You are still
        /// asked to type y or yes; an AI tool cannot share them.
        #[arg(long, conflicts_with_all = ["dry_run", "confirm"])]
        include_keys: bool,
    },
    /// Status of the personal setup in each tool.
    #[command(hide = !rulesync_personal::enabled())]
    Status {
        /// Check for edits and sync approved content now.
        #[arg(long)]
        sync: bool,
    },
    /// To review: list changes, or answer one item.
    ///
    /// Skill proposals and updates also accept `dismiss`.
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
    /// Restore the original tool setup and keep rules you added.
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
    #[value(hide = true)]
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
    agent_name: Option<&str>,
    explicit_session: Option<&str>,
) -> Result<()> {
    run_inner(command, format, quiet, agent_name, explicit_session)
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
    agent_name: Option<&str>,
    explicit_session: Option<&str>,
) -> Result<()> {
    let session = wenlan_types::session_context::resolve_session_from_env(explicit_session);
    if matches!(command, RulesSetupCmd::On | RulesSetupCmd::Off) {
        ensure!(agent_name.is_none() && session.session.is_none(), "Only you can change your personal tool setup. Run this from your own terminal outside an AI session.");
        let on = matches!(command, RulesSetupCmd::On);
        rulesync_personal::set_enabled(on)?;
        println!("Tool sync is {}.", if on { "on" } else { "off" });
        return Ok(());
    }
    if matches!(command, RulesSetupCmd::SharedEdits) {
        let client = after_shared_edit_gate(
            agent_name.is_some() || session.session.is_some(),
            io::stdin().is_terminal(),
            io::stdout().is_terminal(),
            || WenlanClient::for_local_rules_with_identity(agent_name, session.session.as_ref()),
        )?;
        return run_shared_edits(&client).await;
    }
    if matches!(command, RulesSetupCmd::Adopt { dry_run: true, .. }) {
        if rulesync_personal::enabled() {
            if let Ok(client) =
                WenlanClient::for_local_rules_with_identity(agent_name, session.session.as_ref())
            {
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
    if let RulesSetupCmd::Share {
        dry_run: true,
        details,
        keep_rule,
        ..
    } = command
    {
        let client =
            WenlanClient::for_local_rules_with_identity(agent_name, session.session.as_ref())?;
        let plan = client
            .preview_share_as_person(&share_request(keep_rule)?)
            .await?;
        return show(format, quiet, &plan, |plan| {
            sharing_for_a_person(plan, *details, false)
        });
    }
    let confirmed_adopt = matches!(
        command,
        RulesSetupCmd::Adopt {
            confirm: Some(_),
            ..
        } | RulesSetupCmd::Share {
            confirm: Some(_),
            ..
        }
    );
    if matches!(command, RulesSetupCmd::Adopt { confirm: None, .. })
        && (agent_name.is_some() || session.session.is_some())
    {
        bail!("Only you can change your personal tool setup. Run this from your own terminal outside an AI session. From an AI tool: show the person `wenlan rules adopt --dry-run`, and after they say yes run `wenlan rules adopt --confirm <plan id>`.");
    }
    if matches!(command, RulesSetupCmd::Share { confirm: None, .. })
        && (agent_name.is_some() || session.session.is_some())
    {
        bail!("Only you can change your personal tool setup. Run this from your own terminal outside an AI session. From an AI tool: show the person `wenlan rules share --dry-run`, and after they say yes run `wenlan rules share --confirm <plan id>`.");
    }
    ensure!(
        confirmed_adopt || matches!(command, RulesSetupCmd::Status { .. } | RulesSetupCmd::Review { id: None, .. })
            || (agent_name.is_none() && session.session.is_none()),
        "Only you can change your personal tool setup. Run this from your own terminal outside an AI session."
    );
    ensure!(
        confirmed_adopt
            || matches!(
                command,
                RulesSetupCmd::Sync
                    | RulesSetupCmd::Status { .. }
                    | RulesSetupCmd::Review { id: None, .. }
                    | RulesSetupCmd::SkillUpdates
                    | RulesSetupCmd::Settings { dry_run: true, .. }
            )
            || (io::stdin().is_terminal() && io::stdout().is_terminal()),
        "Run this command from your own terminal and type yes when asked."
    );

    let client = WenlanClient::for_local_rules_with_identity(agent_name, session.session.as_ref())?;
    let report = match command {
        RulesSetupCmd::SharedEdits => unreachable!(),
        RulesSetupCmd::On | RulesSetupCmd::Off => unreachable!(),
        RulesSetupCmd::Adopt {
            confirm: confirmed, ..
        } => {
            let expected = AdoptPreviewRequest {
                home: rulesync_personal::system_home()?.canonicalize()?,
            };
            let plan = client.preview_adoption_as_person(&expected).await?;
            if let Some(id) = confirmed {
                ensure!(plan.plan_id == *id && !id.is_empty(), "The setup changed since that preview. Run `wenlan rules adopt --dry-run` again and show the person the new list.");
            } else {
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
            }
            client
                .adopt_rules_as_person(&AdoptRequest {
                    token: plan.token,
                    confirmed_plan_id: confirmed.clone(),
                    home: Some(expected.home),
                    include_keys: false,
                })
                .await?
        }
        RulesSetupCmd::Share {
            confirm: confirmed,
            details,
            keep_rule,
            ..
        } => match share_with_person(&client, *details, keep_rule, confirmed.as_deref(), false)
            .await?
        {
            ShareResult::Applied(report) => report,
            ShareResult::Cancelled | ShareResult::Nothing => return Ok(()),
        },
        RulesSetupCmd::Restore => {
            if !confirm("Restore the original tool setup and keep rules you added?")? {
                return Ok(());
            }
            client.restore_rules_as_person().await?
        }
        RulesSetupCmd::Status { sync } => {
            let sync_report = if *sync {
                Some(client.sync_rules_as_person().await?)
            } else {
                None
            };
            let status = client.rules_setup_status().await;
            if let Some(report) = sync_report.filter(|_| !quiet) {
                let shown_notes = status.as_ref().map_or(&[][..], |status| &status.notes);
                eprint!("{}", report_for_a_person(&report, false, shown_notes));
            }
            return show(format, quiet, &status?, status_for_a_person);
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
    let show_kept_in_place = matches!(
        command,
        RulesSetupCmd::Share { .. }
            | RulesSetupCmd::Sync
            | RulesSetupCmd::Adopt {
                confirm: Some(_),
                ..
            }
    );
    show(format, quiet, &report, |report| {
        report_for_a_person(report, show_kept_in_place, &[])
    })
}

async fn answer_item(
    client: &WenlanClient,
    format: ResolvedFormat,
    quiet: bool,
    item: RulesReviewItem,
    action: PullbackAction,
    file: Option<String>,
) -> Result<()> {
    let action = match (&item.content, action) {
        (RulesReviewContent::ToolEdit { card }, PullbackAction::Dismiss) => match card.kind {
            PullbackKind::Scope => PullbackAction::ToolOnly,
            PullbackKind::Conflict => {
                bail!("This review puts the tool back to the earlier version. Use `revert`.");
            }
        },
        (RulesReviewContent::ToolEdit { card }, PullbackAction::Revert)
            if card.kind == PullbackKind::Scope =>
        {
            bail!("This rule-line review has nothing to put back. Use `tool-only`.");
        }
        (_, action) => action,
    };
    if !item.answers.contains(&action) {
        let tool_edit = matches!(item.content, RulesReviewContent::ToolEdit { .. });
        bail!(
            "Available answers: {}",
            item.answers
                .iter()
                .filter(|&&answer| !tool_edit || answer != PullbackAction::Dismiss)
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
        RulesReviewContent::PersonalPlan { .. } => {
            bail!("Confirm this personal rule plan in Wenlan or from your own terminal.")
        }
    };
    show(format, quiet, &report, |report| {
        report_for_a_person(report, false, &[])
    })
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
        PullbackAction::ToolOnly | PullbackAction::Dismiss => {
            format!("Keep proposal {proposal} for this tool only?")
        }
        PullbackAction::AllTools => format!("Use proposal {proposal} in all compatible tools?"),
        PullbackAction::Revert => {
            format!("Put the tool back to the earlier version for proposal {proposal}?")
        }
    };
    person_says_yes(input, output, &question)
}

fn adoption_plan() -> Result<AdoptionPlan> {
    rulesync_personal::plan_adoption_offline(&rulesync_personal::system_home()?)
}

/// What `wenlan setup` tells the person once their tool setup is in.
#[derive(Debug, Serialize)]
struct SetupSync {
    tools: Vec<String>,
    rules: Option<usize>,
    skills: usize,
    following: usize,
    copied: usize,
    left_alone: usize,
}

fn share_request(keep_rule: &[usize]) -> Result<SharePreviewRequest> {
    Ok(SharePreviewRequest {
        home: rulesync_personal::system_home()?.canonicalize()?,
        keep_rule: Some(keep_rule.to_vec()),
    })
}

enum ShareResult {
    Applied(RulesSyncReport),
    Cancelled,
    Nothing,
}

/// Both setup and `rules share` use the same preview, approval and write path.
async fn share_with_person(
    client: &WenlanClient,
    details: bool,
    keep_rule: &[usize],
    confirmed: Option<&str>,
    setup: bool,
) -> Result<ShareResult> {
    let expected = share_request(keep_rule)?;
    let plan = client.preview_share_as_person(&expected).await?;
    let include_keys = if let Some(id) = confirmed {
        ensure!(plan.plan_id == id && !id.is_empty(), "The setup changed after the preview. Run `wenlan rules share --dry-run` again and show the person the new list.");
        false
    } else {
        // The person must see the plan even with --quiet or --format json.
        let preview = sharing_for_a_person(&plan, details, setup);
        if setup {
            print!("{}", wrap_setup_text(&preview)?);
        } else {
            print!("{preview}");
        }
        if !has_safe_share(&plan) && plan.key_servers.is_empty() {
            return Ok(ShareResult::Nothing);
        }
        let Some(include_keys) =
            share_confirmation(&plan, &mut io::stdin().lock(), &mut io::stdout())?
        else {
            if !setup {
                println!("Cancelled.");
            }
            return Ok(ShareResult::Cancelled);
        };
        if !has_safe_share(&plan) && !include_keys {
            return Ok(ShareResult::Nothing);
        }
        include_keys
    };
    let report = client
        .share_rules_as_person(&ShareRequest {
            approval: AdoptRequest {
                token: plan.token,
                confirmed_plan_id: confirmed.map(str::to_owned),
                home: Some(expected.home),
                include_keys,
            },
            keep_rule: expected.keep_rule,
        })
        .await?;
    Ok(ShareResult::Applied(report))
}

fn has_safe_share(plan: &SharingPlan) -> bool {
    !plan.tools.is_empty() || !plan.rules.is_empty()
}

/// The key group always needs its own answer, including when it is the only group.
fn share_confirmation(
    plan: &SharingPlan,
    input: &mut dyn io::BufRead,
    output: &mut dyn io::Write,
) -> Result<Option<bool>> {
    let question = if plan.tools.is_empty() && plan.key_servers.is_empty() && !plan.rules.is_empty()
    {
        "Apply these rule choices?"
    } else {
        "Copy these items to the tools above?"
    };
    if !person_says_yes(input, output, question)? {
        return Ok(None);
    }
    let include_keys = if plan.key_servers.is_empty() {
        false
    } else {
        writeln!(output)?;
        person_says_yes(
            input,
            output,
            "Also copy the servers that may contain a key?",
        )?
    };
    Ok(Some(include_keys))
}

/// Wrap at spaces without cutting words. Refuse an unshowable preview rather
/// than asking someone to approve a shortened or misleading item name.
pub(super) fn wrap_setup_text(text: &str) -> Result<String> {
    let mut out = String::new();
    for line in text.lines() {
        let indent: String = line.chars().take_while(|ch| *ch == ' ').collect();
        let mut current = indent.clone();
        for word in line.split_whitespace() {
            let width = console::measure_text_width(word);
            ensure!(
                width + indent.len() < 80,
                "A name is too long to show here. Run `wenlan rules share --details`."
            );
            let spacer = usize::from(current.len() > indent.len());
            if console::measure_text_width(&current) + spacer + width >= 80 {
                out.push_str(&current);
                out.push('\n');
                current.clone_from(&indent);
            }
            if current.len() > indent.len() {
                current.push(' ');
            }
            current.push_str(word);
        }
        out.push_str(&current);
        out.push('\n');
    }
    Ok(out)
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum StayReason {
    Credentials,
    Placeholders,
    Directives,
    Format,
    Conflict,
    Other,
}

impl StayReason {
    fn from_reason(reason: &str) -> Self {
        let reason = reason.to_ascii_lowercase();
        if reason.contains("credential") || reason.contains("may contain a key") {
            Self::Credentials
        } else if reason.contains("'$'") || reason.contains("$ placeholder") {
            Self::Placeholders
        } else if reason.contains("'!'") {
            Self::Directives
        } else if reason.contains("format")
            || reason.contains("frontmatter")
            || reason.contains("cannot read")
            || reason.contains("cannot represent")
            || reason.contains("could not be converted")
            || reason.contains("do not interpret")
        {
            Self::Format
        } else if reason.contains("same name")
            || reason.contains("different file")
            || reason.contains("conflict")
            || (reason.contains("different") && reason.contains("name"))
            || reason.contains("own version")
        {
            Self::Conflict
        } else {
            Self::Other
        }
    }

    fn summary(self, count: usize) -> String {
        let plural = count != 1;
        let words = match self {
            Self::Credentials => "may hold credentials",
            Self::Placeholders if plural => "use $ placeholders",
            Self::Placeholders => "uses $ placeholders",
            Self::Directives if plural => "use ! directives",
            Self::Directives => "uses ! directives",
            Self::Format if plural => "use a format other tools can't read",
            Self::Format => "uses a format other tools can't read",
            Self::Conflict if plural => "have a conflicting item in another tool",
            Self::Conflict => "has a conflicting item in another tool",
            Self::Other => "can't be copied for other reasons",
        };
        format!("{count} {words}")
    }
}

fn sharing_for_a_person(plan: &SharingPlan, details: bool, setup: bool) -> String {
    let mut out = String::new();
    if !has_safe_share(plan) && plan.key_servers.is_empty() {
        out.push_str("Nothing to share.\n");
    } else {
        out.push_str("Share with your other tools\n");
    }
    for tool in &plan.tools {
        let groups = [
            ("skill", &tool.skills),
            ("command", &tool.commands),
            ("subagent", &tool.subagents),
            ("MCP server", &tool.mcp_servers),
        ];
        let counts = (tool.rules > 0)
            .then(|| plural(tool.rules, "rule"))
            .into_iter()
            .chain(
                groups
                    .iter()
                    .filter(|(_, names)| !names.is_empty())
                    .map(|(word, names)| plural(names.len(), word)),
            )
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(&mut out, "\n{}: {counts}", one_line(&tool.tool)).unwrap();
        if details {
            for (word, names) in groups {
                for name in names {
                    writeln!(&mut out, "  + {word} {}", one_line(name)).unwrap();
                }
            }
        }
    }
    append_rule_preview(&mut out, plan, details, if setup { 79 } else { 100 });
    if plan.already_there > 0 {
        writeln!(
            &mut out,
            "\n{} already in every tool that supports {}.",
            plural(plan.already_there, "item"),
            if plan.already_there == 1 {
                "it"
            } else {
                "them"
            }
        )
        .unwrap();
    }
    if !plan.key_servers.is_empty() {
        // Names and tools only; no value of these servers is ever shown.
        let count = plan.key_servers.len();
        writeln!(
            &mut out,
            "\n{} may contain a key. Sharing copies the key into: {}.",
            if count == 1 {
                "This server".to_owned()
            } else {
                format!("These {count} servers")
            },
            plan.key_tools
                .iter()
                .map(|tool| one_line(tool))
                .collect::<Vec<_>>()
                .join(", ")
        )
        .unwrap();
        for name in &plan.key_servers {
            writeln!(&mut out, "  ? MCP server {}", one_line(name)).unwrap();
        }
        out.push_str("Only you can share them, with a second yes in your own terminal.\n");
    }
    if !plan.stays.is_empty() {
        out.push('\n');
        if details {
            let mut seen = BTreeSet::new();
            for stay in &plan.stays {
                if !stay.item.starts_with("subagent ") {
                    writeln!(
                        &mut out,
                        "Stays in {}: {}: {}",
                        one_line(&stay.tool),
                        one_line(&stay.item),
                        one_line(&stay.reason)
                    )
                    .unwrap();
                    continue;
                }
                if !seen.insert((&stay.tool, &stay.item)) {
                    continue;
                }
                let mut reasons = BTreeSet::new();
                let mut tools = BTreeSet::new();
                for same in plan
                    .stays
                    .iter()
                    .filter(|other| other.tool == stay.tool && other.item == stay.item)
                {
                    let (reason, destinations) = same
                        .reason
                        .rsplit_once(" Not copied to ")
                        .unwrap_or((&same.reason, ""));
                    reasons.insert(one_line(reason.trim_end_matches('.')).replace(". ", "; "));
                    tools.extend(
                        destinations
                            .trim_end_matches('.')
                            .split(", ")
                            .filter(|tool| !tool.is_empty())
                            .map(one_line),
                    );
                }
                let mut reason = reasons.into_iter().collect::<Vec<_>>().join("; ");
                if !tools.is_empty() {
                    write!(
                        &mut reason,
                        "; not copied to {}",
                        tools.into_iter().collect::<Vec<_>>().join(", ")
                    )
                    .unwrap();
                }
                writeln!(
                    &mut out,
                    "Stays in {}: {}: {reason}.",
                    one_line(&stay.tool),
                    one_line(&stay.item)
                )
                .unwrap();
            }
        } else {
            let mut items = BTreeMap::new();
            for stay in &plan.stays {
                let reason = StayReason::from_reason(&stay.reason);
                items
                    .entry((&stay.tool, &stay.item))
                    .and_modify(|old: &mut StayReason| *old = (*old).min(reason))
                    .or_insert(reason);
            }
            let mut counts = BTreeMap::new();
            for reason in items.values() {
                *counts.entry(*reason).or_insert(0) += 1;
            }
            let count = items.len();
            writeln!(
                &mut out,
                "{} {} in {} own tool: {}.",
                plural(count, "item"),
                if count == 1 { "stays" } else { "stay" },
                if count == 1 { "its" } else { "their" },
                counts
                    .into_iter()
                    .map(|(reason, count)| reason.summary(count))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
            .unwrap();
        }
    }
    if !details && (has_safe_share(plan) || !plan.stays.is_empty() || !plan.rule_stays.is_empty()) {
        out.push_str("\nAdd --details to see every item.\n");
    }
    if has_safe_share(plan) && !plan.plan_id.is_empty() {
        writeln!(
            &mut out,
            "\nPlan id: {}\nFrom an AI tool: wenlan rules share --confirm {}{}",
            plan.plan_id,
            plan.plan_id,
            kept_rule_args(plan)
        )
        .unwrap();
    }
    out
}

fn append_rule_preview(out: &mut String, plan: &SharingPlan, details: bool, width: usize) {
    let kept = plan.rules.iter().filter(|rule| rule.kept).count();
    if kept > 0 {
        writeln!(
            out,
            "\nKeep {} in {} own {}.",
            plural(kept, "rule line"),
            if kept == 1 { "its" } else { "their" },
            if kept == 1 { "tool" } else { "tools" }
        )
        .unwrap();
    }
    if plan.tools.iter().all(|tool| tool.rules == 0) {
        let moving = plan.rules.len() - kept;
        if moving > 0 {
            writeln!(
                out,
                "\nMove {} into the shared rules.",
                plural(moving, "rule line")
            )
            .unwrap();
        }
    }
    for note in &plan.rule_notes {
        writeln!(out, "\n{}", one_line(note)).unwrap();
    }
    if details {
        if !plan.rules.is_empty() {
            out.push_str("\nNumbered rule lines\n");
            for rule in &plan.rules {
                writeln!(
                    out,
                    "  {}. {} [{}{}]",
                    rule.number,
                    one_line(&rule.line),
                    rule.tools
                        .iter()
                        .map(|tool| one_line(tool))
                        .collect::<Vec<_>>()
                        .join(", "),
                    if rule.kept { ", keep here" } else { "" }
                )
                .unwrap();
            }
            out.push_str("Use --keep-rule N[,N] to keep rule lines where they are.\n");
        }
        for stay in &plan.rule_stays {
            let description = stay
                .line
                .as_deref()
                .map_or_else(|| "a line that may hold a credential".into(), one_line);
            if stay.line.is_some() {
                writeln!(
                    out,
                    "Stays in {}: {description} ({})",
                    one_line(&stay.tool),
                    one_line(&stay.reason)
                )
                .unwrap();
            } else {
                let location = stay
                    .line_number
                    .map(|number| format!(" (line {number})"))
                    .unwrap_or_default();
                writeln!(
                    out,
                    "Stays in {}{location}: {description}.",
                    one_line(&stay.tool)
                )
                .unwrap();
            }
        }
    } else {
        if kept < plan.rules.len() {
            out.push_str("\nRule lines to share\n");
            for rule in plan.rules.iter().filter(|rule| !rule.kept) {
                let line = format!("  {}. {}", rule.number, one_line(&rule.line));
                writeln!(out, "{}", console::truncate_str(&line, width, "…")).unwrap();
            }
        }
        if !plan.rule_stays.is_empty() {
            let credentials = plan
                .rule_stays
                .iter()
                .filter(|stay| stay.line.is_none())
                .count();
            let local = plan.rule_stays.len() - credentials;
            let mut reasons = Vec::new();
            if local > 0 {
                reasons.push(format!("{local} tool-only"));
            }
            if credentials > 0 {
                reasons.push(format!("{credentials} with possible credentials"));
            }
            let place = if plan.rule_stays.len() == 1 {
                "stays in its own tool"
            } else {
                "stay in their own tools"
            };
            writeln!(
                out,
                "{} {place} ({}).",
                plural(plan.rule_stays.len(), "rule line"),
                reasons.join(", "),
            )
            .unwrap();
        }
    }
}

fn kept_rule_args(plan: &SharingPlan) -> String {
    let numbers = plan
        .rules
        .iter()
        .filter(|rule| rule.kept)
        .map(|rule| rule.number.to_string())
        .collect::<Vec<_>>();
    if numbers.is_empty() {
        String::new()
    } else {
        format!(" --keep-rule {}", numbers.join(","))
    }
}

fn plural(count: usize, word: &str) -> String {
    format!("{count} {word}{}", if count == 1 { "" } else { "s" })
}

fn setup_sync_line(sync: &SetupSync) -> String {
    let tools = match sync.tools.as_slice() {
        [] => "your AI tools".to_string(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    };
    let rules = sync.rules.map_or_else(
        || "rule count unavailable".to_string(),
        |count| plural(count, "rule"),
    );
    let mut line = format!("Set up {tools}: {rules}, {}.", plural(sync.skills, "skill"));
    if sync.left_alone > 0 {
        line.push_str(&format!(
            " {} {} left as {} {}.",
            plural(sync.left_alone, "item"),
            if sync.left_alone == 1 { "was" } else { "were" },
            if sync.left_alone == 1 { "it" } else { "they" },
            if sync.left_alone == 1 { "was" } else { "were" }
        ));
    }
    line
}

/// Running setup is the person's consent: turn tool sync on, bring the setup
/// in and sync once, with no question and no terminal needed. A plan that
/// cannot be adopted safely is left alone with one line saying so; setup
/// itself still succeeds.
pub(super) async fn adopt_during_setup(format: ResolvedFormat) -> bool {
    match try_adopt_during_setup(format).await {
        Ok(adopted) => adopted,
        Err(error) => {
            let message = format!(
            "Your tool setup could not be fully set up: {}.\nSee `wenlan rules adopt --dry-run`.\n",
            one_line(&printable(&format!("{error:#}"))).trim_end_matches('.')
        );
            print!("{}", wrap_setup_text(&message).unwrap_or_else(|_| {
            "Your tool setup could not be fully set up.\nSee `wenlan rules adopt --dry-run`.\n".to_string()
        }));
            false
        }
    }
}

pub(super) async fn share_during_setup(details: bool, keep_rule: &[usize]) {
    let result = async {
        let client = WenlanClient::for_local_rules_with_identity(None, None)?;
        share_with_person(&client, details, keep_rule, None, true).await
    }
    .await;
    match result {
        Ok(ShareResult::Cancelled) => {
            println!("\nRun wenlan rules share to copy skills to your other tools.");
        }
        Ok(ShareResult::Nothing) => {}
        Ok(ShareResult::Applied(report)) => {
            let mut text = format!("\n{}\n", one_line(&report.message));
            if !report.blocked.is_empty() {
                text.push_str(&format!(
                    "{} could not be copied.\n",
                    plural(report.blocked.len(), "item")
                ));
            }
            if !report.proposals.is_empty() {
                let count = report.proposals.len();
                text.push_str(&format!(
                    "{} {} review. Run `wenlan rules review`.\n",
                    plural(count, "item"),
                    if count == 1 { "needs" } else { "need" }
                ));
            }
            append_kept_in_place_summary(&mut text, &report.kept_in_place);
            match wrap_setup_text(&text) {
                Ok(text) => print!("{text}"),
                Err(_) => println!("Sharing finished. Run `wenlan rules status` for details."),
            }
        }
        Err(error) => {
            let message = format!(
                "\nCould not share: {}.\nRun `wenlan rules share` to try again.\n",
                one_line(&printable(&format!("{error:#}"))).trim_end_matches('.')
            );
            print!(
                "{}",
                wrap_setup_text(&message).unwrap_or_else(|_| {
                    "\nCould not share. Run `wenlan rules share` to try again.\n".to_string()
                })
            );
        }
    }
}

async fn try_adopt_during_setup(format: ResolvedFormat) -> Result<bool> {
    rulesync_personal::set_enabled(true)?;
    // An AI tool running setup says who it is; the daemon then adopts only
    // the plan it just previewed (the same check as `rules adopt --confirm`).
    let session = wenlan_types::session_context::resolve_session_from_env(None);
    let client = WenlanClient::for_local_rules_with_identity(None, session.session.as_ref())?;
    if client.rules_setup_status().await?.adopted {
        return Ok(true);
    }
    let home = rulesync_personal::system_home()?.canonicalize()?;
    let plan = client
        .preview_adoption_as_person(&AdoptPreviewRequest { home: home.clone() })
        .await?;
    if plan.files.is_empty()
        && plan.settings.is_none()
        && plan.skills.is_empty()
        && plan.tools.is_empty()
    {
        return Ok(false);
    }
    ensure!(
        !plan.note_codes.iter().any(|note| matches!(
            note.code,
            RulesSetupCode::OfflineInventory | RulesSetupCode::SkillPreviewFailed
        )),
        "Wenlan could not preview it completely"
    );
    let report = client
        .adopt_rules_as_person(&AdoptRequest {
            token: plan.token.clone(),
            confirmed_plan_id: Some(plan.plan_id.clone()).filter(|id| !id.is_empty()),
            home: Some(home),
            include_keys: false,
        })
        .await?;
    ensure!(
        report.blocked.is_empty(),
        "{} need your review",
        plural(report.blocked.len(), "item")
    );
    let synced = client.sync_rules_as_person().await?;
    ensure!(
        synced.blocked.is_empty(),
        "{} could not be synced",
        plural(synced.blocked.len(), "item")
    );
    // Name only the tools that ended up managed, not everything the plan listed.
    let managed = client.rules_setup_status().await.ok().map(|status| {
        status
            .tools
            .into_iter()
            .filter(|tool| tool.state != RulesToolState::NotManaged)
            .map(|tool| tool.tool.name)
            .collect::<std::collections::BTreeSet<_>>()
    });
    let count = |wanted: fn(&AdoptionSkillOutcome) -> bool| {
        plan.skills
            .iter()
            .filter(|skill| wanted(&skill.outcome))
            .count()
    };
    let following =
        count(|outcome| matches!(outcome, AdoptionSkillOutcome::TrackedUpstream { .. }));
    let copied = count(|outcome| matches!(outcome, AdoptionSkillOutcome::PlainCopy));
    let sync = SetupSync {
        tools: plan
            .tools
            .iter()
            .filter(|tool| {
                managed
                    .as_ref()
                    .is_none_or(|managed| managed.contains(&tool.name))
            })
            .map(|tool| tool.name.clone())
            .collect(),
        rules: plan.rule_lines,
        skills: following + copied,
        following,
        copied,
        left_alone: count(|outcome| matches!(outcome, AdoptionSkillOutcome::Skipped { .. })),
    };
    match format {
        ResolvedFormat::Json => println!("{}", terminal_json(&sync)?),
        ResolvedFormat::Table => {
            print!("{}", wrap_setup_text(&printable(&setup_sync_line(&sync)))?);
            println!("To undo: wenlan rules restore");
        }
    }
    Ok(true)
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
    for path in &plan.rule_sources {
        writeln!(
            &mut out,
            "\nWenlan imports {} once and leaves the file in place.",
            one_line(path)
        )
        .unwrap();
    }
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
    if !plan.new_files.is_empty() {
        let tools = plan
            .new_files
            .iter()
            .map(|new| one_line(&new.tool))
            .collect::<Vec<_>>()
            .join(", ");
        let files: usize = plan.new_files.iter().map(|new| new.files).sum();
        writeln!(&mut out, "\nWill also write to: {tools} ({files} files).").unwrap();
    }
    if let Some(settings) = &plan.settings {
        out.push('\n');
        out.push_str(&settings_plan_for_a_person(settings));
    }
    append_kept_in_place_summary(&mut out, &plan.kept_in_place);
    if !plan.plan_id.is_empty() {
        writeln!(
            &mut out,
            "Plan id: {} — to go ahead from an AI tool: wenlan rules adopt --confirm {}",
            plan.plan_id, plan.plan_id
        )
        .unwrap();
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
            RulesReviewContent::PersonalPlan { plan } => {
                out.push('\n');
                out.push_str(&personal_rule_plan(plan));
                if plan.page_merge.is_some() {
                    out.push_str("  wenlan rules merge-pages\n");
                } else {
                    writeln!(out, "  wenlan rules shared apply {}", plan.id).unwrap();
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
    for note in &status.notes {
        writeln!(out, "{}", one_line(note)).unwrap();
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
            "{} — {state} — {}, {}, {}, {}, {}",
            one_line(&row.tool.name),
            plural(counts.rules, "rule"),
            plural(counts.skills, "skill"),
            plural(counts.commands, "command"),
            plural(counts.subagents, "subagent"),
            plural(counts.mcp, "MCP server")
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

fn report_for_a_person(
    report: &RulesSyncReport,
    show_kept_in_place: bool,
    shown_notes: &[String],
) -> String {
    let mut out = format!("{}\n", printable(&report.message));
    for path in &report.written {
        out.push_str(&format!("Wrote {}\n", file_label(path)));
    }
    for path in report
        .blocked
        .iter()
        .filter(|note| !shown_notes.contains(note))
    {
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
    if show_kept_in_place {
        append_kept_in_place_summary(&mut out, &report.kept_in_place);
    }
    out
}

fn append_kept_in_place_summary(
    out: &mut String,
    kept_in_place: &[wenlan_types::rulesync::KeptInPlace],
) {
    let count = kept_in_place
        .iter()
        .map(|entry| entry.count)
        .fold(0usize, usize::saturating_add);
    if count > 0 {
        if count == 1 {
            out.push_str(
                "Kept 1 setting in its own tool because it may hold a key or cannot be copied.\n",
            );
        } else {
            writeln!(
                out,
                "Kept {count} settings in place in their own tools because they may hold keys or cannot be copied."
            )
            .unwrap();
        }
    }
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
    use wenlan_types::personal_rules::PersonalRuleLayer;
    use wenlan_types::rulesync::{
        AdoptionChange, SharingRule, SharingRuleStay, SharingStay, SharingTool,
    };

    #[test]
    fn personal_rule_proposal_reads_multiple_explicit_layers_as_one_edit_set() {
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared.md");
        let tool = dir.path().join("claude.md");
        std::fs::write(&shared, "Shared line.\n").unwrap();
        std::fs::write(&tool, "Claude only.\n").unwrap();
        let specs = vec![
            format!("shared={}", shared.display()),
            format!("tool:claude={}", tool.display()),
        ];
        let edits = read_personal_rule_edits(&specs).unwrap();
        assert_eq!(edits.len(), 2);
        assert!(matches!(&edits[0].layer, PersonalRuleLayer::Shared));
        assert_eq!(edits[0].text, "Shared line.\n");
        assert!(matches!(
            &edits[1].layer,
            PersonalRuleLayer::Tool { tool } if tool == "claude"
        ));
        assert_eq!(edits[1].text, "Claude only.\n");
    }

    #[test]
    fn personal_rule_proposal_refuses_an_unspecified_or_unknown_layer() {
        assert!(read_personal_rule_edits(&["rules.md".into()]).is_err());
        assert!(read_personal_rule_edits(&["tool:=rules.md".into()]).is_err());
        assert!(read_personal_rule_edits(&["project=rules.md".into()]).is_err());
    }

    #[test]
    fn personal_rule_plan_shows_an_unmanaged_tool_is_not_delivered() {
        let plan: wenlan_types::personal_rules::PersonalRulePlan =
            serde_json::from_value(serde_json::json!({
                "id": "plan", "expires_at": 1, "proposer": null,
                "changes": [], "blocked_tools": [],
                "unmanaged_tools": ["geminicli"]
            }))
            .unwrap();
        assert!(personal_rule_plan(&plan).contains("Not delivered to unmanaged tools: geminicli"));
    }

    #[test]
    fn personal_rule_apply_refuses_ai_identity_and_piped_input_or_output() {
        assert_eq!(require_personal_rule_confirmation(true, true, true).unwrap_err().to_string(), "Only you can confirm personal rule changes. Run this from your own terminal outside an AI session.");
        assert_eq!(require_personal_rule_confirmation(false, false, true).unwrap_err().to_string(), "Only you can confirm personal rule changes: run this in a terminal and type yes yourself. There is no flag or piped approval.");
        assert!(require_personal_rule_confirmation(true, true, true).is_err());
        assert!(require_personal_rule_confirmation(false, false, true).is_err());
        assert!(require_personal_rule_confirmation(false, true, false).is_err());
        assert!(require_personal_rule_confirmation(false, true, true).is_ok());

        let mut output = Vec::new();
        assert!(!person_types_yes(&mut &b"y\n"[..], &mut output, "plan123").unwrap());
        assert!(person_types_yes(&mut &b"yes\n"[..], &mut output, "plan123").unwrap());
        assert!(String::from_utf8(output)
            .unwrap()
            .contains("Type yes to go on"));
    }

    #[test]
    fn personal_rules_page_move_requires_an_explicit_yes_or_no() {
        let mut output = Vec::new();
        for (typed, expected) in [
            ("yes\n", Some(true)),
            ("no\n", Some(false)),
            ("n\n", None),
            ("", None),
        ] {
            assert_eq!(
                person_rule_answer(&mut typed.as_bytes(), &mut output, "fixture", true).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn rulesync_shared_edits_answer_requires_yes_or_no() {
        for (typed, expected) in [
            ("yes\n", Some(true)),
            ("NO\n", Some(false)),
            ("\n", None),
            ("y\n", None),
        ] {
            let mut output = Vec::new();
            assert_eq!(
                shared_edit_answer(&mut typed.as_bytes(), &mut output, "Design").unwrap(),
                expected
            );
            assert_eq!(String::from_utf8(output).unwrap(), "Send this edit to Design for review? Type yes to send, no to discard it (the next sync puts the approved lines back), or press Enter to decide later: ");
        }
    }

    #[test]
    fn rulesync_shared_edits_refuses_ai_or_pipes_before_request() {
        assert_eq!(require_shared_edit_confirmation(true, true, true).unwrap_err().to_string(), "Only you can confirm team or project rule edits. Run this from your own terminal outside an AI session.");
        assert_eq!(require_shared_edit_confirmation(false, false, true).unwrap_err().to_string(), "Only you can confirm team or project rule edits: run this in a terminal and type yes yourself. There is no flag or piped approval.");
        for (ai, stdin, stdout) in [
            (true, true, true),
            (false, false, true),
            (false, true, false),
        ] {
            let mut requested = false;
            assert!(after_shared_edit_gate(ai, stdin, stdout, || {
                requested = true;
                Ok(())
            })
            .is_err());
            assert!(!requested);
        }
    }

    #[test]
    fn rulesync_shared_edits_continues_after_stale_revision() {
        let decisions = [Err(anyhow::anyhow!(SHARED_EDIT_REBASE_MESSAGE)), Ok(())];
        let mut completed = 0;
        for decision in decisions {
            if shared_edit_decision_or_continue(decision)
                .unwrap()
                .is_none()
            {
                continue;
            }
            completed += 1;
        }
        assert_eq!(completed, 1);
        assert!(
            shared_edit_decision_or_continue::<()>(Err(anyhow::anyhow!("different failure")))
                .is_err()
        );
    }

    #[test]
    fn ux_cli_8_setup_share_asks_separately_for_keys_and_accepts_no() {
        let plan = SharingPlan {
            tools: vec![SharingTool {
                tool: "Codex".into(),
                skills: vec!["example".into()],
                ..Default::default()
            }],
            key_servers: vec!["example server".into()],
            ..Default::default()
        };
        let ask = |answers: &[u8]| {
            let mut output = Vec::new();
            let result = share_confirmation(&plan, &mut &answers[..], &mut output).unwrap();
            (result, String::from_utf8(output).unwrap())
        };
        assert_eq!(ask(b"n\n").0, None);
        assert_eq!(ask(b"").0, None);
        assert_eq!(ask(b"y\nn\n").0, Some(false));
        assert_eq!(ask(b"Y\nYeS\n").0, Some(true));
        assert_eq!(ask(b"y\n").0, Some(false));
        let (answer, output) = ask(b"y\ny\n");
        assert_eq!(answer, Some(true));
        assert_eq!(output.matches("Type y to go on:").count(), 2);

        let keys_only = SharingPlan {
            tools: Vec::new(),
            ..plan
        };
        let mut output = Vec::new();
        assert_eq!(
            share_confirmation(&keys_only, &mut &b"y\n"[..], &mut output).unwrap(),
            Some(false)
        );
        assert_eq!(
            String::from_utf8(output)
                .unwrap()
                .matches("Type y to go on:")
                .count(),
            2
        );
    }

    #[test]
    fn ux_cli_8_setup_share_previews_numbered_rules_and_uses_one_yes() {
        let plan = SharingPlan {
            plan_id: "rule-plan".into(),
            tools: vec![SharingTool {
                tool: "Codex".into(),
                rules: 1,
                ..Default::default()
            }],
            rules: vec![
                SharingRule {
                    number: 1,
                    line: "Run the release checklist.".into(),
                    tools: vec!["Claude Code".into()],
                    kept: false,
                },
                SharingRule {
                    number: 2,
                    line: "Review the logs.".into(),
                    tools: vec!["Claude Code".into()],
                    kept: true,
                },
            ],
            rule_stays: vec![
                SharingRuleStay {
                    tool: "Claude Code".into(),
                    line: Some("@./local.md".into()),
                    line_number: None,
                    reason: "Only Claude Code reads this import.".into(),
                },
                SharingRuleStay {
                    tool: "Claude Code".into(),
                    line: Some("Use CLAUDE.md here.".into()),
                    line_number: None,
                    reason: "This line names a tool-specific file.".into(),
                },
                SharingRuleStay {
                    tool: "Claude Code".into(),
                    line: None,
                    line_number: Some(12),
                    reason: "This line may hold a credential.".into(),
                },
            ],
            rule_notes: vec!["Remove the possible credential to review the other rules.".into()],
            ..Default::default()
        };
        let short = sharing_for_a_person(&plan, false, false);
        for details in [false, true] {
            let preview = sharing_for_a_person(&plan, details, false);
            assert!(preview.contains(&plan.rule_notes[0]), "{preview}");
        }
        assert!(short.contains("Codex: 1 rule"), "{short}");
        assert!(
            short.contains(
                "3 rule lines stay in their own tools (2 tool-only, 1 with possible credentials)."
            ),
            "{short}"
        );
        assert!(short.contains("1. Run the release checklist."), "{short}");
        assert!(!short.contains("Review the logs."), "{short}");
        let details = sharing_for_a_person(&plan, true, true);
        assert!(
            details.contains("1. Run the release checklist."),
            "{details}"
        );
        assert!(
            details.contains("2. Review the logs. [Claude Code, keep here]"),
            "{details}"
        );
        assert!(
            details.contains("Stays in Claude Code (line 12): a line that may hold a credential."),
            "{details}"
        );
        assert!(
            details.contains("wenlan rules share --confirm rule-plan --keep-rule 2"),
            "{details}"
        );
        assert!(wrap_setup_text(&details)
            .unwrap()
            .lines()
            .all(|line| line.len() < 80));

        let mut output = Vec::new();
        assert_eq!(
            share_confirmation(&plan, &mut &b"y\n"[..], &mut output).unwrap(),
            Some(false)
        );
        assert_eq!(
            String::from_utf8(output)
                .unwrap()
                .matches("Type y to go on:")
                .count(),
            1
        );
    }

    #[test]
    fn ux_cli_8_setup_report_counts_and_preview_wrap_without_splitting_words() {
        let sync = SetupSync {
            tools: [
                "Claude Code",
                "Codex",
                "Gemini CLI",
                "GitHub Copilot CLI",
                "Cursor",
                "OpenCode",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            rules: Some(3),
            skills: 1,
            following: 0,
            copied: 1,
            left_alone: 1,
        };
        let shown = wrap_setup_text(&setup_sync_line(&sync)).unwrap();
        assert!(shown.contains("3 rules, 1 skill."), "{shown}");
        assert!(shown.contains("1 item was left as it was."), "{shown}");
        assert!(
            shown
                .lines()
                .all(|line| console::measure_text_width(line) < 80),
            "{shown}"
        );
        assert!(!shown.contains("follow their source"), "{shown}");
        assert!(wrap_setup_text(&"x".repeat(80)).is_err());
    }

    #[test]
    fn ux_skill_26_rules_share_short_preview_summarizes_distinct_stays() {
        let plan = SharingPlan {
            plan_id: "plan-id".into(),
            tools: vec![SharingTool {
                tool: "Codex".into(),
                skills: vec!["one".into(), "two".into()],
                commands: vec!["hello".into()],
                ..Default::default()
            }],
            stays: vec![
                SharingStay { tool: "Claude Code".into(), item: "subagent review".into(), reason: "The tools use different subagent formats. Not copied to Codex.".into() },
                SharingStay { tool: "Claude Code".into(), item: "subagent review".into(), reason: "The subagent's frontmatter is specific to its tool. Not copied to Cursor.".into() },
                SharingStay { tool: "Claude Code".into(), item: "command args".into(), reason: "'$' can introduce argument placeholders. The command stays in its originating tool. Not copied to Codex.".into() },
                SharingStay { tool: "Codex".into(), item: "MCP server docs".into(), reason: ".claude.json: MCP server docs conflicts with a separate entry.".into() },
            ],
            key_servers: vec!["billing".into()],
            key_tools: vec!["Codex".into()],
            ..Default::default()
        };
        let text = sharing_for_a_person(&plan, false, false);
        assert!(text.contains("Codex: 2 skills, 1 command"), "{text}");
        assert!(!text.contains("  + skill one"), "{text}");
        assert!(text.contains("3 items stay in their own tool"), "{text}");
        assert!(
            text.contains("1 uses a format other tools can't read"),
            "{text}"
        );
        assert!(text.contains("1 uses $ placeholders"), "{text}");
        assert!(
            text.contains("1 has a conflicting item in another tool"),
            "{text}"
        );
        assert!(text.contains("  ? MCP server billing"), "{text}");
        assert!(text.contains("Add --details to see every item."), "{text}");
        assert!(text.contains("Plan id: plan-id"), "{text}");
        let details = sharing_for_a_person(&plan, true, false);
        assert!(details.contains("  + skill one"), "{details}");
        assert_eq!(
            details
                .matches("Stays in Claude Code: subagent review:")
                .count(),
            1,
            "{details}"
        );
        assert!(details.contains("not copied to Codex, Cursor"), "{details}");
        assert!(!details.contains("Add --details"), "{details}");
    }

    #[test]
    fn rules_status_uses_singular_and_plural_counts() {
        use wenlan_types::rulesync::{RulesSetupTool, RulesToolCounts, RulesToolStatus};
        let status = RulesSetupStatus {
            enabled: true,
            tools: vec![
                RulesToolStatus {
                    tool: RulesSetupTool {
                        id: "one".into(),
                        name: "One".into(),
                    },
                    state: RulesToolState::UpToDate,
                    counts: RulesToolCounts {
                        rules: 1,
                        skills: 1,
                        commands: 1,
                        subagents: 1,
                        mcp: 1,
                    },
                },
                RulesToolStatus {
                    tool: RulesSetupTool {
                        id: "two".into(),
                        name: "Two".into(),
                    },
                    state: RulesToolState::UpToDate,
                    counts: RulesToolCounts {
                        rules: 2,
                        skills: 2,
                        commands: 2,
                        subagents: 2,
                        mcp: 2,
                    },
                },
            ],
            ..Default::default()
        };
        let output = status_for_a_person(&status);
        assert!(
            output.contains("1 rule, 1 skill, 1 command, 1 subagent, 1 MCP server"),
            "{output}"
        );
        assert!(
            output.contains("2 rules, 2 skills, 2 commands, 2 subagents, 2 MCP servers"),
            "{output}"
        );
    }

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
            let (before, _) = output.split_once("Type y to go on:").unwrap();
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
            notice_codes: Vec::new(),
            proposals: Vec::new(),
            message: "Checked the local tool setup".into(),
            kept_in_place: Vec::new(),
        };
        assert!(report_for_a_person(&report, false, &[])
            .contains("Blocked .codex/AGENTS.md: Review your changes before syncing.\n"));
    }

    #[test]
    fn rulesync_report_summarizes_kept_in_place_entries_once_when_enabled() {
        let report = RulesSyncReport {
            written: Vec::new(),
            blocked: Vec::new(),
            blocked_codes: Vec::new(),
            notice_codes: Vec::new(),
            proposals: Vec::new(),
            message: "Synced settings".into(),
            kept_in_place: vec![
                wenlan_types::rulesync::KeptInPlace {
                    tool: "Claude Code".into(),
                    count: 2,
                },
                wenlan_types::rulesync::KeptInPlace {
                    tool: "Codex CLI".into(),
                    count: 3,
                },
            ],
        };
        let summary = "Kept 5 settings in place in their own tools because they may hold keys or cannot be copied.\n";
        assert_eq!(
            report_for_a_person(&report, true, &[])
                .matches(summary)
                .count(),
            1
        );
        assert!(!report_for_a_person(&report, false, &[]).contains("Kept "));
    }

    #[test]
    fn personal_preview_lists_tools_that_get_new_files() {
        let mut plan = AdoptionPlan {
            plan_id: String::new(),
            token: "t".into(),
            rule_lines: None,
            settings: None,
            files: Vec::new(),
            rule_sources: Vec::new(),
            skills: Vec::new(),
            notes: Vec::new(),
            tools: Vec::new(),
            skipped_files: Vec::new(),
            skipped_tools: Vec::new(),
            skipped_tool_codes: Vec::new(),
            note_codes: Vec::new(),
            new_files: Vec::new(),
            kept_in_place: Vec::new(),
        };
        assert!(!plan_for_a_person(&plan).contains("Will also write to"));
        assert!(!plan_for_a_person(&plan).contains("Kept "));
        plan.new_files = vec![
            wenlan_types::rulesync::AdoptionNewFiles {
                tool: "Cursor".into(),
                files: 1,
            },
            wenlan_types::rulesync::AdoptionNewFiles {
                tool: "Gemini CLI".into(),
                files: 2,
            },
        ];
        assert!(plan_for_a_person(&plan)
            .contains("Will also write to: Cursor, Gemini CLI (3 files).\n"));
        plan.kept_in_place = vec![wenlan_types::rulesync::KeptInPlace {
            tool: "Claude Code".into(),
            count: 4,
        }];
        assert_eq!(
            plan_for_a_person(&plan)
                .matches(
                    "Kept 4 settings in place in their own tools because they may hold keys or cannot be copied.\n"
                )
                .count(),
            1
        );
    }

    #[test]
    fn personal_preview_neutralizes_terminal_controls() {
        let plan = AdoptionPlan {
            plan_id: "test".into(),
            token: "test".into(),
            rule_lines: None,
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
            rule_sources: Vec::new(),
            skills: Vec::new(),
            notes: Vec::new(),
            tools: Vec::new(),
            skipped_files: Vec::new(),
            skipped_tools: Vec::new(),
            skipped_tool_codes: Vec::new(),
            note_codes: Vec::new(),
            new_files: Vec::new(),
            kept_in_place: Vec::new(),
        };
        let text = plan_for_a_person(&plan);
        assert!(text.contains("tool<U+000A>settings.md (+2 / -1)"));
        assert!(text.contains("-old\n+<U+001B>[2J\n+🐈<U+202E><U+007F>\n"));

        let json = terminal_json(&plan).unwrap();
        assert_eq!(json, printable(&json));
        assert_eq!(serde_json::from_str::<AdoptionPlan>(&json).unwrap(), plan);
    }
}
