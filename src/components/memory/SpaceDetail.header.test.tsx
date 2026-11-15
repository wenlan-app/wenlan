// SPDX-License-Identifier: AGPL-3.0-only
import { describe, it, expect, vi, beforeEach } from "vitest";
import { cloneElement } from "react";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

vi.mock("../../lib/tauri", () => ({
  getSpace: vi.fn(),
  listMemoriesRich: vi.fn(),
  listEntities: vi.fn(),
  listPages: vi.fn().mockResolvedValue([]),
  getNurtureCards: vi.fn().mockResolvedValue([]),
  setStability: vi.fn(),
  updateSpace: vi.fn(),
  deleteSpace: vi.fn(),
  confirmSpace: vi.fn(),
  updateMemory: vi.fn(),
  deleteFileChunks: vi.fn(),
  getVersionChain: vi.fn().mockResolvedValue([]),
  FACET_COLORS: {},
  STABILITY_TIERS: {},
  getPendingRevision: vi.fn().mockResolvedValue(null),
  skillsFromLibraryEnabled: vi.fn().mockResolvedValue(false),
  listSpaceRulesSuggestions: vi.fn().mockResolvedValue([]),
  acceptPendingRevision: vi.fn(),
  dismissPendingRevision: vi.fn(),
  pinMemory: vi.fn().mockResolvedValue(undefined),
  unpinMemory: vi.fn().mockResolvedValue(undefined),
  agentDisplayName: (slug: string | null) => slug,
}));

import {
  deleteSpace,
  getSpace,
  listMemoriesRich,
  listEntities,
  updateSpace,
} from "../../lib/tauri";
import SpaceDetail, { type SpaceDetailProps } from "./SpaceDetail";
import { SPACE_DETAIL_TEST_COPY } from "./space-detail/testTranslation";

const mockGetSpace = vi.mocked(getSpace);
const mockListMemoriesRich = vi.mocked(listMemoriesRich);
const mockListEntities = vi.mocked(listEntities);
const mockDeleteSpace = vi.mocked(deleteSpace);
const mockUpdateSpace = vi.mocked(updateSpace);

function renderWithQuery(ui: React.ReactElement<SpaceDetailProps>) {
  const qc = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  const detail = cloneElement(ui, { copy: SPACE_DETAIL_TEST_COPY });
  return render(<QueryClientProvider client={qc}>{detail}</QueryClientProvider>);
}

const baseSpace = {
  id: "s1",
  name: "Origin",
  description: null,
  suggested: false,
  starred: false,
  sort_order: 0,
  memory_count: 47,
  entity_count: 12,
  created_at: 1000,
  updated_at: 2000,
};

const confirmedMemories = [
  {
    source_id: "m1",
    title: "Prefers TDD workflow",
    content: "TDD workflow details",
    summary: null,
    memory_type: "preference",
    domain: "origin",
    source_agent: "claude-code",
    confidence: 0.9,
    confirmed: true,
    pinned: false,
    supersedes: null,
    last_modified: 1000,
    chunk_count: 1,
    access_count: 5,
    is_recap: false,
  },
  {
    source_id: "m2",
    title: "Uses Rust for backend",
    content: "Rust backend details",
    summary: null,
    memory_type: "fact",
    domain: "origin",
    source_agent: "claude-code",
    confidence: 0.8,
    confirmed: true,
    pinned: false,
    supersedes: null,
    last_modified: 2000,
    chunk_count: 1,
    access_count: 3,
    is_recap: false,
  },
];

describe("SpaceDetail context header", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockGetSpace.mockResolvedValue(baseSpace);
    mockListMemoriesRich.mockResolvedValue(confirmedMemories);
    mockListEntities.mockResolvedValue([]);
    mockUpdateSpace.mockResolvedValue(baseSpace);
  });

  it("does not render context section", async () => {
    renderWithQuery(
      <SpaceDetail
        spaceName="Origin"
        onBack={() => {}}
        onSelectPage={() => {}}
      />,
    );

    // Wait for component to load
    await screen.findByRole("heading", { name: "Origin" });
    // Context section should not exist
    expect(screen.queryByText("Context")).not.toBeInTheDocument();
  });

  it("saves the Space name and description through one daemon mutation", async () => {
    renderWithQuery(
      <SpaceDetail
        spaceName="Origin"
        onBack={() => {}}
        onSelectPage={() => {}}
      />,
    );

    await screen.findByRole("heading", { name: "Origin" });
    fireEvent.change(screen.getByRole("textbox", { name: "Name" }), {
      target: { value: "Origin research" },
    });
    fireEvent.change(screen.getByRole("textbox", { name: "Edit description" }), {
      target: { value: "Unified context" },
    });
    fireEvent.keyDown(screen.getByRole("textbox", { name: "Edit description" }), { key: "Enter", ctrlKey: true });

    await waitFor(() => expect(mockUpdateSpace).toHaveBeenCalledWith(
      "Origin",
      "Origin research",
      "Unified context",
    ));
    expect(mockUpdateSpace).toHaveBeenCalledTimes(1);
  });

  it("keeps memory counts out of the header", async () => {
    renderWithQuery(
      <SpaceDetail
        spaceName="Origin"
        onBack={() => {}}
        onSelectPage={() => {}}
      />,
    );

    await screen.findByRole("heading", { name: "Origin" });
    expect(document.querySelector(".space-dossier-metrics")).toBeNull();
    expect(screen.queryByRole("button", { name: "Raw memories (47)" })).not.toBeInTheDocument();
  });

  it("keeps topic counts out of the header", async () => {
    renderWithQuery(
      <SpaceDetail
        spaceName="Origin"
        onBack={() => {}}
        onSelectPage={() => {}}
      />,
    );

    await screen.findByRole("heading", { name: "Origin" });
    expect(screen.queryByText("Entities")).not.toBeInTheDocument();
    expect(document.querySelector(".space-dossier-metrics")).toBeNull();
  });

  it("deletes a space with only the daemon-supported keep behavior", async () => {
    renderWithQuery(
      <SpaceDetail
        spaceName="Origin"
        onBack={() => {}}
        onSelectPage={() => {}}
      />,
    );

    fireEvent.click(await screen.findByRole("button", { name: "Actions for Origin" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Delete space" }));

    expect(screen.queryByText("Delete")).not.toBeInTheDocument();
    expect(screen.queryByText(/Keep 47 memories/)).not.toBeInTheDocument();
    expect(
      screen.getByText(/Memories keep their current space tag/),
    ).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Confirm delete space" }));

    await waitFor(() => {
      const calls = mockDeleteSpace.mock.calls;
      expect(calls[calls.length - 1]).toEqual(["Origin"]);
    });
  });

  it("discards a suggested space without unsupported memoryAction arguments", async () => {
    mockGetSpace.mockResolvedValue({ ...baseSpace, suggested: true });

    renderWithQuery(
      <SpaceDetail
        spaceName="Origin"
        onBack={() => {}}
        onSelectPage={() => {}}
      />,
    );

    fireEvent.click(await screen.findByText("Discard"));

    await waitFor(() => {
      const calls = mockDeleteSpace.mock.calls;
      expect(calls[calls.length - 1]).toEqual(["Origin"]);
    });
  });
});
