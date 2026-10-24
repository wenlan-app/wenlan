// SPDX-License-Identifier: Apache-2.0
//! The person's own notes in an Obsidian vault: whether AI may change them,
//! and AI-proposed changes that are written only when the person accepts.

use crate::error::ServerError;
use crate::memory_routes::extract_agent_name;
use crate::route_registry::{get, post, TrackedRouter};
use crate::state::SharedState;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::response::Json;
use serde::Deserialize;
use std::path::PathBuf;
use wenlan_core::sources::Source;
use wenlan_core::vault_notes;
use wenlan_types::requests::{ProposeNoteChangeRequest, SetNoteAiChangesRequest};
use wenlan_types::responses::{
    AcceptNoteProposalResponse, NoteProposalView, NoteProposalsResponse,
};
use wenlan_types::sources::{NoteAiChanges, NoteProposal, SourceType};

pub(crate) fn register(router: TrackedRouter<SharedState>) -> TrackedRouter<SharedState> {
    router
        .route(
            "/api/sources/{id}/ai-changes",
            get(handle_get_note_ai_changes).put(handle_set_note_ai_changes),
        )
        .route(
            "/api/notes/proposals",
            get(handle_list_note_proposals).post(handle_propose_note_change),
        )
        .route(
            "/api/notes/proposals/{id}",
            get(handle_get_note_proposal).delete(handle_dismiss_note_proposal),
        )
        .route(
            "/api/notes/proposals/{id}/accept",
            post(handle_accept_note_proposal),
        )
}

/// Settings and accepting a change are the person's, never an AI tool's: a
/// request that names an agent is refused.
fn require_person(headers: &HeaderMap) -> Result<(), ServerError> {
    if extract_agent_name(headers, None) != "unknown" {
        return Err(ServerError::ValidationError("note_needs_the_person".into()));
    }
    Ok(())
}

fn vault(id: &str) -> Result<Source, ServerError> {
    let config = wenlan_core::config::load_config();
    let source = config
        .sources
        .into_iter()
        .find(|s| s.id == id)
        .ok_or_else(|| ServerError::NotFound(format!("Source not found: {id}")))?;
    if source.source_type != SourceType::Obsidian {
        return Err(ServerError::ValidationError("note_source_not_vault".into()));
    }
    Ok(source)
}

async fn pages_folder(state: &SharedState) -> PathBuf {
    let root = {
        let s = state.read().await;
        s.lint_config.page_root().map(std::path::Path::to_path_buf)
    };
    root.unwrap_or_else(|| wenlan_core::config::load_config().knowledge_path_or_default())
}

#[derive(Debug, Deserialize)]
pub struct AiChangesQuery {
    #[serde(default)]
    pub path: String,
}

/// GET /api/sources/{id}/ai-changes?path=dir/note.md
///
/// The setting that applies at `path` and where it comes from.
pub async fn handle_get_note_ai_changes(
    Path(id): Path<String>,
    Query(query): Query<AiChangesQuery>,
) -> Result<Json<NoteAiChanges>, ServerError> {
    let source = vault(&id)?;
    let path = vault_notes::normalize_key(&query.path)?;
    let config = wenlan_core::config::load_config();
    Ok(Json(vault_notes::effective(
        config.note_ai_changes.get(&source.id),
        &path,
    )))
}

/// PUT /api/sources/{id}/ai-changes
///
/// Sets or clears the setting at one path. Open proposals the new setting no
/// longer allows are dropped.
pub async fn handle_set_note_ai_changes(
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<SetNoteAiChangesRequest>,
) -> Result<Json<NoteAiChanges>, ServerError> {
    require_person(&headers)?;
    let source = vault(&id)?;
    let path = vault_notes::normalize_key(&req.path)?;
    let settings = tokio::task::spawn_blocking(move || {
        wenlan_core::config::update_config(|cfg| {
            let settings = cfg.note_ai_changes.entry(source.id.clone()).or_default();
            match req.value {
                Some(value) => {
                    settings.insert(path.clone(), value);
                }
                None => {
                    settings.remove(&path);
                }
            }
            if settings.is_empty() {
                cfg.note_ai_changes.remove(&source.id);
            }
            Ok(cfg.note_ai_changes.get(&source.id).cloned())
        })
        .map(|settings| (source, path, settings))
    })
    .await
    .map_err(|e| ServerError::Internal(e.to_string()))?;
    let (source, path, settings) = settings?;
    vault_notes::drop_disallowed(&source.id, settings.as_ref())?;
    Ok(Json(vault_notes::effective(settings.as_ref(), &path)))
}

#[derive(Debug, Deserialize)]
pub struct ProposalsQuery {
    pub source_id: Option<String>,
}

/// GET /api/notes/proposals?source_id=...
pub async fn handle_list_note_proposals(
    Query(query): Query<ProposalsQuery>,
) -> Json<NoteProposalsResponse> {
    Json(NoteProposalsResponse {
        proposals: vault_notes::list_proposals(query.source_id.as_deref()),
    })
}

/// A note's path from either its absolute path or the id of a memory read
/// from it (`<source>::<path>::<chunk>`).
fn note_path_from(file: &str) -> PathBuf {
    let file = file.trim();
    if let Some((_, rest)) = file.split_once("::") {
        if let Some((path, chunk)) = rest.rsplit_once("::") {
            if !chunk.is_empty() && chunk.bytes().all(|b| b.is_ascii_digit()) {
                return PathBuf::from(path);
            }
        }
    }
    PathBuf::from(file)
}

/// The vault that holds `file`; the innermost one when vaults nest.
fn vault_for_file(file: &std::path::Path) -> Result<(Source, String), ServerError> {
    let config = wenlan_core::config::load_config();
    config
        .sources
        .into_iter()
        .filter(|s| s.source_type == SourceType::Obsidian)
        .filter_map(|s| {
            let path = vault_notes::vault_relative(&s.path, file).ok()?;
            Some((s, path))
        })
        .max_by_key(|(s, _)| s.path.components().count())
        .ok_or_else(|| ServerError::ValidationError("note_outside_vault".into()))
}

/// POST /api/notes/proposals
///
/// An AI proposes a whole new text for one of the person's notes. Refused
/// unless the note is set to "Ask me first". Writes nothing to the note.
pub async fn handle_propose_note_change(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(req): Json<ProposeNoteChangeRequest>,
) -> Result<Json<NoteProposal>, ServerError> {
    let agent = extract_agent_name(&headers, None);
    let (source, path) = vault_for_file(&note_path_from(&req.file))?;
    let pages = pages_folder(&state).await;
    let settings = wenlan_core::config::load_config()
        .note_ai_changes
        .remove(&source.id);
    let proposal = tokio::task::spawn_blocking(move || {
        vault_notes::propose(
            &source,
            settings.as_ref(),
            &pages,
            &path,
            req.content,
            req.reason,
            &agent,
        )
    })
    .await
    .map_err(|e| ServerError::Internal(e.to_string()))??;
    Ok(Json(proposal))
}

/// GET /api/notes/proposals/{id}: the proposal beside the note as it is now.
pub async fn handle_get_note_proposal(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<Json<NoteProposalView>, ServerError> {
    let proposal = vault_notes::load_proposal(&id)?;
    let source = vault(&proposal.source_id)?;
    let pages = pages_folder(&state).await;
    let current = vault_notes::current_text(&source, &pages, &proposal.path)?;
    Ok(Json(NoteProposalView { proposal, current }))
}

/// DELETE /api/notes/proposals/{id}: decline. The note is not touched.
pub async fn handle_dismiss_note_proposal(
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ServerError> {
    if !vault_notes::remove_proposal(&id)? {
        return Err(ServerError::NotFound(format!("proposal {id}")));
    }
    Ok(Json(serde_json::json!({ "dismissed": true })))
}

/// POST /api/notes/proposals/{id}/accept
///
/// Writes the accepted text into the note, unless the note changed since the
/// proposal was made, then reads the vault again so Wenlan has the new text.
pub async fn handle_accept_note_proposal(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<AcceptNoteProposalResponse>, ServerError> {
    require_person(&headers)?;
    let proposal = vault_notes::load_proposal(&id)?;
    let source = vault(&proposal.source_id)?;
    let pages = pages_folder(&state).await;
    let synced = source.clone();
    let file = tokio::task::spawn_blocking(move || vault_notes::accept(&source, &pages, &id))
        .await
        .map_err(|e| ServerError::Internal(e.to_string()))??;

    let (db, quality_gate) = {
        let s = state.read().await;
        (s.db.clone(), s.quality_gate.clone())
    };
    if let Some(db) = db {
        tokio::spawn(async move {
            if let Err(e) = crate::source_routes::obsidian_sync::sync_obsidian_source(
                db,
                &quality_gate,
                &synced,
            )
            .await
            {
                tracing::warn!("[notes] sync after an accepted change failed: {e}");
            }
        });
    }
    Ok(Json(AcceptNoteProposalResponse {
        file: file.to_string_lossy().into_owned(),
    }))
}

#[cfg(test)]
#[path = "note_routes_tests.rs"]
mod tests;

/// Refuses an AI tool's change to a memory read from one of the person's
/// vault notes: forgetting it, replacing it, or editing it. The note is the
/// person's file, so the only way for an AI to change it is a proposal the
/// person accepts. Requests from the app (no agent name) pass.
pub(crate) fn refuse_ai_change_to_note(
    headers: &HeaderMap,
    memory_source_id: &str,
) -> Result<(), ServerError> {
    if extract_agent_name(headers, None) == "unknown" {
        return Ok(());
    }
    let from_vault = wenlan_core::config::load_config()
        .sources
        .iter()
        .filter(|s| s.source_type == SourceType::Obsidian)
        .any(|s| {
            memory_source_id
                .strip_prefix(s.id.as_str())
                .is_some_and(|rest| rest.starts_with("::"))
        });
    if from_vault {
        return Err(ServerError::ValidationError(
            "note_change_needs_proposal: this comes from the person's own note; \
             propose the change to the note instead, and they decide"
                .into(),
        ));
    }
    Ok(())
}
