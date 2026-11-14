// SPDX-License-Identifier: Apache-2.0
//! Durable ledger for library-grown skills. Like the AI-memory follower, this
//! feature owns rows in `source_sync_state` under one source id. The source
//! page remains the canonical content; ledger rows only remember generator
//! ownership, deletions, installed copies, and review state.

use super::MemoryDB;
use crate::error::WenlanError;
use crate::skill_grow::GeneratedSkillLine;
use wenlan_types::{SkillBorrowRecord, SkillSuggestion};

const LEDGER: &str = "skills";
const SPACE_PREFIX: &str = "space:";
const INSTALL_PREFIX: &str = "install:";
const SUGGESTION_PREFIX: &str = "suggestion:";
pub const NIGHTLY_KEY: &str = "nightly";

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SkillPageState {
    /// Kept explicit so a future team Space can reuse the format without
    /// treating a personal Space as a team-owned skill.
    pub scope_kind: String,
    pub space_id: String,
    pub space_name: String,
    pub slug: String,
    pub page_id: String,
    #[serde(default)]
    pub generated: Vec<GeneratedSkillLine>,
    #[serde(default)]
    pub deleted: Vec<GeneratedSkillLine>,
    #[serde(default)]
    pub last_grown_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SkillInstallState {
    pub tool: String,
    pub name: String,
    pub dir: String,
    pub content_hash: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuggestionStatus {
    Pending,
    Applied,
    Dismissed,
    Undone,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredSkillSuggestion {
    pub suggestion: SkillSuggestion,
    pub skill_file: String,
    pub status: SuggestionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup_file: Option<String>,
}

fn db_err(context: &str) -> impl Fn(libsql::Error) -> WenlanError + '_ {
    move |error| WenlanError::VectorDb(format!("{context}: {error}"))
}

impl MemoryDB {
    async fn skill_ledger_rows<T: serde::de::DeserializeOwned>(
        &self,
        prefix: &str,
    ) -> Result<Vec<(String, T)>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT file_path, content_hash FROM source_sync_state \
                 WHERE source_id = ?1 AND file_path LIKE ?2 ORDER BY file_path",
                libsql::params![LEDGER, format!("{prefix}%")],
            )
            .await
            .map_err(db_err("skills ledger"))?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.map_err(db_err("skills ledger row"))? {
            let key: String = row.get(0).map_err(db_err("skills ledger key"))?;
            let value: String = row.get(1).map_err(db_err("skills ledger value"))?;
            // Unknown/newer rows are preserved and ignored, never deleted.
            if let Ok(value) = serde_json::from_str(&value) {
                out.push((key, value));
            }
        }
        Ok(out)
    }

    async fn set_skill_ledger<T: serde::Serialize>(
        &self,
        key: &str,
        value: &T,
    ) -> Result<(), WenlanError> {
        let value = serde_json::to_string(value)
            .map_err(|error| WenlanError::Generic(format!("skills ledger: {error}")))?;
        self.upsert_sync_state(LEDGER, key, 0, &value).await
    }

    pub async fn skill_page_states(&self) -> Result<Vec<SkillPageState>, WenlanError> {
        Ok(self
            .skill_ledger_rows::<SkillPageState>(SPACE_PREFIX)
            .await?
            .into_iter()
            .map(|(_, state)| state)
            .collect())
    }

    pub async fn set_skill_page_state(&self, state: &SkillPageState) -> Result<(), WenlanError> {
        self.set_skill_ledger(&format!("{SPACE_PREFIX}{}", state.space_id), state)
            .await
    }

    pub async fn delete_skill_page_state(&self, space_id: &str) -> Result<(), WenlanError> {
        self.delete_sync_state(LEDGER, &format!("{SPACE_PREFIX}{space_id}"))
            .await
    }

    pub async fn skill_install_states(&self) -> Result<Vec<SkillInstallState>, WenlanError> {
        Ok(self
            .skill_ledger_rows::<SkillInstallState>(INSTALL_PREFIX)
            .await?
            .into_iter()
            .map(|(_, state)| state)
            .collect())
    }

    pub async fn set_skill_install_state(
        &self,
        state: &SkillInstallState,
    ) -> Result<(), WenlanError> {
        self.set_skill_ledger(
            &format!("{INSTALL_PREFIX}{}:{}", state.tool, state.name),
            state,
        )
        .await
    }

    pub async fn delete_skill_install_state(
        &self,
        tool: &str,
        name: &str,
    ) -> Result<(), WenlanError> {
        self.delete_sync_state(LEDGER, &format!("{INSTALL_PREFIX}{tool}:{name}"))
            .await
    }

    pub async fn skill_suggestions(&self) -> Result<Vec<StoredSkillSuggestion>, WenlanError> {
        Ok(self
            .skill_ledger_rows::<StoredSkillSuggestion>(SUGGESTION_PREFIX)
            .await?
            .into_iter()
            .map(|(_, suggestion)| suggestion)
            .collect())
    }

    pub async fn set_skill_suggestion(
        &self,
        suggestion: &StoredSkillSuggestion,
    ) -> Result<(), WenlanError> {
        self.set_skill_ledger(
            &format!("{SUGGESTION_PREFIX}{}", suggestion.suggestion.id),
            suggestion,
        )
        .await
    }

    pub async fn delete_skill_suggestion(&self, id: &str) -> Result<(), WenlanError> {
        self.delete_sync_state(LEDGER, &format!("{SUGGESTION_PREFIX}{id}"))
            .await
    }

    pub async fn skill_last_nightly(&self) -> Result<Option<i64>, WenlanError> {
        Ok(self
            .get_sync_state(LEDGER, NIGHTLY_KEY)
            .await?
            .and_then(|row| row.content_hash.parse().ok()))
    }

    pub async fn set_skill_last_nightly(&self, at: i64) -> Result<(), WenlanError> {
        self.upsert_sync_state(LEDGER, NIGHTLY_KEY, 0, &at.to_string())
            .await
    }

    /// Record only reads of generated source pages. One activity row per page
    /// keeps the record query simple and contains no conversation text.
    pub async fn record_skill_page_reads(
        &self,
        tool: &str,
        page_ids: &[String],
    ) -> Result<(), WenlanError> {
        let tool = tool.trim();
        if tool.is_empty() || tool.eq_ignore_ascii_case("unknown") {
            return Ok(());
        }
        let managed: std::collections::HashSet<String> = self
            .skill_page_states()
            .await?
            .into_iter()
            .map(|state| state.page_id)
            .collect();
        let mut recorded = std::collections::HashSet::new();
        for page_id in page_ids
            .iter()
            .filter(|id| managed.contains(*id) && recorded.insert((*id).clone()))
        {
            self.log_agent_activity(tool, "skill_read", std::slice::from_ref(page_id), None, "")
                .await?;
        }
        Ok(())
    }

    pub async fn record_skill_self_report(
        &self,
        tool: &str,
        page_id: &str,
        report: &str,
    ) -> Result<bool, WenlanError> {
        let known = self
            .skill_page_states()
            .await?
            .into_iter()
            .any(|state| state.page_id == page_id);
        if !known {
            return Ok(false);
        }
        let report = crate::privacy::redact_pii(report);
        let report: String = report.split_whitespace().collect::<Vec<_>>().join(" ");
        let report: String = report.chars().take(300).collect();
        self.log_agent_activity(
            tool,
            "skill_reported",
            &[page_id.to_string()],
            None,
            &report,
        )
        .await?;
        Ok(true)
    }

    pub async fn skill_borrow_records(
        &self,
        since: i64,
        until: i64,
    ) -> Result<Vec<SkillBorrowRecord>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT a.timestamp, a.agent_name, a.action, a.memory_ids, a.detail, \
                        COALESCE(p.title, '') \
                   FROM agent_activity a \
                   LEFT JOIN pages p ON p.id = a.memory_ids \
                  WHERE a.action IN ('skill_read', 'skill_reported') \
                    AND a.timestamp >= ?1 AND a.timestamp < ?2 \
                  ORDER BY a.timestamp DESC",
                libsql::params![since, until],
            )
            .await
            .map_err(db_err("skill borrow records"))?;
        let mut out = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(db_err("skill borrow records row"))?
        {
            let page_id: String = row.get(3).unwrap_or_default();
            let title: String = row.get(5).unwrap_or_default();
            if page_id.is_empty() || title.is_empty() {
                continue;
            }
            let action: String = row.get(2).unwrap_or_default();
            let reported = action == "skill_reported";
            let detail: String = row.get(4).unwrap_or_default();
            out.push(SkillBorrowRecord {
                at: row.get(0).unwrap_or(0),
                tool: row.get(1).unwrap_or_default(),
                page_id,
                page_title: title,
                ai_reported: reported,
                report: (reported && !detail.is_empty()).then_some(detail),
            });
        }
        Ok(out)
    }
}
