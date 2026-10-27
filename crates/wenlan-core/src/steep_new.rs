// SPDX-License-Identifier: Apache-2.0
//! **Steep's new pages**: material no note uses yet becomes notes.
//!
//! Steep's first job, before bringing out-of-date notes up to date. Imported
//! notes, chats and files, and what an AI saved, sit in Wenlan as searchable
//! memories until something writes them into notes. In Automatic mode the
//! daemon's model does it in the background; On demand, the user's AI tool
//! does the model work through this module and the daemon does the rest.
//!
//! The protocol mirrors [`crate::steep`]:
//!
//! 1. [`list_new`]: the material, folded into items (one note of a vault, one
//!    chat, one file, one capture), newest first. No embeddings: the caller's
//!    small model groups the items by topic from their titles and snippets.
//! 2. [`claim_new`]: one group's items, numbered as sources, and a token that
//!    holds them so another run does not write the same note.
//! 3. [`commit_new`]: the note is checked against that snapshot (every
//!    source unchanged and still uncited, every `[n]` in range, enough cited
//!    sentences backed by their source) and born through the same page write
//!    the automatic run uses, as a `distilled` note.
//!
//! Holds live in memory, like steep's: a restart ends them, and a caller whose
//! token is gone claims again.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use wenlan_types::requests::CreateConceptRequest;
use wenlan_types::space_context::WriteSpaceTarget;
use wenlan_types::steep::SteepCodeAnswer;
use wenlan_types::steep::SteepSource;
use wenlan_types::steep_new::{
    SteepExistingPage, SteepNewItem, SteepNewLinksResponse, SteepOrganizeCommitResponse,
    SteepOrganizeWork, STEEP_NEW_KIND_CAPTURE, STEEP_NEW_KIND_CHAT, STEEP_NEW_KIND_FILE,
    STEEP_NEW_KIND_NOTE,
};

use crate::db::MemoryDB;
use crate::error::WenlanError;
use crate::page_titles::{loose_title_key, strict_same_title_note, strict_title_key};
use crate::post_write::{page_write, PageWrite};
use crate::read_scope::ReadScope;
use crate::steep::SteepError;
use crate::steep_organize::{OrganizeOptions, OrganizeTask};

/// How long a claim holds its items: long enough for a main model to write one
/// note, after every other group of the run was claimed with it (a run claims
/// its groups together and writes them as their helpers finish). Reading a
/// claim's tasks or links, or landing its answers, renews it.
pub const NEW_PAGE_HOLD: Duration = Duration::from_secs(45 * 60);

/// Characters of an item's text the list shows: with the title, enough for
/// a small model to group items by topic, and small enough that one run's
/// list of the newest items stays a few thousand tokens.
pub const SNIPPET_CHARS: usize = 120;

/// Most items one claim takes. A topic bigger than this is written from its
/// first items; the rest stay on the list for the next note.
pub const MAX_CLAIM_ITEMS: usize = 12;

/// Most numbered sources one claim hands out, and most characters of each.
/// Together they keep one claim inside a main model's comfortable context.
pub const MAX_CLAIM_SOURCES: usize = 24;
pub const SOURCE_CHAR_CAP: usize = 3_000;
pub const CLAIM_CHAR_CAP: usize = 40_000;

/// A note from fewer sources than this needs one source of at least
/// [`MIN_SINGLE_SOURCE_CHARS`] characters.
pub const MIN_SOURCES: usize = 2;
/// A group below this source count is worth warning about when enough other
/// material is waiting to make a fuller note.
pub const THIN_GROUP_SOURCES: usize = 4;
const PLENTY_WAITING_ITEMS: usize = 4;
pub const MIN_SINGLE_SOURCE_CHARS: usize = 800;

/// Longest title a note may get.
const MAX_TITLE_CHARS: usize = 120;

/// Most existing pages a claim offers as `[[links]]`.
const MAX_LINK_TARGETS: usize = 10;

/// Most existing page titles the list hands to grouping.
pub const MAX_PAGE_TITLES: usize = 50;

/// How many of the newest notes a topic is matched against by title.
const TITLE_MATCH_NOTES: usize = 2_000;

/// One claim, as handed out.
#[derive(Debug, Clone)]
struct NewClaim {
    item_ids: Vec<String>,
    /// `(memory source id, text)` in citation order: `[n]` is `sources[n - 1]`.
    sources: Vec<(String, String)>,
    space: Option<String>,
    scope: ReadScope,
    until: Instant,
    /// Organize tasks still without an answer ([`organize_new`]).
    organize: Vec<OrganizeTask>,
    /// The group's topic, as the caller named it.
    topic: Option<String>,
}

#[derive(Default)]
struct Holds {
    by_token: HashMap<String, NewClaim>,
    /// item id -> token holding it.
    by_item: HashMap<String, String>,
}

impl Holds {
    fn expire(&mut self, now: Instant) {
        let gone: Vec<String> = self
            .by_token
            .iter()
            .filter(|(_, claim)| claim.until <= now)
            .map(|(token, _)| token.clone())
            .collect();
        for token in gone {
            self.drop_token(&token);
        }
    }

    fn drop_token(&mut self, token: &str) -> Option<NewClaim> {
        let claim = self.by_token.remove(token)?;
        for item in &claim.item_ids {
            if self.by_item.get(item).map(String::as_str) == Some(token) {
                self.by_item.remove(item);
            }
        }
        Some(claim)
    }
}

static HOLDS: LazyLock<Mutex<Holds>> = LazyLock::new(|| Mutex::new(Holds::default()));

fn holds() -> std::sync::MutexGuard<'static, Holds> {
    HOLDS.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// The item a memory row belongs to. A chat's messages share their source id;
/// a vault note's or file's chunks are `<source>::<path>::<n>`.
pub(crate) fn item_key(source_id: &str) -> &str {
    if source_id.starts_with("import_") {
        return source_id;
    }
    match source_id.rsplit_once("::") {
        Some((head, tail)) if !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_digit()) => head,
        _ => source_id,
    }
}

fn item_kind(source_id: &str, source_agent: Option<&str>, has_file_hash: bool) -> &'static str {
    if source_id.starts_with("import_chatgpt_") || source_id.starts_with("import_claude_") {
        STEEP_NEW_KIND_CHAT
    } else if matches!(source_agent, Some("obsidian" | "logseq")) {
        STEEP_NEW_KIND_NOTE
    } else if has_file_hash || matches!(source_agent, Some("folder" | "reconcile")) {
        STEEP_NEW_KIND_FILE
    } else {
        STEEP_NEW_KIND_CAPTURE
    }
}

/// A chunk title from a vault note can carry its heading path
/// (`Note > Heading`); the item is the whole note.
fn item_title(title: Option<&str>, key: &str, head: &str) -> String {
    let from_title = title
        .map(|t| t.split(" > ").next().unwrap_or(t).trim())
        .filter(|t| !t.is_empty());
    if let Some(t) = from_title {
        return t.to_string();
    }
    if let Some((_, path)) = key.rsplit_once("::") {
        let name = path.rsplit('/').next().unwrap_or(path);
        let name = name.strip_suffix(".md").unwrap_or(name);
        if !name.is_empty() {
            return name.to_string();
        }
    }
    let line = head
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    line.trim_start_matches('#')
        .trim()
        .chars()
        .take(80)
        .collect()
}

fn snippet(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= SNIPPET_CHARS {
        flat
    } else {
        let cut: String = flat.chars().take(SNIPPET_CHARS).collect();
        format!("{cut}…")
    }
}

/// One item and the memory source ids it is made of.
#[derive(Debug, Clone)]
pub struct NewMaterial {
    pub item: SteepNewItem,
    pub source_ids: Vec<String>,
}

/// Every item of new material in `scope`, newest change first.
pub async fn new_material(
    db: &MemoryDB,
    scope: &ReadScope,
) -> Result<Vec<NewMaterial>, WenlanError> {
    let rows = db.list_unpaged_memory_rows(SNIPPET_CHARS + 40).await?;
    let material = fold_material(db, scope, rows).await?;
    remember_short_ids(&material);
    Ok(material)
}

/// Item ids by the short id a compact list shows them under
/// ([`wenlan_types::steep_short_id`]), as the lists handed them out, so a
/// claim finds its items without reading every item again.
static SHORT_IDS: LazyLock<Mutex<HashMap<String, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Most short ids remembered; past it the map starts over.
const MAX_SHORT_IDS: usize = 200_000;

fn remember_short_ids(material: &[NewMaterial]) {
    let mut ids = SHORT_IDS
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    if ids.len() + material.len() > MAX_SHORT_IDS {
        ids.clear();
    }
    for m in material {
        ids.insert(
            wenlan_types::steep_short_id(&m.item.item_id),
            m.item.item_id.clone(),
        );
    }
}

/// The new material of the items `wanted` names (item ids, or the short ids
/// a list showed), read through the source id index; `None` when some id
/// names nothing this way (a short id no list in this run of the daemon
/// handed out), and the caller reads all of it.
async fn material_of(
    db: &MemoryDB,
    scope: &ReadScope,
    wanted: &[String],
) -> Result<Option<Vec<NewMaterial>>, WenlanError> {
    let keys: Vec<String> = {
        let ids = SHORT_IDS
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        wanted
            .iter()
            .map(|id| ids.get(id).cloned().unwrap_or_else(|| id.clone()))
            .collect()
    };
    let rows = db
        .unpaged_memory_rows_of(SNIPPET_CHARS + 40, &keys)
        .await?
        .into_iter()
        .filter(|row| keys.iter().any(|key| item_key(&row.source_id) == key))
        .collect();
    let material = fold_material(db, scope, rows).await?;
    let found = |key: &String| material.iter().any(|m| &m.item.item_id == key);
    if keys.iter().all(found) {
        Ok(Some(material))
    } else {
        Ok(None)
    }
}

/// `rows` (newest first) folded into items, leaving out items a note drew on
/// and rows outside `scope`.
async fn fold_material(
    db: &MemoryDB,
    scope: &ReadScope,
    rows: Vec<crate::db::steep_new::UnpagedRow>,
) -> Result<Vec<NewMaterial>, WenlanError> {
    // An item a note already drew on is done, even if the note left out a
    // chunk (a heading-only chunk of a file, a greeting in a chat): offering
    // the rest again would ask for the same page twice.
    let taken: HashSet<String> = db
        .list_paged_memory_sources()
        .await?
        .iter()
        .map(|source_id| item_key(source_id).to_string())
        .collect();
    let mut order: Vec<String> = Vec::new();
    let mut by_key: HashMap<String, NewMaterial> = HashMap::new();
    let mut seen_sources: HashSet<String> = HashSet::new();
    // (key -> (chunk index, head)) so the snippet comes from the item's start.
    let mut first_chunk: HashMap<String, (i64, String, Option<String>)> = HashMap::new();
    for row in rows {
        if !scope.matches(row.space.as_deref()) {
            continue;
        }
        let key = item_key(&row.source_id).to_string();
        if taken.contains(&key) {
            continue;
        }
        let entry = by_key.entry(key.clone()).or_insert_with(|| {
            order.push(key.clone());
            NewMaterial {
                item: SteepNewItem {
                    item_id: key.clone(),
                    kind: item_kind(
                        &row.source_id,
                        row.source_agent.as_deref(),
                        row.has_file_hash,
                    )
                    .to_string(),
                    title: String::new(),
                    snippet: String::new(),
                    space: row.space.clone(),
                    chars: 0,
                    last_modified: row.last_modified,
                    held: false,
                },
                source_ids: Vec::new(),
            }
        });
        entry.item.chars = entry
            .item
            .chars
            .saturating_add(u32::try_from(row.chars.max(0)).unwrap_or(u32::MAX));
        entry.item.last_modified = entry.item.last_modified.max(row.last_modified);
        if seen_sources.insert(row.source_id.clone()) {
            entry.source_ids.push(row.source_id.clone());
        }
        let better = first_chunk
            .get(&key)
            .is_none_or(|(index, _, _)| row.chunk_index < *index);
        if better {
            first_chunk.insert(key, (row.chunk_index, row.head, row.title));
        }
    }
    let held: HashSet<String> = {
        let mut holds = holds();
        holds.expire(Instant::now());
        holds.by_item.keys().cloned().collect()
    };
    let mut out = Vec::with_capacity(order.len());
    for key in order {
        let Some(mut material) = by_key.remove(&key) else {
            continue;
        };
        let (_, head, title) = first_chunk.remove(&key).unwrap_or_default();
        material.item.title = item_title(title.as_deref(), &key, &head);
        material.item.snippet = snippet(&head);
        material.item.held = held.contains(&key);
        material.source_ids.sort();
        out.push(material);
    }
    out.sort_by(|a, b| {
        b.item
            .last_modified
            .cmp(&a.item.last_modified)
            .then_with(|| a.item.item_id.cmp(&b.item.item_id))
    });
    Ok(out)
}

/// How many items of new material wait in `scope`. Cheap enough for a hint on
/// every capture: one read, no texts.
pub async fn new_material_count(db: &MemoryDB, scope: &ReadScope) -> Result<usize, WenlanError> {
    Ok(new_material(db, scope).await?.len())
}

/// A page of [`new_material`]: `(items, total, next cursor)`. The cursor is
/// the offset of the next page.
pub async fn list_new(
    db: &MemoryDB,
    scope: &ReadScope,
    limit: usize,
    cursor: Option<&str>,
) -> Result<NewList, WenlanError> {
    let all = new_material(db, scope).await?;
    let total = all.len();
    let start = cursor
        .and_then(|c| c.parse::<usize>().ok())
        .unwrap_or(0)
        .min(total);
    let limit = limit.clamp(1, 200);
    let end = (start + limit).min(total);
    let mut kinds: BTreeMap<String, u32> = BTreeMap::new();
    for m in &all {
        *kinds.entry(m.item.kind.clone()).or_default() += 1;
    }
    // On the first page only: grouping reads them once.
    let mut page_titles: Vec<String> = Vec::new();
    if total > 0 && start == 0 {
        match db.recent_notes(MAX_PAGE_TITLES * 4).await {
            Ok(notes) => {
                for (_, title, space) in notes {
                    if page_titles.len() < MAX_PAGE_TITLES
                        && scope.matches(space.as_deref())
                        && !page_titles.contains(&title)
                    {
                        page_titles.push(title);
                    }
                }
            }
            Err(error) => log::warn!("[steep-new] page titles: {error}"),
        }
    }
    Ok(NewList {
        items: all[start..end].iter().map(|m| m.item.clone()).collect(),
        total,
        next_cursor: (end < total).then(|| end.to_string()),
        kinds,
        held: all.iter().filter(|m| m.item.held).count(),
        page_titles,
    })
}

/// One page of the new material, with counts over all of it.
#[derive(Debug, Clone)]
pub struct NewList {
    pub items: Vec<SteepNewItem>,
    pub total: usize,
    pub next_cursor: Option<String>,
    /// How many items of each kind, over all of it.
    pub kinds: BTreeMap<String, u32>,
    /// How many items a claim holds now.
    pub held: usize,
    /// The newest existing page titles in scope, for grouping.
    pub page_titles: Vec<String>,
}

/// What a claim hands out.
#[derive(Debug, Clone)]
pub struct NewPageClaim {
    pub token: String,
    pub held_until: i64,
    pub sources: Vec<SteepSource>,
    pub skipped: Vec<String>,
    /// Organizing the sources still need, when `organize` options were given.
    pub organize: Option<SteepOrganizeWork>,
    /// Existing pages the page may link as `[[Title]]`.
    pub link_targets: Vec<String>,
    /// An existing page on this subject: the sources go into it
    /// ([`attach_new`]) instead of a second page.
    pub existing_page: Option<SteepExistingPage>,
    /// The group is small even though enough unclaimed material remains to
    /// reconsider it. Advisory only.
    pub thin_group: bool,
}

/// Hold `item_ids` (one topic) for one note and number their text.
///
/// With `organize` (the caller's AI tool does the model work, no background
/// model does), the claim also plans the classification and entity
/// detection its sources still need ([`crate::steep_organize`]); the answers
/// land through [`organize_new`] before the page is written.
pub async fn claim_new(
    db: &MemoryDB,
    scope: &ReadScope,
    item_ids: &[String],
    organize: Option<&OrganizeOptions>,
) -> Result<NewPageClaim, SteepError> {
    claim_new_on(db, scope, item_ids, organize, None).await
}

/// [`claim_new`] for a group the caller named `topic`: when an existing
/// page in the claim's Space has that title (years and version numbers
/// aside), the claim offers it as `existing_page`.
pub async fn claim_new_on(
    db: &MemoryDB,
    scope: &ReadScope,
    item_ids: &[String],
    organize: Option<&OrganizeOptions>,
    topic: Option<&str>,
) -> Result<NewPageClaim, SteepError> {
    let topic = topic
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string);
    let mut wanted: Vec<String> = Vec::new();
    for id in item_ids {
        let id = id.trim();
        if !id.is_empty() && !wanted.iter().any(|w| w == id) {
            wanted.push(id.to_string());
        }
    }
    if wanted.is_empty() {
        return Err(SteepError::Invalid("item_ids is empty".into()));
    }
    let started = Instant::now();
    let material = match material_of(db, scope, &wanted).await? {
        Some(material) => material,
        None => new_material(db, scope).await?,
    };
    let read_items = started.elapsed();
    let by_id: HashMap<&str, &NewMaterial> = material
        .iter()
        .map(|m| (m.item.item_id.as_str(), m))
        .collect();
    // A compact list names each item by its short id.
    let by_short_id: HashMap<String, &NewMaterial> = material
        .iter()
        .map(|m| (wenlan_types::steep_short_id(&m.item.item_id), m))
        .collect();

    let now = Instant::now();
    let mut skipped = Vec::new();
    let mut taken: Vec<&NewMaterial> = Vec::new();
    {
        let mut holds = holds();
        holds.expire(now);
        for id in &wanted {
            let found = by_id
                .get(id.as_str())
                .copied()
                .or_else(|| by_short_id.get(id.as_str()).copied());
            match found {
                Some(m)
                    if !holds.by_item.contains_key(&m.item.item_id)
                        && taken.len() < MAX_CLAIM_ITEMS =>
                {
                    if !taken.iter().any(|t| t.item.item_id == m.item.item_id) {
                        taken.push(m)
                    }
                }
                _ => skipped.push(id.clone()),
            }
        }
    }
    if taken.is_empty() {
        return Err(SteepError::NotWaiting(
            "none of these items is free: each is already in a note, held by another run, \
             or gone. List again"
                .into(),
        ));
    }

    let source_ids: Vec<String> = taken
        .iter()
        .flat_map(|m| m.source_ids.iter().cloned())
        .collect();
    let texts = db.unpaged_texts(&source_ids).await?;
    let mut joined: HashMap<String, String> = HashMap::new();
    for (source_id, _, text) in texts {
        let slot = joined.entry(source_id).or_default();
        if !slot.is_empty() {
            slot.push_str("\n\n");
        }
        slot.push_str(text.trim());
    }

    let mut sources: Vec<(String, String)> = Vec::new();
    let mut total_chars = 0usize;
    let mut claimed_items: Vec<String> = Vec::new();
    'items: for m in &taken {
        let mut used_any = false;
        for source_id in &m.source_ids {
            let Some(text) = joined.get(source_id) else {
                continue;
            };
            if text.trim().is_empty() {
                continue;
            }
            if sources.len() >= MAX_CLAIM_SOURCES || total_chars >= CLAIM_CHAR_CAP {
                if !used_any {
                    skipped.push(m.item.item_id.clone());
                }
                break 'items;
            }
            let capped: String = text.chars().take(SOURCE_CHAR_CAP).collect();
            total_chars += capped.chars().count();
            sources.push((source_id.clone(), capped));
            used_any = true;
        }
        if used_any {
            claimed_items.push(m.item.item_id.clone());
        } else if !skipped.contains(&m.item.item_id) {
            skipped.push(m.item.item_id.clone());
        }
    }
    // Items the loop never reached (the claim filled up) stay on the list.
    for m in &taken {
        let id = &m.item.item_id;
        if !claimed_items.contains(id) && !skipped.contains(id) {
            skipped.push(id.clone());
        }
    }
    if sources.is_empty() {
        return Err(SteepError::Changed(
            "these items have no text left. List again".into(),
        ));
    }
    // A note needs something to say: two sources, or one long enough to
    // stand alone (a long chat, a full vault note).
    if sources.len() < MIN_SOURCES && total_chars < MIN_SINGLE_SOURCE_CHARS {
        return Err(SteepError::Invalid(format!(
            "too little to make a note from ({total_chars} characters in one source). Group it with related items, or leave it for later"
        )));
    }

    // The note goes to the items' Space when they share one, else unfiled.
    let mut spaces = taken
        .iter()
        .filter(|m| claimed_items.contains(&m.item.item_id))
        .map(|m| m.item.space.clone());
    let first = spaces.next().flatten();
    let space = if spaces.all(|s| s == first) {
        first
    } else {
        None
    };
    let waiting_after_claim = new_material_count(db, scope)
        .await?
        .saturating_sub(claimed_items.len());
    let thin_group =
        sources.len() < THIN_GROUP_SOURCES && waiting_after_claim >= PLENTY_WAITING_ITEMS;

    let token = uuid::Uuid::new_v4().to_string();
    let until = now + NEW_PAGE_HOLD;
    {
        let mut holds = holds();
        holds.expire(Instant::now());
        if let Some(clash) = claimed_items
            .iter()
            .find(|i| holds.by_item.contains_key(*i))
        {
            return Err(SteepError::NotWaiting(format!(
                "another run took {clash} just now. List again"
            )));
        }
        for item in &claimed_items {
            holds.by_item.insert(item.clone(), token.clone());
        }
        holds.by_token.insert(
            token.clone(),
            NewClaim {
                item_ids: claimed_items,
                sources: sources.clone(),
                space: space.clone(),
                scope: scope.clone(),
                until,
                organize: Vec::new(),
                topic: topic.clone(),
            },
        );
    }
    let held_until = chrono::Utc::now().timestamp() + NEW_PAGE_HOLD.as_secs() as i64;
    let source_ids: Vec<String> = sources.iter().map(|(id, _)| id.clone()).collect();
    // Planning runs after the hold, so no other run can claim these sources
    // while their model-free entity steps commit. A failure leaves the claim
    // as it was before organizing existed: the page can still be written.
    let read_sources = started.elapsed();
    let mut organize_work = None;
    if let Some(options) = organize {
        match crate::steep_organize::plan(db, options, &source_ids).await {
            Ok(tasks) => {
                organize_work = crate::steep_organize::work(&tasks);
                if let Some(claim) = holds().by_token.get_mut(&token) {
                    claim.organize = tasks;
                }
            }
            Err(error) => log::warn!("[steep-new] organize planning failed: {error}"),
        }
    }
    let planned = started.elapsed();
    let existing_page =
        existing_page(db, scope, space.as_deref(), &source_ids, topic.as_deref()).await;
    let link_targets = link_targets(
        db,
        scope,
        space.as_deref(),
        &sources,
        existing_page.as_ref().map(|page| page.page_id.as_str()),
    )
    .await;
    log::info!(
        "[steep-new] claim of {} sources: items {:?}, sources {:?}, organize {:?}, total {:?}",
        sources.len(),
        read_items,
        read_sources,
        planned,
        started.elapsed()
    );
    Ok(NewPageClaim {
        token,
        held_until,
        sources: sources
            .into_iter()
            .enumerate()
            .map(|(i, (memory_id, text))| SteepSource {
                n: u32::try_from(i + 1).unwrap_or(u32::MAX),
                memory_id,
                text,
            })
            .collect(),
        skipped,
        organize: organize_work,
        link_targets,
        existing_page,
        thin_group,
    })
}

/// A held claim, its hold renewed for another [`NEW_PAGE_HOLD`].
fn renew(token: &str) -> Result<NewClaim, SteepError> {
    let mut holds = holds();
    holds.expire(Instant::now());
    let claim = holds.by_token.get_mut(token).ok_or(SteepError::TokenGone)?;
    claim.until = Instant::now() + NEW_PAGE_HOLD;
    Ok(claim.clone())
}

/// A held claim's organize tasks still without an answer, as a helper on a
/// small model fetches them with nothing but the token: the main model never
/// passes source text on. `scope` is the caller's: a Space-locked caller
/// reads only a claim made in its Space. Needs no model.
pub fn organize_tasks_new(
    token: &str,
    scope: &ReadScope,
) -> Result<Option<SteepOrganizeWork>, SteepError> {
    let claim = renew(token)?;
    if *scope != ReadScope::Global && claim.scope != *scope {
        return Err(SteepError::TokenGone);
    }
    Ok(crate::steep_organize::work(&claim.organize))
}

/// Words too common to tell two titles apart.
const TITLE_STOPWORDS: &[&str] = &[
    "a", "an", "and", "the", "of", "in", "on", "for", "to", "with", "my", "our", "notes", "note",
    "about", "&", "和", "與", "的", "筆記",
];

/// How close two titles are, 0 to 1, from their normalized words: 1 when
/// they share every word, else the share of words in common, counted only
/// when they have at least two in common or one title's words are all in
/// the other ("Portugal trip" and "Portugal trip and food"). A one-word
/// title in a script without spaces matches a title that contains it.
fn title_closeness_with(a: &str, b: &str, key: fn(&str) -> String) -> f64 {
    let (ka, kb) = (key(a), key(b));
    if ka.is_empty() || kb.is_empty() {
        return 0.0;
    }
    if ka == kb {
        return 1.0;
    }
    let words = |key: &str| -> std::collections::BTreeSet<String> {
        key.split(' ')
            .filter(|w| !TITLE_STOPWORDS.contains(w))
            .map(str::to_string)
            .collect()
    };
    let (wa, wb) = (words(&ka), words(&kb));
    if wa.is_empty() || wb.is_empty() {
        return 0.0;
    }
    if wa == wb {
        return 1.0;
    }
    let shared = wa.intersection(&wb).count();
    let (small, large) = if wa.len() <= wb.len() {
        (&wa, &wb)
    } else {
        (&wb, &wa)
    };
    if small.len() == 1 && shared == 0 {
        let only = small.iter().next().map(String::as_str).unwrap_or("");
        if !only.is_ascii() && only.chars().count() >= 2 && large.iter().any(|w| w.contains(only)) {
            return 0.8;
        }
        return 0.0;
    }
    let union = wa.union(&wb).count();
    let jaccard = shared as f64 / union as f64;
    if (small.len() >= 2 && shared == small.len()) || shared >= 2 {
        jaccard.max(if shared == small.len() { 0.75 } else { 0.0 })
    } else {
        0.0
    }
}

/// Subject closeness for the claim-time existing-page suggestion. Dates,
/// years and versions do not prevent the caller from seeing a possible match.
fn loose_title_closeness(a: &str, b: &str) -> f64 {
    title_closeness_with(a, b, loose_title_key)
}

/// Whether `title` names the entity called `name`: close to it, or holding
/// every word of it ("Mira's kiln" names Mira; "Billing migration" does not
/// name "Onboarding v2 setup checklist").
fn names_subject_with(name: &str, title: &str, key: fn(&str) -> String) -> bool {
    if title_closeness_with(name, title, key) >= TITLE_CLOSE_ENOUGH {
        return true;
    }
    let words = |text: &str| -> Vec<String> {
        key(text)
            .split(' ')
            .filter(|w| !w.is_empty() && !TITLE_STOPWORDS.contains(w))
            .map(str::to_string)
            .collect()
    };
    let (name_words, title_words) = (words(name), words(title));
    !name_words.is_empty()
        && name_words.iter().all(|w| {
            title_words.iter().any(|t| {
                t == w || (!w.is_ascii() && w.chars().count() >= 2 && t.contains(w.as_str()))
            })
        })
}

/// Whether `title` names the entity called `name` as its only subject:
/// [`names_subject_with`], and the title does not join the name to another thing
/// of the same rank ("Lisbon and Porto 2026", "Billing & Onboarding"): such
/// a note is about both, and so about neither one entity. What a title
/// joins counts as another thing when it starts with a capital letter or is
/// in a script without capitals; "Portugal trip and food" is still about
/// Portugal.
fn names_sole_subject_with(name: &str, title: &str, key: fn(&str) -> String) -> bool {
    if !names_subject_with(name, title, key) {
        return false;
    }
    // "Rock and Roll" for the entity Rock and Roll.
    if title_closeness_with(name, title, key) >= TITLE_CLOSE_ENOUGH {
        return true;
    }
    const JOINS: &[&str] = &[
        "and", "&", "+", "vs", "vs.", "versus", "or", "/", "和", "與", "与", "及", "跟",
    ];
    let name_key = key(name);
    let mut sides: Vec<Vec<&str>> = vec![Vec::new()];
    for word in title.split_whitespace() {
        if JOINS.contains(&word.to_lowercase().as_str()) {
            sides.push(Vec::new());
        } else if let Some(side) = sides.last_mut() {
            side.push(word);
        }
    }
    let sides: Vec<String> = sides
        .into_iter()
        .map(|side| side.join(" "))
        .filter(|side| !key(side).is_empty())
        .collect();
    if sides.len() < 2 {
        return true;
    }
    let is_name = |side: &str| names_subject_with(name, side, key) || key(side) == name_key;
    sides.iter().filter(|side| !is_name(side)).all(|side| {
        let first = side.chars().find(|c| c.is_alphabetic());
        // Another thing of the same rank: a proper name, or a word in a
        // script that has no capitals to tell.
        !first.is_some_and(|c| c.is_uppercase() || !c.is_ascii())
    })
}

/// Least title closeness for a topic to name an existing page.
const TITLE_CLOSE_ENOUGH: f64 = 0.6;

/// An existing note on a claim's subject in the caller's scope: the note in
/// the claim's Space (`space`) about the first entity a strict majority of
/// the sources mention that has one, else a note whose title matches `topic`
/// ([`loose_title_closeness`], the closest, newest first), in the claim's Space
/// before any other. Best effort: a failed read offers none.
async fn existing_page(
    db: &MemoryDB,
    scope: &ReadScope,
    space: Option<&str>,
    source_ids: &[String],
    topic: Option<&str>,
) -> Option<SteepExistingPage> {
    let fits = |page_space: &Option<String>| {
        page_space.as_deref() == space && scope.matches(page_space.as_deref())
    };
    let entities = majority_entities(db, source_ids)
        .await
        .unwrap_or_else(|error| {
            log::warn!("[steep-new] existing page: {error}");
            Vec::new()
        });
    // A note about an entity the sources mention is this group's page only
    // when that entity is the subject of both: named by the note's title and
    // by the group's topic. A note can carry an entity that is not its
    // subject (most of its sources mentioned it in passing).
    let names = db
        .entity_page_titles(&entities)
        .await
        .unwrap_or_else(|error| {
            log::warn!("[steep-new] existing page: {error}");
            Vec::new()
        });
    for entity_id in entities {
        let Some((_, name)) = names.iter().find(|(id, _)| *id == entity_id) else {
            continue;
        };
        if topic.is_some_and(|topic| !names_sole_subject_with(name, topic, loose_title_key)) {
            continue;
        }
        match db.notes_about_entity(&entity_id).await {
            Ok(notes) => {
                if let Some((page_id, title, _)) =
                    notes.into_iter().find(|(_, title, page_space)| {
                        fits(page_space) && names_sole_subject_with(name, title, loose_title_key)
                    })
                {
                    return Some(SteepExistingPage { page_id, title });
                }
            }
            Err(error) => log::warn!("[steep-new] existing page: {error}"),
        }
    }
    let topic = topic?;
    if loose_title_key(topic).is_empty() {
        return None;
    }
    let mut notes = db.notes_titled(topic).await.unwrap_or_else(|error| {
        log::warn!("[steep-new] existing page: {error}");
        Vec::new()
    });
    match db.recent_notes(TITLE_MATCH_NOTES).await {
        Ok(recent) => notes.extend(recent),
        Err(error) => log::warn!("[steep-new] existing page: {error}"),
    }
    // The closest note in the claim's Space; else, when the claim's Space
    // has none, the closest in another Space the caller reads: the sources
    // go into it rather than a second page on the subject.
    let mut best: Option<(f64, String, String)> = None;
    let mut best_elsewhere: Option<(f64, String, String)> = None;
    for (page_id, title, page_space) in notes {
        if !scope.matches(page_space.as_deref()) {
            continue;
        }
        let close = loose_title_closeness(topic, &title);
        if close < TITLE_CLOSE_ENOUGH {
            continue;
        }
        let slot = if page_space.as_deref() == space {
            &mut best
        } else {
            &mut best_elsewhere
        };
        // Newest first: an equally close older note does not win.
        if slot.as_ref().is_none_or(|(b, _, _)| close > *b) {
            *slot = Some((close, page_id, title));
        }
    }
    best.or(best_elsewhere)
        .map(|(_, page_id, title)| SteepExistingPage { page_id, title })
}

/// A held claim's `link_targets` and `existing_page` as they are now: pages
/// written since the claim (by this run's earlier groups) count. Needs no
/// model, and leaves the claim as it is.
pub async fn links_new(db: &MemoryDB, token: &str) -> Result<SteepNewLinksResponse, SteepError> {
    let claim = renew(token)?;
    let source_ids: Vec<String> = claim.sources.iter().map(|(id, _)| id.clone()).collect();
    let space = claim.space.as_deref();
    let existing_page =
        existing_page(db, &claim.scope, space, &source_ids, claim.topic.as_deref()).await;
    let link_targets = link_targets(
        db,
        &claim.scope,
        space,
        &claim.sources,
        existing_page.as_ref().map(|page| page.page_id.as_str()),
    )
    .await;
    Ok(SteepNewLinksResponse {
        link_targets,
        existing_page,
        next_step: String::new(),
    })
}

/// Existing pages a note from `sources` (`(memory id, text)`) may link,
/// best first: notes written from vault notes that the sources' vault notes
/// link to as `[[Note]]` or are linked from, then notes whose title the
/// sources' text names, then notes that share the most entities with the
/// sources (the note is about one, or cites memories that mention one).
/// `existing` (the claim's existing page, which the sources go into) is
/// never one. Each comes as the label a `[[link]]` in a note born in `space`
/// resolves with: the bare title when that reaches this exact note, otherwise
/// `Space/Title` for a note in a named Space. Entity pages are not offered: a
/// wikilink never resolves to one. Best effort: a failed read offers none.
async fn link_targets(
    db: &MemoryDB,
    scope: &ReadScope,
    space: Option<&str>,
    sources: &[(String, String)],
    existing: Option<&str>,
) -> Vec<String> {
    let source_ids: Vec<String> = sources.iter().map(|(id, _)| id.clone()).collect();
    let raw: String = sources
        .iter()
        .map(|(_, text)| text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let text = raw.to_lowercase();
    let mut candidates: Vec<(String, String, Option<String>)> = vault_linked_notes(db, sources)
        .await
        .unwrap_or_else(|error| {
            log::warn!("[steep-new] link targets: {error}");
            Vec::new()
        });
    let mut named = 0;
    match db.recent_notes(TITLE_MATCH_NOTES).await {
        Ok(notes) => {
            for (page_id, title, page_space) in notes {
                if named < MAX_LINK_TARGETS * 2
                    && names_in_text(&text, &title)
                    && !named_only_in_passing(&raw, &title)
                {
                    named += 1;
                    candidates.push((page_id, title, page_space));
                }
            }
        }
        Err(error) => log::warn!("[steep-new] link targets: {error}"),
    }
    let mut entity_ids: Vec<String> = match db.source_entity_links(&source_ids).await {
        Ok(links) => links.into_iter().map(|(_, entity, _)| entity).collect(),
        Err(error) => {
            log::warn!("[steep-new] link targets: {error}");
            Vec::new()
        }
    };
    entity_ids.sort();
    entity_ids.dedup();
    match db
        .notes_sharing_entities(&entity_ids, MAX_LINK_TARGETS * 3)
        .await
    {
        Ok(pages) => candidates.extend(
            pages
                .into_iter()
                .map(|(page_id, title, page_space, _)| (page_id, title, page_space)),
        ),
        Err(error) => log::warn!("[steep-new] link targets: {error}"),
    }
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<String> = Vec::new();
    for (page_id, title, page_space) in candidates {
        if out.len() >= MAX_LINK_TARGETS {
            break;
        }
        if existing == Some(page_id.as_str())
            || !seen.insert(page_id.clone())
            || !scope.matches(page_space.as_deref())
        {
            continue;
        }
        let bare_resolves = if page_space.as_deref() == space {
            true
        } else {
            crate::synthesis::wikilinks::resolve_against_pages(
                db,
                std::slice::from_ref(&title),
                space,
            )
            .await
            .ok()
            .and_then(|links| links.into_iter().next())
            .and_then(|link| link.target_page_id)
            .is_some_and(|resolved| resolved == page_id)
        };
        let label = if bare_resolves {
            title
        } else if let Some(other) = page_space.as_deref() {
            format!("{other}/{title}")
        } else {
            continue;
        };
        let resolves = crate::synthesis::wikilinks::resolve_against_pages(
            db,
            std::slice::from_ref(&label),
            space,
        )
        .await
        .ok()
        .and_then(|links| links.into_iter().next())
        .and_then(|link| link.target_page_id)
        .is_some_and(|resolved| resolved == page_id);
        if resolves && !out.contains(&label) {
            out.push(label);
        }
    }
    out
}

/// Related pages `content` could still link. Candidate generation uses only
/// existing vault links, title mentions and shared entities; this removes the
/// saved page itself and any target its current wikilinks already resolve to.
pub async fn unused_link_targets(
    db: &MemoryDB,
    scope: &ReadScope,
    space: Option<&str>,
    sources: &[(String, String)],
    page_id: &str,
    content: &str,
) -> Vec<String> {
    let candidates = link_targets(db, scope, space, sources, Some(page_id)).await;
    if candidates.is_empty() {
        return candidates;
    }
    let written = crate::synthesis::wikilinks::extract_wikilinks(content);
    let used_ids: HashSet<String> =
        crate::synthesis::wikilinks::resolve_against_pages(db, &written, space)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter_map(|link| link.target_page_id)
            .collect();
    let written_lower: HashSet<String> = written
        .into_iter()
        .map(|label| label.trim().to_lowercase())
        .collect();
    let resolved =
        match crate::synthesis::wikilinks::resolve_against_pages(db, &candidates, space).await {
            Ok(resolved) => resolved,
            Err(_) => {
                return candidates
                    .into_iter()
                    .filter(|label| !written_lower.contains(&label.trim().to_lowercase()))
                    .collect()
            }
        };
    candidates
        .into_iter()
        .zip(resolved)
        .filter_map(|(label, link)| {
            let already_used = written_lower.contains(&label.trim().to_lowercase())
                || link
                    .target_page_id
                    .as_ref()
                    .is_some_and(|target| used_ids.contains(target));
            (!already_used).then_some(label)
        })
        .collect()
}

/// The name a vault note goes by in `[[links]]`: the file name of the note
/// a memory's source id comes from (`<source>::<path>::<n>`), without
/// `.md`. `None` for a memory that is no vault note.
fn vault_note_name(source_id: &str) -> Option<&str> {
    let (_, path) = item_key(source_id).rsplit_once("::")?;
    let file = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let stem = file.get(..file.len().checked_sub(3)?)?;
    let name = stem.trim();
    (file[stem.len()..].eq_ignore_ascii_case(".md") && !name.is_empty()).then_some(name)
}

/// The vault note names `text` links to as `[[Note]]`, `[[folder/Note]]`,
/// `[[Note#Heading]]` or `[[Note|shown]]`.
fn linked_note_names(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for label in crate::synthesis::wikilinks::extract_wikilinks(text) {
        let file = label.rsplit(['/', '\\']).next().unwrap_or(&label).trim();
        let name = match file.len().checked_sub(3) {
            Some(cut) if file.is_char_boundary(cut) && file[cut..].eq_ignore_ascii_case(".md") => {
                file[..cut].trim()
            }
            _ => file,
        };
        if !name.is_empty() && !out.iter().any(|n| same_note_name(n, name)) {
            out.push(name.to_string());
        }
    }
    out
}

/// Whether two vault note names name one note: Obsidian matches them case
/// aside.
fn same_note_name(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

/// Most vault note names one claim looks up links for, each way.
const MAX_LINKED_NAMES: usize = 200;

/// Notes written from vault notes that are linked with the claim's vault
/// notes: a note one of `sources` links to as `[[Note]]`, or a note whose
/// own text links to one of the sources' notes. The note with the most such
/// links comes first, then the newest. The strongest sign two pages belong
/// together: the user linked their notes by hand.
async fn vault_linked_notes(
    db: &MemoryDB,
    sources: &[(String, String)],
) -> Result<Vec<(String, String, Option<String>)>, WenlanError> {
    let mut own: Vec<String> = Vec::new();
    for (source_id, _) in sources {
        if let Some(name) = vault_note_name(source_id) {
            if !own.iter().any(|n| same_note_name(n, name)) {
                own.push(name.to_string());
            }
        }
    }
    let mut linked: Vec<String> = Vec::new();
    for (_, text) in sources {
        for name in linked_note_names(text) {
            let known = |n: &String| same_note_name(n, &name);
            if !own.iter().any(known) && !linked.iter().any(known) {
                linked.push(name);
            }
        }
    }
    own.truncate(MAX_LINKED_NAMES);
    linked.truncate(MAX_LINKED_NAMES);
    // (page id, title, space, links), newest first as found.
    let mut found: Vec<(String, String, Option<String>, usize)> = Vec::new();
    let mut add = |page_id: &str, title: &str, page_space: &Option<String>| match found
        .iter_mut()
        .find(|(id, ..)| id == page_id)
    {
        Some(entry) => entry.3 += 1,
        None => found.push((
            page_id.to_string(),
            title.to_string(),
            page_space.clone(),
            1,
        )),
    };
    // The notes the sources link to: pages citing a memory of that note.
    if !linked.is_empty() {
        let probes: Vec<String> = linked
            .iter()
            .map(|name| format!("{}.md::", name.to_ascii_lowercase()))
            .collect();
        let mut seen: HashSet<(String, String)> = HashSet::new();
        for (page_id, title, page_space, source_id) in db.notes_citing_sources_with(&probes).await?
        {
            let Some(name) = vault_note_name(&source_id) else {
                continue;
            };
            if linked.iter().any(|n| same_note_name(n, name))
                && seen.insert((page_id.clone(), name.to_lowercase()))
            {
                add(&page_id, &title, &page_space);
            }
        }
    }
    // The notes that link to the sources' notes: pages citing a memory
    // whose text links to one.
    if !own.is_empty() {
        // The name alone: a link can name the note's folder first.
        let probes: Vec<String> = own.iter().map(|name| name.to_ascii_lowercase()).collect();
        let mut seen: HashSet<(String, String)> = HashSet::new();
        for (page_id, title, page_space, text) in db.notes_citing_text_with(&probes).await? {
            for name in linked_note_names(&text) {
                if own.iter().any(|n| same_note_name(n, &name))
                    && seen.insert((page_id.clone(), name.to_lowercase()))
                {
                    add(&page_id, &title, &page_space);
                }
            }
        }
    }
    // Stable: equally linked notes keep the newest-first order they came in.
    found.sort_by_key(|entry| std::cmp::Reverse(entry.3));
    Ok(found
        .into_iter()
        .map(|(page_id, title, page_space, _)| (page_id, title, page_space))
        .collect())
}

/// Whether lowercased `text` names the page `title` as a whole phrase. A
/// title of under four characters (an initialism, a one-word stub) counts
/// for nothing: too many texts contain it by chance.
fn names_in_text(text: &str, title: &str) -> bool {
    let wanted = title.trim().to_lowercase();
    if wanted.chars().count() < 4 {
        return false;
    }
    !whole_phrase_starts(text, &wanted).is_empty()
}

/// Where `wanted` stands in `text` as a whole phrase, by byte offset.
fn whole_phrase_starts(text: &str, wanted: &str) -> Vec<usize> {
    let mut out = Vec::new();
    if wanted.is_empty() {
        return out;
    }
    let mut from = 0;
    while let Some(at) = text[from..].find(wanted) {
        let start = from + at;
        let end = start + wanted.len();
        let before = text[..start].chars().next_back();
        let after = text[end..].chars().next();
        let edge = |c: Option<char>| c.is_none_or(|c| !c.is_alphanumeric() || !c.is_ascii());
        if edge(before) && edge(after) {
            out.push(start);
        }
        from = start + wanted.chars().next().map_or(1, char::len_utf8);
    }
    out
}

/// Whether the sources' `raw` text names the one-word English page `title`
/// only in passing: once, and in lower case, as a common word ("bought a
/// garden hose" in a note on finances) rather than a name ("Oslo"). Such a
/// page is still offered when it shares an entity with the sources.
fn named_only_in_passing(raw: &str, title: &str) -> bool {
    let title = title.trim();
    if !title.is_ascii() || title.contains(|c: char| !c.is_ascii_alphanumeric()) {
        return false;
    }
    // ASCII lowercasing keeps every byte offset, so a mention found in the
    // lowered text sits at the same place in `raw`.
    let lowered = raw.to_ascii_lowercase();
    let starts = whole_phrase_starts(&lowered, &title.to_ascii_lowercase());
    starts.len() == 1
        && !raw[starts[0]..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_uppercase())
}

/// An entity `source_ids` mention whose entity page has the same strict title,
/// as `(entity id, its title)`.
async fn strict_entity_titled(
    db: &MemoryDB,
    source_ids: &[String],
    title: &str,
) -> Result<Option<(String, String)>, WenlanError> {
    let key = strict_title_key(title);
    if key.is_empty() {
        return Ok(None);
    }
    let mut entity_ids: Vec<String> = db
        .source_entity_links(source_ids)
        .await?
        .into_iter()
        .map(|(_, entity, _)| entity)
        .collect();
    entity_ids.sort();
    entity_ids.dedup();
    Ok(db
        .entity_page_titles(&entity_ids)
        .await?
        .into_iter()
        .find(|(_, entity_title)| strict_title_key(entity_title) == key))
}

/// `name` with each word written as `title` writes it, ASCII case aside:
/// ("kitchen renovation", "Kitchen Renovation 2023") gives "Kitchen
/// Renovation". Only ASCII case changes, so the result still equals `name`
/// to SQL `LOWER`.
fn cased_like(name: &str, title: &str) -> String {
    name.split(' ')
        .map(|word| {
            title
                .split(|c: char| c.is_whitespace())
                .find(|t| t.eq_ignore_ascii_case(word))
                .unwrap_or(word)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The entities most of `source_ids` mention: each linked to more than half
/// of them, the most-linked first.
async fn majority_entities(
    db: &MemoryDB,
    source_ids: &[String],
) -> Result<Vec<String>, WenlanError> {
    if source_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for (_, entity_id, _) in db.source_entity_links(source_ids).await? {
        *counts.entry(entity_id).or_default() += 1;
    }
    let mut majority: Vec<(String, usize)> = counts
        .into_iter()
        .filter(|(_, count)| count * 2 > source_ids.len())
        .collect();
    majority.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    Ok(majority
        .into_iter()
        .map(|(entity_id, _)| entity_id)
        .collect())
}

/// Land a small model's answers to a claim's organize tasks, each through
/// the lane's own parser and version-checked commit. Answered tasks leave
/// the claim; a repeated or unknown `n` is reported, never applied twice.
/// When the claim is not Space-locked and classification filed every source
/// into one Space, the page is born there, as when the sources already were.
pub async fn organize_new(
    db: &MemoryDB,
    token: &str,
    answers: &[SteepCodeAnswer],
) -> Result<SteepOrganizeCommitResponse, SteepError> {
    let (taken, unknown, sources, scope, topic) = {
        let mut holds = holds();
        holds.expire(Instant::now());
        let claim = holds.by_token.get_mut(token).ok_or(SteepError::TokenGone)?;
        claim.until = Instant::now() + NEW_PAGE_HOLD;
        let mut taken: Vec<(OrganizeTask, String)> = Vec::new();
        let mut unknown: Vec<u32> = Vec::new();
        for answer in answers {
            match claim.organize.iter().position(|task| task.n == answer.n) {
                Some(index) => taken.push((claim.organize.remove(index), answer.answer.clone())),
                None => unknown.push(answer.n),
            }
        }
        (
            taken,
            unknown,
            claim.sources.clone(),
            claim.scope.clone(),
            claim.topic.clone(),
        )
    };
    let source_ids: Vec<String> = sources.iter().map(|(id, _)| id.clone()).collect();

    // The answers may create the very entities they confirm, so compare
    // against every entity confirmed before them.
    let confirmed_before: HashSet<String> = db.confirmed_entity_ids().await?;
    let mut out = SteepOrganizeCommitResponse {
        unknown,
        ..Default::default()
    };
    for (task, answer) in &taken {
        match crate::steep_organize::apply(db, task, answer).await? {
            crate::steep_organize::Applied::Saved => match task.input {
                crate::steep_organize::OrganizeInput::Classify(_) => out.classified += 1,
                crate::steep_organize::OrganizeInput::Entities(_) => out.detected += 1,
            },
            crate::steep_organize::Applied::Unreadable => out.failed += 1,
            crate::steep_organize::Applied::Stale => out.stale += 1,
        }
    }
    let links = db.source_entity_links(&source_ids).await?;
    let linked: HashSet<&str> = links.iter().map(|(memory, _, _)| memory.as_str()).collect();
    out.linked = u32::try_from(linked.len()).unwrap_or(u32::MAX);
    let confirmed_after: HashSet<&str> = links
        .iter()
        .filter(|(_, _, confirmed)| *confirmed)
        .map(|(_, entity, _)| entity.as_str())
        .collect();
    out.confirmed = u32::try_from(
        confirmed_after
            .iter()
            .filter(|entity| !confirmed_before.contains(**entity))
            .count(),
    )
    .unwrap_or(u32::MAX);

    // Classification may file unfiled sources into a Space. A claim that is
    // not Space-locked follows its sources, as it would have if they had
    // been filed before the claim.
    let mut space = None;
    if scope == ReadScope::Global {
        let mut spaces = Vec::with_capacity(source_ids.len());
        for id in &source_ids {
            spaces.push(db.get_memory_space(id).await?);
        }
        let first = spaces.first().cloned().flatten();
        space = Some(if spaces.iter().all(|s| *s == first) {
            first
        } else {
            None
        });
    }
    let space_now = {
        let mut holds = holds();
        let claim = holds.by_token.get_mut(token).ok_or(SteepError::TokenGone)?;
        if let Some(space) = space {
            claim.space = space;
        }
        out.remaining = u32::try_from(claim.organize.len()).unwrap_or(u32::MAX);
        claim.space.clone()
    };
    out.existing_page = existing_page(
        db,
        &scope,
        space_now.as_deref(),
        &source_ids,
        topic.as_deref(),
    )
    .await;
    out.link_targets = link_targets(
        db,
        &scope,
        space_now.as_deref(),
        &sources,
        out.existing_page.as_ref().map(|page| page.page_id.as_str()),
    )
    .await;
    log::info!(
        "[steep-new] organized {} classified, {} detected, {} unreadable, {} stale, {} linked of {} sources",
        out.classified,
        out.detected,
        out.failed,
        out.stale,
        out.linked,
        source_ids.len()
    );
    Ok(out)
}

/// What a new-page commit did.
#[derive(Debug, Clone)]
pub struct NewPageCommit {
    /// `created`, `merged`, `attached` or `released`.
    pub outcome: &'static str,
    pub page_id: Option<String>,
    pub title: Option<String>,
    pub citations: Option<String>,
    pub link_targets: Vec<String>,
}

/// Give a claim's items back without writing a note.
pub fn release_new(token: &str) -> Result<NewPageCommit, SteepError> {
    match holds().drop_token(token) {
        Some(_) => Ok(NewPageCommit {
            outcome: "released",
            page_id: None,
            title: None,
            citations: None,
            link_targets: Vec::new(),
        }),
        None => Err(SteepError::TokenGone),
    }
}

/// The claim's source ids, once every source reads as it did at the claim
/// and is still in no note. Otherwise the claim is dropped.
async fn sources_as_claimed(
    db: &MemoryDB,
    token: &str,
    claim: &NewClaim,
) -> Result<Vec<String>, SteepError> {
    let ids: Vec<String> = claim.sources.iter().map(|(id, _)| id.clone()).collect();
    let mut now_texts: HashMap<String, String> = HashMap::new();
    for (source_id, _, text) in db.unpaged_texts(&ids).await? {
        let slot = now_texts.entry(source_id).or_default();
        if !slot.is_empty() {
            slot.push_str("\n\n");
        }
        slot.push_str(text.trim());
    }
    for (id, text) in &claim.sources {
        let now: Option<String> = now_texts
            .get(id)
            .map(|t| t.chars().take(SOURCE_CHAR_CAP).collect());
        if now.as_deref() != Some(text.as_str()) {
            holds().drop_token(token);
            return Err(SteepError::Changed(
                "a source changed, was removed, or went into another note since the claim. \
                 Nothing was written; list again and claim again"
                    .into(),
            ));
        }
    }
    Ok(ids)
}

/// Add a claim's sources to its `existing_page` instead of writing a second
/// page on the same subject. The page then waits, out of date with its
/// sources, for a rewrite that uses all of them (steep's own worklist, or
/// Automatic mode). The claim ends either way but on a refusal for no
/// existing page, which keeps it so the caller writes the page instead.
pub async fn attach_new(
    db: &MemoryDB,
    token: &str,
    agent: &str,
) -> Result<NewPageCommit, SteepError> {
    let claim = {
        let mut holds = holds();
        holds.expire(Instant::now());
        holds.by_token.get(token).cloned()
    }
    .ok_or(SteepError::TokenGone)?;
    let source_ids: Vec<String> = claim.sources.iter().map(|(id, _)| id.clone()).collect();
    let Some(page) = existing_page(
        db,
        &claim.scope,
        claim.space.as_deref(),
        &source_ids,
        claim.topic.as_deref(),
    )
    .await
    else {
        return Err(SteepError::Invalid(
            "no page on this subject exists in this Space: write the page with action \
             \"write\" and the same token"
                .into(),
        ));
    };
    let ids = sources_as_claimed(db, token, &claim).await?;
    attach_sources(db, token, &ids, page, agent).await
}

/// Add `ids` to `page`, end the claim, and say so.
async fn attach_sources(
    db: &MemoryDB,
    token: &str,
    ids: &[String],
    page: SteepExistingPage,
    agent: &str,
) -> Result<NewPageCommit, SteepError> {
    let claim = renew(token)?;
    let result = page_write(
        db,
        PageWrite::Attach {
            page_id: &page.page_id,
            source_memory_ids: ids,
            link_reason: "steep_attach",
            agent,
        },
    )
    .await;
    holds().drop_token(token);
    result?;
    let saved = db.get_page(&page.page_id).await?;
    let content = saved
        .as_ref()
        .map(|saved| saved.content.as_str())
        .unwrap_or_default();
    let page_space = saved
        .as_ref()
        .and_then(|saved| saved.workspace.as_deref().or(saved.space.as_deref()));
    let link_sources = match saved.as_ref() {
        Some(saved) => match db
            .get_memory_contents_by_ids(&saved.source_memory_ids)
            .await
        {
            Ok(sources) => sources,
            Err(error) => {
                log::warn!("[steep-new] attached-page link sources: {error}");
                claim.sources.clone()
            }
        },
        None => claim.sources.clone(),
    };
    let link_targets = unused_link_targets(
        db,
        &claim.scope,
        page_space,
        &link_sources,
        &page.page_id,
        content,
    )
    .await;
    if let Err(error) = db
        .stamp_last_distilled_at(ids, chrono::Utc::now().timestamp())
        .await
    {
        log::warn!("[steep-new] stamp last_distilled_at failed: {error}");
    }
    log::info!(
        "[steep-new] attached {} sources to note {} '{}'",
        ids.len(),
        page.page_id,
        page.title
    );
    Ok(NewPageCommit {
        outcome: "attached",
        page_id: Some(page.page_id),
        title: Some(page.title),
        citations: None,
        link_targets,
    })
}

/// Write one note from a claim. The hold is kept when the citation check
/// refuses the text, so the caller fixes it and commits again.
pub async fn commit_new(
    db: &MemoryDB,
    token: &str,
    title: &str,
    content: &str,
    agent: &str,
    knowledge_path: Option<&Path>,
) -> Result<NewPageCommit, SteepError> {
    let claim = {
        let mut holds = holds();
        holds.expire(Instant::now());
        holds.by_token.get(token).cloned()
    }
    .ok_or(SteepError::TokenGone)?;

    let title = title.trim();
    if title.is_empty() {
        return Err(SteepError::Invalid("title is empty".into()));
    }
    if title.chars().count() > MAX_TITLE_CHARS {
        return Err(SteepError::Invalid(format!(
            "title is longer than {MAX_TITLE_CHARS} characters"
        )));
    }
    let content = content.trim();
    if content.is_empty() {
        return Err(SteepError::Invalid("content is empty".into()));
    }
    crate::export::provenance::validate_canonical_page_content(content)?;
    let ids = sources_as_claimed(db, token, &claim).await?;

    // A note with this title already is this subject's page: the sources go
    // into it, as `attach` does, instead of a second page under the same
    // title. The claim's Space first; else another Space the caller reads
    // (a claim whose sources classification filed apart has no Space).
    if let Some(page) =
        strict_same_title_note(db, &claim.scope, claim.space.as_deref(), title, false).await?
    {
        return attach_sources(
            db,
            token,
            &ids,
            SteepExistingPage {
                page_id: page.page_id,
                title: page.title,
            },
            agent,
        )
        .await;
    }

    let kinds = db.resolve_source_kinds(&ids).await.unwrap_or_default();
    let numbered: Vec<crate::citations::NumberedSource> = claim
        .sources
        .iter()
        .enumerate()
        .map(|(i, (id, text))| crate::citations::NumberedSource {
            index: u32::try_from(i + 1).unwrap_or(u32::MAX),
            source_kind: kinds
                .get(id)
                .cloned()
                .unwrap_or_else(|| "memory".to_string()),
            locator: id.clone(),
            text: text.clone(),
        })
        .collect();
    let (body, cites, stats) = crate::citations::process_citation_output(content, &numbered);
    if stats.stripped > 0 {
        return Err(SteepError::CitationGate(format!(
            "the note was not saved: {} citation(s) name a source number that is not in this \
             claim ({}). Use only the numbers [1] to [{}], or drop the sentence; then commit \
             again with the same token",
            stats.stripped,
            stats.summary(),
            numbered.len()
        )));
    }
    if stats.verified == 0 || stats.unverified > stats.verified {
        return Err(SteepError::CitationGate(format!(
            "the note was not saved: too few cited sentences match their source ({}). Cite \
             each claim with the number of the source that says it, like [1], and keep the \
             wording close to the source; then commit again with the same token",
            stats.summary()
        )));
    }
    let citations_json = serde_json::to_string(&cites).map_err(WenlanError::from)?;
    let summary = crate::synthesis::distill::extract_page_summary(&body);
    let tuning = crate::tuning::DistillationConfig::default();

    let space_target = match claim.space.as_deref() {
        Some(space) => WriteSpaceTarget::Named(space.to_string()),
        None => WriteSpaceTarget::Uncategorized,
    };
    // A note most of whose sources mention one entity is about it, as an
    // automatic note from an entity's memories is. Only the first note about
    // an entity says so: the page match folds a new note into an existing
    // note about the same entity, which would drop the text just written. A
    // later note links that one through `link_targets` instead, and is about
    // the next majority entity that has no note yet, if any.
    //
    // An entity the sources mention whose own entity page carries this
    // strict title is this subject already: the note
    // becomes its page, about it and under its title, so the Wiki lists one
    // page (see `entity_row_browse_predicate`, which ignores ASCII case), not
    // two. The entity's words keep the caller's capitals: entity names are
    // often stored lowercase.
    let mut title = title.to_string();
    let mut entity_id = None;
    if let Some((entity, entity_title)) = strict_entity_titled(db, &ids, &title).await? {
        if !db.entity_has_note(&entity).await? {
            entity_id = Some(entity);
            title = cased_like(&entity_title, &title);
        }
    }
    if entity_id.is_none() {
        // Most sources mentioning an entity does not make it the subject:
        // the title must name it, and nothing else of its rank, or the note
        // stays about no entity.
        let majority = majority_entities(db, &ids).await?;
        let names = db.entity_page_titles(&majority).await?;
        for candidate in majority {
            let named = names.iter().any(|(id, name)| {
                *id == candidate && names_sole_subject_with(name, &title, strict_title_key)
            });
            if named && !db.entity_has_note(&candidate).await? {
                entity_id = Some(candidate);
                break;
            }
        }
    }
    let result = page_write(
        db,
        PageWrite::Create {
            page_id: None,
            req: CreateConceptRequest {
                title: title.clone(),
                content: body,
                summary,
                entity_id: entity_id.clone(),
                space: space_target,
                source_memory_ids: ids.clone(),
                creation_kind: Some("distilled".to_string()),
                workspace: claim.space.clone(),
            },
            agent,
            knowledge_path,
            // The claim already held the note to MIN_SOURCES or one long
            // source; the caller's model judged that the items belong together.
            page_min_cluster_size: 1,
            page_match_threshold: tuning.page_match_threshold,
            citations_json: Some(citations_json),
            basis_marks: Vec::new(),
            basis_space_guard: (claim.scope != ReadScope::Global).then(|| claim.scope.clone()),
            skip_protected_title_match: false,
        },
    )
    .await;
    holds().drop_token(token);
    let result = result?;
    // A note written about an entity cites it, and a cited entity has earned
    // its Wiki page, as for an automatic note (`synthesis::distill`). Runs for
    // a merge too: the page that took the sources cites the entity as much.
    if let Some(entity_id) = entity_id.as_deref() {
        match db.establish_entity_by_citation(entity_id).await {
            Ok(true) => log::info!("[steep-new] established entity {entity_id} by citation"),
            Ok(false) => {}
            Err(error) => {
                log::warn!("[steep-new] establish by citation failed for {entity_id}: {error}")
            }
        }
    }
    if let Err(error) = db
        .stamp_last_distilled_at(&ids, chrono::Utc::now().timestamp())
        .await
    {
        log::warn!("[steep-new] stamp last_distilled_at failed: {error}");
    }
    let (outcome, page_id) = match result.attached_to {
        Some(existing) => ("merged", existing),
        None => ("created", result.id),
    };
    let saved = db.get_page(&page_id).await.ok().flatten();
    let page_title = saved
        .as_ref()
        .map(|page| page.title.clone())
        .unwrap_or_else(|| title.to_string());
    let saved_content = saved
        .as_ref()
        .map(|page| page.content.as_str())
        .unwrap_or_default();
    let page_space = saved
        .as_ref()
        .and_then(|page| page.workspace.as_deref().or(page.space.as_deref()));
    let link_sources = match saved.as_ref() {
        Some(saved) => match db
            .get_memory_contents_by_ids(&saved.source_memory_ids)
            .await
        {
            Ok(sources) => sources,
            Err(error) => {
                log::warn!("[steep-new] committed-page link sources: {error}");
                claim.sources.clone()
            }
        },
        None => claim.sources.clone(),
    };
    let link_targets = unused_link_targets(
        db,
        &claim.scope,
        page_space,
        &link_sources,
        &page_id,
        saved_content,
    )
    .await;
    log::info!(
        "[steep-new] {outcome} note {page_id} '{page_title}' from {} sources ({})",
        ids.len(),
        stats.summary()
    );
    Ok(NewPageCommit {
        outcome,
        page_id: Some(page_id),
        title: Some(page_title),
        citations: Some(stats.summary()),
        link_targets,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_title_joining_two_names_has_no_sole_subject() {
        let names = |name, title| names_sole_subject_with(name, title, strict_title_key);
        assert!(!names("Lisbon", "Lisbon and Porto 2026"));
        assert!(!names("Billing", "Billing & Onboarding"));
        assert!(!names("里斯本", "里斯本 和 波爾圖"));
        assert!(names("Lisbon", "Lisbon 2026"));
        assert!(names("Portugal", "Portugal trip and food"));
        assert!(names("Mira", "Mira's kiln"));
        assert!(names("Rock and Roll", "Rock and Roll"));
        assert!(!names("Lisbon", "Porto"));
    }

    #[test]
    fn a_title_names_a_note_in_text_only_as_a_whole_phrase() {
        let text = "we fired the kiln in oslo; glaze recipes are in the red notebook";
        assert!(names_in_text(text, "Glaze recipes"));
        assert!(names_in_text(text, "Oslo"));
        assert!(!names_in_text(text, "Glaze recipe book"));
        assert!(!names_in_text(text, "Kil"), "under four characters");
        assert!(!names_in_text("oslofjord trips", "Oslo"));
    }

    #[test]
    fn a_one_word_title_named_once_in_lower_case_is_only_in_passing() {
        let finances = "Paid the water bill. Bought a garden hose on the card.";
        assert!(named_only_in_passing(finances, "garden"));
        assert!(named_only_in_passing(finances, "Garden"));
        assert!(!named_only_in_passing(
            "The garden needs mulch. Garden beds go in by May; the garden faces south.",
            "Garden"
        ));
        assert!(!named_only_in_passing(
            "Mira fires the kiln in Oslo.",
            "Oslo"
        ));
        assert!(!named_only_in_passing(finances, "Water bill"), "two words");
        assert!(
            !named_only_in_passing("走過花園 garden", "花園"),
            "not English"
        );
    }

    #[test]
    fn chunks_of_a_note_fold_into_one_item() {
        assert_eq!(
            item_key("vault1::notes/bread.md::0"),
            "vault1::notes/bread.md"
        );
        assert_eq!(
            item_key("vault1::notes/bread.md::12"),
            "vault1::notes/bread.md"
        );
        assert_eq!(item_key("import_chatgpt_abc::1"), "import_chatgpt_abc::1");
        assert_eq!(item_key("mem_123"), "mem_123");
        assert_eq!(item_key("folder::a::b.md"), "folder::a::b.md");
    }

    #[test]
    fn kinds_follow_where_the_material_came_from() {
        assert_eq!(
            item_kind("import_chatgpt_x", None, false),
            STEEP_NEW_KIND_CHAT
        );
        assert_eq!(
            item_kind("v::a.md::0", Some("obsidian"), false),
            STEEP_NEW_KIND_NOTE
        );
        assert_eq!(
            item_kind("f::a.md::0", Some("folder"), true),
            STEEP_NEW_KIND_FILE
        );
        assert_eq!(
            item_kind("mem_1", Some("claude-code"), false),
            STEEP_NEW_KIND_CAPTURE
        );
    }

    #[test]
    fn titles_drop_heading_paths_and_fall_back_to_the_file_name() {
        assert_eq!(item_title(Some("Bread > Starter"), "k", ""), "Bread");
        assert_eq!(
            item_title(None, "v::notes/Home network.md", ""),
            "Home network"
        );
        assert_eq!(
            item_title(None, "mem_1", "\n# Kyoto trip\nday one"),
            "Kyoto trip"
        );
    }

    #[test]
    fn an_entity_title_keeps_the_callers_capitals() {
        assert_eq!(
            cased_like("kitchen renovation", "Kitchen Renovation 2023"),
            "Kitchen Renovation"
        );
        assert_eq!(cased_like("garden", "Garden"), "Garden");
        assert_eq!(cased_like("garden", "garden"), "garden");
        assert_eq!(cased_like("Mira Chen", "mira chen"), "mira chen");
        // A word the title does not write as such keeps the entity's spelling.
        assert_eq!(cased_like("rock-n-roll", "Rock N Roll"), "rock-n-roll");
        assert_eq!(cased_like("廚房裝修", "廚房裝修2023"), "廚房裝修");
    }

    #[test]
    fn close_titles_name_one_subject() {
        assert_eq!(
            loose_title_key("Kitchen Renovation 2023"),
            "kitchen renovation"
        );
        assert_eq!(loose_title_key("Wenlan v2 notes"), "wenlan notes");
        assert_eq!(loose_title_key("廚房裝修2023"), "廚房裝修");
        let close = |a, b| loose_title_closeness(a, b) >= TITLE_CLOSE_ENOUGH;
        assert!(close("Kitchen Renovation 2023", "Kitchen renovation"));
        assert!(close("Portugal Trip", "Portugal trip and food"));
        assert!(close("Lisbon and Porto trip", "Porto and Lisbon"));
        assert!(close("廚房裝修", "廚房裝修筆記 2024"));
        assert!(!close("Kiln glazes", "Kiln renovation"));
        assert!(!close("Portugal Trip and Food", "Lisbon and Porto"));
        assert!(!close("Notes", "My notes"));
    }

    #[test]
    fn snippets_flatten_and_cut() {
        let long = "word ".repeat(200);
        let s = snippet(&long);
        assert!(s.ends_with('…'));
        assert!(s.chars().count() <= SNIPPET_CHARS + 1);
        assert_eq!(snippet("a\n\n b"), "a b");
    }
}

#[cfg(test)]
#[path = "steep_new_test.rs"]
mod steep_new_test;
