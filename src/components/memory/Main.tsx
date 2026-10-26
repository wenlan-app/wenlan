// SPDX-License-Identifier: AGPL-3.0-only
import { createPortal } from "react-dom";
import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { SearchModal } from "./SearchModal";
import { RecentlyOpened, type RecentSearchItem } from "./RecentlyOpened";
import { ArrowLeft, ArrowRight, X } from "@phosphor-icons/react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import type { TrayPanelTarget } from "../TrayPanel";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import {
  listMemoriesRich,
  listSpaces,
  openSearchResult as openSearchResultTarget,
  searchEntities,
  searchPages,
  takeRemotePairingLink,
  type Page,
  type SearchResult,
  type Space,
} from "../../lib/tauri";
import { reloadWindow } from "../../lib/reloadWindow";
import { setPendingPairingCode } from "../../lib/pairingLink";
import { confirmAction } from "../../lib/confirmAction";
import { MAIN_HEADER_HEIGHT, topBarLeftInset } from "../../lib/windowChrome";
import ActivityFeed from "./ActivityFeed";
import { useSearch } from "../../hooks/useSearch";
import EntityDetail from "./EntityDetail";
import MemoryStream from "./MemoryStream";
import type { SortMode } from "./MemoryStream";
import { FirstUseGuide } from "../onboarding/FirstUseGuide";
import { useGettingStartedState } from "../onboarding/useGettingStartedState";
import { readRememberedDirectoryHidden, rememberDirectoryHidden } from "../../lib/wikiLayoutMemory";
import AtlasView from "./AtlasView";
import { SearchResults } from "./SearchResults";
import MemoryDetail from "./MemoryDetail";
import PageDetail from "./PageDetail";
import DistillReviewPanel from "./DistillReviewPanel";
import SettingsPage from "./SettingsPage";
import { ImportView } from "./ImportView";
import { OrganizeActionsContext, type OrganizeActions } from "./askAi/TurnIntoPages";
import { SetupWizard } from "../SetupWizard";
import Sidebar, { SidebarToggleButton } from "./Sidebar";
import SettingsSidebar from "./settings/SettingsSidebar";
import SpaceDetail from "./SpaceDetail";
import { SpacesOverview } from "./spaces";
import { PagesOverview } from "./pages/PagesOverview";
import { BASIS_UPDATES_REVIEW_ID } from "./page/useBasisActions";
import { WikiWorkspace } from "./pages/WikiWorkspace";
import { useLibraryInventory } from "./pages/useLibraryInventory";
import { SecondaryNoteGroup, type SecondaryNoteGroupHandle } from "./pages/SecondaryNoteGroup";
import { NoteTabs } from "./pages/NoteTabs";
import { isNoteView, matchesNote, noteId, noteReadingKey, noteReadingPositionsFor, promoteNoteTab, reorderNoteTab, upsertNoteTab, type NoteTab, type NoteView } from "./pages/noteTabState";
import { EntitiesView } from "./entities/EntitiesView";
import {
  PageDraftEditor,
  type PageDraftEditorHandle,
} from "./pages/PageDraftEditor";
import SourcesView from "./SourcesView";
import { SourceDocumentPage } from "./sources/SourceDocumentReader";
import { WorkspaceReadingHostContext } from "./navigation/WorkspacePaneHost";
import type { SourceLibraryState } from "./sources/SourceLibrary";
import AboutWenlanDialog from "./AboutWenlanDialog";
import PairingApprovalDialog from "./PairingApprovalDialog";
import RemoteAccessNotifier from "./RemoteAccessNotifier";
import { searchResultTarget } from "../../lib/searchResultTarget";
import { readRecentPageHistory, recordRecentPageVisit } from "../../lib/recentPages";
import { readWikiInventoryMode, writeWikiInventoryMode, type WikiInventoryMode } from "../../lib/wikiNotesPreferences";
import { deleteRecentSpace, readRecentSpaceHistory, recordRecentSpaceVisit, renameRecentSpace } from "../../lib/recentSpaces";
import { createSpaceDetailCopy, createSpacesOverviewLabels } from "./navigation/copy";
import { activeNavigationForView, type View } from "./navigation/viewState";
import { ReviewEnvironmentBadge } from "./navigation/ReviewEnvironmentBadge";
import QuickCaptureScrim from "./QuickCaptureScrim";
import { useResponsiveSidebar } from "./navigation/useResponsiveSidebar";
import { SidebarResizeHandle } from "./navigation/SidebarResizeHandle";
import { useAutoCompactSidebar } from "./navigation/useAutoCompactSidebar";
import { WorkspaceDemandContext } from "./navigation/workspaceDemand";
import {
  readSidebarPreferences,
  writeSidebarPreferences,
  type SidebarPreferences,
} from "./navigation/navigationPreferences";
import { ContextBrowser } from "./navigation/ContextBrowser";
import { useLaunchPinFill } from "../../lib/launchPinFill";
import type { MarkdownEditorSelection } from "./editor/MarkdownEditor";
import { NoteTabDragProvider } from "./pages/NoteTabDragProvider";
import type { NoteTabDrop } from "./pages/noteTabDragContext";
import { useViewScroll } from "./navigation/useViewScroll";
import { WorkspaceBackButton, WorkspaceNavigationProvider } from "./navigation/WorkspaceNavigation";
import { WorkspacePaneHostContext, WorkspaceDocumentToolsHostContext } from "./navigation/WorkspacePaneHost";
import { ReferenceNavigationProvider } from "./links/ReferenceNavigationContext";
import "./navigation/navigation-shell.css";
import "./assets/collectionToolbar.css";
import { inventoryFolderPath } from "./pages/pageInventory";

interface MainProps {
  initialView?: View;
  initialMemoryId?: string | null;
  initialPageId?: string | null;
  onBackFromDetail?: () => void;
  onRegisterQuitGuard?: (guard: (() => Promise<boolean>) | null) => void;
}
function scrollDestinationKey(view: View): string {
  switch (view.kind) {
    case "graph":
      return `graph:${view.focusPageId ?? "all"}`;
    case "source-document":
      return JSON.stringify(["source-document", view.file.source, view.file.source_id, view.file.space ?? null]);
    case "entity":
      return `entity:${view.entityId}`;
    case "memory":
      return `memory:${view.sourceId}`;
    case "page":
      return `page:${view.pageId}`;
    case "page-draft":
      return noteReadingKey(view);
    case "settings":
      return `settings:${view.section ?? "general"}`;
    case "space":
      return `space:${view.spaceId ?? view.spaceName}`;
    default:
      return view.kind;
  }
}

// Compare destinations rather than object identity. A fresh unsaved draft is
// always a new intent; persisted drafts retain their session through history.
function sameDestination(current: View, next: View): boolean {
  if ((current.kind === "home" || current.kind === "pages")
    && (next.kind === "home" || next.kind === "pages")) {
    return (current.kind === "pages" ? current.inventoryScope ?? "all" : "all")
      === (next.kind === "pages" ? next.inventoryScope ?? "all" : "all");
  }
  if (current.kind !== next.kind) return false;
  switch (current.kind) {
    case "page": return next.kind === "page" && current.pageId === next.pageId
      && (current.mode ?? "edit") === (next.mode ?? "edit");
    case "graph": return next.kind === "graph" && current.focusPageId === next.focusPageId;
    case "source-document": return next.kind === "source-document" && current.file.source === next.file.source && current.file.source_id === next.file.source_id && (current.file.space ?? null) === (next.file.space ?? null);
    case "memory": return next.kind === "memory" && current.sourceId === next.sourceId;
    case "entity": return next.kind === "entity" && current.entityId === next.entityId;
    case "settings": return next.kind === "settings"
      && (current.section ?? "general") === (next.section ?? "general");
    case "space": return next.kind === "space" && (current.spaceId && next.spaceId
      ? current.spaceId === next.spaceId : current.spaceName === next.spaceName);
    case "spaces": return next.kind === "spaces" && !!current.create === !!next.create;
    case "import": return next.kind === "import" && !!current.fromFirstUse === !!next.fromFirstUse;
    case "first-use": return next.kind === "first-use"
      && !!current.showKnowledge === !!next.showKnowledge && current.batchId === next.batchId;
    case "page-draft": return next.kind === "page-draft" && current.space === next.space
      && (next.sessionKey != null ? current.sessionKey === next.sessionKey
        : !!next.draftId && current.draftId === next.draftId);
    default: return true;
  }
}

function navigationSettlement(callback?: () => void) {
  let settled = false;
  return {
    succeed: (action: () => void) => {
      if (settled) return;
      settled = true;
      action();
    },
    refuse: () => {
      if (settled) return;
      settled = true;
      callback?.();
    },
  };
}

export default function Main({
  initialView,
  initialMemoryId,
  initialPageId,
  onBackFromDetail,
  onRegisterQuitGuard,
}: MainProps) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  // Give the unpinned routing jobs a source once per launch. Main is the first
  // screen a set-up user reaches, and the only fills before this one lived in
  // the webview, so a quit during the model download left both jobs blank
  // forever. See src/lib/launchPinFill.ts.
  useLaunchPinFill();
  const mainContentRef = useRef<HTMLElement>(null);
  const pageDraftEditorRef = useRef<PageDraftEditorHandle>(null);
  const nextDraftSessionRef = useRef(0);
  const secondaryGroupRef = useRef<SecondaryNoteGroupHandle>(null);
  const [secondaryOpen, setSecondaryOpen] = useState(false);
  const [groupToolsHost, setGroupToolsHost] = useState<HTMLDivElement | null>(null);
  const [secondaryHost, setSecondaryHost] = useState<HTMLDivElement | null>(null);
  const focusedGroup = useRef<"primary" | "secondary">("primary");
  const [ownershipNotice, setOwnershipNotice] = useState<{ side: "primary" | "secondary"; note: NoteView } | null>(null);
  const [movingNote, setMovingNote] = useState(false);
  const movingNoteRef = useRef(false);
  const focusPrimaryGroup = useCallback(() => { focusedGroup.current = "primary"; }, []);
  const focusSecondaryGroup = useCallback(() => { focusedGroup.current = "secondary"; }, []);
  const setSecondaryPresence = useCallback((present: boolean) => {
    setSecondaryOpen(present);
    if (!present) focusedGroup.current = "primary";
  }, []);
  const pendingDraftNavigationRef = useRef<{
    readonly action: (sourceView: View) => void;
    readonly refuse: () => void;
    readonly token: symbol;
  } | null>(null);
  const draftNavigationFlushRef = useRef<Promise<boolean> | null>(null);
  const pendingDraftSearchCancelRef = useRef<(() => void) | null>(null);
  const searchInputRef = useRef<HTMLInputElement>(null);
  const searchReturnFocusRef = useRef<HTMLElement | null>(null);
  const searchFallbackFocusRef = useRef<HTMLElement | null>(null);
  const searchHeaderButtonRef = useRef<HTMLButtonElement>(null);
  const selectSearchOnOpenRef = useRef(false);
  const sidebarToggleRef = useRef<HTMLButtonElement>(null);
  const initialPageRequestRef = useRef<{
    observed: string | null;
    pending: string | null;
    inFlight: symbol | null;
  }>({ observed: initialPageId ?? null, pending: null, inFlight: null });
  const initialMemoryRequestRef = useRef<{
    observed: string | null;
    active: string | null;
    pending: { memoryId: string | null } | null;
    inFlight: symbol | null;
  }>({
    observed: initialMemoryId ?? null,
    active: initialMemoryId ?? null,
    pending: null,
    inFlight: null,
  });
  const pageFlushRef = useRef<(() => Promise<boolean>) | null>(null);
  const memoryFlushRef = useRef<(() => Promise<boolean>) | null>(null);
  const registerMemoryFlush = useCallback((flush: (() => Promise<boolean>) | null) => {
    memoryFlushRef.current = flush;
  }, []);
  const topicFlushRef = useRef<(() => Promise<boolean>) | null>(null);
  const registerTopicFlush = useCallback((flush: (() => Promise<boolean>) | null) => {
    topicFlushRef.current = flush;
  }, []);
  const spaceIdentityFlushRef = useRef<(() => Promise<boolean>) | null>(null);
  const registerSpaceIdentityFlush = useCallback((flush: (() => Promise<boolean>) | null) => {
    spaceIdentityFlushRef.current = flush;
  }, []);
  const sourceDescriptionFlushRef = useRef<(() => Promise<boolean>) | null>(null);
  const registerSourceDescriptionFlush = useCallback((flush: (() => Promise<boolean>) | null) => {
    sourceDescriptionFlushRef.current = flush;
  }, []);
  const pageNavigationTokenRef = useRef<symbol | null>(null);
  const deletedPageIdsRef = useRef(new Set<string>());
  const registerPageFlush = useCallback((flush: (() => Promise<boolean>) | null) => {
    pageFlushRef.current = flush;
    // A remembered View object may be reused by Back/Notes. Readiness belongs
    // to the mounted editor, so a departed editor cannot unlock restoration.
    if (!flush) setReadyEditorView(null);
  }, []);
  const pageSavePendingRef = useRef(false);
  const pageEditDirtyRef = useRef(false);
  const [view, setView] = useState<View>(
    initialMemoryId ? { kind: "memory", sourceId: initialMemoryId }
    : initialPageId ? { kind: "page", pageId: initialPageId }
    : initialView ?? { kind: "pages" },
  );
  const [noteTabs, setNoteTabs] = useState<NoteTab[]>(() => isNoteView(view) ? upsertNoteTab([], view) : []);
  const noteTabsRef = useRef(noteTabs);
  const primaryDraftIdentityByTab = useRef(new Map<string, string>());
  const [wikiCreationFolderPath, setWikiCreationFolderPath] = useState<string | null>(null);
  const [wikiInventoryMode, setWikiInventoryMode] = useState<WikiInventoryMode>(readWikiInventoryMode);
  const [recentPageRevision, setRecentPageRevision] = useState(0);
  const updateNoteTabs = (next: NoteTab[]) => { noteTabsRef.current = next; setNoteTabs(next); };
  const findPrimaryNoteTab = (next: NoteView) => noteTabsRef.current.find(tab =>
    matchesNote(tab, next)
    || (next.kind === "page-draft" && !!next.draftId && primaryDraftIdentityByTab.current.get(tab.key) === next.draftId)
  );
  const viewRef = useRef(view);
  viewRef.current = view;
  const primaryDraftTabKey = view.kind === "page-draft"
    ? noteTabsRef.current.find(item => matchesNote(item, view))?.key
    : undefined;
  const reportPrimaryDraftIdentity = useCallback((draftId: string) => {
    if (primaryDraftTabKey && noteTabsRef.current.some(item => item.key === primaryDraftTabKey)) {
      primaryDraftIdentityByTab.current.set(primaryDraftTabKey, draftId);
    }
  }, [primaryDraftTabKey]);
  const [viewHistory, setViewHistory] = useState<View[]>([]);
  const viewHistoryRef = useRef<View[]>([]);
  const [viewForward, setViewForward] = useState<View[]>([]);
  const viewForwardRef = useRef<View[]>([]);
  const updateHistory = (past: View[], forward: View[]) => {
    viewHistoryRef.current = past;
    viewForwardRef.current = forward;
    setViewHistory(past);
    setViewForward(forward);
  };
  const dismissSearch = () => {
    pendingDraftSearchCancelRef.current?.();
    pendingDraftSearchCancelRef.current = null;
    setPendingDraftSearchQuery(null);
    if (query) setQuery("");
    setMobileSearchOpen(false);
  };
  const openSearch = (trigger?: HTMLElement | null) => {
    if (pageSavePendingRef.current) return;
    const activeElement = document.activeElement;
    searchReturnFocusRef.current = trigger ?? (
      activeElement instanceof HTMLElement && activeElement !== document.body
        ? activeElement
        : searchFallbackFocusRef.current
    );
    if (responsiveSidebar.presentation === "overlay" && responsiveSidebar.open) responsiveSidebar.close();
    setMobileSearchOpen(true);
  };
  const openSearchRef = useRef(openSearch);
  openSearchRef.current = openSearch;
  const showView = (next: View, dismissSearchOverlay = true) => {
    setOwnershipNotice(null);
    if (dismissSearchOverlay) dismissSearch();
    if (isNoteView(next) && secondaryGroupRef.current?.contains(next)) {
      void secondaryGroupRef.current.open(next);
      if (isNoteView(viewRef.current) || viewRef.current.kind === "pages" || viewRef.current.kind === "home") return;
      next = { kind: "pages" };
    }
    if (isNoteView(next)) {
      const existing = findPrimaryNoteTab(next);
      if (existing) next = existing.view;
      else updateNoteTabs(upsertNoteTab(noteTabsRef.current, next));
    }
    const currentView = viewRef.current;
    if (currentView.kind === "sources" || currentView.kind === "source-document") {
      readingPositions.set(scrollDestinationKey(currentView), mainContentRef.current?.scrollTop ?? 0);
    }
    viewRef.current = next;
    setView(next);
    setReadySourceView(null);
    if (next.kind === "pages" || next.kind === "home") setActiveTab("pages");
    else if (next.kind === "activity") setActiveTab("activity");
  };
  // Identity promotion and completed workflows replace the current entry.
  // Their abandoned forward branch must not restore a superseded editor.
  const replaceView = (next: View) => {
    const source = viewRef.current;
    if (source.kind === "page-draft" && next.kind === "page") {
      updateNoteTabs(promoteNoteTab(noteTabsRef.current, source, next));
    }
    updateHistory(viewHistoryRef.current, []);
    showView(next);
  };
  const lastNoteViewRef = useRef<View | null>(null);
  if (view.kind === "page" || view.kind === "page-draft") lastNoteViewRef.current = view;
  const pageSelectionsRef = useRef(new Map<string, MarkdownEditorSelection>());
  const readingPositions = noteReadingPositionsFor(pageSelectionsRef.current);
  const [readyEditorView, setReadyEditorView] = useState<View | null>(null);
  const [readySourceView, setReadySourceView] = useState<View | null>(null);
  const [sourceLibraryState, setSourceLibraryState] = useState<SourceLibraryState>({ search: "", filter: "all" });
  const contextSpace = viewHistory.slice().reverse().find(
    (item): item is Extract<View, { kind: "space" }> => item.kind === "space",
  );
  const [activeTab, setActiveTab] = useState<"pages" | "activity">(view.kind === "activity" ? "activity" : "pages");
  const [aboutOpen, setAboutOpen] = useState(false);
  const [pageSavePending, setPageSavePending] = useState(false);
  const [pageEditDirty, setPageEditDirty] = useState(false);
  const reportPageDirty = useCallback((value: boolean) => { pageEditDirtyRef.current = value; setPageEditDirty(value); }, []);
  const reportPageSaving = useCallback((value: boolean) => { pageSavePendingRef.current = value; setPageSavePending(value); }, []);
  const [mobileSearchOpen, setMobileSearchOpen] = useState(false);
  const searchDialogOpenRef = useRef(mobileSearchOpen);
  searchDialogOpenRef.current = mobileSearchOpen;
  const [recentSearchItems, setRecentSearchItems] = useState<RecentSearchItem[]>([]);
  const [documentToolsHost, setDocumentToolsHost] = useState<HTMLDivElement | null>(null);
  const [workspacePaneHost, setWorkspacePaneHost] = useState<HTMLDivElement | null>(null);
  const setWorkspacePaneHostRef = useCallback((node: HTMLDivElement | null) => setWorkspacePaneHost(node), []);
  const [pendingDraftSearchQuery, setPendingDraftSearchQuery] = useState<string | null>(null);
  const [pageInventoryFilter, setPageInventoryFilter] = useState("");
  const viewScrollDestination = scrollDestinationKey(view);
  pageSavePendingRef.current = pageSavePending;
  pageEditDirtyRef.current = pageEditDirty;

  // Calls onLeave, or onStay, once. An unsaved page draft asks first; the
  // returned cancel stays on the page if the question is still open.
  const leaveCurrentPage = (onLeave: () => void, onStay: () => void): (() => void) => {
    if (pageSavePending) { onStay(); return () => {}; }
    if (view.kind !== "page" || !pageEditDirty) {
      if (pageEditDirty) setPageEditDirty(false);
      onLeave();
      return () => {};
    }
    const sourceView = view;
    let settled = false;
    void confirmAction(t("pageDetail.editor.discardConfirm"), {
      confirm: t("pageDetail.editor.discard"),
      cancel: t("common.cancel"),
    }).then((discard) => {
      if (settled) return;
      settled = true;
      if (!discard || viewRef.current !== sourceView) { onStay(); return; }
      setPageEditDirty(false);
      onLeave();
    });
    return () => {
      if (settled) return;
      settled = true;
      onStay();
    };
  };

  const prepareForQuit = useCallback(async (): Promise<boolean> => {
    if (movingNoteRef.current || (secondaryGroupRef.current && !await secondaryGroupRef.current.flush())) return false;
    if (viewRef.current.kind === "memory" && memoryFlushRef.current) {
      try { return await memoryFlushRef.current(); } catch { return false; }
    }
    if (viewRef.current.kind === "entity" && topicFlushRef.current) {
      try { return await topicFlushRef.current(); } catch { return false; }
    }
    if (viewRef.current.kind === "space" && spaceIdentityFlushRef.current) {
      try { return await spaceIdentityFlushRef.current(); } catch { return false; }
    }
    if ((viewRef.current.kind === "source-document" || viewRef.current.kind === "sources") && sourceDescriptionFlushRef.current) {
      try { return await sourceDescriptionFlushRef.current(); } catch { return false; }
    }
    if (viewRef.current.kind === "page" && pageFlushRef.current) {
      try { return await pageFlushRef.current(); } catch { return false; }
    }
    if (pageSavePendingRef.current || pageEditDirtyRef.current) return false;
    const editor = pageDraftEditorRef.current;
    if (viewRef.current.kind !== "page-draft" || !editor) return true;
    const source = viewRef.current;
    const saved = await editor.flush();
    if (saved && source.kind === "page-draft" && viewRef.current === source) {
      const identity = editor.getIdentity();
      if (identity.publishedPage) {
        const next: View = { kind: "page", pageId: identity.publishedPage.id, inventoryScope: source.inventoryScope, projectionIssue: identity.projectionIssue };
        updateNoteTabs(promoteNoteTab(noteTabsRef.current, source, next).map(tab => matchesNote(tab, next) ? { ...tab, title: identity.publishedPage!.title } : tab));
        lastNoteViewRef.current = next;
        showView(next, false);
      }
    }
    return saved;
  }, []);

  useEffect(() => {
    onRegisterQuitGuard?.(prepareForQuit);
    return () => onRegisterQuitGuard?.(null);
  }, [onRegisterQuitGuard, prepareForQuit]);


  const afterPageDraftFlush = (action: (sourceView: View) => void, onRefused?: () => void): (() => void) => {
    const settlement = navigationSettlement(onRefused);
    const editor = pageDraftEditorRef.current;
    if (view.kind !== "page-draft" || !editor) {
      settlement.succeed(() => action(view));
      return () => {};
    }

    const token = Symbol("draft-navigation");
    pendingDraftNavigationRef.current?.refuse();
    pendingDraftNavigationRef.current = {
      action: source => settlement.succeed(() => action(source)),
      refuse: settlement.refuse,
      token,
    };
    const cancel = () => {
      if (pendingDraftNavigationRef.current?.token === token) {
        pendingDraftNavigationRef.current = null;
        settlement.refuse();
      }
    };
    if (draftNavigationFlushRef.current) return cancel;

    const flush = editor.flush();
    draftNavigationFlushRef.current = flush;
    void flush.then(
      (saved) => {
        const pending = pendingDraftNavigationRef.current;
        pendingDraftNavigationRef.current = null;
        if (saved && viewRef.current === view) {
          const identity = editor.getIdentity();
          const sourceView: View = identity.publishedPage
            ? { kind: "page", pageId: identity.publishedPage.id, inventoryScope: view.inventoryScope, projectionIssue: identity.projectionIssue }
            : { ...view, draftId: identity.draftId ?? undefined };
          if (view.kind === "page-draft" && sourceView.kind === "page") {
            const currentTab = noteTabsRef.current.find(tab => matchesNote(tab, view));
            if (currentTab) primaryDraftIdentityByTab.current.delete(currentTab.key);
            updateNoteTabs(promoteNoteTab(noteTabsRef.current, view, sourceView).map(tab => matchesNote(tab, sourceView) ? { ...tab, title: identity.publishedPage!.title } : tab));
          } else if (view.kind === "page-draft" && sourceView.kind === "page-draft") {
            const currentTab = noteTabsRef.current.find(tab => matchesNote(tab, view));
            if (currentTab) {
              if (identity.draftId) primaryDraftIdentityByTab.current.set(currentTab.key, identity.draftId);
              else primaryDraftIdentityByTab.current.delete(currentTab.key);
              updateNoteTabs(noteTabsRef.current.map(tab => tab.key === currentTab.key ? { ...tab, view: sourceView } : tab));
            }
          }
          // The intermediate setView may be batched away by navigation.
          // Remember the saved identity before leaving so Notes reopens it.
          lastNoteViewRef.current = sourceView;
          if (sourceView.kind === "page") showView(sourceView, false);
          pending?.action(sourceView);
        } else pending?.refuse();
      },
      () => {
        const pending = pendingDraftNavigationRef.current;
        pendingDraftNavigationRef.current = null;
        pending?.refuse();
      },
    ).finally(() => {
      if (draftNavigationFlushRef.current === flush) {
        draftNavigationFlushRef.current = null;
      }
    });
    return cancel;
  };

  const afterPrimaryNavigationGuards = (
    action: (sourceView: View) => void,
    onRefused?: () => void,
  ): (() => void) => {
    const settlement = navigationSettlement(onRefused);
    const flush = view.kind === "page" ? pageFlushRef.current
      : view.kind === "entity" ? topicFlushRef.current
      : view.kind === "memory" ? memoryFlushRef.current
      : view.kind === "space" ? spaceIdentityFlushRef.current
      : view.kind === "source-document" || view.kind === "sources" ? sourceDescriptionFlushRef.current : null;
    if (flush) {
      const sourceView = view;
      const token = Symbol("page-navigation");
      pageNavigationTokenRef.current = token;
      void flush().then((saved) => {
        if (pageNavigationTokenRef.current !== token || viewRef.current !== sourceView) { settlement.refuse(); return; }
        pageNavigationTokenRef.current = null;
        if (saved) settlement.succeed(() => action(sourceView));
        else settlement.refuse();
      }, () => {
        if (pageNavigationTokenRef.current === token) pageNavigationTokenRef.current = null;
        settlement.refuse();
      });
      return () => {
        if (pageNavigationTokenRef.current === token) pageNavigationTokenRef.current = null;
        settlement.refuse();
      };
    }
    let cancelDraftFlush = () => {};
    const cancelLeave = leaveCurrentPage(() => {
      cancelDraftFlush = afterPageDraftFlush(
        source => settlement.succeed(() => action(source)),
        settlement.refuse,
      );
    }, settlement.refuse);
    return () => {
      cancelLeave();
      cancelDraftFlush();
    };
  };

  const leaveSpaceToParent = () => {
    afterPrimaryNavigationGuards(() => replaceView({ kind: "spaces" }));
  };

  const groupNavigationToken = useRef<symbol | null>(null);
  const groupNavigationCancelRef = useRef<(() => void) | null>(null);
  const afterNavigationGuards = (action: (source: View) => void, onRefused?: () => void, includeSecondary = false) => {
    if (!includeSecondary || !secondaryOpen) return afterPrimaryNavigationGuards(action, onRefused);
    groupNavigationCancelRef.current?.();
    const settlement = navigationSettlement(onRefused);
    const token = Symbol("workspace-navigation"); groupNavigationToken.current = token;
    let cancelPrimary = () => {};
    let cancelled = false;
    const cancel = () => {
      if (cancelled) return;
      cancelled = true;
      if (groupNavigationToken.current === token) groupNavigationToken.current = null;
      if (groupNavigationCancelRef.current === cancel) groupNavigationCancelRef.current = null;
      cancelPrimary();
      settlement.refuse();
    };
    groupNavigationCancelRef.current = cancel;
    void (secondaryGroupRef.current?.flush() ?? Promise.resolve(true)).then(saved => {
      if (cancelled || groupNavigationToken.current !== token) { settlement.refuse(); return; }
      if (!saved) { settlement.refuse(); return; }
      cancelPrimary = afterPrimaryNavigationGuards(source => {
        if (cancelled || groupNavigationToken.current !== token) { settlement.refuse(); return; }
        settlement.succeed(() => action(source));
      }, settlement.refuse);
    }).catch(() => settlement.refuse());
    return cancel;
  };

  useEffect(() => {
    const request = initialPageRequestRef.current;
    const next = initialPageId ?? null;
    if (request.observed !== next) {
      request.observed = next;
      request.pending = next;
    }
    const target = request.pending;
    if (!target || pageSavePending || movingNoteRef.current || request.inFlight) return;
    if (
      (view.kind === "page" && view.pageId === target)
    ) {
      request.pending = null;
      return;
    }
    const token = Symbol("initial-page-navigation");
    request.inFlight = token;
    const settle = () => { if (request.inFlight === token) request.inFlight = null; };
    const cancel = afterNavigationGuards(() => {
      if (request.inFlight !== token) return;
      settle();
      if (request.pending === target) request.pending = null;
      replaceView({ kind: "page", pageId: target });
    }, settle);
    // An autosave changes pending state while this navigation waits. Do not
    // cancel the requested destination merely because that state changed.
    return pageFlushRef.current || topicFlushRef.current || memoryFlushRef.current
      || spaceIdentityFlushRef.current || sourceDescriptionFlushRef.current ? undefined : cancel;
  }, [initialPageId, pageSavePending, movingNote]);

  useEffect(() => {
    const request = initialMemoryRequestRef.current;
    const next = initialMemoryId ?? null;
    if (request.observed !== next) {
      request.observed = next;
      request.pending = next || request.active ? { memoryId: next } : null;
    }
    const target = request.pending;
    if (!target || pageSavePending || movingNoteRef.current || request.inFlight) return;
    if (target.memoryId) {
      if (
        view.kind === "memory"
        && view.sourceId === target.memoryId
      ) {
        request.active = target.memoryId;
        request.pending = null;
        return;
      }
    } else if (view.kind === activeTab) {
      request.active = null;
      request.pending = null;
      return;
    }
    const token = Symbol("initial-memory-navigation");
    request.inFlight = token;
    const settle = () => { if (request.inFlight === token) request.inFlight = null; };
    const cancel = afterNavigationGuards(() => {
      if (request.inFlight !== token) return;
      settle();
      if (request.pending === target) request.pending = null;
      request.active = target.memoryId;
      updateHistory([], []);
      showView(
        target.memoryId
          ? { kind: "memory", sourceId: target.memoryId }
          : { kind: activeTab },
      );
    }, settle);
    return pageFlushRef.current || topicFlushRef.current || memoryFlushRef.current
      || spaceIdentityFlushRef.current || sourceDescriptionFlushRef.current ? undefined : cancel;
  }, [initialMemoryId, activeTab, pageSavePending, movingNote]);

  // New destinations branch from the current entry only after saving succeeds.
  const navigateTo = (next: View, onRefused?: () => void) => {
    if (movingNoteRef.current) return;
    if (isNoteView(next) && secondaryGroupRef.current?.contains(next)) {
      const ownedNote = next;
      if (isNoteView(viewRef.current) || viewRef.current.kind === "pages" || viewRef.current.kind === "home") {
        setOwnershipNotice({ side: "secondary", note: ownedNote });
      }
      else afterNavigationGuards(() => { showView({ kind: "pages" }); void secondaryGroupRef.current?.open(ownedNote); }, onRefused, true);
      return;
    }
    if (next.kind === "page-draft" && next.draftId) {
      const open = findPrimaryNoteTab(next);
      if (open?.view.kind === "page-draft") next = { ...next, sessionKey: open.view.sessionKey };
    }
    if (sameDestination(viewRef.current, next)) {
      if (query || pendingDraftSearchQuery || mobileSearchOpen) {
        afterNavigationGuards(dismissSearch, onRefused);
      }
      return;
    }
    const destination = next;
    const existingTab = isNoteView(destination) ? findPrimaryNoteTab(destination) : undefined;
    afterNavigationGuards((sourceView) => {
      // A tab may be closed while the current editor's asynchronous save runs.
      // Completion must not resurrect that explicitly closed destination.
      if (existingTab && !noteTabsRef.current.some(tab => tab.key === existingTab.key)) return;
      updateHistory([...viewHistoryRef.current, sourceView], []);
      // Allocate only after the current editor's save guard succeeds.
      showView(existingTab?.view ?? (next.kind === "page-draft" && next.sessionKey == null
        ? { ...next, sessionKey: ++nextDraftSessionRef.current }
        : next));
    }, onRefused, !isNoteView(next) && next.kind !== "pages" && next.kind !== "home");
  };

  const closeNoteTab = (tab: NoteTab) => {
    if (movingNoteRef.current) return;
    const active = isNoteView(viewRef.current) && matchesNote(tab, viewRef.current);
    const close = () => {
      const before = noteTabsRef.current;
      const index = before.findIndex(current => current.key === tab.key);
      if (index < 0) return;
      const remaining = before.filter(current => current.key !== tab.key);
      primaryDraftIdentityByTab.current.delete(tab.key);
      updateNoteTabs(remaining);
      if (active) {
        const next = remaining[Math.min(index, remaining.length - 1)]?.view ?? { kind: "pages" } as View;
        lastNoteViewRef.current = isNoteView(next) ? next : null;
        showView(next);
      } else if (lastNoteViewRef.current && isNoteView(lastNoteViewRef.current) && matchesNote(tab, lastNoteViewRef.current)) {
        lastNoteViewRef.current = remaining[remaining.length - 1]?.view ?? null;
      }
    };
    if (active) afterNavigationGuards(close);
    else close();
  };
  const openWikiNote = (next: NoteView) => {
    if (movingNoteRef.current) return;
    if (findPrimaryNoteTab(next)) {
      if (secondaryOpen && focusedGroup.current === "secondary") {
        setOwnershipNotice({ side: "primary", note: next });
      } else { focusPrimaryGroup(); navigateTo(next); }
    }
    else if (secondaryOpen && focusedGroup.current === "secondary") void secondaryGroupRef.current?.open(next);
    else navigateTo(next);
  };
  const moveToSecondary = (tab: NoteTab, beforeKey?: string | null) => {
    if (movingNoteRef.current) return;
    // Block editing only during the save-and-transfer transaction. Neither
    // group gives up its tab until both current documents have been saved.
    movingNoteRef.current = true; setMovingNote(true);
    const finish = () => { movingNoteRef.current = false; setMovingNote(false); };
    const source = viewRef.current;
    afterPrimaryNavigationGuards(() => {
      const current = noteTabsRef.current.find(item => item.key === tab.key);
      if (!current || !secondaryGroupRef.current) { finish(); return; }
      const draftId = primaryDraftIdentityByTab.current.get(current.key);
      const outgoing = draftId && current.view.kind === "page-draft"
        ? { ...current, view: { ...current.view, draftId } }
        : current;
      void secondaryGroupRef.current.open(outgoing.view, outgoing, () => {
        if (viewRef.current !== source && source.kind !== "page-draft") return false;
        const before = noteTabsRef.current;
        const index = before.findIndex(item => item.key === current.key);
        if (index < 0) return false;
        const active = isNoteView(viewRef.current) && matchesNote(current, viewRef.current);
        const remaining = before.filter(item => item.key !== current.key);
        primaryDraftIdentityByTab.current.delete(current.key);
        updateNoteTabs(remaining);
        const retained = (entry: View) => !isNoteView(entry) || !matchesNote(current, entry);
        updateHistory(viewHistoryRef.current.filter(retained), viewForwardRef.current.filter(retained));
        if (lastNoteViewRef.current && isNoteView(lastNoteViewRef.current) && matchesNote(current, lastNoteViewRef.current)) lastNoteViewRef.current = null;
        if (active) {
          const next = remaining[Math.min(index, remaining.length - 1)]?.view ?? { kind: "pages" } as View;
          lastNoteViewRef.current = isNoteView(next) ? next : null;
          showView(next);
        }
        return true;
      }, beforeKey).finally(finish);
    }, finish);
  };
  const moveToPrimary = (tab: NoteTab, commit: () => boolean, beforeKey?: string | null): Promise<boolean> => new Promise(resolve => {
    if (movingNoteRef.current) { resolve(false); return; }
    movingNoteRef.current = true; setMovingNote(true);
    const finish = (saved: boolean) => { movingNoteRef.current = false; setMovingNote(false); resolve(saved); };
    afterPrimaryNavigationGuards(() => {
      if (beforeKey != null && !noteTabsRef.current.some(item => item.key === beforeKey)) { finish(false); return; }
      if (!commit()) { finish(false); return; }
      showView(tab.view);
      updateNoteTabs(noteTabsRef.current.map(item => matchesNote(item, tab.view) ? { ...item, title: tab.title } : item));
      if (beforeKey !== undefined) {
        const inserted = noteTabsRef.current.find(item => matchesNote(item, tab.view));
        if (inserted) updateNoteTabs(reorderNoteTab(noteTabsRef.current, inserted.key, beforeKey));
      }
      focusPrimaryGroup();
      finish(true);
    }, () => finish(false));
  });
  const dropNoteTab = ({ sourceGroup, targetGroup, tabKey, beforeKey }: NoteTabDrop) => {
    if (movingNoteRef.current) return;
    if (sourceGroup === targetGroup) {
      if (sourceGroup === "primary") updateNoteTabs(reorderNoteTab(noteTabsRef.current, tabKey, beforeKey));
      else secondaryGroupRef.current?.reorder(tabKey, beforeKey);
    } else if (sourceGroup === "primary") {
      const tab = noteTabsRef.current.find(item => item.key === tabKey);
      if (tab) moveToSecondary(tab, beforeKey);
    } else void secondaryGroupRef.current?.move(tabKey, beforeKey);
  };
  const noteTabStrip = <NoteTabs tabs={noteTabs}
    activeKey={isNoteView(view) ? noteTabs.find(tab => matchesNote(tab, view))?.key : undefined}
    onSelect={tab => { focusPrimaryGroup(); navigateTo(tab.view); }} onClose={closeNoteTab}
    onHome={() => { focusPrimaryGroup(); navigatePages(); }} homeActive={view.kind === "pages" || view.kind === "home"}
    onMoveToOtherGroup={moveToSecondary} moveLabel={t("pages.groups.moveToRight")}
    onCreate={() => navigateTo({
      kind: "page-draft",
      space: null,
      folderPath: wikiInventoryMode === "folders" ? wikiCreationFolderPath ?? undefined : undefined,
      inventoryScope: wikiInventoryMode === "folders" && (view.kind === "pages" || view.kind === "page" || view.kind === "page-draft")
        ? view.inventoryScope : undefined,
    })} />;

  const navigateSpaces = (create: boolean) => navigateTo(create
    ? { kind: "spaces", create: true } : { kind: "spaces" });
  const navigatePages = () => navigateTo({ kind: "pages" });
  // Every import finish inside this window hands new material on through
  // these. Stable, so the cards under it do not re-render with the shell.
  const navigateToRef = useRef(navigateTo);
  navigateToRef.current = navigateTo;
  const organizeActions = useMemo<OrganizeActions>(() => ({
    onConnectTool: () => navigateToRef.current({ kind: "connect-agent" }),
    onOpenIntelligence: () => navigateToRef.current({ kind: "settings", section: "intelligence" }),
    onOpenActivity: () => navigateToRef.current({ kind: "activity" }),
    onSelectPage: (pageId) => navigateToRef.current({ kind: "page", pageId }),
  }), []);
  const navigateSettingsHome = () => {
    navigatePages();
    if (responsiveSidebar.presentation === "overlay") responsiveSidebar.close();
  };
  const navigateNotes = () => {
    if (activeNavigationForView(view) !== "pages" && lastNoteViewRef.current) {
      navigateTo(lastNoteViewRef.current);
    } else {
      navigatePages();
    }
  };

  const externalDetailExit = view.kind === "memory" && !!initialMemoryId
    && !!onBackFromDetail && viewHistory.length === 0;
  const fallbackView: View = view.kind === "source-document" ? { kind: "sources" } : view.kind === "space" ? { kind: "spaces" } : { kind: activeTab };
  const canNavigateBack = viewHistory.length > 0 || externalDetailExit || !sameDestination(view, fallbackView);
  const applyBackNavigation = (sourceView: View) => {
    const sourceStillExists = sourceView.kind !== "page" || !deletedPageIdsRef.current.has(sourceView.pageId);
    const past = viewHistoryRef.current;
    if (past.length === 0) {
      if (sourceView.kind === "memory" && initialMemoryId && onBackFromDetail) {
        onBackFromDetail();
        return;
      }
      const fallback: View = sourceView.kind === "source-document" ? { kind: "sources" } : sourceView.kind === "space" ? { kind: "spaces" } : { kind: activeTab };
      if (sameDestination(sourceView, fallback)) return;
      updateHistory([], sourceStillExists ? [...viewForwardRef.current, sourceView] : viewForwardRef.current);
      showView(fallback);
      return;
    }
    updateHistory(past.slice(0, -1), sourceStillExists ? [...viewForwardRef.current, sourceView] : viewForwardRef.current);
    showView(past[past.length - 1]);
  };
  const navigateBack = () => {
    if (movingNoteRef.current) return;
    const destination = viewHistoryRef.current[viewHistoryRef.current.length - 1] ?? fallbackView;
    const leavingNotes = !isNoteView(destination) && destination.kind !== "pages" && destination.kind !== "home";
    afterNavigationGuards(applyBackNavigation, undefined, leavingNotes);
  };
  const navigateForward = () => {
    if (movingNoteRef.current) return;
    if (viewForwardRef.current.length === 0) return;
    afterNavigationGuards((sourceView) => {
      const forward = viewForwardRef.current;
      if (!forward.length) return;
      updateHistory([...viewHistoryRef.current, sourceView], forward.slice(0, -1));
      showView(forward[forward.length - 1]);
    }, undefined, (() => { const next = viewForwardRef.current[viewForwardRef.current.length - 1]; return !!next && !isNoteView(next) && next.kind !== "pages" && next.kind !== "home"; })());
  };

  // Legacy standalone PageDetail confirms before calling this callback.
  const navigateBackFromPageDetail = () => {
    if (pageFlushRef.current) {
      navigateBack();
      return;
    }
    if (pageSavePending) return;
    setPageEditDirty(false);
    applyBackNavigation(viewRef.current);
  };
  const [sortMode, setSortMode] = useState<SortMode>("recent");
  const [memoryCollectionFilter, setMemoryCollectionFilter] = useState("");
  const [stabilityFilter, setStabilityFilter] = useState<string | null>(null);
  const [sidebarPreferences, setSidebarPreferences] = useState<SidebarPreferences>(readSidebarPreferences);
  useEffect(() => writeSidebarPreferences(sidebarPreferences), [sidebarPreferences]);
  const [sidebarPreview, setSidebarPreview] = useState<SidebarPreferences | null>(null);
  const autoSidebar = useAutoCompactSidebar(sidebarPreferences, view.kind !== "connect-agent");
  const shownSidebarPreferences: SidebarPreferences = autoSidebar.compact ? { ...sidebarPreferences, mode: "icons" } : sidebarPreferences;
  const effectiveSidebar = sidebarPreview ?? shownSidebarPreferences;
  const { query, setQuery, debouncedQuery, results, isLoading: memorySearchLoading, error: memorySearchError } = useSearch();
  const isWikiWorkspaceView = view.kind === "pages" || view.kind === "home" || view.kind === "page" || view.kind === "page-draft";

  useViewScroll(mainContentRef, viewScrollDestination,
    view.kind === "source-document" ? readySourceView === view : view.kind !== "page" || view.mode === "read" || readyEditorView === view,
    isWikiWorkspaceView ? ".wiki-workspace-content" : undefined, readingPositions);
  const handleSearchQueryChange = (nextQuery: string) => {
    const needsFlush = view.kind === "page-draft" || (view.kind === "page" && !!pageFlushRef.current)
      || (view.kind === "entity" && !!topicFlushRef.current) || (view.kind === "memory" && !!memoryFlushRef.current)
      || (view.kind === "space" && !!spaceIdentityFlushRef.current)
      || ((view.kind === "source-document" || view.kind === "sources") && !!sourceDescriptionFlushRef.current);
    if (!query && nextQuery && !needsFlush) {
      leaveCurrentPage(() => applySearchQuery(nextQuery), () => {});
      return;
    }
    if (!query && nextQuery && needsFlush) {
      pendingDraftSearchCancelRef.current?.();
      setPendingDraftSearchQuery(nextQuery);
      pendingDraftSearchCancelRef.current = afterNavigationGuards(() => {
        pendingDraftSearchCancelRef.current = null;
        setQuery(nextQuery);
        setPendingDraftSearchQuery(null);
      }, () => {
        pendingDraftSearchCancelRef.current = null;
      });
      return;
    }
    applySearchQuery(nextQuery);
  };
  const applySearchQuery = (nextQuery: string) => {
    pendingDraftSearchCancelRef.current?.();
    pendingDraftSearchCancelRef.current = null;
    setPendingDraftSearchQuery(null);
    setQuery(nextQuery);
  };
  const displayedSearchQuery = pendingDraftSearchQuery ?? query;
  const memoryResults = results.filter((result) => searchResultTarget(result).kind === "copy");
  const sourceResults = results.filter((result) => searchResultTarget(result).kind === "file");

  useEffect(() => {
    if (view.kind !== "page-draft" && view.kind !== "page") setPendingDraftSearchQuery(null);
  }, [view.kind]);

  const { data: entityResults = [], isFetching: entitySearchLoading, error: entitySearchError } = useQuery({
    queryKey: ["searchEntities", debouncedQuery],
    queryFn: () => searchEntities(debouncedQuery, 5),
    enabled: debouncedQuery.length > 0,
  });

  const { data: conceptResults = [], isFetching: conceptSearchLoading, error: conceptSearchError } = useQuery({
    queryKey: ["searchPages", debouncedQuery],
    queryFn: () => searchPages(debouncedQuery, 5),
    enabled: debouncedQuery.length > 0,
  });

  const searchLoading = !!displayedSearchQuery.trim() && (
    displayedSearchQuery !== debouncedQuery || memorySearchLoading || entitySearchLoading || conceptSearchLoading
  );
  const searchError = displayedSearchQuery === debouncedQuery
    && !!(memorySearchError || entitySearchError || conceptSearchError);

  const toggleSidebar = () => {
    setSidebarPreview(null);
    if (autoSidebar.compact) {
      autoSidebar.keepExpanded();
      return;
    }
    setSidebarPreferences((current) => ({ ...current, visible: !current.visible }));
  };
  const activeNavigation = activeNavigationForView(view);
  const wikiInventoryScope = view.kind === "pages" || view.kind === "page" || view.kind === "page-draft"
    ? view.inventoryScope ?? "all"
    : "all";
  const retainWikiScope = view.kind === "pages" || view.kind === "page" || view.kind === "page-draft";
  const openWikiDraft = (draftId: string, space: string | null) => openWikiNote({
    kind: "page-draft", draftId, space, inventoryScope: retainWikiScope ? view.inventoryScope : undefined,
  });
  const openWikiPage = (page: Page) => openWikiNote({
    kind: "page", pageId: page.id, inventoryScope: retainWikiScope ? view.inventoryScope : undefined,
  });
  // The menu bar panel hands over here once it has shown this window: a page
  // from its recent list, or the review list for an AI edit waiting on the
  // user. "Open Wenlan" sends Home, which keeps whatever was open.
  const trayPanelNavigateRef = useRef<(target: TrayPanelTarget) => void>(() => {});
  trayPanelNavigateRef.current = (target) => {
    if (target.kind === "page") navigateTo({ kind: "page", pageId: target.pageId });
    else if (target.kind === "review") navigateTo({ kind: "distill-review" });
  };
  useEffect(() => {
    const unlisten = listen<TrayPanelTarget>("tray-panel-navigate", ({ payload }) => {
      trayPanelNavigateRef.current(payload);
    });
    return () => { unlisten.then((f) => f()); };
  }, []);
  const responsiveSidebar = useResponsiveSidebar(!sidebarPreferences.visible, toggleSidebar, sidebarToggleRef);
  useEffect(() => setSidebarPreview(null), [view.kind, responsiveSidebar.isNarrow]);
  const sidebarCollapsed = responsiveSidebar.isNarrow ? responsiveSidebar.collapsed : !effectiveSidebar.visible;
  const sidebarOpen = responsiveSidebar.isNarrow ? responsiveSidebar.open : effectiveSidebar.visible;
  searchFallbackFocusRef.current = searchHeaderButtonRef.current ?? sidebarToggleRef.current;
  const standardSidebarMounted = view.kind !== "settings" && view.kind !== "connect-agent";
  const sidebarLayout = responsiveSidebar.presentation === "overlay"
    ? "overlay"
    : (!effectiveSidebar.visible || view.kind === "connect-agent")
      ? "zero"
      : effectiveSidebar.mode;
  const headerSidebarBackdropClass = `is-sidebar-backdrop-${sidebarLayout}`;
  const spacesOverviewLabels = createSpacesOverviewLabels(t);
  const spaceDetailCopy = createSpaceDetailCopy(t);
  const { data: spaces } = useQuery({ queryKey: ["spaces"], queryFn: listSpaces });
  // The directory has nothing to say to a brand-new library, and it reads as a
  // second sidebar next to the open getting-started checklist, so both give the
  // Wiki a single column; see WikiWorkspace `directoryHidden`. Once the card is
  // down to its one-line form the pages are the home again, directory included.
  const { empty: libraryEmpty } = useLibraryInventory();
  const gettingStarted = useGettingStartedState();
  const liveDirectoryHidden = libraryEmpty
    || (gettingStarted.visible && !gettingStarted.dismissed && !gettingStarted.compact);
  // Until both reads answer (they wait on the daemon at launch), keep the
  // layout of last time so the Wiki does not collapse under the user.
  const layoutKnown = gettingStarted.loaded || libraryEmpty;
  const [rememberedDirectoryHidden] = useState(readRememberedDirectoryHidden);
  const directoryHidden = layoutKnown ? liveDirectoryHidden : rememberedDirectoryHidden;
  useEffect(() => {
    if (layoutKnown) rememberDirectoryHidden(liveDirectoryHidden);
  }, [layoutKnown, liveDirectoryHidden]);

  useEffect(() => {
    if (!mobileSearchOpen) return;
    const pages = readRecentPageHistory().entries.map((entry) => ({
      kind: "page" as const,
      id: entry.id,
      title: entry.title,
      visitedAt: entry.visitedAt,
    }));
    const spacesHistory = readRecentSpaceHistory(spaces === undefined ? undefined : { spaces });
    const recentSpaces = spacesHistory.entries.map((entry) => ({
      kind: "space" as const,
      id: entry.id,
      title: entry.name,
      visitedAt: entry.visitedAt,
    }));
    setRecentSearchItems([...pages, ...recentSpaces]
      .sort((left, right) => right.visitedAt - left.visitedAt || left.kind.localeCompare(right.kind) || left.id.localeCompare(right.id))
      .slice(0, 6));
  }, [mobileSearchOpen, spaces]);

  const handlePageLoaded = useCallback((page: Pick<Page, "id" | "status" | "title">) => {
    recordRecentPageVisit(page);
    setRecentPageRevision(revision => revision + 1);
    const next = noteTabsRef.current.map(tab => noteId(tab.view) === page.id ? { ...tab, title: page.title } : tab);
    if (next.some((tab, index) => tab.title !== noteTabsRef.current[index].title)) updateNoteTabs(next);
  }, []);
  const handlePageDeleted = useCallback((pageId: string) => {
    deletedPageIdsRef.current.add(pageId);
    const isPage = (candidate: View) => candidate.kind === "page" && candidate.pageId === pageId;
    updateNoteTabs(noteTabsRef.current.filter(tab => !isPage(tab.view)));
    updateHistory(viewHistoryRef.current.filter(candidate => !isPage(candidate)), viewForwardRef.current.filter(candidate => !isPage(candidate)));
    if (lastNoteViewRef.current && isPage(lastNoteViewRef.current)) lastNoteViewRef.current = null;
    pageSelectionsRef.current.delete(pageId);
    secondaryGroupRef.current?.removePage(pageId);
    if (isPage(viewRef.current)) {
      pageFlushRef.current = null;
      pageNavigationTokenRef.current = null;
      pageSavePendingRef.current = false;
      pageEditDirtyRef.current = false;
      setPageSavePending(false);
      setPageEditDirty(false);
      setReadyEditorView(null);
    }
  }, []);
  // A note deleted from the Wiki directory (published page or draft) may have a
  // tab here. Drop it, and when it was on screen show its neighbour or the Wiki
  // home. No navigation guard runs: the note no longer exists to be saved.
  const handleDirectoryNoteDeleted = (id: string) => {
    const before = noteTabsRef.current;
    const goneTabs = before.filter(tab => noteId(tab.view) === id || primaryDraftIdentityByTab.current.get(tab.key) === id);
    const firstIndex = before.findIndex(tab => goneTabs.includes(tab));
    const isGone = (candidate: View) => isNoteView(candidate)
      && (noteId(candidate) === id || goneTabs.some(tab => matchesNote(tab, candidate)));
    const wasOnScreen = isGone(viewRef.current);
    handlePageDeleted(id);
    if (goneTabs.length > 0) {
      for (const tab of goneTabs) primaryDraftIdentityByTab.current.delete(tab.key);
      updateNoteTabs(noteTabsRef.current.filter(tab => !goneTabs.some(gone => gone.key === tab.key)));
    }
    updateHistory(viewHistoryRef.current.filter(entry => !isGone(entry)), viewForwardRef.current.filter(entry => !isGone(entry)));
    if (lastNoteViewRef.current && isGone(lastNoteViewRef.current)) lastNoteViewRef.current = null;
    if (!wasOnScreen) return;
    const remaining = noteTabsRef.current;
    const next: View = remaining[Math.min(Math.max(firstIndex, 0), remaining.length - 1)]?.view ?? { kind: "pages" };
    lastNoteViewRef.current = isNoteView(next) ? next : null;
    showView(next);
  };
  const handleDraftTitleChange = useCallback((title: string) => {
    const current = viewRef.current;
    if (current.kind !== "page-draft") return;
    const next = noteTabsRef.current.map(tab => matchesNote(tab, current) ? { ...tab, title } : tab);
    if (next.some((tab, index) => tab.title !== noteTabsRef.current[index].title)) updateNoteTabs(next);
  }, []);
  const handleSpaceLoaded = useCallback((space: Space) => {
    const runtime = spaces === undefined
      ? undefined
      : { spaces: [space, ...spaces.filter((current) => current.id !== space.id)] };
    recordRecentSpaceVisit(space, runtime);
    setView((current) => current.kind === "space" && current.spaceName === space.name
      ? { ...current, spaceId: space.id }
      : current);
  }, [spaces]);
  const handleSpaceRenamed = useCallback((space: Pick<Space, "id" | "name">) => {
    const runtime = spaces === undefined
      ? undefined
      : {
          spaces: spaces.map((current) => current.id === space.id
            ? { ...current, name: space.name }
            : current),
        };
    renameRecentSpace(space, runtime);
    setView((current) => current.kind === "space" && current.spaceId === space.id
      ? { ...current, spaceName: space.name }
      : current);
  }, [spaces]);
  const handleSpaceDeleted = useCallback((spaceId: string) => {
    const runtime = spaces === undefined
      ? undefined
      : { spaces: spaces.filter(({ id }) => id !== spaceId) };
    deleteRecentSpace(spaceId, runtime);
  }, [spaces]);

  const { data: memories = [] } = useQuery({
    queryKey: ["memories"],
    queryFn: () => listMemoriesRich(undefined, undefined, undefined, 200),
    refetchInterval: view.kind === "stream" ? 5000 : false,
  });

  // Listen for capture events
  useEffect(() => {
    const unlisten = listen<{ source: string }>("capture-event", () => {
      queryClient.invalidateQueries({ queryKey: ["memories"] });
      queryClient.invalidateQueries({ queryKey: ["memoryStats"] });
      queryClient.invalidateQueries({ queryKey: ["recentChanges"] });
      queryClient.invalidateQueries({ queryKey: ["recentRetrievals"] });
      queryClient.invalidateQueries({ queryKey: ["spaces"] });
    });
    return () => { unlisten.then((f) => f()); };
  }, [queryClient]);

  // "Open in Wenlan" on the relay pairing page sends a wenlan://pair link. The
  // native side holds the code until asked, so a link that launched the app is
  // not lost before this listener exists. The approval dialog takes it from here.
  useEffect(() => {
    const pull = () => {
      takeRemotePairingLink()
        .then((code) => { if (code) setPendingPairingCode(code); })
        .catch(() => {});
    };
    pull();
    const unlisten = listen("remote-pairing-link", pull);
    return () => { unlisten.then((f) => f()); };
  }, []);

  // Cmd/Ctrl+K is delivered by App.tsx and opens the shared search dialog.
  useEffect(() => {
    const unlisten = listen("focus-search", () => {
      if (pageSavePending) return;
      if (searchDialogOpenRef.current) {
        searchInputRef.current?.focus();
        searchInputRef.current?.select();
        return;
      }
      selectSearchOnOpenRef.current = true;
      openSearchRef.current();
    });
    return () => { unlisten.then((f) => f()); };
  }, [pageSavePending]);

  useEffect(() => {
    if (!mobileSearchOpen || !selectSearchOnOpenRef.current) return;
    selectSearchOnOpenRef.current = false;
    searchInputRef.current?.select();
  }, [mobileSearchOpen]);

  // Keyboard shortcuts
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.defaultPrevented || e.isComposing) return;
      // Cmd/Ctrl+R reloads the window, like other desktop apps built on a web
      // view. It works from inside an editor too, and saves the open note first.
      if ((e.metaKey || e.ctrlKey) && !e.shiftKey && !e.altKey && e.key.toLowerCase() === "r") {
        e.preventDefault();
        afterPrimaryNavigationGuards(() => reloadWindow());
        return;
      }
      if (e.key === "Escape" && movingNoteRef.current) return;
      const target = e.target;
      const closesOpenSearch = e.key === "Escape" && mobileSearchOpen;
      if (
        target instanceof Element
        && target.closest(
          'input, textarea, select, [contenteditable]:not([contenteditable="false"])',
        )
        && !closesOpenSearch
      ) {
        return;
      }
      if (e.key === "/" && !e.metaKey && !e.ctrlKey) {
        if (pageSavePending) return;
        e.preventDefault();
        openSearch();
      }
      if (e.key === "Escape") {
        if (e.defaultPrevented) return;
        if (!query && !mobileSearchOpen && target instanceof Element && target.closest('[data-note-group-id="secondary"]')) return;
        if (responsiveSidebar.presentation === "overlay" && responsiveSidebar.open) return;
        if (query) {
          setQuery("");
        } else if (mobileSearchOpen) {
          dismissSearch();
        } else if (view.kind === "page-draft") {
          // PageDraftEditor owns Escape so it can await the same flush gate as Back.
          return;
        } else if (view.kind === "source-document" || view.kind === "entity" || view.kind === "memory" || view.kind === "settings" || view.kind === "import" || view.kind === "graph" || view.kind === "page" || view.kind === "space" || view.kind === "distill-review") {
          navigateBack();
        }
      }
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [mobileSearchOpen, pageEditDirty, pageSavePending, query, responsiveSidebar.open, responsiveSidebar.presentation, view]);

  // Close dropdowns on outside click
  const handleEntityClick = (entityId: string) => {
    if (entityId === "__create_profile__") {
      navigateTo({ kind: "settings", section: "general" });
    } else if (entityId.startsWith("memory:")) {
      navigateTo({ kind: "memory", sourceId: entityId.replace("memory:", "") });
    } else if (entityId.startsWith("page:")) {
      navigateTo({ kind: "page", pageId: entityId.replace("page:", "") });
    } else {
      navigateTo({ kind: "entity", entityId });
    }
  };

  const openSearchResult = async (result: SearchResult) => {
    dismissSearch();
    const target = searchResultTarget(result);
    if (target.kind === "page") {
      navigateTo({ kind: "page", pageId: target.pageId });
    } else if (target.kind === "file") {
      try {
        await openSearchResultTarget(target.url);
      } catch (err) {
        console.warn("[search] open file failed", err);
        toast(t("sourceAccess.openFileFailed"));
      }
    } else {
      navigateTo({ kind: "memory", sourceId: result.source_id });
    }
  };

  const isSettingsView = view.kind === "settings";
  const keyboardShortcutModifier = /Mac|iPhone|iPad/.test(navigator.platform) ? "⌘" : "Ctrl";
  const searchActionLabel = t(isSettingsView ? "settings.home" : "main.searchButton");
  const searchActionTitle = isSettingsView
    ? searchActionLabel
    : `${searchActionLabel} (${t("main.searchShortcut", { modifier: keyboardShortcutModifier })})`;
  const globalActions = (
          <div className="workspace-header-actions relative z-[1] flex items-center gap-1.5 shrink-0">
            {(view.kind === "connect-agent" || sidebarLayout === "zero" || (responsiveSidebar.presentation === "overlay" && !responsiveSidebar.open)) && (
              <button
                aria-expanded={isSettingsView ? undefined : mobileSearchOpen}
                aria-label={searchActionLabel}
                className="workspace-header-search-fallback rounded-md p-1.5 transition-colors duration-150 hover:bg-[var(--mem-hover-strong)]"
                disabled={pageSavePending}
                onClick={(event) => isSettingsView ? navigateSettingsHome() : openSearch(event.currentTarget)}
                ref={searchHeaderButtonRef}
                style={{ color: "var(--mem-text-secondary)" }}
                title={searchActionTitle}
                type="button"
              >
                {isSettingsView ? <ArrowLeft size={18} aria-hidden="true" /> : (
                <svg aria-hidden="true" fill="none" height="16" stroke="currentColor" viewBox="0 0 24 24" width="16">
                  <path d="m21 21-4.35-4.35m2.35-5.65a8 8 0 1 1-16 0 8 8 0 0 1 16 0Z" strokeLinecap="round" strokeLinejoin="round" strokeWidth="1.8" />
                </svg>
                )}
              </button>
            )}
            <div className="workspace-document-tools-host" ref={setDocumentToolsHost} />
          </div>
  );

  return (
    <ReferenceNavigationProvider
      onOpenPage={(pageId) => navigateTo({ kind: "page", pageId })}
      onOpenMemory={(sourceId) => navigateTo({ kind: "memory", sourceId })}
    >
    <WorkspaceNavigationProvider>
    <WorkspaceReadingHostContext.Provider value={mainContentRef}>
    <WorkspacePaneHostContext.Provider value={workspacePaneHost}>
    <WorkspaceDocumentToolsHostContext.Provider value={documentToolsHost}>
    <WorkspaceDemandContext.Provider value={autoSidebar.demand}>
    <OrganizeActionsContext.Provider value={organizeActions}>
    <div
      className="memory-shell h-screen w-full"
      data-sidebar-layout={sidebarLayout}
      data-sidebar-auto-transition={autoSidebar.transitioning || undefined}
      data-note-workspace={isWikiWorkspaceView || undefined}
      inert={mobileSearchOpen || movingNote}
      style={{ backgroundColor: "var(--mem-bg)", color: "var(--mem-text)", "--workspace-labels-width": `${effectiveSidebar.width ?? 240}px` } as CSSProperties}
    >
      {/* Header follows the width of the primary workspace. */}
      <header
        className="memory-workspace-header workspace-header-container relative flex items-center gap-3 shrink-0"
        style={{
          height: MAIN_HEADER_HEIGHT,
          paddingLeft: topBarLeftInset(),
          paddingRight: "var(--workspace-header-right-padding, 20px)",
          background: "transparent",
        }}
        data-tauri-drag-region
      >
        {view.kind !== "connect-agent" && (
          <SidebarToggleButton collapsed={sidebarCollapsed || (!responsiveSidebar.isNarrow && autoSidebar.compact)} onToggle={responsiveSidebar.toggle} ref={sidebarToggleRef} />
        )}
        <div className="workspace-history-navigation" role="group" aria-label={t("main.historyNavigation")}>
          <button type="button" aria-label={t("main.back")} title={t("main.back")}
            className="workspace-history-button" disabled={!canNavigateBack}
            onClick={navigateBack}><ArrowLeft size={18} aria-hidden="true" /></button>
          <button type="button" aria-label={t("main.forward")} title={t("main.forward")}
            className="workspace-history-button" disabled={!viewForward.length}
            onClick={navigateForward}><ArrowRight size={18} aria-hidden="true" /></button>
        </div>
        {view.kind === "memory" && (
          <ContextBrowser
            key={activeNavigation}
            kind="memories"
            inventoryScope="all"
            browsingPages={false}
            currentPageId={null}
            currentMemoryId={view.kind === "memory" ? (view.sourceId ?? null) : null}
            onBrowsePages={() => navigateTo({ kind: "pages" })}
            onCreatePage={() => navigateTo({ kind: "page-draft", space: null })}
            onSelectDraft={openWikiDraft}
            onSelectPage={openWikiPage}
            onSelectMemory={(sourceId) => navigateTo({ kind: "memory", sourceId })}
          />
        )}
        {(!standardSidebarMounted || !responsiveSidebar.open) && <ReviewEnvironmentBadge compact />}
        <div className="flex-1" data-tauri-drag-region />

        {isWikiWorkspaceView && groupToolsHost ? createPortal(globalActions, groupToolsHost) : globalActions}

        <div
          aria-hidden="true"
          className={`workspace-header-sidebar-backdrop ${headerSidebarBackdropClass}`}
        />

      </header>

      <SidebarResizeHandle
        key={view.kind}
        enabled={!responsiveSidebar.isNarrow && view.kind !== "connect-agent"}
        preferences={shownSidebarPreferences}
        onPreview={setSidebarPreview}
        onCommit={setSidebarPreferences}
        onHidden={() => sidebarToggleRef.current?.focus()}
      />

      {/* Sidebar + Content row */}
      <div className="memory-shell-content flex flex-1 overflow-hidden">
        {view.kind === "settings" ? (
        <SettingsSidebar
            hidden={sidebarCollapsed}
            mode={effectiveSidebar.mode}
            active={view.section ?? "general"}
            open={sidebarOpen}
            presentation={responsiveSidebar.presentation}
            onRequestClose={responsiveSidebar.close}
            onNavigateHome={navigateSettingsHome}
            navigationDisabled={pageSavePending}
            onSelect={(section) => {
              navigateTo({ kind: "settings", section });
              if (responsiveSidebar.presentation === "overlay") responsiveSidebar.close();
            }}
          />
        ) : view.kind === "connect-agent" ? null : (
          <Sidebar
            activeNavigation={activeNavigation}
            hidden={sidebarCollapsed}
            mode={effectiveSidebar.mode}
            onEntityClick={handleEntityClick}
            onNavigateLog={() => navigateTo({ kind: "stream" })}
            onNavigateActivity={() => navigateTo({ kind: "activity" })}
            activityCurrent={view.kind === "activity"}
            onConnectTool={() => navigateTo({ kind: "connect-agent" })}
            onNavigatePages={navigateNotes}
            onNavigateEntities={() => navigateTo({ kind: "entities" })}
            onNavigateGraph={() => navigateTo({ kind: "graph" })}
            onNavigateSources={() => navigateTo({ kind: "sources" })}
            onNavigateSpaces={navigateSpaces}
            onNavigateSettings={() => navigateTo({ kind: "settings", section: "general" })}
            onOpenSearch={openSearch}
            onOpenAbout={() => setAboutOpen(true)}
            searchOpen={mobileSearchOpen}
            searchDisabled={pageSavePending}
            onRequestClose={responsiveSidebar.close}
            open={sidebarOpen}
            presentation={responsiveSidebar.presentation}
          />
        )}

        {/* Main content */}
        <main ref={mainContentRef} className={`flex-1 ${view.kind === "graph" || view.kind === "sources" || view.kind === "connect-agent" ? "min-w-0 overflow-hidden p-0" : isWikiWorkspaceView ? "memory-main-content memory-main-content--wiki overflow-hidden" : `memory-main-content overflow-y-auto${view.kind === "memory" || view.kind === "source-document" || view.kind === "entity" ? " memory-main-content--page" : ["spaces", "entities", "stream"].includes(view.kind) ? " memory-main-content--collection" : ""}`}`}>
          {view.kind === "import" ? (
            <ImportView
              onBack={navigateBack}
              completeLabel={view.fromFirstUse ? t("firstUse.guide.seeKnowledge") : undefined}
              onComplete={(_source, result) => {
                if (view.fromFirstUse) {
                  showView({
                    kind: "first-use",
                    showKnowledge: true,
                    batchId: result.imported > 0 ? result.batch_id : undefined,
                  });
                  updateHistory([{ kind: "pages" }], []);
                } else {
                  replaceView({ kind: "stream" });
                }
              }}
            />
          ) : view.kind === "settings" ? (
            <SettingsPage
              currentSpace={contextSpace?.spaceName}
              section={view.section ?? "general"}
              onBack={navigateBack}
              onSetupAgent={() => navigateTo({ kind: "connect-agent" })}
              onOpenConnections={() => navigateTo({ kind: "settings", section: "agents" })}
              onOpenIntelligence={() => navigateTo({ kind: "settings", section: "intelligence" })}
              onOpenActivity={() => navigateTo({ kind: "activity" })}
              onImport={() => navigateTo({ kind: "import" })}
            />
          ) : (view.kind === "pages" || view.kind === "home") ? (
            <WikiWorkspace
              directoryHidden={directoryHidden}
              tabs={noteTabStrip} onGroupFocus={focusPrimaryGroup} onGlobalToolsHost={setGroupToolsHost}
              secondaryOpen={secondaryOpen} onSecondaryHost={setSecondaryHost}
              inventoryMode={wikiInventoryMode}
              onInventoryModeChange={setWikiInventoryMode}
              recentRevision={recentPageRevision}
              onOpenRecentPage={(pageId) => openWikiNote({ kind: "page", pageId })}
              inventoryScope={wikiInventoryScope}
              onCreationFolderPathChange={setWikiCreationFolderPath}
              browsing
              filter={pageInventoryFilter}
              onFilterChange={setPageInventoryFilter}
              onBrowse={(scope) => navigateTo({ kind: "pages", inventoryScope: scope })}
              onCreatePage={(folderPath) => navigateTo({ kind: "page-draft", space: null, folderPath, inventoryScope: view.kind === "pages" ? view.inventoryScope : undefined })}
              onOpenDraft={openWikiDraft}
              onOpenPage={openWikiPage}
              onDeletePage={handleDirectoryNoteDeleted}
            >
              <PagesOverview
                inventoryScope={view.kind === "pages" ? view.inventoryScope : undefined}
                onBrowseAll={() => {
                  // The home lists only the newest pages; every page is in the
                  // directory beside it, by folder.
                  writeWikiInventoryMode("folders");
                  setWikiInventoryMode("folders");
                }}
                onBrowseFolder={(inventoryScope) => navigateTo({ kind: "pages", inventoryScope })}
                onCreatePage={(space, folderPath) => navigateTo({ kind: "page-draft", space, folderPath, inventoryScope: view.kind === "pages" ? view.inventoryScope : undefined })}
                onSelectDraft={openWikiDraft}
                onSelectPage={(id) => navigateTo({ kind: "page", pageId: id, inventoryScope: view.kind === "pages" ? view.inventoryScope : undefined })}
                onSelectSpace={(spaceName) => navigateTo({ kind: "space", spaceId: null, spaceName })}
                onConnectTool={() => navigateTo({ kind: "connect-agent" })}
                onSeeExample={() => navigateTo({ kind: "first-use" })}
                onOpenIntelligence={() => navigateTo({ kind: "settings", section: "intelligence" })}
                onOpenBasisUpdates={() => navigateTo({ kind: "distill-review", reviewItemId: BASIS_UPDATES_REVIEW_ID })}
                onOpenNoteSuggestion={(container) => {
                  setSourceLibraryState({ search: "", filter: "all", container });
                  navigateTo({ kind: "sources" });
                }}
              />
            </WikiWorkspace>
          ) : view.kind === "entities" ? (
            <EntitiesView
              onEntityClick={handleEntityClick}
            />
          ) : view.kind === "spaces" ? (
            <SpacesOverview
              createIntent={view.create}
              labels={spacesOverviewLabels}
              onCreateIntentHandled={() => replaceView({ kind: "spaces" })}
              onSelectSpace={(spaceName) => navigateTo({ kind: "space", spaceId: null, spaceName })}
              onSpaceDeleted={handleSpaceDeleted}
              onSpaceRenamed={handleSpaceRenamed}
            />
          ) : view.kind === "space" ? (
            <SpaceDetail
              copy={spaceDetailCopy}
              spaceName={view.spaceName}
              onBack={leaveSpaceToParent}
              onCreatePage={(space) => navigateTo({ kind: "page-draft", space })}
              onReviewAll={() => navigateTo({ kind: "distill-review" })}
              onSelectPage={(id) => navigateTo({ kind: "page", pageId: id })}
              onOpenSource={(file) => navigateTo({ kind: "source-document", file })}
              onSpaceDeleted={(spaceId) => {
                handleSpaceDeleted(spaceId);
                replaceView({ kind: "spaces" });
              }}
              onSpaceLoaded={handleSpaceLoaded}
              onSpaceRenamed={handleSpaceRenamed}
              onRegisterFlush={registerSpaceIdentityFlush}
            />
          ) : view.kind === "memory" ? (
            <MemoryDetail
              key={view.sourceId}
              onRegisterFlush={registerMemoryFlush}
              sourceId={view.sourceId}
              onBack={navigateBack}
              onNavigateEntity={handleEntityClick}
              onNavigateMemory={(sid) => navigateTo({ kind: "memory", sourceId: sid })}
              onNavigatePage={(pageId) => navigateTo({ kind: "page", pageId })}
            />
          ) : view.kind === "connect-agent" ? (
            <SetupWizard
              embedded
              initialStep="connect"
              onComplete={navigateBack}
            />
          ) : view.kind === "entity" ? (
            <EntityDetail
              key={view.entityId}
              entityId={view.entityId}
              onRegisterFlush={registerTopicFlush}
              onBack={navigateBack}
              onEntityClick={handleEntityClick}
              onMemoryClick={(sid) => navigateTo({ kind: "memory", sourceId: sid })}
              onPageClick={(pageId) => navigateTo({ kind: "page", pageId })}
            />
          ) : view.kind === "page-draft" ? (
            <WikiWorkspace
              tabs={noteTabStrip} onGroupFocus={focusPrimaryGroup} onGlobalToolsHost={setGroupToolsHost}
              secondaryOpen={secondaryOpen} onSecondaryHost={setSecondaryHost}
              inventoryMode={wikiInventoryMode}
              onInventoryModeChange={setWikiInventoryMode}
              recentRevision={recentPageRevision}
              onOpenRecentPage={(pageId) => openWikiNote({ kind: "page", pageId })}
              inventoryScope={wikiInventoryScope}
              onCreationFolderPathChange={setWikiCreationFolderPath}
              currentPageId={view.draftId ?? null}
              currentFolderPath={view.folderPath}
              filter={pageInventoryFilter}
              onFilterChange={setPageInventoryFilter}
              onBrowse={(scope) => navigateTo({ kind: "pages", inventoryScope: scope })}
              onCreatePage={(folderPath) => navigateTo({ kind: "page-draft", space: null, folderPath, inventoryScope: view.inventoryScope })}
              onOpenDraft={openWikiDraft}
              onOpenPage={openWikiPage}
              onDeletePage={handleDirectoryNoteDeleted}
            >
            <PageDraftEditor
              key={view.sessionKey ?? view.draftId ?? "new"}
              draftId={view.draftId}
              onDraftIdentity={reportPrimaryDraftIdentity}
              onTitleChange={handleDraftTitleChange}
              onBack={() => {
                if (responsiveSidebar.presentation === "overlay" && responsiveSidebar.open) {
                  responsiveSidebar.close();
                  return;
                }
                navigateBack();
              }}
              onEscapeBeforeLeave={() => {
                if (
                  responsiveSidebar.presentation === "overlay"
                  && responsiveSidebar.open
                ) {
                  responsiveSidebar.close();
                  return true;
                }
                return false;
              }}
              onOpenExisting={(pageId) => {
                afterPageDraftFlush(() => replaceView({ kind: "page", pageId }));
              }}
              onPublished={(pageId, projectionIssue) => replaceView({ kind: "page", pageId, inventoryScope: view.inventoryScope, projectionIssue })}
              ref={pageDraftEditorRef}
              folderPath={view.folderPath ?? (view.inventoryScope ? inventoryFolderPath(view.inventoryScope) ?? undefined : undefined)}
              space={view.space}
            />
            </WikiWorkspace>
          ) : view.kind === "page" ? (
            <WikiWorkspace
              tabs={noteTabStrip} onGroupFocus={focusPrimaryGroup} onGlobalToolsHost={setGroupToolsHost}
              secondaryOpen={secondaryOpen} onSecondaryHost={setSecondaryHost}
              inventoryMode={wikiInventoryMode}
              onInventoryModeChange={setWikiInventoryMode}
              recentRevision={recentPageRevision}
              onOpenRecentPage={(pageId) => openWikiNote({ kind: "page", pageId })}
              inventoryScope={wikiInventoryScope}
              onCreationFolderPathChange={setWikiCreationFolderPath}
              currentPageId={view.pageId}
              filter={pageInventoryFilter}
              onFilterChange={setPageInventoryFilter}
              fullBleed
              onBrowse={(scope) => navigateTo({ kind: "pages", inventoryScope: scope })}
              onCreatePage={(folderPath) => navigateTo({ kind: "page-draft", space: null, folderPath, inventoryScope: view.inventoryScope })}
              onOpenDraft={openWikiDraft}
              onOpenPage={openWikiPage}
              onDeletePage={handleDirectoryNoteDeleted}
            >
            <PageDetail
              pageId={view.pageId}
              onDeleted={handlePageDeleted}
              onOpenReview={(reviewItemId) => navigateTo({ kind: "distill-review", reviewItemId })}
              projectionIssue={view.projectionIssue}
              onProjectionResolved={() => replaceView({ ...view, projectionIssue: undefined })}
              initialSelection={pageSelectionsRef.current.get(view.pageId)}
              onSelectionChange={(selection) => pageSelectionsRef.current.set(view.pageId, selection)}
              onEditorReady={() => setReadyEditorView(view)}
              initialMode={view.mode ?? "edit"}
              onBack={navigateBackFromPageDetail}
              onRegisterFlush={registerPageFlush}
              onEditDirtyChange={reportPageDirty}
              onMemoryClick={(sid) => navigateTo({ kind: "memory", sourceId: sid })}
              onOpenGraph={(pageId) => navigateTo({ kind: "graph", focusPageId: pageId })}
              onPageLoaded={handlePageLoaded}
              onPageClick={(id) => navigateTo({ kind: "page", pageId: id })}
              onEntityClick={handleEntityClick}
              onSavePendingChange={reportPageSaving}
            />
            </WikiWorkspace>
          ) : view.kind === "distill-review" ? (
            <DistillReviewPanel
              onBack={navigateBack}
              onOpenActivity={() => navigateTo({ kind: "activity" })}
              initialReviewItemId={view.reviewItemId}
              onPageClick={(id) => navigateTo({ kind: "page", pageId: id, mode: "read" })}
              onMemoryClick={(sid) => navigateTo({ kind: "memory", sourceId: sid })}
            />
          ) : view.kind === "first-use" ? (
            <FirstUseGuide
              onBack={() => navigatePages()}
              initialView={view.showKnowledge ? "live" : "sample"}
              batchId={view.batchId}
              onImport={() => navigateTo({ kind: "import", fromFirstUse: true })}
              onOpenIntelligence={() => navigateTo({ kind: "settings", section: "intelligence" })}
              onOpenPage={(id) => {
                updateHistory([...viewHistoryRef.current, {
                  kind: "first-use",
                  showKnowledge: true,
                  batchId: view.batchId,
                }], []);
                showView({ kind: "page", pageId: id });
              }}
            />
          ) : view.kind === "activity" ? (
            <ActivityFeed
              onOpenReview={(reviewItemId) => navigateTo({ kind: "distill-review", reviewItemId })}
              onOpenPage={(pageId) => navigateTo({ kind: "page", pageId })}
              onNavigateMemory={(sid) => navigateTo({ kind: "memory", sourceId: sid })}
              onOpenIntelligence={() => navigateTo({ kind: "settings", section: "intelligence" })}
            />
          ) : view.kind === "source-document" ? (
            <SourceDocumentPage key={JSON.stringify([view.file.source, view.file.source_id, view.file.space ?? null])} file={view.file} onReady={() => setReadySourceView(view)} onRegisterFlush={registerSourceDescriptionFlush} />
          ) : view.kind === "sources" ? (
            <SourcesView
              libraryState={sourceLibraryState}
              onLibraryStateChange={setSourceLibraryState}
              onOpenDocument={(file) => navigateTo({ kind: "source-document", file })}
              onRegisterFlush={registerSourceDescriptionFlush}
              onManageSources={() => navigateTo({ kind: "settings", section: "sources" })}
            />
          ) : view.kind === "graph" ? (
            <div style={{ position: "relative", width: "100%", height: "100%" }}>
              <AtlasView
                key={view.focusPageId ?? "all"}
                focusPageId={view.focusPageId}
                onNodeClick={(target) =>
                  target.kind === "memory"
                    ? navigateTo({ kind: "memory", sourceId: target.id })
                    : target.kind === "page"
                      ? navigateTo({ kind: "page", pageId: target.id })
                      : handleEntityClick(target.id)
                }
                onBack={navigateBack}
              />
            </div>
          ) : (
            <>
              <WorkspaceBackButton onClick={() => navigateTo({ kind: "pages" })} className="p-1.5 -ml-1.5 rounded-md transition-colors duration-150 hover:bg-[var(--mem-hover)] mb-3" style={{ color: "var(--mem-text-tertiary)", background: "none", border: "none", cursor: "pointer", lineHeight: 0 }}>
                <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5"><path d="M19 12H5M12 19l-7-7 7-7" /></svg>
              </WorkspaceBackButton>
              <MemoryStream
                memories={memories}
                selectedDomain={null}
                sortMode={sortMode}
                onSortChange={setSortMode}
                stabilityFilter={stabilityFilter}
                onStabilityFilterChange={setStabilityFilter}
                onSelectMemory={(sid) => navigateTo({ kind: "memory", sourceId: sid })}
                presentation="parent-list"
                filter={memoryCollectionFilter}
                onFilterChange={setMemoryCollectionFilter}
              />
            </>
          )}
        </main>
      </div>

      {ownershipNotice && <div className="note-owner-notice" role="status">
        <span>{t(ownershipNotice.side === "secondary" ? "pages.groups.openInRight" : "pages.groups.openInCentral")}</span>
        <button type="button" onClick={() => {
          const notice = ownershipNotice; setOwnershipNotice(null);
          if (notice.side === "secondary") {
            if (secondaryGroupRef.current?.contains(notice.note)) void secondaryGroupRef.current.open(notice.note);
          } else if (findPrimaryNoteTab(notice.note)) { focusPrimaryGroup(); navigateTo(notice.note); }
        }}>{t("pages.groups.showNote")}</button>
        <button className="note-owner-notice-dismiss" type="button" aria-label={t("pages.groups.dismiss")} onClick={() => setOwnershipNotice(null)}><X size={16} aria-hidden="true" /></button>
      </div>}
      <SecondaryNoteGroup readingPositions={readingPositions} selections={pageSelectionsRef.current} ref={secondaryGroupRef} host={isWikiWorkspaceView ? secondaryHost : null}
        movingNote={movingNote}
        onPageDeleted={handlePageDeleted}
        onPresenceChange={setSecondaryPresence} onFocus={focusSecondaryGroup}
        onNavigationIntent={() => { groupNavigationCancelRef.current?.(); }}
        onNavigate={navigateTo} onOpenNote={next => {
          if (findPrimaryNoteTab(next)) {
            setOwnershipNotice({ side: "primary", note: next });
          }
          else void secondaryGroupRef.current?.open(next);
        }} onMove={moveToPrimary} allocateDraftSession={() => ++nextDraftSessionRef.current} />
      <div className="workspace-pane-host" ref={setWorkspacePaneHostRef} />

      <AboutWenlanDialog open={aboutOpen} onClose={() => setAboutOpen(false)} />
      {/* The approval dialog opens over whatever screen is showing; it never navigates. */}
      <PairingApprovalDialog currentSpace={contextSpace?.spaceName} />
      <RemoteAccessNotifier />
      <QuickCaptureScrim />
      <SearchModal
        open={mobileSearchOpen}
        query={displayedSearchQuery}
        placeholder={t("main.searchPlaceholder")}
        inputRef={searchInputRef}
        returnFocusRef={searchReturnFocusRef}
        fallbackFocusRef={searchFallbackFocusRef}
        loading={searchLoading}
        error={searchError}
        onQueryChange={handleSearchQueryChange}
        onClose={dismissSearch}
      >
        <RecentlyOpened
          query={displayedSearchQuery}
          items={recentSearchItems}
          onSelect={(item) => {
            dismissSearch();
            navigateTo(item.kind === "page"
              ? { kind: "page", pageId: item.id }
              : { kind: "space", spaceId: item.id, spaceName: item.title });
          }}
        />
        <SearchResults
          query={displayedSearchQuery}
          memoryResults={memoryResults}
          sourceResults={sourceResults}
          entityResults={entityResults}
          conceptResults={conceptResults}
          loading={searchLoading}
          error={searchError}
          ready={!searchLoading && !searchError}
          onOpenResult={(result) => void openSearchResult(result)}
          onOpenPage={(pageId) => { dismissSearch(); navigateTo({ kind: "page", pageId }); }}
          onOpenEntity={(entityId) => { dismissSearch(); handleEntityClick(entityId); }}
        />
      </SearchModal>
      <NoteTabDragProvider onDrop={dropNoteTab} disabled={movingNote || !isWikiWorkspaceView || mobileSearchOpen} />
    </div>
    </OrganizeActionsContext.Provider>
    </WorkspaceDemandContext.Provider>
    </WorkspaceDocumentToolsHostContext.Provider>
    </WorkspacePaneHostContext.Provider>
    </WorkspaceReadingHostContext.Provider>
    </WorkspaceNavigationProvider>
    </ReferenceNavigationProvider>
  );
}
