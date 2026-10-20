// SPDX-License-Identifier: AGPL-3.0-only
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../../lib/tauri", () => ({
  acceptPendingRevision: vi.fn(), agentDisplayName: (slug: string | null) => slug,
  confirmSpace: vi.fn(), createPage: vi.fn(), deleteFileChunks: vi.fn(), deleteSpace: vi.fn(),
  dismissPendingRevision: vi.fn(), FACET_COLORS: {}, getNurtureCards: vi.fn().mockResolvedValue([]),
  getPendingRevision: vi.fn().mockResolvedValue(null), getSpace: vi.fn(),
  getVersionChain: vi.fn().mockResolvedValue([]), listEntities: vi.fn(),
  listMemoriesRich: vi.fn(), listPages: vi.fn(), listSpaces: vi.fn(), pinMemory: vi.fn(),
  setStability: vi.fn(), STABILITY_TIERS: {}, unpinMemory: vi.fn(),
  updateMemory: vi.fn(), updateSpace: vi.fn(),
}));

import type { MemoryItem, Page, Space } from "../../../lib/tauri";
import { getSpace, listEntities, listMemoriesRich, listPages, listSpaces } from "../../../lib/tauri";
import SpaceDetail from "../SpaceDetail";
import { SPACE_DETAIL_TEST_COPY } from "./testTranslation";

const baseSpace: Space = {
  id: "s1", name: "Wenlan", description: "Editorial memory", suggested: false,
  starred: true, sort_order: 0, memory_count: 250, entity_count: 8,
  created_at: 1_700_000_000, updated_at: 1_700_000_000,
};

function makePage(id: string, title: string, lastModified: string, staleReason?: string): Page {
  return {
    id, title, summary: `${title} summary`, content: title, entity_id: null,
    domain: "Wenlan", source_memory_ids: [`m-${id}`], version: 1, status: "active",
    created_at: "2026-07-01T00:00:00Z", last_compiled: lastModified,
    last_modified: lastModified, ...(staleReason ? { stale_reason: staleReason } : {}),
  };
}

const memory: MemoryItem = {
  source_id: "m1", title: "Latest raw memory", content: "Body", summary: null,
  memory_type: "fact", domain: "Wenlan", source_agent: "codex", confidence: 0.9,
  confirmed: true, pinned: false, supersedes: null, last_modified: 2_000, chunk_count: 1,
};

function renderDetail(overrides: Partial<React.ComponentProps<typeof SpaceDetail>> = {}) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  const props = {
    copy: SPACE_DETAIL_TEST_COPY, spaceName: "Wenlan", onBack: vi.fn(), onSelectMemory: vi.fn(),
    onSelectPage: vi.fn(), onEntityClick: vi.fn(), onReviewAll: vi.fn(), ...overrides,
  };
  render(<QueryClientProvider client={client}><SpaceDetail {...props} /></QueryClientProvider>);
  return props;
}

describe("SpaceDetail editorial dossier", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(getSpace).mockResolvedValue(baseSpace);
    vi.mocked(listMemoriesRich).mockResolvedValue([memory]);
    vi.mocked(listEntities).mockResolvedValue([]);
    vi.mocked(listPages).mockResolvedValue([]);
    vi.mocked(listSpaces).mockResolvedValue([baseSpace]);
  });


  it("opens a Page editor for the current Space before the overflow action", async () => {
    const onCreatePage = vi.fn();
    renderDetail({ onCreatePage });

    const newPage = await screen.findByRole("button", { name: "New page" });
    const overflow = screen.getByRole("button", { name: "Actions for Wenlan" });
    expect(newPage.compareDocumentPosition(overflow) & Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(0);

    fireEvent.click(newPage);
    expect(onCreatePage).toHaveBeenCalledWith("Wenlan");
    expect(screen.queryByRole("dialog", { name: "New page" })).not.toBeInTheDocument();
  });

  it("loads the active page cap with a notice and starts with 20 pages", async () => {
    vi.mocked(listPages).mockResolvedValue(Array.from({ length: 1_000 }, (_, index) =>
      makePage(`p${index}`, `Page ${String(index).padStart(4, "0")}`, "2026-07-09T20:00:00Z"),
    ));
    renderDetail();
    await screen.findByRole("heading", { level: 1, name: "Wenlan" });
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    expect(screen.getByRole("status")).toHaveTextContent("1,000+ pages in this space. Only the first 1,000 are loaded here.");
    const region = screen.getByRole("region", { name: "Pages" });
    expect(region.querySelectorAll(".space-dossier-page-list > button")).toHaveLength(20);
    fireEvent.click(within(region).getByRole("button", { name: "Show more" }));
    expect(region.querySelectorAll(".space-dossier-page-list > button")).toHaveLength(40);
    expect(document.querySelector(".space-dossier-metrics")).toBeNull();
    expect(listPages).toHaveBeenCalledWith("active", "Wenlan", 1_000);
  });

  it("makes every loaded page reachable through modest show-more steps", async () => {
    vi.mocked(listPages).mockResolvedValue(Array.from({ length: 45 }, (_, index) =>
      makePage(`p${index}`, `Page ${String(index).padStart(2, "0")}`, index === 44 ? "" : "2026-07-09T20:00:00Z"),
    ));
    const onSelectPage = vi.fn();
    renderDetail({ onSelectPage });
    const region = await screen.findByRole("region", { name: "Pages" });
    expect(region.querySelectorAll(".space-dossier-page-list > button")).toHaveLength(20);
    fireEvent.click(within(region).getByRole("button", { name: "Show more" }));
    expect(region.querySelectorAll(".space-dossier-page-list > button")).toHaveLength(40);
    fireEvent.click(within(region).getByRole("button", { name: "Show more" }));
    expect(region.querySelectorAll(".space-dossier-page-list > button")).toHaveLength(45);
    expect(within(region).queryByRole("button", { name: "Show more" })).not.toBeInTheDocument();
    fireEvent.click(within(region).getByRole("button", { name: "Page 44 Page 44 summary" }));
    expect(onSelectPage).toHaveBeenCalledWith("p44");
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  it("uses compact tonal actions for a suggested Space", async () => {
    vi.mocked(getSpace).mockResolvedValue({ ...baseSpace, suggested: true });
    renderDetail();

    const keep = await screen.findByRole("button", { name: "Keep" });
    const discard = screen.getByRole("button", { name: "Discard" });
    expect(keep).toHaveClass("space-dossier-suggestion-action", "space-dossier-suggestion-keep");
    expect(discard).toHaveClass("space-dossier-suggestion-action", "space-dossier-suggestion-discard");

    const css = readFileSync(resolve("src/components/memory/space-detail/space-detail-header.css"), "utf8");
    expect(css).toMatch(/\.space-dossier-suggestion-action\s*\{[^}]*font-size:\s*12px[^}]*padding:\s*4px 10px/s);
    expect(css).toMatch(/\.space-dossier-suggestion-keep\s*\{[^}]*background:\s*transparent[^}]*color:\s*var\(--mem-accent-indigo\)/s);
    expect(css).toMatch(/\.space-dossier-suggestion-keep:hover\s*\{[^}]*background:\s*var\(--mem-indigo-bg\)/s);
  });

  it("sorts all pages with a stable title tie-break and keeps undated pages", async () => {
    vi.mocked(listPages).mockResolvedValue([
      makePage("old", "Old", "2026-07-01T00:00:00Z"),
      makePage("b", "Beta", "2026-07-10T00:00:00Z"),
      makePage("a", "Alpha", "2026-07-10T00:00:00Z"),
      makePage("c", "Charlie", "2026-07-09T00:00:00Z"),
      makePage("d", "Delta", "2026-07-08T00:00:00Z"),
      makePage("e", "Echo", "2026-07-07T00:00:00Z"),
      makePage("bad", "Invalid", "not-a-date"),
    ]);
    const onSelectPage = vi.fn();
    renderDetail({ onSelectPage });

    const region = await screen.findByRole("region", { name: "Pages" });
    const rows = within(region).getAllByRole("button");
    expect(rows[0].querySelector("svg")).toHaveAttribute("width", "20");
    expect(rows.map((row) => row.textContent)).toEqual([
      expect.stringContaining("Alpha"), expect.stringContaining("Beta"),
      expect.stringContaining("Charlie"), expect.stringContaining("Delta"),
      expect.stringContaining("Echo"), expect.stringContaining("Old"), expect.stringContaining("Invalid"),
    ]);
    expect(region).toHaveTextContent("Invalid summary");
    expect(region.querySelector("time")).toBeNull();
    expect(region).not.toHaveTextContent("source");
    expect(document.body).not.toHaveTextContent("NaN");
    fireEvent.click(rows[0]);
    expect(onSelectPage).toHaveBeenCalledWith("a");
  });

  it("keeps review in overflow and does not read standalone memories or topics", async () => {
    const onReviewAll = vi.fn();
    renderDetail({ onReviewAll });
    await screen.findByRole("tab", { name: "Pages" });
    expect(screen.queryByRole("region", { name: "Key entities" })).not.toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Raw memories" })).not.toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Needs review" })).not.toBeInTheDocument();
    expect(listEntities).not.toHaveBeenCalled();
    expect(listMemoriesRich).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Actions for Wenlan" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Review page changes" }));
    expect(onReviewAll).toHaveBeenCalledExactlyOnceWith();
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
  });
  it("omits the review action when no callback is available", async () => {
    renderDetail({ onReviewAll: undefined });
    fireEvent.click(await screen.findByRole("button", { name: "Actions for Wenlan" }));
    expect(screen.queryByRole("menuitem", { name: "Review page changes" })).not.toBeInTheDocument();
  });

});
