// SPDX-License-Identifier: Apache-2.0
//! Agent presence recorded from the read path.
//!
//! A tool that only searches never reaches `check_agent_for_write`, so the
//! Connections list could not tell "added but never used" from "working".
//! A read that names an agent now records that the agent was seen.
//!
//! A read must not grant write trust. A name the daemon has never written for
//! resolves to `"unknown"` (search only) on reads, and registration on the
//! write path defaults to `"full"`. So a row created here starts at
//! `"unknown"` with `memory_count = 0` and `updated_at = created_at`, and
//! never calls `register_agent` or `touch_agent`. The first write promotes it
//! in one conditional UPDATE (`promote_untouched_read_agent`) that only
//! applies while that marker still holds.
//!
//! An explicit connection is the other way in: `register_trusted_agent` is the
//! user's own gesture (connecting a tool from the app), so it registers the
//! tool at `"full"` and lifts a read-created row the same way the first write
//! would, before the tool has written anything.

use super::{canonicalize_agent_id, new_agent_display_name, MemoryDB};
use crate::error::WenlanError;

/// Longest agent label a read may turn into a row. Longer values are not a
/// tool name, they are noise or abuse of an unauthenticated header.
const MAX_PRESENCE_NAME_CHARS: usize = 64;

/// Most never-written, never-edited rows the read path will hold at once.
/// Reads create nothing today, so unbounded header values must not become
/// unbounded rows. The user can delete a row in Connections to free a slot.
const MAX_UNTOUCHED_READ_AGENTS: i64 = 64;

/// What a read-path presence call did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentPresence {
    /// The name is empty, reserved or malformed. Nothing was written.
    Ignored,
    /// The agent already had a row; only `last_seen_at` moved.
    Seen,
    /// A new `"unknown"` row was created.
    Created,
    /// The name is valid but the cap on untouched read-created rows is full.
    AtCapacity,
}

/// The canonical agent id a read may record, or `None` for a name that must
/// create nothing.
///
/// Same normalisation as the write path (`canonicalize_agent_id`, with the
/// `"unknown"` sentinel meaning "no agent identified"), plus the bounds a row
/// created by a bare read needs: a length limit, no control characters, and
/// not the reserved setup-probe identity.
pub fn presence_agent_id(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.chars().count() > MAX_PRESENCE_NAME_CHARS
        || trimmed.chars().any(char::is_control)
    {
        return None;
    }
    let canonical = canonicalize_agent_id(trimmed);
    if canonical.is_empty()
        || canonical == "unknown"
        || crate::onboarding::is_setup_probe_agent(&canonical)
    {
        return None;
    }
    Some(canonical)
}

impl MemoryDB {
    /// Record that `raw_agent` was seen on a read. Best-effort by contract:
    /// callers log an `Err` and carry on, a read never fails because of it.
    ///
    /// An existing row only gets a fresh `last_seen_at`; trust, `memory_count`,
    /// `updated_at` and `enabled` are untouched. A missing row is created at
    /// trust `"unknown"`.
    pub async fn record_agent_presence(
        &self,
        raw_agent: &str,
    ) -> Result<AgentPresence, WenlanError> {
        let Some(canonical) = presence_agent_id(raw_agent) else {
            return Ok(AgentPresence::Ignored);
        };
        let now = chrono::Utc::now().timestamp();
        // One lock hold for both statements: the UPDATE and the INSERT cannot
        // interleave with another registration of the same name.
        let conn = self.conn.lock().await;
        let seen = conn
            .execute(
                "UPDATE agent_connections SET last_seen_at = ?1 WHERE name = ?2",
                libsql::params![now, canonical.clone()],
            )
            .await
            .map_err(|e| WenlanError::VectorDb(format!("record_agent_presence update: {}", e)))?;
        if seen > 0 {
            return Ok(AgentPresence::Seen);
        }
        let display_name = new_agent_display_name(raw_agent, &canonical);
        let created = conn
            .execute(
                "INSERT INTO agent_connections (id, name, display_name, agent_type, description, enabled, trust_level, last_seen_at, memory_count, created_at, updated_at)
                 SELECT ?1, ?2, ?3, 'api', NULL, 1, 'unknown', ?4, 0, ?4, ?4
                 WHERE (SELECT COUNT(*) FROM agent_connections
                        WHERE trust_level = 'unknown' AND memory_count = 0 AND updated_at = created_at) < ?5
                 ON CONFLICT(name) DO NOTHING",
                libsql::params![
                    uuid::Uuid::new_v4().to_string(),
                    canonical,
                    display_name,
                    now,
                    MAX_UNTOUCHED_READ_AGENTS
                ],
            )
            .await
            .map_err(|e| WenlanError::VectorDb(format!("record_agent_presence insert: {}", e)))?;
        Ok(if created > 0 {
            AgentPresence::Created
        } else {
            AgentPresence::AtCapacity
        })
    }

    /// Register `raw_name` as a tool the user has authorised, so its first
    /// read already sees what a trusted agent sees.
    ///
    /// Idempotent. A missing row is created at `"full"` (`register_agent`). A
    /// row the read path created (`"unknown"`, never written, never edited) is
    /// promoted to `"full"` by the same conditional UPDATE the first write
    /// uses. Every other row is returned exactly as it is: a user's edit
    /// (`updated_at != created_at`), a tool that has written, a trust level the
    /// user chose, and a disabled row all stay untouched. `last_seen_at` is not
    /// set, so "connected" still means "has been used".
    ///
    /// Names are held to the read path's rules (`presence_agent_id`); an
    /// empty, over-long, control-character, reserved or `"unknown"` name is a
    /// `WenlanError::Validation` and creates nothing.
    pub async fn register_trusted_agent(
        &self,
        raw_name: &str,
    ) -> Result<super::AgentConnection, WenlanError> {
        let Some(canonical) = presence_agent_id(raw_name) else {
            return Err(WenlanError::Validation(format!(
                "invalid agent name: {:?}",
                raw_name
            )));
        };
        let mut agent = self.register_agent(raw_name.trim()).await?;
        if agent.enabled && agent.trust_level == "unknown" {
            agent.trust_level = self.promote_untouched_read_agent(&canonical).await?;
        }
        Ok(agent)
    }

    /// Record that `raw_agent` wrote a page (write_page, a steep rewrite or a
    /// new steep page). Same first-contact rules as a memory write
    /// (`check_agent_for_write`), without counting a memory.
    ///
    /// A missing row is registered at `"full"`, a row the read path created is
    /// promoted the way the first memory write promotes it, and
    /// `last_seen_at` moves so the tool reads as used. A disabled tool is
    /// refused with `WenlanError::AgentDisabled`, as a memory write is.
    /// Returns the trust level after the call, or `None` for a name a read
    /// may not record (`presence_agent_id`), which is left alone.
    ///
    /// Without this an AI that had only written pages stayed at the read
    /// path's `"unknown"`, and the page gate then hid the pages it wrote from
    /// its own recall.
    pub async fn note_page_writer(&self, raw_agent: &str) -> Result<Option<String>, WenlanError> {
        let Some(canonical) = presence_agent_id(raw_agent) else {
            return Ok(None);
        };
        let agent = self.register_agent(raw_agent.trim()).await?;
        if !agent.enabled {
            return Err(WenlanError::AgentDisabled(format!(
                "Agent '{}' is disabled",
                raw_agent.trim()
            )));
        }
        let trust_level = if agent.trust_level == "unknown" {
            self.promote_untouched_read_agent(&canonical).await?
        } else {
            agent.trust_level
        };
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().await;
        conn.execute(
            "UPDATE agent_connections SET last_seen_at = ?1 WHERE name = ?2",
            libsql::params![now, canonical],
        )
        .await
        .map_err(|e| WenlanError::VectorDb(format!("note_page_writer: {}", e)))?;
        Ok(Some(trust_level))
    }

    /// First-write promotion of a row created by `record_agent_presence`.
    /// Returns the agent's trust level after the attempt.
    ///
    /// ONE conditional UPDATE decides: the row must still be `"unknown"`,
    /// never written (`memory_count = 0`) and never edited
    /// (`updated_at = created_at`; `update_agent` always moves `updated_at`
    /// past `created_at`). Anything else, including a user who set the trust
    /// to `"unknown"` themselves, is left exactly as it is. When the UPDATE
    /// applies to no row the current trust is read back in the same lock hold,
    /// so a concurrent first write that won the race is reported as `"full"`.
    pub(super) async fn promote_untouched_read_agent(
        &self,
        agent_name: &str,
    ) -> Result<String, WenlanError> {
        let canonical = canonicalize_agent_id(agent_name);
        let conn = self.conn.lock().await;
        let promoted = conn
            .execute(
                "UPDATE agent_connections SET trust_level = 'full'
                 WHERE name = ?1 AND trust_level = 'unknown' AND memory_count = 0 AND updated_at = created_at",
                libsql::params![canonical.clone()],
            )
            .await
            .map_err(|e| WenlanError::VectorDb(format!("promote_untouched_read_agent: {}", e)))?;
        if promoted > 0 {
            return Ok("full".to_string());
        }
        let mut rows = conn
            .query(
                "SELECT trust_level FROM agent_connections WHERE name = ?1",
                libsql::params![canonical],
            )
            .await
            .map_err(|e| {
                WenlanError::VectorDb(format!("promote_untouched_read_agent read: {}", e))
            })?;
        match rows
            .next()
            .await
            .map_err(|e| WenlanError::VectorDb(e.to_string()))?
        {
            Some(row) => Ok(row
                .get::<String>(0)
                .unwrap_or_else(|_| "unknown".to_string())),
            None => Ok("unknown".to_string()),
        }
    }
}
