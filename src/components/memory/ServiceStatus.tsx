// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useQueryClient, type QueryClient } from "@tanstack/react-query";
import type { TFunction } from "i18next";
import { ArrowCircleUp, WarningCircle } from "@phosphor-icons/react";
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
import { installUpdate, postponeUpdate, retryUpdate } from "../../lib/appUpdate";

/** Every lasting notice, in one place: the top of the sidebar footer, above
 *  「AI 能做什麼」 and Activity. The service says here what it is doing and
 *  what is wrong with it, and the updater says a new version is ready.
 *  One-off messages go to the toast stack at the bottom centre instead.
 *
 *  Something in progress (starting, updating) is one muted line. Something
 *  that needs the user (not answering, out of date, not running, update
 *  failed) turns the same row into a warning styled like Activity's failed
 *  state, which opens once by itself with what is wrong and the one action
 *  that fixes it. A new version is the same row in a calm colour that opens
 *  only when clicked. When the state goes away the row goes quiet again.
 *  Nothing is laid over the workspace. Which one shows is decided in
 *  src/lib/serviceNotices.ts. */
export default function ServiceStatus({ compact = false }: { readonly compact?: boolean }) {
  const phase = useDaemonStartupPhase();
  const notices = useServiceNotices();
  const kind = serviceStatusKind(phase, notices);
  if (kind === null) return null;
  if (kind === "starting" || kind === "updating") {
    return <ProgressLine kind={kind} pct={notices.update?.pct ?? null} compact={compact} />;
  }
  // Keyed by kind: a new problem opens its explanation once and starts
  // without the last one's error.
  return <ServiceWarning key={kind} kind={kind} notices={notices} compact={compact} />;
}

function ProgressLine({
  kind,
  pct,
  compact,
}: {
  readonly kind: "starting" | "updating";
  readonly pct: number | null;
  readonly compact: boolean;
}) {
  const { t } = useTranslation();
  const label =
    kind === "starting"
      ? t("boot.startingInBackground")
      : pct === null
        ? t("updater.updating")
        : t("updater.updatingPct", { pct });
  return (
    <div
      role="status"
      aria-label={compact ? label : undefined}
      title={compact ? label : undefined}
      data-testid={`service-status-${kind}`}
      className={`mem-daemon-startup-status${compact ? " mem-daemon-startup-status--compact" : ""}`}
    >
      <span
        aria-hidden="true"
        className={`mem-daemon-startup-dot${kind === "updating" ? " mem-daemon-startup-dot--calm" : ""}`}
      />
      {!compact && <span className="mem-daemon-startup-label">{label}</span>}
    </div>
  );
}

type WarningKind = Exclude<ServiceStatusKind, "starting" | "updating">;

interface WarningCopy {
  readonly label: string;
  readonly title?: string;
  readonly body: readonly string[];
  readonly action: string;
  /** A second, quieter choice, such as putting an update off. */
  readonly secondary?: string;
  readonly busy: string;
  /** Calm notices wait to be clicked; warnings open once by themselves. */
  readonly calm?: boolean;
}

function warningCopy(kind: WarningKind, { version, update }: ServiceNotices, t: TFunction): WarningCopy {
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
    case "updateFailed":
      return {
        label: t("updater.failed"),
        body: [t("updater.failedBody")],
        action: t("updater.retry"),
        busy: t("updater.updating"),
      };
    case "updateAvailable":
      return {
        label: t("updater.available"),
        title: t("updater.ready", { version: update?.version }),
        body: [t("updater.installBody")],
        action: t("updater.install"),
        secondary: t("updater.later"),
        busy: t("updater.updating"),
        calm: true,
      };
  }
}

/** Runs the warning's one action. Resolves with the text to show when it did
 *  not fix things, or null when it did. */
async function runAction(kind: WarningKind, queryClient: QueryClient, t: TFunction): Promise<string | null> {
  if (kind === "updateAvailable") {
    await installUpdate();
    return null;
  }
  if (kind === "updateFailed") {
    await retryUpdate();
    return null;
  }
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
  if (kind === "notAnswering") {
    // Asking the gate again puts the workspace back to "starting": the row
    // goes back to the quiet line, and returns only if that attempt fails.
    void queryClient.invalidateQueries({ queryKey: ["shouldShowWizard"] });
  } else {
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
  const copy = warningCopy(kind, notices, t);
  // A warning opens by itself once, since it needs the user. After that it
  // opens only on request; the row itself stays until the problem is gone.
  const [expanded, setExpanded] = useState(!copy.calm);
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const busyRef = useRef(false);
  const anchorRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const popoverId = useId();

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
      setFailure(key ? t(key) : raw);
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  }

  async function handleSecondary() {
    setFailure(null);
    try {
      await postponeUpdate();
    } catch (error) {
      setFailure(error instanceof Error ? error.message : String(error));
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
        {copy.calm ? (
          <ArrowCircleUp
            aria-hidden="true"
            className="mem-activity-status-icon"
            style={{ color: "var(--mem-accent-indigo)" }}
            size={22}
            weight="regular"
          />
        ) : (
          <WarningCircle
            aria-hidden="true"
            className="mem-activity-status-icon"
            style={{ color: "var(--mem-accent-amber)" }}
            size={22}
            weight="regular"
          />
        )}
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
          <h2 className="mem-activity-summary-title">{copy.title ?? copy.label}</h2>
          {copy.body.map((line) => (
            <p key={line} className="mem-daemon-startup-body">{line}</p>
          ))}
          {failure && <p role="alert" className="mem-daemon-startup-failure">{failure}</p>}
          <div className="mem-service-status-actions">
            <button
              type="button"
              className="mem-activity-popover-action"
              data-testid="service-status-action"
              disabled={busy}
              onClick={() => { void handleAction(); }}
            >
              {busy ? copy.busy : copy.action}
            </button>
            {copy.secondary && (
              <button
                type="button"
                className="mem-activity-popover-action"
                data-testid="service-status-secondary"
                disabled={busy}
                onClick={() => { void handleSecondary(); }}
              >
                {copy.secondary}
              </button>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
