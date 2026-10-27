// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";

import {
  getDaemonVersionStatus,
  restartDaemon,
  type DaemonVersionEvent,
  type DaemonVersionStatus,
} from "../lib/tauri";
import { getDaemonStartupPhase, useDaemonStartupPhase } from "../lib/daemonStartup";
import { setVersionReport } from "../lib/serviceNotices";

type VersionReport = DaemonVersionEvent | DaemonVersionStatus;

function isTerminalFailure(report: VersionReport | null): boolean {
  return Boolean(
    report &&
    !report.matched &&
    (("restarted" in report && report.restarted === true) || ("error" in report && report.error)),
  );
}

/**
 * Keeps the daemon/app version report current for the sidebar's service row.
 *
 * The app self-heals a mismatch on launch and reports each outcome as
 * `daemon://version`; the row says "out of date" only while the last report
 * is a real mismatch (`src/lib/serviceNotices.ts`). Renders nothing.
 */
export default function DaemonVersionWatcher() {
  const startupPhase = useDaemonStartupPhase();
  const [listenerReady, setListenerReady] = useState(false);
  const eventRevision = useRef(0);
  const automaticAttempted = useRef(false);
  const recoveryInFlight = useRef(false);
  const deferredMismatchEvent = useRef<DaemonVersionEvent | null>(null);
  const terminalFailure = useRef<VersionReport | null>(null);
  const alive = useRef(false);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | null = null;
    alive.current = true;
    const confirmMismatch = async (report: DaemonVersionEvent, revision: number) => {
      try {
        const current = await getDaemonVersionStatus();
        if (!alive.current || cancelled || eventRevision.current !== revision) return;
        if (current.matched) {
          terminalFailure.current = null;
          setVersionReport(current);
          return;
        }
        const confirmed = {
          ...current,
          restarted: report.restarted,
          error: report.error,
        };
        if (isTerminalFailure(confirmed)) {
          automaticAttempted.current = true;
          terminalFailure.current = confirmed;
        }
        setVersionReport(confirmed);
      } catch {
        // Preserve the failed heal report if a fresh health read is unavailable.
        if (alive.current && !cancelled && eventRevision.current === revision) {
          if (isTerminalFailure(report)) {
            automaticAttempted.current = true;
            terminalFailure.current = report;
          }
          setVersionReport(report);
        }
      }
    };

    void listen<DaemonVersionEvent>("daemon://version", (event) => {
      if (cancelled) return;
      if (!event.payload.matched && recoveryInFlight.current) {
        // Startup's own heal may finish while the frontend's serialized retry
        // is waiting. Keep its freshest report for a possible retry failure,
        // but leave the service row quiet until that retry settles.
        deferredMismatchEvent.current = event.payload;
        return;
      }
      eventRevision.current += 1;
      deferredMismatchEvent.current = null;
      if (event.payload.matched) {
        terminalFailure.current = null;
        setVersionReport(event.payload);
        return;
      }
      if (isTerminalFailure(event.payload)) {
        automaticAttempted.current = true;
        terminalFailure.current = event.payload;
      }
      if (!event.payload.daemon) {
        setVersionReport(event.payload);
        return;
      }
      if (getDaemonStartupPhase() === "starting" && !isTerminalFailure(event.payload)) return;
      // Startup's asynchronous emit can arrive after a newer restart command
      // already reported success. Confirm mismatches against live health so a
      // delayed failure cannot reopen a warning for an already-current daemon.
      void confirmMismatch(event.payload, eventRevision.current);
    }).then((stop) => {
      if (cancelled) {
        stop();
        return;
      }
      unlisten = stop;
      setListenerReady(true);
    }).catch(() => {
      // A listener failure must not disable the read-only startup check. The
      // revision guard still protects in-flight reads where an event arrives.
      if (!cancelled) setListenerReady(true);
    });
    return () => {
      cancelled = true;
      alive.current = false;
      eventRevision.current += 1;
      unlisten?.();
    };
  }, []);

  // Listener setup can miss a startup result emitted just before registration.
  // While the gate is waiting, do one read-only check: show only a cached
  // terminal recovery failure or a confirmed match, never a bare mismatch and
  // never start another recovery attempt from this phase.
  useEffect(() => {
    if (!listenerReady || startupPhase !== "starting" || recoveryInFlight.current) return;
    let cancelled = false;
    const revision = eventRevision.current;
    void getDaemonVersionStatus()
      .then((report) => {
        if (cancelled || !alive.current || eventRevision.current !== revision) return;
        if (report.matched) {
          terminalFailure.current = null;
          setVersionReport(report);
          return;
        }
        // The read may have started before a retry began. Do not publish its
        // stale mismatch while the newer recovery still owns the outcome.
        if (recoveryInFlight.current) return;
        if (!isTerminalFailure(report)) return;
        automaticAttempted.current = true;
        terminalFailure.current = report;
        setVersionReport(report);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [listenerReady, startupPhase]);

  // Query only after the app's startup gate is ready. A version read while a
  // restart is still in progress can report the old daemon and recreate the
  // warning after the matching startup event has cleared it.
  useEffect(() => {
    if (!listenerReady || startupPhase !== "ready") return;
    let cancelled = false;
    const revision = eventRevision.current;
    void getDaemonVersionStatus()
      .then((report) => {
        if (cancelled || eventRevision.current !== revision) return;
        if (report.matched) {
          terminalFailure.current = null;
          setVersionReport(report);
          return;
        }
        // A cached failure can predate a newer frontend retry. The retry's
        // result is authoritative until it settles.
        if (recoveryInFlight.current) return;

        // Rust may have cached a completed startup failure before this first
        // ready-phase read. Treat that as the finished automatic attempt, not
        // as a fresh mismatch that deserves another automatic restart.
        if (isTerminalFailure(report)) {
          automaticAttempted.current = true;
          terminalFailure.current = report;
          setVersionReport(report);
          return;
        }

        // The normal startup path already attempts this heal in Rust. This
        // read can race that attempt or run after the app became ready from a
        // delayed daemon response, so use the same serialized safe command.
        if (automaticAttempted.current) {
          if (isTerminalFailure(report)) terminalFailure.current = report;
          const knownFailure = terminalFailure.current;
          if (knownFailure && !isTerminalFailure(report)) {
            setVersionReport({
              ...report,
              restarted: "restarted" in knownFailure ? knownFailure.restarted : true,
              error: "error" in knownFailure ? knownFailure.error : null,
            });
            return;
          }
          setVersionReport(report);
          return;
        }
        automaticAttempted.current = true;
        recoveryInFlight.current = true;
        setVersionReport(null);
        const recoveryRevision = eventRevision.current;
        void restartDaemon()
          .then(() => {
            if (alive.current && eventRevision.current === recoveryRevision) {
              terminalFailure.current = null;
              setVersionReport(null);
            }
          })
          .catch((error: unknown) => {
            if (!alive.current || eventRevision.current !== recoveryRevision) return;
            const message = deferredMismatchEvent.current?.error
              ?? (error instanceof Error ? error.message : String(error));
            const latestReport = deferredMismatchEvent.current ?? report;
            const failedReport: DaemonVersionEvent = {
              ...latestReport,
              restarted: true,
              error: message,
            };
            terminalFailure.current = failedReport;
            setVersionReport(failedReport);
          })
          .finally(() => {
            recoveryInFlight.current = false;
            deferredMismatchEvent.current = null;
          });
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [listenerReady, startupPhase]);

  return null;
}
