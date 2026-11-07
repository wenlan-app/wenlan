// SPDX-License-Identifier: AGPL-3.0-only
// Browser fixture: all daemon responses come from the test, never live data.
import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { initializeI18n } from "../src/i18n";
import SourceDocumentPreview from "../src/components/memory/sources/SourceDocumentPreview";
import ReviewDialog from "../src/components/memory/ReviewDialog";
import NoteReviewNotice from "../src/components/memory/page/NoteReviewNotice";
import type { Page } from "../src/lib/tauri";
import type { ResearchDisputePayload } from "../src/lib/disputeApi";
import "../src/index.css";
const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
const digest = "a".repeat(64);
const lifecycle = {state:"active" as const, revision:0, reason:""};
const payload: ResearchDisputePayload = { action: "research_dispute", space: "Research", payload_digest: digest, dispute: { space_id: "space-id", kind: "opposing_claims", conditions: "Different study methods", reason: "Findings disagree", evidence: [
  { source: "file", source_id: "paper-a", source_digest: digest, source_lifecycle:lifecycle, quote: "BERT: Pre-training of Deep Bidirectional Transformers for", pdf_target: { attachment_key: "PAPER001", pdf_digest: digest, page: 1, quote: "BERT: Pre-training of Deep Bidirectional Transformers for" } },
  { source: "file", source_id: "paper-b", source_digest: digest, source_lifecycle:lifecycle, quote: "Second source excerpt for comparison." },
] } };
const mode = new URLSearchParams(location.search).get("mode");
void initializeI18n(undefined, { systemLanguages: ["en"] }).then(() => {
  createRoot(document.getElementById("root")!).render(<QueryClientProvider client={client}>{mode?.startsWith("selection")
    ? <main style={{padding:40}}><NoteReviewNotice page={{id:"selection-page",title:"Research",content:"Saved research",summary:null,entity_id:null,domain:null,source_memory_ids:[],version:1,status:"active",created_at:"",last_compiled:"",last_modified:"",stale_reason:"source_updated",refresh_blocked_reason:"zotero_selection_changed"} as Page} onReview={() => {}} onShowSources={() => {}} />{mode === "selection-review" && <ReviewDialog items={[{kind:"revision",id:"selection-page",targetSourceId:"selection-page",targetKind:"page",revisionSourceId:"pending",content:"Proposed research",agent:"fixture",timestampMs:null,reviewReason:"source_selection_changed"}]} openId="revision:selection-page" onOpenChange={() => {}} onResolve={async () => {}} isResolving={false} />}</main>
    : mode === "source"
    ? <SourceDocumentPreview file={{ source: "file", source_id: "paper-a", title: "Study A", chunk_count: 1, last_modified: 0 }} space="Research" onClose={() => {}} />
    : <ReviewDialog items={[{ kind: "refinement", id: "dispute-1", action: "research_dispute", sourceIds: ["paper-a", "paper-b"], payload, confidence: 1, timestampMs: null }]} openId="refinement:dispute-1" onOpenChange={() => {}} onResolve={async () => {}} isResolving={false} />}</QueryClientProvider>);
});
