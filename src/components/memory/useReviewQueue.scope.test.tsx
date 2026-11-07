import type { ReactNode } from "react";
import { describe, expect, it, vi } from "vitest";
import { renderHook } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { useReviewQueue, type ReviewItem } from "./useReviewQueue";
const mocks = vi.hoisted(() => ({ accept: vi.fn(), reject: vi.fn(), edited: vi.fn() }));
vi.mock("../../lib/tauri", async original => ({ ...(await original<typeof import("../../lib/tauri")>()), acceptRefinement: mocks.accept, rejectRefinement: mocks.reject, listRefinements: vi.fn().mockResolvedValue({proposals:[]}), listPendingRevisions: vi.fn().mockResolvedValue([]), listUnconfirmedMemories: vi.fn().mockResolvedValue([]) }));
vi.mock("../../lib/researchApi", async original => ({ ...(await original<typeof import("../../lib/researchApi")>()), acceptClassification: mocks.edited }));
const item: ReviewItem = {kind:"refinement",id:"classification",action:"source_classification",sourceIds:[],payload:null,confidence:1,timestampMs:null};
describe("classification review Space", () => {
  it("passes active Space to ordinary acceptance, rejection, and edited classification", async () => {
    const client = new QueryClient({defaultOptions:{queries:{retry:false}}});
    const wrapper = ({children}:{children:ReactNode}) => <QueryClientProvider client={client}>{children}</QueryClientProvider>;
    const {result} = renderHook(() => useReviewQueue(true,"default","Research"),{wrapper});
    const changes = [{source:"file" as const,source_id:"paper",source_content_digest:"a".repeat(64),expected_decision_revision:1,add_tags:["tag"],remove_tags:[],reason:"Reason"}];
    await result.current.resolve({item,approve:true}); await result.current.resolve({item,approve:false}); await result.current.resolve({item,approve:true,classification_changes:changes});
    expect(mocks.accept).toHaveBeenCalledWith("classification","Research"); expect(mocks.reject).toHaveBeenCalledWith("classification","Research"); expect(mocks.edited).toHaveBeenCalledWith("classification",changes,"Research");
  });
});
