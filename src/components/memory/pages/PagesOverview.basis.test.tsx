// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
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
}));

import { listBasisPending, listPagesExplicitBrowse } from "../../../lib/tauri";
import { PagesOverview } from "./PagesOverview";

const pricing: BasisPendingEntry = {
  basis_page_id: "pricing",
  title: "Pricing",
  state: "pending",
  changed_version: 4,
  citers: Array.from({ length: 20 }, (_, index) => ({ page_id: `n${index}`, title: `Note ${index}`, minor_only: false })),
};

function renderOverview(onOpenBasisUpdates?: () => void) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<QueryClientProvider client={client}>
    <PagesOverview
      inventoryScope="all"
      onCreatePage={vi.fn()}
      onSelectDraft={vi.fn()}
      onSelectPage={vi.fn()}
      onSelectSpace={vi.fn()}
      onOpenBasisUpdates={onOpenBasisUpdates}
    />
  </QueryClientProvider>);
}

beforeEach(() => {
  vi.mocked(listPagesExplicitBrowse).mockResolvedValue([]);
});

afterEach(() => {
  cleanup();
  vi.mocked(listBasisPending).mockReset();
});

describe("PagesOverview source-update summary", () => {
  it("shows one line per changed source note and leads to the review group", async () => {
    vi.mocked(listBasisPending).mockResolvedValue([pricing]);
    const onOpen = vi.fn();
    renderOverview(onOpen);

    const list = await screen.findByRole("list", { name: "Source updates" });
    expect(list).toHaveTextContent("“Pricing” changed. 20 pages to confirm");
    // The notice sits at the top of the pane, above the reading-start panel.
    const heading = screen.getByRole("heading", { level: 1, name: "Open a page" });
    expect(list.compareDocumentPosition(heading) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    fireEvent.click(within(list).getByRole("button", { name: "Review" }));
    expect(onOpen).toHaveBeenCalledTimes(1);
  });

  it("shows nothing, and asks for nothing, when no destination is wired", async () => {
    vi.mocked(listBasisPending).mockResolvedValue([pricing]);
    renderOverview(undefined);

    await screen.findByRole("heading", { level: 1, name: "Open a page" });
    expect(listBasisPending).not.toHaveBeenCalled();
    expect(screen.queryByRole("list", { name: "Source updates" })).toBeNull();
  });

  it("shows nothing when no note is waiting", async () => {
    vi.mocked(listBasisPending).mockResolvedValue([]);
    renderOverview(vi.fn());

    await waitFor(() => expect(listBasisPending).toHaveBeenCalled());
    await screen.findByRole("heading", { level: 1, name: "Open a page" });
    expect(screen.queryByRole("list", { name: "Source updates" })).toBeNull();
  });
});
