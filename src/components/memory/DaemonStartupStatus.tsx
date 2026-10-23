// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useQueryClient } from "@tanstack/react-query";
import { WarningCircle } from "@phosphor-icons/react";
import { useDaemonStartupPhase } from "../../lib/daemonStartup";
import { restartDaemon } from "../../lib/tauri";
import { restartErrorKey } from "../../lib/daemonRestartErrors";

/** The daemon's startup, said in one place: the top of the sidebar footer.
 *
 *  While the workspace waits for the daemon (src/lib/daemonStartup.ts) this is
 *  one muted line. If the first-run gate gives up, the same row turns into a
 *  warning, styled like Activity's failed state, and opens once to say what
 *  is wrong and offer "Restart service". Nothing is laid over the workspace. */
export default function DaemonStartupStatus({ compact = false }: { readonly compact?: boolean }) {
  const phase = useDaemonStartupPhase();
  if (phase === "starting") return <StartingLine compact={compact} />;
  if (phase === "failed") return <NotAnswering compact={compact} />;
  return null;
}

function StartingLine({ compact }: { readonly compact: boolean }) {
  const { t } = useTranslation();
  const label = t("boot.startingInBackground");
  return (
    <div
      role="status"
      aria-label={compact ? label : undefined}
      title={compact ? label : undefined}
      data-testid="daemon-startup-status"
      className={`mem-daemon-startup-status${compact ? " mem-daemon-startup-status--compact" : ""}`}
    >
      <span aria-hidden="true" className="mem-daemon-startup-dot" />
      {!compact && <span className="mem-daemon-startup-label">{label}</span>}
    </div>
  );
}

function NotAnswering({ compact }: { readonly compact: boolean }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  // Opens by itself once: nothing in the workspace can load, so this is the
  // one case that needs the user. After that it opens only on request.
  const [expanded, setExpanded] = useState(true);
  const [restarting, setRestarting] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const restartingRef = useRef(false);
  const anchorRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const popoverId = useId();
  const label = t("boot.notAnsweringLabel");

  useEffect(() => {
    if (!expanded) return;
    const closeOutside = (event: Event) => {
      if (!anchorRef.current?.contains(event.target as Node)) setExpanded(false);
    };
    document.addEventListener("pointerdown", closeOutside);
    return () => document.removeEventListener("pointerdown", closeOutside);
  }, [expanded]);

  async function handleRestart() {
    if (restartingRef.current) return;
    restartingRef.current = true;
    setRestarting(true);
    setFailure(null);
    try {
      await restartDaemon();
      // Asking the gate again puts the workspace back to "starting": this row
      // goes back to the quiet line, and returns only if that attempt fails.
      void queryClient.invalidateQueries({ queryKey: ["shouldShowWizard"] });
    } catch (error) {
      const raw = error instanceof Error ? error.message : String(error);
      const key = restartErrorKey(raw);
      setFailure(key ? t(key) : raw);
    } finally {
      restartingRef.current = false;
      setRestarting(false);
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
        data-testid="daemon-not-answering"
        aria-label={label}
        title={label}
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
        {!compact && <span aria-hidden="true" className="mem-activity-status-label">{label}</span>}
      </button>
      {expanded && (
        <div
          id={popoverId}
          className="mem-popover-surface mem-activity-popover"
          data-testid="daemon-not-answering-popover"
          role="dialog"
          aria-label={label}
        >
          <h2 className="mem-activity-summary-title">{label}</h2>
          <p className="mem-daemon-startup-body">{t("boot.notAnsweringBody")}</p>
          {failure && <p role="alert" className="mem-daemon-startup-failure">{failure}</p>}
          <div>
            <button
              type="button"
              className="mem-activity-popover-action"
              data-testid="daemon-not-answering-restart"
              disabled={restarting}
              onClick={() => { void handleRestart(); }}
            >
              {restarting ? t("boot.starting") : t("daemonService.restart")}
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
