// SPDX-License-Identifier: Apache-2.0
//! Durable consent, checkpoint, undo, and suggestion state for Space rules.

use super::MemoryDB;
use crate::error::WenlanError;
use wenlan_types::{SkillBorrowRecord, SkillLine, SkillReportKind, SkillSuggestion};

const LEDGER: &str = "skills";
const SPACE_PREFIX: &str = "space:";
const SUGGESTION_PREFIX: &str = "suggestion:";
const FILE_PREFIX: &str = "file:";
const NIGHT_BUDGET_KEY: &str = "night_budget";

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpaceRulesStatus {
    ExcludedPrivate,
    Suggested,
    Accepting,
    Declined,
    Accepted,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RulesPageUndo {
    pub before_content: String,
    pub before_lines: Vec<SkillLine>,
    pub before_deleted_fingerprints: Vec<String>,
    pub applied_version: i64,
    pub applied_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SpaceRulesState {
    pub space_id: String,
    pub space_name: String,
    pub status: SpaceRulesStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules_page_id: Option<String>,
    #[serde(default)]
    pub lines: Vec<SkillLine>,
    #[serde(default)]
    pub deleted_fingerprints: Vec<String>,
    #[serde(default)]
    pub input_hash: String,
    #[serde(default)]
    pub last_success_at: i64,
    #[serde(default)]
    pub failure_count: u32,
    #[serde(default)]
    pub next_retry_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undo: Option<RulesPageUndo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuggestionStatus {
    Pending,
    Applying,
    Applied,
    Undoing,
    Dismissed,
    Undone,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredSkillSuggestion {
    pub suggestion: SkillSuggestion,
    pub skill_file: String,
    pub skill_root: String,
    pub status: SuggestionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SkillFileCheckpoint {
    pub skill_file: String,
    pub input_hash: String,
    #[serde(default)]
    pub last_success_at: i64,
    #[serde(default)]
    pub failure_count: u32,
    #[serde(default)]
    pub next_retry_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NightlySkillBudget {
    pub local_date: String,
    pub calls: usize,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct BorrowDetail {
    space_id: String,
    rule_id: String,
    #[serde(default)]
    page_title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    self_report: Option<SkillReportKind>,
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
            // Unknown/newer and v1 rows are preserved and ignored.
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

    pub async fn space_rules_states(&self) -> Result<Vec<SpaceRulesState>, WenlanError> {
        Ok(self
            .skill_ledger_rows::<SpaceRulesState>(SPACE_PREFIX)
            .await?
            .into_iter()
            .map(|(_, state)| state)
            .collect())
    }

    pub async fn space_rules_state(
        &self,
        space_id: &str,
    ) -> Result<Option<SpaceRulesState>, WenlanError> {
        Ok(self
            .space_rules_states()
            .await?
            .into_iter()
            .find(|state| state.space_id == space_id))
    }

    pub async fn set_space_rules_state(&self, state: &SpaceRulesState) -> Result<(), WenlanError> {
        self.set_skill_ledger(&format!("{SPACE_PREFIX}{}", state.space_id), state)
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

    pub async fn skill_file_checkpoints(&self) -> Result<Vec<SkillFileCheckpoint>, WenlanError> {
        Ok(self
            .skill_ledger_rows::<SkillFileCheckpoint>(FILE_PREFIX)
            .await?
            .into_iter()
            .map(|(_, checkpoint)| checkpoint)
            .collect())
    }

    pub async fn set_skill_file_checkpoint(
        &self,
        checkpoint: &SkillFileCheckpoint,
    ) -> Result<(), WenlanError> {
        let key = crate::skill_grow::line_id(
            wenlan_types::SkillLineKind::Rule,
            &checkpoint.skill_file,
            0,
            &checkpoint.skill_file,
        );
        self.set_skill_ledger(&format!("{FILE_PREFIX}{key}"), checkpoint)
            .await
    }

    pub async fn nightly_skill_budget(&self) -> Result<Option<NightlySkillBudget>, WenlanError> {
        Ok(self
            .get_sync_state(LEDGER, NIGHT_BUDGET_KEY)
            .await?
            .and_then(|row| serde_json::from_str(&row.content_hash).ok()))
    }

    pub async fn set_nightly_skill_budget(
        &self,
        budget: &NightlySkillBudget,
    ) -> Result<(), WenlanError> {
        self.set_skill_ledger(NIGHT_BUDGET_KEY, budget).await
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

    /// The versions of a page that an accepted sentence proposal made. Such a
    /// version is recorded as the person's edit, but its new text was an AI
    /// tool's (`sentence_history`, `sentence.accept`).
    pub async fn sentence_accept_versions(&self, page_id: &str) -> Result<Vec<i64>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT DISTINCT version FROM sentence_history \
                 WHERE op = 'sentence.accept' AND target_type = 'page' AND target_id = ?1 \
                   AND version IS NOT NULL",
                libsql::params![page_id],
            )
            .await
            .map_err(|e| WenlanError::VectorDb(format!("sentence_accept_versions: {e}")))?;
        let mut out = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| WenlanError::VectorDb(format!("sentence_accept_versions row: {e}")))?
        {
            out.push(row.get::<i64>(0).unwrap_or_default());
        }
        Ok(out)
    }

    pub async fn record_skill_rules_read(
        &self,
        tool: &str,
        space_id: &str,
        lines: &[SkillLine],
    ) -> Result<(), WenlanError> {
        let tool = tool.trim();
        if tool.is_empty() || tool.eq_ignore_ascii_case("unknown") {
            return Ok(());
        }
        for line in lines {
            let detail = serde_json::to_string(&BorrowDetail {
                space_id: space_id.to_string(),
                rule_id: line.id.clone(),
                page_title: line.source_page_title.clone(),
                self_report: None,
            })
            .map_err(|error| WenlanError::Generic(format!("skill borrow detail: {error}")))?;
            self.log_agent_activity(
                tool,
                "skill_read",
                std::slice::from_ref(&line.source_page_id),
                None,
                &detail,
            )
            .await?;
        }
        Ok(())
    }

    pub async fn record_skill_self_report(
        &self,
        tool: &str,
        space_id: &str,
        line: &SkillLine,
        report: SkillReportKind,
    ) -> Result<(), WenlanError> {
        let detail = serde_json::to_string(&BorrowDetail {
            space_id: space_id.to_string(),
            rule_id: line.id.clone(),
            page_title: line.source_page_title.clone(),
            self_report: Some(report),
        })
        .map_err(|error| WenlanError::Generic(format!("skill report detail: {error}")))?;
        self.log_agent_activity(
            tool,
            "skill_reported",
            std::slice::from_ref(&line.source_page_id),
            None,
            &detail,
        )
        .await
    }

    pub async fn skill_borrow_records(
        &self,
        space_id: &str,
        since: i64,
        until: i64,
    ) -> Result<Vec<SkillBorrowRecord>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT a.timestamp, a.agent_name, a.action, a.detail \
                   FROM agent_activity a \
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
            let detail: String = row.get(3).unwrap_or_default();
            let Ok(detail) = serde_json::from_str::<BorrowDetail>(&detail) else {
                continue;
            };
            if detail.space_id != space_id {
                continue;
            }
            let action: String = row.get(2).unwrap_or_default();
            out.push(SkillBorrowRecord {
                at: row.get(0).unwrap_or(0),
                tool: row.get(1).unwrap_or_default(),
                space_id: detail.space_id,
                page_title: detail.page_title,
                rule_id: detail.rule_id,
                self_report: (action == "skill_reported")
                    .then_some(detail.self_report)
                    .flatten(),
            });
        }
        Ok(out)
    }
}
