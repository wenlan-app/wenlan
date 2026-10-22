// SPDX-License-Identifier: Apache-2.0
//! Durable envelopes shared by the CLI outbox writer and daemon drainer.

use crate::{requests::StoreMemoryRequest, BriefUpdateRequest};
use serde::{Deserialize, Serialize};

pub const OUTBOX_SCHEMA: u32 = 1;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct OutboxEnvelope {
    pub schema: u32,
    pub created_at: String,
    pub caller_id: String,
    pub operation_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    #[serde(default)]
    pub reconcile_summary_version: bool,
    /// `<tool>:<external_id>` of the AI session that made the write, sent as
    /// `x-wenlan-session` on replay. Absent in envelopes queued before sessions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(flatten)]
    pub payload: OutboxPayload,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", content = "payload", rename_all = "snake_case")]
pub enum OutboxPayload {
    BriefUpdate(BriefUpdateRequest),
    MemoryStore(StoreMemoryRequest),
}

impl OutboxPayload {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::BriefUpdate(_) => "brief_update",
            Self::MemoryStore(_) => "memory_store",
        }
    }
}

impl OutboxEnvelope {
    /// Return the canonical durable filename for this envelope.
    pub fn file_name(&self, unix_millis: u128) -> String {
        format!(
            "{unix_millis:013}-{}-{}.json",
            self.operation_id,
            self.payload.kind()
        )
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct OutboxDrainReport {
    pub applied: u32,
    pub duplicate: u32,
    pub failed: u32,
    pub remaining: u32,
    pub details: Vec<OutboxDrainDetail>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct OutboxDrainDetail {
    pub file: String,
    pub kind: String,
    pub outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BriefSummaryUpdate, WriteSpaceTarget};

    fn brief_envelope() -> OutboxEnvelope {
        OutboxEnvelope {
            schema: OUTBOX_SCHEMA,
            created_at: "2026-08-16T12:00:00.000Z".into(),
            caller_id: "wenlan-cli".into(),
            operation_id: "brief-op".into(),
            space: Some("demo".into()),
            agent_name: Some("codex".into()),
            reconcile_summary_version: true,
            session: None,
            payload: OutboxPayload::BriefUpdate(BriefUpdateRequest {
                space: "demo".into(),
                caller_id: "wenlan-cli".into(),
                operation_id: "brief-op".into(),
                summary: Some(BriefSummaryUpdate {
                    text: "queued summary".into(),
                    expected_version: 99,
                }),
                mutations: vec![],
            }),
        }
    }

    #[test]
    fn brief_envelope_roundtrips() {
        let envelope = brief_envelope();
        let json = serde_json::to_string(&envelope).unwrap();
        let decoded: OutboxEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, envelope);
        assert_eq!(decoded.payload.kind(), "brief_update");
    }

    #[test]
    fn memory_envelope_roundtrips() {
        let envelope = OutboxEnvelope {
            schema: OUTBOX_SCHEMA,
            created_at: "2026-08-16T12:00:00.000Z".into(),
            caller_id: "wenlan-cli".into(),
            operation_id: "memory-op".into(),
            space: None,
            agent_name: None,
            reconcile_summary_version: false,
            session: None,
            payload: OutboxPayload::MemoryStore(StoreMemoryRequest {
                content: "a queued memory with enough content".into(),
                memory_type: Some("fact".into()),
                space: WriteSpaceTarget::Inherit,
                source_agent: None,
                title: None,
                confidence: None,
                supersedes: None,
                entity: None,
                entity_type: None,
                entity_id: None,
                structured_fields: None,
                retrieval_cue: None,
            }),
        };
        let json = serde_json::to_string(&envelope).unwrap();
        let decoded: OutboxEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, envelope);
        assert_eq!(decoded.payload.kind(), "memory_store");
    }

    #[test]
    fn session_survives_the_roundtrip_and_old_files_still_parse() {
        let mut envelope = brief_envelope();
        envelope.session = Some("claude-code:abc-123".into());
        let json = serde_json::to_value(&envelope).unwrap();
        assert_eq!(json["session"], "claude-code:abc-123");
        let decoded: OutboxEnvelope = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, envelope);

        // An envelope queued before sessions existed has no `session` key, and
        // an unattributed one does not write the key back out.
        let mut old = serde_json::to_value(brief_envelope()).unwrap();
        assert!(old.get("session").is_none());
        old.as_object_mut().unwrap().remove("session");
        let decoded: OutboxEnvelope = serde_json::from_value(old).unwrap();
        assert_eq!(decoded.session, None);
    }

    #[test]
    fn canonical_file_name_is_zero_padded_and_kind_suffixed() {
        assert_eq!(
            brief_envelope().file_name(42),
            "0000000000042-brief-op-brief_update.json"
        );
    }
}
