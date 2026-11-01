// SPDX-License-Identifier: Apache-2.0
//! Things (一件事): one line of work that conversations belong to
//! (UX-THING-1).
//!
//! Migration 149 adds three tables:
//! - `things`: one row per thing. `title` is `NULL` until the AI has a
//!   name; `brief_item_id` links a thing that started as a Space Brief item;
//!   `merged_into` is reserved for merges (UX-THING-3), which only the
//!   person makes.
//! - `thing_sessions`: which conversations (`agent_sessions.id`) are in which
//!   thing, and whether the AI or the person put them there.
//! - `thing_history`: one row per create, rename, assign and unassign, with
//!   who did it, so a correction stays on record for UX-THING-2.
//!
//! The daemon sorts conversations itself and asks nobody
//! ([`MemoryDB::sort_sessions_into_things`]). Activity times are computed on
//! read from the conversations, never stored.

use super::sessions::{last_ai_title, SESSION_COLUMNS};
use super::MemoryDB;
use crate::read_scope::ReadScope;
use crate::WenlanError;
use std::collections::{HashMap, HashSet};
use wenlan_types::{
    AgentSession, BriefAppliedMutation, BriefMutationKind, ThingActor, ThingDetailResponse,
    ThingPageEntry, ThingPageVia, ThingRef, ThingSessionEntry, ThingSummary,
};

const THINGS_DDL: &str = "
CREATE TABLE IF NOT EXISTS things (
    id TEXT PRIMARY KEY,
    title TEXT,
    title_by TEXT NOT NULL CHECK (title_by IN ('ai', 'person')),
    brief_item_id TEXT UNIQUE,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    merged_into TEXT
);
CREATE TABLE IF NOT EXISTS thing_sessions (
    thing_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    assigned_by TEXT NOT NULL CHECK (assigned_by IN ('ai', 'person')),
    at INTEGER NOT NULL,
    PRIMARY KEY (thing_id, session_id)
);
CREATE INDEX IF NOT EXISTS idx_thing_sessions_session ON thing_sessions(session_id);
CREATE TABLE IF NOT EXISTS thing_history (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    ts INTEGER NOT NULL,
    actor TEXT NOT NULL CHECK (actor IN ('ai', 'person')),
    op TEXT NOT NULL CHECK (op IN ('thing.create', 'thing.rename', 'thing.assign',
                                   'thing.unassign')),
    thing_id TEXT NOT NULL,
    session_id TEXT,
    data TEXT NOT NULL DEFAULT '{}'
);
CREATE INDEX IF NOT EXISTS idx_thing_history_thing ON thing_history(thing_id, seq);
";

/// Most conversations one sorting pass takes; the rest wait for the next.
const SORT_BATCH: i64 = 500;
/// Longest thing title, in characters.
pub const MAX_THING_TITLE_CHARS: usize = 200;
/// Most conversations `GET /api/things/{id}` reads pages and memories from.
const THING_DETAIL_SESSIONS: usize = 50;
/// Most pages and memories `GET /api/things/{id}` lists.
const THING_DETAIL_CAP: usize = 200;
/// Branches that say nothing about which line of work a conversation is on.
const DEFAULT_BRANCHES: &[&str] = &["main", "master", "trunk", "develop", "dev", "HEAD"];
/// Words too common to make two titles alike.
const STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "at", "by", "for", "from", "in", "is", "of", "on", "or", "the", "to",
    "with",
];

fn db_err(context: &str) -> impl Fn(libsql::Error) -> WenlanError + '_ {
    move |e| WenlanError::VectorDb(format!("{context}: {e}"))
}

/// Why the daemon put a conversation in a thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortRule {
    /// The conversation added, edited or moved this thing's Brief item.
    Brief,
    /// Same Space, same repository, same non-default branch.
    Branch,
    /// Same Space, a title alike enough.
    Title,
    /// Nothing fit: a new thing named after the conversation.
    New,
}

impl SortRule {
    fn as_str(self) -> &'static str {
        match self {
            Self::Brief => "brief",
            Self::Branch => "branch",
            Self::Title => "title",
            Self::New => "new",
        }
    }
}

/// What one sorting pass did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SortReport {
    /// Things made for active Brief items.
    pub brief_things: usize,
    /// Conversations sorted, by the rule that placed them.
    pub sorted: Vec<(String, SortRule)>,
    /// Things the AI named this pass.
    pub named: usize,
}

/// Where a moved conversation goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoveTarget {
    Thing(String),
    NewThing(String),
}

fn actor_str(actor: ThingActor) -> &'static str {
    match actor {
        ThingActor::Ai => "ai",
        ThingActor::Person => "person",
    }
}

fn parse_actor(raw: &str) -> ThingActor {
    if raw == "person" {
        ThingActor::Person
    } else {
        ThingActor::Ai
    }
}

fn new_thing_id() -> String {
    format!("thg_{}", &uuid::Uuid::new_v4().simple().to_string()[..24])
}

/// A title trimmed, with whitespace collapsed and cut to
/// [`MAX_THING_TITLE_CHARS`]. `None` when nothing is left.
pub fn clean_thing_title(raw: &str) -> Option<String> {
    let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return None;
    }
    Some(collapsed.chars().take(MAX_THING_TITLE_CHARS).collect())
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF)
}

/// A plain plural folded to its singular ("trips" → "trip"), so two titles
/// about the same thing still match.
fn stem(word: &str) -> String {
    match word.strip_suffix('s') {
        Some(stem) if word.chars().count() > 3 && !stem.ends_with('s') => stem.to_string(),
        _ => word.to_string(),
    }
}

/// Lowercased words of a title, without stopwords and with plain plurals
/// folded; each CJK character is a word of its own.
fn title_words(text: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut HashSet<String>| {
        if !word.is_empty() && !STOPWORDS.contains(&word.as_str()) {
            out.insert(stem(word));
        }
        word.clear();
    };
    for c in text.chars() {
        if is_cjk(c) {
            flush(&mut word, &mut out);
            out.insert(c.to_string());
        } else if c.is_alphanumeric() {
            word.extend(c.to_lowercase());
        } else {
            flush(&mut word, &mut out);
        }
    }
    flush(&mut word, &mut out);
    out
}

/// How alike two titles are, from 0 to 1, or `None` when they share too
/// few words to count: at least two, or every word of a one-word title.
pub fn title_likeness(a: &str, b: &str) -> Option<f64> {
    let (a, b) = (title_words(a), title_words(b));
    let shared = a.intersection(&b).count();
    let needed = 2.min(a.len()).min(b.len());
    if a.is_empty() || b.is_empty() || shared < needed.max(1) {
        return None;
    }
    let all = a.union(&b).count();
    Some(shared as f64 / all as f64)
}

/// Titles at least this alike name the same line of work.
const TITLE_LIKENESS: f64 = 0.5;

fn branch_says_something(branch: Option<&str>) -> Option<&str> {
    branch
        .map(str::trim)
        .filter(|b| !b.is_empty() && !DEFAULT_BRANCHES.contains(b))
}

/// The conversation's title: the one stored, else the title Claude Code wrote
/// into its transcript, when that file is still here.
fn session_title(session: &AgentSession) -> Option<String> {
    if let Some(title) = session.title.as_deref().and_then(clean_thing_title) {
        return Some(title);
    }
    if session.tool != "claude-code" {
        return None;
    }
    let path = std::path::Path::new(session.transcript_path.as_deref()?);
    path.is_file()
        .then(|| last_ai_title(path))
        .flatten()
        .and_then(|t| clean_thing_title(&t))
}

async fn history(
    tx: &libsql::Transaction,
    now: i64,
    actor: ThingActor,
    op: &str,
    thing_id: &str,
    session_id: Option<&str>,
    data: serde_json::Value,
) -> Result<(), WenlanError> {
    tx.execute(
        "INSERT INTO thing_history (ts, actor, op, thing_id, session_id, data)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        libsql::params![
            now,
            actor_str(actor),
            op,
            thing_id,
            session_id,
            data.to_string()
        ],
    )
    .await
    .map_err(db_err("thing history"))?;
    Ok(())
}

async fn create_thing(
    tx: &libsql::Transaction,
    now: i64,
    title: Option<&str>,
    actor: ThingActor,
    brief_item_id: Option<&str>,
    rule: &str,
) -> Result<String, WenlanError> {
    let id = new_thing_id();
    tx.execute(
        "INSERT INTO things (id, title, title_by, brief_item_id, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
        libsql::params![id.as_str(), title, actor_str(actor), brief_item_id, now],
    )
    .await
    .map_err(db_err("thing insert"))?;
    history(
        tx,
        now,
        actor,
        "thing.create",
        &id,
        None,
        serde_json::json!({ "title": title, "rule": rule, "brief_item_id": brief_item_id }),
    )
    .await?;
    Ok(id)
}

async fn assign(
    tx: &libsql::Transaction,
    now: i64,
    thing_id: &str,
    session_id: &str,
    actor: ThingActor,
    data: serde_json::Value,
) -> Result<(), WenlanError> {
    let changed = tx
        .execute(
            "INSERT OR IGNORE INTO thing_sessions (thing_id, session_id, assigned_by, at)
             VALUES (?1, ?2, ?3, ?4)",
            libsql::params![thing_id, session_id, actor_str(actor), now],
        )
        .await
        .map_err(db_err("thing assign"))?;
    if changed > 0 {
        history(
            tx,
            now,
            actor,
            "thing.assign",
            thing_id,
            Some(session_id),
            data,
        )
        .await?;
    }
    Ok(())
}

async fn query_strings(
    tx: &libsql::Transaction,
    sql: &str,
    params: Vec<libsql::Value>,
    context: &str,
) -> Result<Vec<String>, WenlanError> {
    let mut rows = tx
        .query(sql, libsql::params_from_iter(params))
        .await
        .map_err(db_err(context))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.map_err(db_err(context))? {
        out.push(row.get::<String>(0).unwrap_or_default());
    }
    Ok(out)
}

fn text_or_null(value: Option<&str>) -> libsql::Value {
    value.map_or(libsql::Value::Null, |v| libsql::Value::Text(v.to_string()))
}

/// The things a conversation that touched these Brief items belongs to.
async fn brief_things_of(
    tx: &libsql::Transaction,
    session_id: &str,
) -> Result<Vec<String>, WenlanError> {
    let mutations = query_strings(
        tx,
        "SELECT mutations_json FROM brief_history WHERE session_id = ?1",
        vec![libsql::Value::Text(session_id.to_string())],
        "thing sort brief history",
    )
    .await?;
    let mut items: Vec<String> = Vec::new();
    for json in mutations {
        let applied: Vec<BriefAppliedMutation> = serde_json::from_str(&json).unwrap_or_default();
        for mutation in applied {
            if mutation.kind == BriefMutationKind::Summary {
                continue;
            }
            if let Some(item) = mutation.item_id {
                if !items.contains(&item) {
                    items.push(item);
                }
            }
        }
    }
    let mut things = Vec::new();
    for item in items {
        things.extend(
            query_strings(
                tx,
                "SELECT id FROM things WHERE brief_item_id = ?1 AND merged_into IS NULL",
                vec![libsql::Value::Text(item)],
                "thing sort brief thing",
            )
            .await?,
        );
    }
    Ok(things)
}

/// The thing of the newest conversation on the same repository branch in the
/// same Space.
async fn branch_thing_of(
    tx: &libsql::Transaction,
    session: &AgentSession,
) -> Result<Option<String>, WenlanError> {
    let (Some(root), Some(branch)) = (
        session.repo_root.as_deref(),
        branch_says_something(session.repo_branch.as_deref()),
    ) else {
        return Ok(None);
    };
    Ok(query_strings(
        tx,
        "SELECT ts.thing_id
           FROM thing_sessions ts
           JOIN agent_sessions a ON a.id = ts.session_id
           JOIN things t ON t.id = ts.thing_id
          WHERE t.merged_into IS NULL AND a.id <> ?1
            AND a.repo_root = ?2 AND a.repo_branch = ?3 AND a.space IS ?4
          ORDER BY a.last_write_at DESC, ts.thing_id
          LIMIT 1",
        vec![
            libsql::Value::Text(session.id.clone()),
            libsql::Value::Text(root.to_string()),
            libsql::Value::Text(branch.to_string()),
            text_or_null(session.space.as_deref()),
        ],
        "thing sort branch",
    )
    .await?
    .into_iter()
    .next())
}

/// The named thing in the same Space whose title is most like `title`.
async fn title_thing_of(
    tx: &libsql::Transaction,
    space: Option<&str>,
    title: &str,
) -> Result<Option<String>, WenlanError> {
    let mut rows = tx
        .query(
            "SELECT t.id, t.title FROM things t
              WHERE t.merged_into IS NULL AND t.title IS NOT NULL
                AND (EXISTS (SELECT 1 FROM thing_sessions ts
                               JOIN agent_sessions a ON a.id = ts.session_id
                              WHERE ts.thing_id = t.id AND a.space IS ?1)
                     OR EXISTS (SELECT 1 FROM brief_items bi
                                  JOIN spaces sp ON sp.id = bi.space_id
                                 WHERE bi.id = t.brief_item_id AND sp.name IS ?1))
              ORDER BY t.created_at, t.id",
            libsql::params_from_iter(vec![text_or_null(space)]),
        )
        .await
        .map_err(db_err("thing sort title"))?;
    let mut best: Option<(f64, String)> = None;
    while let Some(row) = rows.next().await.map_err(db_err("thing sort title row"))? {
        let id = row.get::<String>(0).unwrap_or_default();
        let candidate = row.get::<String>(1).unwrap_or_default();
        let Some(score) = title_likeness(title, &candidate) else {
            continue;
        };
        if score >= TITLE_LIKENESS && best.as_ref().is_none_or(|(b, _)| score > *b) {
            best = Some((score, id));
        }
    }
    Ok(best.map(|(_, id)| id))
}

impl MemoryDB {
    /// Migration 149: `things`, `thing_sessions` and `thing_history`. The
    /// tables are `IF NOT EXISTS`, so a crash before the version bump replays
    /// cleanly. They start empty; the first sorting pass fills them.
    pub(super) async fn migrate_149_things(&self, prior_version: i64) -> Result<(), WenlanError> {
        self.backup_before_migration(149, prior_version).await?;
        let conn = self.conn.lock().await;
        conn.execute_batch(THINGS_DDL)
            .await
            .map_err(db_err("m149 things DDL"))?;
        conn.execute("PRAGMA user_version = 149", ())
            .await
            .map_err(db_err("m149 bump"))?;
        log::info!("[migration] Migration 149 applied: things");
        Ok(())
    }

    /// Sort every conversation that is in no thing yet, and name the AI's
    /// unnamed things. Nobody is asked (UX-THING-1). First every active Brief
    /// item gets a thing; then each conversation, oldest write first, goes to
    /// the first of:
    /// 1. the things of the Brief items it added, edited or moved;
    /// 2. the thing of the newest conversation on the same repository and
    ///    non-default branch in the same Space;
    /// 3. the named thing in the same Space with the most similar title;
    /// 4. a new thing named after the conversation (unnamed until it has a
    ///    title).
    ///
    /// Idempotent: a pass with nothing new changes nothing.
    pub async fn sort_sessions_into_things(&self) -> Result<SortReport, WenlanError> {
        let mut report = SortReport::default();
        // Read what needs sorting, then read transcript titles without the
        // lock held.
        let (unsorted, unnamed) = {
            let conn = self.conn.lock().await;
            let mut rows = conn
                .query(
                    &format!(
                        "SELECT {SESSION_COLUMNS} FROM agent_sessions s
                          WHERE NOT EXISTS (SELECT 1 FROM thing_sessions ts
                                              JOIN things t ON t.id = ts.thing_id
                                             WHERE ts.session_id = s.id
                                               AND t.merged_into IS NULL)
                          ORDER BY s.last_write_at, s.id
                          LIMIT ?1"
                    ),
                    libsql::params![SORT_BATCH],
                )
                .await
                .map_err(db_err("thing sort sessions"))?;
            let mut unsorted = Vec::new();
            while let Some(row) = rows
                .next()
                .await
                .map_err(db_err("thing sort session row"))?
            {
                unsorted.push(super::sessions::session_from_row(&row));
            }
            drop(rows);
            let mut rows = conn
                .query(
                    &format!(
                        "SELECT ts.thing_id, {}
                           FROM things t
                           JOIN thing_sessions ts ON ts.thing_id = t.id
                           JOIN agent_sessions s ON s.id = ts.session_id
                          WHERE t.title IS NULL AND t.title_by = 'ai'
                            AND t.merged_into IS NULL
                          ORDER BY s.first_write_at, s.id",
                        super::sessions::prefixed_columns("s")
                    ),
                    (),
                )
                .await
                .map_err(db_err("thing sort unnamed"))?;
            let mut unnamed: Vec<(String, AgentSession)> = Vec::new();
            while let Some(row) = rows
                .next()
                .await
                .map_err(db_err("thing sort unnamed row"))?
            {
                let thing = row.get::<String>(0).unwrap_or_default();
                unnamed.push((thing, super::sessions::session_from_row_at(&row, 1)));
            }
            (unsorted, unnamed)
        };
        let (unsorted, unnamed) = tokio::task::spawn_blocking(move || {
            let unsorted: Vec<(AgentSession, Option<String>)> = unsorted
                .into_iter()
                .map(|s| {
                    let title = session_title(&s);
                    (s, title)
                })
                .collect();
            let unnamed: Vec<(String, Option<String>)> = unnamed
                .into_iter()
                .map(|(thing, s)| (thing, session_title(&s)))
                .collect();
            (unsorted, unnamed)
        })
        .await
        .map_err(|e| WenlanError::Generic(format!("thing sort titles: {e}")))?;

        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(libsql::TransactionBehavior::Immediate)
            .await
            .map_err(db_err("thing sort begin"))?;

        let mut rows = tx
            .query(
                "SELECT bi.id, bi.text FROM brief_items bi
                  WHERE bi.state = 'active'
                    AND NOT EXISTS (SELECT 1 FROM things t WHERE t.brief_item_id = bi.id)
                  ORDER BY bi.added_at, bi.id",
                (),
            )
            .await
            .map_err(db_err("thing sort brief items"))?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().await.map_err(db_err("thing sort brief row"))? {
            items.push((
                row.get::<String>(0).unwrap_or_default(),
                row.get::<String>(1).unwrap_or_default(),
            ));
        }
        drop(rows);
        for (item, text) in items {
            let title = clean_thing_title(&text);
            create_thing(
                &tx,
                now,
                title.as_deref(),
                ThingActor::Ai,
                Some(&item),
                "brief",
            )
            .await?;
            report.brief_things += 1;
        }

        for (session, title) in unsorted {
            let already = query_strings(
                &tx,
                "SELECT ts.thing_id FROM thing_sessions ts JOIN things t ON t.id = ts.thing_id
                  WHERE ts.session_id = ?1 AND t.merged_into IS NULL LIMIT 1",
                vec![libsql::Value::Text(session.id.clone())],
                "thing sort recheck",
            )
            .await?;
            if !already.is_empty() {
                continue;
            }
            let brief = brief_things_of(&tx, &session.id).await?;
            let (targets, rule) = if !brief.is_empty() {
                (brief, SortRule::Brief)
            } else if let Some(thing) = branch_thing_of(&tx, &session).await? {
                (vec![thing], SortRule::Branch)
            } else if let Some(thing) = match title.as_deref() {
                Some(title) => title_thing_of(&tx, session.space.as_deref(), title).await?,
                None => None,
            } {
                (vec![thing], SortRule::Title)
            } else {
                let thing =
                    create_thing(&tx, now, title.as_deref(), ThingActor::Ai, None, "new").await?;
                (vec![thing], SortRule::New)
            };
            for thing in &targets {
                assign(
                    &tx,
                    now,
                    thing,
                    &session.id,
                    ThingActor::Ai,
                    serde_json::json!({ "rule": rule.as_str() }),
                )
                .await?;
            }
            report.sorted.push((session.id.clone(), rule));
        }

        let mut named: HashSet<String> = HashSet::new();
        for (thing, title) in unnamed {
            let Some(title) = title else { continue };
            if named.contains(&thing) {
                continue;
            }
            let changed = tx
                .execute(
                    "UPDATE things SET title = ?1, updated_at = ?2
                      WHERE id = ?3 AND title IS NULL AND title_by = 'ai'",
                    libsql::params![title.as_str(), now, thing.as_str()],
                )
                .await
                .map_err(db_err("thing name"))?;
            if changed > 0 {
                history(
                    &tx,
                    now,
                    ThingActor::Ai,
                    "thing.rename",
                    &thing,
                    None,
                    serde_json::json!({ "before": null, "after": title }),
                )
                .await?;
                named.insert(thing);
            }
        }
        report.named = named.len();
        tx.commit().await.map_err(db_err("thing sort commit"))?;
        Ok(report)
    }

    /// Every thing a scoped caller may see, newest activity first, or just
    /// the one `only` names. A thing is listed while it has a conversation or
    /// its Brief item is active, and never once merged. A Space-scoped caller
    /// sees a thing when one of its conversations ran in that Space or its
    /// Brief item is there, and sees no other Space named.
    pub async fn list_things(
        &self,
        scope: &ReadScope,
        only: Option<&str>,
    ) -> Result<Vec<ThingSummary>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut params = Vec::new();
        let only_sql = match only {
            Some(id) => {
                params.push(libsql::Value::Text(id.to_string()));
                " AND t.id = ?1"
            }
            None => "",
        };
        let mut rows = conn
            .query(
                &format!(
                    "SELECT t.id, t.title, t.title_by, t.created_at, bi.text, bi.state, sp.name,
                            (SELECT MAX(a.last_write_at) FROM thing_sessions ts
                               JOIN agent_sessions a ON a.id = ts.session_id
                              WHERE ts.thing_id = t.id),
                            (SELECT COUNT(*) FROM thing_sessions ts WHERE ts.thing_id = t.id)
                       FROM things t
                       LEFT JOIN brief_items bi ON bi.id = t.brief_item_id
                       LEFT JOIN spaces sp ON sp.id = bi.space_id
                      WHERE t.merged_into IS NULL{only_sql}"
                ),
                libsql::params_from_iter(params.clone()),
            )
            .await
            .map_err(db_err("things list"))?;
        struct Row {
            summary: ThingSummary,
            item_text: Option<String>,
            unfiled: bool,
        }
        let mut things: Vec<Row> = Vec::new();
        while let Some(row) = rows.next().await.map_err(db_err("things list row"))? {
            let state = row.get::<Option<String>>(5).unwrap_or(None);
            let sessions = row.get::<i64>(8).unwrap_or(0);
            if sessions == 0 && state.as_deref() != Some("active") {
                continue;
            }
            let created_at = row.get::<i64>(3).unwrap_or(0);
            let item_space = row.get::<Option<String>>(6).unwrap_or(None);
            things.push(Row {
                summary: ThingSummary {
                    id: row.get::<String>(0).unwrap_or_default(),
                    title: row.get::<Option<String>>(1).unwrap_or(None),
                    title_by: parse_actor(&row.get::<String>(2).unwrap_or_default()),
                    status_line: None,
                    spaces: item_space.into_iter().collect(),
                    last_activity_at: row
                        .get::<Option<i64>>(7)
                        .unwrap_or(None)
                        .unwrap_or(created_at),
                },
                item_text: row.get::<Option<String>>(4).unwrap_or(None),
                unfiled: false,
            });
        }
        drop(rows);

        let index: HashMap<String, usize> = things
            .iter()
            .enumerate()
            .map(|(i, row)| (row.summary.id.clone(), i))
            .collect();
        let mut rows = conn
            .query(
                &format!(
                    "SELECT DISTINCT ts.thing_id, a.space
                       FROM thing_sessions ts
                       JOIN agent_sessions a ON a.id = ts.session_id
                       JOIN things t ON t.id = ts.thing_id
                      WHERE t.merged_into IS NULL{only_sql}
                      ORDER BY a.space"
                ),
                libsql::params_from_iter(params.clone()),
            )
            .await
            .map_err(db_err("things spaces"))?;
        while let Some(row) = rows.next().await.map_err(db_err("things spaces row"))? {
            let Some(&i) = index.get(&row.get::<String>(0).unwrap_or_default()) else {
                continue;
            };
            match crate::space_context::normalize_unfiled_space(
                row.get::<Option<String>>(1).unwrap_or(None),
            ) {
                Some(space) => {
                    if !things[i].summary.spaces.contains(&space) {
                        things[i].summary.spaces.push(space);
                    }
                }
                None => things[i].unfiled = true,
            }
        }
        drop(rows);

        // Where it stands: the Brief summary as of the newest handoff one of
        // its conversations made, in a Space this caller may see.
        let mut rows = conn
            .query(
                &format!(
                    "SELECT ts.thing_id, sp.name, h.summary
                       FROM thing_sessions ts
                       JOIN things t ON t.id = ts.thing_id
                       JOIN brief_history h ON h.session_id = ts.session_id
                       JOIN spaces sp ON sp.id = h.space_id
                      WHERE t.merged_into IS NULL AND trim(h.summary) <> ''{only_sql}
                      ORDER BY h.created_at DESC, h.brief_version DESC"
                ),
                libsql::params_from_iter(params),
            )
            .await
            .map_err(db_err("things status"))?;
        while let Some(row) = rows.next().await.map_err(db_err("things status row"))? {
            let Some(&i) = index.get(&row.get::<String>(0).unwrap_or_default()) else {
                continue;
            };
            if things[i].summary.status_line.is_some() {
                continue;
            }
            let space = row.get::<String>(1).unwrap_or_default();
            if scope.matches(Some(space.as_str())) {
                things[i].summary.status_line = Some(row.get::<String>(2).unwrap_or_default());
            }
        }
        drop(rows);

        let mut out: Vec<ThingSummary> = things
            .into_iter()
            .filter(|row| match scope {
                ReadScope::Global => true,
                ReadScope::Space(space) => row.summary.spaces.contains(space),
                ReadScope::Uncategorized => row.unfiled,
            })
            .map(|mut row| {
                if row.summary.status_line.is_none() {
                    row.summary.status_line = row.item_text.take();
                }
                row.summary.spaces.sort();
                match scope {
                    ReadScope::Global => {}
                    ReadScope::Space(space) => row.summary.spaces = vec![space.clone()],
                    ReadScope::Uncategorized => row.summary.spaces.clear(),
                }
                row.summary
            })
            .collect();
        out.sort_by(|a, b| {
            b.last_activity_at
                .cmp(&a.last_activity_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(out)
    }

    /// One thing as `list_things` shows it, or `None` when it is unknown,
    /// merged, empty or out of the caller's scope.
    pub async fn get_thing(
        &self,
        id: &str,
        scope: &ReadScope,
    ) -> Result<Option<ThingSummary>, WenlanError> {
        Ok(self.list_things(scope, Some(id)).await?.into_iter().next())
    }

    /// `GET /api/things/{id}`: the thing, its conversations newest first, and
    /// the pages and memories they wrote or read, each conversation read as
    /// its session card is (`get_session_detail`), so a scoped caller sees only
    /// what it may. Page titles still need the route's truth filter.
    pub async fn get_thing_detail(
        &self,
        id: &str,
        scope: &ReadScope,
    ) -> Result<Option<ThingDetailResponse>, WenlanError> {
        let Some(thing) = self.get_thing(id, scope).await? else {
            return Ok(None);
        };
        let session_ids = {
            let conn = self.conn.lock().await;
            let mut rows = conn
                .query(
                    "SELECT ts.session_id FROM thing_sessions ts
                       JOIN agent_sessions a ON a.id = ts.session_id
                      WHERE ts.thing_id = ?1
                      ORDER BY a.last_write_at DESC, a.id",
                    libsql::params![id],
                )
                .await
                .map_err(db_err("thing sessions"))?;
            let mut ids = Vec::new();
            while let Some(row) = rows.next().await.map_err(db_err("thing sessions row"))? {
                ids.push(row.get::<String>(0).unwrap_or_default());
            }
            ids
        };
        let mut sessions = Vec::new();
        let mut pages: Vec<ThingPageEntry> = Vec::new();
        let mut page_index: HashMap<String, usize> = HashMap::new();
        let mut memories = Vec::new();
        let mut seen_memories = HashSet::new();
        for session_id in session_ids {
            let Some(detail) = self.get_session_detail(&session_id, scope).await? else {
                continue;
            };
            if sessions.len() >= THING_DETAIL_SESSIONS {
                break;
            }
            sessions.push(ThingSessionEntry {
                id: detail.session.id.clone(),
                tool: detail.session.tool.clone(),
                title: detail.session.title.clone(),
                last_write_at: detail.session.last_write_at,
            });
            let wrote = detail.pages.into_iter().map(|p| ThingPageEntry {
                id: p.id,
                title: p.title,
                via: ThingPageVia::Wrote,
                at: p.edited_at,
            });
            let read = detail
                .reads
                .into_iter()
                .filter(|r| r.kind == wenlan_types::SessionReadKind::ReadPage)
                .filter_map(|r| {
                    Some(ThingPageEntry {
                        title: r.title?,
                        id: r.label,
                        via: ThingPageVia::Read,
                        at: r.created_at,
                    })
                });
            for entry in wrote.chain(read) {
                match page_index.get(&entry.id) {
                    Some(&i) => {
                        let existing = &mut pages[i];
                        let wrote_now = entry.via == ThingPageVia::Wrote;
                        if wrote_now && existing.via == ThingPageVia::Read {
                            *existing = entry;
                        } else if entry.via == existing.via && entry.at > existing.at {
                            existing.at = entry.at;
                        }
                    }
                    None => {
                        page_index.insert(entry.id.clone(), pages.len());
                        pages.push(entry);
                    }
                }
            }
            for memory in detail.memories {
                if seen_memories.insert(memory.source_id.clone()) {
                    memories.push(memory);
                }
            }
        }
        pages.sort_by(|a, b| b.at.cmp(&a.at).then_with(|| a.id.cmp(&b.id)));
        pages.truncate(THING_DETAIL_CAP);
        memories.sort_by(|a: &wenlan_types::SessionMemoryEntry, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| a.source_id.cmp(&b.source_id))
        });
        memories.truncate(THING_DETAIL_CAP);
        Ok(Some(ThingDetailResponse {
            thing,
            sessions,
            pages,
            memories,
        }))
    }

    /// The things a conversation is in, oldest assignment first.
    pub async fn session_things(&self, session_id: &str) -> Result<Vec<ThingRef>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT t.id, t.title FROM thing_sessions ts
                   JOIN things t ON t.id = ts.thing_id
                  WHERE ts.session_id = ?1 AND t.merged_into IS NULL
                  ORDER BY ts.at, t.id",
                libsql::params![session_id],
            )
            .await
            .map_err(db_err("session things"))?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.map_err(db_err("session things row"))? {
            out.push(ThingRef {
                id: row.get::<String>(0).unwrap_or_default(),
                title: row.get::<Option<String>>(1).unwrap_or(None),
            });
        }
        Ok(out)
    }

    /// The person renames a thing. From then on the AI leaves its name alone.
    pub async fn rename_thing(&self, id: &str, title: &str) -> Result<(), WenlanError> {
        let title = clean_thing_title(title)
            .ok_or_else(|| WenlanError::Validation("thing_title_empty: give it a name".into()))?;
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(libsql::TransactionBehavior::Immediate)
            .await
            .map_err(db_err("thing rename begin"))?;
        let mut rows = tx
            .query(
                "SELECT title FROM things WHERE id = ?1 AND merged_into IS NULL",
                libsql::params![id],
            )
            .await
            .map_err(db_err("thing rename read"))?;
        let before = match rows.next().await.map_err(db_err("thing rename row"))? {
            Some(row) => row.get::<Option<String>>(0).unwrap_or(None),
            None => return Err(WenlanError::NotFound("thing not found".into())),
        };
        drop(rows);
        tx.execute(
            "UPDATE things SET title = ?1, title_by = 'person', updated_at = ?2 WHERE id = ?3",
            libsql::params![title.as_str(), now, id],
        )
        .await
        .map_err(db_err("thing rename"))?;
        history(
            &tx,
            now,
            ThingActor::Person,
            "thing.rename",
            id,
            None,
            serde_json::json!({ "before": before, "after": title }),
        )
        .await?;
        tx.commit().await.map_err(db_err("thing rename commit"))?;
        Ok(())
    }

    /// The person moves a conversation out of `from` into another thing or a
    /// new one they name. Its other things stay. Returns the target's id.
    /// 404 for an unknown conversation or thing, 409 when the conversation
    /// is not in `from`. Moving to `from` itself changes nothing.
    pub async fn move_session_thing(
        &self,
        session_id: &str,
        from: &str,
        to: &MoveTarget,
    ) -> Result<String, WenlanError> {
        let new_title = match to {
            MoveTarget::NewThing(title) => Some(clean_thing_title(title).ok_or_else(|| {
                WenlanError::Validation("thing_title_empty: give the new thing a name".into())
            })?),
            MoveTarget::Thing(_) => None,
        };
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(libsql::TransactionBehavior::Immediate)
            .await
            .map_err(db_err("thing move begin"))?;
        let exists = |sql: &'static str, id: String| {
            let tx = &tx;
            async move {
                Ok::<bool, WenlanError>(
                    !query_strings(tx, sql, vec![libsql::Value::Text(id)], "thing move check")
                        .await?
                        .is_empty(),
                )
            }
        };
        if !exists(
            "SELECT id FROM agent_sessions WHERE id = ?1",
            session_id.into(),
        )
        .await?
        {
            return Err(WenlanError::NotFound("session not found".into()));
        }
        let live_thing = "SELECT id FROM things WHERE id = ?1 AND merged_into IS NULL";
        if !exists(live_thing, from.into()).await? {
            return Err(WenlanError::NotFound("thing not found".into()));
        }
        let member = query_strings(
            &tx,
            "SELECT thing_id FROM thing_sessions WHERE thing_id = ?1 AND session_id = ?2",
            vec![
                libsql::Value::Text(from.to_string()),
                libsql::Value::Text(session_id.to_string()),
            ],
            "thing move member",
        )
        .await?;
        if member.is_empty() {
            return Err(WenlanError::Conflict(
                "session_not_in_thing: this conversation is not in that thing".into(),
            ));
        }
        let target = match (to, new_title) {
            (MoveTarget::Thing(id), _) => {
                if !exists(live_thing, id.clone()).await? {
                    return Err(WenlanError::NotFound("thing not found".into()));
                }
                if id == from {
                    return Ok(id.clone());
                }
                id.clone()
            }
            (MoveTarget::NewThing(_), Some(title)) => {
                create_thing(&tx, now, Some(&title), ThingActor::Person, None, "move").await?
            }
            (MoveTarget::NewThing(_), None) => unreachable!("validated above"),
        };
        tx.execute(
            "DELETE FROM thing_sessions WHERE thing_id = ?1 AND session_id = ?2",
            libsql::params![from, session_id],
        )
        .await
        .map_err(db_err("thing move unassign"))?;
        history(
            &tx,
            now,
            ThingActor::Person,
            "thing.unassign",
            from,
            Some(session_id),
            serde_json::json!({ "to": target }),
        )
        .await?;
        assign(
            &tx,
            now,
            &target,
            session_id,
            ThingActor::Person,
            serde_json::json!({ "rule": "move", "from": from }),
        )
        .await?;
        tx.commit().await.map_err(db_err("thing move commit"))?;
        Ok(target)
    }
}
