// SPDX-License-Identifier: Apache-2.0
//! Preview and manage the personal tool setup on this computer.

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
    RulesSetupCode, RulesSetupStatus, RulesSyncReport, RulesToolState, SharingPlan,
    SkillUpdateStatus, SkillUpstream, TO_REVIEW_COMMAND,
};
use wenlan_types::rulesync_settings::{SettingsApplyRequest, SettingsPlan, SettingsPreviewRequest};

use super::skills::{
    one_line, person_says_yes, review_saved_skill, skill_proposal_summary, SkillReviewChoice,
};
use crate::{client::WenlanClient, output::ResolvedFormat};
use wenlan_types::rulesync_skill::{SavedSkillScope, SkillProposalOrigin};

#[derive(Debug, Subcommand)]
pub enum RulesSetupCmd {
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
    /// Copy your existing skills, commands, subagents and MCP servers to
    /// every managed tool that supports them.
    Share {
        /// Preview what each tool would get.
        #[arg(long)]
        dry_run: bool,
        /// Confirm the plan shown to the person.
        #[arg(long, conflicts_with = "dry_run")]
        confirm: Option<String>,
        /// Also offer the MCP servers that may contain a key. You are still
        /// asked to type yes; an AI tool cannot share them.
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
    if matches!(command, RulesSetupCmd::Share { dry_run: true, .. }) {
        let client =
            WenlanClient::for_local_rules_with_identity(agent_name, session.session.as_ref())?;
        let plan = client.preview_share_as_person(&home_request()?).await?;
        return show(format, quiet, &plan, sharing_for_a_person);
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
            confirm: confirmed, ..
        } => {
            let expected = home_request()?;
            let plan = client.preview_share_as_person(&expected).await?;
            let mut include_keys = false;
            if let Some(id) = confirmed {
                ensure!(plan.plan_id == *id && !id.is_empty(), "The setup changed since that preview. Run `wenlan rules share --dry-run` again and show the person the new list.");
            } else {
                // The person must see the same plan even with --quiet or --format json.
                print!("{}", sharing_for_a_person(&plan));
                if !plan.tools.is_empty()
                    && !confirm("Copy these items to the tools listed above?")?
                {
                    return Ok(());
                }
                // Servers that may contain a key need their own typed yes.
                if !plan.key_servers.is_empty() {
                    let count = plan.key_servers.len();
                    include_keys = confirm(&format!(
                        "Also copy {} that may contain a key into {}?",
                        if count == 1 {
                            "this server".to_owned()
                        } else {
                            format!("these {count} servers")
                        },
                        plan.key_tools
                            .iter()
                            .map(|tool| one_line(tool))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))?;
                }
                if plan.tools.is_empty() && !include_keys {
                    return Ok(());
                }
            }
            client
                .share_rules_as_person(&AdoptRequest {
                    token: plan.token,
                    confirmed_plan_id: confirmed.clone(),
                    home: Some(expected.home),
                    include_keys,
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
                let sync_report = client.sync_rules_as_person().await?;
                if !quiet {
                    eprint!("{}", report_for_a_person(&sync_report, false));
                }
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
        report_for_a_person(report, show_kept_in_place)
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
    };
    show(format, quiet, &report, |report| {
        report_for_a_person(report, false)
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
    rules: usize,
    skills: usize,
    following: usize,
    copied: usize,
    left_alone: usize,
}

fn home_request() -> Result<AdoptPreviewRequest> {
    Ok(AdoptPreviewRequest {
        home: rulesync_personal::system_home()?.canonicalize()?,
    })
}

fn sharing_for_a_person(plan: &SharingPlan) -> String {
    let mut out = String::new();
    if plan.tools.is_empty() {
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
        let counts = groups
            .iter()
            .filter(|(_, names)| !names.is_empty())
            .map(|(word, names)| plural(names.len(), word))
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(&mut out, "\n{}: {counts}", one_line(&tool.tool)).unwrap();
        for (word, names) in groups {
            for name in names {
                writeln!(&mut out, "  + {word} {}", one_line(name)).unwrap();
            }
        }
    }
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
        for stay in &plan.stays {
            writeln!(
                &mut out,
                "Stays in {}: {}: {}",
                one_line(&stay.tool),
                one_line(&stay.item),
                one_line(&stay.reason)
            )
            .unwrap();
        }
    }
    if !plan.tools.is_empty() && !plan.plan_id.is_empty() {
        writeln!(
            &mut out,
            "\nPlan id: {} — to go ahead from an AI tool: wenlan rules share --confirm {}",
            plan.plan_id, plan.plan_id
        )
        .unwrap();
    }
    out
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
    let mut line = format!(
        "Synced {tools}: {}, {} ({} follow their source, {} copied).",
        plural(sync.rules, "rule"),
        plural(sync.skills, "skill"),
        sync.following,
        sync.copied
    );
    if sync.left_alone > 0 {
        line.push_str(&format!(" {} left as they are.", sync.left_alone));
    }
    line
}

/// Running setup is the person's consent: turn tool sync on, bring the setup
/// in and sync once, with no question and no terminal needed. A plan that
/// cannot be adopted safely is left alone with one line saying so; setup
/// itself still succeeds.
pub(super) async fn adopt_during_setup(format: ResolvedFormat) {
    if let Err(error) = try_adopt_during_setup(format).await {
        println!(
            "Your tool setup was left as it is: {}. See `wenlan rules adopt --dry-run`.",
            one_line(&printable(&format!("{error:#}"))).trim_end_matches('.')
        );
    }
}

async fn try_adopt_during_setup(format: ResolvedFormat) -> Result<()> {
    rulesync_personal::set_enabled(true)?;
    // An AI tool running setup says who it is; the daemon then adopts only
    // the plan it just previewed (the same check as `rules adopt --confirm`).
    let session = wenlan_types::session_context::resolve_session_from_env(None);
    let client = WenlanClient::for_local_rules_with_identity(None, session.session.as_ref())?;
    if client.rules_setup_status().await?.adopted {
        return Ok(());
    }
    let home = rulesync_personal::system_home()?.canonicalize()?;
    let plan = client
        .preview_adoption_as_person(&AdoptPreviewRequest { home: home.clone() })
        .await?;
    if plan.files.is_empty() && plan.settings.is_none() && plan.skills.is_empty() {
        return Ok(());
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
    let _ = client.sync_rules_as_person().await;
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
            .filter(|tool| tool.rule_files + tool.skills + tool.other_files > 0)
            .filter(|tool| {
                managed
                    .as_ref()
                    .is_none_or(|managed| managed.contains(&tool.name))
            })
            .map(|tool| tool.name.clone())
            .collect(),
        rules: plan.tools.iter().map(|tool| tool.rule_files).sum(),
        skills: following + copied,
        following,
        copied,
        left_alone: count(|outcome| matches!(outcome, AdoptionSkillOutcome::Skipped { .. })),
    };
    match format {
        ResolvedFormat::Json => println!("{}", terminal_json(&sync)?),
        ResolvedFormat::Table => {
            println!("{}", printable(&setup_sync_line(&sync)));
            println!("To undo: wenlan rules restore");
        }
    }
    Ok(())
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

fn report_for_a_person(report: &RulesSyncReport, show_kept_in_place: bool) -> String {
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
        writeln!(
            out,
            "Kept {count} settings in place in their own tools because they may hold keys or cannot be copied."
        )
        .unwrap();
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
            kept_in_place: Vec::new(),
        };
        assert!(report_for_a_person(&report, false)
            .contains("Blocked .codex/AGENTS.md: Review your changes before syncing.\n"));
    }

    #[test]
    fn rulesync_report_summarizes_kept_in_place_entries_once_when_enabled() {
        let report = RulesSyncReport {
            written: Vec::new(),
            blocked: Vec::new(),
            blocked_codes: Vec::new(),
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
            report_for_a_person(&report, true).matches(summary).count(),
            1
        );
        assert!(!report_for_a_person(&report, false).contains("Kept "));
    }

    #[test]
    fn personal_preview_lists_tools_that_get_new_files() {
        let mut plan = AdoptionPlan {
            plan_id: String::new(),
            token: "t".into(),
            settings: None,
            files: Vec::new(),
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
