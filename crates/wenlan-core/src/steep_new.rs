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
use wenlan_types::steep::SteepSource;
use wenlan_types::steep_new::{
    SteepNewItem, STEEP_NEW_KIND_CAPTURE, STEEP_NEW_KIND_CHAT, STEEP_NEW_KIND_FILE,
    STEEP_NEW_KIND_NOTE,
};

use crate::db::MemoryDB;
use crate::error::WenlanError;
use crate::post_write::{page_write, PageWrite};
use crate::read_scope::ReadScope;
use crate::steep::SteepError;

/// How long a claim holds its items: long enough for a main model to write one
/// note.
pub const NEW_PAGE_HOLD: Duration = Duration::from_secs(15 * 60);

/// Characters of an item's text the list shows.
pub const SNIPPET_CHARS: usize = 280;

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
pub const MIN_SINGLE_SOURCE_CHARS: usize = 800;

/// Longest title a note may get.
const MAX_TITLE_CHARS: usize = 120;

/// One claim, as handed out.
#[derive(Debug, Clone)]
struct NewClaim {
    item_ids: Vec<String>,
    /// `(memory source id, text)` in citation order: `[n]` is `sources[n - 1]`.
    sources: Vec<(String, String)>,
    space: Option<String>,
    scope: ReadScope,
    until: Instant,
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
    Ok(NewList {
        items: all[start..end].iter().map(|m| m.item.clone()).collect(),
        total,
        next_cursor: (end < total).then(|| end.to_string()),
        kinds,
        held: all.iter().filter(|m| m.item.held).count(),
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
}

/// What a claim hands out.
#[derive(Debug, Clone)]
pub struct NewPageClaim {
    pub token: String,
    pub held_until: i64,
    pub sources: Vec<SteepSource>,
    pub skipped: Vec<String>,
}

/// Hold `item_ids` (one topic) for one note and number their text.
pub async fn claim_new(
    db: &MemoryDB,
    scope: &ReadScope,
    item_ids: &[String],
) -> Result<NewPageClaim, SteepError> {
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
    let material = new_material(db, scope).await?;
    let by_id: HashMap<&str, &NewMaterial> = material
        .iter()
        .map(|m| (m.item.item_id.as_str(), m))
        .collect();

    let now = Instant::now();
    let mut skipped = Vec::new();
    let mut taken: Vec<&NewMaterial> = Vec::new();
    {
        let mut holds = holds();
        holds.expire(now);
        for id in &wanted {
            match by_id.get(id.as_str()) {
                Some(m) if !holds.by_item.contains_key(id) && taken.len() < MAX_CLAIM_ITEMS => {
                    taken.push(m)
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
            "too little to make a note from ({total_chars} characters in one source). Group              it with related items, or leave it for later"
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
                space,
                scope: scope.clone(),
                until,
            },
        );
    }
    let held_until = chrono::Utc::now().timestamp() + NEW_PAGE_HOLD.as_secs() as i64;
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
    })
}

/// What a new-page commit did.
#[derive(Debug, Clone)]
pub struct NewPageCommit {
    /// `created`, `merged` or `released`.
    pub outcome: &'static str,
    pub page_id: Option<String>,
    pub title: Option<String>,
    pub citations: Option<String>,
}

/// Give a claim's items back without writing a note.
pub fn release_new(token: &str) -> Result<NewPageCommit, SteepError> {
    match holds().drop_token(token) {
        Some(_) => Ok(NewPageCommit {
            outcome: "released",
            page_id: None,
            title: None,
            citations: None,
        }),
        None => Err(SteepError::TokenGone),
    }
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

    // Every source must read as it did at the claim and still be in no note.
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
    let result = page_write(
        db,
        PageWrite::Create {
            page_id: None,
            req: CreateConceptRequest {
                title: title.to_string(),
                content: body,
                summary,
                entity_id: None,
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
        },
    )
    .await;
    holds().drop_token(token);
    let result = result?;
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
    let page_title = db
        .get_page(&page_id)
        .await
        .ok()
        .flatten()
        .map(|page| page.title)
        .unwrap_or_else(|| title.to_string());
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
