// SPDX-License-Identifier: Apache-2.0
//! Routes for a note that relies on another note (its **basis**), and the
//! shared helpers every other page route uses to put a note's bases on the
//! wire.
//!
//! The storage rules (what may be a basis, when a basis turns pending, the
//! version check on confirm) live in `wenlan_core::db::page_basis`. This module
//! only frames them as HTTP and decides what the caller may see: a basis note
//! the caller cannot read in its Space or by truth grant reads as absent, and
//! its title is withheld from lists rather than leaked.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::Json;
use serde::Deserialize;
use tokio::sync::RwLock;

use wenlan_core::db::{BasisDiffOutcome, BasisError, MemoryDB, PageBasisEntry};
use wenlan_core::read_scope::ReadScope;
use wenlan_core::truth_contract::TruthGrant;
use wenlan_types::{
    BasisDiffEdit, BasisDiffResponse, BasisPendingCiterRef, BasisPendingEntry, BasisRef,
    BasisRemoveResponse, BasisVersionRequest,
};

use crate::error::ServerError;
use crate::route_registry::{get, post, put, TrackedRouter};
use crate::state::{ServerState, SharedState};

pub(crate) fn register(router: TrackedRouter<SharedState>) -> TrackedRouter<SharedState> {
    router
        .route("/api/pages/basis-pending", get(handle_basis_pending))
        .route(
            "/api/pages/{id}/basis/{basis_id}",
            put(handle_mark_basis).delete(handle_unmark_basis),
        )
        .route(
            "/api/pages/{id}/basis/{basis_id}/confirm",
            post(handle_confirm_basis),
        )
        .route(
            "/api/pages/{id}/basis/{basis_id}/diff",
            get(handle_basis_diff),
        )
}

/// The grant the mutation routes in this module resolve to. Their manifest
/// marker shape is `None`, so a reader-intent marker never reaches the handler:
/// a mutation response can only be automatic.
pub(crate) const MUTATION_GRANT: TruthGrant = TruthGrant::Automatic;

/// Who marks or confirms: the agent named in `x-agent-name`, otherwise the
/// person using the app.
pub(crate) fn actor_from(headers: &HeaderMap) -> String {
    headers
        .get("x-agent-name")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| "human".to_string())
}

/// Map a basis refusal onto the HTTP error its kind implies: 404 for a note
/// that is not there, 409 for one that moved on or cannot be read, 422 for a
/// request the rules forbid.
pub(crate) fn basis_error(error: BasisError) -> ServerError {
    ServerError::from(wenlan_core::WenlanError::from(error))
}

/// Of `ids`, those the caller may see: in scope, and in full under `grant`.
/// Anything else must read as absent.
pub(crate) async fn visible_ids(
    db: &MemoryDB,
    grant: &TruthGrant,
    scope: &ReadScope,
    ids: &[String],
) -> Result<HashSet<String>, ServerError> {
    let mut unique: Vec<String> = ids.to_vec();
    unique.sort();
    unique.dedup();
    let allowed =
        wenlan_core::truth_adapter::filter_page_refs(db, grant, unique, |id| id.as_str()).await?;
    if matches!(scope, ReadScope::Global) {
        return Ok(allowed.into_iter().collect());
    }
    let mut out = HashSet::new();
    for id in allowed {
        if db.get_page_scoped_browse(&id, scope).await?.is_some() {
            out.insert(id);
        }
    }
    Ok(out)
}

/// A note's bases on the wire. The title of a basis the caller cannot see is
/// withheld (empty); the state and versions are not secret and stay, so the
/// notice still makes sense. A deleted basis keeps the title the citing note
/// last knew it by.
fn to_ref(entry: PageBasisEntry, visible: &HashSet<String>) -> BasisRef {
    let title = if entry.current_version.is_none() || visible.contains(&entry.basis_page_id) {
        entry.title
    } else {
        String::new()
    };
    BasisRef {
        page_id: entry.basis_page_id,
        title,
        state: entry.state,
        from_version: entry.confirmed_version,
        to_version: entry.current_version,
        minor_only: entry.minor_only,
    }
}

pub(crate) async fn basis_refs(
    db: &MemoryDB,
    grant: &TruthGrant,
    scope: &ReadScope,
    entries: Vec<PageBasisEntry>,
) -> Result<Vec<BasisRef>, ServerError> {
    if entries.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<String> = entries.iter().map(|e| e.basis_page_id.clone()).collect();
    let visible = visible_ids(db, grant, scope, &ids).await?;
    Ok(entries.into_iter().map(|e| to_ref(e, &visible)).collect())
}

/// The non-ok bases of many notes, keyed by the citing note (a list or a
/// recall result). Notes with nothing to look at are absent.
pub(crate) async fn non_ok_basis_by_citer(
    db: &MemoryDB,
    grant: &TruthGrant,
    scope: &ReadScope,
    page_ids: &[String],
) -> Result<HashMap<String, Vec<BasisRef>>, ServerError> {
    if page_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let by_citer = db.list_non_ok_basis(page_ids).await?;
    if by_citer.is_empty() {
        return Ok(HashMap::new());
    }
    let ids: Vec<String> = by_citer
        .values()
        .flatten()
        .map(|e| e.basis_page_id.clone())
        .collect();
    let visible = visible_ids(db, grant, scope, &ids).await?;
    Ok(by_citer
        .into_iter()
        .map(|(citer, entries)| {
            (
                citer,
                entries.into_iter().map(|e| to_ref(e, &visible)).collect(),
            )
        })
        .collect())
}

/// Every basis of a note just written, for the write response: the same shape
/// `read_page` uses, so the writer sees pending versus ok directly. They are
/// shown under the mutation grant, within the writer's Space: a basis the note
/// kept after it moved to another Space has its title withheld, as on a read.
pub(crate) async fn written_basis(
    db: &MemoryDB,
    scope: &ReadScope,
    page_id: &str,
) -> Result<Vec<BasisRef>, ServerError> {
    let entries = db.list_page_basis(page_id).await?;
    basis_refs(db, &MUTATION_GRANT, scope, entries).await
}

/// A source list is for memories. A note id in one is a mistake an agent can
/// act on, so name the right field instead of silently dropping the id. Only a
/// note the caller can see is named; one in another Space is left for the
/// source check, which answers for it as for any id that is not a memory.
pub(crate) async fn refuse_note_ids_in_sources(
    db: &MemoryDB,
    scope: &ReadScope,
    source_memory_ids: &[String],
) -> Result<(), ServerError> {
    if source_memory_ids.is_empty() {
        return Ok(());
    }
    let present = db.page_ids_present(source_memory_ids).await?;
    if present.is_empty() {
        return Ok(());
    }
    let visible = visible_ids(db, &MUTATION_GRANT, scope, &present).await?;
    let notes: Vec<String> = present
        .into_iter()
        .filter(|id| visible.contains(id))
        .collect();
    if notes.is_empty() {
        return Ok(());
    }
    Err(ServerError::ValidationError(format!(
        "source_memory_ids takes memories, but {} {} a note: use `basis` for notes",
        notes.join(", "),
        if notes.len() == 1 { "is" } else { "are" },
    )))
}

/// Every note a write names as a basis must be visible to the caller, or it
/// reads as absent. This runs before the rule checks, so a refusal about a
/// version or a Space can never confirm that a note the caller cannot see
/// exists.
pub(crate) async fn require_basis_visible(
    db: &MemoryDB,
    scope: &ReadScope,
    marks: &[(String, i64)],
) -> Result<(), ServerError> {
    if marks.is_empty() {
        return Ok(());
    }
    let ids: Vec<String> = marks.iter().map(|(id, _)| id.clone()).collect();
    let visible = visible_ids(db, &MUTATION_GRANT, scope, &ids).await?;
    match ids.iter().find(|id| !visible.contains(*id)) {
        Some(id) => Err(ServerError::NotFound(format!("page {id} not found"))),
        None => Ok(()),
    }
}

/// Both notes of a basis pair must be visible, or the pair reads as absent.
async fn require_pair_visible(
    db: &MemoryDB,
    grant: &TruthGrant,
    scope: &ReadScope,
    page_id: &str,
    basis_id: &str,
) -> Result<(), ServerError> {
    let visible = visible_ids(
        db,
        grant,
        scope,
        &[page_id.to_string(), basis_id.to_string()],
    )
    .await?;
    if !visible.contains(page_id) {
        return Err(ServerError::NotFound(format!("page {page_id} not found")));
    }
    if !visible.contains(basis_id) {
        return Err(ServerError::NotFound(format!("page {basis_id} not found")));
    }
    Ok(())
}

async fn open_db(state: &Arc<RwLock<ServerState>>) -> Result<Arc<MemoryDB>, ServerError> {
    state
        .read()
        .await
        .db
        .clone()
        .ok_or(ServerError::DbNotInitialized)
}

/// The pair's current basis row as the wire shape.
async fn pair_ref(
    db: &MemoryDB,
    grant: &TruthGrant,
    scope: &ReadScope,
    entry: PageBasisEntry,
) -> Result<BasisRef, ServerError> {
    let mut refs = basis_refs(db, grant, scope, vec![entry]).await?;
    refs.pop()
        .ok_or_else(|| ServerError::Internal("basis row vanished".to_string()))
}

/// PUT /api/pages/{id}/basis/{basis_id}
///
/// Mark `basis_id` as a basis of `id`, at the version the caller read. A
/// basis already at a newer version lands pending, not ok.
pub async fn handle_mark_basis(
    State(state): State<Arc<RwLock<ServerState>>>,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    headers: HeaderMap,
    Path((id, basis_id)): Path<(String, String)>,
    Json(req): Json<BasisVersionRequest>,
) -> Result<Json<BasisRef>, ServerError> {
    let db = open_db(&state).await?;
    let scope = crate::read_scope::effective_read_scope(&db, None, header_space.as_deref()).await?;
    require_pair_visible(&db, &MUTATION_GRANT, &scope, &id, &basis_id).await?;
    // Checked again inside the mark: a note moved out of a locked caller's
    // Space after the check above is not marked.
    let space_guard = (scope != wenlan_core::read_scope::ReadScope::Global).then(|| scope.clone());
    let entry = db
        .mark_page_basis_in(
            &id,
            &basis_id,
            req.version,
            &actor_from(&headers),
            space_guard.as_ref(),
        )
        .await
        .map_err(basis_error)?;
    Ok(Json(pair_ref(&db, &MUTATION_GRANT, &scope, entry).await?))
}

/// DELETE /api/pages/{id}/basis/{basis_id}
pub async fn handle_unmark_basis(
    State(state): State<Arc<RwLock<ServerState>>>,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    Path((id, basis_id)): Path<(String, String)>,
) -> Result<Json<BasisRemoveResponse>, ServerError> {
    let db = open_db(&state).await?;
    let scope = crate::read_scope::effective_read_scope(&db, None, header_space.as_deref()).await?;
    // Only the citing note has to be visible: removing a basis the caller can
    // no longer read must still work.
    let visible = visible_ids(&db, &MUTATION_GRANT, &scope, std::slice::from_ref(&id)).await?;
    if !visible.contains(&id) {
        return Err(ServerError::NotFound(format!("page {id} not found")));
    }
    let removed = db
        .unmark_page_basis(&id, &basis_id)
        .await
        .map_err(basis_error)?;
    Ok(Json(BasisRemoveResponse { removed }))
}

/// POST /api/pages/{id}/basis/{basis_id}/confirm
///
/// "I have accounted for the basis at `version`." The version check and the
/// write share one transaction: 409 when the basis is no longer at `version`
/// (the caller confirmed an older diff).
pub async fn handle_confirm_basis(
    State(state): State<Arc<RwLock<ServerState>>>,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    headers: HeaderMap,
    Path((id, basis_id)): Path<(String, String)>,
    Json(req): Json<BasisVersionRequest>,
) -> Result<Json<BasisRef>, ServerError> {
    let db = open_db(&state).await?;
    let scope = crate::read_scope::effective_read_scope(&db, None, header_space.as_deref()).await?;
    require_pair_visible(&db, &MUTATION_GRANT, &scope, &id, &basis_id).await?;
    let entry = db
        .confirm_page_basis(&id, &basis_id, req.version, &actor_from(&headers))
        .await
        .map_err(basis_error)?;
    Ok(Json(pair_ref(&db, &MUTATION_GRANT, &scope, entry).await?))
}

/// GET /api/pages/{id}/basis/{basis_id}/diff
///
/// What the citing note relied on next to what the basis says now, both as
/// text (the client renders the word diff), and the versions in between.
/// 409 with a `reason` when the comparison is no longer possible.
pub async fn handle_basis_diff(
    State(state): State<Arc<RwLock<ServerState>>>,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    view: crate::truth_guard::TruthView,
    Path((id, basis_id)): Path<(String, String)>,
) -> Result<Json<BasisDiffResponse>, ServerError> {
    let db = open_db(&state).await?;
    let scope = crate::read_scope::effective_read_scope(&db, None, header_space.as_deref()).await?;
    // The response carries the basis' prose, so the caller must be allowed to
    // read both notes in full.
    require_pair_visible(&db, &view.grant, &scope, &id, &basis_id).await?;
    // The read itself carries the scope: a note moved out of the caller's
    // Space after the check above reads as missing, never as its prose.
    match db
        .basis_diff_in_scope(&id, &basis_id, &scope)
        .await
        .map_err(basis_error)?
    {
        BasisDiffOutcome::Available(diff) => Ok(Json(BasisDiffResponse {
            from_version: diff.from_version,
            to_version: diff.to_version,
            from_content: diff.from_content,
            to_content: diff.to_content,
            edits: diff
                .edits
                .into_iter()
                .map(|edit| BasisDiffEdit {
                    version: edit.version,
                    edited_by: edit.edited_by,
                    at: edit.at,
                    minor: edit.minor,
                    delta_summary: edit.delta_summary,
                    agent: edit.agent,
                })
                .collect(),
        })),
        BasisDiffOutcome::OutOfScope(page_id) => {
            Err(ServerError::NotFound(format!("page {page_id} not found")))
        }
        BasisDiffOutcome::Unavailable(reason) => Err(ServerError::Structured {
            status: axum::http::StatusCode::CONFLICT,
            body: serde_json::json!({
                "error": format!("the comparison is not available: {reason}"),
                "reason": reason,
            }),
        }),
    }
}

#[derive(Debug, Deserialize)]
pub struct BasisPendingQuery {
    #[serde(default)]
    space: Option<String>,
}

/// GET /api/pages/basis-pending?space=
///
/// Every basis that is not ok with the notes waiting on it, grouped by basis
/// note ("A changed, 20 notes to confirm").
pub async fn handle_basis_pending(
    State(state): State<Arc<RwLock<ServerState>>>,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    view: crate::truth_guard::TruthView,
    Query(query): Query<BasisPendingQuery>,
) -> Result<Json<Vec<BasisPendingEntry>>, ServerError> {
    let db = open_db(&state).await?;
    let scope = crate::read_scope::effective_read_scope(
        &db,
        query.space.as_deref(),
        header_space.as_deref(),
    )
    .await?;
    let groups = db.list_basis_pending_in_scope(&scope).await?;
    // `?space=` only picks which citing notes are listed. What the caller may
    // see follows its Space lock alone, as on the other basis routes, so the
    // app filtering to one Space still names a basis kept in another (#927).
    let visibility =
        crate::read_scope::effective_read_scope(&db, None, header_space.as_deref()).await?;
    // A citing note the caller may not read in full is left out, and so is its
    // title; a basis group with no visible citer is left out whole.
    let citer_ids: Vec<String> = groups
        .iter()
        .flat_map(|g| g.citers.iter().map(|c| c.page_id.clone()))
        .collect();
    let visible_citers = visible_ids(&db, &view.grant, &visibility, &citer_ids).await?;
    let basis_ids: Vec<String> = groups.iter().map(|g| g.basis_page_id.clone()).collect();
    let visible_bases = visible_ids(&db, &view.grant, &visibility, &basis_ids).await?;
    let mut out = Vec::new();
    for group in groups {
        let citers: Vec<BasisPendingCiterRef> = group
            .citers
            .into_iter()
            .filter(|c| visible_citers.contains(&c.page_id))
            .map(|c| BasisPendingCiterRef {
                page_id: c.page_id,
                title: c.title,
                minor_only: c.minor_only,
            })
            .collect();
        if citers.is_empty() {
            continue;
        }
        let gone = group.state == wenlan_core::db::BASIS_STATE_DELETED;
        out.push(BasisPendingEntry {
            title: if gone || visible_bases.contains(&group.basis_page_id) {
                group.title
            } else {
                String::new()
            },
            basis_page_id: group.basis_page_id,
            changed_version: group.changed_version,
            state: group.state,
            citers,
        });
    }
    Ok(Json(out))
}
