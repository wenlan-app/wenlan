// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useId, useRef, useState } from "react";
import ActivitySummaryPopover from "./ActivitySummaryPopover";
import { Pulse, WarningCircle } from "@phosphor-icons/react";
import { useTranslation } from "react-i18next";
import { useQuery } from "@tanstack/react-query";
import { useActivity } from "../../../lib/useActivity";
import { listRegisteredSources, type RegisteredSource } from "../../../lib/tauri";

/** A folder Wenlan follows that it cannot read: moved, its drive gone, or a
 *  read that failed. Paused is the person's own choice and is not broken. */
export function brokenSources(sources: readonly RegisteredSource[] | undefined): RegisteredSource[] {
  return (sources ?? []).filter((source) =>
    typeof source.status === "object" && ("Unavailable" in source.status || "Error" in source.status));
}

interface ActivityStatusProps {
  readonly onOpenActivity: () => void;
  readonly current?: boolean;
  readonly compact?: boolean;
  /** Opens a source that cannot be reached, or the whole source list for
   *  null. Without it, Activity does not check sources. */
  readonly onOpenSource?: (source: RegisteredSource | null) => void;
}

export default function ActivityStatus({
  onOpenActivity,
  current = false,
  compact = false,
  onOpenSource,
}: ActivityStatusProps) {
  const { t } = useTranslation();
  const query = useActivity();
  const { data: activity, isError } = query;
  // A source that cannot be reached needs the person, so it is said here, in
  // Activity's place, and never on Home (UX-HOME-4).
  const sourcesQuery = useQuery({
    queryKey: ["registeredSources"],
    queryFn: () => listRegisteredSources(),
    enabled: onOpenSource !== undefined,
    refetchInterval: 60_000,
    retry: false,
  });
  const broken = onOpenSource ? brokenSources(sourcesQuery.data) : [];
  const [expanded, setExpanded] = useState(false);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const firstActionRef = useRef<HTMLButtonElement>(null);
  const anchorRef = useRef<HTMLDivElement>(null);
  const summaryId = useId();
  useEffect(() => {
    if (!expanded) return;
    const closeOutside = (event: Event) => {
      if (!anchorRef.current?.contains(event.target as Node)) setExpanded(false);
    };
    document.addEventListener("pointerdown", closeOutside);
    document.addEventListener("focusin", closeOutside);
    return () => {
      document.removeEventListener("pointerdown", closeOutside);
      document.removeEventListener("focusin", closeOutside);
    };
  }, [expanded]);
  const measuredActivity = isError ? undefined : activity;
  const failed = measuredActivity?.assets.some((asset) =>
    asset.steps.some((step) => step.failed > 0),
  ) ?? false;
  // On demand (`off`) runs nothing in the background, so pending steps there
  // wait for the user's AI tool and must not read as Steeping.
  const running = measuredActivity !== undefined && measuredActivity.state !== "off" && measuredActivity.assets.some((asset) =>
    asset.steps.some((step) => step.state === "running"),
  );
  const stateWord = failed
    ? t("activityStatus.state.failed")
    : running
      ? t("activityStatus.state.organizing")
      : undefined;
  const brokenLabel = broken.length > 0
    ? t("activityStatus.sourcesBroken", { count: broken.length })
    : undefined;
  const attention = failed || brokenLabel !== undefined;
  const label = brokenLabel ?? (stateWord === undefined
    ? t("main.activity")
    : t("activityStatus.buttonLabel", { state: stateWord }));

  return (
    <div
      ref={anchorRef}
      className={`mem-activity-status-anchor${compact ? " mem-activity-status-anchor--compact" : ""}`}
      data-sidebar-escape-scope="true"
      onKeyDown={(event) => {
        if (
          event.key === "Tab" &&
          !event.shiftKey &&
          !event.altKey &&
          !event.ctrlKey &&
          !event.metaKey &&
          expanded &&
          document.activeElement === triggerRef.current
        ) {
          const firstAction = firstActionRef.current;
          if (firstAction?.isConnected && !firstAction.disabled && anchorRef.current?.contains(firstAction)) {
            event.preventDefault();
            firstAction.focus();
            return;
          }
        }
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
        data-testid="activity-status"
        data-state={failed ? "failed" : brokenLabel ? "source" : running ? "organizing" : undefined}
        aria-current={current ? "page" : undefined}
        aria-label={label}
        title={label}
        aria-haspopup="dialog"
        aria-expanded={expanded}
        aria-controls={expanded ? summaryId : undefined}
        onClick={(event) => {
          // Safari does not focus buttons on pointer activation. Keep Escape local.
          event.currentTarget.focus();
          setExpanded((open) => !open);
        }}
        className={`mem-activity-status${compact ? " mem-activity-status--compact" : ""}`}
      >
        {attention ? (
          <WarningCircle
            aria-hidden="true"
            className="mem-activity-status-icon"
            data-testid="activity-status-icon"
            data-icon-kind="attention"
            style={{ color: "var(--mem-accent-amber)" }}
            size={22}
            weight="regular"
          />
        ) : (
          <Pulse
            aria-hidden="true"
            className={`mem-activity-status-icon${running ? " mem-activity-pulse-running" : ""}`}
            data-testid="activity-status-icon"
            data-icon-state={running ? "organizing" : undefined}
            data-icon-kind="pulse"
            size={22}
            weight="regular"
          />
        )}
        {!compact && <span aria-hidden="true" className="mem-activity-status-label">{brokenLabel ?? t("main.activity")}</span>}
      </button>
      {expanded && (
        <ActivitySummaryPopover
          id={summaryId}
          activity={measuredActivity}
          isError={isError}
          firstActionRef={firstActionRef}
          brokenSources={broken}
          onOpenSource={(source) => {
            setExpanded(false);
            onOpenSource?.(source);
          }}
          onReadAgain={() => void query.refetch()}
          onOpenActivity={() => {
            setExpanded(false);
            triggerRef.current?.focus();
            onOpenActivity();
          }}
        />
      )}
    </div>
  );
}
