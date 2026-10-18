// SPDX-License-Identifier: AGPL-3.0-only
import type { ReactNode } from "react";
import { describe, expect, it, vi } from "vitest";
import { renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { PendingRevisionItem } from "../../lib/tauri";
import { useReviewQueue } from "./useReviewQueue";

const mocks = vi.hoisted(() => ({
  listPendingRevisions: vi.fn(),
}));

vi.mock("../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../lib/tauri")>()),
  listPendingRevisions: mocks.listPendingRevisions,
  listRefinements: vi.fn().mockResolvedValue({ proposals: [] }),
  listUnconfirmedMemories: vi.fn().mockResolvedValue([]),
}));

const card = (id: string, over: Partial<PendingRevisionItem> = {}): PendingRevisionItem => ({
  target_source_id: `page_${id}`,
  revision_source_id: `mem_${id}`,
  revision_content: "Proposed body",
  source_agent: "claude-code",
  last_modified: 1_760_000_000,
  target_kind: "page",
  ...over,
});

describe("useReviewQueue revision cards", () => {
  it("carries the daemon's review_reason onto the revision item", async () => {
    mocks.listPendingRevisions.mockResolvedValue([
      card("a", { review_reason: "locked" }),
      card("b"),
      card("c", { review_reason: null }),
    ]);
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const wrapper = ({ children }: { children: ReactNode }) => (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    );
    const { result } = renderHook(() => useReviewQueue(true), { wrapper });
    await waitFor(() => expect(result.current.items).toHaveLength(3));
    const reasons = Object.fromEntries(
      result.current.items.map((item) => [
        item.kind === "revision" ? item.targetSourceId : "",
        item.kind === "revision" ? item.reviewReason : undefined,
      ]),
    );
    expect(reasons).toEqual({ page_a: "locked", page_b: null, page_c: null });
  });
});
