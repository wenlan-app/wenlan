// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import type { BasisPendingEntry, DistillReviewResponse } from "../../lib/tauri";

vi.mock("../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../lib/tauri")>()),
  distillReview: vi.fn(),
  listPendingRevisions: vi.fn(),
  listRefinements: vi.fn(),
  listUnconfirmedMemories: vi.fn(),
  listRecentChanges: vi.fn(),
  listBasisPending: vi.fn(),
}));

import {
  distillReview,
  listBasisPending,
  listPendingRevisions,
  listRecentChanges,
  listRefinements,
  listUnconfirmedMemories,
} from "../../lib/tauri";
import DistillReviewPanel from "./DistillReviewPanel";
import { BASIS_UPDATES_REVIEW_ID } from "./page/useBasisActions";

const empty: DistillReviewResponse = {
  pages_created: 0,
  scoped: false,
  created_ids: [],
  pending: [],
  stale_pages: [],
  stale_truncated: false,
  orphan_topics: [],
};

const pricing: BasisPendingEntry = {
  basis_page_id: "pricing",
  title: "Pricing",
  state: "pending",
  changed_version: 4,
  citers: [{ page_id: "note-1", title: "Launch plan", minor_only: false }],
};

function renderPanel(initialReviewItemId?: string) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  render(<QueryClientProvider client={client}>
    <DistillReviewPanel
      initialReviewItemId={initialReviewItemId}
      onBack={vi.fn()}
      onPageClick={vi.fn()}
      onMemoryClick={vi.fn()}
      onOpenActivity={vi.fn()}
    />
  </QueryClientProvider>);
}

beforeEach(() => {
  localStorage.clear();
  vi.mocked(distillReview).mockResolvedValue(empty);
  vi.mocked(listPendingRevisions).mockResolvedValue([]);
  vi.mocked(listRefinements).mockResolvedValue({ proposals: [] });
  vi.mocked(listUnconfirmedMemories).mockResolvedValue([]);
  vi.mocked(listRecentChanges).mockResolvedValue([]);
});

afterEach(() => {
  cleanup();
  vi.mocked(listBasisPending).mockReset();
});

describe("DistillReviewPanel source updates", () => {
  it("shows a group for notes whose source note changed, and is not 'all caught up'", async () => {
    vi.mocked(listBasisPending).mockResolvedValue([pricing]);
    renderPanel();

    expect(await screen.findByRole("region", { name: "Source updates" })).toBeInTheDocument();
    expect(screen.getByText("“Pricing” changed. 1 page to confirm")).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "All caught up" })).toBeNull();
  });

  it("shows no group, and stays 'all caught up', when no note is waiting", async () => {
    vi.mocked(listBasisPending).mockResolvedValue([]);
    renderPanel();

    expect(await screen.findByRole("heading", { name: "All caught up" })).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Source updates" })).toBeNull();
  });

  it("scrolls to the group when Home sent the person here for it", async () => {
    vi.mocked(listBasisPending).mockResolvedValue([pricing]);
    const scroll = vi.fn();
    const original = HTMLElement.prototype.scrollIntoView;
    HTMLElement.prototype.scrollIntoView = scroll;
    try {
      renderPanel(BASIS_UPDATES_REVIEW_ID);
      await screen.findByRole("region", { name: "Source updates" });
      await waitFor(() => expect(scroll).toHaveBeenCalled());
      // The group id is not a queue item, so the queue is not asked for one.
      expect(screen.queryByText(/could not be found/i)).toBeNull();
    } finally {
      HTMLElement.prototype.scrollIntoView = original;
    }
  });
});
