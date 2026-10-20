// SPDX-License-Identifier: AGPL-3.0-only
import { WorkspaceBackButton } from "./navigation/WorkspaceNavigation";
import { useEffect, useRef, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import {
  distillReview,
  type DistillReviewResponse,
} from "../../lib/tauri";
import ReviewDialog, {
  reviewKindLabel,
  reviewKindTone,
  useReviewItemSummary,
} from "./ReviewDialog";
import { RecentRevisionsSection } from "./ReviewHistory";
import { relativeMs } from "./page/format";
import { BasisUpdatesSection } from "./page/BasisUpdatesSection";
import { BASIS_UPDATES_REVIEW_ID, useBasisPending } from "./page/useBasisActions";
import {
  reviewSuppressKey,
  useSuppressedReviewItems,
  type HiddenReviewEntry,
} from "./reviewSuppression";
import {
  reviewItemId,
  reviewItemSection,
  useReviewQueue,
  type ReviewItem,
  type ReviewSection,
} from "./useReviewQueue";
import {
  cacheDistillReviewSession,
  pageCandidateItems,
} from "./pages/pageReviewSignals";
import { isActionableReviewItem } from "./reviewDisposition";

interface DistillReviewPanelProps {
  initialReviewItemId?: string;
  onBack: () => void;
  onPageClick: (pageId: string) => void;
  onMemoryClick?: (sourceId: string) => void;
  onOpenActivity?: () => void;
}

function truncateText(value: string, max: number): string {
  if (max <= 0) return "";
  if (value.length <= max) return value;
  if (max <= 3) return ".".repeat(max);
  return `${value.slice(0, max - 3).trimEnd()}...`;
}

// Captures and informational discovery never reach this filter row.
type ReviewFilterKey = "all" | Exclude<ReviewSection, "captures" | "candidates" | "topics">;

// Mirrors useReviewQueue's SECTION_ORDER (not exported); keep in sync if that
// list changes.
const FILTER_ORDER: Exclude<ReviewFilterKey, "all">[] = [
  "revisions",
  "conflicts",
  "pages",
  "memory",
];

const FILTER_LABEL_KEYS = {
  all: "review.filterAll",
  revisions: "review.filterRevisions",
  conflicts: "review.filterConflicts",
  pages: "review.filterPages",
  memory: "review.filterMemory",
} as const;

const panelTextStyle = {
  fontFamily: "var(--mem-font-body)",
  color: "var(--mem-text)",
};

const secondaryTextStyle = {
  fontFamily: "var(--mem-font-body)",
  color: "var(--mem-text-secondary)",
};

/** Mockup section-label: small uppercase eyebrow with a mono count beside it. */
const sectionTitleStyle = {
  margin: "0 0 10px",
  fontFamily: "var(--mem-font-body)",
  fontSize: "var(--mem-text-meta)",
  fontWeight: 600,
  letterSpacing: "0.08em",
  textTransform: "uppercase" as const,
  color: "var(--mem-text-tertiary)",
  display: "flex",
  alignItems: "baseline",
  gap: 8,
};

const sectionCountStyle = {
  fontFamily: "var(--mem-font-mono)",
  fontVariantNumeric: "tabular-nums" as const,
  fontWeight: 400,
};

const emptyTitleStyle = {
  margin: "0 0 10px",
  fontFamily: "var(--mem-font-heading)",
  fontSize: "15px",
  fontWeight: 500,
  color: "var(--mem-text)",
};

const itemSurfaceStyle = {
  border: "1px solid var(--mem-border)",
  borderRadius: 8,
  backgroundColor: "var(--mem-surface)",
};

function QueueCard({
  item,
  onOpen,
}: {
  item: ReviewItem;
  onOpen: (id: string) => void;
}) {
  const { t, i18n } = useTranslation();
  // Mockup card anatomy: chip + age on top, a real title (page/entity names,
  // never the bare kind label), then the evidence line (word delta, overlap,
  // similarity) — the reason a decision is being asked for.
  const { title, reason, delta } = useReviewItemSummary(item);
  const age = item.timestampMs != null ? relativeMs(item.timestampMs, i18n.language) : null;
  const tone = reviewKindTone(item);
  const preview =
    item.kind === "page_candidate"
      ? item.cluster.contents
          .map((content) => content.trim())
          .filter((content) => content.length > 0)
          .slice(0, 2)
      : [];
  return (
    <button
      type="button"
      aria-label={t("review.openItem", { title })}
      onClick={() => onOpen(reviewItemId(item))}
      className="text-left transition-[background-color,border-color,transform] duration-150 hover:bg-[var(--mem-hover)] hover:border-[var(--mem-accent-indigo)] active:scale-[0.99] focus-visible:outline-2 focus-visible:outline-[var(--mem-accent-indigo)] focus-visible:outline-offset-2"
      style={{
        ...itemSurfaceStyle,
        display: "grid",
        gap: 6,
        width: "100%",
        padding: "13px 14px",
        color: "var(--mem-text)",
        cursor: "pointer",
        fontFamily: "var(--mem-font-body)",
      }}
    >
      <span style={{ display: "flex", alignItems: "center", gap: 8 }}>
        <span
          style={{
            fontSize: "var(--mem-text-meta)",
            letterSpacing: "0.04em",
            borderRadius: 5,
            padding: "2px 8px",
            color: tone.color,
            backgroundColor: tone.background,
          }}
        >
          {reviewKindLabel(t, item)}
        </span>
        {age && (
          <span
            style={{
              marginLeft: "auto",
              fontSize: "var(--mem-text-meta)",
              color: "var(--mem-text-tertiary)",
              whiteSpace: "nowrap",
            }}
          >
            {age}
          </span>
        )}
      </span>
      <span
        style={{
          fontFamily: "var(--mem-font-heading)",
          fontSize: "var(--mem-text-card-title)",
          fontWeight: 500,
          lineHeight: 1.35,
        }}
      >
        {title}
      </span>
      {(delta != null || reason) && (
        <span
          style={{
            ...secondaryTextStyle,
            display: "flex",
            gap: 10,
            alignItems: "baseline",
            flexWrap: "wrap",
            fontSize: "var(--mem-text-control)",
            lineHeight: 1.45,
          }}
        >
          {delta != null && (
            <span
              style={{
                fontFamily: "var(--mem-font-mono)",
                fontSize: "var(--mem-text-meta)",
                fontVariantNumeric: "tabular-nums",
              }}
            >
              <span style={{ color: "var(--mem-accent-sage)" }}>
                +{delta.added}
              </span>{" "}
              <span style={{ color: "var(--mem-status-danger-text)" }}>
                −{delta.removed}
              </span>
            </span>
          )}
          {reason}
        </span>
      )}
      {preview.map((content, index) => (
        <span
          key={index}
          style={{
            ...secondaryTextStyle,
            fontSize: "var(--mem-text-control)",
            lineHeight: 1.45,
          }}
        >
          {truncateText(content, 140)}
        </span>
      ))}
    </button>
  );
}

/** Mirrors reviewKindTone/reviewKindLabel (ReviewDialog.tsx) for the three
 * locally-hideable kinds — those helpers key off a full ReviewItem, which a
 * HiddenReviewEntry (key/label/kind/at only) doesn't carry. */
function hiddenKindTone(kind: string): { color: string; background: string } {
  const mix = (token: string) => `color-mix(in srgb, ${token} 15%, transparent)`;
  switch (kind) {
    case "stale_page":
    case "page_candidate":
      return { color: "var(--mem-accent-warm)", background: mix("var(--mem-accent-warm)") };
    case "topic":
      return { color: "var(--mem-accent-amber)", background: mix("var(--mem-accent-amber)") };
    default:
      return { color: "var(--mem-text-tertiary)", background: "var(--mem-hover)" };
  }
}

function hiddenKindLabel(t: TFunction, kind: string): string {
  switch (kind) {
    case "stale_page":
      return t("review.kindPageRefresh");
    case "page_candidate":
      return t("review.kindPageCandidate");
    case "topic":
      return t("review.kindTopic");
    // Only example items land here with these two kinds — real revisions and
    // refinements dismiss through the daemon instead (see reviewSuppression.ts).
    case "revision":
      return t("review.kindRevision");
    case "refinement":
      return t("review.kindRefinement");
    default:
      return kind;
  }
}

/** Permanent, quiet escape hatch for items hidden via the dialog's "Hide"
 * button — no timed undo toast; Restore is always one click away. */
function HiddenFooter({
  entries,
  onRestore,
  onRestoreAll,
}: {
  entries: HiddenReviewEntry[];
  onRestore: (key: string) => void;
  onRestoreAll: () => void;
}) {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState(false);

  return (
    <div>
      <button
        type="button"
        onClick={() => setExpanded((prev) => !prev)}
        className="rounded-md transition-colors duration-150 hover:bg-[var(--mem-hover)]"
        style={{
          fontFamily: "var(--mem-font-body)",
          fontSize: "var(--mem-text-meta)",
          color: "var(--mem-text-tertiary)",
          border: "none",
          background: "none",
          cursor: "pointer",
          padding: "6px 4px",
          margin: "-6px -4px",
        }}
      >
        <span
          style={{
            fontFamily: "var(--mem-font-mono)",
            fontVariantNumeric: "tabular-nums",
          }}
        >
          {t("review.hiddenCount", { count: entries.length })}
        </span>
        {` · ${t("review.showHidden")}`}
      </button>
      {expanded && (
        <section style={{ marginTop: 8 }}>
          <h2 style={sectionTitleStyle}>
            {t("review.hiddenSectionTitle")}
            <span aria-hidden="true" style={sectionCountStyle}>
              {entries.length}
            </span>
            <button
              type="button"
              onClick={onRestoreAll}
              style={{
                marginLeft: "auto",
                fontFamily: "var(--mem-font-body)",
                fontSize: "var(--mem-text-meta)",
                fontWeight: 400,
                textTransform: "none",
                letterSpacing: "normal",
                color: "var(--mem-accent-indigo)",
                border: "none",
                background: "none",
                cursor: "pointer",
              }}
            >
              {t("review.restoreAll")}
            </button>
          </h2>
          <div className="grid gap-1.5">
            {entries.map((entry) => {
              const tone = hiddenKindTone(entry.kind);
              return (
                <div
                  key={entry.key}
                  style={{
                    ...itemSurfaceStyle,
                    display: "flex",
                    alignItems: "center",
                    gap: 8,
                    padding: "8px 12px",
                  }}
                >
                  <span
                    style={{
                      fontSize: "var(--mem-text-meta)",
                      letterSpacing: "0.04em",
                      borderRadius: 5,
                      padding: "2px 8px",
                      color: tone.color,
                      backgroundColor: tone.background,
                      whiteSpace: "nowrap",
                    }}
                  >
                    {hiddenKindLabel(t, entry.kind)}
                  </span>
                  <span
                    style={{
                      ...secondaryTextStyle,
                      flex: 1,
                      minWidth: 0,
                      overflow: "hidden",
                      textOverflow: "ellipsis",
                      whiteSpace: "nowrap",
                      fontSize: "var(--mem-text-control)",
                    }}
                  >
                    {truncateText(entry.label, 60)}
                  </span>
                  <button
                    type="button"
                    onClick={() => onRestore(entry.key)}
                    style={{
                      fontFamily: "var(--mem-font-body)",
                      fontSize: "var(--mem-text-meta)",
                      color: "var(--mem-accent-indigo)",
                      border: "none",
                      background: "none",
                      cursor: "pointer",
                      whiteSpace: "nowrap",
                    }}
                  >
                    {t("review.restore")}
                  </button>
                </div>
              );
            })}
          </div>
        </section>
      )}
    </div>
  );
}

/** Wrapping pill-chip row under the header, not a segmented control — the
 * category count is dynamic (2-7 chips), which reads wrong as segments. */
function FilterChipRow({
  filter,
  onSelect,
  counts,
  allCount,
}: {
  filter: ReviewFilterKey;
  onSelect: (filter: ReviewFilterKey) => void;
  counts: Partial<Record<ReviewSection, number>>;
  allCount: number;
}) {
  const { t } = useTranslation();
  // "All" and "Revisions" always render — Revisions existing even at 0 shows
  // the section exists. Every other chip only appears once it has items.
  const chips: ReviewFilterKey[] = [
    "all",
    ...FILTER_ORDER.filter((key) => key === "revisions" || (counts[key] ?? 0) > 0),
  ];
  return (
    <div
      role="group"
      aria-label={t("review.filterLabel")}
      style={{ display: "flex", flexWrap: "wrap", gap: 8, margin: "0 0 18px" }}
    >
      {chips.map((key) => {
        const selected = filter === key;
        const count = key === "all" ? allCount : counts[key] ?? 0;
        return (
          <button
            key={key}
            type="button"
            aria-pressed={selected}
            onClick={() => onSelect(key)}
            className="transition-colors duration-150 hover:bg-[var(--mem-hover)] focus-visible:outline-2 focus-visible:outline-[var(--mem-accent-indigo)] focus-visible:outline-offset-2"
            style={{
              fontFamily: "var(--mem-font-body)",
              fontSize: "var(--mem-text-meta)",
              lineHeight: 1,
              padding: "6px 12px",
              borderRadius: 999,
              cursor: "pointer",
              border: selected
                ? "1px solid color-mix(in srgb, var(--mem-accent-indigo) 35%, transparent)"
                : "1px solid var(--mem-border)",
              color: selected ? "var(--mem-accent-indigo)" : "var(--mem-text-secondary)",
              backgroundColor: selected ? "var(--mem-indigo-bg)" : "transparent",
            }}
          >
            {t(FILTER_LABEL_KEYS[key])}
            <span
              style={{
                fontFamily: "var(--mem-font-mono)",
                fontVariantNumeric: "tabular-nums",
                fontSize: "var(--mem-text-meta)",
                marginLeft: 6,
              }}
            >
              {count}
            </span>
          </button>
        );
      })}
    </div>
  );
}

export default function DistillReviewPanel({
  initialReviewItemId,
  onBack,
  onPageClick,
  onMemoryClick,
  onOpenActivity,
}: DistillReviewPanelProps) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [lastResult, setLastResult] = useState<DistillReviewResponse | null>(null);
  const [openId, setOpenId] = useState<string | null>(null);
  // Session-only — a review queue is a to-do list, so every mount starts at
  // the top instead of remembering the last category.
  const [filter, setFilter] = useState<ReviewFilterKey>("all");
  // Stale pages refreshed this session — a new distill result resets the set.
  const [resolvedStaleIds, setResolvedStaleIds] = useState<ReadonlySet<string>>(
    new Set(),
  );
  const didLoadInitialReview = useRef(false);
  // "basis-updates" points at a group, not at one queue item.
  const focusBasisUpdates = initialReviewItemId === BASIS_UPDATES_REVIEW_ID;
  const focusedQueueItemId = focusBasisUpdates ? undefined : initialReviewItemId;
  const queue = useReviewQueue(true, focusedQueueItemId ? "note-context" : "default");
  const basisPending = useBasisPending();
  const focusedRequestRef = useRef<string | null>(null);
  const [focusedItemMissing, setFocusedItemMissing] = useState(false);
  useEffect(() => {
    if (!focusedQueueItemId || focusedRequestRef.current === focusedQueueItemId || queue.isLoading) return;
    const match = queue.items.find(item =>
      isActionableReviewItem(item) && reviewItemId(item) === focusedQueueItemId,
    );
    if (!match && queue.error) return;
    focusedRequestRef.current = focusedQueueItemId;
    setFocusedItemMissing(!match);
    if (match) {
      setFilter("all");
      setOpenId(focusedQueueItemId);
    }
  }, [focusedQueueItemId, queue.isLoading, queue.error, queue.items]);
  const { hiddenKeys, hiddenEntries, hide, restore, restoreMany } =
    useSuppressedReviewItems();
  const review = useMutation({
    mutationFn: distillReview,
    retry: false,
    onSuccess: (result) => {
      setLastResult(result);
      cacheDistillReviewSession(queryClient, result);
      setResolvedStaleIds(new Set());
    },
  });
  // Mutation errors only — a failed queue fetch (queue.error) gets its own
  // dedicated UI in the caught-up slot / section list below, so it never
  // doubles up with this box.
  const rawError = review.error;
  const error = rawError instanceof Error
    ? rawError.message
    : rawError
      ? String(rawError)
      : null;

  // Deferred past StrictMode's setup→cleanup→setup cycle: mutating inside the
  // first effect setup detaches the mutation observer on cleanup, leaving the
  // hook stuck reporting pending after the mutation has already succeeded.
  useEffect(() => {
    const id = setTimeout(() => {
      if (didLoadInitialReview.current) return;
      didLoadInitialReview.current = true;
      review.mutate();
    }, 0);
    return () => clearTimeout(id);
  }, [review.mutate]);

  // Captures, unsupported proposals and informational discovery do not ask for
  // a decision here. Their records remain in the underlying query/cache.
  const decisionItems = queue.items.filter(isActionableReviewItem);

  // Older locally hidden stale-page entries remain restorable.
  const isHiddenItem = (item: ReviewItem) => {
    const key = reviewSuppressKey(item);
    return key != null && hiddenKeys.has(key);
  };

  // Keep discovery available to the Activity surface without turning it into
  // a review decision or discarding the source records.
  const backgroundCandidates = pageCandidateItems(
    lastResult ?? undefined,
    t("review.untitledCluster"),
  );
  // Compiled pages whose sources changed — actionable: approve re-distills.
  const stalePageItems = (lastResult?.stale_pages ?? [])
    .filter((page) => !resolvedStaleIds.has(page.page_id))
    .map(
      (page): ReviewItem => ({
        kind: "stale_page",
        id: page.page_id,
        title: page.title,
        summary: page.summary ?? null,
        sourcesUpdated: page.sources_updated_count ?? null,
        timestampMs: null,
      }),
    )
    .filter((item) => !isHiddenItem(item));
  // Stale pages still offer a real refresh decision. Candidate/topic rows
  // remain in the distill result for Activity and never enter this sequence.
  const actionableItems = [...decisionItems, ...stalePageItems];
  const sectionCounts = actionableItems.reduce<Partial<Record<ReviewSection, number>>>(
    (acc, item) => {
      const section = reviewItemSection(item);
      acc[section] = (acc[section] ?? 0) + 1;
      return acc;
    },
    {},
  );
  const matchesFilter = (item: ReviewItem) =>
    filter === "all" || reviewItemSection(item) === filter;
  const dialogItems = actionableItems.filter(matchesFilter);
  const unsupportedProposals = queue.items.some(
    (item) => item.kind !== "capture" && !isActionableReviewItem(item),
  );
  const hasBackgroundNotice =
    backgroundCandidates.length > 0 ||
    (lastResult?.orphan_topics.length ?? 0) > 0 ||
    unsupportedProposals;

  // Stale-page refreshes resolve against the panel's distill result, not the
  // queue caches — drop them here after the daemon verb succeeds.
  const resolveItem = async (args: { item: ReviewItem; approve: boolean }) => {
    const { item, approve } = args;
    if (item.kind === "stale_page" && !approve) {
      hide(item);
      return;
    }
    const result = await queue.resolve(args);
    if (item.kind === "stale_page" && approve) {
      setResolvedStaleIds((prev) => new Set(prev).add(item.id));
    }
    return result;
  };

  // Section order mirrors the queue's revisions > conflicts > pages ranking.
  const sections = [
    { key: "revisions", title: t("review.sectionRevisions") },
    { key: "conflicts", title: t("review.sectionConflicts") },
    { key: "pages", title: t("review.sectionPages") },
    { key: "memory", title: t("review.sectionRefinements") },
  ].map((section) => ({
    ...section,
    items: decisionItems.filter(
      (item) => reviewItemSection(item) === section.key,
    ),
  }));

  // Candidates and topics remain background discoveries. Stale pages still
  // require a refresh decision; a cap warning also keeps the state cautious.
  const stalePageWork = stalePageItems.length > 0 || lastResult?.stale_truncated === true;
  const allCaughtUp =
    !queue.isLoading &&
    !queue.error &&
    !queue.decisionsTruncated &&
    !review.isPending &&
    !review.error &&
    decisionItems.length === 0 &&
    basisPending.entries.length === 0 &&
    !stalePageWork;

  const refresh = () => {
    review.mutate();
    queryClient.invalidateQueries({ queryKey: ["pending-revisions"] });
    queryClient.invalidateQueries({ queryKey: ["refinement-proposals"] });
    queryClient.invalidateQueries({ queryKey: ["unconfirmed-captures"] });
  };

  return (
    <div
      className="flex flex-col"
      style={{
        ...panelTextStyle,
        padding: "28px 36px",
        animation: "mem-fade-up 350ms cubic-bezier(0.16, 1, 0.3, 1) both",
      }}
    >
      <div className="flex items-center mb-5">
        <WorkspaceBackButton
          type="button"
          onClick={onBack}
          aria-label="Back"
          className="p-1.5 -ml-1.5 rounded-md transition-colors duration-150 hover:bg-[var(--mem-hover)]"
          style={{
            color: "var(--mem-text-tertiary)",
            background: "none",
            border: "none",
            cursor: "pointer",
            lineHeight: 0,
          }}
        >
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5">
            <path d="M19 12H5M12 19l-7-7 7-7" />
          </svg>
        </WorkspaceBackButton>
      </div>

      {focusedItemMissing && <p role="status" style={secondaryTextStyle}>{t("noteReview.noLongerPending")}</p>}
      <div className="flex flex-col items-stretch gap-3 sm:flex-row sm:items-start sm:justify-between sm:gap-4">
        <div>
          <h1
            style={{
              margin: 0,
              fontFamily: "var(--mem-font-heading)",
              fontSize: "22px",
              fontWeight: 500,
              color: "var(--mem-text)",
            }}
          >
            {t("review.title")}
          </h1>
          <p style={{ ...secondaryTextStyle, margin: "5px 0 0", fontSize: "var(--mem-text-control)" }}>
            {t("review.subtitle")}
          </p>
        </div>
        <div className="flex flex-wrap items-center gap-2.5 sm:flex-nowrap">
          {actionableItems.length > 0 && (
            <span
              style={{
                fontFamily: "var(--mem-font-mono)",
                fontVariantNumeric: "tabular-nums",
                fontSize: "var(--mem-text-meta)",
                color: "var(--mem-accent-indigo)",
                backgroundColor: "var(--mem-indigo-bg)",
                borderRadius: 999,
                padding: "4px 11px",
                whiteSpace: "nowrap",
              }}
            >
              {t("review.pendingCount", {
                count: queue.decisionsTruncated
                  ? `${actionableItems.length}+`
                  : actionableItems.length,
              })}
            </span>
          )}
          <button
            type="button"
            onClick={refresh}
            disabled={review.isPending}
            className="shrink-0 whitespace-nowrap rounded-md px-3 py-2 text-sm transition-colors duration-150 hover:bg-[var(--mem-hover)] disabled:opacity-60"
            style={{
              border: "1px solid var(--mem-border)",
              backgroundColor: "var(--mem-surface)",
              color: "var(--mem-text)",
              cursor: review.isPending ? "default" : "pointer",
              fontFamily: "var(--mem-font-body)",
            }}
          >
            {review.isPending ? t("review.refreshing") : t("review.refresh")}
          </button>
        </div>
      </div>

      <FilterChipRow
        filter={filter}
        onSelect={setFilter}
        counts={sectionCounts}
        allCount={actionableItems.length}
      />

      {queue.decisionsTruncated && (
        <p role="status" style={{ ...secondaryTextStyle, margin: "10px 0 0", fontSize: "var(--mem-text-meta)" }}>
          {t("noteReview.incomplete")}
        </p>
      )}

      {error && (
        <div
          role="alert"
          style={{
            ...itemSurfaceStyle,
            marginTop: 18,
            padding: "10px 12px",
            color: "var(--mem-text)",
            fontFamily: "var(--mem-font-body)",
            fontSize: "var(--mem-text-control)",
          }}
        >
          {error}
        </div>
      )}

      <div className="grid gap-6" style={{ marginTop: 24 }}>
        {filter === "all" && allCaughtUp && (
          <section>
            <h2 style={emptyTitleStyle}>
              {t(hasBackgroundNotice ? "review.noDecisionsWaiting" : "review.allCaughtUp")}
            </h2>
            <p style={{ ...secondaryTextStyle, margin: 0, fontSize: "var(--mem-text-control)" }}>
              {t("review.allCaughtUpHint")}
            </p>
          </section>
        )}

        {queue.error && actionableItems.length === 0 && (
          <section>
            <h2 style={emptyTitleStyle}>{t("review.loadFailed")}</h2>
            <button
              type="button"
              onClick={refresh}
              className="rounded-md px-3 py-2 text-sm transition-colors duration-150 hover:bg-[var(--mem-hover)]"
              style={{
                border: "1px solid var(--mem-border)",
                backgroundColor: "var(--mem-surface)",
                color: "var(--mem-text)",
                cursor: "pointer",
                fontFamily: "var(--mem-font-body)",
              }}
            >
              {t("review.retry")}
            </button>
          </section>
        )}

      {queue.error && actionableItems.length > 0 && (
        <p style={{ ...secondaryTextStyle, margin: 0, fontSize: "var(--mem-text-meta)" }}>
          {t("review.loadPartial")}
        </p>
      )}

      {hasBackgroundNotice && (
        <aside
          style={{
            ...secondaryTextStyle,
            display: "flex",
            alignItems: "center",
            justifyContent: "space-between",
            gap: 12,
            flexWrap: "wrap",
            marginTop: 12,
            fontSize: "var(--mem-text-meta)",
          }}
        >
          <span>{t("review.backgroundNotice")}</span>
          {onOpenActivity && (
            <button
              type="button"
              onClick={onOpenActivity}
              style={{
                fontFamily: "var(--mem-font-body)",
                fontSize: "var(--mem-text-meta)",
                color: "var(--mem-accent-indigo)",
                background: "none",
                border: "none",
                padding: 0,
                cursor: "pointer",
              }}
            >
              {t("review.openActivity")}
            </button>
          )}
        </aside>
      )}

        {/* Revisions keeps its own richer empty state below instead of this
         * generic box — see the "revisions" branch in the sections.map. No
         * auto-jump back to "all" when a filter empties; Show all is the
         * escape hatch. */}
        {filter !== "all" && filter !== "revisions" && (sectionCounts[filter] ?? 0) === 0 && (
          <div
            style={{
              ...itemSurfaceStyle,
              padding: "18px 16px",
              display: "grid",
              gap: 10,
              justifyItems: "start",
            }}
          >
            <p
              style={{
                margin: 0,
                fontFamily: "var(--mem-font-body)",
                fontSize: "var(--mem-text-meta)",
                color: "var(--mem-text-secondary)",
              }}
            >
              {t("review.filterEmpty")}
            </p>
            <button
              type="button"
              onClick={() => setFilter("all")}
              style={{
                fontFamily: "var(--mem-font-body)",
                fontSize: "var(--mem-text-meta)",
                color: "var(--mem-accent-indigo)",
                background: "none",
                border: "none",
                padding: 0,
                cursor: "pointer",
              }}
            >
              {t("review.filterShowAll")}
            </button>
          </div>
        )}

        {sections.map((section) => {
          const hasItems = section.items.length > 0;
          if (section.key === "revisions") {
            // Always-on once the queue has settled — proves revisions exist
            // even when nothing is pending. Hidden during a real load
            // failure: the loadFailed/loadPartial blocks above already
            // cover that state. Hidden under any other filter — the generic
            // empty box above never applies to revisions (it keeps this
            // richer empty state instead).
            if (filter !== "all" && filter !== "revisions") return null;
            if (queue.isLoading || (!hasItems && queue.error)) return null;
            return (
              <section key={section.key}>
                <h2 style={sectionTitleStyle}>
                  {section.title}
                  <span aria-hidden="true" style={sectionCountStyle}>
                    {section.items.length}
                  </span>
                </h2>
                {hasItems ? (
                  <div className="grid gap-2.5">
                    {section.items.map((item) => (
                      <QueueCard
                        key={reviewItemId(item)}
                        item={item}
                        onOpen={setOpenId}
                      />
                    ))}
                  </div>
                ) : (
                  <div style={{ ...itemSurfaceStyle, padding: "13px 14px" }}>
                    <p style={{ ...secondaryTextStyle, margin: 0, fontSize: "var(--mem-text-control)" }}>
                      {t("review.revisionsEmptyTitle")}
                    </p>
                    <p
                      style={{
                        ...secondaryTextStyle,
                        margin: "6px 0 0",
                        fontSize: "var(--mem-text-control)",
                        lineHeight: 1.5,
                        color: "var(--mem-text-tertiary)",
                      }}
                    >
                      {t("review.revisionsEmptyBody")}
                    </p>
                  </div>
                )}
              </section>
            );
          }
          return (
            (filter === "all" || filter === section.key) &&
            hasItems && (
              <section key={section.key}>
                <h2 style={sectionTitleStyle}>
                  {section.title}
                  <span aria-hidden="true" style={sectionCountStyle}>
                    {section.items.length}
                  </span>
                </h2>
                <div className="grid gap-2.5">
                  {section.items.map((item) => (
                    <QueueCard
                      key={reviewItemId(item)}
                      item={item}
                      onOpen={setOpenId}
                    />
                  ))}
                </div>
              </section>
            )
          );
        })}

        {/* Stale pages share the "pages" filter with the refinement-driven
         * pages section above. Under "all" this is unchanged (shows even at
         * zero, via pagesCurrent); under any other filter it only renders
         * with real items — the generic empty box covers "pages" at zero
         * across both sources. */}
        {lastResult &&
          (filter === "all" || (filter === "pages" && stalePageItems.length > 0)) && (
          <section>
            <h2 style={sectionTitleStyle}>
              {t("review.sectionStalePages")}
              {stalePageItems.length > 0 && (
                <span aria-hidden="true" style={sectionCountStyle}>
                  {stalePageItems.length}
                </span>
              )}
            </h2>
            {lastResult.stale_truncated && (
              <p style={{ ...secondaryTextStyle, margin: "0 0 10px", fontSize: "var(--mem-text-meta)" }}>
                {t("review.staleTruncated")}
              </p>
            )}
            {stalePageItems.length === 0 ? (
              <p style={{ ...secondaryTextStyle, margin: 0, fontSize: "var(--mem-text-control)" }}>
                {t("review.pagesCurrent")}
              </p>
            ) : (
              <div className="grid gap-2.5">
                {stalePageItems.map((item) => (
                  <QueueCard
                    key={reviewItemId(item)}
                    item={item}
                    onOpen={setOpenId}
                  />
                ))}
              </div>
            )}
          </section>
        )}

        {/* Notes whose source note changed: a group of its own, under the
         * same "pages" filter as the other note work. */}
        {(filter === "all" || filter === "pages") && (
          <BasisUpdatesSection
            entries={basisPending.entries}
            focusOnMount={focusBasisUpdates}
            onPageClick={onPageClick}
            headingStyle={sectionTitleStyle}
            countStyle={sectionCountStyle}
          />
        )}

        {/* Changelog, not category work — noise under a category filter. */}
        {(filter === "all" || filter === "revisions") && (
          <RecentRevisionsSection onPageClick={onPageClick} />
        )}

        {/* Housekeeping, not category work. */}
        {filter === "all" && hiddenEntries.some((entry) => entry.kind === "stale_page") && (
          <HiddenFooter
            entries={hiddenEntries.filter((entry) => entry.kind === "stale_page")}
            onRestore={restore}
            onRestoreAll={() => restoreMany(hiddenEntries
              .filter((entry) => entry.kind === "stale_page")
              .map((entry) => entry.key))}
          />
        )}
      </div>

      <ReviewDialog
        items={dialogItems}
        openId={openId}
        onOpenChange={setOpenId}
        onResolve={resolveItem}
        isResolving={queue.isResolving}
        onOpenMemory={onMemoryClick}
        onOpenPage={onPageClick}
      />
    </div>
  );
}
