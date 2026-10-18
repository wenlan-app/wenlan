// SPDX-License-Identifier: AGPL-3.0-only
import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { useState, type ComponentProps } from "react";
import { i18n } from "../../i18n";
import { RECENT_PAGES_STORAGE_KEY } from "../../lib/recentPages";
import { RECENT_SPACES_STORAGE_KEY } from "../../lib/recentSpaces";
import type { Page, SearchResult, Space } from "../../lib/tauri";
import { createPageDraft, getPage, publishPageDraft, searchPages, updatePageDraft } from "../../lib/tauri";
import { clearPendingPairingCode } from "../../lib/pairingLink";
import Main from "./Main";
import ActivityStatus from "./activity/ActivityStatus";

const eventListeners = vi.hoisted(
  () => new Map<string, (payload?: unknown) => void>(),
);
const importCompletion = vi.hoisted(() => ({ imported: 1 }));
const listSpacesMock = vi.hoisted(() => vi.fn<() => Promise<readonly Space[]>>());
const openFileMock = vi.hoisted(() => vi.fn<(url: string) => Promise<void>>());
const openSearchResultMock = vi.hoisted(() => vi.fn<(url: string) => Promise<void>>());
const draftRequestBackMock = vi.hoisted(
  () => vi.fn<(onBack: () => void) => Promise<void>>(),
);
const draftFlushMock = vi.hoisted(
  () => vi.fn<() => Promise<boolean>>(),
);
const draftIdentityMock = vi.hoisted(
  () => vi.fn<() => {
    readonly draftId: string | null;
    readonly version: number | null;
    readonly publishedPage?: Page;
    readonly projectionIssue?: { readonly expectedVersion: number };
  }>(),
);
const realDraftEditor = vi.hoisted(() => ({ enabled: false }));
const setSearchQueryMock = vi.hoisted(() => vi.fn());
const takeRemotePairingLinkMock = vi.hoisted(() => vi.fn<() => Promise<string | null>>());
const useSearchMock = vi.hoisted(() => vi.fn((): {
  query: string;
  setQuery: typeof setSearchQueryMock;
  debouncedQuery: string;
  results: SearchResult[];
  isLoading?: boolean;
  error?: unknown;
} => ({
  query: "",
  setQuery: setSearchQueryMock,
  debouncedQuery: "",
  results: [],
})));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn((event: string, handler: (payload?: unknown) => void) => {
    eventListeners.set(event, handler);
    return Promise.resolve(() => eventListeners.delete(event));
  }),
}));

const reloadWindowMock = vi.hoisted(() => vi.fn());
vi.mock("../../lib/reloadWindow", () => ({ reloadWindow: reloadWindowMock }));

vi.mock("../../hooks/useSearch", () => ({
  useSearch: useSearchMock,
}));

vi.mock("../../lib/tauri", () => ({
  shouldShowWizard: vi.fn().mockResolvedValue(true),
  listMemoriesRich: vi.fn().mockResolvedValue([]),
  getMemoryStats: vi.fn().mockResolvedValue({ total: 0, new_today: 0, confirmed: 0, domains: [] }),
  searchEntities: vi.fn().mockResolvedValue([]),
  searchPages: vi.fn().mockResolvedValue([]),
  // Main reads the getting-started state to decide whether the Wiki shows its directory.
  listAgents: vi.fn().mockResolvedValue([]),
  listRegisteredSources: vi.fn().mockResolvedValue([]),
  listOnboardingMilestones: vi.fn().mockResolvedValue([]),
  listSpaces: listSpacesMock,
  openFile: openFileMock,
  openSearchResult: openSearchResultMock,
  deleteFileChunks: vi.fn().mockResolvedValue(undefined),
  takeRemotePairingLink: takeRemotePairingLinkMock,
  // Never settles, so the toolbar Activity button stays the plain navigation
  // button these routing tests click.
  getActivity: vi.fn(() => new Promise(() => {})),
  createPageDraft: vi.fn(),
  discardPageDraft: vi.fn(),
  getPage: vi.fn(),
  publishPageDraft: vi.fn(),
  updatePageDraft: vi.fn(),
}));

vi.mock("./ActivityFeed", () => ({ default: () => <div data-testid="activity-feed" /> }));
// The approval dialog has its own tests; here only that Main shows it, and where.
vi.mock("./PairingApprovalDialog", async () => {
  const { usePendingPairingCode } = await import("../../lib/pairingLink");
  return { default: () => (usePendingPairingCode() ? <div role="dialog" data-testid="pairing-dialog" /> : null) };
});
vi.mock("./RemoteAccessNotifier", () => ({ default: () => <div data-testid="remote-access-notifier" /> }));
vi.mock("./IdentityDetail", () => ({ default: () => <div /> }));
vi.mock("./MemoryStream", () => ({ default: ({ toolbarActions }: { toolbarActions?: import("react").ReactNode }) => <div><h2>Memories</h2>{toolbarActions}</div> }));
vi.mock("./AtlasView", () => ({
  default: (props: {
    onBack?: () => void;
    onNodeClick?: (target: { kind: "entity" | "memory"; id: string }) => void;
  }) => (
    <div data-testid="atlas-view">
      <button type="button" onClick={() => props.onBack?.()}>
        Atlas back
      </button>
      <button type="button" onClick={() => props.onNodeClick?.({ kind: "entity", id: "ent-1" })}>
        Atlas node
      </button>
      <button
        type="button"
        onClick={() => props.onNodeClick?.({ kind: "memory", id: "memory-1" })}
      >
        Atlas memory node
      </button>
    </div>
  ),
}));
vi.mock("./EntityDetail", () => ({ default: () => <div data-testid="entity-detail" /> }));
vi.mock("./MemorySearchResult", () => ({
  default: (props: { result: { content: string; source_id: string }; onClick?: (sourceId: string) => void }) => (
    <button type="button" onClick={() => props.onClick?.(props.result.source_id)}>
      {props.result.content}
    </button>
  ),
}));
vi.mock("./MemoryDetail", () => ({ default: () => <div data-testid="memory-detail" /> }));
vi.mock("./RecapsList", () => ({
  RecapsList: (props: { onBack: () => void }) => (
    <section data-testid="recaps-list">
      <button onClick={props.onBack} type="button">Recaps back</button>
    </section>
  ),
}));
vi.mock("./PageDetail", () => ({
  default: (props: {
    onBack?: () => void;
    onPageLoaded?: (page: Pick<Page, "id" | "status" | "title">) => void;
    onSavePendingChange?: (pending: boolean) => void;
    pageId: string;
  }) => (
    <div
      data-page-id={props.pageId}
      data-testid="page-detail"
    >
      <button
        onClick={() => props.onPageLoaded?.({ id: props.pageId, status: "active", title: "Visited page" })}
        type="button"
      >
        Finish loading page
      </button>
      <button onClick={props.onBack} type="button">Page back</button>
      <button type="button" onClick={() => props.onSavePendingChange?.(true)}>Start mocked page save</button>
      <button type="button" onClick={() => props.onSavePendingChange?.(false)}>Finish mocked page save</button>
    </div>
  ),
}));
vi.mock("./DistillReviewPanel", () => ({ default: () => <div /> }));
vi.mock("./SettingsPage", () => ({
  default: (props: { onSetupAgent?: () => void; onImport?: () => void; section?: string; onBack?: () => void }) => (
    <div data-testid="settings-page" data-section={props.section}>
      <button type="button" onClick={props.onSetupAgent}>Connect agent</button>
      <button type="button" onClick={props.onBack}>Settings back</button>
      <button type="button" onClick={props.onImport}>Settings import</button>
    </div>
  ),
}));
vi.mock("../SetupWizard", () => ({ SetupWizard: () => <div data-testid="client-setup-wizard" /> }));
vi.mock("./Sidebar", () => ({
  default: (props: {
    activeNavigation?: string | null;
    hidden: boolean;
    mode: "icons" | "labels";
    open?: boolean;
    onEntityClick: (entityId: string) => void;
    onNavigateLog?: () => void;
    onNavigateActivity?: () => void;
    activityCurrent?: boolean;
    onNavigateGraph?: () => void;
    onNavigatePages?: () => void;
    onNavigateSettings?: () => void;
    onNavigateSpaces?: (create: boolean) => void;
    onOpenSearch?: (trigger: HTMLButtonElement) => void;
    onRequestClose?: () => void;
    searchDisabled?: boolean;
    searchOpen?: boolean;
    presentation?: string;
  }) => (
    <aside
      data-active={props.activeNavigation ?? "none"}
      data-collapsed={props.hidden ? "true" : "false"}
      data-mode={props.mode}
      data-open={props.open ? "true" : "false"}
      data-presentation={props.presentation ?? "desktop"}
    >
      <button
        aria-expanded={props.searchOpen ?? false}
        disabled={props.searchDisabled}
        onClick={(event) => props.onOpenSearch?.(event.currentTarget)}
        type="button"
      >
        {i18n.t("main.searchButton")}
      </button>
      <button type="button" onClick={() => props.onEntityClick("__create_profile__")}>
        Open avatar menu destination
      </button>
      <ActivityStatus onOpenActivity={props.onNavigateActivity ?? (() => {})} current={props.activityCurrent} />
      <button type="button" onClick={props.onNavigateLog}>Open memories</button>
      <button type="button" onClick={props.onNavigatePages}>Open wiki</button>
      <button type="button" onClick={props.onNavigateSettings}>Open settings</button>
      <button type="button" onClick={props.onNavigateGraph}>Open graph</button>
      <button type="button" onClick={() => props.onNavigateSpaces?.(false)}>Open spaces</button>
      <button type="button" onClick={() => props.onNavigateSpaces?.(true)}>Create space</button>
      {props.presentation === "overlay" && props.open && (
        <button type="button" aria-label="Close sidebar" onClick={props.onRequestClose} />
      )}
    </aside>
  ),
  SidebarToggleButton: (props: { onToggle: () => void; ref?: React.Ref<HTMLButtonElement> }) => (
    <button ref={props.ref} type="button" aria-label="Toggle sidebar" onClick={props.onToggle} />
  ),
  SidebarHeaderDivider: (props: { visible: boolean }) => (
    props.visible ? <div data-sidebar-header-divider="true" /> : null
  ),
}));
vi.mock("./navigation/ContextBrowser", () => ({
  ContextBrowser: (props: {
    currentPageId: string | null;
    onCreatePage: () => void;
    onSelectDraft: (draftId: string, space: string | null) => void;
    onSelectPage: (page: Page) => void;
  }) => (
    <div data-current-page={props.currentPageId ?? "none"} data-testid="context-browser">
      <button type="button" onClick={props.onCreatePage}>New context note</button>
      <button type="button" onClick={() => props.onSelectDraft("draft-a", null)}>Select draft A</button>
      <button type="button" onClick={() => props.onSelectDraft("draft-b", null)}>Select draft B</button>
      <button type="button" onClick={() => props.onSelectPage({ id: "page-1", status: "active", title: "Recent page" } as Page)}>Open recent page</button>
    </div>
  ),
}));
vi.mock("./pages/WikiWorkspace", () => ({
  WikiWorkspace: (props: {
    children: React.ReactNode;
    tabs?: React.ReactNode;
    currentPageId?: string | null;
    onCreatePage: () => void;
    onOpenDraft: (draftId: string, space: string | null) => void;
    onOpenPage: (page: Page) => void;
  }) => (
    <div data-current-page={props.currentPageId ?? "none"} data-testid="wiki-workspace">
      {props.tabs}{props.children}
      <button type="button" onClick={() => props.onCreatePage()}>New context note</button>
      <button type="button" onClick={() => props.onOpenDraft("draft-a", null)}>Select draft A</button>
      <button type="button" onClick={() => props.onOpenDraft("draft-b", null)}>Select draft B</button>
      <button type="button" onClick={() => props.onOpenDraft("draft-new", null)}>Open saved draft</button>
      <button type="button" onClick={() => props.onOpenPage({ id: "page-1", status: "active", title: "Recent page" } as Page)}>Open recent page</button>
    </div>
  ),
}));
vi.mock("./pages/PagesOverview", () => ({
  PagesOverview: (props: {
    onCreatePage: (space: string | null) => void;
    onSelectDraft: (draftId: string, space: string | null) => void;
  }) => (
    <section data-testid="pages-overview">
      <button
        onClick={() => props.onCreatePage(null)}
        type="button"
      >
        Create standalone draft
      </button>
      <button
        onClick={() => props.onSelectDraft("draft-resume", "Research")}
        type="button"
      >
        Resume draft
      </button>
    </section>
  ),
}));
vi.mock("./pages/PageDraftEditor", async () => {
  const React = await import("react");
  const actual = await vi.importActual<typeof import("./pages/PageDraftEditor")>("./pages/PageDraftEditor");
  return {
    PageDraftEditor: React.forwardRef((props: {
      draftId?: string;
      onBack: () => void;
      onDraftIdentity?: (draftId: string) => void;
      onEscapeBeforeLeave?: () => boolean;
      onOpenExisting: (pageId: string) => void;
      onPublished: (pageId: string) => void;
      space: string | null;
    }, ref) => {
      if (realDraftEditor.enabled) return <actual.PageDraftEditor {...props} ref={ref as React.Ref<import("./pages/PageDraftEditor").PageDraftEditorHandle>} />;
      const requestBack = () => draftRequestBackMock(props.onBack);
      React.useImperativeHandle(ref, () => ({
        flush: async () => {
          const saved = await draftFlushMock();
          const identity = draftIdentityMock();
          if (saved && identity.draftId) props.onDraftIdentity?.(identity.draftId);
          return saved;
        },
        getIdentity: draftIdentityMock,
        requestBack,
      }));
      React.useEffect(() => {
        const handleEscape = (event: KeyboardEvent) => {
          if (event.key !== "Escape") return;
          event.preventDefault();
          event.stopPropagation();
          if (props.onEscapeBeforeLeave?.()) return;
          void requestBack();
        };
        window.addEventListener("keydown", handleEscape, true);
        return () => window.removeEventListener("keydown", handleEscape, true);
      });
      return (
        <section
          data-draft-id={props.draftId ?? "new"}
          data-space={props.space ?? "none"}
          data-testid="page-draft-editor"
        >
          <button onClick={() => void requestBack()} type="button">Draft back</button>
          <button onClick={() => props.onPublished(props.draftId ?? "draft-new")} type="button">
            Publish draft
          </button>
          <button onClick={() => props.onOpenExisting("page-existing")} type="button">
            Open existing conflict
          </button>
        </section>
      );
    }),
  };
});
vi.mock("./settings/SettingsSidebar", () => ({ default: () => <aside /> }));
vi.mock("./navigation/ReviewEnvironmentBadge", () => ({
  ReviewEnvironmentBadge: (props: { compact?: boolean }) => (
    <div data-compact={props.compact ? "true" : "false"} data-testid="review-environment-badge" />
  ),
}));
vi.mock("./SpaceDetail", () => ({
  default: (props: {
    onBack: () => void;
    onCreatePage?: (space: string) => void;
    onSpaceDeleted?: (spaceId: string) => void;
    onSpaceLoaded?: (space: Space) => void;
    onSpaceRenamed?: (space: Pick<Space, "id" | "name">) => void;
    spaceName: string;
  }) => (
    <div data-space-name={props.spaceName} data-testid="space-detail">
      <button type="button" onClick={props.onBack}>Space parent</button>
      <button type="button" onClick={() => props.onCreatePage?.(props.spaceName)}>Create Space draft</button>
      <button type="button" onClick={() => props.onSpaceLoaded?.(space("space-1", props.spaceName))}>Finish loading space</button>
      <button type="button" onClick={() => props.onSpaceRenamed?.({ id: "space-1", name: "Renamed Work" })}>Rename loaded space</button>
      <button type="button" onClick={() => props.onSpaceDeleted?.("space-1")}>Delete loaded space</button>
      <button type="button" onClick={() => {
        props.onSpaceDeleted?.("space-1");
        props.onBack();
      }}>Complete Space deletion</button>
    </div>
  ),
}));
vi.mock("./spaces", () => ({
  SpacesOverview: (props: { createIntent?: boolean; onSelectSpace: (name: string) => void }) => (
    <section data-testid="spaces-overview">
      {props.createIntent && <input aria-label="Space name" autoFocus />}
      <button type="button" onClick={() => props.onSelectSpace("Work")}>Open managed space</button>
    </section>
  ),
}));
vi.mock("./DecisionLog", () => ({ default: () => <div /> }));
vi.mock("./MemoryCard", () => ({ default: () => <div /> }));
vi.mock("./ImportView", () => ({
  ImportView: (props: {
    onComplete: (source: string, result: { batch_id: string; imported: number }) => void;
    onBack: () => void;
  }) => (
    <div data-testid="import-view">
      <button
        type="button"
        onClick={() => props.onComplete("chatgpt", {
          batch_id: "import-batch-1",
          imported: importCompletion.imported,
        })}
      >
        Finish import
      </button>
      <button type="button" onClick={props.onBack}>Cancel import</button>
    </div>
  ),
}));
vi.mock("../onboarding/FirstUseGuide", () => ({
  FirstUseGuide: (props: {
    initialView?: string;
    batchId?: string;
    onImport: () => void;
    onBack: () => void;
    onOpenPage: (id: string) => void;
  }) => (
    <section
      data-testid="first-use-guide"
      data-view={props.initialView}
      data-batch-id={props.batchId ?? "none"}
    >
      <button type="button" onClick={props.onImport}>Bring memories</button>
      <button type="button" onClick={props.onBack}>Leave first use</button>
      <button type="button" onClick={() => props.onOpenPage("library-page")}>Open knowledge result</button>
    </section>
  ),
}));

function draftPage(id: string, title: string, content: string): Page {
  return {
    id, title, content, summary: null, entity_id: null, domain: null, space: null,
    source_memory_ids: [], version: 1, status: "draft", creation_kind: "authored",
    review_status: "unconfirmed", created_at: "", last_compiled: "", last_modified: "",
  };
}

function activePage(page: Page, version = page.version + 1): Page {
  return { ...page, status: "active", version };
}

function space(id: string, name: string): Space {
  return {
    id,
    name,
    description: null,
    suggested: false,
    starred: false,
    sort_order: 0,
    memory_count: 1,
    entity_count: 0,
    created_at: 0,
    updated_at: 1,
  };
}

function stubResponsiveViewport(initialMatches: boolean) {
  let matches = initialMatches;
  const listeners = new Set<(event: { readonly matches: boolean }) => void>();
  vi.stubGlobal("matchMedia", vi.fn((query: string) => ({
    get matches() { return matches; },
    media: query,
    onchange: null,
    addEventListener: (_type: string, listener: (event: { readonly matches: boolean }) => void) => listeners.add(listener),
    removeEventListener: (_type: string, listener: (event: { readonly matches: boolean }) => void) => listeners.delete(listener),
    addListener: vi.fn(),
    removeListener: vi.fn(),
    dispatchEvent: vi.fn(),
  })));
  return {
    setMatches(next: boolean) {
      matches = next;
      for (const listener of listeners) listener({ matches: next });
    },
  };
}

function renderMain(props: ComponentProps<typeof Main> = {}) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const view = render(
    <QueryClientProvider client={queryClient}>
      <Main {...props} />
    </QueryClientProvider>,
  );
  return {
    ...view,
    queryClient,
    rerenderMain: (nextProps: ComponentProps<typeof Main> = {}) =>
      view.rerender(
        <QueryClientProvider client={queryClient}>
          <Main {...nextProps} />
        </QueryClientProvider>,
      ),
  };
}

async function openManagedSpace(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByRole("button", { name: "Open spaces" }));
  await user.click(screen.getByRole("button", { name: "Open managed space" }));
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((next) => {
    resolve = next;
  });
  return { promise, resolve };
}

function getSearchInput() {
  if (!screen.queryByRole("dialog")) {
    fireEvent.click(screen.getByRole("button", { name: i18n.t("main.searchButton") }));
  }
  return screen.getByPlaceholderText(i18n.t("main.searchPlaceholder"));
}

describe("Main search", () => {
  beforeEach(async () => {
    realDraftEditor.enabled = false;
    vi.mocked(createPageDraft).mockReset();
    vi.mocked(getPage).mockReset();
    vi.mocked(publishPageDraft).mockReset();
    vi.mocked(updatePageDraft).mockReset();
    eventListeners.clear();
    importCompletion.imported = 1;
    listSpacesMock.mockReset();
    listSpacesMock.mockResolvedValue([]);
    openFileMock.mockReset();
    openFileMock.mockResolvedValue(undefined);
    openSearchResultMock.mockReset();
    openSearchResultMock.mockResolvedValue(undefined);
    setSearchQueryMock.mockReset();
    vi.mocked(searchPages).mockReset();
    vi.mocked(searchPages).mockResolvedValue([]);
    takeRemotePairingLinkMock.mockReset();
    takeRemotePairingLinkMock.mockResolvedValue(null);
    clearPendingPairingCode();
    draftRequestBackMock.mockReset();
    draftRequestBackMock.mockImplementation(async (onBack) => onBack());
    draftFlushMock.mockReset();
    draftFlushMock.mockResolvedValue(true);
    draftIdentityMock.mockReset();
    draftIdentityMock.mockReturnValue({ draftId: "draft-new", version: 1 });
    useSearchMock.mockReset();
    useSearchMock.mockReturnValue({ query: "", setQuery: setSearchQueryMock, debouncedQuery: "", results: [] });
    localStorage.clear();
    vi.unstubAllGlobals();
    await i18n.changeLanguage("en");
  });

  it("shows returned page excerpts and opens the same page from search", async () => {
    vi.mocked(searchPages).mockResolvedValue([{
      id: "page-recovery", title: "Recovery", domain: "Wenlan", summary: null,
      content: `${"Introduction. ".repeat(30)}Rollback preserves the original source.`,
    } as Page]);
    useSearchMock.mockImplementation(() => {
      const [query, setQuery] = useState("rollback");
      setSearchQueryMock.mockImplementation(setQuery);
      return { query, setQuery: setSearchQueryMock, debouncedQuery: query, results: [] };
    });
    const user = userEvent.setup();
    renderMain();
    await user.click(screen.getByRole("button", { name: i18n.t("main.searchButton") }));
    const row = await screen.findByRole("button", { name: /Recovery.*Rollback preserves the original source/ });
    expect(searchPages).toHaveBeenCalledWith("rollback", 5);
    await user.click(row);
    expect(screen.getByTestId("page-detail")).toHaveAttribute("data-page-id", "page-recovery");
  });

  it("keeps the Wiki directory inside the workspace instead of the header", () => {
    renderMain();
    expect(screen.getByTestId("wiki-workspace")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Notes and folders" })).not.toBeInTheDocument();
  });

  it("traverses Wiki, Spaces, and Page in both directions without resetting root history", async () => {
    const user = userEvent.setup();
    renderMain();
    const back = () => screen.getByRole("button", { name: i18n.t("main.back") });
    const forward = () => screen.getByRole("button", { name: i18n.t("main.forward") });
    expect(back()).toBeDisabled();
    expect(forward()).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "Open wiki" }));
    expect(back()).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "Open spaces" }));
    await user.click(screen.getByRole("button", { name: "Open wiki" }));
    await user.click(screen.getByRole("button", { name: "Open recent page" }));
    expect(screen.getByTestId("page-detail")).toHaveAttribute("data-page-id", "page-1");
    expect(screen.getByRole("main")).toHaveClass("memory-main-content--wiki");
    await user.click(back());
    expect(screen.getByTestId("pages-overview")).toBeVisible();
    await user.click(back());
    expect(screen.getByTestId("spaces-overview")).toBeVisible();
    await user.click(back());
    expect(screen.getByTestId("pages-overview")).toBeVisible();
    expect(back()).toBeDisabled();
    await user.click(forward());
    expect(screen.getByTestId("spaces-overview")).toBeVisible();
    await user.click(forward());
    expect(screen.getByTestId("pages-overview")).toBeVisible();
    await user.click(forward());
    expect(screen.getByTestId("page-detail")).toBeVisible();
    expect(forward()).toBeDisabled();
  });

  it("keeps Activity and Memories in the shared history across sidebar roots", async () => {
    const user = userEvent.setup();
    renderMain();
    await user.click(screen.getByRole("button", { name: "Activity" }));
    await user.click(screen.getByTestId("activity-summary-open"));
    expect(screen.getByTestId("activity-feed")).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Open memories" }));
    expect(screen.getByRole("heading", { name: "Memories" })).toBeVisible();
    await user.click(screen.getByRole("button", { name: i18n.t("main.back") }));
    expect(screen.getByTestId("activity-feed")).toBeVisible();
    await user.click(screen.getByRole("button", { name: i18n.t("main.back") }));
    expect(screen.getByTestId("pages-overview")).toBeVisible();
    expect(screen.getByRole("button", { name: i18n.t("main.back") })).toBeDisabled();
    await user.click(screen.getByRole("button", { name: i18n.t("main.forward") }));
    expect(screen.getByTestId("activity-feed")).toBeVisible();
    await user.click(screen.getByRole("button", { name: i18n.t("main.forward") }));
    expect(screen.getByRole("heading", { name: "Memories" })).toBeVisible();
  });

  it("uses Spaces as the initial Space fallback and records both exits for Forward", async () => {
    const user = userEvent.setup();
    renderMain({ initialView: { kind: "space", spaceId: "space-1", spaceName: "Work" } });
    await user.click(screen.getByRole("button", { name: i18n.t("main.back") }));
    expect(screen.getByTestId("spaces-overview")).toBeVisible();
    await user.click(screen.getByRole("button", { name: i18n.t("main.back") }));
    expect(screen.getByTestId("pages-overview")).toBeVisible();
    await user.click(screen.getByRole("button", { name: i18n.t("main.forward") }));
    expect(screen.getByTestId("spaces-overview")).toBeVisible();
    await user.click(screen.getByRole("button", { name: i18n.t("main.forward") }));
    expect(screen.getByTestId("space-detail")).toHaveAttribute("data-space-name", "Work");
  });

  it("clears Forward when a new sidebar destination branches from Back", async () => {
    const user = userEvent.setup();
    renderMain();
    await user.click(screen.getByRole("button", { name: "Open spaces" }));
    await user.click(screen.getByRole("button", { name: "Open wiki" }));
    await user.click(screen.getByRole("button", { name: "Open recent page" }));
    await user.click(screen.getByRole("button", { name: i18n.t("main.back") }));
    expect(screen.getByRole("button", { name: i18n.t("main.forward") })).toBeEnabled();
    await user.click(screen.getByRole("button", { name: "Open graph" }));
    expect(screen.getByTestId("atlas-view")).toBeVisible();
    expect(screen.getByRole("button", { name: i18n.t("main.forward") })).toBeDisabled();
  });

  it("uses the external detail exit only after local history is exhausted", async () => {
    const user = userEvent.setup();
    const onBackFromDetail = vi.fn();
    renderMain({ initialMemoryId: "memory-1", onBackFromDetail });
    await user.click(screen.getByRole("button", { name: "Open spaces" }));
    await user.click(screen.getByRole("button", { name: i18n.t("main.back") }));
    expect(screen.getByTestId("memory-detail")).toBeVisible();
    expect(onBackFromDetail).not.toHaveBeenCalled();
    await user.click(screen.getByRole("button", { name: i18n.t("main.back") }));
    expect(onBackFromDetail).toHaveBeenCalledOnce();
  });

  it("reloads on Cmd+R only after the open draft has saved", async () => {
    const user = userEvent.setup();
    const flush = deferred<boolean>();
    draftFlushMock.mockReturnValue(flush.promise);
    reloadWindowMock.mockReset();
    renderMain();
    await user.click(screen.getByRole("button", { name: "Create standalone draft" }));
    fireEvent.keyDown(window, { key: "r", metaKey: true });
    expect(reloadWindowMock).not.toHaveBeenCalled();
    await act(async () => flush.resolve(true));
    expect(reloadWindowMock).toHaveBeenCalledOnce();
  });

  it("keeps global Back in a draft until it saves and restores the saved identity with Forward", async () => {
    const user = userEvent.setup();
    const flush = deferred<boolean>();
    draftFlushMock.mockReturnValue(flush.promise);
    renderMain();
    await user.click(screen.getByRole("button", { name: "Create standalone draft" }));
    await user.click(screen.getByRole("button", { name: i18n.t("main.back") }));
    expect(screen.getByTestId("page-draft-editor")).toHaveAttribute("data-draft-id", "new");
    expect(screen.getByRole("button", { name: i18n.t("main.forward") })).toBeDisabled();
    await act(async () => flush.resolve(true));
    expect(screen.getByTestId("pages-overview")).toBeVisible();
    await user.click(screen.getByRole("button", { name: i18n.t("main.forward") }));
    expect(screen.getByTestId("page-draft-editor")).toHaveAttribute("data-draft-id", "draft-new");
  });

  it("clears Forward when an external memory replaces the current route", async () => {
    const user = userEvent.setup();
    const view = renderMain();
    await user.click(screen.getByRole("button", { name: "Open spaces" }));
    await user.click(screen.getByRole("button", { name: i18n.t("main.back") }));
    view.rerenderMain({ initialMemoryId: "memory-1" });
    expect(await screen.findByTestId("memory-detail")).toBeVisible();
    expect(screen.getByRole("button", { name: i18n.t("main.forward") })).toBeDisabled();
  });

  it("dismisses the search results and mobile overlay when Back or Forward commits", async () => {
    useSearchMock.mockImplementation(() => {
      const [query, setLocalQuery] = useState("");
      return { query, debouncedQuery: query, results: [], setQuery: setLocalQuery as typeof setSearchQueryMock };
    });
    const user = userEvent.setup();
    renderMain();
    await user.click(screen.getByRole("button", { name: "Open spaces" }));
    const search = getSearchInput();
    await user.click(screen.getByRole("button", { name: "Search" }));
    await user.type(search, "architecture");
    expect(screen.getByTestId("spaces-overview")).toBeVisible();
    await user.click(screen.getByRole("button", { name: i18n.t("main.back") }));
    expect(screen.queryByRole("dialog", { name: "Search" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Search" })).toHaveAttribute("aria-expanded", "false");
    expect(screen.getByTestId("pages-overview")).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Search" }));
    await user.type(getSearchInput(), "new query");
    await user.click(screen.getByRole("button", { name: i18n.t("main.forward") }));
    expect(screen.queryByRole("dialog", { name: "Search" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Search" })).toHaveAttribute("aria-expanded", "false");
    expect(screen.getByTestId("spaces-overview")).toBeVisible();
  });

  it("dismisses search on a same-destination sidebar intent without pushing history", async () => {
    useSearchMock.mockImplementation(() => {
      const [query, setLocalQuery] = useState("");
      return { query, debouncedQuery: query, results: [], setQuery: setLocalQuery as typeof setSearchQueryMock };
    });
    const user = userEvent.setup();
    renderMain();
    await user.click(screen.getByRole("button", { name: "Search" }));
    const search = getSearchInput();
    await user.type(search, "architecture");
    await user.click(screen.getByRole("button", { name: "Open wiki" }));
    expect(screen.queryByRole("dialog", { name: "Search" })).not.toBeInTheDocument();
    expect(screen.getByTestId("pages-overview")).toBeVisible();
    expect(screen.getByRole("button", { name: "Search" })).toHaveAttribute("aria-expanded", "false");
    expect(screen.getByRole("button", { name: i18n.t("main.back") })).toBeDisabled();
    expect(screen.getByRole("button", { name: i18n.t("main.forward") })).toBeDisabled();
  });

  it("keeps the search overlay on a same-Page intent until its pending save ends", async () => {
    const user = userEvent.setup();
    renderMain({ initialPageId: "page-1" });
    const searchAction = screen.getByRole("button", { name: "Search" });
    await user.click(searchAction);
    await user.click(screen.getByRole("button", { name: "Start mocked page save" }));
    await user.click(screen.getByRole("button", { name: "Open recent page" }));
    expect(searchAction).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByTestId("page-detail")).toHaveAttribute("data-page-id", "page-1");
    await user.click(screen.getByRole("button", { name: "Finish mocked page save" }));
    await user.click(screen.getByRole("button", { name: "Open recent page" }));
    expect(searchAction).toHaveAttribute("aria-expanded", "false");
    expect(screen.getByRole("button", { name: i18n.t("main.forward") })).toBeDisabled();
  });

  it("retains a draft's pending search and overlay when the latest Back save fails", async () => {
    const flush = deferred<boolean>();
    draftFlushMock.mockReturnValue(flush.promise);
    useSearchMock.mockImplementation(() => {
      const [query, setLocalQuery] = useState("");
      return { query, debouncedQuery: query, results: [], setQuery: setLocalQuery as typeof setSearchQueryMock };
    });
    const user = userEvent.setup();
    renderMain();
    await user.click(screen.getByRole("button", { name: "Create standalone draft" }));
    await user.click(screen.getByRole("button", { name: "Search" }));
    const search = getSearchInput();
    await user.type(search, "architecture");
    await user.click(screen.getByRole("button", { name: i18n.t("main.back") }));
    await act(async () => flush.resolve(false));
    expect(search).toHaveValue("architecture");
    expect(screen.getByRole("button", { name: "Search" })).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByTestId("page-draft-editor")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: i18n.t("main.forward") })).toBeDisabled();
  });

  it("replaces a deleted Space with its parent so Back cannot reopen the deleted destination", async () => {
    const user = userEvent.setup();
    renderMain();
    await openManagedSpace(user);
    await user.click(screen.getByRole("button", { name: "Complete Space deletion" }));
    expect(screen.getByTestId("spaces-overview")).toBeVisible();
    await user.click(screen.getByRole("button", { name: i18n.t("main.back") }));
    expect(screen.getByTestId("spaces-overview")).toBeVisible();
    expect(screen.queryByTestId("space-detail")).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: i18n.t("main.forward") }));
    expect(screen.getByTestId("spaces-overview")).toBeVisible();
    expect(screen.queryByTestId("space-detail")).not.toBeInTheDocument();
  });

  it("shows the approval dialog for a wenlan://pair link that launched the app, without leaving the screen", async () => {
    takeRemotePairingLinkMock.mockResolvedValueOnce("a".repeat(64));
    renderMain();
    expect(await screen.findByTestId("pairing-dialog")).toBeVisible();
    expect(screen.getByTestId("pages-overview")).toBeVisible();
    expect(screen.queryByTestId("settings-page")).not.toBeInTheDocument();
  });

  it("listens for Web access notices from the app shell, whichever screen is open", async () => {
    renderMain();
    expect(await screen.findByTestId("remote-access-notifier")).toBeInTheDocument();
  });

  it("shows the approval dialog when a pairing link arrives while the app is open", async () => {
    renderMain();
    expect(screen.getByTestId("pages-overview")).toBeVisible();
    expect(screen.queryByTestId("pairing-dialog")).not.toBeInTheDocument();
    takeRemotePairingLinkMock.mockResolvedValueOnce("a".repeat(64));
    await act(async () => { eventListeners.get("remote-pairing-link")?.(); });
    expect(await screen.findByTestId("pairing-dialog")).toBeVisible();
    expect(screen.getByTestId("pages-overview")).toBeVisible();
    expect(screen.queryByTestId("settings-page")).not.toBeInTheDocument();
  });

  it("completes a Settings import in Memories and returns to Wiki", async () => {
    const user = userEvent.setup();
    renderMain();
    await user.click(screen.getByRole("button", { name: "Open settings" }));
    await user.click(screen.getByRole("button", { name: "Settings import" }));
    expect(screen.getByTestId("import-view")).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Finish import" }));
    expect(screen.getByRole("heading", { name: "Memories" })).toBeVisible();
    expect(screen.getByRole("complementary")).toHaveAttribute("data-active", "memories");
    await user.click(screen.getByRole("button", { name: "Open wiki" }));
    expect(screen.getByTestId("pages-overview")).toBeVisible();
  });

  it("cancels a Settings import back to its settings context", async () => {
    const user = userEvent.setup();
    renderMain();
    await user.click(screen.getByRole("button", { name: "Open settings" }));
    await user.click(screen.getByRole("button", { name: "Settings import" }));
    await user.click(screen.getByRole("button", { name: "Cancel import" }));
    expect(screen.getByTestId("settings-page")).toHaveAttribute("data-section", "general");
    expect(screen.queryByRole("heading", { name: "Memories" })).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Settings back" }));
    expect(screen.getByTestId("pages-overview")).toBeVisible();
  });

  it("keeps the Memories collection free of retired Recaps and duplicate recent-memory navigation", async () => {
    const user = userEvent.setup();
    renderMain();
    await user.click(screen.getByRole("button", { name: "Open memories" }));
    expect(screen.getByRole("heading", { name: "Memories" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Recaps" })).not.toBeInTheDocument();
    expect(screen.queryByTestId("context-browser")).not.toBeInTheDocument();
  });

  it("migrates the legacy collapsed preference to hidden labels", async () => {
    // Given a legacy collapsed value, Main preserves its hidden state in the current two-state model
    localStorage.setItem("wenlan-sidebar-collapsed", "true");
    const user = userEvent.setup();

    // When Main mounts and the user toggles it
    renderMain();
    const sidebar = screen.getByRole("complementary");
    expect(sidebar).toHaveAttribute("data-collapsed", "true");
    expect(sidebar).toHaveAttribute("data-mode", "labels");
    await user.click(screen.getByRole("button", { name: "Toggle sidebar" }));

    // Then the sidebar is hidden without losing the selected icon mode
    expect(sidebar).toHaveAttribute("data-collapsed", "false");
    await waitFor(() => expect(JSON.parse(localStorage.getItem("wenlan-navigation-v1")!).sidebar).toEqual({ visible: true, mode: "labels" }));
  });

  it("keeps the top bar uninterrupted when the sidebar toggles", async () => {
    const user = userEvent.setup();
    renderMain();

    expect(document.querySelector('[data-sidebar-header-divider="true"]')).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Toggle sidebar" }));
    expect(document.querySelector('[data-sidebar-header-divider="true"]')).not.toBeInTheDocument();
  });

  it("routes Spaces and create intent to one overview with focused creation", async () => {
    // Given the Wiki view
    const user = userEvent.setup();
    renderMain();

    // When the sidebar add action is used
    await user.click(screen.getByRole("button", { name: "Create space" }));

    // Then the shared overview opens and focuses its create form
    expect(screen.getByTestId("spaces-overview")).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "Space name" })).toHaveFocus();
    expect(screen.getByRole("complementary")).toHaveAttribute("data-active", "spaces");
  });

  it("uses parent replacement for Spaces and Escape for pushed history", async () => {
    // Given a recent Space opened from Wiki
    const user = userEvent.setup();
    renderMain();
    await openManagedSpace(user);

    // When Escape is pressed
    fireEvent.keyDown(window, { key: "Escape" });

    // Then pushed history returns the Spaces overview that launched the detail
    expect(screen.getByTestId("spaces-overview")).toBeInTheDocument();

    // Given the Space is opened again and its parent is used
    await openManagedSpace(user);
    await user.click(screen.getByRole("button", { name: "Space parent" }));

    // Then the parent always replaces with the Spaces overview
    expect(screen.getByTestId("spaces-overview")).toBeInTheDocument();
  });

  it("closes a narrow drawer without mutating desktop preference and returns focus", async () => {
    // Given a narrow viewport and a collapsed desktop preference
    localStorage.setItem("wenlan-navigation-v1", JSON.stringify({ version: 1, visible: ["pages", "spaces", "graph"], sidebar: { visible: false, mode: "icons" } }));
    stubResponsiveViewport(true);
    const user = userEvent.setup();
    renderMain();
    const toggle = screen.getByRole("button", { name: "Toggle sidebar" });

    // When the drawer opens and its backdrop closes it
    await user.click(toggle);
    expect(screen.getByRole("complementary")).toHaveAttribute("data-presentation", "overlay");
    expect(screen.getByRole("complementary")).toHaveAttribute("data-open", "true");
    await user.click(screen.getByRole("button", { name: "Close sidebar" }));

    // Then the desktop preference is untouched and focus returns to the toggle
    expect(JSON.parse(localStorage.getItem("wenlan-navigation-v1")!).sidebar).toEqual({ visible: false, mode: "icons" });
    expect(toggle).toHaveFocus();
  });

  it("uses the first narrow Escape only for the drawer and the next Escape for Space history", async () => {
    // Given a Space opened from Wiki with the 899px drawer open
    stubResponsiveViewport(true);
    const user = userEvent.setup();
    renderMain();
    await openManagedSpace(user);
    const toggle = screen.getByRole("button", { name: "Toggle sidebar" });
    await user.click(toggle);
    expect(screen.getByRole("complementary")).toHaveAttribute("data-open", "true");

    // When Escape is pressed once
    fireEvent.keyDown(window, { key: "Escape" });

    // Then only the overlay closes and the Space plus its history remain
    expect(screen.getByRole("complementary")).toHaveAttribute("data-open", "false");
    expect(screen.getByTestId("space-detail")).toBeInTheDocument();
    expect(toggle).toHaveFocus();

    // When Escape is pressed again after the drawer has closed
    fireEvent.keyDown(window, { key: "Escape" });

    // Then the pushed history returns the Spaces overview that launched the detail
    expect(screen.getByTestId("spaces-overview")).toBeInTheDocument();
  });

  it("returns focus when 899px overlay content becomes a collapsed 900px desktop sidebar", async () => {
    // Given a persisted collapsed desktop sidebar with overlay focus at 899px
    localStorage.setItem("wenlan-navigation-v1", JSON.stringify({ version: 1, visible: ["pages", "spaces", "graph"], sidebar: { visible: false, mode: "icons" } }));
    const viewport = stubResponsiveViewport(true);
    const user = userEvent.setup();
    renderMain();
    const toggle = screen.getByRole("button", { name: "Toggle sidebar" });
    await user.click(toggle);
    const drawerDestination = screen.getByRole("button", { name: "Open spaces" });
    drawerDestination.focus();
    expect(drawerDestination).toHaveFocus();

    // When the viewport crosses from overlay to desktop at 900px
    act(() => viewport.setMatches(false));

    // Then focus leaves the aria-hidden sidebar without changing the desktop preference
    expect(screen.getByRole("complementary")).toHaveAttribute("data-presentation", "desktop");
    expect(screen.getByRole("complementary")).toHaveAttribute("data-open", "false");
    expect(toggle).toHaveFocus();
    expect(JSON.parse(localStorage.getItem("wenlan-navigation-v1")!).sidebar).toEqual({ visible: false, mode: "icons" });
  });

  it("prunes a missing MRU id on the next unrelated successful Space visit", async () => {
    // Given valid MRU entries for one current and one deleted Space
    const current = space("space-1", "Work");
    listSpacesMock.mockResolvedValue([current]);
    localStorage.setItem(RECENT_SPACES_STORAGE_KEY, JSON.stringify({
      version: 1,
      entries: [
        { id: "missing", name: "Deleted elsewhere", visitedAt: Date.now() - 2 },
        { id: current.id, name: current.name, visitedAt: Date.now() - 3 },
      ],
    }));
    const user = userEvent.setup();
    renderMain();
    await openManagedSpace(user);

    // When an unrelated current Space finishes loading and records its visit
    await user.click(screen.getByRole("button", { name: "Finish loading space" }));

    // Then the successful write reconciles history against the current inventory
    const stored: unknown = JSON.parse(localStorage.getItem(RECENT_SPACES_STORAGE_KEY) ?? "null");
    expect(stored).toEqual({
      version: 1,
      entries: [expect.objectContaining({ id: current.id, name: current.name })],
    });
  });

  it("records a loaded Space when its inventory query is still pending", async () => {
    // Given Space detail can load before the shared inventory query resolves
    const current = space("space-1", "Work");
    const other = space("space-2", "Previously visited");
    const inventory = deferred<readonly Space[]>();
    listSpacesMock.mockReturnValue(inventory.promise);
    localStorage.setItem(RECENT_SPACES_STORAGE_KEY, JSON.stringify({
      version: 1,
      entries: [
        { id: other.id, name: other.name, visitedAt: Date.now() - 100 },
      ],
    }));
    const user = userEvent.setup();
    renderMain();
    await openManagedSpace(user);

    // When the selected Space finishes loading first, followed by the inventory
    await user.click(screen.getByRole("button", { name: "Finish loading space" }));
    await act(async () => {
      inventory.resolve([current, other]);
      await inventory.promise;
    });

    // Then the genuine visit persists without adding an unvisited inventory item
    const stored: unknown = JSON.parse(localStorage.getItem(RECENT_SPACES_STORAGE_KEY) ?? "null");
    expect(stored).toEqual({
      version: 1,
      entries: [
        expect.objectContaining({ id: current.id, name: current.name }),
        expect.objectContaining({ id: other.id, name: other.name }),
      ],
    });
  });

  it("preserves unrelated Recent Spaces when rename happens before inventory resolves", async () => {
    const current = space("space-1", "Work");
    const other = space("space-2", "Previously visited");
    const inventory = deferred<readonly Space[]>();
    listSpacesMock.mockReturnValue(inventory.promise);
    localStorage.setItem(RECENT_SPACES_STORAGE_KEY, JSON.stringify({
      version: 1,
      entries: [
        { id: current.id, name: current.name, visitedAt: Date.now() - 50 },
        { id: other.id, name: other.name, visitedAt: Date.now() - 100 },
      ],
    }));
    const user = userEvent.setup();
    renderMain();
    await openManagedSpace(user);

    await user.click(screen.getByRole("button", { name: "Rename loaded space" }));

    const stored: unknown = JSON.parse(localStorage.getItem(RECENT_SPACES_STORAGE_KEY) ?? "null");
    expect(stored).toEqual({
      version: 1,
      entries: [
        expect.objectContaining({ id: current.id, name: "Renamed Work" }),
        expect.objectContaining({ id: other.id, name: other.name }),
      ],
    });
    await act(async () => {
      inventory.resolve([current, other]);
      await inventory.promise;
    });
  });

  it("removes only the deleted Recent Space before inventory resolves", async () => {
    const current = space("space-1", "Work");
    const other = space("space-2", "Previously visited");
    const inventory = deferred<readonly Space[]>();
    listSpacesMock.mockReturnValue(inventory.promise);
    localStorage.setItem(RECENT_SPACES_STORAGE_KEY, JSON.stringify({
      version: 1,
      entries: [
        { id: current.id, name: current.name, visitedAt: Date.now() - 50 },
        { id: other.id, name: other.name, visitedAt: Date.now() - 100 },
      ],
    }));
    const user = userEvent.setup();
    renderMain();
    await openManagedSpace(user);

    await user.click(screen.getByRole("button", { name: "Delete loaded space" }));

    const stored: unknown = JSON.parse(localStorage.getItem(RECENT_SPACES_STORAGE_KEY) ?? "null");
    expect(stored).toEqual({
      version: 1,
      entries: [expect.objectContaining({ id: other.id, name: other.name })],
    });
    await act(async () => {
      inventory.resolve([other]);
      await inventory.promise;
    });
  });

  it("keeps a selected Recent Space active by id when its name changes", async () => {
    // Given a Recent Space selected from its stable production object
    const current = space("space-1", "Work");
    listSpacesMock.mockResolvedValue([current]);
    const user = userEvent.setup();
    renderMain();
    await openManagedSpace(user);
    await user.click(screen.getByRole("button", { name: "Finish loading space" }));
    expect(JSON.parse(localStorage.getItem(RECENT_SPACES_STORAGE_KEY)!).entries[0]).toMatchObject({ id: current.id, name: current.name });

    // When the loaded Space is renamed under the same id
    await user.click(screen.getByRole("button", { name: "Rename loaded space" }));

    // Then selection remains on that id while the detail route refreshes its name
    expect(JSON.parse(localStorage.getItem(RECENT_SPACES_STORAGE_KEY)!).entries[0]).toMatchObject({ id: current.id, name: "Renamed Work" });
    expect(screen.getByTestId("space-detail")).toHaveAttribute("data-space-name", "Renamed Work");
  });

  it("records a Recent Page only after its detail finishes loading", async () => {
    const user = userEvent.setup();
    renderMain({ initialPageId: "page-1" });

    expect(screen.getByTestId("wiki-workspace")).toHaveAttribute("data-current-page", "page-1");
    expect(localStorage.getItem(RECENT_PAGES_STORAGE_KEY)).toBeNull();
    await user.click(screen.getByRole("button", { name: "Finish loading page" }));

    const stored: unknown = JSON.parse(localStorage.getItem(RECENT_PAGES_STORAGE_KEY) ?? "null");
    expect(stored).toEqual({
      version: 1,
      entries: [expect.objectContaining({ id: "page-1", title: "Visited page" })],
    });
  });

  it("replaces a Wiki draft editor with the published Page while preserving Wiki history", async () => {
    const user = userEvent.setup();
    renderMain();

    await user.click(screen.getByRole("button", { name: "Open wiki" }));
    await user.click(screen.getByRole("button", { name: "Create standalone draft" }));

    expect(screen.getByTestId("page-draft-editor")).toHaveAttribute("data-space", "none");
    expect(screen.getByRole("complementary")).toHaveAttribute("data-active", "pages");
    draftFlushMock.mockResolvedValue(false);
    await user.click(screen.getByRole("button", { name: "Publish draft" }));
    expect(screen.getByTestId("page-detail")).toHaveAttribute("data-page-id", "draft-new");

    await user.click(screen.getByRole("button", { name: "Page back" }));
    expect(screen.getByTestId("pages-overview")).toBeInTheDocument();
  });

  it("saves sidebar New note into a distinct draft without changing the old note", async () => {
    realDraftEditor.enabled = true;
    const saved: Page[] = [];
    vi.mocked(createPageDraft).mockImplementation(async (request) => {
      const draft = draftPage(request.clientDraftId, request.title, request.content);
      saved.push(draft);
      return draft;
    });
    vi.mocked(publishPageDraft).mockImplementation(async (request) => {
      const draft = saved.find((candidate) => candidate.id === request.id);
      if (!draft) throw new Error(`Missing saved draft ${request.id}`);
      return activePage(draft, request.expectedVersion + 1);
    });
    const user = userEvent.setup();
    const { rerenderMain } = renderMain();
    await user.click(screen.getByRole("button", { name: "New context note" }));
    await user.type(await screen.findByRole("textbox", { name: "Title" }), "Original");
    await user.type(screen.getByRole("textbox", { name: "Content" }), "Keep this body");
    rerenderMain();
    expect(screen.getByRole("textbox", { name: "Title" })).toHaveValue("Original");

    await user.click(screen.getByRole("button", { name: "New context note" }));
    await waitFor(() => expect(screen.getByRole("textbox", { name: "Title" })).toHaveValue(""));
    expect(saved).toHaveLength(1);
    await user.type(screen.getByRole("textbox", { name: "Title" }), "Second");
    await user.type(screen.getByRole("textbox", { name: "Content" }), "Separate body");
    await user.click(screen.getByRole("button", { name: "Open graph" }));
    await screen.findByTestId("atlas-view");

    expect(saved).toHaveLength(2);
    expect(saved[0]).toMatchObject({ title: "Original", content: "Keep this body" });
    expect(saved[1]).toMatchObject({ title: "Second", content: "Separate body" });
    expect(saved[0].id).not.toBe(saved[1].id);
    expect(updatePageDraft).not.toHaveBeenCalled();
    expect(publishPageDraft).toHaveBeenCalledTimes(2);
    expect(publishPageDraft).toHaveBeenNthCalledWith(1, {
      id: saved[0]!.id,
      expectedVersion: saved[0]!.version,
    });
    expect(publishPageDraft).toHaveBeenNthCalledWith(2, {
      id: saved[1]!.id,
      expectedVersion: saved[1]!.version,
    });
  });

  it("starts a distinct session even when both old and new drafts have no persisted id", async () => {
    realDraftEditor.enabled = true;
    const user = userEvent.setup();
    renderMain();
    await user.click(screen.getByRole("button", { name: "New context note" }));
    const oldTitle = await screen.findByRole("textbox", { name: "Title" });
    await user.click(screen.getByRole("button", { name: "New context note" }));
    await waitFor(() => expect(screen.getByRole("textbox", { name: "Title" })).not.toBe(oldTitle));
    expect(createPageDraft).not.toHaveBeenCalled();
  });

  it("keeps the autosaved note mounted while typing, then finalizes it once on quit", async () => {
    realDraftEditor.enabled = true;
    const saved = new Map<string, Page>();
    let published: Page | null = null;
    vi.mocked(createPageDraft).mockImplementation(async (request) => {
      const draft = draftPage(request.clientDraftId, request.title, request.content);
      saved.set(draft.id, draft);
      return draft;
    });
    vi.mocked(updatePageDraft).mockImplementation(async (request) => {
      const draft = { ...draftPage(request.id, request.title, request.content), version: request.expectedVersion + 1 };
      saved.set(draft.id, draft);
      return draft;
    });
    vi.mocked(publishPageDraft).mockImplementation(async (request) => {
      const draft = saved.get(request.id);
      if (!draft) throw new Error(`Missing saved draft ${request.id}`);
      published = activePage(draft, request.expectedVersion + 1);
      return published;
    });
    let quitGuard: (() => Promise<boolean>) | null = null;
    const props = { onRegisterQuitGuard: (guard: (() => Promise<boolean>) | null) => { quitGuard = guard; } };
    const user = userEvent.setup();
    const { rerenderMain } = renderMain(props);
    await user.click(screen.getByRole("button", { name: "New context note" }));
    const title = await screen.findByRole("textbox", { name: "Title" });
    await user.type(title, "Stable note");
    await waitFor(() => expect(createPageDraft).toHaveBeenCalledTimes(1), { timeout: 2_000 });
    rerenderMain(props);
    expect(screen.getByRole("textbox", { name: "Title" })).toBe(title);
    expect(title).toHaveValue("Stable note");
    await user.type(screen.getByRole("textbox", { name: "Content" }), "More content");
    await act(async () => { expect(await quitGuard!()).toBe(true); });
    expect(createPageDraft).toHaveBeenCalledTimes(1);
    expect(updatePageDraft).toHaveBeenCalledWith(expect.objectContaining({
      id: vi.mocked(createPageDraft).mock.calls[0][0].clientDraftId,
      title: "Stable note", content: "More content",
    }));
    const draftId = vi.mocked(createPageDraft).mock.calls[0]![0].clientDraftId;
    expect(publishPageDraft).toHaveBeenCalledTimes(1);
    expect(publishPageDraft).toHaveBeenCalledWith({ id: draftId, expectedVersion: 2 });
    expect(screen.getByTestId("page-detail")).toHaveAttribute("data-page-id", draftId);
    // The canonical Page keeps the draft id; Main's test renderer does not
    // mount the tab strip, so this also proves the same note was promoted.
    expect(published).toMatchObject({
      id: draftId,
      title: "Stable note",
      content: "More content",
      status: "active",
      version: 3,
    });
    expect(saved.get(draftId)).toMatchObject({ title: "Stable note", content: "More content", version: 2 });
  });

  it("remounts an existing draft A to cached draft B instead of retaining A's autosave identity", async () => {
    realDraftEditor.enabled = true;
    const saved = new Map<string, Page>([
      ["draft-a", draftPage("draft-a", "A title", "A body")],
      ["draft-b", draftPage("draft-b", "B title", "B body")],
    ]);
    vi.mocked(getPage).mockImplementation(async (id) => saved.get(id) ?? null);
    vi.mocked(updatePageDraft).mockImplementation(async (request) => {
      const draft = { ...draftPage(request.id, request.title, request.content), version: request.expectedVersion + 1 };
      saved.set(draft.id, draft);
      return draft;
    });
    vi.mocked(publishPageDraft).mockImplementation(async (request) => {
      const draft = saved.get(request.id);
      if (!draft) throw new Error(`Missing saved draft ${request.id}`);
      const page = activePage(draft, request.expectedVersion + 1);
      saved.set(page.id, page);
      return page;
    });
    const user = userEvent.setup();
    const { queryClient } = renderMain();
    queryClient.setQueryData(["page-draft", "draft-b"], draftPage("draft-b", "B title", "B body"));
    await user.click(screen.getByRole("button", { name: "Select draft A" }));
    await waitFor(() => expect(screen.getByRole("textbox", { name: "Title" })).toHaveValue("A title"));
    await user.click(screen.getByRole("button", { name: "Select draft B" }));
    await waitFor(() => expect(screen.getByRole("textbox", { name: "Title" })).toHaveValue("B title"));
    await user.type(screen.getByRole("textbox", { name: "Content" }), " changed");
    await user.click(screen.getByRole("button", { name: "Open graph" }));
    await screen.findByTestId("atlas-view");
    expect(updatePageDraft).toHaveBeenCalledWith(expect.objectContaining({ id: "draft-b", content: "B body changed" }));
    expect(updatePageDraft).not.toHaveBeenCalledWith(expect.objectContaining({ id: "draft-a" }));
  });

  it("keeps the same editor and content when New note cannot flush", async () => {
    realDraftEditor.enabled = true;
    vi.mocked(createPageDraft).mockRejectedValue(new Error("offline"));
    const user = userEvent.setup();
    renderMain();
    await user.click(screen.getByRole("button", { name: "New note" }));
    const oldTitle = await screen.findByRole("textbox", { name: "Title" });
    await user.type(oldTitle, "Unsaved original");
    await user.click(screen.getByRole("button", { name: "New note" }));
    await screen.findByRole("alert");
    expect(screen.getByRole("textbox", { name: "Title" })).toBe(oldTitle);
    expect(oldTitle).toHaveValue("Unsaved original");
  });

  it("waits for an in-flight New note flush before allocating the next editor session", async () => {
    realDraftEditor.enabled = true;
    const saving = deferred<Page>();
    vi.mocked(createPageDraft).mockReturnValue(saving.promise);
    vi.mocked(publishPageDraft).mockResolvedValue(
      activePage(draftPage("draft-original", "Pending original", ""), 2),
    );
    const user = userEvent.setup();
    renderMain();
    await user.click(screen.getByRole("button", { name: "New note" }));
    const oldTitle = await screen.findByRole("textbox", { name: "Title" });
    await user.type(oldTitle, "Pending original");
    await user.click(screen.getByRole("button", { name: "New note" }));
    await user.click(screen.getByRole("button", { name: "New note" }));
    expect(screen.getByRole("textbox", { name: "Title" })).toBe(oldTitle);
    expect(oldTitle).toHaveValue("Pending original");
    expect(createPageDraft).toHaveBeenCalledTimes(1);
    await act(async () => saving.resolve(draftPage("draft-original", "Pending original", "")));
    await waitFor(() => expect(screen.getByRole("textbox", { name: "Title" })).toHaveValue(""));
    expect(screen.getByRole("textbox", { name: "Title" })).not.toBe(oldTitle);
  });

  it("keeps the editor mounted until a sidebar destination passes the draft flush gate", async () => {
    const flush = deferred<boolean>();
    draftFlushMock.mockReturnValue(flush.promise);
    const user = userEvent.setup();
    renderMain();

    await user.click(screen.getByRole("button", { name: "Open wiki" }));
    await user.click(screen.getByRole("button", { name: "Create standalone draft" }));
    await user.click(screen.getByRole("button", { name: "Open spaces" }));

    expect(screen.getByTestId("page-draft-editor")).toBeInTheDocument();
    expect(draftFlushMock).toHaveBeenCalledTimes(1);

    await act(async () => {
      flush.resolve(true);
      await flush.promise;
    });
    expect(await screen.findByTestId("spaces-overview")).toBeInTheDocument();
  });

  it("registers a quit guard that flushes only while a draft editor is active", async () => {
    const onRegisterQuitGuard = vi.fn();
    const user = userEvent.setup();
    renderMain({ onRegisterQuitGuard });
    const lastRegistration = onRegisterQuitGuard.mock.calls[
      onRegisterQuitGuard.mock.calls.length - 1
    ];
    const guard = lastRegistration?.[0] as () => Promise<boolean>;

    await expect(guard()).resolves.toBe(true);
    expect(draftFlushMock).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: "Open wiki" }));
    await user.click(screen.getByRole("button", { name: "Create standalone draft" }));
    draftFlushMock.mockResolvedValue(false);

    await expect(guard()).resolves.toBe(false);
    expect(draftFlushMock).toHaveBeenCalledTimes(1);
    expect(screen.getByTestId("page-draft-editor")).toBeInTheDocument();
  });

  it("promotes a newly saved draft into history before leaving for Graph", async () => {
    const user = userEvent.setup();
    renderMain();

    await user.click(screen.getByRole("button", { name: "Open wiki" }));
    await user.click(screen.getByRole("button", { name: "Create standalone draft" }));
    await user.click(screen.getByRole("button", { name: "Open graph" }));

    expect(await screen.findByTestId("atlas-view")).toBeInTheDocument();
    fireEvent.keyDown(window, { key: "Escape" });

    expect(await screen.findByTestId("page-draft-editor")).toHaveAttribute(
      "data-draft-id",
      "draft-new",
    );
  });

  it("flushes a new draft before its first global search query while keeping the editor mounted", async () => {
    const flush = deferred<boolean>();
    draftFlushMock.mockReturnValue(flush.promise);
    useSearchMock.mockImplementation(() => {
      const [query, setLocalQuery] = useState("");
      return {
        query,
        debouncedQuery: query,
        results: [],
        setQuery: ((next: string) => {
          setSearchQueryMock(next);
          setLocalQuery(next);
        }) as typeof setSearchQueryMock,
      };
    });
    const user = userEvent.setup();
    renderMain();

    await user.click(screen.getByRole("button", { name: "Open wiki" }));
    await user.click(screen.getByRole("button", { name: "Create standalone draft" }));
    const originalEditor = screen.getByTestId("page-draft-editor");
    await user.type(
      getSearchInput(),
      "architecture",
    );

    expect(draftFlushMock).toHaveBeenCalledTimes(1);
    expect(screen.getByTestId("page-draft-editor")).toBe(originalEditor);
    await act(async () => {
      flush.resolve(true);
      await flush.promise;
    });

    expect(screen.getByTestId("page-draft-editor")).toBeInTheDocument();
    const search = getSearchInput();
    expect(search).toHaveValue(
      "architecture",
    );
    await user.clear(search);
    expect(await screen.findByTestId("page-draft-editor")).toBe(originalEditor);
    expect(originalEditor).toHaveAttribute("data-draft-id", "new");
    await user.click(screen.getByRole("button", { name: "Open saved draft" }));
    expect(screen.getAllByTestId("page-draft-editor")).toHaveLength(1);
    expect(screen.getByTestId("page-draft-editor")).toBe(originalEditor);
  });

  it("cancels a pending draft search without resurrecting it or promoting the mounted editor", async () => {
    const flush = deferred<boolean>();
    draftFlushMock.mockReturnValue(flush.promise);
    draftIdentityMock.mockReturnValue({
      draftId: "draft-new",
      version: 1,
      publishedPage: activePage(draftPage("draft-new", "Untitled note", ""), 2),
    });
    useSearchMock.mockImplementation(() => {
      const [query, setLocalQuery] = useState("");
      return {
        query,
        debouncedQuery: query,
        results: [],
        setQuery: ((next: string) => {
          setSearchQueryMock(next);
          setLocalQuery(next);
        }) as typeof setSearchQueryMock,
      };
    });
    const user = userEvent.setup();
    renderMain();
    await user.click(screen.getByRole("button", { name: "Open wiki" }));
    await user.click(screen.getByRole("button", { name: "Create standalone draft" }));
    const search = getSearchInput();

    await user.type(search, "architecture");
    await user.clear(search);
    expect(search).toHaveValue("");
    expect(screen.getByTestId("page-draft-editor")).toHaveAttribute("data-draft-id", "new");

    await act(async () => {
      flush.resolve(true);
      await flush.promise;
    });

    expect(search).toHaveValue("");
    expect(screen.queryByTestId("search-results")).not.toBeInTheDocument();
    expect(screen.getByTestId("page-detail")).toHaveAttribute("data-page-id", "draft-new");
    expect(screen.queryByTestId("page-draft-editor")).not.toBeInTheDocument();
  });

  it("keeps a new draft editor visible when its first global search query cannot flush", async () => {
    draftFlushMock.mockResolvedValue(false);
    useSearchMock.mockImplementation(() => {
      const [query, setLocalQuery] = useState("");
      return {
        query,
        debouncedQuery: query,
        results: [],
        setQuery: ((next: string) => {
          setSearchQueryMock(next);
          setLocalQuery(next);
        }) as typeof setSearchQueryMock,
      };
    });
    const user = userEvent.setup();
    renderMain();

    await user.click(screen.getByRole("button", { name: "Open wiki" }));
    await user.click(screen.getByRole("button", { name: "Create standalone draft" }));
    await user.type(
      getSearchInput(),
      "architecture",
    );

    expect(await screen.findByTestId("page-draft-editor")).toBeInTheDocument();
    expect(getSearchInput()).toHaveValue(
      "architecture",
    );
  });

  it("stays in the editor when a sidebar destination cannot save the draft", async () => {
    draftFlushMock.mockResolvedValue(false);
    const user = userEvent.setup();
    renderMain();

    await user.click(screen.getByRole("button", { name: "Open wiki" }));
    await user.click(screen.getByRole("button", { name: "Create standalone draft" }));
    await user.click(screen.getByRole("button", { name: "Open spaces" }));

    expect(screen.getByTestId("page-draft-editor")).toBeInTheDocument();
    expect(screen.queryByTestId("spaces-overview")).not.toBeInTheDocument();
  });

  it("routes a cross-window memory arrival through the draft flush gate", async () => {
    const flush = deferred<boolean>();
    draftFlushMock.mockReturnValue(flush.promise);
    const user = userEvent.setup();
    const view = renderMain();
    await user.click(screen.getByRole("button", { name: "Open wiki" }));
    await user.click(screen.getByRole("button", { name: "Create standalone draft" }));

    view.rerenderMain({ initialMemoryId: "memory-1" });

    expect(screen.getByTestId("page-draft-editor")).toBeInTheDocument();
    await act(async () => {
      flush.resolve(true);
      await flush.promise;
    });
    expect(await screen.findByTestId("memory-detail")).toBeInTheDocument();
  });

  it("keeps the editor when a cross-window memory arrival cannot save the draft", async () => {
    draftFlushMock.mockResolvedValue(false);
    const user = userEvent.setup();
    const view = renderMain();
    await user.click(screen.getByRole("button", { name: "Open wiki" }));
    await user.click(screen.getByRole("button", { name: "Create standalone draft" }));

    view.rerenderMain({ initialMemoryId: "memory-1" });

    expect(await screen.findByTestId("page-draft-editor")).toBeInTheDocument();
    expect(screen.queryByTestId("memory-detail")).not.toBeInTheDocument();
  });

  it.each([
    ["memory", { initialMemoryId: "memory-1" }, "memory-detail"],
    ["page", { initialPageId: "page-1" }, "page-detail"],
  ])("cancels a withdrawn external %s destination while draft flush is pending", async (
    _label,
    externalProps,
    destinationTestId,
  ) => {
    const flush = deferred<boolean>();
    draftFlushMock.mockReturnValue(flush.promise);
    draftIdentityMock.mockReturnValue({
      draftId: "draft-new",
      version: 1,
      publishedPage: activePage(draftPage("draft-new", "Untitled note", ""), 2),
    });
    const user = userEvent.setup();
    const view = renderMain();
    await user.click(screen.getByRole("button", { name: "Open wiki" }));
    await user.click(screen.getByRole("button", { name: "Create standalone draft" }));

    view.rerenderMain(externalProps);
    view.rerenderMain({});
    await act(async () => {
      flush.resolve(true);
      await flush.promise;
    });

    if (destinationTestId === "memory-detail") {
      expect(screen.queryByTestId(destinationTestId)).not.toBeInTheDocument();
    } else {
      expect(screen.getByTestId(destinationTestId)).not.toHaveAttribute("data-page-id", "page-1");
    }
    expect(screen.getByTestId("page-detail")).toHaveAttribute("data-page-id", "draft-new");
    expect(screen.queryByTestId("page-draft-editor")).not.toBeInTheDocument();
  });

  it("clears a discarded resumed draft identity before putting it in navigation history", async () => {
    draftIdentityMock.mockReturnValue({ draftId: null, version: null });
    const user = userEvent.setup();
    renderMain();
    await user.click(screen.getByRole("button", { name: "Open wiki" }));
    await user.click(screen.getByRole("button", { name: "Resume draft" }));

    await user.click(screen.getByRole("button", { name: "Open graph" }));
    fireEvent.keyDown(window, { key: "Escape" });

    expect(await screen.findByTestId("page-draft-editor")).toHaveAttribute(
      "data-draft-id",
      "new",
    );
  });

  it("replaces a resumed draft with an existing conflict destination without pushing history", async () => {
    const user = userEvent.setup();
    renderMain();

    await user.click(screen.getByRole("button", { name: "Open wiki" }));
    await user.click(screen.getByRole("button", { name: "Resume draft" }));

    expect(screen.getByTestId("page-draft-editor")).toHaveAttribute("data-draft-id", "draft-resume");
    await user.click(screen.getByRole("button", { name: "Open existing conflict" }));
    expect(screen.getByTestId("page-detail")).toHaveAttribute("data-page-id", "page-existing");

    await user.click(screen.getByRole("button", { name: "Page back" }));
    expect(screen.getByTestId("pages-overview")).toBeInTheDocument();
  });

  it("awaits the editor-owned Escape gate before returning to the originating Space", async () => {
    const gate = deferred<void>();
    draftRequestBackMock.mockImplementationOnce(async (onBack) => {
      await gate.promise;
      onBack();
    });
    const user = userEvent.setup();
    renderMain();

    await openManagedSpace(user);
    await user.click(screen.getByRole("button", { name: "Create Space draft" }));
    expect(screen.getByTestId("page-draft-editor")).toHaveAttribute("data-space", "Work");
    expect(screen.getByRole("complementary")).toHaveAttribute("data-active", "pages");

    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.getByTestId("page-draft-editor")).toBeInTheDocument();
    await act(async () => {
      gate.resolve();
      await gate.promise;
    });
    expect(screen.getByTestId("space-detail")).toHaveAttribute("data-space-name", "Work");
  });

  it("uses the first narrow Escape only for the drawer while a Page draft is open", async () => {
    stubResponsiveViewport(true);
    const user = userEvent.setup();
    renderMain();
    await user.click(screen.getByRole("button", { name: "Open wiki" }));
    await user.click(screen.getByRole("button", { name: "Create standalone draft" }));
    await user.click(screen.getByRole("button", { name: "Toggle sidebar" }));
    expect(screen.getByRole("complementary")).toHaveAttribute("data-open", "true");

    fireEvent.keyDown(window, { key: "Escape" });

    expect(screen.getByRole("complementary")).toHaveAttribute("data-open", "false");
    expect(screen.getByTestId("page-draft-editor")).toBeInTheDocument();
    expect(draftRequestBackMock).not.toHaveBeenCalled();
  });

  it("keeps the Wenlan brand out of the top chrome", () => {
    renderMain();

    expect(screen.queryByAltText("Wenlan")).toBeNull();
    expect(screen.queryByRole("button", { name: "Go to home" })).toBeNull();
  });

  it("keeps Settings out of the top chrome because the account menu owns it", () => {
    renderMain();

    expect(screen.queryByRole("button", { name: "Settings" })).toBeNull();
  });

  it("labels the global search around pages, memories, and sources", () => {
    renderMain();

    expect(getSearchInput()).toBeInTheDocument();
    expect(useSearchMock).toHaveBeenCalledWith();
    expect(useSearchMock).not.toHaveBeenCalledWith("memory");
  });

  it("offers a compact search action below the desktop breakpoint", async () => {
    const user = userEvent.setup();
    renderMain();

    const searchAction = screen.getByRole("button", { name: "Search" });
    expect(searchAction).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByPlaceholderText("Search notes, memories, sources...")).not.toBeInTheDocument();
    await user.click(searchAction);
    const searchInput = screen.getByPlaceholderText("Search notes, memories, sources...");

    expect(searchAction).toHaveAttribute("aria-expanded", "true");
    expect(searchInput).toHaveFocus();
    fireEvent.keyDown(searchInput, { key: "Escape" });
    expect(searchAction).toHaveAttribute("aria-expanded", "false");
  });

  it("opens a portal dialog, makes the background inert, and restores focus", async () => {
    const user = userEvent.setup();
    renderMain();
    const trigger = screen.getByRole("button", { name: "Search" });
    await user.click(trigger);

    const dialog = screen.getByRole("dialog", { name: "Search" });
    const shell = document.querySelector(".memory-shell");
    const input = screen.getByPlaceholderText("Search notes, memories, sources...");
    expect(shell).toHaveAttribute("inert");
    expect(input).toHaveFocus();

    fireEvent.keyDown(input, { key: "Escape" });

    expect(dialog).not.toBeInTheDocument();
    expect(shell).not.toHaveAttribute("inert");
    expect(trigger).toHaveFocus();
  });

  it("shows loading and search errors without claiming there are no matches", () => {
    useSearchMock.mockReturnValue({
      query: "offline",
      debouncedQuery: "offline",
      setQuery: setSearchQueryMock,
      results: [],
      isLoading: true,
      error: null,
    });
    const firstRender = renderMain();
    getSearchInput();
    expect(screen.getByRole("status")).toHaveTextContent("Searching");
    expect(screen.queryByText(/0 results for/)).not.toBeInTheDocument();

    firstRender.unmount();
    useSearchMock.mockReturnValue({
      query: "offline",
      debouncedQuery: "offline",
      setQuery: setSearchQueryMock,
      results: [],
      isLoading: false,
      error: new Error("offline"),
    });
    renderMain();
    getSearchInput();
    expect(screen.getByRole("alert")).toHaveTextContent("could not be loaded");
    expect(screen.queryByText(/0 results for/)).not.toBeInTheDocument();
  });

  it("shows recent pages in an empty search and opens the selected page", async () => {
    localStorage.setItem(RECENT_PAGES_STORAGE_KEY, JSON.stringify({
      version: 1,
      entries: [{ id: "page-recent", title: "Research notes", visitedAt: Date.now() }],
    }));
    const user = userEvent.setup();
    renderMain();
    await user.click(screen.getByRole("button", { name: "Search" }));

    expect(await screen.findByText("Recently opened")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Open Note: Research notes" }));
    expect(await screen.findByTestId("page-detail")).toBeInTheDocument();
    expect(screen.queryByRole("dialog", { name: "Search" })).not.toBeInTheDocument();
  });

  it("opens URL-backed source search results through the file bridge", async () => {
    useSearchMock.mockReturnValue({
      query: "source",
      setQuery: setSearchQueryMock,
      debouncedQuery: "source",
      results: [{
        id: "source-hit",
        content: "Source credibility notes",
        source: "local_files",
        source_id: "source-1",
        title: "source.md",
        url: "/tmp/source.md",
        chunk_index: 0,
        last_modified: 0,
        score: 0.9,
      }],
    });
    const user = userEvent.setup();
    renderMain();

    await user.click(screen.getByRole("button", { name: "Search" }));
    await user.click(await screen.findByRole("button", { name: "Source credibility notes" }));

    expect(openSearchResultMock).toHaveBeenCalledWith("/tmp/source.md");
  });

  it("passes a file:// source url through to the file bridge intact", async () => {
    useSearchMock.mockReturnValue({
      query: "source",
      setQuery: setSearchQueryMock,
      debouncedQuery: "source",
      results: [{
        id: "source-hit",
        content: "Source credibility notes",
        source: "local_files",
        source_id: "source-1",
        title: "source.md",
        url: "file:///tmp/source.md",
        chunk_index: 0,
        last_modified: 0,
        score: 0.9,
      }],
    });
    const user = userEvent.setup();
    renderMain();

    await user.click(screen.getByRole("button", { name: "Search" }));
    await user.click(await screen.findByRole("button", { name: "Source credibility notes" }));

    expect(openSearchResultMock).toHaveBeenCalledWith("file:///tmp/source.md");
  });

  it("uses one Activity action instead of a Home and Activity segmented control", async () => {
    const user = userEvent.setup();
    renderMain();

    expect(screen.queryByRole("button", { name: "Home" })).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Activity" }));
    await user.click(screen.getByTestId("activity-summary-open"));

    expect(await screen.findByTestId("activity-feed")).toBeInTheDocument();
  });

  it("routes the former profile sentinel to General settings instead of the old profile page", async () => {
    const user = userEvent.setup();
    renderMain();

    await user.click(screen.getByRole("button", { name: "Open avatar menu destination" }));

    expect(await screen.findByTestId("settings-page")).toBeInTheDocument();
    expect(screen.queryByTestId("profile-page")).not.toBeInTheDocument();
  });

  it("keeps the compact Review marker visible when Settings or Connect Agent replaces the standard sidebar", async () => {
    const user = userEvent.setup();
    renderMain();

    expect(screen.queryByTestId("review-environment-badge")).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Open avatar menu destination" }));
    expect(await screen.findByTestId("settings-page")).toBeInTheDocument();
    expect(screen.getByTestId("review-environment-badge")).toHaveAttribute("data-compact", "true");

    await user.click(screen.getByRole("button", { name: "Connect agent" }));
    expect(screen.getByTestId("review-environment-badge")).toHaveAttribute("data-compact", "true");
  });

  it("opens and focuses responsive search from the Tauri event when the placeholder is translated", async () => {
    await i18n.changeLanguage("zh-Hant");
    renderMain();

    const searchInput = getSearchInput();
    const searchAction = screen.getByRole("button", { name: "搜尋" });
    act(() => eventListeners.get("focus-search")?.());

    expect(searchAction).toHaveAttribute("aria-expanded", "true");
    expect(searchInput).toHaveFocus();
  });

  it("renders AtlasView as the Graph view — back returns Wiki, node clicks open the entity", async () => {
    const user = userEvent.setup();
    renderMain();

    await user.click(screen.getByRole("button", { name: "Open graph" }));
    expect(screen.getByTestId("atlas-view")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Atlas back" }));
    expect(screen.getByTestId("pages-overview")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Open graph" }));
    await user.click(screen.getByRole("button", { name: "Atlas node" }));
    expect(screen.getByTestId("entity-detail")).toBeInTheDocument();
  });

  it("opens the memory, not an entity, when a memory node is clicked in the Graph view", async () => {
    const user = userEvent.setup();
    renderMain();

    await user.click(screen.getByRole("button", { name: "Open graph" }));
    await user.click(screen.getByRole("button", { name: "Atlas memory node" }));

    expect(await screen.findByTestId("memory-detail")).toBeInTheDocument();
    expect(screen.queryByTestId("entity-detail")).not.toBeInTheDocument();
  });

  it("opens memory detail when initialMemoryId arrives after mount", async () => {
    const view = renderMain();

    expect(screen.getByTestId("pages-overview")).toBeInTheDocument();

    view.rerenderMain({ initialMemoryId: "memory-1" });

    expect(await screen.findByTestId("memory-detail")).toBeInTheDocument();
  });
});
