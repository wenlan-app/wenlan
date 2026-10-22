// SPDX-License-Identifier: AGPL-3.0-only
import SourceDocumentPreview from "./sources/SourceDocumentPreview";
import { WorkspaceBackButton } from "./navigation/WorkspaceNavigation";
import { useState, useCallback, useMemo, useRef, useEffect, useLayoutEffect, useContext, useId } from "react";
import { useQuery, useQueries, useMutation, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import { createPortal } from "react-dom";
import { WorkspaceDocumentToolsHostContext, WorkspaceNoteGroupContext } from "./navigation/WorkspacePaneHost";
import { NoteInspectorTabs, type NoteInspectorTab } from "./page/NoteInspectorTabs";
import { ArrowLeft, SidebarSimple } from "@phosphor-icons/react";
import {
  getPage,
  getPageLinks,
  getPageRevisions,
  getEntityDetail,
  redistillPage,
  setPageReviewRequired,
  updatePage,
  renamePage,
  getDaemonVersion,
  getSystemInfo,
  daemonMeetsFloor,
  recordPageEditorDiagnostic,
  deletePage,
  clipboardWrite,
  exportPageToObsidian,
  listRegisteredSources,
  getPageSources,
  reviewPage,
  pageReviewSupported,
  parseEntityGuardError,
  markPageBasis,
  type Entity,
  type BasisConfirmOutcome,
  type BasisRef,
  type CodeBasisRef,
  type Page,
  type PageReviewOutcome,
  type IndexedFileInfo,
  type UpdatePageFailureKind,
} from "../../lib/tauri";
// Imported from its own module, not the lib/tauri barrel: tests mock that
// barrel, and a mocked module would shadow the value this must stay bound to.
import { PAGE_EDIT_DAEMON_FLOOR } from "../../lib/daemonVersion";
import { EXPLICIT_BROWSE_QUERY_POLICY, useTruthStatus } from "../../hooks/useTruthStatus";
import { PageTruthBadges } from "./PageTruthBadges";
import ContentRenderer from "./ContentRenderer";
import RelatedPages from "./page/RelatedPages";
import PageInfo from "./page/PageInfo";
import { PageSessionSources } from "./sessions/SessionSourceSection";
import PageInfoDrawer from "./page/PageInfoDrawer";
import { DetailInfoContent, DetailReadingFrame } from "./DetailLayout";
import { InfoAction } from "./page/InfoAction";
import "./page/pageDocumentTools.css";
import KnowledgeContext from "./context/KnowledgeContext";
import { pageReviewNotice, type PageReviewNotice } from "./page/pageReviewNotice";
import { LockIcon, RailPanelTitle } from "./MemoryDetailPrimitives";
import { processCitations, stripCitationLinks } from "../../lib/pageCitations";
import { stripLedeLabel } from "../../lib/pageLede";
import CitationChip from "./page/CitationChip";
import { ReferencePreview } from "./links/ReferencePreview";
import { ReferenceNavigationProvider } from "./links/ReferenceNavigationContext";
import type { ReferencePreviewRequest, ReferenceTarget } from "./links/referenceTypes";
import PageCanvas from "./PageCanvas";
import {
  prepareMarkdownSource,
  serializeMarkdownSource,
  type MarkdownSourceProfile,
  type PreparedMarkdownSource,
} from "./editor/markdownSourceContract";
import {
  type PageEditBaseline,
  type PageSaveCoordinatorState,
} from "./editor/pageSaveCoordinator";
import {
  MarkdownEditor,
  type MarkdownEditorHandle,
  type MarkdownEditorStatus,
  type MarkdownEditorSelection,
} from "./editor/MarkdownEditor";
import type { LinkCompletionNote } from "./editor/wikiLinkCompletion";
import { linkCompletionText, pageTitleKey, trimLikeDaemon } from "./editor/pageTitleKey";
import { loadAllLinkCandidates, linkNotesFor } from "./page/linkCandidates";
import { leadingMarkdownH1MatchesTitle } from "./editor/pageEditorPresentation";
import { MovePageDialog } from "./pages/MovePageDialog";
import { PageProjectionNotice } from "./pages/PageProjectionNotice";
import { inventoryPageFilename, type PageProjectionIssue } from "./pages/pageInventory";
import { PageAutosave } from "./editor/pageAutosave";
import { NoteReviewNotice } from "./page/NoteReviewNotice";
import { useBasisActions } from "./page/useBasisActions";
import { useCodeBasisActions } from "./page/useCodeBasisActions";
import { basisCountLabel } from "./page/format";

interface PageDetailProps {
  pageId: string;
  projectionIssue?: PageProjectionIssue;
  onProjectionResolved?: () => void;
  /** Navigation intent: ordinary pages can start editing; review origins read. */
  initialMode?: "read" | "edit";
  initialSelection?: MarkdownEditorSelection;
  onSelectionChange?: (selection: MarkdownEditorSelection) => void;
  onEditorReady?: () => void;
  onBack: () => void;
  onDeleted?: (pageId: string) => void;
  onMemoryClick: (sourceId: string) => void;
  onPageClick?: (pageId: string) => void;
  onEntityClick?: (entityId: string) => void;
  onOpenGraph?: (pageId: string) => void;
  onOpenReview?: (itemId?: string) => void;
  onDismissAttachedPageNotice?: () => void;
  onPageLoaded?: (page: Pick<Page, "id" | "status" | "title">) => void;
  onSavePendingChange?: (pending: boolean) => void;
  onEditDirtyChange?: (dirty: boolean) => void;
  onRegisterFlush?: (flush: (() => Promise<boolean>) | null) => void;
  showAttachedPageNotice?: boolean;
}

function relativeTimeFromISO(iso: string, t: TFunction): string {
  const diff = Math.floor((Date.now() - new Date(iso).getTime()) / 1000);
  if (diff < 60) return t("pageDetail.dateline.relativeJustNow");
  if (diff < 3600) {
    return t("pageDetail.dateline.relativeMinutesAgo", {
      count: Math.floor(diff / 60),
    });
  }
  if (diff < 86400) {
    return t("pageDetail.dateline.relativeHoursAgo", {
      count: Math.floor(diff / 3600),
    });
  }
  return t("pageDetail.dateline.relativeDaysAgo", {
    count: Math.floor(diff / 86400),
  });
}

function normalizeLinkLabel(label: string): string {
  return pageTitleKey(label);
}

function parseWikilink(inner: string): { targetLabel: string; displayText: string; hasAlias: boolean } {
  const pipeIndex = inner.indexOf("|");
  const rawTarget = pipeIndex >= 0 ? inner.slice(0, pipeIndex) : inner;
  const headingIndex = rawTarget.indexOf("#");
  const targetLabel = trimLikeDaemon(headingIndex >= 0 ? rawTarget.slice(0, headingIndex) : rawTarget);
  const targetDisplay = targetLabel || rawTarget.trim();
  const alias = pipeIndex >= 0 ? inner.slice(pipeIndex + 1).trim() : "";
  return {
    targetLabel,
    displayText: alias || targetDisplay || inner.trim(),
    hasAlias: alias.length > 0,
  };
}

function folderName(path: string): string {
  return path.split("/").filter(Boolean).pop() || path;
}

function createPageOperationId(): string {
  return globalThis.crypto.randomUUID();
}

function localShortcutModifier(): "Cmd" | "Ctrl" {
  if (typeof navigator === "undefined") return "Ctrl";
  return navigator.platform.toLowerCase().startsWith("mac") ? "Cmd" : "Ctrl";
}

const PAGE_LINK_ANCHOR_PREFIX = "#concept:";
type MenuInitialFocus = "first" | "last";

type PageEditGate =
  | { kind: "closed" }
  | { kind: "checking" }
  | { kind: "unsupported"; version: string | null }
  | { kind: "normalize"; prepared: PreparedMarkdownSource }
  | { kind: "editor" };

type ConflictLatestSource =
  | { kind: "idle" }
  | { kind: "loading"; operationId: string }
  | { kind: "loaded"; operationId: string; page: Page }
  | { kind: "error"; operationId: string };

function enabledMenuItems(menu: HTMLDivElement | null): HTMLElement[] {
  if (!menu) return [];
  const items = Array.from(
    menu.querySelectorAll<HTMLElement>('[role^="menuitem"]:not(:disabled)'),
  );
  const renderedItems = items.filter((item) => item.getClientRects().length > 0);
  return renderedItems.length > 0 ? renderedItems : items;
}

function focusMenuBoundary(
  menu: HTMLDivElement | null,
  boundary: MenuInitialFocus,
): void {
  const items = enabledMenuItems(menu);
  (items[boundary === "first" ? 0 : items.length - 1] ?? menu)?.focus();
}

function handleMenuKeyDown(
  event: React.KeyboardEvent<HTMLDivElement>,
  menu: HTMLDivElement | null,
  closeMenu: () => void,
  trigger: HTMLButtonElement | null,
): void {
  if (!["ArrowDown", "ArrowUp", "Home", "End", "Escape"].includes(event.key)) {
    return;
  }

  event.preventDefault();
  event.stopPropagation();

  if (event.key === "Escape") {
    closeMenu();
    trigger?.focus();
    return;
  }

  const items = enabledMenuItems(menu);
  if (items.length === 0) return;

  const currentIndex = items.indexOf(document.activeElement as HTMLElement);
  let nextIndex = 0;
  if (event.key === "End") {
    nextIndex = items.length - 1;
  } else if (event.key === "ArrowDown") {
    nextIndex = currentIndex < 0 ? 0 : (currentIndex + 1) % items.length;
  } else if (event.key === "ArrowUp") {
    nextIndex = currentIndex < 0
      ? items.length - 1
      : (currentIndex - 1 + items.length) % items.length;
  }
  items[nextIndex]?.focus();
}

export default function PageDetail({
  pageId,
  projectionIssue,
  onProjectionResolved,
  initialMode = "read",
  initialSelection,
  onSelectionChange,
  onEditorReady,
  onBack,
  onDeleted,
  onMemoryClick,
  onPageClick,
  onEntityClick,
  onOpenGraph,
  onOpenReview,
  onDismissAttachedPageNotice,
  onPageLoaded,
  onSavePendingChange,
  onEditDirtyChange,
  onRegisterFlush,
  showAttachedPageNotice = false,
}: PageDetailProps) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [wikiLinkPreview, setWikiLinkPreview] = useState<ReferencePreviewRequest | null>(null);
  const wikiLinkPreviewLeaveTimerRef = useRef<number | null>(null);
  const [moveOpen, setMoveOpen] = useState(false);
  const [exported, setExported] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [editing, setEditing] = useState(false);
  const [renaming, setRenaming] = useState(false);
  const [renameTitle, setRenameTitle] = useState("");
  const [renameExpectedVersion, setRenameExpectedVersion] = useState<number | null>(null);
  const [renameError, setRenameError] = useState(false);
  const [renameNeedsReview, setRenameNeedsReview] = useState(false);
  const [infoOpen, setInfoOpen] = useState(false);
  const [sourcePreview, setSourcePreview] = useState<IndexedFileInfo | null>(null);
  useEffect(() => setSourcePreview(null), [pageId]);
  const [canvasWorkspaceExpanded, setCanvasWorkspaceExpanded] = useState(false);
  const [canvasWorkspaceHost, setCanvasWorkspaceHost] = useState<HTMLDivElement | null>(null);
  const [canvasWorkspaceSupported, setCanvasWorkspaceSupported] = useState(false);
  const pageDetailRootRef = useRef<HTMLDivElement | null>(null);
  const canvasWorkspaceExpandedRef = useRef(false);
  const canvasWorkspaceScrollRef = useRef<{ pageId: string; element: HTMLElement; scrollTop: number } | null>(null);
  const canvasWorkspaceRestoreRef = useRef<{ pageId: string; element: HTMLElement; scrollTop: number } | null>(null);
  const inspectorId = useId();
  const documentToolsHost = useContext(WorkspaceDocumentToolsHostContext);
  const noteGroup = useContext(WorkspaceNoteGroupContext);
  const setPageDetailRootRef = useCallback((node: HTMLDivElement | null) => {
    pageDetailRootRef.current = node;
    setCanvasWorkspaceSupported(node?.parentElement?.closest(".wiki-workspace-content") != null);
  }, []);
  const changeCanvasWorkspaceExpanded = useCallback((expanded: boolean) => {
    if (expanded) {
      const wikiWorkspace = pageDetailRootRef.current?.parentElement?.closest<HTMLElement>(".wiki-workspace-content");
      canvasWorkspaceScrollRef.current = wikiWorkspace
        ? { pageId, element: wikiWorkspace, scrollTop: wikiWorkspace.scrollTop }
        : null;
      canvasWorkspaceRestoreRef.current = null;
    } else if (canvasWorkspaceExpandedRef.current) {
      canvasWorkspaceRestoreRef.current = canvasWorkspaceScrollRef.current;
    }
    canvasWorkspaceExpandedRef.current = expanded;
    setCanvasWorkspaceExpanded(expanded);
  }, [pageId]);
  canvasWorkspaceExpandedRef.current = canvasWorkspaceExpanded;
  useLayoutEffect(() => {
    if (canvasWorkspaceScrollRef.current?.pageId !== pageId) {
      canvasWorkspaceScrollRef.current = null;
      canvasWorkspaceRestoreRef.current = null;
    }
  }, [pageId]);
  useLayoutEffect(() => {
    if (canvasWorkspaceExpanded) return;
    const snapshot = canvasWorkspaceRestoreRef.current;
    canvasWorkspaceRestoreRef.current = null;
    canvasWorkspaceScrollRef.current = null;
    if (!snapshot || snapshot.pageId !== pageId || !snapshot.element.isConnected) return;
    snapshot.element.scrollTop = snapshot.scrollTop;
  }, [canvasWorkspaceExpanded, pageId]);
  const [editDirty, setEditDirty] = useState(false);
  const [editInitialDocument, setEditInitialDocument] = useState("");
  const [editHasMatchingTitle, setEditHasMatchingTitle] = useState(false);
  const [editGate, setEditGate] = useState<PageEditGate>({ kind: "closed" });
  const [editBaseline, setEditBaseline] = useState<PageEditBaseline | null>(null);
  const [sourceProfile, setSourceProfile] =
    useState<MarkdownSourceProfile | null>(null);
  const [saveState, setSaveState] = useState<PageSaveCoordinatorState>({
    phase: "idle",
  });
  const [conflictLatest, setConflictLatest] =
    useState<ConflictLatestSource>({ kind: "idle" });
  const [editValidation, setEditValidation] = useState<string | null>(null);
  const [editorFallbackSessionId, setEditorFallbackSessionId] =
    useState<string | null>(null);
  const [editorShortcutModifier, setEditorShortcutModifier] =
    useState<"Cmd" | "Ctrl">(localShortcutModifier);
  const [editorStatus, setEditorStatus] = useState<MarkdownEditorStatus>({
    sessionId: "",
    engine: "loading",
    ready: false,
    compositionActive: false,
    canUndo: false,
    canRedo: false,
  });
  const [editorSessionToken, setEditorSessionToken] = useState<{
    id: string;
    epoch: number;
  } | null>(null);
  const editorFocusIntentRef = useRef<{
    pageId: string;
    beginEditAttempt: number;
    epoch: number | null;
    focus: boolean;
  } | null>(null);
  const focusedEditorSessionRef = useRef<number | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  // Set only alongside a delete-blocked-by-entity actionError, so the "Open
  // the entity" link renders next to that message and nowhere else.
  const [deleteGuardEntityId, setDeleteGuardEntityId] = useState<string | null>(null);
  const setActionErrorMessage = (message: string | null, entityId: string | null = null) => {
    setActionError(message);
    setDeleteGuardEntityId(entityId);
  };
  const [actionMenuOpen, setActionMenuOpen] = useState(false);
  const [redistillNotice, setRedistillNotice] = useState<{
    kind: "success" | "warning" | "error";
    message: string;
  } | null>(null);
  const [reviewNotice, setReviewNotice] = useState<PageReviewNotice | null>(null);
  const basisActions = useBasisActions();
  const codeBasisActions = useCodeBasisActions();
  // "Edit it yourself" on a changed basis: the next save also confirms it. The
  // ref is what the (once-built) autosave writer reads; the state drives the UI.
  const confirmIntentRef = useRef<{ pageId: string; basis: BasisRef } | null>(null);
  // Opening the review waits for the stored action to finish: while it is
  // pending, the shell refuses to navigate away from this note.
  const openReviewAfterActionRef = useRef<{ pageId: string; itemId: string } | null>(null);
  const takeReviewToOpen = () => {
    const pending = openReviewAfterActionRef.current;
    openReviewAfterActionRef.current = null;
    return pending;
  };
  const basisSaveOutcomeRef = useRef<(outcome: BasisConfirmOutcome) => void>(() => undefined);
  // The same for changed code: the next save of this note also confirms these lines.
  const codeConfirmIntentRef = useRef<{ pageId: string; row: CodeBasisRef } | null>(null);
  const codeSaveConfirmRef = useRef<(intent: { pageId: string; row: CodeBasisRef }) => void>(() => undefined);
  const [canvasOpen, setCanvasOpen] = useState(false);
  const [canvasSwitchPending, setCanvasSwitchPending] = useState(false);
  const [storedActionPending, setStoredActionPending] = useState(false);
  const storedActionRef = useRef<object | null>(null);
  const canvasSwitchAttemptRef = useRef(0);
  const canvasSwitchPendingRef = useRef(false);
  const canvasReturnToEditorRef = useRef(false);
  const mountedRef = useRef(false);
  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      storedActionRef.current = null;
      canvasSwitchAttemptRef.current += 1;
    };
  }, []);
  const showCanvas = canvasOpen && !editing;
  const editorRef = useRef<MarkdownEditorHandle>(null);
  const editDocumentRef = useRef("");
  const editDirtyRef = useRef(false);
  const editPageTitleRef = useRef("");
  const beginEditAttemptRef = useRef(0);
  const renameAttemptRef = useRef(0);
  const autoEditIntentRef = useRef<{ pageId: string; mode: "read" | "edit" } | null>(null);
  const editorSessionEpochRef = useRef(0);
  const activeEditorSessionRef = useRef<{
    id: string;
    epoch: number;
  } | null>(null);
  const saveStateRef = useRef<PageSaveCoordinatorState>({ phase: "idle" });
  const autosaveRef = useRef<PageAutosave | null>(null);
  const editorPageRef = useRef<Page | null>(null);
  const backAttemptRef = useRef(0);
  const actionMenuTriggerRef = useRef<HTMLButtonElement>(null);
  const inspectorToggleRef = useRef<HTMLButtonElement>(null);
  const actionMenuRef = useRef<HTMLDivElement>(null);
  const actionMenuListRef = useRef<HTMLDivElement>(null);
  const actionMenuInitialFocusRef = useRef<MenuInitialFocus>("first");
  // Mouse opens skip the focus ring; keyboard opens keep it, like the Memory menu.
  const [actionMenuKeyboardNavigation, setActionMenuKeyboardNavigation] = useState(false);
  const activePageIdRef = useRef(pageId);
  activePageIdRef.current = pageId;
  // Canonical versions advance during autosave without remounting the editor.
  const editorSessionId = editorSessionToken?.id ?? null;
  const editorSessionEpoch = editorSessionToken?.epoch ?? 0;

  const {
    data: canonicalPage,
    isLoading,
    isLoadingError: pageLoadFailed,
    isFetching: pageIsFetching,
    refetch: refetchPage,
  } = useQuery({
    queryKey: ["page", pageId],
    queryFn: () => getPage(pageId, "explicit"),
    ...EXPLICIT_BROWSE_QUERY_POLICY,
  });
  // A remote deletion must not unmount the only copy of the local document.
  const page = canonicalPage ?? (
    editing && editorPageRef.current?.id === pageId ? editorPageRef.current : null
  );
  const { cutoverLive } = useTruthStatus();

  // Fails closed on purpose, exactly like `useDaemonVersion`: an unreachable
  // daemon leaves this undefined, and undefined must read as "not available"
  // rather than "unknown, offer it anyway". The backend answers with WHY it is
  // unavailable, because the two reasons send you to different places — one is
  // fixed by upgrading the daemon, the other cannot be fixed here at all.
  const { data: reviewAvailability } = useQuery({
    queryKey: ["page-review-supported"],
    queryFn: pageReviewSupported,
    staleTime: 60_000,
    retry: 1,
  });
  const reviewSupported = reviewAvailability === "ready";
  const reviewUnavailableReason =
    reviewAvailability === "platform_unsupported"
      ? t("pageDetail.reviewUnsupportedPlatform")
      : t("pageDetail.reviewUnsupported");

  useEffect(() => {
    if (page == null) return;
    onPageLoaded?.({ id: page.id, status: page.status, title: page.title });
  }, [onPageLoaded, page?.id, page?.status, page?.title]);

  const { data: pageLinks } = useQuery({
    queryKey: ["page-links", pageId],
    queryFn: () => getPageLinks(pageId),
    enabled: !!pageId,
    staleTime: 30_000,
    retry: false,
  });

  const { data: pageRevisions, isFetching: revisionsLoading, isError: revisionsError, refetch: refetchRevisions } = useQuery({
    queryKey: ["page-revisions", pageId],
    queryFn: () => getPageRevisions(pageId),
    enabled: !!pageId,
    staleTime: 30_000,
    retry: false,
  });

  const outboundTargetByLabel = useMemo(() => {
    const map = new Map<string, { id: string; title?: string | null }>();
    for (const link of pageLinks?.outbound ?? []) {
      if (link.target_page_id) {
        map.set(normalizeLinkLabel(link.label), { id: link.target_page_id, title: link.target_title });
      }
    }
    return map;
  }, [pageLinks]);
  // Notes linked from the `[[` menu render as links before the save comes back.
  const [pickedLinkTargets, setPickedLinkTargets] = useState<{ pageId: string; targets: ReadonlyMap<string, string> }>({ pageId, targets: new Map() });
  const editorWikiLinkTargets = useMemo(() => new Map([
    ...(pickedLinkTargets.pageId === pageId ? pickedLinkTargets.targets : []),
    ...[...outboundTargetByLabel]
      .filter(([, target]) => target.id !== pageId)
      .map(([label, target]) => [label, target.id] as const),
  ]), [outboundTargetByLabel, pageId, pickedLinkTargets]);

  // `[[Title]]` names a note in the page's own Space and `[[Space/Title]]` one
  // in another, so the `[[` menu reads every Space.
  const linkSpace = page?.space ?? null;
  const { data: linkCandidates } = useQuery({
    // Under ["pages"] so every rename and delete that refreshes the library refreshes this too.
    queryKey: ["pages", "link-notes", linkSpace],
    queryFn: () => loadAllLinkCandidates(linkSpace),
    enabled: editing && !!page,
    staleTime: 30_000,
    retry: false,
  });
  const linkNotes = useMemo((): LinkCompletionNote[] => (
    page ? linkNotesFor({ page, ownSpace: linkSpace, spaces: linkCandidates ?? [] }) : []
  ), [linkCandidates, linkSpace, page]);

  const showWikiLinkPreview = useCallback((targetId: string, anchor: HTMLAnchorElement, keyboard: boolean) => {
    if (wikiLinkPreviewLeaveTimerRef.current !== null) {
      window.clearTimeout(wikiLinkPreviewLeaveTimerRef.current);
      wikiLinkPreviewLeaveTimerRef.current = null;
    }
    setWikiLinkPreview({ target: { kind: "page", id: targetId }, anchor, keyboard });
  }, []);
  const showReferencePreview = useCallback((target: ReferenceTarget, anchor: HTMLAnchorElement, keyboard: boolean) => {
    if (wikiLinkPreviewLeaveTimerRef.current !== null) window.clearTimeout(wikiLinkPreviewLeaveTimerRef.current);
    wikiLinkPreviewLeaveTimerRef.current = null;
    setWikiLinkPreview({ target, anchor, keyboard });
  }, []);
  const leaveWikiLinkPreview = useCallback(() => {
    if (wikiLinkPreviewLeaveTimerRef.current !== null) window.clearTimeout(wikiLinkPreviewLeaveTimerRef.current);
    wikiLinkPreviewLeaveTimerRef.current = window.setTimeout(() => {
      wikiLinkPreviewLeaveTimerRef.current = null;
      setWikiLinkPreview(null);
    }, 120);
  }, []);
  const dismissWikiLinkPreview = useCallback(() => {
    if (wikiLinkPreviewLeaveTimerRef.current !== null) window.clearTimeout(wikiLinkPreviewLeaveTimerRef.current);
    wikiLinkPreviewLeaveTimerRef.current = null;
    setWikiLinkPreview(null);
  }, []);
  useEffect(() => {
    dismissWikiLinkPreview();
    return () => {
      if (wikiLinkPreviewLeaveTimerRef.current !== null) window.clearTimeout(wikiLinkPreviewLeaveTimerRef.current);
    };
  }, [dismissWikiLinkPreview, editing, pageId]);

  const { data: registeredSources = [] } = useQuery({
    queryKey: ["registeredSources"],
    queryFn: () => listRegisteredSources(),
    staleTime: 30000,
  });

  const obsidianSources = useMemo(
    () => registeredSources.filter((s) => s.source_type === "obsidian"),
    [registeredSources],
  );

  const { data: pageSources, isPending: pageSourcesLoading } = useQuery({
    queryKey: ["page-sources", pageId],
    queryFn: () => getPageSources(pageId),
    enabled: !!pageId,
  });

  // Imported source memories can be read alongside this note. Ordinary
  // memories retain their existing detail navigation and editing behavior.
  const openSourceMemory = (sourceId: string) => {
    const memory = pageSources?.find((row) => row.source.memory_source_id === sourceId)?.memory;
    if (memory && ["folder", "obsidian", "logseq"].includes(memory.source_agent ?? "")) {
      if (showCanvas) void requestToggleCanvas();
      changeCanvasWorkspaceExpanded(false);
      setSourcePreview({
        source: "memory",
        source_id: memory.source_id,
        title: memory.title,
        chunk_count: memory.chunk_count,
        last_modified: memory.last_modified,
      });
    } else {
      onMemoryClick(sourceId);
    }
  };

  const editorReferenceContext = useMemo(() => ({
    citations: page?.citations,
    sourceMemories: new Map((pageSources ?? []).flatMap((source) => source.memory
      ? [[source.source.memory_source_id, source.memory] as const] : [])),
    sourcesLoading: pageSourcesLoading,
    labels: { page: t("references.page"), memory: t("citation.kind.memory"), source: t("references.source"),
      authored: t("citation.kind.authored"), unverified: t("citation.unverified") },
  }), [page?.citations, pageSources, pageSourcesLoading, t]);
  useEffect(() => {
    // A save can clear or replace the evidence map. Never keep a stale citation open.
    setWikiLinkPreview((current) => current?.target.kind === "citation" ? null : current);
  }, [page?.citations]);

  // Entities on this page = the page's own anchor entity plus the anchor
  // entities of its source memories. These are enrichment links, not search.
  const pageEntityIds = useMemo(() => {
    const ids = new Set<string>();
    if (page?.entity_id) ids.add(page.entity_id);
    for (const s of pageSources ?? []) {
      if (s.memory?.entity_id) ids.add(s.memory.entity_id);
    }
    return [...ids];
  }, [page?.entity_id, pageSources]);

  const entityQueries = useQueries({
    queries: pageEntityIds.map((id) => ({
      queryKey: ["entityDetail", id],
      queryFn: () => getEntityDetail(id),
      staleTime: 60_000,
      retry: false,
    })),
  });
  const pageEntities = entityQueries
    .map((q) => q.data?.entity)
    .filter((e): e is Entity => !!e);

  // Canvas nodes arrive with label: null for memory/entity/page refs — the
  // daemon stores the reference, the client renders the backing object. Every
  // name below comes from a query this component already runs; the canvas
  // must not fetch anything of its own.
  const entitySignature = pageEntities
    .map((e) => `${e.id}\u0000${e.name}`)
    .join("|");
  const labelOverrides = useMemo(() => {
    const overrides = new Map<string, string>();
    if (page?.title) overrides.set(`page:${pageId}`, page.title);
    for (const cs of pageSources ?? []) {
      const memory = cs.memory;
      if (!memory) continue;
      const text = (memory.title || memory.summary || memory.content || "").trim();
      if (!text) continue;
      overrides.set(
        `memory:${memory.source_id}`,
        text.length > 64 ? `${text.slice(0, 64).trimEnd()}\u2026` : text,
      );
    }
    for (const entity of pageEntities) {
      overrides.set(`entity:${entity.id}`, entity.name);
    }
    return overrides;
    // pageEntities is rebuilt every render by useQueries; entitySignature is
    // the stable stand-in that actually tracks its content.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pageId, page?.title, pageSources, entitySignature]);

  useEffect(() => {
    renameAttemptRef.current += 1;
    setRenaming(false);
    setRenameExpectedVersion(null);
    setRenameError(false);
    setRenameNeedsReview(false);
    storedActionRef.current = null;
    setStoredActionPending(false);
    autosaveRef.current?.reset(null);
    editorPageRef.current = null;
    autoEditIntentRef.current = null;
    confirmIntentRef.current = null;
    setRedistillNotice(null);
    setReviewNotice(null);
    setActionErrorMessage(null);
    setExported(false);
    // Inspector open/selected state belongs to the workspace, not this note.
    setCanvasSwitchPending(false);
    canvasSwitchPendingRef.current = false;
    canvasReturnToEditorRef.current = false;
    canvasSwitchAttemptRef.current += 1;
    setEditing(false);
    setEditGate({ kind: "closed" });
    setEditBaseline(null);
    setEditHasMatchingTitle(false);
    setSourceProfile(null);
    setEditValidation(null);
    setEditorFallbackSessionId(null);
    setEditorSessionToken(null);
    activeEditorSessionRef.current = null;
    beginEditAttemptRef.current += 1;
    editorSessionEpochRef.current += 1;
    editDocumentRef.current = "";
    editDirtyRef.current = false;
    editPageTitleRef.current = "";
    saveStateRef.current = { phase: "idle" };
    setSaveState({ phase: "idle" });
  }, [pageId]);

  const updateSaveState = useCallback((next: PageSaveCoordinatorState) => {
    saveStateRef.current = next;
    setSaveState(next);
  }, []);

  useEffect(() => {
    onSavePendingChange?.(saveState.phase === "pending");
  }, [onSavePendingChange, saveState.phase]);

  useEffect(
    () => () => {
      if (saveStateRef.current.phase !== "pending") {
        onSavePendingChange?.(false);
      }
    },
    [onSavePendingChange],
  );

  useEffect(() => {
    onEditDirtyChange?.(editDirtyRef.current);
    return () => onEditDirtyChange?.(false);
  }, [onEditDirtyChange]);

  const updateEditDirty = useCallback(
    (dirty: boolean) => {
      if (editDirtyRef.current === dirty) return;
      editDirtyRef.current = dirty;
      setEditDirty(dirty);
      onEditDirtyChange?.(dirty);
    },
    [onEditDirtyChange],
  );

  if (!autosaveRef.current) {
    autosaveRef.current = new PageAutosave({
      write: async (input) => {
        // The confirm intent belongs to the note it was chosen on.
        const intent = confirmIntentRef.current?.pageId === input.id ? confirmIntentRef.current : null;
        const outcome = await updatePage({
          ...input,
          ...(intent && intent.basis.to_version != null
            ? { confirmBasis: { basis_page_id: intent.basis.page_id, version: intent.basis.to_version } }
            : {}),
        });
        if (outcome.outcome === "saved" && intent && outcome.basisConfirm) {
          basisSaveOutcomeRef.current(outcome.basisConfirm);
        }
        const codeIntent = codeConfirmIntentRef.current;
        if (outcome.outcome === "saved" && codeIntent?.pageId === input.id) {
          codeSaveConfirmRef.current(codeIntent);
        }
        return outcome;
      },
      read: (id) => getPage(id, "explicit"),
      operationId: createPageOperationId,
      onChange: () => {},
      onMissing: (id) => { queryClient.setQueryData(["page", id], null); },
      onCanonical: (canonical) => {
        queryClient.setQueryData(["page", canonical.id], canonical);
        void queryClient.invalidateQueries({ queryKey: ["pages"] });
        void queryClient.invalidateQueries({ queryKey: ["knowledge-graph"] });
        void queryClient.invalidateQueries({ queryKey: ["page-links", canonical.id] });
        void queryClient.invalidateQueries({ queryKey: ["page-revisions", canonical.id] });
      },
    });
  }
  const autosave = autosaveRef.current;
  useLayoutEffect(() => {
    autosave.setObserver((snapshot) => {
      updateSaveState(snapshot.state);
      setEditBaseline(snapshot.baseline);
      updateEditDirty(snapshot.dirty);
      setEditValidation(snapshot.validation ? t("pageDetail.editor.empty") : null);
      if (snapshot.state.phase === "conflict") {
        setConflictLatest(snapshot.latest
          ? { kind: "loaded", operationId: snapshot.state.pending.operationId, page: snapshot.latest }
          : { kind: "error", operationId: snapshot.state.pending.operationId });
      }
    });
  }, [autosave, t, updateEditDirty, updateSaveState]);
  useEffect(() => () => autosave.dispose(), [autosave]);

  const flushEditor = useCallback(() => storedActionRef.current
    ? Promise.resolve(false)
    : autosave.flush(), [autosave]);
  useEffect(() => {
    onRegisterFlush?.(flushEditor);
    return () => onRegisterFlush?.(null);
  }, [flushEditor, onRegisterFlush, pageId]);
  useEffect(() => {
    const protectUnsavedPage = (event: BeforeUnloadEvent) => {
      if (!autosave.snapshot().dirty) return;
      event.preventDefault();
      event.returnValue = "";
    };
    window.addEventListener("beforeunload", protectUnsavedPage);
    return () => window.removeEventListener("beforeunload", protectUnsavedPage);
  }, [autosave]);

  const fetchConflictLatest = useCallback(
    async (operationId: string): Promise<Page | null> => {
      setConflictLatest((currentLatest) =>
        currentLatest.kind === "loaded" &&
        currentLatest.operationId === operationId
          ? currentLatest
          : { kind: "loading", operationId },
      );
      try {
        const latest = await autosave.readConflictLatest(operationId);
        const current = saveStateRef.current;
        if (
          current.phase !== "conflict" ||
          current.pending.operationId !== operationId
        ) {
          return null;
        }
        // Feed both recovery reads and any newer cache observation into the
        // autosaver, so typing cannot restore an older/null conflict preview.
        autosave.observeConflictLatest(latest, operationId);
        const cached = queryClient.getQueryData<Page | null>(["page", pageId]);
        const baseline = autosave.snapshot().baseline;
        if (cached && baseline && (
          cached.version > baseline.version || cached.content !== baseline.content
        )) autosave.observeConflictLatest(cached, operationId);
        const newest = autosave.snapshot().latest;
        if (!newest) {
          setConflictLatest({ kind: "error", operationId });
          return null;
        }
        queryClient.setQueryData<Page | null>(["page", pageId], (currentPage) =>
          currentPage?.id === pageId && currentPage.version >= newest.version
            ? currentPage
            : newest,
        );
        return newest;
      } catch {
        const current = saveStateRef.current;
        if (
          current.phase === "conflict" &&
          current.pending.operationId === operationId
        ) {
          setConflictLatest((currentLatest) =>
            currentLatest.kind === "loaded" &&
            currentLatest.operationId === operationId
              ? currentLatest
              : { kind: "error", operationId },
          );
        }
        return null;
      }
    },
    [autosave, pageId, queryClient],
  );

  const deleteMutation = useMutation({
    mutationFn: (id: string) => deletePage(id),
    onSuccess: (_result, id) => {
      // The detail remains mounted until navigation completes. Mark its caches
      // stale without immediately requesting a page that was just deleted.
      queryClient.invalidateQueries({ queryKey: ["page", id], refetchType: "none" });
      queryClient.invalidateQueries({ queryKey: ["pages"] });
      queryClient.invalidateQueries({ queryKey: ["knowledge-graph"] });
      queryClient.invalidateQueries({ queryKey: ["page-links", id], refetchType: "none" });
      queryClient.invalidateQueries({ queryKey: ["page-revisions", id], refetchType: "none" });
      queryClient.invalidateQueries({ queryKey: ["page-sources", id], refetchType: "none" });
      onDeleted?.(id);
      if (activePageIdRef.current !== id) return;
      setActionErrorMessage(null);
      // Release the action gate before Main requests its navigation flush.
      closeEditor();
      storedActionRef.current = null;
      setStoredActionPending(false);
      void requestBack();
    },
    onError: (error, id) => {
      if (activePageIdRef.current !== id) return;
      const guard = parseEntityGuardError(error);
      if (guard) setActionErrorMessage(guard.message, guard.entityId);
      else setActionErrorMessage(t("pageDetail.deleteError"));
    },
  });

  const renameMutation = useMutation({
    mutationFn: (input: { id: string; title: string; expectedVersion: number }) =>
      renamePage(input.id, input.title, input.expectedVersion),
    onSuccess: (renamed, input) => {
      if (renamed.id !== input.id) return;
      queryClient.setQueryData<Page | null>(["page", input.id], (current) =>
        current?.id === renamed.id && current.version > renamed.version
          ? current
          : current?.id === renamed.id
            ? { ...current, title: renamed.title, version: renamed.version }
            : current,
      );
      for (const queryKey of [
        ["pages"],
        ["searchPages"],
        ["search"],
        ["knowledge-graph"],
        ["constellation-cartography"],
        ["recent-pages"],
        ["space-pages"],
        ["spaces-page-counts"],
        ["sidebar-space-page-counts"],
        ["page-links"],
        ["page-revisions", input.id],
        ["page", input.id],
      ]) {
        void queryClient.invalidateQueries({ queryKey });
      }
      if (activePageIdRef.current !== input.id) return;
      renameAttemptRef.current += 1;
      setRenaming(false);
      setRenameExpectedVersion(null);
      setRenameError(false);
      setRenameNeedsReview(false);
    },
    onError: (error, input) => {
      if (activePageIdRef.current !== input.id) return;
      const needsReview = String(error).includes("page_review_required");
      setRenameNeedsReview(needsReview);
      setRenameError(!needsReview);
    },
  });

  const redistillMutation = useMutation({
    mutationFn: (id: string) => redistillPage(id),
    onSuccess: (result, id) => {
      queryClient.invalidateQueries({ queryKey: ["page", id] });
      queryClient.invalidateQueries({ queryKey: ["pages"] });
      queryClient.invalidateQueries({ queryKey: ["knowledge-graph"] });
      queryClient.invalidateQueries({ queryKey: ["page-links", id] });
      queryClient.invalidateQueries({ queryKey: ["page-revisions", id] });
      queryClient.invalidateQueries({ queryKey: ["page-sources", id] });
      if (activePageIdRef.current !== id) return;
      if (result.status === "skipped") {
        setRedistillNotice({
          kind: "warning",
          message: result.hint || "Page re-distill skipped.",
        });
        return;
      }
      if (result.gated) {
        // The rebuild was staged as a preview card rather than applied: say so
        // and open it, because nothing changes until the person confirms.
        void queryClient.invalidateQueries({ queryKey: ["pending-revisions"] });
        setRedistillNotice({ kind: "success", message: t("noteBasis.previewReady") });
        openReviewAfterActionRef.current = { pageId: id, itemId: `revision:${id}` };
        return;
      }
      if (!result.updated && result.reason) {
        // The daemon ran the rebuild and discarded it (citation gate);
        // saying "already up to date" here would hide the failure.
        setRedistillNotice({
          kind: "warning",
          message: t("pageDetail.redistillBlocked", { reason: result.reason }),
        });
        return;
      }
      setRedistillNotice({
        kind: "success",
        message: result.updated ? t("pageDetail.redistilled") : t("pageDetail.redistillUpToDate"),
      });
    },
    onError: (error, id) => {
      if (activePageIdRef.current !== id) return;
      setRedistillNotice({
        kind: "error",
        message: t("pageDetail.redistillFailed", {
          message: error instanceof Error ? error.message : String(error),
        }),
      });
    },
  });

  const reviewMutation = useMutation({
    // The content goes with the request because the mark is bound to the exact
    // text that was on screen, not to the page as a moving target. The backend
    // hashes what it is handed here; if the daemon's copy has since changed,
    // the answer is `stale` and the reader is asked to look again.
    mutationFn: ({ id, content }: { id: string; content: string }) => reviewPage(id, content),
    onSuccess: (outcome: PageReviewOutcome, { id }) => {
      if (outcome.kind === "applied") {
        queryClient.invalidateQueries({ queryKey: ["page", id] });
        // The wiki list renders the trust badges, so it is stale now too.
        queryClient.invalidateQueries({ queryKey: ["pages"] });
        queryClient.invalidateQueries({ queryKey: ["knowledge-graph"] });
      }
      if (activePageIdRef.current !== id) return;
      setReviewNotice(pageReviewNotice(outcome, t));
    },
    onError: (_error, { id }) => {
      if (activePageIdRef.current !== id) return;
      setReviewNotice({
        kind: "error",
        message: t("pageDetail.reviewFailed"),
        offerReload: false,
      });
    },
  });

  const reviewRequiredMutation = useMutation({
    mutationFn: ({ id, required }: { id: string; required: boolean }) => setPageReviewRequired(id, required),
    onSuccess: (receipt, { id }) => {
      // The receipt is the daemon's answer; write it into the cached page so
      // the menu and notice flip at once, then let the refetch confirm it.
      queryClient.setQueryData<Page | null>(["page", id], (current) => {
        if (!current) return current;
        const { review_required: _previous, ...rest } = current;
        return receipt.review_required ? { ...rest, review_required: true } : rest;
      });
      queryClient.invalidateQueries({ queryKey: ["page", id] });
      queryClient.invalidateQueries({ queryKey: ["pages"] });
    },
    onError: (_error, { id }) => {
      if (activePageIdRef.current !== id) return;
      setActionErrorMessage(t("pageDetail.requireReviewFailed"));
    },
  });

  const handleExportToVault = useCallback(
    async (vaultPath: string) => {
      const originPageId = pageId;
      setActionErrorMessage(null);
      setExporting(true);
      try {
        await exportPageToObsidian(originPageId, `${vaultPath}/Wenlan/pages`);
        if (activePageIdRef.current !== originPageId) return;
        setExported(true);
        setTimeout(() => setExported(false), 2000);
      } catch {
        if (activePageIdRef.current !== originPageId) return;
        setActionErrorMessage(t("pageDetail.exportError"));
      } finally {
        setExporting(false);
      }
    },
    [pageId, t],
  );

  const clearConfirmIntent = () => {
    confirmIntentRef.current = null;
    codeConfirmIntentRef.current = null;
  };
  codeSaveConfirmRef.current = (intent) => {
    // Once the edit is saved the person has accounted for the code: confirm it.
    codeConfirmIntentRef.current = null;
    void codeBasisActions.confirm(intent.pageId, intent.row);
  };
  basisSaveOutcomeRef.current = (outcome) => {
    const intent = confirmIntentRef.current;
    if (!intent || outcome === "not_saved") return;
    clearConfirmIntent();
    if (outcome === "confirmed") basisActions.refresh(intent.pageId);
    else basisActions.noteChangedAgain(intent.pageId, intent.basis.page_id);
  };

  const closeEditor = () => {
    clearConfirmIntent();
    canvasSwitchAttemptRef.current += 1;
    canvasSwitchPendingRef.current = false;
    setCanvasSwitchPending(false);
    autosave.reset(null);
    editorPageRef.current = null;
    activeEditorSessionRef.current = null;
    beginEditAttemptRef.current += 1;
    editorFocusIntentRef.current = null;
    focusedEditorSessionRef.current = null;
    editorSessionEpochRef.current += 1;
    editDocumentRef.current = "";
    editPageTitleRef.current = "";
    setEditing(false);
    setEditGate({ kind: "closed" });
    setEditBaseline(null);
    setEditHasMatchingTitle(false);
    setSourceProfile(null);
    setEditValidation(null);
    setEditorFallbackSessionId(null);
    setEditorSessionToken(null);
    setEditorStatus({
      sessionId: "",
      engine: "loading",
      ready: false,
      compositionActive: false,
      canUndo: false,
      canRedo: false,
    });
    setConflictLatest({ kind: "idle" });
    updateSaveState({ phase: "idle" });
    updateEditDirty(false);
  };

  const openPageSourceForEditing = (sourcePage: Page, automatic = false) => {
    if (activePageIdRef.current !== sourcePage.id) return;

    editorFocusIntentRef.current = {
      pageId: sourcePage.id,
      beginEditAttempt: beginEditAttemptRef.current,
      epoch: null,
      focus: !automatic,
    };
    autosave.reset(sourcePage);
    editorPageRef.current = sourcePage;
    const prepared = prepareMarkdownSource(sourcePage.content);
    const sessionId = `${sourcePage.id}:${sourcePage.version}`;
    editorSessionEpochRef.current += 1;
    const sessionToken = {
      id: sessionId,
      epoch: editorSessionEpochRef.current,
    };
    editorFocusIntentRef.current = {
      pageId: sourcePage.id,
      beginEditAttempt: beginEditAttemptRef.current,
      epoch: sessionToken.epoch,
      focus: !automatic,
    };
    activeEditorSessionRef.current = sessionToken;
    setEditorSessionToken(sessionToken);
    setEditorStatus({
      sessionId,
      engine: "loading",
      ready: false,
      compositionActive: false,
      canUndo: false,
      canRedo: false,
    });
    editDocumentRef.current = prepared.editorDocument;
    editPageTitleRef.current = sourcePage.title;
    updateEditDirty(false);
    setEditHasMatchingTitle(
      leadingMarkdownH1MatchesTitle(
        prepared.editorDocument,
        sourcePage.title,
      ),
    );
    setEditBaseline({
      pageId: sourcePage.id,
      content: sourcePage.content,
      version: sourcePage.version,
    });
    setEditValidation(null);
    setEditorFallbackSessionId(null);
    setConflictLatest({ kind: "idle" });
    updateSaveState({ phase: "idle" });
    if (!prepared.canEditLosslessly) {
      if (automatic) {
        closeEditor();
        setActionErrorMessage(`${t("pageDetail.editor.normalizeTitle")} ${t("pageDetail.editor.normalizeDescription")}`);
        return;
      }
      setSourceProfile(null);
      setEditGate({ kind: "normalize", prepared });
      return;
    }

    setEditInitialDocument(prepared.editorDocument);
    setSourceProfile(prepared.profile);
    setEditGate({ kind: "editor" });
  };

  const beginEditing = async (automatic = false) => {
    if (!page || renaming || renameMutation.isPending) return;
    setCanvasOpen(false);
    canvasReturnToEditorRef.current = false;
    const originPageId = page.id;
    const beginEditAttempt = ++beginEditAttemptRef.current;
    editorFocusIntentRef.current = {
      pageId: originPageId,
      beginEditAttempt,
      epoch: null,
      focus: !automatic,
    };
    const isActiveBeginEdit = () =>
      activePageIdRef.current === originPageId &&
      beginEditAttemptRef.current === beginEditAttempt;

    setActionErrorMessage(null);
    setEditing(true);
    setActionMenuOpen(false);
    setEditGate({ kind: "checking" });
    setEditValidation(null);
    updateSaveState({ phase: "idle" });

    let version: string;
    try {
      const [reportedVersion, systemInfo] = await Promise.all([
        getDaemonVersion(),
        getSystemInfo().catch(() => null),
      ]);
      if (!isActiveBeginEdit()) return;

      version = reportedVersion;
      if (systemInfo?.os) {
        setEditorShortcutModifier(
          systemInfo.os.toLowerCase() === "macos" ? "Cmd" : "Ctrl",
        );
      }
    } catch {
      if (!isActiveBeginEdit()) return;

      if (automatic) {
        closeEditor();
        setActionErrorMessage(t("pageDetail.editor.upgradeRequired", {
          floor: PAGE_EDIT_DAEMON_FLOOR,
          version: t("pageDetail.editor.unavailableVersion"),
        }));
      } else {
        setEditGate({ kind: "unsupported", version: null });
      }
      void recordPageEditorDiagnostic({
        event: "daemon_floor_blocked",
        reportedVersion: null,
        requiredFloor: PAGE_EDIT_DAEMON_FLOOR,
      }).catch(() => undefined);
      return;
    }
    if (!isActiveBeginEdit()) return;

    if (!daemonMeetsFloor(version, PAGE_EDIT_DAEMON_FLOOR)) {
      if (automatic) {
        closeEditor();
        setActionErrorMessage(t("pageDetail.editor.upgradeRequired", {
          floor: PAGE_EDIT_DAEMON_FLOOR,
          version,
        }));
      } else {
        setEditGate({ kind: "unsupported", version });
      }
      void recordPageEditorDiagnostic({
        event: "daemon_floor_blocked",
        reportedVersion: version,
        requiredFloor: PAGE_EDIT_DAEMON_FLOOR,
      }).catch(() => undefined);
      return;
    }

    if (!isActiveBeginEdit()) return;
    openPageSourceForEditing(page, automatic);
  };

  useEffect(() => {
    if (initialMode === "read") {
      autoEditIntentRef.current = { pageId, mode: "read" };
      return;
    }
    if (!page || page.id !== pageId || page.status !== "active" || canvasOpen) return;
    const attempted = autoEditIntentRef.current;
    if (attempted?.pageId === pageId && attempted.mode === "edit") return;
    autoEditIntentRef.current = { pageId, mode: "edit" };
    void beginEditing(true);
    // The intent key, not the page query object, controls this one-shot gate.
    // Refetches and successful saves must not open or reset the editor.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [initialMode, pageId, page?.id, page?.status, canvasOpen]);

  const handleNormalizeAndEdit = () => {
    if (editGate.kind !== "normalize") return;
    const normalizedProfile: MarkdownSourceProfile = {
      bom: editGate.prepared.profile.bom,
      lineEndings: { kind: "lf", separator: "\n" },
    };
    editDocumentRef.current = editGate.prepared.editorDocument;
    setEditInitialDocument(editGate.prepared.editorDocument);
    setSourceProfile(normalizedProfile);
    setEditGate({ kind: "editor" });
    autosave.setSource(serializeMarkdownSource(editGate.prepared.editorDocument, normalizedProfile));
  };

  const saveDocument = (document: string) => {
    if (!sourceProfile) return;
    editDocumentRef.current = document;
    autosave.setSource(serializeMarkdownSource(document, sourceProfile));
    void autosave.flush();
  };

  const handleDocumentChange = (content: string) => {
    if (content !== editDocumentRef.current) setReviewNotice(null);
    editDocumentRef.current = content;
    setEditHasMatchingTitle(leadingMarkdownH1MatchesTitle(content, editPageTitleRef.current));
    if (sourceProfile) autosave.setSource(serializeMarkdownSource(content, sourceProfile));
  };

  const currentDraftSource = () =>
    sourceProfile
      ? serializeMarkdownSource(editDocumentRef.current, sourceProfile)
      : editDocumentRef.current;

  useLayoutEffect(() => {
    if (editing && editGate.kind === "editor" && canonicalPage !== undefined) {
      autosave.observeCanonical(canonicalPage);
    }
  }, [autosave, editing, editGate.kind, canonicalPage]);

  const requestCloseEditor = async () => {
    if (storedActionRef.current) return;
    const originPageId = pageId;
    if (!await flushEditor() || activePageIdRef.current !== originPageId) return;
    closeEditor();
  };

  const requestToggleCanvas = async () => {
    if (canvasSwitchPendingRef.current) return;
    if (showCanvas) {
      const returnToEditor = canvasReturnToEditorRef.current;
      canvasReturnToEditorRef.current = false;
      setCanvasOpen(false);
      if (returnToEditor) void beginEditing();
      return;
    }
    if (!editing) {
      canvasReturnToEditorRef.current = false;
      setInfoOpen(false);
      setCanvasOpen(true);
      return;
    }

    const originPageId = pageId;
    const originSession = activeEditorSessionRef.current;
    const originBeginAttempt = beginEditAttemptRef.current;
    const attempt = ++canvasSwitchAttemptRef.current;
    canvasSwitchPendingRef.current = true;
    setCanvasSwitchPending(true);
    const isCurrentAttempt = () =>
      mountedRef.current && activePageIdRef.current === originPageId &&
      canvasSwitchAttemptRef.current === attempt &&
      activeEditorSessionRef.current === originSession &&
      beginEditAttemptRef.current === originBeginAttempt;
    try {
      if (!await flushEditor() || !isCurrentAttempt()) return;
      closeEditor();
      canvasReturnToEditorRef.current = true;
      setInfoOpen(false);
      setCanvasOpen(true);
    } finally {
      if (mountedRef.current && canvasSwitchAttemptRef.current === attempt) {
        canvasSwitchPendingRef.current = false;
        setCanvasSwitchPending(false);
      }
    }
  };

  const requestPageInfo = async () => {
    if (canvasSwitchPendingRef.current) return;
    // Keep the same pane mounted while the map yields back to document editing.
    setInfoOpen(true);
    if (showCanvas) {
      // Switching tools closes the map writer before the editor can resume.
      await requestToggleCanvas();
    }
  };

  const selectInspectorTab = (tab: NoteInspectorTab) => {
    if (tab === "canvas") {
      if (!showCanvas) void requestToggleCanvas();
    } else {
      void requestPageInfo();
    }
  };
  const closeInspector = () => {
    if (sourcePreview) {
      setSourcePreview(null);
      inspectorToggleRef.current?.focus({ preventScroll: true });
      return;
    }
    const returningToEditor = showCanvas && canvasReturnToEditorRef.current;
    changeCanvasWorkspaceExpanded(false);
    if (showCanvas) void requestToggleCanvas();
    else setInfoOpen(false);
    // Each group owns its return target. A map opened while writing resumes
    // that editor; otherwise closing the inspector returns to its own toggle.
    if (!returningToEditor) inspectorToggleRef.current?.focus({ preventScroll: true });
  };

  useEffect(() => {
    if (!editing || infoOpen || actionMenuOpen) return;
    const captureUnfocusedEditorEscape = (event: KeyboardEvent) => {
      if (noteGroup && !noteGroup.element?.contains(event.target as Node)) return;
      if (
        event.defaultPrevented ||
        event.isComposing ||
        event.key !== "Escape"
      ) {
        return;
      }
      if (
        event.target instanceof Element &&
        event.target.closest(
          // Navigation owns Escape while its popover or narrow drawer is open.
          // Dismissing those layers must not flush and leave the writing view.
          'input, textarea, select, [contenteditable]:not([contenteditable="false"]), [data-sidebar-escape-scope], [data-sidebar-overlay="true"], [data-page-link-preview], [data-reference-preview], [role="menu"]',
        )
      ) {
        return;
      }
      event.preventDefault();
      event.stopImmediatePropagation();
      requestCloseEditor();
    };
    window.addEventListener("keydown", captureUnfocusedEditorEscape, true);
    return () =>
      window.removeEventListener(
        "keydown",
        captureUnfocusedEditorEscape,
        true,
      );
  }, [editing, infoOpen, actionMenuOpen, requestCloseEditor, noteGroup]);

  const requestBack = async () => {
    // Main owns navigation ordering when the flush handle is registered. Queue
    // the intent there immediately so a later sidebar intent can supersede it.
    if (onRegisterFlush) {
      onBack();
      return;
    }
    const originPageId = pageId;
    const attempt = ++backAttemptRef.current;
    if (!await flushEditor() || activePageIdRef.current !== originPageId || backAttemptRef.current !== attempt) return;
    onBack();
  };

  const handleCopyDraft = async () => {
    await clipboardWrite(currentDraftSource());
  };

  const handleEditorFallback = (
    sessionId: string,
    sessionEpoch: number,
    reason: "load" | "construction",
  ) => {
    const active = activeEditorSessionRef.current;
    if (
      !active ||
      active.id !== sessionId ||
      active.epoch !== sessionEpoch
    ) {
      return;
    }
    setEditorFallbackSessionId(sessionId);
    void recordPageEditorDiagnostic({
      event: "editor_fallback",
      reason,
    }).catch(() => undefined);
  };

  const handleEditorStatus = (
    sessionId: string,
    sessionEpoch: number,
    status: MarkdownEditorStatus,
  ) => {
    const active = activeEditorSessionRef.current;
    if (
      !active ||
      active.id !== sessionId ||
      active.epoch !== sessionEpoch ||
      status.sessionId !== sessionId
    ) {
      return;
    }
    autosave.setComposing(status.compositionActive);
    setEditorStatus(status);
  };

  const handleReloadLatest = async () => {
    if (saveStateRef.current.phase !== "conflict") return;
    if (
      editBaseline &&
      currentDraftSource() !== editBaseline.content &&
      !confirm(t("pageDetail.editor.reloadConfirm"))
    ) {
      return;
    }
    const operationId = saveStateRef.current.pending.operationId;
    if (!autosave.snapshot().latest) await fetchConflictLatest(operationId);
    const current = autosave.snapshot();
    if (
      activePageIdRef.current !== pageId ||
      current.state.phase !== "conflict" ||
      current.state.pending.operationId !== operationId
    ) return;
    const cached = queryClient.getQueryData<Page | null>(["page", pageId]);
    if (cached) autosave.observeConflictLatest(cached, operationId);
    const latest = autosave.snapshot().latest;
    if (!latest) return;
    queryClient.setQueryData<Page | null>(["page", pageId], (currentPage) =>
      currentPage?.id === pageId && currentPage.version >= latest.version
        ? currentPage
        : latest,
    );
    openPageSourceForEditing(latest);
  };

  const handleDiscardDeletedDraft = () => {
    if (
      canonicalPage !== null ||
      autosave.snapshot().state.phase !== "conflict" ||
      !confirm(t("pageDetail.editor.discardConfirm"))
    ) return;
    closeEditor();
  };

  const failureMessage = (kind: UpdatePageFailureKind | "transport") => {
    switch (kind) {
      case "not_found":
        return t("pageDetail.editor.failure.notFound");
      case "auth_required":
        return t("pageDetail.editor.failure.authRequired");
      case "payload_too_large":
        return t("pageDetail.editor.failure.payloadTooLarge");
      case "validation":
        return t("pageDetail.editor.failure.validation");
      case "rate_limited":
        return t("pageDetail.editor.failure.rateLimited");
      case "server":
        return t("pageDetail.editor.failure.server");
      case "transport":
        return t("pageDetail.editor.failure.transport");
      case "other":
        return t("pageDetail.editor.failure.other");
    }
  };

  const requestStoredPageAction = async (kind: "redistill" | "review" | "delete") => {
    if (storedActionRef.current || renaming || renameMutation.isPending || canvasSwitchPendingRef.current || showCanvas ||
        (editing && (editGate.kind !== "editor" || !editorStatus?.ready || editorStatus.compositionActive)) ||
        (kind === "review" && !reviewSupported)) return;
    const originPageId = pageId;
    const originSession = activeEditorSessionRef.current;
    const wasEditing = editing;
    const token = {};
    openReviewAfterActionRef.current = null;
    storedActionRef.current = token;
    setStoredActionPending(true);
    setActionMenuOpen(false);
    const ownsAction = () => mountedRef.current && activePageIdRef.current === originPageId &&
      storedActionRef.current === token;
    let editorClosed = false;
    try {
      // Keep the editor mounted and its draft intact until this exact session
      // has drained. Main's navigation flush remains blocked for this action.
      if (!await autosave.flush() || !ownsAction() ||
          activeEditorSessionRef.current !== originSession) return;
      const snapshot = autosave.snapshot();
      if (snapshot.dirty || snapshot.state.phase !== "idle") return;
      const saved = queryClient.getQueryData<Page | null>(["page", originPageId]) ?? page;
      if (!saved || saved.id !== originPageId) return;
      const content = snapshot.baseline?.content ?? saved.content;
      if (kind === "review") {
        setRedistillNotice(null);
        setReviewNotice(null);
        await reviewMutation.mutateAsync({ id: originPageId, content });
      } else if (kind === "delete") {
        if (!confirm(t("pageDetail.deleteConfirm")) || !ownsAction()) return;
        setActionErrorMessage(null);
        await deleteMutation.mutateAsync(originPageId);
      } else {
        if ((saved.user_edited || pageRevisions?.user_edited) &&
            !confirm(t("pageDetail.redistillEditedConfirm"))) return;
        if (!ownsAction()) return;
        setReviewNotice(null);
        setRedistillNotice(null);
        // A rebuild changes the saved body. Retire the old autosave session
        // before the request; it must never overwrite the rebuilt result.
        if (wasEditing) { closeEditor(); editorClosed = true; }
        await redistillMutation.mutateAsync(originPageId);
      }
    } catch {
      // Mutation callbacks own error notices. Failed flushes leave the draft
      // and their existing save/conflict recovery controls mounted.
    } finally {
      if (editorClosed && ownsAction()) {
        try {
          const fresh = await getPage(originPageId, "explicit");
          if (ownsAction() && fresh === null) queryClient.setQueryData(["page", originPageId], null);
          if (ownsAction() && fresh?.id === originPageId) {
            const cached = queryClient.getQueryData<Page | null>(["page", originPageId]);
            const latest = cached && cached.version > fresh.version ? cached : fresh;
            queryClient.setQueryData(["page", originPageId], latest);
            if (latest.status === "active") {
              setEditing(true);
              openPageSourceForEditing(latest, true);
            }
          }
        } catch {
          // Never reopen a possibly stale source after an uncertain rebuild.
          if (ownsAction()) setActionErrorMessage(t("pageDetail.loadError"));
        }
      }
      if (ownsAction()) {
        storedActionRef.current = null;
        setStoredActionPending(false);
        const reviewToOpen = takeReviewToOpen();
        if (reviewToOpen?.pageId === originPageId) onOpenReview?.(reviewToOpen.itemId);
      }
      takeReviewToOpen();
    }
  };

  const closeActionMenu = () => {
    setActionMenuOpen(false);
    actionMenuTriggerRef.current?.focus();
  };

  const openActionMenu = (initialFocus: MenuInitialFocus) => {
    actionMenuInitialFocusRef.current = initialFocus;
    setActionMenuOpen(true);
  };

  const handleMenuTriggerKeyDown = (
    event: React.KeyboardEvent<HTMLButtonElement>,
    openMenu: (initialFocus: MenuInitialFocus) => void,
  ) => {
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
    event.preventDefault();
    event.stopPropagation();
    openMenu(event.key === "ArrowDown" ? "first" : "last");
  };

  const handlePageDetailKeyDown = (e: React.KeyboardEvent) => {
    if (
      // Portaled context controls still bubble through this React parent.
      // The drawer owns Escape while open; leaving the editor here would
      // consume the key before the drawer's document listener receives it.
      !editing || infoOpen || actionMenuOpen ||
      e.defaultPrevented ||
      e.nativeEvent.isComposing ||
      e.key !== "Escape"
    ) {
      return;
    }
    if (
      e.target instanceof Element &&
      e.target.closest(
        'input, textarea, select, [contenteditable]:not([contenteditable="false"]), [data-page-link-preview]',
      )
    ) {
      return;
    }
    e.preventDefault();
    e.stopPropagation();
    requestCloseEditor();
  };

  useEffect(() => {
    const focusIntent = editorFocusIntentRef.current;
    if (
      editing &&
      editGate.kind === "editor" &&
      editorSessionId &&
      focusIntent?.focus &&
      focusIntent.pageId === pageId &&
      focusIntent.beginEditAttempt === beginEditAttemptRef.current &&
      focusIntent.epoch === editorSessionEpoch &&
      focusedEditorSessionRef.current !== editorSessionEpoch
    ) {
      focusedEditorSessionRef.current = editorSessionEpoch;
      editorRef.current?.focus();
    }
  }, [editing, editGate.kind, editorSessionEpoch, editorSessionId, pageId]);

  useEffect(() => {
    if (!actionMenuOpen) return;
    focusMenuBoundary(actionMenuListRef.current, actionMenuInitialFocusRef.current);
  }, [actionMenuOpen]);

  // Opened while every item was still disabled (editor not ready yet): focus
  // fell back to the menu itself, so move it to an item once one enables.
  useEffect(() => {
    const menu = actionMenuListRef.current;
    if (!actionMenuOpen || !menu || document.activeElement !== menu) return;
    if (enabledMenuItems(menu).length > 0) {
      focusMenuBoundary(menu, actionMenuInitialFocusRef.current);
    }
  });

  useEffect(() => {
    if (!actionMenuOpen) return;
    const closeOnOutsideClick = (event: MouseEvent) => {
      if (!actionMenuRef.current?.contains(event.target as Node)) {
        setActionMenuOpen(false);
      }
    };
    document.addEventListener("mousedown", closeOnOutsideClick);
    return () => {
      document.removeEventListener("mousedown", closeOnOutsideClick);
    };
  }, [actionMenuOpen]);

  if (isLoading && !page) return null;

  if (pageLoadFailed && !page) {
    return (
      <div className="page-detail-load-state">
        <p role="alert">{t("pageDetail.loadError")}</p>
        <button
          disabled={pageIsFetching}
          onClick={() => void refetchPage()}
          type="button"
        >
          {t("pageDetail.retry")}
        </button>
      </div>
    );
  }

  if (!page) {
    return (
      <div className="flex flex-col items-center justify-center gap-2 py-20">
        <span
          style={{
            fontFamily: "var(--mem-font-body)",
            fontSize: "13px",
            color: "var(--mem-text-tertiary)",
          }}
        >
          Page not found
        </span>
        <WorkspaceBackButton
          onClick={requestBack}
          className="transition-colors text-sm"
          style={{ color: "var(--mem-text-secondary)" }}
        >
          Back
        </WorkspaceBackButton>
      </div>
    );
  }

  const sourceCount = pageSources?.length ?? page.source_memory_ids.length;
  const basisRefs = page.basis ?? [];
  // A note that relies on other notes is not rebuilt by itself: the daemon says
  // "blocked: has_basis" for that normal state, which reads as a plain update.
  const datelineBlocked = Boolean(page.refresh_blocked_reason) && page.refresh_blocked_reason !== "has_basis";
  // A note can be rebuilt from its memories or from notes it still relies on.
  const canUpdateFromSources = sourceCount > 0
    || basisRefs.some((entry) => entry.state !== "deleted" && entry.state !== "unavailable");
  const startConfirmEdit = (basis: BasisRef) => {
    confirmIntentRef.current = { pageId: page.id, basis };
    if (!editing) void beginEditing();
  };
  const startCodeConfirmEdit = (row: CodeBasisRef) => {
    codeConfirmIntentRef.current = { pageId: page.id, row };
    if (!editing) void beginEditing();
  };
  const markBasis = async (basisPageId: string): Promise<boolean> => {
    const originPageId = page.id;
    try {
      await markPageBasis(originPageId, basisPageId);
      basisActions.refresh(originPageId);
      void queryClient.invalidateQueries({ queryKey: ["page", basisPageId] });
      return true;
    } catch {
      return false;
    }
  };
  const unmarkBasis = (basisPageId: string) => { void basisActions.stop(page.id, basisPageId); };
  const codeBasisRefs = page.code_basis ?? [];
  const unmarkCodeBasis = (row: CodeBasisRef) => { void codeBasisActions.stop(page.id, row); };
  const pickLink = (note: LinkCompletionNote, asSource: boolean) => {
    const originPageId = page.id;
    setPickedLinkTargets((current) => ({
      pageId: originPageId,
      targets: new Map([...(current.pageId === originPageId ? current.targets : []), [normalizeLinkLabel(linkCompletionText(note)), note.id]]),
    }));
    if (!asSource) return;
    void markBasis(note.id).then((ok) => {
      if (!ok && mountedRef.current && activePageIdRef.current === originPageId) setActionErrorMessage(t("noteBasis.markFailed"));
    });
  };
  // An entity's own page cannot rely on, or be, a note.
  const basisEditable = page.creation_kind !== "entity";

  // Citations first (occurrence counting mirrors the backend and runs over the
  // raw stored body), then the existing display transforms.
  const processed = processCitations(page.content, page.citations);

  // Strip ## Sources (shown in Page info below)
  // Convert [[wikilinks]] to markdown links if they resolve to pages, else plain text
  const renderWikilinks = (content: string) => content.replace(/\[\[([^\]]+)\]\]/g, (_match, inner) => {
    const link = parseWikilink(inner);
    const target = outboundTargetByLabel.get(normalizeLinkLabel(link.targetLabel));
    const cid = target?.id;
    // A `[[Space/Title]]` link reads as written, so it keeps naming the other Space.
    const labelKey = normalizeLinkLabel(link.targetLabel);
    const namesOtherSpace = !!target?.title && labelKey !== normalizeLinkLabel(target.title)
      && labelKey.endsWith(`/${normalizeLinkLabel(target.title)}`);
    const currentTitle = cid === page.id ? page.title : namesOtherSpace ? link.targetLabel : target?.title;
    const displayText = !link.hasAlias && currentTitle
      ? currentTitle.replace(/[\\`*_[\]<>]/g, "\\$&")
      : link.displayText;
    // Self references read as the page's name, not a link back to this page.
    if (cid && cid !== page.id) return `[${displayText}](${PAGE_LINK_ANCHOR_PREFIX}${cid})`;
    return displayText;
  });
  const cleanedContent = renderWikilinks(processed.content)
    .replace(/^#\s+.*\n+/, "")
    .replace(/## Sources\n[\s\S]*?(?=\n## |\s*$)/, "")
    .trim();

  const sourceMemoryByLocator = new Map(
    (pageSources ?? [])
      .filter((cs) => cs.memory !== null)
      .map((cs) => [cs.source.memory_source_id, cs.memory]),
  );

  // Extract the lede (first sentence) for native rendering under title.
  // Match first sentence ending with ". " or ".\n" — but not inside [[wikilinks]] or after abbreviations.
  // Scan starts after any leading heading lines so a body that opens with
  // "## Section" doesn't leak raw markdown into the plain-text lede.
  const leadingHeadings = cleanedContent.match(/^(?:#{1,6}[^\n]*\n+)+/)?.[0].length ?? 0;
  const bodyAfterHeadings = cleanedContent.slice(leadingHeadings);
  const sentenceEnd = bodyAfterHeadings.search(/\.\s/);
  // The first sentence in two forms: markdown, with its citation links intact
  // for the quote, and the plain text a reader sees. A leading "TLDR:" label
  // is not part of the sentence, so it comes off both. The length cap is
  // measured on the plain text, because the markers here have already been
  // rewritten to `[1](#citation:1)` links: a short sentence carrying a few of
  // them was pushed past the cap and dropped out of the lede, which left the
  // quote without its chips and repeated the sentence in the body.
  const firstSentenceMarkdown =
    sentenceEnd > 0 ? stripLedeLabel(bodyAfterHeadings.slice(0, sentenceEnd + 1).trim()) : "";
  const firstSentenceText = stripCitationLinks(firstSentenceMarkdown);
  const tldr =
    firstSentenceText.length > 0 && firstSentenceText.length < 400 ? firstSentenceText : "";
  // The lede shows the page summary when there is one; the first sentence is
  // cut from the body only when it is what the lede shows (no summary, or a
  // summary that repeats it). Cutting it under a different summary dropped
  // that sentence, and its citations, from the page.
  // Markers sit before the period ("... setup [1][2]."), so stripping them
  // leaves " ." behind; drop that space with the period before comparing.
  const normalizeSentence = (s: string) =>
    stripLedeLabel(stripCitationLinks(renderWikilinks(s)))
      .replace(/\[\d+\]/g, "").replace(/\s+/g, " ").trim().replace(/\s*\.$/, "").toLowerCase();
  const summaryMarkdown = page.summary ? renderWikilinks(stripLedeLabel(page.summary)) : "";
  const ledeText = summaryMarkdown || tldr;
  const ledeIsFirstSentence =
    !page.summary || (tldr !== "" && normalizeSentence(page.summary) === normalizeSentence(tldr));
  // When the lede is the first sentence, render that sentence with its
  // citation links so the chips move up with it instead of disappearing.
  const ledeMarkdown = tldr && ledeIsFirstSentence ? firstSentenceMarkdown : summaryMarkdown;
  const displayContent = tldr && ledeIsFirstSentence
    ? (cleanedContent.slice(0, leadingHeadings) + bodyAfterHeadings.slice(sentenceEnd + 1).trimStart()).trim()
    : cleanedContent;

  const renderCitation = (k: number) => {
    const c = processed.byOccurrence.get(k);
    if (!c) return null;
    return (
      <CitationChip
        occurrence={k}
        citation={c}
        sourceMemory={sourceMemoryByLocator.get(c.locator) ?? null}
        sourcesLoading={pageSourcesLoading}
        onOpenMemory={openSourceMemory}
      />
    );
  };

  // Intercept page/memory link clicks in rendered content (capture phase beats target="_blank")
  const handleContentClick = (e: React.MouseEvent) => {
    const anchor = (e.target as HTMLElement).closest("a");
    if (!anchor || anchor.hasAttribute("data-reference-link")) return;
    const href = anchor.getAttribute("href") || "";
    if (href.startsWith(PAGE_LINK_ANCHOR_PREFIX)) {
      e.preventDefault();
      e.stopPropagation();
      onPageClick?.(href.replace(PAGE_LINK_ANCHOR_PREFIX, ""));
    } else if (href.startsWith("#memory:")) {
      e.preventDefault();
      e.stopPropagation();
      onMemoryClick(href.replace("#memory:", ""));
    }
  };

  const outboundLinks = pageLinks?.outbound ?? [];
  const inboundLinks = pageLinks?.inbound ?? [];
  const pageRevisionEntries = pageRevisions?.entries ?? [];

  const storedActionBlocked = storedActionPending || renaming || renameMutation.isPending || showCanvas || canvasSwitchPending ||
    (editing && (editGate.kind !== "editor" || !editorStatus?.ready || editorStatus.compositionActive));
  // PageDetail is reused across pages; a slow request for another page must
  // not disable this page's toggle.
  const reviewLockPending = reviewRequiredMutation.isPending && reviewRequiredMutation.variables?.id === page.id;
  const hasRail = pageEntities.length > 0 || outboundLinks.length > 0;
  const canRenamePage = page.status === "active" &&
    page.creation_kind !== "entity" && page.creation_kind !== "source" &&
    page.creation_kind !== "imported";
  const renameDisabled = storedActionPending || canvasSwitchPending || editing || editDirty || saveState.phase !== "idle" ||
    !!editorStatus?.compositionActive || renameMutation.isPending;

  const startRename = () => {
    if (!canRenamePage || renaming || renameDisabled) return;
    renameAttemptRef.current += 1;
    setRenameTitle(page.title);
    setRenameExpectedVersion(page.version);
    setRenameError(false);
    setRenameNeedsReview(false);
    setRenaming(true);
  };

  const cancelRename = () => {
    if (renameMutation.isPending) return;
    renameAttemptRef.current += 1;
    setRenaming(false);
    setRenameExpectedVersion(null);
    setRenameTitle(page.title);
    setRenameError(false);
    setRenameNeedsReview(false);
  };

  const submitRename = (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const title = renameTitle.trim();
    if (!title || title === page.title || renameDisabled) return;
    setRenameError(false);
    setRenameNeedsReview(false);
    if (renameExpectedVersion === null) return;
    renameMutation.mutate({ id: page.id, title, expectedVersion: renameExpectedVersion });
  };

  const reloadForRename = async () => {
    const originPageId = page.id;
    const attempt = renameAttemptRef.current;
    const result = await refetchPage();
    if (
      activePageIdRef.current === originPageId &&
      result.data?.id === originPageId &&
      renameAttemptRef.current === attempt
    ) {
      setRenameExpectedVersion(result.data.version);
      setRenameError(false);
      setRenameNeedsReview(false);
    }
  };

  const hideOuterTitleWhileEditing =
    editing && editGate.kind === "editor" && editHasMatchingTitle;

  const inspectorToggle = <button ref={inspectorToggleRef} type="button" className="mem-icon-action workspace-panel-toggle note-inspector-toggle"
    aria-label={t(infoOpen || showCanvas || sourcePreview ? "pageInspector.close" : "pageInspector.open")}
    title={t(infoOpen || showCanvas || sourcePreview ? "pageInspector.close" : "pageInspector.open")}
    aria-expanded={infoOpen || showCanvas || !!sourcePreview} disabled={canvasSwitchPending}
    onClick={(event) => {
      // WebKit pointer clicks do not focus buttons; give the pane a stable return target.
      event.currentTarget.focus({ preventScroll: true });
      if (infoOpen || showCanvas || sourcePreview) closeInspector(); else void requestPageInfo();
    }}>
    <SidebarSimple aria-hidden="true" size={18} style={{transform:"scaleX(-1)"}} />
  </button>;

  const documentTools = (
    <div className="page-document-tools">
      {!documentToolsHost && inspectorToggle}
      <WorkspaceBackButton
          aria-label={t("main.back")}
          className="mem-icon-action"
          onClick={requestBack}
          type="button"
        >
          <ArrowLeft aria-hidden="true" size={16} />
        </WorkspaceBackButton>
      <div className="page-detail-actions-anchor" ref={actionMenuRef}>
                <button
                  ref={actionMenuTriggerRef}
                  type="button"
                  className="mem-icon-action page-detail-actions-menu-trigger"
                  aria-expanded={actionMenuOpen}
                  aria-haspopup="menu"
                  aria-label={t("pageDetail.actions")}
                  title={t("pageDetail.actions")}
                  onKeyDown={(event) => {
                    setActionMenuKeyboardNavigation(true);
                    handleMenuTriggerKeyDown(event, openActionMenu);
                  }}
                  onClick={(event) => {
                    setActionMenuKeyboardNavigation(event.detail === 0);
                    if (actionMenuOpen) {
                      setActionMenuOpen(false);
                    } else {
                      openActionMenu("first");
                    }
                  }}
                >
                  <svg aria-hidden="true" width="16" height="4" viewBox="0 0 16 4" fill="currentColor">
                    <circle cx="2" cy="2" r="1.5" />
                    <circle cx="8" cy="2" r="1.5" />
                    <circle cx="14" cy="2" r="1.5" />
                  </svg>
                </button>
                {moveOpen && <MovePageDialog page={page} onClose={() => setMoveOpen(false)} />}
                {actionMenuOpen ? (
                  <div
                    aria-label={t("pageDetail.actions")}
                    className="mem-popover-surface page-detail-actions-menu"
                    data-keyboard-navigation={actionMenuKeyboardNavigation ? "true" : "false"}
                    onKeyDown={(event) => {
                      setActionMenuKeyboardNavigation(true);
                      handleMenuKeyDown(
                        event,
                        actionMenuListRef.current,
                        () => setActionMenuOpen(false),
                        actionMenuTriggerRef.current,
                      );
                    }}
                    ref={actionMenuListRef}
                    role="menu"
                    tabIndex={-1}
                  >
                    <button type="button" role="menuitem" disabled={storedActionPending || !inventoryPageFilename(page) || editDirty || saveState.phase !== "idle" || editorStatus?.compositionActive} onClick={() => { closeActionMenu(); setMoveOpen(true); }}>{t("pages.folders.move")}</button>
                    {!editing && !showCanvas ? (
                      <button type="button" role="menuitem" disabled={storedActionPending || renaming || renameMutation.isPending} onClick={() => { setActionMenuOpen(false); void beginEditing(); }}>
                        {t("pageDetail.editPage")}
                      </button>
                    ) : null}
                    {!editing && !showCanvas && canRenamePage ? (
                      <button
                        disabled={renaming || renameDisabled}
                        onClick={() => {
                          closeActionMenu();
                          startRename();
                        }}
                        role="menuitem"
                        type="button"
                      >
                        {t("pageDetail.renamePage")}
                      </button>
                    ) : null}
                    {/* Re-distill rebuilds a page from its sources; a page
                        with none has nothing to rebuild from. */}
                    {canUpdateFromSources ? <button
                        className="page-detail-mobile-menu-item"
                        disabled={storedActionBlocked || redistillMutation.isPending}
                        aria-busy={redistillMutation.isPending}
                        onClick={() => {
                          setActionMenuOpen(false);
                          void requestStoredPageAction("redistill");
                        }}
                        role="menuitem"
                        type="button"
                      >
                        {redistillMutation.isPending
                          ? t("pageDetail.redistillingPage")
                          : t("pageDetail.redistillPage")}
                    </button> : null}
                    {/* The daemon refuses to lock an entity shadow page. */}
                    {page.creation_kind !== "entity" ? <button
                      aria-checked={page.review_required === true}
                      disabled={storedActionBlocked || reviewLockPending}
                      onClick={() => {
                        closeActionMenu();
                        setActionErrorMessage(null);
                        reviewRequiredMutation.mutate({ id: page.id, required: page.review_required !== true });
                      }}
                      role="menuitemcheckbox"
                      type="button"
                    >
                      <LockIcon filled={page.review_required === true} size={12} />
                      {t("pageDetail.requireReview")}
                    </button> : null}
                    {obsidianSources.length === 0 ? (
                      <button
                        className="page-detail-mobile-menu-item"
                        disabled
                        role="menuitem"
                        type="button"
                      >
                        {t("pageDetail.exportToObsidian")}
                      </button>
                    ) : (
                      obsidianSources.map((source) => (
                        <button
                          className="page-detail-mobile-menu-item"
                          disabled={storedActionPending || exporting || editDirty || saveState.phase !== "idle" || editorStatus?.compositionActive}
                          key={source.id}
                          onClick={() => {
                            setActionMenuOpen(false);
                            void handleExportToVault(source.path);
                          }}
                          role="menuitem"
                          type="button"
                        >
                          {exported ? t("pageDetail.exported") : obsidianSources.length === 1
                            ? t("pageDetail.exportToObsidian")
                            : t("pageDetail.exportToVault", { vault: folderName(source.path) })}
                        </button>
                      ))
                    )}
                    <button
                      className="page-detail-menu-danger"
                      disabled={
                        storedActionBlocked || renaming || renameMutation.isPending || deleteMutation.isPending
                      }
                      onClick={() => void requestStoredPageAction("delete")}
                      role="menuitem"
                      type="button"
                    >
                      {t("pageDetail.deletePage")}
                    </button>
                  </div>
                ) : null}
              </div>
    </div>
  );


  return (
    <div className={`page-detail document-context-host${infoOpen || showCanvas ? " document-context-open" : ""}${showCanvas ? " page-document-map-open" : ""}${canvasWorkspaceExpanded && (infoOpen || showCanvas) ? " page-detail--workspace-expanded" : ""}`} onKeyDown={handlePageDetailKeyDown} ref={setPageDetailRootRef}>
      {documentToolsHost && createPortal(inspectorToggle, documentToolsHost)}
      <DetailReadingFrame className="page-detail-document">
        {!hideOuterTitleWhileEditing && (
          <div className="page-document-title-row">
            {renaming ? (
              <form
                className="flex flex-wrap items-center gap-2"
                onSubmit={submitRename}
                aria-busy={renameMutation.isPending}
              >
                <label className="sr-only" htmlFor="page-rename-title">
                  {t("pageDetail.pageTitle")}
                </label>
                <input
                  autoFocus
                  id="page-rename-title"
                  aria-label={t("pageDetail.pageTitle")}
                  maxLength={500}
                  className="page-detail-title min-w-0 flex-1 rounded border border-[var(--mem-border)] bg-[var(--mem-surface)] px-2 py-1"
                  value={renameTitle}
                  onChange={(event) => {
                    setRenameTitle(event.target.value);
                    setRenameError(false);
                    setRenameNeedsReview(false);
                  }}
                  onKeyDown={(event) => {
                    if (event.key === "Escape") {
                      event.preventDefault();
                      cancelRename();
                    }
                  }}
                  disabled={renameMutation.isPending}
                />
                <button type="submit" disabled={!renameTitle.trim() || renameTitle.trim() === page.title || renameExpectedVersion === null || renameMutation.isPending}>
                  {renameMutation.isPending ? t("pageDetail.renaming") : t("pageDetail.saveTitle")}
                </button>
                <button type="button" onClick={cancelRename} disabled={renameMutation.isPending}>
                  {t("pageDetail.cancelRename")}
                </button>
                {renameError || renameNeedsReview ? (
                  <span className="flex basis-full items-center gap-2" role="alert">
                    <span>{t(renameNeedsReview ? "pageDetail.renameReviewRequired" : "pageDetail.renameError")}</span>
                    {!renameNeedsReview ? (
                      <button type="button" onClick={() => void reloadForRename()} disabled={pageIsFetching}>
                        {t("pageDetail.reloadPage")}
                      </button>
                    ) : null}
                  </span>
                ) : null}
              </form>
            ) : (
              <h1 className="page-detail-title">{page.title}</h1>
            )}
            {documentTools}
          </div>
        )}


        {onOpenReview && <NoteReviewNotice key={pageId} page={page}
          onReview={onOpenReview}
          onShowSources={() => void requestPageInfo()}
          disabled={storedActionPending || canvasSwitchPending || !!editorStatus?.compositionActive}
          basis={{
            actions: basisActions,
            onEditToConfirm: startConfirmEdit,
            onUpdateFromSources: () => { void requestStoredPageAction("redistill"); },
            updateFromSourcesDisabled: !canUpdateFromSources || storedActionBlocked || redistillMutation.isPending,
          }}
          codeBasis={{ actions: codeBasisActions, onEditToConfirm: startCodeConfirmEdit }}
        />}

        {page.review_required === true && (
          <div className="note-review-notice" data-testid="page-review-lock-notice" role="status">
            <p className="note-review-notice__message">{t("pageDetail.reviewLockedNotice")}</p>
          </div>
        )}

        {projectionIssue && !inventoryPageFilename(page) && <PageProjectionNotice key={`${pageId}:${projectionIssue.expectedVersion}`} pageId={pageId} issue={projectionIssue} disabled={editDirty || saveState.phase !== "idle" || !!editorStatus?.compositionActive} onResolved={onProjectionResolved} />}

        {showAttachedPageNotice && (
          <div
            aria-label={t("pages.composer.attachedNotice", { title: page.title })}
            aria-live="polite"
            className="page-detail-attached-notice"
            role="status"
          >
            <span>{t("pages.composer.attachedNotice", { title: page.title })}</span>
            <button onClick={onDismissAttachedPageNotice} type="button">
              {t("pages.composer.dismissNotice")}
            </button>
          </div>
        )}

        {redistillNotice && (
          <div
            role="status"
            aria-live="polite"
            className="rounded-lg px-3 py-2"
            style={{
              backgroundColor:
                redistillNotice.kind === "error"
                  ? "rgba(239, 68, 68, 0.08)"
                  : redistillNotice.kind === "warning"
                    ? "rgba(245, 158, 11, 0.08)"
                    : "rgba(16, 185, 129, 0.08)",
              border: "1px solid var(--mem-border)",
              color:
                redistillNotice.kind === "error"
                  ? "#ef4444"
                  : redistillNotice.kind === "warning"
                    ? "var(--mem-accent-amber)"
                    : "var(--mem-text-secondary)",
              fontFamily: "var(--mem-font-body)",
              fontSize: "12px",
              lineHeight: "1.5",
            }}
          >
            {redistillNotice.message}
          </div>
        )}

        {reviewNotice && (
          <div
            role="status"
            aria-live="polite"
            className="rounded-lg px-3 py-2"
            data-testid="page-review-notice"
            style={{
              backgroundColor:
                reviewNotice.kind === "error"
                  ? "rgba(239, 68, 68, 0.08)"
                  : reviewNotice.kind === "warning"
                    ? "rgba(245, 158, 11, 0.08)"
                    : "rgba(16, 185, 129, 0.08)",
              border: "1px solid var(--mem-border)",
              color:
                reviewNotice.kind === "error"
                  ? "#ef4444"
                  : reviewNotice.kind === "warning"
                    ? "var(--mem-accent-amber)"
                    : "var(--mem-text-secondary)",
              fontFamily: "var(--mem-font-body)",
              fontSize: "12px",
              lineHeight: "1.5",
            }}
          >
            {reviewNotice.message}
            {reviewNotice.offerReload && (
              <button
                className="ml-2 underline"
                onClick={() => {
                  // Clear only once the reload has actually landed. Dropping the
                  // warning first and firing the refetch into the void leaves the
                  // worst state on screen: the same stale text, no warning, and a
                  // Review action that looks ready to approve content the daemon
                  // has already refused once.
                  void (async () => {
                    const reloaded = await refetchPage();
                    if (reloaded.isError) {
                      setReviewNotice({
                        kind: "error",
                        message: t("pageDetail.reviewReloadFailed"),
                        offerReload: true,
                      });
                      return;
                    }
                    setReviewNotice(null);
                  })();
                }}
                type="button"
              >
                {t("pageDetail.reviewReload")}
              </button>
            )}
          </div>
        )}

        {actionError && (
          <div
            aria-live="assertive"
            className="page-detail-action-error"
            role="alert"
          >
            {actionError}
            {deleteGuardEntityId && onEntityClick && (
              <button
                onClick={() => onEntityClick(deleteGuardEntityId)}
                style={{
                  background: "none",
                  border: "none",
                  color: "inherit",
                  cursor: "pointer",
                  font: "inherit",
                  marginLeft: 6,
                  padding: 0,
                  textDecoration: "underline",
                }}
                type="button"
              >
                {t("pageDetail.entityGuardOpen")}
              </button>
            )}
          </div>
        )}

        {editing ? (
          editGate.kind === "checking" ? (
            <div role="status" className="page-editor-notice">
              {t("pageDetail.editor.checking")}
            </div>
          ) : editGate.kind === "unsupported" ? (
            <div className="flex flex-col gap-3">
              <div role="alert" className="page-editor-notice">
                {t("pageDetail.editor.upgradeRequired", {
                  floor: PAGE_EDIT_DAEMON_FLOOR,
                  version:
                    editGate.version ??
                    t("pageDetail.editor.unavailableVersion"),
                })}
              </div>
              <button
                type="button"
                className="page-editor-action self-start"
                onClick={closeEditor}
              >
                {t("pageDetail.editor.cancel")}
              </button>
            </div>
          ) : editGate.kind === "normalize" ? (
            <div className="flex flex-col gap-3">
              <div role="alert" className="page-editor-notice">
                <strong>{t("pageDetail.editor.normalizeTitle")}</strong>
                <div>{t("pageDetail.editor.normalizeDescription")}</div>
              </div>
              <div className="flex items-center gap-2">
                <button
                  type="button"
                  className="page-editor-action"
                  onClick={handleNormalizeAndEdit}
                >
                  {t("pageDetail.editor.normalizeAction")}
                </button>
                <button
                  type="button"
                  className="page-editor-action"
                  onClick={closeEditor}
                >
                  {t("pageDetail.editor.cancel")}
                </button>
              </div>
            </div>
          ) : (
            <div className="page-editor-stack flex flex-col gap-2">
              {saveState.phase === "conflict" && (
                <div role="alert" className="page-editor-notice">
                  <strong>{t("pageDetail.editor.conflictTitle")}</strong>
                  <div>{t(canonicalPage === null ? "pageDetail.editor.failure.notFound" : "pageDetail.editor.conflictBody")}</div>
                  {conflictLatest.kind === "loading" && (
                    <div role="status">
                      {t("pageDetail.editor.latestLoading")}
                    </div>
                  )}
                  {conflictLatest.kind === "error" && (
                    <div>{t("pageDetail.editor.latestLoadFailed")}</div>
                  )}
                  {conflictLatest.kind === "loaded" && (
                    <details>
                      <summary>
                        {t("pageDetail.editor.latestSource", {
                          version: conflictLatest.page.version,
                        })}
                      </summary>
                      <pre>{conflictLatest.page.content}</pre>
                    </details>
                  )}
                  <div className="flex flex-wrap items-center gap-2">
                    <button
                      type="button"
                      className="page-editor-action"
                      onClick={handleCopyDraft}
                    >
                      {t("pageDetail.editor.copyDraft")}
                    </button>
                    {conflictLatest.kind === "error" ? (
                      <button
                        type="button"
                        className="page-editor-action"
                        onClick={() => {
                          if (saveStateRef.current.phase === "conflict") {
                            void fetchConflictLatest(
                              saveStateRef.current.pending.operationId,
                            );
                          }
                        }}
                      >
                        {t("pageDetail.editor.retryLatest")}
                      </button>
                    ) : (
                      <button
                        type="button"
                        className="page-editor-action"
                        onClick={handleReloadLatest}
                        disabled={conflictLatest.kind !== "loaded"}
                      >
                        {t("pageDetail.editor.reloadLatest")}
                      </button>
                    )}
                    {canonicalPage === null && (
                      <button
                        type="button"
                        className="page-editor-action"
                        onClick={handleDiscardDeletedDraft}
                      >
                        {t("pageDetail.editor.discard")}
                      </button>
                    )}
                    <button
                      type="button"
                      className="page-editor-action"
                      onClick={() => editorRef.current?.focus()}
                    >
                      {t("pageDetail.editor.keepEditing")}
                    </button>
                  </div>
                </div>
              )}
              {saveState.phase === "retryable" && (
                <div role="alert" className="page-editor-notice">
                  {failureMessage(
                    saveState.failure.outcome === "transport"
                      ? "transport"
                      : saveState.failure.kind,
                  )}
                  {saveState.failure.outcome !== "transport" &&
                    saveState.failure.message && (
                      <details>
                        <summary>
                          {t("pageDetail.editor.technicalDetails")}
                        </summary>
                        {saveState.failure.message}
                      </details>
                    )}
                  <button
                    type="button"
                    className="page-editor-action"
                    onClick={handleCopyDraft}
                  >
                    {t("pageDetail.editor.copyDraft")}
                  </button>
                </div>
              )}
              {saveState.phase === "upgrade_required" && (
                <div role="alert" className="page-editor-notice">
                  {t("pageDetail.editor.upgradeRequired", {
                    floor: saveState.requiredFloor,
                    version: saveState.reportedVersion,
                  })}
                  <button
                    type="button"
                    className="page-editor-action"
                    onClick={handleCopyDraft}
                  >
                    {t("pageDetail.editor.copyDraft")}
                  </button>
                </div>
              )}
              {saveState.phase === "failed" && (
                <div role="alert" className="page-editor-notice">
                  {failureMessage(saveState.failure.kind)}
                  {saveState.failure.message && (
                    <details>
                      <summary>{t("pageDetail.editor.technicalDetails")}</summary>
                      {saveState.failure.message}
                    </details>
                  )}
                  <button
                    type="button"
                    className="page-editor-action"
                    onClick={handleCopyDraft}
                  >
                    {t("pageDetail.editor.copyDraft")}
                  </button>
                </div>
              )}
              {editValidation && (
                <div role="alert" className="page-editor-notice">
                  {editValidation}
                </div>
              )}
              {editorSessionId === editorFallbackSessionId && (
                <div role="alert" className="page-editor-notice">
                  <strong>{t("pageDetail.editor.fallbackTitle")}</strong>
                  <div>{t("pageDetail.editor.fallbackBody")}</div>
                </div>
              )}
              <div role="status" aria-live="polite" className="sr-only">
                {saveState.phase === "pending"
                  ? t("pageDetail.editor.saving")
                  : editDirty
                    ? t("pageDetail.editor.unsaved")
                    : t("pageDetail.editor.saved")}
              </div>
              {(saveState.phase === "retryable" || saveState.phase === "failed" || saveState.phase === "upgrade_required") && (
                <button type="button" className="page-editor-action self-start" onClick={() => void autosave.retry()}>
                  {t("pageDetail.editor.retry")}
                </button>
              )}
              <p
                id="page-markdown-editor-description"
                className="sr-only"
              >
                {t(
                  editorStatus.engine === "native"
                    ? "pageDetail.editor.autosaveDescriptionFallback"
                    : "pageDetail.editor.autosaveDescription",
                  { modifier: editorShortcutModifier },
                )}
              </p>
              {editorSessionId && (
                <div className={hideOuterTitleWhileEditing ? "page-document-editor page-document-editor--title" : "page-document-editor"}>
                {hideOuterTitleWhileEditing && <><h1 className="sr-only">{page.title}</h1>{documentTools}</>}
                <MarkdownEditor
                  ref={editorRef}
                  initialDocument={editInitialDocument}
                  initialSelection={initialSelection}
                  onSelectionChange={onSelectionChange}
                  sessionId={editorSessionId}
                  disabled={storedActionPending}
                  seamless
                  ariaLabel={t("pageDetail.editor.label")}
                  describedBy="page-markdown-editor-description"
                  wikiLinkTargets={editorWikiLinkTargets}
                  linkNotes={linkNotes}
                  onLinkPick={pickLink}
                  referenceContext={editorReferenceContext}
                  onReferenceActivate={(target, anchor) => {
                    if (target.kind === "page") { dismissWikiLinkPreview(); onPageClick?.(target.id); }
                    else if (target.kind === "memory") { dismissWikiLinkPreview(); onMemoryClick(target.id); }
                    else if (target.citation.source_kind === "memory" && target.sourceMemory) {
                      dismissWikiLinkPreview(); onMemoryClick(target.citation.locator);
                    } else showReferencePreview(target, anchor, false);
                  }}
                  onReferencePreview={showReferencePreview}
                  onWikiPageActivate={(targetId) => {
                    dismissWikiLinkPreview();
                    onPageClick?.(targetId);
                  }}
                  onWikiLinkPreview={showWikiLinkPreview}
                  onWikiLinkPreviewLeave={leaveWikiLinkPreview}
                  onDocumentChange={handleDocumentChange}
                  onSave={saveDocument}
                  onCancel={requestCloseEditor}
                  onStatusChange={(status) => {
                    if (status.ready) onEditorReady?.();
                    handleEditorStatus(
                      editorSessionId,
                      editorSessionEpoch,
                      status,
                    );
                  }}
                  onFallback={(reason) =>
                    handleEditorFallback(
                      editorSessionId,
                      editorSessionEpoch,
                      reason,
                    )
                  }
                />
                <ReferencePreview
                  dataPageLinkPreview
                  request={wikiLinkPreview?.target.kind === "citation" ? {
                    ...wikiLinkPreview,
                    target: { ...wikiLinkPreview.target,
                      sourceMemory: editorReferenceContext.sourceMemories.get(wikiLinkPreview.target.citation.locator) ?? null,
                      sourcesLoading: pageSourcesLoading },
                  } : wikiLinkPreview}
                  onDismiss={dismissWikiLinkPreview}
                  onOpenPage={(targetId) => onPageClick?.(targetId)}
                  onOpenMemory={openSourceMemory}
                  onPointerEnter={() => {
                    if (wikiLinkPreviewLeaveTimerRef.current !== null) window.clearTimeout(wikiLinkPreviewLeaveTimerRef.current);
                    wikiLinkPreviewLeaveTimerRef.current = null;
                  }}
                  onPointerLeave={leaveWikiLinkPreview}
                />
                </div>
              )}
            </div>
          )
        ) : (
          <div>
            <ReferenceNavigationProvider onOpenPage={onPageClick} onOpenMemory={openSourceMemory}>
            <div className="page-detail-prose" data-testid="page-document-reading" onClickCapture={handleContentClick}>
              {ledeText && (
                <div className="page-detail-lede">
                  {ledeMarkdown ? (
                    <ContentRenderer
                      content={ledeMarkdown}
                      variant="lede"
                      renderCitation={renderCitation}
                    />
                  ) : (
                    <p>{ledeText}</p>
                  )}
                </div>
              )}
              <ContentRenderer
                content={displayContent}
                variant="detail"
                renderCitation={renderCitation}
              />
            </div>
            </ReferenceNavigationProvider>
          </div>
        )}
      </DetailReadingFrame>

      <div aria-hidden={!canvasWorkspaceExpanded} className="page-detail-canvas-workspace-host" ref={setCanvasWorkspaceHost} />

      <PageInfoDrawer
        docked inspector
        variant={showCanvas ? "canvas" : "info"}
        expanded={canvasWorkspaceExpanded}
        expandedHost={canvasWorkspaceHost}
        onExpandedChange={canvasWorkspaceSupported ? changeCanvasWorkspaceExpanded : undefined}
        expandLabel={t("pageCanvas.expandWorkspace")}
        restoreLabel={t("pageCanvas.restoreSidebar")}
        open={!sourcePreview && (infoOpen || showCanvas)}
        onClose={closeInspector}
        title={t("pageInspector.label")}
        closeLabel={t("common.close")}
        headerContent={<>
          <NoteInspectorTabs idPrefix={inspectorId} active={showCanvas ? "canvas" : "info"}
            onSelect={selectInspectorTab} disabled={canvasSwitchPending} />
        </>}
      >
        <div id={`${inspectorId}-panel`} role="tabpanel" aria-labelledby={`${inspectorId}-${showCanvas ? "canvas" : "info"}`}
          className={`note-inspector-panel${showCanvas ? " note-inspector-panel--canvas" : ""}`}>
          {showCanvas ? <PageCanvas key={pageId} pageId={pageId} pageTitle={page.title} labelOverrides={labelOverrides}
            onMemoryClick={onMemoryClick} onPageClick={onPageClick} onEntityClick={onEntityClick} />
            : <DetailInfoContent className="note-inspector-info">
          <InfoAction
            label={t("pageDetail.markPageReviewed")}
            description={reviewSupported ? t("infoActions.reviewPageHint") : reviewUnavailableReason}
            active={cutoverLive && Boolean(page.truth?.human_reviewed)}
            disabled={storedActionBlocked || !reviewSupported}
            pending={reviewMutation.isPending}
            onClick={() => { void requestStoredPageAction("review"); }}
          />
          <div className="note-info-summary">
          <h3 className="note-info-section-heading">{t("detailInfo.details")}</h3>
          <p className="document-kind-label">{t("knowledgeContext.noteKind")}</p>
            <div className="note-info-dateline">
              <span className="page-detail-dateline-item">
                {/* "Last distilled" only for a page Wenlan distills from its
                    sources. A page someone wrote (authored, which includes every
                    page created with no sources) or edited by hand was not
                    distilled into its current text; it shows when it last changed. */}
                {page.creation_kind === "source" || page.creation_kind === "imported"
                  || page.creation_kind === "authored" || page.user_edited
                  ? t("pageDetail.dateline.lastUpdated", {
                      time: relativeTimeFromISO(page.last_modified, t),
                    })
                  : t("pageDetail.dateline.lastDistilled", {
                      time: relativeTimeFromISO(page.last_compiled, t),
                    })}
              </span>
              {basisRefs.length > 0 || codeBasisRefs.length > 0
                ? <span className="page-detail-dateline-item">
                  {basisCountLabel(t, sourceCount, basisRefs.length, codeBasisRefs.length)}
                </span>
                : sourceCount > 0 && <span className="page-detail-dateline-item">
                  {t("pageDetail.dateline.sourceMemories", { count: sourceCount })}
                </span>}
            </div>
          <div className="flex flex-wrap gap-2">
            {page.stale_reason && <span style={datelineBlocked || page.stale_reason === "source_conflict"
              ? { color: "var(--mem-accent-amber)" }
              : undefined}>
              {page.stale_reason === "source_conflict"
                ? t("pageDetail.dateline.needsReview")
                : datelineBlocked
                  ? t("pageDetail.dateline.updateBlocked")
                  : t("noteReview.sourceUpdated")}
            </span>}
            <PageTruthBadges cutoverLive={cutoverLive} truth={page.truth} />
          </div>
          </div>
          <div className="note-info-sections">
            <KnowledgeContext standaloneGraph diagramOnly key={pageId} kind="page" id={pageId} title={page.title}
              onOpenGraph={onOpenGraph ? () => onOpenGraph(pageId) : undefined}
              onNavigateMemory={(id) => { setInfoOpen(false); onMemoryClick(id); }}
              onNavigatePage={onPageClick}
              onNavigateEntity={onEntityClick ? (id) => { setInfoOpen(false); onEntityClick(id); } : undefined}
            />
            {hasRail && (
              <div className="flex flex-col gap-4">
                {pageEntities.length > 0 && (
                  <section className="memory-detail-rail-section">
                    <RailPanelTitle>{t("pageDetail.entities")}</RailPanelTitle>
                    <div className="memory-detail-entity-chip-list">
                      {pageEntities.map((entity) => (
                        <button
                          key={entity.id}
                          type="button"
                          onClick={() => { setInfoOpen(false); onEntityClick?.(entity.id); }}
                          className="memory-detail-entity-chip"
                        >
                          <span className="memory-detail-entity-name">{entity.name}</span>
                          <span className="memory-detail-entity-type">{entity.entity_type}</span>
                        </button>
                      ))}
                    </div>
                  </section>
                )}
                <RelatedPages
                  outbound={outboundLinks}
                  onPageClick={onPageClick}
                  currentPageId={page.id}
                  basis={basisRefs}
                  onMarkBasis={basisEditable ? markBasis : undefined}
                  onUnmarkBasis={basisEditable ? unmarkBasis : undefined}
                />
              </div>
            )}
            <PageInfo
              embedded
              sourceCount={sourceCount}
              sources={pageSources}
              inbound={inboundLinks}
              revisions={pageRevisionEntries}
              revisionsLoading={revisionsLoading}
              revisionsError={revisionsError}
              onRetryRevisions={() => void refetchRevisions()}
              citations={page.citations}
              citationState={processed.state}
              onMemoryClick={openSourceMemory}
              onPageClick={onPageClick}
              currentPageId={page.id}
              currentSpace={page.space ?? null}
              basis={basisRefs}
              onMarkBasis={basisEditable ? markBasis : undefined}
              onUnmarkBasis={basisEditable ? unmarkBasis : undefined}
              codeBasis={codeBasisRefs}
              onUnmarkCodeBasis={unmarkCodeBasis}
            />
            <PageSessionSources
              pageId={pageId}
              headingClassName="note-info-section-heading"
              onOpenMemory={(id) => { setInfoOpen(false); onMemoryClick(id); }}
              onOpenPage={onPageClick}
            />
          </div>
            </DetailInfoContent>}
        </div>
      </PageInfoDrawer>
      {sourcePreview && <SourceDocumentPreview
        key={`${pageId}:${sourcePreview.source_id}`}
        file={sourcePreview}
        space={page.space ?? undefined}
        onClose={() => setSourcePreview(null)}
      />}
    </div>
  );
}
