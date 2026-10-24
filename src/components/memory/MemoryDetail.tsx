// SPDX-License-Identifier: AGPL-3.0-only
import { WorkspaceBackButton } from "./navigation/WorkspaceNavigation";
import { useState, useRef, useEffect, useId, useCallback } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import {
  getMemoryDetail,
  getMemoryRevisions,
  getVersionChain,
  listAllTags,
  search,
  updateMemory,
  reclassifyMemory,
  pinMemory,
  unpinMemory,
  deleteFileChunks,
  getPendingRevision,
  acceptPendingRevision,
  dismissPendingRevision,
  listSpaces,
  agentDisplayName,
  MEMORY_FACETS,
  FACET_COLORS,
  STABILITY_TIERS,
  type MemoryType,
  type PendingRevision,
} from "../../lib/tauri";
import { confirmAction } from "../../lib/confirmAction";
import { MemoryDocumentEditor } from "./editor/MemoryDocumentEditor";
import { InfoToggle } from "./page/InfoToggle";
import TagEditor from "../TagEditor";
import PageInfoDrawer from "./page/PageInfoDrawer";
import { DetailInfoContent, DetailReadingFrame } from "./DetailLayout";
import { InfoAction } from "./page/InfoAction";
import { useDetailInfoOpen } from "./useDetailInfoOpen";
import KnowledgeContext from "./context/KnowledgeContext";
import ContentRenderer from "./ContentRenderer";
import { DisclosureButton, PinIcon, RailPanelTitle } from "./MemoryDetailPrimitives";
import { MemorySessionSource } from "./sessions/SessionSourceSection";
import { formatRelativeEntityTime } from "./entity-detail/formatEntityMetadata";
import "./memoryDetailReading.css";

interface MemoryDetailProps {
  onRegisterFlush?: (flush: (() => Promise<boolean>) | null) => void;
  sourceId: string;
  onBack: () => void;
  onNavigateEntity: (entityId: string) => void;
  onNavigateMemory: (sourceId: string) => void;
  onNavigatePage?: (pageId: string) => void;
}

interface MemoryDetailStatusProps {
  readonly ariaLabel: string;
  readonly body: string;
  readonly onBack: () => void;
  readonly title: string;
  readonly backLabel: string;
}

function MemoryDetailStatus({ ariaLabel, body, onBack, title, backLabel }: MemoryDetailStatusProps) {
  return (
    <main className="memory-detail-dossier memory-detail-content-first" aria-label={ariaLabel}>
      <header className="memory-detail-header">
        <WorkspaceBackButton
          type="button"
          onClick={onBack}
          className="memory-detail-back"
          aria-label={backLabel}
          title={backLabel}
        >
          <svg aria-hidden="true" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5"><path d="M19 12H5M12 19l-7-7 7-7" /></svg>
        </WorkspaceBackButton>
      </header>
      <section className="memory-detail-status-card" aria-live="polite">
        <h2 className="memory-detail-status-title">{title}</h2>
        <p className="memory-detail-status-copy">{body}</p>
      </section>
    </main>
  );
}

function normalizedMemoryText(value: string): string {
  return value.replace(/\s+/gu, " ").trim();
}

function structuredFieldLabel(key: string, locale: string): string {
  const words = key.replace(/_/gu, " ").trim().toLocaleLowerCase(locale);
  return words.replace(/^\p{L}/u, (first) => first.toLocaleUpperCase(locale));
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function parseStructuredEntries(raw: string | null | undefined): Array<[string, string]> {
  if (!raw || raw === "{}") return [];
  try {
    const parsed: unknown = JSON.parse(raw);
    if (!isRecord(parsed)) return [];
    return Object.entries(parsed).filter(
      (entry): entry is [string, string] => typeof entry[1] === "string" && entry[1].trim().length > 0,
    );
  } catch (error) {
    if (error instanceof SyntaxError) return [];
    throw error;
  }
}

function parseSourceIds(raw: string | null | undefined): string[] {
  if (!raw) return [];
  try {
    const parsed: unknown = JSON.parse(raw);
    if (!isRecord(parsed) || !Array.isArray(parsed.source_ids)) return [];
    return parsed.source_ids.filter((sourceId): sourceId is string => typeof sourceId === "string");
  } catch (error) {
    if (error instanceof SyntaxError) return [];
    throw error;
  }
}

function isMemoryType(value: string | null | undefined): value is MemoryType {
  return MEMORY_FACETS.some((facet) => facet.type === value);
}

function displayTitle(content: string, title: string | null | undefined, fallbackTitle: string): string {
  return title || content.split("\n")[0]?.substring(0, 80) || fallbackTitle;
}

export default function MemoryDetail({
  onRegisterFlush,
  sourceId,
  onBack,
  onNavigateEntity,
  onNavigateMemory,
  onNavigatePage,
}: MemoryDetailProps) {
  const { t, i18n } = useTranslation();
  const queryClient = useQueryClient();
  const [contentBusy, setContentBusy] = useState(false);
  const [revisionPending, setRevisionPending] = useState(false);
  const [revisionError, setRevisionError] = useState(false);
  const contentFlushRef = useRef<(() => Promise<boolean>) | null>(null);
  const deletedRef = useRef(false);
  const registerContentFlush = useCallback((flush: (() => Promise<boolean>) | null) => { contentFlushRef.current = flush; }, []);
  useEffect(() => {
    onRegisterFlush?.(async () => deletedRef.current || (contentFlushRef.current ? contentFlushRef.current() : !revisionPending));
    return () => onRegisterFlush?.(null);
  }, [onRegisterFlush, revisionPending]);
  const [reclassifyOpen, setReclassifyOpen] = useState(false);
  const [spacePickerOpen, setSpacePickerOpen] = useState(false);
  const [expandedVersion, setExpandedVersion] = useState<string | null>(null);
  const [pendingRevision, setPendingRevision] = useState<PendingRevision | null>(null);
  const [editingTags, setEditingTags] = useState(false);
  const [sourceExpanded, setSourceExpanded] = useState(false);
  const [revisionHistoryExpanded, setRevisionHistoryExpanded] = useState(false);
  const [versionHistoryExpanded, setVersionHistoryExpanded] = useState(false);
  const [relatedMemoriesExpanded, setRelatedMemoriesExpanded] = useState(false);
  const [actionsOpen, setActionsOpen] = useState(false);
  const [keyboardNavigation, setKeyboardNavigation] = useState(false);
  const { open: infoOpen, setOpen: setInfoOpen, toggle: toggleInfo } = useDetailInfoOpen(sourceId);
  const infoToggleRef = useRef<HTMLButtonElement>(null);
  const actionsId = useId();
  const actionsRef = useRef<HTMLDivElement>(null);
  const actionsTriggerRef = useRef<HTMLButtonElement>(null);
  const initialActionFocusRef = useRef<"first" | "last">("first");
  const dropdownRef = useRef<HTMLDivElement>(null);
  const spaceDropdownRef = useRef<HTMLDivElement>(null);

  const { data: spaces = [] } = useQuery({
    queryKey: ["spaces"],
    queryFn: listSpaces,
  });

  const {
    data: fetchedMemory,
    isError: memoryLoadFailed,
    isPending: memoryPending,
  } = useQuery({
    queryKey: ["memoryDetail", sourceId],
    queryFn: () => getMemoryDetail(sourceId),
    refetchInterval: 5000,
  });

  const lastMemory = useRef(fetchedMemory);
  if (fetchedMemory) lastMemory.current = fetchedMemory;
  const memory = fetchedMemory ?? (lastMemory.current?.source_id === sourceId ? lastMemory.current : null);

  const { data: versionChain = [] } = useQuery({
    queryKey: ["version-chain", sourceId],
    queryFn: () => getVersionChain(sourceId),
    enabled: !!memory?.supersedes,
  });

  const { data: memoryRevisions } = useQuery({
    queryKey: ["memory-revisions", sourceId],
    queryFn: () => getMemoryRevisions(sourceId),
    enabled: !!sourceId,
    staleTime: 30_000,
    retry: false,
  });

  // For recaps: fetch exact source memories from structured_fields.source_ids
  // For regular memories: semantic search for related
  const recapSourceIds = memory?.is_recap ? parseSourceIds(memory.structured_fields) : [];

  const { data: relatedSearchResults = [] } = useQuery({
    queryKey: ["relatedMemories", sourceId, recapSourceIds],
    queryFn: async () => {
      if (!memory) return [];
      // Recaps: only show source memories when we have exact IDs
      if (memory.is_recap) {
        if (recapSourceIds.length === 0) return [];
        const results = await Promise.all(
          recapSourceIds.map((id) => getMemoryDetail(id))
        );
        return results.filter((r): r is NonNullable<typeof r> => r != null);
      }
      // Regular memories: semantic search for related
      const results = await search(memory.content.substring(0, 200), 6, "memory");
      return results.filter((r) => r.source_id !== sourceId);
    },
    enabled: !!memory,
    staleTime: 30000,
  });

  const { data: tagData } = useQuery({
    queryKey: ["tags"],
    queryFn: listAllTags,
    refetchInterval: 5000,
  });

  const docTagKey = `memory::${sourceId}`;
  const currentTags = tagData?.document_tags[docTagKey] ?? [];
  const allTags = tagData?.tags ?? [];

  const invalidate = () => {
    queryClient.invalidateQueries({ queryKey: ["memoryDetail", sourceId] });
    queryClient.invalidateQueries({ queryKey: ["memory-revisions", sourceId] });
    queryClient.invalidateQueries({ queryKey: ["memories"] });
    queryClient.invalidateQueries({ queryKey: ["knowledge-graph"] });
    queryClient.invalidateQueries({ queryKey: ["memoryStats"] });
  };

  const confirmMutation = useMutation({
    mutationFn: ({ id, confirmed }: { id: string; confirmed: boolean }) => updateMemory(id, undefined, undefined, confirmed),
    onSuccess: async (_result, { id }) => {
      await Promise.all([
        ["memoryDetail", id], ["memory-revisions", id], ["memories"], ["knowledge-graph"], ["memoryStats"],
      ].map((queryKey) => queryClient.invalidateQueries({ queryKey })));
    },
  });

  const reclassifyMutation = useMutation({
    mutationFn: (memoryType: MemoryType) => reclassifyMemory(sourceId, memoryType),
    onSuccess: () => {
      setReclassifyOpen(false);
      invalidate();
    },
  });

  const pinMutation = useMutation({
    mutationFn: () => memory?.pinned ? unpinMemory(sourceId) : pinMemory(sourceId),
    onSuccess: invalidate,
  });

  const deleteMutation = useMutation({
    mutationFn: () => deleteFileChunks("memory", sourceId),
    onSuccess: () => {
      deletedRef.current = true;
      queryClient.invalidateQueries({ queryKey: ["memories"] });
      queryClient.invalidateQueries({ queryKey: ["knowledge-graph"] });
      queryClient.invalidateQueries({ queryKey: ["memoryStats"] });
      onBack();
    },
  });

  const changeSpaceMutation = useMutation({
    mutationFn: (newDomain: string | undefined) =>
      updateMemory(sourceId, undefined, newDomain, undefined, undefined),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["memoryDetail", sourceId] });
      queryClient.invalidateQueries({ queryKey: ["memories"] });
      queryClient.invalidateQueries({ queryKey: ["knowledge-graph"] });
      queryClient.invalidateQueries({ queryKey: ["spaces"] });
      setSpacePickerOpen(false);
    },
  });

  // Only disclosure state resets on navigation; editing drafts keep their existing lifecycle.
  useEffect(() => {
    setActionsOpen(false);
    setSourceExpanded(false);
    setRevisionHistoryExpanded(false);
    setVersionHistoryExpanded(false);
    setEditingTags(false);
    setRelatedMemoriesExpanded(false);
    setExpandedVersion(null);
    setReclassifyOpen(false);
    setSpacePickerOpen(false);
  }, [sourceId]);

  useEffect(() => {
    if (!actionsOpen) return;
    const items = Array.from(actionsRef.current?.querySelectorAll<HTMLButtonElement>("[role^='menuitem']:not(:disabled)") ?? []);
    if (items.length > 0) {
      (initialActionFocusRef.current === "last" ? items[items.length - 1] : items[0])?.focus();
    } else {
      actionsRef.current?.querySelector<HTMLElement>("[role='menu']")?.focus();
    }
    const closeOutside = (event: PointerEvent) => {
      if (!actionsRef.current?.contains(event.target as Node)) setActionsOpen(false);
    };
    const closeOnFocusOutside = (event: FocusEvent) => {
      if (!actionsRef.current?.contains(event.target as Node)) setActionsOpen(false);
    };
    document.addEventListener("pointerdown", closeOutside);
    document.addEventListener("focusin", closeOnFocusOutside);
    return () => {
      document.removeEventListener("pointerdown", closeOutside);
      document.removeEventListener("focusin", closeOnFocusOutside);
    };
  }, [actionsOpen]);

  const closeActions = () => {
    setActionsOpen(false);
    actionsTriggerRef.current?.focus();
  };

  const handleActionsKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      closeActions();
      return;
    }
    const items = Array.from(actionsRef.current?.querySelectorAll<HTMLButtonElement>("[role^='menuitem']:not(:disabled)") ?? []);
    const index = items.indexOf(document.activeElement as HTMLButtonElement);
    let next = index;
    if (event.key === "ArrowDown") next = (index + 1) % items.length;
    else if (event.key === "ArrowUp") next = (index - 1 + items.length) % items.length;
    else if (event.key === "Home") next = 0;
    else if (event.key === "End") next = items.length - 1;
    else return;
    event.preventDefault();
    setKeyboardNavigation(true);
    items[next]?.focus();
  };

  // Close reclassify dropdown on click outside
  useEffect(() => {
    if (!reclassifyOpen) return;
    const handler = (e: MouseEvent) => {
      if (dropdownRef.current && !dropdownRef.current.contains(e.target as Node)) {
        setReclassifyOpen(false);
      }
    };
    document.addEventListener("mousedown", handler);
    return () => document.removeEventListener("mousedown", handler);
  }, [reclassifyOpen]);

  // Close space picker on click outside
  useEffect(() => {
    if (!spacePickerOpen) return;
    const handler = (e: MouseEvent) => {
      if (spaceDropdownRef.current && !spaceDropdownRef.current.contains(e.target as Node)) {
        setSpacePickerOpen(false);
      }
    };
    document.addEventListener("mousedown", handler);
    return () => document.removeEventListener("mousedown", handler);
  }, [spacePickerOpen]);

  // Fetch pending revision for protected confirmed memories
  useEffect(() => {
    if (!memory) return;
    const facetType = memory.memory_type ?? "fact";
    const tier = STABILITY_TIERS[facetType] ?? "ephemeral";
    if (tier === "protected" && memory.confirmed) {
      getPendingRevision(memory.source_id).then(setPendingRevision).catch(() => {});
    }
  }, [memory]);

  const handleAcceptRevision = async () => {
    if (!pendingRevision || revisionPending) return;
    setRevisionError(false);
    setRevisionPending(true);
    try {
      if (contentFlushRef.current && !await contentFlushRef.current()) return;
      await acceptPendingRevision(sourceId);
      setPendingRevision(null);
      await queryClient.invalidateQueries({ queryKey: ["memoryDetail", sourceId] });
      invalidate();
    } catch { setRevisionError(true); }
    finally { setRevisionPending(false); }
  };

  const handleDismissRevision = async () => {
    if (!pendingRevision) return;
    await dismissPendingRevision(sourceId);
    setPendingRevision(null);
  };

  if (!memory || (fetchedMemory === null && !contentBusy)) {
    const titleKey = memoryLoadFailed
      ? "memoryDetail.loadFailedTitle"
      : memoryPending
        ? "memoryDetail.loadingTitle"
        : "memoryDetail.notFoundTitle";
    const bodyKey = memoryLoadFailed
      ? "memoryDetail.loadFailedBody"
      : memoryPending
        ? "memoryDetail.loadingBody"
        : "memoryDetail.notFoundBody";

    return (
      <MemoryDetailStatus
        ariaLabel={t("memoryDetail.dossierLabel")}
        backLabel={t("memoryDetail.backToMemories")}
        body={t(bodyKey)}
        onBack={onBack}
        title={t(titleKey)}
      />
    );
  }

  const facetType = isMemoryType(memory.memory_type) ? memory.memory_type : "fact";
  const facetLabel = t(`importView.typeLabels.${facetType}`);
  const isConfirmed = memory.confirmed;
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const formatTimeAgo = (ts: number) => formatRelativeEntityTime(ts, locale) ?? "";
  const formatAbsoluteTime = (ts: number) => new Date(ts * 1000).toLocaleString(locale);
  const relatedMemories = relatedSearchResults.slice(0, 5);
  const revisionEntries = memoryRevisions?.entries ?? [];
  const visibleRevisionEntries = revisionHistoryExpanded ? revisionEntries : revisionEntries.slice(0, 1);
  const visibleVersionChain = versionHistoryExpanded ? versionChain : versionChain.slice(0, 3);
  const visibleRelatedMemories = relatedMemoriesExpanded ? relatedMemories : relatedMemories.slice(0, 3);
  const hasDaemonRevisionHistory = revisionEntries.length > 0;
  const title = displayTitle(memory.content, memory.title, t("memoryDetail.untitledMemory"));
  const structuredEntries = parseStructuredEntries(memory.structured_fields).filter(([, value]) =>
    normalizedMemoryText(value) !== normalizedMemoryText(memory.content),
  );
  const hasLegacyVersionHistory = !hasDaemonRevisionHistory && Boolean(memory.supersedes) && versionChain.length > 0;
  const sourceText = memory.source_text?.trim() ?? "";
  const hasSourceExcerpt = sourceText.length > 0 && sourceText !== memory.content.trim();
  const sourcePreviewLength = 120;
  const sourceClipped = sourceText.length > sourcePreviewLength;
  const visibleSourceText = sourceExpanded || !sourceClipped ? sourceText : `${sourceText.slice(0, sourcePreviewLength)}…`;
  const hasSimilarMemories = relatedMemories.length > 0;
  const navigateMemory = (id: string) => onNavigateMemory(id);
  const navigateEntity = (id: string) => onNavigateEntity(id);
  const navigatePage = onNavigatePage ? (id: string) => onNavigatePage(id) : undefined;

  return (
    <main className={`memory-detail-dossier memory-detail-content-first document-context-host${infoOpen ? " document-context-open" : ""}`} aria-label={t("memoryDetail.dossierLabel")}>
      <DetailReadingFrame variant="compact" className="memory-document-frame">
      <header className="memory-detail-header">
        <WorkspaceBackButton
          type="button"
          onClick={onBack}
          className="memory-detail-back"
          aria-label={t("memoryDetail.backToMemories")}
          title={t("memoryDetail.backToMemories")}
        >
          <svg aria-hidden="true" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5"><path d="M19 12H5M12 19l-7-7 7-7" /></svg>
        </WorkspaceBackButton>
        {fetchedMemory !== null && <div className="memory-detail-actions">
          <InfoToggle ref={infoToggleRef} open={infoOpen} onClick={toggleInfo} />

          <div
            className="page-detail-actions-anchor"
            ref={actionsRef}
          >
            <button
              ref={actionsTriggerRef}
              type="button"
              className="mem-icon-action page-detail-actions-menu-trigger"
              aria-label={t("memoryDetail.actions")}
              aria-haspopup="menu"
              aria-controls={actionsOpen ? actionsId : undefined}
              aria-expanded={actionsOpen}
              onClick={(event) => {
                setKeyboardNavigation(event.detail === 0);
                initialActionFocusRef.current = "first";
                setActionsOpen((open) => !open);
              }}
              onKeyDown={(event) => {
                if (event.key === "ArrowDown" || event.key === "ArrowUp") {
                  event.preventDefault();
                  event.stopPropagation();
                  setKeyboardNavigation(true);
                  initialActionFocusRef.current = event.key === "ArrowUp" ? "last" : "first";
                  setActionsOpen(true);
                }
              }}
            >
              <svg aria-hidden="true" width="16" height="4" viewBox="0 0 16 4" fill="currentColor">
                <circle cx="2" cy="2" r="1.5" /><circle cx="8" cy="2" r="1.5" /><circle cx="14" cy="2" r="1.5" />
              </svg>
            </button>
            {actionsOpen && (
              <div id={actionsId} className="mem-popover-surface page-detail-actions-menu" data-keyboard-navigation={keyboardNavigation ? "true" : "false"} role="menu" aria-label={t("memoryDetail.actions")} tabIndex={-1} onKeyDown={handleActionsKeyDown}>
                <button
                  type="button"
                  role="menuitemcheckbox"
                  tabIndex={-1}
                  aria-checked={memory.pinned}
                  aria-label={t("memoryDetail.pinnedState", { state: memory.pinned ? t("memoryDetail.yes") : t("memoryDetail.no") })}
                  title={memory.pinned ? t("memoryDetail.unpin") : t("memoryDetail.pin")}
                  disabled={pinMutation.isPending || deleteMutation.isPending}
                  onClick={() => { pinMutation.mutate(); closeActions(); }}
                >
                  <PinIcon filled={memory.pinned} size={12} />
                  {t("memoryDetail.pinned")}
                </button>
                <button
                  type="button"
                  role="menuitem"
                  tabIndex={-1}
                  className="page-detail-menu-danger"
                  aria-label={t("memoryDetail.deleteMemory")}
                  disabled={pinMutation.isPending || deleteMutation.isPending}
                  onClick={async () => {
                    closeActions();
                    if (await confirmAction(t("memoryDetail.deleteConfirm"), { confirm: t("memoryDetail.deleteMemory"), cancel: t("common.cancel") })) {
                      void (contentFlushRef.current?.() ?? Promise.resolve(true)).then(saved => { if (saved) deleteMutation.mutate(); });
                    }
                  }}
                >
                  {t("memoryDetail.deleteMemory")}
                </button>
              </div>
            )}
          </div>
        </div>}
      </header>

      <section className="memory-detail-reading reading-surface" aria-label={t("memoryDetail.readingLabel")}>
        <h2 className="sr-only">{title}</h2>

        {memory.memory_type === "recap" ? (
          <div className="memory-detail-body-text"><ContentRenderer content={memory.content} structuredFields={memory.structured_fields} variant="detail" /></div>
        ) : (
          <MemoryDocumentEditor key={sourceId} memory={memory} canonical={fetchedMemory}
            disabled={revisionPending || deleteMutation.isPending || confirmMutation.isPending}
            onBusyChange={setContentBusy} onRegisterFlush={registerContentFlush} />
        )}
        {revisionError ? <p role="alert">{t("infoActions.saveError")}</p> : null}

      {memory.quality === "low" && (
        <div className="memory-detail-quality-warning">
          <span className="memory-chip warning">{t("memoryDetail.lowQuality")}</span>
        </div>
      )}

      {/* Pending revision */}
      {pendingRevision && fetchedMemory !== null && (
        <div className="memory-detail-pending">
          <div className="memory-detail-pending-title">
            {pendingRevision.source_agent
              ? t("memoryDetail.proposedUpdateFrom", { agent: pendingRevision.source_agent })
              : t("memoryDetail.proposedUpdate")}
          </div>
          <p className="memory-detail-pending-copy">
            {pendingRevision.content}
          </p>
          <div className="flex gap-2">
            <button
              onClick={handleAcceptRevision}
              disabled={contentBusy || revisionPending}
              className="memory-detail-text-button primary"
            >
              {t("memoryDetail.accept")}
            </button>
            <button
              onClick={handleDismissRevision}
              className="memory-detail-text-button"
            >
              {t("memoryDetail.dismiss")}
            </button>
          </div>
        </div>
      )}

      </section>
      </DetailReadingFrame>

      <PageInfoDrawer docked inspector focusOnOpen={false} returnFocusRef={infoToggleRef} open={infoOpen && fetchedMemory !== null} onClose={() => setInfoOpen(false)} title={t("knowledgeContext.memoryPanel")} closeLabel={t("common.close")}>
        <DetailInfoContent className="memory-detail-context-content">
          <InfoAction
            label={t(isConfirmed ? "infoActions.contentConfirmed" : "infoActions.confirmContent")}
            description={t(contentBusy ? "infoActions.saveMemoryFirst" : isConfirmed ? "infoActions.contentConfirmedHint" : "infoActions.confirmContentHint")}
            showDescription={!isConfirmed}
            active={isConfirmed}
            pending={confirmMutation.isPending}
            disabled={isConfirmed || contentBusy || revisionPending || deleteMutation.isPending}
            error={confirmMutation.isError && confirmMutation.variables?.id === sourceId ? t("infoActions.saveError") : undefined}
            onClick={() => { void (contentFlushRef.current?.() ?? Promise.resolve(true)).then(saved => { if (saved) confirmMutation.mutate({ id: sourceId, confirmed: true }); }); }}
          />

          <section className="memory-detail-information">
            <h3 className="memory-detail-subsection">{t("detailInfo.details")}</h3>
            <dl className="memory-detail-info-list">
              <div className="memory-detail-info-row">
                <dt className="memory-detail-info-label">{t("memoryList.type")}</dt>
                <dd className="memory-detail-info-value">
                  <div className="relative" ref={dropdownRef}>
                    <button
                      onClick={() => setReclassifyOpen(!reclassifyOpen)}
                      className={`memory-detail-facet-button ${FACET_COLORS[facetType]}`}
                    >
                      {facetLabel}
                      <span className="ml-1 opacity-50">&#x25BE;</span>
                    </button>
                    {reclassifyOpen && (
                      <div
                        className="absolute left-0 top-7 z-50 rounded-lg shadow-xl py-1 min-w-[140px]"
                        style={{ backgroundColor: "var(--mem-surface)", border: "1px solid var(--mem-border)" }}
                      >
                        {MEMORY_FACETS.map((facet) => (
                          <button
                            key={facet.type}
                            className="w-full text-left px-3 py-1.5 text-xs flex items-center gap-2 hover:bg-[var(--mem-hover)]"
                            style={{
                              fontFamily: "var(--mem-font-body)",
                              color: facetType === facet.type ? "var(--mem-text)" : "var(--mem-text-secondary)",
                            }}
                            onClick={() => reclassifyMutation.mutate(facet.type)}
                          >
                            <span className={`memory-detail-facet-dot ${FACET_COLORS[facet.type].split(" ")[0]}`} />
                            {facet.label}
                          </button>
                        ))}
                      </div>
                    )}
                  </div>
                </dd>
              </div>
              <div className="memory-detail-info-row">
                <dt className="memory-detail-info-label">{t("memoryList.space")}</dt>
                <dd className="memory-detail-info-value">
                  <div className="relative" ref={spaceDropdownRef}>
                    <button
                      onClick={() => setSpacePickerOpen(!spacePickerOpen)}
                      className={`memory-detail-eyebrow-button ${memory.domain ? "" : "is-empty"}`}
                    >
                      {memory.domain ?? t("memoryDetail.assignSpace")}
                    </button>
                    {spacePickerOpen && (
                      <div
                        className="absolute left-0 top-8 z-50 rounded-lg shadow-xl py-1 min-w-[140px]"
                        style={{ backgroundColor: "var(--mem-surface)", border: "1px solid var(--mem-border)" }}
                      >
                        {memory.domain && (
                          <>
                            <button
                              onClick={() => changeSpaceMutation.mutate(undefined)}
                              className="w-full text-left px-3 py-1.5 text-xs hover:bg-[var(--mem-hover)]"
                              style={{ fontFamily: "var(--mem-font-body)", color: "var(--mem-text-tertiary)" }}
                            >
                              {t("memoryDetail.removeFromSpace")}
                            </button>
                            <div style={{ height: "1px", backgroundColor: "var(--mem-border)", margin: "2px 0" }} />
                          </>
                        )}
                        {spaces.filter((space) => space.name !== memory.domain).map((space) => (
                          <button
                            key={space.id}
                            onClick={() => changeSpaceMutation.mutate(space.name)}
                            className="w-full text-left px-3 py-1.5 text-xs hover:bg-[var(--mem-hover)]"
                            style={{ fontFamily: "var(--mem-font-body)", color: "var(--mem-text-secondary)" }}
                          >
                            <span className="capitalize">{space.name}</span>
                          </button>
                        ))}
                      </div>
                    )}
                  </div>
                </dd>
              </div>
              {memory.source_agent && (
                <div className="memory-detail-info-row">
                  <dt className="memory-detail-info-label">{t("memoryList.agent")}</dt>
                  <dd className="memory-detail-info-value">{agentDisplayName(memory.source_agent)}</dd>
                </div>
              )}
              <div className="memory-detail-info-row">
                <dt className="memory-detail-info-label">{t("memoryList.updated")}</dt>
                <dd className="memory-detail-info-value">
                  <time title={formatAbsoluteTime(memory.last_modified)}>{formatTimeAgo(memory.last_modified)}</time>
                </dd>
              </div>
            </dl>
            <div className="memory-detail-tagline">
              <span className="memory-detail-strip-label">{t("memoryDetail.tags")}</span>
              {currentTags.map((tag) => <span key={tag} className="memory-chip indigo">{tag}</span>)}
              <div className="relative">
                <button
                  onClick={() => setEditingTags(!editingTags)}
                  className="memory-detail-tag-edit"
                  aria-label={t("memoryDetail.editTags")}
                  title={t("memoryDetail.editTags")}
                >
                  {editingTags ? t("memoryDetail.done") : "+"}
                </button>
                {editingTags && (
                  <TagEditor
                    source="memory"
                    sourceId={sourceId}
                    lastModified={memory.last_modified}
                    currentTags={currentTags}
                    allTags={allTags}
                    onClose={() => setEditingTags(false)}
                    onTagsChanged={() => queryClient.invalidateQueries({ queryKey: ["tags"] })}
                  />
                )}
              </div>
            </div>
          </section>

          <MemorySessionSource
            sessionId={memory.session_id}
            headingClassName="memory-detail-subsection"
            onOpenMemory={navigateMemory}
            onOpenPage={navigatePage}
          />

          {structuredEntries.length > 0 && (
            <section className="memory-detail-structured">
              <h3 className="memory-detail-subsection">{t("detailInfo.additional")}</h3>
              <div className="memory-detail-field-list">
                {structuredEntries.map(([key, value]) => (
                  <div key={key} className="memory-detail-field-row">
                    <span className="memory-detail-field-label">{structuredFieldLabel(key, locale)}</span>
                    <span className="memory-detail-field-value">{value}</span>
                  </div>
                ))}
              </div>
            </section>
          )}

          {hasSourceExcerpt && (
            <details key={`source-${sourceId}`} className="memory-detail-disclosure memory-detail-source" open>
              <summary>{t("memoryDetail.sourceTitle")}</summary>
              <div className="memory-detail-disclosure-body">
                <blockquote className="memory-detail-source-quote">{visibleSourceText}</blockquote>
                {sourceClipped && (
                  <DisclosureButton
                    ariaLabel={sourceExpanded ? t("memoryDetail.showLess") : t("memoryDetail.showFullSource")}
                    onClick={() => setSourceExpanded(!sourceExpanded)}
                  >
                    {sourceExpanded ? t("memoryDetail.showLessCompact") : t("memoryDetail.showFullSource")}
                  </DisclosureButton>
                )}
              </div>
            </details>
          )}

          <section className="memory-detail-connections" aria-label={t("knowledgeContext.localGraph")}>
            {memory.entity_id && (
              <button
                onClick={() => navigateEntity(memory.entity_id!)}
                className="memory-detail-link-button"
              >
                {t("memoryDetail.entity")}: {t("memoryDetail.linked")}
              </button>
            )}
            <KnowledgeContext
              key={sourceId}
              kind="memory"
              id={sourceId}
              title={title}
              onNavigateMemory={navigateMemory}
              onNavigatePage={navigatePage}
              onNavigateEntity={navigateEntity}
            />
          </section>

          {hasSimilarMemories && (
            <details key={`similar-${sourceId}`} className="memory-detail-disclosure memory-detail-connections">
              <summary>{memory.is_recap ? t("memoryDetail.sourceMemories") : t("knowledgeContext.similarMemories")}</summary>
              <div className="memory-detail-disclosure-body">
                <section className="memory-detail-rail-section">
                  <div className="memory-detail-panel-heading">
                    {relatedMemories.length > visibleRelatedMemories.length && (
                      <DisclosureButton
                        ariaLabel={t("memoryDetail.showAll", { count: relatedMemories.length })}
                        count={relatedMemories.length}
                        onClick={() => setRelatedMemoriesExpanded(true)}
                      >
                        {t("memoryDetail.showAllCompact")}
                      </DisclosureButton>
                    )}
                    {relatedMemoriesExpanded && relatedMemories.length > 3 && (
                      <DisclosureButton ariaLabel={t("memoryDetail.showLess")} onClick={() => setRelatedMemoriesExpanded(false)}>
                        {t("memoryDetail.showLessCompact")}
                      </DisclosureButton>
                    )}
                  </div>
                  <div className="memory-detail-related-grid">
                    {visibleRelatedMemories.map((related) => {
                      const relatedFacet = related.memory_type ?? null;
                      const relatedColor = relatedFacet ? FACET_COLORS[relatedFacet] : null;
                      return (
                        <button
                          key={'id' in related ? related.id : related.source_id}
                          onClick={() => navigateMemory(related.source_id)}
                          className="memory-detail-related-card"
                        >
                          <div className="flex-1 min-w-0">
                            <p className="memory-detail-related-copy line-clamp-2">
                              {related.content.length > 160 ? `${related.content.substring(0, 160)}…` : related.content}
                            </p>
                            <div className="memory-detail-related-meta">
                              {relatedFacet && relatedColor && (
                                <span className={`memory-detail-related-facet ${relatedColor}`}>
                                  {t(`importView.typeLabels.${relatedFacet as MemoryType}`)}
                                </span>
                              )}
                              <span>{formatTimeAgo(related.last_modified)}</span>
                            </div>
                          </div>
                        </button>
                      );
                    })}
                  </div>
                </section>
              </div>
            </details>
          )}

          {(hasDaemonRevisionHistory || hasLegacyVersionHistory) && (
            <div className="memory-detail-provenance-group">
              {hasDaemonRevisionHistory && (
                <section className="memory-detail-rail-panel memory-detail-secondary-panel">
                  <div className="memory-detail-panel-heading">
                    <RailPanelTitle>{t("memoryDetail.revisionHistory")}</RailPanelTitle>
                    {revisionEntries.length > visibleRevisionEntries.length && (
                      <DisclosureButton
                        ariaLabel={t("memoryDetail.showAll", { count: revisionEntries.length })}
                        count={revisionEntries.length}
                        onClick={() => setRevisionHistoryExpanded(true)}
                      >
                        {t("memoryDetail.showAllCompact")}
                      </DisclosureButton>
                    )}
                    {revisionHistoryExpanded && revisionEntries.length > 1 && (
                      <DisclosureButton ariaLabel={t("memoryDetail.showLess")} onClick={() => setRevisionHistoryExpanded(false)}>
                        {t("memoryDetail.showLessCompact")}
                      </DisclosureButton>
                    )}
                  </div>
                  <div className="memory-detail-compact-list">
                    {visibleRevisionEntries.map((entry) => {
                      const isCurrent = entry.source_id === memoryRevisions?.current_source_id;
                      return (
                        <div key={entry.source_id}>
                          {isCurrent ? (
                            <div className="memory-detail-context-row is-current">
                              <div className="memory-detail-context-row-body">
                                <div className="memory-detail-context-row-meta">
                                  <span className="memory-detail-context-row-title truncate">{entry.title || t("memoryDetail.untitledMemory")}</span>
                                  <span className="memory-detail-context-row-badge">{t("memoryDetail.current")}</span>
                                  <span className="memory-detail-context-row-meta-text">{formatTimeAgo(entry.last_modified)}</span>
                                </div>
                                {entry.delta_summary && <p className="memory-detail-context-row-summary">{entry.delta_summary}</p>}
                                {entry.source_agent && <p className="memory-detail-context-row-agent">{agentDisplayName(entry.source_agent)}</p>}
                              </div>
                            </div>
                          ) : (
                            <button
                              onClick={() => navigateMemory(entry.source_id)}
                              className="memory-detail-context-row memory-detail-context-row-button"
                            >
                              <div className="memory-detail-context-row-body">
                                <div className="memory-detail-context-row-meta">
                                  <span className="memory-detail-context-row-title truncate">{entry.title || t("memoryDetail.untitledMemory")}</span>
                                  <span className="memory-detail-context-row-meta-text">{formatTimeAgo(entry.last_modified)}</span>
                                </div>
                                {entry.delta_summary && <p className="memory-detail-context-row-summary">{entry.delta_summary}</p>}
                                <p className="memory-detail-context-row-preview line-clamp-2">{entry.content_preview}</p>
                                {entry.source_agent && <p className="memory-detail-context-row-agent">{agentDisplayName(entry.source_agent)}</p>}
                              </div>
                            </button>
                          )}
                        </div>
                      );
                    })}
                  </div>
                </section>
              )}

              {hasLegacyVersionHistory && (
                <section className="memory-detail-rail-panel memory-detail-secondary-panel">
                  <div className="memory-detail-panel-heading">
                    <RailPanelTitle>{t("memoryDetail.versionHistory")}</RailPanelTitle>
                    {versionChain.length > visibleVersionChain.length && (
                      <DisclosureButton
                        ariaLabel={t("memoryDetail.showAll", { count: versionChain.length })}
                        count={versionChain.length}
                        onClick={() => setVersionHistoryExpanded(true)}
                      >
                        {t("memoryDetail.showAllCompact")}
                      </DisclosureButton>
                    )}
                    {versionHistoryExpanded && versionChain.length > 3 && (
                      <DisclosureButton ariaLabel={t("memoryDetail.showLess")} onClick={() => setVersionHistoryExpanded(false)}>
                        {t("memoryDetail.showLessCompact")}
                      </DisclosureButton>
                    )}
                  </div>
                  <div className="memory-detail-compact-list">
                    {visibleVersionChain.map((version, index) => (
                      <div key={version.source_id}>
                        <button
                          onClick={() => setExpandedVersion(expandedVersion === version.source_id ? null : version.source_id)}
                          className="memory-detail-context-row memory-detail-context-row-button"
                        >
                          <span className="memory-detail-version-index">v{index + 1}</span>
                          <span className={`memory-detail-version-title ${version.source_id === sourceId ? "is-current" : ""} truncate`}>
                            {version.title}
                          </span>
                          {version.confirmed && <span className="memory-detail-context-row-meta-text is-warm">{t("memoryDetail.confirmedStatus")}</span>}
                          <span className="memory-detail-context-row-meta-text">{formatTimeAgo(version.last_modified)}</span>
                          <svg
                            width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"
                            className="memory-detail-version-chevron"
                            style={{ transform: expandedVersion === version.source_id ? "rotate(90deg)" : "rotate(0deg)" }}
                          >
                            <path d="M9 18l6-6-6-6" />
                          </svg>
                        </button>
                        {expandedVersion === version.source_id && version.source_id !== sourceId && (
                          <div className="memory-detail-version-preview">{version.content}</div>
                        )}
                      </div>
                    ))}
                  </div>
                </section>
              )}
            </div>
          )}
        </DetailInfoContent>
      </PageInfoDrawer>
    </main>
  );
}
