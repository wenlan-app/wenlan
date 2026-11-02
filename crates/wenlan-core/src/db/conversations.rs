// SPDX-License-Identifier: Apache-2.0
//! Which conversations search may read the words of (UX-FIND-1).

use super::sessions::SESSION_COLUMNS;
use super::MemoryDB;
use crate::read_scope::ReadScope;
use crate::WenlanError;
use wenlan_types::{AgentSession, ThingRef};

/// A conversation whose transcript is named, and the thing it is in.
#[derive(Debug, Clone)]
pub struct SearchableConversation {
    pub session: AgentSession,
    pub thing: Option<ThingRef>,
}

impl MemoryDB {
    /// Conversations that name a transcript, newest first, at most `limit`,
    /// each with the thing it was last put in. A Space-scoped caller gets
    /// only conversations it can see a write of (`get_session_detail`), so it
    /// never reads the words of a conversation that ran elsewhere.
    pub async fn searchable_conversations(
        &self,
        scope: &ReadScope,
        limit: usize,
    ) -> Result<Vec<SearchableConversation>, WenlanError> {
        let found = {
            let conn = self.conn.lock().await;
            let mut rows = conn
                .query(
                    &format!(
                        "SELECT {SESSION_COLUMNS},
                                (SELECT t.id FROM thing_sessions ts JOIN things t ON t.id = ts.thing_id
                                  WHERE ts.session_id = agent_sessions.id AND t.merged_into IS NULL
                                  ORDER BY ts.at DESC, t.id LIMIT 1),
                                (SELECT t.title FROM thing_sessions ts JOIN things t ON t.id = ts.thing_id
                                  WHERE ts.session_id = agent_sessions.id AND t.merged_into IS NULL
                                  ORDER BY ts.at DESC, t.id LIMIT 1)
                           FROM agent_sessions
                          WHERE transcript_path IS NOT NULL AND trim(transcript_path) <> ''
                          ORDER BY last_write_at DESC, id
                          LIMIT ?1"
                    ),
                    libsql::params![limit as i64],
                )
                .await
                .map_err(|e| WenlanError::VectorDb(format!("searchable conversations: {e}")))?;
            let mut found = Vec::new();
            while let Some(row) = rows
                .next()
                .await
                .map_err(|e| WenlanError::VectorDb(format!("searchable conversations row: {e}")))?
            {
                let session = super::sessions::session_from_row(&row);
                let thing = row
                    .get::<Option<String>>(13)
                    .unwrap_or(None)
                    .map(|id| ThingRef {
                        id,
                        title: row.get::<Option<String>>(14).unwrap_or(None),
                    });
                found.push(SearchableConversation { session, thing });
            }
            found
        };
        if matches!(scope, ReadScope::Global) {
            return Ok(found);
        }
        let mut visible = Vec::new();
        for conversation in found {
            if self
                .get_session_detail(&conversation.session.id, scope)
                .await?
                .is_some()
            {
                visible.push(conversation);
            }
        }
        Ok(visible)
    }

    /// One conversation as `searchable_conversations` lists it, or `None`
    /// when it is unknown, names no transcript, or is out of `scope`.
    pub async fn searchable_conversation(
        &self,
        id: &str,
        scope: &ReadScope,
    ) -> Result<Option<SearchableConversation>, WenlanError> {
        let Some(session) = self.get_session(id).await? else {
            return Ok(None);
        };
        if session
            .transcript_path
            .as_deref()
            .is_none_or(|p| p.trim().is_empty())
        {
            return Ok(None);
        }
        if !matches!(scope, ReadScope::Global)
            && self.get_session_detail(id, scope).await?.is_none()
        {
            return Ok(None);
        }
        let thing = self.session_things(id).await?.pop();
        Ok(Some(SearchableConversation { session, thing }))
    }
}
