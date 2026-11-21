// SPDX-License-Identifier: Apache-2.0

use super::MemoryDB;
use crate::error::WenlanError;
use std::collections::HashMap;

pub const BRIEF_ITEM_TEXT_MAX_CHARS: usize = 500;
pub const BRIEF_ITEM_DETAILS_MAX_CHARS: usize = 8_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyBriefItem {
    pub text: String,
    pub state: wenlan_types::BriefItemState,
    pub added_at: i64,
    pub gate: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BriefUpdateOutcome {
    Applied(wenlan_types::BriefUpdateReceipt),
    Duplicate(wenlan_types::BriefUpdateReceipt),
}

impl BriefUpdateOutcome {
    fn into_receipt(self) -> wenlan_types::BriefUpdateReceipt {
        match self {
            Self::Applied(receipt) | Self::Duplicate(receipt) => receipt,
        }
    }
}

const BRIEF_TABLES_DDL: &str = "
CREATE TABLE IF NOT EXISTS briefs (
    space_id TEXT PRIMARY KEY REFERENCES spaces(id) ON DELETE CASCADE,
    last_session_summary TEXT NOT NULL DEFAULT '',
    last_handoff_at INTEGER,
    version INTEGER NOT NULL DEFAULT 1,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS brief_items (
    id TEXT PRIMARY KEY,
    space_id TEXT NOT NULL REFERENCES spaces(id) ON DELETE CASCADE,
    text TEXT NOT NULL CHECK(length(trim(text)) > 0),
    details TEXT,
    state TEXT NOT NULL CHECK(state IN ('active', 'backlog')),
    added_at INTEGER NOT NULL,
    gate TEXT,
    version INTEGER NOT NULL DEFAULT 1,
    confirmed INTEGER NOT NULL DEFAULT 1,
    filled_by_wenlan INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_brief_items_space_state
    ON brief_items(space_id, state, added_at, id);

CREATE TABLE IF NOT EXISTS brief_legacy_imports (
    source_key TEXT PRIMARY KEY,
    space_name TEXT NOT NULL,
    imported_at INTEGER NOT NULL
);
";

/// One row per successful Brief update. `brief_version` is the version AFTER
/// the update; `snapshot_json` is the full resulting Brief. Written in the
/// same transaction as the update, so history never disagrees with the Brief.
pub(super) const BRIEF_HISTORY_DDL: &str = "
CREATE TABLE IF NOT EXISTS brief_history (
    space_id TEXT NOT NULL REFERENCES spaces(id) ON DELETE CASCADE,
    brief_version INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    caller_id TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    summary TEXT NOT NULL,
    mutations_json TEXT NOT NULL,
    snapshot_json TEXT NOT NULL,
    PRIMARY KEY (space_id, brief_version)
);
";

impl MemoryDB {
    pub(super) async fn ensure_brief_tables(tx: &libsql::Transaction) -> Result<(), WenlanError> {
        tx.execute_batch(BRIEF_TABLES_DDL)
            .await
            .map(|_| ())
            .map_err(|error| WenlanError::VectorDb(format!("brief tables DDL: {error}")))
    }

    pub(super) async fn migrate_102_brief(&self, prior_version: i64) -> Result<(), WenlanError> {
        self.backup_before_migration(102, prior_version).await?;

        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(libsql::TransactionBehavior::Immediate)
            .await
            .map_err(|error| WenlanError::VectorDb(format!("m102 brief begin: {error}")))?;
        Self::ensure_brief_tables(&tx).await?;
        tx.commit()
            .await
            .map_err(|error| WenlanError::VectorDb(format!("m102 brief commit: {error}")))?;

        conn.execute("PRAGMA user_version = 102", ())
            .await
            .map_err(|error| WenlanError::VectorDb(format!("m102 brief bump: {error}")))?;
        log::info!("[migration] Migration 102 applied: Space-owned Brief tables");
        Ok(())
    }

    /// Migration 168: distinguish a real handoff from the deterministic idle
    /// fallback that Wenlan derives from a local transcript.
    pub(super) async fn migrate_168_handoff_origin(
        &self,
        prior_version: i64,
    ) -> Result<(), WenlanError> {
        self.backup_before_migration(168, prior_version).await?;
        let conn = self.conn.lock().await;
        for (name, ddl) in [
            (
                "confirmed",
                "ALTER TABLE brief_items ADD COLUMN confirmed INTEGER NOT NULL DEFAULT 1",
            ),
            (
                "filled_by_wenlan",
                "ALTER TABLE brief_items ADD COLUMN filled_by_wenlan INTEGER NOT NULL DEFAULT 0",
            ),
        ] {
            let mut rows = conn
                .query(
                    "SELECT 1 FROM pragma_table_info('brief_items') WHERE name=?1",
                    libsql::params![name],
                )
                .await
                .map_err(|e| WenlanError::VectorDb(format!("m168 brief columns: {e}")))?;
            let exists = rows
                .next()
                .await
                .map_err(|e| WenlanError::VectorDb(format!("m168 brief column row: {e}")))?
                .is_some();
            drop(rows);
            if !exists {
                conn.execute(ddl, ())
                    .await
                    .map_err(|e| WenlanError::VectorDb(format!("m168 {name}: {e}")))?;
            }
        }
        conn.execute("PRAGMA user_version = 168", ())
            .await
            .map_err(|e| WenlanError::VectorDb(format!("m168 bump: {e}")))?;
        Ok(())
    }

    /// Migration 169: keep the AI-facing handoff context separate from the
    /// one-line person-facing Brief text.
    pub(super) async fn migrate_169_brief_item_details(
        &self,
        prior_version: i64,
    ) -> Result<(), WenlanError> {
        self.backup_before_migration(169, prior_version).await?;
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT 1 FROM pragma_table_info('brief_items') WHERE name='details'",
                (),
            )
            .await
            .map_err(|e| WenlanError::VectorDb(format!("m169 brief column: {e}")))?;
        let exists = rows
            .next()
            .await
            .map_err(|e| WenlanError::VectorDb(format!("m169 brief column row: {e}")))?
            .is_some();
        drop(rows);
        if !exists {
            conn.execute("ALTER TABLE brief_items ADD COLUMN details TEXT", ())
                .await
                .map_err(|e| WenlanError::VectorDb(format!("m169 details: {e}")))?;
        }
        conn.execute("PRAGMA user_version = 169", ())
            .await
            .map_err(|e| WenlanError::VectorDb(format!("m169 bump: {e}")))?;
        Ok(())
    }
}

fn validate_item_text(label: &str, value: &str, max: usize) -> Result<(), WenlanError> {
    if value.chars().count() > max {
        return Err(WenlanError::Validation(format!(
            "brief item {label} exceeds the {max}-character limit"
        )));
    }
    Ok(())
}

fn validate_update_request(request: &wenlan_types::BriefUpdateRequest) -> Result<(), WenlanError> {
    if request.space.trim().is_empty() {
        return Err(WenlanError::Validation(
            "brief update requires a non-empty space".into(),
        ));
    }
    if request.caller_id.trim().is_empty() || request.operation_id.trim().is_empty() {
        return Err(WenlanError::Validation(
            "brief update requires caller_id and operation_id".into(),
        ));
    }
    if let Some(summary) = &request.summary {
        if summary.text.trim().is_empty() {
            return Err(WenlanError::Validation(
                "brief summary must not be empty".into(),
            ));
        }
        if summary.expected_version < 0 {
            return Err(WenlanError::Validation(
                "brief summary expected_version must be non-negative".into(),
            ));
        }
    }
    for mutation in &request.mutations {
        match mutation {
            wenlan_types::BriefMutation::Add {
                text,
                details,
                added_at,
                ..
            } => {
                if text.trim().is_empty() {
                    return Err(WenlanError::Validation(
                        "brief item text must not be empty".into(),
                    ));
                }
                if let Some(details) = details {
                    validate_item_text("details", details, BRIEF_ITEM_DETAILS_MAX_CHARS)?;
                }
                if added_at.is_some_and(|value| value <= 0) {
                    return Err(WenlanError::Validation(
                        "brief item added_at must be positive".into(),
                    ));
                }
            }
            wenlan_types::BriefMutation::Edit {
                text,
                details,
                expected_version,
                ..
            } => {
                if text.trim().is_empty() {
                    return Err(WenlanError::Validation(
                        "brief item text must not be empty".into(),
                    ));
                }
                if let Some(details) = details {
                    validate_item_text("details", details, BRIEF_ITEM_DETAILS_MAX_CHARS)?;
                }
                if *expected_version < 1 {
                    return Err(WenlanError::Validation(
                        "brief item expected_version must be positive".into(),
                    ));
                }
            }
            wenlan_types::BriefMutation::Move {
                expected_version, ..
            }
            | wenlan_types::BriefMutation::SetGate {
                expected_version, ..
            }
            | wenlan_types::BriefMutation::Complete {
                expected_version, ..
            } => {
                if *expected_version < 1 {
                    return Err(WenlanError::Validation(
                        "brief item expected_version must be positive".into(),
                    ));
                }
            }
        }
    }
    Ok(())
}

fn update_request_digest(
    request: &wenlan_types::BriefUpdateRequest,
) -> Result<String, WenlanError> {
    use sha2::{Digest, Sha256};

    let encoded = serde_json::to_vec(request)?;
    Ok(format!("{:x}", Sha256::digest(encoded)))
}

async fn current_item_version(
    tx: &libsql::Transaction,
    space_id: &str,
    item_id: &str,
) -> Result<Option<i64>, WenlanError> {
    let mut rows = tx
        .query(
            "SELECT version FROM brief_items WHERE space_id=?1 AND id=?2",
            libsql::params![space_id, item_id],
        )
        .await
        .map_err(|error| WenlanError::VectorDb(format!("brief item version: {error}")))?;
    let version = rows
        .next()
        .await
        .map_err(|error| WenlanError::VectorDb(format!("brief item version row: {error}")))?
        .map(|row| row.get::<i64>(0).unwrap_or_default());
    Ok(version)
}

fn item_conflict(
    mutation_index: usize,
    item_id: &str,
    current_version: Option<i64>,
) -> wenlan_types::BriefMutationConflict {
    wenlan_types::BriefMutationConflict {
        mutation_index: Some(mutation_index),
        item_id: Some(item_id.to_string()),
        reason: if current_version.is_some() {
            wenlan_types::BriefConflictReason::VersionMismatch
        } else {
            wenlan_types::BriefConflictReason::ItemNotFound
        },
        current_version,
    }
}

fn item_matches_snapshot(
    snapshot: &HashMap<String, Option<i64>>,
    mutation_index: usize,
    item_id: &str,
    expected_version: i64,
    conflicts: &mut Vec<wenlan_types::BriefMutationConflict>,
) -> bool {
    let current_version = snapshot.get(item_id).copied().flatten();
    if current_version == Some(expected_version) {
        true
    } else {
        conflicts.push(item_conflict(mutation_index, item_id, current_version));
        false
    }
}

/// Read a Space's Brief on any connection, so the update transaction can
/// snapshot the exact state it is about to commit.
async fn read_brief(
    conn: &libsql::Connection,
    space_name: &str,
) -> Result<Option<wenlan_types::Brief>, WenlanError> {
    let header = {
        let mut rows = conn
            .query(
                "SELECT b.space_id, s.name, b.last_session_summary,
                        b.last_handoff_at, b.version
                   FROM briefs b
                   JOIN spaces s ON s.id=b.space_id
                  WHERE s.name=?1",
                libsql::params![space_name],
            )
            .await
            .map_err(|error| WenlanError::VectorDb(format!("get brief: {error}")))?;
        let Some(row) = rows
            .next()
            .await
            .map_err(|error| WenlanError::VectorDb(format!("get brief row: {error}")))?
        else {
            return Ok(None);
        };
        (
            row.get::<String>(0).unwrap_or_default(),
            row.get::<String>(1).unwrap_or_default(),
            row.get::<String>(2).unwrap_or_default(),
            row.get::<Option<i64>>(3).unwrap_or(None),
            row.get::<i64>(4).unwrap_or(1),
        )
    };

    let mut rows = conn
        .query(
            "SELECT id, text, details, state, added_at, gate, version, confirmed, filled_by_wenlan
               FROM brief_items
              WHERE space_id=?1
              ORDER BY CASE state WHEN 'active' THEN 0 ELSE 1 END,
                       added_at, id",
            libsql::params![header.0.as_str()],
        )
        .await
        .map_err(|error| WenlanError::VectorDb(format!("get brief items: {error}")))?;
    let mut active = Vec::new();
    let mut backlog = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|error| WenlanError::VectorDb(format!("get brief item row: {error}")))?
    {
        let state_text = row.get::<String>(3).unwrap_or_default();
        let state = match state_text.as_str() {
            "active" => wenlan_types::BriefItemState::Active,
            "backlog" => wenlan_types::BriefItemState::Backlog,
            other => {
                return Err(WenlanError::VectorDb(format!(
                    "brief item has invalid state '{other}'"
                )));
            }
        };
        let item = wenlan_types::BriefItem {
            id: row.get::<String>(0).unwrap_or_default(),
            text: row.get::<String>(1).unwrap_or_default(),
            details: row.get::<Option<String>>(2).unwrap_or(None),
            state,
            added_at: row.get::<i64>(4).unwrap_or_default(),
            gate: row.get::<Option<String>>(5).unwrap_or(None),
            version: row.get::<i64>(6).unwrap_or(1),
            confirmed: row.get::<i64>(7).unwrap_or(1) != 0,
            filled_by_wenlan: row.get::<i64>(8).unwrap_or(0) != 0,
        };
        match state {
            wenlan_types::BriefItemState::Active => active.push(item),
            wenlan_types::BriefItemState::Backlog => backlog.push(item),
        }
    }

    Ok(Some(wenlan_types::Brief {
        space_id: header.0,
        space: header.1,
        last_session_summary: header.2,
        last_handoff_at: header.3,
        version: header.4,
        active,
        backlog,
    }))
}

impl MemoryDB {
    pub async fn get_brief_by_space_name(
        &self,
        space_name: &str,
    ) -> Result<Option<wenlan_types::Brief>, WenlanError> {
        let conn = self.conn.lock().await;
        read_brief(&conn, space_name).await
    }

    pub async fn apply_brief_update(
        &self,
        request: &wenlan_types::BriefUpdateRequest,
    ) -> Result<wenlan_types::BriefUpdateReceipt, WenlanError> {
        self.apply_brief_update_in_session_outcome(request, None)
            .await
            .map(BriefUpdateOutcome::into_receipt)
    }

    /// [`Self::apply_brief_update`], recording `session` on the history row
    /// the update produces. A replayed operation records nothing new.
    pub async fn apply_brief_update_in_session(
        &self,
        request: &wenlan_types::BriefUpdateRequest,
        session: Option<&wenlan_types::SessionRef>,
    ) -> Result<wenlan_types::BriefUpdateReceipt, WenlanError> {
        self.apply_brief_update_in_session_outcome(request, session)
            .await
            .map(BriefUpdateOutcome::into_receipt)
    }

    /// Apply a Brief update while preserving whether the operation id was
    /// newly applied or replayed from its durable idempotency receipt.
    pub async fn apply_brief_update_in_session_outcome(
        &self,
        request: &wenlan_types::BriefUpdateRequest,
        session: Option<&wenlan_types::SessionRef>,
    ) -> Result<BriefUpdateOutcome, WenlanError> {
        self.apply_brief_update_with_origin(request, session, false)
            .await
    }

    async fn apply_brief_update_with_origin(
        &self,
        request: &wenlan_types::BriefUpdateRequest,
        session: Option<&wenlan_types::SessionRef>,
        filled_by_wenlan: bool,
    ) -> Result<BriefUpdateOutcome, WenlanError> {
        validate_update_request(request)?;
        let request_digest = update_request_digest(request)?;
        let _space_write_guard = self.space_write_lock.lock().await;
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(libsql::TransactionBehavior::Immediate)
            .await
            .map_err(|error| WenlanError::VectorDb(format!("brief update begin: {error}")))?;

        {
            let mut rows = tx
                .query(
                    "SELECT request_digest, response
                       FROM operation_receipts
                      WHERE caller_id=?1 AND operation_id=?2",
                    libsql::params![request.caller_id.as_str(), request.operation_id.as_str()],
                )
                .await
                .map_err(|error| {
                    WenlanError::VectorDb(format!("brief operation receipt lookup: {error}"))
                })?;
            if let Some(row) = rows.next().await.map_err(|error| {
                WenlanError::VectorDb(format!("brief operation receipt row: {error}"))
            })? {
                let stored_digest = row.get::<String>(0).unwrap_or_default();
                if stored_digest != request_digest {
                    return Err(WenlanError::Conflict(format!(
                        "operation id '{}' for '{}' was already used with a different request",
                        request.operation_id, request.caller_id
                    )));
                }
                let response = row.get::<String>(1).unwrap_or_default();
                let receipt = serde_json::from_str(&response)?;
                drop(rows);
                tx.commit().await.map_err(|error| {
                    WenlanError::VectorDb(format!("brief replay commit: {error}"))
                })?;
                return Ok(BriefUpdateOutcome::Duplicate(receipt));
            }
        }

        let now = chrono::Utc::now().timestamp();
        let space_id = {
            let existing = {
                let mut rows = tx
                    .query(
                        "SELECT id FROM spaces WHERE name=?1",
                        libsql::params![request.space.trim()],
                    )
                    .await
                    .map_err(|error| {
                        WenlanError::VectorDb(format!("brief space lookup: {error}"))
                    })?;
                rows.next()
                    .await
                    .map_err(|error| WenlanError::VectorDb(format!("brief space row: {error}")))?
                    .map(|row| row.get::<String>(0).unwrap_or_default())
            };
            if let Some(id) = existing {
                id
            } else {
                // A Brief update creates its Space on first use; apply the same
                // reserved names `create_space` rejects.
                super::reject_reserved_space_name(&request.space)?;
                let id = uuid::Uuid::new_v4().to_string();
                tx.execute(
                    "INSERT INTO spaces
                        (id, name, description, suggested, sort_order, created_at, updated_at)
                     VALUES (
                        ?1, ?2, NULL, 0,
                        (SELECT COALESCE(MAX(sort_order), -1) + 1 FROM spaces),
                        ?3, ?3
                     )",
                    libsql::params![id.as_str(), request.space.trim(), now as f64],
                )
                .await
                .map_err(|error| WenlanError::VectorDb(format!("brief create space: {error}")))?;
                id
            }
        };

        let existing_brief_version = {
            let mut rows = tx
                .query(
                    "SELECT version FROM briefs WHERE space_id=?1",
                    libsql::params![space_id.as_str()],
                )
                .await
                .map_err(|error| WenlanError::VectorDb(format!("brief version lookup: {error}")))?;
            rows.next()
                .await
                .map_err(|error| WenlanError::VectorDb(format!("brief version row: {error}")))?
                .map(|row| row.get::<i64>(0).unwrap_or(1))
        };
        let created_brief = existing_brief_version.is_none();
        let starting_brief_version = existing_brief_version.unwrap_or(1);
        if created_brief {
            tx.execute(
                "INSERT INTO briefs
                    (space_id, last_session_summary, last_handoff_at, version, updated_at)
                 VALUES (?1, '', NULL, 1, ?2)",
                libsql::params![space_id.as_str(), now],
            )
            .await
            .map_err(|error| WenlanError::VectorDb(format!("brief create: {error}")))?;
        }

        let mut applied = Vec::new();
        let mut conflicts = Vec::new();
        let mut changed = created_brief;

        if let Some(summary) = &request.summary {
            let version_matches = if created_brief {
                summary.expected_version == 0
            } else {
                summary.expected_version == starting_brief_version
            };
            if version_matches {
                tx.execute(
                    "UPDATE briefs SET last_session_summary=?1, updated_at=?2
                      WHERE space_id=?3",
                    libsql::params![summary.text.trim(), now, space_id.as_str()],
                )
                .await
                .map_err(|error| WenlanError::VectorDb(format!("brief summary update: {error}")))?;
                applied.push(wenlan_types::BriefAppliedMutation {
                    mutation_index: None,
                    kind: wenlan_types::BriefMutationKind::Summary,
                    item_id: None,
                    version: None,
                });
                changed = true;
            } else {
                conflicts.push(wenlan_types::BriefMutationConflict {
                    mutation_index: None,
                    item_id: None,
                    reason: wenlan_types::BriefConflictReason::BriefVersionMismatch,
                    current_version: Some(starting_brief_version),
                });
            }
        }

        // All deltas for one item compare against the same pre-request version.
        // The immediate transaction excludes concurrent writers; later deltas in this
        // request may therefore build on earlier ones without self-conflicting.
        let mut item_snapshot_versions = HashMap::new();
        for mutation in &request.mutations {
            let Some(item_id) = mutation.item_id() else {
                continue;
            };
            if !item_snapshot_versions.contains_key(item_id) {
                let version = current_item_version(&tx, &space_id, item_id).await?;
                item_snapshot_versions.insert(item_id.to_string(), version);
            }
        }

        for (mutation_index, mutation) in request.mutations.iter().enumerate() {
            match mutation {
                wenlan_types::BriefMutation::Add {
                    text,
                    details,
                    state,
                    added_at,
                    gate,
                } => {
                    let item_id = format!("brief_item_{}", uuid::Uuid::new_v4().simple());
                    let gate = gate
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty());
                    tx.execute(
                        "INSERT INTO brief_items
                            (id, space_id, text, details, state, added_at, gate, version,
                             confirmed, filled_by_wenlan, updated_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, 1, 0, ?8)",
                        libsql::params![
                            item_id.as_str(),
                            space_id.as_str(),
                            text.trim(),
                            details.as_deref().map(str::trim).filter(|v| !v.is_empty()),
                            state.as_str(),
                            added_at.unwrap_or(now),
                            gate,
                            now
                        ],
                    )
                    .await
                    .map_err(|error| WenlanError::VectorDb(format!("brief add item: {error}")))?;
                    applied.push(wenlan_types::BriefAppliedMutation {
                        mutation_index: Some(mutation_index),
                        kind: mutation.kind(),
                        item_id: Some(item_id),
                        version: Some(1),
                    });
                    changed = true;
                }
                wenlan_types::BriefMutation::Edit {
                    item_id,
                    expected_version,
                    text,
                    details,
                } => {
                    if !item_matches_snapshot(
                        &item_snapshot_versions,
                        mutation_index,
                        item_id,
                        *expected_version,
                        &mut conflicts,
                    ) {
                        continue;
                    }
                    let updated = tx
                        .execute(
                            "UPDATE brief_items
                                SET text=?1,
                                    details=CASE WHEN ?2 THEN NULLIF(trim(?3), '') ELSE details END,
                                    version=version+1, confirmed=?4,
                                    filled_by_wenlan=?5, updated_at=?6
                              WHERE space_id=?7 AND id=?8",
                            libsql::params![
                                text.trim(),
                                i64::from(details.is_some()),
                                details.as_deref(),
                                i64::from(!filled_by_wenlan),
                                i64::from(filled_by_wenlan),
                                now,
                                space_id.as_str(),
                                item_id.as_str()
                            ],
                        )
                        .await
                        .map_err(|error| {
                            WenlanError::VectorDb(format!("brief edit item: {error}"))
                        })?;
                    if updated == 1 {
                        applied.push(wenlan_types::BriefAppliedMutation {
                            mutation_index: Some(mutation_index),
                            kind: mutation.kind(),
                            item_id: Some(item_id.clone()),
                            version: current_item_version(&tx, &space_id, item_id).await?,
                        });
                        changed = true;
                    } else {
                        conflicts.push(item_conflict(
                            mutation_index,
                            item_id,
                            current_item_version(&tx, &space_id, item_id).await?,
                        ));
                    }
                }
                wenlan_types::BriefMutation::Move {
                    item_id,
                    expected_version,
                    state,
                } => {
                    if !item_matches_snapshot(
                        &item_snapshot_versions,
                        mutation_index,
                        item_id,
                        *expected_version,
                        &mut conflicts,
                    ) {
                        continue;
                    }
                    let updated = tx
                        .execute(
                            "UPDATE brief_items
                                SET state=?1, version=version+1, updated_at=?2
                              WHERE space_id=?3 AND id=?4",
                            libsql::params![
                                state.as_str(),
                                now,
                                space_id.as_str(),
                                item_id.as_str()
                            ],
                        )
                        .await
                        .map_err(|error| {
                            WenlanError::VectorDb(format!("brief move item: {error}"))
                        })?;
                    if updated == 1 {
                        applied.push(wenlan_types::BriefAppliedMutation {
                            mutation_index: Some(mutation_index),
                            kind: mutation.kind(),
                            item_id: Some(item_id.clone()),
                            version: current_item_version(&tx, &space_id, item_id).await?,
                        });
                        changed = true;
                    } else {
                        conflicts.push(item_conflict(
                            mutation_index,
                            item_id,
                            current_item_version(&tx, &space_id, item_id).await?,
                        ));
                    }
                }
                wenlan_types::BriefMutation::SetGate {
                    item_id,
                    expected_version,
                    gate,
                } => {
                    if !item_matches_snapshot(
                        &item_snapshot_versions,
                        mutation_index,
                        item_id,
                        *expected_version,
                        &mut conflicts,
                    ) {
                        continue;
                    }
                    let gate = gate
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty());
                    let updated = tx
                        .execute(
                            "UPDATE brief_items
                                SET gate=?1, version=version+1, updated_at=?2
                              WHERE space_id=?3 AND id=?4",
                            libsql::params![gate, now, space_id.as_str(), item_id.as_str()],
                        )
                        .await
                        .map_err(|error| {
                            WenlanError::VectorDb(format!("brief gate item: {error}"))
                        })?;
                    if updated == 1 {
                        applied.push(wenlan_types::BriefAppliedMutation {
                            mutation_index: Some(mutation_index),
                            kind: mutation.kind(),
                            item_id: Some(item_id.clone()),
                            version: current_item_version(&tx, &space_id, item_id).await?,
                        });
                        changed = true;
                    } else {
                        conflicts.push(item_conflict(
                            mutation_index,
                            item_id,
                            current_item_version(&tx, &space_id, item_id).await?,
                        ));
                    }
                }
                wenlan_types::BriefMutation::Complete {
                    item_id,
                    expected_version,
                } => {
                    if !item_matches_snapshot(
                        &item_snapshot_versions,
                        mutation_index,
                        item_id,
                        *expected_version,
                        &mut conflicts,
                    ) {
                        continue;
                    }
                    let deleted = tx
                        .execute(
                            "DELETE FROM brief_items
                              WHERE space_id=?1 AND id=?2",
                            libsql::params![space_id.as_str(), item_id.as_str()],
                        )
                        .await
                        .map_err(|error| {
                            WenlanError::VectorDb(format!("brief complete item: {error}"))
                        })?;
                    if deleted == 1 {
                        applied.push(wenlan_types::BriefAppliedMutation {
                            mutation_index: Some(mutation_index),
                            kind: mutation.kind(),
                            item_id: Some(item_id.clone()),
                            version: None,
                        });
                        changed = true;
                    } else {
                        conflicts.push(item_conflict(
                            mutation_index,
                            item_id,
                            current_item_version(&tx, &space_id, item_id).await?,
                        ));
                    }
                }
            }
        }

        let final_brief_version = if changed && !created_brief {
            tx.execute(
                "UPDATE briefs
                    SET version=version+1, last_handoff_at=?1, updated_at=?1
                  WHERE space_id=?2",
                libsql::params![now, space_id.as_str()],
            )
            .await
            .map_err(|error| WenlanError::VectorDb(format!("brief version bump: {error}")))?;
            starting_brief_version + 1
        } else {
            if changed {
                tx.execute(
                    "UPDATE briefs SET last_handoff_at=?1, updated_at=?1 WHERE space_id=?2",
                    libsql::params![now, space_id.as_str()],
                )
                .await
                .map_err(|error| {
                    WenlanError::VectorDb(format!("brief handoff timestamp: {error}"))
                })?;
            }
            starting_brief_version
        };
        for entry in &mut applied {
            if entry.kind == wenlan_types::BriefMutationKind::Summary {
                entry.version = Some(final_brief_version);
            }
        }

        let receipt = wenlan_types::BriefUpdateReceipt {
            space: request.space.trim().to_string(),
            brief_version: final_brief_version,
            applied,
            conflicts,
            projection_path: None,
            warnings: Vec::new(),
        };
        if !receipt.applied.is_empty() {
            let snapshot = read_brief(&tx, request.space.trim())
                .await?
                .ok_or_else(|| WenlanError::VectorDb("brief missing after update".into()))?;
            let session_id = match session {
                Some(session) => Some(
                    super::sessions::upsert_session_span(
                        &tx,
                        session,
                        None,
                        Some(request.space.trim()),
                        now,
                        now,
                    )
                    .await
                    .map_err(|error| {
                        WenlanError::VectorDb(format!("brief history session: {error}"))
                    })?,
                ),
                None => None,
            };
            tx.execute(
                "INSERT INTO brief_history
                    (space_id, brief_version, created_at, caller_id, operation_id,
                     summary, mutations_json, snapshot_json, session_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                libsql::params![
                    space_id.as_str(),
                    final_brief_version,
                    now,
                    request.caller_id.as_str(),
                    request.operation_id.as_str(),
                    snapshot.last_session_summary.as_str(),
                    serde_json::to_string(&receipt.applied)?,
                    serde_json::to_string(&snapshot)?,
                    session_id
                ],
            )
            .await
            .map_err(|error| WenlanError::VectorDb(format!("brief history insert: {error}")))?;
        }
        let response = serde_json::to_string(&receipt)?;
        tx.execute(
            "INSERT INTO operation_receipts
                (caller_id, operation_id, request_digest, response, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            libsql::params![
                request.caller_id.as_str(),
                request.operation_id.as_str(),
                request_digest.as_str(),
                response,
                now
            ],
        )
        .await
        .map_err(|error| {
            WenlanError::Conflict(format!(
                "operation id '{}' for '{}' was already used: {error}",
                request.operation_id, request.caller_id
            ))
        })?;

        tx.commit()
            .await
            .map_err(|error| WenlanError::VectorDb(format!("brief update commit: {error}")))?;
        Ok(BriefUpdateOutcome::Applied(receipt))
    }

    /// Rule 2: once a local conversation has been idle for `idle_secs`, fill
    /// its last handed-off item from the later transcript if the session has
    /// not made a newer handoff. `now` is injected so tests never sleep.
    pub async fn fill_idle_handoffs(
        &self,
        conversations: &[crate::conversation_intake::FoundConversation],
        shared_space: &(dyn Fn(&str) -> Result<bool, String> + Sync),
        model: Option<&dyn crate::llm_provider::LlmProvider>,
        language: &str,
        now: i64,
        idle_secs: i64,
    ) -> Result<usize, WenlanError> {
        let mut filled = 0;
        for conversation in conversations {
            if conversation.last_at > now.saturating_sub(idle_secs) {
                continue;
            }
            let Some(session_id) = super::intake::found_session_id(conversation) else {
                continue;
            };
            let latest = {
                let conn = self.conn.lock().await;
                let mut rows = conn
                    .query(
                        "SELECT h.created_at, h.mutations_json, h.snapshot_json, s.name
                           FROM brief_history h
                           JOIN spaces s ON s.id=h.space_id
                          WHERE h.session_id=?1
                          ORDER BY h.created_at DESC, h.brief_version DESC
                          LIMIT 1",
                        libsql::params![session_id],
                    )
                    .await
                    .map_err(|e| WenlanError::VectorDb(format!("idle handoff lookup: {e}")))?;
                rows.next()
                    .await
                    .map_err(|e| WenlanError::VectorDb(format!("idle handoff row: {e}")))?
                    .map(|row| {
                        (
                            row.get::<i64>(0).unwrap_or_default(),
                            row.get::<String>(1).unwrap_or_default(),
                            row.get::<String>(2).unwrap_or_default(),
                            row.get::<String>(3).unwrap_or_default(),
                        )
                    })
            };
            let Some((handed_at, mutations_json, snapshot_json, space)) = latest else {
                continue;
            };
            if shared_space(&space).unwrap_or(true) {
                continue;
            }
            if handed_at >= conversation.last_content_at {
                continue;
            }
            let mutations: Vec<wenlan_types::BriefAppliedMutation> =
                serde_json::from_str(&mutations_json)?;
            let Some((item_id, handed_version)) = mutations
                .iter()
                .rev()
                .find_map(|mutation| Some((mutation.item_id.clone()?, mutation.version?)))
            else {
                continue;
            };
            let snapshot: wenlan_types::Brief = serde_json::from_str(&snapshot_json)?;
            let Some(prior) = snapshot
                .active
                .iter()
                .chain(snapshot.backlog.iter())
                .find(|item| item.id == item_id)
            else {
                continue;
            };
            let Some(current) = self
                .get_brief_by_space_name(&space)
                .await?
                .and_then(|brief| {
                    brief
                        .active
                        .into_iter()
                        .chain(brief.backlog)
                        .find(|item| item.id == item_id)
                })
            else {
                continue;
            };
            if current.filled_by_wenlan || current.version != handed_version {
                continue;
            }
            // The reader has already established that at least one real
            // person/AI message is newer than the handoff. Offer only its
            // bounded tail fields, never tool or metadata events.
            let transcript = conversation.last_content.trim().to_string();
            if transcript.is_empty() {
                continue;
            }
            let transcript: String = transcript.chars().take(4_000).collect();
            let fallback_details: String = transcript.chars().take(1_000).collect();
            let generated = if let Some(model) = model.filter(|model| model.is_available()) {
                let prompt = format!(
                    "Previous Brief line:\n{}\n\nPrevious AI-facing details:\n{}\n\nLater conversation:\n{}\n\nReturn JSON only as {{\"text\":\"one person-facing line\",\"details\":\"goal, decisions and why, tried and failed, unverified, next\"}}.",
                    prior.text,
                    prior.details.as_deref().unwrap_or("(none)"),
                    transcript
                );
                let request = crate::llm_provider::LlmRequest {
                    system_prompt: Some(format!(
                        "Update one handoff item. Write the line and the details in {language}. Do not quote raw transcript text in the person-facing line. Answer with JSON only."
                    )),
                    user_prompt: prompt,
                    max_tokens: 700,
                    temperature: 0.0,
                    label: Some("idle_handoff_fill".into()),
                    timeout_secs: Some(30),
                };
                tokio::time::timeout(std::time::Duration::from_secs(30), model.generate(request))
                    .await
                    .ok()
                    .and_then(Result::ok)
                    .and_then(|answer| serde_json::from_str::<IdleFillAnswer>(answer.trim()).ok())
                    .and_then(|answer| answer.validated(&transcript))
            } else {
                None
            };
            let (text, details) = generated.unwrap_or_else(|| {
                let details = if fallback_details.is_empty() {
                    prior.details.clone()
                } else {
                    Some(fallback_details)
                };
                (prior.text.clone(), details)
            });
            // Sharing may have changed while the model was running. Fail
            // closed at the last point before the version-guarded write.
            if shared_space(&space).unwrap_or(true) {
                continue;
            }
            let request = wenlan_types::BriefUpdateRequest {
                space,
                caller_id: "wenlan-idle-fill".into(),
                operation_id: format!(
                    "idle-{}-{}-{}",
                    conversation.tool, conversation.external_id, conversation.last_at
                ),
                summary: None,
                mutations: vec![wenlan_types::BriefMutation::Edit {
                    item_id,
                    expected_version: current.version,
                    text,
                    details,
                }],
            };
            let receipt = self
                .apply_brief_update_with_origin(&request, None, true)
                .await?
                .into_receipt();
            if !receipt.applied.is_empty() {
                filled += 1;
            }
        }
        Ok(filled)
    }
}

#[derive(serde::Deserialize)]
struct IdleFillAnswer {
    text: String,
    details: String,
}

impl IdleFillAnswer {
    fn validated(self, transcript: &str) -> Option<(String, Option<String>)> {
        let text = self.text.trim();
        let details = self.details.trim();
        if text.is_empty()
            || details.is_empty()
            || text.chars().count() > BRIEF_ITEM_TEXT_MAX_CHARS
            || details.chars().count() > BRIEF_ITEM_DETAILS_MAX_CHARS
            || echoes_transcript(text, transcript)
            || echoes_transcript(details, transcript)
        {
            return None;
        }
        Some((text.to_string(), Some(details.to_string())))
    }
}

fn echoes_transcript(value: &str, transcript: &str) -> bool {
    const VERBATIM_RUN_CHARS: usize = 40;
    let normalize = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    let value = normalize(value);
    let transcript: Vec<char> = normalize(transcript).chars().collect();
    if transcript.len() < VERBATIM_RUN_CHARS {
        return false;
    }
    transcript.windows(VERBATIM_RUN_CHARS).any(|window| {
        let run: String = window.iter().collect();
        value.contains(&run)
    })
}

fn history_entry_from_row(
    row: &libsql::Row,
) -> Result<wenlan_types::BriefHistoryEntry, WenlanError> {
    let mutations: Vec<wenlan_types::BriefAppliedMutation> =
        serde_json::from_str(&row.get::<String>(5).unwrap_or_default())?;
    Ok(wenlan_types::BriefHistoryEntry {
        version: row.get::<i64>(0).unwrap_or_default(),
        created_at: row.get::<i64>(1).unwrap_or_default(),
        caller_id: row.get::<String>(2).unwrap_or_default(),
        operation_id: row.get::<String>(3).unwrap_or_default(),
        summary: row.get::<String>(4).unwrap_or_default(),
        mutation_count: mutations.len(),
        mutations,
        session_id: row.get::<Option<String>>(6).unwrap_or(None),
    })
}

impl MemoryDB {
    /// Brief update history for a Space, newest first. Empty when the Space
    /// has no Brief or no recorded updates.
    pub async fn list_brief_history(
        &self,
        space_name: &str,
        limit: usize,
    ) -> Result<Vec<wenlan_types::BriefHistoryEntry>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT h.brief_version, h.created_at, h.caller_id, h.operation_id,
                        h.summary, h.mutations_json, h.session_id
                   FROM brief_history h
                   JOIN spaces s ON s.id=h.space_id
                  WHERE s.name=?1
                  ORDER BY h.brief_version DESC
                  LIMIT ?2",
                libsql::params![space_name, limit as i64],
            )
            .await
            .map_err(|error| WenlanError::VectorDb(format!("list brief history: {error}")))?;
        let mut entries = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|error| WenlanError::VectorDb(format!("brief history row: {error}")))?
        {
            entries.push(history_entry_from_row(&row)?);
        }
        Ok(entries)
    }

    /// The Brief as it stood right after update `version`, or `None` when
    /// that version was never recorded for the Space.
    pub async fn get_brief_version(
        &self,
        space_name: &str,
        version: i64,
    ) -> Result<Option<wenlan_types::BriefVersionResponse>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT h.brief_version, h.created_at, h.caller_id, h.operation_id,
                        h.summary, h.mutations_json, h.session_id, h.snapshot_json
                   FROM brief_history h
                   JOIN spaces s ON s.id=h.space_id
                  WHERE s.name=?1 AND h.brief_version=?2",
                libsql::params![space_name, version],
            )
            .await
            .map_err(|error| WenlanError::VectorDb(format!("get brief version: {error}")))?;
        let Some(row) = rows
            .next()
            .await
            .map_err(|error| WenlanError::VectorDb(format!("brief version row: {error}")))?
        else {
            return Ok(None);
        };
        let entry = history_entry_from_row(&row)?;
        let mut brief: wenlan_types::Brief =
            serde_json::from_str(&row.get::<String>(7).unwrap_or_default())?;
        // The snapshot predates any later Space rename; show the current name.
        brief.space = space_name.to_string();
        Ok(Some(wenlan_types::BriefVersionResponse { entry, brief }))
    }
}

impl MemoryDB {
    pub async fn import_legacy_brief(
        &self,
        source_key: &str,
        space_name: &str,
        last_session_summary: &str,
        last_handoff_at: Option<i64>,
        items: &[LegacyBriefItem],
    ) -> Result<bool, WenlanError> {
        let source_key = source_key.trim();
        let space_name = space_name.trim();
        if source_key.is_empty() {
            return Err(WenlanError::Validation(
                "legacy Brief import requires a non-empty source key".into(),
            ));
        }
        if space_name.is_empty() {
            return Err(WenlanError::Validation(
                "legacy Brief import requires a non-empty space".into(),
            ));
        }
        if items.iter().any(|item| item.text.trim().is_empty()) {
            return Err(WenlanError::Validation(
                "legacy Brief item text must not be empty".into(),
            ));
        }

        let _space_write_guard = self.space_write_lock.lock().await;
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(libsql::TransactionBehavior::Immediate)
            .await
            .map_err(|error| {
                WenlanError::VectorDb(format!("legacy Brief import begin: {error}"))
            })?;
        let now = chrono::Utc::now().timestamp();

        let already_imported = {
            let mut rows = tx
                .query(
                    "SELECT 1 FROM brief_legacy_imports WHERE source_key=?1",
                    libsql::params![source_key],
                )
                .await
                .map_err(|error| {
                    WenlanError::VectorDb(format!("legacy Brief marker lookup: {error}"))
                })?;
            rows.next()
                .await
                .map_err(|error| {
                    WenlanError::VectorDb(format!("legacy Brief marker row: {error}"))
                })?
                .is_some()
        };
        if already_imported {
            tx.commit().await.map_err(|error| {
                WenlanError::VectorDb(format!("legacy Brief marker skip commit: {error}"))
            })?;
            return Ok(false);
        }

        let space_id = {
            let existing = {
                let mut rows = tx
                    .query(
                        "SELECT id FROM spaces WHERE name=?1",
                        libsql::params![space_name],
                    )
                    .await
                    .map_err(|error| {
                        WenlanError::VectorDb(format!("legacy Brief space lookup: {error}"))
                    })?;
                rows.next()
                    .await
                    .map_err(|error| {
                        WenlanError::VectorDb(format!("legacy Brief space row: {error}"))
                    })?
                    .map(|row| row.get::<String>(0).unwrap_or_default())
            };
            if let Some(id) = existing {
                id
            } else {
                let id = uuid::Uuid::new_v4().to_string();
                tx.execute(
                    "INSERT INTO spaces
                        (id, name, description, suggested, sort_order, created_at, updated_at)
                     VALUES (
                        ?1, ?2, NULL, 0,
                        (SELECT COALESCE(MAX(sort_order), -1) + 1 FROM spaces),
                        ?3, ?3
                     )",
                    libsql::params![id.as_str(), space_name, now as f64],
                )
                .await
                .map_err(|error| {
                    WenlanError::VectorDb(format!("legacy Brief create space: {error}"))
                })?;
                id
            }
        };

        let brief_exists = {
            let mut rows = tx
                .query(
                    "SELECT 1 FROM briefs WHERE space_id=?1",
                    libsql::params![space_id.as_str()],
                )
                .await
                .map_err(|error| {
                    WenlanError::VectorDb(format!("legacy Brief existence check: {error}"))
                })?;
            rows.next()
                .await
                .map_err(|error| {
                    WenlanError::VectorDb(format!("legacy Brief existence row: {error}"))
                })?
                .is_some()
        };
        if brief_exists {
            tx.execute(
                "INSERT INTO brief_legacy_imports (source_key, space_name, imported_at) VALUES (?1, ?2, ?3)",
                libsql::params![source_key, space_name, now],
            )
            .await
            .map_err(|error| {
                WenlanError::VectorDb(format!("legacy Brief marker insert: {error}"))
            })?;
            tx.commit().await.map_err(|error| {
                WenlanError::VectorDb(format!("legacy Brief skip commit: {error}"))
            })?;
            return Ok(false);
        }

        tx.execute(
            "INSERT INTO briefs
                (space_id, last_session_summary, last_handoff_at, version, updated_at)
             VALUES (?1, ?2, ?3, 1, ?4)",
            libsql::params![
                space_id.as_str(),
                last_session_summary.trim(),
                last_handoff_at,
                now
            ],
        )
        .await
        .map_err(|error| WenlanError::VectorDb(format!("legacy Brief insert: {error}")))?;

        for item in items {
            let id = format!("brief_item_{}", uuid::Uuid::new_v4().simple());
            let gate = item
                .gate
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty());
            tx.execute(
                "INSERT INTO brief_items
                    (id, space_id, text, state, added_at, gate, version, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7)",
                libsql::params![
                    id,
                    space_id.as_str(),
                    item.text.trim(),
                    item.state.as_str(),
                    item.added_at,
                    gate,
                    now
                ],
            )
            .await
            .map_err(|error| WenlanError::VectorDb(format!("legacy Brief item insert: {error}")))?;
        }

        tx.execute(
            "INSERT INTO brief_legacy_imports (source_key, space_name, imported_at) VALUES (?1, ?2, ?3)",
            libsql::params![source_key, space_name, now],
        )
        .await
        .map_err(|error| {
            WenlanError::VectorDb(format!("legacy Brief marker insert: {error}"))
        })?;

        tx.commit()
            .await
            .map_err(|error| WenlanError::VectorDb(format!("legacy Brief commit: {error}")))?;
        Ok(true)
    }
}
