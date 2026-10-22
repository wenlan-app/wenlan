// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useId, useRef, useState } from "react";
import ActivitySummaryPopover from "./ActivitySummaryPopover";
import { Pulse, WarningCircle } from "@phosphor-icons/react";
import { useTranslation } from "react-i18next";
import { useActivity } from "../../../lib/useActivity";

interface ActivityStatusProps {
  readonly onOpenActivity: () => void;
  readonly current?: boolean;
  readonly compact?: boolean;
}

export default function ActivityStatus({
  onOpenActivity,
  current = false,
  compact = false,
}: ActivityStatusProps) {
  const { t } = useTranslation();
  const query = useActivity();
  const { data: activity, isError } = query;
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
  const label = stateWord === undefined
    ? t("main.activity")
    : t("activityStatus.buttonLabel", { state: stateWord });

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
        data-state={failed ? "failed" : running ? "organizing" : undefined}
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
        {failed ? (
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
        {!compact && <span aria-hidden="true" className="mem-activity-status-label">{t("main.activity")}</span>}
      </button>
      {expanded && (
        <ActivitySummaryPopover
          id={summaryId}
          activity={measuredActivity}
          isError={isError}
          firstActionRef={firstActionRef}
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
