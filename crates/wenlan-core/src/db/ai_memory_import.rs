// SPDX-License-Identifier: Apache-2.0
//! The record of the one-time import of AI tools' memory
//! (`crate::ai_memory_import`), kept for its one Undo, and the lookups the
//! import needs: which Space a Claude Code project's memory belongs in.

use super::MemoryDB;
use crate::ai_memory_import::ImportCounts;
use crate::WenlanError;

/// Where the import keeps what it did while it stands. Unset, it may run
/// (again).
const PASS_KEY: &str = "ai_memory_import_v1";

fn db_err(context: &str) -> impl Fn(libsql::Error) -> WenlanError + '_ {
    move |e| WenlanError::VectorDb(format!("{context}: {e}"))
}

/// What one import did.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AiMemoryImport {
    pub at: i64,
    pub counts: ImportCounts,
    /// The new points the import wrote, by `source_id`. Undo forgets these
    /// and nothing else.
    pub written: Vec<String>,
    /// How many came from each tool, as in `agent_sessions.tool`.
    #[serde(default)]
    pub by_tool: Vec<(String, usize)>,
}

impl MemoryDB {
    /// The import, if it ran and was not taken back.
    pub async fn ai_memory_import(&self) -> Result<Option<AiMemoryImport>, WenlanError> {
        Ok(self
            .get_app_metadata(PASS_KEY)
            .await?
            .and_then(|value| serde_json::from_str(&value).ok()))
    }

    /// Record what an import did, once it is done. Refused while an earlier
    /// one stands: the caller checks first, and this keeps two runs from both
    /// landing.
    pub async fn record_ai_memory_import(&self, pass: &AiMemoryImport) -> Result<(), WenlanError> {
        let value = serde_json::to_string(pass)
            .map_err(|e| WenlanError::Generic(format!("ai memory import record: {e}")))?;
        let conn = self.conn.lock().await;
        let changed = conn
            .execute(
                "INSERT INTO app_metadata (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO NOTHING",
                libsql::params![PASS_KEY, value],
            )
            .await
            .map_err(db_err("ai memory import record"))?;
        if changed == 0 {
            return Err(WenlanError::Conflict(
                "the AI memory import already ran; take it back first".into(),
            ));
        }
        Ok(())
    }

    /// Take the whole import back: every point it wrote is forgotten. A
    /// memory the person
    /// has since edited is theirs now and stays. Returns what was taken back,
    /// or `None` when no import stands.
    pub async fn undo_ai_memory_import(&self) -> Result<Option<AiMemoryImport>, WenlanError> {
        let Some(pass) = self.ai_memory_import().await? else {
            return Ok(None);
        };
        self.forget_ai_memory_rows(&pass.written).await?;
        let conn = self.conn.lock().await;
        conn.execute(
            "DELETE FROM app_metadata WHERE key = ?1",
            libsql::params![PASS_KEY],
        )
        .await
        .map_err(db_err("ai memory import undo record"))?;
        Ok(Some(pass))
    }

    /// Forget memories an import wrote, leaving any the person edited since.
    /// Also used when an import fails partway, so it leaves nothing behind.
    pub async fn forget_ai_memory_rows(&self, ids: &[String]) -> Result<usize, WenlanError> {
        let mut forgotten = 0;
        for id in ids {
            if self.person_edited_memory(id).await? {
                continue;
            }
            self.delete_by_source_id("memory", id).await?;
            forgotten += 1;
        }
        Ok(forgotten)
    }

    async fn person_edited_memory(&self, id: &str) -> Result<bool, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT authored_content IS NOT NULL FROM memories
                  WHERE source = 'memory' AND source_id = ?1 AND chunk_index = 0",
                libsql::params![id],
            )
            .await
            .map_err(db_err("ai memory import author"))?;
        Ok(
            match rows
                .next()
                .await
                .map_err(db_err("ai memory import author row"))?
            {
                Some(row) => row.get::<i64>(0).unwrap_or(0) != 0,
                None => false,
            },
        )
    }

    /// The registered Space most of a Claude Code project's conversations
    /// went to: the project's memory belongs there too. Conversations are
    /// found by their transcript, which lives in the project's folder. `None`
    /// when none went to a registered Space.
    pub async fn space_for_transcript_folder(
        &self,
        folder: &str,
    ) -> Result<Option<String>, WenlanError> {
        let folder = folder.trim_end_matches('/');
        if folder.is_empty() {
            return Ok(None);
        }
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT a.space FROM agent_sessions a
                   JOIN spaces sp ON sp.name = a.space
                  WHERE sp.id <> ?3
                    AND (substr(a.transcript_path, 1, length(?1) + 1) = ?1 || '/'
                         OR substr(a.transcript_path, 1, length(?2) + 1) = ?2 || '\\')
                  GROUP BY a.space
                  ORDER BY COUNT(*) DESC, MAX(a.last_write_at) DESC
                  LIMIT 1",
                libsql::params![folder, folder, super::UNFILED_SPACE_ID],
            )
            .await
            .map_err(db_err("ai memory import space"))?;
        Ok(
            match rows
                .next()
                .await
                .map_err(db_err("ai memory import space row"))?
            {
                Some(row) => row.get::<Option<String>>(0).unwrap_or(None),
                None => None,
            },
        )
    }
}
