// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { useQueryClient, type QueryClient } from "@tanstack/react-query";
import type { TFunction } from "i18next";
import { Pulse, WarningCircle } from "@phosphor-icons/react";
import { useDaemonStartupPhase } from "../../lib/daemonStartup";
import {
  serviceStatusKind,
  setServiceFallback,
  setVersionReport,
  useServiceNotices,
  type ServiceNotices,
  type ServiceStatusKind,
} from "../../lib/serviceNotices";
import { getWireState, restartDaemon, startDaemonSidecar } from "../../lib/tauri";
import { restartErrorKey } from "../../lib/daemonRestartErrors";

/** The service speaks in Activity's place in the sidebar footer, so the
 *  footer never grows a row of its own for it. Activity shows one thing at a
 *  time, the one that matters most: a service problem, then the service
 *  starting, then Activity itself (organizing, or quiet). An app update lives
 *  on the account entry instead (src/components/memory/IdentityCard.tsx), and
 *  one-off messages go to the toast stack, so nothing else is laid over the
 *  workspace.
 *
 *  Starting is Activity's own row with a pulse and 「正在背景啟動…」. Something
 *  that needs the user (not answering, out of date, not running) turns the
 *  same slot amber, like Activity's failed state, and opens once by itself
 *  with what is wrong and the one action that fixes it. When the state goes
 *  away Activity comes back. Which one shows is decided in
 *  src/lib/serviceNotices.ts. */
export default function ServiceStatus({
  compact = false,
  children,
}: {
  readonly compact?: boolean;
  /** Activity, shown whenever the service has nothing to say. */
  readonly children?: ReactNode;
}) {
  const phase = useDaemonStartupPhase();
  const notices = useServiceNotices();
  const kind = serviceStatusKind(phase, notices);
  if (kind === null) return <>{children}</>;
  if (kind === "starting") return <StartingRow compact={compact} />;
  // Keyed by kind: a new problem opens its explanation once and starts
  // without the last one's error.
  return <ServiceWarning key={kind} kind={kind} notices={notices} compact={compact} />;
}

function StartingRow({ compact }: { readonly compact: boolean }) {
  const { t } = useTranslation();
  const label = t("boot.startingInBackground");
  return (
    <div
      className={`mem-activity-status-anchor${compact ? " mem-activity-status-anchor--compact" : ""}`}
    >
      <div
        role="status"
        aria-label={label}
        title={label}
        data-testid="service-status-starting"
        className={`mem-activity-status mem-activity-status--still${compact ? " mem-activity-status--compact" : ""}`}
      >
        <Pulse
          aria-hidden="true"
          className="mem-activity-status-icon mem-activity-pulse-running"
          data-icon-state="organizing"
          size={22}
          weight="regular"
        />
        {!compact && <span aria-hidden="true" className="mem-activity-status-label">{label}</span>}
      </div>
    </div>
  );
}

type WarningKind = Exclude<ServiceStatusKind, "starting">;

interface WarningCopy {
  readonly label: string;
  readonly body: readonly string[];
  readonly action: string;
  readonly busy: string;
}

function warningCopy(kind: WarningKind, { version }: ServiceNotices, t: TFunction): WarningCopy {
  switch (kind) {
    case "notAnswering":
      return {
        label: t("boot.notAnsweringLabel"),
        body: [t("boot.notAnsweringBody")],
        action: t("daemonService.restart"),
        busy: t("boot.starting"),
      };
    case "outOfDate":
      return {
        label: t("daemonService.label"),
        body: [
          t("daemonService.body", { daemon: version?.daemon, app: version?.app }),
          ...(version?.program ? [t("daemonService.outsideBundle", { program: version.program })] : []),
        ],
        action: t("daemonService.restart"),
        busy: t("boot.starting"),
      };
    case "notRunning":
      return {
        label: t("boot.fallbackLabel"),
        body: [t("boot.fallbackBody")],
        action: t("boot.startService"),
        busy: t("boot.starting"),
      };
  }
}

/** Runs the warning's one action. Resolves with the text to show when it did
 *  not fix things, or null when it did. */
async function runAction(
  kind: WarningKind,
  queryClient: QueryClient,
  t: TFunction,
): Promise<string | null> {
  if (kind === "notRunning") {
    const result = await startDaemonSidecar();
    if (result.status === "failed") return `${t("boot.startFailed")} ${result.message}`;
    // A spawn is not a service: only a probe that comes back clears this.
    const wire = await getWireState();
    if (!wire.daemon.reachable) return wire.daemon.error ?? t("boot.startNotReachable");
    setServiceFallback(false);
    return null;
  }
  await restartDaemon();
  if (kind === "notAnswering" || kind === "outOfDate") {
    // Re-read the gate after a successful retry so a manual version repair
    // releases any daemon calls that are still waiting for startup.
    void queryClient.invalidateQueries({ queryKey: ["shouldShowWizard"] });
  }
  if (kind !== "notAnswering") {
    setVersionReport(null);
  }
  return null;
}

function ServiceWarning({
  kind,
  notices,
  compact,
}: {
  readonly kind: WarningKind;
  readonly notices: ServiceNotices;
  readonly compact: boolean;
}) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  // Opens by itself once, since this needs the user. After that it opens
  // only on request; the row itself stays until the problem is gone.
  const [expanded, setExpanded] = useState(true);
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const busyRef = useRef(false);
  const anchorRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const popoverId = useId();
  const copy = warningCopy(kind, notices, t);
  const terminalVersionFailure = kind === "outOfDate" && notices.version && !notices.version.matched && (
    ("restarted" in notices.version && notices.version.restarted === true) ||
    ("error" in notices.version && Boolean(notices.version.error))
  );
  const reportError = terminalVersionFailure && "error" in notices.version
    ? notices.version.error
    : null;
  const reportErrorKey = reportError ? restartErrorKey(reportError) : null;
  const visibleFailure = failure ?? (terminalVersionFailure
    ? reportError
      ? reportErrorKey ? t(reportErrorKey) : t("boot.startFailed")
      : t("daemonService.errors.stillMismatched")
    : null);

  useEffect(() => {
    if (!expanded) return;
    const closeOutside = (event: Event) => {
      if (!anchorRef.current?.contains(event.target as Node)) setExpanded(false);
    };
    document.addEventListener("pointerdown", closeOutside);
    return () => document.removeEventListener("pointerdown", closeOutside);
  }, [expanded]);

  async function handleAction() {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    setFailure(null);
    try {
      setFailure(await runAction(kind, queryClient, t));
    } catch (error) {
      const raw = error instanceof Error ? error.message : String(error);
      const key = restartErrorKey(raw);
      setFailure(key ? t(key) : kind === "outOfDate" ? t("boot.startFailed") : raw);
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  }

  return (
    <div
      ref={anchorRef}
      className={`mem-activity-status-anchor${compact ? " mem-activity-status-anchor--compact" : ""}`}
      data-sidebar-escape-scope="true"
      onKeyDown={(event) => {
        if (event.key !== "Escape" || !expanded) return;
        event.preventDefault();
        event.stopPropagation();
        setExpanded(false);
        triggerRef.current?.focus();
      }}
    >
      <button
        ref={triggerRef}
        type="button"
        data-testid="service-status"
        data-kind={kind}
        aria-label={copy.label}
        title={copy.label}
        aria-haspopup="dialog"
        aria-expanded={expanded}
        aria-controls={expanded ? popoverId : undefined}
        onClick={(event) => {
          event.currentTarget.focus();
          setExpanded((open) => !open);
        }}
        className={`mem-activity-status${compact ? " mem-activity-status--compact" : ""}`}
      >
        <WarningCircle
          aria-hidden="true"
          className="mem-activity-status-icon"
          style={{ color: "var(--mem-accent-amber)" }}
          size={22}
          weight="regular"
        />
        {!compact && <span aria-hidden="true" className="mem-activity-status-label">{copy.label}</span>}
      </button>
      {expanded && (
        <div
          id={popoverId}
          className="mem-popover-surface mem-activity-popover"
          data-testid="service-status-popover"
          role="dialog"
          aria-label={copy.label}
        >
          <h2 className="mem-activity-summary-title">{copy.label}</h2>
          {copy.body.map((line) => (
            <p key={line} className="mem-daemon-startup-body">{line}</p>
          ))}
          {visibleFailure && <p role="alert" className="mem-daemon-startup-failure">{visibleFailure}</p>}
          <div>
            <button
              type="button"
              className="mem-activity-popover-action"
              data-testid="service-status-action"
              disabled={busy}
              onClick={() => { void handleAction(); }}
            >
              {busy ? copy.busy : copy.action}
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
