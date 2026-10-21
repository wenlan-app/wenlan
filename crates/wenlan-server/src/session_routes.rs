// SPDX-License-Identifier: Apache-2.0
//! Agent sessions: record which conversation a write came from, and read back
//! what a session wrote. Writes name their session in the `x-wenlan-session`
//! header (`<tool>:<external_id>`).

use crate::route_registry::{get, post, TrackedRouter};
use crate::{error::ServerError, space_header::SpaceHeader, state::SharedState};
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::Json,
};
use std::sync::Arc;
use wenlan_core::db::sessions::{last_ai_title, SESSION_READS_LISTED};
use wenlan_core::db::MemoryDB;
use wenlan_types::{
    AgentSession, PageSessionsResponse, SessionDetailResponse, SessionReadKind, SessionRef,
    UpsertSessionRequest, SESSION_HEADER,
};

pub(crate) fn register(router: TrackedRouter<SharedState>) -> TrackedRouter<SharedState> {
    router
        .route("/api/sessions", post(handle_upsert_session))
        .route("/api/sessions/{id}", get(handle_get_session))
        .route("/api/pages/{id}/sessions", get(handle_get_page_sessions))
}

/// Resolve the `x-wenlan-session` header.
///
/// A missing header is an unattributed write. A malformed one is ignored with
/// a warning: attribution is best effort and must never fail the write it
/// rides on.
pub(crate) fn extract_session_ref(headers: &HeaderMap) -> Option<SessionRef> {
    let raw = headers.get(SESSION_HEADER)?;
    let parsed = raw
        .to_str()
        .map_err(|_| "header is not valid text".to_string())
        .and_then(SessionRef::parse);
    match parsed {
        Ok(session) => Some(session),
        Err(reason) => {
            tracing::warn!("[session] ignoring malformed {SESSION_HEADER} header: {reason}");
            None
        }
    }
}

/// Record that the session named in `headers` just read something, for the
/// session card's "what it looked up" list.
///
/// Best effort, like the write stamping and the agent presence signal: no
/// header or a malformed one records nothing, the write runs on a spawned task
/// off the response path, and an error is logged and never reaches the read.
/// `space` is the Space the read was scoped to (`session_read_space`,
/// `session_read_space_of`), `None` for the whole library. The handle
/// is returned only so tests can await the write; handlers drop it.
pub(crate) fn note_session_read(
    db: Arc<MemoryDB>,
    headers: &HeaderMap,
    kind: SessionReadKind,
    label: &str,
    space: Option<String>,
) -> Option<tokio::task::JoinHandle<()>> {
    let session = extract_session_ref(headers)?;
    let label = label.to_string();
    Some(tokio::spawn(async move {
        if let Err(error) = db
            .record_session_read(&session, kind, &label, space.as_deref())
            .await
        {
            tracing::warn!("[session] could not record a {kind:?} read: {error}");
        }
    }))
}

/// POST /api/sessions
///
/// Upsert a session and its metadata. Absent fields keep what is stored, and
/// `last_write_at` does not move: this records where a session ran, not a write.
pub async fn handle_upsert_session(
    State(state): State<SharedState>,
    Json(request): Json<UpsertSessionRequest>,
) -> Result<Json<AgentSession>, ServerError> {
    let db = {
        let state = state.read().await;
        state.db.clone().ok_or(ServerError::DbNotInitialized)?
    };
    Ok(Json(db.upsert_session_metadata(&request).await?))
}

/// GET /api/sessions/{id}
///
/// The session and what it wrote, newest first. A Space-scoped caller sees only
/// what it wrote in that Space, and 404 when none of it is visible.
pub async fn handle_get_session(
    State(state): State<SharedState>,
    SpaceHeader(header_space): SpaceHeader,
    view: crate::truth_guard::TruthView,
    Path(id): Path<String>,
) -> Result<Json<SessionDetailResponse>, ServerError> {
    let db = {
        let state = state.read().await;
        state.db.clone().ok_or(ServerError::DbNotInitialized)?
    };
    let scope = crate::read_scope::effective_read_scope(&db, None, header_space.as_deref()).await?;
    let mut detail = db
        .get_session_detail(&id, &scope)
        .await?
        .ok_or_else(|| ServerError::NotFound("session not found".to_string()))?;
    // Note titles are page-bearing: a note this caller may not see is not
    // listed, the same contract as the other note listings.
    detail.pages = wenlan_core::truth_adapter::filter_page_refs(
        &db,
        &view.grant,
        std::mem::take(&mut detail.pages),
        |page| page.id.as_str(),
    )
    .await?;
    // Note reads name a note: list only the ones this caller may see.
    let read_pages: Vec<String> = detail
        .reads
        .iter()
        .filter(|read| read.kind == SessionReadKind::ReadPage)
        .map(|read| read.label.clone())
        .collect();
    if !read_pages.is_empty() {
        let visible: std::collections::HashSet<String> =
            wenlan_core::truth_adapter::filter_page_refs(&db, &view.grant, read_pages, |id| {
                id.as_str()
            })
            .await?
            .into_iter()
            .collect();
        detail
            .reads
            .retain(|read| read.kind != SessionReadKind::ReadPage || visible.contains(&read.label));
    }
    detail.reads_total = detail.reads.len() as i64;
    detail.reads.truncate(SESSION_READS_LISTED);
    // The same contract for notes that cite the session's memories.
    detail.cited_by = wenlan_core::truth_adapter::filter_page_refs(
        &db,
        &view.grant,
        std::mem::take(&mut detail.cited_by),
        |note| note.id.as_str(),
    )
    .await?;
    detail.transcript_available = transcript_exists(detail.session.transcript_path.as_deref());
    fill_title_from_transcript(&mut detail.session).await;
    Ok(Json(detail))
}

/// Claude Code writes its own conversation title into the transcript and may
/// rename the conversation later. When a `claude-code` session has no title an
/// agent sent and its transcript is still on this machine, show the newest one.
/// Read fresh each time and never stored, so this read stays write-free and a
/// later rename shows up. Best effort; any failure leaves the title empty.
async fn fill_title_from_transcript(session: &mut AgentSession) {
    if session.tool != "claude-code"
        || session
            .title
            .as_deref()
            .is_some_and(|t| !t.trim().is_empty())
    {
        return;
    }
    let Some(path) = session.transcript_path.clone() else {
        return;
    };
    session.title = tokio::task::spawn_blocking(move || {
        let path = std::path::PathBuf::from(path);
        path.is_file().then(|| last_ai_title(&path)).flatten()
    })
    .await
    .ok()
    .flatten();
}

/// Whether a stored transcript location still names a file on this machine.
/// Tools prune old transcripts, so the stored path alone does not say.
fn transcript_exists(path: Option<&str>) -> bool {
    path.is_some_and(|path| std::path::Path::new(path).is_file())
}

/// GET /api/pages/{id}/sessions
///
/// Sessions that wrote a version of the note (`wrote`) and sessions of the
/// memories it cites (`source`).
pub async fn handle_get_page_sessions(
    State(state): State<SharedState>,
    SpaceHeader(header_space): SpaceHeader,
    view: crate::truth_guard::TruthView,
    Path(id): Path<String>,
) -> Result<Json<PageSessionsResponse>, ServerError> {
    let db = {
        let state = state.read().await;
        state.db.clone().ok_or(ServerError::DbNotInitialized)?
    };
    let scope = crate::read_scope::effective_read_scope(&db, None, header_space.as_deref()).await?;
    let page = db.get_page_scoped_browse(&id, &scope).await?;
    wenlan_core::truth_adapter::filter_page(&db, &view.grant, page)
        .await?
        .ok_or_else(|| ServerError::NotFound("page not found".to_string()))?;
    Ok(Json(db.list_page_sessions(&id, &scope).await?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(SESSION_HEADER, HeaderValue::from_str(value).unwrap());
        headers
    }

    #[test]
    fn transcript_exists_checks_the_file() {
        let dir = std::env::temp_dir().join(format!("wenlan-transcript-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("session.jsonl");
        std::fs::write(&file, "{}").unwrap();
        assert!(transcript_exists(file.to_str()));
        assert!(
            !transcript_exists(dir.to_str()),
            "a folder is not a transcript"
        );
        std::fs::remove_file(&file).unwrap();
        assert!(!transcript_exists(file.to_str()), "pruned file");
        assert!(!transcript_exists(None));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn header_resolves_to_a_session() {
        let session =
            extract_session_ref(&headers("claude-code:00893aaf-19fa-41d2-8238-13269b9b3ca0"))
                .expect("valid header");
        assert_eq!(session.tool, "claude-code");
        assert_eq!(session.external_id, "00893aaf-19fa-41d2-8238-13269b9b3ca0");
    }

    #[test]
    fn missing_header_is_unattributed() {
        assert!(extract_session_ref(&HeaderMap::new()).is_none());
    }

    #[test]
    fn malformed_header_is_ignored_not_fatal() {
        for bad in ["", "nocolon", "Tool:id", "codex:has space", "codex:a:b"] {
            assert!(extract_session_ref(&headers(bad)).is_none(), "{bad:?}");
        }
        let mut opaque = HeaderMap::new();
        opaque.insert(
            SESSION_HEADER,
            HeaderValue::from_bytes(&[0xff, 0xfe]).unwrap(),
        );
        assert!(extract_session_ref(&opaque).is_none());
    }
}
