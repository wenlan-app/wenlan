// SPDX-License-Identifier: Apache-2.0
//! `wenlan team rules` — the team host's person reviews the Rules pages
//! this computer keeps for its team: the team-wide page (`everyone`) and
//! each Space's page. Only a person at the host's own terminal; no AI tool.

use std::io::{self, IsTerminal};
use std::path::PathBuf;

use anyhow::{ensure, Result};
use clap::Subcommand;
use wenlan_core::skill_registry::printable;
use wenlan_types::team::TeamRulesHostProposal;

use crate::client::WenlanClient;
use crate::commands::skills::{one_line, person_says_yes, NEEDS_A_TERMINAL};
use crate::output::{print_json, ResolvedFormat};

#[derive(Subcommand)]
pub enum TeamRulesCmd {
    /// List the changes to the team's Rules waiting for review.
    Proposals {
        #[command(subcommand)]
        command: Option<TeamRulesProposalsCmd>,
    },
    /// Propose new text for a Rules page this team host keeps.
    Propose {
        /// The Space whose Rules change, or `everyone` for the team-wide Rules.
        #[arg(long, value_name = "NAME")]
        space: String,
        /// A Markdown file with the whole new Rules text.
        file: PathBuf,
    },
}

#[derive(Subcommand)]
pub enum TeamRulesProposalsCmd {
    /// Show one change, then accept or reject it after you type yes.
    #[command(group(
        clap::ArgGroup::new("answer").required(true).args(["accept", "reject"])
    ))]
    Decide {
        /// The change's ID, from `wenlan team rules proposals`.
        id: String,
        /// Make this text the Space's Rules.
        #[arg(long)]
        accept: bool,
        /// Keep the current Rules and record the refusal.
        #[arg(long)]
        reject: bool,
    },
}

impl TeamRulesCmd {
    /// `propose --space` is this command's own argument, not the global
    /// Space selection.
    pub fn names_a_space(&self) -> bool {
        matches!(self, Self::Propose { .. })
    }

    /// Preflight before the CLI contacts the daemon.
    pub fn require_person(&self, is_ai_caller: bool) -> Result<()> {
        ensure!(
            !is_ai_caller,
            "Only you can review the team's Rules. Run this from your own terminal outside an AI session."
        );
        ensure!(
            !matches!(
                self,
                Self::Proposals {
                    command: Some(TeamRulesProposalsCmd::Decide { .. })
                }
            ) || (io::stdin().is_terminal() && io::stdout().is_terminal()),
            "{NEEDS_A_TERMINAL}"
        );
        Ok(())
    }
}

pub async fn run(
    client: &WenlanClient,
    format: ResolvedFormat,
    quiet: bool,
    command: TeamRulesCmd,
) -> Result<()> {
    match command {
        TeamRulesCmd::Propose { space, file } => {
            let markdown = std::fs::read_to_string(file)?;
            let id = client.team_rules_propose(&space, markdown).await?;
            if !quiet {
                match format {
                    ResolvedFormat::Json => print_json(&id)?,
                    ResolvedFormat::Table => println!(
                        "The change {} to the Rules of {} is waiting for review: wenlan team rules proposals",
                        one_line(&id),
                        one_line(&space)
                    ),
                }
            }
            Ok(())
        }
        TeamRulesCmd::Proposals { command: None } => {
            let pending = client.team_rules_proposals().await?;
            if !quiet {
                match format {
                    ResolvedFormat::Json => print_json(&pending)?,
                    ResolvedFormat::Table if pending.is_empty() => {
                        println!("No changes to the team's Rules are waiting for review.")
                    }
                    ResolvedFormat::Table => {
                        for proposal in &pending {
                            print!("{}", describe(proposal));
                        }
                    }
                }
            }
            Ok(())
        }
        TeamRulesCmd::Proposals {
            command: Some(TeamRulesProposalsCmd::Decide { id, accept, .. }),
        } => {
            let proposal = client
                .team_rules_proposals()
                .await?
                .into_iter()
                .find(|proposal| proposal.proposal.id == id)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "No change to the team's Rules with that ID is waiting for review."
                    )
                })?;
            print!("{}", describe(&proposal));
            let question = if accept {
                "Make this text the Rules?"
            } else {
                "Reject this change and keep the current Rules?"
            };
            if !person_says_yes(&mut io::stdin().lock(), &mut io::stdout(), question)? {
                println!("Nothing changed.");
                return Ok(());
            }
            let decision = client.team_rules_decide(&id, accept).await?;
            match format {
                ResolvedFormat::Json => print_json(&decision)?,
                ResolvedFormat::Table => {
                    if let Some(note) = &decision.note {
                        println!("{}", one_line(note));
                    } else if decision.accepted {
                        println!(
                            "Accepted. These are now the Rules of {}.",
                            one_line(&decision.space)
                        );
                    } else {
                        println!(
                            "Rejected. The Rules of {} did not change.",
                            one_line(&decision.space)
                        );
                    }
                }
            }
            Ok(())
        }
    }
}

/// The Space, who proposed it and when, then the whole text.
fn describe(proposal: &TeamRulesHostProposal) -> String {
    let mut out = format!(
        "{}  {}  proposed by {}  {}\n",
        one_line(&proposal.proposal.id),
        one_line(&proposal.space),
        proposal
            .author_name
            .as_deref()
            .map_or_else(|| "unknown".to_string(), one_line),
        one_line(&proposal.proposal.proposed_at),
    );
    for line in printable(&proposal.proposal.markdown).lines() {
        out.push_str("    ");
        out.push_str(line);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use wenlan_types::team::TeamRulesProposal;

    #[test]
    fn a_proposal_without_a_recorded_person_shows_unknown() {
        let mut proposal = TeamRulesHostProposal {
            space: "Roadmap".into(),
            author_name: None,
            proposal: TeamRulesProposal {
                id: "abc".into(),
                author: "member-1".into(),
                markdown: "Ship behind a flag.\nWrite the test first.".into(),
                base: "base".into(),
                proposed_at: "2026-11-27T00:00:00Z".into(),
            },
        };
        assert_eq!(
            describe(&proposal),
            "abc  Roadmap  proposed by unknown  2026-11-27T00:00:00Z\n    Ship behind a flag.\n    Write the test first.\n"
        );
        proposal.author_name = Some("Amy".into());
        assert!(describe(&proposal).contains("proposed by Amy"));
    }
}
