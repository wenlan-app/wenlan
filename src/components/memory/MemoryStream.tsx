// SPDX-License-Identifier: AGPL-3.0-only
import { useState, useMemo, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import Masonry from "react-masonry-css";
import { Funnel } from "@phosphor-icons/react";
import MemoryCard from "./MemoryCard";
import MemoryListSurface from "./MemoryListSurface";
import { AssetLensToggle } from "./assets/AssetLensToggle";
import type { AssetLens } from "../../lib/assetLens";
import type { MemoryItem } from "../../lib/tauri";
import { setStability, deleteFileChunks, getVersionChain, pinMemory, unpinMemory } from "../../lib/tauri";
import { readPreference, writePreference } from "../../lib/preferenceStorage";
import "./assets/collectionToolbar.css";
import "./memoryCollection.css";

export type SortMode = "curated" | "recent" | "oldest";
export type ViewMode = "grid" | "list";
export type MemoryStreamPresentation = "embedded" | "parent-list";

const VIEW_MODE_KEY = "wenlan-memory-view-mode";
const LEGACY_VIEW_MODE_KEY = "origin-memory-view-mode";

function getStoredViewMode(fallback: ViewMode): ViewMode {
  return (readPreference(VIEW_MODE_KEY, LEGACY_VIEW_MODE_KEY) as ViewMode) || fallback;
}

interface MemoryStreamProps {
  memories: MemoryItem[];
  selectedDomain: string | null;
  sortMode?: SortMode;
  onSortChange?: (mode: SortMode) => void;
  stabilityFilter?: string | null;
  onStabilityFilterChange?: (filter: string | null) => void;
  agentFilter?: string | null;
  onSelectMemory?: (sourceId: string) => void;
  cardVariant?: "full" | "insight";
  presentation?: MemoryStreamPresentation;
  toolbarActions?: ReactNode;
  filter?: string;
  onFilterChange?: (filter: string) => void;
}

const STABILITY_RANK: Record<string, number> = { confirmed: 3, learned: 2, new: 1 };

function sortMemories(memories: MemoryItem[], mode: SortMode): MemoryItem[] {
  return [...memories].sort((a, b) => {
    // Pinned always first
    if (a.pinned !== b.pinned) return a.pinned ? -1 : 1;

    switch (mode) {
      case "curated": {
        // Distilled first, then confirmed > learned > new, then recency
        const da = a.supersedes != null && a.supersede_mode !== "archive" ? 1 : 0;
        const db = b.supersedes != null && b.supersede_mode !== "archive" ? 1 : 0;
        if (db !== da) return db - da;
        const sa = a.stability ?? (a.confirmed ? "confirmed" : "new");
        const sb = b.stability ?? (b.confirmed ? "confirmed" : "new");
        const rankDiff = (STABILITY_RANK[sb] ?? 0) - (STABILITY_RANK[sa] ?? 0);
        if (rankDiff !== 0) return rankDiff;
        return b.last_modified - a.last_modified;
      }
      case "oldest":
        return a.last_modified - b.last_modified;
      case "recent":
      default:
        return b.last_modified - a.last_modified;
    }
  });
}

export default function MemoryStream({
  memories,
  selectedDomain,
  sortMode = "recent",
  onSortChange,
  stabilityFilter = null,
  onStabilityFilterChange: _onStabilityFilterChange,
  agentFilter = null,
  onSelectMemory,
  cardVariant,
  presentation = "embedded",
  toolbarActions,
  filter,
  onFilterChange,
}: MemoryStreamProps) {
  const queryClient = useQueryClient();
  const { t, i18n } = useTranslation();
  const [undoItem, setUndoItem] = useState<{ sourceId: string; timer: number } | null>(null);
  const [expandedChain, setExpandedChain] = useState<string | null>(null);
  const [viewMode, setViewMode] = useState<ViewMode>(() =>
    getStoredViewMode(presentation === "parent-list" ? "list" : "grid"),
  );
  const [sortOpen, setSortOpen] = useState(false);
  const [localFilter, setLocalFilter] = useState("");
  const collectionFilter = filter ?? localFilter;
  const isCollection = presentation === "parent-list";

  const toggleViewMode = () => {
    const next = viewMode === "grid" ? "list" : "grid";
    setViewMode(next);
    writePreference(VIEW_MODE_KEY, next);
  };

  const handleCollectionLensChange = (lens: AssetLens) => {
    const next = lens === "rows" ? "list" : "grid";
    setViewMode(next);
    writePreference(VIEW_MODE_KEY, next);
  };

  const regularMemories = useMemo(() => {
    let filtered = selectedDomain
      ? memories.filter((m) => (m.domain ?? "uncategorized") === selectedDomain)
      : memories;
    if (agentFilter) {
      filtered = filtered.filter((m) => m.source_agent === agentFilter);
    }
    if (stabilityFilter) {
      filtered = filtered.filter((m) => (m.stability ?? (m.confirmed ? "confirmed" : "new")) === stabilityFilter);
    }
    // This collection shows individual memories; retired recap data stays excluded.
    const regular = filtered.filter((m) => !m.is_recap);
    return sortMemories(regular, sortMode);
  }, [memories, selectedDomain, sortMode, agentFilter, stabilityFilter]);

  const collectionNeedle = isCollection ? collectionFilter.trim().toLocaleLowerCase(i18n.language) : "";
  const visibleMemories = useMemo(() => {
    if (!collectionNeedle) return regularMemories;
    return regularMemories.filter((memory) =>
      `${memory.title} ${memory.content}`.toLocaleLowerCase(i18n.language).includes(collectionNeedle),
    );
  }, [regularMemories, collectionNeedle, i18n.language]);

  const updateCollectionFilter = (next: string) => {
    if (filter === undefined) setLocalFilter(next);
    onFilterChange?.(next);
  };

  const confirmMutation = useMutation({
    mutationFn: ({ sourceId, confirmed, prevStability }: { sourceId: string; confirmed: boolean; prevStability?: string }) =>
      setStability(sourceId, confirmed ? "confirmed" : (prevStability === "learned" ? "learned" : "new")),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["memories"] });
      queryClient.invalidateQueries({ queryKey: ["memoryStats"] });
      queryClient.invalidateQueries({ queryKey: ["profile-memories"] });
      queryClient.invalidateQueries({ queryKey: ["nurture-cards"] });
    },
  });

  const deleteMutation = useMutation({
    mutationFn: (sourceId: string) => deleteFileChunks("memory", sourceId),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["memories"] });
      queryClient.invalidateQueries({ queryKey: ["memoryStats"] });
      queryClient.invalidateQueries({ queryKey: ["profile-memories"] });
    },
  });

  const pinMutation = useMutation({
    mutationFn: (sourceId: string) => pinMemory(sourceId),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["memories"] });
      queryClient.invalidateQueries({ queryKey: ["profile-memories"] });
    },
  });

  const unpinMutation = useMutation({
    mutationFn: (sourceId: string) => unpinMemory(sourceId),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["memories"] });
      queryClient.invalidateQueries({ queryKey: ["profile-memories"] });
    },
  });

  const { data: versionChain } = useQuery({
    queryKey: ["version-chain", expandedChain],
    queryFn: () => expandedChain ? getVersionChain(expandedChain) : Promise.resolve([]),
    enabled: !!expandedChain,
  });

  const handleDelete = (sourceId: string) => {
    if (undoItem) {
      clearTimeout(undoItem.timer);
      deleteMutation.mutate(undoItem.sourceId);
    }
    const timer = window.setTimeout(() => {
      deleteMutation.mutate(sourceId);
      setUndoItem(null);
    }, 5000);
    setUndoItem({ sourceId, timer });
  };

  const handleUndo = () => {
    if (undoItem) {
      clearTimeout(undoItem.timer);
      setUndoItem(null);
      queryClient.invalidateQueries({ queryKey: ["memories"] });
    }
  };

  const renderCard = (mem: MemoryItem, i: number, delay?: number) => (
    <MemoryCard
      key={mem.source_id}
      memory={mem}
      variant={cardVariant ?? "full"}
      onConfirm={(id, confirmed) => confirmMutation.mutate({ sourceId: id, confirmed, prevStability: mem.stability })}
      onDelete={handleDelete}
      onPin={(id) => pinMutation.mutate(id)}
      onUnpin={(id) => unpinMutation.mutate(id)}
      expandedChain={expandedChain === mem.source_id}
      onToggleChain={() => setExpandedChain(expandedChain === mem.source_id ? null : mem.source_id)}
      versionChain={expandedChain === mem.source_id ? versionChain ?? [] : []}
      onClick={onSelectMemory}
      presentation={presentation === "parent-list" ? "parent-list" : "card"}
      style={{
        animation: `mem-fade-up 400ms cubic-bezier(0.16, 1, 0.3, 1) both`,
        animationDelay: `${delay ?? i * 40}ms`,
      }}
    />
  );

  const hasActiveFilter =
    !!stabilityFilter || !!agentFilter || !!selectedDomain || !!collectionNeedle;
  const filteredToEmpty =
    memories.length > 0 && visibleMemories.length === 0 && hasActiveFilter;

  // Combined toolbar: sort + view toggle — single row.
  // Always render when there are any memories so users can CLEAR filters that
  // hid everything. Previously gated on `regularMemories.length > 0`, which
  // trapped users: filter everything out → toolbar vanishes → no way to unfilter.
  //
  // The stability (new/learned/confirmed) filter was removed here: `learned`
  // is auto-assigned by the refinery and users can't action on it, and only
  // ~1.6% of memories get explicitly confirmed. The tiers are still used by
  // the backend for decay + ranking — they just aren't useful as a user-
  // facing filter. If confirm-queue UX gets real use, reinstate as a proper
  // "nurture / needs review" surface rather than a select dropdown.
  const toolbar = (memories.length > 0 || isCollection) && (
    <div
      className={isCollection ? "collection-toolbar memory-collection-toolbar" : undefined}
      style={isCollection ? undefined : {
        display: "flex",
        justifyContent: "flex-end",
        alignItems: "center",
        gap: "8px",
        position: "relative",
      }}
    >
      {isCollection && (
        <h2
          style={{
            position: "absolute",
            width: 1,
            height: 1,
            padding: 0,
            margin: -1,
            overflow: "hidden",
            clip: "rect(0, 0, 0, 0)",
            whiteSpace: "nowrap",
            border: 0,
            opacity: 0,
          }}
        >
          {t("main.memories")}
        </h2>
      )}
      {isCollection && (
        <label className="collection-search memory-collection-search">
          <Funnel size={18} aria-hidden="true" />
          <input
            className="collection-search-input"
            type="search"
            value={collectionFilter}
            onChange={(event) => updateCollectionFilter(event.target.value)}
            placeholder={t("memoryList.filterLabel")}
            aria-label={t("memoryList.filterLabel")}
          />
        </label>
      )}
      <div className={isCollection ? "collection-toolbar-actions" : "flex items-center gap-2"}>
        {isCollection && (
          <AssetLensToggle
            value={viewMode === "list" ? "rows" : "cards"}
            onChange={handleCollectionLensChange}
          />
        )}
        {/* Sort dropdown */}
        {onSortChange && memories.length > 0 && (
          <div style={{ position: "relative" }}>
            <button
              onClick={() => setSortOpen(!sortOpen)}
              className={isCollection ? "collection-control" : "flex items-center gap-1.5 px-2 py-1 rounded-md transition-colors duration-150 hover:bg-[var(--mem-hover)]"}
              aria-expanded={sortOpen}
              style={isCollection ? undefined : {
                color: sortMode !== "curated" ? "var(--mem-accent-indigo)" : "var(--mem-text-tertiary)",
                background: "none",
                border: "none",
                cursor: "pointer",
              }}
            >
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round"><path d="M3 6h18M6 12h12M9 18h6" /></svg>
              <span style={{ fontFamily: "var(--mem-font-body)", fontSize: isCollection ? "14px" : "12px" }}>
                {t(`memoryList.sort.${sortMode}`)}
              </span>
            </button>
            {sortOpen && (
              <div
                className="absolute right-0 top-full mt-1 rounded-lg shadow-lg overflow-hidden z-10"
                style={{ backgroundColor: "var(--mem-surface)", border: "1px solid var(--mem-border)", minWidth: 140 }}
              >
                {([
                  { value: "curated" as SortMode, label: t("memoryList.sort.curated") },
                  { value: "recent" as SortMode, label: t("memoryList.sort.recent") },
                  { value: "oldest" as SortMode, label: t("memoryList.sort.oldest") },
                ]).map(({ value, label }) => (
                  <button
                    key={value}
                    className="w-full text-left px-3 py-2 transition-colors duration-150 hover:bg-[var(--mem-hover)]"
                    style={{ fontFamily: "var(--mem-font-body)", fontSize: "13px", color: sortMode === value ? "var(--mem-text)" : "var(--mem-text-secondary)" }}
                    onClick={() => { onSortChange(value); setSortOpen(false); }}
                  >
                    {sortMode === value && <span className="mr-1.5">&#10003;</span>}
                    {label}
                  </button>
                ))}
              </div>
            )}
          </div>
        )}
        {toolbarActions}
        {/* View toggle */}
        {presentation !== "parent-list" && (
          <button
            onClick={toggleViewMode}
            className="p-1.5 rounded-md transition-colors duration-150 hover:bg-[var(--mem-hover)]"
            style={{ color: "var(--mem-text-tertiary)", background: "none", border: "none", cursor: "pointer", lineHeight: 0 }}
            title={viewMode === "grid" ? "Switch to list view" : "Switch to grid view"}
          >
            {viewMode === "grid" ? (
              <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round">
                <circle cx="4" cy="6" r="1" fill="currentColor" /><line x1="9" y1="6" x2="21" y2="6" />
                <circle cx="4" cy="12" r="1" fill="currentColor" /><line x1="9" y1="12" x2="21" y2="12" />
                <circle cx="4" cy="18" r="1" fill="currentColor" /><line x1="9" y1="18" x2="21" y2="18" />
              </svg>
            ) : (
              <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round">
                <rect x="3" y="3" width="7" height="7" rx="1" /><rect x="14" y="3" width="7" height="7" rx="1" />
                <rect x="3" y="14" width="7" height="7" rx="1" /><rect x="14" y="14" width="7" height="7" rx="1" />
              </svg>
            )}
          </button>
        )}
      </div>
    </div>
  );

  if (presentation === "parent-list") {
    const renderCollectionMemory = (mem: MemoryItem, i: number) => {
      const row = renderCard(mem, i);
      return viewMode === "grid"
        ? <div className="memory-collection-card" key={mem.source_id}>{row}</div>
        : row;
    };

    return (
      <MemoryListSurface
        toolbar={toolbar}
        memories={visibleMemories}
        filteredToEmpty={filteredToEmpty}
        filterActive={!!collectionNeedle}
        onClearFilter={() => updateCollectionFilter("")}
        renderMemory={renderCollectionMemory}
        undoPending={undoItem !== null}
        onUndo={handleUndo}
      />
    );
  }

  return (
    <div className="flex flex-col gap-3 pb-16">
      {/* View toggle — exposed for parent to render in toolbar */}
      {toolbar}

      {regularMemories.length > 0 ? (
        viewMode === "list" ? (
          /* ── List view (original) ── */
          <div
            className="rounded-lg overflow-hidden"
            style={{ border: "1px solid var(--mem-border)" }}
          >
            {regularMemories.map((mem, i) => renderCard(mem, i))}
          </div>
        ) : (
          /* ── Grid view (masonry) ── */
          <Masonry
            breakpointCols={{ default: 3, 900: 2, 500: 1 }}
            className="masonry-grid"
            columnClassName="masonry-column"
          >
            {regularMemories.map((mem, i) => {
              const stability = mem.stability ?? (mem.confirmed ? "confirmed" : "new");
              const tileVariant = cardVariant ?? (stability === "new" ? "insight" : "full");
              const clamp = stability === "confirmed" ? 3 : stability === "learned" ? 2 : undefined;
              return (
                <div
                  key={mem.source_id}
                  className="overflow-hidden"
                  style={{
                    border: "1px solid var(--mem-border)",
                    borderRadius: "var(--mem-r-12)",
                    marginBottom: "14px",
                    animation: `mem-fade-up 400ms cubic-bezier(0.16, 1, 0.3, 1) both`,
                    animationDelay: `${i * 40}ms`,
                  }}
                >
                  <MemoryCard
                    memory={mem}
                    variant={tileVariant}
                    lineClamp={clamp}
                    hideBorderBottom
                    onConfirm={(id, confirmed) => confirmMutation.mutate({ sourceId: id, confirmed, prevStability: mem.stability })}
                    onDelete={handleDelete}
                    expandedChain={expandedChain === mem.source_id}
                    onToggleChain={() => setExpandedChain(expandedChain === mem.source_id ? null : mem.source_id)}
                    versionChain={expandedChain === mem.source_id ? versionChain ?? [] : []}
                    onClick={onSelectMemory}
                  />
                </div>
              );
            })}
          </Masonry>
        )
      ) : filteredToEmpty ? (
        <div
          className="px-4 py-8 text-center rounded-lg flex flex-col items-center gap-3"
          style={{
            fontFamily: "var(--mem-font-body)",
            fontSize: "13px",
            color: "var(--mem-text-tertiary)",
            border: "1px solid var(--mem-border)",
          }}
        >
          <span>No memories match your filters.</span>
          {/* No "Clear filters" CTA here — the filters that can reach this
              branch (`agentFilter`, `selectedDomain`) are parent-owned, so
              the only meaningful clear is navigating back to Home. The
              earlier stability filter was the only in-component filter and
              it's been removed, so a local Clear button would be a no-op. */}
          <span
            style={{
              fontFamily: "var(--mem-font-body)",
              fontSize: "11px",
              color: "var(--mem-text-tertiary)",
              opacity: 0.7,
            }}
          >
            Go back Home to clear.
          </span>
        </div>
      ) : (
        <div
          className="px-4 py-6 text-center rounded-lg"
          style={{
            fontFamily: "var(--mem-font-body)",
            fontSize: "13px",
            color: "var(--mem-text-tertiary)",
            border: "1px solid var(--mem-border)",
          }}
        >
          Nothing here yet — agents will fill this in
        </div>
      )}

      {/* Undo toast */}
      {undoItem && (
        <div
          className="fixed bottom-4 left-1/2 -translate-x-1/2 flex items-center gap-3 px-4 py-2.5 rounded-lg shadow-lg"
          style={{
            backgroundColor: "var(--mem-text)",
            color: "var(--mem-bg)",
            fontFamily: "var(--mem-font-body)",
            fontSize: "13px",
            animation: "mem-fade-up 300ms cubic-bezier(0.16, 1, 0.3, 1)",
          }}
        >
          <span>Memory deleted</span>
          <button
            onClick={handleUndo}
            className="font-medium underline"
            style={{ color: "var(--mem-accent-glow)" }}
          >
            Undo
          </button>
        </div>
      )}
    </div>
  );
}
