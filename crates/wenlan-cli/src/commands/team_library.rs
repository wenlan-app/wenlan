// SPDX-License-Identifier: Apache-2.0
//! Terminal review of an explicit team publication.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, ensure, Context, Result};
use serde::Serialize;
use wenlan_types::team::TeamAccess;
use wenlan_types::team_library::{
    TeamAuthorship, TeamItemKind, TeamOriginal, TeamPublication, TeamTerminalAction,
    TeamTerminalReply, TeamText,
};

use crate::client::WenlanClient;

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum OriginalKind {
    Page,
    Memory,
}

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum LibraryAccess {
    Read,
    Write,
    None,
}

impl LibraryAccess {
    pub(super) fn grant(self) -> Option<TeamAccess> {
        match self {
            Self::Read => Some(TeamAccess::Read),
            Self::Write => Some(TeamAccess::Write),
            Self::None => None,
        }
    }
}

#[derive(clap::Args)]
pub struct PublishArgs {
    #[arg(value_enum)]
    kind: OriginalKind,
    /// ID in this computer's personal library.
    id: String,
    /// Destination Space ID. Omit to publish directly to the team.
    #[arg(long)]
    space_id: Option<String>,
    /// Publish the page as the Brief for this destination.
    #[arg(long)]
    brief: bool,
    /// A local JSON file containing edited title and body.
    #[arg(long)]
    text: Option<PathBuf>,
    /// A local JSON object mapping personal source IDs to edited title and body.
    /// Sources still require selection during review.
    #[arg(long)]
    redactions: Option<PathBuf>,
}

#[derive(Serialize)]
struct Original {
    original: TeamOriginal,
    text: TeamText,
}

#[derive(Serialize)]
struct Draft {
    item: Original,
    sources: Vec<Original>,
    omitted_sources: bool,
    authorship: Option<TeamAuthorship>,
}

fn print_json(value: &impl Serialize) -> Result<()> {
    println!("{}", crate::commands::rulesync::terminal_json(value)?);
    Ok(())
}

fn safe_id(id: &str) -> Result<()> {
    ensure!(
        wenlan_core::shared_space::version::is_safe_id(id),
        "Use a Wenlan item ID."
    );
    Ok(())
}

async fn prepare(client: &WenlanClient, kind: OriginalKind, id: &str) -> Result<Draft> {
    ensure!(
        client.is_local(),
        "Prepare the draft against your personal Wenlan on this computer."
    );
    safe_id(id)?;
    match kind {
        OriginalKind::Memory => {
            let item = client
                .get_memory_detail(id)
                .await?
                .memory
                .and_then(personal_memory)
                .ok_or_else(|| anyhow!("Personal memory not found."))?;
            Ok(Draft {
                item,
                sources: Vec::new(),
                omitted_sources: false,
                authorship: None,
            })
        }
        OriginalKind::Page => {
            let page = client.read_page(id).await?.page.page;
            ensure!(page.status == "active", "The personal page must be active.");
            let mut source_ids: BTreeSet<String> = page.source_memory_ids.iter().cloned().collect();
            let mut omitted_sources = false;
            for citation in page
                .citations
                .iter()
                .filter(|citation| !citation.historical)
            {
                if citation.source_kind == "memory" {
                    let locator = citation
                        .locator
                        .strip_prefix("wenlan:")
                        .unwrap_or(&citation.locator);
                    let id = locator.split_once('@').map_or(locator, |(id, _)| id);
                    source_ids.insert(id.into());
                } else {
                    omitted_sources = true;
                }
            }
            ensure!(
                source_ids.len() <= 32,
                "This page has more than 32 personal sources. Review a shorter draft."
            );
            let mut sources = Vec::new();
            for id in source_ids {
                if safe_id(&id).is_ok() {
                    let found = client
                        .get_memory_detail(&id)
                        .await?
                        .memory
                        .and_then(personal_memory);
                    if let Some(source) = found {
                        sources.push(source);
                        continue;
                    }
                }
                omitted_sources = true;
            }
            let authorship = match page.creation_kind.as_str() {
                "authored" if page.user_edited => Some(TeamAuthorship::Human),
                "distilled" => Some(TeamAuthorship::Ai),
                _ => None,
            };
            Ok(Draft {
                item: Original {
                    original: TeamOriginal {
                        kind: TeamItemKind::Page,
                        id: page.id,
                        revision: page.version,
                    },
                    text: TeamText {
                        title: page.title,
                        body: wenlan_core::team_library::draft_page_body(&page.content),
                    },
                },
                sources,
                omitted_sources,
                authorship,
            })
        }
    }
}

fn personal_memory(memory: wenlan_types::memory::MemoryItem) -> Option<Original> {
    (memory.supersede_mode != "evicted" && !memory.pending_revision).then_some(Original {
        original: TeamOriginal {
            kind: TeamItemKind::Memory,
            id: memory.source_id,
            revision: memory.version,
        },
        text: TeamText {
            title: memory.title,
            body: memory.content,
        },
    })
}

pub(super) async fn draft(client: &WenlanClient, kind: OriginalKind, id: &str) -> Result<()> {
    print_json(&prepare(client, kind, id).await?)
}

fn reviewable(client: &WenlanClient) -> Result<()> {
    ensure!(
        client.is_local(),
        "Use the team host's terminal or sign in to the team publication API for a joined team."
    );
    ensure!(
        !client.names_an_agent() && client.session().is_none(),
        "A person must review this from their own terminal."
    );
    ensure!(
        io::stdin().is_terminal() && io::stdout().is_terminal(),
        "Use an interactive terminal to review and confirm this change."
    );
    Ok(())
}

fn answer(question: &str) -> Result<String> {
    print!("{question} ");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    Ok(answer.trim().to_owned())
}

fn read_edits<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    serde_json::from_slice(&std::fs::read(path)?).context("Read the local review JSON")
}

pub(super) async fn publish(client: &WenlanClient, args: PublishArgs) -> Result<()> {
    reviewable(client)?;
    let mut candidate = prepare(client, args.kind, &args.id).await?;
    let kind = candidate.item.original.kind;
    ensure!(
        !args.brief || kind == TeamItemKind::Page,
        "Only a page can be a Brief."
    );
    if let Some(path) = args.text {
        candidate.item.text = read_edits(&path)?;
    }
    let mut edits: BTreeMap<String, TeamText> = args
        .redactions
        .as_deref()
        .map(read_edits)
        .transpose()?
        .unwrap_or_default();
    for source in &mut candidate.sources {
        if let Some(edit) = edits.remove(&source.original.id) {
            source.text = edit;
        }
    }
    ensure!(
        edits.is_empty(),
        "The redactions file names an unavailable personal source."
    );
    let mut selected = BTreeSet::new();
    if !candidate.sources.is_empty() {
        println!("Personal sources for review");
        for (index, source) in candidate.sources.iter().enumerate() {
            print!("[ ] {}. ", index + 1);
            print_json(&source)?;
        }
        let selections = answer("Also make any cited memories available to this team? Enter numbers separated by commas, or press Enter to skip.")?;
        for selection in selections
            .split(',')
            .map(str::trim)
            .filter(|text| !text.is_empty())
        {
            let index = selection
                .parse::<usize>()
                .context("Use the numbers shown next to the sources")?;
            ensure!(
                (1..=candidate.sources.len()).contains(&index),
                "No source numbered {index}."
            );
            selected.insert(index - 1);
        }
    }
    let private_source = candidate.omitted_sources || selected.len() < candidate.sources.len();
    let mut originals = vec![candidate.item.original];
    let mut memories = Vec::new();
    for (index, source) in candidate.sources.into_iter().enumerate() {
        if selected.contains(&index) {
            originals.push(source.original);
            memories.push(source.text);
        }
    }
    let draft = TeamPublication {
        kind,
        title: candidate.item.text.title,
        body: candidate.item.text.body,
        authorship: candidate.authorship,
        space_id: args.space_id,
        item_id: None,
        expected_revision: None,
        memories,
        private_source,
        brief: args.brief,
    };
    let reply = client
        .team_library_terminal(&TeamTerminalAction::Preview { draft, originals })
        .await?;
    let TeamTerminalReply::Preview { preview } = reply else {
        bail!("The host did not return a publication preview.");
    };
    print_json(&preview)?;
    if answer("Publish the reviewed text to this audience? Type yes to confirm.")? != "yes" {
        println!("Cancelled.");
        return Ok(());
    }
    let reply = client
        .team_library_terminal(&TeamTerminalAction::Publish {
            preview_id: preview.id,
        })
        .await?;
    print_json(&reply)
}

pub(super) async fn change(client: &WenlanClient, action: TeamTerminalAction) -> Result<()> {
    reviewable(client)?;
    if matches!(&action, TeamTerminalAction::Withdraw { .. })
        && answer("Withdraw this team item and its revisions? Type yes to confirm.")? != "yes"
    {
        println!("Cancelled.");
        return Ok(());
    }
    print_json(&client.team_library_terminal(&action).await?)
}
