// SPDX-License-Identifier: Apache-2.0
//! Exact-scope research runs and classification decisions. Every mutation owns
//! the connection lock through validation, CAS, write and commit.
use super::{metadata_json, MemoryDB};
use crate::error::WenlanError;
use serde::{de::DeserializeOwned, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use wenlan_types::research::*;

fn sql(error: libsql::Error) -> WenlanError {
    WenlanError::VectorDb(error.to_string())
}

fn digest(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}
pub(super) fn key(kind: &str, identity: &impl Serialize) -> Result<String, WenlanError> {
    if kind == "run" {
        let identity = serde_json::to_value(identity)?;
        let parts = identity
            .as_array()
            .ok_or_else(|| validation("invalid run key"))?;
        let space = parts
            .first()
            .and_then(|v| v.as_str())
            .ok_or_else(|| validation("invalid run Space key"))?;
        let id = parts
            .get(1)
            .and_then(|v| v.as_str())
            .ok_or_else(|| validation("invalid run identity key"))?;
        return Ok(format!("research_v1:run:{}:{}", digest(space), digest(id)));
    }

    Ok(format!(
        "research_v1:{kind}:{}",
        digest(&serde_json::to_string(identity)?)
    ))
}
fn validation(message: &str) -> WenlanError {
    WenlanError::Validation(message.into())
}
async fn load<T: DeserializeOwned>(
    conn: &libsql::Connection,
    key: &str,
) -> Result<Option<T>, WenlanError> {
    metadata_json::load(conn, key, sql).await
}
pub(super) async fn save(
    conn: &libsql::Connection,
    key: &str,
    value: &impl Serialize,
) -> Result<(), WenlanError> {
    metadata_json::save(conn, key, value, sql).await
}
pub(super) async fn space_name(
    conn: &libsql::Connection,
    space_id: &str,
) -> Result<String, WenlanError> {
    let mut rows = conn
        .query("SELECT name FROM spaces WHERE id=?1", [space_id])
        .await
        .map_err(sql)?;
    rows.next()
        .await
        .map_err(sql)?
        .map(|r| r.get::<String>(0).map_err(sql))
        .transpose()?
        .ok_or_else(|| WenlanError::NotFound("research Space no longer exists".into()))
}
pub(super) async fn source_snapshot(
    conn: &libsql::Connection,
    space_id: &str,
    source: &str,
    source_id: &str,
) -> Result<(String, String), WenlanError> {
    let (title, digest, _) = source_snapshot_with_text(conn, space_id, source, source_id).await?;
    Ok((title, digest))
}
async fn source_snapshot_with_text(
    conn: &libsql::Connection,
    space_id: &str,
    source: &str,
    source_id: &str,
) -> Result<(String, String, String), WenlanError> {
    if !matches!(source, "file" | "webpage") || source_id.is_empty() {
        return Err(validation(
            "research requires exact file/webpage identities",
        ));
    }
    let name = space_name(conn, space_id).await?;
    let value = super::citation_sources::read_on(
        conn,
        source_id,
        &crate::read_scope::ReadScope::Space(name),
    )
    .await?
    .ok_or_else(|| validation("source missing, incomplete, ambiguous, or outside this Space"))?;
    if value.snapshot.source != source || value.snapshot.source_id != source_id {
        return Err(validation("source identity is ambiguous"));
    }
    let mut rows = conn
        .query(
            "SELECT title FROM memories WHERE source=?1 AND source_id=?2 AND chunk_index=0 AND NULLIF(space,?3) IS ?4",
            libsql::params![source, source_id, super::UNFILED_SPACE_ID, value.snapshot.space.as_deref()],
        )
        .await
        .map_err(sql)?;
    let title = rows
        .next()
        .await
        .map_err(sql)?
        .ok_or_else(|| validation("source title missing"))?
        .get::<String>(0)
        .map_err(sql)?;
    Ok((title, digest(&value.full_text), value.full_text))
}
pub(super) fn complete_receipt(item: &OrganizeRunItem) -> bool {
    item.served_total_chars
        .is_some_and(|total| total > 0 && item.served_chars == total)
}
async fn decisions(
    conn: &libsql::Connection,
    source: &str,
    source_id: &str,
) -> Result<SourceClassificationDecisions, WenlanError> {
    let mut value: SourceClassificationDecisions =
        load(conn, &key("decisions", &(source, source_id))?)
            .await?
            .unwrap_or_default();
    // Existing untracked tags are human decisions, never AI-owned by assumption.
    let mut rows = conn
        .query(
            "SELECT tag FROM document_tags WHERE source=?1 AND source_id=?2 ORDER BY tag",
            libsql::params![source, source_id],
        )
        .await
        .map_err(sql)?;
    while let Some(row) = rows.next().await.map_err(sql)? {
        let tag = row.get::<String>(0).map_err(sql)?;
        if !value.decisions.iter().any(|d| d.tag == tag) {
            value.decisions.push(TagDecision {
                tag,
                present: true,
                actor: "manual_api".into(),
                decision_id: "legacy_unknown".into(),
            });
        }
    }
    value.decisions.sort_by(|a, b| a.tag.cmp(&b.tag));
    Ok(value)
}
pub(super) async fn record_manual_tag(
    conn: &libsql::Connection,
    source: &str,
    source_id: &str,
    tag: &str,
    present: bool,
) -> Result<(), WenlanError> {
    let mut value = decisions(conn, source, source_id).await?;
    value.revision += 1;
    value.decisions.retain(|d| d.tag != tag);
    value.decisions.push(TagDecision {
        tag: tag.into(),
        present,
        actor: "manual_api".into(),
        decision_id: format!("manual:{}", value.revision),
    });
    save(conn, &key("decisions", &(source, source_id))?, &value).await
}
fn default_rules(space_id: &str) -> OrganizeRules {
    let instructions="Summarize commonalities, differences, limitations and evidence; propose classifications; preserve human decisions and update the existing relevant page.".to_string();
    OrganizeRules {
        space_id: space_id.into(),
        revision: 0,
        name: "General organizing".into(),
        content_digest: digest(&instructions),
        instructions,
    }
}
async fn rules(conn: &libsql::Connection, space_id: &str) -> Result<OrganizeRules, WenlanError> {
    space_name(conn, space_id).await?;
    Ok(load(conn, &key("rules", &space_id)?)
        .await?
        .unwrap_or_else(|| default_rules(space_id)))
}
pub(super) async fn run(
    conn: &libsql::Connection,
    space_id: &str,
    id: &str,
) -> Result<OrganizeRun, WenlanError> {
    space_name(conn, space_id).await?;
    load(conn, &key("run", &(space_id, id))?)
        .await?
        .ok_or_else(|| WenlanError::NotFound("organizing run not found in this Space".into()))
}

async fn list_runs_on(
    conn: &libsql::Connection,
    space_id: &str,
) -> Result<ResearchRunsResponse, WenlanError> {
    space_name(conn, space_id).await?;
    let mut rows = conn
            .query(
                "SELECT value FROM app_metadata WHERE key LIKE ?1 ORDER BY json_extract(value,'$.updated_at') DESC,key LIMIT 100",
                [format!("research_v1:run:{}:%",digest(space_id))],
            )
            .await
            .map_err(sql)?;
    let mut runs = Vec::new();
    while let Some(row) = rows.next().await.map_err(sql)? {
        let value: OrganizeRun = serde_json::from_str(&row.get::<String>(0).map_err(sql)?)?;
        if value.space_id == space_id {
            runs.push(value);
        }
    }
    runs.sort_by_key(|r| std::cmp::Reverse(r.updated_at));
    runs.truncate(100);
    Ok(ResearchRunsResponse { runs })
}

impl MemoryDB {
    pub async fn merge_research_tags(
        &self,
        space_id: &str,
        request: MergeResearchTagsRequest,
    ) -> Result<MergeResearchTagsResponse, WenlanError> {
        let from = request.from_tag.trim().to_lowercase();
        let into = request.into_tag.trim().to_lowercase();
        if from.is_empty()
            || into.is_empty()
            || from == into
            || from.len() > 200
            || into.len() > 200
            || request.sources.is_empty()
            || request.sources.len() > 1000
        {
            return Err(validation("select sources and distinct bounded tags"));
        }
        let conn = self.conn.lock().await;
        conn.execute("BEGIN", ()).await.map_err(sql)?;
        let result = async {
            let mut seen = BTreeSet::new();
            for source in &request.sources {
                if !seen.insert((&source.source, &source.source_id)) {
                    return Err(validation("duplicate merge source"));
                }
                source_snapshot(&conn, space_id, &source.source, &source.source_id).await?;
            }
            for source in &request.sources {
                record_manual_tag(&conn, &source.source, &source.source_id, &from, false).await?;
                record_manual_tag(&conn, &source.source, &source.source_id, &into, true).await?;
                conn.execute(
                    "DELETE FROM document_tags WHERE source=?1 AND source_id=?2 AND tag=?3",
                    libsql::params![
                        source.source.as_str(),
                        source.source_id.as_str(),
                        from.as_str()
                    ],
                )
                .await
                .map_err(sql)?;
                conn.execute(
                    "INSERT OR IGNORE INTO document_tags(source,source_id,tag) VALUES(?1,?2,?3)",
                    libsql::params![
                        source.source.as_str(),
                        source.source_id.as_str(),
                        into.as_str()
                    ],
                )
                .await
                .map_err(sql)?;
            }
            Ok(MergeResearchTagsResponse {
                updated: request.sources.len(),
            })
        }
        .await;
        metadata_json::finish(&conn, result, sql).await
    }
    pub async fn research_rules(&self, space_id: &str) -> Result<OrganizeRules, WenlanError> {
        let conn = self.conn.lock().await;
        rules(&conn, space_id).await
    }
    pub async fn save_research_rules(
        &self,
        space_id: &str,
        request: SaveOrganizeRulesRequest,
    ) -> Result<OrganizeRules, WenlanError> {
        if request.name.trim().is_empty()
            || request.name.len() > 200
            || request.instructions.len() > 32000
        {
            return Err(validation("invalid organizing rules size or name"));
        }
        let conn = self.conn.lock().await;
        conn.execute("BEGIN", ()).await.map_err(sql)?;
        let result = async {
            let old = rules(&conn, space_id).await?;
            if old.revision != request.expected_revision {
                return Err(WenlanError::Conflict(
                    "organizing rules changed; reload".into(),
                ));
            }
            let value = OrganizeRules {
                space_id: space_id.into(),
                revision: old.revision + 1,
                name: request.name.trim().into(),
                content_digest: digest(&request.instructions),
                instructions: request.instructions,
            };
            save(
                &conn,
                &key("rules-version", &(space_id, value.revision))?,
                &value,
            )
            .await?;
            save(&conn, &key("rules", &space_id)?, &value).await?;
            Ok(value)
        }
        .await;
        metadata_json::finish(&conn, result, sql).await
    }
    pub async fn create_research_run(
        &self,
        space_id: &str,
        request: CreateOrganizeRunRequest,
    ) -> Result<OrganizeRun, WenlanError> {
        if request.sources.is_empty()
            || request.sources.len() > 1000
            || request
                .instructions
                .as_ref()
                .is_some_and(|text| text.len() > 32000)
        {
            return Err(validation(
                "select 1..1000 sources and bounded instructions",
            ));
        }
        let conn = self.conn.lock().await;
        conn.execute("BEGIN", ()).await.map_err(sql)?;
        let result = async {
            if space_name(&conn, space_id).await? != request.space {
                return Err(validation("Space name changed; retry in the current Space"));
            }
            let mut rows = conn.query("SELECT COALESCE(description, '') FROM spaces WHERE id=?1", [space_id]).await.map_err(sql)?;
            let description = rows.next().await.map_err(sql)?
                .ok_or_else(|| validation("Space not found"))?
                .get::<String>(0).map_err(sql)?;
            let mut selected = BTreeSet::new();
            let mut items = Vec::new();
            for source in request.sources {
                if !selected.insert((source.source.clone(), source.source_id.clone())) {
                    return Err(validation("duplicate source selection"));
                }
                let (title, content_digest) =
                    source_snapshot(&conn, space_id, &source.source, &source.source_id).await?;
                let current = decisions(&conn, &source.source, &source.source_id).await?;
                let lifecycle_revision = super::citation_provenance::summary_on(
                    &conn,
                    Some(&space_name(&conn, space_id).await?),
                    &source.source,
                    &source.source_id,
                )
                .await?
                .revision;
                items.push(OrganizeRunItem {
                    source_ref: ResearchSourceRef { title, ..source },
                    coverage: ResearchCoverage::Pending,
                    content_digest,
                    decision_revision: current.revision,
                    lifecycle_revision,
                    served_chars: 0,
                    served_total_chars: None,
                    next_offset: None,
                    error: None,
                });
            }
            let mut rules = rules(&conn, space_id).await?;
            if let Some(instructions) = request.instructions {
                rules.content_digest = digest(&instructions);
                rules.instructions = instructions;
                rules.name = "This run".into();
            }
            let (target_page_version, target_page_incarnation) = if let Some(id) = &request.target_page_id {
                let name = space_name(&conn, space_id).await?;
                let mut rows = conn.query(
                    "SELECT version,incarnation FROM pages WHERE id=?1 AND workspace=?2 AND status='active'",
                    libsql::params![id.as_str(), name],
                ).await.map_err(sql)?;
                let row = rows.next().await.map_err(sql)?
                    .ok_or_else(|| validation("target page is outside this Space"))?;
                (Some(row.get::<i64>(0).map_err(sql)?), Some(row.get::<String>(1).map_err(sql)?))
            } else {
                (None, None)
            };
            let now = chrono::Utc::now().timestamp();
            let value = OrganizeRun {
                id: uuid::Uuid::new_v4().to_string(),
                space_id: space_id.into(),
                revision: 1,
                rules,
                research_description: Some(description),
                items,
                target_page_id: request.target_page_id,
                target_page_version,
                target_page_incarnation,
                result_page_version: None,
                pending_revision_id: None,
                state: OrganizeRunState::Prepared,
                created_at: now,
                updated_at: now,
            };
            save(&conn, &key("run", &(space_id, &value.id))?, &value).await?;
            Ok(value)
        }
        .await;
        metadata_json::finish(&conn, result, sql).await
    }
    pub async fn research_run(&self, space_id: &str, id: &str) -> Result<OrganizeRun, WenlanError> {
        let conn = self.conn.lock().await;
        run(&conn, space_id, id).await
    }
    pub async fn research_run_scoped(
        &self,
        space_id: &str,
        name: &str,
        id: &str,
    ) -> Result<OrganizeRun, WenlanError> {
        let conn = self.conn.lock().await;
        if space_name(&conn, space_id).await? != name {
            return Err(validation("Space name changed"));
        }
        run(&conn, space_id, id).await
    }
    pub async fn research_runs(&self, space_id: &str) -> Result<ResearchRunsResponse, WenlanError> {
        let conn = self.conn.lock().await;
        list_runs_on(&conn, space_id).await
    }
    pub async fn research_runs_scoped(
        &self,
        space_id: &str,
        name: &str,
    ) -> Result<ResearchRunsResponse, WenlanError> {
        let conn = self.conn.lock().await;
        if space_name(&conn, space_id).await? != name {
            return Err(validation("Space name changed"));
        }
        list_runs_on(&conn, space_id).await
    }
    /// Return an exact Unicode slice and persist only its contiguous served prefix.
    pub async fn read_organize_source(
        &self,
        space_id: &str,
        id: &str,
        request: ReadOrganizeSourceRequest,
    ) -> Result<ReadOrganizeSourceResponse, WenlanError> {
        if request.limit == 0 || request.limit > 16000 {
            return Err(validation("read limit must be 1..16000 characters"));
        }
        let conn = self.conn.lock().await;
        conn.execute("BEGIN", ()).await.map_err(sql)?;
        let result = async {
            let mut value = run(&conn, space_id, id).await?;
            let name = space_name(&conn, space_id).await?;
            if request.space != name {
                return Err(validation(
                    "read Space does not match registered run identity",
                ));
            }
            if matches!(
                value.state,
                OrganizeRunState::Cancelled
                    | OrganizeRunState::AwaitingReview
                    | OrganizeRunState::Completed
            ) || value.pending_revision_id.is_some()
            {
                return Err(validation("run is not available for source reading"));
            }
            let item = value
                .items
                .iter_mut()
                .find(|item| {
                    item.source_ref.source == request.source
                        && item.source_ref.source_id == request.source_id
                })
                .ok_or_else(|| validation("source outside frozen run selection"))?;
            super::citation_provenance::research_fence_on(
                &conn,
                space_id,
                &request.source,
                &request.source_id,
                item.lifecycle_revision,
                false,
            )
            .await?;
            let (title, current, text) =
                source_snapshot_with_text(&conn, space_id, &request.source, &request.source_id)
                    .await?;
            if current != item.content_digest {
                return Err(WenlanError::Conflict(
                    "source changed; begin a new exact-scope run".into(),
                ));
            }
            let total_chars = text.chars().count();
            if total_chars == 0 {
                return Err(validation(
                    "source has no indexed text; report unavailable instead of complete",
                ));
            }
            if request.offset > item.served_chars || request.offset > total_chars {
                return Err(validation(
                    "read offset must continue the served prefix; gaps are refused",
                ));
            }
            let slice = text
                .chars()
                .skip(request.offset)
                .take(request.limit)
                .collect::<String>();
            let end = request.offset + slice.chars().count();
            let served = item.served_chars.max(end);
            let progress = item.error.is_some()
                || item.next_offset != (served < total_chars).then_some(served)
                || served != item.served_chars
                || item.served_total_chars != Some(total_chars)
                || matches!(
                    item.coverage,
                    ResearchCoverage::Pending
                        | ResearchCoverage::Failed
                        | ResearchCoverage::Unavailable
                );
            item.served_chars = served;
            item.served_total_chars = Some(total_chars);
            if item.coverage != ResearchCoverage::Complete || served != total_chars {
                item.coverage = ResearchCoverage::Reading;
            }
            item.next_offset = (served < total_chars).then_some(served);
            item.error = None;
            if progress {
                value.state = if value.items.iter().any(|i| {
                    matches!(
                        i.coverage,
                        ResearchCoverage::Failed | ResearchCoverage::Unavailable
                    )
                }) {
                    OrganizeRunState::Partial
                } else {
                    OrganizeRunState::InProgress
                };
                value.revision += 1;
                value.updated_at = chrono::Utc::now().timestamp();
                save(&conn, &key("run", &(space_id, id))?, &value).await?;
            }
            let name = space_name(&conn, space_id).await?;
            let source_lifecycle = super::citation_provenance::summary_on(
                &conn,
                Some(&name),
                &request.source,
                &request.source_id,
            )
            .await?;
            Ok(ReadOrganizeSourceResponse {
                run_id: id.into(),
                run_revision: value.revision,
                source: request.source,
                source_id: request.source_id,
                title,
                space: name,
                text: slice,
                content_digest: current,
                source_lifecycle,
                offset: request.offset,
                total_chars,
                next_offset: (end < total_chars).then_some(end),
                complete: end == total_chars,
                served_chars: served,
            })
        }
        .await;
        metadata_json::finish(&conn, result, sql).await
    }
    pub async fn update_research_run(
        &self,
        space_id: &str,
        id: &str,
        request: UpdateOrganizeRunRequest,
    ) -> Result<OrganizeRun, WenlanError> {
        if request.error.as_ref().is_some_and(|s| s.len() > 4000) {
            return Err(validation("coverage error too long"));
        }
        let conn = self.conn.lock().await;
        conn.execute("BEGIN", ()).await.map_err(sql)?;
        let result = async {
            if space_name(&conn, space_id).await? != request.space {
                return Err(validation("Space name changed; retry in the current Space"));
            }
            let mut value = run(&conn, space_id, id).await?;
            if value.revision != request.expected_revision {
                return Err(WenlanError::Conflict(
                    "organizing run changed; reload".into(),
                ));
            }
            if value.pending_revision_id.is_some() {
                return Err(WenlanError::Conflict(
                    "organizing result awaits Review; resolve it before continuing".into(),
                ));
            }
            if request.resume && request.cancel {
                return Err(validation("cannot cancel and resume together"));
            }
            if value.state == OrganizeRunState::Completed
                || (value.state == OrganizeRunState::Cancelled && !request.resume)
            {
                return Err(validation("terminal organizing run cannot be changed"));
            }
            if let Some(coverage) = request.coverage {
                let item = value
                    .items
                    .iter_mut()
                    .find(|i| {
                        Some(&i.source_ref.source) == request.source.as_ref()
                            && Some(&i.source_ref.source_id) == request.source_id.as_ref()
                    })
                    .ok_or_else(|| validation("source is outside frozen run selection"))?;
                if request.content_digest.as_deref() != Some(item.content_digest.as_str()) {
                    return Err(WenlanError::Conflict(
                        "progress does not match the frozen source digest".into(),
                    ));
                }
                if matches!(
                    coverage,
                    ResearchCoverage::Complete | ResearchCoverage::Reading
                ) {
                    super::citation_provenance::research_fence_on(
                        &conn,
                        space_id,
                        &item.source_ref.source,
                        &item.source_ref.source_id,
                        item.lifecycle_revision,
                        false,
                    )
                    .await?;
                    let (_, current) = source_snapshot(
                        &conn,
                        space_id,
                        &item.source_ref.source,
                        &item.source_ref.source_id,
                    )
                    .await?;
                    if current != item.content_digest {
                        return Err(WenlanError::Conflict(
                            "source changed; create a new exact-scope run".into(),
                        ));
                    }
                }
                if coverage == ResearchCoverage::Complete && !complete_receipt(item) {
                    return Err(validation(
                        "complete coverage requires the full server-served source prefix",
                    ));
                }
                if coverage == ResearchCoverage::Complete
                    && (request.next_offset.is_some() || request.error.is_some())
                {
                    return Err(validation(
                        "complete coverage cannot retain continuation or failure",
                    ));
                }
                item.coverage = coverage;
                item.next_offset = request.next_offset;
                item.error = request.error;
            }
            value.state = if request.cancel {
                OrganizeRunState::Cancelled
            } else if value.items.iter().any(|i| {
                matches!(
                    i.coverage,
                    ResearchCoverage::Failed | ResearchCoverage::Unavailable
                )
            }) {
                OrganizeRunState::Partial
            } else {
                OrganizeRunState::InProgress
            };
            value.revision += 1;
            value.updated_at = chrono::Utc::now().timestamp();
            save(&conn, &key("run", &(space_id, id))?, &value).await?;
            Ok(value)
        }
        .await;
        metadata_json::finish(&conn, result, sql).await
    }
    pub async fn research_classification_decisions(
        &self,
        space_id: &str,
        source: &str,
        source_id: &str,
    ) -> Result<SourceClassificationDecisions, WenlanError> {
        let conn = self.conn.lock().await;
        source_snapshot(&conn, space_id, source, source_id).await?;
        decisions(&conn, source, source_id).await
    }
    pub async fn propose_research_classifications(
        &self,
        space_id: &str,
        mut request: ProposeSourceClassificationsRequest,
    ) -> Result<ClassificationProposalResult, WenlanError> {
        if request.changes.is_empty() || request.changes.len() > 1000 {
            return Err(validation(
                "classification changes must contain 1..1000 documents",
            ));
        }
        let conn = self.conn.lock().await;
        conn.execute("BEGIN", ()).await.map_err(sql)?;
        let result = async {
            let value = run(&conn, space_id, &request.run_id).await?;
            if value.state == OrganizeRunState::Cancelled {
                return Err(validation("cancelled run cannot propose changes"));
            }
            let mut identities = BTreeSet::new();
            for change in &mut request.changes {
                if !identities.insert((change.source.clone(), change.source_id.clone())) {
                    return Err(validation("duplicate classification source"));
                }
                let item = value
                    .items
                    .iter()
                    .find(|item| {
                        item.source_ref.source == change.source
                            && item.source_ref.source_id == change.source_id
                    })
                    .ok_or_else(|| validation("classification source outside run selection"))?;
                if item.coverage != ResearchCoverage::Complete
                    || !complete_receipt(item)
                    || item.content_digest != change.source_content_digest
                {
                    return Err(validation(
                        "classification requires complete, unchanged run coverage",
                    ));
                }
                super::citation_provenance::research_fence_on(
                    &conn,
                    space_id,
                    &change.source,
                    &change.source_id,
                    item.lifecycle_revision,
                    false,
                )
                .await?;
                change.title = validate_change(&conn, space_id, change).await?;
                change.before_tags = decisions(&conn, &change.source, &change.source_id)
                    .await?
                    .decisions
                    .into_iter()
                    .filter(|decision| decision.present)
                    .map(|decision| decision.tag)
                    .collect();
            }
            // A new run or explanation must never resurrect a dismissed decision.
            // Only source/rule/decision generations and normalized actions identify it.
            let mut actions = request
                .changes
                .iter()
                .map(|change| {
                    let mut add = change
                        .add_tags
                        .iter()
                        .map(|tag| tag.trim().to_lowercase())
                        .collect::<Vec<_>>();
                    let mut remove = change
                        .remove_tags
                        .iter()
                        .map(|tag| tag.trim().to_lowercase())
                        .collect::<Vec<_>>();
                    add.sort();
                    remove.sort();
                    (
                        &change.source,
                        &change.source_id,
                        &change.source_content_digest,
                        change.expected_decision_revision,
                        value
                            .items
                            .iter()
                            .find(|item| {
                                item.source_ref.source == change.source
                                    && item.source_ref.source_id == change.source_id
                            })
                            .map(|item| item.lifecycle_revision)
                            .unwrap_or(0),
                        add,
                        remove,
                    )
                })
                .collect::<Vec<_>>();
            actions.sort();
            let identity =
                serde_json::to_string(&(space_id, &value.rules.content_digest, actions))?;
            let id = format!("source-classification:{}", digest(&identity));
            let payload = wenlan_types::RefinementPayload::SourceClassification {
                run_id: value.id,
                space_id: space_id.into(),
                rules_digest: value.rules.content_digest,
                changes: request.changes,
            };
            let source_ids = identities.into_iter().map(|(_, id)| id).collect::<Vec<_>>();
            let created = conn
                .execute(
                    "INSERT INTO refinement_queue(id,action,source_ids,payload,confidence,status)
                 VALUES(?1,'source_classification',?2,?3,1.0,'awaiting_review') ON CONFLICT(id) DO NOTHING",
                    libsql::params![
                        id.as_str(),
                        serde_json::to_string(&source_ids)?,
                        serde_json::to_string(&payload)?
                    ],
                )
                .await
                .map_err(sql)?
                > 0;
            conn.execute("UPDATE refinement_queue SET status='awaiting_review' WHERE id=?1 AND action='source_classification' AND status='pending'", [id.as_str()]).await.map_err(sql)?;
            Ok(ClassificationProposalResult { id, created })
        }
        .await;
        metadata_json::finish(&conn, result, sql).await
    }
    /// Narrow dismissal path: scope authorization and terminal transition share one transaction.
    pub async fn reject_research_classification(
        &self,
        id: &str,
        scope: &crate::read_scope::ReadScope,
    ) -> Result<(), WenlanError> {
        let conn = self.conn.lock().await;
        conn.execute("BEGIN", ()).await.map_err(sql)?;
        let result = async {
            let mut rows = conn.query("SELECT payload FROM refinement_queue WHERE id=?1 AND action='source_classification' AND status NOT IN ('resolved','dismissed','auto_applied')", [id]).await.map_err(sql)?;
            let payload = rows.next().await.map_err(sql)?.ok_or_else(|| validation("classification proposal missing or resolved"))?.get::<String>(0).map_err(sql)?;
            drop(rows);
            let wenlan_types::RefinementPayload::SourceClassification {space_id,..} = serde_json::from_str(&payload)? else {return Err(validation("invalid classification proposal"));};
            classification_scope_on(&conn, &space_id, scope).await?;
            conn.execute("UPDATE refinement_queue SET status='dismissed',resolved_at=datetime('now') WHERE id=?1", [id]).await.map_err(sql)?;
            Ok(())
        }.await;
        metadata_json::finish(&conn, result, sql).await
    }
    pub async fn accept_research_classifications(
        &self,
        id: &str,
        actor: &str,
        edited: Option<Vec<SourceClassificationChange>>,
        scope: &crate::read_scope::ReadScope,
    ) -> Result<(), WenlanError> {
        let conn = self.conn.lock().await;
        conn.execute("BEGIN", ()).await.map_err(sql)?;
        let result = async {
            let mut rows = conn.query(
                "SELECT payload FROM refinement_queue WHERE id=?1 AND action='source_classification'
                 AND status NOT IN ('resolved','dismissed','auto_applied')", [id],
            ).await.map_err(sql)?;
            let payload = rows.next().await.map_err(sql)?
                .ok_or_else(|| validation("classification proposal missing or already resolved"))?
                .get::<String>(0).map_err(sql)?;
            drop(rows);
            let wenlan_types::RefinementPayload::SourceClassification {
                space_id, run_id, changes: original, ..
            } = serde_json::from_str(&payload)? else {
                return Err(validation("invalid classification proposal"));
            };
            classification_scope_on(&conn, &space_id, scope).await?;
            let frozen=run(&conn,&space_id,&run_id).await?;
            let changes = edited.unwrap_or_else(|| original.clone());
            let mut seen = BTreeSet::new();
            // Preflight the complete bundle before changing any tag or decision.
            for change in &changes {
                if !seen.insert((&change.source, &change.source_id)) {
                    return Err(validation("duplicate edited source"));
                }
                let base = original.iter().find(|base| {
                    base.source == change.source && base.source_id == change.source_id
                }).ok_or_else(|| validation("edited proposal broadened source scope"))?;
                if base.source_content_digest != change.source_content_digest
                    || base.expected_decision_revision != change.expected_decision_revision
                {
                    return Err(validation("edited proposal changed its acceptance fence"));
                }
                let item=frozen.items.iter().find(|item|item.source_ref.source==base.source && item.source_ref.source_id==base.source_id).ok_or_else(||validation("original classification run source is unavailable"))?;
                if !complete_receipt(item) {return Err(validation("original classification has no complete source-read receipt"));}
                super::citation_provenance::research_fence_on(&conn,&space_id,&base.source,&base.source_id,item.lifecycle_revision,false).await?;
                validate_change(&conn, &space_id, change).await?;
            }
            for change in changes {
                let mut value = decisions(&conn, &change.source, &change.source_id).await?;
                value.revision += 1;
                for (present, tags) in [(false, change.remove_tags), (true, change.add_tags)] {
                    for tag in tags {
                        let tag = tag.trim().to_lowercase();
                        if present {
                            conn.execute(
                                "INSERT OR IGNORE INTO document_tags(source,source_id,tag) VALUES(?1,?2,?3)",
                                libsql::params![change.source.as_str(), change.source_id.as_str(), tag.as_str()],
                            ).await.map_err(sql)?;
                        } else {
                            conn.execute(
                                "DELETE FROM document_tags WHERE source=?1 AND source_id=?2 AND tag=?3",
                                libsql::params![change.source.as_str(), change.source_id.as_str(), tag.as_str()],
                            ).await.map_err(sql)?;
                        }
                        value.decisions.retain(|decision| decision.tag != tag);
                        value.decisions.push(TagDecision {
                            tag, present, actor: actor.into(), decision_id: id.into(),
                        });
                    }
                }
                save(&conn, &key("decisions", &(&change.source, &change.source_id))?, &value).await?;
            }
            conn.execute("UPDATE refinement_queue SET status='resolved' WHERE id=?1", [id])
                .await.map_err(sql)?;
            Ok(())
        }.await;
        metadata_json::finish(&conn, result, sql).await
    }
}
async fn classification_scope_on(
    conn: &libsql::Connection,
    space_id: &str,
    scope: &crate::read_scope::ReadScope,
) -> Result<(), WenlanError> {
    let mut rows = conn
        .query("SELECT name FROM spaces WHERE id=?1", [space_id])
        .await
        .map_err(sql)?;
    let name = rows
        .next()
        .await
        .map_err(sql)?
        .ok_or_else(|| WenlanError::NotFound("classification Space unavailable".into()))?
        .get::<String>(0)
        .map_err(sql)?;
    if !scope.matches(Some(&name)) {
        return Err(WenlanError::NotFound(
            "classification proposal unavailable in this Space".into(),
        ));
    }
    Ok(())
}
async fn validate_change(
    conn: &libsql::Connection,
    space_id: &str,
    change: &SourceClassificationChange,
) -> Result<String, WenlanError> {
    if change.reason.len() > 4000 || change.add_tags.len() + change.remove_tags.len() > 100 {
        return Err(validation("classification change exceeds bounds"));
    }
    let mut tags = BTreeSet::new();
    for tag in change.add_tags.iter().chain(&change.remove_tags) {
        let tag = tag.trim().to_lowercase();
        if tag.is_empty() || tag.len() > 200 || !tags.insert(tag) {
            return Err(validation("tags must be nonempty, distinct and disjoint"));
        }
    }
    let (title, current) =
        source_snapshot(conn, space_id, &change.source, &change.source_id).await?;
    if current != change.source_content_digest {
        return Err(WenlanError::Conflict(
            "classification source changed; reread and propose again".into(),
        ));
    }
    if decisions(conn, &change.source, &change.source_id)
        .await?
        .revision
        != change.expected_decision_revision
    {
        return Err(WenlanError::Conflict(
            "human classification changed; reread decisions and propose again".into(),
        ));
    }
    Ok(title)
}

#[cfg(test)]
#[path = "research_runs_tests.rs"]
mod tests;
