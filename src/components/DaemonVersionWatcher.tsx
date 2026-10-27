// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";

import { getDaemonVersionStatus, restartDaemon, type DaemonVersionEvent } from "../lib/tauri";
import { useDaemonStartupPhase } from "../lib/daemonStartup";
import { setVersionReport } from "../lib/serviceNotices";

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
          setVersionReport(current);
          return;
        }
        setVersionReport({
          ...current,
          restarted: report.restarted,
          error: report.error,
        });
      } catch {
        // Preserve the failed heal report if a fresh health read is unavailable.
        if (alive.current && !cancelled && eventRevision.current === revision) setVersionReport(report);
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
      if (event.payload.matched || !event.payload.daemon) {
        setVersionReport(event.payload);
        return;
      }
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
          setVersionReport(report);
          return;
        }

        // The normal startup path already attempts this heal in Rust. This
        // read can race that attempt or run after the app became ready from a
        // delayed daemon response, so use the same serialized safe command.
        if (automaticAttempted.current) {
          if (recoveryInFlight.current) return;
          setVersionReport(report);
          return;
        }
        automaticAttempted.current = true;
        recoveryInFlight.current = true;
        setVersionReport(null);
        const recoveryRevision = eventRevision.current;
        void restartDaemon()
          .then(() => {
            if (alive.current && eventRevision.current === recoveryRevision) setVersionReport(null);
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
