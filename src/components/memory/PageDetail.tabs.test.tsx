// SPDX-License-Identifier: AGPL-3.0-only
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, cleanup, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import PageDetail from "./PageDetail";
import { i18n } from "../../i18n";

vi.mock("./sources/SourceDocumentReader", () => ({
  SourceDocumentReader: ({ file, space }: { file: { source_id: string }; space?: string }) => (
    <section aria-label="PDF reader" data-source={file.source_id} data-space={space} />
  ),
}));
vi.mock("./sources/SourceDocumentPreview", () => ({
  default: ({ file, onClose }: { file: { title: string }; onClose: () => void }) => (
    <section aria-label="External source preview">
      <span>{file.title}</span>
      <button onClick={onClose} type="button">Close source preview</button>
    </section>
  ),
}));

// Same reasoning as PageCanvas.test.tsx: React Flow needs real dimensions.
// These tests only care that the canvas view mounts, so the node state hook is
// stubbed down to plain useState — selection and dragging are exercised in
// PageCanvas.test.tsx, not here.
vi.mock("@xyflow/react", async () => {
  const React = await import("react");
  return {
    ReactFlow: ({ nodes, nodeTypes }: any) => (
      <div data-testid="react-flow">
        {nodes.map((n: any) => {
          const NodeComponent = nodeTypes[n.type];
          return <NodeComponent key={n.id} id={n.id} data={n.data} />;
        })}
      </div>
    ),
    ReactFlowProvider: ({ children }: any) => <>{children}</>,
    Background: () => null,
    Controls: () => null,
    Handle: () => null,
    Position: { Top: "top", Bottom: "bottom", Left: "left", Right: "right" },
    useReactFlow: () => ({ getViewport: () => ({ x: 0, y: 0, zoom: 1 }) }),
    useNodesState: (initial: any) => {
      const [ns, setNs] = React.useState(initial);
      return [ns, setNs, () => {}];
    },
  };
});

vi.mock("../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../lib/tauri")>()),
  // Inlined on purpose: vi.mock factories are hoisted above every top-level
  // binding, so a shared fixture const would be in its temporal dead zone.
  getPage: vi.fn().mockResolvedValue({
    id: "concept_abc",
    title: "libSQL Architecture",
    summary: "Core database layer",
    content: "libSQL is the core database layer.\n\nMore prose here.",
    entity_id: null,
    domain: null,
    source_memory_ids: ["mem_1"],
    version: 3,
    status: "active",
    created_at: "2026-04-01T00:00:00+00:00",
    last_compiled: "2026-04-07T12:00:00+00:00",
    last_modified: "2026-04-07T12:00:00+00:00",
  }),
  getPageSources: vi.fn().mockResolvedValue([
    {
      source: { page_id: "concept_abc", memory_source_id: "mem_1", linked_at: 1, link_reason: "page_growth" },
      memory: {
        source_id: "mem_1",
        title: "libSQL stores vectors",
        content: "libSQL stores vectors in F32_BLOB columns",
        summary: null,
        memory_type: "fact",
        domain: null,
        source_agent: "claude",
        confidence: 0.9,
        confirmed: true,
        last_modified: 1,
      },
    },
  ]),
  getPageLinks: vi.fn().mockResolvedValue({ outbound: [], inbound: [] }),
  getPageRevisions: vi.fn().mockResolvedValue({ entries: [], user_edited: false }),
  listRegisteredSources: vi.fn().mockResolvedValue([]),
  getEntityDetail: vi.fn().mockResolvedValue(null),
  getPageMap: vi.fn().mockResolvedValue({
    page_id: "concept_abc",
    revision: 3,
    map_schema: 1,
    viewport: null,
    nodes: [
      {
        id: "n_root", parent_id: null, rank: 0, ref_kind: "page", ref_id: "concept_abc",
        label: null, status: "active", pinned: false, placed: false, collapsed: false,
        x: null, y: null, width: null, height: null, ref_state: "live",
      },
      {
        id: "n_mem", parent_id: "n_root", rank: 0, ref_kind: "memory", ref_id: "mem_1",
        label: null, status: "active", pinned: false, placed: false, collapsed: false,
        x: null, y: null, width: null, height: null, ref_state: "live",
      },
    ],
    edges: [],
  }),
}));

class ResizeObserverStub {
  observe() {}
  unobserve() {}
  disconnect() {}
}
vi.stubGlobal("ResizeObserver", ResizeObserverStub);

function renderDetail(pageId = "concept_abc") {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return {
    user: userEvent.setup(),
    client,
    ...render(
      <QueryClientProvider client={client}>
        <PageDetail
          pageId={pageId}
          onBack={vi.fn()}
          onMemoryClick={vi.fn()}
          onPageClick={vi.fn()}
          onEntityClick={vi.fn()}
        />
      </QueryClientProvider>,
    ),
  };
}

beforeEach(() => vi.clearAllMocks());
afterEach(() => cleanup());

async function openInspector(user: ReturnType<typeof userEvent.setup>) {
  await user.click(await screen.findByRole("button", { name: i18n.t("pageInspector.info") }));
  return screen.queryByRole("dialog", { name: i18n.t("pageInspector.label") })
    ?? screen.getByRole("complementary", { name: i18n.t("pageInspector.label") });
}

async function selectInspectorTool(user: ReturnType<typeof userEvent.setup>, name: string) {
  await user.click(await screen.findByRole("button", { name }));
}

describe("PageDetail page inspector", () => {
  it("reveals the PDF tool only after opening a Zotero citation and restores that reader", async () => {
    const { getPageSources } = await import("../../lib/tauri");
    const sources = await getPageSources("concept_abc");
    vi.mocked(getPageSources).mockResolvedValueOnce(sources.map(row => ({
      ...row, memory: row.memory ? { ...row.memory, source_agent: "zotero" } : row.memory,
    })));
    const original = window.matchMedia;
    window.matchMedia = vi.fn().mockReturnValue({ matches: true, addEventListener: vi.fn(), removeEventListener: vi.fn() });
    try {
      const { user } = renderDetail();
      await screen.findByText("libSQL Architecture");
      expect(screen.queryByRole("button", { name: "PDF" })).toBeNull();
      const panel = await openInspector(user);
      await user.click(within(panel).getByRole("button", { name: /libSQL stores vectors/ }));
      const pdfButton = await screen.findByRole("button", { name: "PDF" });
      expect(pdfButton).toHaveAttribute("aria-pressed", "true");
      expect(screen.getAllByRole("complementary")).toHaveLength(1);
      expect(screen.getByRole("complementary", { name: i18n.t("pageInspector.label") })).toBe(panel);
      const reader = within(panel).getByRole("region", { name: "PDF reader" });
      expect(reader).toHaveAttribute("data-source", "mem_1");
      expect(screen.getByText("More prose here.")).toBeTruthy();

      await selectInspectorTool(user, i18n.t("pageCanvas.tabCanvas"));
      expect(await screen.findByRole("region", { name: "Canvas for libSQL Architecture" })).toBeTruthy();
      expect(screen.queryByRole("region", { name: "PDF reader" })).toBeNull();
      await user.click(pdfButton);
      expect(screen.getByRole("region", { name: "PDF reader" })).toBe(reader);
      expect(screen.getByRole("region", { name: "PDF reader" })).toHaveAttribute("data-source", "mem_1");
      await selectInspectorTool(user, i18n.t("pageInspector.info"));
      expect(screen.queryByRole("region", { name: "PDF reader" })).toBeNull();
      await user.click(pdfButton);
      expect(screen.getByRole("region", { name: "PDF reader" })).toBe(reader);
      await user.click(pdfButton);
      expect(screen.queryByRole("region", { name: "PDF reader" })).toBeNull();
      expect(pdfButton).toHaveAttribute("aria-pressed", "false");
      await user.click(pdfButton);
      expect(screen.getByRole("region", { name: "PDF reader" })).toBe(reader);
    } finally { window.matchMedia = original; }
  });

  it("opens the dedicated page sidebar with Info selected and no inspector actions in the overflow menu", async () => {
    const { user, container } = renderDetail();
    await screen.findByText("libSQL Architecture");
    expect(container.querySelector(".page-detail-top-row")).toBeNull();
    expect(screen.getByRole("button", { name: i18n.t("pageInspector.info") })).toBeTruthy();
    expect(screen.getByRole("button", { name: i18n.t("pageCanvas.tabCanvas") })).toBeTruthy();
    expect(screen.queryByRole("tablist")).toBeNull();
    expect(screen.getByRole("button", { name: "Page actions" }).closest(".page-document-title-row")).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Page actions" }));
    expect(screen.queryByRole("menuitem", { name: i18n.t("pageInspector.info") })).toBeNull();
    expect(screen.queryByRole("menuitem", { name: i18n.t("pageCanvas.tabCanvas") })).toBeNull();
    await user.keyboard("{Escape}");
    await openInspector(user);
    expect(screen.getByRole("button", { name: i18n.t("pageInspector.info") })).toHaveAttribute("aria-pressed", "true");
    expect(screen.queryByRole("tablist")).toBeNull();
    expect(screen.getByRole("button", { name: i18n.t("pageCanvas.tabCanvas") })).toHaveAttribute("aria-expanded", "true");
  });

  it("opens the Mind map directly without first opening Info", async () => {
    const { user } = renderDetail();
    await screen.findByText("libSQL Architecture");
    expect(screen.queryByRole("tablist")).toBeNull();
    await user.click(screen.getByRole("button", { name: i18n.t("pageCanvas.tabCanvas") }));
    expect(await screen.findByRole("region", { name: "Canvas for libSQL Architecture" })).toBeTruthy();
    expect(screen.getByRole("button", { name: i18n.t("pageCanvas.tabCanvas") })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("button", { name: i18n.t("pageInspector.info") })).toHaveAttribute("aria-expanded", "true");
    await user.click(screen.getByRole("button", { name: i18n.t("pageCanvas.tabCanvas") }));
    expect(screen.queryByRole("tablist")).toBeNull();
    expect(screen.getByRole("button", { name: i18n.t("pageCanvas.tabCanvas") })).toHaveFocus();
  });

  it("switches from an external source preview to the requested page view", async () => {
    const { getPageSources } = await import("../../lib/tauri");
    const sources = await getPageSources("concept_abc");
    vi.mocked(getPageSources).mockResolvedValueOnce(sources.map(row => ({
      ...row, memory: row.memory ? { ...row.memory, source_agent: "obsidian" } : row.memory,
    })));
    const { user } = renderDetail();
    await screen.findByText("libSQL Architecture");
    await user.click(screen.getByRole("button", { name: i18n.t("pageInspector.info") }));
    const panel = screen.queryByRole("dialog", { name: i18n.t("pageInspector.label") })
      ?? screen.getByRole("complementary", { name: i18n.t("pageInspector.label") });
    await user.click(within(panel)
      .getByRole("button", { name: /libSQL stores vectors/ }));
    expect(await screen.findByRole("region", { name: "External source preview" })).toBeTruthy();

    // Info was already active under the external preview: choosing it dismisses
    // the preview while leaving the requested Info pane visible.
    await user.click(screen.getByRole("button", { name: i18n.t("pageInspector.info") }));
    expect(screen.queryByRole("region", { name: "External source preview" })).toBeNull();
    expect(screen.getByRole("button", { name: i18n.t("pageInspector.info") })).toHaveAttribute("aria-pressed", "true");
  });

  it("lets the Info control close its own pane and reclaim the reading width", async () => {
    const { user } = renderDetail();
    await screen.findByText("libSQL Architecture");
    const info = screen.getByRole("button", { name: i18n.t("pageInspector.info") });
    await user.click(info);
    expect(await screen.findByRole("dialog", { name: i18n.t("pageInspector.label") })).toBeTruthy();
    expect(info).toHaveAttribute("aria-pressed", "true");
    await user.click(info);
    expect(screen.queryByRole("tablist")).toBeNull();
    expect(info).toHaveAttribute("aria-expanded", "false");
    expect(info).toHaveFocus();
  });

  it("opens the map beside the reading page and returns through the panel close button", async () => {
    const { user } = renderDetail();
    await screen.findByText("libSQL Architecture");
    await selectInspectorTool(user, i18n.t("pageCanvas.tabCanvas"));
    const panel = await screen.findByRole("dialog", { name: i18n.t("pageInspector.label") });
    expect(await screen.findByRole("region", { name: "Canvas for libSQL Architecture" })).toBeTruthy();
    expect(screen.getByText("More prose here.")).toBeTruthy();
    expect(screen.getByTestId("page-document-reading")).not.toHaveAttribute("contenteditable");
    await user.click(panel.querySelector<HTMLButtonElement>(".page-info-drawer-close")!);
    expect(screen.queryByRole("region", { name: "Canvas for libSQL Architecture" })).toBeNull();
    expect(screen.getByRole("button", { name: i18n.t("pageCanvas.tabCanvas") })).toHaveFocus();
  });

  it("resolves node labels from the page and sources already loaded", async () => {
    const { user } = renderDetail();
    await screen.findByText("libSQL Architecture");
    await selectInspectorTool(user, i18n.t("pageCanvas.tabCanvas"));
    await screen.findByTestId("react-flow");
    expect(screen.getAllByText("libSQL Architecture").length).toBeGreaterThan(1);
    expect(screen.getByText("libSQL stores vectors")).toBeTruthy();
  });

  it("switches the only wide right panel between information and map", async () => {
    const original = window.matchMedia;
    window.matchMedia = vi.fn().mockReturnValue({ matches: true, addEventListener: vi.fn(), removeEventListener: vi.fn() });
    try {
      const { user } = renderDetail();
      await screen.findByText("libSQL Architecture");
      await openInspector(user);
      const panel = screen.getByRole("complementary", { name: i18n.t("pageInspector.label") });
      const infoTab = screen.getByRole("button", { name: i18n.t("pageInspector.info") });
      expect(infoTab).toHaveAttribute("aria-pressed", "true");
      expect(screen.queryByRole("tablist")).toBeNull();
      await selectInspectorTool(user, i18n.t("pageCanvas.tabCanvas"));
      expect(screen.getByRole("complementary", { name: i18n.t("pageInspector.label") })).toBe(panel);
      expect(await screen.findByRole("region", { name: "Canvas for libSQL Architecture" })).toBeTruthy();
      await selectInspectorTool(user, i18n.t("pageInspector.info"));
      expect(screen.getByRole("complementary", { name: i18n.t("pageInspector.label") })).toBe(panel);
      expect(screen.queryByRole("region", { name: "Canvas for libSQL Architecture" })).toBeNull();
    } finally { window.matchMedia = original; }
  });

  it("keeps inspector state with its Wiki page and starts a different page closed", async () => {
    const { getPageSources } = await import("../../lib/tauri");
    const baseSources = await getPageSources("concept_abc");
    vi.mocked(getPageSources).mockImplementation(async id => {
      return id === "page_a" ? baseSources.map(row => ({
        ...row, memory: row.memory ? { ...row.memory, source_agent: "zotero" } : row.memory,
      })) : baseSources;
    });
    const { getPage } = await import("../../lib/tauri");
    const page = await getPage("concept_abc");
    if (!page) throw new Error("the mocked page is missing");
    vi.mocked(getPage).mockImplementation(async id => ({ ...page, id, title: id === "page_b" ? "Second page" : "First page" }));
    const { user, client, rerender } = renderDetail("page_a");
    await screen.findByText("First page");
    await openInspector(user);
    const panel = screen.queryByRole("dialog", { name: i18n.t("pageInspector.label") })
      ?? screen.getByRole("complementary", { name: i18n.t("pageInspector.label") });
    await user.click(within(panel).getByRole("button", { name: /libSQL stores vectors/ }));
    const pdf = await screen.findByRole("button", { name: "PDF" });
    expect(screen.getByRole("region", { name: "PDF reader" })).toBeTruthy();
    const reader = screen.getByRole("region", { name: "PDF reader" });
    await user.click(pdf);
    expect(screen.queryByRole("region", { name: "PDF reader" })).toBeNull();
    expect(pdf).toHaveAttribute("aria-pressed", "false");
    await user.click(pdf);
    expect(screen.getByRole("region", { name: "PDF reader" })).toBe(reader);

    rerender(<QueryClientProvider client={client}><PageDetail pageId="page_b" onBack={vi.fn()} onMemoryClick={vi.fn()} onPageClick={vi.fn()} onEntityClick={vi.fn()} /></QueryClientProvider>);
    await screen.findByText("Second page");
    expect(screen.queryByRole("region", { name: "PDF reader" })).toBeNull();
    expect(screen.queryByRole("button", { name: "PDF" })).toBeNull();
    expect(screen.getByRole("button", { name: i18n.t("pageInspector.info") })).toHaveAttribute("aria-expanded", "false");

    rerender(<QueryClientProvider client={client}><PageDetail pageId="page_a" onBack={vi.fn()} onMemoryClick={vi.fn()} onPageClick={vi.fn()} onEntityClick={vi.fn()} /></QueryClientProvider>);
    await screen.findByText("First page");
    expect(screen.getByRole("button", { name: "PDF" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("button", { name: "PDF" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("region", { name: "PDF reader" })).toHaveAttribute("data-source", "mem_1");
  });
});
