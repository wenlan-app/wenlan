// SPDX-License-Identifier: AGPL-3.0-only
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, cleanup, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import PageDetail from "./PageDetail";
import { i18n } from "../../i18n";

// Same reasoning as PageCanvas.test.tsx: React Flow needs real dimensions.
// These tests only care that the canvas tab mounts, so the node state hook is
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

function renderDetail() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return {
    user: userEvent.setup(),
    ...render(
      <QueryClientProvider client={client}>
        <PageDetail
          pageId="concept_abc"
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
  await user.click(await screen.findByRole("button", { name: i18n.t("pageInspector.open") }));
  return screen.queryByRole("dialog", { name: i18n.t("pageInspector.label") })
    ?? screen.getByRole("complementary", { name: i18n.t("pageInspector.label") });
}

async function selectInspectorTab(user: ReturnType<typeof userEvent.setup>, name: string) {
  if (!screen.queryByRole("tablist")) await openInspector(user);
  await user.click(await screen.findByRole("tab", { name }));
}

describe("PageDetail page inspector", () => {
  it("opens the dedicated page sidebar with Info selected and no inspector actions in the overflow menu", async () => {
    const { user, container } = renderDetail();
    await screen.findByText("libSQL Architecture");
    expect(container.querySelector(".page-detail-top-row")).toBeNull();
    expect(screen.getByRole("button", { name: i18n.t("pageInspector.open") })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Page actions" }).closest(".page-document-title-row")).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Page actions" }));
    expect(screen.queryByRole("menuitem", { name: i18n.t("pageInspector.info") })).toBeNull();
    expect(screen.queryByRole("menuitem", { name: i18n.t("pageCanvas.tabCanvas") })).toBeNull();
    await user.keyboard("{Escape}");
    await openInspector(user);
    const tablist = screen.getByRole("tablist", { name: i18n.t("pageInspector.label") });
    expect(within(tablist).getByRole("tab", { name: i18n.t("pageInspector.info") })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("tabpanel")).toHaveAttribute("aria-labelledby", expect.stringContaining("-info"));
    expect(screen.getByRole("button", { name: i18n.t("pageInspector.close") })).toHaveAttribute("aria-expanded", "true");
  });

  it("opens the map beside the reading page and returns through the panel close button", async () => {
    const { user } = renderDetail();
    await screen.findByText("libSQL Architecture");
    await selectInspectorTab(user, i18n.t("pageCanvas.tabCanvas"));
    const panel = await screen.findByRole("dialog", { name: i18n.t("pageInspector.label") });
    expect(await screen.findByRole("region", { name: "Canvas for libSQL Architecture" })).toBeTruthy();
    expect(screen.getByText("More prose here.")).toBeTruthy();
    expect(screen.getByTestId("page-document-reading")).not.toHaveAttribute("contenteditable");
    await user.click(panel.querySelector<HTMLButtonElement>(".page-info-drawer-close")!);
    expect(screen.queryByRole("region", { name: "Canvas for libSQL Architecture" })).toBeNull();
    expect(screen.getByRole("button", { name: i18n.t("pageInspector.open") })).toHaveFocus();
  });

  it("resolves node labels from the page and sources already loaded", async () => {
    const { user } = renderDetail();
    await screen.findByText("libSQL Architecture");
    await selectInspectorTab(user, i18n.t("pageCanvas.tabCanvas"));
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
      const infoTab = screen.getByRole("tab", { name: i18n.t("pageInspector.info") });
      expect(infoTab).toHaveAttribute("aria-selected", "true");
      expect(screen.queryByRole("tab", { name: i18n.t("knowledgeContext.localGraph") })).toBeNull();
      expect(within(panel).getAllByRole("tab")).toHaveLength(2);
      await user.click(screen.getByRole("tab", { name: i18n.t("pageCanvas.tabCanvas") }));
      expect(screen.getByRole("complementary", { name: i18n.t("pageInspector.label") })).toBe(panel);
      expect(await screen.findByRole("region", { name: "Canvas for libSQL Architecture" })).toBeTruthy();
      await user.click(infoTab);
      expect(screen.getByRole("complementary", { name: i18n.t("pageInspector.label") })).toBe(panel);
      expect(screen.queryByRole("region", { name: "Canvas for libSQL Architecture" })).toBeNull();
    } finally { window.matchMedia = original; }
  });
});
