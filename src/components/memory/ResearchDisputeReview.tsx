// SPDX-License-Identifier: AGPL-3.0-only
import { useLayoutEffect, useState } from "react";
import { useMutation, useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import "./ResearchDisputeReview.css";
import { decideResearchDispute, getResearchDispute, isResearchDisputePayload, type DisputeDecision, type ResearchDisputePayload } from "../../lib/disputeApi";
import { resolveSourceReference } from "../../lib/sourceLifecycleApi";
import type { IndexedFileInfo, PageCitation } from "../../lib/tauri";
import SourceDocumentPreview from "./sources/SourceDocumentPreview";

export function ResearchDisputeReview({ id, space, payload, onSourceOpenChange }: { id: string; space: string; payload: ResearchDisputePayload | null; onSourceOpenChange?: (open: boolean) => void }) {
  const { t } = useTranslation();
  const client = useQueryClient();
  const [decision, setDecision] = useState<DisputeDecision | "">("");
  const [reason, setReason] = useState("");
  const [failed, setFailed] = useState(false);
  const [opened, setOpened] = useState<{ file: IndexedFileInfo; citation: PageCitation } | null>(null);
  const valid = isResearchDisputePayload(payload) && !!id && !!space;
  const query = useQuery({ queryKey: ["research-dispute", space, id], queryFn: () => getResearchDispute(space, id), enabled: valid, retry: false });
  const current = query.data;
  const sourceQueries = useQueries({ queries: (Array.isArray(current?.dispute?.evidence) ? current.dispute.evidence : []).filter(evidence => evidence && typeof evidence.source_id === "string").map(evidence => ({ queryKey: ["dispute-source", space, evidence.source, evidence.source_id], queryFn: () => resolveSourceReference(space, evidence.source_id), enabled: !!space && !!evidence.source_id, retry: false })) });
  const currentValid = current && isResearchDisputePayload({ action: "research_dispute", space: current.space, dispute: current.dispute, payload_digest: current.payload_digest }) && current.id === id && current.space === space && current.dispute.space_id === payload?.dispute.space_id && ["awaiting_review", "resolved"].includes(current.status) && Array.isArray(current.decisions) && current.decisions.every(event => !!event && ["confirm_difference", "not_comparable", "correct_interpretation", "defer", "close"].includes(event.decision) && typeof event.reason === "string" && !!event.reason.trim() && typeof event.actor === "string" && Number.isFinite(event.decided_at) && Array.isArray(event.source_states)) && Array.isArray(current.source_states) && current.source_states.length === current.dispute.evidence.length && current.source_states.every((state, index) => {
    const evidence = current.dispute.evidence[index];
    return !!state && state.source === evidence.source && state.source_id === evidence.source_id && state.expected_digest === evidence.source_digest && state.expected_pdf_digest === (evidence.pdf_target?.pdf_digest ?? null) && (state.current_digest === null || typeof state.current_digest === "string") && (state.current_pdf_digest === null || typeof state.current_pdf_digest === "string") && ["unchanged", "changed", "unavailable", "historical"].includes(state.state) && (state.state !== "unchanged" || (state.current_digest === state.expected_digest && state.current_pdf_digest === state.expected_pdf_digest));
  });
  const lifecycleCurrent = (state: NonNullable<typeof current>["source_states"][number], index: number) => {
    const expected = state.expected_lifecycle, now = state.current_lifecycle, original = current?.dispute.evidence[index]?.source_lifecycle;
    return expected?.state === "active" && now?.state === "active" && original?.state === "active" && Number.isSafeInteger(expected.revision) && expected.revision === now.revision && expected.revision === original.revision && typeof expected.reason === "string" && expected.reason === now.reason && expected.reason === original.reason;
  };
  const evidenceChanged = !!current?.source_states?.some((state, index) => state.state !== "unchanged" || !lifecycleCurrent(state, index));
  const choiceAllowed = !!decision && (!evidenceChanged || ["defer", "close"].includes(decision));
  const save = useMutation({ mutationFn: () => decideResearchDispute(space, id, current!.payload_digest, decision as DisputeDecision, reason.trim()),
    onSuccess: async result => { setFailed(false); setReason(""); setDecision(""); client.setQueryData(["research-dispute", space, id], result); await client.invalidateQueries({ queryKey: ["refinement-proposals"] }); },
    onError: async () => { setFailed(true); await query.refetch(); },
  });
  const sourceVisible = opened !== null && valid && !query.isPending && !query.isError && !!currentValid;
  // Keep this review and its unsaved decision mounted while the portaled
  // source reader owns the screen. Cleanup also releases an external close.
  useLayoutEffect(() => {
    onSourceOpenChange?.(sourceVisible);
    return () => onSourceOpenChange?.(false);
  }, [sourceVisible, onSourceOpenChange]);
  if (!valid) return <p role="alert">{t("researchDispute.invalid")}</p>;
  if (query.isPending) return <p role="status">{t("researchDispute.loading")}</p>;
  if (query.isError || !currentValid) return <p role="alert">{t("researchDispute.loadFailed")} <button type="button" className="page-editor-action" onClick={() => void query.refetch()}>{t("sourceAccess.retry")}</button></p>;
  return <section className="research-dispute-review">
    <p><strong>{t(`researchDispute.${current.dispute.kind}`)}</strong></p>
    <p>{t("researchDispute.conditions")}: {current.dispute.conditions}</p>
    <p>{t("researchDispute.reason")}: {current.dispute.reason}</p>
    <ol>{current.dispute.evidence.map((evidence, index) => {
      const source = sourceQueries[index];
      const resolved = source?.isSuccess && !source.isFetching && source.data.space_id === current.dispute.space_id && source.data.source === evidence.source && source.data.source_id === evidence.source_id ? source.data : null;
      const title = resolved?.title?.trim() && resolved.title !== evidence.source_id ? resolved.title : t("researchDispute.sourceTitleUnavailable");
      const citation: PageCitation = { occurrence: 1, marker: 1, source_kind: "memory", locator: evidence.source_id, score: 0, status: "unevaluated", scope: "sentence", pdf_target: evidence.pdf_target };
      const status = current.source_states[index];
      const stateLabel = status.state === "unchanged" && !lifecycleCurrent(status, index) ? "source_changed" : `source_${status.state}` as const;
      return <li key={`${evidence.source}:${evidence.source_id}:${index}`}><span>{title} — {t(`researchDispute.${stateLabel}`)}</span>{resolved && <button type="button" className="page-editor-action" onClick={() => setOpened({ file: { source: resolved.source, source_id: resolved.source_id, source_agent: resolved.source_agent, title, chunk_count: 0, last_modified: 0 }, citation })}>{t("researchDispute.openSource")}</button>}<blockquote>{evidence.quote}</blockquote></li>;
    })}</ol>
    {opened && <SourceDocumentPreview file={opened.file} space={space} citation={opened.citation} onClose={() => setOpened(null)} />}
    <label>{t("researchDispute.decision")}<select aria-label={t("researchDispute.decision")} value={decision} onChange={event => setDecision(event.target.value as DisputeDecision | "")} disabled={save.isPending || current.status !== "awaiting_review"}>
      <option value="">{t("researchDispute.choose")}</option>
      {(["confirm_difference", "not_comparable", "correct_interpretation", "defer", "close"] as const).map(value => <option key={value} value={value} disabled={evidenceChanged && !["defer", "close"].includes(value)}>{t(`researchDispute.${value}`)}</option>)}
    </select></label>
    <label>{t("researchDispute.decisionReason")}<textarea aria-label={t("researchDispute.decisionReason")} value={reason} maxLength={4000} onChange={event => setReason(event.target.value)} disabled={save.isPending || current.status !== "awaiting_review"} /></label>
    {failed && <p role="alert">{t("researchDispute.saveFailed")}</p>}
    <button type="button" className="page-editor-action" disabled={!choiceAllowed || !reason.trim() || save.isPending || current.status !== "awaiting_review"} onClick={() => save.mutate()}>{t("researchDispute.recordDecision")}</button>
    {current.decisions.length > 0 && <div><strong>{t("researchDispute.history")}</strong><ul>{current.decisions.map((event, index) => <li key={`${event.decided_at}:${index}`}>{t(`researchDispute.${event.decision}`)} — {event.reason}</li>)}</ul></div>}
    {current.status === "resolved" && <p role="status">{t("researchDispute.resolved")}</p>}
    <p>{t("researchDispute.noAutomaticEdit")}</p>
  </section>;
}
