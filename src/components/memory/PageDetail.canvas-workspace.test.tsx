// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import PageDetail from "./PageDetail";
import { WorkspaceDocumentToolsHostContext } from "./navigation/WorkspacePaneHost";
import { i18n } from "../../i18n";

vi.mock("@xyflow/react", async () => {
  const React = await import("react");
  return {
    ReactFlow: ({ nodes, nodeTypes }: any) => <div data-testid="react-flow">
      {nodes.map((node: any) => {
        const NodeComponent = nodeTypes[node.type];
        return <NodeComponent key={node.id} id={node.id} data={node.data} />;
      })}
    </div>,
    ReactFlowProvider: ({ children }: any) => <>{children}</>,
    Background: () => null,
    Controls: () => null,
    Handle: () => null,
    Position: { Top: "top", Bottom: "bottom", Left: "left", Right: "right" },
    useReactFlow: () => ({ getViewport: () => ({ x: 0, y: 0, zoom: 1 }) }),
    useNodesState: (initial: any) => {
      const [nodes, setNodes] = React.useState(initial);
      return [nodes, setNodes, () => {}];
    },
  };
});

vi.mock("../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../lib/tauri")>()),
  getPage: vi.fn().mockResolvedValue({
    id: "concept_abc", title: "Workspace scroll", summary: "Map workspace test",
    content: "Reading content stays here.", entity_id: null, domain: null,
    source_memory_ids: [], version: 3, status: "active",
    created_at: "2026-04-01T00:00:00+00:00", last_compiled: "2026-04-07T12:00:00+00:00",
    last_modified: "2026-04-07T12:00:00+00:00",
  }),
  getPageSources: vi.fn().mockResolvedValue([]),
  getPageLinks: vi.fn().mockResolvedValue({ outbound: [], inbound: [] }),
  getPageRevisions: vi.fn().mockResolvedValue({ entries: [], user_edited: false }),
  listRegisteredSources: vi.fn().mockResolvedValue([]),
  getEntityDetail: vi.fn().mockResolvedValue(null),
  pageReviewSupported: vi.fn().mockResolvedValue("ready"),
  getPageMap: vi.fn().mockResolvedValue({
    page_id: "concept_abc", revision: 3, map_schema: 1, viewport: null,
    nodes: [{
      id: "n_root", parent_id: null, rank: 0, ref_kind: "page", ref_id: "concept_abc",
      label: null, status: "active", pinned: false, placed: false, collapsed: false,
      x: null, y: null, width: null, height: null, ref_state: "live",
    }],
    edges: [],
  }),
  putPageMapLayout: vi.fn().mockResolvedValue({ revision: 4 }),
}));

class ResizeObserverStub {
  observe() {}
  unobserve() {}
  disconnect() {}
}
vi.stubGlobal("ResizeObserver", ResizeObserverStub);
afterEach(() => cleanup());

describe("PageDetail canvas workspace", () => {
  it("shows expansion after page load and restores the wiki scroll position without a map write", async () => {
    const originalMatchMedia = window.matchMedia;
    window.matchMedia = vi.fn().mockReturnValue({
      matches: true, addEventListener: vi.fn(), removeEventListener: vi.fn(),
    });
    const user = userEvent.setup();
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
      const toolsHost = document.createElement("div");
      document.body.append(toolsHost);
    try {
      const { container } = render(
        <QueryClientProvider client={client}>
          <WorkspaceDocumentToolsHostContext.Provider value={toolsHost}>
          <div className="wiki-workspace-content" data-testid="wiki-content">
            <PageDetail pageId="concept_abc" onBack={vi.fn()} onMemoryClick={vi.fn()} onPageClick={vi.fn()} onEntityClick={vi.fn()} />
          </div>
          </WorkspaceDocumentToolsHostContext.Provider>
        </QueryClientProvider>,
      );
      await screen.findByText("Workspace scroll");
      const wikiContent = screen.getByTestId("wiki-content");
      wikiContent.scrollTop = 640;
      await user.click(screen.getByRole("button", { name: i18n.t("pageCanvas.tabCanvas") }));

      const map = await screen.findByRole("region", { name: "Canvas for Workspace scroll" });
      const expand = await within(toolsHost).findByRole("button", { name: i18n.t("pageCanvas.expandWorkspace") });
      await user.click(expand);
      expect(container.querySelector(".page-detail--workspace-expanded")).toBeInTheDocument();
      expect(screen.getByRole("region", { name: "Canvas for Workspace scroll" })).toBe(map);

      // Browsers can clamp scrollTop when the reading document leaves flow.
      wikiContent.scrollTop = 0;
      await user.click(within(toolsHost).getByRole("button", { name: i18n.t("pageCanvas.restoreSidebar") }));
      expect(container.querySelector(".page-detail--workspace-expanded")).toBeNull();
      expect(wikiContent.scrollTop).toBe(640);
      expect(screen.getByRole("region", { name: "Canvas for Workspace scroll" })).toBe(map);

      await user.click(within(toolsHost).getByRole("button", { name: i18n.t("pageCanvas.expandWorkspace") }));
      wikiContent.scrollTop = 0;
      await user.click(screen.getByRole("button", { name: i18n.t("pageInspector.info") }));
      expect(container.querySelector(".page-detail--workspace-expanded")).toBeInTheDocument();
      expect(wikiContent.scrollTop).toBe(0);
      expect(screen.queryByRole("tablist")).toBeNull();

      await user.click(screen.getByRole("button", { name: i18n.t("pageCanvas.tabCanvas") }));
      expect(container.querySelector(".page-detail--workspace-expanded")).toBeInTheDocument();
      wikiContent.scrollTop = 0;
      await user.click(screen.getByRole("button", { name: i18n.t("pageCanvas.tabCanvas") }));
      expect(container.querySelector(".page-detail--workspace-expanded")).toBeNull();
      expect(wikiContent.scrollTop).toBe(640);

      const { putPageMapLayout } = await import("../../lib/tauri");
      expect(putPageMapLayout).not.toHaveBeenCalled();
    } finally {
      window.matchMedia = originalMatchMedia;
      toolsHost.remove();
    }
  });
});
