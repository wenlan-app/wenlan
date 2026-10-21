// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import {
  agentDisplayName,
  clipboardWrite,
  daemonErrorMessage,
  isPageRevisionBasisRefused,
  isPageRevisionOutdated,
  getEntityDetail,
  getMemoryDetail,
  getPage,
  getPageSources,
  search,
} from "../../lib/tauri";
import { diffWords, diffWordCounts, type DiffSegment } from "../../lib/wordDiff";
import { isExampleReviewItem } from "./reviewExamples";
import { reviewItemId, type ReviewItem } from "./useReviewQueue";
import { PageMergeStripOff, usePageMergeEvidence } from "./ReviewPageMerge";
import { MemoryRevisionChain, PageRevisionChain } from "./ReviewHistory";
import SourceRepairReview from "./SourceRepairReview";
import { useActivity } from "../../lib/useActivity";
import { STEEP_COMMAND, SteepNeedsAiToolError } from "../../lib/steepMode";

// The writer id the desktop app sends on its own saves and rebuilds.
const APP_AGENT_ID = "wenlan-app";

export function reviewKindLabel(t: TFunction, item: ReviewItem): string {
  if (item.kind === "revision")
    return t(
      item.targetKind === "page"
        ? "review.kindPageRevision"
        : "review.kindRevision",
    );
  if (item.kind === "capture") return t("review.kindCapture");
  if (item.kind === "stale_page") return t("review.kindPageRefresh");
  if (item.kind === "page_candidate") return t("review.kindPageCandidate");
  if (item.kind === "topic") return t("review.kindTopic");
  switch (item.action) {
    case "entity_merge":
      return t("review.kindEntityMerge");
    case "detect_contradiction":
      return t("review.kindContradiction");
    case "dedup_merge":
      return t("review.kindDuplicate");
    case "relation_conflict":
      return t("review.kindRelationConflict");
    case "suggest_entity":
      return t("review.kindEntitySuggestion");
    case "page_merge":
      return t("review.kindPageMerge");
    case "cross_space_discovery":
      return t("review.kindCrossSpace");
    case "page_keep_or_archive":
      return t("review.kindPageArchive");
    case "lint_repair_review":
      return t("review.kindLintRepair");
    case "vocab_promote":
      return t("review.kindVocabPromote");
    default:
      return t("review.kindUnsupported");
  }
}

/** Distill discovery items have no daemon verb at all — the dialog shows them
 * without approve or dismiss; the daemon's compile pass consumes them. */
export function reviewReadOnly(item: ReviewItem): boolean {
  return item.kind === "page_candidate" || item.kind === "topic";
}

function pageKeepOrArchiveId(item: ReviewItem | null): string | null {
  if (
    item?.kind !== "refinement" ||
    item.action !== "page_keep_or_archive" ||
    item.payload?.action !== "page_keep_or_archive"
  ) {
    return null;
  }
  const pageId = typeof item.payload.page_id === "string" ? item.payload.page_id.trim() : "";
  return pageId.length > 0 ? pageId : null;
}

/** Only actions whose decision contract this UI understands can be approved.
 * Compare displayed payload targets with the ids the daemon actually mutates. */
export function reviewApproveBlocked(item: ReviewItem): boolean {
  if (isExampleReviewItem(item) || reviewReadOnly(item)) return true;
  const nonempty = (value: unknown): value is string =>
    typeof value === "string" && value.trim().length > 0;
  if (item.kind === "revision") return !nonempty(item.targetSourceId);
  if (item.kind === "capture" || item.kind === "stale_page") return !nonempty(item.id);
  if (item.kind !== "refinement") return true;
  const [first, second] = item.sourceIds;
  const pair = nonempty(first) && nonempty(second) && first !== second;
  const payload = item.payload;
  switch (item.action) {
    case "page_keep_or_archive":
      return pageKeepOrArchiveId(item) === null ||
        pageKeepOrArchiveId(item) !== first ||
        (payload?.action === "page_keep_or_archive" &&
          payload.allowed_actions != null && !payload.allowed_actions.includes("accept"));
    case "entity_merge":
    case "relation_conflict":
      return !pair || payload?.action !== item.action ||
        !("new_id" in payload) || payload.new_id !== first || payload.existing_id !== second;
    case "page_merge":
      return !pair || (payload != null && (payload.action !== "page_merge" ||
        payload.left_page_id !== first || payload.right_page_id !== second));
    case "detect_contradiction":
      return !pair;
    case "vocab_promote":
      return payload?.action !== "vocab_promote" ||
        !["entity", "relation"].includes(payload.kind) ||
        !nonempty(payload.old_value) || payload.old_value !== payload.old_value.trim() ||
        (payload.kind === "entity" && !item.sourceIds.every(nonempty));
    default:
      // Includes known actions requiring dedicated choice flows and future tags.
      return true;
  }
}

/** Every kind now has a dismiss-side action: read-only discovery and
 * stale-page refreshes hide locally (see DistillReviewPanel's resolveItem
 * wrapper) rather than calling a daemon dismiss verb. */
export function reviewDismissBlocked(item: ReviewItem): boolean {
  return item.kind === "refinement" && item.payload?.action === "page_keep_or_archive" &&
    item.payload.allowed_actions != null && !item.payload.allowed_actions.includes("dismiss");
}

/** Per-kind chip tone: revisions indigo, page work warm, entity merges amber,
 * conflicts danger, new entities/topics sage. */
export function reviewKindTone(item: ReviewItem): {
  color: string;
  background: string;
} {
  const mix = (token: string) =>
    `color-mix(in srgb, ${token} 15%, transparent)`;
  if (item.kind === "revision" || item.kind === "capture") {
    return {
      color: "var(--mem-accent-indigo)",
      background: "var(--mem-indigo-bg)",
    };
  }
  if (item.kind === "page_candidate" || item.kind === "stale_page") {
    return {
      color: "var(--mem-accent-warm)",
      background: mix("var(--mem-accent-warm)"),
    };
  }
  if (item.kind === "topic") {
    return {
      color: "var(--mem-accent-amber)",
      background: mix("var(--mem-accent-amber)"),
    };
  }
  switch (item.action) {
    case "page_merge":
    case "page_keep_or_archive":
      return {
        color: "var(--mem-accent-warm)",
        background: mix("var(--mem-accent-warm)"),
      };
    case "entity_merge":
    case "dedup_merge":
      return {
        color: "var(--mem-accent-amber)",
        background: mix("var(--mem-accent-amber)"),
      };
    case "detect_contradiction":
    case "relation_conflict":
      return {
        color: "var(--mem-status-danger-text)",
        background: "var(--mem-status-danger-bg)",
      };
    default:
      // suggest_entity / cross_space_discovery — new entities and spaces.
      return {
        color: "var(--mem-accent-amber)",
        background: mix("var(--mem-accent-amber)"),
      };
  }
}

export function truncateReviewText(value: string, max: number): string {
  const trimmed = value.trim().replace(/\s+/g, " ");
  if (trimmed.length <= max) return trimmed;
  return `${trimmed.slice(0, max - 3).trimEnd()}...`;
}

/** Why a page revision card needs review; nothing for a reason we do not know. */
function revisionReasonText(t: TFunction, reason: string | null | undefined): string | null {
  switch (reason) {
    case "locked": return t("review.reasonLocked");
    case "background": return t("review.reasonBackground");
    case "owned": return t("review.reasonOwned");
    default: return null;
  }
}

function sourceRepairHint(t: TFunction, item: Extract<ReviewItem, { kind: "refinement" }>): string {
  if (item.payload?.action !== "lint_repair_review") return t("sourceRepair.reviewHint");
  switch (item.payload.check_id) {
    case "memories.enrichment_failures": return t("sourceRepair.extractionHint");
    case "pages.duplicate_active_titles": return t("sourceRepair.duplicateHint");
    case "memories.semantic.classification": return t("sourceRepair.classificationHint");
    case "kg.semantic.entity_relations": return t("sourceRepair.relationHint");
    case "pages.semantic.provenance_adequacy": return t("sourceRepair.provenanceHint");
    case "pages.semantic.faithfulness": return t("sourceRepair.faithfulnessHint");
    default: return item.payload.issue;
  }
}

type ReviewLookup = "page" | "entity" | "memory" | null;

/** Resolve which two ids a review item's evidence points at, if any. */
function reviewLookupRefs(item: ReviewItem | null): {
  lookup: ReviewLookup;
  aId: string | null;
  bId: string | null;
} {
  if (item?.kind === "revision") {
    // `targetKind` comes from the daemon; a page card's target is a page id and
    // resolves through `getPage`, not `getMemoryDetail`.
    return {
      lookup: item.targetKind === "page" ? "page" : "memory",
      aId: item.targetSourceId,
      bId: null,
    };
  }
  if (item?.kind !== "refinement") return { lookup: null, aId: null, bId: null };
  switch (item.action) {
    case "lint_repair_review": {
      if (item.payload?.action !== "lint_repair_review" || item.sourceIds.length !== 1) {
        return { lookup: null, aId: null, bId: null };
      }
      const check = item.payload.check_id;
      const lookup = check === "pages.duplicate_active_titles" ||
        check === "pages.semantic.provenance_adequacy" ||
        check === "pages.semantic.faithfulness" ? "page"
        : check === "memories.enrichment_failures" || check === "memories.semantic.classification" ? "memory" : null;
      return { lookup, aId: lookup ? item.sourceIds[0] : null, bId: null };
    }
    case "page_merge":
      return {
        lookup: "page",
        aId: item.sourceIds[0] ?? null,
        bId: item.sourceIds[1] ?? null,
      };
    case "page_keep_or_archive": {
      const pageId = pageKeepOrArchiveId(item);
      return pageId !== null
        ? { lookup: "page", aId: pageId, bId: null }
        : { lookup: null, aId: null, bId: null };
    }
    case "entity_merge":
      return item.payload?.action === "entity_merge"
        ? {
            lookup: "entity",
            aId: item.payload.existing_id,
            bId: item.payload.new_id,
          }
        : { lookup: null, aId: null, bId: null };
    case "detect_contradiction":
    case "dedup_merge":
      return {
        lookup: "memory",
        aId: item.sourceIds[0] ?? null,
        bId: item.sourceIds[1] ?? null,
      };
    default:
      // relation_conflict / suggest_entity / cross_space_discovery carry
      // their evidence in the payload; their source_ids are not fetchable.
      return { lookup: null, aId: null, bId: null };
  }
}

async function fetchReviewName(
  lookup: Exclude<ReviewLookup, null>,
  id: string,
): Promise<{ name: string | null; text: string | null }> {
  if (lookup === "page") {
    const page = await getPage(id);
    return { name: page?.title ?? null, text: page?.content ?? null };
  }
  if (lookup === "entity") {
    const detail = await getEntityDetail(id);
    return { name: detail?.entity.name ?? null, text: null };
  }
  const detail = await getMemoryDetail(id);
  return {
    name: detail?.title?.trim() || detail?.content || null,
    text: detail?.content ?? null,
  };
}

/**
 * Rich card/rail summary for a review item: fetches the names its ids point
 * at and surfaces the payload evidence (page names, overlap, similarity, word
 * delta) so an item never reads as just its kind label. Falls back to the
 * kind label while names load.
 */
export function useReviewItemSummary(item: ReviewItem | null): {
  title: string;
  reason: string | null;
  delta: { added: number; removed: number } | null;
} {
  const { t } = useTranslation();
  const { lookup, aId, bId } = reviewLookupRefs(item);
  const a = useQuery({
    queryKey: ["review-summary", lookup, aId],
    queryFn: () => fetchReviewName(lookup as Exclude<ReviewLookup, null>, aId as string),
    enabled: lookup != null && aId != null,
    staleTime: 60_000,
  });
  const b = useQuery({
    queryKey: ["review-summary", lookup, bId],
    queryFn: () => fetchReviewName(lookup as Exclude<ReviewLookup, null>, bId as string),
    enabled: lookup != null && bId != null,
    staleTime: 60_000,
  });
  const delta = useMemo(
    () =>
      item?.kind === "revision" && a.data?.text
        ? diffWordCounts(diffWords(a.data.text, item.content))
        : null,
    [item, a.data],
  );
  if (!item) return { title: "", reason: null, delta: null };

  const short = (value: string | null | undefined, max = 36): string | null => {
    const trimmed = value?.trim();
    return trimmed ? truncateReviewText(trimmed, max) : null;
  };
  const aName = short(a.data?.name);
  const bName = short(b.data?.name);
  let title: string | null = null;
  let reason: string | null = null;
  if (item.kind === "revision") {
    title = short(a.data?.name, 96) ?? short(item.content, 96);
    reason = item.targetKind === "page" && item.agent === "page_write"
      ? t("review.pageUpdate")
      : item.agent
        // The app's own writer id reads as "Wenlan"; other agents keep the
        // name they sent.
        ? t("review.proposedBy", { agent: item.agent === APP_AGENT_ID ? agentDisplayName(item.agent) : item.agent })
        : null;
  } else if (item.kind === "capture") {
    title = short(item.title, 96);
    reason = short(item.snippet, 96);
  } else if (item.kind === "page_candidate") {
    title = item.title;
    reason =
      (item.cluster.new_memory_count != null
        ? t("review.newSources", { count: item.cluster.new_memory_count })
        : t("review.sources", { count: item.cluster.source_ids.length })) +
      (item.cluster.existing_page_id
        ? ` · ${t("review.linkedExistingPage")}`
        : "");
  } else if (item.kind === "topic") {
    title = item.label;
    reason = t("review.topicReason", { count: item.count });
  } else if (item.kind === "stale_page") {
    title = item.title;
    reason =
      item.sourcesUpdated != null
        ? t("review.sourcesUpdated", { count: item.sourcesUpdated })
        : null;
  } else {
    const confidence = t("review.confidence", {
      percent: Math.round(item.confidence * 100),
    });
    reason = confidence;
    switch (item.action) {
      case "lint_repair_review":
        title = short(a.data?.name, 96);
        reason = sourceRepairHint(t, item);
        break;
      case "page_merge":
        if (aName && bName)
          title = t("review.pageMergeTitle", { keep: aName, absorb: bName });
        if (item.payload?.action === "page_merge")
          reason = t("review.mergeReason", {
            count: item.payload.source_overlap,
            percent: Math.round(item.payload.source_overlap_ratio * 100),
          });
        break;
      case "entity_merge":
        if (aName && bName)
          title = t("review.entityMergeTitle", { a: aName, b: bName });
        if (item.payload?.action === "entity_merge")
          reason = t("review.similarity", {
            percent: Math.round(item.payload.similarity * 100),
          });
        break;
      case "detect_contradiction":
        if (aName && bName)
          title = t("review.contradictionTitle", { a: aName, b: bName });
        break;
      case "dedup_merge":
        if (aName && bName)
          title = t("review.dedupTitle", { a: aName, b: bName });
        break;
      case "page_keep_or_archive":
        title = short(a.data?.name, 96);
        if (item.payload?.action === "page_keep_or_archive")
          reason = `${t("review.sources", { count: item.payload.source_count })} · ${confidence}`;
        break;
      case "relation_conflict":
        if (item.payload?.action === "relation_conflict")
          title = `${item.payload.from} → ${item.payload.to}`;
        break;
      case "suggest_entity":
        if (item.payload?.action === "suggest_entity")
          title = short(item.payload.name_hint, 96);
        break;
      case "cross_space_discovery":
        if (item.payload?.action === "cross_space_discovery")
          title = item.payload.spaces.join(" · ");
        break;
      case "vocab_promote":
        if (item.payload?.action === "vocab_promote")
          title = item.payload.old_value;
        break;
    }
  }
  return { title: title ?? reviewKindLabel(t, item), reason, delta };
}

const INS_STYLE: React.CSSProperties = {
  backgroundColor: "color-mix(in srgb, var(--mem-accent-sage) 24%, transparent)",
  textDecoration: "none",
  borderRadius: 3,
  padding: "0 2px",
};

const DEL_STYLE: React.CSSProperties = {
  backgroundColor: "var(--mem-status-danger-bg)",
  borderRadius: 3,
  padding: "0 2px",
};

const paneStyle: React.CSSProperties = {
  border: "1px solid var(--mem-border)",
  borderRadius: 10,
  backgroundColor: "var(--mem-detail-surface-raised)",
  padding: "13px 15px",
  fontFamily: "var(--mem-font-body)",
  fontSize: 14,
  lineHeight: 1.65,
  color: "var(--mem-text)",
  whiteSpace: "pre-wrap",
  overflowWrap: "anywhere",
};

const paneLabelStyle: React.CSSProperties = {
  fontFamily: "var(--mem-font-body)",
  fontSize: "var(--mem-text-meta)",
  letterSpacing: "0.08em",
  textTransform: "uppercase",
  color: "var(--mem-text-secondary)",
  margin: "0 0 7px",
};

const actionButtonStyle: React.CSSProperties = {
  fontFamily: "var(--mem-font-body)",
  fontSize: "var(--mem-text-control)",
  borderRadius: 8,
  padding: "8px 15px",
  cursor: "pointer",
  border: "1px solid var(--mem-border)",
  backgroundColor: "var(--mem-surface)",
  color: "var(--mem-text)",
};

/** Dashed pill marking a dev-only sample item (see reviewExamples.ts) — card
 * and dialog share this recipe. */
const examplePillStyle: React.CSSProperties = {
  fontFamily: "var(--mem-font-mono)",
  fontSize: "var(--mem-text-meta)",
  letterSpacing: "0.06em",
  textTransform: "uppercase",
  borderRadius: 5,
  padding: "1px 7px",
  color: "var(--mem-text-secondary)",
  border: "1px dashed var(--mem-border)",
  whiteSpace: "nowrap",
};

/** On-open evidence for topic/suggest_entity cards: their daemon payloads
 * carry no source ids to look up, so the dialog runs a search for the topic
 * label / entity name hint instead — never per-card, only once the dialog is
 * actually open. */
function useReviewEvidence(query: string | null) {
  return useQuery({
    queryKey: ["review-evidence", query],
    queryFn: () => search(query as string, 4),
    enabled: query != null,
    staleTime: 60_000,
  });
}

const evidenceRowStyle: React.CSSProperties = {
  display: "block",
  width: "100%",
  textAlign: "left",
  border: "1px solid var(--mem-border)",
  borderRadius: 8,
  padding: "8px 12px",
  backgroundColor: "var(--mem-surface)",
  cursor: "pointer",
};

function ReviewEvidencePane({
  query,
  onOpenMemory,
}: {
  query: string | null;
  onOpenMemory?: (sourceId: string) => void;
}) {
  const { t } = useTranslation();
  const evidence = useReviewEvidence(query);
  if (query == null) return null;
  const results = (evidence.data ?? []).slice(0, 3);
  return (
    <div>
      <p style={paneLabelStyle}>{t("review.mentionedIn")}</p>
      {evidence.isLoading ? (
        <div style={paneStyle}>{t("review.loadingCurrent")}</div>
      ) : evidence.isError ? (
        <div role="alert" style={paneStyle}>
          {t("review.evidenceError")}
          <button type="button" style={actionButtonStyle} onClick={() => void evidence.refetch()}>
            {t("review.retryEvidence")}
          </button>
        </div>
      ) : results.length === 0 ? (
        <div style={paneStyle}>{t("review.evidenceNone")}</div>
      ) : (
        <div style={{ display: "grid", gap: 8 }}>
          {results.map((result) => (
            <button
              key={result.id}
              type="button"
              onClick={() => onOpenMemory?.(result.source_id)}
              style={evidenceRowStyle}
            >
              <div
                style={{
                  fontFamily: "var(--mem-font-heading)",
                  fontWeight: 500,
                  fontSize: "var(--mem-text-control)",
                  color: "var(--mem-text)",
                }}
              >
                {result.title || truncateReviewText(result.content, 60)}
              </div>
              <div
                style={{
                  fontFamily: "var(--mem-font-body)",
                  fontSize: "var(--mem-text-meta)",
                  color: "var(--mem-text-secondary)",
                  marginTop: 2,
                }}
              >
                {truncateReviewText(result.content, 100)}
              </div>
            </button>
          ))}
          <p
            style={{
              fontFamily: "var(--mem-font-body)",
              color: "var(--mem-text-secondary)",
              fontSize: "var(--mem-text-meta)",
              margin: 0,
            }}
          >
            {t("review.evidenceBySearch")}
          </p>
        </div>
      )}
    </div>
  );
}

export function DiffText({ segments }: { segments: DiffSegment[] }) {
  return (
    <>
      {segments.map((segment, index) =>
        segment.kind === "ins" ? (
          <ins key={index} style={INS_STYLE}>
            {segment.text}
          </ins>
        ) : segment.kind === "del" ? (
          <del key={index} style={DEL_STYLE}>
            {segment.text}
          </del>
        ) : (
          <span key={index}>{segment.text}</span>
        ),
      )}
    </>
  );
}

interface ReviewDialogProps {
  items: ReviewItem[];
  openId: string | null;
  onOpenChange: (id: string | null) => void;
  onResolve: (args: { item: ReviewItem; approve: boolean }) => Promise<unknown>;
  isResolving: boolean;
  onOpenMemory?: (sourceId: string) => void;
  onOpenPage?: (pageId: string) => void;
}

export default function ReviewDialog({
  items,
  openId,
  onOpenChange,
  onResolve,
  isResolving,
  onOpenMemory,
  onOpenPage,
}: ReviewDialogProps) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [showDone, setShowDone] = useState(false);
  const [sideBySide, setSideBySide] = useState(false);
  const [flash, setFlash] = useState<string | null>(null);
  // A notice flash explains an outcome the human did not choose (a card the
  // daemon discarded), so it stays up longer and carries no checkmark.
  const [flashNotice, setFlashNotice] = useState(false);
  // The sentence shown when a resolve fails, not just a flag. Some refusals
  // are permanent and say so — a revision card staged before source-revision
  // fencing is discarded and its page re-queued, so "Try again" would be an
  // instruction to do something that can never work. Falls back to the
  // generic wording when the daemon sent no explanation.
  const [resolveError, setResolveError] = useState<string | null>(null);
  const [resolvingLocally, setResolvingLocally] = useState(false);
  const [repairBusy, setRepairBusy] = useState(false);
  const [repairActionHost, setRepairActionHost] = useState<HTMLDivElement | null>(null);
  const repairBusyRef = useRef(false);
  const displayedItemRef = useRef<ReviewItem | null>(null);
  const retainedRepairItemRef = useRef<ReviewItem | null>(null);
  const handleRepairBusy = useCallback((busy: boolean) => {
    if (busy && !repairBusyRef.current) {
      retainedRepairItemRef.current = displayedItemRef.current;
    } else if (!busy) {
      retainedRepairItemRef.current = null;
    }
    repairBusyRef.current = busy;
    setRepairBusy(busy);
  }, []);
  const [repairCopyState, setRepairCopyState] = useState<"idle" | "copying" | "copied" | "error">("idle");
  const dialogRef = useRef<HTMLDivElement>(null);
  const navigationVersionRef = useRef(0);
  const resolveRequestRef = useRef(0);

  const open = openId != null;
  const foundIndex = items.findIndex((entry) => reviewItemId(entry) === openId);
  const index = foundIndex >= 0 ? foundIndex : items.length > 0 ? 0 : -1;
  // A queue refresh may remove/reorder a proposal while its exact repair is
  // applying or awaiting verification. Keep that recovery surface mounted.
  const item = repairBusy && retainedRepairItemRef.current
    ? retainedRepairItemRef.current
    : showDone ? null : index >= 0 ? items[index] : null;
  displayedItemRef.current = item;
  const done = open && !repairBusy && (showDone || items.length === 0);
  const resolving = isResolving || resolvingLocally || repairBusy;
  const summary = useReviewItemSummary(item);
  // "Refresh page" rebuilds with the daemon's model. With none, the card stays
  // and hands the update to the user's AI tool instead of silently vanishing.
  const activity = useActivity().data;
  const [staleNeedsAiToolIds, setStaleNeedsAiToolIds] = useState<ReadonlySet<string>>(() => new Set());
  const noDaemonModel = activity !== undefined && !activity.everyday.available && !activity.synthesis.available;
  const staleNeedsAiTool = item?.kind === "stale_page" && (noDaemonModel || staleNeedsAiToolIds.has(item.id));
  const copySteepCommand = async () => {
    try {
      await clipboardWrite(STEEP_COMMAND);
      setFlash(t("review.commandCopied"));
      window.setTimeout(() => setFlash(null), 1200);
    } catch {
      setResolveError(t("review.actionError"));
    }
  };

  // The diff's "before" side. Both revision kinds resolve through the same two
  // accept/dismiss verbs, but a memory revision reads its target from the
  // memory detail and a page revision reads the page itself — two caches, two
  // shapes, so they stay two queries and are folded together below.
  const detailSourceId =
    item?.kind === "revision" && item.targetKind === "memory"
      ? item.targetSourceId
      : item?.kind === "capture"
        ? item.id
        : null;
  const target = useQuery({
    queryKey: ["memory-detail", detailSourceId],
    queryFn: () => getMemoryDetail(detailSourceId as string),
    enabled: detailSourceId != null,
  });
  const targetPageId =
    item?.kind === "revision" && item.targetKind === "page"
      ? item.targetSourceId
      : null;
  const targetPage = useQuery({
    queryKey: ["page", targetPageId],
    queryFn: () => getPage(targetPageId as string),
    enabled: targetPageId != null,
  });

  // Actions whose source_ids are memory ids; the rest point at entities,
  // pages, or relations and get dedicated panes below. suggest_entity's
  // source_ids aren't fetchable memory ids at all (see reviewLookupRefs
  // above) — it gets a search-backed evidence pane instead.
  const memoryPaneIds =
    item?.kind === "refinement" &&
    (item.action === "detect_contradiction" ||
      item.action === "dedup_merge" ||
      item.action === "cross_space_discovery")
      ? item.sourceIds.slice(0, 2)
      : [];
  const memoryPaneA = useQuery({
    queryKey: ["memory-detail", memoryPaneIds[0] ?? null],
    queryFn: () => getMemoryDetail(memoryPaneIds[0]),
    enabled: memoryPaneIds.length > 0,
  });
  const memoryPaneB = useQuery({
    queryKey: ["memory-detail", memoryPaneIds[1] ?? null],
    queryFn: () => getMemoryDetail(memoryPaneIds[1]),
    enabled: memoryPaneIds.length > 1,
  });

  const archivePageId = pageKeepOrArchiveId(item);
  const archivePage = useQuery({
    queryKey: ["page", archivePageId],
    queryFn: () => getPage(archivePageId as string),
    enabled: archivePageId != null,
  });

  const mergePayload =
    item?.kind === "refinement" && item.payload?.action === "entity_merge"
      ? item.payload
      : null;
  const mergeExisting = useQuery({
    queryKey: ["entity-detail", mergePayload?.existing_id ?? null],
    queryFn: () => getEntityDetail(mergePayload?.existing_id as string),
    enabled: mergePayload != null,
  });
  const mergeIncoming = useQuery({
    queryKey: ["entity-detail", mergePayload?.new_id ?? null],
    queryFn: () => getEntityDetail(mergePayload?.new_id as string),
    enabled: mergePayload != null,
  });

  const beforeTitle = target.data?.title ?? targetPage.data?.title ?? null;
  const beforeContent = target.data?.content ?? targetPage.data?.content ?? "";
  const beforeLoading = target.isLoading || targetPage.isLoading;
  const beforeLoaded = target.data != null || targetPage.data != null;
  const segments = useMemo(
    () =>
      item?.kind === "revision" && beforeLoaded
        ? diffWords(beforeContent, item.content)
        : [],
    [item, beforeLoaded, beforeContent],
  );
  const wordCounts = useMemo(() => diffWordCounts(segments), [segments]);

  const isContradiction =
    item?.kind === "refinement" && item.action === "detect_contradiction";
  // Daemon order: source_ids[0] is the new memory, source_ids[1] the existing
  // one — so pane A holds "after" and pane B holds "before".
  const contradictionSegments = useMemo(
    () =>
      isContradiction && memoryPaneA.data && memoryPaneB.data
        ? diffWords(memoryPaneB.data.content, memoryPaneA.data.content)
        : [],
    [isContradiction, memoryPaneA.data, memoryPaneB.data],
  );

  useEffect(() => {
    if (!open) return;
    const previouslyFocused =
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;
    dialogRef.current?.focus();
    return () => {
      if (previouslyFocused?.isConnected) previouslyFocused.focus();
    };
  }, [open]);

  useEffect(() => {
    navigationVersionRef.current += 1;
    setResolveError(null);
    setRepairCopyState("idle");
  }, [openId]);

  const pageMerge = usePageMergeEvidence(
    item?.kind === "refinement" && item.action === "page_merge" ? item.sourceIds[0] ?? "" : "",
    item?.kind === "refinement" && item.action === "page_merge" ? item.sourceIds[1] ?? "" : "",
  );
  const archiveSources = useQuery({
    queryKey: ["page-sources", archivePageId],
    queryFn: () => getPageSources(archivePageId as string),
    enabled: archivePageId != null,
  });
  const vocabulary = item?.kind === "refinement" && item.action === "vocab_promote" &&
    item.payload?.action === "vocab_promote" ? item.payload : null;
  const vocabularyEntityIds = vocabulary?.kind === "entity" && item?.kind === "refinement"
    ? [...new Set(item.sourceIds.filter((id) => typeof id === "string" && id.trim().length > 0))] : [];
  const vocabularyEntities = useQueries({
    queries: vocabularyEntityIds.map((id) => ({
      queryKey: ["entity-detail", id],
      queryFn: () => getEntityDetail(id),
    })),
  });
  const decisionQueries = item?.kind === "revision"
    ? [item.targetKind === "page" ? targetPage : target]
    : item?.kind === "capture" ? [target]
    : item?.kind === "refinement" && item.action === "page_keep_or_archive" ? [archivePage, archiveSources]
    : item?.kind === "refinement" && item.action === "page_merge"
      ? [pageMerge.keepPageQ, pageMerge.retirePageQ, pageMerge.keepSourcesQ, pageMerge.retireSourcesQ]
    : item?.kind === "refinement" && item.action === "entity_merge" ? [mergeExisting, mergeIncoming]
    : vocabulary ? vocabularyEntities
    : isContradiction ? [memoryPaneA, memoryPaneB] : [];
  const evidenceLoading = decisionQueries.some((query) => query.isLoading);
  const sourceLists = item?.kind === "refinement" && item.action === "page_keep_or_archive" ? [archiveSources.data]
    : item?.kind === "refinement" && item.action === "page_merge" ? [pageMerge.keepSourcesQ.data, pageMerge.retireSourcesQ.data] : [];
  const missingSources = sourceLists.some((sources) => sources?.some((entry) => entry.memory == null));
  const missingMergeSources = missingSources && item?.kind === "refinement" && item.action === "page_merge";
  const evidenceFailed = decisionQueries.some((query) => query.isError);
  const vocabularyTargetMismatch = vocabularyEntities.some((query, index) => query.isSuccess && query.data?.entity?.id !== vocabularyEntityIds[index]);
  const missingTarget = decisionQueries.some((query) => query.isSuccess && query.data == null) || vocabularyTargetMismatch;
  const evidenceUnavailable = decisionQueries.some((query) => query.isError || query.data == null) || missingMergeSources || vocabularyTargetMismatch;
  const contractBlocked = item != null && reviewApproveBlocked(item);
  const canApprove = !contractBlocked && !evidenceLoading && !evidenceUnavailable;
  const retryEvidence = () => { decisionQueries.forEach((query) => void query.refetch()); };

  const copyRepairDetails = async () => {
    if (item?.kind !== "refinement" || item.payload?.action !== "lint_repair_review" || repairCopyState === "copying") return;
    const version = navigationVersionRef.current;
    setRepairCopyState("copying");
    try {
      // This is a reference packet, never an approval or a prepared manifest.
      await clipboardWrite(JSON.stringify({ review_id: item.id, source_ids: item.sourceIds, ...item.payload }, null, 2));
      if (version === navigationVersionRef.current) setRepairCopyState("copied");
    } catch {
      if (version === navigationVersionRef.current) setRepairCopyState("error");
    }
  };

  const resolveCurrent = async (approve: boolean) => {
    if (!item || resolving || repairBusyRef.current) return;
    // reviewApproveBlocked already folds in reviewReadOnly — read-only kinds
    // (topic/page_candidate) can still dismiss (hide) even though they can't
    // approve.
    if (approve && !canApprove) return;
    if (!approve && reviewDismissBlocked(item)) return;
    const isCapture = item.kind === "capture";
    const isConflict =
      item.kind === "refinement" && item.action === "detect_contradiction";
    const isLocalHide =
      item.kind === "topic" ||
      item.kind === "page_candidate" ||
      item.kind === "stale_page";
    const next = items[index + 1] ?? (index > 0 ? items[index - 1] : null);
    const navigationVersion = navigationVersionRef.current;
    const requestId = ++resolveRequestRef.current;
    setResolveError(null);
    setResolvingLocally(true);
    let outdated = false;
    let discardedNotice: "review.revisionOutdated" | "review.revisionBasisRefused" = "review.revisionOutdated";
    try {
      await onResolve({ item, approve });
    } catch (error) {
      if (error instanceof SteepNeedsAiToolError && item.kind === "stale_page") {
        const staleId = item.id;
        setStaleNeedsAiToolIds((ids) => new Set(ids).add(staleId));
        return;
      }
      // The page moved after this revision was proposed, or a page the change
      // relied on is gone; either way the daemon discarded the card. Say so
      // and move on, the way a resolved card would.
      const basisRefused = item.kind === "revision" && isPageRevisionBasisRefused(error);
      if (basisRefused) discardedNotice = "review.revisionBasisRefused";
      outdated = basisRefused || (item.kind === "revision" && isPageRevisionOutdated(error));
      if (
        !outdated &&
        resolveRequestRef.current === requestId &&
        navigationVersionRef.current === navigationVersion
      ) {
        const code = daemonErrorMessage(error);
        setResolveError(code === "repair_write_fence_conflict"
          ? t("sourceRepair.sourceBusy")
          : code ?? t("review.actionError"));
      }
      if (!outdated) return;
    } finally {
      if (resolveRequestRef.current === requestId) {
        setResolvingLocally(false);
      }
    }
    if (
      resolveRequestRef.current !== requestId ||
      navigationVersionRef.current !== navigationVersion
    ) {
      return;
    }
    if (outdated) {
      setFlashNotice(true);
      setFlash(t(discardedNotice));
      window.setTimeout(() => { setFlash(null); setFlashNotice(false); }, 2400);
      if (next) onOpenChange(reviewItemId(next));
      else setShowDone(true);
      return;
    }
    setFlash(
      approve
        ? t(
            isCapture
              ? "review.confirmed"
              : isConflict
                ? "review.resolved"
                : "review.approved",
          )
        : t(
            isCapture
              ? "review.forgotten"
              : isConflict
                ? "review.keptBoth"
                : isLocalHide
                  ? "review.hidden"
                  : "review.dismissed",
          ),
    );
    window.setTimeout(() => setFlash(null), 450);
    if (next) onOpenChange(reviewItemId(next));
    else setShowDone(true);
  };

  const goTo = (offset: number) => {
    if (items.length < 2 || repairBusyRef.current) return;
    const nextIndex = (index + offset + items.length) % items.length;
    navigationVersionRef.current += 1;
    onOpenChange(reviewItemId(items[nextIndex]));
  };

  const close = () => {
    if (repairBusyRef.current) return;
    navigationVersionRef.current += 1;
    setShowDone(false);
    onOpenChange(null);
  };

  useEffect(() => {
    if (!open) return;
    const onKeyDown = (event: KeyboardEvent) => {
      const el = event.target as HTMLElement | null;
      if (event.key === "Tab") {
        const container = dialogRef.current;
        if (!container) return;
        const focusable = Array.from(
          container.querySelectorAll<HTMLElement>(
            'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
          ),
        );
        if (focusable.length === 0) {
          event.preventDefault();
          container.focus();
          return;
        }
        const first = focusable[0];
        const last = focusable[focusable.length - 1];
        const active = document.activeElement;
        const activeIsFocusable = active instanceof HTMLElement && focusable.includes(active);
        if (event.shiftKey && (!activeIsFocusable || active === first)) {
          event.preventDefault();
          last.focus();
        } else if (!event.shiftKey && (!activeIsFocusable || active === last)) {
          event.preventDefault();
          first.focus();
        }
        return;
      }
      if (event.key === "Escape") {
        event.preventDefault();
        close();
        return;
      }
      if (
        el?.closest(
          'a[href], button, input, select, textarea, [contenteditable="true"], [role="button"]',
        )
      ) {
        return;
      }
      switch (event.key) {
        case "d":
        case "D":
          void resolveCurrent(false);
          break;
        case "ArrowRight":
          goTo(1);
          break;
        case "ArrowLeft":
          goTo(-1);
          break;
      }
    };
    // The modal owns Escape before the shell's bubbling navigation shortcut.
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  });

  if (!open) return null;

  const heading = done
    ? t("review.reviewComplete")
    : item?.kind === "revision"
      ? (beforeTitle?.trim() || truncateReviewText(item.content, 72))
      : item?.kind === "capture"
        ? truncateReviewText(item.title, 72)
        : item?.kind === "page_candidate" || item?.kind === "stale_page"
          ? item.title
          : item?.kind === "topic"
            ? item.label
            : item
              ? // Refinements: the resolved names ("A" and "B" look like the
                // same entity), falling back to the kind label while loading.
                summary.title
              : "";

  // Keep viewport positioning independent of the review page's entry transform.
  return createPortal(
    <div
      role="dialog"
      aria-modal="true"
      aria-label={t("review.title")}
      style={{
        position: "fixed",
        inset: 0,
        display: "flex",
        alignItems: "flex-start",
        justifyContent: "center",
        padding: "6vh 16px 16px",
        backgroundColor: "rgba(0,0,0,0.45)",
        zIndex: 1100,
      }}
      onClick={(event) => {
        if (event.target === event.currentTarget) close();
      }}
    >
      <div
        ref={dialogRef}
        tabIndex={-1}
        style={{
          position: "relative",
          width: "min(760px, 100%)",
          maxHeight: "86vh",
          overflowY: item?.kind === "refinement" && item.action === "lint_repair_review" ? "hidden" : "auto",
          backgroundColor: "var(--mem-surface)",
          border: "1px solid var(--mem-border)",
          borderRadius: 16,
          boxShadow: "0 24px 48px rgba(0,0,0,0.35)",
          outline: "none",
          display: "flex",
          flexDirection: "column",
        }}
      >
        {flash && (
          <div
            style={{
              position: "absolute",
              inset: 0,
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              gap: 10,
              backgroundColor: "var(--mem-surface)",
              borderRadius: 16,
              zIndex: 3,
              fontFamily: "var(--mem-font-heading)",
              fontSize: flashNotice ? 16 : 19,
              color: flashNotice
                ? "var(--mem-text)"
                : "var(--mem-status-success-text)",
              padding: "0 32px",
              textAlign: "center",
            }}
          >
            {flashNotice ? flash : `${flash} ✓`}
          </div>
        )}

        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 10,
            padding: "16px 20px 14px",
            borderBottom: "1px solid var(--mem-detail-divider)",
          }}
        >
          {item && (
            <span
              style={{
                fontFamily: "var(--mem-font-body)",
                fontSize: "var(--mem-text-meta)",
                letterSpacing: "0.04em",
                borderRadius: 5,
                padding: "2px 8px",
                color: reviewKindTone(item).color,
                backgroundColor: reviewKindTone(item).background,
              }}
            >
              {reviewKindLabel(t, item)}
            </span>
          )}
          {item && isExampleReviewItem(item) && (
            <span style={examplePillStyle}>{t("review.exampleBadge")}</span>
          )}
          <span
            style={{
              marginLeft: "auto",
              fontFamily: "var(--mem-font-mono)",
              fontVariantNumeric: "tabular-nums",
              fontSize: "var(--mem-text-meta)",
              color: "var(--mem-text-secondary)",
            }}
          >
            {item
              ? t("review.progress", {
                  position: index + 1,
                  total: items.length,
                })
              : ""}
          </span>
          <button
            type="button"
            aria-label={t("review.close")}
            disabled={repairBusy}
            onClick={close}
            style={{
              background: "none",
              border: "none",
              color: "var(--mem-text-secondary)",
              cursor: "pointer",
              fontSize: 16,
              lineHeight: 1,
              padding: 6,
              borderRadius: 6,
            }}
          >
            ✕
          </button>
        </div>

        {done ? (
          <div style={{ textAlign: "center", padding: "40px 24px 44px" }}>
            <div
              style={{
                width: 52,
                height: 52,
                borderRadius: "50%",
                margin: "0 auto 16px",
                backgroundColor: "var(--mem-status-success-bg)",
                color: "var(--mem-status-success-text)",
                display: "flex",
                alignItems: "center",
                justifyContent: "center",
                fontSize: 24,
              }}
            >
              ✓
            </div>
            <h3
              style={{
                fontFamily: "var(--mem-font-heading)",
                fontWeight: 500,
                fontSize: 20,
                margin: "0 0 6px",
                color: "var(--mem-text)",
              }}
            >
              {t("review.reviewComplete")}
            </h3>
            <p
              style={{
                fontFamily: "var(--mem-font-body)",
                color: "var(--mem-text-secondary)",
                fontSize: "var(--mem-text-control)",
                margin: "0 0 20px",
              }}
            >
              {t("review.reviewCompleteHint")}
            </p>
            <button
              type="button"
              onClick={close}
              style={{
                ...actionButtonStyle,
                backgroundColor: "var(--mem-accent-indigo)",
                borderColor: "var(--mem-accent-indigo)",
                color: "var(--mem-bg)",
                fontWeight: 600,
              }}
            >
              {t("review.backToReview")}
            </button>
          </div>
        ) : item ? (
          <>
            <div style={{ padding: "20px 22px 8px", ...(item.kind === "refinement" && item.action === "lint_repair_review" ? {
              minHeight: 0, flex: 1, overflowY: "auto" as const,
            } : {}) }}>
              <h3
                style={{
                  fontFamily: "var(--mem-font-heading)",
                  fontWeight: 500,
                  fontSize: 19,
                  margin: "0 0 3px",
                  color: "var(--mem-text)",
                }}
              >
                {heading}
              </h3>
              <p
                style={{
                  fontFamily: "var(--mem-font-body)",
                  color: "var(--mem-text-secondary)",
                  fontSize: "var(--mem-text-description)",
                  margin: "0 0 18px",
                }}
              >
                {item.kind === "revision"
                  ? <>
                      {summary.reason}
                      {revisionReasonText(t, item.reviewReason) && (
                        <span data-testid="review-reason" style={{ display: "block" }}>
                          {revisionReasonText(t, item.reviewReason)}
                        </span>
                      )}
                    </>
                  : item.kind === "capture"
                    ? t("review.captureHint")
                    : item.kind === "page_candidate"
                      ? (item.cluster.new_memory_count != null
                          ? t("review.newSources", {
                              count: item.cluster.new_memory_count,
                            })
                          : t("review.sources", {
                              count: item.cluster.source_ids.length,
                            })) +
                        (item.cluster.existing_page_id
                          ? ` · ${t("review.linkedExistingPage")}`
                          : "")
                      : item.kind === "topic"
                        ? t("review.topicReason", { count: item.count })
                        : item.kind === "stale_page"
                          ? (item.sourcesUpdated != null
                              ? t("review.sourcesUpdated", {
                                  count: item.sourcesUpdated,
                                })
                              : "")
                        : isContradiction
                          ? t("review.contradictionHint")
                      : item.action === "lint_repair_review"
                        ? sourceRepairHint(t, item)
                      : item.action === "relation_conflict"
                        ? t("review.relationConflictHint")
                        : item.action === "page_keep_or_archive"
                          ? t("review.pageArchiveHint")
                          : item.action === "cross_space_discovery"
                            ? t("review.crossSpaceHint")
                            : item.action === "suggest_entity"
                              ? t("review.suggestEntityHint")
                              : item.action === "dedup_merge"
                                ? t("review.dedupHint")
                                : item.payload?.action === "page_merge"
                                  ? `${t("review.mergeReason", {
                                      count: item.payload.source_overlap,
                                      percent: Math.round(
                                        item.payload.source_overlap_ratio * 100,
                                      ),
                                    })} · ${t("review.confidence", {
                                      percent: Math.round(item.confidence * 100),
                                    })}`
                                  : t("review.confidence", {
                                      percent: Math.round(item.confidence * 100),
                                    })}
              </p>

              {contractBlocked && !reviewReadOnly(item) && !isExampleReviewItem(item) &&
                !(item.kind === "refinement" && item.action === "lint_repair_review" && item.payload?.action === "lint_repair_review") && (
                <p role="status" style={paneStyle}>{t("review.approvalUnavailable")}</p>
              )}
              {!contractBlocked && (evidenceLoading || evidenceUnavailable) && (
                <div role={evidenceLoading ? "status" : "alert"} style={paneStyle}>
                  {t(evidenceLoading ? "review.loadingEvidence" : missingTarget ? "review.targetMissing" : missingMergeSources ? "review.mergeSourcesMissing" : "review.evidenceUnavailable")}
                  {!evidenceLoading && evidenceFailed && !missingTarget && !missingMergeSources && <button type="button" style={actionButtonStyle} onClick={retryEvidence}>
                    {t("review.retryEvidence")}
                  </button>}
                </div>
              )}
              {missingSources && !missingMergeSources && (
                <p role="status" style={paneStyle}>{t("review.archiveSourcesMissing")}</p>
              )}
              {item.kind === "refinement" && item.action === "lint_repair_review" && item.payload?.action === "lint_repair_review" && (
                <div>
                  <SourceRepairReview key={item.id} item={item} actionHost={repairActionHost} onBusyChange={handleRepairBusy} onVerified={() => {
                    // A repair closes through its verified daemon receipt, never
                    // through the generic refinement approval endpoint.
                    handleRepairBusy(false);
                    const remaining = items.filter((entry) => reviewItemId(entry) !== reviewItemId(item));
                    const next = remaining[Math.min(index, remaining.length - 1)] ?? null;
                    setFlash(t("review.resolved"));
                    window.setTimeout(() => setFlash(null), 650);
                    if (next) onOpenChange(reviewItemId(next));
                    else setShowDone(true);
                    for (const key of ["refinement-proposals", "pending-revisions", "pages", "space-pages", "recent-concepts", "memories", "memoryStats", "memory-detail", "page", "page-links", "page-revisions", "page-sources", "entities", "entity-detail", "entityDetail", "space-entities", "constellation-entities", "knowledge-graph", "distill-review"]) {
                      void queryClient.invalidateQueries({ queryKey: [key] });
                    }
                  }} technicalDetails={<>
                  <p>{item.payload.issue}</p>
                  <ul>{item.payload.choices.map((choice, index) => <li key={index}>{choice}</li>)}</ul>
                  {item.payload.suggested_research_queries.length > 0 && <>
                    <p style={paneLabelStyle}>{t("review.repairResearchQueries")}</p>
                    <ul>{item.payload.suggested_research_queries.map((query, index) => <li key={index}>{query}</li>)}</ul>
                  </>}
                  <button type="button" style={actionButtonStyle} disabled={repairCopyState === "copying"} onClick={() => void copyRepairDetails()}>
                    {t("review.copyRepairDetails")}
                  </button>
                  {repairCopyState === "copied" && <p role="status">{t("review.repairDetailsCopied")}</p>}
                  {repairCopyState === "error" && <p role="alert">{t("review.repairCopyFailed")}</p>}
                  </>} />
                </div>
              )}
              {vocabulary && (
                <div style={paneStyle}>
                  <p>{t("review.vocabProposal", { value: vocabulary.old_value.toLowerCase(), kind: t(vocabulary.kind === "entity" ? "review.vocabEntityType" : "review.vocabRelationType") })}</p>
                  {vocabulary.category && <p>{t("review.vocabCategory", { category: vocabulary.category })}</p>}
                  <p>{t(vocabulary.kind === "entity" ? "review.vocabEntityImpact" : "review.vocabRelationImpact")}</p>
                  {vocabulary.kind === "entity" && (
                    <>
                      <p style={paneLabelStyle}>{t("review.vocabAffected", { count: vocabularyEntityIds.length })}</p>
                      {vocabularyEntityIds.length === 0 ? <p>{t("review.vocabNoEntities")}</p> : (
                        <ul aria-label={t("review.vocabAffected", { count: vocabularyEntityIds.length })}>
                          {vocabularyEntities.map((query, index) => (
                            <li key={vocabularyEntityIds[index]}>
                              {query.data?.entity
                                ? t("review.vocabCurrentEntity", { name: query.data.entity.name, type: query.data.entity.entity_type })
                                : t(query.isLoading ? "review.loadingCurrent" : "review.targetMissing")}
                            </li>
                          ))}
                        </ul>
                      )}
                    </>
                  )}
                </div>
              )}

              {item.kind === "revision" && (
                <>
                  <div
                    role="tablist"
                    style={{
                      display: "inline-flex",
                      border: "1px solid var(--mem-border)",
                      borderRadius: 8,
                      overflow: "hidden",
                      marginBottom: 14,
                    }}
                  >
                    {[
                      { side: false, label: t("review.unified") },
                      { side: true, label: t("review.sideBySide") },
                    ].map(({ side, label }) => (
                      <button
                        key={label}
                        type="button"
                        role="tab"
                        aria-selected={sideBySide === side}
                        onClick={() => setSideBySide(side)}
                        style={{
                          fontFamily: "var(--mem-font-body)",
                          fontSize: "var(--mem-text-meta)",
                          padding: "5px 12px",
                          border: "none",
                          cursor: "pointer",
                          backgroundColor:
                            sideBySide === side
                              ? "var(--mem-indigo-bg)"
                              : "var(--mem-surface)",
                          color:
                            sideBySide === side
                              ? "var(--mem-accent-indigo)"
                              : "var(--mem-text-secondary)",
                        }}
                      >
                        {label}
                      </button>
                    ))}
                  </div>

                  {beforeLoading ? (
                    <div style={paneStyle}>{t("review.loadingCurrent")}</div>
                  ) : sideBySide ? (
                    <div
                      style={{
                        display: "grid",
                        gridTemplateColumns:
                          "repeat(auto-fit, minmax(240px, 1fr))",
                        gap: 12,
                      }}
                    >
                      <div>
                        <p style={paneLabelStyle}>{t("review.current")}</p>
                        <div style={paneStyle}>
                          <DiffText
                            segments={segments.filter(
                              (segment) => segment.kind !== "ins",
                            )}
                          />
                        </div>
                      </div>
                      <div>
                        <p style={paneLabelStyle}>{t("review.proposed")}</p>
                        <div style={paneStyle}>
                          <DiffText
                            segments={segments.filter(
                              (segment) => segment.kind !== "del",
                            )}
                          />
                        </div>
                      </div>
                    </div>
                  ) : (
                    <div style={paneStyle}>
                      <DiffText segments={segments} />
                    </div>
                  )}

                  {!beforeLoading && (
                    <p
                      style={{
                        fontFamily: "var(--mem-font-body)",
                        color: "var(--mem-text-secondary)",
                        fontSize: "var(--mem-text-meta)",
                        margin: "10px 2px 0",
                      }}
                    >
                      {t("review.wordDelta", {
                        added: wordCounts.added,
                        removed: wordCounts.removed,
                      })}
                      {" · "}
                      <del style={DEL_STYLE}>{t("review.stripped")}</del>
                      {" · "}
                      <ins style={INS_STYLE}>{t("review.added")}</ins>
                    </p>
                  )}

                  {item.targetKind === "page" ? (
                    <PageRevisionChain pageId={item.targetSourceId} />
                  ) : (
                    <MemoryRevisionChain
                      sourceId={item.targetSourceId}
                      onOpenMemory={(id) => onOpenMemory?.(id)}
                    />
                  )}
                </>
              )}

              {item.kind === "capture" && (
                <div style={paneStyle}>
                  {target.isLoading
                    ? t("review.loadingCurrent")
                    : (target.data?.content ?? item.snippet ?? "")}
                </div>
              )}

              {item.kind === "page_candidate" && (
                <div style={{ display: "grid", gap: 12 }}>
                  <p
                    style={{
                      fontFamily: "var(--mem-font-body)",
                      color: "var(--mem-text-secondary)",
                      fontSize: "var(--mem-text-control)",
                      margin: 0,
                    }}
                  >
                    {t("review.candidateQueued")}
                  </p>
                  {item.cluster.contents
                    .map((content) => content.trim())
                    .filter((content) => content.length > 0)
                    .slice(0, 3)
                    .map((content, contentIndex) => (
                      <div key={contentIndex} style={paneStyle}>
                        {content}
                      </div>
                    ))}
                </div>
              )}

              {item.kind === "topic" && (
                <div style={{ display: "grid", gap: 12 }}>
                  <p
                    style={{
                      fontFamily: "var(--mem-font-body)",
                      color: "var(--mem-text-secondary)",
                      fontSize: "var(--mem-text-control)",
                      margin: 0,
                    }}
                  >
                    {t("review.topicHint")}
                  </p>
                  <ReviewEvidencePane
                    query={item.label}
                    onOpenMemory={onOpenMemory}
                  />
                </div>
              )}

              {item.kind === "stale_page" && (
                <div style={{ display: "grid", gap: 12 }}>
                  <p
                    style={{
                      fontFamily: "var(--mem-font-body)",
                      color: "var(--mem-text-secondary)",
                      fontSize: "var(--mem-text-control)",
                      margin: 0,
                    }}
                  >
                    {t(staleNeedsAiTool ? "review.refreshNeedsAiTool" : "review.refreshHint")}
                  </p>
                  {item.summary && <div style={paneStyle}>{item.summary}</div>}
                </div>
              )}

              {item.kind === "refinement" &&
                item.action === "entity_merge" && (
                  <div style={{ display: "grid", gap: 12 }}>
                    <div>
                      <p style={paneLabelStyle}>{t("review.mergeKeep")}</p>
                      <div style={paneStyle}>
                        {mergeExisting.data?.entity.name ??
                          mergePayload?.existing_id}
                      </div>
                    </div>
                    <div>
                      <p style={paneLabelStyle}>{t("review.mergeFoldsIn")}</p>
                      <div style={paneStyle}>
                        {mergeIncoming.data?.entity.name ??
                          mergePayload?.new_id}
                      </div>
                    </div>
                  </div>
                )}

              {item.kind === "refinement" &&
                item.action === "detect_contradiction" && (
                  <div
                    style={{
                      display: "grid",
                      gridTemplateColumns:
                        "repeat(auto-fit, minmax(240px, 1fr))",
                      gap: 12,
                    }}
                  >
                    <div>
                      <p style={paneLabelStyle}>{t("review.existingMemory")}</p>
                      <div style={paneStyle}>
                        {memoryPaneB.isLoading ? (
                          t("review.loadingCurrent")
                        ) : contradictionSegments.length > 0 ? (
                          <DiffText
                            segments={contradictionSegments.filter(
                              (segment) => segment.kind !== "ins",
                            )}
                          />
                        ) : (
                          (memoryPaneB.data?.content ?? "")
                        )}
                      </div>
                    </div>
                    <div>
                      <p style={paneLabelStyle}>{t("review.newMemoryNewer")}</p>
                      <div style={paneStyle}>
                        {memoryPaneA.isLoading ? (
                          t("review.loadingCurrent")
                        ) : contradictionSegments.length > 0 ? (
                          <DiffText
                            segments={contradictionSegments.filter(
                              (segment) => segment.kind !== "del",
                            )}
                          />
                        ) : (
                          (memoryPaneA.data?.content ?? "")
                        )}
                      </div>
                    </div>
                  </div>
                )}

              {item.kind === "refinement" && item.action === "page_merge" && (
                <PageMergeStripOff
                  keepId={item.sourceIds[0]}
                  retireId={item.sourceIds[1]}
                  onOpenPage={(id) => onOpenPage?.(id)}
                  onOpenMemory={(id) => onOpenMemory?.(id)}
                  sourceOverlap={
                    item.payload?.action === "page_merge"
                      ? item.payload.source_overlap
                      : 0
                  }
                  sourceOverlapRatio={
                    item.payload?.action === "page_merge"
                      ? item.payload.source_overlap_ratio
                      : 0
                  }
                />
              )}

              {item.kind === "refinement" &&
                item.action === "relation_conflict" &&
                item.payload?.action === "relation_conflict" && (
                  <div style={{ display: "grid", gap: 12 }}>
                    <div>
                      <p style={paneLabelStyle}>{t("review.relationOld")}</p>
                      <div style={{ ...paneStyle, fontFamily: "var(--mem-font-mono)", fontSize: 13 }}>
                        <del style={DEL_STYLE}>
                          {item.payload.from} —{item.payload.old_type}→ {item.payload.to}
                        </del>
                      </div>
                    </div>
                    <div>
                      <p style={paneLabelStyle}>{t("review.relationNew")}</p>
                      <div style={{ ...paneStyle, fontFamily: "var(--mem-font-mono)", fontSize: 13 }}>
                        <ins style={INS_STYLE}>
                          {item.payload.from} —{item.payload.new_type}→ {item.payload.to}
                        </ins>
                      </div>
                    </div>
                  </div>
                )}

              {item.kind === "refinement" &&
                item.action === "page_keep_or_archive" && (
                  <div style={{ display: "grid", gap: 12 }}>
                    <div>
                      <p style={paneLabelStyle}>
                        {archivePage.data?.title ??
                          archivePageId ??
                          t("review.kindPageArchive")}
                      </p>
                      <div style={paneStyle}>
                        {archivePage.isLoading
                          ? t("review.loadingCurrent")
                          : (archivePage.data?.summary ??
                            archivePage.data?.content ??
                            "")}
                      </div>
                    </div>
                    {archiveSources.data && (
                      <div>
                        <p style={paneLabelStyle}>{t("review.sources", { count: archiveSources.data.length })}</p>
                        {archiveSources.data.map((source) => (
                          <button key={source.source.memory_source_id} type="button" style={evidenceRowStyle}
                            disabled={!onOpenMemory || !source.memory}
                            onClick={() => onOpenMemory?.(source.source.memory_source_id)}>
                            {source.memory?.title || truncateReviewText(source.memory?.content ?? source.source.memory_source_id, 100)}
                          </button>
                        ))}
                      </div>
                    )}
                  </div>
                )}

              {item.kind === "refinement" &&
                (item.action === "suggest_entity" ||
                  item.action === "dedup_merge" ||
                  item.action === "cross_space_discovery") && (
                  <div style={{ display: "grid", gap: 12 }}>
                    {item.payload?.action === "suggest_entity" &&
                      item.payload.name_hint && (
                        <div style={paneStyle}>{item.payload.name_hint}</div>
                      )}
                    {item.action === "suggest_entity" && (
                      <ReviewEvidencePane
                        query={
                          item.payload?.action === "suggest_entity"
                            ? (item.payload.name_hint ?? null)
                            : null
                        }
                        onOpenMemory={onOpenMemory}
                      />
                    )}
                    {item.payload?.action === "cross_space_discovery" && (
                      <p
                        style={{
                          fontFamily: "var(--mem-font-body)",
                          color: "var(--mem-text-secondary)",
                          fontSize: "var(--mem-text-control)",
                          margin: 0,
                        }}
                      >
                        {t("review.crossSpaceSpaces", {
                          spaces: item.payload.spaces.join(" · "),
                          count: item.payload.memory_count,
                        })}
                      </p>
                    )}
                    {[memoryPaneA, memoryPaneB].map(
                      (pane, paneIndex) =>
                        memoryPaneIds[paneIndex] && (
                          <div key={paneIndex}>
                            <p style={paneLabelStyle}>
                              {pane.data?.title ?? ""}
                            </p>
                            <div style={paneStyle}>
                              {pane.isLoading
                                ? t("review.loadingCurrent")
                                : (pane.data?.content ?? "")}
                            </div>
                          </div>
                        ),
                    )}
                  </div>
                )}
            </div>

            {isExampleReviewItem(item) && (
              <p
                style={{
                  fontFamily: "var(--mem-font-body)",
                  fontSize: "var(--mem-text-meta)",
                  color: "var(--mem-text-secondary)",
                  margin: "0 20px 10px",
                }}
              >
                {t("review.exampleDialogNote")}
              </p>
            )}

            {resolveError && (
              <p
                role="alert"
                style={{
                  background: "var(--mem-status-danger-bg)",
                  border: "1px solid var(--mem-status-danger-border)",
                  borderRadius: 8,
                  color: "var(--mem-status-danger-text)",
                  font: "var(--mem-text-description)/1.5 var(--mem-font-body)",
                  margin: "4px 20px 0",
                  padding: "9px 11px",
                }}
              >
                {resolveError}
              </p>
            )}

            <div
              style={{
                display: "flex",
                alignItems: "center",
                gap: 8,
                padding: "14px 20px 16px",
                borderTop: "1px solid var(--mem-detail-divider)",
                marginTop: 14,
                ...(item.kind === "refinement" && item.action === "lint_repair_review" ? {
                  flexShrink: 0,
                  backgroundColor: "var(--mem-surface)",
                } : {}),
              }}
            >
              {!reviewDismissBlocked(item) && (
                <button
                  type="button"
                  disabled={resolving}
                  onClick={() => void resolveCurrent(false)}
                  style={{
                    ...actionButtonStyle,
                    // Forget hard-deletes the capture; every other dismiss just
                    // clears the proposal, so only Forget wears the danger tone.
                    ...(item.kind === "capture"
                      ? {
                          color: "var(--mem-status-danger-text)",
                          borderColor: "var(--mem-status-danger-border)",
                        }
                      : { color: "var(--mem-text-secondary)" }),
                  }}
                >
                  {item.kind === "capture"
                    ? t("review.forget")
                    : isContradiction
                      ? t("review.keepBoth")
                      : item.kind === "refinement" &&
                          item.action === "page_keep_or_archive"
                        ? t("review.keepPage")
                        : item.kind === "refinement" && item.action === "lint_repair_review"
                          ? t("sourceRepair.keepCurrent")
                        : item.kind === "topic" ||
                            item.kind === "page_candidate" ||
                            item.kind === "stale_page"
                          ? t("review.hide")
                          : t("review.dismiss")}
                </button>
              )}
              {item.kind === "page_candidate" &&
                item.cluster.existing_page_id &&
                onOpenPage && (
                  <button
                    type="button"
                    onClick={() =>
                      onOpenPage(item.cluster.existing_page_id as string)
                    }
                    style={actionButtonStyle}
                  >
                    {t("review.openPage")}
                  </button>
                )}
              {archivePageId && archivePage.data && onOpenPage && (
                <button type="button" onClick={() => onOpenPage(archivePageId)} style={actionButtonStyle}>
                  {t("review.openPage")}
                </button>
              )}
              {item.kind === "stale_page" && onOpenPage && (
                <button
                  type="button"
                  onClick={() => onOpenPage(item.id)}
                  style={actionButtonStyle}
                >
                  {t("review.openPage")}
                </button>
              )}
              {item.kind === "revision" &&
                item.targetKind === "page" &&
                onOpenPage && (
                  <button
                    type="button"
                    onClick={() => onOpenPage(item.targetSourceId)}
                    style={actionButtonStyle}
                  >
                    {t("review.openPage")}
                  </button>
                )}
              {((item.kind === "revision" && item.targetKind !== "page") ||
                item.kind === "capture") &&
                onOpenMemory && (
                  <button
                    type="button"
                    onClick={() =>
                      onOpenMemory(
                        item.kind === "revision" ? item.targetSourceId : item.id,
                      )
                    }
                    style={actionButtonStyle}
                  >
                    {t("review.openMemory")}
                  </button>
                )}
              <span style={{ flex: 1 }} />
              {items.length > 1 && (
                <button
                  type="button"
                  disabled={repairBusy}
                  onClick={() => goTo(1)}
                  style={actionButtonStyle}
                >
                  {t("review.skip")}
                </button>
              )}
              {item.kind === "refinement" && item.action === "lint_repair_review" && <div className="source-repair-actions" ref={setRepairActionHost} />}
              {staleNeedsAiTool && (
                <button type="button" data-testid="review-copy-steep-command" onClick={() => void copySteepCommand()} style={actionButtonStyle}>
                  {t("review.copyCommand")}
                </button>
              )}
              {!reviewApproveBlocked(item) && !staleNeedsAiTool && (
                <button
                  type="button"
                  disabled={resolving || !canApprove}
                  onClick={() => void resolveCurrent(true)}
                  style={{
                    ...actionButtonStyle,
                    backgroundColor: archivePageId ? "var(--mem-surface)" : "var(--mem-accent-indigo)",
                    borderColor: archivePageId ? "var(--mem-border)" : "var(--mem-accent-indigo)",
                    color: archivePageId ? "var(--mem-text)" : "var(--mem-bg)",
                    opacity: resolving || !canApprove ? 0.55 : 1,
                    cursor: resolving || !canApprove ? "not-allowed" : "pointer",
                    fontWeight: 600,
                  }}
                >
                  {item.kind === "capture"
                    ? t("review.confirm")
                    : item.kind === "stale_page"
                      ? t("review.refreshPage")
                      : isContradiction
                        ? t("review.resolve")
                        : item.kind === "refinement" &&
                            item.action === "page_keep_or_archive"
                          ? t("review.archive")
                          : item.kind === "refinement" &&
                              item.action === "page_merge"
                            ? t("review.mergePages")
                            : vocabulary
                              ? t("review.promoteVocabulary")
                              : t("review.approve")}
                </button>
              )}
            </div>

            {(item.kind === "topic" ||
              item.kind === "page_candidate" ||
              item.kind === "stale_page") && (
              <p
                style={{
                  fontFamily: "var(--mem-font-body)",
                  color: "var(--mem-text-secondary)",
                  fontSize: "var(--mem-text-meta)",
                  margin: "0 20px 14px",
                }}
              >
                {t("review.hideHint")}
              </p>
            )}
          </>
        ) : null}
      </div>
    </div>,
    document.body,
  );
}
