// SPDX-License-Identifier: AGPL-3.0-only
import { describe, it, expect, vi, beforeEach } from "vitest";
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { fireEvent } from "@testing-library/react";
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
  getPageLook: vi.fn(),
  getPageLookData: vi.fn(),
  removePageLook: vi.fn(),
  restorePageLook: vi.fn(),
  reportPageLookError: vi.fn(),
  exportPageToObsidian: vi.fn(),
  reviewPage: vi.fn(),
  parseEntityGuardError: vi.fn().mockReturnValue(null),
}));

// The fake editor stays "loading" until the test reports it ready, which is
// the window where every menu item is disabled.
const editor = vi.hoisted(() => ({ reportReady: null as null | (() => void) }));

const toasts = vi.hoisted(() => ({ calls: [] as { message: string; options?: { action?: { label: string; onClick: () => void } } }[] }));
vi.mock("sonner", () => ({
  toast: Object.assign(
    (message: string, options?: { action?: { label: string; onClick: () => void } }) => { toasts.calls.push({ message, options }); },
    { success: vi.fn(), error: vi.fn() },
  ),
}));

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

const LOOK = { id: 4, html: "<p>look</p>", api: 1, kept_at: 1 };
const LOOK_DATA = {
  api: 1,
  page: { id: "page-1", title: "Focus Test Page", space: "", created: "", modified: "", text: "", html: "", sections: [], links: [] },
  space: { name: "", pages: [], more: false },
  theme: { mode: "dark" },
};

function renderPage() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <PageDetail pageId="page-1" onBack={vi.fn()} onMemoryClick={vi.fn()} />
    </QueryClientProvider>,
  );
}

async function openMenu(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByRole("button", { name: i18n.t("pageDetail.actions") }));
  return screen.getByRole("menu", { name: i18n.t("pageDetail.actions") });
}

describe("PageDetail looks", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    toasts.calls.length = 0;
    window.localStorage.clear();
    editor.reportReady = null;
    tauriMocks.getPage.mockResolvedValue(PAGE);
    tauriMocks.getPageSources.mockResolvedValue([]);
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
    tauriMocks.getPageLook.mockResolvedValue({ look: null, versions: [] });
    tauriMocks.getPageLookData.mockResolvedValue(LOOK_DATA);
    tauriMocks.reportPageLookError.mockResolvedValue({ recorded: true });
    tauriMocks.clipboardWrite.mockResolvedValue(undefined);
  });

  it("UX-LOOK-1 shows no switch and no look on a page without one, and offers to ask an AI for one", async () => {
    const user = userEvent.setup();
    renderPage();
    await screen.findByRole("heading", { name: "Focus Test Page" });
    await waitFor(() => expect(tauriMocks.getPageLook).toHaveBeenCalledWith("page-1"));
    expect(screen.queryByTestId("page-look-switch")).toBeNull();
    expect(screen.queryByTestId("page-look-frame")).toBeNull();
    const menu = await openMenu(user);
    expect(menu.querySelector('[role="menuitem"]:not([disabled])')).not.toBeNull();
    await waitFor(() => expect(screen.getByRole("menuitem", { name: i18n.t("pageLook.askAi") })).toBeEnabled());
    expect(screen.queryByRole("menuitem", { name: i18n.t("pageLook.remove") })).toBeNull();
    await user.click(screen.getByRole("menuitem", { name: i18n.t("pageLook.askAi") }));
    await waitFor(() => expect(tauriMocks.clipboardWrite).toHaveBeenCalledWith(i18n.t("pageLook.askAiPrompt", { title: "Focus Test Page" })));
  });

  it("UX-LOOK-1 and UX-LOOK-2 shows the look in place of the text, keeps the text behind it, and remembers Text", async () => {
    tauriMocks.getPageLook.mockResolvedValue({ look: LOOK, versions: [] });
    renderPage();
    const frame = await screen.findByTestId("page-look-frame");
    expect(frame.getAttribute("sandbox")).toBe("allow-scripts");
    const text = document.querySelector(".page-look-text") as HTMLElement;
    expect(text.hidden).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: i18n.t("pageLook.text") }));
    expect(screen.queryByTestId("page-look-frame")).toBeNull();
    expect(text.hidden).toBe(false);
    expect(window.localStorage.getItem("wenlan-look-mode:page-1")).toBe("text");
  });

  it("UX-LOOK-5 takes the look off and puts it back with Undo", async () => {
    tauriMocks.getPageLook.mockResolvedValue({ look: LOOK, versions: [] });
    tauriMocks.removePageLook.mockResolvedValue(4);
    tauriMocks.restorePageLook.mockResolvedValue(undefined);
    const user = userEvent.setup();
    renderPage();
    await screen.findByTestId("page-look-frame");
    await openMenu(user);
    await waitFor(() => expect(screen.getByRole("menuitem", { name: i18n.t("pageLook.remove") })).toBeEnabled());
    tauriMocks.getPageLook.mockResolvedValue({ look: null, versions: [{ id: 4, kept_at: 1, active: false, bytes: 9 }] });
    await user.click(screen.getByRole("menuitem", { name: i18n.t("pageLook.remove") }));
    await waitFor(() => expect(tauriMocks.removePageLook).toHaveBeenCalledWith("page-1"));
    await waitFor(() => expect(screen.queryByTestId("page-look-frame")).toBeNull());
    expect(screen.queryByTestId("page-look-switch")).toBeNull();
    await waitFor(() => expect(toasts.calls.length).toBe(1));
    expect(toasts.calls[0].message).toBe(i18n.t("pageLook.removed"));
    expect(toasts.calls[0].options?.action?.label).toBe(i18n.t("pageLook.undo"));
    tauriMocks.getPageLook.mockResolvedValue({ look: LOOK, versions: [] });
    act(() => toasts.calls[0].options!.action!.onClick());
    await waitFor(() => expect(tauriMocks.restorePageLook).toHaveBeenCalledWith("page-1", 4));
    await screen.findByTestId("page-look-frame");
  });

  it("takes no look on a kept original or an archived page", async () => {
    tauriMocks.getPageLook.mockResolvedValue({ look: LOOK, versions: [] });
    tauriMocks.getPage.mockResolvedValue({ ...PAGE, creation_kind: "original" });
    const user = userEvent.setup();
    const view = renderPage();
    await screen.findByRole("heading", { name: "Focus Test Page" });
    expect(tauriMocks.getPageLook).not.toHaveBeenCalled();
    expect(screen.queryByTestId("page-look-switch")).toBeNull();
    await openMenu(user);
    expect(screen.queryByRole("menuitem", { name: i18n.t("pageLook.askAi") })).toBeNull();
    view.unmount();

    tauriMocks.getPage.mockResolvedValue({ ...PAGE, status: "archived" });
    renderPage();
    await screen.findByRole("heading", { name: "Focus Test Page" });
    expect(tauriMocks.getPageLook).not.toHaveBeenCalled();
    expect(screen.queryByTestId("page-look-frame")).toBeNull();
  });
});
