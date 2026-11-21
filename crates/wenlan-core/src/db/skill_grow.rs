// SPDX-License-Identifier: Apache-2.0
//! Durable consent, checkpoint, undo, and suggestion state for Space rules.

use super::{not_hidden_by_superseder, not_self_archived, MemoryDB, UNFILED_SPACE_ID};
use crate::error::WenlanError;
use crate::read_scope::ReadScope;
use wenlan_types::{SkillBorrowRecord, SkillLine, SkillReportKind, SkillSuggestion};

const LEDGER: &str = "skills";
const SPACE_PREFIX: &str = "space:";
const SUGGESTION_PREFIX: &str = "suggestion:";
const FILE_PREFIX: &str = "file:";
const NIGHT_BUDGET_KEY: &str = "night_budget";
const SAVED_RULES_PREFIX: &str = "saved-rules:";

/// The rule pages the person keeps for their own tools. Null/malformed rows
/// yield NULL, so callers select or test only concrete ids.
pub(super) fn personal_rule_pages_sql() -> String {
    format!(
        "SELECT CASE WHEN json_valid(content_hash) THEN
            CASE WHEN file_path LIKE '{SPACE_PREFIX}%' THEN json_extract(content_hash, '$.rules_page_id')
                 ELSE json_extract(content_hash, '$.page_id') END END AS page_id
         FROM source_sync_state WHERE source_id = '{LEDGER}'
            AND (file_path LIKE '{SPACE_PREFIX}%' OR file_path LIKE '{SAVED_RULES_PREFIX}%')"
    )
}

/// One rule the person asked to be saved (UX-SKILL-14). Kept after the rule
/// is removed: it is the record of a save, and the day's count reads it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SavedRule {
    /// This record's own id.
    #[serde(default)]
    pub id: String,
    /// The rule as saved, without its `[decision]` or `[pitfall]` tag.
    pub text: String,
    /// The tool the save came from, or `terminal`: what rules lists show.
    pub tool: String,
    /// The name the caller reported, which the day's count and the right to
    /// remove are kept by (compared without case or padding).
    #[serde(default)]
    pub tool_id: String,
    pub person_asked: String,
    pub at: i64,
    /// The page version that brought the rule in: the save's own, or the
    /// one that accepted its card. `None` while the save waits as a card on
    /// a locked page; a waiting save gives no rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<i64>,
    /// The revision card a save on a locked page waits as. Only accepting
    /// this very card gives the save its `version`; when the card is
    /// dismissed, replaced or discarded the record is dropped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card: Option<String>,
}

/// One writer of the saved-rules rows at a time: a save, a removal and the
/// accept or dismissal of a waiting save's card each read a row, change it
/// and write it back.
static SAVED_RULES_EDIT: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The "Rules I saved" page of one scope: a Space, or every Space.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SavedRulesPage {
    /// The Space's id; empty for the rules used in every Space.
    pub space_id: String,
    pub page_id: String,
    #[serde(default)]
    pub saves: Vec<SavedRule>,
}

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
    pub(crate) async fn personal_rule_page_ids(&self) -> Result<Vec<String>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(&personal_rule_pages_sql(), ())
            .await
            .map_err(db_err("personal rule pages"))?;
        let mut ids = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(db_err("personal rule page row"))?
        {
            if let Some(id) = row.get::<Option<String>>(0).ok().flatten() {
                ids.push(id);
            }
        }
        Ok(ids)
    }

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

    /// Every "Rules I saved" page the ledger knows, one per scope.
    pub async fn saved_rules_pages(&self) -> Result<Vec<SavedRulesPage>, WenlanError> {
        Ok(self
            .skill_ledger_rows::<SavedRulesPage>(SAVED_RULES_PREFIX)
            .await?
            .into_iter()
            .map(|(_, page)| page)
            .collect())
    }

    /// Read the saved-rules row of one scope (`space_id`, empty for every
    /// Space), change it and write it back, with no other change to these
    /// rows in between. A scope with no row yet starts empty. Returns what
    /// `change` returns, and the row as written.
    pub async fn edit_saved_rules_page<T>(
        &self,
        space_id: &str,
        change: impl FnOnce(&mut SavedRulesPage) -> T,
    ) -> Result<(T, SavedRulesPage), WenlanError> {
        let _one_writer = SAVED_RULES_EDIT.lock().await;
        let mut page = self
            .saved_rules_pages()
            .await?
            .into_iter()
            .find(|page| page.space_id == space_id)
            .unwrap_or_else(|| SavedRulesPage {
                space_id: space_id.to_string(),
                page_id: String::new(),
                saves: Vec::new(),
            });
        let before = page.clone();
        let out = change(&mut page);
        if page != before {
            self.set_skill_ledger(&format!("{SAVED_RULES_PREFIX}{}", page.space_id), &page)
                .await?;
        }
        Ok((out, page))
    }

    /// The person accepted revision card `card_id`, which made `version` of
    /// its page: a save that waited as that very card counts from that
    /// version on. Any other card changes nothing here.
    pub async fn saved_rule_card_accepted(
        &self,
        card_id: &str,
        version: i64,
    ) -> Result<(), WenlanError> {
        self.settle_saved_rule_card(card_id, Some(version)).await
    }

    /// Revision card `card_id` is gone without being accepted (dismissed,
    /// replaced by a newer proposal, or discarded as outdated): a save that
    /// waited as it is dropped, so no later accept can bring its rule in.
    pub async fn saved_rule_card_gone(&self, card_id: &str) -> Result<(), WenlanError> {
        self.settle_saved_rule_card(card_id, None).await
    }

    async fn settle_saved_rule_card(
        &self,
        card_id: &str,
        version: Option<i64>,
    ) -> Result<(), WenlanError> {
        let waits =
            |save: &SavedRule| save.version.is_none() && save.card.as_deref() == Some(card_id);
        // Nearly every card is not a saved rule's: look before taking the turn.
        let scopes: Vec<String> = self
            .saved_rules_pages()
            .await?
            .into_iter()
            .filter(|page| page.saves.iter().any(waits))
            .map(|page| page.space_id)
            .collect();
        for space_id in scopes {
            self.edit_saved_rules_page(&space_id, |page| match version {
                Some(version) => {
                    for save in page.saves.iter_mut().filter(|save| waits(save)) {
                        save.version = Some(version);
                    }
                }
                None => page.saves.retain(|save| !waits(save)),
            })
            .await?;
        }
        Ok(())
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

    /// The preferences the person said once, kept outside every Space: what
    /// their own AI tools are given in every Space (UX-CHAT-5). Only a
    /// memory the person has confirmed, still current, with no Space at all;
    /// one in a Space (a tool put it there itself) is that Space's, not this.
    /// Newest first, as `(id, text)`.
    pub async fn person_preferences(
        &self,
        limit: usize,
    ) -> Result<Vec<(String, String)>, WenlanError> {
        let sql = format!(
            "SELECT m.source_id, m.content FROM memories m
              WHERE m.source = 'memory' AND m.chunk_index = 0
                AND m.memory_type = 'preference'
                AND COALESCE(m.confirmed, 0) = 1
                AND (m.space IS NULL OR m.space = '' OR m.space = '{UNFILED_SPACE_ID}')
                AND COALESCE(m.pending_revision, 0) = 0
                AND COALESCE(m.is_recap, 0) = 0
                AND {archived} AND {hidden}
              ORDER BY m.created_at DESC, m.source_id LIMIT ?1",
            archived = not_self_archived("m"),
            hidden = not_hidden_by_superseder(&ReadScope::Global, "m"),
        );
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(&sql, libsql::params![limit as i64])
            .await
            .map_err(|e| WenlanError::Generic(format!("person preferences: {e}")))?;
        let mut out = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| WenlanError::Generic(format!("person preferences row: {e}")))?
        {
            let text: String = row.get(1).unwrap_or_default();
            if !text.trim().is_empty() {
                out.push((row.get(0).unwrap_or_default(), text));
            }
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
        // A line with no page behind it (a preference the person said once)
        // has nothing to count a borrow against.
        for line in lines.iter().filter(|line| !line.source_page_id.is_empty()) {
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
        if line.source_page_id.is_empty() {
            return Ok(());
        }
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
