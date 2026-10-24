// SPDX-License-Identifier: AGPL-3.0-only
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import Main from "./Main";
import { i18n } from "../../i18n";
import { clearPendingPairingCode, setPendingPairingCode } from "../../lib/pairingLink";

const flushHarness = vi.hoisted(() => ({
  enabled: false,
  flush: vi.fn<() => Promise<boolean>>(),
  draftIdentity: { draftId: null as string | null, version: null as number | null },
}));
const routeFlushHarness = vi.hoisted(() => ({
  enabled: false,
  flush: vi.fn<() => Promise<boolean>>(),
}));

const eventListeners = vi.hoisted(
  () => new Map<string, (payload?: unknown) => void>(),
);

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn((event: string, handler: (payload?: unknown) => void) => {
    eventListeners.set(event, handler);
    return Promise.resolve(() => {
      if (eventListeners.get(event) === handler) eventListeners.delete(event);
    });
  }),
}));

vi.mock("../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../lib/tauri")>()),
  FACET_COLORS: {},
  STABILITY_TIERS: {},
  shouldShowWizard: vi.fn().mockResolvedValue(true),
  listMemoriesRich: vi.fn().mockResolvedValue([]),
  listSpaces: vi.fn().mockResolvedValue([]),
  getMemoryStats: vi.fn().mockResolvedValue({
    total: 0,
    new_today: 0,
    confirmed: 0,
    domains: [],
  }),
  search: vi.fn().mockResolvedValue([]),
  searchEntities: vi.fn().mockResolvedValue([]),
  searchPages: vi.fn().mockResolvedValue([]),
  openFile: vi.fn().mockResolvedValue(undefined),
  deleteFileChunks: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("./ActivityFeed", () => ({ default: () => <div /> }));
vi.mock("./IdentityDetail", () => ({ default: () => <div /> }));
vi.mock("./MemoryStream", () => ({ default: () => <div /> }));
vi.mock("./AtlasView", () => ({ default: () => <div /> }));
vi.mock("./MemorySearchResult", () => ({ default: () => <div /> }));
vi.mock("./MemoryDetail", () => ({
  default: ({ sourceId }: { sourceId: string }) => (
    <div data-testid="memory-detail">{sourceId}</div>
  ),
}));
vi.mock("./PageDetail", async () => {
  const { useEffect } = await import("react");
  return ({
  default: (props: {
    onRegisterFlush?: (flush: (() => Promise<boolean>) | null) => void;
    onBack?: () => void;
    onDeleted?: (pageId: string) => void;
    onEditDirtyChange?: (dirty: boolean) => void;
    onSavePendingChange?: (pending: boolean) => void;
    pageId: string;
  }) => {
    useEffect(() => {
      if (!flushHarness.enabled) return;
      props.onRegisterFlush?.(flushHarness.flush);
      return () => props.onRegisterFlush?.(null);
    }, [props.onRegisterFlush]);
    return (
    <section data-testid="page-detail">
      <h1>{props.pageId === "page-two" ? "Second page" : "Pending page"}</h1>
      <button type="button" onClick={() => props.onEditDirtyChange?.(true)}>
        Make page dirty
      </button>
      <button type="button" onClick={() => props.onSavePendingChange?.(true)}>
        Start page save
      </button>
      <button type="button" onClick={() => props.onSavePendingChange?.(false)}>
        Finish page save
      </button>
      <button type="button" onClick={() => { props.onDeleted?.(props.pageId); props.onBack?.(); }}>
        Delete current page
      </button>
      <button
        type="button"
        onClick={() => {
          if (flushHarness.enabled || confirm("Discard your unsaved draft?")) props.onBack?.();
        }}
      >
        PageDetail back
      </button>
      <div aria-label="Page editor" contentEditable role="textbox" />
      <textarea aria-label="Page pages" />
      <select aria-label="Block style" defaultValue="paragraph">
        <option value="paragraph">Paragraph</option>
      </select>
    </section>
  ); },
}); });
vi.mock("./DistillReviewPanel", () => ({ default: () => <div /> }));
vi.mock("./SettingsPage", () => ({ default: () => <div /> }));
vi.mock("../SetupWizard", () => ({ SetupWizard: () => <div /> }));
vi.mock("./Sidebar", () => ({
  default: (props: {
    onNavigatePages: () => void;
    onOpenSearch?: (trigger: HTMLButtonElement) => void;
    searchDisabled?: boolean;
    searchOpen?: boolean;
  }) => (
    <aside>
      <button
        aria-expanded={props.searchOpen ?? false}
        disabled={props.searchDisabled}
        onClick={(event) => props.onOpenSearch?.(event.currentTarget)}
        type="button"
      >
        Search
      </button>
      <button type="button" onClick={props.onNavigatePages}>
        Sidebar Wiki
      </button>
    </aside>
  ),
  SidebarToggleButton: (props: {
    onToggle: () => void;
    ref?: React.Ref<HTMLButtonElement>;
  }) => (
    <button ref={props.ref} type="button" onClick={props.onToggle}>
      Toggle sidebar
    </button>
  ),
  SidebarHeaderDivider: () => null,
}));
vi.mock("./navigation/ContextBrowser", () => ({
  ContextBrowser: (props: { currentPageId: string | null; onSelectPage: (page: { id: string }) => void }) => (
    <div data-testid="context-browser">
      <button type="button" onClick={() => props.onSelectPage({ id: "page-two" })}>Context browser second page</button>
      {props.currentPageId && <button type="button" onClick={() => props.onSelectPage({ id: props.currentPageId! })}>Reselect current page</button>}
    </div>
  ),
}));
vi.mock("./pages/WikiWorkspace", () => ({
  WikiWorkspace: (props: {
    children: React.ReactNode;
    tabs?: React.ReactNode;
    onSecondaryHost?: (node: HTMLDivElement | null) => void;
    currentPageId?: string | null;
    onOpenPage: (page: { id: string }) => void;
    onCreatePage?: () => void;
    onOpenDraft?: (draftId: string, space: string | null) => void;
    onDeletePage?: (pageId: string) => void;
  }) => (
    <div data-current-page={props.currentPageId ?? "none"} data-testid="wiki-workspace">
      {props.tabs}{props.children}<div ref={props.onSecondaryHost} />
      {props.currentPageId && <button type="button" onClick={() => props.onOpenPage({ id: props.currentPageId! })}>Reselect current page</button>}
      <button type="button" onClick={() => props.onOpenPage({ id: "page-two" })}>Wiki directory second page</button>
      {props.onDeletePage && <button type="button" onClick={() => props.onDeletePage!("page-two")}>Delete second page from directory</button>}
      {props.onDeletePage && <button type="button" onClick={() => props.onDeletePage!("draft-x")}>Delete draft X from directory</button>}
      {props.onCreatePage && <button type="button" onClick={props.onCreatePage}>Create note draft</button>}
      {props.onOpenDraft && <button type="button" onClick={() => props.onOpenDraft!("draft-x", null)}>Open draft X</button>}
    </div>
  ),
}));
vi.mock("./pages/PagesOverview", () => ({
  PagesOverview: ({ onSelectPage }: { onSelectPage: (id: string) => void }) => (
    <div data-testid="pages-overview">
      <button type="button" onClick={() => onSelectPage("page-two")}>
        Open second page
      </button>
    </div>
  ),
}));
vi.mock("./pages/PageDraftEditor", async () => {
  const React = await import("react");
  return { PageDraftEditor: React.forwardRef((props: {
    draftId?: string;
    onBack?: () => void;
    onDraftIdentity?: (draftId: string) => void;
  }, ref) => {
    React.useImperativeHandle(ref, () => props.draftId ? null : ({
      flush: () => flushHarness.enabled ? flushHarness.flush() : Promise.resolve(true),
      getIdentity: () => flushHarness.draftIdentity,
      requestBack: async () => {},
    }));
    return (
      <div data-testid="draft-editor">
        {props.draftId && <p>Unhydrated draft</p>}
        <button type="button" onClick={() => {
          flushHarness.draftIdentity = { draftId: "draft-x", version: 1 };
          props.onDraftIdentity?.("draft-x");
        }}>Report draft X identity</button>
        <button type="button" onClick={props.onBack}>Draft back</button>
      </div>
    );
  }) };
});
vi.mock("./spaces", () => ({ SpacesOverview: () => <div /> }));
vi.mock("./settings/SettingsSidebar", () => ({ default: () => <aside /> }));
vi.mock("./SpaceDetail", async () => {
  const { useEffect } = await import("react");
  return { default: (props: { onRegisterFlush?: (flush: (() => Promise<boolean>) | null) => void; onBack?: () => void }) => {
    useEffect(() => {
      if (!routeFlushHarness.enabled) return;
      props.onRegisterFlush?.(routeFlushHarness.flush);
      return () => props.onRegisterFlush?.(null);
    }, [props.onRegisterFlush]);
    return <div data-testid="space-detail"><button type="button" onClick={props.onBack}>Space back</button></div>;
  } };
});
vi.mock("./SourcesView", async () => {
  const { useEffect } = await import("react");
  return { default: (props: { onRegisterFlush?: (flush: (() => Promise<boolean>) | null) => void }) => {
    useEffect(() => {
      if (!routeFlushHarness.enabled) return;
      props.onRegisterFlush?.(routeFlushHarness.flush);
      return () => props.onRegisterFlush?.(null);
    }, [props.onRegisterFlush]);
    return <div data-testid="sources-view" />;
  } };
});
vi.mock("./sources/SourceDocumentReader", async () => {
  const { useEffect } = await import("react");
  return { SourceDocumentPage: (props: { onRegisterFlush?: (flush: (() => Promise<boolean>) | null) => void }) => {
    useEffect(() => {
      if (!routeFlushHarness.enabled) return;
      props.onRegisterFlush?.(routeFlushHarness.flush);
      return () => props.onRegisterFlush?.(null);
    }, [props.onRegisterFlush]);
    return <div data-testid="source-document" />;
  } };
});
vi.mock("./RecapsList", () => ({ RecapsList: () => <div /> }));
vi.mock("./ImportView", () => ({
  ImportView: () => <div data-testid="import-view" />,
}));
vi.mock("./AboutWenlanDialog", () => ({ default: () => <div /> }));
// The approval dialog has its own tests; here only that Main shows it over the page.
vi.mock("./PairingApprovalDialog", async () => {
  const { usePendingPairingCode } = await import("../../lib/pairingLink");
  return { default: () => (usePendingPairingCode() ? <div role="dialog" data-testid="pairing-dialog" /> : null) };
});
vi.mock("./RemoteAccessNotifier", () => ({ default: () => null }));

interface RenderMainProps {
  initialView?: React.ComponentProps<typeof Main>["initialView"];
  initialMemoryId?: string | null;
  initialPageId?: string | null;
  onRegisterQuitGuard?: (guard: (() => Promise<boolean>) | null) => void;
}

function renderMain(props: RenderMainProps = { initialPageId: "page-one" }) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const rendered = render(
    <QueryClientProvider client={client}>
      <Main {...props} />
    </QueryClientProvider>,
  );
  return {
    user: userEvent.setup(),
    ...rendered,
    rerenderMain: (nextProps: RenderMainProps) =>
      rendered.rerender(
        <QueryClientProvider client={client}>
          <Main {...nextProps} />
        </QueryClientProvider>,
      ),
  };
}

describe("Main published PageDetail navigation guards", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    flushHarness.enabled = false;
    flushHarness.flush.mockReset();
    routeFlushHarness.enabled = false;
    routeFlushHarness.flush.mockReset();
    flushHarness.draftIdentity = { draftId: null, version: null };
    eventListeners.clear();
  });

  afterEach(() => {
    vi.restoreAllMocks();
    clearPendingPairingCode();
  });

  it("includes the independently mounted right editor in the quit guard", async () => {
    flushHarness.enabled = true;
    flushHarness.flush.mockResolvedValue(true);
    let quitGuard: (() => Promise<boolean>) | null = null;
    const { user } = renderMain({ initialPageId: "page-one", onRegisterQuitGuard: guard => { quitGuard = guard; } });
    await user.pointer({ keys: "[MouseRight]", target: document.querySelector<HTMLElement>('[data-note-tab-group="primary"] [role="tab"][aria-selected="true"]')! });
    await user.click(screen.getByRole("menuitem", { name: "Move to right group" }));
    const right = await screen.findByRole("region", { name: "Right pages" });
    await user.click(screen.getByRole("button", { name: "Open second page" }));
    expect(within(right).getByRole("heading", { name: "Pending page" })).toBeInTheDocument();
    flushHarness.flush.mockResolvedValue(false);
    await act(async () => { await expect(quitGuard!()).resolves.toBe(false); });
    expect(within(right).getByRole("heading", { name: "Pending page" })).toBeInTheDocument();
    flushHarness.flush.mockResolvedValue(true);
    await act(async () => { await expect(quitGuard!()).resolves.toBe(true); });
  });

  it("waits for a source-description save before navigation and quit", async () => {
    routeFlushHarness.enabled = true;
    let settle!: (saved: boolean) => void;
    const saving = new Promise<boolean>((resolve) => { settle = resolve; });
    routeFlushHarness.flush.mockReturnValue(saving);
    let quitGuard: (() => Promise<boolean>) | null = null;
    const initialView = { kind: "source-document", file: { source: "webpage", source_id: "https://example.com", title: "Source", chunk_count: 0, last_modified: 0 } } as never;
    const { user } = renderMain({ initialView, onRegisterQuitGuard: (guard) => { quitGuard = guard; } });
    await screen.findByTestId("source-document");
    await user.click(screen.getByRole("button", { name: "Sidebar Wiki" }));
    expect(screen.getByTestId("source-document")).toBeInTheDocument();
    expect(routeFlushHarness.flush).toHaveBeenCalledOnce();
    const quitting = quitGuard!();
    await act(async () => settle(true));
    await expect(quitting).resolves.toBe(true);
    expect(await screen.findByTestId("pages-overview")).toBeInTheDocument();
  });

  it("keeps a Space visible and prevents quit when its identity flush fails", async () => {
    routeFlushHarness.enabled = true;
    routeFlushHarness.flush.mockResolvedValue(false);
    let quitGuard: (() => Promise<boolean>) | null = null;
    const initialView = { kind: "space", spaceId: null, spaceName: "Reading" } as never;
    const { user } = renderMain({ initialView, onRegisterQuitGuard: (guard) => { quitGuard = guard; } });
    await screen.findByTestId("space-detail");
    await user.click(screen.getByRole("button", { name: "Space back" }));
    expect(screen.getByTestId("space-detail")).toBeInTheDocument();
    await expect(quitGuard!()).resolves.toBe(false);
  });

  it("blocks Back, Escape, and tab close while a page move waits, then remains recoverable", async () => {
    flushHarness.enabled = true;
    let settle!: (saved: boolean) => void;
    const saving = new Promise<boolean>((resolve) => { settle = resolve; });
    flushHarness.flush.mockReturnValue(saving);
    let quitGuard: (() => Promise<boolean>) | null = null;
    const { user } = renderMain({ initialPageId: "page-one", onRegisterQuitGuard: guard => { quitGuard = guard; } });

    await user.pointer({ keys: "[MouseRight]", target: document.querySelector<HTMLElement>('[data-note-tab-group="primary"] [role="tab"][aria-selected="true"]')! });
    await user.click(screen.getByRole("menuitem", { name: "Move to right group" }));
    expect(flushHarness.flush).toHaveBeenCalledOnce();

    await user.click(screen.getByRole("button", { name: i18n.t("main.back") }));
    fireEvent.keyDown(window, { key: "Escape" });
    await user.click(screen.getByRole("button", { name: "Close Untitled page" }));
    expect(flushHarness.flush).toHaveBeenCalledOnce();
    await act(async () => settle(true));

    const right = await screen.findByRole("region", { name: "Right pages" });
    expect(within(right).getByRole("heading", { name: "Pending page" })).toBeInTheDocument();
    await expect(quitGuard!()).resolves.toBe(true);
  });

  it("keeps an initial Page destination pending until a page move completes", async () => {
    flushHarness.enabled = true;
    let settle!: (saved: boolean) => void;
    const saving = new Promise<boolean>((resolve) => { settle = resolve; });
    flushHarness.flush.mockReturnValue(saving);
    const { user, rerenderMain } = renderMain();

    await user.pointer({ keys: "[MouseRight]", target: document.querySelector<HTMLElement>('[data-note-tab-group="primary"] [role="tab"][aria-selected="true"]')! });
    await user.click(screen.getByRole("menuitem", { name: "Move to right group" }));
    rerenderMain({ initialPageId: "page-two" });
    expect(flushHarness.flush).toHaveBeenCalledOnce();
    expect(screen.getByText("Pending page")).toBeInTheDocument();

    await act(async () => settle(true));
    expect(await screen.findByText("Second page")).toBeInTheDocument();
  });

  it("releases a refused initial-memory request after draft flush failure", async () => {
    flushHarness.enabled = true;
    flushHarness.flush
      .mockResolvedValueOnce(false)
      .mockResolvedValue(true);
    const initialView = { kind: "page-draft" as const, space: null, sessionKey: 90 };
    const { rerenderMain } = renderMain({ initialView });

    rerenderMain({ initialView, initialMemoryId: "memory-one" });
    await waitFor(() => expect(flushHarness.flush).toHaveBeenCalledOnce());

    rerenderMain({ initialView, initialMemoryId: "memory-two" });
    await waitFor(() => expect(flushHarness.flush).toHaveBeenCalledTimes(2));
    expect(await screen.findByTestId("memory-detail")).toHaveTextContent("memory-two");
  });

  it("removes a deleted primary page from tabs and does not restore it with Forward", async () => {
    const { user } = renderMain();
    await user.click(screen.getByRole("button", { name: "Wiki directory second page" }));
    expect(await screen.findByText("Second page")).toBeInTheDocument();
    const tabs = screen.getByRole("tablist", { name: "Open pages" });
    expect(within(tabs).getAllByRole("tab")).toHaveLength(2);

    await user.click(screen.getByRole("button", { name: "Delete current page" }));
    expect(await screen.findByText("Pending page")).toBeInTheDocument();
    expect(within(tabs).getAllByRole("tab")).toHaveLength(1);
    expect(screen.getByRole("button", { name: i18n.t("main.forward") })).toBeDisabled();
  });

  it("closes a page deleted from the Wiki directory and leaves the previous page on screen", async () => {
    const { user } = renderMain();
    await user.click(screen.getByRole("button", { name: "Wiki directory second page" }));
    expect(await screen.findByText("Second page")).toBeInTheDocument();
    const tabs = screen.getByRole("tablist", { name: "Open pages" });
    expect(within(tabs).getAllByRole("tab")).toHaveLength(2);

    await user.click(screen.getByRole("button", { name: "Delete second page from directory" }));
    expect(await screen.findByText("Pending page")).toBeInTheDocument();
    expect(within(tabs).getAllByRole("tab")).toHaveLength(1);
    expect(screen.queryByText("Second page")).toBeNull();
    expect(screen.getByRole("button", { name: i18n.t("main.forward") })).toBeDisabled();
  });

  it("closes a page deleted from the Wiki directory that is open in a background tab", async () => {
    const { user } = renderMain();
    await user.click(screen.getByRole("button", { name: "Wiki directory second page" }));
    expect(await screen.findByText("Second page")).toBeInTheDocument();
    const tabs = screen.getByRole("tablist", { name: "Open pages" });
    await user.click(within(tabs).getAllByRole("tab")[0]);
    expect(await screen.findByText("Pending page")).toBeInTheDocument();
    expect(within(tabs).getAllByRole("tab")).toHaveLength(2);

    await user.click(screen.getByRole("button", { name: "Delete second page from directory" }));
    await waitFor(() => expect(within(tabs).getAllByRole("tab")).toHaveLength(1));
    expect(screen.getByText("Pending page")).toBeInTheDocument();
  });

  it("closes a draft deleted from the Wiki directory and returns to the previous page", async () => {
    const { user } = renderMain();
    await user.click(screen.getByRole("button", { name: "Open draft X" }));
    expect(await screen.findByTestId("draft-editor")).toBeInTheDocument();
    const tabs = screen.getByRole("tablist", { name: "Open pages" });
    expect(within(tabs).getAllByRole("tab")).toHaveLength(2);

    await user.click(screen.getByRole("button", { name: "Delete draft X from directory" }));
    expect(await screen.findByText("Pending page")).toBeInTheDocument();
    expect(screen.queryByTestId("draft-editor")).toBeNull();
    expect(within(tabs).getAllByRole("tab")).toHaveLength(1);
  });

  it("closes an unhydrated secondary draft and allows quit without an editor handle", async () => {
    let quitGuard: (() => Promise<boolean>) | null = null;
    const { user } = renderMain({
      initialView: { kind: "page-draft", draftId: "missing-draft", space: null, sessionKey: 72 },
      onRegisterQuitGuard: guard => { quitGuard = guard; },
    });
    await user.pointer({ keys: "[MouseRight]", target: document.querySelector<HTMLElement>('[data-note-tab-group="primary"] [role="tab"][aria-selected="true"]')! });
    await user.click(screen.getByRole("menuitem", { name: "Move to right group" }));
    const right = await screen.findByRole("region", { name: "Right pages" });
    expect(within(right).getByText("Unhydrated draft")).toBeInTheDocument();
    await expect(quitGuard!()).resolves.toBe(true);

    await user.click(within(right).getByRole("button", { name: "Draft back" }));
    await waitFor(() => expect(screen.queryByRole("region", { name: "Right pages" })).toBeNull());
  });

  it("routes a newly saved draft identity to its current owner without mounting a second editor", async () => {
    const { user } = renderMain({
      initialView: { kind: "page-draft", space: null, sessionKey: 81 },
    });
    await user.click(screen.getByRole("button", { name: "Report draft X identity" }));
    await user.pointer({ keys: "[MouseRight]", target: document.querySelector<HTMLElement>('[data-note-tab-group="primary"] [role="tab"][aria-selected="true"]')! });
    await user.click(screen.getByRole("menuitem", { name: "Move to right group" }));
    const right = await screen.findByRole("region", { name: "Right pages" });
    const ownerEditor = within(right).getByTestId("draft-editor");

    await user.click(screen.getByRole("button", { name: "Open draft X" }));
    expect(screen.getAllByTestId("draft-editor")).toHaveLength(1);
    expect(within(right).getByTestId("draft-editor")).toBe(ownerEditor);
  });

  it("records a guarded root exit from an initial Page and restores it with Forward", async () => {
    flushHarness.enabled = true;
    let settle!: (saved: boolean) => void;
    flushHarness.flush.mockImplementation(() => new Promise((resolve) => { settle = resolve; }));
    const { user } = renderMain();
    await user.click(screen.getByRole("button", { name: i18n.t("main.back") }));
    expect(screen.getByText("Pending page")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: i18n.t("main.forward") })).toBeDisabled();
    await act(async () => settle(true));
    expect(screen.getByTestId("pages-overview")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: i18n.t("main.forward") }));
    expect(screen.getByText("Pending page")).toBeInTheDocument();
  });

  it("keeps Back and Forward stacks intact when a published Page cannot save", async () => {
    flushHarness.enabled = true;
    flushHarness.flush.mockResolvedValue(false);
    const { user } = renderMain();
    await user.click(screen.getByRole("button", { name: i18n.t("main.back") }));
    expect(screen.getByText("Pending page")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: i18n.t("main.forward") })).toBeDisabled();
    flushHarness.flush.mockResolvedValue(true);
    await user.click(screen.getByRole("button", { name: "Wiki directory second page" }));
    await user.click(screen.getByRole("button", { name: i18n.t("main.back") }));
    expect(screen.getByText("Pending page")).toBeInTheDocument();
    flushHarness.flush.mockResolvedValue(false);
    await user.click(screen.getByRole("button", { name: i18n.t("main.forward") }));
    expect(screen.getByText("Pending page")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: i18n.t("main.forward") })).toBeEnabled();
  });

  it("lets the latest destination supersede global Back while a Page save waits", async () => {
    flushHarness.enabled = true;
    let settle!: (saved: boolean) => void;
    const saving = new Promise<boolean>((resolve) => { settle = resolve; });
    flushHarness.flush.mockReturnValue(saving);
    const { user } = renderMain();
    await user.click(screen.getByRole("button", { name: i18n.t("main.back") }));
    await user.click(screen.getByRole("button", { name: "Wiki directory second page" }));
    expect(screen.getByText("Pending page")).toBeInTheDocument();
    await act(async () => settle(true));
    expect(screen.getByText("Second page")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: i18n.t("main.forward") })).toBeDisabled();
  });

  it("waits for automatic saving before sidebar navigation without a discard prompt", async () => {
    flushHarness.enabled = true;
    let settle!: (saved: boolean) => void;
    flushHarness.flush.mockImplementation(() => new Promise((resolve) => { settle = resolve; }));
    const confirmSpy = vi.spyOn(window, "confirm");
    const { user } = renderMain();
    await user.click(screen.getByRole("button", { name: "Make page dirty" }));
    await user.click(screen.getByRole("button", { name: "Start page save" }));
    await user.click(screen.getByRole("button", { name: "Sidebar Wiki" }));
    expect(screen.getByText("Pending page")).toBeInTheDocument();
    expect(flushHarness.flush).toHaveBeenCalledOnce();
    await act(async () => settle(true));
    expect(await screen.findByTestId("pages-overview")).toBeInTheDocument();
    expect(confirmSpy).not.toHaveBeenCalled();
  });

  it("lets the latest sidebar intent supersede repeated Back while saving", async () => {
    flushHarness.enabled = true;
    let settle!: (saved: boolean) => void;
    const saving = new Promise<boolean>((resolve) => { settle = resolve; });
    flushHarness.flush.mockReturnValue(saving);
    const confirmSpy = vi.spyOn(window, "confirm");
    const { user } = renderMain();
    await user.click(screen.getByRole("button", { name: "Start page save" }));
    await user.click(screen.getByRole("button", { name: "PageDetail back" }));
    await user.click(screen.getByRole("button", { name: "PageDetail back" }));
    await user.click(screen.getByRole("button", { name: "Wiki directory second page" }));
    expect(screen.getByText("Pending page")).toBeInTheDocument();
    expect(flushHarness.flush).toHaveBeenCalledTimes(3);
    await act(async () => settle(true));
    expect(await screen.findByText("Second page")).toBeInTheDocument();
    expect(screen.queryByTestId("pages-overview")).toBeNull();
    expect(confirmSpy).not.toHaveBeenCalled();
  });

  it("keeps the current page when automatic saving fails and prevents quit", async () => {
    flushHarness.enabled = true;
    flushHarness.flush.mockResolvedValue(false);
    let quitGuard: (() => Promise<boolean>) | null = null;
    const { user } = renderMain({ initialPageId: "page-one", onRegisterQuitGuard: (guard) => { quitGuard = guard; } });
    await user.click(screen.getByRole("button", { name: "Make page dirty" }));
    await user.click(screen.getByRole("button", { name: "Sidebar Wiki" }));
    expect(screen.getByText("Pending page")).toBeInTheDocument();
    await expect(quitGuard!()).resolves.toBe(false);
  });

  it("flushes the first search query and keeps the mounted editor and latest input", async () => {
    flushHarness.enabled = true;
    let settle!: (saved: boolean) => void;
    const saving = new Promise<boolean>((resolve) => { settle = resolve; });
    flushHarness.flush.mockReturnValue(saving);
    const { user } = renderMain();
    await user.click(screen.getByRole("button", { name: "Make page dirty" }));
    await user.click(screen.getByRole("button", { name: "Search" }));
    const search = screen.getByPlaceholderText("Search pages, memories, sources...");
    fireEvent.change(search, { target: { value: "first" } });
    fireEvent.change(search, { target: { value: "latest" } });
    expect(screen.getByText("Pending page")).toBeInTheDocument();
    await act(async () => settle(true));
    expect(search).toHaveValue("latest");
    expect(screen.getByText("Pending page")).toBeInTheDocument();
    expect(flushHarness.flush).toHaveBeenCalledTimes(2);
  });

  it("shows the pairing dialog over a page that is saving or has unsaved edits, without navigating or asking", async () => {
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(false);
    const { user } = renderMain();
    expect(await screen.findByText("Pending page")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Make page dirty" }));
    await user.click(screen.getByRole("button", { name: "Start page save" }));
    act(() => setPendingPairingCode("a".repeat(64)));
    expect(screen.getByTestId("pairing-dialog")).toBeVisible();
    // The page stays open, and nothing asked to discard its edits.
    expect(screen.getByText("Pending page")).toBeInTheDocument();
    expect(confirmSpy).not.toHaveBeenCalled();
  });

  it("blocks sidebar, search focus, and quit while a page save is pending", async () => {
    let quitGuard: (() => Promise<boolean>) | null = null;
    const { user } = renderMain({
      initialPageId: "page-one",
      onRegisterQuitGuard: (guard) => {
        quitGuard = guard;
      },
    });
    expect(await screen.findByText("Pending page")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Start page save" }));
    const searchAction = screen.getByRole("button", { name: "Search" });
    expect(searchAction).toBeDisabled();
    expect(screen.queryByRole("dialog", { name: "Search" })).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Sidebar Wiki" }));
    expect(screen.getByText("Pending page")).toBeInTheDocument();

    eventListeners.get("focus-search")?.();
    expect(screen.queryByRole("dialog", { name: "Search" })).not.toBeInTheDocument();
    expect(quitGuard).not.toBeNull();
    await expect(quitGuard!()).resolves.toBe(false);

    await user.click(screen.getByRole("button", { name: "Finish page save" }));
    await waitFor(() => expect(searchAction).toBeEnabled());
    await expect(quitGuard!()).resolves.toBe(true);
    eventListeners.get("focus-search")?.();
    expect(await screen.findByRole("dialog", { name: "Search" })).toBeInTheDocument();
    expect(screen.getByPlaceholderText("Search pages, memories, sources...")).toHaveFocus();

    await user.click(screen.getByRole("button", { name: "Make page dirty" }));
    await expect(quitGuard!()).resolves.toBe(false);
  });

  it("defers an external page replacement until the pending save settles", async () => {
    const { user, rerenderMain } = renderMain();
    expect(await screen.findByText("Pending page")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Start page save" }));

    rerenderMain({ initialPageId: "page-two" });
    expect(screen.getByText("Pending page")).toBeInTheDocument();
    expect(screen.queryByText("Second page")).toBeNull();

    await user.click(screen.getByRole("button", { name: "Finish page save" }));
    expect(await screen.findByText("Second page")).toBeInTheDocument();
  });

  it("does not replay the consumed initial page after internal navigation", async () => {
    const { user } = renderMain();
    expect(await screen.findByText("Pending page")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Sidebar Wiki" }));
    await user.click(screen.getByRole("button", { name: "Open second page" }));
    expect(await screen.findByText("Second page")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Start page save" }));
    await user.click(screen.getByRole("button", { name: "Finish page save" }));

    expect(screen.getByText("Second page")).toBeInTheDocument();
    expect(screen.queryByText("Pending page")).toBeNull();
  });

  it("keeps a dirty draft guarded when the active page is reselected", async () => {
    let quitGuard: (() => Promise<boolean>) | null = null;
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(true);
    const { user } = renderMain({
      initialPageId: "page-one",
      onRegisterQuitGuard: (guard) => {
        quitGuard = guard;
      },
    });
    expect(await screen.findByText("Pending page")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Make page dirty" }));

    await user.click(
      screen.getByRole("button", { name: "Reselect current page" }),
    );

    expect(confirmSpy).not.toHaveBeenCalled();
    expect(screen.getByText("Pending page")).toBeInTheDocument();
    expect(quitGuard).not.toBeNull();
    await expect(quitGuard!()).resolves.toBe(false);
  });

  it("leaves slash and Escape to editor input, textarea, select, and contenteditable targets", async () => {
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(false);
    const { user } = renderMain();
    expect(await screen.findByText("Pending page")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Make page dirty" }));

    await user.click(screen.getByRole("button", { name: "Search" }));
    const search = screen.getByPlaceholderText(
      "Search pages, memories, sources...",
    );
    const editor = screen.getByRole("textbox", { name: "Page editor" });
    const textarea = screen.getByRole("textbox", { name: "Page pages" });
    const select = screen.getByRole("combobox", { name: "Block style" });
    fireEvent.keyDown(editor, { key: "/" });
    fireEvent.keyDown(editor, { key: "Escape" });
    fireEvent.keyDown(textarea, { key: "Escape" });
    fireEvent.keyDown(select, { key: "Escape" });
    fireEvent.keyDown(search, { key: "Escape" });

    expect(search).not.toHaveFocus();
    expect(confirmSpy).not.toHaveBeenCalled();
    expect(screen.getByText("Pending page")).toBeInTheDocument();
  });

  it("ignores default-prevented and composing global shortcuts", async () => {
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(false);
    renderMain();
    expect(await screen.findByText("Pending page")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Make page dirty" }));

    const preventedEscape = new KeyboardEvent("keydown", {
      bubbles: true,
      cancelable: true,
      key: "Escape",
    });
    preventedEscape.preventDefault();
    act(() => window.dispatchEvent(preventedEscape));
    fireEvent.keyDown(window, { isComposing: true, key: "Escape" });

    expect(confirmSpy).not.toHaveBeenCalled();
    expect(screen.getByText("Pending page")).toBeInTheDocument();
  });

  it("confirms dirty global Escape but avoids a second confirmation for PageDetail Back", async () => {
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(false);
    const { user } = renderMain();
    expect(await screen.findByText("Pending page")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Make page dirty" }));

    const sidebarToggle = screen.getByRole("button", { name: "Toggle sidebar" });
    sidebarToggle.focus();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(confirmSpy).toHaveBeenCalledOnce();
    expect(screen.getByText("Pending page")).toBeInTheDocument();

    confirmSpy.mockReturnValue(true);
    await user.click(screen.getByRole("button", { name: "PageDetail back" }));
    expect(confirmSpy).toHaveBeenCalledTimes(2);
    expect(screen.getByTestId("pages-overview")).toBeInTheDocument();
  });

  it("confirms before dirty header search and sidebar navigation", async () => {
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(false);
    const { user } = renderMain();
    expect(await screen.findByText("Pending page")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Make page dirty" }));

    await user.click(screen.getByRole("button", { name: "Search" }));
    const search = screen.getByPlaceholderText(
      "Search pages, memories, sources...",
    );
    fireEvent.change(search, { target: { value: "blocked" } });
    expect(search).toHaveValue("");
    expect(confirmSpy).toHaveBeenCalledOnce();

    await user.click(screen.getByRole("button", { name: "Sidebar Wiki" }));
    expect(confirmSpy).toHaveBeenCalledTimes(2);
    expect(screen.getByText("Pending page")).toBeInTheDocument();

    confirmSpy.mockReturnValue(true);
    fireEvent.change(search, { target: { value: "accepted" } });
    await waitFor(() => expect(search).toHaveValue("accepted"));
    expect(confirmSpy).toHaveBeenCalledTimes(3);
  });

  it("guards prop-driven page and memory replacements while dirty", async () => {
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(false);
    const { user, rerenderMain } = renderMain();
    expect(await screen.findByText("Pending page")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Make page dirty" }));

    rerenderMain({ initialPageId: "page-two" });
    await waitFor(() => expect(confirmSpy).toHaveBeenCalledOnce());
    expect(screen.getByText("Pending page")).toBeInTheDocument();

    rerenderMain({
      initialMemoryId: "memory-one",
      initialPageId: "page-one",
    });
    await waitFor(() => expect(confirmSpy).toHaveBeenCalledTimes(2));
    expect(screen.getByText("Pending page")).toBeInTheDocument();

    confirmSpy.mockReturnValue(true);
    rerenderMain({ initialPageId: "page-two" });
    expect(await screen.findByText("Second page")).toBeInTheDocument();
  });
});
