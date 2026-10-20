// SPDX-License-Identifier: AGPL-3.0-only
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { i18n } from "../../i18n";
import HomePage from "./HomePage";

class ResizeObserverStub {
  observe() {}
  unobserve() {}
  disconnect() {}
}
vi.stubGlobal("ResizeObserver", ResizeObserverStub);

vi.mock("../../lib/tauri", async () => {
  const actual = await vi.importActual<typeof import("../../lib/tauri")>("../../lib/tauri");
  return {
    ...actual,
    listRecentRetrievals: vi.fn(),
    listRecentPages: vi.fn(),
    listRecentConcepts: vi.fn(),
    listRecentMemories: vi.fn(),
    listUnconfirmedMemories: vi.fn(),
    listPages: vi.fn(),
    listConcepts: vi.fn(),
    listRecentChanges: vi.fn(),
    listEntities: vi.fn(),
    getMemoryStats: vi.fn(),
    getProfile: vi.fn(),
    getPendingContradictions: vi.fn(),
    dismissContradiction: vi.fn(),
    confirmMemory: vi.fn(),
    deleteMemory: vi.fn(),
    listPendingRevisions: vi.fn(),
    acceptPendingRevision: vi.fn(),
    dismissPendingRevision: vi.fn(),
    listRefinements: vi.fn(),
    acceptRefinement: vi.fn(),
    rejectRefinement: vi.fn(),
    getMemoryDetail: vi.fn(),
    getEntityDetail: vi.fn(),
    getPage: vi.fn(),
    getPageSources: vi.fn(),
    getMemoryRevisions: vi.fn(),
    listOnboardingMilestones: vi.fn(),
    acknowledgeOnboardingMilestone: vi.fn(),
    getApiKey: vi.fn(),
    getExternalLlm: vi.fn(),
    getOnDeviceModel: vi.fn(),
    getResolvedRouting: vi.fn(),
  };
});

import * as tauri from "../../lib/tauri";

function renderHome(
  props: {
    onSelectPage?: (pageId: string) => void;
    onCreatePage?: (space: string | null) => void;
    onOpenIntelligenceSettings?: () => void;
  } = {},
) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const rendered = render(
    <QueryClientProvider client={qc}>
      <HomePage
        onNavigateGraph={() => {}}
        onSelectPage={props.onSelectPage}
        onCreatePage={props.onCreatePage ?? (() => {})}
        onOpenIntelligenceSettings={props.onOpenIntelligenceSettings ?? (() => {})}
      />
    </QueryClientProvider>,
  );
  return { ...rendered, queryClient: qc };
}

/**
 * The daemon latches `intelligence-ready` on the first successful inference
 * and never clears it, so it says nothing about what is configured now.
 */
const nowIso = new Date().toISOString();

function page(overrides: Partial<tauri.Page> & Pick<tauri.Page, "id" | "title">): tauri.Page {
  return {
    summary: null,
    content: "",
    entity_id: null,
    domain: null,
    source_memory_ids: [],
    version: 1,
    status: "active",
    created_at: nowIso,
    last_compiled: nowIso,
    last_modified: nowIso,
    ...overrides,
  };
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
  localStorage.clear();
  vi.clearAllMocks();
  vi.mocked(tauri.listRecentRetrievals).mockResolvedValue([]);
  vi.mocked(tauri.listRecentPages).mockResolvedValue([]);
  vi.mocked(tauri.listRecentConcepts).mockResolvedValue([]);
  vi.mocked(tauri.listRecentMemories).mockResolvedValue([]);
  vi.mocked(tauri.listUnconfirmedMemories).mockResolvedValue([]);
  vi.mocked(tauri.listPages).mockResolvedValue([]);
  vi.mocked(tauri.listConcepts).mockResolvedValue([]);
  vi.mocked(tauri.listRecentChanges).mockResolvedValue([]);
  vi.mocked(tauri.getPageSources).mockResolvedValue([]);
  vi.mocked(tauri.getMemoryRevisions).mockResolvedValue({
    current_source_id: "mem-target",
    chain_depth: 1,
    entries: [],
  } as any);
  vi.mocked(tauri.listEntities).mockResolvedValue([]);
  vi.mocked(tauri.listOnboardingMilestones).mockResolvedValue([]);
  vi.mocked(tauri.acknowledgeOnboardingMilestone).mockResolvedValue(undefined);
  // Default: no provider of any kind, and all three queries answer.
  vi.mocked(tauri.getApiKey).mockResolvedValue(null);
  vi.mocked(tauri.getExternalLlm).mockResolvedValue([null, null]);
  vi.mocked(tauri.getOnDeviceModel).mockResolvedValue({
    loaded: null,
    selected: null,
    models: [],
  });
  // Default: whatever provider a test turns on is also pinned for background
  // work, which is the state the app writes for itself at launch.
  vi.mocked(tauri.getResolvedRouting).mockResolvedValue(null);
  vi.mocked(tauri.getMemoryStats).mockResolvedValue({
    total: 0,
    new_today: 0,
    confirmed: 0,
    domains: [],
  } as any);
  vi.mocked(tauri.getProfile).mockResolvedValue(null);
  vi.mocked(tauri.confirmMemory).mockResolvedValue(undefined);
  vi.mocked(tauri.deleteMemory).mockResolvedValue(undefined);
  vi.mocked(tauri.dismissContradiction).mockResolvedValue({ source_id: "mem-new", wrote: true });
  vi.mocked(tauri.listPendingRevisions).mockResolvedValue([]);
  vi.mocked(tauri.acceptPendingRevision).mockResolvedValue({
    target_source_id: "mem-target",
    revision_source_id: "mem-revision",
    wrote: true,
  });
  vi.mocked(tauri.dismissPendingRevision).mockResolvedValue({
    target_source_id: "mem-target",
    wrote: true,
  });
  vi.mocked(tauri.listRefinements).mockResolvedValue({ proposals: [] });
  vi.mocked(tauri.acceptRefinement).mockResolvedValue({
    id: "ref-merge",
    action_applied: "entity_merge",
  });
  vi.mocked(tauri.rejectRefinement).mockResolvedValue({ id: "ref-merge" });
  vi.mocked(tauri.getMemoryDetail).mockResolvedValue({
    source_id: "mem-target",
    title: "Target memory",
    content: "The durable original wording from the daemon.",
    summary: null,
    memory_type: null,
    domain: null,
    source_agent: null,
    confidence: null,
    confirmed: true,
    pinned: false,
    supersedes: null,
    last_modified: 1_782_365_000,
    chunk_count: 1,
  } as any);
  vi.mocked(tauri.getEntityDetail).mockResolvedValue({
    entity: {
      id: "ent-a",
      name: "Wenlan",
      entity_type: "tool",
      domain: null,
      source_agent: null,
      confidence: null,
      confirmed: true,
      created_at: 0,
      updated_at: 0,
    },
    observations: [],
    relations: [],
  } as any);
  vi.mocked(tauri.getPendingContradictions).mockResolvedValue([
    {
      id: "contra-1",
      existing_content: "First claim",
      new_content: "Second claim",
      new_source_id: "mem-new",
      existing_source_id: "mem-existing",
    } as any,
  ]);
});

describe("HomePage redesign", () => {
  it("keeps the empty page slot focused on writing without a tour", async () => {
    renderHome();
    const empty = await screen.findByTestId("wiki-page-empty");
    expect(within(empty).queryByRole("button", { name: i18n.t("firstUse.entry") })).toBeNull();
    expect(within(empty).getByRole("button", { name: i18n.t("home.empty.writePage") })).toBeInTheDocument();
    expect(screen.getAllByTestId("wiki-home")).toHaveLength(1);
  });

  it("does not show a tour beside a populated page list", async () => {
    vi.mocked(tauri.listPages).mockResolvedValue([page({ id: "real-page", title: "My knowledge" })]);
    renderHome();
    await screen.findByText("My knowledge");
    expect(screen.queryByRole("button", { name: i18n.t("firstUse.entry") })).toBeNull();
    expect(screen.queryByTestId("wiki-page-empty")).not.toBeInTheDocument();
  });

  it("uses wiki pages as the primary home surface when pages exist without activity", async () => {
    vi.mocked(tauri.listPages).mockResolvedValue([
      page({
        id: "page-architecture",
        title: "Wenlan app architecture",
        domain: "Projects",
        summary: "How the desktop app, daemon, and page compiler fit together.",
        source_memory_ids: ["m1", "m2", "m3", "m4"],
        version: 3,
      }),
      page({
        id: "page-policy",
        title: "Codex workflow policy",
        domain: "Decisions",
        source_memory_ids: ["m5", "m6"],
        version: 2,
      }),
    ]);

    renderHome();

    const todayHeading = await screen.findByRole("heading", { name: "Today in Wenlan", level: 1 });
    expect(todayHeading).toHaveStyle({
      fontFamily: "var(--mem-font-heading)",
      fontSize: "var(--mem-destination-title-size)",
      fontWeight: "500",
      letterSpacing: "-0.03em",
      lineHeight: "1.12",
    });
    expect(todayHeading.parentElement).toHaveStyle({ marginBottom: "16px" });
    expect(tauri.listPages).toHaveBeenCalledWith("active", undefined, 500, 0);
    expect(screen.getByTestId("wiki-home")).toHaveStyle({ display: "grid" });
    expect(screen.getByTestId("wiki-home")).not.toHaveTextContent("Recently active");
    // Home does not present a review count.
    expect(screen.queryByTestId("wiki-context-needs-review")).toBeNull();
    expect(screen.queryByTestId("wiki-space-filter-row")).toBeNull();
    expect(screen.queryByTestId("wiki-recent-spaces")).toBeNull();
    expect(screen.queryByText("Pages")).toBeNull();
    expect(screen.queryByText("Compiled pages, links, and sources your agents can traverse.")).toBeNull();
    expect(screen.queryByRole("heading", { name: "Recent Space" })).toBeNull();
    expect(screen.queryByRole("heading", { name: "Recently refined" })).toBeNull();
    expect(screen.getByText("Wenlan app architecture")).toBeInTheDocument();
    expect(screen.getByTestId("wiki-page-list").querySelector("svg path")).toHaveAttribute(
      "d",
      "M12 2L2 7l10 5 10-5-10-5zM2 17l10 5 10-5M2 12l10 5 10-5",
    );
    expect(screen.getByText("4 sources")).toBeInTheDocument();
    expect(screen.getAllByText("updated today").length).toBeGreaterThan(0);
    expect(screen.queryByText("Key facts")).toBeNull();
    expect(screen.queryByText("Related pages")).toBeNull();
    expect(screen.queryByText("Related sources")).toBeNull();
    expect(screen.queryByText("source-backed")).toBeNull();
  });

  it("lists only knowledge pages, never entity shadow pages", async () => {
    // Given two knowledge pages (one updated today) and one entity shadow page
    // updated today — the daemon's browse list returns all three by contract
    const lastWeek = new Date(Date.now() - 7 * 86_400_000).toISOString();
    vi.mocked(tauri.listPages).mockResolvedValue([
      page({ id: "page-architecture", title: "Wenlan app architecture", creation_kind: "distilled" }),
      page({ id: "page-policy", title: "Codex workflow policy", last_modified: lastWeek }),
      page({ id: "shadow-lucian", title: "Lucian", creation_kind: "entity", entity_id: "ent-1" }),
    ]);

    // When Home renders its page list
    renderHome();
    const list = await screen.findByTestId("wiki-page-list");

    // Then the list holds the two knowledge pages and skips the shadow page
    expect(within(list).getByText("Wenlan app architecture")).toBeInTheDocument();
    expect(within(list).getByText("Codex workflow policy")).toBeInTheDocument();
    expect(within(list).queryByText("Lucian")).toBeNull();
  });

  it("puts the Today heading above one full-width page list", async () => {
    const rectSpy = vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue({
      bottom: 0,
      height: 720,
      left: 0,
      right: 1000,
      top: 0,
      width: 1000,
      x: 0,
      y: 0,
      toJSON: () => ({}),
    });
    vi.mocked(tauri.listPages).mockResolvedValue([
      page({ id: "page-architecture", title: "Wenlan app architecture", source_memory_ids: ["m1", "m2", "m3"] }),
      page({ id: "page-policy", title: "Codex workflow policy", source_memory_ids: ["m4"] }),
    ]);

    try {
      renderHome();

      await screen.findByTestId("wiki-home");
      const todayHeading = screen.getByTestId("wiki-today-heading");
      const pageList = screen.getByTestId("wiki-page-list");
      expect(screen.queryByTestId("wiki-page-updates")).toBeNull();

      expect(todayHeading.compareDocumentPosition(pageList) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
      expect(todayHeading).not.toHaveTextContent("2 pages");
      // Even at a wide width the page list is the only column.
      expect(screen.getByTestId("wiki-content-grid")).toHaveStyle({ gridTemplateColumns: "minmax(0, 1fr)" });

      expect(screen.getByTestId("wiki-page-list")).toHaveStyle({ borderTopStyle: "none" });
    } finally {
      rectSpy.mockRestore();
    }
  });

  it("keeps the Today heading free of the latest-update dateline", async () => {
    vi.mocked(tauri.listPages).mockResolvedValue([
      page({ id: "page-current", title: "Current page", last_modified: nowIso }),
    ]);

    renderHome();

    const todayHeading = await screen.findByTestId("wiki-today-heading");
    expect(within(todayHeading).queryByTestId("wiki-context-latest")).toBeNull();
    expect(within(todayHeading).queryByText("updated today")).toBeNull();
    expect(screen.getByText("updated today")).toBeInTheDocument();
  });

  it("opens wiki page rows from the home index", async () => {
    const onSelectPage = vi.fn();
    const user = userEvent.setup();
    vi.mocked(tauri.listPages).mockResolvedValue([
      page({
        id: "page-architecture",
        title: "Wenlan app architecture",
        domain: "Projects",
        source_memory_ids: ["m1", "m2", "m3"],
        version: 2,
      }),
    ]);

    renderHome({ onSelectPage });

    await user.click(await screen.findByRole("button", { name: /open Wenlan app architecture/i }));

    expect(onSelectPage).toHaveBeenCalledWith("page-architecture");
  });

  it("does not duplicate space navigation on the home index", async () => {
    vi.mocked(tauri.listPages).mockResolvedValue([
      page({
        id: "page-architecture",
        title: "Wenlan app architecture",
        domain: "Projects",
      }),
      page({
        id: "page-policy",
        title: "Codex workflow policy",
        domain: "Decisions",
      }),
    ]);

    renderHome();

    await screen.findByRole("heading", { name: "Today in Wenlan" });

    expect(screen.queryByTestId("wiki-space-filter-row")).toBeNull();
    expect(screen.queryByRole("button", { name: /open Projects space/i })).toBeNull();
    expect(screen.queryByRole("heading", { name: "Spaces" })).toBeNull();
    expect(screen.queryByLabelText("Recent spaces")).toBeNull();
  });

  it("does not expose recent spaces on Home", async () => {
    vi.mocked(tauri.listPages).mockResolvedValue([
      page({
        id: "page-architecture",
        title: "Wenlan app architecture",
        domain: "Projects",
        last_modified: "2026-06-30T12:00:00Z",
      }),
      page({
        id: "page-policy",
        title: "Codex workflow policy",
        domain: "Decisions",
        last_modified: "2026-06-29T12:00:00Z",
      }),
    ]);

    renderHome();

    await screen.findByRole("heading", { name: "Today in Wenlan" });

    expect(screen.queryByTestId("wiki-recent-spaces")).toBeNull();
    expect(screen.getByTestId("wiki-home")).not.toHaveTextContent("Recently active");
  });

  it("keeps pending decisions off Home and never fetches the review queue", async () => {
    vi.mocked(tauri.listPages).mockResolvedValue([
      page({ id: "page-a", title: "A" }),
      page({ id: "page-b", title: "B" }),
      page({ id: "page-c", title: "C" }),
      page({ id: "page-d", title: "D", last_modified: "2026-06-01T12:00:00Z" }),
      page({ id: "page-e", title: "E", last_modified: "2026-06-01T12:00:00Z" }),
    ]);
    vi.mocked(tauri.listPendingRevisions).mockResolvedValue([
      {
        target_source_id: "mem-a",
        revision_source_id: "mem-a-rev",
        revision_content: "First proposed wording",
        source_agent: "claude-code",
        last_modified: 1_782_365_076,
        target_kind: "memory" as const,
      },
      {
        target_source_id: "mem-b",
        revision_source_id: "mem-b-rev",
        revision_content: "Second proposed wording",
        source_agent: "claude-code",
        last_modified: 1_782_365_077,
        target_kind: "memory" as const,
      },
      {
        target_source_id: "mem-c",
        revision_source_id: "mem-c-rev",
        revision_content: "Third proposed wording",
        source_agent: "claude-code",
        last_modified: 1_782_365_078,
        target_kind: "memory" as const,
      },
    ]);
    vi.mocked(tauri.listRefinements).mockResolvedValue({
      proposals: [
        {
          id: "ref-merge",
          action: "entity_merge",
          source_ids: ["ent-a", "ent-b"],
          payload: { action: "entity_merge", existing_id: "ent-a", new_id: "ent-b", similarity: 0.86 },
          confidence: 0.86,
          created_at: nowIso,
        },
      ],
    });

    renderHome();

    await screen.findByRole("heading", { name: "Today in Wenlan" });

    expect(screen.queryByTestId("wiki-page-updates")).toBeNull();
    expect(screen.queryByTestId("wiki-context-needs-review")).toBeNull();
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(tauri.listPendingRevisions).not.toHaveBeenCalled();
    expect(tauri.listRefinements).not.toHaveBeenCalled();
  });

  it("does not navigate to the synthetic Unsorted page bucket", async () => {
    vi.mocked(tauri.listPages).mockResolvedValue([
      page({ id: "page-unsorted", title: "Unassigned page" }),
    ]);

    renderHome();

    await screen.findByRole("heading", { name: "Today in Wenlan" });

    expect(screen.getByText("Unassigned page")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /open Unsorted space/i })).toBeNull();
    expect(screen.queryByText("Unsorted")).toBeNull();
  });

  it("does not render traversal paths on the home surface", async () => {
    vi.mocked(tauri.listPages).mockResolvedValue([
      page({ id: "page-wenlan", title: "Wenlan app architecture", domain: "Projects" }),
    ]);
    vi.mocked(tauri.listRecentRetrievals).mockResolvedValue([
      {
        timestamp_ms: Date.now(),
        agent_name: "claude-code",
        query: "wiki home",
        page_titles: ["Wenlan app architecture", "Codex workflow policy"],
        page_ids: ["page-wenlan", "page-policy"],
        memory_snippets: [],
      },
    ]);

    renderHome();

    await screen.findByRole("heading", { name: "Today in Wenlan" });

    expect(screen.queryByTestId("wiki-traversal-paths")).toBeNull();
    expect(screen.queryByText("Traversal paths")).toBeNull();
  });

  it("never renders a greeting screen, with or without pages", async () => {
    vi.mocked(tauri.getProfile).mockResolvedValue({
      id: "p1",
      name: "Lucian",
      display_name: null,
      email: null,
      bio: null,
      avatar_path: null,
      created_at: 0,
      updated_at: 0,
    } as any);

    // Empty library: one home, no salutation.
    const emptyHome = renderHome();
    await screen.findByTestId("wiki-home");
    expect(screen.queryByTestId("greeting")).toBeNull();
    expect(screen.queryByText(/Good (morning|afternoon|evening)/)).toBeNull();
    expect(screen.queryByText(/your library holds/)).toBeNull();
    emptyHome.unmount();

    // Populated library: the same one home.
    vi.mocked(tauri.listPages).mockResolvedValue([
      page({ id: "page-architecture", title: "Wenlan app architecture" }),
    ]);
    renderHome();
    await screen.findByTestId("wiki-page-list");
    expect(screen.queryByTestId("greeting")).toBeNull();
    expect(screen.queryByText(/Good (morning|afternoon|evening)/)).toBeNull();
  });

  it("does NOT render ProfileNarrativeCompact on home", async () => {
    const now = Date.now();
    vi.mocked(tauri.listRecentConcepts).mockResolvedValue([
      { kind: "concept", id: "c1", title: "A", snippet: "s", timestamp_ms: now, badge: { kind: "new" } },
    ] as any);
    renderHome();
    // Settle React Query before asserting absence.
    await new Promise((r) => setTimeout(r, 100));
    expect(screen.queryByText(/^Updated/i)).toBeNull();
  });

  // Retrievals belong to the Activity view, which already shows agents reading
  // from the library. Home never renders them, however many events the daemon
  // has — so a re-add has to delete this test on purpose rather than slip in.
  it("does NOT render a retrievals section even when retrieval events exist", async () => {
    vi.mocked(tauri.listRecentRetrievals).mockResolvedValue([
      {
        timestamp_ms: Date.now(),
        agent_name: "claude-code",
        query: "positioning",
        page_titles: ["Origin positioning", "Daemon architecture"],
        page_ids: ["concept_pos", "concept_arch"],
        memory_snippets: [],
      },
    ]);
    vi.mocked(tauri.listPages).mockResolvedValue([
      page({ id: "page-architecture", title: "Wenlan app architecture" }),
    ]);

    renderHome();

    const home = await screen.findByTestId("wiki-home");
    await new Promise((r) => setTimeout(r, 100));
    expect(within(home).queryByTestId("retrievals")).toBeNull();
    expect(screen.queryByTestId("retrievals")).toBeNull();
    expect(screen.queryByTestId("retrieval-item")).toBeNull();
    expect(screen.queryByText(/Where AI looked/i)).toBeNull();
    expect(screen.queryByText(/Origin positioning/)).toBeNull();
  });

  it("does NOT render contradiction resolver on home", async () => {
    renderHome();
    await new Promise((r) => setTimeout(r, 100));
    expect(screen.queryByTestId("contradiction-resolver")).toBeNull();
  });

  it("shows no library statistics on Home and never fetches them", async () => {
    vi.mocked(tauri.listPages).mockResolvedValue([
      page({ id: "page-current", title: "Current page" }),
    ]);

    renderHome();

    await screen.findByText("Current page");
    expect(screen.queryByTestId("wiki-context-rail")).toBeNull();
    expect(screen.queryByTestId("wiki-index-summary")).toBeNull();
    expect(screen.queryByTestId("wiki-context-pages")).toBeNull();
    expect(screen.queryByTestId("wiki-context-memories")).toBeNull();
    expect(screen.queryByTestId("wiki-context-entities")).toBeNull();
    expect(screen.queryByText(i18n.t("home.memories"))).toBeNull();
    expect(screen.queryByText(i18n.t("home.entities"))).toBeNull();
    expect(tauri.getMemoryStats).not.toHaveBeenCalled();
    expect(tauri.listEntities).not.toHaveBeenCalled();
  });

  it("does not flash the empty state while the pages query is still loading", async () => {
    let resolvePages!: (pages: tauri.Page[]) => void;
    vi.mocked(tauri.listPages).mockImplementation(
      () =>
        new Promise((resolve) => {
          resolvePages = resolve;
        }),
    );

    renderHome();

    // The query has not resolved yet — its data defaults to `[]`, which must
    // not be read as "no pages" and paint the empty state over a library that
    // in fact has pages.
    expect(screen.queryByTestId("wiki-page-empty")).toBeNull();
    expect(screen.queryByTestId("wiki-home")).toBeNull();

    resolvePages([
      page({ id: "page-architecture", title: "Wenlan app architecture" }),
    ]);

    expect(await screen.findByTestId("wiki-home")).toBeInTheDocument();
    expect(screen.queryByTestId("wiki-page-empty")).toBeNull();
    expect(screen.getByTestId("wiki-page-list")).toBeInTheDocument();
  });

  it("lets an empty library start writing without waiting for AI setup", async () => {
    const onCreatePage = vi.fn();
    const onOpenIntelligenceSettings = vi.fn();
    const user = userEvent.setup();
    vi.mocked(tauri.getApiKey).mockImplementation(() => new Promise(() => {}));
    vi.mocked(tauri.getResolvedRouting).mockImplementation(() => new Promise(() => {}));
    renderHome({ onCreatePage, onOpenIntelligenceSettings });
    const empty = await screen.findByTestId("wiki-page-empty");
    expect(empty).toHaveTextContent("No pages yet.");
    expect(empty).not.toHaveTextContent(/usually within a day|Pages will appear here/);
    const buttons = within(empty).getAllByRole("button");
    expect(buttons[0]).toHaveTextContent("Write a page");
    await user.click(buttons[0]);
    expect(onCreatePage).toHaveBeenCalledWith(null);
    expect(onOpenIntelligenceSettings).not.toHaveBeenCalled();
    await user.click(buttons[1]);
    expect(onOpenIntelligenceSettings).toHaveBeenCalledTimes(1);
    expect(tauri.getApiKey).not.toHaveBeenCalled();
    expect(tauri.getResolvedRouting).not.toHaveBeenCalled();
  });

  it("treats a library of nothing but entity shadow pages as empty", async () => {
    // The daemon's browse list carries a shadow page per entity by contract.
    // The empty-state switch must exclude them, or Home lists shadow pages as
    // if they were pages someone wrote.
    vi.mocked(tauri.listPages).mockResolvedValue([
      page({
        id: "shadow-lucian",
        title: "Lucian",
        creation_kind: "entity",
        entity_id: "ent-1",
      }),
    ]);

    renderHome();

    expect(await screen.findByTestId("wiki-page-empty")).toBeInTheDocument();
    expect(screen.queryByTestId("wiki-page-list")).toBeNull();
    expect(screen.queryByText("Lucian")).toBeNull();
  });

  it("keeps the empty state free of a retrievals section when there are no pages yet", async () => {
    vi.mocked(tauri.listRecentRetrievals).mockResolvedValue([
      {
        timestamp_ms: Date.now(),
        agent_name: "claude-code",
        query: "positioning",
        page_titles: ["Wenlan positioning"],
        page_ids: ["page-positioning"],
        memory_snippets: [],
      },
    ]);

    renderHome();

    const home = await screen.findByTestId("wiki-home");
    expect(await screen.findByTestId("wiki-page-empty")).toBeInTheDocument();
    expect(within(home).queryByTestId("retrievals")).toBeNull();
    expect(screen.queryByText(/Where AI looked/i)).toBeNull();
  });

  it("shows the page list and no empty state once pages exist", async () => {
    const onCreatePage = vi.fn();
    const onOpenIntelligenceSettings = vi.fn();
    vi.mocked(tauri.listPages).mockResolvedValue([
      page({ id: "page-architecture", title: "Wenlan app architecture" }),
    ]);

    renderHome({ onCreatePage, onOpenIntelligenceSettings });

    await screen.findByTestId("wiki-page-list");
    expect(screen.queryByTestId("wiki-page-empty")).toBeNull();
    expect(screen.queryByRole("button", { name: "Turn on a model" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Write a page" })).toBeNull();
    expect(
      screen.getByRole("button", { name: /open Wenlan app architecture/i }),
    ).toBeInTheDocument();
  });

  it("never asks for provider status on a home that has pages", async () => {
    // The provider triple is settings IPC, and only the empty state branches on
    // it. Read one level up it fired on every home mount, and in the
    // fixture-only Review flavor those three commands sit outside the Review
    // command contract (`review/commandCapabilities.ts`), which fails closed —
    // so the home page rejected three commands on a screen that never asks the
    // question. Keep the read inside the component that answers it.
    vi.mocked(tauri.listPages).mockResolvedValue([
      page({ id: "page-architecture", title: "Wenlan app architecture" }),
    ]);

    renderHome();

    await screen.findByTestId("wiki-page-list");
    // Give any stray query a turn of the event loop to fire before asserting.
    await waitFor(() => expect(screen.queryByTestId("wiki-page-empty")).toBeNull());
    expect(tauri.getApiKey).not.toHaveBeenCalled();
    expect(tauri.getExternalLlm).not.toHaveBeenCalled();
    expect(tauri.getOnDeviceModel).not.toHaveBeenCalled();
  });

  it("does not query model configuration to render an empty notebook", async () => {
    renderHome();
    await screen.findByTestId("wiki-page-empty");
    expect(tauri.getApiKey).not.toHaveBeenCalled();
    expect(tauri.getExternalLlm).not.toHaveBeenCalled();
    expect(tauri.getOnDeviceModel).not.toHaveBeenCalled();
    expect(tauri.getResolvedRouting).not.toHaveBeenCalled();
  });

});
