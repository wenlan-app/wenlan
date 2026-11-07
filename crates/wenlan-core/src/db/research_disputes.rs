// SPDX-License-Identifier: Apache-2.0
//! Disputes remain Review proposals. Resolution never writes a Page.
use super::{citation_provenance, metadata_json, research_runs, MemoryDB};
use crate::{read_scope::ReadScope, WenlanError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use wenlan_types::source_lifecycle::SourceLifecycleState;
use wenlan_types::{pages::PageCitation, research_disputes::*};

fn sql(error: libsql::Error) -> WenlanError {
    WenlanError::VectorDb(error.to_string())
}
fn invalid(message: &str) -> WenlanError {
    WenlanError::Validation(message.into())
}
fn stale() -> WenlanError {
    WenlanError::Conflict(
        "research_dispute_changed: reload the proposal and source states before deciding".into(),
    )
}
fn normalized(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
fn digest(value: &impl Serialize) -> Result<String, WenlanError> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)?)))
}
fn decision_key(id: &str) -> String {
    format!("research_v1:dispute_decisions:{id}")
}

#[derive(Serialize, Deserialize)]
struct StoredDispute {
    action: String,
    dispute: ResearchDispute,
    payload_digest: String,
}
#[derive(Serialize, Deserialize)]
struct DecisionHistory {
    /// The original immutable source quotes and rationale survive queue resolution.
    dispute: ResearchDispute,
    payload_digest: String,
    events: Vec<ResearchDisputeDecisionEvent>,
}

fn canonicalize(mut dispute: ResearchDispute) -> Result<(ResearchDispute, String), WenlanError> {
    dispute.conditions = normalized(&dispute.conditions);
    dispute.reason = dispute.reason.trim().to_owned();
    if dispute.space_id.is_empty()
        || dispute.conditions.is_empty()
        || dispute.conditions.len() > 2000
        || dispute.reason.is_empty()
        || dispute.reason.len() > 4000
        || !(2..=8).contains(&dispute.evidence.len())
    {
        return Err(invalid(
            "dispute requires bounded conditions, reason and two to eight source passages",
        ));
    }
    let mut identified = Vec::new();
    for mut evidence in dispute.evidence {
        evidence.quote = normalized(&evidence.quote);
        if !matches!(evidence.source.as_str(), "file" | "webpage")
            || evidence.source_id.is_empty()
            || evidence.source_id.len() > 4096
            || evidence.source_digest.len() != 64
            || !evidence
                .source_digest
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
            || evidence.quote.is_empty()
            || evidence.quote.len() > 16_000
        {
            return Err(invalid("dispute evidence requires exact source identities, SHA256 digests and bounded original quotes"));
        }
        if let Some(target) = &mut evidence.pdf_target {
            target.quote = normalized(&target.quote);
            if target.quote != evidence.quote {
                return Err(invalid("PDF target quote must equal the evidence passage"));
            }
        }
        // Presentation never reopens a case; PDF and lifecycle generations do.
        let identity = serde_json::to_string(&(
            &evidence.source,
            &evidence.source_id,
            &evidence.source_digest,
            &evidence.quote,
            &evidence.pdf_target,
            evidence
                .source_lifecycle
                .as_ref()
                .map(|s| (s.state, s.revision)),
        ))?;
        identified.push((identity, evidence));
    }
    identified.sort_by(|a, b| a.0.cmp(&b.0));
    if identified.windows(2).any(|pair| pair[0].0 == pair[1].0) {
        return Err(invalid("duplicate dispute evidence passage"));
    }
    let identities: Vec<_> = identified.iter().map(|(id, _)| id).collect();
    let id = format!(
        "research-dispute-{}",
        digest(&(
            &dispute.space_id,
            &dispute.kind,
            &dispute.conditions,
            &identities
        ))?
    );
    dispute.evidence = identified.into_iter().map(|(_, e)| e).collect();
    Ok((dispute, id))
}
fn pdf_citations(dispute: &ResearchDispute) -> Vec<PageCitation> {
    dispute
        .evidence
        .iter()
        .enumerate()
        .filter(|(_, e)| e.pdf_target.is_some())
        .map(|(i, e)| PageCitation {
            occurrence: (i + 1) as u32,
            marker: (i + 1) as u32,
            source_kind: "memory".into(),
            locator: e.source_id.clone(),
            score: 0.0,
            status: "unevaluated".into(),
            scope: "sentence".into(),
            cited_version: None,
            cited_digest: None,
            disproven: false,
            historical: false,
            pdf_target: e.pdf_target.clone(),
        })
        .collect()
}
async fn load_on(
    conn: &libsql::Connection,
    space_id: &str,
    id: &str,
) -> Result<(StoredDispute, String, String), WenlanError> {
    let mut rows = conn
        .query(
            "SELECT payload,status FROM refinement_queue WHERE id=?1 AND action='research_dispute'",
            [id],
        )
        .await
        .map_err(sql)?;
    let row = rows
        .next()
        .await
        .map_err(sql)?
        .ok_or_else(|| WenlanError::NotFound("dispute not found in this Space".into()))?;
    let raw = row.get::<String>(0).map_err(sql)?;
    let status = row.get::<String>(1).map_err(sql)?;
    let stored: StoredDispute = serde_json::from_str(&raw)?;
    if stored.action != "research_dispute" || stored.dispute.space_id != space_id {
        return Err(WenlanError::NotFound(
            "dispute not found in this Space".into(),
        ));
    }
    if digest(&stored.dispute)? != stored.payload_digest {
        return Err(stale());
    }
    Ok((stored, status, raw))
}
async fn history_on(
    conn: &libsql::Connection,
    id: &str,
) -> Result<Option<DecisionHistory>, WenlanError> {
    metadata_json::load(conn, &decision_key(id), sql).await
}
async fn states_on(
    conn: &libsql::Connection,
    dispute: &ResearchDispute,
) -> Result<Vec<ResearchDisputeSourceState>, WenlanError> {
    let name = research_runs::space_name(conn, &dispute.space_id).await?;
    let scope = ReadScope::Space(name.clone());
    let mut states = Vec::new();
    for evidence in &dispute.evidence {
        let current_digest = match research_runs::source_snapshot(
            conn,
            &dispute.space_id,
            &evidence.source,
            &evidence.source_id,
        )
        .await
        {
            Ok((_, digest)) => Some(digest),
            Err(WenlanError::Validation(_) | WenlanError::NotFound(_)) => None,
            Err(error) => return Err(error),
        };
        let expected_pdf_digest = evidence
            .pdf_target
            .as_ref()
            .map(|target| target.pdf_digest.clone());
        let current_pdf_digest = if let Some(target) = &evidence.pdf_target {
            crate::pdf_citations::current_pdf_digest(
                &evidence.source_id,
                &target.attachment_key,
                &scope,
            )
            .await?
        } else {
            None
        };
        let current_lifecycle = if current_digest.is_some() {
            Some(
                citation_provenance::summary_on(
                    conn,
                    Some(&name),
                    &evidence.source,
                    &evidence.source_id,
                )
                .await?,
            )
        } else {
            None
        };
        let state = if current_digest.is_none() {
            ResearchEvidenceState::Unavailable
        } else if current_lifecycle
            .as_ref()
            .is_some_and(|s| s.state != SourceLifecycleState::Active)
        {
            ResearchEvidenceState::Historical
        } else if evidence.source_lifecycle.is_none()
            || evidence
                .source_lifecycle
                .as_ref()
                .map(|s| (s.state, s.revision))
                != current_lifecycle.as_ref().map(|s| (s.state, s.revision))
            || current_digest.as_ref() != Some(&evidence.source_digest)
            || current_pdf_digest != expected_pdf_digest
        {
            ResearchEvidenceState::Changed
        } else {
            ResearchEvidenceState::Unchanged
        };
        states.push(ResearchDisputeSourceState {
            source: evidence.source.clone(),
            source_id: evidence.source_id.clone(),
            expected_digest: evidence.source_digest.clone(),
            current_digest,
            expected_pdf_digest,
            current_pdf_digest,
            expected_lifecycle: evidence.source_lifecycle.clone(),
            current_lifecycle,
            state,
        });
    }
    Ok(states)
}
async fn validate_quotes_on(
    conn: &libsql::Connection,
    dispute: &ResearchDispute,
) -> Result<(), WenlanError> {
    let name = research_runs::space_name(conn, &dispute.space_id).await?;
    let scope = ReadScope::Space(name);
    for evidence in &dispute.evidence {
        let (_, current) = research_runs::source_snapshot(
            conn,
            &dispute.space_id,
            &evidence.source,
            &evidence.source_id,
        )
        .await?;
        if current != evidence.source_digest {
            return Err(stale());
        }
        let source = super::citation_sources::read_on(conn, &evidence.source_id, &scope)
            .await?
            .ok_or_else(stale)?;
        if !normalized(&source.full_text).contains(&evidence.quote) && evidence.pdf_target.is_none()
        {
            return Err(invalid("evidence quote is absent from the exact indexed source; provide a validated original PDF target"));
        }
    }
    Ok(())
}
impl MemoryDB {
    pub async fn propose_research_dispute(
        &self,
        space_id: &str,
        request: ProposeResearchDisputeRequest,
    ) -> Result<ResearchDisputeProposalResult, WenlanError> {
        let registered = self
            .get_space(&request.space)
            .await?
            .filter(|space| space.id == space_id)
            .ok_or_else(|| invalid("registered dispute Space must match request"))?;
        let (mut dispute, _) = canonicalize(ResearchDispute {
            space_id: space_id.into(),
            kind: request.kind,
            conditions: request.conditions,
            reason: request.reason,
            evidence: request.evidence,
        })?;
        crate::pdf_citations::validate_pdf_targets(
            &pdf_citations(&dispute),
            &ReadScope::Space(registered.name),
        )
        .await?;
        let conn = self.conn.lock().await;
        conn.execute("BEGIN", ()).await.map_err(sql)?;
        let result=async {
            if research_runs::space_name(&conn,space_id).await? != request.space {return Err(stale());}
            // Capture authoritative state under the same writer transaction as the proposal.
            // Caller-supplied lifecycle metadata never establishes evidence freshness.
            for evidence in &mut dispute.evidence {
                evidence.source_lifecycle = Some(citation_provenance::summary_on(&conn, Some(&request.space), &evidence.source, &evidence.source_id).await?);
            }
            let (dispute, id) = canonicalize(dispute)?;
            validate_quotes_on(&conn,&dispute).await?;
            if states_on(&conn,&dispute).await?.iter().any(|s| !matches!(s.state, ResearchEvidenceState::Unchanged | ResearchEvidenceState::Historical)) {return Err(stale());}
            let payload_digest = digest(&dispute)?;
            let stored = StoredDispute { action: "research_dispute".into(), dispute, payload_digest };
            let raw = serde_json::to_string(&stored)?;
            let source_ids: Vec<_> = stored.dispute.evidence.iter().map(|e| &e.source_id).collect();
            let created=conn.execute("INSERT INTO refinement_queue(id,action,source_ids,payload,confidence,status) VALUES(?1,'research_dispute',?2,?3,0.0,'awaiting_review') ON CONFLICT(id) DO NOTHING",libsql::params![id.as_str(),serde_json::to_string(&source_ids)?,raw]).await.map_err(sql)?>0;
            let (original,_,_)=load_on(&conn,space_id,&id).await?;
            Ok(ResearchDisputeProposalResult{id:id.clone(),created,payload_digest:original.payload_digest})
        }.await;
        metadata_json::finish(&conn, result, sql).await
    }
    pub async fn research_dispute(
        &self,
        space_id: &str,
        id: &str,
    ) -> Result<ResearchDisputeView, WenlanError> {
        let conn = self.conn.lock().await;
        let space = research_runs::space_name(&conn, space_id).await?;
        let (stored, status, _) = load_on(&conn, space_id, id).await?;
        let source_states = states_on(&conn, &stored.dispute).await?;
        let decisions = history_on(&conn, id)
            .await?
            .map(|h| h.events)
            .unwrap_or_default();
        Ok(ResearchDisputeView {
            space,
            id: id.into(),
            status,
            dispute: stored.dispute,
            payload_digest: stored.payload_digest,
            source_states,
            decisions,
        })
    }
    pub async fn decide_research_dispute(
        &self,
        space_id: &str,
        id: &str,
        request: DecideResearchDisputeRequest,
        actor: &str,
    ) -> Result<ResearchDisputeView, WenlanError> {
        let reason = request.reason.trim();
        if actor.trim().is_empty() || actor.len() > 200 {
            return Err(invalid("decision actor is invalid"));
        }
        if reason.is_empty() || reason.len() > 4000 {
            return Err(invalid("dispute decision requires a bounded reason"));
        }
        let before = self.research_dispute(space_id, id).await?;
        if before.payload_digest != request.expected_payload_digest {
            return Err(stale());
        }
        // Parsing is outside the DB writer lock. Immutable identity is rechecked below.
        if request.decision.confirms_evidence() {
            let registered = self
                .get_space(&request.space)
                .await?
                .filter(|space| space.id == space_id)
                .ok_or_else(stale)?;
            crate::pdf_citations::validate_pdf_targets(
                &pdf_citations(&before.dispute),
                &ReadScope::Space(registered.name),
            )
            .await
            .map_err(|_| stale())?;
        }
        let conn = self.conn.lock().await;
        conn.execute("BEGIN", ()).await.map_err(sql)?;
        let result=async {
            if research_runs::space_name(&conn,space_id).await?!=request.space {return Err(stale());}
            let (stored,status,raw)=load_on(&conn,space_id,id).await?;
            if stored.payload_digest!=request.expected_payload_digest || status!="awaiting_review" {return Err(stale());}
            let source_states=states_on(&conn,&stored.dispute).await?;
            if request.decision.confirms_evidence() {
                if source_states.iter().any(|state|state.state!=ResearchEvidenceState::Unchanged){return Err(stale());}
                validate_quotes_on(&conn,&stored.dispute).await?;
            }
            let mut history=history_on(&conn,id).await?.unwrap_or_else(||DecisionHistory{dispute:stored.dispute.clone(),payload_digest:stored.payload_digest.clone(),events:Vec::new()});
            if history.payload_digest!=stored.payload_digest || history.dispute!=stored.dispute {return Err(stale());}
            let repeat_defer=request.decision==ResearchDisputeDecision::Defer && history.events.last().is_some_and(|event|event.decision==ResearchDisputeDecision::Defer && event.reason==reason && event.source_states==source_states);
            if !repeat_defer {
                history.events.push(ResearchDisputeDecisionEvent{decision:request.decision.clone(),reason:reason.into(),actor:actor.into(),decided_at:chrono::Utc::now().timestamp(),source_states:source_states.clone()});
                research_runs::save(&conn,&decision_key(id),&history).await?;
            }
            let final_status=if request.decision==ResearchDisputeDecision::Defer {status} else {
                let changed=conn.execute("UPDATE refinement_queue SET status='resolved' WHERE id=?1 AND action='research_dispute' AND status='awaiting_review' AND payload=?2",libsql::params![id,raw]).await.map_err(sql)?;
                if changed!=1{return Err(stale());} "resolved".into()
            };
            Ok(ResearchDisputeView{space:request.space.clone(),id:id.into(),status:final_status,dispute:stored.dispute,payload_digest:stored.payload_digest,source_states,decisions:history.events})
        }.await;
        metadata_json::finish(&conn, result, sql).await
    }
}

#[cfg(test)]
#[path = "research_disputes_tests.rs"]
mod tests;
