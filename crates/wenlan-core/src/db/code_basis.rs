// SPDX-License-Identifier: Apache-2.0
//! A note can rely on lines of code: its **code basis**.
//!
//! The sibling of `page_basis` for code. A row records which lines of which
//! file in which repository a note relies on, plus the fingerprint
//! ([`CodeAnchor`]) of those lines when the note was last confirmed against
//! them. Nothing here reads a file: the caller reads, computes anchors and
//! verdicts (`crate::code_basis`), and this module stores them.
//!
//! | state         | meaning                                                              |
//! |---------------|----------------------------------------------------------------------|
//! | `ok`          | the lines still hash the same (whitespace and CRLF do not count)    |
//! | `moved`       | same content, other line numbers; the row already holds the new span |
//! | `pending`     | the content changed or the lines cannot be found                    |
//! | `unavailable` | the file or repository is gone, unreadable, too large or escapes it  |
//!
//! A row stays `moved` (it is not quietly turned back to `ok` by the next
//! check) until the note is written again with that span or the row is
//! confirmed. A `pending` row keeps the span and anchor it was confirmed with,
//! so reverting the file turns it `ok` again, and keeps its first `changed_at`
//! while it stays pending.
//!
//! Unlike `page_basis`, a code basis never depends on another page's state, so
//! no page writer has to re-evaluate it. Rows follow their note: deleted with
//! it, copied to the survivor of a merge, re-pointed when a source note changes
//! id.

use super::{MemoryDB, UNFILED_SPACE_ID};
use crate::code_anchor::{CodeAnchor, LineSpan};
use crate::code_basis::{
    CodeBasisError, FileStamp, PreparedCodeBasis, CODE_BASIS_STATE_MOVED, CODE_BASIS_STATE_OK,
    CODE_BASIS_STATE_PENDING, CODE_BASIS_STATE_UNAVAILABLE,
};
use crate::error::WenlanError;
use std::collections::HashMap;
use wenlan_types::{CodeBasisCitedResponse, CodeBasisLocator, CodeBasisRef};

pub(super) const PAGE_CODE_BASIS_DDL: &str = "
    CREATE TABLE IF NOT EXISTS page_code_basis (
        page_id TEXT NOT NULL,
        repo_root TEXT NOT NULL,
        path TEXT NOT NULL,
        line_start INTEGER NOT NULL,
        line_end INTEGER NOT NULL,
        anchor_json TEXT NOT NULL,
        confirmed_digest TEXT NOT NULL,
        confirmed_by TEXT NOT NULL,
        confirmed_at INTEGER NOT NULL,
        state TEXT NOT NULL CHECK(state IN ('ok','moved','pending','unavailable')),
        unavailable_reason TEXT,
        changed_at INTEGER,
        file_len INTEGER,
        file_mtime_ns INTEGER,
        cited_blob TEXT,
        recheck_note TEXT,
        recheck_verdict TEXT,
        recheck_digest TEXT,
        recheck_attempts INTEGER NOT NULL DEFAULT 0,
        added_by TEXT NOT NULL,
        added_at INTEGER NOT NULL,
        PRIMARY KEY (page_id, repo_root, path, line_start, line_end)
    );
    CREATE INDEX IF NOT EXISTS page_code_basis_by_page ON page_code_basis(page_id, state);";

/// What the background model recheck has recorded about a row
/// (`crate::code_basis_recheck`). All four clear when the row is confirmed,
/// removed or re-anchored.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CodeBasisRecheckState {
    /// The sentence the model left about the current change: what changed
    /// (`needs_human`) or what the note should now say (`stale`). Empty or
    /// absent: the model has not judged the current change.
    pub note: Option<String>,
    /// What the model concluded: `stale` or `needs_human`. Absent while the
    /// model has not judged the current change (or gave no verdict).
    pub verdict: Option<String>,
    /// The digest of the changed lines the model was last asked about, so a
    /// further change to the file is judged afresh.
    pub digest: Option<String>,
    /// Attempts at that digest that ended without a verdict.
    pub attempts: i64,
}

/// One stored code basis with the fingerprint a check compares against.
#[derive(Debug, Clone)]
pub struct CodeBasisRow {
    pub page_id: String,
    /// The note's title (for a recheck report).
    pub title: String,
    pub reference: CodeBasisRef,
    pub anchor: CodeAnchor,
    /// The anchor exactly as stored. A verdict is applied only while the row
    /// still holds it, so a check that raced a write cannot undo the write.
    pub anchor_json: String,
    /// The file's length and modification time when the row was last written,
    /// confirmed or checked. A check that finds both unchanged on an `ok` or
    /// `moved` row skips reading the file.
    pub stamp: Option<FileStamp>,
    /// What the background model recheck recorded for this row.
    pub recheck: CodeBasisRecheckState,
}

/// What a check of one row decided.
#[derive(Debug, Clone)]
pub enum CodeBasisVerdict {
    /// The lines are where they were and read the same.
    Unchanged { stamp: Option<FileStamp> },
    /// The same content now sits at `span`; `anchor` is its fresh fingerprint.
    Moved {
        span: LineSpan,
        anchor: CodeAnchor,
        stamp: Option<FileStamp>,
    },
    /// The content changed, or the lines could not be found.
    Pending,
    /// The file could not be read; the word is the reason.
    Unavailable(&'static str),
}

const REF_COLUMNS: &str =
    "repo_root, path, line_start, line_end, state, unavailable_reason, changed_at, recheck_note, \
     recheck_verdict";

fn ref_from(row: &libsql::Row) -> Result<CodeBasisRef, libsql::Error> {
    let state: String = row.get(4)?;
    let reason: Option<String> = row.get(5)?;
    let changed_at: Option<i64> = row.get(6)?;
    let recheck_note: Option<String> = row.get(7)?;
    let recheck_verdict: Option<String> = row.get(8)?;
    Ok(CodeBasisRef {
        repo_root: row.get(0)?,
        path: row.get(1)?,
        line_start: u32::try_from(row.get::<i64>(2)?).unwrap_or(u32::MAX),
        line_end: u32::try_from(row.get::<i64>(3)?).unwrap_or(u32::MAX),
        // Only the reason for an unavailable row and the first-seen time of a
        // pending one mean anything to a reader.
        reason: if state == CODE_BASIS_STATE_UNAVAILABLE {
            reason
        } else {
            None
        },
        changed_at: if state == CODE_BASIS_STATE_PENDING {
            changed_at
        } else {
            None
        },
        // The model's sentence and verdict mean something only while the row
        // waits for the main model or a person.
        explanation: if state == CODE_BASIS_STATE_PENDING {
            recheck_note
                .map(|note| note.trim().to_string())
                .filter(|note| !note.is_empty())
        } else {
            None
        },
        ai_verdict: if state == CODE_BASIS_STATE_PENDING {
            recheck_verdict
                .map(|verdict| verdict.trim().to_string())
                .filter(|verdict| !verdict.is_empty())
        } else {
            None
        },
        state,
    })
}

fn decode(context: &'static str) -> impl Fn(libsql::Error) -> WenlanError {
    move |e| WenlanError::VectorDb(format!("{context}: {e}"))
}

/// The row-with-anchor shape used by checks: `REF_COLUMNS`, then anchor_json,
/// page id, the note's title, the file stamp and the model recheck state.
pub(crate) const ROW_SELECT: &str =
    "SELECT b.repo_root, b.path, b.line_start, b.line_end, b.state, \
    b.unavailable_reason, b.changed_at, b.recheck_note, b.recheck_verdict, b.anchor_json, \
    b.page_id, c.title, b.file_len, b.file_mtime_ns, b.recheck_digest, b.recheck_attempts \
    FROM page_code_basis b JOIN pages c ON c.id = b.page_id";

fn stamp_from(len: Option<i64>, mtime_ns: Option<i64>) -> Option<FileStamp> {
    Some(FileStamp {
        len: len?,
        mtime_ns: mtime_ns?,
    })
}

pub(crate) fn row_from(row: &libsql::Row) -> Result<Option<CodeBasisRow>, libsql::Error> {
    let anchor_json: String = row.get(9)?;
    // An anchor that no longer parses cannot be compared against; leave the
    // row out of checks rather than mislabel it.
    let Ok(anchor) = serde_json::from_str::<CodeAnchor>(&anchor_json) else {
        return Ok(None);
    };
    Ok(Some(CodeBasisRow {
        recheck: CodeBasisRecheckState {
            note: row.get::<Option<String>>(7)?,
            verdict: row.get::<Option<String>>(8)?,
            digest: row.get::<Option<String>>(14)?,
            attempts: row.get::<Option<i64>>(15)?.unwrap_or(0),
        },
        reference: ref_from(row)?,
        anchor,
        anchor_json,
        page_id: row.get(10)?,
        title: row.get(11)?,
        stamp: stamp_from(row.get::<Option<i64>>(12)?, row.get::<Option<i64>>(13)?),
    }))
}

/// Only an active, non-entity note can rely on code.
async fn require_citer(conn: &libsql::Connection, page_id: &str) -> Result<(), CodeBasisError> {
    let mut rows = conn
        .query(
            "SELECT status, COALESCE(kind, 'concept') FROM pages WHERE id = ?1",
            libsql::params![page_id],
        )
        .await?;
    let Some(row) = rows.next().await? else {
        return Err(CodeBasisError::Invalid(format!(
            "the note {page_id} was not found"
        )));
    };
    let status: String = row.get(0)?;
    let kind: String = row.get(1)?;
    if kind == "entity" {
        return Err(CodeBasisError::Invalid(
            "an entity page cannot rely on code".into(),
        ));
    }
    match status.as_str() {
        "active" => Ok(()),
        "draft" => Err(CodeBasisError::Invalid(
            "a draft cannot rely on code".into(),
        )),
        _ => Err(CodeBasisError::Invalid(
            "only an active note can rely on code".into(),
        )),
    }
}

/// Record that `page_id` relies on each `(span, anchor)` as of now, on the
/// caller's open transaction. A span the note already has is re-anchored and
/// returns to `ok`; spans it has that this write does not name stay as they
/// are (the same rule as `basis`: a write adds, it never silently drops), except
/// that a span this write names replaces the note's stale (`pending`, `moved`
/// or `unavailable`) rows on the same file whose lines overlap it.
pub(super) async fn upsert_code_basis_on_conn(
    conn: &libsql::Connection,
    page_id: &str,
    entries: &[PreparedCodeBasis],
    by: &str,
    now: i64,
) -> Result<(), CodeBasisError> {
    if entries.is_empty() {
        return Ok(());
    }
    require_citer(conn, page_id).await?;
    for PreparedCodeBasis {
        input,
        anchor,
        stamp,
        cited_blob,
    } in entries
    {
        let anchor_json = serde_json::to_string(anchor)?;
        // The note now names where this code reads; a stale row for the lines
        // it used to occupy would only keep the Brief counting the note. Rows
        // in state `ok` are deliberate and stay; the exact same span is
        // updated by the upsert below, keeping its history.
        conn.execute(
            "DELETE FROM page_code_basis \
             WHERE page_id = ?1 AND repo_root = ?2 AND path = ?3 AND state <> 'ok' \
               AND line_start <= ?5 AND line_end >= ?4 \
               AND NOT (line_start = ?4 AND line_end = ?5)",
            libsql::params![
                page_id,
                input.repo_root.as_str(),
                input.path.as_str(),
                i64::from(input.line_start),
                i64::from(input.line_end)
            ],
        )
        .await?;
        conn.execute(
            "INSERT INTO page_code_basis \
               (page_id, repo_root, path, line_start, line_end, anchor_json, confirmed_digest, \
                confirmed_by, confirmed_at, state, unavailable_reason, changed_at, \
                file_len, file_mtime_ns, cited_blob, added_by, added_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'ok', NULL, NULL, ?10, ?11, ?12, ?8, ?9) \
             ON CONFLICT(page_id, repo_root, path, line_start, line_end) DO UPDATE SET \
               anchor_json = excluded.anchor_json, \
               confirmed_digest = excluded.confirmed_digest, \
               confirmed_by = excluded.confirmed_by, \
               confirmed_at = excluded.confirmed_at, \
               file_len = excluded.file_len, file_mtime_ns = excluded.file_mtime_ns, \
               cited_blob = excluded.cited_blob, \
               state = 'ok', unavailable_reason = NULL, changed_at = NULL, \
               recheck_note = NULL, recheck_verdict = NULL, recheck_digest = NULL, \
               recheck_attempts = 0",
            libsql::params![
                page_id,
                input.repo_root.as_str(),
                input.path.as_str(),
                i64::from(input.line_start),
                i64::from(input.line_end),
                anchor_json,
                anchor.digest.as_str(),
                by,
                now,
                stamp.map(|s| s.len),
                stamp.map(|s| s.mtime_ns),
                cited_blob.as_deref()
            ],
        )
        .await?;
    }
    Ok(())
}

/// A deleted note's code bases go with it (same transaction as the delete).
pub(super) async fn delete_code_basis_for_page(
    conn: &libsql::Connection,
    page_id: &str,
) -> Result<(), libsql::Error> {
    conn.execute(
        "DELETE FROM page_code_basis WHERE page_id = ?1",
        libsql::params![page_id],
    )
    .await?;
    Ok(())
}

/// When `absorbed` is merged into `survivor`, the survivor relies on the code
/// the absorbed note relied on: its rows are copied across as they are (the
/// anchors describe the code, not the note). A span the survivor already has
/// keeps the survivor's own row. The absorbed note is archived and its rows
/// stay with it, out of every check.
pub(super) async fn carry_code_basis_into_survivor(
    conn: &libsql::Connection,
    absorbed_id: &str,
    survivor_id: &str,
) -> Result<(), libsql::Error> {
    conn.execute(
        "INSERT OR IGNORE INTO page_code_basis \
           (page_id, repo_root, path, line_start, line_end, anchor_json, confirmed_digest, \
            confirmed_by, confirmed_at, state, unavailable_reason, changed_at, \
            file_len, file_mtime_ns, cited_blob, added_by, added_at) \
         SELECT ?2, repo_root, path, line_start, line_end, anchor_json, confirmed_digest, \
                confirmed_by, confirmed_at, state, unavailable_reason, changed_at, \
                file_len, file_mtime_ns, cited_blob, added_by, added_at \
         FROM page_code_basis WHERE page_id = ?1",
        libsql::params![absorbed_id, survivor_id],
    )
    .await?;
    Ok(())
}

/// A SOURCE note keeps its content but changes id: its code bases follow it.
pub(super) async fn retarget_code_basis(
    conn: &libsql::Connection,
    old_page_id: &str,
    new_page_id: &str,
) -> Result<(), libsql::Error> {
    conn.execute(
        "UPDATE OR IGNORE page_code_basis SET page_id = ?2 WHERE page_id = ?1",
        libsql::params![old_page_id, new_page_id],
    )
    .await?;
    delete_code_basis_for_page(conn, old_page_id).await
}

/// Check that `span` is free for `reference`'s row to move to. Another row of
/// the same note already sitting there is a conflict, never deleted: the
/// caller says so and the person decides which reliance to keep.
async fn ensure_span_free(
    conn: &libsql::Connection,
    page_id: &str,
    reference: &CodeBasisRef,
    span: LineSpan,
) -> Result<(), CodeBasisError> {
    if span.start == reference.line_start && span.end == reference.line_end {
        return Ok(());
    }
    let mut rows = conn
        .query(
            "SELECT 1 FROM page_code_basis \
             WHERE page_id = ?1 AND repo_root = ?2 AND path = ?3 \
               AND line_start = ?4 AND line_end = ?5",
            libsql::params![
                page_id,
                reference.repo_root.as_str(),
                reference.path.as_str(),
                i64::from(span.start),
                i64::from(span.end)
            ],
        )
        .await?;
    if rows.next().await?.is_some() {
        return Err(CodeBasisError::Conflict(format!(
            "{}: another reliance of this note already covers lines {}-{}; remove one of them",
            reference.path, span.start, span.end
        )));
    }
    Ok(())
}

impl MemoryDB {
    /// Record that `page_id` relies on these lines (one transaction). For the
    /// writes that have no page write to ride on: a create, and a refresh whose
    /// text did not change. See [`upsert_code_basis_on_conn`] for the rules.
    pub async fn upsert_page_code_basis(
        &self,
        page_id: &str,
        entries: &[PreparedCodeBasis],
        by: &str,
    ) -> Result<(), CodeBasisError> {
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(libsql::TransactionBehavior::Immediate)
            .await?;
        upsert_code_basis_on_conn(&tx, page_id, entries, by, chrono::Utc::now().timestamp())
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Every code basis of a note, oldest first.
    pub async fn list_page_code_basis(
        &self,
        page_id: &str,
    ) -> Result<Vec<CodeBasisRef>, WenlanError> {
        let conn = self.conn.lock().await;
        let sql = format!(
            "SELECT {REF_COLUMNS} FROM page_code_basis WHERE page_id = ?1 \
             ORDER BY added_at, repo_root, path, line_start, line_end"
        );
        let mut rows = conn
            .query(&sql, libsql::params![page_id])
            .await
            .map_err(decode("list_page_code_basis"))?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.map_err(decode("list_page_code_basis"))? {
            out.push(ref_from(&row).map_err(decode("list_page_code_basis"))?);
        }
        Ok(out)
    }

    /// The code bases of every note that has any, by note id, as the OKF export
    /// renders them. One query.
    pub async fn code_basis_locators_by_page(
        &self,
    ) -> Result<HashMap<String, Vec<CodeBasisLocator>>, WenlanError> {
        let conn = self.conn.lock().await;
        let sql = format!(
            "SELECT {REF_COLUMNS}, page_id FROM page_code_basis \
             ORDER BY added_at, repo_root, path, line_start, line_end"
        );
        let mut rows = conn
            .query(&sql, ())
            .await
            .map_err(decode("code_basis_locators_by_page"))?;
        let mut out: HashMap<String, Vec<CodeBasisLocator>> = HashMap::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(decode("code_basis_locators_by_page"))?
        {
            let reference = ref_from(&row).map_err(decode("code_basis_locators_by_page"))?;
            let page_id: String = row.get(9).map_err(decode("code_basis_locators_by_page"))?;
            out.entry(page_id)
                .or_default()
                .push(CodeBasisLocator::from(&reference));
        }
        Ok(out)
    }

    /// The code bases of many notes that are NOT `ok`, keyed by note. Notes with
    /// nothing to look at are absent, so a list of mostly-quiet notes stays
    /// small.
    pub async fn list_non_ok_code_basis(
        &self,
        page_ids: &[String],
    ) -> Result<HashMap<String, Vec<CodeBasisRef>>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut out: HashMap<String, Vec<CodeBasisRef>> = HashMap::new();
        for chunk in page_ids.chunks(400) {
            let placeholders = vec!["?"; chunk.len()].join(",");
            let sql = format!(
                "SELECT {REF_COLUMNS}, page_id FROM page_code_basis \
                 WHERE page_id IN ({placeholders}) AND state <> 'ok' \
                 ORDER BY added_at, repo_root, path, line_start, line_end"
            );
            let params: Vec<libsql::Value> = chunk
                .iter()
                .map(|id| libsql::Value::Text(id.clone()))
                .collect();
            let mut rows = conn
                .query(&sql, libsql::params_from_iter(params))
                .await
                .map_err(decode("list_non_ok_code_basis"))?;
            while let Some(row) = rows
                .next()
                .await
                .map_err(decode("list_non_ok_code_basis"))?
            {
                let reference = ref_from(&row).map_err(decode("list_non_ok_code_basis"))?;
                let page_id: String = row.get(9).map_err(decode("list_non_ok_code_basis"))?;
                out.entry(page_id).or_default().push(reference);
            }
        }
        Ok(out)
    }

    /// Every code basis of an active, non-entity note in `space` (`None`: every
    /// Space), with the fingerprint a check compares against.
    pub async fn list_code_basis_rows(
        &self,
        space: Option<&str>,
    ) -> Result<Vec<CodeBasisRow>, WenlanError> {
        let conn = self.conn.lock().await;
        let sql = format!(
            "{ROW_SELECT} \
             WHERE c.status = 'active' AND COALESCE(c.kind, 'concept') <> 'entity' \
               AND (?1 IS NULL OR COALESCE(c.space, ?2) = ?1) \
             ORDER BY b.page_id, b.repo_root, b.path, b.line_start, b.line_end"
        );
        let mut rows = conn
            .query(&sql, libsql::params![space, UNFILED_SPACE_ID])
            .await
            .map_err(decode("list_code_basis_rows"))?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.map_err(decode("list_code_basis_rows"))? {
            if let Some(found) = row_from(&row).map_err(decode("list_code_basis_rows"))? {
                out.push(found);
            }
        }
        Ok(out)
    }

    /// The one row a locator names, with its fingerprint.
    pub async fn get_code_basis_row(
        &self,
        page_id: &str,
        locator: &CodeBasisLocator,
    ) -> Result<Option<CodeBasisRow>, WenlanError> {
        let conn = self.conn.lock().await;
        let sql = format!(
            "{ROW_SELECT} WHERE b.page_id = ?1 AND b.repo_root = ?2 AND b.path = ?3 \
               AND b.line_start = ?4 AND b.line_end = ?5"
        );
        let mut rows = conn
            .query(
                &sql,
                libsql::params![
                    page_id,
                    locator.repo_root.as_str(),
                    locator.path.as_str(),
                    i64::from(locator.line_start),
                    i64::from(locator.line_end)
                ],
            )
            .await
            .map_err(decode("get_code_basis_row"))?;
        match rows.next().await.map_err(decode("get_code_basis_row"))? {
            Some(row) => row_from(&row).map_err(decode("get_code_basis_row")),
            None => Ok(None),
        }
    }

    /// `pending` code bases of active, non-entity notes, in the order the
    /// background model recheck should look at them: rows it has never been
    /// asked about first, then oldest `changed_at`. At most `limit` rows.
    pub async fn list_code_basis_recheck_candidates(
        &self,
        limit: usize,
    ) -> Result<Vec<CodeBasisRow>, WenlanError> {
        let conn = self.conn.lock().await;
        let sql = format!(
            "{ROW_SELECT} \
             WHERE c.status = 'active' AND COALESCE(c.kind, 'concept') <> 'entity' \
               AND b.state = 'pending' \
             ORDER BY (b.recheck_digest IS NOT NULL), COALESCE(b.changed_at, 0), \
                      b.page_id, b.repo_root, b.path, b.line_start, b.line_end \
             LIMIT ?1"
        );
        let mut rows = conn
            .query(
                &sql,
                libsql::params![i64::try_from(limit).unwrap_or(i64::MAX)],
            )
            .await
            .map_err(decode("list_code_basis_recheck_candidates"))?;
        let mut out = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(decode("list_code_basis_recheck_candidates"))?
        {
            if let Some(found) =
                row_from(&row).map_err(decode("list_code_basis_recheck_candidates"))?
            {
                out.push(found);
            }
        }
        Ok(out)
    }

    /// Record what the background model recheck concluded about a `pending`
    /// row without settling it: the digest of the changed lines it was asked
    /// about, the sentence and the verdict (`stale` or `needs_human`; `None`
    /// for an attempt that ended without a verdict), and how many attempts at
    /// that digest ended without a verdict. It lands only while the row still holds
    /// the anchor `row` was read with and is still `pending`; returns whether
    /// it did (a concurrent confirm, write or check wins).
    pub async fn record_code_basis_recheck(
        &self,
        row: &CodeBasisRow,
        digest: &str,
        note: Option<&str>,
        verdict: Option<&str>,
        attempts: i64,
    ) -> Result<bool, WenlanError> {
        let conn = self.conn.lock().await;
        let r = &row.reference;
        let changed = conn
            .execute(
                "UPDATE page_code_basis \
                 SET recheck_note = ?7, recheck_digest = ?8, recheck_attempts = ?9, \
                     recheck_verdict = ?10 \
                 WHERE page_id = ?1 AND repo_root = ?2 AND path = ?3 \
                   AND line_start = ?4 AND line_end = ?5 AND anchor_json = ?6 \
                   AND state = 'pending'",
                libsql::params![
                    row.page_id.as_str(),
                    r.repo_root.as_str(),
                    r.path.as_str(),
                    i64::from(r.line_start),
                    i64::from(r.line_end),
                    row.anchor_json.as_str(),
                    note,
                    digest,
                    attempts,
                    verdict
                ],
            )
            .await
            .map_err(decode("record_code_basis_recheck"))?;
        Ok(changed > 0)
    }

    /// Whether the model has looked at every code basis the Brief counts in
    /// `space`: there is at least one `pending` row, every `pending` row has a
    /// stored verdict, and no row is `unavailable` (those are not the model's
    /// to judge).
    pub async fn space_code_basis_all_explained(&self, space: &str) -> Result<bool, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT \
                   COALESCE(SUM(CASE WHEN b.state = 'pending' THEN 1 ELSE 0 END), 0), \
                   COALESCE(SUM(CASE WHEN b.state = 'pending' \
                        AND TRIM(COALESCE(b.recheck_verdict, '')) <> '' THEN 1 ELSE 0 END), 0), \
                   COALESCE(SUM(CASE WHEN b.state = 'unavailable' THEN 1 ELSE 0 END), 0) \
                 FROM page_code_basis b JOIN pages c ON c.id = b.page_id \
                 WHERE c.status = 'active' AND COALESCE(c.kind, 'concept') <> 'entity' \
                   AND COALESCE(c.space, ?2) = ?1",
                libsql::params![space, UNFILED_SPACE_ID],
            )
            .await
            .map_err(decode("space_code_basis_all_explained"))?;
        let Some(row) = rows
            .next()
            .await
            .map_err(decode("space_code_basis_all_explained"))?
        else {
            return Ok(false);
        };
        let get = |index| {
            row.get::<i64>(index)
                .map_err(decode("space_code_basis_all_explained"))
        };
        let (pending, explained, unavailable) = (get(0)?, get(1)?, get(2)?);
        Ok(pending > 0 && explained == pending && unavailable == 0)
    }

    /// Notes (active, in `space`, or in any Space when `None`) holding a code
    /// basis that is `pending` and that the background model judged `stale`:
    /// the note states something the code now contradicts and the correction
    /// follows from the code. Returns `(page_id, title, row)` ordered by note,
    /// each row carrying the model's sentence (`explanation`) and verdict
    /// (`ai_verdict`).
    ///
    /// This is the handoff to the on-demand steep: it rewrites these notes with
    /// the main model through a revision card and confirms the row with the
    /// existing confirm route. The recheck job never writes a page.
    pub async fn list_code_basis_stale(
        &self,
        space: Option<&str>,
    ) -> Result<Vec<(String, String, CodeBasisRef)>, WenlanError> {
        let conn = self.conn.lock().await;
        let sql = format!(
            "{ROW_SELECT} \
             WHERE c.status = 'active' AND COALESCE(c.kind, 'concept') <> 'entity' \
               AND b.state = 'pending' AND b.recheck_verdict = 'stale' \
               AND (?1 IS NULL OR COALESCE(c.space, ?2) = ?1) \
             ORDER BY b.page_id, b.added_at, b.repo_root, b.path, b.line_start, b.line_end"
        );
        let mut rows = conn
            .query(&sql, libsql::params![space, UNFILED_SPACE_ID])
            .await
            .map_err(decode("list_code_basis_stale"))?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.map_err(decode("list_code_basis_stale"))? {
            out.push((
                row.get::<String>(10)
                    .map_err(decode("list_code_basis_stale"))?,
                row.get::<String>(11)
                    .map_err(decode("list_code_basis_stale"))?,
                ref_from(&row).map_err(decode("list_code_basis_stale"))?,
            ));
        }
        Ok(out)
    }

    /// The git blob id of the file a code basis was last written or confirmed
    /// against, with when that was and the row's state now. Reads the stored row only: no file is
    /// opened and no check runs. `None` when the note has no such row.
    pub async fn get_code_basis_cited(
        &self,
        page_id: &str,
        locator: &CodeBasisLocator,
    ) -> Result<Option<CodeBasisCitedResponse>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT cited_blob, confirmed_at, state FROM page_code_basis \
                 WHERE page_id = ?1 AND repo_root = ?2 AND path = ?3 \
                   AND line_start = ?4 AND line_end = ?5",
                libsql::params![
                    page_id,
                    locator.repo_root.as_str(),
                    locator.path.as_str(),
                    i64::from(locator.line_start),
                    i64::from(locator.line_end)
                ],
            )
            .await
            .map_err(decode("get_code_basis_cited"))?;
        match rows.next().await.map_err(decode("get_code_basis_cited"))? {
            Some(row) => Ok(Some(CodeBasisCitedResponse {
                cited_blob: row
                    .get::<Option<String>>(0)
                    .map_err(decode("get_code_basis_cited"))?,
                confirmed_at: row.get::<i64>(1).map_err(decode("get_code_basis_cited"))?,
                state: row
                    .get::<String>(2)
                    .map_err(decode("get_code_basis_cited"))?,
            })),
            None => Ok(None),
        }
    }

    /// Notes (active, in `space`) with a `pending` or `unavailable` code basis,
    /// as `(page_id, title, row)` ordered by note.
    pub async fn list_code_basis_needing_attention(
        &self,
        space: Option<&str>,
    ) -> Result<Vec<(String, String, CodeBasisRef)>, WenlanError> {
        let conn = self.conn.lock().await;
        let sql = format!(
            "{ROW_SELECT} \
             WHERE c.status = 'active' AND COALESCE(c.kind, 'concept') <> 'entity' \
               AND b.state IN ('pending', 'unavailable') \
               AND (?1 IS NULL OR COALESCE(c.space, ?2) = ?1) \
             ORDER BY b.page_id, b.added_at, b.repo_root, b.path, b.line_start, b.line_end"
        );
        let mut rows = conn
            .query(&sql, libsql::params![space, UNFILED_SPACE_ID])
            .await
            .map_err(decode("list_code_basis_needing_attention"))?;
        let mut out = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(decode("list_code_basis_needing_attention"))?
        {
            out.push((
                row.get::<String>(10)
                    .map_err(decode("list_code_basis_needing_attention"))?,
                row.get::<String>(11)
                    .map_err(decode("list_code_basis_needing_attention"))?,
                ref_from(&row).map_err(decode("list_code_basis_needing_attention"))?,
            ));
        }
        Ok(out)
    }

    /// Whether any active note in `space` has a code basis.
    pub async fn space_has_code_basis(&self, space: &str) -> Result<bool, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT 1 FROM page_code_basis b JOIN pages c ON c.id = b.page_id \
                 WHERE c.status = 'active' AND COALESCE(c.kind, 'concept') <> 'entity' \
                   AND COALESCE(c.space, ?2) = ?1 LIMIT 1",
                libsql::params![space, UNFILED_SPACE_ID],
            )
            .await
            .map_err(decode("space_has_code_basis"))?;
        Ok(rows
            .next()
            .await
            .map_err(decode("space_has_code_basis"))?
            .is_some())
    }

    /// How many active notes in `space` have at least one `pending` or
    /// `unavailable` code basis.
    pub async fn count_pages_with_code_basis_pending(
        &self,
        space: &str,
    ) -> Result<u32, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT COUNT(DISTINCT b.page_id) FROM page_code_basis b \
                 JOIN pages c ON c.id = b.page_id \
                 WHERE c.status = 'active' AND COALESCE(c.kind, 'concept') <> 'entity' \
                   AND b.state IN ('pending', 'unavailable') \
                   AND COALESCE(c.space, ?2) = ?1",
                libsql::params![space, UNFILED_SPACE_ID],
            )
            .await
            .map_err(decode("count_pages_with_code_basis_pending"))?;
        let count = match rows
            .next()
            .await
            .map_err(decode("count_pages_with_code_basis_pending"))?
        {
            Some(row) => row
                .get::<i64>(0)
                .map_err(decode("count_pages_with_code_basis_pending"))?,
            None => 0,
        };
        Ok(u32::try_from(count).unwrap_or(u32::MAX))
    }

    /// Apply the verdicts of a check, in one transaction. A verdict lands only
    /// while its row still holds the anchor the check was made against;
    /// otherwise a write got there first and the row is left as it is.
    pub async fn apply_code_basis_verdicts(
        &self,
        verdicts: &[(&CodeBasisRow, CodeBasisVerdict)],
        now: i64,
    ) -> Result<(), WenlanError> {
        if verdicts.is_empty() {
            return Ok(());
        }
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(libsql::TransactionBehavior::Immediate)
            .await
            .map_err(decode("apply_code_basis_verdicts begin"))?;
        // Moves first-come, first-served by free place: a row can move into the
        // place another row of the note is about to leave, so a blocked move is
        // retried after the others. A move still blocked when nothing else
        // progresses lands on lines the note already relies on (the same
        // reliance stated twice), and that row is dropped.
        let mut blocked: Vec<(&CodeBasisRow, LineSpan, &CodeAnchor, Option<FileStamp>)> =
            Vec::new();
        for (row, verdict) in verdicts {
            match verdict {
                CodeBasisVerdict::Moved {
                    span,
                    anchor,
                    stamp,
                } => blocked.push((row, *span, anchor, *stamp)),
                other => apply_verdict(&tx, row, other, now)
                    .await
                    .map_err(decode("apply_code_basis_verdicts"))?,
            }
        }
        loop {
            let before = blocked.len();
            let mut still = Vec::new();
            for (row, span, anchor, stamp) in blocked {
                let moved = try_move_row(&tx, row, span, anchor, stamp)
                    .await
                    .map_err(decode("apply_code_basis_verdicts"))?;
                if !moved {
                    still.push((row, span, anchor, stamp));
                }
            }
            blocked = still;
            if blocked.is_empty() || blocked.len() == before {
                break;
            }
        }
        for (row, _, _, _) in blocked {
            tx.execute(
                "DELETE FROM page_code_basis \
                 WHERE page_id = ?1 AND repo_root = ?2 AND path = ?3 \
                   AND line_start = ?4 AND line_end = ?5 AND anchor_json = ?6",
                libsql::params![
                    row.page_id.as_str(),
                    row.reference.repo_root.as_str(),
                    row.reference.path.as_str(),
                    i64::from(row.reference.line_start),
                    i64::from(row.reference.line_end),
                    row.anchor_json.as_str()
                ],
            )
            .await
            .map_err(decode("apply_code_basis_verdicts"))?;
        }
        tx.commit()
            .await
            .map_err(decode("apply_code_basis_verdicts commit"))?;
        Ok(())
    }

    /// Settle one row against the current file: new span and anchor, state
    /// `ok`. `seen_anchor_json` is the anchor the caller compared against; a
    /// row that changed since is a [`CodeBasisError::Conflict`].
    #[allow(clippy::too_many_arguments)]
    pub async fn confirm_code_basis(
        &self,
        page_id: &str,
        locator: &CodeBasisLocator,
        seen_anchor_json: &str,
        span: LineSpan,
        anchor: &CodeAnchor,
        stamp: Option<FileStamp>,
        cited_blob: Option<&str>,
        by: &str,
        now: i64,
    ) -> Result<CodeBasisRef, CodeBasisError> {
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(libsql::TransactionBehavior::Immediate)
            .await?;
        let current = CodeBasisRef {
            repo_root: locator.repo_root.clone(),
            path: locator.path.clone(),
            line_start: locator.line_start,
            line_end: locator.line_end,
            state: String::new(),
            changed_at: None,
            reason: None,
            explanation: None,
            ai_verdict: None,
        };
        let exists = {
            let mut rows = tx
                .query(
                    "SELECT 1 FROM page_code_basis \
                     WHERE page_id = ?1 AND repo_root = ?2 AND path = ?3 \
                       AND line_start = ?4 AND line_end = ?5 AND anchor_json = ?6",
                    libsql::params![
                        page_id,
                        locator.repo_root.as_str(),
                        locator.path.as_str(),
                        i64::from(locator.line_start),
                        i64::from(locator.line_end),
                        seen_anchor_json
                    ],
                )
                .await?;
            rows.next().await?.is_some()
        };
        if !exists {
            let mut rows = tx
                .query(
                    "SELECT 1 FROM page_code_basis \
                     WHERE page_id = ?1 AND repo_root = ?2 AND path = ?3 \
                       AND line_start = ?4 AND line_end = ?5",
                    libsql::params![
                        page_id,
                        locator.repo_root.as_str(),
                        locator.path.as_str(),
                        i64::from(locator.line_start),
                        i64::from(locator.line_end)
                    ],
                )
                .await?;
            return Err(if rows.next().await?.is_some() {
                CodeBasisError::Conflict(
                    "the note's code basis changed while it was being confirmed; read it again"
                        .into(),
                )
            } else {
                CodeBasisError::NotFound
            });
        }
        ensure_span_free(&tx, page_id, &current, span).await?;
        tx.execute(
            "UPDATE page_code_basis \
             SET line_start = ?6, line_end = ?7, anchor_json = ?8, confirmed_digest = ?9, \
                 confirmed_by = ?10, confirmed_at = ?11, state = 'ok', \
                 unavailable_reason = NULL, changed_at = NULL, \
                 file_len = ?12, file_mtime_ns = ?13, cited_blob = ?14, \
                 recheck_note = NULL, recheck_verdict = NULL, recheck_digest = NULL, \
                 recheck_attempts = 0 \
             WHERE page_id = ?1 AND repo_root = ?2 AND path = ?3 \
               AND line_start = ?4 AND line_end = ?5",
            libsql::params![
                page_id,
                locator.repo_root.as_str(),
                locator.path.as_str(),
                i64::from(locator.line_start),
                i64::from(locator.line_end),
                i64::from(span.start),
                i64::from(span.end),
                serde_json::to_string(anchor)?,
                anchor.digest.as_str(),
                by,
                now,
                stamp.map(|s| s.len),
                stamp.map(|s| s.mtime_ns),
                cited_blob
            ],
        )
        .await?;
        tx.commit().await?;
        Ok(CodeBasisRef {
            line_start: span.start,
            line_end: span.end,
            state: CODE_BASIS_STATE_OK.to_string(),
            ..current
        })
    }

    /// Stop relying on these lines. Returns whether a row was removed.
    pub async fn remove_code_basis(
        &self,
        page_id: &str,
        locator: &CodeBasisLocator,
    ) -> Result<bool, WenlanError> {
        let conn = self.conn.lock().await;
        let removed = conn
            .execute(
                "DELETE FROM page_code_basis \
                 WHERE page_id = ?1 AND repo_root = ?2 AND path = ?3 \
                   AND line_start = ?4 AND line_end = ?5",
                libsql::params![
                    page_id,
                    locator.repo_root.as_str(),
                    locator.path.as_str(),
                    i64::from(locator.line_start),
                    i64::from(locator.line_end)
                ],
            )
            .await
            .map_err(decode("remove_code_basis"))?;
        Ok(removed > 0)
    }
}

/// Move a row to the span its content now has, if that place is free.
/// Returns `false` when another row of the note occupies it; a row that no
/// longer holds the anchor the check saw counts as done (a write got there
/// first).
async fn try_move_row(
    conn: &libsql::Connection,
    row: &CodeBasisRow,
    span: LineSpan,
    anchor: &CodeAnchor,
    stamp: Option<FileStamp>,
) -> Result<bool, libsql::Error> {
    let r = &row.reference;
    let still_there = {
        let mut rows = conn
            .query(
                "SELECT 1 FROM page_code_basis \
                 WHERE page_id = ?1 AND repo_root = ?2 AND path = ?3 \
                   AND line_start = ?4 AND line_end = ?5 AND anchor_json = ?6",
                libsql::params![
                    row.page_id.as_str(),
                    r.repo_root.as_str(),
                    r.path.as_str(),
                    i64::from(r.line_start),
                    i64::from(r.line_end),
                    row.anchor_json.as_str()
                ],
            )
            .await?;
        rows.next().await?.is_some()
    };
    if !still_there {
        return Ok(true);
    }
    let occupied = span.start != r.line_start || span.end != r.line_end;
    if occupied {
        let mut rows = conn
            .query(
                "SELECT 1 FROM page_code_basis \
                 WHERE page_id = ?1 AND repo_root = ?2 AND path = ?3 \
                   AND line_start = ?4 AND line_end = ?5",
                libsql::params![
                    row.page_id.as_str(),
                    r.repo_root.as_str(),
                    r.path.as_str(),
                    i64::from(span.start),
                    i64::from(span.end)
                ],
            )
            .await?;
        if rows.next().await?.is_some() {
            return Ok(false);
        }
    }
    conn.execute(
        "UPDATE page_code_basis \
         SET line_start = ?7, line_end = ?8, anchor_json = ?9, state = ?10, \
             unavailable_reason = NULL, changed_at = NULL, file_len = ?11, file_mtime_ns = ?12, \
             recheck_note = NULL, recheck_verdict = NULL, recheck_digest = NULL, \
             recheck_attempts = 0 \
         WHERE page_id = ?1 AND repo_root = ?2 AND path = ?3 \
           AND line_start = ?4 AND line_end = ?5 AND anchor_json = ?6",
        libsql::params![
            row.page_id.as_str(),
            r.repo_root.as_str(),
            r.path.as_str(),
            i64::from(r.line_start),
            i64::from(r.line_end),
            row.anchor_json.as_str(),
            i64::from(span.start),
            i64::from(span.end),
            serde_json::to_string(anchor).unwrap_or_else(|_| row.anchor_json.clone()),
            CODE_BASIS_STATE_MOVED,
            stamp.map(|s| s.len),
            stamp.map(|s| s.mtime_ns)
        ],
    )
    .await?;
    Ok(true)
}

/// One verdict, on the caller's transaction.
async fn apply_verdict(
    conn: &libsql::Connection,
    row: &CodeBasisRow,
    verdict: &CodeBasisVerdict,
    now: i64,
) -> Result<(), libsql::Error> {
    let r = &row.reference;
    match verdict {
        CodeBasisVerdict::Unchanged { stamp } => {
            // A `moved` row stays `moved` until confirmed; anything else that
            // reads the same again is `ok`. The row takes the file's current
            // stamp, so the next check can skip it.
            conn.execute(
                "UPDATE page_code_basis \
                 SET state = CASE WHEN state = 'moved' THEN 'moved' ELSE 'ok' END, \
                     unavailable_reason = NULL, changed_at = NULL, \
                     file_len = ?7, file_mtime_ns = ?8, \
                     recheck_note = NULL, recheck_verdict = NULL, recheck_digest = NULL, \
                     recheck_attempts = 0 \
                 WHERE page_id = ?1 AND repo_root = ?2 AND path = ?3 \
                   AND line_start = ?4 AND line_end = ?5 AND anchor_json = ?6",
                libsql::params![
                    row.page_id.as_str(),
                    r.repo_root.as_str(),
                    r.path.as_str(),
                    i64::from(r.line_start),
                    i64::from(r.line_end),
                    row.anchor_json.as_str(),
                    stamp.map(|s| s.len),
                    stamp.map(|s| s.mtime_ns)
                ],
            )
            .await?;
        }
        // Handled by `try_move_row` (it needs the batch to order the moves).
        CodeBasisVerdict::Moved { .. } => {}
        CodeBasisVerdict::Pending => {
            // The first time the content was seen changed is kept while the
            // row stays pending.
            conn.execute(
                "UPDATE page_code_basis \
                 SET changed_at = CASE WHEN state = 'pending' AND changed_at IS NOT NULL \
                                       THEN changed_at ELSE ?7 END, \
                     state = 'pending', unavailable_reason = NULL \
                 WHERE page_id = ?1 AND repo_root = ?2 AND path = ?3 \
                   AND line_start = ?4 AND line_end = ?5 AND anchor_json = ?6",
                libsql::params![
                    row.page_id.as_str(),
                    r.repo_root.as_str(),
                    r.path.as_str(),
                    i64::from(r.line_start),
                    i64::from(r.line_end),
                    row.anchor_json.as_str(),
                    now
                ],
            )
            .await?;
        }
        CodeBasisVerdict::Unavailable(reason) => {
            conn.execute(
                "UPDATE page_code_basis \
                 SET state = 'unavailable', unavailable_reason = ?7, changed_at = NULL \
                 WHERE page_id = ?1 AND repo_root = ?2 AND path = ?3 \
                   AND line_start = ?4 AND line_end = ?5 AND anchor_json = ?6",
                libsql::params![
                    row.page_id.as_str(),
                    r.repo_root.as_str(),
                    r.path.as_str(),
                    i64::from(r.line_start),
                    i64::from(r.line_end),
                    row.anchor_json.as_str(),
                    *reason
                ],
            )
            .await?;
        }
    }
    Ok(())
}
