// SPDX-License-Identifier: Apache-2.0
//! A note can rely on another note: its **basis**.
//!
//! The citer ("B") records which version of the basis ("A") it was last
//! confirmed against, as a digest of A's text ([`basis_text_digest`]). When A
//! is written, every row pointing at A is re-evaluated in the SAME
//! transaction as the write (`refresh_basis_state`, called from
//! `append_page_history` and from the few writers that change a note's status
//! or Space without a history row). The row's `state` is therefore always a
//! function of the basis' current state, never of a stale event:
//!
//! | state         | meaning                                                         |
//! |---------------|-----------------------------------------------------------------|
//! | `ok`          | A's text equals what B was confirmed against                    |
//! | `pending`     | A's text differs (`changed_version` = A's version that did it) |
//! | `archived`    | A is archived (for example merged into another note)            |
//! | `deleted`     | A is gone, or its id was reused by a different note             |
//! | `unavailable` | A and B now sit in different Spaces; reads are refused          |
//!
//! `deleted` is terminal: a recreated note with the same id has a different
//! `pages.incarnation`, so it can never be mistaken for the basis B relied on.
//!
//! Mark, unmark and confirm are metadata only. They never bump the citer's
//! `version`, never write a history row, and so never invalidate an
//! `expected_version` (same as the review lock). They bump
//! `pages.basis_revision` instead, which the export reconciliation compares.

use super::{MemoryDB, UNFILED_SPACE_ID};
use crate::error::WenlanError;
use crate::provenance::basis_text_digest;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub(super) const PAGE_BASIS_DDL: &str = "
    CREATE TABLE IF NOT EXISTS page_basis (
        page_id TEXT NOT NULL,
        basis_page_id TEXT NOT NULL,
        basis_title TEXT NOT NULL,
        confirmed_version INTEGER NOT NULL,
        confirmed_digest TEXT NOT NULL,
        confirmed_by TEXT NOT NULL,
        confirmed_at INTEGER NOT NULL,
        state TEXT NOT NULL
            CHECK(state IN ('ok','pending','archived','deleted','unavailable')),
        changed_version INTEGER,
        minor_only INTEGER NOT NULL DEFAULT 0,
        added_by TEXT NOT NULL,
        added_at INTEGER NOT NULL,
        basis_incarnation TEXT NOT NULL,
        PRIMARY KEY (page_id, basis_page_id)
    );
    CREATE INDEX IF NOT EXISTS page_basis_by_basis ON page_basis(basis_page_id, state);";

pub const BASIS_STATE_OK: &str = "ok";
pub const BASIS_STATE_PENDING: &str = "pending";
pub const BASIS_STATE_ARCHIVED: &str = "archived";
pub const BASIS_STATE_DELETED: &str = "deleted";
pub const BASIS_STATE_UNAVAILABLE: &str = "unavailable";

/// Why a basis cannot be read or confirmed right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BasisUnavailableReason {
    Deleted,
    IncarnationChanged,
    Archived,
    OtherSpace,
    HistoryGone,
}

impl std::fmt::Display for BasisUnavailableReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Deleted => "the basis note was deleted",
            Self::IncarnationChanged => "the basis note was deleted and its id reused",
            Self::Archived => "the basis note is archived",
            Self::OtherSpace => "the two notes are in different Spaces",
            Self::HistoryGone => "the version this note relied on is no longer stored",
        })
    }
}

/// Typed refusals for the basis operations. Each variant is a distinct,
/// plain-language reason a caller can show; `Db` carries a storage failure.
#[derive(Debug, thiserror::Error)]
pub enum BasisError {
    #[error("a note cannot be its own basis")]
    SelfReference,
    #[error("the note that would rely on it was not found")]
    CiterNotFound,
    #[error("only an active note can rely on another note")]
    CiterNotActive,
    #[error("a draft cannot rely on another note")]
    CiterIsDraft,
    #[error("an entity page cannot rely on another note")]
    CiterIsEntityPage,
    #[error("the basis note was not found")]
    BasisNotFound,
    #[error("the basis note was deleted")]
    BasisDeleted,
    #[error("the basis note is archived")]
    BasisArchived,
    #[error("a draft cannot be a basis")]
    BasisIsDraft,
    #[error("an entity page cannot be a basis")]
    BasisIsEntityPage,
    #[error("the basis note is in a different Space")]
    DifferentSpace,
    #[error("version {requested} is not a version of the basis note (current version {current})")]
    InvalidVersion { requested: i64, current: i64 },
    #[error("the basis note moved on: it is now at version {current}")]
    MovedOn { current: i64 },
    #[error("this note does not rely on that note")]
    NotABasis,
    /// The note relying on the basis was edited after the write checked it.
    #[error("the note changed: it is now at version {current}")]
    CiterMovedOn { current: i64 },
    #[error("the basis note is not available: {0}")]
    BasisUnavailable(BasisUnavailableReason),
    /// A mark recorded on a revision card (an agent's write to a locked note)
    /// could not be applied when the card was accepted. The message starts
    /// with [`wenlan_types::PAGE_REVISION_BASIS_REFUSED`] so the accept path,
    /// the app and the MCP client can recognise it.
    #[error(
        "{code}: the change relied on note {basis_page_id}, and {reason}",
        code = wenlan_types::PAGE_REVISION_BASIS_REFUSED
    )]
    CardMarkRefused {
        basis_page_id: String,
        reason: Box<BasisError>,
    },
    #[error(transparent)]
    Db(#[from] WenlanError),
}

impl From<libsql::Error> for BasisError {
    fn from(error: libsql::Error) -> Self {
        BasisError::Db(WenlanError::VectorDb(format!("page_basis: {error}")))
    }
}

impl From<BasisError> for WenlanError {
    fn from(error: BasisError) -> Self {
        match error {
            BasisError::Db(inner) => inner,
            BasisError::CiterNotFound
            | BasisError::BasisNotFound
            | BasisError::BasisDeleted
            | BasisError::NotABasis => WenlanError::NotFound(error.to_string()),
            BasisError::MovedOn { .. }
            | BasisError::CiterMovedOn { .. }
            | BasisError::BasisUnavailable(_)
            | BasisError::CardMarkRefused { .. } => WenlanError::Conflict(error.to_string()),
            _ => WenlanError::Validation(error.to_string()),
        }
    }
}

/// One basis of a note, as the note's readers see it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PageBasisEntry {
    pub basis_page_id: String,
    /// Live title while the basis exists; the snapshot taken before it was
    /// deleted otherwise.
    pub title: String,
    pub state: String,
    pub confirmed_version: i64,
    /// `None` once the basis is deleted (or its id was reused).
    pub current_version: Option<i64>,
    pub changed_version: Option<i64>,
    pub minor_only: bool,
    pub confirmed_by: String,
    pub confirmed_at: i64,
    pub added_by: String,
    pub added_at: i64,
}

/// One note waiting on a changed basis.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BasisPendingCiter {
    pub page_id: String,
    pub title: String,
    pub minor_only: bool,
}

/// Every note waiting on one basis in one state ("A changed, 20 notes to
/// confirm").
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BasisPendingGroup {
    pub basis_page_id: String,
    pub title: String,
    pub state: String,
    pub changed_version: Option<i64>,
    pub citers: Vec<BasisPendingCiter>,
}

/// One version written to the basis since the citer last confirmed it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BasisEdit {
    pub version: i64,
    pub edited_by: String,
    pub at: i64,
    pub minor: bool,
    /// The edit's one-line summary from the basis' changelog; empty when that
    /// version recorded none (a plain human save, or a trimmed changelog).
    pub delta_summary: String,
}

/// The text the citer relied on next to the basis' text now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BasisDiff {
    pub basis_page_id: String,
    pub title: String,
    pub from_version: i64,
    pub to_version: i64,
    pub from_content: String,
    pub to_content: String,
    pub edits: Vec<BasisEdit>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", content = "detail", rename_all = "snake_case")]
pub enum BasisDiffOutcome {
    Available(BasisDiff),
    Unavailable(BasisUnavailableReason),
    /// The named note (citer or basis) is not in the caller's read scope at
    /// the moment the texts would be read. Callers answer as for a missing
    /// note.
    OutOfScope(String),
}

#[derive(Debug, Clone)]
struct BasisRow {
    page_id: String,
    basis_page_id: String,
    basis_title: String,
    confirmed_version: i64,
    confirmed_digest: String,
    state: String,
    changed_version: Option<i64>,
    minor_only: bool,
    added_by: String,
    added_at: i64,
    basis_incarnation: String,
}

const ROW_COLUMNS: &str = "b.page_id, b.basis_page_id, b.basis_title, b.confirmed_version, \
    b.confirmed_digest, b.state, b.changed_version, \
    b.minor_only, b.added_by, b.added_at, b.basis_incarnation";

fn row_from(row: &libsql::Row) -> Result<BasisRow, libsql::Error> {
    Ok(BasisRow {
        page_id: row.get(0)?,
        basis_page_id: row.get(1)?,
        basis_title: row.get(2)?,
        confirmed_version: row.get(3)?,
        confirmed_digest: row.get(4)?,
        state: row.get(5)?,
        changed_version: row.get::<Option<i64>>(6)?,
        minor_only: row.get::<i64>(7)? != 0,
        added_by: row.get(8)?,
        added_at: row.get(9)?,
        basis_incarnation: row.get(10)?,
    })
}

/// The facts about a page that decide a basis row's state.
#[derive(Debug, Clone)]
struct PageFacts {
    title: String,
    status: String,
    kind: String,
    version: i64,
    space: String,
    incarnation: String,
    content: String,
}

async fn load_facts(
    conn: &libsql::Connection,
    page_id: &str,
    with_content: bool,
) -> Result<Option<PageFacts>, libsql::Error> {
    let content_column = if with_content { "content" } else { "''" };
    let sql = format!(
        "SELECT title, status, COALESCE(kind, 'concept'), version, \
                COALESCE(space, ?2), COALESCE(incarnation, ''), {content_column} \
         FROM pages WHERE id = ?1"
    );
    let mut rows = conn
        .query(&sql, libsql::params![page_id, UNFILED_SPACE_ID])
        .await?;
    let Some(row) = rows.next().await? else {
        return Ok(None);
    };
    Ok(Some(PageFacts {
        title: row.get(0)?,
        status: row.get(1)?,
        kind: row.get(2)?,
        version: row.get(3)?,
        space: row.get(4)?,
        incarnation: row.get(5)?,
        content: row.get(6)?,
    }))
}

async fn load_row(
    conn: &libsql::Connection,
    page_id: &str,
    basis_page_id: &str,
) -> Result<Option<BasisRow>, libsql::Error> {
    let sql = format!(
        "SELECT {ROW_COLUMNS} FROM page_basis b WHERE b.page_id = ?1 AND b.basis_page_id = ?2"
    );
    let mut rows = conn
        .query(&sql, libsql::params![page_id, basis_page_id])
        .await?;
    match rows.next().await? {
        Some(row) => Ok(Some(row_from(&row)?)),
        None => Ok(None),
    }
}

/// `minor_only`: every version the basis gained since the citer's confirmed
/// version was saved as a minor edit. A gap in the stored history (a version
/// bump that wrote no row) is not provably minor, so it counts as not minor.
async fn minor_only_between(
    conn: &libsql::Connection,
    basis_page_id: &str,
    confirmed_version: i64,
    current_version: i64,
) -> Result<bool, libsql::Error> {
    if current_version <= confirmed_version {
        return Ok(false);
    }
    let mut rows = conn
        .query(
            "SELECT COUNT(*), COALESCE(MIN(minor), 0) FROM page_history \
             WHERE page_id = ?1 AND version > ?2 AND version <= ?3",
            libsql::params![basis_page_id, confirmed_version, current_version],
        )
        .await?;
    let Some(row) = rows.next().await? else {
        return Ok(false);
    };
    let count: i64 = row.get(0)?;
    let min_minor: i64 = row.get(1)?;
    Ok(count == current_version - confirmed_version && min_minor == 1)
}

struct Evaluated {
    state: &'static str,
    changed_version: Option<i64>,
    minor_only: bool,
    title: String,
}

/// The ONE rule that turns "what the basis is now" into a row's state.
async fn evaluate_row(
    conn: &libsql::Connection,
    row: &BasisRow,
    basis: Option<&PageFacts>,
    citer_space: Option<&str>,
) -> Result<Evaluated, libsql::Error> {
    let unchanged = |state: &'static str| Evaluated {
        state,
        changed_version: row.changed_version,
        minor_only: false,
        title: row.basis_title.clone(),
    };
    // `deleted` is terminal: even a recreated note with the same id is not
    // the note this row was confirmed against.
    if row.state == BASIS_STATE_DELETED {
        return Ok(unchanged(BASIS_STATE_DELETED));
    }
    let Some(basis) = basis else {
        return Ok(unchanged(BASIS_STATE_DELETED));
    };
    if basis.incarnation != row.basis_incarnation {
        return Ok(unchanged(BASIS_STATE_DELETED));
    }
    // The stored title is what the citer shows once the basis is deleted, so
    // it only follows the basis' name while the citer could read the basis. A
    // note renamed in another Space must not reveal its new name later.
    let title = if citer_space.is_none_or(|space| space == basis.space) {
        basis.title.clone()
    } else {
        row.basis_title.clone()
    };
    if basis.status != "active" {
        return Ok(Evaluated {
            title,
            ..unchanged(BASIS_STATE_ARCHIVED)
        });
    }
    if citer_space.is_some_and(|space| space != basis.space) {
        return Ok(Evaluated {
            title,
            ..unchanged(BASIS_STATE_UNAVAILABLE)
        });
    }
    if basis_text_digest(&basis.content) == row.confirmed_digest {
        return Ok(Evaluated {
            state: BASIS_STATE_OK,
            changed_version: None,
            minor_only: false,
            title,
        });
    }
    let minor_only = minor_only_between(
        conn,
        &row.basis_page_id,
        row.confirmed_version,
        basis.version,
    )
    .await?;
    Ok(Evaluated {
        state: BASIS_STATE_PENDING,
        changed_version: Some(basis.version),
        minor_only,
        title,
    })
}

async fn load_rows(
    conn: &libsql::Connection,
    where_sql: &str,
    page_id: Option<&str>,
) -> Result<Vec<BasisRow>, libsql::Error> {
    let sql = format!("SELECT {ROW_COLUMNS} FROM page_basis b WHERE {where_sql}");
    let mut rows = match page_id {
        Some(id) => conn.query(&sql, libsql::params![id]).await?,
        None => conn.query(&sql, ()).await?,
    };
    let mut out = Vec::new();
    while let Some(row) = rows.next().await? {
        out.push(row_from(&row)?);
    }
    Ok(out)
}

async fn refresh_rows(conn: &libsql::Connection, rows: Vec<BasisRow>) -> Result<(), libsql::Error> {
    if rows.is_empty() {
        return Ok(());
    }
    // Keyed by (id, with_content): a page can be a basis in one row (its text
    // matters) and a citer in another (only its Space matters).
    let mut facts: HashMap<(String, bool), Option<PageFacts>> = HashMap::new();
    for row in rows {
        for (id, with_content) in [(&row.basis_page_id, true), (&row.page_id, false)] {
            let key = (id.clone(), with_content);
            if let std::collections::hash_map::Entry::Vacant(slot) = facts.entry(key) {
                slot.insert(load_facts(conn, id, with_content).await?);
            }
        }
        let citer_space = facts
            .get(&(row.page_id.clone(), false))
            .and_then(|f| f.as_ref())
            .map(|f| f.space.clone());
        let basis = facts
            .get(&(row.basis_page_id.clone(), true))
            .and_then(|f| f.as_ref());
        let evaluated = evaluate_row(conn, &row, basis, citer_space.as_deref()).await?;
        if evaluated.state != row.state
            || evaluated.changed_version != row.changed_version
            || evaluated.minor_only != row.minor_only
            || evaluated.title != row.basis_title
        {
            conn.execute(
                "UPDATE page_basis \
                 SET state = ?3, changed_version = ?4, minor_only = ?5, basis_title = ?6 \
                 WHERE page_id = ?1 AND basis_page_id = ?2",
                libsql::params![
                    row.page_id.as_str(),
                    row.basis_page_id.as_str(),
                    evaluated.state,
                    evaluated.changed_version,
                    i64::from(evaluated.minor_only),
                    evaluated.title.as_str()
                ],
            )
            .await?;
        }
    }
    Ok(())
}

/// Re-evaluate every basis row that involves `page_id`, as the basis (its text
/// or status changed) or as the citer (its Space changed). Runs on the
/// caller's open transaction so a row never disagrees with the write that
/// moved it. Statements are issued only for rows that actually change, which
/// is what lets the repair writers count the effect exactly.
pub(super) async fn refresh_basis_state(
    conn: &libsql::Connection,
    page_id: &str,
) -> Result<(), libsql::Error> {
    let rows = load_rows(
        conn,
        "b.basis_page_id = ?1 OR b.page_id = ?1",
        Some(page_id),
    )
    .await?;
    refresh_rows(conn, rows).await
}

/// Re-evaluate every basis row. For the writers that move many notes between
/// Spaces at once (delete/merge/reassign a Space); `page_basis` is small, so a
/// full pass is cheaper than enumerating the moved notes.
pub(super) async fn refresh_basis_all(conn: &libsql::Connection) -> Result<(), libsql::Error> {
    let rows = load_rows(conn, "1 = 1", None).await?;
    refresh_rows(conn, rows).await
}

/// R4: a deleted note's rows stop being a basis (`deleted`, title kept) and
/// the rows it held as a citer go away. Same transaction as the delete.
pub(super) async fn retire_basis_for_deleted_page(
    conn: &libsql::Connection,
    page_id: &str,
) -> Result<(), libsql::Error> {
    conn.execute(
        "UPDATE page_basis SET state = 'deleted', changed_version = NULL, minor_only = 0 \
         WHERE basis_page_id = ?1 AND state <> 'deleted'",
        libsql::params![page_id],
    )
    .await?;
    conn.execute(
        "DELETE FROM page_basis WHERE page_id = ?1",
        libsql::params![page_id],
    )
    .await?;
    Ok(())
}

/// R4b: when `absorbed` is merged into `survivor`, the bases `absorbed`
/// relied on are copied onto `survivor` as `pending`, never `ok`. The copy
/// carries an empty digest, so no later re-evaluation can turn it `ok`; only
/// an explicit confirmation can. A copy that would make the survivor its own
/// basis is dropped, and a basis the survivor already has keeps the
/// survivor's own row. Rows that point AT `absorbed` are left alone: it is
/// archived by the merge, so their citers see it as such.
pub(super) async fn carry_bases_into_survivor(
    conn: &libsql::Connection,
    absorbed_id: &str,
    survivor_id: &str,
    now_ts: i64,
) -> Result<(), libsql::Error> {
    let rows = load_rows(conn, "b.page_id = ?1", Some(absorbed_id)).await?;
    let mut copied = false;
    for row in rows {
        if row.basis_page_id == survivor_id {
            continue;
        }
        let state = if row.state == BASIS_STATE_DELETED {
            BASIS_STATE_DELETED
        } else {
            BASIS_STATE_PENDING
        };
        let inserted = conn
            .execute(
                "INSERT OR IGNORE INTO page_basis \
                   (page_id, basis_page_id, basis_title, confirmed_version, confirmed_digest, \
                    confirmed_by, confirmed_at, state, changed_version, minor_only, \
                    added_by, added_at, basis_incarnation) \
                 VALUES (?1, ?2, ?3, ?4, '', 'page_merge', ?5, ?6, ?7, 0, ?8, ?9, ?10)",
                libsql::params![
                    survivor_id,
                    row.basis_page_id.as_str(),
                    row.basis_title.as_str(),
                    row.confirmed_version,
                    now_ts,
                    state,
                    row.changed_version,
                    row.added_by.as_str(),
                    row.added_at,
                    row.basis_incarnation.as_str()
                ],
            )
            .await?;
        copied |= inserted > 0;
    }
    if copied {
        bump_basis_revision(conn, survivor_id).await?;
        refresh_basis_state(conn, survivor_id).await?;
    }
    Ok(())
}

/// A SOURCE note keeps its content but changes id (the document it mirrors was
/// renamed): the rows it holds and the rows that rely on it follow it, in the
/// caller's transaction. Content and digests are unchanged, so confirmations
/// stay valid; `basis_incarnation` is re-pointed at the clone's own generation
/// (the clone is a new row, so it has a new one).
pub(super) async fn retarget_page_identity(
    conn: &libsql::Connection,
    old_page_id: &str,
    new_page_id: &str,
) -> Result<(), libsql::Error> {
    conn.execute(
        "UPDATE page_basis SET basis_page_id = ?2, \
             basis_incarnation = COALESCE((SELECT incarnation FROM pages WHERE id = ?2), \
                                          basis_incarnation) \
         WHERE basis_page_id = ?1",
        libsql::params![old_page_id, new_page_id],
    )
    .await?;
    conn.execute(
        "UPDATE pages SET basis_revision = basis_revision + 1 \
         WHERE id IN (SELECT page_id FROM page_basis WHERE basis_page_id = ?1)",
        libsql::params![new_page_id],
    )
    .await?;
    let held = conn
        .execute(
            "UPDATE page_basis SET page_id = ?2 WHERE page_id = ?1",
            libsql::params![old_page_id, new_page_id],
        )
        .await?;
    if held > 0 {
        // The clone does not copy `basis_revision`; carry it over and bump it,
        // so projections see the list as changed.
        conn.execute(
            "UPDATE pages SET basis_revision = \
                 (SELECT basis_revision FROM pages WHERE id = ?1) + 1 \
             WHERE id = ?2",
            libsql::params![old_page_id, new_page_id],
        )
        .await?;
    }
    refresh_basis_state(conn, new_page_id).await
}

async fn bump_basis_revision(
    conn: &libsql::Connection,
    page_id: &str,
) -> Result<(), libsql::Error> {
    conn.execute(
        "UPDATE pages SET basis_revision = basis_revision + 1 WHERE id = ?1",
        libsql::params![page_id],
    )
    .await?;
    Ok(())
}

/// A basis id the daemon has a record of deleting (so the refusal can say
/// "deleted" rather than "not found").
async fn known_deleted_basis(
    conn: &libsql::Connection,
    basis_page_id: &str,
) -> Result<bool, libsql::Error> {
    let mut rows = conn
        .query(
            "SELECT 1 FROM page_basis WHERE basis_page_id = ?1 AND state = 'deleted' LIMIT 1",
            libsql::params![basis_page_id],
        )
        .await?;
    Ok(rows.next().await?.is_some())
}

/// R6: the rules about the BASIS side of a mark (the citer's own rules are
/// checked separately): an ordinary active note, in the citer's Space, at a
/// version it really has.
fn check_basis_facts(
    basis: &PageFacts,
    citer_space: &str,
    read_version: i64,
) -> Result<(), BasisError> {
    if basis.kind == "entity" {
        return Err(BasisError::BasisIsEntityPage);
    }
    match basis.status.as_str() {
        "active" => {}
        "draft" => return Err(BasisError::BasisIsDraft),
        _ => return Err(BasisError::BasisArchived),
    }
    if basis.space != citer_space {
        return Err(BasisError::DifferentSpace);
    }
    if read_version < 1 || read_version > basis.version {
        return Err(BasisError::InvalidVersion {
            requested: read_version,
            current: basis.version,
        });
    }
    Ok(())
}

/// R5: the rules about the CITER side of a mark: an ordinary active note.
fn check_citer_facts(citer: &PageFacts) -> Result<(), BasisError> {
    if citer.kind == "entity" {
        return Err(BasisError::CiterIsEntityPage);
    }
    match citer.status.as_str() {
        "active" => Ok(()),
        "draft" => Err(BasisError::CiterIsDraft),
        _ => Err(BasisError::CiterNotActive),
    }
}

/// R7 for a staged mark, applied when its revision card is accepted: the same
/// rules as any mark, plus the basis must still be the note the agent read
/// (same incarnation). A reconfirm (the pair already exists) upserts like any
/// mark, so it confirms at the version the agent read.
async fn apply_card_mark(
    conn: &libsql::Connection,
    page_id: &str,
    mark: &StagedBasisMark,
    added_by: &str,
) -> Result<PageBasisEntry, BasisError> {
    if let Some(now) = load_facts(conn, &mark.basis_page_id, false).await? {
        if now.incarnation != mark.incarnation {
            return Err(BasisError::BasisUnavailable(
                BasisUnavailableReason::IncarnationChanged,
            ));
        }
    }
    mark_page_basis_on_conn(conn, page_id, &mark.basis_page_id, mark.version, added_by).await
}

/// R5-R7: mark `basis_page_id` as a basis of `page_id`, on the caller's open
/// transaction. Confirmed (`ok`) only when the basis is still at
/// `read_version`; otherwise `pending`, with an empty digest so that no later
/// refresh can promote it (a v1 read must never confirm v2).
pub(super) async fn mark_page_basis_on_conn(
    conn: &libsql::Connection,
    page_id: &str,
    basis_page_id: &str,
    read_version: i64,
    added_by: &str,
) -> Result<PageBasisEntry, BasisError> {
    if page_id == basis_page_id {
        return Err(BasisError::SelfReference);
    }
    let citer = load_facts(conn, page_id, false)
        .await?
        .ok_or(BasisError::CiterNotFound)?;
    check_citer_facts(&citer)?;
    let Some(basis) = load_facts(conn, basis_page_id, true).await? else {
        return Err(if known_deleted_basis(conn, basis_page_id).await? {
            BasisError::BasisDeleted
        } else {
            BasisError::BasisNotFound
        });
    };
    check_basis_facts(&basis, &citer.space, read_version)?;

    let now_ts = chrono::Utc::now().timestamp();
    let current = read_version == basis.version;
    let (state, digest, changed_version, minor_only) = if current {
        (
            BASIS_STATE_OK,
            basis_text_digest(&basis.content),
            None,
            false,
        )
    } else {
        (
            BASIS_STATE_PENDING,
            String::new(),
            Some(basis.version),
            minor_only_between(conn, basis_page_id, read_version, basis.version).await?,
        )
    };
    conn.execute(
        "INSERT INTO page_basis \
           (page_id, basis_page_id, basis_title, confirmed_version, confirmed_digest, \
            confirmed_by, confirmed_at, state, changed_version, minor_only, \
            added_by, added_at, basis_incarnation) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?6, ?7, ?11) \
         ON CONFLICT(page_id, basis_page_id) DO UPDATE SET \
           basis_title = excluded.basis_title, \
           confirmed_version = excluded.confirmed_version, \
           confirmed_digest = excluded.confirmed_digest, \
           confirmed_by = excluded.confirmed_by, \
           confirmed_at = excluded.confirmed_at, \
           state = excluded.state, \
           changed_version = excluded.changed_version, \
           minor_only = excluded.minor_only, \
           basis_incarnation = excluded.basis_incarnation",
        libsql::params![
            page_id,
            basis_page_id,
            basis.title.as_str(),
            read_version,
            digest,
            added_by,
            now_ts,
            state,
            changed_version,
            i64::from(minor_only),
            basis.incarnation.as_str()
        ],
    )
    .await?;
    bump_basis_revision(conn, page_id).await?;
    entry_for(conn, page_id, basis_page_id)
        .await?
        .ok_or(BasisError::NotABasis)
}

/// R10/R11: confirm that `page_id` has accounted for `basis_page_id` at
/// `version`, on the caller's open transaction. The version check and the write
/// are one unit: if the basis moved on, nothing is written and the error names
/// the version it is at now.
pub(super) async fn confirm_page_basis_on_conn(
    conn: &libsql::Connection,
    page_id: &str,
    basis_page_id: &str,
    version: i64,
    confirmed_by: &str,
) -> Result<PageBasisEntry, BasisError> {
    let row = load_row(conn, page_id, basis_page_id)
        .await?
        .ok_or(BasisError::NotABasis)?;
    let citer = load_facts(conn, page_id, false)
        .await?
        .ok_or(BasisError::CiterNotFound)?;
    let basis =
        load_facts(conn, basis_page_id, true)
            .await?
            .ok_or(BasisError::BasisUnavailable(
                BasisUnavailableReason::Deleted,
            ))?;
    if basis.incarnation != row.basis_incarnation {
        return Err(BasisError::BasisUnavailable(
            BasisUnavailableReason::IncarnationChanged,
        ));
    }
    if row.state == BASIS_STATE_DELETED {
        return Err(BasisError::BasisUnavailable(
            BasisUnavailableReason::Deleted,
        ));
    }
    if basis.status != "active" {
        return Err(BasisError::BasisUnavailable(
            BasisUnavailableReason::Archived,
        ));
    }
    if basis.space != citer.space {
        return Err(BasisError::BasisUnavailable(
            BasisUnavailableReason::OtherSpace,
        ));
    }
    if basis.version != version {
        return Err(BasisError::MovedOn {
            current: basis.version,
        });
    }
    conn.execute(
        "UPDATE page_basis \
         SET confirmed_version = ?3, confirmed_digest = ?4, confirmed_by = ?5, \
             confirmed_at = ?6, state = 'ok', changed_version = NULL, minor_only = 0, \
             basis_title = ?7 \
         WHERE page_id = ?1 AND basis_page_id = ?2",
        libsql::params![
            page_id,
            basis_page_id,
            version,
            basis_text_digest(&basis.content),
            confirmed_by,
            chrono::Utc::now().timestamp(),
            basis.title.as_str()
        ],
    )
    .await?;
    bump_basis_revision(conn, page_id).await?;
    entry_for(conn, page_id, basis_page_id)
        .await?
        .ok_or(BasisError::NotABasis)
}

/// What a page write also does to the note's bases, inside the write's own
/// transaction. The default (nothing to mark, nothing to confirm, a major
/// edit) is what every writer that predates the page basis passes.
#[derive(Debug, Clone, Default)]
pub struct BasisWrite {
    /// The history row is flagged as a minor edit (typo, formatting).
    pub minor: bool,
    /// Who marks and confirms: `human`, or the agent's name.
    pub actor: String,
    /// R7: bases this write relied on, as `(basis_page_id, version read)`.
    /// A basis still at that version is confirmed; one that moved on lands
    /// `pending`. A mark that is refused rolls the whole write back.
    pub marks: Vec<(String, i64)>,
    /// R7 for an accepted revision card: the marks an agent's write carried
    /// when it was staged instead of landing (a locked note). Each is applied
    /// exactly like `marks`, and is refused when the basis note is no longer
    /// the incarnation the agent read. A refused one rolls the accept back.
    pub card_marks: Vec<StagedBasisMark>,
    /// R11: confirm one basis in the save's own transaction, only if it is
    /// still at that version.
    pub confirm: Option<(String, i64)>,
    /// R9: the basis notes an "update from sources" card was written from.
    /// Each one is confirmed in this write's transaction only if it still
    /// matches what the card read (same incarnation, same normalized text;
    /// the version may have moved by a formatting-only edit); one whose text
    /// changed, that was removed, or that became unreadable is left as it is.
    /// Nothing is ever added.
    pub confirm_snapshot: Vec<BasisSnapshotEntry>,
    /// Lines of code this write relied on, each already fingerprinted from
    /// the file as it reads now (`crate::code_basis::prepare_code_basis`).
    /// Recorded in this write's transaction, only if the write lands. A span
    /// the note already has is re-anchored and `ok`; others are untouched.
    pub code_basis: Vec<crate::code_basis::PreparedCodeBasis>,
    /// A Space-locked agent's write lands only while the note is still in
    /// this Space, checked in the write's own transaction. A note moved out
    /// of it after the caller's check reads as not found: nothing is written,
    /// marked, or staged as a revision card. `None` writes wherever it is.
    pub space_guard: Option<crate::read_scope::ReadScope>,
}

/// One basis note as an "update from sources" run read it, recorded on the
/// revision card it staged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BasisSnapshotEntry {
    pub basis_page_id: String,
    pub incarnation: String,
    pub version: i64,
    pub digest: String,
}

/// A basis an agent's write relied on, recorded on the revision card the
/// write was staged as (a locked note), so accepting the card records it. The
/// incarnation is the basis note's when the card was staged: a note deleted
/// and recreated under the same id since is not the note the agent read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StagedBasisMark {
    pub basis_page_id: String,
    pub version: i64,
    pub incarnation: String,
}

/// A basis note's current text, as context for an "update from sources" run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BasisContext {
    pub title: String,
    pub content: String,
    pub snapshot: BasisSnapshotEntry,
}

/// What `BasisWrite::confirm` did, when the write carried one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BasisWriteOutcome {
    pub confirm: Option<wenlan_types::BasisConfirmOutcome>,
}

/// Apply a write's marks and confirm on the caller's open transaction, after
/// the note's own row and history row are written. A basis that cannot be
/// confirmed because it moved on (or is no longer readable) is reported, not
/// raised: the save itself must still land. Anything else is a caller error
/// and fails the write.
pub(super) async fn apply_basis_write_on_conn(
    conn: &libsql::Connection,
    page_id: &str,
    basis: &BasisWrite,
) -> Result<BasisWriteOutcome, BasisError> {
    for (basis_page_id, version) in &basis.marks {
        mark_page_basis_on_conn(conn, page_id, basis_page_id, *version, &basis.actor).await?;
    }
    for mark in &basis.card_marks {
        apply_card_mark(conn, page_id, mark, &basis.actor)
            .await
            .map_err(|reason| match reason {
                BasisError::Db(inner) => BasisError::Db(inner),
                reason => BasisError::CardMarkRefused {
                    basis_page_id: mark.basis_page_id.clone(),
                    reason: Box::new(reason),
                },
            })?;
    }
    let mut outcome = BasisWriteOutcome::default();
    if let Some((basis_page_id, version)) = &basis.confirm {
        outcome.confirm = Some(
            match confirm_page_basis_on_conn(conn, page_id, basis_page_id, *version, &basis.actor)
                .await
            {
                Ok(_) => wenlan_types::BasisConfirmOutcome::Confirmed,
                Err(BasisError::MovedOn { .. } | BasisError::BasisUnavailable(_)) => {
                    wenlan_types::BasisConfirmOutcome::MovedOn
                }
                Err(other) => return Err(other),
            },
        );
    }
    for entry in &basis.confirm_snapshot {
        confirm_snapshot_entry_on_conn(conn, page_id, entry, &basis.actor).await?;
    }
    Ok(outcome)
}

/// R9: confirm one basis only if it is exactly what the card read. Returns
/// whether it was confirmed. A basis that is no longer a basis of the note
/// (removed after the card was made), moved on, changed text, or became
/// unreadable stays as it is; this never inserts a row.
async fn confirm_snapshot_entry_on_conn(
    conn: &libsql::Connection,
    page_id: &str,
    entry: &BasisSnapshotEntry,
    confirmed_by: &str,
) -> Result<bool, BasisError> {
    let Some(basis) = load_facts(conn, &entry.basis_page_id, true).await? else {
        return Ok(false);
    };
    // The text decides, not the version: a formatting-only edit moves the
    // version but not the digest, and costs the user no extra confirm. A match
    // confirms at the basis' CURRENT version.
    if basis.incarnation != entry.incarnation || basis_text_digest(&basis.content) != entry.digest {
        return Ok(false);
    }
    match confirm_page_basis_on_conn(
        conn,
        page_id,
        &entry.basis_page_id,
        basis.version,
        confirmed_by,
    )
    .await
    {
        Ok(_) => Ok(true),
        Err(
            BasisError::NotABasis | BasisError::MovedOn { .. } | BasisError::BasisUnavailable(_),
        ) => Ok(false),
        Err(other) => Err(other),
    }
}

const ENTRY_SELECT: &str = "SELECT b.basis_page_id, b.basis_title, b.state, b.confirmed_version, \
    b.changed_version, b.minor_only, b.confirmed_by, b.confirmed_at, b.added_by, b.added_at, \
    p.title, p.version \
    FROM page_basis b \
    LEFT JOIN pages p ON p.id = b.basis_page_id AND p.incarnation = b.basis_incarnation";

fn entry_from(row: &libsql::Row) -> Result<PageBasisEntry, libsql::Error> {
    let stored_state: String = row.get(2)?;
    let live_title: Option<String> = row.get(10)?;
    let live_version: Option<i64> = row.get(11)?;
    // A basis that no longer resolves (gone, or its id reused) reads as
    // deleted whatever the stored state says.
    let gone = live_version.is_none();
    Ok(PageBasisEntry {
        basis_page_id: row.get(0)?,
        title: live_title.unwrap_or(row.get::<String>(1)?),
        state: if gone {
            BASIS_STATE_DELETED.to_string()
        } else {
            stored_state
        },
        confirmed_version: row.get(3)?,
        current_version: live_version,
        changed_version: row.get::<Option<i64>>(4)?,
        minor_only: row.get::<i64>(5)? != 0,
        confirmed_by: row.get(6)?,
        confirmed_at: row.get(7)?,
        added_by: row.get(8)?,
        added_at: row.get(9)?,
    })
}

async fn entry_for(
    conn: &libsql::Connection,
    page_id: &str,
    basis_page_id: &str,
) -> Result<Option<PageBasisEntry>, libsql::Error> {
    let sql = format!("{ENTRY_SELECT} WHERE b.page_id = ?1 AND b.basis_page_id = ?2");
    let mut rows = conn
        .query(&sql, libsql::params![page_id, basis_page_id])
        .await?;
    match rows.next().await? {
        Some(row) => Ok(Some(entry_from(&row)?)),
        None => Ok(None),
    }
}

impl MemoryDB {
    /// Mark `basis_page_id` as a basis of `page_id` at the version the caller
    /// read. See [`mark_page_basis_on_conn`] for the rules.
    pub async fn mark_page_basis(
        &self,
        page_id: &str,
        basis_page_id: &str,
        read_version: i64,
        added_by: &str,
    ) -> Result<PageBasisEntry, BasisError> {
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(libsql::TransactionBehavior::Immediate)
            .await?;
        let entry =
            mark_page_basis_on_conn(&tx, page_id, basis_page_id, read_version, added_by).await?;
        tx.commit().await?;
        Ok(entry)
    }

    /// Stop relying on `basis_page_id`. Returns whether a row was removed.
    pub async fn unmark_page_basis(
        &self,
        page_id: &str,
        basis_page_id: &str,
    ) -> Result<bool, BasisError> {
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(libsql::TransactionBehavior::Immediate)
            .await?;
        let removed = tx
            .execute(
                "DELETE FROM page_basis WHERE page_id = ?1 AND basis_page_id = ?2",
                libsql::params![page_id, basis_page_id],
            )
            .await?;
        if removed > 0 {
            bump_basis_revision(&tx, page_id).await?;
            // The automatic refresh skipped this note only because it relied
            // on other notes; with the last one gone, it is refreshable again.
            tx.execute(
                "UPDATE pages SET refresh_blocked_reason = NULL \
                 WHERE id = ?1 AND refresh_blocked_reason = ?2 \
                   AND NOT EXISTS (SELECT 1 FROM page_basis WHERE page_id = ?1)",
                libsql::params![
                    page_id,
                    crate::synthesis::distill::REFRESH_BLOCKED_HAS_BASIS
                ],
            )
            .await?;
        }
        tx.commit().await?;
        Ok(removed > 0)
    }

    /// Confirm the citer has accounted for the basis at `version`. The version
    /// check and the write share one transaction; `MovedOn` means the basis
    /// changed again after the caller looked.
    pub async fn confirm_page_basis(
        &self,
        page_id: &str,
        basis_page_id: &str,
        version: i64,
        confirmed_by: &str,
    ) -> Result<PageBasisEntry, BasisError> {
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(libsql::TransactionBehavior::Immediate)
            .await?;
        let entry =
            confirm_page_basis_on_conn(&tx, page_id, basis_page_id, version, confirmed_by).await?;
        tx.commit().await?;
        Ok(entry)
    }

    /// Every basis of a note, oldest first.
    pub async fn list_page_basis(&self, page_id: &str) -> Result<Vec<PageBasisEntry>, WenlanError> {
        let conn = self.conn.lock().await;
        let sql =
            format!("{ENTRY_SELECT} WHERE b.page_id = ?1 ORDER BY b.added_at, b.basis_page_id");
        let mut rows = conn
            .query(&sql, libsql::params![page_id])
            .await
            .map_err(|e| WenlanError::VectorDb(format!("list_page_basis: {e}")))?;
        let mut out = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| WenlanError::VectorDb(format!("list_page_basis row: {e}")))?
        {
            out.push(
                entry_from(&row)
                    .map_err(|e| WenlanError::VectorDb(format!("list_page_basis decode: {e}")))?,
            );
        }
        Ok(out)
    }

    /// R9: the bases a note may read when it is updated from its sources: state
    /// `ok` or `pending`, active, in the note's own Space, and still the same
    /// incarnation that was marked. Each carries the text and the snapshot a
    /// revision card records. One consistent read per basis (text, version and
    /// incarnation come from the same row). Oldest first.
    pub async fn list_basis_context(
        &self,
        page_id: &str,
    ) -> Result<Vec<BasisContext>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT p.id, p.title, p.content, p.version, COALESCE(p.incarnation, '') \
                 FROM page_basis b \
                 JOIN pages c ON c.id = b.page_id \
                 JOIN pages p ON p.id = b.basis_page_id AND p.incarnation = b.basis_incarnation \
                 WHERE b.page_id = ?1 AND b.state IN ('ok', 'pending') \
                   AND p.status = 'active' \
                   AND COALESCE(p.space, ?2) = COALESCE(c.space, ?2) \
                 ORDER BY b.added_at, b.basis_page_id",
                libsql::params![page_id, UNFILED_SPACE_ID],
            )
            .await
            .map_err(|e| WenlanError::VectorDb(format!("list_basis_context: {e}")))?;
        let mut out = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| WenlanError::VectorDb(format!("list_basis_context row: {e}")))?
        {
            let decode =
                |e: libsql::Error| WenlanError::VectorDb(format!("list_basis_context decode: {e}"));
            let content: String = row.get(2).map_err(decode)?;
            out.push(BasisContext {
                title: row.get(1).map_err(decode)?,
                snapshot: BasisSnapshotEntry {
                    basis_page_id: row.get(0).map_err(decode)?,
                    incarnation: row.get(4).map_err(decode)?,
                    version: row.get(3).map_err(decode)?,
                    digest: basis_text_digest(&content),
                },
                content,
            });
        }
        Ok(out)
    }

    /// Whether the note has any basis row at all, whatever its state.
    pub async fn page_has_basis(&self, page_id: &str) -> Result<bool, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT 1 FROM page_basis WHERE page_id = ?1 LIMIT 1",
                libsql::params![page_id],
            )
            .await
            .map_err(|e| WenlanError::VectorDb(format!("page_has_basis: {e}")))?;
        Ok(rows
            .next()
            .await
            .map_err(|e| WenlanError::VectorDb(format!("page_has_basis row: {e}")))?
            .is_some())
    }

    /// Notes waiting on a basis that is not `ok`, grouped by basis note and
    /// state, optionally limited to one Space (the citer's). Archived, draft
    /// and entity citers are left out: nobody is waiting on their behalf.
    pub async fn list_basis_pending(
        &self,
        space: Option<&str>,
    ) -> Result<Vec<BasisPendingGroup>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT b.basis_page_id, b.state, b.changed_version, b.basis_title, \
                        b.page_id, b.minor_only, c.title, p.title, p.id \
                 FROM page_basis b \
                 JOIN pages c ON c.id = b.page_id \
                 LEFT JOIN pages p ON p.id = b.basis_page_id \
                                  AND p.incarnation = b.basis_incarnation \
                 WHERE c.status = 'active' AND COALESCE(c.kind, 'concept') <> 'entity' \
                   AND (?1 IS NULL OR COALESCE(c.space, ?2) = ?1)",
                libsql::params![space, UNFILED_SPACE_ID],
            )
            .await
            .map_err(|e| WenlanError::VectorDb(format!("list_basis_pending: {e}")))?;
        let mut groups: Vec<BasisPendingGroup> = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| WenlanError::VectorDb(format!("list_basis_pending row: {e}")))?
        {
            let decode =
                |e: libsql::Error| WenlanError::VectorDb(format!("list_basis_pending decode: {e}"));
            let basis_page_id: String = row.get(0).map_err(decode)?;
            let stored_state: String = row.get(1).map_err(decode)?;
            let changed_version: Option<i64> = row.get(2).map_err(decode)?;
            let snapshot_title: String = row.get(3).map_err(decode)?;
            let citer = BasisPendingCiter {
                page_id: row.get(4).map_err(decode)?,
                minor_only: row.get::<i64>(5).map_err(decode)? != 0,
                title: row.get(6).map_err(decode)?,
            };
            let live_title: Option<String> = row.get(7).map_err(decode)?;
            let live_id: Option<String> = row.get(8).map_err(decode)?;
            let state = if live_id.is_none() {
                BASIS_STATE_DELETED.to_string()
            } else {
                stored_state
            };
            if state == BASIS_STATE_OK {
                continue;
            }
            match groups
                .iter_mut()
                .find(|g| g.basis_page_id == basis_page_id && g.state == state)
            {
                Some(group) => {
                    group.changed_version = group.changed_version.max(changed_version);
                    group.citers.push(citer);
                }
                None => groups.push(BasisPendingGroup {
                    basis_page_id,
                    title: live_title.unwrap_or(snapshot_title),
                    state,
                    changed_version,
                    citers: vec![citer],
                }),
            }
        }
        for group in &mut groups {
            group
                .citers
                .sort_by_key(|c| (c.title.to_lowercase(), c.page_id.clone()));
        }
        groups.sort_by_key(|g| {
            (
                g.title.to_lowercase(),
                g.basis_page_id.clone(),
                g.state.clone(),
            )
        });
        Ok(groups)
    }

    /// [`Self::list_basis_pending`] for a read scope: `Global` lists every
    /// Space, `Uncategorized` the unfiled notes.
    pub async fn list_basis_pending_in_scope(
        &self,
        scope: &crate::read_scope::ReadScope,
    ) -> Result<Vec<BasisPendingGroup>, WenlanError> {
        use crate::read_scope::ReadScope;
        match scope {
            ReadScope::Global => self.list_basis_pending(None).await,
            ReadScope::Space(space) => self.list_basis_pending(Some(space)).await,
            ReadScope::Uncategorized => self.list_basis_pending(Some(UNFILED_SPACE_ID)).await,
        }
    }

    /// What changed in the basis since the citer last confirmed it: both texts
    /// and the versions in between. `Unavailable` when the basis was deleted
    /// (or its id reused), is archived, now sits in another Space, or the
    /// version the citer relied on is no longer stored.
    pub async fn basis_diff(
        &self,
        page_id: &str,
        basis_page_id: &str,
    ) -> Result<BasisDiffOutcome, BasisError> {
        self.basis_diff_in_scope(
            page_id,
            basis_page_id,
            &crate::read_scope::ReadScope::Global,
        )
        .await
    }

    /// [`Self::basis_diff`] for a caller limited to `scope`. The Space check
    /// runs under the same connection lock as the read of both texts, so a
    /// note moved out of the caller's Space after any earlier visibility check
    /// can never have its text returned: it reads as
    /// [`BasisDiffOutcome::OutOfScope`], the same as a note that is not there.
    pub async fn basis_diff_in_scope(
        &self,
        page_id: &str,
        basis_page_id: &str,
        scope: &crate::read_scope::ReadScope,
    ) -> Result<BasisDiffOutcome, BasisError> {
        let conn = self.conn.lock().await;
        if !matches!(scope, crate::read_scope::ReadScope::Global) {
            // Citer first, then basis: the order the route's visibility check
            // names them in.
            // The column and match are the ones `get_page_scoped_browse` (the
            // route's visibility check) uses.
            for id in [page_id, basis_page_id] {
                let mut rows = conn
                    .query(
                        "SELECT workspace FROM pages WHERE id = ?1",
                        libsql::params![id],
                    )
                    .await?;
                let in_scope = match rows.next().await? {
                    Some(row) => scope.matches(row.get::<Option<String>>(0)?.as_deref()),
                    None => false,
                };
                drop(rows);
                if !in_scope {
                    return Ok(BasisDiffOutcome::OutOfScope(id.to_string()));
                }
            }
        }
        let row = load_row(&conn, page_id, basis_page_id)
            .await?
            .ok_or(BasisError::NotABasis)?;
        let unavailable = |reason| Ok(BasisDiffOutcome::Unavailable(reason));
        let Some(basis) = load_facts(&conn, basis_page_id, true).await? else {
            return unavailable(BasisUnavailableReason::Deleted);
        };
        // A reused id reads deleted in the list, but the diff can say why.
        if basis.incarnation != row.basis_incarnation {
            return unavailable(BasisUnavailableReason::IncarnationChanged);
        }
        if row.state == BASIS_STATE_DELETED {
            return unavailable(BasisUnavailableReason::Deleted);
        }
        if basis.status != "active" {
            return unavailable(BasisUnavailableReason::Archived);
        }
        let citer = load_facts(&conn, page_id, false)
            .await?
            .ok_or(BasisError::CiterNotFound)?;
        if citer.space != basis.space {
            return unavailable(BasisUnavailableReason::OtherSpace);
        }
        let mut rows = conn
            .query(
                "SELECT content FROM page_history WHERE page_id = ?1 AND version = ?2",
                libsql::params![basis_page_id, row.confirmed_version],
            )
            .await?;
        let Some(from_row) = rows.next().await? else {
            return unavailable(BasisUnavailableReason::HistoryGone);
        };
        let from_content: String = from_row.get(0)?;
        drop(rows);
        let mut summaries: HashMap<i64, String> = HashMap::new();
        let mut changelog_rows = conn
            .query(
                "SELECT changelog FROM pages WHERE id = ?1",
                libsql::params![basis_page_id],
            )
            .await?;
        if let Some(changelog_row) = changelog_rows.next().await? {
            let json: Option<String> = changelog_row.get(0)?;
            let entries: Vec<serde_json::Value> = json
                .and_then(|j| serde_json::from_str(&j).ok())
                .unwrap_or_default();
            for entry in entries {
                if let (Some(version), Some(summary)) = (
                    entry.get("version").and_then(|v| v.as_i64()),
                    entry.get("delta_summary").and_then(|v| v.as_str()),
                ) {
                    summaries.insert(version, summary.to_string());
                }
            }
        }
        drop(changelog_rows);
        let mut edit_rows = conn
            .query(
                "SELECT version, edited_by, created_at, minor FROM page_history \
                 WHERE page_id = ?1 AND version > ?2 AND version <= ?3 ORDER BY version",
                libsql::params![basis_page_id, row.confirmed_version, basis.version],
            )
            .await?;
        let mut edits = Vec::new();
        while let Some(edit) = edit_rows.next().await? {
            let version: i64 = edit.get(0)?;
            edits.push(BasisEdit {
                version,
                edited_by: edit.get(1)?,
                at: edit.get(2)?,
                minor: edit.get::<i64>(3)? != 0,
                delta_summary: summaries.get(&version).cloned().unwrap_or_default(),
            });
        }
        Ok(BasisDiffOutcome::Available(BasisDiff {
            basis_page_id: basis_page_id.to_string(),
            title: basis.title,
            from_version: row.confirmed_version,
            to_version: basis.version,
            from_content,
            to_content: basis.content,
            edits,
        }))
    }

    /// R7 for a write that is staged as a revision card instead of landing
    /// (a locked note): every mark must pass the rules a direct write's mark
    /// does, checked now so the agent hears of a refusal at once rather than
    /// when the card is accepted. Returns each mark with the basis note's
    /// current incarnation, which is what the card records.
    pub async fn prepare_staged_basis_marks(
        &self,
        page_id: &str,
        marks: &[(String, i64)],
    ) -> Result<Vec<StagedBasisMark>, BasisError> {
        if marks.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.conn.lock().await;
        let citer = load_facts(&conn, page_id, false)
            .await?
            .ok_or(BasisError::CiterNotFound)?;
        check_citer_facts(&citer)?;
        let mut staged = Vec::with_capacity(marks.len());
        for (basis_page_id, version) in marks {
            if basis_page_id == page_id {
                return Err(BasisError::SelfReference);
            }
            let Some(basis) = load_facts(&conn, basis_page_id, false).await? else {
                return Err(if known_deleted_basis(&conn, basis_page_id).await? {
                    BasisError::BasisDeleted
                } else {
                    BasisError::BasisNotFound
                });
            };
            check_basis_facts(&basis, &citer.space, *version)?;
            staged.push(StagedBasisMark {
                basis_page_id: basis_page_id.clone(),
                version: *version,
                incarnation: basis.incarnation,
            });
        }
        Ok(staged)
    }

    /// R7 for a write that changed nothing (same text, same sources): there is
    /// no page write for the marks to ride on, so they land here on their own,
    /// all in one transaction. A refused mark records none of them. The note
    /// must still be at `at_version`, the version the write compared itself
    /// against; an edit that landed since answers [`BasisError::CiterMovedOn`]
    /// and records nothing. A note no longer in `space_guard` answers
    /// [`BasisError::CiterNotFound`].
    pub async fn mark_page_bases(
        &self,
        page_id: &str,
        at_version: i64,
        marks: &[(String, i64)],
        added_by: &str,
        space_guard: Option<&crate::read_scope::ReadScope>,
    ) -> Result<(), BasisError> {
        if marks.is_empty() {
            return Ok(());
        }
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(libsql::TransactionBehavior::Immediate)
            .await?;
        let mut rows = tx
            .query(
                "SELECT version, space FROM pages WHERE id = ?1",
                libsql::params![page_id],
            )
            .await?;
        let (current, space) = match rows.next().await? {
            Some(row) => (row.get::<i64>(0)?, row.get::<Option<String>>(1)?),
            None => return Err(BasisError::CiterNotFound),
        };
        drop(rows);
        if space_guard.is_some_and(|scope| !scope.matches(space.as_deref())) {
            return Err(BasisError::CiterNotFound);
        }
        if current != at_version {
            return Err(BasisError::CiterMovedOn { current });
        }
        for (basis_page_id, version) in marks {
            mark_page_basis_on_conn(&tx, page_id, basis_page_id, *version, added_by).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// R7, before a NEW note exists: would every `(basis_page_id, version)`
    /// be an acceptable basis for a note in `space`? Same rules as a mark,
    /// checked up front so a create is refused whole rather than created
    /// without the basis the agent said it relied on.
    pub async fn check_basis_candidates(
        &self,
        space: Option<&str>,
        marks: &[(String, i64)],
    ) -> Result<(), BasisError> {
        let conn = self.conn.lock().await;
        let space = space.unwrap_or(UNFILED_SPACE_ID);
        for (basis_page_id, version) in marks {
            let Some(basis) = load_facts(&conn, basis_page_id, false).await? else {
                return Err(if known_deleted_basis(&conn, basis_page_id).await? {
                    BasisError::BasisDeleted
                } else {
                    BasisError::BasisNotFound
                });
            };
            check_basis_facts(&basis, space, *version)?;
        }
        Ok(())
    }

    /// Which of `ids` are the id of a note (in any status). A source list is
    /// for memories; this is how a note id slipped into one is caught.
    pub async fn page_ids_present(&self, ids: &[String]) -> Result<Vec<String>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut found = Vec::new();
        for chunk in ids.chunks(400) {
            let placeholders = vec!["?"; chunk.len()].join(",");
            let sql = format!("SELECT id FROM pages WHERE id IN ({placeholders})");
            let params: Vec<libsql::Value> = chunk
                .iter()
                .map(|id| libsql::Value::Text(id.clone()))
                .collect();
            let mut rows = conn
                .query(&sql, libsql::params_from_iter(params))
                .await
                .map_err(|e| WenlanError::VectorDb(format!("page_ids_present: {e}")))?;
            while let Some(row) = rows
                .next()
                .await
                .map_err(|e| WenlanError::VectorDb(format!("page_ids_present row: {e}")))?
            {
                found.push(
                    row.get::<String>(0)
                        .map_err(|e| WenlanError::VectorDb(format!("page_ids_present id: {e}")))?,
                );
            }
        }
        Ok(found)
    }

    /// The bases of many notes that are NOT `ok`, keyed by citer. Notes with
    /// nothing to look at are absent, so a list of mostly-quiet notes stays
    /// small.
    pub async fn list_non_ok_basis(
        &self,
        page_ids: &[String],
    ) -> Result<HashMap<String, Vec<PageBasisEntry>>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut out: HashMap<String, Vec<PageBasisEntry>> = HashMap::new();
        let select =
            ENTRY_SELECT.replacen(" FROM page_basis b", ", b.page_id FROM page_basis b", 1);
        for chunk in page_ids.chunks(400) {
            let placeholders = vec!["?"; chunk.len()].join(",");
            // `p.id IS NULL` keeps a basis whose note is gone: it reads as
            // deleted whatever state the row last stored.
            let sql = format!(
                "{select} WHERE b.page_id IN ({placeholders}) \
                 AND (b.state <> 'ok' OR p.id IS NULL) \
                 ORDER BY b.added_at, b.basis_page_id"
            );
            let params: Vec<libsql::Value> = chunk
                .iter()
                .map(|id| libsql::Value::Text(id.clone()))
                .collect();
            let mut rows = conn
                .query(&sql, libsql::params_from_iter(params))
                .await
                .map_err(|e| WenlanError::VectorDb(format!("list_non_ok_basis: {e}")))?;
            while let Some(row) = rows
                .next()
                .await
                .map_err(|e| WenlanError::VectorDb(format!("list_non_ok_basis row: {e}")))?
            {
                let decode = |e: libsql::Error| {
                    WenlanError::VectorDb(format!("list_non_ok_basis decode: {e}"))
                };
                let entry = entry_from(&row).map_err(decode)?;
                let citer: String = row.get(12).map_err(decode)?;
                out.entry(citer).or_default().push(entry);
            }
        }
        Ok(out)
    }

    /// R12 (agents only): the notes `page_id` links to that changed after it
    /// did. Computed from the link index and the two `last_modified` stamps at
    /// read time; nothing is stored.
    pub async fn linked_changed(
        &self,
        page_id: &str,
    ) -> Result<Vec<(String, String, i64)>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT DISTINCT t.id, t.title, t.version \
                 FROM edges l \
                 JOIN pages c ON c.id = l.src_id \
                 JOIN pages t ON t.id = l.dst_id \
                 WHERE l.edge_type = 'links' AND l.src_kind = 'page' AND l.dst_kind = 'page' \
                   AND l.valid_until IS NULL \
                   AND l.src_id = ?1 AND t.id <> c.id \
                   AND t.status = 'active' AND COALESCE(t.kind, 'concept') <> 'entity' \
                   AND julianday(t.last_modified) > julianday(c.last_modified) \
                 ORDER BY lower(t.title), t.id",
                libsql::params![page_id],
            )
            .await
            .map_err(|e| WenlanError::VectorDb(format!("linked_changed: {e}")))?;
        let mut out = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| WenlanError::VectorDb(format!("linked_changed row: {e}")))?
        {
            let decode =
                |e: libsql::Error| WenlanError::VectorDb(format!("linked_changed decode: {e}"));
            out.push((
                row.get(0).map_err(decode)?,
                row.get(1).map_err(decode)?,
                row.get(2).map_err(decode)?,
            ));
        }
        Ok(out)
    }
}
