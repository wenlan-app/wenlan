// SPDX-License-Identifier: Apache-2.0
//! Automatic mode's share of new pages: Wenlan's own page writer turns
//! material no page cites yet into a note, the way an on-demand caller's AI
//! tool does through steep.
//!
//! One turn writes at most one note, through the on-demand path's own steps:
//! [`new_material`] lists the waiting items, the writer groups them by topic,
//! [`claim_new_on`] holds one group (the two-source / one-long-source floor,
//! folder documents included), and the writer's text lands through
//! [`commit_new`] (the citation check) or [`attach_new`] when a page on that
//! subject exists. Only the model differs. A turn makes at most two model
//! calls: one to group, one to write.
//!
//! A group that does not become a note (too little to say, a citation
//! refusal, a model error) is remembered by its items and their last change,
//! in app metadata, so the next turn moves on; an item that changes is tried
//! again. A note that lands needs no record: its items are cited and leave
//! the list.

use std::sync::Arc;
use std::time::Duration;

use super::{
    attach_new, claim_new_on, commit_new, new_material, release_new, NewMaterial, NewPageClaim,
    MAX_CLAIM_ITEMS, MAX_PAGE_TITLES, MAX_TITLE_CHARS, MIN_SINGLE_SOURCE_CHARS,
};
use crate::db::MemoryDB;
use crate::error::WenlanError;
use crate::llm_provider::{LlmProvider, LlmRequest};
use crate::prompts::PromptRegistry;
use crate::read_scope::ReadScope;
use crate::steep::SteepError;

/// The agent name the automatic writer's notes carry, as an on-demand
/// commit without a named agent does.
const AGENT: &str = "steep";

/// Most waiting items one grouping call reads: the newest ones.
const GROUP_ITEMS: usize = 40;

/// How long one model call may take before the turn gives up on it.
const MODEL_TIMEOUT: Duration = Duration::from_secs(180);

/// Items that did not become a note, as `item_id@last_modified`.
const FAILED_ITEMS_KEY: &str = "steep_new_automatic_failed_items_v1";
/// Oldest records are dropped past this many, so the record stays small.
const FAILED_ITEMS_CAP: usize = 2_000;

const GROUP_SYSTEM: &str = "\
You sort a person's saved material into topics for wiki pages.\n\
You get a numbered list of items, each with an id, its kind, its title and the start of its text, and the titles of pages that already exist.\n\
Pick ONE group of items that clearly share one real topic and are worth one page: two to twelve items, or a single long item that stands alone.\n\
Answer with exactly two lines and nothing else:\n\
TOPIC: <a short page title for the group, in the items' language; reuse an existing page title when the group belongs to that page>\n\
ITEMS: <the ids of the group's items, separated by commas>\n\
If no items belong together, answer with the one item that has the most to say.";

/// What one automatic new-page turn did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NewPageOutcome {
    /// A note was written (or merged into one with the same title).
    Written { page_id: String },
    /// The group went into an existing page on its subject.
    Attached { page_id: String },
    /// The group did not become a note; its items wait until they change.
    Failed,
}

/// One automatic new-page turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPageTurn {
    pub outcome: NewPageOutcome,
    /// Other waiting items this run may still take.
    pub more: bool,
}

fn failed_key(material: &NewMaterial) -> String {
    format!("{}@{}", material.item.item_id, material.item.last_modified)
}

async fn load_failed(db: &MemoryDB) -> Result<Vec<String>, WenlanError> {
    Ok(db
        .get_app_metadata(FAILED_ITEMS_KEY)
        .await?
        .and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok())
        .unwrap_or_default())
}

async fn remember_failed(
    db: &MemoryDB,
    mut failed: Vec<String>,
    items: &[&NewMaterial],
) -> Result<(), WenlanError> {
    for item in items {
        let key = failed_key(item);
        if !failed.contains(&key) {
            failed.push(key);
        }
    }
    if failed.len() > FAILED_ITEMS_CAP {
        let excess = failed.len() - FAILED_ITEMS_CAP;
        failed.drain(..excess);
    }
    let raw = serde_json::to_string(&failed).unwrap_or_else(|_| "[]".to_string());
    db.set_app_metadata(FAILED_ITEMS_KEY, &raw).await
}

/// Write at most one note from waiting material with `llm`. `None` when no
/// item is waiting that this run may take (none waiting, each held by an
/// on-demand caller, or each already failed as it is).
pub async fn write_one_new_page(
    db: &MemoryDB,
    llm: &Arc<dyn LlmProvider>,
    prompts: &PromptRegistry,
    knowledge_path: Option<&std::path::Path>,
) -> Result<Option<NewPageTurn>, WenlanError> {
    let failed = load_failed(db).await?;
    let material = new_material(db, &ReadScope::Global).await?;
    let open: Vec<&NewMaterial> = material
        .iter()
        .filter(|m| !m.item.held && !failed.contains(&failed_key(m)))
        .collect();
    if open.is_empty() {
        return Ok(None);
    }
    let offered = &open[..open.len().min(GROUP_ITEMS)];
    let page_titles = recent_titles(db).await;
    let (topic, group) = match ask(
        llm,
        Some(GROUP_SYSTEM),
        &group_prompt(offered, &page_titles),
        256,
        "steep_new_group",
    )
    .await
    .and_then(|reply| parse_group(&reply, offered))
    {
        Some(group) => group,
        // A reply that names no usable group still moves the run on: the
        // newest item alone when it is long enough to stand alone, else it is
        // set aside.
        None => {
            let newest = offered[0];
            if (newest.item.chars as usize) < MIN_SINGLE_SOURCE_CHARS {
                log::info!(
                    "[steep-new-auto] no group named; setting aside '{}'",
                    newest.item.title
                );
                remember_failed(db, failed, &[newest]).await?;
                return Ok(Some(NewPageTurn {
                    outcome: NewPageOutcome::Failed,
                    more: open.len() > 1,
                }));
            }
            (newest.item.title.clone(), vec![newest])
        }
    };
    let more = open.len() > group.len();
    let ids: Vec<String> = group.iter().map(|m| m.item.item_id.clone()).collect();
    let claim = match claim_new_on(db, &ReadScope::Global, &ids, None, Some(&topic)).await {
        Ok(claim) => claim,
        Err(SteepError::Db(error)) => return Err(error),
        Err(error) => {
            log::info!("[steep-new-auto] '{topic}' not claimed: {error}");
            remember_failed(db, failed, &group).await?;
            return Ok(Some(NewPageTurn {
                outcome: NewPageOutcome::Failed,
                more,
            }));
        }
    };
    let outcome = land(db, llm, prompts, &claim, &topic, knowledge_path).await;
    // `commit_new` and `attach_new` end the claim; a turn that did neither
    // gives the items back.
    let _ = release_new(&claim.token);
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(SteepError::Db(error)) => return Err(error),
        Err(error) => {
            log::info!("[steep-new-auto] '{topic}' not saved: {error}");
            NewPageOutcome::Failed
        }
    };
    if outcome == NewPageOutcome::Failed {
        remember_failed(db, failed, &group).await?;
    }
    log::info!("[steep-new-auto] '{topic}': {outcome:?}");
    Ok(Some(NewPageTurn { outcome, more }))
}

async fn land(
    db: &MemoryDB,
    llm: &Arc<dyn LlmProvider>,
    prompts: &PromptRegistry,
    claim: &NewPageClaim,
    topic: &str,
    knowledge_path: Option<&std::path::Path>,
) -> Result<NewPageOutcome, SteepError> {
    if claim.existing_page.is_some() {
        let done = attach_new(db, &claim.token, AGENT).await?;
        return Ok(done.page_id.map_or(NewPageOutcome::Failed, |page_id| {
            NewPageOutcome::Attached { page_id }
        }));
    }
    let Some(reply) = ask(
        llm,
        Some(&prompts.distill_page),
        &write_prompt(claim, topic),
        llm.recommended_max_output(),
        "steep_new_write",
    )
    .await
    else {
        return Ok(NewPageOutcome::Failed);
    };
    let body = without_title_heading(&crate::llm_provider::strip_think_tags(&reply));
    let (body, dropped) = crate::diagrams::drop_broken_diagrams(&crate::steep::unfence(&body));
    if dropped > 0 {
        log::info!(
            "[steep-new-auto] '{topic}': dropped {dropped} diagram(s) Mermaid could not draw"
        );
    }
    let title: String = topic.chars().take(MAX_TITLE_CHARS).collect();
    let done = commit_new(db, &claim.token, &title, &body, AGENT, knowledge_path).await?;
    Ok(match (done.outcome, done.page_id) {
        ("attached", Some(page_id)) => NewPageOutcome::Attached { page_id },
        (_, Some(page_id)) => NewPageOutcome::Written { page_id },
        (_, None) => NewPageOutcome::Failed,
    })
}

/// One model call; `None` for an error, a timeout or an empty reply.
async fn ask(
    llm: &Arc<dyn LlmProvider>,
    system: Option<&str>,
    prompt: &str,
    max_tokens: u32,
    label: &str,
) -> Option<String> {
    let request = LlmRequest {
        system_prompt: system.map(str::to_string),
        user_prompt: prompt.to_string(),
        max_tokens,
        temperature: 0.1,
        label: Some(label.to_string()),
        timeout_secs: Some(MODEL_TIMEOUT.as_secs()),
    };
    match tokio::time::timeout(MODEL_TIMEOUT, llm.generate(request)).await {
        Ok(Ok(reply)) => Some(reply.trim().to_string()).filter(|reply| !reply.is_empty()),
        Ok(Err(error)) => {
            log::warn!("[steep-new-auto] {label} model call failed: {error}");
            None
        }
        Err(_) => {
            log::warn!("[steep-new-auto] {label} model call timed out");
            None
        }
    }
}

async fn recent_titles(db: &MemoryDB) -> Vec<String> {
    let mut titles = Vec::new();
    match db.recent_notes(MAX_PAGE_TITLES).await {
        Ok(notes) => {
            for (_, title, _) in notes {
                if !titles.contains(&title) {
                    titles.push(title);
                }
            }
        }
        Err(error) => log::warn!("[steep-new-auto] page titles: {error}"),
    }
    titles
}

fn group_prompt(items: &[&NewMaterial], page_titles: &[String]) -> String {
    let mut prompt = String::from("Items:\n");
    for m in items {
        prompt.push_str(&format!(
            "- id {} | {} | {} chars | {} | {}\n",
            wenlan_types::steep_short_id(&m.item.item_id),
            m.item.kind,
            m.item.chars,
            m.item.title,
            m.item.snippet
        ));
    }
    if !page_titles.is_empty() {
        prompt.push_str("\nExisting pages:\n");
        for title in page_titles {
            prompt.push_str(&format!("- {title}\n"));
        }
    }
    prompt
}

/// The group a reply names: its topic and the offered items its ids name
/// (short ids or item ids), in the order offered. `None` without a topic or
/// a known item.
fn parse_group<'a>(
    reply: &str,
    offered: &[&'a NewMaterial],
) -> Option<(String, Vec<&'a NewMaterial>)> {
    let field = |name: &str| {
        reply.lines().find_map(|line| {
            let line = line.trim().trim_start_matches(['-', '*', ' ']);
            let (head, rest) = line.split_once(':')?;
            head.trim()
                .trim_matches('*')
                .eq_ignore_ascii_case(name)
                .then(|| rest.trim().to_string())
        })
    };
    let topic = field("TOPIC")?
        .trim_matches(['"', '\'', '*', '`', ' '])
        .to_string();
    if topic.is_empty() {
        return None;
    }
    let named: Vec<String> = field("ITEMS")?
        .split([',', ' ', ';'])
        .map(|id| {
            id.trim()
                .trim_matches(['"', '\'', '`', '[', ']'])
                .to_string()
        })
        .filter(|id| !id.is_empty())
        .collect();
    let group: Vec<&NewMaterial> = offered
        .iter()
        .copied()
        .filter(|m| {
            named.iter().any(|id| {
                *id == m.item.item_id || *id == wenlan_types::steep_short_id(&m.item.item_id)
            })
        })
        .take(MAX_CLAIM_ITEMS)
        .collect();
    (!group.is_empty()).then_some((topic, group))
}

fn write_prompt(claim: &NewPageClaim, topic: &str) -> String {
    let mut prompt = format!("Topic: {topic}\n\nNumbered sources (cite only these numbers):\n");
    for source in &claim.sources {
        prompt.push_str(&format!("[{}] {}\n\n", source.n, source.text));
    }
    if !claim.link_targets.is_empty() {
        prompt.push_str("Existing pages you may link as [[Title]] where relevant:\n");
        for title in &claim.link_targets {
            prompt.push_str(&format!("- {title}\n"));
        }
    }
    prompt
}

/// The body without a `# Title` first line: the title is stored apart.
fn without_title_heading(reply: &str) -> String {
    let trimmed = reply.trim();
    match trimmed.split_once('\n') {
        Some((first, rest)) if first.starts_with("# ") => rest.trim().to_string(),
        _ => trimmed.to_string(),
    }
}

#[cfg(test)]
#[path = "steep_new_automatic_test.rs"]
mod steep_new_automatic_test;
