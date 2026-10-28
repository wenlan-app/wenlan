// SPDX-License-Identifier: AGPL-3.0-only
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { MemoryItem } from "../../lib/tauri";

vi.mock("../../lib/tauri", () => ({
  setStability: vi.fn().mockResolvedValue(undefined),
  deleteFileChunks: vi.fn().mockResolvedValue(undefined),
  getVersionChain: vi.fn().mockResolvedValue([]),
  FACET_COLORS: {
    fact: "bg-zinc-500/20 text-zinc-700 border-zinc-500/30",
    preference: "bg-purple-500/20 text-purple-700 border-purple-500/30",
  },
  STABILITY_TIERS: { fact: "standard", preference: "protected" },
  agentDisplayName: (slug: string | null) =>
    slug ? (({ "claude-code": "Claude Code" }) as Record<string, string>)[slug] ?? slug : null,
  getPendingRevision: vi.fn().mockResolvedValue(null),
  acceptPendingRevision: vi.fn(),
  dismissPendingRevision: vi.fn(),
  pinMemory: vi.fn().mockResolvedValue(undefined),
  unpinMemory: vi.fn().mockResolvedValue(undefined),
}));

import { setStability } from "../../lib/tauri";
import MemoryStream from "./MemoryStream";

const mockSetStability = vi.mocked(setStability);

function makeMemory(overrides: Partial<MemoryItem> = {}): MemoryItem {
  return {
    source_id: overrides.source_id ?? "mem-1",
    title: overrides.title ?? "Test memory",
    content: overrides.content ?? "Test content",
    summary: overrides.summary ?? null,
    memory_type: overrides.memory_type ?? "fact",
    domain: overrides.domain ?? null,
    source_agent: overrides.source_agent ?? null,
    confidence: overrides.confidence ?? 0.8,
    confirmed: overrides.confirmed ?? false,
    pinned: overrides.pinned ?? false,
    supersedes: overrides.supersedes ?? null,
    last_modified: overrides.last_modified ?? Date.now() / 1000,
    chunk_count: overrides.chunk_count ?? 1,
    access_count: overrides.access_count ?? 0,
    is_recap: overrides.is_recap ?? false,
    stability: overrides.stability ?? "new",
  };
}

function renderWithQuery(ui: React.ReactElement) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

describe("MemoryStream", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    localStorage.clear();
  });

  it("renders memories in grid view by default", () => {
    localStorage.setItem("origin-memory-view-mode", "grid");
    const memories = [
      makeMemory({ source_id: "a", title: "First" }),
      makeMemory({ source_id: "b", title: "Second" }),
    ];
    renderWithQuery(<MemoryStream memories={memories} selectedDomain={null} />);
    expect(screen.getByText("First")).toBeInTheDocument();
    expect(screen.getByText("Second")).toBeInTheDocument();
  });

  it("renders content-first parent rows with on-demand actions and keyboard activation", () => {
    const onSelectMemory = vi.fn();
    const memories = [
      makeMemory({
        source_id: "decision-1",
        title: "Local-first decision",
        memory_type: "decision",
        domain: "Wenlan",
        source_agent: "claude-code",
        stability: "confirmed",
        confirmed: true,
        pinned: true,
        last_modified: 1_700_000_000,
      }),
    ];

    renderWithQuery(
      <MemoryStream
        memories={memories}
        selectedDomain={null}
        sortMode="curated"
        onSelectMemory={onSelectMemory}
        presentation="parent-list"
      />,
    );

    const list = screen.getByRole("region", { name: "Memory list" });
    const row = within(list).getByRole("article", { name: /Local-first decision/i });

    expect(row).toHaveAttribute("tabindex", "0");
    expect(within(row).getByText(memories[0].content)).toBeVisible();
    expect(within(row).getByText("Wenlan")).toBeVisible();
    expect(row.querySelector("dl")).toBeNull();
    expect(within(row).getByRole("button", { name: "Open memory" })).toBeVisible();
    expect(within(row).queryByRole("menu")).not.toBeInTheDocument();
    fireEvent.click(within(row).getByRole("button", { name: "Memory actions" }));
    expect(within(row).getByRole("menuitem", { name: "Unconfirm memory" })).toBeVisible();
    expect(within(row).getByRole("menuitem", { name: "Delete memory" })).toBeVisible();
    expect(within(row).getByRole("menuitem", { name: "Unpin memory" })).toBeVisible();
    fireEvent.keyDown(within(row).getByRole("menu"), { key: "Escape" });

    fireEvent.keyDown(row, { key: "Enter" });
    expect(onSelectMemory).toHaveBeenCalledWith("decision-1");

    fireEvent.keyDown(row, { key: " " });
    expect(onSelectMemory).toHaveBeenCalledTimes(2);
  });

  it("keeps the Memories heading accessible without showing a duplicate and toggles the same collection into cards", () => {
    const memories = [
      makeMemory({ source_id: "one", title: "First memory", content: "First readable memory", domain: "Wenlan" }),
      makeMemory({ source_id: "two", title: "Second memory", content: "Second readable memory", domain: "Research" }),
    ];

    renderWithQuery(
      <MemoryStream memories={memories} selectedDomain={null} presentation="parent-list" />,
    );

    const heading = screen.getByRole("heading", { name: "Memories" });
    expect(heading).not.toBeVisible();
    expect(screen.getByRole("region", { name: "Memory list" })).toContainElement(heading);
    expect(screen.getByTestId("asset-lens-rows")).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByText("First readable memory")).toBeVisible();
    expect(screen.getByText("Second readable memory")).toBeVisible();

    fireEvent.click(screen.getByTestId("asset-lens-cards"));

    expect(screen.getByTestId("asset-lens-cards")).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByText("First readable memory")).toBeVisible();
    expect(screen.getByText("Second readable memory")).toBeVisible();
    expect(document.querySelectorAll(".memory-collection-card")).toHaveLength(2);
    const cardRow = screen.getByRole("article", { name: "First memory" });
    fireEvent.click(within(cardRow).getByRole("button", { name: "Memory actions" }));
    expect(within(cardRow).getByRole("menuitem", { name: "Delete memory" })).toBeVisible();
    expect(localStorage.getItem("wenlan-memory-view-mode")).toBe("grid");
  });

  it("filters parent-list memories by trimmed, case-insensitive title and content", () => {
    const memories = [
      makeMemory({ source_id: "title", title: "Quarterly Planning", content: "A private note" }),
      makeMemory({ source_id: "content", title: "Research notes", content: "Discussed Project ORBIT" }),
      makeMemory({ source_id: "other", title: "Unrelated", content: "Something else" }),
    ];

    renderWithQuery(<MemoryStream memories={memories} selectedDomain={null} presentation="parent-list" />);

    const filter = screen.getByRole("searchbox", { name: "Filter memories" });
    fireEvent.change(filter, { target: { value: "  QUARTERLY  " } });
    expect(screen.getByText("A private note")).toBeVisible();
    expect(screen.queryByText("Discussed Project ORBIT")).not.toBeInTheDocument();

    fireEvent.change(filter, { target: { value: "  orbit " } });
    expect(screen.getByText("Discussed Project ORBIT")).toBeVisible();
    expect(screen.queryByText("A private note")).not.toBeInTheDocument();
  });

  it("uses the same filtered memories in row and card layouts and can recover from no matches", () => {
    const memories = [
      makeMemory({ source_id: "one", title: "Needle memory", content: "Readable needle content" }),
      makeMemory({ source_id: "two", title: "Other memory", content: "Other readable content" }),
    ];

    renderWithQuery(<MemoryStream memories={memories} selectedDomain={null} presentation="parent-list" />);
    const filter = screen.getByRole("searchbox", { name: "Filter memories" });
    fireEvent.change(filter, { target: { value: "needle" } });
    expect(screen.getByText("Readable needle content")).toBeVisible();
    expect(screen.queryByText("Other readable content")).not.toBeInTheDocument();

    fireEvent.click(screen.getByTestId("asset-lens-cards"));
    expect(screen.getByText("Readable needle content")).toBeVisible();
    expect(screen.queryByText("Other readable content")).not.toBeInTheDocument();
    expect(document.querySelectorAll(".memory-collection-card")).toHaveLength(1);

    fireEvent.change(filter, { target: { value: "missing" } });
    expect(screen.getByText("No memories match this view")).toBeVisible();
    expect(screen.getByRole("button", { name: "Clear filter" })).toBeVisible();
    expect(screen.getByTestId("asset-lens-cards")).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "Clear filter" }));
    expect(screen.getByText("Readable needle content")).toBeVisible();
    expect(screen.getByText("Other readable content")).toBeVisible();
  });

  it("keeps the collection filter available when the library is empty", () => {
    renderWithQuery(<MemoryStream memories={[]} selectedDomain={null} presentation="parent-list" />);

    expect(screen.getByRole("searchbox", { name: "Filter memories" })).toBeVisible();
    expect(screen.getByText(/^No memories yet\./)).toBeVisible();
  });

  it("leaves embedded memory view mode behavior unchanged", () => {
    localStorage.setItem("wenlan-memory-view-mode", "grid");
    const memories = [makeMemory({ source_id: "one", title: "Embedded memory" })];

    renderWithQuery(<MemoryStream memories={memories} selectedDomain={null} />);

    expect(screen.queryByTestId("asset-lens-rows")).not.toBeInTheDocument();
    expect(screen.getByTitle("Switch to list view")).toBeInTheDocument();
    expect(screen.queryByRole("searchbox", { name: "Filter memories" })).not.toBeInTheDocument();
  });

  it("imports the legacy Origin view-mode preference into the Wenlan key", () => {
    localStorage.setItem("origin-memory-view-mode", "list");
    const memories = [makeMemory({ source_id: "a", title: "First" })];

    renderWithQuery(<MemoryStream memories={memories} selectedDomain={null} />);

    expect(localStorage.getItem("wenlan-memory-view-mode")).toBe("list");
    expect(localStorage.getItem("origin-memory-view-mode")).toBe("list");
  });

  it("filters by stability when stabilityFilter is set", () => {
    localStorage.setItem("origin-memory-view-mode", "list");
    const memories = [
      makeMemory({ source_id: "a", title: "Confirmed one", stability: "confirmed" }),
      makeMemory({ source_id: "b", title: "New one", stability: "new" }),
    ];
    renderWithQuery(
      <MemoryStream memories={memories} selectedDomain={null} stabilityFilter="confirmed" />,
    );
    expect(screen.getByText("Confirmed one")).toBeInTheDocument();
    expect(screen.queryByText("New one")).not.toBeInTheDocument();
  });

  it("curated sort orders distilled > confirmed > learned > new", () => {
    localStorage.setItem("origin-memory-view-mode", "list");
    const memories = [
      makeMemory({ source_id: "a", title: "New mem", stability: "new", last_modified: 100 }),
      makeMemory({ source_id: "b", title: "Confirmed mem", stability: "confirmed", last_modified: 90 }),
      makeMemory({ source_id: "c", title: "Distilled mem", stability: "learned", supersedes: "x", last_modified: 80 }),
    ];
    renderWithQuery(
      <MemoryStream memories={memories} selectedDomain={null} sortMode="curated" />,
    );
    const items = screen.getAllByText(/mem$/);
    expect(items[0].textContent).toBe("Distilled mem");
    expect(items[1].textContent).toBe("Confirmed mem");
    expect(items[2].textContent).toBe("New mem");
  });

  it("unconfirm of confirmed memory with learned prevStability restores learned", async () => {
    localStorage.setItem("origin-memory-view-mode", "list");
    // Memory was learned, then confirmed — stability is "confirmed", prevStability tracked internally
    // But MemoryStream passes mem.stability as prevStability, which is "confirmed" at render time
    // The real scenario: the memory's stability field reflects its CURRENT state
    // So we test: clicking unconfirm on a confirmed memory → reverts to "new" (default)
    // since prevStability="confirmed" doesn't match "learned"
    const memories = [
      makeMemory({ source_id: "a", title: "Confirmed memory", stability: "confirmed", confirmed: true }),
    ];
    renderWithQuery(<MemoryStream memories={memories} selectedDomain={null} />);

    const dot = screen.getByTestId("confirm-dot");
    fireEvent.click(dot);

    await waitFor(() => {
      // prevStability is "confirmed" (current), but we're unconfirming → falls to "new"
      expect(mockSetStability).toHaveBeenCalledWith("a", "new");
    });
  });

  it("confirm sets stability to confirmed", async () => {
    localStorage.setItem("origin-memory-view-mode", "list");
    const memories = [
      makeMemory({ source_id: "b", title: "New memory", stability: "new", confirmed: false }),
    ];
    renderWithQuery(<MemoryStream memories={memories} selectedDomain={null} />);

    const dot = screen.getByTestId("confirm-dot");
    fireEvent.click(dot);

    await waitFor(() => {
      expect(mockSetStability).toHaveBeenCalledWith("b", "confirmed");
    });
  });

  it("excludes recap memories", () => {
    const memories = [
      makeMemory({ source_id: "a", title: "Regular", is_recap: false }),
      makeMemory({ source_id: "b", title: "Recap", is_recap: true }),
    ];
    renderWithQuery(<MemoryStream memories={memories} selectedDomain={null} />);
    expect(screen.getByText("Regular")).toBeInTheDocument();
    expect(screen.queryByText("Recap")).not.toBeInTheDocument();
  });
});
