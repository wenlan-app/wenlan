// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen } from "@testing-library/react";
import type { BasisPendingEntry } from "../../../lib/tauri";

vi.mock("../../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../../lib/tauri")>()),
  listPagesExplicitBrowse: vi.fn(),
  listRefinements: vi.fn(),
  listOnboardingMilestones: vi.fn().mockResolvedValue([]),
  listAgents: vi.fn().mockResolvedValue([]),
  listRegisteredSources: vi.fn().mockResolvedValue([]),
  getTruthStatus: vi.fn().mockResolvedValue(null),
  listBasisPending: vi.fn(),
  listCodeBasisPending: vi.fn(),
}));

import { listBasisPending, listPagesExplicitBrowse } from "../../../lib/tauri";
import { PagesOverview } from "./PagesOverview";

const pricing: BasisPendingEntry = {
  basis_page_id: "pricing",
  title: "Pricing",
  state: "pending",
  changed_version: 4,
  citers: [{ page_id: "n1", title: "Launch plan", minor_only: false }],
};

beforeEach(() => {
  vi.mocked(listPagesExplicitBrowse).mockResolvedValue([]);
  vi.mocked(listBasisPending).mockResolvedValue([pricing]);
});

afterEach(() => {
  cleanup();
  vi.mocked(listBasisPending).mockReset();
});

describe("PagesOverview and source updates", () => {
  it("UX-HOME-4 leaves a changed source off Home, for the pages that cite it", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<QueryClientProvider client={client}>
      <PagesOverview
        inventoryScope="all"
        onCreatePage={vi.fn()}
        onSelectDraft={vi.fn()}
        onSelectPage={vi.fn()}
        onSelectSpace={vi.fn()}
      />
    </QueryClientProvider>);

    await screen.findByRole("heading", { level: 1, name: "Open a page" });
    expect(listBasisPending).not.toHaveBeenCalled();
    expect(screen.queryByRole("list", { name: "Source updates" })).toBeNull();
    expect(screen.queryByText(/“Pricing” changed/)).toBeNull();
    expect(screen.queryByRole("button", { name: "Review" })).toBeNull();
  });
});
