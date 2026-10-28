import { useState, useEffect, useCallback, useRef } from "react";
import type { TFunction } from "i18next";
import { useTranslation } from "react-i18next";
import { DropZone } from "./DropZone";
import {
  importChatExport,
  saveTempFile,
  listPendingImports,
  type ImportChatExportResponse,
  type PendingImport,
} from "../../lib/tauri";
import { formatImportErrorDetail } from "../memory/importCopy";
import { ImportOrganize, useOrganizeActions } from "../memory/askAi/TurnIntoPages";

/** What the user just triggered in this session. */
type LocalAction =
  | null
  | { kind: "reading" }
  | { kind: "done"; result: ImportChatExportResponse }
  | { kind: "error"; detail: string };

export interface ImportFlowProps {
  /** Reports the short frontend read/upload window to a containing flow. */
  onBusyChange?: (busy: boolean) => void;
  /** Reports the daemon-accepted result without navigating the containing flow. */
  onImportAccepted?: (result: ImportChatExportResponse) => void;
  /** Called when the finish's "turn them into pages" sends the user to
   *  another screen, so a dialog holding this flow can close. */
  onLeave?: () => void;
}

const POLL_INTERVAL_MS = 5_000;

/**
 * Consecutive status-poll rejections before the strip stops claiming the
 * import is still being refined.
 *
 * One rejection is noise: the daemon restarts, a request races a reload, and
 * the next tick answers fine. Three in a row is fifteen seconds of a daemon
 * that will not say anything, and the strip's "refining" spinner has by then
 * become a claim nobody is checking. Losing the status is NOT the same as the
 * import failing, and the copy says so.
 */
const POLL_FAILURE_LIMIT = 3;

// The import finish already shows in the window, so a system notification is
// only for someone who switched away. Never ask for notification permission
// here: a first import is too early for a macOS permission dialog.
async function maybeNotify(title: string, body: string) {
  try {
    if (typeof document !== "undefined" && document.hasFocus()) return;
    const { sendNotification, isPermissionGranted } =
      await import("@tauri-apps/plugin-notification");
    if (await isPermissionGranted()) {
      await sendNotification({ title, body });
    }
  } catch {
    // Plugin unavailable or not initialized
  }
}

export function ImportFlow({ onBusyChange, onImportAccepted, onLeave }: ImportFlowProps = {}) {
  const { t } = useTranslation();
  const organize = useOrganizeActions() !== null;
  const [localAction, setLocalAction] = useState<LocalAction>(null);
  // The daemon has answered at least once since this import was accepted
  // with nothing still pending: the chats are in, and searchable.
  const [settledAfterDone, setSettledAfterDone] = useState(false);
  const localActionRef = useRef<LocalAction>(null);
  localActionRef.current = localAction;
  const doneAtRef = useRef(0);
  const [pending, setPending] = useState<PendingImport | null>(null);
  const [busy, setBusy] = useState(false);
  const [dismissed, setDismissed] = useState(false);
  // The daemon stopped answering about this import. Distinct from a failed
  // import: we do not know that it failed, only that we cannot see it.
  const [pollLost, setPollLost] = useState(false);
  const [pollNonce, setPollNonce] = useState(0);
  const prevPendingRef = useRef<PendingImport | null>(null);
  const busyRef = useRef(false);
  const pollFailuresRef = useRef(0);
  // What Retry re-runs. Held as the operation, not the path, so a dropped
  // file retries through the same read/upload path a first attempt took.
  const lastOperationRef = useRef<(() => Promise<void>) | null>(null);

  useEffect(() => {
    onBusyChange?.(busy);
  }, [busy, onBusyChange]);

  // Poll daemon for actual import state. Survives page switches because the
  // daemon is the source of truth, not React state.
  useEffect(() => {
    let alive = true;
    const poll = () => {
      const askedAt = Date.now();
      // Resolve first so a synchronous mock/runtime failure follows the same
      // visible failure-safe path as an IPC rejection.
      Promise.resolve()
        .then(() => listPendingImports())
        .then((imports) => {
          if (!alive) return;
          const previous = prevPendingRef.current;
          if (imports.length > 0) {
            const next = imports[0];
            if (next.id !== previous?.id) setDismissed(false);
            setPending(next);
          } else {
            // Both done and failed imports leave this list. Only the explicit
            // import response can establish success; enrichment runs separately.
            setPending(null);
            // Only an answer asked for after the import was accepted counts.
            if (localActionRef.current?.kind === "done" && askedAt >= doneAtRef.current) setSettledAfterDone(true);
          }
          prevPendingRef.current = imports[0] ?? null;
          pollFailuresRef.current = 0;
          setPollLost((lost) => (lost ? false : lost));
        })
        .catch(() => {
          if (!alive) return;
          pollFailuresRef.current += 1;
          if (pollFailuresRef.current < POLL_FAILURE_LIMIT) return;
          // Clearing `pending` is the point: `isRefining` is derived from it,
          // so leaving it set is what used to spin "refining" forever behind
          // a daemon that had gone away.
          setPollLost(true);
          setPending(null);
          prevPendingRef.current = null;
        });
    };
    poll();
    const id = setInterval(poll, POLL_INTERVAL_MS);
    return () => { alive = false; clearInterval(id); };
  }, [t, pollNonce]);

  const acceptImport = useCallback(async (path: string) => {
    const result = await importChatExport(path);
    setSettledAfterDone(false);
    doneAtRef.current = Date.now();
    setLocalAction({ kind: "done", result });
    onImportAccepted?.(result);
    const msg = result.conversations_new > 0
      ? t("chatImport.importFlow.notificationImported", {
          conversations: result.conversations_new,
          memories: result.memories_stored,
          vendor: result.vendor,
        })
      : t("chatImport.importFlow.notificationAlreadyImported", {
          count: result.conversations_total,
        });
    maybeNotify("Wenlan", msg);
  }, [onImportAccepted, t]);

  const beginImport = useCallback(async (operation: () => Promise<void>) => {
    if (busyRef.current) return;
    busyRef.current = true;
    lastOperationRef.current = operation;
    // A new import does not inherit the last one's lost poll. Without this
    // the strip opens in the error style, with "lost track of this import"
    // still on it, before the new import has done anything at all.
    pollFailuresRef.current = 0;
    setPollLost(false);
    setBusy(true);
    setSettledAfterDone(false);
    setLocalAction({ kind: "reading" });
    setDismissed(false);
    try {
      await operation();
    } catch (e: unknown) {
      // Same treatment ImportView already gives a failed import: a localized
      // heading, with the backend detail demoted rather than dropped. The raw
      // string embeds `req.path` (import_routes.rs), so it is a file path on
      // screen, not a sentence.
      setLocalAction({ kind: "error", detail: formatImportErrorDetail(e) });
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  }, []);

  const runImport = useCallback((path: string) => {
    return beginImport(() => acceptImport(path));
  }, [acceptImport, beginImport]);

  const handleFileSelected = useCallback((file: File) => {
    return beginImport(async () => {
      const bytes = new Uint8Array(await file.arrayBuffer());
      const tempPath = await saveTempFile(bytes, file.name);
      await acceptImport(tempPath);
    });
  }, [acceptImport, beginImport]);

  const handlePathSelected = useCallback(async (path: string) => {
    await runImport(path);
  }, [runImport]);

  const handleRetry = useCallback(() => {
    if (pollLost) {
      pollFailuresRef.current = 0;
      setPollLost(false);
      setPollNonce((n) => n + 1);
    }
    const operation = lastOperationRef.current;
    if (localAction?.kind === "error" && operation) {
      setLocalAction(null);
      void beginImport(operation);
    }
  }, [beginImport, localAction, pollLost]);

  // Derive display state from local action + daemon state
  const isRefining = pending !== null;
  const pendingFailed = pending?.stage === "error";
  const errored = localAction?.kind === "error" || pollLost;
  const showLocal = localAction !== null && !dismissed;
  const showRefining = isRefining && !dismissed;
  const showPollLost = pollLost && !dismissed;
  // New chats are in and nothing is pending: the finish turns them into
  // pages (or says who does), in place of the "imported" strip.
  const handOnChats = organize && !dismissed && !errored && !isRefining && settledAfterDone
    && localAction?.kind === "done" && localAction.result.conversations_new > 0
    ? localAction.result.conversations_new
    : null;
  const showStrip = (showLocal || showRefining || showPollLost) && handOnChats === null;
  const canRetry =
    (localAction?.kind === "error" && lastOperationRef.current !== null) || pollLost;

  return (
    <div>
      <DropZone
        onFileSelected={handleFileSelected}
        onPathSelected={handlePathSelected}
        disabled={busy}
      />

      {showStrip && (
        <div
          style={{
            marginTop: 10,
            borderRadius: 8,
            padding: "8px 12px",
            fontFamily: "var(--mem-font-body)",
            fontSize: "var(--mem-text-sm)",
            lineHeight: "1.5",
            display: "flex",
            alignItems: "center",
            gap: 8,
            background: errored
              ? "var(--mem-status-danger-bg)"
              : "color-mix(in srgb, var(--mem-accent-indigo) 8%, transparent)",
            border: `1px solid ${
              errored
                ? "var(--mem-status-danger-border)"
                : "color-mix(in srgb, var(--mem-accent-indigo) 16%, transparent)"
            }`,
            transition: "all 0.2s ease",
          }}
        >
          <StatusIcon
            reading={localAction?.kind === "reading"}
            error={errored || pendingFailed}
            refining={isRefining && !pendingFailed && !errored}
          />

          <span style={{
            flex: 1,
            minWidth: 0,
            color: errored ? "var(--mem-status-danger-text)" : "var(--mem-text-secondary)",
          }}>
            {localAction?.kind === "reading" && t("chatImport.importFlow.importing")}
            {localAction?.kind === "done" && formatDoneMessage(t, localAction.result, isRefining, pending)}
            {localAction?.kind === "error" && (
              <>
                <span style={{ display: "block" }}>{t("importView.importFailedTitle")}</span>
                {localAction.detail && (
                  <span
                    data-testid="chat-import-error-detail"
                    style={{
                      display: "block",
                      color: "var(--mem-text-tertiary)",
                      overflowWrap: "anywhere",
                    }}
                  >
                    {localAction.detail}
                  </span>
                )}
              </>
            )}
            {!localAction && pollLost && t("chatImport.importFlow.statusUnavailable")}
            {!localAction && !pollLost && isRefining && formatRefiningMessage(t, pending)}
          </span>

          {canRetry && (
            <button
              onClick={handleRetry}
              data-testid="chat-import-retry"
              style={{
                flexShrink: 0,
                padding: "2px 8px",
                borderRadius: 6,
                border: "1px solid var(--mem-status-danger-border)",
                background: "transparent",
                color: "inherit",
                cursor: "pointer",
                fontFamily: "inherit",
                fontSize: "inherit",
              }}
            >
              {t("chatImport.importFlow.retry")}
            </button>
          )}

          {localAction?.kind !== "reading" && (
            <button
              onClick={() => { setDismissed(true); setLocalAction(null); }}
              style={{
                background: "none",
                border: "none",
                cursor: "pointer",
                color: "var(--mem-text-tertiary)",
                padding: 2,
                lineHeight: 0,
                flexShrink: 0,
              }}
              aria-label={t("chatImport.importFlow.dismiss")}
            >
              <svg width="12" height="12" viewBox="0 0 12 12" fill="none" aria-hidden="true">
                <path d="M3 3L9 9M9 3L3 9" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
              </svg>
            </button>
          )}
        </div>
      )}

      {handOnChats !== null ? (
        <div style={{ marginTop: 12 }}>
          <ImportOrganize count={handOnChats} kind="chat" bare onLeave={onLeave} />
        </div>
      ) : null}
    </div>
  );
}

function StatusIcon({ reading, error, refining }: { reading?: boolean; error?: boolean; refining: boolean }) {
  if (reading || refining) {
    return (
      <span
        className="mem-node-pulse"
        style={{
          width: 6, height: 6, borderRadius: "50%",
          background: "var(--mem-accent-indigo)",
          flexShrink: 0,
        }}
      />
    );
  }
  if (error) {
    return (
      <svg width="14" height="14" viewBox="0 0 14 14" fill="none" style={{ flexShrink: 0 }} aria-hidden="true">
        <circle cx="7" cy="7" r="6" stroke="var(--mem-status-danger-text)" strokeWidth="1.5" />
        <path d="M5 5L9 9M9 5L5 9" stroke="var(--mem-status-danger-text)" strokeWidth="1.5" strokeLinecap="round" />
      </svg>
    );
  }
  return (
    <svg width="14" height="14" viewBox="0 0 14 14" fill="none" style={{ flexShrink: 0 }} aria-hidden="true">
      <circle cx="7" cy="7" r="6" stroke="var(--mem-accent-sage)" strokeWidth="1.5" />
      <path d="M4.5 7L6.5 9L9.5 5" stroke="var(--mem-accent-sage)" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}

function importStageText(t: TFunction, stage: string): string {
  switch (stage) {
    case "parsing":
      return t("chatImport.stage.parsing");
    case "stage_a":
      return t("chatImport.stage.stageA");
    case "stage_b":
      return t("chatImport.stage.stageB");
    case "done":
      return t("chatImport.stage.done");
    case "error":
      return t("chatImport.stage.error");
    default:
      return stage;
  }
}

function formatDoneMessage(
  t: TFunction,
  r: ImportChatExportResponse,
  refining: boolean,
  pending: PendingImport | null,
): string {
  const stageSuffix = refining && pending
    ? t("chatImport.importFlow.stageInBackground", {
        stage: importStageText(t, pending.stage),
      })
    : "";
  if (r.conversations_new > 0) {
    return t("chatImport.importFlow.conversationsImported", {
      count: r.conversations_new,
      vendor: r.vendor,
      stageSuffix,
    });
  }
  return t("chatImport.importFlow.allAlreadyImported", {
    count: r.conversations_total,
    stageSuffix,
  });
}

function formatRefiningMessage(t: TFunction, p: PendingImport | null): string {
  if (!p) return "";
  const count = p.total_conversations ?? 0;
  return t("chatImport.importFlow.refining", {
    count,
    vendor: p.vendor,
    stage: importStageText(t, p.stage),
  });
}
