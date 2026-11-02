// SPDX-License-Identifier: Apache-2.0
//! The words of conversations, for search (UX-FIND-1, UX-FIND-2).
//!
//! Search finds the sentence someone said, in the transcript the AI tool
//! keeps, and shows a little of what came before and after; "show all" reads
//! the whole conversation. Only the person reads these: a conversation's
//! words are not handed to another AI tool through the daemon.

use std::time::{Duration, Instant};

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::Json;
use serde::Deserialize;

use wenlan_core::db::conversations::SearchableConversation;
use wenlan_core::transcript_lines::{
    conversation_lines, excerpt, head, line_matches, search_terms, tail,
};
use wenlan_types::conversations::{
    ConversationHit, ConversationLinesResponse, ConversationSearchResponse,
};

use crate::error::ServerError;
use crate::memory_routes::extract_agent_name;
use crate::route_registry::{get, TrackedRouter};
use crate::space_header::SpaceHeader;
use crate::state::SharedState;

/// Conversations search looks through, newest first.
const SEARCHED_CONVERSATIONS: usize = 300;
/// Sentences one conversation contributes, so one long chat does not fill
/// the list.
const HITS_PER_CONVERSATION: usize = 3;
const DEFAULT_HITS: usize = 12;
const MAX_HITS: usize = 50;
/// Search stops looking once it has spent this long reading transcripts.
const SEARCH_BUDGET: Duration = Duration::from_secs(2);
const HIT_CHARS: usize = 240;
const CONTEXT_CHARS: usize = 160;
/// Lines "show all" returns, the newest kept.
const MAX_LINES: usize = 5_000;

pub(crate) fn register(router: TrackedRouter<SharedState>) -> TrackedRouter<SharedState> {
    router
        .route(
            "/api/conversations/search",
            get(handle_search_conversations),
        )
        .route("/api/sessions/{id}/lines", get(handle_conversation_lines))
}

fn require_person(headers: &HeaderMap) -> Result<(), ServerError> {
    if extract_agent_name(headers, None) != "unknown" {
        return Err(ServerError::ValidationError(
            "conversation_words_are_for_the_person: only the app reads a conversation's words"
                .into(),
        ));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct ConversationSearchQuery {
    #[serde(default)]
    q: String,
    #[serde(default)]
    limit: Option<usize>,
}

/// GET /api/conversations/search?q=&limit=
///
/// Sentences with every search word, newest conversation first, each with the
/// line before and after it.
pub async fn handle_search_conversations(
    State(state): State<SharedState>,
    SpaceHeader(header_space): SpaceHeader,
    headers: HeaderMap,
    Query(query): Query<ConversationSearchQuery>,
) -> Result<Json<ConversationSearchResponse>, ServerError> {
    require_person(&headers)?;
    let terms = search_terms(&query.q);
    if terms.is_empty() {
        return Ok(Json(ConversationSearchResponse::default()));
    }
    let limit = query.limit.unwrap_or(DEFAULT_HITS).clamp(1, MAX_HITS);
    let db = {
        let state = state.read().await;
        state.db.clone().ok_or(ServerError::DbNotInitialized)?
    };
    let scope = crate::read_scope::effective_read_scope(&db, None, header_space.as_deref()).await?;
    let conversations = db
        .searchable_conversations(&scope, SEARCHED_CONVERSATIONS)
        .await?;
    let hits = tokio::task::spawn_blocking(move || search(conversations, &terms, limit))
        .await
        .map_err(|e| ServerError::Internal(format!("conversation search: {e}")))?;
    Ok(Json(ConversationSearchResponse { hits }))
}

fn search(
    conversations: Vec<SearchableConversation>,
    terms: &[String],
    limit: usize,
) -> Vec<ConversationHit> {
    let started = Instant::now();
    let mut hits = Vec::new();
    for conversation in conversations {
        if hits.len() >= limit || started.elapsed() > SEARCH_BUDGET {
            break;
        }
        let session = &conversation.session;
        let Some(path) = session.transcript_path.as_deref() else {
            continue;
        };
        let Some(lines) = conversation_lines(&session.tool, std::path::Path::new(path)) else {
            continue;
        };
        let found: Vec<usize> = (0..lines.len())
            .rev()
            .filter(|&i| line_matches(&lines[i].text, terms))
            .take(HITS_PER_CONVERSATION)
            .collect();
        if found.is_empty() {
            continue;
        }
        let title = conversation_title(session);
        for index in found {
            if hits.len() >= limit {
                break;
            }
            let mut line = lines[index].clone();
            line.text = excerpt(&line.text, terms, HIT_CHARS);
            let before = index.checked_sub(1).map(|i| {
                let mut l = lines[i].clone();
                l.text = tail(&l.text, CONTEXT_CHARS);
                l
            });
            let after = lines.get(index + 1).map(|l| {
                let mut l = l.clone();
                l.text = head(&l.text, CONTEXT_CHARS);
                l
            });
            hits.push(ConversationHit {
                session_id: session.id.clone(),
                tool: session.tool.clone(),
                title: title.clone(),
                thing_id: conversation.thing.as_ref().map(|t| t.id.clone()),
                thing_title: conversation.thing.as_ref().and_then(|t| t.title.clone()),
                index,
                line,
                before,
                after,
            });
        }
    }
    hits
}

/// The stored title, else the one Claude Code wrote into its transcript.
fn conversation_title(session: &wenlan_types::AgentSession) -> Option<String> {
    if let Some(title) = session
        .title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
    {
        return Some(title.to_string());
    }
    if session.tool != "claude-code" {
        return None;
    }
    let path = std::path::Path::new(session.transcript_path.as_deref()?);
    wenlan_core::db::sessions::last_ai_title(path)
}

/// GET /api/sessions/{id}/lines
///
/// The whole conversation, oldest line first. 404 when the conversation is
/// unknown, out of the caller's Space, or its transcript is gone or in a
/// format Wenlan does not read.
pub async fn handle_conversation_lines(
    State(state): State<SharedState>,
    SpaceHeader(header_space): SpaceHeader,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<ConversationLinesResponse>, ServerError> {
    require_person(&headers)?;
    let db = {
        let state = state.read().await;
        state.db.clone().ok_or(ServerError::DbNotInitialized)?
    };
    let scope = crate::read_scope::effective_read_scope(&db, None, header_space.as_deref()).await?;
    let not_found = || ServerError::NotFound("conversation not found".to_string());
    let conversation = db
        .searchable_conversation(&id, &scope)
        .await?
        .ok_or_else(not_found)?;
    let SearchableConversation { session, thing } = conversation;
    let session_id = session.id.clone();
    let tool = session.tool.clone();
    let (title, lines) = tokio::task::spawn_blocking(move || {
        let path = std::path::PathBuf::from(session.transcript_path.clone().unwrap_or_default());
        let lines = conversation_lines(&session.tool, &path);
        (conversation_title(&session), lines.map(|l| (*l).clone()))
    })
    .await
    .map_err(|e| ServerError::Internal(format!("conversation lines: {e}")))?;
    let mut lines = lines.ok_or_else(not_found)?;
    if lines.len() > MAX_LINES {
        lines.drain(..lines.len() - MAX_LINES);
    }
    Ok(Json(ConversationLinesResponse {
        session_id,
        tool,
        title,
        thing_id: thing.as_ref().map(|t| t.id.clone()),
        thing_title: thing.and_then(|t| t.title),
        lines,
    }))
}
