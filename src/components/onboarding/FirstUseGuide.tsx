// SPDX-License-Identifier: AGPL-3.0-only
import { useTranslation } from "react-i18next";
import { useQuery } from "@tanstack/react-query";
import { ArrowLeft, BookOpenText } from "@phosphor-icons/react";
import {
  getActiveImportBatches,
  getImportBatchStatus,
  getMemoryStats,
  getResolvedRouting,
  type ImportBatchStatus,
} from "../../lib/tauri";
import { isKnowledgePage, listAllActivePages } from "../memory/pages/listAllPages";
import {
  ImportPhaseList,
  importSourceLabel,
  summarizeImportBatches,
} from "../memory/ImportPhases";
import { FirstUseSample } from "./FirstUseSample";
import "./firstUseGuide.css";
import { useSteepOnDemand } from "../../lib/steepMode";

export type FirstUseGuideView = "sample" | "live";

export interface FirstUseGuideProps {
  /** The single way out of either view: back to the Wiki home. */
  onBack: () => void;
  onImport: () => void;
  onOpenIntelligence: () => void;
  onOpenPage: (id: string) => void;
  /** Entry view. The parent passes "live" when returning from a real import. */
  initialView?: FirstUseGuideView;
  /** The import just completed, if this guide was opened from that import. */
  batchId?: string;
}

/** Live queries poll only while the live view is mounted: entering the view
 *  mounts them, leaving it unmounts them. The sample view mounts no queries. */
const LIVE_BATCH_POLL_MS = 5_000;
/** Library pages re-read on this cadence so a newly distilled page appears
 *  without a manual refresh. Batch phases keep their own faster poll. */
const LIVE_PAGE_POLL_MS = 15_000;
/** Real library pages shown as "yours". Never attributed to one import: the
 *  current batch status carries no produced-page ids. */
const LIVE_PAGE_COUNT = 3;

export function FirstUseGuide({
  onBack,
  onImport,
  onOpenIntelligence,
  onOpenPage,
  initialView = "sample",
  batchId,
}: FirstUseGuideProps) {
  return (
    <div
      data-testid="first-use-guide"
      data-view={initialView}
      className="fug-root"
    >
      {initialView === "sample" ? (
        <FirstUseSample onBack={onBack} onFinish={onBack} />
      ) : (
        <FirstUseLive
          onBack={onBack}
          onImport={onImport}
          onOpenIntelligence={onOpenIntelligence}
          onOpenPage={onOpenPage}
          batchId={batchId}
        />
      )}
    </div>
  );
}

function FirstUseLive({
  onBack,
  onImport,
  onOpenIntelligence,
  onOpenPage,
  batchId,
}: {
  onBack: () => void;
  onImport: () => void;
  onOpenIntelligence: () => void;
  onOpenPage: (id: string) => void;
  batchId?: string;
}) {
  const { t } = useTranslation();
  const steepOnDemand = useSteepOnDemand() === true;
  const pagesQuery = useQuery({
    queryKey: ["first-use-pages"],
    queryFn: listAllActivePages,
    refetchInterval: LIVE_PAGE_POLL_MS,
  });
  const activeBatchesQuery = useQuery({
    queryKey: ["first-use-batches"],
    queryFn: getActiveImportBatches,
    refetchInterval: LIVE_BATCH_POLL_MS,
  });
  const batchStatusQuery = useQuery({
    queryKey: ["first-use-batch", batchId],
    queryFn: () => getImportBatchStatus(batchId!),
    enabled: Boolean(batchId),
    refetchInterval: (query) =>
      query.state.data?.complete ? false : LIVE_BATCH_POLL_MS,
  });

  const routingQuery = useQuery({
    queryKey: ["first-use-routing"],
    queryFn: getResolvedRouting,
    refetchInterval: LIVE_BATCH_POLL_MS,
  });

  const listPending = pagesQuery.isPending
    || activeBatchesQuery.isPending
    || (Boolean(batchId) && batchStatusQuery.isPending);
  const listFailed = pagesQuery.isError
    || activeBatchesQuery.isError
    || (Boolean(batchId) && batchStatusQuery.isError);

  const activeBatches = activeBatchesQuery.data?.batches ?? [];
  const targetedBatch = batchStatusQuery.data as ImportBatchStatus | undefined;
  const batches = targetedBatch
    ? [
        targetedBatch,
        ...activeBatches.filter((batch) => batch.batch_id !== targetedBatch.batch_id),
      ]
    : activeBatches;
  const summary = summarizeImportBatches(batches);
  const knowledgePages = (pagesQuery.data ?? [])
    .filter(isKnowledgePage)
    .slice(0, LIVE_PAGE_COUNT);

  // Pages and batches decide the view before the memory count is ever read:
  // the count stays off whenever there is real progress to show.
  const decidesEmpty = !listPending && !listFailed
    && batches.length === 0 && knowledgePages.length === 0;
  const memoryStatsQuery = useQuery({
    queryKey: ["memoryStats"],
    queryFn: getMemoryStats,
    enabled: decidesEmpty,
    refetchInterval: LIVE_PAGE_POLL_MS,
  });
  const savedCount = memoryStatsQuery.data?.total ?? 0;

  // A failed query is a failure, never an empty success: it gets its own
  // state with an explicit retry, not the empty-state copy.
  const pending = listPending || (decidesEmpty && memoryStatsQuery.isPending);
  const failed = listFailed || (decidesEmpty && memoryStatsQuery.isError);

  const routing = routingQuery.data;
  const everydayBlocked = routing != null && routing.everyday.mode !== "pinned";
  const synthesisBlocked = routing != null && routing.synthesis.mode !== "pinned";
  const needsIntelligence = batches.some((batch) => !batch.complete)
    && (everydayBlocked || synthesisBlocked);
  // Same blocked check, for the saved-with-no-batches state below.
  const savedNeedsIntelligence = everydayBlocked || synthesisBlocked;
  const canReportRunning = routing != null && !routingQuery.isError && !needsIntelligence;
  // Batch counters describe queued work, not whether a model can run it.
  const displayPhases = summary.phases.map((phase) => {
    const blocked = phase.phase === "distill"
      ? everydayBlocked || synthesisBlocked
      : everydayBlocked && ["detect", "enrich", "link"].includes(phase.phase);
    return blocked && phase.state === "running" ? { ...phase, state: "pending" as const } : phase;
  });
  const runningNames = summary.runningPhases.map((phase) =>
    t(`importBatch.phases.${phase}`),
  );
  const settled = batches.length > 0 && batches.every((batch) => batch.complete);

  const retry = () => {
    void routingQuery.refetch();
    void pagesQuery.refetch();
    void activeBatchesQuery.refetch();
    if (batchId) void batchStatusQuery.refetch();
    if (decidesEmpty) void memoryStatsQuery.refetch();
  };

  return (
    <div data-testid="first-use-live" className="fug-guide">
      <div className="fug-row">
        <button type="button" className="fug-back" onClick={onBack}>
          <ArrowLeft aria-hidden="true" weight="regular" />
          {t("firstUse.live.back")}
        </button>
        <button
          type="button"
          className="fus-link"
          onClick={retry}
          disabled={pending}
        >
          {t("firstUse.live.refresh")}
        </button>
      </div>
      <h1 className="fug-title">{t("firstUse.live.title")}</h1>
      <p className="fug-lede">{t("firstUse.live.body")}</p>

      {pending && !failed ? <p className="fus-note">{t("firstUse.live.waiting")}</p> : null}

      {failed ? (
        <div className="fus-state" role="alert">
          <p>{t("firstUse.live.loadFailed")}</p>
          <button type="button" className="fug-button-secondary" onClick={retry}>
            {t("firstUse.live.retry")}
          </button>
        </div>
      ) : null}

      {!pending && !failed && batches.length > 0 ? (
        <section aria-labelledby="fus-live-active">
          <h2 id="fus-live-active" className="fus-section-title">
            {t("firstUse.live.activeTitle")}
          </h2>
          <ImportPhaseList phases={displayPhases} onDemand={steepOnDemand} />
          <p className="fus-note">
            {t("importBatch.summaryImported", {
              count: summary.memoriesImported,
              source: [...new Set(batches.map((batch) => batch.source === "other" ? t("importBatch.otherSource") : importSourceLabel(batch.source)))].join(" · "),
            })}
            {summary.memoriesSkipped > 0
              ? ` · ${t("importBatch.summarySkipped", { count: summary.memoriesSkipped })}`
              : ""}
          </p>
          {canReportRunning && runningNames.length > 0 ? (
            <p className="fus-note">
              {t("firstUse.live.queuedNote")}
            </p>
          ) : null}
          {needsIntelligence ? (
            <p className="fus-note" role="status">{t("firstUse.live.needsIntelligence")}</p>
          ) : null}
          {!settled && routingQuery.isPending ? (
            <p className="fus-note" role="status">{t("firstUse.live.modelStatusPending")}</p>
          ) : null}
          {!settled && routingQuery.isError ? (
            <div className="fus-state" role="alert">
              <p>{t("firstUse.live.modelStatusFailed")}</p>
              <button type="button" className="fug-button-secondary" onClick={() => void routingQuery.refetch()}>
                {t("firstUse.live.retry")}
              </button>
            </div>
          ) : null}
          {summary.complete ? (
            <p className="fus-note">{t("importBatch.backgroundSettled")}</p>
          ) : null}
          {summary.failedPhases.length > 0 ? (
            <p className="fus-warning" role="alert">
              {t("firstUse.live.failedNote")}
            </p>
          ) : null}
          {settled && !summary.hasRelatedPages ? (
            <p className="fus-note">{t("firstUse.live.settledNoPages")}</p>
          ) : null}
          {needsIntelligence || (settled && (!summary.hasRelatedPages || summary.failedPhases.length > 0)) ? (
            <button type="button" className="fug-button-secondary" onClick={onOpenIntelligence}>
              {t("firstUse.live.intelligenceAction")}
            </button>
          ) : null}
        </section>
      ) : null}

      {!pending && !failed && batches.length === 0 && knowledgePages.length > 0 ? (
        <p className="fus-note">{t("firstUse.live.noActiveImports")}</p>
      ) : null}

      {!pending && !failed && knowledgePages.length > 0 ? (
        <section aria-labelledby="fus-live-pages">
          <h2 id="fus-live-pages" className="fus-section-title">
            {t("firstUse.live.yourPages")}
          </h2>
          <p className="fus-note">{t("firstUse.live.yourPagesNote")}</p>
          <ul className="fus-pages">
            {knowledgePages.map((page) => (
              <li key={page.id}>
                <button
                  type="button"
                  data-testid={`first-use-page-${page.id}`}
                  className="fus-page-row"
                  onClick={() => onOpenPage(page.id)}
                >
                  <BookOpenText aria-hidden="true" weight="regular" />
                  <span>{page.title}</span>
                </button>
              </li>
            ))}
          </ul>
        </section>
      ) : null}

      {!pending && !failed && batches.length === 0 && knowledgePages.length === 0
      && savedCount === 0 ? (
        <div className="fus-state">
          <p className="fus-section-title">{t("firstUse.live.emptyTitle")}</p>
          <p className="fus-note">{t("firstUse.live.emptyBody")}</p>
          <div className="fus-cta-row">
            <button
              type="button"
              className="fug-button-primary"
              onClick={onImport}
            >
              {t("firstUse.live.importAction")}
            </button>
            <button
              type="button"
              className="fug-button-secondary"
              onClick={onOpenIntelligence}
            >
              {t("firstUse.live.intelligenceAction")}
            </button>
          </div>
        </div>
      ) : null}

      {!pending && !failed && batches.length === 0 && knowledgePages.length === 0
      && savedCount > 0 ? (
        <div className="fus-state" data-testid="first-use-live-saved">
          <p className="fus-section-title">{t("firstUse.live.savedTitle")}</p>
          <p className="fus-note">
            {t("firstUse.live.savedBody", { count: savedCount })}
          </p>
          {savedNeedsIntelligence ? (
            <>
              <p className="fus-note" role="status">{t("firstUse.live.needsIntelligence")}</p>
              <div className="fus-cta-row">
                <button
                  type="button"
                  className="fug-button-secondary"
                  onClick={onOpenIntelligence}
                >
                  {t("firstUse.live.intelligenceAction")}
                </button>
              </div>
            </>
          ) : (
            <p className="fus-note">{t("firstUse.live.savedNote")}</p>
          )}
        </div>
      ) : null}
    </div>
  );
}
