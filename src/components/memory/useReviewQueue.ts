// SPDX-License-Identifier: AGPL-3.0-only
import { useMemo } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import {
  acceptPendingRevision,
  acceptRefinement,
  agentDisplayName,
  confirmMemory,
  deleteMemory,
  dismissPendingRevision,
  isPageRevisionBasisRefused,
  isPageRevisionOutdated,
  isPageRevisionReplaced,
  listPendingRevisions,
  listRefinements,
  listUnconfirmedMemories,
  redistillPage,
  rejectRefinement,
  type DistillPendingCluster,
  type PendingRevisionItem,
  type ProposalAction,
  type RecentActivityItem,
  type RefinementPayload,
  type RefinementProposalSummary,
  type RevisionTargetKind,
} from "../../lib/tauri";
import { acceptClassification, type ClassificationChange, type ClassificationPayload } from "../../lib/researchApi";
import type { ResearchDisputePayload } from "../../lib/disputeApi";
import { BASIS_PENDING_QUERY_KEY } from "./page/useBasisActions";
import { CODE_BASIS_PENDING_QUERY_KEY } from "./page/useCodeBasisActions";
import { sharedWriter, useShares } from "./sharing/sharing";
import { SteepNeedsAiToolError } from "../../lib/steepMode";

export type ReviewItem =
  | {
      kind: "revision";
      id: string;
      targetSourceId: string;
      /** Whether `targetSourceId` names a memory or a page. Straight from the
       * daemon — the two need different reads for the diff's "before" side. */
      targetKind: RevisionTargetKind;
      revisionSourceId: string;
      content: string;
      agent: string | null;
      /** Why a page revision needs review ("authored", "locked", "background", "owned"). */
      reviewReason?: string | null;
      timestampMs: number | null;
    }
  | {
      kind: "refinement";
      id: string;
      action: ProposalAction | "source_classification" | "research_dispute";
      sourceIds: string[];
      payload: RefinementPayload | ClassificationPayload | ResearchDisputePayload | null;
      confidence: number;
      timestampMs: number | null;
    }
  | {
      kind: "capture";
      id: string;
      title: string;
      snippet: string | null;
      timestampMs: number | null;
    }
  // A compiled page whose sources changed since the last distill. Approve
  // re-runs redistill_page; there is no dismiss verb, so the dialog hides it.
  | {
      kind: "stale_page";
      id: string;
      title: string;
      summary: string | null;
      sourcesUpdated: number | null;
      timestampMs: null;
    }
  // Read-only discovery items from distill review — no daemon verb exists for
  // them yet, so the dialog shows them without approve/dismiss actions.
  | {
      kind: "page_candidate";
      id: string;
      title: string;
      cluster: DistillPendingCluster;
      timestampMs: null;
    }
  | {
      kind: "topic";
      id: string;
      label: string;
      count: number;
      timestampMs: null;
    };

export function reviewItemId(item: ReviewItem): string {
  return item.kind + ":" + item.id;
}

export type ReviewSection =
  | "revisions"
  | "conflicts"
  | "pages"
  | "classifications"
  | "memory"
  | "candidates"
  | "topics"
  | "captures";

/** Queue order: memory revisions and contradictions carry the most decision
 * value, then page/entity merges, then read-only discovery, captures last. */
const SECTION_ORDER: ReviewSection[] = [
  "revisions",
  "conflicts",
  "pages",
  "classifications",
  "memory",
  "candidates",
  "topics",
  "captures",
];

export function reviewItemSection(item: ReviewItem): ReviewSection {
  if (item.kind === "revision") return "revisions";
  if (item.kind === "capture") return "captures";
  if (item.kind === "stale_page") return "pages";
  if (item.kind === "page_candidate") return "candidates";
  if (item.kind === "topic") return "topics";
  switch (item.action) {
    case "source_classification":
      return "classifications";
    case "page_merge":
    case "page_keep_or_archive":
      return "pages";
    case "research_dispute":
    case "detect_contradiction":
    case "relation_conflict":
      return "conflicts";
    default:
      return "memory";
  }
}

function compareReviewItems(a: ReviewItem, b: ReviewItem): number {
  const rankDelta =
    SECTION_ORDER.indexOf(reviewItemSection(a)) -
    SECTION_ORDER.indexOf(reviewItemSection(b));
  if (rankDelta !== 0) return rankDelta;
  return (b.timestampMs ?? -Infinity) - (a.timestampMs ?? -Infinity);
}

const REVISIONS_KEY = ["pending-revisions"];
const REFINEMENTS_KEY = ["refinement-proposals"];
const CAPTURES_KEY = ["unconfirmed-captures"];

/** Per-source fetch cap. A source returning a full page means the true count
 * is unknown — surface counts as "N+" via `isTruncated`, never as exact. */
export const REVIEW_QUEUE_LIMIT = 50;

/**
 * Unified actionable review queue: pending memory revisions plus daemon
 * refinement proposals. `resolve` approves or dismisses one item through the
 * matching daemon verb and removes it from the cached queue immediately.
 */
export function useReviewQueue(enabled: boolean = true, scope: "default" | "note-context" = "default", currentSpace?: string) {
  const decisionLimit = scope === "note-context" ? 500 : REVIEW_QUEUE_LIMIT;
  const revisionsKey = scope === "note-context" ? [...REVISIONS_KEY, scope] : REVISIONS_KEY;
  const refinementsKey = scope === "note-context" ? [...REFINEMENTS_KEY, scope] : REFINEMENTS_KEY;
  const queryClient = useQueryClient();
  const { t } = useTranslation();
  // A card another member's AI tool made in a shared Space names that member.
  const shares = useShares();

  const revisions = useQuery({
    queryKey: revisionsKey,
    queryFn: () => listPendingRevisions(decisionLimit),
    refetchInterval: 30_000,
    enabled,
  });
  const refinements = useQuery({
    queryKey: refinementsKey,
    queryFn: () => listRefinements(decisionLimit),
    refetchInterval: 30_000,
    enabled,
  });
  const captures = useQuery({
    queryKey: CAPTURES_KEY,
    queryFn: () => listUnconfirmedMemories(REVIEW_QUEUE_LIMIT),
    refetchInterval: 30_000,
    enabled,
  });

  const items: ReviewItem[] = useMemo(
    () => [
      ...(revisions.data ?? []).map(
        (item: PendingRevisionItem): ReviewItem => ({
          kind: "revision",
          id: item.target_source_id,
          targetSourceId: item.target_source_id,
          targetKind: item.target_kind,
          revisionSourceId: item.revision_source_id,
          content: item.revision_content,
          agent: sharedWriter(item.edited_by, shares.data, t) ?? agentDisplayName(item.source_agent),
          reviewReason: item.review_reason ?? null,
          timestampMs: item.last_modified ? item.last_modified * 1000 : null,
        }),
      ),
      ...(refinements.data?.proposals ?? []).map(
        (proposal: RefinementProposalSummary): ReviewItem => ({
          kind: "refinement",
          id: proposal.id,
          action: proposal.action,
          sourceIds: proposal.source_ids,
          payload: proposal.payload ?? null,
          confidence: proposal.confidence,
          // The daemon emits "YYYY-MM-DD HH:MM:SS"; Date.parse needs the "T".
          timestampMs: proposal.created_at
            ? (Number.isNaN(Date.parse(proposal.created_at.replace(" ", "T")))
                ? null
                : Date.parse(proposal.created_at.replace(" ", "T")))
            : null,
        }),
      ),
      ...(captures.data ?? []).map(
        (entry: RecentActivityItem): ReviewItem => ({
          kind: "capture",
          id: entry.id,
          title: entry.title,
          snippet: entry.snippet,
          timestampMs: entry.timestamp_ms ?? null,
        }),
      ),
    ].sort(compareReviewItems),
    [revisions.data, refinements.data, captures.data],
  );

  const resolveMutation = useMutation({
    mutationFn: async ({
      item,
      approve,
      edited_content,
      classification_changes,
    }: {
      item: ReviewItem;
      approve: boolean;
      edited_content?: string;
      classification_changes?: ClassificationChange[];
    }) => {
      if (item.kind === "revision") {
        // Always the card's own id, never the page's: a page id resolves to
        // whichever card is open now, which may be a newer proposal than the
        // one on screen. A replaced card's id is refused instead.
        return approve
          ? edited_content === undefined
            ? acceptPendingRevision(item.revisionSourceId)
            : acceptPendingRevision(item.revisionSourceId, { expected_content: item.content, content: edited_content })
          : dismissPendingRevision(item.revisionSourceId);
      }
      if (item.kind === "capture") {
        // Curate semantics: confirm keeps the memory, "dismiss" forgets it.
        return approve ? confirmMemory(item.id) : deleteMemory(item.id);
      }
      if (item.kind === "stale_page") {
        // Approve rebuilds the page from its sources; dismiss has no verb and
        // the dialog never offers it.
        if (!approve) return undefined;
        const result = await redistillPage(item.id);
        // No model in the daemon: nothing was rebuilt, so the card must stay.
        if (result.status === "skipped") throw new SteepNeedsAiToolError();
        return result;
      }
      if (item.kind === "page_candidate" || item.kind === "topic") {
        // Read-only discovery — no daemon verb; the dialog never offers one.
        return;
      }
      if (item.action === "source_classification" && approve && classification_changes) {
        return acceptClassification(item.id, classification_changes.map(({ source, source_id, source_content_digest, expected_decision_revision, add_tags, remove_tags, reason }) => ({ source, source_id, source_content_digest, expected_decision_revision, add_tags, remove_tags, reason })), currentSpace);
      }
      return approve ? (currentSpace ? acceptRefinement(item.id, currentSpace) : acceptRefinement(item.id)) : (currentSpace ? rejectRefinement(item.id, currentSpace) : rejectRefinement(item.id));
    },
    onError: (error, { item, edited_content }) => {
      // The daemon discards a page revision card that can no longer apply.
      // Drop it here too, so neither the queue nor the page banner keeps
      // offering a card that is already gone. A card a newer proposal
      // replaced is gone too, edited or not; the refetch brings in the newer
      // one.
      if (item.kind !== "revision") return;
      const replaced = isPageRevisionReplaced(error);
      if (!replaced && (edited_content !== undefined || !(isPageRevisionOutdated(error) || isPageRevisionBasisRefused(error)))) return;
      queryClient.setQueriesData<PendingRevisionItem[]>(
        { queryKey: REVISIONS_KEY },
        (old) =>
          old?.filter((entry) => entry.target_source_id !== item.targetSourceId) ?? [],
      );
      for (const key of ["page", "page-revisions"]) {
        void queryClient.invalidateQueries({ queryKey: [key] });
      }
      queryClient.invalidateQueries({ queryKey: REVISIONS_KEY });
    },
    onSuccess: (_result, { item, approve, edited_content }) => {
      if (approve && item.kind === "refinement" && item.action === "vocab_promote") {
        // Promoting an entity type also retypes its active concept pages.
        for (const key of ["entity-detail", "entityDetail", "entities", "space-entities",
          "constellation-entities", "connections-entities", "searchEntities", "knowledge-graph",
          "pages", "page", "space-pages"]) {
          void queryClient.invalidateQueries({ queryKey: [key] });
        }
      }
      // Panel-owned distill items never live in these caches — stale pages are
      // removed from the panel's last distill result by its resolve wrapper.
      if (
        item.kind === "stale_page" ||
        item.kind === "page_candidate" ||
        item.kind === "topic"
      )
        return;
      // Drop the resolved item from the cache right away so the queue and any
      // open dialog advance without waiting for the refetch.
      if (item.kind === "revision") {
        queryClient.setQueriesData<PendingRevisionItem[]>(
          { queryKey: REVISIONS_KEY },
          (old) =>
            old?.filter(
              (entry) => edited_content === undefined
                ? entry.target_source_id !== item.targetSourceId
                : entry.revision_source_id !== item.revisionSourceId,
            ) ?? [],
        );
      } else if (item.kind === "capture") {
        queryClient.setQueryData<RecentActivityItem[]>(
          CAPTURES_KEY,
          (old) => old?.filter((entry) => entry.id !== item.id) ?? [],
        );
      } else {
        queryClient.setQueriesData<{ proposals: RefinementProposalSummary[] }>(
          { queryKey: REFINEMENTS_KEY },
          (old) => ({
            proposals:
              old?.proposals.filter((entry) => entry.id !== item.id) ?? [],
          }),
        );
      }
      // Decisions entered from a note must refresh that note and its inline
      // signal as well as every capped queue cache. Never leave approved text
      // or a dismissed card looking pending on return. Accepting an update can
      // confirm a basis, so the "source note changed" group and each note's
      // basis diff refresh too.
      for (const key of ["tags", "space-sources", "research-runs", "page", "pages", "page-revisions", "page-sources", "page-links", "space-pages", "memory-detail", "page-basis-diff", ...BASIS_PENDING_QUERY_KEY, ...CODE_BASIS_PENDING_QUERY_KEY]) {
        void queryClient.invalidateQueries({ queryKey: [key] });
      }
      queryClient.invalidateQueries({ queryKey: REVISIONS_KEY });
      queryClient.invalidateQueries({ queryKey: REFINEMENTS_KEY });
      queryClient.invalidateQueries({ queryKey: CAPTURES_KEY });
    },
  });

  const decisionsTruncated =
    (revisions.data?.length ?? 0) >= decisionLimit ||
    (refinements.data?.proposals.length ?? 0) >= decisionLimit;
  const capturesTruncated = (captures.data?.length ?? 0) >= REVIEW_QUEUE_LIMIT;

  // Re-fetch all three sources — used to retry after a failed queue load.
  const refetch = () => {
    void revisions.refetch();
    void refinements.refetch();
    void captures.refetch();
    void queryClient.invalidateQueries({ queryKey: BASIS_PENDING_QUERY_KEY });
    void queryClient.invalidateQueries({ queryKey: CODE_BASIS_PENDING_QUERY_KEY });
  };

  return {
    items,
    isLoading: revisions.isLoading || refinements.isLoading,
    error: revisions.error ?? refinements.error ?? null,
    capturesLoading: captures.isLoading,
    capturesError: captures.error,
    /** True when any source filled its page — the queue may hold more. */
    isTruncated: decisionsTruncated || capturesTruncated,
    /** Decision sources (revisions + refinements) at their fetch cap. */
    decisionsTruncated,
    /** New-memory captures at their fetch cap. */
    capturesTruncated,
    resolve: resolveMutation.mutateAsync,
    isResolving: resolveMutation.isPending,
    refetch,
  };
}
