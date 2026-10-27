// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  getImportBatchStatus,
  type ImportBatchStatus,
  type ImportPhase,
  type ImportPhaseState,
  type ImportPhaseStatus,
} from "../../lib/tauri";
import { ImportOrganize, useOrganizeActions } from "./askAi/TurnIntoPages";

/** Every phase, in the order a user sees them. Matches ImportPhase::ALL. */
export const IMPORT_PHASE_ORDER: ImportPhase[] = [
  "ingest",
  "store",
  "detect",
  "enrich",
  "link",
  "distill",
];

/**
 * The phases the import surface shows.
 *
 * The other four moved to the toolbar Activity button and the Activity page. Shown
 * in the foreground they read as cost before proof: a user watched six phases
 * grind for minutes with nothing usable in sight. Ingest and Store are the two
 * that end in something they can use, so those are the two that stay.
 *
 * IMPORT_PHASE_ORDER is unchanged: it mirrors ImportPhase::ALL and other
 * callers read it for the full pipeline.
 */
export const IMPORT_VISIBLE_PHASES: ImportPhase[] = ["ingest", "store"];

/** How often to re-read the daemon's live row counts. Matches the chat-import
 *  poll in ChatImport/ImportFlow (immediate read, then an interval). */
export const IMPORT_BATCH_POLL_MS = 1_500;

/** Display names for import source slugs. Product names stay English in every
 *  locale, matching the existing settings copy. */
export const IMPORT_SOURCE_LABELS: Record<string, string> = {
  chatgpt: "ChatGPT",
  claude: "Claude",
  other: "Other",
};

export function importSourceLabel(source: string): string {
  return IMPORT_SOURCE_LABELS[source] ?? source;
}

/**
 * Live row counts for one import batch. Same polling shape as ImportFlow's
 * pending-import poll: an immediate read, a 1.5 s interval, an `alive` guard,
 * and cleanup on unmount. Stops polling once the daemon reports `complete`.
 *
 * Pass `uploading` while chunks are still being posted. The daemon reports
 * `complete` for the memories it has, not for the ones still queued in the
 * browser, so on an install with no LLM provider — where every enrichment step
 * records `skipped` the moment it lands — chunk 1 can settle before chunk 2 is
 * sent. Stopping there froze the phase list at chunk-1 numbers and printed
 * "settled" over an import with most of its work still to come, and nothing
 * restarted the poll. Held open, the poll survives to the end of the upload.
 */
export function useImportBatchStatus(
  batchId: string | null,
  uploading = false,
): ImportBatchStatus | null {
  const [status, setStatus] = useState<ImportBatchStatus | null>(null);
  // A ref, not a dep: flipping `uploading` must not tear down the interval and
  // blank the counts mid-import.
  const uploadingRef = useRef(uploading);
  uploadingRef.current = uploading;

  useEffect(() => {
    setStatus(null);
    if (!batchId) return;
    let alive = true;
    let id: ReturnType<typeof setInterval> | null = null;
    const poll = () => {
      getImportBatchStatus(batchId)
        .then((next) => {
          if (!alive) return;
          setStatus(next);
          if (next.complete && !uploadingRef.current && id !== null) {
            clearInterval(id);
            id = null;
          }
        })
        .catch(() => {});
    };
    poll();
    id = setInterval(poll, IMPORT_BATCH_POLL_MS);
    return () => {
      alive = false;
      if (id !== null) clearInterval(id);
    };
  }, [batchId]);

  return status;
}

export interface ImportBatchSummary {
  phases: ImportPhaseStatus[];
  memoriesImported: number;
  memoriesSkipped: number;
  entitiesDetected: number;
  entitiesEstablished: number;
  /** Unknown across batches: one page can cite several imports. */
  pagesDistilled: number | null;
  hasRelatedPages: boolean;
  /** Every batch reports every background phase settled. */
  complete: boolean;
  /** Phases still doing work, in display order. */
  runningPhases: ImportPhase[];
  failedPhases: ImportPhase[];
}

function emptyPhase(phase: ImportPhase): ImportPhaseStatus {
  return { phase, state: "pending", done: 0, total: 0, failed: 0 };
}

/**
 * One pill however many batches are active: per-phase counts add up across
 * batches, and a phase reads as failed/running while any batch says so.
 */
export function summarizeImportBatches(batches: ImportBatchStatus[]): ImportBatchSummary {
  const byPhase = new Map<ImportPhase, ImportPhaseStatus>();
  let memoriesImported = 0;
  let memoriesSkipped = 0;
  let entitiesDetected = 0;
  let entitiesEstablished = 0;
  let pagesDistilled = 0;

  for (const batch of batches) {
    memoriesImported += batch.memories_imported;
    memoriesSkipped += batch.memories_skipped;
    entitiesDetected += batch.entities_detected;
    entitiesEstablished += batch.entities_established;
    pagesDistilled += batch.pages_distilled;
    for (const entry of batch.phases) {
      const prev = byPhase.get(entry.phase);
      if (!prev) {
        byPhase.set(entry.phase, { ...entry });
        continue;
      }
      prev.done += entry.done;
      prev.total += entry.total;
      prev.failed += entry.failed;
      prev.state = combineState(prev.state, entry.state);
    }
  }

  // A batch that never reported a phase row leaves it pending — and a phase
  // no batch finished stays out of `complete`, so the pill cannot claim work
  // is done that a batch has not even started.
  const phases = IMPORT_PHASE_ORDER.map((phase) => byPhase.get(phase) ?? emptyPhase(phase));
  const batchesSettled = batches.length > 0 && batches.every((batch) => batch.complete);
  const runningPhases = phases
    .filter((p) => (p.state === "running" || p.state === "pending")
      && !(p.phase === "distill" && p.state === "pending" && batchesSettled))
    .map((p) => p.phase);
  const failedPhases = phases.filter((p) => p.state === "failed").map((p) => p.phase);
  const complete =
    batches.length > 0 &&
    batches.every((b) => b.complete) &&
    runningPhases.length === 0 &&
    failedPhases.length === 0;

  return {
    phases,
    memoriesImported,
    memoriesSkipped,
    entitiesDetected,
    entitiesEstablished,
    pagesDistilled: batches.length > 1 ? null : pagesDistilled,
    hasRelatedPages: pagesDistilled > 0,
    complete,
    runningPhases,
    failedPhases,
  };
}

function combineState(a: ImportPhaseState, b: ImportPhaseState): ImportPhaseState {
  if (a === "failed" || b === "failed") return "failed";
  if (a === "running" || b === "running") return "running";
  if (a === "pending" || b === "pending") return "pending";
  return "complete";
}

function PhaseDot({ state }: { state: ImportPhaseState }) {
  const color =
    state === "complete"
      ? "var(--mem-accent-sage)"
      : state === "failed"
        ? "#ef4444"
        : state === "running"
          ? "var(--mem-accent-indigo)"
          : "var(--mem-text-tertiary)";
  if (state === "complete") {
    return (
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none"
        stroke={color} strokeWidth="2.5"
        strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
        <polyline points="20 6 9 17 4 12" />
      </svg>
    );
  }
  return (
    <span
      aria-hidden="true"
      style={{
        width: "8px",
        height: "8px",
        borderRadius: "50%",
        backgroundColor: color,
        flexShrink: 0,
        opacity: state === "pending" ? 0.5 : 1,
      }}
    />
  );
}

/**
 * The six import phases with their real daemon counts. Shared by the import
 * view and the Home detail panel — one rendering, never a fork.
 *
 * A phase with no known total reads as waiting, never as 0%. `distill` never
 * reports a usable total, so it always renders as a live count with no bar.
 */
/** `onDemand`: organizing is On demand, so the handoff names the AI tool, not the background. */
export function ImportPhaseList({ phases, onDemand = false }: { phases: ImportPhaseStatus[]; onDemand?: boolean }) {
  const { t } = useTranslation();
  const organize = useOrganizeActions() !== null;
  const byPhase = new Map<ImportPhase, ImportPhaseStatus>();
  for (const p of phases) byPhase.set(p.phase, p);

  const stored = byPhase.get("store");
  const handedOff = stored?.state === "complete";

  return (
    <div style={{ width: "100%", display: "flex", flexDirection: "column", gap: "4px" }}>
      {IMPORT_VISIBLE_PHASES.map((phase) => {
        const entry = byPhase.get(phase) ?? emptyPhase(phase);
        const hasTotal = entry.total > 0;
        const fraction = hasTotal ? Math.min(entry.done / entry.total, 1) : 0;
        return (
          <div
            key={phase}
            data-testid={`import-phase-${phase}`}
            data-state={entry.state}
            style={{
              display: "flex",
              alignItems: "center",
              gap: "14px",
              padding: "10px 14px",
              borderRadius: "10px",
              backgroundColor: entry.state === "running" ? "var(--mem-surface)" : "transparent",
              border: entry.state === "running"
                ? "1px solid var(--mem-border)"
                : "1px solid transparent",
              opacity: entry.state === "pending" ? 0.55 : 1,
            }}
          >
            <div style={{
              width: "32px",
              height: "32px",
              borderRadius: "8px",
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              flexShrink: 0,
            }}>
              <PhaseDot state={entry.state} />
            </div>

            <span style={{
              fontFamily: "var(--mem-font-body)",
              fontSize: "13px",
              fontWeight: entry.state === "running" ? 500 : 400,
              color: entry.state === "failed" ? "#ef4444" : "var(--mem-text)",
            }}>
              {t(`importBatch.phases.${phase}`)}
            </span>

            <span style={{
              marginLeft: "auto",
              display: "flex",
              alignItems: "center",
              gap: "10px",
              fontFamily: "var(--mem-font-mono)",
              fontSize: "11px",
              color: "var(--mem-text-tertiary)",
              whiteSpace: "nowrap",
            }}>
              {entry.state === "failed" && (
                <span style={{ color: "#ef4444" }}>{t("importBatch.states.failed")}</span>
              )}
              {hasTotal ? (
                <span>{t("importBatch.countOf", { done: entry.done, total: entry.total })}</span>
              ) : (
                <span>{t("importBatch.states.waiting")}</span>
              )}
              {entry.failed > 0 && (
                <span style={{ color: "#ef4444" }}>
                  {t("importBatch.failedUnits", { count: entry.failed })}
                </span>
              )}
            </span>

            {hasTotal && (
              <div
                role="progressbar"
                aria-label={t(`importBatch.phases.${phase}`)}
                aria-valuemin={0}
                aria-valuemax={entry.total}
                aria-valuenow={entry.done}
                style={{
                  width: "48px",
                  height: "3px",
                  borderRadius: "2px",
                  backgroundColor: "var(--mem-border)",
                  overflow: "hidden",
                  flexShrink: 0,
                }}
              >
                <div style={{
                  height: "100%",
                  borderRadius: "2px",
                  backgroundColor: entry.state === "failed" ? "#ef4444" : "var(--mem-accent-indigo)",
                  width: `${fraction * 100}%`,
                  transition: "width 0.3s ease-out",
                }} />
              </div>
            )}
          </div>
        );
      })}
      {handedOff && organize && (stored?.done ?? 0) > 0 ? (
        // Inside the main window the handoff is the next step itself: the
        // shared card that turns the stored material into pages.
        <div style={{ margin: "8px 14px" }}>
          <ImportOrganize count={stored?.done ?? 0} kind="note" />
        </div>
      ) : handedOff && (
        // The handoff: everything stored is searchable now, and the organizing
        // that follows is reported by the toolbar Activity button, not here.
        <p
          data-testid="import-handoff"
          style={{ margin: "8px 14px", fontSize: "12px", lineHeight: 1.6, color: "var(--mem-text-secondary)" }}
        >
          {t(onDemand ? "activityStatus.importHandoffOnDemand" : "activityStatus.importHandoff", { count: stored?.done ?? 0 })}
        </p>
      )}
    </div>
  );
}

/**
 * One header pill summarising the background phases of every active batch.
 * Renders nothing when no batch is active.
 */
export function ImportStatusPill({
  batches,
  onOpen,
}: {
  batches: ImportBatchStatus[];
  onOpen: () => void;
}) {
  const { t } = useTranslation();
  if (batches.length === 0) return null;
  const summary = summarizeImportBatches(batches);
  const failed = summary.failedPhases.length > 0;
  const nextPhase = summary.runningPhases[0] ?? summary.failedPhases[0];

  return (
    <button
      type="button"
      data-testid="import-status-pill"
      onClick={onOpen}
      aria-label={t("importBatch.openDetail")}
      style={{
        display: "inline-flex",
        alignItems: "center",
        gap: "6px",
        fontFamily: "var(--mem-font-mono)",
        fontVariantNumeric: "tabular-nums",
        fontSize: "var(--mem-text-control)",
        fontWeight: 400,
        borderRadius: 999,
        padding: "1px 8px",
        border: "none",
        cursor: "pointer",
        color: failed ? "#ef4444" : "var(--mem-accent-indigo)",
        backgroundColor: failed
          ? "color-mix(in srgb, #ef4444 12%, transparent)"
          : "var(--mem-indigo-bg)",
        whiteSpace: "nowrap",
      }}
    >
      <span
        aria-hidden="true"
        style={{
          width: "6px",
          height: "6px",
          borderRadius: "50%",
          backgroundColor: "currentColor",
        }}
      />
      {failed ? t("importBatch.pillFailed") : t("importBatch.pillActive")}
      {nextPhase && (
        <span style={{ opacity: 0.75 }}>{t(`importBatch.phases.${nextPhase}`)}</span>
      )}
    </button>
  );
}

/**
 * The pill's detail view: per-phase progress plus the live totals, with a
 * Back control returning to Home.
 */
export function ImportDetailPanel({
  batches,
  onBack,
  onDemand = false,
}: {
  batches: ImportBatchStatus[];
  onBack: () => void;
  onDemand?: boolean;
}) {
  const { t } = useTranslation();
  const summary = summarizeImportBatches(batches);

  return (
    <div data-testid="import-detail-panel" style={{ display: "flex", flexDirection: "column", gap: "16px" }}>
      <div style={{ display: "flex", alignItems: "center", gap: "12px" }}>
        <button
          type="button"
          data-testid="import-detail-back"
          onClick={onBack}
          style={{
            display: "inline-flex",
            alignItems: "center",
            gap: "4px",
            background: "none",
            border: "none",
            cursor: "pointer",
            fontFamily: "var(--mem-font-body)",
            fontSize: "13px",
            color: "var(--mem-text-secondary)",
            padding: 0,
          }}
        >
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" aria-hidden="true">
            <path d="M19 12H5M12 19l-7-7 7-7" />
          </svg>
          {t("main.back")}
        </button>
        <h2 style={{
          fontFamily: "var(--mem-font-heading)",
          fontSize: 18,
          fontWeight: 500,
          color: "var(--mem-text)",
          margin: 0,
        }}>
          {t("importBatch.title")}
        </h2>
      </div>

      <ImportPhaseList phases={summary.phases} onDemand={onDemand} />

      <p style={{
        fontFamily: "var(--mem-font-mono)",
        fontSize: "11px",
        color: "var(--mem-text-tertiary)",
        margin: 0,
      }}>
        {t("importBatch.summaryImported", {
          count: summary.memoriesImported,
          source: [...new Set(batches.map((batch) => batch.source === "other" ? t("importBatch.otherSource") : importSourceLabel(batch.source)))].join(" · "),
        })}
        {summary.memoriesSkipped > 0 && ` · ${t("importBatch.summarySkipped", { count: summary.memoriesSkipped })}`}
      </p>
      {!summary.complete && summary.failedPhases.length === 0 ? (
        <p style={{ fontSize: "12px", color: "var(--mem-text-secondary)", margin: 0 }}>
          {t("importBatch.backgroundRunning")}
        </p>
      ) : null}
    </div>
  );
}
