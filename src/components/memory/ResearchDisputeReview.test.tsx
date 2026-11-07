import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { decideResearchDispute, getResearchDispute, type ResearchDisputePayload, type ResearchDisputeView } from "../../lib/disputeApi";
import { ResearchDisputeReview } from "./ResearchDisputeReview";
import { resolveSourceReference } from "../../lib/sourceLifecycleApi";
vi.mock("../../lib/disputeApi", async original => ({ ...(await original<typeof import("../../lib/disputeApi")>()), getResearchDispute: vi.fn(), decideResearchDispute: vi.fn() }));
vi.mock("../../lib/sourceLifecycleApi", async original => ({ ...(await original<typeof import("../../lib/sourceLifecycleApi")>()), resolveSourceReference: vi.fn() }));
vi.mock("./sources/SourceDocumentPreview", () => ({ default: ({ file, citation }: {file: {title: string}; citation: {pdf_target?: {page: number}}}) => <div>Reader: {file.title} page {citation.pdf_target?.page}</div> }));
const digest = "a".repeat(64);
const lifecycle = {state:"active" as const, revision:0, reason:""};
const payload: ResearchDisputePayload = { action: "research_dispute", space: "Research", payload_digest: digest, dispute: { space_id: "space-id", kind: "opposing_claims", conditions: "different methods", reason: "findings differ", evidence: [
  { source: "file", source_id: "paper-a", source_digest: digest, source_lifecycle: lifecycle, quote: "First finding" }, { source: "file", source_id: "paper-b", source_digest: digest, source_lifecycle: lifecycle, quote: "Second finding" },
] } };
const view: ResearchDisputeView = { space: "Research", id: "dispute-1", status: "awaiting_review", dispute: payload.dispute, payload_digest: digest, decisions: [], source_states: payload.dispute.evidence.map(evidence => ({ source: evidence.source, source_id: evidence.source_id, expected_digest: digest, current_digest: digest, expected_pdf_digest: null, current_pdf_digest: null, expected_lifecycle:lifecycle, current_lifecycle:lifecycle, state: "unchanged" })) };
function show(value: ResearchDisputePayload | null = payload) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={client}><ResearchDisputeReview id="dispute-1" space="Research" payload={value} /></QueryClientProvider>);
}
beforeEach(() => { vi.resetAllMocks(); vi.mocked(getResearchDispute).mockResolvedValue(view); vi.mocked(resolveSourceReference).mockImplementation(async (_space,id) => ({space_id:"space-id",source:"file",source_id:id,source_agent:"zotero",title:id === "paper-a" ? "First study" : "Second study"})); });
describe("research disagreement review", () => {
  it("requires a decision and reason and sends the observed payload digest", async () => {
    vi.mocked(decideResearchDispute).mockResolvedValue({ ...view, status: "resolved" });
    show();
    expect(await screen.findByText("First finding")).toBeInTheDocument();
    expect(screen.getByText("Second finding")).toBeInTheDocument();
    const button = screen.getByRole("button", { name: "Record decision" });
    expect(button).toBeDisabled();
    fireEvent.change(screen.getByLabelText("Decision"), { target: { value: "not_comparable" } });
    fireEvent.change(screen.getByLabelText("Reason for your decision"), { target: { value: "Methods differ" } });
    fireEvent.click(button);
    expect(await screen.findByText("Decision recorded")).toBeInTheDocument();
    expect(decideResearchDispute).toHaveBeenCalledWith("Research", "dispute-1", digest, "not_comparable", "Methods differ");
    expect(button).toBeDisabled();
  });
  it("disables interpretation decisions when a source changed but permits deferral", async () => {
    vi.mocked(getResearchDispute).mockResolvedValue({ ...view, source_states: view.source_states.map((state, index) => index ? { ...state, state: "changed", current_digest: "b".repeat(64) } : state) });
    show();
    expect(await screen.findByText(/Second study — Changed/)).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "Confirm the difference" })).toBeDisabled();
    expect(screen.getByRole("option", { name: "Defer" })).toBeEnabled();
  });
  it("resolves scoped titles and opens the selected frozen PDF target without exposing source IDs", async () => {
    const target = {attachment_key:"ABCDEFGH",pdf_digest:digest,page:7,quote:"First finding"};
    const dispute = {...payload.dispute,evidence:payload.dispute.evidence.map((item,index) => index === 0 ? {...item,pdf_target:target} : item)};
    vi.mocked(getResearchDispute).mockResolvedValue({...view,dispute,source_states:view.source_states.map((state,index) => index === 0 ? {...state,expected_pdf_digest:digest,current_pdf_digest:digest} : state)});
    show({...payload,dispute});
    expect(await screen.findByText(/First study — Unchanged/)).toBeInTheDocument();
    expect(screen.queryByText(/paper-a/)).not.toBeInTheDocument();
    fireEvent.click(screen.getAllByRole("button",{name:"Open source"})[0]);
    expect(screen.getByText("Reader: First study page 7")).toBeInTheDocument();
    expect(resolveSourceReference).toHaveBeenCalledWith("Research","paper-a");
  });
  it("does not expose foreign or unavailable source identities", async () => {
    vi.mocked(resolveSourceReference).mockImplementation(async (_space,id) => { if(id === "paper-b") throw new Error("missing"); return {space_id:"foreign",source:"file",source_id:id,source_agent:null,title:"Secret title"}; });
    show(); expect(await screen.findByText("First finding")).toBeInTheDocument();
    expect(screen.queryByText("Secret title")).not.toBeInTheDocument(); expect(screen.queryByText(/paper-a|paper-b/)).not.toBeInTheDocument();
    expect(screen.queryByRole("button",{name:"Open source"})).not.toBeInTheDocument();
  });
  it("treats historical and unknown lifecycle as requiring review", async () => {
    vi.mocked(getResearchDispute).mockResolvedValue({...view,source_states:view.source_states.map((state,index) => index === 0 ? {...state,state:"historical",current_lifecycle:{state:"retracted",revision:1,reason:"Retracted"}} : {...state,expected_lifecycle:null})});
    show(); expect(await screen.findByText(/Historical; needs review/)).toBeInTheDocument();
    expect(screen.getByRole("option",{name:"Confirm the difference"})).toBeDisabled();
    expect(screen.getByRole("option",{name:"Close"})).toBeEnabled();
  });
  it("does not read or decide an invalid proposal", () => {
    show({ ...payload, dispute: { ...payload.dispute, evidence: payload.dispute.evidence.slice(0, 1) } });
    expect(screen.getByRole("alert")).toHaveTextContent("incomplete");
    expect(getResearchDispute).not.toHaveBeenCalled();
    expect(decideResearchDispute).not.toHaveBeenCalled();
  });
});
