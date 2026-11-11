// SPDX-License-Identifier: Apache-2.0
//! What Wenlan keeps while it follows AI tools' memory
//! (`crate::ai_memory_import`): whether the person turned following on, and
//! a ledger of every tool memory already decided, with the point it became.
//! Also the lookup of which Space a Claude Code project's memory belongs in.
//!
//! The ledger lives in `source_sync_state` under its own source id, one row
//! per memory (keyed by [`crate::ai_memory_import::memory_key`]); the row's
//! `content_hash` column holds the [`Decided`] outcome as JSON.

use super::MemoryDB;
use crate::ai_memory_import::ImportCounts;
use crate::WenlanError;

/// Where following is recorded while it is on. Unset, nothing is followed.
const FOLLOW_KEY: &str = "ai_memory_import_v1";
/// The ledger's source id in `source_sync_state`.
const LEDGER: &str = "ai-memory";

fn db_err(context: &str) -> impl Fn(libsql::Error) -> WenlanError + '_ {
    move |e| WenlanError::VectorDb(format!("{context}: {e}"))
}

/// Following, while it is on.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AiMemoryImport {
    /// When the person turned it on.
    pub at: i64,
    /// When the tools' memory was last read, and what that pass did.
    #[serde(default)]
    pub last_at: i64,
    pub counts: ImportCounts,
    /// How many memories each tool keeps, as in `agent_sessions.tool`.
    #[serde(default)]
    pub by_tool: Vec<(String, usize)>,
}

/// What became of one tool memory.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "o", rename_all = "snake_case")]
pub enum Decided {
    /// Written as this point, with the fingerprint of the words written
    /// ([`crate::ai_memory_import::text_hash`]): a point whose words no
    /// longer match was edited after, and is not Wenlan's to change.
    Point {
        id: String,
        hash: String,
    },
    Sensitive,
    NotKnowledge,
    /// Wenlan already held it.
    Already,
    /// Disagreed with what Wenlan holds, or could not be told apart.
    Conflict,
    /// Kept, but Wenlan's own checks refused to store it (too short to be
    /// a memory, for one).
    Refused,
}

/// One ledger row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerRow {
    pub key: String,
    /// The memory file it was read from, to tell an edit from a new memory.
    pub file: String,
    pub decided: Decided,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Stored {
    f: String,
    #[serde(flatten)]
    d: Decided,
}

impl MemoryDB {
    /// Following, if the person turned it on.
    pub async fn ai_memory_import(&self) -> Result<Option<AiMemoryImport>, WenlanError> {
        Ok(self
            .get_app_metadata(FOLLOW_KEY)
            .await?
            .and_then(|value| serde_json::from_str(&value).ok()))
    }

    /// Record following and what its latest pass did.
    pub async fn record_ai_memory_import(&self, pass: &AiMemoryImport) -> Result<(), WenlanError> {
        let value = serde_json::to_string(pass)
            .map_err(|e| WenlanError::Generic(format!("ai memory import record: {e}")))?;
        self.set_app_metadata(FOLLOW_KEY, &value).await
    }

    /// Every memory decided so far.
    pub async fn ai_memory_ledger(&self) -> Result<Vec<LedgerRow>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT file_path, content_hash FROM source_sync_state WHERE source_id = ?1",
                libsql::params![LEDGER],
            )
            .await
            .map_err(db_err("ai memory ledger"))?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.map_err(db_err("ai memory ledger row"))? {
            let key: String = row.get(0).map_err(db_err("ai memory ledger key"))?;
            let raw: String = row.get(1).map_err(db_err("ai memory ledger value"))?;
            // A row this version cannot read is left alone, never acted on.
            if let Ok(stored) = serde_json::from_str::<Stored>(&raw) {
                out.push(LedgerRow {
                    key,
                    file: stored.f,
                    decided: stored.d,
                });
            }
        }
        Ok(out)
    }

    /// Note what became of one memory.
    pub async fn set_ai_memory_decided(
        &self,
        key: &str,
        file: &str,
        decided: &Decided,
    ) -> Result<(), WenlanError> {
        let value = serde_json::to_string(&Stored {
            f: file.to_string(),
            d: decided.clone(),
        })
        .map_err(|e| WenlanError::Generic(format!("ai memory ledger: {e}")))?;
        self.upsert_sync_state(LEDGER, key, 0, &value).await
    }

    /// The memory left its tool: forget the ledger row.
    pub async fn drop_ai_memory_decided(&self, key: &str) -> Result<(), WenlanError> {
        self.delete_sync_state(LEDGER, key).await
    }

    /// Stop following: every point it wrote is forgotten, except one the
    /// person has edited since (theirs now), and the ledger is cleared, so
    /// turning it on again starts over. Returns the record, or `None` when
    /// following was off.
    pub async fn undo_ai_memory_import(&self) -> Result<Option<AiMemoryImport>, WenlanError> {
        let Some(pass) = self.ai_memory_import().await? else {
            return Ok(None);
        };
        let points: Vec<(String, String)> = self
            .ai_memory_ledger()
            .await?
            .into_iter()
            .filter_map(|row| match row.decided {
                Decided::Point { id, hash } => Some((id, hash)),
                _ => None,
            })
            .collect();
        self.forget_ai_memory_rows(&points).await?;
        self.delete_all_sync_state(LEDGER).await?;
        let conn = self.conn.lock().await;
        conn.execute(
            "DELETE FROM app_metadata WHERE key = ?1",
            libsql::params![FOLLOW_KEY],
        )
        .await
        .map_err(db_err("ai memory import undo record"))?;
        Ok(Some(pass))
    }

    /// Forget points the import wrote, by id and the fingerprint of the
    /// words written, leaving any edited since.
    pub async fn forget_ai_memory_rows(
        &self,
        points: &[(String, String)],
    ) -> Result<usize, WenlanError> {
        let mut forgotten = 0;
        for (id, hash) in points {
            if self.ai_memory_point(id, hash).await? != PointState::Untouched {
                continue;
            }
            self.delete_by_source_id("memory", id).await?;
            forgotten += 1;
        }
        Ok(forgotten)
    }

    /// The fingerprint of a point as it is stored now, read back right after
    /// it is written: what [`Self::ai_memory_point`] later compares with. A
    /// long point is stored in parts, so this is not the memory's own text.
    pub async fn ai_memory_point_fingerprint(
        &self,
        id: &str,
    ) -> Result<Option<String>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT COALESCE(authored_content, content) FROM memories
                  WHERE source = 'memory' AND source_id = ?1 AND chunk_index = 0",
                libsql::params![id],
            )
            .await
            .map_err(db_err("ai memory fingerprint"))?;
        Ok(rows
            .next()
            .await
            .map_err(db_err("ai memory fingerprint row"))?
            .map(|row| {
                crate::ai_memory_import::text_hash(&row.get::<String>(0).unwrap_or_default())
            }))
    }

    /// Whether a point the import wrote is still there, and still as written
    /// (its stored words match the fingerprint taken when it was written).
    pub async fn ai_memory_point(&self, id: &str, hash: &str) -> Result<PointState, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT COALESCE(authored_content, content) FROM memories
                  WHERE source = 'memory' AND source_id = ?1 AND chunk_index = 0",
                libsql::params![id],
            )
            .await
            .map_err(db_err("ai memory point"))?;
        Ok(
            match rows.next().await.map_err(db_err("ai memory point row"))? {
                Some(row) => {
                    let text = row.get::<String>(0).unwrap_or_default();
                    if crate::ai_memory_import::text_hash(&text) == hash {
                        PointState::Untouched
                    } else {
                        PointState::PersonEdited
                    }
                }
                None => PointState::Gone,
            },
        )
    }

    /// The Space a written point is in now (`None`: unsorted), so an edit
    /// of its memory stays where the person may have moved it.
    pub async fn ai_memory_point_space(&self, id: &str) -> Result<Option<String>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT space FROM memories
                  WHERE source = 'memory' AND source_id = ?1 AND chunk_index = 0",
                libsql::params![id],
            )
            .await
            .map_err(db_err("ai memory point space"))?;
        Ok(
            match rows
                .next()
                .await
                .map_err(db_err("ai memory point space row"))?
            {
                Some(row) => row
                    .get::<Option<String>>(0)
                    .unwrap_or(None)
                    .filter(|s| !s.is_empty() && s != super::UNFILED_SPACE_ID),
                None => None,
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

/// A written point, as it stands now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointState {
    /// As the import wrote it: it follows its memory.
    Untouched,
    /// Edited since it was written (by the person, or at their word): theirs,
    /// never changed or removed by following.
    PersonEdited,
    /// The person removed it: it does not come back.
    Gone,
}
