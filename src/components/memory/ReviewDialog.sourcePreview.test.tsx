// SPDX-License-Identifier: AGPL-3.0-only
import { useState } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { i18n } from "../../i18n";
import { getResearchDispute, type ResearchDisputePayload, type ResearchDisputeView } from "../../lib/disputeApi";
import { resolveSourceReference } from "../../lib/sourceLifecycleApi";
import ReviewDialog from "./ReviewDialog";
import { reviewItemId, type ReviewItem } from "./useReviewQueue";

vi.mock("../../lib/disputeApi", async original => ({ ...(await original<typeof import("../../lib/disputeApi")>()), getResearchDispute: vi.fn() }));
vi.mock("../../lib/sourceLifecycleApi", async original => ({ ...(await original<typeof import("../../lib/sourceLifecycleApi")>()), resolveSourceReference: vi.fn() }));
vi.mock("./sources/SourceLifecycleControl", () => ({ SourceLifecycleControl: () => null }));
vi.mock("./sources/SourceDocumentReader", () => ({ SourceDocumentReader: ({ citation }: { citation?: { pdf_target?: { page: number } } }) => <button type="button">Original PDF page {citation?.pdf_target?.page}</button> }));

const digest = "a".repeat(64);
const lifecycle = { state: "active" as const, revision: 0, reason: "" };
const payload: ResearchDisputePayload = { action: "research_dispute", space: "Research", payload_digest: digest, dispute: { space_id: "space-id", kind: "incomparable_conditions", conditions: "Different interventions", reason: "Check conditions", evidence: [
  { source: "file", source_id: "paper-a", source_digest: digest, source_lifecycle: lifecycle, quote: "First finding", pdf_target: { attachment_key: "ABCDEFGH", pdf_digest: digest, page: 1, quote: "First finding" } },
  { source: "file", source_id: "paper-b", source_digest: digest, source_lifecycle: lifecycle, quote: "Second finding" },
] } };
const view: ResearchDisputeView = { id: "dispute-1", space: "Research", status: "awaiting_review", dispute: payload.dispute, payload_digest: digest, decisions: [], source_states: payload.dispute.evidence.map(e => ({ source: e.source, source_id: e.source_id, expected_digest: digest, current_digest: digest, expected_pdf_digest: e.pdf_target?.pdf_digest ?? null, current_pdf_digest: e.pdf_target?.pdf_digest ?? null, expected_lifecycle: lifecycle, current_lifecycle: lifecycle, state: "unchanged" })) };
const item: ReviewItem = { kind: "refinement", id: "dispute-1", action: "research_dispute", sourceIds: ["paper-a", "paper-b"], payload, confidence: 0, timestampMs: 0 };
const resolve = vi.fn();
function Harness({ queryClient }: { queryClient?: QueryClient } = {}) {
  const [openId, setOpenId] = useState<string | null>(reviewItemId(item));
  const [client] = useState(() => queryClient ?? new QueryClient({ defaultOptions: { queries: { retry: false } } }));
  return <QueryClientProvider client={client}><button onClick={() => setOpenId(reviewItemId(item))}>Open review</button><button onClick={() => setOpenId(null)}>Leave review</button><ReviewDialog items={[item]} openId={openId} onOpenChange={setOpenId} onResolve={resolve} isResolving={false} /></QueryClientProvider>;
}
beforeEach(async () => {
  await i18n.changeLanguage("en");
  vi.clearAllMocks();
  vi.mocked(getResearchDispute).mockResolvedValue(view);
  vi.mocked(resolveSourceReference).mockImplementation(async (_space, id) => ({ space_id: "space-id", source: "file", source_id: id, source_agent: "zotero", title: id === "paper-a" ? "First study" : "Second study" }));
});

it("suspends Review while the portaled source is open and restores the decision draft after reader Escape", async () => {
  render(<Harness />);
  expect(await screen.findByText(/First study — Unchanged/)).toBeInTheDocument();
  const review = screen.getByRole("dialog", { name: "Review" });
  fireEvent.change(screen.getByLabelText("Decision"), { target: { value: "not_comparable" } });
  fireEvent.change(screen.getByLabelText("Reason for your decision"), { target: { value: "The interventions differ" } });
  fireEvent.click(screen.getAllByRole("button", { name: "Open source" })[0]);
  const reader = screen.getByRole("button", { name: "Original PDF page 1" });
  expect(review).not.toBeVisible();
  expect(reader).toBeVisible();
  expect(review).not.toContainElement(reader);
  reader.focus();
  fireEvent.keyDown(reader, { key: "Escape" });
  await waitFor(() => expect(screen.queryByRole("button", { name: "Original PDF page 1" })).not.toBeInTheDocument());
  expect(review).toBeVisible();
  expect(screen.getByLabelText("Decision")).toHaveValue("not_comparable");
  expect(screen.getByLabelText("Reason for your decision")).toHaveValue("The interventions differ");
  expect(resolve).not.toHaveBeenCalled();
});

it("cleans up the source suspension when Review is closed externally", async () => {
  render(<Harness />);
  await screen.findByText(/First study — Unchanged/);
  fireEvent.click(screen.getAllByRole("button", { name: "Open source" })[0]);
  fireEvent.click(screen.getByRole("button", { name: "Leave review" }));
  expect(screen.queryByRole("button", { name: "Original PDF page 1" })).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Open review" }));
  expect(await screen.findByRole("dialog", { name: "Review" })).toBeVisible();
  expect(resolve).not.toHaveBeenCalled();
});

it("restores Review if refreshed evidence becomes invalid while its reader is open", async () => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<Harness queryClient={client} />);
  await screen.findByText(/First study — Unchanged/);
  fireEvent.click(screen.getAllByRole("button", { name: "Open source" })[0]);
  await act(async () => { client.setQueryData(["research-dispute", "Research", "dispute-1"], { ...view, space: "Other space" }); });
  await waitFor(() => expect(screen.queryByRole("button", { name: "Original PDF page 1" })).not.toBeInTheDocument());
  expect(screen.getByRole("dialog", { name: "Review" })).toBeVisible();
  expect(screen.getByRole("alert")).toBeVisible();
  expect(resolve).not.toHaveBeenCalled();
});
