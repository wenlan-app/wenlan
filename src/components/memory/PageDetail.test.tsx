// SPDX-License-Identifier: AGPL-3.0-only
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, it, expect, vi, beforeAll, beforeEach } from "vitest";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import PageDetail from "./PageDetail";
import { i18n } from "../../i18n";
import {
  editorViewFromTextbox,
  installCodeMirrorDomPolyfills,
  pressKey,
  replaceDocument,
} from "./editor/editorTestUtils";

// Map rendering is exercised in PageDetail.tabs.test.tsx; these tests exercise
// the real editor and its autosave boundary when changing views.
vi.mock("./PageCanvas", () => ({
  default: ({ pageTitle }: { pageTitle: string }) => (
    <section aria-label={`Canvas for ${pageTitle}`} />
  ),
}));

vi.mock("../../lib/tauri", () => ({
  getKnowledgeGraph: vi.fn().mockResolvedValue({entities:[],relations:[],memories:[],memory_links:[],pages:[],page_links:[]}),
  // Fails closed, which is what an older or unreachable daemon looks like:
  // the review action stays disabled unless a test opts in.
  pageReviewSupported: vi.fn().mockResolvedValue("daemon_unsupported"),
  reviewPage: vi.fn(),
  agentDisplayName: (slug: string | null) => slug,
  getPage: vi.fn().mockResolvedValue({
    id: "concept_abc",
    title: "libSQL Architecture",
    summary: "Core database layer for Origin",
    content: "libSQL is the core database layer powering Origin's memory system, chosen for its vector support and SQLite compatibility.\n\nOrigin stores 768-dimensional embeddings in F32_BLOB columns with DiskANN indexing for fast approximate nearest neighbor search. The database also hosts the knowledge graph ([[Entity Graph]]) and full-text search via FTS5 triggers. This architecture enables hybrid retrieval combining vector similarity with keyword matching through [[Reciprocal Rank Fusion]].\n\n## Open Questions\n- Performance at scale beyond 10k memories?\n\n## Sources\n- mem_1 (2026-04-01)\n- mem_2 (2026-04-01)",
    entity_id: "entity_libsql",
    domain: "architecture",
    source_memory_ids: ["mem_1", "mem_2"],
    version: 3,
    status: "active",
    created_at: "2026-04-01T00:00:00+00:00",
    last_compiled: "2026-04-07T12:00:00+00:00",
    last_modified: "2026-04-07T12:00:00+00:00",
  }),
  getPageSources: vi.fn().mockResolvedValue([
    {
      source: { page_id: "page_abc", memory_source_id: "mem_1", linked_at: 1712000000, link_reason: "page_growth" },
      memory: {
        source_id: "mem_1",
        title: "libSQL stores vectors",
        content: "libSQL stores vectors in F32_BLOB columns",
        summary: null,
        memory_type: "fact",
        domain: "architecture",
        source_agent: "claude",
        confidence: 0.9,
        confirmed: false,
        last_modified: 1712000000,
      },
    },
    {
      source: { page_id: "page_abc", memory_source_id: "mem_2", linked_at: 1712100000, link_reason: "page_growth" },
      memory: {
        source_id: "mem_2",
        title: "DiskANN indexing strategy",
        content: "DiskANN provides fast approximate nearest neighbor search",
        summary: null,
        memory_type: "fact",
        domain: "architecture",
        source_agent: "claude-code",
        confidence: 0.85,
        confirmed: true,
        last_modified: 1712100000,
      },
    },
  ]),
  clipboardWrite: vi.fn().mockResolvedValue(undefined),
  exportPagesToObsidian: vi.fn().mockResolvedValue({ exported: 1, skipped: 0, failed: 0 }),
  exportPageToObsidian: vi.fn().mockResolvedValue({ path: "/path/to/file.md" }),
  listRegisteredSources: vi.fn().mockResolvedValue([
    { id: "obsidian-vault", source_type: "obsidian", path: "/Users/test/vault", status: "Active", last_sync: null, file_count: 10, memory_count: 20 },
  ]),
  getPageLinks: vi.fn().mockResolvedValue({ outbound: [], inbound: [] }),
  getPageRevisions: vi.fn().mockResolvedValue({
    page_id: "concept_abc",
    current_version: 3,
    user_edited: false,
    stale_reason: null,
    entries: [],
  }),
  listOrphanLinks: vi.fn().mockResolvedValue({ min_count: 2, orphan_labels: [] }),
  listPages: vi.fn().mockResolvedValue([]),
  redistillPage: vi.fn().mockResolvedValue({ status: "ok", updated: true }),
  updatePage: vi.fn().mockResolvedValue({ outcome: "saved" }),
  renamePage: vi.fn(),
  getDaemonVersion: vi.fn().mockResolvedValue("0.14.1"),
  getSystemInfo: vi.fn().mockResolvedValue({ os: "macos" }),
  daemonMeetsFloor: vi.fn().mockReturnValue(true),
  recordPageEditorDiagnostic: vi.fn().mockResolvedValue(undefined),
  deletePage: vi.fn().mockResolvedValue(undefined),
  // None of this file's delete-rejection scenarios carry an entity_id body;
  // the guard-parsing path itself is covered by PageDetail.entityGuard.test.tsx.
  parseEntityGuardError: vi.fn().mockReturnValue(null),
  FACET_COLORS: {},
  STABILITY_TIERS: {},
  getPendingRevision: vi.fn().mockResolvedValue(null),
  acceptPendingRevision: vi.fn(),
  dismissPendingRevision: vi.fn(),
  getTruthStatus: vi.fn().mockResolvedValue(null),
}));

function renderWithQuery(
  ui: React.ReactElement,
  client = new QueryClient({ defaultOptions: { queries: { retry: false } } }),
) {
  return {
    user: userEvent.setup(),
    ...render(<QueryClientProvider client={client}>{ui}</QueryClientProvider>),
  };
}

function deferred<T>() {
  let resolve!: (value: T | PromiseLike<T>) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, reject, resolve };
}

beforeAll(() => {
  installCodeMirrorDomPolyfills();
});

async function openPageInfo(user = userEvent.setup()) {
  await user.click(await screen.findByRole("button", { name: i18n.t("pageInspector.open") }));
  return screen.getByRole("dialog", { name: i18n.t("pageInspector.label") });
}

async function selectInspectorTab(user: ReturnType<typeof userEvent.setup>, name: string) {
  if (!screen.queryByRole("tablist")) {
    await user.click(screen.getByRole("button", { name: i18n.t("pageInspector.open") }));
  }
  await user.click(await screen.findByRole("tab", { name }));
}

async function makeNextPageResolvable() {
  const { getPage } = await import("../../lib/tauri");
  const getPageMock = getPage as ReturnType<typeof vi.fn>;
  const currentPage = await getPage("concept_abc");
  getPageMock.mockImplementation(async (id: string) => {
    if (id !== "concept_next") return currentPage;
    return {
      id: "concept_next",
      title: "Next Page",
      summary: null,
      content: "Next page content.",
      entity_id: null,
      domain: null,
      source_memory_ids: [],
      version: 1,
      status: "active",
      created_at: "2026-04-09T00:00:00+00:00",
      last_compiled: "2026-04-09T12:00:00+00:00",
      last_modified: "2026-04-09T12:00:00+00:00",
    };
  });
  getPageMock.mockClear();
}

describe("PageDetail", () => {
  const defaultProps = {
    pageId: "concept_abc",
    onBack: vi.fn(),
    onMemoryClick: vi.fn(),
    onPageClick: vi.fn(),
  };

  beforeEach(async () => {
    vi.clearAllMocks();
    await i18n.changeLanguage("en");
  });

  it("renders page title", async () => {
    renderWithQuery(<PageDetail {...defaultProps} />);
    expect(await screen.findByRole("heading", { level: 1, name: "libSQL Architecture" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Rename page" })).toHaveTextContent("libSQL Architecture");
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
  });

  it("opens rename from the page title and cancels without changing it", async () => {
    const { getPage, renamePage, updatePage } = await import("../../lib/tauri");
    const { user } = renderWithQuery(<PageDetail {...defaultProps} initialMode="edit" />);

    const editor = await screen.findByRole("textbox", { name: "Page editor" });
    const currentPage = await getPage("concept_abc");
    if (!currentPage) throw new Error("Missing page fixture");
    const savedPage = {
      ...currentPage,
      content: "Body saved before renaming",
      version: 4,
    };
    await vi.mocked(getPage).withImplementation(async () => savedPage, async () => {
      act(() => replaceDocument(editorViewFromTextbox(editor), "Body saved before renaming"));
      await user.click(screen.getByRole("button", { name: "Rename page" }));
      expect(await screen.findByRole("textbox", { name: "Page title" })).toHaveValue("libSQL Architecture");
      expect(updatePage).toHaveBeenCalledWith(expect.objectContaining({
        id: "concept_abc",
        content: "Body saved before renaming",
        expectedVersion: 3,
      }));

      expect(renamePage).not.toHaveBeenCalled();
      await user.click(screen.getByRole("button", { name: "Cancel rename" }));
      expect(await screen.findByRole("heading", { level: 1, name: "libSQL Architecture" })).toBeInTheDocument();
      expect(screen.getByText("Body saved before renaming")).toBeInTheDocument();
    });
  });

  it("renames a page with its current version and keeps its content", async () => {
    const { getPage, renamePage } = await import("../../lib/tauri");
    (renamePage as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
      id: "concept_abc",
      title: "Renamed architecture",
      version: 4,
    });
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    client.setQueryData(["page-links", "referencing-note"], {
      outbound: [{ label: "libSQL Architecture", target_page_id: "concept_abc", target_title: "libSQL Architecture" }],
      inbound: [],
    });
    const { user } = renderWithQuery(<PageDetail {...defaultProps} />, client);

    await screen.findByRole("heading", { level: 1, name: "libSQL Architecture" });
    const currentPage = await getPage("concept_abc");
    (getPage as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
      ...currentPage,
      title: "Renamed architecture",
      version: 4,
    });
    await user.click(screen.getByRole("button", { name: "Page actions" }));
    await user.click(screen.getByRole("menuitem", { name: "Rename page" }));
    const titleInput = screen.getByRole("textbox", { name: "Page title" });
    await user.clear(titleInput);
    await user.type(titleInput, "Renamed architecture");
    await user.click(screen.getByRole("button", { name: "Save title" }));

    expect(renamePage).toHaveBeenCalledWith("concept_abc", "Renamed architecture", 3);
    expect(await screen.findByRole("heading", { level: 1, name: "Renamed architecture" })).toBeInTheDocument();
    expect(client.getQueryState(["page-links", "referencing-note"])?.isInvalidated).toBe(true);
    expect(screen.getByText(/libSQL is the core database layer powering Origin/)).toBeInTheDocument();
  });

  it("cancels a page rename without changing the title", async () => {
    const { renamePage } = await import("../../lib/tauri");
    const { user } = renderWithQuery(<PageDetail {...defaultProps} />);

    await screen.findByRole("heading", { level: 1, name: "libSQL Architecture" });
    await user.click(screen.getByRole("button", { name: "Page actions" }));
    await user.click(screen.getByRole("menuitem", { name: "Rename page" }));
    await user.click(screen.getByRole("button", { name: "Cancel rename" }));

    expect(renamePage).not.toHaveBeenCalled();
    expect(screen.getByRole("heading", { level: 1, name: "libSQL Architecture" })).toBeInTheDocument();
  });

  it("keeps a failed page rename retryable with the proposed title", async () => {
    const { renamePage } = await import("../../lib/tauri");
    (renamePage as ReturnType<typeof vi.fn>).mockRejectedValueOnce(new Error("daemon offline"));
    const { user } = renderWithQuery(<PageDetail {...defaultProps} />);

    await screen.findByRole("heading", { level: 1, name: "libSQL Architecture" });
    await user.click(screen.getByRole("button", { name: "Page actions" }));
    await user.click(screen.getByRole("menuitem", { name: "Rename page" }));
    await user.clear(screen.getByRole("textbox", { name: "Page title" }));
    await user.type(screen.getByRole("textbox", { name: "Page title" }), "Changed title");
    await user.click(screen.getByRole("button", { name: "Save title" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("Could not rename this page. Reload and try again.");
    expect(screen.getByRole("textbox", { name: "Page title" })).toHaveValue("Changed title");
    expect(screen.getByRole("button", { name: "Reload page" })).toBeInTheDocument();
  });

  it("asks for review after a policy denial and retains the original page title", async () => {
    const { renamePage } = await import("../../lib/tauri");
    (renamePage as ReturnType<typeof vi.fn>).mockRejectedValueOnce(
      new Error("page_review_required: review this page before renaming"),
    );
    const { user } = renderWithQuery(<PageDetail {...defaultProps} />);

    await screen.findByRole("heading", { level: 1, name: "libSQL Architecture" });
    await user.click(screen.getByRole("button", { name: "Page actions" }));
    await user.click(screen.getByRole("menuitem", { name: "Rename page" }));
    await user.clear(screen.getByRole("textbox", { name: "Page title" }));
    await user.type(screen.getByRole("textbox", { name: "Page title" }), "Proposed title");
    await user.click(screen.getByRole("button", { name: "Save title" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("Review this page before renaming it.");
    expect(screen.getByRole("textbox", { name: "Page title" })).toHaveValue("Proposed title");
    expect(screen.queryByRole("button", { name: "Reload page" })).toBeNull();
    await user.click(screen.getByRole("button", { name: "Cancel rename" }));
    expect(screen.getByRole("heading", { level: 1, name: "libSQL Architecture" })).toBeInTheDocument();
  });

  it("ignores a stale rename reload after navigating to another page", async () => {
    const { getPage, renamePage } = await import("../../lib/tauri");
    const originalPage = (await getPage("concept_abc"))!;
    (renamePage as ReturnType<typeof vi.fn>).mockRejectedValueOnce(new Error("version conflict"));
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const { user, rerender } = renderWithQuery(<PageDetail {...defaultProps} />, queryClient);

    await screen.findByRole("heading", { level: 1, name: "libSQL Architecture" });
    await user.click(screen.getByRole("button", { name: "Page actions" }));
    await user.click(screen.getByRole("menuitem", { name: "Rename page" }));
    await user.clear(screen.getByRole("textbox", { name: "Page title" }));
    await user.type(screen.getByRole("textbox", { name: "Page title" }), "Old page proposal");
    await user.click(screen.getByRole("button", { name: "Save title" }));
    await screen.findByRole("alert");

    const staleReload = deferred<Awaited<ReturnType<typeof getPage>>>();
    (getPage as ReturnType<typeof vi.fn>).mockImplementationOnce(() => staleReload.promise);
    await user.click(screen.getByRole("button", { name: "Reload page" }));
    (getPage as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
      ...originalPage,
      id: "concept_next",
      title: "Next page",
      version: 8,
    });
    rerender(
      <QueryClientProvider client={queryClient}>
        <PageDetail {...defaultProps} pageId="concept_next" />
      </QueryClientProvider>,
    );
    await screen.findByRole("heading", { level: 1, name: "Next page" });

    await user.click(screen.getByRole("button", { name: "Page actions" }));
    await user.click(screen.getByRole("menuitem", { name: "Rename page" }));
    await user.clear(screen.getByRole("textbox", { name: "Page title" }));
    await user.type(screen.getByRole("textbox", { name: "Page title" }), "Renamed next page");
    await act(async () => staleReload.resolve({ ...originalPage, version: 99 }));
    await user.click(screen.getByRole("button", { name: "Save title" }));
    expect(renamePage).toHaveBeenLastCalledWith("concept_next", "Renamed next page", 8);
  });

  it.each(["entity", "source", "imported"] as const)(
    "does not offer rename for autogenerated %s pages",
    async (creationKind) => {
      const { getPage } = await import("../../lib/tauri");
      (getPage as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
        id: "concept_abc",
        title: "Generated page",
        summary: null,
        content: "Generated page body.",
        entity_id: creationKind === "entity" ? "entity_abc" : null,
        domain: null,
        source_memory_ids: [],
        version: 1,
        status: "active",
        creation_kind: creationKind,
        created_at: "2026-07-17T11:00:00+00:00",
        last_compiled: "2026-07-17T11:55:00+00:00",
        last_modified: "2026-07-17T11:59:00+00:00",
      });
      const { user } = renderWithQuery(<PageDetail {...defaultProps} />);

      await screen.findByRole("heading", { level: 1, name: "Generated page" });
      await user.click(screen.getByRole("button", { name: "Page actions" }));
      expect(screen.queryByRole("menuitem", { name: "Rename page" })).toBeNull();
    },
  );

  it("fetches the page with the explicit-browse intent, not the automatic default", async () => {
    const { getPage } = await import("../../lib/tauri");
    renderWithQuery(<PageDetail {...defaultProps} />);

    await screen.findByRole("heading", { level: 1, name: "libSQL Architecture" });

    expect(getPage).toHaveBeenCalledWith("concept_abc", "explicit");
  });

  it("opens an active page through the editor gate without stealing focus", async () => {
    const { getDaemonVersion, updatePage } = await import("../../lib/tauri");
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    renderWithQuery(<PageDetail {...defaultProps} initialMode="edit" />, client);

    const editor = await screen.findByRole("textbox", { name: "Page editor" });
    expect(editor).not.toHaveFocus();
    expect(getDaemonVersion).toHaveBeenCalledTimes(1);
    expect(updatePage).not.toHaveBeenCalled();

    // A background page refresh must not reinitialize the live editor.
    await act(async () => {
      await client.invalidateQueries({ queryKey: ["page", "concept_abc"] });
    });
    expect(screen.getByRole("textbox", { name: "Page editor" })).toBe(editor);
    expect(getDaemonVersion).toHaveBeenCalledTimes(1);
  });

  it("keeps a review-origin page in reading mode", async () => {
    const { getDaemonVersion, updatePage } = await import("../../lib/tauri");
    renderWithQuery(<PageDetail {...defaultProps} initialMode="read" />);

    expect(await screen.findByRole("heading", { level: 1, name: "libSQL Architecture" })).toBeVisible();
    expect(screen.queryByRole("textbox", { name: "Page editor" })).toBeNull();
    expect(getDaemonVersion).not.toHaveBeenCalled();
    expect(updatePage).not.toHaveBeenCalled();
  });

  it("saves a writing draft before opening the map and resumes writing on return", async () => {
    const { getPage, updatePage } = await import("../../lib/tauri");
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const { user } = renderWithQuery(<PageDetail {...defaultProps} initialMode="edit" />, client);
    const editor = await screen.findByRole("textbox", { name: "Page editor" });
    const baseline = client.getQueryData(["page", "concept_abc"]);
    (getPage as ReturnType<typeof vi.fn>).mockResolvedValueOnce({ ...baseline as object, content: "Draft for the map", version: 4 });
    act(() => replaceDocument(editorViewFromTextbox(editor), "Draft for the map"));
    await selectInspectorTab(user, i18n.t("pageCanvas.tabCanvas"));

    await screen.findByRole("region", { name: "Canvas for libSQL Architecture" });
    expect(updatePage).toHaveBeenCalledWith(expect.objectContaining({ content: "Draft for the map", expectedVersion: 3 }));
    expect(screen.queryByRole("textbox", { name: "Page editor" })).toBeNull();
    await user.click(screen.getByRole("button", { name: i18n.t("common.close") }));
    const resumed = await screen.findByRole("textbox", { name: "Page editor" });
    expect(editorViewFromTextbox(resumed).state.doc.toString()).toBe("Draft for the map");
    act(() => replaceDocument(editorViewFromTextbox(resumed), "Continue writing"));
    expect(editorViewFromTextbox(resumed).state.doc.toString()).toBe("Continue writing");
  });

  it("dismisses the title menu with Escape without leaving or saving the active draft", async () => {
    const { updatePage } = await import("../../lib/tauri");
    const { user } = renderWithQuery(<PageDetail {...defaultProps} initialMode="edit" />);
    const editor = await screen.findByRole("textbox", { name: "Page editor" });
    const trigger = screen.getByRole("button", { name: "Page actions" });
    trigger.focus();
    await user.keyboard("{ArrowDown}");
    expect(screen.getByRole("menuitem", { name: "Update from sources" })).toHaveFocus();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("menu", { name: "Page actions" })).toBeNull();
    expect(trigger).toHaveFocus();
    act(() => replaceDocument(editorViewFromTextbox(editor), "Menu Escape draft"));
    expect(screen.getByRole("textbox", { name: "Page editor" })).toBe(editor);
    expect(editorViewFromTextbox(editor).state.doc.toString()).toBe("Menu Escape draft");
    expect(updatePage).not.toHaveBeenCalled();
  });

  it("closes page information on Escape while preserving the editor and unsaved draft", async () => {
    const { updatePage } = await import("../../lib/tauri");
    renderWithQuery(<PageDetail {...defaultProps} initialMode="edit" />);
    const editor = await screen.findByRole("textbox", { name: "Page editor" });
    const trigger = screen.getByRole("button", { name: i18n.t("pageInspector.open") });
    // Control the 650 ms debounce so CI wall-clock load cannot save the draft
    // mid-test. Testing Library's async helpers wait on a setTimeout(0) these
    // fake timers never run, so the pane is driven with synchronous events.
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    try {
      act(() => replaceDocument(editorViewFromTextbox(editor), "Keep writing after information"));
      trigger.focus();
      act(() => { fireEvent.click(trigger); });
      expect(screen.getByRole("dialog", { name: i18n.t("pageInspector.label") })).toBeInTheDocument();
      const close = screen.getByRole("button", { name: "Close" });
      expect(close).toHaveFocus();
      act(() => { pressKey(close, "Escape"); });
      expect(screen.queryByRole("dialog", { name: i18n.t("pageInspector.label") })).toBeNull();
      expect(trigger).toHaveFocus();
      expect(screen.getByRole("textbox", { name: "Page editor" })).toBe(editor);
      expect(editorViewFromTextbox(editor).state.doc.toString()).toBe("Keep writing after information");
      expect(updatePage).not.toHaveBeenCalled();

      // The draft is still pending: its own debounce saves it once the delay elapses.
      await act(async () => {
        await vi.advanceTimersByTimeAsync(650);
      });
      expect(updatePage).toHaveBeenCalledWith(
        expect.objectContaining({ content: "Keep writing after information" }),
      );
    } finally {
      vi.useRealTimers();
    }
  });

  it("keeps a failed map-switch draft in the same editor", async () => {
    const { updatePage } = await import("../../lib/tauri");
    (updatePage as ReturnType<typeof vi.fn>).mockRejectedValueOnce(new Error("write failed"));
    const { user } = renderWithQuery(<PageDetail {...defaultProps} initialMode="edit" />);
    const editor = await screen.findByRole("textbox", { name: "Page editor" });
    act(() => replaceDocument(editorViewFromTextbox(editor), "Keep this draft"));
    await selectInspectorTab(user, i18n.t("pageCanvas.tabCanvas"));
    await screen.findByRole("alert");
    expect(screen.getByRole("textbox", { name: "Page editor" })).toBe(editor);
    expect(editorViewFromTextbox(editor).state.doc.toString()).toBe("Keep this draft");
    expect(screen.getByRole("tab", { name: i18n.t("pageInspector.info") })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("tab", { name: i18n.t("pageCanvas.tabCanvas") })).toHaveAttribute("aria-selected", "false");
    expect(screen.queryByRole("region", { name: "Canvas for libSQL Architecture" })).toBeNull();
  });

  it("keeps an active composition in the editor when the map is requested", async () => {
    const { updatePage } = await import("../../lib/tauri");
    const { user } = renderWithQuery(<PageDetail {...defaultProps} initialMode="edit" />);
    const editor = await screen.findByRole("textbox", { name: "Page editor" });
    act(() => replaceDocument(editorViewFromTextbox(editor), "Composing draft"));
    fireEvent.compositionStart(editor);
    await selectInspectorTab(user, i18n.t("pageCanvas.tabCanvas"));
    expect(screen.getByRole("textbox", { name: "Page editor" })).toBe(editor);
    expect(editorViewFromTextbox(editor).state.doc.toString()).toBe("Composing draft");
    expect(updatePage).not.toHaveBeenCalled();
    expect(screen.queryByRole("region", { name: "Canvas for libSQL Architecture" })).toBeNull();
    expect(screen.getByRole("tab", { name: i18n.t("pageInspector.info") })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("tab", { name: i18n.t("pageCanvas.tabCanvas") })).toHaveAttribute("aria-selected", "false");
    fireEvent.compositionEnd(editor);
  });

  it("keeps a conflicting map-switch draft in the editor", async () => {
    const { getPage, updatePage } = await import("../../lib/tauri");
    const original = await getPage("concept_abc");
    const { user } = renderWithQuery(<PageDetail {...defaultProps} initialMode="edit" />);
    const editor = await screen.findByRole("textbox", { name: "Page editor" });
    (updatePage as ReturnType<typeof vi.fn>).mockResolvedValueOnce({ outcome: "conflict", message: "Remote edit" });
    (getPage as ReturnType<typeof vi.fn>).mockResolvedValueOnce({ ...original, content: "Remote document", version: 4 });
    act(() => replaceDocument(editorViewFromTextbox(editor), "Local conflicting draft"));
    await selectInspectorTab(user, i18n.t("pageCanvas.tabCanvas"));
    await screen.findByText("Latest source (version 4)");
    expect(screen.getByRole("textbox", { name: "Page editor" })).toBe(editor);
    expect(editorViewFromTextbox(editor).state.doc.toString()).toBe("Local conflicting draft");
    expect(screen.queryByRole("region", { name: "Canvas for libSQL Architecture" })).toBeNull();
    expect(screen.getByRole("tab", { name: i18n.t("pageInspector.info") })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("tab", { name: i18n.t("pageCanvas.tabCanvas") })).toHaveAttribute("aria-selected", "false");
  });

  it("disables a pending map switch and ignores its save after navigating away", async () => {
    const { getPage, updatePage } = await import("../../lib/tauri");
    const original = await getPage("concept_abc");
    const pending = deferred<{ outcome: "saved" }>();
    (updatePage as ReturnType<typeof vi.fn>).mockReturnValueOnce(pending.promise);
    await makeNextPageResolvable();
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const { rerender, user } = renderWithQuery(<PageDetail {...defaultProps} initialMode="edit" />, client);
    const editor = await screen.findByRole("textbox", { name: "Page editor" });
    act(() => replaceDocument(editorViewFromTextbox(editor), "Pending map draft"));
    await selectInspectorTab(user, i18n.t("pageCanvas.tabCanvas"));
    const pendingMap = screen.getByRole("tab", { name: i18n.t("pageCanvas.tabCanvas") });
    expect(pendingMap).toBeDisabled();
    expect(pendingMap).toHaveAttribute("aria-selected", "false");
    expect(pendingMap).toHaveFocus();
    expect(screen.queryByRole("region", { name: "Canvas for libSQL Architecture" })).toBeNull();
    expect(screen.getByRole("textbox", { name: "Page editor" })).toBe(editor);
    await user.click(pendingMap);
    expect(updatePage).toHaveBeenCalledTimes(1);
    rerender(<QueryClientProvider client={client}><PageDetail {...defaultProps} pageId="concept_next" initialMode="edit" /></QueryClientProvider>);
    const nextEditor = await screen.findByRole("textbox", { name: "Page editor" });
    await waitFor(() => expect(editorViewFromTextbox(nextEditor).state.doc.toString()).toBe("Next page content."));
    await act(async () => pending.resolve({ outcome: "saved" }));
    expect(screen.getByRole("textbox", { name: "Page editor" })).toBe(nextEditor);
    expect(screen.queryByRole("region", { name: "Canvas for Next Page" })).toBeNull();
    expect(screen.getByRole("button", { name: i18n.t("pageInspector.close") })).toHaveAttribute("aria-expanded", "true");
    (getPage as ReturnType<typeof vi.fn>).mockResolvedValue(original);
  });

  it("keeps the same editor after a confirmed save and background refetch", async () => {
    const { getDaemonVersion, getPage, updatePage } = await import("../../lib/tauri");
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    renderWithQuery(<PageDetail {...defaultProps} initialMode="edit" />, client);
    const editor = await screen.findByRole("textbox", { name: "Page editor" });
    const baseline = client.getQueryData(["page", "concept_abc"]);
    (getPage as ReturnType<typeof vi.fn>).mockResolvedValueOnce({ ...baseline as object, content: "Revised Page body", version: 4 });
    act(() => {
      replaceDocument(editorViewFromTextbox(editor), "Revised Page body");
      pressKey(editor, "s", { ctrlKey: true });
    });

    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Saved"));
    expect(screen.getByRole("textbox", { name: "Page editor" })).toBe(editor);
    expect(updatePage).toHaveBeenCalledWith(expect.objectContaining({
      id: "concept_abc",
      content: "Revised Page body",
      expectedVersion: 3,
    }));
    await act(async () => {
      await client.invalidateQueries({ queryKey: ["page", "concept_abc"] });
    });
    expect(screen.getByRole("textbox", { name: "Page editor" })).toBe(editor);
    expect(getDaemonVersion).toHaveBeenCalledTimes(1);
  });

  it("keeps source text readable when the automatic editor gate cannot reach the daemon", async () => {
    const { getDaemonVersion, updatePage } = await import("../../lib/tauri");
    (getDaemonVersion as ReturnType<typeof vi.fn>).mockRejectedValueOnce(new Error("offline"));
    renderWithQuery(<PageDetail {...defaultProps} initialMode="edit" />);

    expect(await screen.findByText(/Page editing requires stable Wenlan daemon/)).toBeVisible();
    expect(screen.getByText(/libSQL is the core database layer/)).toBeVisible();
    expect(screen.queryByRole("textbox", { name: "Page editor" })).toBeNull();
    expect(updatePage).not.toHaveBeenCalled();
  });

  it("keeps source text readable when automatic editing needs normalization", async () => {
    const { getPage, updatePage } = await import("../../lib/tauri");
    const source = await getPage("concept_abc");
    (getPage as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
      ...source,
      content: "First line\r\nSecond line\nThird line",
    });
    renderWithQuery(<PageDetail {...defaultProps} initialMode="edit" />);

    expect(await screen.findByText(/Line-ending normalization required/)).toBeVisible();
    expect(screen.getByText(/First line/)).toBeVisible();
    expect(screen.queryByRole("textbox", { name: "Page editor" })).toBeNull();
    expect(updatePage).not.toHaveBeenCalled();
  });

  it("names and dismisses the destination Page when authored content attaches to it", async () => {
    const onDismissAttachedPageNotice = vi.fn();
    const { user } = renderWithQuery(
      <PageDetail
        {...defaultProps}
        onDismissAttachedPageNotice={onDismissAttachedPageNotice}
        showAttachedPageNotice
      />,
    );

    const notice = await screen.findByRole("status", { name: "Added to “libSQL Architecture”" });
    expect(notice).toHaveTextContent("Added to “libSQL Architecture”");
    await user.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(onDismissAttachedPageNotice).toHaveBeenCalledOnce();
  });

  it("reports the loaded Page once so visit history records real opens", async () => {
    const onPageLoaded = vi.fn();
    renderWithQuery(<PageDetail {...defaultProps} onPageLoaded={onPageLoaded} />);

    await screen.findByRole("heading", { level: 1, name: "libSQL Architecture" });
    await waitFor(() => expect(onPageLoaded).toHaveBeenCalledWith({
      id: "concept_abc",
      status: "active",
      title: "libSQL Architecture",
    }));
    expect(onPageLoaded).toHaveBeenCalledTimes(1);
  });

  it("renders meta line with distilled time", async () => {
    renderWithQuery(<PageDetail {...defaultProps} />);
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(screen.queryByText(/Last organized/)).toBeNull();
    const info = await openPageInfo();
    expect(await screen.findByText(/Last organized/)).toBeTruthy();
    expect(await screen.findByText(/from 2 memories/)).toBeTruthy();

    const dateline = info.querySelector(".note-info-dateline");
    const items = dateline?.querySelectorAll(".page-detail-dateline-item");
    expect(items).toHaveLength(2);
    expect(Array.from(items ?? []).map((item) => item.textContent)).toEqual([
      expect.stringMatching(/^Last organized \S+(?: \S+)?$/),
      "from 2 memories",
    ]);
    expect(dateline?.querySelector(".page-detail-dateline-separator")).toBeNull();

    const css = readFileSync(resolve("src/index.css"), "utf8");
    expect(css).toMatch(
      /\.page-detail-dateline-item\s*\{[^}]*white-space:\s*nowrap;/s,
    );
    expect(css).toMatch(
      /@media \(max-width:\s*599px\)\s*\{[\s\S]*?\.page-detail-dateline-item\s*\+\s*\.page-detail-dateline-item::before\s*\{[^}]*display:\s*none;/,
    );
  });

  it("renders last distilled info", async () => {
    renderWithQuery(<PageDetail {...defaultProps} />);
    await openPageInfo();
    expect(await screen.findByText(/Last organized/)).toBeTruthy();
  });

  it("renders source memory count", async () => {
    renderWithQuery(<PageDetail {...defaultProps} />);
    await openPageInfo();
    expect(await screen.findByText(/from 2 memories/)).toBeTruthy();
  });

  it("removes copy as context and keeps export in the title menu", async () => {
    const { user, container } = renderWithQuery(<PageDetail {...defaultProps} />);
    await screen.findByText("libSQL Architecture");
    expect(container.querySelector(".page-detail-header-actions")).toBeNull();
    await user.click(screen.getByRole("button", { name: "Page actions" }));
    expect(screen.queryByRole("menuitem", { name: "Copy as context" })).toBeNull();
    expect(screen.getByRole("menuitem", { name: "Export to Obsidian" })).toBeVisible();
  });

  it("offers one keyboard-safe title menu for all page tools", async () => {
    const { deletePage } = await import("../../lib/tauri");
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(false);
    const { user, container } = renderWithQuery(<PageDetail {...defaultProps} />);
    await screen.findByText("libSQL Architecture");

    expect(container.querySelector(".page-detail-primary-action")).toBeNull();
    expect(container.querySelector('button[title="Delete page"]')).toBeNull();

    const trigger = screen.getByRole("button", { name: "Page actions" });
    await user.click(trigger);
    const menu = screen.getByRole("menu", { name: "Page actions" });
    expect(within(menu).getByRole("menuitem", { name: "Update from sources" })).toBeInTheDocument();
    expect(within(menu).queryByRole("menuitem", { name: "Copy as context" })).toBeNull();
    expect(within(menu).getByRole("menuitem", { name: "Delete page" })).toBeInTheDocument();

    expect(within(menu).queryByRole("menuitem", { name: "Mind map" })).toBeNull();
    expect(within(menu).queryByRole("menuitem", { name: i18n.t("pageInspector.info") })).toBeNull();
    expect(within(menu).getByRole("menuitem", { name: "Edit page" })).toBeVisible();
    // The first enabled page action receives focus.
    expect(within(menu).getByRole("menuitem", { name: "Edit page" })).toHaveFocus();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("menu", { name: "Page actions" })).toBeNull();
    expect(trigger).toHaveFocus();

    await user.click(trigger);
    fireEvent.mouseDown(document.body);
    expect(screen.queryByRole("menu", { name: "Page actions" })).toBeNull();

    await user.click(trigger);
    await user.click(screen.getByRole("menuitem", { name: "Delete page" }));
    expect(confirmSpy).toHaveBeenCalledWith("Delete this page?");
    expect(deletePage).not.toHaveBeenCalled();

    confirmSpy.mockReturnValue(true);
    await user.click(trigger);
    await user.click(screen.getByRole("menuitem", { name: "Delete page" }));
    await waitFor(() => expect(deletePage).toHaveBeenCalledWith("concept_abc"));

    const css = readFileSync(resolve("src/index.css"), "utf8");
    expect(css).toMatch(
      /@media \(max-width:\s*599px\)\s*\{[\s\S]*?\.page-detail-icon-actions\s*\{[^}]*display:\s*none;[\s\S]*?\.page-detail-primary-action\s*\{[^}]*display:\s*inline-flex;/,
    );
    confirmSpy.mockRestore();
  });

  it("returns to the Wiki inventory and invalidates Page queries after deletion", async () => {
    const { deletePage } = await import("../../lib/tauri");
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(true);
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const invalidateSpy = vi.spyOn(queryClient, "invalidateQueries");
    const { user } = renderWithQuery(<PageDetail {...defaultProps} />, queryClient);

    try {
      await screen.findByText("libSQL Architecture");
      await user.click(screen.getByRole("button", { name: "Page actions" }));
      await user.click(screen.getByRole("menuitem", { name: "Delete page" }));

      await waitFor(() => expect(deletePage).toHaveBeenCalledWith("concept_abc"));
      await waitFor(() => expect(defaultProps.onBack).toHaveBeenCalledOnce());
      expect(invalidateSpy).toHaveBeenCalledWith({ queryKey: ["pages"] });
    } finally {
      confirmSpy.mockRestore();
    }
  });

  it("surfaces deletion failures without navigating away and remains retryable", async () => {
    const { deletePage } = await import("../../lib/tauri");
    (deletePage as ReturnType<typeof vi.fn>).mockRejectedValueOnce(new Error("daemon offline"));
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(true);
    const { user } = renderWithQuery(<PageDetail {...defaultProps} />);

    try {
      await screen.findByText("libSQL Architecture");
      await user.click(screen.getByRole("button", { name: "Page actions" }));
      await user.click(screen.getByRole("menuitem", { name: "Delete page" }));

      expect(await screen.findByRole("alert")).toHaveTextContent(
        "Could not delete this page. Try again.",
      );
      expect(defaultProps.onBack).not.toHaveBeenCalled();

      await user.click(screen.getByRole("button", { name: "Page actions" }));
      const retry = screen.getByRole("menuitem", { name: "Delete page" });
      expect(retry).toBeEnabled();
      await user.click(retry);

      await waitFor(() => expect(deletePage).toHaveBeenCalledTimes(2));
      await waitFor(() => expect(defaultProps.onBack).toHaveBeenCalledOnce());
    } finally {
      confirmSpy.mockRestore();
    }
  });

  it("does not navigate away from a newer Page when an older deletion finishes", async () => {
    const { deletePage } = await import("../../lib/tauri");
    const pending = deferred<void>();
    (deletePage as ReturnType<typeof vi.fn>).mockReturnValueOnce(pending.promise);
    await makeNextPageResolvable();
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(true);
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const { rerender, user } = renderWithQuery(
      <PageDetail {...defaultProps} />,
      queryClient,
    );

    try {
      await screen.findByRole("heading", { level: 1, name: "libSQL Architecture" });
      await user.click(screen.getByRole("button", { name: "Page actions" }));
      await user.click(screen.getByRole("menuitem", { name: "Delete page" }));
      await waitFor(() => expect(deletePage).toHaveBeenCalledWith("concept_abc"));

      rerender(
        <QueryClientProvider client={queryClient}>
          <PageDetail {...defaultProps} pageId="concept_next" />
        </QueryClientProvider>,
      );
      await screen.findByRole("heading", { level: 1, name: "Next Page" });

      await act(async () => pending.resolve());
      expect(defaultProps.onBack).not.toHaveBeenCalled();
      expect(screen.getByRole("heading", { level: 1, name: "Next Page" })).toBeVisible();
    } finally {
      confirmSpy.mockRestore();
    }
  });

  it("does not show an older Page save failure on a newer Page", async () => {
    const { updatePage } = await import("../../lib/tauri");
    const pending = deferred<void>();
    (updatePage as ReturnType<typeof vi.fn>).mockReturnValueOnce(pending.promise);
    await makeNextPageResolvable();
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const { rerender, user } = renderWithQuery(
      <PageDetail {...defaultProps} />,
      queryClient,
    );

    await screen.findByRole("heading", { level: 1, name: "libSQL Architecture" });
    await user.click(screen.getByRole("button", { name: "Page actions" }));
    await user.click(screen.getByRole("menuitem", { name: "Edit page" }));
    const editor = await screen.findByRole("textbox", { name: "Page editor" });
    await waitFor(() => expect(editor).toHaveFocus());
    act(() => replaceDocument(editorViewFromTextbox(editor), "Revised Page body"));
    act(() => pressKey(editor, "s", { ctrlKey: true }));
    await waitFor(() =>
      expect(updatePage).toHaveBeenCalledWith(
        expect.objectContaining({
          id: "concept_abc",
          content: "Revised Page body",
          expectedVersion: 3,
          callerId: "wenlan-app",
          operationId: expect.any(String),
        }),
      ),
    );

    rerender(
      <QueryClientProvider client={queryClient}>
        <PageDetail {...defaultProps} pageId="concept_next" />
      </QueryClientProvider>,
    );
    await screen.findByRole("heading", { level: 1, name: "Next Page" });

    await act(async () => {
      pending.reject(new Error("late write failure"));
      await pending.promise.catch(() => undefined);
    });
    expect(screen.queryByText("Could not save this page. Try again.")).toBeNull();
    expect(screen.queryByRole("textbox")).toBeNull();
  });

  it("does not open an older Page editor when its readiness check finishes after navigating away and back", async () => {
    const { daemonMeetsFloor, getDaemonVersion, updatePage } =
      await import("../../lib/tauri");
    const pendingVersion = deferred<string>();
    (getDaemonVersion as ReturnType<typeof vi.fn>).mockReturnValueOnce(
      pendingVersion.promise,
    );
    await makeNextPageResolvable();
    const queryClient = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    const { rerender, user } = renderWithQuery(
      <PageDetail {...defaultProps} />,
      queryClient,
    );

    await screen.findByRole("heading", {
      level: 1,
      name: "libSQL Architecture",
    });
    await user.click(screen.getByRole("button", { name: "Page actions" }));
    await user.click(screen.getByRole("menuitem", { name: "Edit page" }));

    rerender(
      <QueryClientProvider client={queryClient}>
        <PageDetail {...defaultProps} pageId="concept_next" />
      </QueryClientProvider>,
    );
    await screen.findByRole("heading", { level: 1, name: "Next Page" });

    rerender(
      <QueryClientProvider client={queryClient}>
        <PageDetail {...defaultProps} />
      </QueryClientProvider>,
    );
    await screen.findByRole("heading", {
      level: 1,
      name: "libSQL Architecture",
    });

    await act(async () => {
      pendingVersion.resolve("0.14.1");
      await pendingVersion.promise;
      await new Promise((resolve) => setTimeout(resolve, 0));
    });

    expect(daemonMeetsFloor).not.toHaveBeenCalled();
    expect(
      screen.queryByRole("textbox", { name: "Page editor" }),
    ).toBeNull();
    await user.click(screen.getByRole("button", { name: "Page actions" }));
    expect(screen.getByRole("menuitem", { name: "Edit page" })).toBeVisible();
    expect(
      screen.getByRole("heading", {
        level: 1,
        name: "libSQL Architecture",
      }),
    ).toBeVisible();
    expect(updatePage).not.toHaveBeenCalled();
  });

  it("owns body-targeted Escape while editor readiness is still pending", async () => {
    const { daemonMeetsFloor, getDaemonVersion } =
      await import("../../lib/tauri");
    const pendingVersion = deferred<string>();
    (getDaemonVersion as ReturnType<typeof vi.fn>).mockReturnValueOnce(
      pendingVersion.promise,
    );
    const { user } = renderWithQuery(<PageDetail {...defaultProps} />);

    await screen.findByRole("heading", {
      level: 1,
      name: "libSQL Architecture",
    });
    await user.click(screen.getByRole("button", { name: "Page actions" }));
    await user.click(screen.getByRole("menuitem", { name: "Edit page" }));
    expect(await screen.findByText("Checking editor compatibility…")).toBeVisible();

    fireEvent.keyDown(document.body, { key: "Escape" });

    expect(defaultProps.onBack).not.toHaveBeenCalled();
    await user.click(await screen.findByRole("button", { name: "Page actions" }));
    expect(screen.getByRole("menuitem", { name: "Edit page" })).toBeVisible();
    expect(
      screen.queryByText("Checking editor compatibility…"),
    ).toBeNull();

    await act(async () => {
      pendingVersion.resolve("0.14.1");
      await pendingVersion.promise;
      await new Promise((resolve) => setTimeout(resolve, 0));
    });

    expect(daemonMeetsFloor).not.toHaveBeenCalled();
    expect(
      screen.queryByRole("textbox", { name: "Page editor" }),
    ).toBeNull();
  });

  it("serializes repeated save shortcuts so an older body cannot finish last", async () => {
    const { updatePage } = await import("../../lib/tauri");
    const pending = deferred<{ outcome: "saved" }>();
    (updatePage as ReturnType<typeof vi.fn>).mockReturnValueOnce(pending.promise);
    const { user } = renderWithQuery(<PageDetail {...defaultProps} />);

    await screen.findByRole("heading", { level: 1, name: "libSQL Architecture" });
    await user.click(screen.getByRole("button", { name: "Page actions" }));
    await user.click(screen.getByRole("menuitem", { name: "Edit page" }));
    const editor = await screen.findByRole("textbox", { name: "Page editor" });
    await waitFor(() => expect(editor).toHaveFocus());
    act(() => replaceDocument(editorViewFromTextbox(editor), "One serialized Page body"));

    try {
      act(() => {
        pressKey(editor, "s", { ctrlKey: true });
        pressKey(editor, "s", { ctrlKey: true });
      });

      await waitFor(() =>
        expect(updatePage).toHaveBeenCalledWith(
          expect.objectContaining({
            id: "concept_abc",
            content: "One serialized Page body",
            expectedVersion: 3,
            callerId: "wenlan-app",
            operationId: expect.any(String),
          }),
        ),
      );
      expect(updatePage).toHaveBeenCalledTimes(1);
      expect(editor).toHaveAttribute("contenteditable", "true");
    } finally {
      await act(async () => pending.resolve({ outcome: "saved" }));
    }
  });

  it("does not show an older re-distill result on a newer Page", async () => {
    const { redistillPage } = await import("../../lib/tauri");
    const pending = deferred<{ status: "ok"; updated: true }>();
    (redistillPage as ReturnType<typeof vi.fn>).mockReturnValueOnce(pending.promise);
    await makeNextPageResolvable();
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const { rerender, user } = renderWithQuery(
      <PageDetail {...defaultProps} />,
      queryClient,
    );

    await screen.findByRole("heading", { level: 1, name: "libSQL Architecture" });
    await user.click(screen.getByRole("button", { name: "Page actions" }));
    await user.click(screen.getByRole("menuitem", { name: "Update from sources" }));
    await waitFor(() => expect(redistillPage).toHaveBeenCalledWith("concept_abc"));

    // While the re-distill promise is still unresolved the button is busy.
    await user.click(screen.getByRole("button", { name: "Page actions" }));
    expect(screen.getByRole("menuitem", { name: "Updating from sources" })).toBeDisabled();

    rerender(
      <QueryClientProvider client={queryClient}>
        <PageDetail {...defaultProps} pageId="concept_next" />
      </QueryClientProvider>,
    );
    await screen.findByRole("heading", { level: 1, name: "Next Page" });

    await act(async () => pending.resolve({ status: "ok", updated: true }));
    expect(screen.queryByText("Page updated from its sources.")).toBeNull();
  });

  it("distinguishes a Page load failure from not found and retries in place", async () => {
    const { getPage } = await import("../../lib/tauri");
    (getPage as ReturnType<typeof vi.fn>).mockRejectedValueOnce(new Error("daemon offline"));
    const { user } = renderWithQuery(<PageDetail {...defaultProps} />);

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Could not load this page.",
    );
    expect(screen.queryByText("Page not found")).toBeNull();

    await user.click(screen.getByRole("button", { name: "Try again" }));
    expect(await screen.findByRole("heading", { level: 1, name: "libSQL Architecture" })).toBeVisible();
  });

  it("keeps cached Page content visible when a background refetch fails", async () => {
    const { getPage } = await import("../../lib/tauri");
    const getPageMock = getPage as ReturnType<typeof vi.fn>;
    const cachedPage = await getPage("concept_abc");
    getPageMock.mockClear();
    getPageMock.mockRejectedValueOnce(new Error("background refresh failed"));
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    queryClient.setQueryData(["page", "concept_abc"], cachedPage);

    renderWithQuery(<PageDetail {...defaultProps} />, queryClient);

    expect(
      await screen.findByRole("heading", { level: 1, name: "libSQL Architecture" }),
    ).toBeVisible();
    await waitFor(() =>
      expect(
        queryClient.getQueryState(["page", "concept_abc"])?.error,
      ).toEqual(expect.any(Error)),
    );
    expect(
      screen.getByRole("heading", { level: 1, name: "libSQL Architecture" }),
    ).toBeVisible();
    expect(screen.queryByText("Could not load this page.")).toBeNull();
  });

  it("keeps the editor open and retryable when saving fails", async () => {
    const { getPage, updatePage } = await import("../../lib/tauri");
    const original = await getPage("concept_abc");
    (updatePage as ReturnType<typeof vi.fn>).mockRejectedValueOnce(new Error("write failed"));
    const { user } = renderWithQuery(<PageDetail {...defaultProps} />);

    await screen.findByText("libSQL Architecture");
    await user.click(screen.getByRole("button", { name: "Page actions" }));
    await user.click(screen.getByRole("menuitem", { name: "Edit page" }));
    const editor = await screen.findByRole("textbox", { name: "Page editor" });
    await waitFor(() => expect(editor).toHaveFocus());
    act(() => replaceDocument(editorViewFromTextbox(editor), "Revised Page body"));
    act(() => pressKey(editor, "s", { ctrlKey: true }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Wenlan could not reach the daemon. Check that it is running, then retry.",
    );
    expect(editor).toBeVisible();

    (getPage as ReturnType<typeof vi.fn>).mockResolvedValueOnce({ ...original, content: "Revised Page body", version: 4 });
    await user.click(screen.getByRole("button", { name: "Retry" }));
    await waitFor(() => expect(updatePage).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Saved"));
    expect(screen.getByRole("textbox", { name: "Page editor" })).toBe(editor);
  });

  it("surfaces export failures and lets the user retry", async () => {
    const { exportPageToObsidian } = await import("../../lib/tauri");
    (exportPageToObsidian as ReturnType<typeof vi.fn>).mockRejectedValueOnce(new Error("vault unavailable"));
    const { user } = renderWithQuery(<PageDetail {...defaultProps} />);

    await screen.findByText("libSQL Architecture");
    await user.click(screen.getByRole("button", { name: "Page actions" }));
    await user.click(screen.getByRole("menuitem", { name: "Export to Obsidian" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Could not export this page. Try again.",
    );

    await user.click(screen.getByRole("button", { name: "Page actions" }));
    await user.click(screen.getByRole("menuitem", { name: "Export to Obsidian" }));
    await waitFor(() => expect(exportPageToObsidian).toHaveBeenCalledTimes(2));
    await user.click(screen.getByRole("button", { name: "Page actions" }));
    expect(await screen.findByRole("menuitem", { name: "Exported!" })).toBeVisible();
  });

  it("keeps Page action menu keyboard events out of Main history and manages item focus", async () => {
    const windowEscapeObserver = vi.fn();
    window.addEventListener("keydown", windowEscapeObserver);

    try {
      const { user } = renderWithQuery(<PageDetail {...defaultProps} />);
      await screen.findByText("libSQL Architecture");

      const trigger = screen.getByRole("button", { name: "Page actions" });
      await user.click(trigger);
      const menu = screen.getByRole("menu", { name: "Page actions" });
      const items = within(menu).getAllByRole("menuitem").filter((item) => !item.hasAttribute("disabled"));

      expect(items[0]).toHaveFocus();
      await user.keyboard("{ArrowDown}");
      expect(items[1]).toHaveFocus();
      await user.keyboard("{End}");
      expect(items[items.length - 1]).toHaveFocus();
      await user.keyboard("{ArrowDown}");
      expect(items[0]).toHaveFocus();
      await user.keyboard("{ArrowUp}");
      expect(items[items.length - 1]).toHaveFocus();
      await user.keyboard("{Home}");
      expect(items[0]).toHaveFocus();

      await user.keyboard("{Escape}");
      expect(screen.queryByRole("menu", { name: "Page actions" })).toBeNull();
      expect(trigger).toHaveFocus();
      expect(windowEscapeObserver).not.toHaveBeenCalled();
    } finally {
      window.removeEventListener("keydown", windowEscapeObserver);
    }
  });

  it("opens Page actions from its trigger with ArrowDown or ArrowUp at the matching boundary", async () => {
    const { user } = renderWithQuery(<PageDetail {...defaultProps} />);
    await screen.findByText("libSQL Architecture");

    const trigger = screen.getByRole("button", { name: "Page actions" });
    trigger.focus();
    await user.keyboard("{ArrowDown}");
    let menu = screen.getByRole("menu", { name: "Page actions" });
    let items = within(menu).getAllByRole("menuitem").filter((item) => !item.hasAttribute("disabled"));
    expect(items[0]).toHaveFocus();

    await user.keyboard("{Escape}");
    await user.keyboard("{ArrowUp}");
    menu = screen.getByRole("menu", { name: "Page actions" });
    items = within(menu).getAllByRole("menuitem").filter((item) => !item.hasAttribute("disabled"));
    expect(items[items.length - 1]).toHaveFocus();
  });

  it("focuses the first rendered Page action instead of a CSS-hidden mobile item", async () => {
    const rectsSpy = vi
      .spyOn(HTMLElement.prototype, "getClientRects")
      .mockImplementation(function (this: HTMLElement) {
        return (
          this.classList.contains("page-detail-mobile-menu-item")
            ? []
            : [{}]
        ) as unknown as DOMRectList;
      });

    try {
      const { user } = renderWithQuery(<PageDetail {...defaultProps} />);
      await screen.findByText("libSQL Architecture");

      await user.click(screen.getByRole("button", { name: "Page actions" }));
      expect(screen.getByRole("menuitem", { name: "Edit page" })).toHaveFocus();
    } finally {
      rectsSpy.mockRestore();
    }
  });

  it.each([
    ["en", "Last organized 5m ago", "from 2 memories", "needs review"],
    ["zh-Hans", "上次整理：5 分钟前", "来自 2 条记忆", "需要审核"],
    ["zh-Hant", "上次整理：5 分鐘前", "來自 2 則記憶", "需要審核"],
  ] as const)(
    "localizes the full Page dateline in %s",
    async (locale, distilled, sources, stale) => {
      const { getPage } = await import("../../lib/tauri");
      (getPage as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
        id: "concept_abc",
        title: "Localized Page",
        summary: null,
        content: "Localized page content.",
        entity_id: null,
        domain: null,
        source_memory_ids: ["mem_1", "mem_2"],
        version: 1,
        status: "active",
        created_at: "2026-07-17T11:00:00+00:00",
        last_compiled: "2026-07-17T11:55:00+00:00",
        last_modified: "2026-07-17T11:55:00+00:00",
        stale_reason: "source_conflict",
      });
      const nowSpy = vi.spyOn(Date, "now").mockReturnValue(
        new Date("2026-07-17T12:00:00+00:00").getTime(),
      );
      await i18n.changeLanguage(locale);

      try {
        renderWithQuery(<PageDetail {...defaultProps} />);

        await openPageInfo();
        expect(await screen.findByText(distilled)).toBeInTheDocument();
        expect(screen.getByText(sources)).toBeInTheDocument();
        expect(screen.getByText(stale)).toBeInTheDocument();
        if (locale !== "en") {
          expect(screen.queryByText("Last organized 5m ago")).toBeNull();
          expect(screen.queryByText("from 2 memories")).toBeNull();
          expect(screen.queryByText("needs review")).toBeNull();
        }
      } finally {
        nowSpy.mockRestore();
      }
    },
  );

  it.each([
    ["source", false, "Last updated 5m ago"],
    ["imported", false, "Last updated 5m ago"],
    ["authored", false, "Last updated 5m ago"],
    ["distilled", true, "Last updated 5m ago"],
  ] as const)(
    "shows a neutral updated dateline from last_modified for %s pages (user_edited=%s)",
    async (creationKind, userEdited, dateline) => {
      const { getPage } = await import("../../lib/tauri");
      (getPage as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
        id: "concept_abc",
        title: "Source Page",
        summary: null,
        content: "Imported raw Markdown source.",
        entity_id: null,
        domain: null,
        source_memory_ids: [],
        version: 1,
        status: "active",
        creation_kind: creationKind,
        user_edited: userEdited,
        created_at: "2026-07-17T11:00:00+00:00",
        // last_compiled is deliberately older: these pages must read
        // last_modified so the dateline never claims a distillation.
        last_compiled: "2026-07-17T10:00:00+00:00",
        last_modified: "2026-07-17T11:55:00+00:00",
      });
      const nowSpy = vi.spyOn(Date, "now").mockReturnValue(
        new Date("2026-07-17T12:00:00+00:00").getTime(),
      );

      try {
        renderWithQuery(<PageDetail {...defaultProps} />);

        await openPageInfo();
        expect(await screen.findByText(dateline)).toBeInTheDocument();
        expect(screen.queryByText(/Last organized/)).toBeNull();
      } finally {
        nowSpy.mockRestore();
      }
    },
  );

  it.each([
    ["en", "Last updated 5m ago"],
    ["zh-Hans", "上次更新：5 分钟前"],
    ["zh-Hant", "上次更新：5 分鐘前"],
  ] as const)(
    "localizes the source-page updated dateline in %s",
    async (locale, dateline) => {
      const { getPage } = await import("../../lib/tauri");
      (getPage as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
        id: "concept_abc",
        title: "Source Page",
        summary: null,
        content: "Imported raw Markdown source.",
        entity_id: null,
        domain: null,
        source_memory_ids: [],
        version: 1,
        status: "active",
        creation_kind: "source",
        created_at: "2026-07-17T11:00:00+00:00",
        last_compiled: "2026-07-17T10:00:00+00:00",
        last_modified: "2026-07-17T11:55:00+00:00",
      });
      const nowSpy = vi.spyOn(Date, "now").mockReturnValue(
        new Date("2026-07-17T12:00:00+00:00").getTime(),
      );
      await i18n.changeLanguage(locale);

      try {
        renderWithQuery(<PageDetail {...defaultProps} />);

        await openPageInfo();
        expect(await screen.findByText(dateline)).toBeInTheDocument();
      } finally {
        nowSpy.mockRestore();
      }
    },
  );

  it("keeps Last organized for organized pages", async () => {
    const { getPage } = await import("../../lib/tauri");
    (getPage as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
      id: "concept_abc",
      title: "Distilled Page",
      summary: null,
      content: "Distilled page content.",
      entity_id: null,
      domain: null,
      source_memory_ids: [],
      version: 1,
      status: "active",
      creation_kind: "distilled",
      created_at: "2026-07-17T11:00:00+00:00",
      last_compiled: "2026-07-17T11:55:00+00:00",
      last_modified: "2026-07-17T11:59:00+00:00",
    });
    const nowSpy = vi.spyOn(Date, "now").mockReturnValue(
      new Date("2026-07-17T12:00:00+00:00").getTime(),
    );

    try {
      renderWithQuery(<PageDetail {...defaultProps} />);

      await openPageInfo();
      expect(await screen.findByText("Last organized 5m ago")).toBeInTheDocument();
      expect(screen.queryByText(/Last updated/)).toBeNull();
    } finally {
      nowSpy.mockRestore();
    }
  });

  it("re-distills the current page and keeps skipped daemon hints visible", async () => {
    const { redistillPage } = await import("../../lib/tauri");
    const hint = "page re-distill needs an LLM in the daemon";
    (redistillPage as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
      status: "skipped",
      updated: false,
      hint,
    });
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const invalidateSpy = vi.spyOn(queryClient, "invalidateQueries");
    const { user } = renderWithQuery(<PageDetail {...defaultProps} />, queryClient);

    await screen.findByText("libSQL Architecture");
    await user.click(screen.getByRole("button", { name: "Page actions" }));
    await user.click(screen.getByRole("menuitem", { name: "Update from sources" }));

    expect(redistillPage).toHaveBeenCalledWith("concept_abc");
    expect(await screen.findByText(hint)).toBeTruthy();
    expect(invalidateSpy).toHaveBeenCalledWith({ queryKey: ["page", "concept_abc"] });
    expect(invalidateSpy).toHaveBeenCalledWith({ queryKey: ["page-revisions", "concept_abc"] });
    expect(invalidateSpy).toHaveBeenCalledWith({ queryKey: ["page-sources", "concept_abc"] });
    expect(defaultProps.onBack).not.toHaveBeenCalled();
  });

  it("surfaces the daemon's discard reason when a re-distill was blocked, not 'already up to date'", async () => {
    const { redistillPage } = await import("../../lib/tauri");
    (redistillPage as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
      status: "ok",
      updated: false,
      reason: "citation verification failed (8 verified, 13 unverified, 0 stripped)",
    });
    const { user } = renderWithQuery(<PageDetail {...defaultProps} />);

    await screen.findByText("libSQL Architecture");
    await user.click(screen.getByRole("button", { name: "Page actions" }));
    await user.click(screen.getByRole("menuitem", { name: "Update from sources" }));

    expect(
      await screen.findByText(
        "Page not updated: citation verification failed (8 verified, 13 unverified, 0 stripped)",
      ),
    ).toBeTruthy();
    expect(screen.queryByText("Page already up to date.")).toBeNull();
  });

  it("shows 'update blocked' instead of 'updating...' when the citation gate discarded the refresh", async () => {
    const { getPage } = await import("../../lib/tauri");
    const basePage = await getPage("concept_abc");
    (getPage as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
      ...basePage,
      stale_reason: "source_updated",
      refresh_blocked_reason:
        "citation verification failed (8 verified, 13 unverified, 0 stripped)",
    });
    renderWithQuery(<PageDetail {...defaultProps} />);

    await openPageInfo();
    expect(
      await screen.findByText("update blocked: citations couldn't be verified"),
    ).toBeTruthy();
    expect(screen.queryByText("updating...")).toBeNull();
  });

  it("confirms before re-distilling a user-edited page", async () => {
    const { getPage, redistillPage } = await import("../../lib/tauri");
    (getPage as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
      id: "concept_abc",
      title: "Edited Page",
      summary: null,
      content: "Edited page prose.",
      entity_id: null,
      domain: null,
      source_memory_ids: [],
      version: 4,
      status: "active",
      created_at: "2026-04-01T00:00:00+00:00",
      last_compiled: "2026-04-07T12:00:00+00:00",
      last_modified: "2026-04-08T12:00:00+00:00",
      user_edited: true,
    });
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(false);
    const { user } = renderWithQuery(<PageDetail {...defaultProps} />);

    await screen.findByText("Edited Page");
    await user.click(screen.getByRole("button", { name: "Page actions" }));
    await user.click(screen.getByRole("menuitem", { name: "Update from sources" }));

    expect(confirmSpy).toHaveBeenCalledWith(
      i18n.t("pageDetail.redistillEditedConfirm"),
    );
    expect(redistillPage).not.toHaveBeenCalled();
    confirmSpy.mockRestore();
  });

  it("clears re-distill notices when navigating between pages", async () => {
    const { getPage, redistillPage } = await import("../../lib/tauri");
    const hint = "page re-distill needs an LLM in the daemon";
    (redistillPage as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
      status: "skipped",
      updated: false,
      hint,
    });
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const { user, rerender } = renderWithQuery(<PageDetail {...defaultProps} />, queryClient);

    await screen.findByText("libSQL Architecture");
    await user.click(screen.getByRole("button", { name: "Page actions" }));
    await user.click(screen.getByRole("menuitem", { name: "Update from sources" }));
    expect(await screen.findByText(hint)).toBeTruthy();

    (getPage as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
      id: "concept_next",
      title: "Next Page",
      summary: null,
      content: "Next page content.",
      entity_id: null,
      domain: null,
      source_memory_ids: [],
      version: 1,
      status: "active",
      created_at: "2026-04-09T00:00:00+00:00",
      last_compiled: "2026-04-09T12:00:00+00:00",
      last_modified: "2026-04-09T12:00:00+00:00",
    });
    rerender(
      <QueryClientProvider client={queryClient}>
        <PageDetail {...defaultProps} pageId="concept_next" />
      </QueryClientProvider>,
    );

    await waitFor(() => expect(screen.queryByText(hint)).toBeNull());
  });

  it("gives the icon-only back button a localized accessible name", async () => {
    const { user } = renderWithQuery(<PageDetail {...defaultProps} />);
    await screen.findByText("libSQL Architecture");
    const backButton = screen.getByRole("button", { name: "Back" });
    expect(backButton.querySelector("svg")).toBeTruthy();

    await user.click(backButton);
    expect(defaultProps.onBack).toHaveBeenCalledOnce();
  });

  it("renders source memories section with count", async () => {
    renderWithQuery(<PageDetail {...defaultProps} />);
    await screen.findByText("libSQL Architecture");
    await openPageInfo();
    const info = screen.getByRole("dialog", { name: i18n.t("pageInspector.label") });
    expect(within(info).getByText(/from 2 memories/)).toBeInTheDocument();
    expect(within(info).getAllByTestId("page-info-source-row")).toHaveLength(2);
  });

  it("uses the page-detail grammar class (matches MemoryDetail's dossier pattern)", async () => {
    const { container } = renderWithQuery(<PageDetail {...defaultProps} />);
    await screen.findByText("libSQL Architecture");
    const topDiv = container.firstElementChild as HTMLElement;
    expect(topDiv.className).toContain("page-detail");
    expect(topDiv.className).not.toContain("h-screen");
  });

  it("shows Page Links from daemon page links without listPages inference", async () => {
    const { getPageLinks, listPages: mockList } = await import("../../lib/tauri");
    (getPageLinks as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
      outbound: [{ label: "Entity Graph", target_page_id: "concept_eg" }],
      inbound: [],
    });
    renderWithQuery(<PageDetail {...defaultProps} />);
    await openPageInfo();
    expect(await screen.findByLabelText(i18n.t("knowledgeContext.linkedPages"))).toBeTruthy();
    const entityEls = await screen.findAllByText("Entity Graph");
    const cardSpan = entityEls.find((el) => el.tagName === "SPAN");
    expect(cardSpan).toBeTruthy();
    expect(mockList).not.toHaveBeenCalled();
  });

  it("hides Page Links when the daemon returns no links", async () => {
    const { listPages: mockList } = await import("../../lib/tauri");
    renderWithQuery(<PageDetail {...defaultProps} />);
    await screen.findByText("libSQL Architecture");
    await openPageInfo();
    expect(screen.queryByLabelText(i18n.t("knowledgeContext.linkedPages"))).toBeNull();
    expect(screen.getByRole("dialog", { name: i18n.t("pageInspector.label") })).toBeInTheDocument();
    expect(mockList).not.toHaveBeenCalled();
  });

  it("hides Page Links section when no daemon links in content", async () => {
    const { getPage } = await import("../../lib/tauri");
    (getPage as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
      id: "concept_abc",
      title: "Simple Page",
      summary: "No links here",
      content: "A simple page with no wikilinks.\n\n## Open Questions\n- None",
      entity_id: null,
      domain: null,
      source_memory_ids: [],
      version: 1,
      status: "active",
      created_at: "2026-04-01T00:00:00+00:00",
      last_compiled: "2026-04-07T12:00:00+00:00",
      last_modified: "2026-04-07T12:00:00+00:00",
    });
    renderWithQuery(<PageDetail {...defaultProps} />);
    await screen.findByText("Simple Page");
    await openPageInfo();
    expect(screen.queryByLabelText(i18n.t("knowledgeContext.linkedPages"))).toBeNull();
    expect(screen.getByRole("dialog", { name: i18n.t("pageInspector.label") })).toBeInTheDocument();
  });

  it("renders one evidence card per source memory after fetch", async () => {
    const { user } = renderWithQuery(<PageDetail {...defaultProps} />);
    await screen.findByText("libSQL Architecture");
    await openPageInfo(user);
    expect(screen.getAllByTestId("page-info-source-row")).toHaveLength(2);
  });

  it("clicking an evidence card calls onMemoryClick with the right source_id", async () => {
    const { user } = renderWithQuery(<PageDetail {...defaultProps} />);
    await screen.findByText("libSQL Architecture");
    await openPageInfo(user);
    const row = screen
      .getByText("libSQL stores vectors")
      .closest('[data-testid="page-info-source-row"]')!;
    await user.click(within(row as HTMLElement).getByRole("button"));
    expect(defaultProps.onMemoryClick).toHaveBeenCalledWith("mem_1");
  });

  it("uses getPageSources (join table) not listMemoriesByIds", async () => {
    const { getPageSources } = await import("../../lib/tauri");
    renderWithQuery(<PageDetail {...defaultProps} />);
    await openPageInfo();
    await screen.findByText("libSQL stores vectors");
    expect(getPageSources).toHaveBeenCalledTimes(1);
    expect(getPageSources).toHaveBeenCalledWith("concept_abc");
  });

  describe("marking a page reviewed", () => {
    async function openPageActions(user: ReturnType<typeof userEvent.setup>) {
      await screen.findByText("libSQL Architecture");
      await openPageInfo(user);
      return screen.getByRole("button", { name: "Mark page reviewed" });
    }

    it("stays disabled while the daemon cannot record reviews", async () => {
      // The gate fails closed: `pageReviewSupported` resolves false in the
      // module mock, which is what a pre-cutover or unreachable daemon looks
      // like. Marking a page reviewed there records something no surface can
      // show, so the action explains itself rather than pretending to work.
      const { user } = renderWithQuery(<PageDetail {...defaultProps} />);

      const action = await openPageActions(user);

      expect(action).toBeDisabled();
      expect(action).toHaveAttribute(
        "title",
        "This version of Wenlan cannot record page reviews yet.",
      );
    });

    it("blames the platform, not the daemon, when this build cannot mint", async () => {
      // Upgrading the daemon cannot fix a platform that has no owner-only
      // secret protection, so the daemon copy would send the reader after the
      // wrong thing entirely.
      const { pageReviewSupported } = await import("../../lib/tauri");
      (pageReviewSupported as ReturnType<typeof vi.fn>).mockResolvedValue(
        "platform_unsupported",
      );
      const { user } = renderWithQuery(<PageDetail {...defaultProps} />);

      const action = await openPageActions(user);

      expect(action).toBeDisabled();
      await waitFor(() =>
        expect(action).toHaveAttribute(
          "title",
          "Marking pages reviewed is not available on this platform yet.",
        ),
      );
    });

    it("keeps the stale warning up when the reload itself fails", async () => {
      // The bad state this prevents: warning gone, same stale text on screen,
      // and a Review action that looks ready to approve content the daemon has
      // already refused once.
      const { pageReviewSupported, reviewPage, getPage } = await import("../../lib/tauri");
      (pageReviewSupported as ReturnType<typeof vi.fn>).mockResolvedValue("ready");
      (reviewPage as ReturnType<typeof vi.fn>).mockResolvedValue({ kind: "stale" });
      const { user } = renderWithQuery(<PageDetail {...defaultProps} />);

      const action = await openPageActions(user);
      await waitFor(() => expect(action).toBeEnabled());
      await user.click(action);
      await screen.findByText(/This page changed after you opened it/);

      // Once, not permanently: `clearAllMocks` between tests resets calls but
      // keeps implementations, so a lasting rejection here would fail every
      // test that runs after this one.
      (getPage as ReturnType<typeof vi.fn>).mockRejectedValueOnce(
        new Error("daemon unreachable"),
      );
      await user.click(screen.getByRole("button", { name: "Reload" }));

      await screen.findByText(/Could not reload this page/);
      // Still offered, because trying again is the whole point.
      expect(screen.getByRole("button", { name: "Reload" })).toBeInTheDocument();
    });

    it("sends the content on screen, not just the page id", async () => {
      // The mark is bound to the exact text the reader had in front of them.
      // Passing anything else either fails as stale or, worse, attributes
      // their approval to content they never saw.
      const { pageReviewSupported, reviewPage, getPage } = await import("../../lib/tauri");
      (pageReviewSupported as ReturnType<typeof vi.fn>).mockResolvedValue("ready");
      (reviewPage as ReturnType<typeof vi.fn>).mockResolvedValue({
        kind: "applied",
        reviewed_page_version: 4,
      });
      const rendered = await getPage("concept_abc");
      const content = rendered?.content;
      const { user } = renderWithQuery(<PageDetail {...defaultProps} />);

      const action = await openPageActions(user);
      await waitFor(() => expect(action).toBeEnabled());
      await user.click(action);

      await screen.findByText("Marked as reviewed.");
      expect(reviewPage).toHaveBeenCalledWith("concept_abc", content);
    });

    it("offers a reload when the page moved on under the reader", async () => {
      const { pageReviewSupported, reviewPage, getPage } = await import("../../lib/tauri");
      (pageReviewSupported as ReturnType<typeof vi.fn>).mockResolvedValue("ready");
      (reviewPage as ReturnType<typeof vi.fn>).mockResolvedValue({ kind: "stale" });
      const { user } = renderWithQuery(<PageDetail {...defaultProps} />);

      const action = await openPageActions(user);
      await waitFor(() => expect(action).toBeEnabled());
      await user.click(action);

      await screen.findByText(/This page changed after you opened it/);
      const fetchesBefore = (getPage as ReturnType<typeof vi.fn>).mock.calls.length;
      await user.click(screen.getByRole("button", { name: "Reload" }));

      // The reload has to actually re-read the page; a notice that only
      // dismisses itself would leave the reader looking at the same stale text
      // and clicking review again forever.
      await waitFor(() =>
        expect((getPage as ReturnType<typeof vi.fn>).mock.calls.length).toBeGreaterThan(
          fetchesBefore,
        ),
      );
      expect(screen.queryByText(/This page changed after you opened it/)).toBeNull();
    });

    it("reads an already-recorded review as done rather than as a failure", async () => {
      const { pageReviewSupported, reviewPage } = await import("../../lib/tauri");
      (pageReviewSupported as ReturnType<typeof vi.fn>).mockResolvedValue("ready");
      (reviewPage as ReturnType<typeof vi.fn>).mockResolvedValue({ kind: "replayed" });
      const { user } = renderWithQuery(<PageDetail {...defaultProps} />);

      const action = await openPageActions(user);
      await waitFor(() => expect(action).toBeEnabled());
      await user.click(action);

      await screen.findByText("This page was already marked as reviewed.");
      expect(screen.queryByRole("button", { name: "Reload" })).toBeNull();
    });
  });
});
