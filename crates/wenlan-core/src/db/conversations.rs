// SPDX-License-Identifier: Apache-2.0
//! Which conversations search may read the words of (UX-FIND-1).

use std::collections::HashSet;

use super::sessions::SESSION_COLUMNS;
use super::{MemoryDB, UNFILED_SPACE_ID};
use crate::read_scope::ReadScope;
use crate::WenlanError;
use wenlan_types::{AgentSession, ThingRef};

/// A conversation whose transcript is named, and the thing it is in.
#[derive(Debug, Clone)]
pub struct SearchableConversation {
    pub session: AgentSession,
    pub thing: Option<ThingRef>,
}

/// The conversations a scoped caller may read the words of: ones that wrote
/// a memory, a page or a handoff in its Space. The same rule as
/// `get_session_detail`, as one SQL predicate so a list is one query.
/// `parameter` is the Space value's placeholder.
fn ran_in_scope(scope: &ReadScope, parameter: usize) -> (String, Option<libsql::Value>) {
    let (column_is, value) = match scope {
        ReadScope::Global => return (String::new(), None),
        ReadScope::Space(space) => (
            format!("= ?{parameter}"),
            Some(libsql::Value::Text(space.clone())),
        ),
        ReadScope::Uncategorized => (String::new(), None),
    };
    let matches = |column: &str| {
        if column_is.is_empty() {
            format!("({column} IS NULL OR {column} = '{UNFILED_SPACE_ID}')")
        } else {
            format!("{column} {column_is}")
        }
    };
    (
        format!(
            " AND (EXISTS (SELECT 1 FROM memories m
                            WHERE m.session_id = agent_sessions.id AND m.source = 'memory'
                              AND m.pending_revision = 0 AND {memory})
                OR EXISTS (SELECT 1 FROM page_history h JOIN pages p ON p.id = h.page_id
                            WHERE h.session_id = agent_sessions.id AND {page})
                OR EXISTS (SELECT 1 FROM brief_history b JOIN spaces s ON s.id = b.space_id
                            WHERE b.session_id = agent_sessions.id AND {space}))",
            memory = matches("m.space"),
            page = matches("p.workspace"),
            space = matches("s.name"),
        ),
        value,
    )
}

impl MemoryDB {
    /// Conversations that name a transcript, newest first, at most `limit`,
    /// each with the thing it was last put in. A Space-scoped caller gets
    /// only conversations that ran in its Space (see `ran_in_scope`), and the
    /// thing only when it may see that thing.
    pub async fn searchable_conversations(
        &self,
        scope: &ReadScope,
        limit: usize,
    ) -> Result<Vec<SearchableConversation>, WenlanError> {
        self.conversations_where("", None, scope, limit).await
    }

    /// One conversation as `searchable_conversations` lists it, or `None`
    /// when it is unknown, names no transcript, or is out of `scope`.
    pub async fn searchable_conversation(
        &self,
        id: &str,
        scope: &ReadScope,
    ) -> Result<Option<SearchableConversation>, WenlanError> {
        Ok(self
            .conversations_where(" AND id = ?3", Some(id), scope, 1)
            .await?
            .pop())
    }

    async fn conversations_where(
        &self,
        only_sql: &str,
        only: Option<&str>,
        scope: &ReadScope,
        limit: usize,
    ) -> Result<Vec<SearchableConversation>, WenlanError> {
        let (scope_sql, scope_value) = ran_in_scope(scope, 2);
        let mut values = vec![libsql::Value::Integer(limit as i64)];
        if let Some(id) = only {
            // `?2` is bound even when the scope does not use it, so the id
            // keeps its number.
            values.push(scope_value.unwrap_or(libsql::Value::Null));
            values.push(libsql::Value::Text(id.to_string()));
        } else {
            values.extend(scope_value);
        }
        let mut found = {
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
                                {scope_sql}{only_sql}
                          ORDER BY last_write_at DESC, id
                          LIMIT ?1"
                    ),
                    libsql::params_from_iter(values),
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
        if !matches!(scope, ReadScope::Global) && found.iter().any(|c| c.thing.is_some()) {
            let visible: HashSet<String> = self
                .list_things(scope, None)
                .await?
                .into_iter()
                .map(|t| t.id)
                .collect();
            for conversation in &mut found {
                if conversation
                    .thing
                    .as_ref()
                    .is_some_and(|t| !visible.contains(&t.id))
                {
                    conversation.thing = None;
                }
            }
        }
        Ok(found)
    }
}
