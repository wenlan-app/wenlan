// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useRef } from "react";
import { listen } from "@tauri-apps/api/event";

import { getDaemonVersionStatus, type DaemonVersionEvent } from "../lib/tauri";
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

  useEffect(() => {
    let cancelled = false;
    void getDaemonVersionStatus()
      .then((report) => {
        if (!cancelled) setVersionReport(report);
      })
      .catch(() => {});
    const unlisten = listen<DaemonVersionEvent>("daemon://version", (event) => {
      if (!cancelled) setVersionReport(event.payload);
    });
    return () => {
      cancelled = true;
      unlisten.then((fn) => fn());
    };
  }, []);

  // A report from while the daemon was not answering is stale once it
  // answers. Ask again when the workspace becomes ready after waiting.
  const waitedForDaemon = useRef(false);
  useEffect(() => {
    if (startupPhase !== "ready") {
      waitedForDaemon.current = true;
      return;
    }
    if (!waitedForDaemon.current) return;
    waitedForDaemon.current = false;
    let cancelled = false;
    void getDaemonVersionStatus()
      .then((report) => {
        if (!cancelled) setVersionReport(report);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [startupPhase]);

  return null;
}
