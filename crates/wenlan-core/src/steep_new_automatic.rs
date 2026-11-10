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
//! refusal, a model error) is set aside: remembered in app metadata with its
//! name, the reason, and its items as they were (item and last change), so
//! the next turn moves on; an item that changes is tried again. The person
//! sees the set-aside groups ([`set_aside_groups`]) and can give one back
//! ([`release_set_aside`]) for Automatic to try again. A note that lands
//! needs no record: its items are cited and leave the list.
//!
//! Setting aside is Automatic's own memory only: the on-demand list and
//! claim ([`new_material`], [`claim_new_on`]) never read it, so a caller's AI
//! tool sees set-aside items like any other waiting item.

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
use wenlan_types::{SteepSetAsideGroup, SteepSetAsideItem, SteepSetAsideReason};

/// The agent name the automatic writer's notes carry, as an on-demand
/// commit without a named agent does.
const AGENT: &str = "steep";

/// Most waiting items one grouping call reads: the newest ones.
const GROUP_ITEMS: usize = 40;

/// How long one model call may take before the turn gives up on it.
const MODEL_TIMEOUT: Duration = Duration::from_secs(180);

/// The first record of set-aside items: a bare list of
/// `item_id@last_modified`, with no group name or reason. Read only when
/// [`SET_ASIDE_KEY`] is missing, and migrated on the next write.
const LEGACY_FAILED_ITEMS_KEY: &str = "steep_new_automatic_failed_items_v1";
/// The set-aside groups ([`SetAsideRecord`]).
const SET_ASIDE_KEY: &str = "steep_new_automatic_set_aside_v2";
/// Oldest groups are dropped past this many items, so the record stays small.
const FAILED_ITEMS_CAP: usize = 2_000;

/// One read-modify-write of the set-aside record at a time: a turn that
/// sets a group aside after minutes of model calls must not undo a release
/// made meanwhile.
static SET_ASIDE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const GROUP_SYSTEM: &str = "\
You sort a person's saved material into topics for wiki pages.\n\
You get a numbered list of items, each with an id, its kind, its title and the start of its text, and the titles of pages that already exist.\n\
Pick ONE group of items that clearly share one real topic and are worth one page: two to twelve items, or a single long item that stands alone.\n\
Answer with exactly two lines and nothing else:\n\
TOPIC: <a short page title for the group, in the items' language; reuse an existing page title when the group belongs to that page>\n\
ITEMS: <the ids of the group's items, separated by commas>\n\
If no items belong together, answer with the one item that has the most to say.";

/// New pages a paid page writer (the Anthropic or external API route) may
/// write per local calendar day, across every automatic round and import
/// window. 8 matches the existing first-import batch; it is our own choice,
/// not a benchmark. The on-device writer is free and not capped.
pub const PAID_NEW_PAGES_PER_DAY: usize = 8;

/// app metadata holding `{"day": "YYYY-MM-DD", "pages": n}` for the paid
/// cap, so a restart does not reset the day's count.
const PAID_NEW_PAGES_KEY: &str = "steep_new_automatic_paid_pages_v1";

#[derive(serde::Serialize, serde::Deserialize)]
struct PaidPagesDay {
    day: String,
    pages: usize,
}

/// Today as the local calendar day, the key of the paid cap.
pub fn local_day() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// New pages a paid writer wrote on `day`; 0 on any other day.
pub async fn paid_new_pages_on(db: &MemoryDB, day: &str) -> Result<usize, WenlanError> {
    Ok(db
        .get_app_metadata(PAID_NEW_PAGES_KEY)
        .await?
        .and_then(|raw| serde_json::from_str::<PaidPagesDay>(&raw).ok())
        .filter(|stored| stored.day == day)
        .map_or(0, |stored| stored.pages))
}

/// Count `pages` more paid pages on `day` and return the day's total. A new
/// day starts again from zero.
pub async fn note_paid_new_pages(
    db: &MemoryDB,
    day: &str,
    pages: usize,
) -> Result<usize, WenlanError> {
    let total = paid_new_pages_on(db, day).await?.saturating_add(pages);
    let raw = serde_json::to_string(&PaidPagesDay {
        day: day.to_string(),
        pages: total,
    })
    .map_err(WenlanError::from)?;
    db.set_app_metadata(PAID_NEW_PAGES_KEY, &raw).await?;
    Ok(total)
}

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

/// One item as it was set aside: a change to it (a newer `last_modified`)
/// opens it again.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct SetAsideKey {
    item_id: String,
    last_modified: i64,
}

impl SetAsideKey {
    fn of(material: &NewMaterial) -> Self {
        Self {
            item_id: material.item.item_id.clone(),
            last_modified: material.item.last_modified,
        }
    }

    fn matches(&self, material: &NewMaterial) -> bool {
        self.item_id == material.item.item_id && self.last_modified == material.item.last_modified
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct SetAsideEntry {
    /// The topic the writer named, or the item's title for one item. Empty
    /// for a group migrated from the first record: the list names it by its
    /// item.
    #[serde(default)]
    name: String,
    reason: SteepSetAsideReason,
    items: Vec<SetAsideKey>,
    #[serde(default)]
    set_aside_at: i64,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct SetAsideRecord {
    /// Oldest first.
    #[serde(default)]
    groups: Vec<SetAsideEntry>,
}

impl SetAsideRecord {
    fn holds(&self, material: &NewMaterial) -> bool {
        self.groups
            .iter()
            .any(|group| group.items.iter().any(|key| key.matches(material)))
    }

    fn push(&mut self, entry: SetAsideEntry) {
        self.groups.push(entry);
        let mut total: usize = self.groups.iter().map(|g| g.items.len()).sum();
        while total > FAILED_ITEMS_CAP && self.groups.len() > 1 {
            total -= self.groups.remove(0).items.len();
        }
    }
}

/// The first record's `item_id@last_modified` list, one group per item
/// (named by its item when listed). An entry that does not parse is dropped.
fn migrate_legacy(raw: &str, now: i64) -> SetAsideRecord {
    let keys = serde_json::from_str::<Vec<String>>(raw).unwrap_or_default();
    let groups = keys
        .iter()
        .filter_map(|key| {
            let (item_id, at) = key.rsplit_once('@')?;
            Some(SetAsideEntry {
                name: String::new(),
                reason: SteepSetAsideReason::Other,
                items: vec![SetAsideKey {
                    item_id: item_id.to_string(),
                    last_modified: at.parse().ok()?,
                }],
                set_aside_at: now,
            })
        })
        .filter(|entry| !entry.items[0].item_id.is_empty())
        .collect();
    SetAsideRecord { groups }
}

async fn load_set_aside(db: &MemoryDB) -> Result<SetAsideRecord, WenlanError> {
    if let Some(raw) = db.get_app_metadata(SET_ASIDE_KEY).await? {
        return Ok(serde_json::from_str(&raw).unwrap_or_else(|error| {
            log::warn!("[steep-new-auto] set-aside record unreadable, starting over: {error}");
            SetAsideRecord::default()
        }));
    }
    Ok(db
        .get_app_metadata(LEGACY_FAILED_ITEMS_KEY)
        .await?
        .map(|raw| migrate_legacy(&raw, chrono::Utc::now().timestamp()))
        .unwrap_or_default())
}

async fn store_set_aside(db: &MemoryDB, record: &SetAsideRecord) -> Result<(), WenlanError> {
    let raw = serde_json::to_string(record).map_err(WenlanError::from)?;
    db.set_app_metadata(SET_ASIDE_KEY, &raw).await
}

async fn remember_failed(
    db: &MemoryDB,
    name: &str,
    reason: SteepSetAsideReason,
    items: &[&NewMaterial],
) -> Result<(), WenlanError> {
    let _guard = SET_ASIDE_LOCK.lock().await;
    let mut record = load_set_aside(db).await?;
    let keys: Vec<SetAsideKey> = items
        .iter()
        .filter(|item| !record.holds(item))
        .map(|item| SetAsideKey::of(item))
        .collect();
    if keys.is_empty() {
        return Ok(());
    }
    record.push(SetAsideEntry {
        name: name.trim().to_string(),
        reason,
        items: keys,
        set_aside_at: chrono::Utc::now().timestamp(),
    });
    store_set_aside(db, &record).await
}

/// The groups Automatic set aside in `scope`, newest first. A group lists
/// only its items still waiting uncited and unchanged; a group with none
/// left is not listed.
pub async fn set_aside_groups(
    db: &MemoryDB,
    scope: &ReadScope,
) -> Result<Vec<SteepSetAsideGroup>, WenlanError> {
    let record = load_set_aside(db).await?;
    if record.groups.is_empty() {
        return Ok(Vec::new());
    }
    let material = new_material(db, scope).await?;
    let mut groups: Vec<SteepSetAsideGroup> = record
        .groups
        .iter()
        .filter_map(|entry| {
            let items: Vec<SteepSetAsideItem> = entry
                .items
                .iter()
                .filter_map(|key| material.iter().find(|m| key.matches(m)))
                .map(|m| SteepSetAsideItem {
                    item_id: m.item.item_id.clone(),
                    title: m.item.title.clone(),
                    kind: m.item.kind.clone(),
                    space: m.item.space.clone(),
                })
                .collect();
            let first = items.first()?;
            let name = if entry.name.is_empty() {
                first.title.clone()
            } else {
                entry.name.clone()
            };
            Some(SteepSetAsideGroup {
                name,
                reason: entry.reason,
                items,
                set_aside_at: entry.set_aside_at,
            })
        })
        .collect();
    // The record is oldest first; a stable reverse keeps same-second groups
    // newest first too.
    groups.reverse();
    groups.sort_by_key(|group| std::cmp::Reverse(group.set_aside_at));
    Ok(groups)
}

/// Give back the set-aside items `item_ids` names that wait in `scope`, so
/// Automatic may try them again. Returns how many were set aside and are
/// given back; an id that is not set aside (or not in `scope`) is ignored.
pub async fn release_set_aside(
    db: &MemoryDB,
    scope: &ReadScope,
    item_ids: &[String],
) -> Result<usize, WenlanError> {
    let wanted: Vec<&str> = item_ids
        .iter()
        .map(|id| id.trim())
        .filter(|id| !id.is_empty())
        .collect();
    if wanted.is_empty() {
        return Ok(0);
    }
    let material = new_material(db, scope).await?;
    let visible: Vec<&NewMaterial> = material
        .iter()
        .filter(|m| wanted.contains(&m.item.item_id.as_str()))
        .collect();
    if visible.is_empty() {
        return Ok(0);
    }
    let _guard = SET_ASIDE_LOCK.lock().await;
    let mut record = load_set_aside(db).await?;
    let mut released: Vec<&str> = Vec::new();
    for group in &mut record.groups {
        group.items.retain(|key| {
            let Some(m) = visible.iter().find(|m| m.item.item_id == key.item_id) else {
                return true;
            };
            // Older versions of the item are not set aside any more either.
            if key.matches(m) && !released.contains(&key.item_id.as_str()) {
                released.push(m.item.item_id.as_str());
            }
            false
        });
    }
    record.groups.retain(|group| !group.items.is_empty());
    store_set_aside(db, &record).await?;
    Ok(released.len())
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
    let set_aside = load_set_aside(db).await?;
    let material = new_material(db, &ReadScope::Global).await?;
    let open: Vec<&NewMaterial> = material
        .iter()
        .filter(|m| !m.item.held && !set_aside.holds(m))
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
                remember_failed(
                    db,
                    &newest.item.title,
                    SteepSetAsideReason::TooShort,
                    &[newest],
                )
                .await?;
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
            // The only `Invalid` a claim of listed items gives is the floor:
            // two sources, or one long enough to stand alone.
            let reason = match error {
                SteepError::Invalid(_) => SteepSetAsideReason::TooShort,
                _ => SteepSetAsideReason::Other,
            };
            remember_failed(db, &topic, reason, &group).await?;
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
    let (outcome, reason) = match outcome {
        Ok(outcome) => (outcome, SteepSetAsideReason::Other),
        Err(SteepError::Db(error)) => return Err(error),
        Err(error) => {
            log::info!("[steep-new-auto] '{topic}' not saved: {error}");
            let reason = match error {
                SteepError::CitationGate(_) => SteepSetAsideReason::SourcesDidNotMatch,
                _ => SteepSetAsideReason::Other,
            };
            (NewPageOutcome::Failed, reason)
        }
    };
    if outcome == NewPageOutcome::Failed {
        remember_failed(db, &topic, reason, &group).await?;
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
