// SPDX-License-Identifier: AGPL-3.0-only
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { useReviewQueue } from "./useReviewQueue";
import { acceptPendingRevision, listPendingRevisions } from "../../lib/tauri";

function card(id: string, body: string) {
  return {
    target_source_id: "page_a",
    revision_source_id: id,
    revision_content: body,
    source_agent: "claude-code",
    last_modified: 1_760_000_000,
    target_kind: "page",
    review_reason: "edited",
  };
}

vi.mock("../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../lib/tauri")>()),
  listPendingRevisions: vi.fn(),
  listRefinements: vi.fn().mockResolvedValue({ proposals: [] }),
  listUnconfirmedMemories: vi.fn().mockResolvedValue([]),
  acceptPendingRevision: vi.fn(),
}));

function setup() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  return renderHook(() => useReviewQueue(true), { wrapper });
}

const REPLACED = new Error(
  'HTTP 409: {"error":"page_revision_replaced: revision card mem_old for page page_a was replaced by a newer one (mem_new); nothing was applied. Review the newer card."}',
);

describe("useReviewQueue replaced card", () => {
  beforeEach(() => {
    vi.mocked(listPendingRevisions)
      .mockReset()
      .mockResolvedValueOnce([card("mem_old", "Old proposal")] as never)
      .mockResolvedValue([card("mem_new", "Newer proposal")] as never);
    vi.mocked(acceptPendingRevision).mockReset().mockRejectedValue(REPLACED);
  });

  it("accepts by the card's own id, then drops a replaced card and shows the newer one (UX-WHO-2)", async () => {
    const { result } = setup();
    await waitFor(() => expect(result.current.items).toHaveLength(1));

    await act(async () => {
      await result.current.resolve({ item: result.current.items[0], approve: true }).catch(() => undefined);
    });

    // Never the page id, which would resolve to the newer card unseen.
    expect(acceptPendingRevision).toHaveBeenCalledExactlyOnceWith("mem_old");
    await waitFor(() =>
      expect(result.current.items.map((item) => item.kind === "revision" && item.revisionSourceId)).toEqual(["mem_new"]),
    );
  });

  it("drops a replaced card after an edited accept too (UX-WHO-2)", async () => {
    const { result } = setup();
    await waitFor(() => expect(result.current.items).toHaveLength(1));

    await act(async () => {
      await result.current
        .resolve({ item: result.current.items[0], approve: true, edited_content: "My draft" })
        .catch(() => undefined);
    });

    expect(acceptPendingRevision).toHaveBeenCalledExactlyOnceWith("mem_old", {
      expected_content: "Old proposal",
      content: "My draft",
    });
    await waitFor(() =>
      expect(result.current.items.map((item) => item.kind === "revision" && item.revisionSourceId)).toEqual(["mem_new"]),
    );
  });
});
