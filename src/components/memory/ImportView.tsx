import { WorkspaceBackButton } from "./navigation/WorkspaceNavigation";
import { useState, useRef, useCallback } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import {
  importMemories,
  clipboardWrite,
  IMPORT_CHUNK_SIZE,
  type ImportResult,
} from "../../lib/tauri";
import { IMPORT_SOURCE_LABELS, ImportPhaseList, useImportBatchStatus } from "./ImportPhases";
import { formatImportErrorDetail } from "./importCopy";
import { useSteepOnDemand } from "../../lib/steepMode";
import { ImportedLead, ImportOrganize, useOrganizeActions } from "./askAi/TurnIntoPages";

type Source = "chatgpt" | "claude" | "other";

interface ImportViewProps {
  onBack: () => void;
  onComplete: (source: string, result: ImportResult) => void;
  /** Name the destination when the importing flow returns to a guided view. */
  completeLabel?: string;
  /** When true, show "Continue" instead of "View memories" / "Import more" in summary. */
  wizardMode?: boolean;
  /** Called when internal phase changes. */
  onPhaseChange?: (phase: Phase) => void;
  /** When provided, shows a skip link in the footer. */
  onSkip?: () => void;
  /** Optional onboarding copy rendered above the import form. */
  wizardHint?: React.ReactNode;
}

type Phase = "input" | "progress" | "summary";

/**
 * Split pasted lines into upload chunks. One memory per non-empty line; empty
 * lines and separators never reach the daemon.
 */
export function chunkImportText(text: string, chunkSize: number = IMPORT_CHUNK_SIZE): string[] {
  const lines = text.split("\n").filter((l) => l.trim() !== "");
  const chunks: string[] = [];
  for (let i = 0; i < lines.length; i += chunkSize) {
    chunks.push(lines.slice(i, i + chunkSize).join("\n"));
  }
  return chunks;
}

const SOURCE_LABELS: Record<Source, string> = IMPORT_SOURCE_LABELS;

// Aligned with Wenlan's --mem-accent-* palette (see FACET_COLORS in tauri.ts)
const TYPE_BADGE_STYLES: Record<string, { bg: string; text: string }> = {
  identity: { bg: "color-mix(in srgb, var(--mem-accent-indigo) 15%, transparent)", text: "var(--mem-accent-indigo)" },
  preference: { bg: "color-mix(in srgb, var(--mem-accent-warm) 15%, transparent)", text: "var(--mem-accent-warm)" },
  fact: { bg: "color-mix(in srgb, var(--mem-accent-glow) 12%, transparent)", text: "var(--mem-accent-glow)" },
  decision: { bg: "color-mix(in srgb, var(--mem-accent-amber) 15%, transparent)", text: "var(--mem-accent-amber)" },
  lesson: { bg: "color-mix(in srgb, var(--mem-accent-sage) 15%, transparent)", text: "var(--mem-accent-sage)" },
  gotcha: { bg: "color-mix(in srgb, #ef4444 15%, transparent)", text: "#ef4444" },
  goal: { bg: "color-mix(in srgb, var(--mem-accent-sage) 15%, transparent)", text: "var(--mem-accent-sage)" },
};

export function ImportView({ onBack, onComplete, completeLabel, wizardMode, onPhaseChange, onSkip, wizardHint }: ImportViewProps) {
  const queryClient = useQueryClient();
  const { t } = useTranslation();
  const onDemand = useSteepOnDemand() === true;
  const organize = useOrganizeActions() !== null;
  // The finish card shows a button: "View memories" steps back to secondary.
  const [cardAction, setCardAction] = useState(false);
  const [phase, setPhaseRaw] = useState<Phase>("input");
  const setPhase = useCallback((p: Phase) => {
    setPhaseRaw(p);
    onPhaseChange?.(p);
  }, [onPhaseChange]);
  const [source, setSource] = useState<Source>("chatgpt");
  const [text, setText] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<ImportResult | null>(null);
  const [batchId, setBatchId] = useState<string | null>(null);
  const [chunkProgress, setChunkProgress] = useState<{ done: number; total: number } | null>(null);
  const [uploading, setUploading] = useState(false);
  const [promptCopied, setPromptCopied] = useState(false);
  const fileRef = useRef<HTMLInputElement>(null);
  // Monotonic id for file reads: a completion (or error) only lands while
  // its id is still current, so selecting a second file — or typing/pasting
  // while a read is pending — can never overwrite newer text.
  const readSeq = useRef(0);
  const batchStatus = useImportBatchStatus(batchId, uploading);
  const typeLabels = t("importView.typeLabels", {
    returnObjects: true,
  }) as unknown as Record<string, string>;
  // Shown and copied from the same localized string: the full export
  // instructions are localized per locale, with only the [TYPE] machine
  // tags staying literal.
  const exportPromptBody = t("importView.exportPromptBody");

  // Brand names stay literal in every locale; only "Other" is localized.
  // IMPORT_SOURCE_LABELS itself is owned by ImportPhases and stays as-is.
  const sourceLabel = (s: Source) =>
    s === "other" ? t("importView.sourceOther") : (SOURCE_LABELS[s] ?? s);

  const handleCopyPrompt = async () => {
    setError(null);
    setPromptCopied(false);
    try {
      await clipboardWrite(exportPromptBody);
      setPromptCopied(true);
      setTimeout(() => setPromptCopied(false), 2000);
    } catch {
      setError(t("importView.copyError"));
    }
  };

  const handleFileUpload = (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    if (!file) return;
    setError(null);
    const id = ++readSeq.current;
    const reader = new FileReader();
    const readFailed = () => {
      if (id === readSeq.current) setError(t("importView.fileReadError"));
    };
    reader.onload = (ev) => {
      if (id !== readSeq.current) return;
      const content = ev.target?.result;
      if (typeof content === "string") {
        setText(content);
      } else {
        setError(t("importView.fileReadError"));
      }
    };
    reader.onerror = readFailed;
    reader.onabort = readFailed;
    try {
      reader.readAsText(file);
    } catch {
      readFailed();
    }
    // Reset so the same file can be re-selected
    e.target.value = "";
  };

  const handleImport = async () => {
    setError(null);
    const chunks = chunkImportText(text);
    // One batch id for the whole import, shared by every chunk, so the
    // daemon can report one honest aggregate under import_batch_status_cmd.
    const id = crypto.randomUUID();
    setBatchId(id);
    setChunkProgress({ done: 0, total: chunks.length });
    setUploading(true);
    setPhase("progress");
    try {
      const agg: ImportResult = {
        imported: 0,
        skipped: 0,
        breakdown: {},
        entities_created: 0,
        observations_added: 0,
        relations_created: 0,
        batch_id: id,
      };
      for (let i = 0; i < chunks.length; i++) {
        const res = await importMemories(source, chunks[i]!, undefined, {
          batchId: id,
          chunkIndex: i,
          chunkTotal: chunks.length,
        });
        agg.imported += res.imported;
        agg.skipped += res.skipped;
        agg.entities_created += res.entities_created;
        agg.observations_added += res.observations_added;
        agg.relations_created += res.relations_created;
        for (const [type, count] of Object.entries(res.breakdown ?? {})) {
          agg.breakdown[type] = (agg.breakdown[type] ?? 0) + count;
        }
        setChunkProgress({ done: i + 1, total: chunks.length });
      }
      setUploading(false);
      setResult(agg);
      // The request phases (ingest, store) are done here; detect and later
      // keep running in the background. The summary says so honestly while
      // the status poll keeps the live counts climbing.
      setPhase("summary");
      queryClient.invalidateQueries();
    } catch (err) {
      setUploading(false);
      // A friendly localized heading, never swallowing the backend detail.
      const detail = formatImportErrorDetail(err);
      const heading = t("importView.importFailedTitle");
      setError(detail ? `${heading}: ${detail}` : heading);
      setBatchId(null);
      setChunkProgress(null);
      setPhase("input");
    }
  };

  const handleReset = () => {
    readSeq.current++;
    setUploading(false);
    setPhase("input");
    setText("");
    setError(null);
    setResult(null);
    setBatchId(null);
    setChunkProgress(null);
  };

  // ── Input form ──────────────────────────────────────────────────────
  if (phase === "input") {
    return (
      <div className="flex flex-col mx-auto py-4" style={{ height: "calc(100vh - 120px)", maxWidth: "672px" }}>
        {/* Header row: back + title + source pills */}
        <div className="flex items-center gap-4 mb-4 shrink-0">
          <WorkspaceBackButton
            onClick={onBack}
            aria-label={t("importView.back")}
            className="flex items-center gap-1 shrink-0 transition-colors duration-150"
            style={{
              fontFamily: "var(--mem-font-body)",
              fontSize: "13px",
              color: "var(--mem-text-secondary)",
            }}
          >
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
              <path d="M19 12H5M12 19l-7-7 7-7" />
            </svg>
          </WorkspaceBackButton>
          <h1
            className="shrink-0"
            style={{
              fontFamily: "var(--mem-font-heading)",
              fontSize: "18px",
              fontWeight: 500,
              color: "var(--mem-text)",
            }}
          >
            {t("importView.title")}
          </h1>
          <div className="flex gap-1 ml-auto shrink-0">
            {(["chatgpt", "claude", "other"] as Source[]).map((s) => (
              <button
                key={s}
                onClick={() => setSource(s)}
                className="px-2.5 py-1 rounded-md text-xs font-medium transition-colors duration-150"
                style={{
                  fontFamily: "var(--mem-font-body)",
                  backgroundColor: source === s ? "var(--mem-accent-indigo)" : "transparent",
                  color: source === s ? "white" : "var(--mem-text-tertiary)",
                }}
              >
                {sourceLabel(s)}
              </button>
            ))}
          </div>
        </div>

        {wizardMode && wizardHint && (
          <div
            className="mb-4 rounded-lg px-3 py-2 shrink-0"
            style={{
              backgroundColor: "var(--mem-hover)",
              border: "1px solid var(--mem-border)",
              fontFamily: "var(--mem-font-body)",
              fontSize: "12px",
              color: "var(--mem-text-secondary)",
              lineHeight: "1.5",
            }}
          >
            {wizardHint}
          </div>
        )}

        {/* Two equal panels */}
        <div className="flex flex-col gap-3 flex-1 min-h-0">
          {/* Top panel: export prompt or instructions */}
          <div className="flex flex-col flex-1 min-h-0">
            <div className="flex items-center justify-between mb-1.5 shrink-0">
              <span style={{ fontFamily: "var(--mem-font-body)", fontSize: "12px", fontWeight: 500, color: "var(--mem-text-secondary)" }}>
                {source !== "other" ? t("importView.exportPrompt") : t("importView.instructions")}
              </span>
              {source !== "other" && (
                <button
                  onClick={handleCopyPrompt}
                  className="px-2.5 py-0.5 rounded-md text-xs font-medium transition-colors"
                  style={{
                    fontFamily: "var(--mem-font-body)",
                    color: promptCopied ? "var(--mem-accent-warm)" : "var(--mem-accent-indigo)",
                    backgroundColor: promptCopied ? "rgba(251, 191, 36, 0.15)" : "rgba(123, 123, 232, 0.1)",
                  }}
                >
                  {promptCopied ? t("importView.copied") : t("importView.copyPrompt")}
                </button>
              )}
            </div>
            <pre
              className="rounded-lg px-3 py-2 overflow-auto text-[11px] leading-relaxed flex-1 min-h-0"
              style={{
                fontFamily: "var(--mem-font-mono)",
                backgroundColor: "var(--mem-hover)",
                color: "var(--mem-text-tertiary)",
                border: "1px solid var(--mem-border)",
                whiteSpace: "pre-wrap",
                margin: 0,
              }}
            >
              {source !== "other"
                ? `${t("importView.exportIntro", { source: sourceLabel(source) })}\n\n${exportPromptBody}`
                : t("importView.otherInstructions")}
            </pre>
          </div>

          {/* Bottom panel: paste area */}
          <div className="flex flex-col flex-1 min-h-0">
            <div className="flex items-center justify-between mb-1.5 shrink-0">
              <span style={{ fontFamily: "var(--mem-font-body)", fontSize: "12px", fontWeight: 500, color: "var(--mem-text-secondary)" }}>
                {t("importView.pasteOutput")}
              </span>
              {text && (
                <span style={{ fontFamily: "var(--mem-font-mono)", fontSize: "11px", color: "var(--mem-text-tertiary)" }}>
                  {t("importView.lines", { count: text.split("\n").filter((l) => l.trim()).length })}
                </span>
              )}
            </div>
            <textarea
              value={text}
              onChange={(e) => {
                // Typing or pasting invalidates any pending file read.
                readSeq.current++;
                setText(e.target.value);
              }}
              placeholder={t("importView.placeholder")}
              aria-label={t("importView.pasteOutput")}
              className="w-full rounded-lg px-3 py-2 focus:outline-none focus:ring-1 focus:ring-[var(--mem-accent-indigo)]/40 flex-1 min-h-0"
              style={{
                fontFamily: "var(--mem-font-body)",
                fontSize: "13px",
                color: "var(--mem-text)",
                backgroundColor: "var(--mem-surface)",
                border: "1px solid var(--mem-border)",
                resize: "none",
                lineHeight: "1.6",
              }}
            />
          </div>
        </div>

        {/* Footer: upload + skip + error + import */}
        <div className="flex items-center gap-3 mt-3 shrink-0">
          <button
            onClick={() => fileRef.current?.click()}
            className="transition-colors"
            style={{
              fontFamily: "var(--mem-font-body)",
              fontSize: "12px",
              color: "var(--mem-accent-indigo)",
            }}
          >
            {t("importView.uploadFile")}
          </button>
          <input ref={fileRef} type="file" accept=".txt,.csv,.json" className="hidden" aria-label={t("importView.uploadFile")} onChange={handleFileUpload} />
          {error && (
            <span style={{ fontFamily: "var(--mem-font-body)", fontSize: "12px", color: "#ef4444" }}>
              {error}
            </span>
          )}
          <div className="flex items-center gap-3 ml-auto">
            {onSkip && (
              <button
                onClick={onSkip}
                className="transition-colors duration-150"
                style={{
                  fontFamily: "var(--mem-font-body)",
                  fontSize: "13px",
                  color: "var(--mem-text-tertiary)",
                  background: "none",
                  border: "none",
                  cursor: "pointer",
                }}
              >
                {t("importView.skip")}
              </button>
            )}
            <button
              onClick={handleImport}
              disabled={!text.trim()}
              className="px-4 py-2 rounded-lg text-sm font-medium transition-colors duration-150 disabled:opacity-40 disabled:cursor-not-allowed"
              style={{
                fontFamily: "var(--mem-font-body)",
                backgroundColor: "var(--mem-accent-indigo)",
                color: "white",
              }}
            >
              {t("importView.import")}
            </button>
          </div>
        </div>
      </div>
    );
  }

  // ── Progress: live daemon row counts, never a timer ───────────────
  if (phase === "progress") {
    // `chunkImportText` returns upload chunks, not memories: a 1,200-line
    // paste is 3 chunks, and the heading has to say 1,200.
    const lineCount = text.split("\n").filter((l) => l.trim() !== "").length;
    return (
      <div className="flex flex-col items-center max-w-md mx-auto py-16" style={{ gap: "32px" }}>
        <div className="text-center" style={{ gap: "8px", display: "flex", flexDirection: "column" }}>
          <p style={{
            fontFamily: "var(--mem-font-heading)",
            fontSize: "28px",
            fontWeight: 400,
            color: "var(--mem-text)",
            letterSpacing: "-0.02em",
          }}>
            {t("importBatch.progressTitle", { count: lineCount })}
          </p>
          {chunkProgress && chunkProgress.total > 1 && (
            <p style={{
              fontFamily: "var(--mem-font-body)",
              fontSize: "13px",
              color: "var(--mem-text-tertiary)",
            }}>
              {t("importBatch.chunkProgress", { done: Math.min(chunkProgress.done + 1, chunkProgress.total), total: chunkProgress.total })}
            </p>
          )}
        </div>

        <ImportPhaseList phases={batchStatus?.phases ?? []} onDemand={onDemand} />
      </div>
    );
  }

  // ── Summary: real figures, honest about background work ─────────────
  if (phase === "summary" && result) {
    const breakdownEntries = Object.entries(result.breakdown).filter(
      ([, count]) => count > 0,
    );
    // Live daemon counts once the first status poll lands; the chunk
    // responses before that. Either way these are measured rows, and the
    // note below says which phases are still moving them.
    const imported = batchStatus?.memories_imported ?? result.imported;
    // memories_skipped is not recoverable from the database after the fact
    // and the daemon always reports it as 0 (db.rs), so the chunk responses
    // are the only real count -- read straight from them, not the batch poll.
    const skipped = result.skipped;
    // Storing is the phase the handoff sentence speaks for: null until it
    // completes, because "stored and searchable now" is a claim this surface
    // must not make while rows are still being written.
    const stored = (batchStatus?.phases ?? []).find((p) => p.phase === "store");
    const storedCount = stored?.state === "complete" ? stored.done : null;
    const importFailed = (batchStatus?.phases ?? []).some((p) => p.state === "failed");
    // Stored and searchable, so the next step is turning it into pages: the
    // shared card (On demand) or the model doing it (Automatic).
    const handOn = organize && batchStatus !== null && !importFailed && storedCount !== null && storedCount > 0;

    return (
      <div className="flex flex-col gap-6 max-w-2xl mx-auto py-4">
        {/* Summary card */}
        <div
          className="rounded-xl px-6 py-5"
          style={{
            backgroundColor: "var(--mem-surface)",
            border: "1px solid var(--mem-border)",
          }}
        >
          {/* Main stat */}
          <div className="flex items-center gap-3 mb-4">
            <div
              className="w-10 h-10 rounded-full flex items-center justify-center"
              style={{ backgroundColor: "rgba(99, 102, 241, 0.15)" }}
            >
              <svg
                className="w-5 h-5"
                fill="none"
                stroke="rgb(99, 102, 241)"
                viewBox="0 0 24 24"
                strokeWidth="2"
              >
                <path
                  strokeLinecap="round"
                  strokeLinejoin="round"
                  d="M5 13l4 4L19 7"
                />
              </svg>
            </div>
            <div>
              <p
                style={{
                  fontFamily: "var(--mem-font-heading)",
                  fontSize: "16px",
                  fontWeight: 500,
                  color: "var(--mem-text)",
                }}
              >
                {t("importBatch.summaryImported", { count: imported, source: sourceLabel(source) })}
              </p>
              {skipped > 0 && (
                <p
                  style={{
                    fontFamily: "var(--mem-font-body)",
                    fontSize: "12px",
                    color: "var(--mem-text-tertiary)",
                    marginTop: "2px",
                  }}
                >
                  {t("importBatch.summarySkipped", { count: skipped })}
                </p>
              )}
            </div>
          </div>

          {/* Type breakdown badges */}
          {breakdownEntries.length > 0 && (
            <div className="flex flex-wrap gap-2 mb-4">
              {breakdownEntries.map(([type, count]) => {
                const style = TYPE_BADGE_STYLES[type] ?? {
                  bg: "var(--mem-hover)",
                  text: "var(--mem-text-secondary)",
                };
                return (
                  <span
                    key={type}
                    className="inline-flex items-center gap-1 px-2 py-1 rounded-full text-xs font-medium"
                    style={{
                      backgroundColor: style.bg,
                      color: style.text,
                      fontFamily: "var(--mem-font-body)",
                    }}
                  >
                    {typeLabels[type] ?? type}
                    <span style={{ opacity: 0.7 }}>{count}</span>
                  </span>
                );
              })}
            </div>
          )}

          {/* The handoff. Entity, link and page figures used to sit here, and
              they are the output of work this surface no longer reports: the
              user reads them as an unfinished bill for an import they were
              told was done. Stored and searchable is the promise the import
              actually keeps, and the toolbar Activity button carries the rest. */}
          {handOn ? (
            // The heading above already gives the count.
            <ImportedLead
              count={storedCount}
              kind="note"
              className="turn-pages-lead"
              short
            />
          ) : batchStatus && (
            <p
              style={{
                fontFamily: "var(--mem-font-body)",
                fontSize: "12px",
                color: "var(--mem-text-secondary)",
                lineHeight: "1.5",
                margin: 0,
              }}
            >
              {importFailed
                ? t("importBatch.pillFailed")
                : storedCount === null
                  ? t("importBatch.backgroundRunning")
                  : t(onDemand ? "activityStatus.importHandoffOnDemand" : "activityStatus.importHandoff", { count: storedCount })}
            </p>
          )}
        </div>

        {handOn ? <ImportOrganize count={storedCount} kind="note" lead="none" onActionChange={setCardAction} /> : null}

        {/* Actions */}
        <div className={`flex items-center gap-3 ${wizardMode ? "justify-center" : ""}`}>
          <button
            onClick={() => onComplete(source, result!)}
            data-emphasis={handOn && cardAction ? "secondary" : "primary"}
            className={`${handOn && cardAction ? "px-4" : "px-6"} py-2 rounded-lg text-sm font-medium transition-colors duration-150`}
            style={handOn && cardAction ? {
              // The card's button is the next step; this one steps back.
              fontFamily: "var(--mem-font-body)",
              backgroundColor: "var(--mem-hover)",
              color: "var(--mem-text-secondary)",
              border: "1px solid var(--mem-border)",
            } : {
              fontFamily: "var(--mem-font-body)",
              backgroundColor: "var(--mem-accent-indigo)",
              color: "white",
            }}
          >
            {completeLabel ?? (wizardMode ? t("importView.continue") : t("importView.viewMemories"))}
          </button>
          {!wizardMode && (
            <button
              onClick={handleReset}
              className="px-4 py-2 rounded-lg text-sm font-medium transition-colors duration-150"
              style={{
                fontFamily: "var(--mem-font-body)",
                backgroundColor: "var(--mem-hover)",
                color: "var(--mem-text-secondary)",
                border: "1px solid var(--mem-border)",
              }}
            >
              {t("importView.importMore")}
            </button>
          )}
        </div>
      </div>
    );
  }

  return null;
}

export default ImportView;
