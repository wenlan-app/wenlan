// SPDX-License-Identifier: Apache-2.0
//! Agent sessions: which conversation a memory, note version, or Brief update
//! came from. A session is named `<tool>:<external_id>`; its row id is a
//! deterministic hash of that name, so outbox replays and backfills converge
//! on one row.

use super::{
    not_hidden_by_superseder, push_read_scope_filter_folded, scoped_pages::page_scope_clause,
    MemoryDB,
};
use crate::read_scope::ReadScope;
use crate::WenlanError;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use wenlan_types::{
    AgentSession, PageSessionEntry, PageSessionVia, PageSessionsResponse, SessionCitedByEntry,
    SessionDetailResponse, SessionHandoffEntry, SessionMemoryEntry, SessionPageEntry,
    SessionReadEntry, SessionReadKind, SessionRef, UpsertSessionRequest,
};

/// Most memories and notes `GET /api/sessions/{id}` returns for one session.
const SESSION_DETAIL_CAP: i64 = 200;
/// Longest accepted metadata value, in bytes. Paths and remotes are short.
const MAX_METADATA_LEN: usize = 4096;
/// Newest reads kept per session; older ones are dropped as new ones arrive.
const SESSION_READS_KEPT: i64 = 50;
/// Most reads `GET /api/sessions/{id}` lists. The total is reported beside.
pub const SESSION_READS_LISTED: usize = 10;
/// Most notes listed under `cited_by`.
const SESSION_CITED_BY_CAP: i64 = 10;
/// Longest stored read label, in characters.
const MAX_READ_LABEL_CHARS: usize = 80;

const SESSION_READS_DDL: &str = "
CREATE TABLE IF NOT EXISTS session_reads (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('recall', 'brief', 'read_page')),
    label TEXT NOT NULL,
    space TEXT,
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_session_reads_session ON session_reads(session_id, id);
";

const AGENT_SESSIONS_DDL: &str = "
CREATE TABLE IF NOT EXISTS agent_sessions (
    id TEXT PRIMARY KEY,
    tool TEXT NOT NULL,
    external_id TEXT NOT NULL,
    title TEXT,
    cwd TEXT,
    repo_root TEXT,
    repo_remote TEXT,
    repo_branch TEXT,
    repo_commit TEXT,
    transcript_path TEXT,
    space TEXT,
    first_write_at INTEGER NOT NULL,
    last_write_at INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE(tool, external_id)
);
";

const SESSION_COLUMNS: &str = "id, tool, external_id, title, cwd, repo_root, repo_remote, \
                               repo_branch, repo_commit, transcript_path, space, \
                               first_write_at, last_write_at";

/// `ses_` + the first 24 hex characters of sha256(`<tool>:<external_id>`).
pub fn session_row_id(session: &SessionRef) -> String {
    let digest = Sha256::digest(session.to_string().as_bytes());
    let mut id = String::with_capacity(28);
    id.push_str("ses_");
    for byte in digest.iter().take(12) {
        id.push_str(&format!("{byte:02x}"));
    }
    id
}

/// The session a chat-export memory belongs to, from its import `source_id`
/// (`import_claude_<conversation id>` or `import_chatgpt_<conversation id>`).
/// `None` for any other source id, or a conversation id that is not a valid
/// session external id.
pub fn import_session_ref(source_id: &str) -> Option<SessionRef> {
    let (tool, external_id) = if let Some(rest) = source_id.strip_prefix("import_claude_") {
        ("claude-ai", rest)
    } else if let Some(rest) = source_id.strip_prefix("import_chatgpt_") {
        ("chatgpt", rest)
    } else {
        return None;
    };
    SessionRef::new(tool, external_id).ok()
}

fn session_from_row(row: &libsql::Row) -> AgentSession {
    AgentSession {
        id: row.get::<String>(0).unwrap_or_default(),
        tool: row.get::<String>(1).unwrap_or_default(),
        external_id: row.get::<String>(2).unwrap_or_default(),
        title: row.get::<Option<String>>(3).unwrap_or(None),
        cwd: row.get::<Option<String>>(4).unwrap_or(None),
        repo_root: row.get::<Option<String>>(5).unwrap_or(None),
        repo_remote: row.get::<Option<String>>(6).unwrap_or(None),
        repo_branch: row.get::<Option<String>>(7).unwrap_or(None),
        repo_commit: row.get::<Option<String>>(8).unwrap_or(None),
        transcript_path: row.get::<Option<String>>(9).unwrap_or(None),
        space: crate::space_context::normalize_unfiled_space(
            row.get::<Option<String>>(10).unwrap_or(None),
        ),
        first_write_at: row.get::<i64>(11).unwrap_or(0),
        last_write_at: row.get::<i64>(12).unwrap_or(0),
    }
}

/// Insert the session, or widen its write span and fill what it lacks:
/// `first_write_at` only moves earlier, `last_write_at` only later, and
/// `title` and `space` are set only while NULL. Runs on the caller's
/// connection or transaction. Returns the row id.
pub(super) async fn upsert_session_span(
    conn: &libsql::Connection,
    session: &SessionRef,
    title: Option<&str>,
    space: Option<&str>,
    first_write_at: i64,
    last_write_at: i64,
) -> Result<String, libsql::Error> {
    let id = session_row_id(session);
    let now = chrono::Utc::now().timestamp();
    let space = space
        .filter(|s| !s.is_empty() && *s != super::UNFILED_SPACE_ID)
        .map(str::to_string);
    let title = title.map(str::trim).filter(|t| !t.is_empty());
    conn.execute(
        "INSERT INTO agent_sessions
            (id, tool, external_id, title, space, first_write_at, last_write_at,
             created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)
         ON CONFLICT(tool, external_id) DO UPDATE SET
            title = COALESCE(title, excluded.title),
            space = COALESCE(space, excluded.space),
            first_write_at = MIN(first_write_at, excluded.first_write_at),
            last_write_at = MAX(last_write_at, excluded.last_write_at),
            updated_at = excluded.updated_at",
        libsql::params![
            id.as_str(),
            session.tool.as_str(),
            session.external_id.as_str(),
            title,
            space,
            first_write_at,
            last_write_at,
            now
        ],
    )
    .await?;
    Ok(id)
}

/// Most transcript bytes read from the end of the file to find a title.
const TRANSCRIPT_TAIL_BYTES: u64 = 32 * 1024 * 1024;
/// Bytes read from the start when the tail holds no title (a title written
/// early in a very long conversation).
const TRANSCRIPT_HEAD_BYTES: u64 = 1024 * 1024;
/// A transcript line longer than this cannot be a title line; it is skipped.
const TRANSCRIPT_MAX_LINE_BYTES: usize = 64 * 1024;
/// Longest stored conversation title, in characters.
const MAX_TRANSCRIPT_TITLE_CHARS: usize = 200;

/// The newest `ai-title` in a Claude Code transcript (JSONL), or `None`.
///
/// Claude Code appends lines such as
/// `{"type":"ai-title","aiTitle":"...","sessionId":"..."}`; the last one is
/// current. Reads at most `TRANSCRIPT_TAIL_BYTES` from the end, then the first
/// `TRANSCRIPT_HEAD_BYTES` when the tail has none. Malformed lines, lines
/// without a non-empty string `aiTitle`, and over-long lines are ignored.
/// Blocking file IO: call from a blocking task.
pub fn last_ai_title(path: &std::path::Path) -> Option<String> {
    use std::io::{Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    if len <= TRANSCRIPT_TAIL_BYTES {
        return scan_ai_title(&mut file, false);
    }
    file.seek(SeekFrom::Start(len - TRANSCRIPT_TAIL_BYTES))
        .ok()?;
    // The first line of the tail is probably cut mid-way.
    if let Some(title) = scan_ai_title(&mut file, true) {
        return Some(title);
    }
    file.seek(SeekFrom::Start(0)).ok()?;
    scan_ai_title(&mut std::io::Read::take(file, TRANSCRIPT_HEAD_BYTES), false)
}

fn scan_ai_title(reader: &mut impl std::io::Read, skip_partial_first: bool) -> Option<String> {
    let mut buf = [0u8; 64 * 1024];
    let mut line: Vec<u8> = Vec::new();
    let mut overlong = false;
    let mut skipping_first = skip_partial_first;
    let mut found = None;
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        for &byte in &buf[..n] {
            if byte == b'\n' {
                if skipping_first {
                    skipping_first = false;
                } else if !overlong {
                    if let Some(title) = ai_title_from_line(&line) {
                        found = Some(title);
                    }
                }
                line.clear();
                overlong = false;
            } else if !overlong && !skipping_first {
                if line.len() >= TRANSCRIPT_MAX_LINE_BYTES {
                    overlong = true;
                    line.clear();
                } else {
                    line.push(byte);
                }
            }
        }
    }
    // A final line without a newline.
    if !skipping_first && !overlong {
        if let Some(title) = ai_title_from_line(&line) {
            found = Some(title);
        }
    }
    found
}

fn ai_title_from_line(line: &[u8]) -> Option<String> {
    const MARKER: &[u8] = b"\"ai-title\"";
    if !line.windows(MARKER.len()).any(|window| window == MARKER) {
        return None;
    }
    let value: serde_json::Value = serde_json::from_slice(line).ok()?;
    if value.get("type")?.as_str()? != "ai-title" {
        return None;
    }
    let title = value.get("aiTitle")?.as_str()?.trim();
    if title.is_empty() {
        return None;
    }
    Some(title.chars().take(MAX_TRANSCRIPT_TITLE_CHARS).collect())
}

/// A read label as stored: whitespace collapsed, cut to
/// `MAX_READ_LABEL_CHARS` characters. `None` when nothing is left.
fn clean_read_label(raw: &str) -> Option<String> {
    let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return None;
    }
    if collapsed.chars().count() <= MAX_READ_LABEL_CHARS {
        return Some(collapsed);
    }
    let mut cut: String = collapsed.chars().take(MAX_READ_LABEL_CHARS - 1).collect();
    cut.truncate(cut.trim_end().len());
    cut.push('…');
    Some(cut)
}

/// The `session_reads.space` value for a read made inside `scope`: `None` for
/// the whole library.
pub fn session_read_space(scope: &ReadScope) -> Option<String> {
    match scope {
        ReadScope::Global => None,
        ReadScope::Space(space) => Some(space.clone()),
        ReadScope::Uncategorized => Some(super::UNFILED_SPACE_ID.to_string()),
    }
}

/// The `session_reads.space` value for a read of one note or Space: its own
/// Space, or the unfiled marker when it has none.
pub fn session_read_space_of(space: Option<&str>) -> String {
    space
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(super::UNFILED_SPACE_ID)
        .to_string()
}

/// SQL that keeps a `session_reads.space` column to what `scope` may see.
/// A read made without a Space (the whole library) is visible only to an
/// unscoped caller, so a scoped caller never sees a query that spanned more
/// than its own Space.
fn push_read_space_filter(
    scope: &ReadScope,
    column: &str,
    conditions: &mut Vec<String>,
    values: &mut Vec<libsql::Value>,
) {
    match scope {
        ReadScope::Global => {}
        ReadScope::Space(space) => {
            conditions.push(format!("{column} = ?"));
            values.push(libsql::Value::Text(space.clone()));
        }
        ReadScope::Uncategorized => {
            conditions.push(format!("{column} = '{}'", super::UNFILED_SPACE_ID));
        }
    }
}

/// Normalize one metadata value: trimmed, empty means absent, bounded.
fn clean_metadata(name: &str, value: &Option<String>) -> Result<Option<String>, WenlanError> {
    let Some(value) = value.as_deref().map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    if value.len() > MAX_METADATA_LEN {
        return Err(WenlanError::Validation(format!(
            "session {name} must be at most {MAX_METADATA_LEN} bytes"
        )));
    }
    Ok(Some(value.to_string()))
}

impl MemoryDB {
    /// Migration 140: `agent_sessions`, the `session_id` columns on
    /// `memories`, `page_history`, and `brief_history`, and a session for every
    /// past ChatGPT / Claude.ai import conversation. The column-exists checks
    /// keep a crash-replay (columns added, version bump lost) idempotent, same
    /// pattern as migration 137.
    pub(super) async fn migrate_140_agent_sessions(
        &self,
        prior_version: i64,
    ) -> Result<(), WenlanError> {
        self.backup_before_migration(140, prior_version).await?;
        let conn = self.conn.lock().await;
        conn.execute_batch(AGENT_SESSIONS_DDL)
            .await
            .map_err(|e| WenlanError::VectorDb(format!("m140 agent_sessions DDL: {e}")))?;
        for table in ["memories", "page_history", "brief_history"] {
            let mut rows = conn
                .query(
                    &format!(
                        "SELECT COUNT(*) FROM pragma_table_info('{table}') \
                         WHERE name = 'session_id'"
                    ),
                    (),
                )
                .await
                .map_err(|e| WenlanError::VectorDb(format!("m140 {table} col check: {e}")))?;
            let has_col = match rows.next().await {
                Ok(Some(row)) => row.get::<i64>(0).unwrap_or(0) > 0,
                _ => false,
            };
            drop(rows);
            if !has_col {
                conn.execute(
                    &format!("ALTER TABLE {table} ADD COLUMN session_id TEXT"),
                    (),
                )
                .await
                .map_err(|e| WenlanError::VectorDb(format!("m140 {table} add column: {e}")))?;
            }
        }
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_memories_session
                 ON memories(session_id) WHERE session_id IS NOT NULL;
             CREATE INDEX IF NOT EXISTS idx_page_history_session
                 ON page_history(session_id) WHERE session_id IS NOT NULL;",
        )
        .await
        .map_err(|e| WenlanError::VectorDb(format!("m140 indexes: {e}")))?;

        Self::backfill_import_sessions(&conn).await?;

        conn.execute("PRAGMA user_version = 140", ())
            .await
            .map_err(|e| WenlanError::VectorDb(format!("m140 bump: {e}")))?;
        log::info!("[migration] Migration 140 applied: agent_sessions and session_id columns");
        Ok(())
    }

    /// Migration 141: `session_reads`. `CREATE ... IF NOT EXISTS` keeps a
    /// crash-replay (table created, version bump lost) idempotent.
    pub(super) async fn migrate_141_session_reads(&self) -> Result<(), WenlanError> {
        let conn = self.conn.lock().await;
        conn.execute_batch(SESSION_READS_DDL)
            .await
            .map_err(|e| WenlanError::VectorDb(format!("m141 session_reads DDL: {e}")))?;
        conn.execute("PRAGMA user_version = 141", ())
            .await
            .map_err(|e| WenlanError::VectorDb(format!("m141 bump: {e}")))?;
        log::info!("[migration] Migration 141 applied: session_reads");
        Ok(())
    }

    /// One session per chat-export conversation, stamped onto its memory rows.
    /// Every chunk of a conversation shares its `source_id`, so grouping by it
    /// yields one session each. Conversation ids that are not valid session
    /// external ids are left unstamped.
    async fn backfill_import_sessions(conn: &libsql::Connection) -> Result<(), WenlanError> {
        let mut rows = conn
            .query(
                "SELECT source_id,
                        MAX(CASE WHEN title <> '' THEN title END),
                        MIN(COALESCE(created_at, last_modified)),
                        MAX(COALESCE(created_at, last_modified))
                   FROM memories
                  WHERE source = 'memory'
                    AND session_id IS NULL
                    AND (source_id LIKE 'import\\_claude\\_%' ESCAPE '\\'
                         OR source_id LIKE 'import\\_chatgpt\\_%' ESCAPE '\\')
                  GROUP BY source_id",
                (),
            )
            .await
            .map_err(|e| WenlanError::VectorDb(format!("m140 backfill read: {e}")))?;
        let mut conversations = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| WenlanError::VectorDb(format!("m140 backfill row: {e}")))?
        {
            conversations.push((
                row.get::<String>(0).unwrap_or_default(),
                row.get::<Option<String>>(1).unwrap_or(None),
                row.get::<i64>(2).unwrap_or(0),
                row.get::<i64>(3).unwrap_or(0),
            ));
        }
        drop(rows);
        if conversations.is_empty() {
            return Ok(());
        }

        let tx = conn
            .transaction_with_behavior(libsql::TransactionBehavior::Immediate)
            .await
            .map_err(|e| WenlanError::VectorDb(format!("m140 backfill begin: {e}")))?;
        for (source_id, title, first, last) in conversations {
            let Some(session) = import_session_ref(&source_id) else {
                log::warn!("[migration] m140: no session for import source_id {source_id}");
                continue;
            };
            let id = upsert_session_span(&tx, &session, title.as_deref(), None, first, last)
                .await
                .map_err(|e| WenlanError::VectorDb(format!("m140 backfill session: {e}")))?;
            tx.execute(
                "UPDATE memories SET session_id = ?1
                  WHERE source = 'memory' AND source_id = ?2 AND session_id IS NULL",
                libsql::params![id, source_id],
            )
            .await
            .map_err(|e| WenlanError::VectorDb(format!("m140 backfill stamp: {e}")))?;
        }
        tx.commit()
            .await
            .map_err(|e| WenlanError::VectorDb(format!("m140 backfill commit: {e}")))?;
        Ok(())
    }

    /// Record that `session` wrote the memory `source_id`: stamp its rows
    /// (first session wins) and upsert the session. A no-op when the memory
    /// does not exist, so a session is only ever created by a real write.
    pub async fn stamp_memory_session(
        &self,
        source_id: &str,
        session: &SessionRef,
        space: Option<&str>,
    ) -> Result<(), WenlanError> {
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(libsql::TransactionBehavior::Immediate)
            .await
            .map_err(|e| WenlanError::VectorDb(format!("stamp memory session begin: {e}")))?;
        let now = chrono::Utc::now().timestamp();
        let id = session_row_id(session);
        let stamped = tx
            .execute(
                "UPDATE memories SET session_id = ?1
                  WHERE source = 'memory' AND source_id = ?2 AND session_id IS NULL",
                libsql::params![id.as_str(), source_id],
            )
            .await
            .map_err(|e| WenlanError::VectorDb(format!("stamp memory session: {e}")))?;
        if stamped > 0 {
            upsert_session_span(&tx, session, None, space, now, now)
                .await
                .map_err(|e| WenlanError::VectorDb(format!("stamp memory session upsert: {e}")))?;
        }
        tx.commit()
            .await
            .map_err(|e| WenlanError::VectorDb(format!("stamp memory session commit: {e}")))?;
        Ok(())
    }

    /// Record that `session` wrote the version(s) of `page_id` created at or
    /// after `since` (unix seconds): stamp those `page_history` rows that have
    /// no session yet, and upsert the session. A no-op when the write produced
    /// no history row (a staged revision card, say).
    pub async fn stamp_page_history_session(
        &self,
        page_id: &str,
        since: i64,
        session: &SessionRef,
        space: Option<&str>,
    ) -> Result<(), WenlanError> {
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(libsql::TransactionBehavior::Immediate)
            .await
            .map_err(|e| WenlanError::VectorDb(format!("stamp page session begin: {e}")))?;
        let now = chrono::Utc::now().timestamp();
        let id = session_row_id(session);
        let stamped = tx
            .execute(
                "UPDATE page_history SET session_id = ?1
                  WHERE page_id = ?2 AND created_at >= ?3 AND session_id IS NULL",
                libsql::params![id.as_str(), page_id, since],
            )
            .await
            .map_err(|e| WenlanError::VectorDb(format!("stamp page session: {e}")))?;
        if stamped > 0 {
            upsert_session_span(&tx, session, None, space, now, now)
                .await
                .map_err(|e| WenlanError::VectorDb(format!("stamp page session upsert: {e}")))?;
        }
        tx.commit()
            .await
            .map_err(|e| WenlanError::VectorDb(format!("stamp page session commit: {e}")))?;
        Ok(())
    }

    /// Record that `session` read something from Wenlan, for the "what it
    /// looked up" list. Best effort bookkeeping on the read path:
    ///
    /// - Only a session the daemon already knows (announced or written for) is
    ///   recorded; a read never creates a session. Returns `false` then.
    /// - `label` is whitespace-collapsed and cut to 80 characters; nothing is
    ///   recorded when it is empty.
    /// - An identical consecutive read (same kind, label and Space) moves the
    ///   newest entry's time instead of adding a row.
    /// - Only the newest `SESSION_READS_KEPT` rows of a session are kept.
    ///
    /// `space` is the Space the read was scoped to, `None` for the whole
    /// library. It decides who may see the entry later.
    pub async fn record_session_read(
        &self,
        session: &SessionRef,
        kind: SessionReadKind,
        label: &str,
        space: Option<&str>,
    ) -> Result<bool, WenlanError> {
        let Some(label) = clean_read_label(label) else {
            return Ok(false);
        };
        let space = space.filter(|s| !s.is_empty());
        let id = session_row_id(session);
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(libsql::TransactionBehavior::Immediate)
            .await
            .map_err(|e| WenlanError::VectorDb(format!("record session read begin: {e}")))?;
        let mut rows = tx
            .query(
                "SELECT 1 FROM agent_sessions WHERE id = ?1",
                libsql::params![id.as_str()],
            )
            .await
            .map_err(|e| WenlanError::VectorDb(format!("record session read lookup: {e}")))?;
        let known = rows
            .next()
            .await
            .map_err(|e| WenlanError::VectorDb(format!("record session read lookup row: {e}")))?
            .is_some();
        drop(rows);
        if !known {
            return Ok(false);
        }

        let mut rows = tx
            .query(
                "SELECT id, kind, label, space FROM session_reads
                  WHERE session_id = ?1 ORDER BY id DESC LIMIT 1",
                libsql::params![id.as_str()],
            )
            .await
            .map_err(|e| WenlanError::VectorDb(format!("record session read newest: {e}")))?;
        let newest = rows
            .next()
            .await
            .map_err(|e| WenlanError::VectorDb(format!("record session read newest row: {e}")))?
            .map(|row| {
                (
                    row.get::<i64>(0).unwrap_or(0),
                    row.get::<String>(1).unwrap_or_default(),
                    row.get::<String>(2).unwrap_or_default(),
                    row.get::<Option<String>>(3).unwrap_or(None),
                )
            });
        drop(rows);

        match newest {
            Some((row_id, newest_kind, newest_label, newest_space))
                if newest_kind == kind.as_str()
                    && newest_label == label
                    && newest_space.as_deref() == space =>
            {
                tx.execute(
                    "UPDATE session_reads SET created_at = ?1 WHERE id = ?2",
                    libsql::params![now, row_id],
                )
                .await
                .map_err(|e| WenlanError::VectorDb(format!("record session read touch: {e}")))?;
            }
            _ => {
                tx.execute(
                    "INSERT INTO session_reads (session_id, kind, label, space, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    libsql::params![id.as_str(), kind.as_str(), label, space, now],
                )
                .await
                .map_err(|e| WenlanError::VectorDb(format!("record session read insert: {e}")))?;
                tx.execute(
                    "DELETE FROM session_reads
                      WHERE session_id = ?1
                        AND id NOT IN (SELECT id FROM session_reads WHERE session_id = ?1
                                        ORDER BY id DESC LIMIT ?2)",
                    libsql::params![id.as_str(), SESSION_READS_KEPT],
                )
                .await
                .map_err(|e| WenlanError::VectorDb(format!("record session read trim: {e}")))?;
            }
        }
        tx.commit()
            .await
            .map_err(|e| WenlanError::VectorDb(format!("record session read commit: {e}")))?;
        Ok(true)
    }

    /// `POST /api/sessions`: create the session if needed, then overwrite the
    /// metadata the request carries. Absent fields keep their stored value and
    /// `last_write_at` does not move; recording metadata is not a write.
    pub async fn upsert_session_metadata(
        &self,
        request: &UpsertSessionRequest,
    ) -> Result<AgentSession, WenlanError> {
        let session = SessionRef::parse(&request.session).map_err(WenlanError::Validation)?;
        let title = clean_metadata("title", &request.title)?;
        let cwd = clean_metadata("cwd", &request.cwd)?;
        let repo_root = clean_metadata("repo_root", &request.repo_root)?;
        let repo_remote = clean_metadata("repo_remote", &request.repo_remote)?;
        let repo_branch = clean_metadata("repo_branch", &request.repo_branch)?;
        let repo_commit = clean_metadata("repo_commit", &request.repo_commit)?;
        let transcript_path = clean_metadata("transcript_path", &request.transcript_path)?;

        let id = session_row_id(&session);
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO agent_sessions
                (id, tool, external_id, title, cwd, repo_root, repo_remote, repo_branch,
                 repo_commit, transcript_path, first_write_at, last_write_at,
                 created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11, ?11, ?11)
             ON CONFLICT(tool, external_id) DO UPDATE SET
                title = COALESCE(excluded.title, title),
                cwd = COALESCE(excluded.cwd, cwd),
                repo_root = COALESCE(excluded.repo_root, repo_root),
                repo_remote = COALESCE(excluded.repo_remote, repo_remote),
                repo_branch = COALESCE(excluded.repo_branch, repo_branch),
                repo_commit = COALESCE(excluded.repo_commit, repo_commit),
                transcript_path = COALESCE(excluded.transcript_path, transcript_path),
                updated_at = excluded.updated_at",
            libsql::params![
                id.as_str(),
                session.tool.as_str(),
                session.external_id.as_str(),
                title,
                cwd,
                repo_root,
                repo_remote,
                repo_branch,
                repo_commit,
                transcript_path,
                now
            ],
        )
        .await
        .map_err(|e| WenlanError::VectorDb(format!("upsert session metadata: {e}")))?;
        let mut rows = conn
            .query(
                &format!("SELECT {SESSION_COLUMNS} FROM agent_sessions WHERE id = ?1"),
                libsql::params![id.as_str()],
            )
            .await
            .map_err(|e| WenlanError::VectorDb(format!("read upserted session: {e}")))?;
        let row = rows
            .next()
            .await
            .map_err(|e| WenlanError::VectorDb(format!("read upserted session row: {e}")))?
            .ok_or_else(|| WenlanError::VectorDb("session missing after upsert".into()))?;
        Ok(session_from_row(&row))
    }

    pub async fn get_session(&self, id: &str) -> Result<Option<AgentSession>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                &format!("SELECT {SESSION_COLUMNS} FROM agent_sessions WHERE id = ?1"),
                libsql::params![id],
            )
            .await
            .map_err(|e| WenlanError::VectorDb(format!("get_session: {e}")))?;
        Ok(rows
            .next()
            .await
            .map_err(|e| WenlanError::VectorDb(format!("get_session row: {e}")))?
            .map(|row| session_from_row(&row)))
    }

    /// `GET /api/sessions/{id}`: the session and what it wrote inside `scope`,
    /// newest first. A scoped caller sees only memories, notes, and Brief
    /// updates in its own space, and `None` when none of them are visible, so a
    /// locked caller cannot learn that a session exists elsewhere.
    pub async fn get_session_detail(
        &self,
        id: &str,
        scope: &ReadScope,
    ) -> Result<Option<SessionDetailResponse>, WenlanError> {
        let Some(session) = self.get_session(id).await? else {
            return Ok(None);
        };
        let conn = self.conn.lock().await;

        let mut conditions = vec![
            "source = 'memory'".to_string(),
            "pending_revision = 0".to_string(),
            "session_id = ?".to_string(),
            not_hidden_by_superseder(scope, "memories"),
        ];
        let mut values = vec![libsql::Value::Text(id.to_string())];
        push_read_scope_filter_folded(scope, "space", &mut conditions, &mut values);
        values.push(libsql::Value::Integer(SESSION_DETAIL_CAP));
        let mut rows = conn
            .query(
                &format!(
                    "SELECT source_id, MAX(title), MAX(memory_type), MAX(space),
                            MAX(COALESCE(created_at, last_modified))
                       FROM memories
                      WHERE {}
                      GROUP BY source_id
                      ORDER BY MAX(COALESCE(created_at, last_modified)) DESC, source_id
                      LIMIT ?",
                    conditions.join(" AND ")
                ),
                libsql::params_from_iter(values),
            )
            .await
            .map_err(|e| WenlanError::VectorDb(format!("session memories: {e}")))?;
        let mut memories = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| WenlanError::VectorDb(format!("session memories row: {e}")))?
        {
            memories.push(SessionMemoryEntry {
                source_id: row.get::<String>(0).unwrap_or_default(),
                title: row.get::<String>(1).unwrap_or_default(),
                memory_type: row.get::<Option<String>>(2).unwrap_or(None),
                space: crate::space_context::normalize_unfiled_space(
                    row.get::<Option<String>>(3).unwrap_or(None),
                ),
                created_at: row.get::<i64>(4).unwrap_or(0),
            });
        }
        drop(rows);

        // One entry per note: the session's newest version of it.
        let (page_scope_sql, page_scope_value) = page_scope_clause(scope, "p.workspace", 2);
        let mut values = vec![libsql::Value::Text(id.to_string())];
        values.extend(page_scope_value);
        let mut rows = conn
            .query(
                &format!(
                    "SELECT h.page_id, COALESCE(h.title, p.title), h.version, h.created_at
                       FROM page_history h
                       JOIN pages p ON p.id = h.page_id
                      WHERE h.session_id = ?1{page_scope_sql}
                      ORDER BY h.created_at DESC, h.version DESC"
                ),
                libsql::params_from_iter(values),
            )
            .await
            .map_err(|e| WenlanError::VectorDb(format!("session pages: {e}")))?;
        let mut pages: Vec<SessionPageEntry> = Vec::new();
        let mut seen = HashSet::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| WenlanError::VectorDb(format!("session pages row: {e}")))?
        {
            let page_id = row.get::<String>(0).unwrap_or_default();
            if pages.len() as i64 >= SESSION_DETAIL_CAP || !seen.insert(page_id.clone()) {
                continue;
            }
            pages.push(SessionPageEntry {
                id: page_id,
                title: row.get::<String>(1).unwrap_or_default(),
                version: row.get::<i64>(2).unwrap_or(0),
                edited_at: row.get::<i64>(3).unwrap_or(0),
            });
        }
        drop(rows);

        let mut conditions = vec!["h.session_id = ?".to_string()];
        let mut values = vec![libsql::Value::Text(id.to_string())];
        push_read_scope_filter_folded(scope, "s.name", &mut conditions, &mut values);
        values.push(libsql::Value::Integer(SESSION_DETAIL_CAP));
        let mut rows = conn
            .query(
                &format!(
                    "SELECT s.name, h.brief_version, h.summary, h.created_at
                       FROM brief_history h
                       JOIN spaces s ON s.id = h.space_id
                      WHERE {}
                      ORDER BY h.created_at DESC, h.brief_version DESC
                      LIMIT ?",
                    conditions.join(" AND ")
                ),
                libsql::params_from_iter(values),
            )
            .await
            .map_err(|e| WenlanError::VectorDb(format!("session handoffs: {e}")))?;
        let mut handoffs = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| WenlanError::VectorDb(format!("session handoffs row: {e}")))?
        {
            handoffs.push(SessionHandoffEntry {
                space: row.get::<String>(0).unwrap_or_default(),
                brief_version: row.get::<i64>(1).unwrap_or(0),
                summary: row.get::<String>(2).unwrap_or_default(),
                created_at: row.get::<i64>(3).unwrap_or(0),
            });
        }
        drop(rows);

        if !matches!(scope, ReadScope::Global)
            && memories.is_empty()
            && pages.is_empty()
            && handoffs.is_empty()
        {
            return Ok(None);
        }

        // What the session looked up. Entries made in another Space stay
        // hidden from a scoped caller; a note read is listed only while the
        // note still exists inside the scope. The newest rows are all
        // returned: the route filters by the caller's grant, then trims.
        let (page_scope_sql, page_scope_value) = page_scope_clause(scope, "p.workspace", 2);
        let mut values = vec![libsql::Value::Text(id.to_string())];
        values.extend(page_scope_value);
        // Placeholders: `?2` (page scope, when bound) comes first in the text,
        // so the anonymous space placeholder numbers after it.
        let mut conditions = vec!["r.session_id = ?1".to_string()];
        let mut space_values = Vec::new();
        push_read_space_filter(scope, "r.space", &mut conditions, &mut space_values);
        values.extend(space_values);
        let mut rows = conn
            .query(
                &format!(
                    "SELECT r.kind, r.label, p.title, r.created_at
                       FROM session_reads r
                       LEFT JOIN pages p ON r.kind = 'read_page' AND p.id = r.label{page_scope_sql}
                      WHERE {}
                      ORDER BY r.id DESC",
                    conditions.join(" AND ")
                ),
                libsql::params_from_iter(values),
            )
            .await
            .map_err(|e| WenlanError::VectorDb(format!("session reads: {e}")))?;
        let mut reads = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| WenlanError::VectorDb(format!("session reads row: {e}")))?
        {
            let Some(kind) = SessionReadKind::parse(&row.get::<String>(0).unwrap_or_default())
            else {
                continue;
            };
            let title = row.get::<Option<String>>(2).unwrap_or(None);
            if kind == SessionReadKind::ReadPage && title.is_none() {
                continue;
            }
            reads.push(SessionReadEntry {
                kind,
                label: row.get::<String>(1).unwrap_or_default(),
                title,
                created_at: row.get::<i64>(3).unwrap_or(0),
            });
        }
        drop(rows);
        let reads_total = reads.len() as i64;

        // Notes that cite a memory this session wrote, minus the notes the
        // session wrote a version of itself (those are `pages`).
        let mut conditions = vec![
            "m.source = 'memory'".to_string(),
            "m.pending_revision = 0".to_string(),
            "m.session_id = ?".to_string(),
            not_hidden_by_superseder(scope, "m"),
        ];
        let mut memory_values = vec![libsql::Value::Text(id.to_string())];
        push_read_scope_filter_folded(scope, "m.space", &mut conditions, &mut memory_values);
        let (page_scope_sql, page_scope_value) = page_scope_clause(scope, "p.workspace", 2);
        let mut values = vec![libsql::Value::Text(id.to_string())];
        values.extend(page_scope_value);
        values.extend(memory_values);
        values.push(libsql::Value::Integer(SESSION_CITED_BY_CAP));
        let mut rows = conn
            .query(
                &format!(
                    "SELECT p.id, p.title,
                            MAX(COALESCE(json_extract(e.payload, '$.linked_at'), e.created_at))
                       FROM edges e
                       JOIN pages p ON p.id = e.src_id
                      WHERE e.edge_type = 'cites' AND e.valid_until IS NULL
                        AND p.status = 'active'{page_scope_sql}
                        AND NOT EXISTS (SELECT 1 FROM page_history h
                                         WHERE h.page_id = p.id AND h.session_id = ?1)
                        AND EXISTS (SELECT 1 FROM memories m
                                     WHERE (m.source_id = e.dst_id OR m.id = e.dst_id)
                                       AND {})
                      GROUP BY p.id, p.title
                      ORDER BY 3 DESC, p.id
                      LIMIT ?",
                    conditions.join(" AND ")
                ),
                libsql::params_from_iter(values),
            )
            .await
            .map_err(|e| WenlanError::VectorDb(format!("session cited by: {e}")))?;
        let mut cited_by = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| WenlanError::VectorDb(format!("session cited by row: {e}")))?
        {
            cited_by.push(SessionCitedByEntry {
                id: row.get::<String>(0).unwrap_or_default(),
                title: row.get::<String>(1).unwrap_or_default(),
                cited_at: row.get::<i64>(2).unwrap_or(0),
            });
        }
        drop(rows);

        Ok(Some(SessionDetailResponse {
            session,
            memories,
            pages,
            handoffs,
            reads,
            reads_total,
            cited_by,
            transcript_available: false,
        }))
    }

    /// `GET /api/pages/{id}/sessions`: sessions that wrote a version of the
    /// note (`wrote`), then sessions of the memories it cites (`source`),
    /// deduped with `wrote` winning. The caller has already checked the note
    /// is visible; cited memories are filtered by `scope` here.
    pub async fn list_page_sessions(
        &self,
        page_id: &str,
        scope: &ReadScope,
    ) -> Result<PageSessionsResponse, WenlanError> {
        let conn = self.conn.lock().await;
        let mut sessions: Vec<PageSessionEntry> = Vec::new();
        let mut seen = HashSet::new();

        let mut conditions = vec![
            "m.source = 'memory'".to_string(),
            "m.pending_revision = 0".to_string(),
            "m.session_id IS NOT NULL".to_string(),
        ];
        let mut cited_values = vec![libsql::Value::Text(page_id.to_string())];
        push_read_scope_filter_folded(scope, "m.space", &mut conditions, &mut cited_values);

        let queries = [
            (
                PageSessionVia::Wrote,
                format!(
                    "SELECT {cols} FROM agent_sessions s
                      WHERE s.id IN (SELECT session_id FROM page_history
                                      WHERE page_id = ?1 AND session_id IS NOT NULL)
                      ORDER BY s.last_write_at DESC, s.id",
                    cols = prefixed_columns("s")
                ),
                vec![libsql::Value::Text(page_id.to_string())],
            ),
            (
                PageSessionVia::Source,
                format!(
                    "SELECT {cols} FROM agent_sessions s
                      WHERE s.id IN (
                          SELECT m.session_id
                            FROM edges e
                            JOIN memories m ON m.source_id = e.dst_id OR m.id = e.dst_id
                           WHERE e.edge_type = 'cites' AND e.valid_until IS NULL
                             AND e.src_id = ? AND {conds})
                      ORDER BY s.last_write_at DESC, s.id",
                    cols = prefixed_columns("s"),
                    conds = conditions.join(" AND ")
                ),
                cited_values,
            ),
        ];
        for (via, sql, values) in queries {
            let mut rows = conn
                .query(&sql, libsql::params_from_iter(values))
                .await
                .map_err(|e| WenlanError::VectorDb(format!("page sessions: {e}")))?;
            while let Some(row) = rows
                .next()
                .await
                .map_err(|e| WenlanError::VectorDb(format!("page sessions row: {e}")))?
            {
                let session = session_from_row(&row);
                if seen.insert(session.id.clone()) {
                    sessions.push(PageSessionEntry { session, via });
                }
            }
        }
        Ok(PageSessionsResponse { sessions })
    }
}

fn prefixed_columns(alias: &str) -> String {
    SESSION_COLUMNS
        .split(',')
        .map(|column| format!("{alias}.{}", column.trim()))
        .collect::<Vec<_>>()
        .join(", ")
}
