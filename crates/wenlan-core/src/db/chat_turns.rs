// SPDX-License-Identifier: Apache-2.0
//! Imported chat turns are conversation text, not the person's memories.
//!
//! A Claude or ChatGPT export lands one `memories` row per `User: … /
//! Assistant: …` turn so the conversation's original stays readable and
//! cited pages keep their sources. Those rows used to look like any other
//! memory: the classification lane typed them `fact`, `preference`,
//! `identity` or `decision`, and every surface that labels memories as the
//! person's showed them that way. A turn is what was said in a chat, not a
//! conclusion the person holds (UX-ADD-4).
//!
//! The flag is `memories.semantic_unit = 'chat_turn'` — the column already
//! names the unit a row's text was cut at, and nothing reads it as a type.
//! New imports set it on insert; [`MemoryDB::flag_imported_chat_turns`]
//! sets it once on rows imported before. The flag is the only change: a
//! flagged row keeps its content, its stored type, its page citations and its
//! topic links, so [`MemoryDB::unflag_imported_chat_turns`] puts it back
//! exactly as it was. Readers give the flag precedence over the stored type:
//! classification skips a flagged row, the review queues leave it out, the
//! readers that pick memories by type leave it out, and confirming or
//! retyping it is refused.

use super::MemoryDB;
use crate::error::WenlanError;
use serde::Serialize;

/// `memories.semantic_unit` value for one imported chat turn.
pub const CHAT_TURN_UNIT: &str = "chat_turn";

/// `app_metadata` marker of the one-time pass over earlier imports.
pub const CHAT_TURNS_FLAGGED_MARKER: &str = "chat_turns_flagged_v1";

/// SQL that matches a turn a chat import wrote, before or without the flag:
/// a `memory` row from a Claude or ChatGPT conversation (the import's
/// `import_<vendor>_<conversation>` source id and its import session) whose
/// text starts with the `User: ` speaker line `pair_messages` writes.
/// GLOB and `substr` are exact and case-sensitive; LIKE would treat `_` as a
/// wildcard and ignore case.
const IMPORTED_TURN_SQL: &str = "m.source = 'memory'
   AND m.session_id IS NOT NULL
   AND (m.source_id GLOB 'import_claude_?*' OR m.source_id GLOB 'import_chatgpt_?*')
   AND substr(m.content, 1, 6) = 'User: '";

/// SQL condition, for a `memories` alias, that keeps only rows which are not
/// imported chat turns. `alias` is a table alias such as `"m"`, or `""` for
/// an unqualified query.
pub(crate) fn not_chat_turn(alias: &str) -> String {
    let prefix = if alias.is_empty() {
        String::new()
    } else {
        format!("{alias}.")
    };
    format!("COALESCE({prefix}semantic_unit, '') <> '{CHAT_TURN_UNIT}'")
}

/// Stable code at the start of the refusal to confirm or retype a flagged
/// turn.
pub const CHAT_TURN_NOT_A_MEMORY: &str = "chat_turn_not_a_memory";

fn not_a_memory_error(source_id: &str) -> WenlanError {
    WenlanError::Conflict(format!(
        "{CHAT_TURN_NOT_A_MEMORY}: {source_id} is text from an imported conversation, \
         not a memory the person stated, so it cannot be confirmed or given a memory type"
    ))
}

/// Refuse, on an open connection, to treat a flagged turn as the person's
/// memory. Used inside a write's own transaction.
pub(super) async fn refuse_chat_turn_on_connection(
    conn: &libsql::Connection,
    source_id: &str,
) -> Result<(), WenlanError> {
    let mut rows = conn
        .query(
            &format!(
                "SELECT 1 FROM memories
                  WHERE source_id = ?1 AND semantic_unit = '{CHAT_TURN_UNIT}' LIMIT 1"
            ),
            libsql::params![source_id],
        )
        .await
        .map_err(db_err("refuse"))?;
    if rows.next().await.map_err(db_err("refuse row"))?.is_some() {
        return Err(not_a_memory_error(source_id));
    }
    Ok(())
}

/// The Rust twin of the import's shape, for one row about to be written.
pub(crate) fn is_imported_chat_turn(source_id: &str, has_session: bool, content: &str) -> bool {
    let conversation = ["import_claude_", "import_chatgpt_"].iter().any(|prefix| {
        source_id
            .strip_prefix(prefix)
            .is_some_and(|rest| !rest.is_empty())
    });
    has_session && conversation && content.starts_with("User: ")
}

/// What the one-time pass found and changed. On a dry run, or once the
/// marker is set, nothing is written and `flagged` is 0.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ChatTurnFlagOutcome {
    pub already_ran: bool,
    pub dry_run: bool,
    /// Rows with the import's shape and no `semantic_unit` yet. Only these
    /// are flagged, so clearing the flag puts each back exactly as it was.
    pub matched: u64,
    /// Rows with the import's shape that carry another `semantic_unit`. They
    /// are left alone: flagging them would lose that value.
    pub skipped_other_unit: u64,
    /// Rows this call flagged.
    pub flagged: u64,
    /// Distinct conversations among `matched`.
    pub conversations: u64,
    /// Of `matched`, how many carry each machine-assigned type. The type is
    /// kept; these counts say what the flag now takes precedence over.
    pub typed_fact: u64,
    pub typed_preference: u64,
    pub typed_identity: u64,
    pub typed_decision: u64,
    pub typed_other: u64,
}

/// One flagged turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChatTurnRow {
    pub id: String,
    pub source_id: String,
    pub chunk_index: i64,
    pub title: Option<String>,
}

/// A page that cites at least one flagged conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChatTurnPage {
    pub page_id: String,
    pub title: String,
    pub status: String,
    pub conversations: u64,
}

/// A topic linked from at least one flagged conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChatTurnTopic {
    pub entity_id: String,
    /// The topic page's title, when the topic has one.
    pub name: Option<String>,
    pub conversations: u64,
}

/// Everything resting on imported chat turns.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ChatTurnReport {
    pub turns: u64,
    pub conversations: u64,
    /// Live `cites` edges from a page to a flagged conversation. `pages` is
    /// built from these: they are what a page's sources are read from.
    pub page_citations: u64,
    /// Legacy `page_sources` rows that point at a flagged conversation, kept
    /// as they are.
    pub legacy_page_source_links: u64,
    /// `memory_entities` rows that point at a flagged conversation.
    pub topic_links: u64,
    pub rows: Vec<ChatTurnRow>,
    pub pages: Vec<ChatTurnPage>,
    pub topics: Vec<ChatTurnTopic>,
}

fn db_err(context: &str) -> impl Fn(libsql::Error) -> WenlanError + '_ {
    move |error| WenlanError::VectorDb(format!("chat turns {context}: {error}"))
}

impl MemoryDB {
    /// Flag every imported chat turn written before new imports flagged their
    /// own, once per database. Detection is the import's source id, its
    /// session and the `User: ` shape together — never content alone, so a
    /// note that happens to start with `User: ` is untouched. Only
    /// `semantic_unit` is written, and only where it was empty: the stored
    /// type, content, citations and topic links stay as they are. `dry_run`
    /// counts without writing and without setting the marker.
    pub async fn flag_imported_chat_turns(
        &self,
        dry_run: bool,
    ) -> Result<ChatTurnFlagOutcome, WenlanError> {
        let conn = self.conn.lock().await;
        let mut marker = conn
            .query(
                "SELECT 1 FROM app_metadata WHERE key = ?1",
                libsql::params![CHAT_TURNS_FLAGGED_MARKER],
            )
            .await
            .map_err(db_err("marker"))?;
        let already_ran = marker.next().await.map_err(db_err("marker row"))?.is_some();
        drop(marker);
        if already_ran && !dry_run {
            return Ok(ChatTurnFlagOutcome {
                already_ran,
                ..ChatTurnFlagOutcome::default()
            });
        }
        let mut rows = conn
            .query(
                &format!(
                    "SELECT SUM(m.semantic_unit IS NULL),
                            COUNT(DISTINCT CASE WHEN m.semantic_unit IS NULL THEN m.source_id END),
                            SUM(m.semantic_unit IS NULL AND m.memory_type = 'fact'),
                            SUM(m.semantic_unit IS NULL AND m.memory_type = 'preference'),
                            SUM(m.semantic_unit IS NULL AND m.memory_type = 'identity'),
                            SUM(m.semantic_unit IS NULL AND m.memory_type = 'decision'),
                            SUM(m.semantic_unit IS NULL AND m.memory_type IS NOT NULL
                                AND m.memory_type NOT IN ('fact', 'preference', 'identity', 'decision')),
                            SUM(m.semantic_unit IS NOT NULL AND m.semantic_unit <> '{CHAT_TURN_UNIT}')
                       FROM memories m
                      WHERE {IMPORTED_TURN_SQL}"
                ),
                (),
            )
            .await
            .map_err(db_err("count"))?;
        let row = rows
            .next()
            .await
            .map_err(db_err("count row"))?
            .ok_or_else(|| WenlanError::VectorDb("chat turns count: no row".into()))?;
        let count = |index: i32| row.get::<Option<i64>>(index).ok().flatten().unwrap_or(0) as u64;
        let mut outcome = ChatTurnFlagOutcome {
            already_ran,
            dry_run,
            matched: count(0),
            conversations: count(1),
            typed_fact: count(2),
            typed_preference: count(3),
            typed_identity: count(4),
            typed_decision: count(5),
            typed_other: count(6),
            skipped_other_unit: count(7),
            flagged: 0,
        };
        drop(rows);
        if dry_run {
            return Ok(outcome);
        }
        conn.execute("BEGIN IMMEDIATE", ())
            .await
            .map_err(db_err("begin"))?;
        let write: Result<u64, WenlanError> = async {
            let flagged = conn
                .execute(
                    &format!(
                        "UPDATE memories AS m
                            SET semantic_unit = '{CHAT_TURN_UNIT}'
                          WHERE {IMPORTED_TURN_SQL} AND m.semantic_unit IS NULL"
                    ),
                    (),
                )
                .await
                .map_err(db_err("update"))?;
            // The marker records what this call wrote, so it carries the
            // flagged count rather than the 0 the outcome starts with.
            let recorded = ChatTurnFlagOutcome {
                flagged,
                ..outcome.clone()
            };
            let summary = serde_json::to_string(&recorded)
                .map_err(|error| WenlanError::VectorDb(format!("chat turns marker: {error}")))?;
            conn.execute(
                "INSERT INTO app_metadata (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                libsql::params![CHAT_TURNS_FLAGGED_MARKER, summary],
            )
            .await
            .map_err(db_err("marker write"))?;
            Ok(flagged)
        }
        .await;
        match write {
            Ok(flagged) => {
                super::commit_or_rollback(&conn)
                    .await
                    .map_err(db_err("commit"))?;
                outcome.flagged = flagged;
                Ok(outcome)
            }
            Err(error) => {
                let _ = conn.execute("ROLLBACK", ()).await;
                Err(error)
            }
        }
    }

    /// Undo the flag: clear `semantic_unit` on every flagged turn, which is
    /// the only column flagging writes, so each row is back exactly as it was.
    /// The marker stays, so the startup pass does not flag them again; turns
    /// imported later are still flagged on insert. Returns the rows cleared.
    pub async fn unflag_imported_chat_turns(&self) -> Result<u64, WenlanError> {
        let conn = self.conn.lock().await;
        conn.execute(
            &format!(
                "UPDATE memories AS m SET semantic_unit = NULL
                  WHERE m.semantic_unit = '{CHAT_TURN_UNIT}' AND {IMPORTED_TURN_SQL}"
            ),
            (),
        )
        .await
        .map_err(db_err("unflag"))
    }

    /// Refuse to treat a flagged turn as the person's memory: confirming it
    /// or giving it a memory type (UX-ADD-4).
    pub async fn refuse_if_chat_turn(&self, source_id: &str) -> Result<(), WenlanError> {
        let conn = self.conn.lock().await;
        refuse_chat_turn_on_connection(&conn, source_id).await
    }

    /// List the flagged turns and what rests on them: the pages that cite
    /// their conversations and the topics linked from those conversations.
    /// Read-only; nothing here removes a citation or a topic link.
    pub async fn chat_turn_report(&self) -> Result<ChatTurnReport, WenlanError> {
        let flagged = format!("NOT ({})", not_chat_turn("m"));
        let conn = self.conn.lock().await;
        let mut report = ChatTurnReport::default();

        let mut rows = conn
            .query(
                &format!(
                    "SELECT m.id, m.source_id, m.chunk_index, m.title
                       FROM memories m WHERE {flagged}
                      ORDER BY m.source_id, m.chunk_index"
                ),
                (),
            )
            .await
            .map_err(db_err("rows"))?;
        let mut conversations = std::collections::BTreeSet::new();
        while let Some(row) = rows.next().await.map_err(db_err("row"))? {
            let source_id: String = row.get(1).map_err(db_err("row source"))?;
            conversations.insert(source_id.clone());
            report.rows.push(ChatTurnRow {
                id: row.get(0).map_err(db_err("row id"))?,
                source_id,
                chunk_index: row.get::<Option<i64>>(2).ok().flatten().unwrap_or(0),
                title: row.get::<Option<String>>(3).ok().flatten(),
            });
        }
        drop(rows);
        report.turns = report.rows.len() as u64;
        report.conversations = conversations.len() as u64;

        let flagged_sources =
            format!("SELECT DISTINCT m.source_id FROM memories m WHERE {flagged}");
        let mut rows = conn
            .query(
                &format!(
                    "SELECT p.id, COALESCE(p.title, ''), COALESCE(p.status, ''),
                            COUNT(DISTINCT e.dst_id), COUNT(*)
                       FROM edges e JOIN pages p ON p.id = e.src_id
                      WHERE e.edge_type = 'cites' AND e.src_kind = 'page'
                        AND e.dst_kind = 'memory' AND e.valid_until IS NULL
                        AND e.dst_id IN ({flagged_sources})
                      GROUP BY p.id ORDER BY p.title, p.id"
                ),
                (),
            )
            .await
            .map_err(db_err("pages"))?;
        while let Some(row) = rows.next().await.map_err(db_err("page"))? {
            report.page_citations += row.get::<i64>(4).unwrap_or(0) as u64;
            report.pages.push(ChatTurnPage {
                page_id: row.get(0).map_err(db_err("page id"))?,
                title: row.get(1).unwrap_or_default(),
                status: row.get(2).unwrap_or_default(),
                conversations: row.get::<i64>(3).unwrap_or(0) as u64,
            });
        }
        drop(rows);

        let mut rows = conn
            .query(
                &format!(
                    "SELECT COUNT(*) FROM page_sources
                      WHERE memory_source_id IN ({flagged_sources})"
                ),
                (),
            )
            .await
            .map_err(db_err("legacy page sources"))?;
        if let Some(row) = rows.next().await.map_err(db_err("legacy page source"))? {
            report.legacy_page_source_links = row.get::<i64>(0).unwrap_or(0) as u64;
        }
        drop(rows);

        let mut rows = conn
            .query(
                &format!(
                    "SELECT me.entity_id,
                            (SELECT p.title FROM entity_page_map epm
                               JOIN pages p ON p.id = epm.page_id
                              WHERE epm.entity_id = me.entity_id),
                            COUNT(DISTINCT me.memory_id)
                       FROM memory_entities me
                      WHERE me.memory_id IN ({flagged_sources})
                      GROUP BY me.entity_id ORDER BY me.entity_id"
                ),
                (),
            )
            .await
            .map_err(db_err("topics"))?;
        while let Some(row) = rows.next().await.map_err(db_err("topic"))? {
            let conversations = row.get::<i64>(2).unwrap_or(0) as u64;
            report.topic_links += conversations;
            report.topics.push(ChatTurnTopic {
                entity_id: row.get(0).map_err(db_err("topic id"))?,
                name: row.get::<Option<String>>(1).ok().flatten(),
                conversations,
            });
        }
        Ok(report)
    }
}

#[cfg(test)]
#[path = "chat_turns_test.rs"]
mod tests;
