// SPDX-License-Identifier: AGPL-3.0-only
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import {
  listPendingRevisions,
  listRefinements,
  type Page,
  type PendingRevisionItem,
  type RefinementProposalSummary,
} from "../../../lib/tauri";
import { isActionableReviewItem } from "../reviewDisposition";
import type { ReviewItem } from "../useReviewQueue";
import { CodeBasisLines, type NoteCodeBasisHandlers } from "./CodeBasisLines";
import { NoteBasisLines, basisNeedingAttention, type NoteBasisHandlers } from "./NoteBasisLines";
import { codeBasisNeedingAttention, useEverydayModelAvailable } from "./useCodeBasisActions";
import "./NoteReviewNotice.css";

const NOTICE_LIMIT = 500;
const revisionQueryKey = ["pending-revisions", "note-context"];
const refinementQueryKey = ["refinement-proposals", "note-context"];

type PendingItem = {
  reviewId: string;
  category: "pageRevision" | "sourceReview" | "pageSuggestion";
};

function proposalMatchesPage(proposal: RefinementProposalSummary, pageId: string): boolean {
  if (proposal.source_ids.includes(pageId)) return true;
  const payload = proposal.payload;
  if (!payload) return false;
  if (payload.action === "page_keep_or_archive") return payload.page_id === pageId;
  return payload.action === "page_merge"
    && (payload.left_page_id === pageId || payload.right_page_id === pageId);
}

function proposalMatchesLinkedSource(proposal: RefinementProposalSummary, sourceIds: ReadonlySet<string>): boolean {
  return proposal.source_ids.some((sourceId) => sourceIds.has(sourceId));
}

function getPendingItems(
  page: Page,
  revisions: readonly PendingRevisionItem[],
  proposals: readonly RefinementProposalSummary[],
): PendingItem[] {
  const sourceIds = new Set(page.source_memory_ids);
  const matches: PendingItem[] = [];

  for (const revision of revisions) {
    if (revision.target_kind === "page" && revision.target_source_id === page.id) {
      const item: ReviewItem = {
        kind: "revision", id: revision.target_source_id, targetSourceId: revision.target_source_id,
        targetKind: revision.target_kind, revisionSourceId: revision.revision_source_id,
        content: revision.revision_content, agent: revision.source_agent,
        timestampMs: revision.last_modified ? revision.last_modified * 1000 : null,
      };
      if (isActionableReviewItem(item)) matches.push({ reviewId: `revision:${item.id}`, category: "pageRevision" });
    } else if (revision.target_kind === "memory" && sourceIds.has(revision.target_source_id)) {
      const item: ReviewItem = {
        kind: "revision", id: revision.target_source_id, targetSourceId: revision.target_source_id,
        targetKind: revision.target_kind, revisionSourceId: revision.revision_source_id,
        content: revision.revision_content, agent: revision.source_agent,
        timestampMs: revision.last_modified ? revision.last_modified * 1000 : null,
      };
      if (isActionableReviewItem(item)) matches.push({ reviewId: `revision:${item.id}`, category: "sourceReview" });
    }
  }

  for (const proposal of proposals) {
    const item: ReviewItem = {
      kind: "refinement", id: proposal.id, action: proposal.action,
      sourceIds: proposal.source_ids, payload: proposal.payload ?? null,
      confidence: proposal.confidence,
      timestampMs: proposal.created_at
        ? (Number.isNaN(Date.parse(proposal.created_at.replace(" ", "T")))
          ? null : Date.parse(proposal.created_at.replace(" ", "T")))
        : null,
    };
    if (!isActionableReviewItem(item)) continue;
    if ((proposal.action === "page_merge" || proposal.action === "page_keep_or_archive")
      && proposalMatchesPage(proposal, page.id)) {
      matches.push({ reviewId: `refinement:${item.id}`, category: "pageSuggestion" });
    } else if ((proposal.action === "detect_contradiction" || proposal.action === "lint_repair_review")
      && proposalMatchesLinkedSource(proposal, sourceIds)) {
      matches.push({ reviewId: `refinement:${item.id}`, category: "sourceReview" });
    }
  }

  // Queue order starts with revisions, then refinements. Preserve that order so
  // the notice opens the first relevant item rather than an unrelated queue row.
  return matches;
}

export interface NoteReviewNoticeProps {
  page: Page;
  onReview: (itemId?: string) => void;
  onShowSources: () => void;
  disabled?: boolean;
  /** Present on the note itself: lines and actions for the notes it relies on. */
  basis?: NoteBasisHandlers;
  /** Present on the note itself: lines and actions for the code it relies on. */
  codeBasis?: NoteCodeBasisHandlers;
}

/**
 * A note that relies on other notes is never rebuilt on its own, so the daemon
 * says "blocked because it has a basis" for a plain source change. That is the
 * normal state of such a note, not a problem to flag.
 */
function isBlockingRefreshReason(reason: string | null | undefined): boolean {
  return Boolean(reason) && reason !== "has_basis";
}

export function NoteReviewNotice({ page, onReview, onShowSources, disabled = false, basis, codeBasis }: NoteReviewNoticeProps) {
  const { t } = useTranslation();
  const modelAvailable = useEverydayModelAvailable();
  const revisions = useQuery({
    queryKey: revisionQueryKey,
    queryFn: () => listPendingRevisions(NOTICE_LIMIT),
    staleTime: 30_000,
    refetchInterval: 30_000,
    retry: false,
  });
  const refinements = useQuery({
    queryKey: refinementQueryKey,
    queryFn: () => listRefinements(NOTICE_LIMIT),
    staleTime: 30_000,
    refetchInterval: 30_000,
    retry: false,
  });

  const pendingItems = getPendingItems(page, revisions.data ?? [], refinements.data?.proposals ?? []);
  const categoryCounts = pendingItems.reduce<Record<PendingItem["category"], number>>((counts, item) => {
    counts[item.category] += 1;
    return counts;
  }, { pageRevision: 0, sourceReview: 0, pageSuggestion: 0 });
  const capped = (revisions.data?.length ?? 0) >= NOTICE_LIMIT
    || (refinements.data?.proposals.length ?? 0) >= NOTICE_LIMIT;
  const queryFailed = revisions.isError || refinements.isError;
  const pageRevisionPending = categoryCounts.pageRevision > 0;

  const refreshBlocked = isBlockingRefreshReason(page.refresh_blocked_reason);
  const staleMessage = page.stale_reason === "source_conflict"
    ? "sourceConflict"
    : refreshBlocked
      ? "updateBlocked"
      : page.stale_reason === "source_updated" && !pageRevisionPending
        ? "sourceUpdated"
        : null;
  const basisLines = basis ? basisNeedingAttention(page.basis) : [];
  const codeLines = codeBasis ? codeBasisNeedingAttention(page.code_basis, modelAvailable) : [];
  // Basis lines alone leave the review row empty; drawing it anyway adds a blank row and a gap above them.
  const hasTopRow = pendingItems.length > 0 || staleMessage !== null || queryFailed || capped;
  const hasContent = hasTopRow || basisLines.length > 0 || codeLines.length > 0;
  if (!hasContent) return null;

  const retry = () => {
    if (disabled) return;
    void Promise.all([revisions.refetch(), refinements.refetch()]);
  };

  const attention = pendingItems.length > 0 || page.stale_reason === "source_conflict" || refreshBlocked
    || basisLines.length > 0 || codeLines.length > 0;
  return <div className={`note-review-notice${attention ? " note-review-notice--attention" : ""}`} role="status" aria-live="polite">
    {hasTopRow && <div className="note-review-notice__content">
      {pendingItems.length > 0 && <div className="note-review-notice__summary">
        <span>{t(pendingItems.length === 1 ? "noteReview.pending_one" : "noteReview.pending_other", { count: pendingItems.length })}</span>
        <span className="note-review-notice__categories">
          {(["pageRevision", "sourceReview", "pageSuggestion"] as const).flatMap((category) => {
            const count = categoryCounts[category];
            return count > 0 ? [<span key={category}>{t(`noteReview.${category}`, { count })}</span>] : [];
          })}
        </span>
      </div>}
      {staleMessage && <p className="note-review-notice__message">{t(`noteReview.${staleMessage}`)}</p>}
      {queryFailed && <p className="note-review-notice__message">{t("noteReview.checkFailed")}</p>}
      {capped && <p className="note-review-notice__message">{t("noteReview.incomplete")}</p>}
    </div>}
    {hasTopRow && <div className="note-review-notice__actions">
      {pendingItems.length > 0 && <button
        type="button"
        className="note-review-notice__button"
        disabled={disabled}
        onClick={() => onReview(pendingItems[0].reviewId)}
      >{t("noteReview.reviewChanges")}</button>}
      {staleMessage && <button
        type="button"
        className="note-review-notice__button note-review-notice__button--quiet"
        disabled={disabled}
        onClick={onShowSources}
      >{t("noteReview.showSources")}</button>}
      {queryFailed && <button
        type="button"
        className="note-review-notice__button note-review-notice__button--quiet"
        disabled={disabled}
        onClick={retry}
      >{t("noteReview.retry")}</button>}
      {(queryFailed || capped) && <button
        type="button"
        className="note-review-notice__button note-review-notice__button--quiet"
        disabled={disabled}
        onClick={() => onReview()}
      >{t("noteReview.openQueue")}</button>}
    </div>}
    {basis && basisLines.length > 0 && <NoteBasisLines
      {...basis}
      pageId={page.id}
      basis={basisLines}
      disabled={disabled}
    />}
    {codeBasis && codeLines.length > 0 && <CodeBasisLines
      {...codeBasis}
      pageId={page.id}
      codeBasis={codeLines}
      disabled={disabled}
    />}
  </div>;
}

export default NoteReviewNotice;
