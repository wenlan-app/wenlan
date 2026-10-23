// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { useTranslation } from "react-i18next";

import {
  getDaemonVersionStatus,
  restartDaemon,
  type DaemonVersionEvent,
  type DaemonVersionStatus,
} from "../lib/tauri";
import { restartErrorKey } from "../lib/daemonRestartErrors";
import { useDaemonStartupPhase } from "../lib/daemonStartup";

type VersionState = DaemonVersionStatus | DaemonVersionEvent;

/**
 * Top-of-window banner for a stale background service. The app self-heals a
 * daemon/app version mismatch on launch and reports the outcome as
 * `daemon://version`; this banner renders only when `matched === false`,
 * i.e. the heal did not land (or the run is isolated and never attempted
 * one). "Restart service" re-runs the same branch logic through the
 * `restart_daemon` command; dismissal lasts for the session only.
 *
 * `program` arrives only when the LaunchAgent runs a daemon outside the app
 * bundle (backend filters it), so the extra sentence renders whenever
 * `program` is present.
 */
export default function DaemonVersionBanner() {
  const { t } = useTranslation();
  const [status, setStatus] = useState<VersionState | null>(null);
  const [dismissed, setDismissed] = useState(false);
  const [restarting, setRestarting] = useState(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const startupPhase = useDaemonStartupPhase();

  useEffect(() => {
    let cancelled = false;
    void getDaemonVersionStatus()
      .then((report) => {
        if (!cancelled && !report.matched) setStatus(report);
      })
      .catch(() => {});
    const unlisten = listen<DaemonVersionEvent>("daemon://version", (event) => {
      if (cancelled) return;
      if (event.payload.matched) {
        setStatus(null);
        setDismissed(false);
        setErrorMsg(null);
      } else {
        setStatus(event.payload);
      }
    });
    return () => {
      cancelled = true;
      unlisten.then((fn) => fn());
    };
  }, []);

  const handleRestart = async () => {
    setRestarting(true);
    setErrorMsg(null);
    try {
      await restartDaemon();
      setStatus(null);
      setDismissed(false);
    } catch (error) {
      const raw = error instanceof Error ? error.message : String(error);
      const key = restartErrorKey(raw);
      setErrorMsg(key ? t(key) : raw);
    } finally {
      setRestarting(false);
    }
  };

  // While the window waits for a daemon that has not answered, the sidebar's
  // startup status is the one place that speaks about the service; a version
  // report from a restart attempt made there must not open a second voice.
  if (startupPhase !== "ready") return null;
  if (dismissed || status === null || status.matched) return null;

  const accent = "var(--mem-accent-amber, #b7791f)";

  return (
    <div
      data-testid="daemon-version-banner"
      role="alert"
      style={{
        position: "fixed",
        top: 12,
        left: "50%",
        transform: "translateX(-50%)",
        zIndex: 1000,
        fontFamily: "var(--mem-font-body)",
        color: "var(--mem-text)",
        backgroundColor: "var(--mem-surface)",
        border: "1px solid var(--mem-border)",
        borderLeft: `3px solid ${accent}`,
        borderRadius: 10,
        padding: "12px 16px",
        boxShadow: "var(--mem-shadow-toast)",
        maxWidth: 560,
        width: "min(560px, calc(100vw - 48px))",
      }}
    >
      <div
        style={{
          fontFamily: "var(--mem-font-heading)",
          fontSize: "14px",
          fontWeight: 600,
          lineHeight: 1.35,
          letterSpacing: "-0.005em",
        }}
      >
        {t("daemonService.title")}
      </div>

      <div
        style={{
          marginTop: 4,
          fontSize: "12.5px",
          lineHeight: 1.5,
          color: "var(--mem-text-secondary)",
        }}
      >
        {t("daemonService.body", { daemon: status.daemon, app: status.app })}
      </div>

      {status.program ? (
        <div
          data-testid="daemon-version-program"
          style={{
            marginTop: 4,
            fontSize: "12.5px",
            lineHeight: 1.5,
            color: "var(--mem-text-secondary)",
          }}
        >
          {t("daemonService.outsideBundle", { program: status.program })}
        </div>
      ) : null}

      {errorMsg ? (
        <div
          data-testid="daemon-version-error"
          style={{
            marginTop: 6,
            fontSize: "12.5px",
            lineHeight: 1.5,
            color: accent,
          }}
        >
          {errorMsg}
        </div>
      ) : null}

      <div style={{ marginTop: 10, display: "flex", gap: 8 }}>
        <button
          type="button"
          data-testid="daemon-version-restart"
          disabled={restarting}
          onClick={() => void handleRestart()}
          style={{
            fontSize: "12.5px",
            fontWeight: 600,
            padding: "6px 14px",
            borderRadius: 8,
            border: "1px solid var(--mem-border)",
            backgroundColor: "var(--mem-accent-indigo)",
            color: "#fff",
            cursor: restarting ? "wait" : "pointer",
            opacity: restarting ? 0.7 : 1,
          }}
        >
          {t("daemonService.restart")}
        </button>
        <button
          type="button"
          data-testid="daemon-version-dismiss"
          aria-label={t("daemonService.dismiss")}
          onClick={() => setDismissed(true)}
          style={{
            fontSize: "12.5px",
            padding: "6px 14px",
            borderRadius: 8,
            border: "1px solid var(--mem-border)",
            backgroundColor: "transparent",
            color: "var(--mem-text-secondary)",
            cursor: "pointer",
          }}
        >
          {t("daemonService.dismiss")}
        </button>
      </div>
    </div>
  );
}
