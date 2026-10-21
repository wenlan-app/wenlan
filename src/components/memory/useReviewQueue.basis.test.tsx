// SPDX-License-Identifier: AGPL-3.0-only
import type { ReactNode } from "react";
import { describe, expect, it, vi } from "vitest";
import { act, renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { useReviewQueue } from "./useReviewQueue";
import { BASIS_PENDING_QUERY_KEY } from "./page/useBasisActions";

vi.mock("../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../lib/tauri")>()),
  listPendingRevisions: vi.fn().mockResolvedValue([
    {
      target_source_id: "page_a",
      revision_source_id: "mem_a",
      revision_content: "Proposed body",
      source_agent: "claude-code",
      last_modified: 1_760_000_000,
      target_kind: "page",
      review_reason: "locked",
    },
  ]),
  listRefinements: vi.fn().mockResolvedValue({ proposals: [] }),
  listUnconfirmedMemories: vi.fn().mockResolvedValue([]),
  acceptPendingRevision: vi.fn().mockResolvedValue(undefined),
}));

function setup() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  // Seed the "source note changed" group as if the review panel had loaded it.
  client.setQueryData(BASIS_PENDING_QUERY_KEY, []);
  client.setQueryData(["page-basis-diff", "page_a"], {});
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  const hook = renderHook(() => useReviewQueue(true), { wrapper });
  return { client, ...hook };
}

describe("useReviewQueue basis refresh", () => {
  it("refreshes the pending-basis group after accepting a page update", async () => {
    const { client, result } = setup();
    await waitFor(() => expect(result.current.items).toHaveLength(1));
    expect(client.getQueryState(BASIS_PENDING_QUERY_KEY)?.isInvalidated).toBe(false);

    await act(async () => {
      await result.current.resolve({ item: result.current.items[0], approve: true });
    });

    expect(client.getQueryState(BASIS_PENDING_QUERY_KEY)?.isInvalidated).toBe(true);
    expect(client.getQueryState(["page-basis-diff", "page_a"])?.isInvalidated).toBe(true);
  });

  it("refreshes the pending-basis group when the queue is reloaded", async () => {
    const { client, result } = setup();
    await waitFor(() => expect(result.current.items).toHaveLength(1));

    act(() => result.current.refetch());

    expect(client.getQueryState(BASIS_PENDING_QUERY_KEY)?.isInvalidated).toBe(true);
  });
});
