// SPDX-License-Identifier: AGPL-3.0-only
import { invokeDaemonCommand as invoke } from "./tauri";
import type { PdfCitationTarget } from "./tauri";
export type DisputeDecision = "confirm_difference" | "not_comparable" | "correct_interpretation" | "defer" | "close";
export interface DisputeLifecycle { state: "active" | "excluded" | "retracted" | "removed"; revision: number; reason: string }
export interface DisputeEvidence { source: "file" | "webpage"; source_id: string; source_digest: string; quote: string; pdf_target?: PdfCitationTarget; source_lifecycle?: DisputeLifecycle | null }
export interface ResearchDispute { space_id: string; kind: "opposing_claims" | "incomparable_conditions" | "different_definitions" | "insufficient_evidence"; conditions: string; reason: string; evidence: DisputeEvidence[] }
export interface ResearchDisputePayload { action: "research_dispute"; space: string; dispute: ResearchDispute; payload_digest: string }
export interface DisputeSourceState { source: "file" | "webpage"; source_id: string; expected_digest: string; current_digest: string | null; expected_pdf_digest: string | null; current_pdf_digest: string | null; expected_lifecycle?: DisputeLifecycle | null; current_lifecycle?: DisputeLifecycle | null; state: "unchanged" | "changed" | "unavailable" | "historical" }
export interface ResearchDisputeView { space: string; id: string; status: "awaiting_review" | "resolved"; dispute: ResearchDispute; payload_digest: string; source_states: DisputeSourceState[]; decisions: { decision: DisputeDecision; reason: string; actor: string; decided_at: number; source_states: DisputeSourceState[] }[] }

export const getResearchDispute = (space: string, id: string) => invoke<ResearchDisputeView>("get_research_dispute", { space, id });
export const decideResearchDispute = (space: string, id: string, expectedPayloadDigest: string, decision: DisputeDecision, reason: string) => invoke<ResearchDisputeView>("decide_research_dispute", { space, id, expectedPayloadDigest, decision, reason });

const nonempty = (value: unknown): value is string => typeof value === "string" && !!value.trim();
const digest = (value: unknown): boolean => typeof value === "string" && /^[0-9a-f]{64}$/i.test(value);
export function isResearchDisputePayload(value: unknown): value is ResearchDisputePayload {
  if (!value || typeof value !== "object") return false;
  const payload = value as Partial<ResearchDisputePayload>;
  const item = payload.dispute;
  if (payload.action !== "research_dispute" || !nonempty(payload.space) || !digest(payload.payload_digest) || !item || !nonempty(item.space_id) || !["opposing_claims", "incomparable_conditions", "different_definitions", "insufficient_evidence"].includes(item.kind) || !nonempty(item.conditions) || !nonempty(item.reason) || !Array.isArray(item.evidence) || item.evidence.length < 2 || item.evidence.length > 8) return false;
  return item.evidence.every(evidence => !!evidence && ["file", "webpage"].includes(evidence.source) && nonempty(evidence.source_id) && digest(evidence.source_digest) && nonempty(evidence.quote) && (!evidence.pdf_target || (nonempty(evidence.pdf_target.attachment_key) && digest(evidence.pdf_target.pdf_digest) && Number.isSafeInteger(evidence.pdf_target.page) && evidence.pdf_target.page > 0 && nonempty(evidence.pdf_target.quote))));
}
