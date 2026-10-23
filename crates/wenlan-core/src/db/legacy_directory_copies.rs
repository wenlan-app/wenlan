use super::MemoryDB;
use crate::error::WenlanError;
use std::path::Path;

struct LegacyCopyRow {
    id: String,
    version: i64,
    changelog: Option<String>,
    previous_mode: Option<String>,
}

struct LegacyCopyUpdate {
    version: i64,
    changelog: String,
    id: String,
    previous_mode: Option<String>,
}

impl MemoryDB {
    /// Return distinct live `local_files` source paths once per directory
    /// sync. Callers use physical-path matching to catch old watcher rows whose
    /// path spelling differs from the registered Directory path.
    pub async fn legacy_directory_copy_paths(&self) -> Result<Vec<String>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT DISTINCT source_id FROM memories \
                 WHERE source = 'local_files' AND COALESCE(supersede_mode, '') <> 'evicted' \
                   AND url LIKE 'file://%' ORDER BY source_id",
                (),
            )
            .await
            .map_err(|e| WenlanError::VectorDb(format!("legacy-copy paths read: {e}")))?;
        let mut paths = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| WenlanError::VectorDb(format!("legacy-copy path row: {e}")))?
        {
            if let Ok(path) = row.get::<String>(0) {
                paths.push(path);
            }
        }
        Ok(paths)
    }

    /// Cheap probe used to decide whether an unchanged-mtime file must still
    /// be read and hashed before normal sync can safely skip it.
    pub async fn has_legacy_directory_copy(&self, legacy_path: &str) -> Result<bool, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT 1 FROM memories WHERE source = 'local_files' AND source_id = ?1 \
                 AND COALESCE(supersede_mode, '') <> 'evicted' LIMIT 1",
                libsql::params![legacy_path],
            )
            .await
            .map_err(|e| WenlanError::VectorDb(format!("legacy-copy candidate read: {e}")))?;
        let found = rows
            .next()
            .await
            .map_err(|e| WenlanError::VectorDb(format!("legacy-copy candidate row: {e}")))?
            .is_some();
        Ok(found)
    }

    /// Retire an auto-generated legacy `local_files` copy after the canonical
    /// folder document has been committed. Rows and their content are retained;
    /// only the existing self-state `evicted` marker changes, which removes them
    /// from default search while keeping the history recoverable.
    ///
    /// The method intentionally accepts exact provenance supplied by the
    /// importer. It will not infer identity from basename, content similarity,
    /// or a different source/space. Both complete document groups are rechecked
    /// under one transaction before any row is touched.
    pub async fn retire_legacy_directory_copies(
        &self,
        legacy_path: &str,
        source_id: &str,
        registered_file_path: &Path,
        knowledge_path: Option<&Path>,
        expected_content_hash: &str,
        expected_space: Option<&str>,
    ) -> Result<usize, WenlanError> {
        let canonical_source_id = crate::sources::directory::document_source_id(
            source_id,
            registered_file_path,
            knowledge_path,
        );
        let expected_space = expected_space.unwrap_or(crate::db::UNFILED_SPACE_ID);
        let filename = Path::new(legacy_path)
            .file_name()
            .and_then(|part| part.to_str())
            .ok_or_else(|| WenlanError::VectorDb("legacy copy path has no filename".into()))?;
        let expected_url = format!("file://{legacy_path}");
        // Most files never had a legacy watcher copy. Avoid opening a write
        // transaction for every ordinary refresh; the same candidate set is
        // checked again after BEGIN before any row changes.
        if !self.has_legacy_directory_copy(legacy_path).await? {
            return Ok(0);
        }
        // The watcher request and registered source can spell the same file
        // differently (for example `/tmp` vs `/private/tmp` on macOS). Accept
        // exact spelling cheaply; otherwise require both existing paths to
        // resolve to the same file. Basename equality is never sufficient.
        let registered_path_string = registered_file_path.to_string_lossy();
        let same_file = legacy_path == registered_path_string
            || std::fs::canonicalize(legacy_path)
                .ok()
                .zip(std::fs::canonicalize(registered_file_path).ok())
                .is_some_and(|(legacy, registered)| legacy == registered);
        if !same_file {
            return Ok(0);
        }
        let conn = self.conn.lock().await;
        conn.execute("BEGIN", ())
            .await
            .map_err(|e| WenlanError::VectorDb(format!("legacy-copy retirement begin: {e}")))?;

        let result: Result<Vec<LegacyCopyRow>, WenlanError> = async {
            // Validate the *entire* canonical group, including chunk zero. Any
            // mixed hash, space, source agent, or pending chunk makes it unsafe.
            let mut canonical_rows = conn
                .query(
                    "SELECT chunk_index, source_agent, content_hash, space, pending_revision, supersede_mode, memory_type \
                     FROM memories WHERE source = 'memory' AND source_id = ?1",
                    libsql::params![canonical_source_id.as_str()],
                )
                .await
                .map_err(|e| WenlanError::VectorDb(format!("legacy-copy canonical read: {e}")))?;
            let mut canonical_count = 0usize;
            let mut has_chunk_zero = false;
            let mut canonical_valid = true;
            while let Some(row) = canonical_rows
                .next()
                .await
                .map_err(|e| WenlanError::VectorDb(format!("legacy-copy canonical row: {e}")))?
            {
                canonical_count += 1;
                let chunk_index: i64 = row
                    .get(0)
                    .map_err(|e| WenlanError::VectorDb(format!("legacy-copy canonical chunk: {e}")))?;
                has_chunk_zero |= chunk_index == 0;
                let source_agent: Option<String> = row.get(1).unwrap_or(None);
                let content_hash: Option<String> = row.get(2).unwrap_or(None);
                let space: Option<String> = row.get(3).unwrap_or(None);
                let pending: i64 = row.get(4).unwrap_or(1);
                let mode: Option<String> = row.get(5).unwrap_or(None);
                let memory_type: Option<String> = row.get(6).unwrap_or(None);
                let self_archived = mode.as_deref() == Some("evicted")
                    || (mode.as_deref() == Some("archive")
                        && memory_type.as_deref() != Some("decision"));
                canonical_valid &= source_agent.as_deref() == Some("folder")
                    && content_hash.as_deref() == Some(expected_content_hash)
                    && space.as_deref() == Some(expected_space)
                    && pending == 0
                    && !self_archived;
            }
            drop(canonical_rows);
            if canonical_count == 0 || !has_chunk_zero || !canonical_valid {
                return Ok(Vec::new());
            }

            // A newer hide-mode memory in the same Space makes this candidate
            // generation inactive. Do not use its presence to retire history.
            let mut hidden = conn
                .query(
                    "SELECT 1 FROM memories superseder \
                     WHERE superseder.source = 'memory' AND superseder.pending_revision = 0 \
                       AND superseder.supersede_mode = 'hide' \
                       AND superseder.supersedes = ?1 AND superseder.space = ?2 LIMIT 1",
                    libsql::params![canonical_source_id.as_str(), expected_space],
                )
                .await
                .map_err(|e| WenlanError::VectorDb(format!("legacy-copy active check: {e}")))?;
            let is_hidden = hidden
                .next()
                .await
                .map_err(|e| WenlanError::VectorDb(format!("legacy-copy active row: {e}")))?
                .is_some();
            drop(hidden);
            if is_hidden {
                return Ok(Vec::new());
            }

            // An old startup/watcher copy has a very narrow signature. Only
            // untouched generated rows qualify; if any chunk was confirmed,
            // pinned, edited, typed, superseded, or already retired, leave the
            // whole document group alone. `update_memory` promotes the source
            // to its interactive service class, including when an origin row
            // already exists with false explicit-field flags; changelog is not
            // updated by this content-edit path.
            let mut legacy_rows = conn
                .query(
                    "SELECT id, version, changelog, supersede_mode, title, url, space, chunk_index, \
                            source_agent, memory_type, confirmed, pinned, pending_revision, \
                            supersedes, content_hash, \
                            EXISTS(SELECT 1 FROM enrichment_origin eo \
                                   WHERE eo.source_id = memories.source_id \
                                     AND eo.service_class = 0) \
                     FROM memories WHERE source = 'local_files' AND source_id = ?1",
                    libsql::params![legacy_path],
                )
                .await
                .map_err(|e| WenlanError::VectorDb(format!("legacy-copy old read: {e}")))?;
            let mut eligible = Vec::new();
            let mut all_eligible = true;
            let mut legacy_has_chunk_zero = false;
            while let Some(row) = legacy_rows
                .next()
                .await
                .map_err(|e| WenlanError::VectorDb(format!("legacy-copy old row: {e}")))?
            {
                let id: String = row
                    .get(0)
                    .map_err(|e| WenlanError::VectorDb(format!("legacy-copy old id: {e}")))?;
                let version: i64 = row.get(1).unwrap_or(0);
                let changelog: Option<String> = row.get(2).unwrap_or(None);
                let previous_mode: Option<String> = row.get(3).unwrap_or(None);
                let title: String = row.get(4).unwrap_or_default();
                let url: Option<String> = row.get(5).unwrap_or(None);
                let space: Option<String> = row.get(6).unwrap_or(None);
                let chunk_index: i64 = row.get(7).unwrap_or(-1);
                legacy_has_chunk_zero |= chunk_index == 0;
                let source_agent: Option<String> = row.get(8).unwrap_or(None);
                let memory_type: Option<String> = row.get(9).unwrap_or(None);
                let confirmed: Option<i64> = row.get(10).unwrap_or(None);
                let pinned: i64 = row.get(11).unwrap_or(1);
                let pending: i64 = row.get(12).unwrap_or(1);
                let supersedes: Option<String> = row.get(13).unwrap_or(None);
                let content_hash: Option<String> = row.get(14).unwrap_or(None);
                let has_interactive_enrichment_origin: i64 = row.get(15).unwrap_or(1);
                let changelog_is_generated = changelog
                    .as_deref()
                    .map(|value| value.trim().is_empty() || value.trim() == "[]")
                    .unwrap_or(true);
                let row_eligible = changelog_is_generated
                    && title == filename
                    && url.as_deref() == Some(expected_url.as_str())
                    && space.as_deref() == Some(expected_space)
                    && source_agent.is_none()
                    && memory_type.is_none()
                    && confirmed.is_none()
                    && pinned == 0
                    && pending == 0
                    && supersedes.is_none()
                    && content_hash.is_none()
                    && has_interactive_enrichment_origin == 0
                    && previous_mode.as_deref().is_none_or(|mode| mode == "hide");
                if row_eligible {
                    eligible.push(LegacyCopyRow {
                        id,
                        version,
                        changelog,
                        previous_mode,
                    });
                } else {
                    all_eligible = false;
                }
            }
            drop(legacy_rows);
            if eligible.is_empty() || !all_eligible || !legacy_has_chunk_zero {
                return Ok(Vec::new());
            }
            Ok(eligible)
        }
        .await;

        let legacy_rows = match result {
            Ok(rows) => rows,
            Err(error) => {
                let rollback = conn.execute("ROLLBACK", ()).await;
                if let Err(rollback_error) = rollback {
                    return Err(WenlanError::VectorDb(format!(
                        "{error}; legacy-copy rollback failed: {rollback_error}"
                    )));
                }
                return Err(error);
            }
        };

        if legacy_rows.is_empty() {
            conn.execute("ROLLBACK", ())
                .await
                .map_err(|e| WenlanError::VectorDb(format!("legacy-copy no-op rollback: {e}")))?;
            return Ok(0);
        }

        let updates: Result<Vec<LegacyCopyUpdate>, WenlanError> = legacy_rows
            .iter()
            .map(|row| {
            let version = row.version + 1;
            let mut history: Vec<serde_json::Value> = row
                .changelog
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .and_then(|value| serde_json::from_str(value).ok())
                .unwrap_or_default();
            history.push(serde_json::json!({
                "version": version,
                "at": chrono::Utc::now().timestamp(),
                "delta": "Automatically retired duplicate folder-imported copy; original content retained",
                "source_agent": null,
                "incoming_source_id": canonical_source_id.as_str(),
                "replacement_source_id": canonical_source_id.as_str(),
                "previous_supersede_mode": row.previous_mode,
                "retirement": "duplicate_directory_import"
            }));
                Ok(LegacyCopyUpdate {
                    version,
                    changelog: serde_json::to_string(&history).map_err(|e| {
                        WenlanError::VectorDb(format!("legacy-copy changelog encode: {e}"))
                    })?,
                    id: row.id.clone(),
                    previous_mode: row.previous_mode.clone(),
                })
            })
            .collect();
        let updates = match updates {
            Ok(updates) => updates,
            Err(error) => {
                let _ = conn.execute("ROLLBACK", ()).await;
                return Err(error);
            }
        };
        let write_result: Result<(), WenlanError> = async {
            for update in &updates {
                conn.execute(
                    "UPDATE memories SET supersede_mode = 'evicted', version = ?1, changelog = ?2 \
                 WHERE id = ?3 AND source = 'local_files' AND source_id = ?4 \
                   AND COALESCE(supersede_mode, 'hide') = COALESCE(?5, 'hide')",
                    libsql::params![
                        update.version,
                        update.changelog.as_str(),
                        update.id.as_str(),
                        legacy_path,
                        update.previous_mode.as_deref()
                    ],
                )
                .await
                .map_err(|e| WenlanError::VectorDb(format!("legacy-copy retire row: {e}")))?;
            }
            Ok(())
        }
        .await;
        if let Err(error) = write_result {
            if let Err(rollback_error) = conn.execute("ROLLBACK", ()).await {
                return Err(WenlanError::VectorDb(format!(
                    "{error}; legacy-copy rollback failed: {rollback_error}"
                )));
            }
            return Err(error);
        }

        if let Err(error) = conn.execute("COMMIT", ()).await {
            if let Err(rollback_error) = conn.execute("ROLLBACK", ()).await {
                return Err(WenlanError::VectorDb(format!(
                    "legacy-copy commit: {error}; rollback failed: {rollback_error}"
                )));
            }
            return Err(WenlanError::VectorDb(format!(
                "legacy-copy commit: {error}"
            )));
        }
        Ok(legacy_rows.len())
    }
}
