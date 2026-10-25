// SPDX-License-Identifier: AGPL-3.0-only
import { describe, it, expect, vi, beforeEach } from "vitest";
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import PageDetail from "./PageDetail";
import { i18n } from "../../i18n";

const tauriMocks = vi.hoisted(() => ({
  getPage: vi.fn(),
  getPageSources: vi.fn(),
  listRegisteredSources: vi.fn(),
  getPageLinks: vi.fn(),
  listOrphanLinks: vi.fn(),
  getPageRevisions: vi.fn(),
  listPages: vi.fn(),
  getDaemonVersion: vi.fn(),
  getSystemInfo: vi.fn(),
  daemonMeetsFloor: vi.fn(),
  recordPageEditorDiagnostic: vi.fn(),
  getTruthStatus: vi.fn(),
  pageReviewSupported: vi.fn(),
  setPageReviewRequired: vi.fn(),
  redistillPage: vi.fn(),
  updatePage: vi.fn(),
  deletePage: vi.fn(),
  clipboardWrite: vi.fn(),
  exportPageToObsidian: vi.fn(),
  reviewPage: vi.fn(),
  parseEntityGuardError: vi.fn().mockReturnValue(null),
}));

// The fake editor stays "loading" until the test reports it ready, which is
// the window where every menu item is disabled.
const editor = vi.hoisted(() => ({ reportReady: null as null | (() => void) }));

vi.mock("./PageCanvas", () => ({ default: () => <div /> }));

vi.mock("./editor/MarkdownEditor", async () => {
  const React = await import("react");
  return {
    MarkdownEditor: (props: { sessionId: string; ariaLabel: string; initialDocument: string; onStatusChange?: (status: object) => void }) => {
      React.useEffect(() => {
        const base = { sessionId: props.sessionId, compositionActive: false, canUndo: false, canRedo: false };
        props.onStatusChange?.({ ...base, engine: "loading", ready: false });
        editor.reportReady = () => props.onStatusChange?.({ ...base, engine: "codemirror", ready: true });
        // eslint-disable-next-line react-hooks/exhaustive-deps
      }, [props.sessionId]);
      return <div role="textbox" aria-label={props.ariaLabel}>{props.initialDocument}</div>;
    },
  };
});

vi.mock("../../lib/tauri", () => ({
  getKnowledgeGraph: vi.fn().mockResolvedValue({ entities: [], relations: [], memories: [], memory_links: [], pages: [], page_links: [] }),
  ...tauriMocks,
  FACET_COLORS: {},
  STABILITY_TIERS: {},
}));

const PAGE = {
  id: "page-1",
  title: "Focus Test Page",
  summary: null,
  content: "Some body text.",
  entity_id: null,
  domain: "testing",
  source_memory_ids: ["mem-1"],
  version: 1,
  status: "active",
  created_at: "2026-06-26T00:00:00+00:00",
  last_compiled: "2026-06-26T00:00:00+00:00",
  last_modified: "2026-06-26T00:00:00+00:00",
};

describe("PageDetail action menu focus", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    editor.reportReady = null;
    tauriMocks.getPage.mockResolvedValue(PAGE);
    // One source, so "Update from sources" joins Delete as a second item
    // that waits for the editor.
    tauriMocks.getPageSources.mockResolvedValue([{ source: { page_id: "page-1", memory_source_id: "mem-1", linked_at: 0 }, memory: null }]);
    tauriMocks.listRegisteredSources.mockResolvedValue([]);
    tauriMocks.getPageLinks.mockResolvedValue({ outbound: [], inbound: [] });
    tauriMocks.listOrphanLinks.mockResolvedValue({ min_count: 2, orphan_labels: [] });
    tauriMocks.getPageRevisions.mockResolvedValue({ page_id: "page-1", current_version: 1, user_edited: false, entries: [] });
    tauriMocks.listPages.mockResolvedValue([]);
    tauriMocks.getDaemonVersion.mockResolvedValue("0.14.1");
    tauriMocks.getSystemInfo.mockResolvedValue({ os: "macos" });
    tauriMocks.daemonMeetsFloor.mockReturnValue(true);
    tauriMocks.recordPageEditorDiagnostic.mockResolvedValue(undefined);
    tauriMocks.getTruthStatus.mockResolvedValue(null);
    tauriMocks.pageReviewSupported.mockResolvedValue("ready");
  });

  it("moves focus to the first item once items enable after an early open", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const user = userEvent.setup();
    render(
      <QueryClientProvider client={client}>
        <PageDetail pageId="page-1" initialMode="edit" onBack={vi.fn()} onMemoryClick={vi.fn()} />
      </QueryClientProvider>,
    );
    await waitFor(() => expect(editor.reportReady).not.toBeNull());

    await user.click(screen.getByRole("button", { name: i18n.t("pageDetail.actions") }));
    const menu = screen.getByRole("menu", { name: i18n.t("pageDetail.actions") });
    const toggle = screen.getByRole("menuitem", { name: i18n.t("pageDetail.deletePage") });
    expect(toggle).toBeDisabled();
    expect(menu).toHaveFocus();

    act(() => editor.reportReady!());
    await waitFor(() => expect(toggle).toBeEnabled());
    const enabled = Array.from(menu.querySelectorAll<HTMLElement>('[role^="menuitem"]:not(:disabled)'));
    expect(enabled.length).toBeGreaterThan(1);
    expect(document.activeElement).toBe(enabled[0]);
  });

  it("moves focus to the last item once items enable after an early ArrowUp open", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const user = userEvent.setup();
    render(
      <QueryClientProvider client={client}>
        <PageDetail pageId="page-1" initialMode="edit" onBack={vi.fn()} onMemoryClick={vi.fn()} />
      </QueryClientProvider>,
    );
    await waitFor(() => expect(editor.reportReady).not.toBeNull());

    screen.getByRole("button", { name: i18n.t("pageDetail.actions") }).focus();
    await user.keyboard("{ArrowUp}");
    const menu = screen.getByRole("menu", { name: i18n.t("pageDetail.actions") });
    expect(menu).toHaveFocus();

    act(() => editor.reportReady!());
    const toggle = screen.getByRole("menuitem", { name: i18n.t("pageDetail.deletePage") });
    await waitFor(() => expect(toggle).toBeEnabled());
    const enabled = Array.from(menu.querySelectorAll<HTMLElement>('[role^="menuitem"]:not(:disabled)'));
    expect(enabled.length).toBeGreaterThan(1);
    expect(document.activeElement).toBe(enabled[enabled.length - 1]);
  });
});
