// SPDX-License-Identifier: Apache-2.0
//! Routes for who wrote each sentence of a page, sentence proposals, the
//! person's 「不對」, and rechecking one sentence.
//!
//! The rules live in `wenlan_core::sentences`; this module frames
//! them as HTTP and decides who may call what. Reads and rechecks answer only
//! for a page the caller can see in its Space and under its truth grant; any
//! other page reads as absent. Accepting, rejecting and flagging are the
//! person's: an agent (a request with `x-agent-name`) is refused, as it is for
//! accepting a vault note proposal. Proposing is what an agent does instead
//! of rewriting the person's sentence.
//!
//! The HTTP contract is pinned in `tests/sentence_route_contracts.rs`.
//!
//! All of it is additive to the whole-page revision cards
//! (`/api/memory/revision/{id}/accept`), which keep working as before.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Json;
use tokio::sync::RwLock;

use wenlan_core::db::MemoryDB;
use wenlan_core::truth_contract::TruthGrant;
use wenlan_types::authorship::{
    PageSentencesResponse, SentenceAcceptResponse, SentenceDecisionRequest, SentenceFlagRequest,
    SentenceFlagResponse, SentenceProposalResponse, SentenceProposeRequest, SentenceRecheckRequest,
    SentenceRecheckResponse,
};

use crate::error::ServerError;
use crate::memory_routes::extract_agent_name;
use crate::route_registry::{get, post, TrackedRouter};
use crate::state::{ServerState, SharedState};

pub(crate) fn register(router: TrackedRouter<SharedState>) -> TrackedRouter<SharedState> {
    router
        .route("/api/pages/{id}/sentences", get(handle_get_page_sentences))
        .route(
            "/api/pages/{id}/sentences/propose",
            post(handle_propose_sentence),
        )
        .route(
            "/api/pages/{id}/sentences/proposals/{proposal_id}/accept",
            post(handle_accept_sentence_proposal),
        )
        .route(
            "/api/pages/{id}/sentences/proposals/{proposal_id}/reject",
            post(handle_reject_sentence_proposal),
        )
        .route("/api/pages/{id}/sentences/flag", post(handle_flag_sentence))
        .route(
            "/api/pages/{id}/sentences/recheck",
            post(handle_recheck_sentence),
        )
}

async fn open_db(state: &Arc<RwLock<ServerState>>) -> Result<Arc<MemoryDB>, ServerError> {
    state
        .read()
        .await
        .db
        .clone()
        .ok_or(ServerError::DbNotInitialized)
}

/// The page must be one the caller can see, in its Space and under `grant`,
/// or it reads as absent. Entity shadow pages have no sentences of their own.
async fn require_visible_page(
    db: &MemoryDB,
    grant: &TruthGrant,
    header_space: Option<&str>,
    id: &str,
) -> Result<(), ServerError> {
    let scope = crate::read_scope::effective_read_scope(db, None, header_space).await?;
    let visible =
        crate::page_basis_routes::visible_ids(db, grant, &scope, &[id.to_string()]).await?;
    let page = db.get_page(id).await?;
    match page {
        Some(page) if visible.contains(id) && page.kind != "entity" => Ok(()),
        _ => Err(ServerError::NotFound(format!("page {id} not found"))),
    }
}

/// Accept, reject and 「不對」 are the person's decisions.
fn require_person(headers: &HeaderMap) -> Result<(), ServerError> {
    if extract_agent_name(headers, None) != "unknown" {
        return Err(ServerError::ValidationError(
            "sentence_needs_the_person: only the person can decide on their sentences".into(),
        ));
    }
    Ok(())
}

/// GET /api/pages/{id}/sentences
///
/// Every sentence of the page with who wrote it (`person`, `ai`, `unknown`,
/// `original`), whether a machine write may change it, its citations with the
/// version each one cited, how weak it is, the person's flags, and the
/// pending proposals. `ai_edits` is the page's AI-change mode.
pub async fn handle_get_page_sentences(
    State(state): State<Arc<RwLock<ServerState>>>,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    view: crate::truth_guard::TruthView,
    Path(id): Path<String>,
) -> Result<Json<PageSentencesResponse>, ServerError> {
    let db = open_db(&state).await?;
    require_visible_page(&db, &view.grant, header_space.as_deref(), &id).await?;
    Ok(Json(
        wenlan_core::sentences::page_sentences(&db, &id).await?,
    ))
}

/// POST /api/pages/{id}/sentences/propose
///
/// An AI's proposal for one sentence, by its hash. Nothing on the page
/// changes until the person accepts it.
pub async fn handle_propose_sentence(
    State(state): State<Arc<RwLock<ServerState>>>,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<SentenceProposeRequest>,
) -> Result<Json<SentenceProposalResponse>, ServerError> {
    let db = open_db(&state).await?;
    require_visible_page(
        &db,
        &crate::page_basis_routes::MUTATION_GRANT,
        header_space.as_deref(),
        &id,
    )
    .await?;
    let agent = extract_agent_name(&headers, None);
    let proposal = wenlan_core::sentences::propose_sentence(&db, &id, &req, &agent).await?;
    Ok(Json(SentenceProposalResponse { proposal }))
}

/// POST /api/pages/{id}/sentences/proposals/{proposal_id}/accept
///
/// The person takes one proposal: its text lands as their edit, in one new
/// page version, and the sentence counts as theirs from then on.
pub async fn handle_accept_sentence_proposal(
    State(state): State<Arc<RwLock<ServerState>>>,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    headers: HeaderMap,
    Path((id, proposal_id)): Path<(String, String)>,
    body: Option<Json<SentenceDecisionRequest>>,
) -> Result<Json<SentenceAcceptResponse>, ServerError> {
    require_person(&headers)?;
    let (db, page_root) = {
        let s = state.read().await;
        (
            s.db.clone().ok_or(ServerError::DbNotInitialized)?,
            s.lint_config.page_root().map(std::path::Path::to_path_buf),
        )
    };
    require_visible_page(
        &db,
        &crate::page_basis_routes::MUTATION_GRANT,
        header_space.as_deref(),
        &id,
    )
    .await?;
    let why = body.and_then(|Json(b)| b.why);
    Ok(Json(
        wenlan_core::sentences::accept_sentence_proposal(
            &db,
            &id,
            &proposal_id,
            why.as_deref(),
            page_root.as_deref(),
        )
        .await?,
    ))
}

/// POST /api/pages/{id}/sentences/proposals/{proposal_id}/reject
pub async fn handle_reject_sentence_proposal(
    State(state): State<Arc<RwLock<ServerState>>>,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    headers: HeaderMap,
    Path((id, proposal_id)): Path<(String, String)>,
    body: Option<Json<SentenceDecisionRequest>>,
) -> Result<Json<SentenceProposalResponse>, ServerError> {
    require_person(&headers)?;
    let db = open_db(&state).await?;
    require_visible_page(
        &db,
        &crate::page_basis_routes::MUTATION_GRANT,
        header_space.as_deref(),
        &id,
    )
    .await?;
    let why = body.and_then(|Json(b)| b.why);
    let proposal =
        wenlan_core::sentences::reject_sentence_proposal(&db, &id, &proposal_id, why.as_deref())
            .await?;
    Ok(Json(SentenceProposalResponse { proposal }))
}

/// POST /api/pages/{id}/sentences/flag
///
/// The person's 「不對」 on one sentence. The fix, by the person or by the AI
/// (as a proposal), comes after.
pub async fn handle_flag_sentence(
    State(state): State<Arc<RwLock<ServerState>>>,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<SentenceFlagRequest>,
) -> Result<Json<SentenceFlagResponse>, ServerError> {
    require_person(&headers)?;
    let db = open_db(&state).await?;
    require_visible_page(
        &db,
        &crate::page_basis_routes::MUTATION_GRANT,
        header_space.as_deref(),
        &id,
    )
    .await?;
    Ok(Json(
        wenlan_core::sentences::flag_sentence(&db, &id, &req.sentence_hash, req.why.as_deref())
            .await?,
    ))
}

/// POST /api/pages/{id}/sentences/recheck
///
/// Recheck only this sentence's citations against their sources' current
/// version. Nothing else on the page is rechecked or rewritten.
pub async fn handle_recheck_sentence(
    State(state): State<Arc<RwLock<ServerState>>>,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    view: crate::truth_guard::TruthView,
    Path(id): Path<String>,
    Json(req): Json<SentenceRecheckRequest>,
) -> Result<Json<SentenceRecheckResponse>, ServerError> {
    let db = open_db(&state).await?;
    require_visible_page(&db, &view.grant, header_space.as_deref(), &id).await?;
    Ok(Json(
        wenlan_core::sentences::recheck_sentence(&db, &id, &req.sentence_hash).await?,
    ))
}
