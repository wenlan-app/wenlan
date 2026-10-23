import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  distillReview,
  listAgents,
  listPagesExplicitBrowse,
  listRefinements,
  listRegisteredSources,
  type DistillReviewResponse,
  type Page,
} from "../../../lib/tauri";
import { PagesOverview } from "./PagesOverview";
import { DISTILL_REVIEW_SESSION_QUERY_KEY } from "./pageReviewSignals";
import type { WikiInventoryScope } from "./pageInventory";

vi.mock("../../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../../lib/tauri")>()),
  distillReview: vi.fn(),
  listPagesExplicitBrowse: vi.fn(),
  listRefinements: vi.fn(),
  listOnboardingMilestones: vi.fn().mockResolvedValue([]),
  listAgents: vi.fn().mockResolvedValue([]),
  listRegisteredSources: vi.fn().mockResolvedValue([]),
  getTruthStatus: vi.fn().mockResolvedValue(null),
}));

function page(overrides: Partial<Page> = {}): Page {
  return {
    id: "page-1",
    title: "Independent research page",
    summary: "A page can stand on its own.",
    content: "",
    entity_id: null,
    domain: null,
    space: null,
    source_memory_ids: [],
    version: 1,
    status: "active",
    created_at: "2026-07-01T00:00:00Z",
    last_compiled: "2026-07-01T00:00:00Z",
    last_modified: "2026-07-10T00:00:00Z",
    ...overrides,
  };
}

function renderOverview({
  onCreatePage = vi.fn(),
  onSelectDraft = vi.fn(),
  onSelectPage = vi.fn(),
  onSelectSpace = vi.fn(),
  onBrowseAll = undefined as (() => void) | undefined,
  inventoryScope = "all" as WikiInventoryScope,
  checklist = false,
  queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } }),
} = {}) {
  const overview = (
    <QueryClientProvider client={queryClient}>
      <PagesOverview
        inventoryScope={inventoryScope}
        onBrowseAll={onBrowseAll}
        onCreatePage={onCreatePage}
        onSelectDraft={onSelectDraft}
        onSelectPage={onSelectPage}
        onSelectSpace={onSelectSpace}
        onConnectTool={checklist ? vi.fn() : undefined}
        onSeeExample={checklist ? vi.fn() : undefined}
      />
    </QueryClientProvider>
  );
  return {
    queryClient,
    onCreatePage,
    onSelectDraft,
    onSelectPage,
    onSelectSpace,
    ...render(overview),
  };
}

describe("PagesOverview reading workspace", () => {
  beforeEach(() => {
    vi.mocked(listPagesExplicitBrowse).mockReset();
    vi.mocked(listRefinements).mockReset();
    vi.mocked(distillReview).mockReset();
    vi.mocked(listPagesExplicitBrowse).mockResolvedValue([]);
  });

  it("shows localized reading guidance while creation lives beside the tabs", async () => {
    vi.mocked(listPagesExplicitBrowse).mockImplementation(async status => status === "draft"
      ? [page({ id: "draft", title: "Cached draft name", status: "draft" })]
      : [page({ id: "active", title: "Cached active name", creation_kind: "source" })]);
    const discovery: DistillReviewResponse = {
      pages_created: 0,
      scoped: false,
      created_ids: [],
      pending: [{ source_ids: ["source"], contents: ["candidate content"], entity_name: "Passive candidate name", estimated_tokens: 20 }],
      stale_pages: [],
      stale_truncated: false,
      orphan_topics: [],
    };
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    queryClient.setQueryData(DISTILL_REVIEW_SESSION_QUERY_KEY, discovery);
    const { container } = renderOverview({ queryClient });

    expect(await screen.findByRole("heading", { level: 1, name: "Open a page" })).toBeInTheDocument();
    expect(screen.getByText("Use Search to find a page, or select + to start writing.")).toBeInTheDocument();
    expect(screen.queryByTestId("wiki-create-options")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "New" })).not.toBeInTheDocument();
    expect(listPagesExplicitBrowse).toHaveBeenCalledWith("active", undefined, 500, 0);
    expect(listPagesExplicitBrowse).toHaveBeenCalledWith("draft", undefined, 500, 0);
    for (const hiddenName of ["Cached active name", "Cached draft name", "Passive candidate name", "candidate content"]) {
      expect(screen.queryByText(hiddenName)).not.toBeInTheDocument();
    }
    expect(container.querySelector("table")).toBeNull();
    expect(container.querySelector(".asset-cards, [data-testid='wiki-cards'], [data-testid='pages-library']")).toBeNull();
    expect(screen.queryByRole("list")).not.toBeInTheDocument();
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
    expect(distillReview).not.toHaveBeenCalled();
    expect(listRefinements).not.toHaveBeenCalled();
  });

  it("marks the start section empty only once both lists loaded with no pages", async () => {
    const { container } = renderOverview();
    const section = container.querySelector(".wiki-reading-start");
    expect(section).not.toHaveAttribute("data-library-empty");
    await waitFor(() => expect(section).toHaveAttribute("data-library-empty", "true"));
    expect(screen.getByRole("heading", { level: 1, name: "Open a page" })).toBeInTheDocument();
  });

  it.each(["active", "draft"] as const)("does not mark the library empty when a %s page exists", async status => {
    vi.mocked(listPagesExplicitBrowse).mockImplementation(async requested => requested === status
      ? [page({ id: `${status}-1`, status })]
      : []);
    const { container } = renderOverview();
    await waitFor(() => expect(listPagesExplicitBrowse).toHaveBeenCalledTimes(2));
    await screen.findByRole("heading", { level: 1, name: "Open a page" });
    expect(container.querySelector(".wiki-reading-start")).not.toHaveAttribute("data-library-empty");
  });

  it("lists the newest pages written from material or by the user, one click each", async () => {
    vi.mocked(listPagesExplicitBrowse).mockImplementation(async status => status === "draft"
      ? [page({ id: "draft", title: "Unfinished draft", status: "draft" })]
      : [
        page({ id: "copy", title: "ISP Outage Runbook (section_h1)", creation_kind: "source" }),
        page({ id: "old", title: "Older launch notes", creation_kind: "distilled", created_at: "2026-07-01T00:00:00Z", source_memory_ids: ["a", "b", "c"] }),
        page({ id: "new", title: "v2 launch plan", creation_kind: "distilled", created_at: "2026-07-02T00:00:00Z", source_memory_ids: ["a", "b"] }),
      ]);
    const onBrowseAll = vi.fn();
    const { onSelectPage } = renderOverview({ onBrowseAll });

    const list = await screen.findByTestId("wiki-new-pages");
    expect(screen.getByRole("heading", { level: 2, name: "New pages" })).toBeInTheDocument();
    const titles = [...list.querySelectorAll(".wiki-new-pages-title")].map(node => node.textContent);
    expect(titles).toEqual(["v2 launch plan", "Older launch notes"]);
    expect(screen.queryByText("ISP Outage Runbook (section_h1)")).not.toBeInTheDocument();
    expect(screen.queryByText("Unfinished draft")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /v2 launch plan/ }));
    expect(onSelectPage).toHaveBeenCalledWith("new");
    fireEvent.click(screen.getByRole("button", { name: "See all pages" }));
    expect(onBrowseAll).toHaveBeenCalled();
  });

  describe("getting started", () => {
    beforeEach(() => {
      localStorage.clear();
      vi.mocked(listAgents).mockResolvedValue([]);
      vi.mocked(listRegisteredSources).mockResolvedValue([]);
    });

    it("compacts the start panel while the checklist is open and restores it when hidden", async () => {
      const { container } = renderOverview({ checklist: true });
      const section = container.querySelector(".wiki-reading-start");
      expect(section).not.toHaveAttribute("data-onboarding");
      await screen.findByTestId("getting-started");
      expect(section).toHaveAttribute("data-onboarding", "true");
      // The landmark heading stays for navigation and the e2e helpers.
      expect(screen.getByRole("heading", { level: 1, name: "Open a page" })).toBeInTheDocument();

      fireEvent.click(screen.getByRole("button", { name: "Hide getting started" }));
      expect(section).not.toHaveAttribute("data-onboarding");
      expect(screen.getByRole("button", { name: "Show getting started" })).toBeInTheDocument();
    });

    it("leaves the panel alone when the checklist is not wired", async () => {
      const { container } = renderOverview();
      await waitFor(() => expect(listAgents).toHaveBeenCalled());
      await new Promise(resolve => setTimeout(resolve, 0));
      expect(container.querySelector(".wiki-reading-start")).not.toHaveAttribute("data-onboarding");
      expect(screen.queryByTestId("getting-started")).not.toBeInTheDocument();
    });

    it("opens the add-source dialog in place instead of leaving the Wiki", async () => {
      renderOverview({ checklist: true });
      fireEvent.click(await screen.findByRole("button", { name: /Folder or files/ }));
      expect(await screen.findByRole("dialog")).toBeInTheDocument();
      expect(screen.getByRole("heading", { level: 1, name: "Open a page" })).toBeInTheDocument();
    });

    it("opens straight at importing AI chats", async () => {
      renderOverview({ checklist: true });
      fireEvent.click(await screen.findByRole("button", { name: /Past AI chats/ }));
      expect(await screen.findByRole("dialog", { name: "Past AI chats" })).toBeInTheDocument();
    });
  });
});
