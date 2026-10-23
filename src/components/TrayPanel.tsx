// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useMemo, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  getBackgroundAiEnabled,
  listNewMaterial,
  listPendingRevisions,
  restartDaemon,
  type Page,
} from "../lib/tauri";
import { STEEP_COMMAND } from "../lib/steepMode";
import { isKnowledgePage, listAllActivePages } from "./memory/pages/listAllPages";
import "./TrayPanel.css";

/** Sent by the native side each time the panel opens. */
export const TRAY_PANEL_OPENED_EVENT = "tray-panel-opened";
const TRAY_STATE_EVENT = "tray-state-changed";
const RECENT_LIMIT = 3;
const REVISION_LIMIT = 50;

export type TrayDaemonState = "starting" | "up" | "down";

/** Where the main window goes; mirrors `tray_panel::TrayPanelTarget`. */
export type TrayPanelTarget =
  | { kind: "home" }
  | { kind: "page"; pageId: string }
  | { kind: "review" }
  | { kind: "search" };

const TRAY_STATE_BY_CODE: Record<number, TrayDaemonState> = { 0: "starting", 1: "up", 2: "down" };

function openMain(target: TrayPanelTarget) {
  return invoke("tray_panel_open_main", { target });
}

function useDaemonState(): TrayDaemonState {
  const [state, setState] = useState<TrayDaemonState>("starting");
  useEffect(() => {
    let alive = true;
    invoke<TrayDaemonState>("tray_panel_daemon_state")
      .then((value) => { if (alive) setState(value); })
      .catch(() => {});
    const unlisten = listen<number>(TRAY_STATE_EVENT, ({ payload }) => {
      setState(TRAY_STATE_BY_CODE[payload] ?? "starting");
    });
    return () => {
      alive = false;
      unlisten.then((f) => f());
    };
  }, []);
  return state;
}

/** "10:42" today, "Yesterday", or a short date. */
function whenLabel(iso: string, now: Date, locale: string, yesterday: string): string {
  const at = new Date(iso);
  if (Number.isNaN(at.getTime())) return "";
  const startOfToday = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime();
  if (at.getTime() >= startOfToday) {
    return at.toLocaleTimeString(locale, { hour: "2-digit", minute: "2-digit", hour12: false });
  }
  if (at.getTime() >= startOfToday - 86_400_000) return yesterday;
  return at.toLocaleDateString(locale, { month: "numeric", day: "numeric" });
}

/** The most recently changed pages a person or Wenlan wrote. */
export function recentPages(pages: Page[], limit = RECENT_LIMIT): Page[] {
  return pages
    .filter(isKnowledgePage)
    .sort((a, b) => Date.parse(b.last_modified) - Date.parse(a.last_modified))
    .slice(0, limit);
}

/** Pages with an AI edit waiting for the user's yes or no. */
export function pagesWithPendingEdits(
  revisions: { target_kind: string; target_source_id: string }[],
): number {
  return new Set(
    revisions.filter((item) => item.target_kind === "page").map((item) => item.target_source_id),
  ).size;
}

function PageIcon() {
  return (
    <svg className="tray-panel-icon" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.4" aria-hidden="true">
      <path d="M4 2h5l3 3v9H4z" />
      <path d="M9 2v3h3" />
    </svg>
  );
}

function SearchIcon() {
  return (
    <svg width="15" height="15" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.6" aria-hidden="true">
      <circle cx="7" cy="7" r="4.8" />
      <path d="M10.6 10.6 14 14" />
    </svg>
  );
}

export default function TrayPanel() {
  const { t, i18n } = useTranslation();
  const queryClient = useQueryClient();
  const daemon = useDaemonState();
  const up = daemon === "up";
  const [restarting, setRestarting] = useState(false);
  const [now, setNow] = useState(() => new Date());
  const rootRef = useRef<HTMLDivElement>(null);

  const pages = useQuery({ queryKey: ["tray-panel", "pages"], queryFn: listAllActivePages, enabled: up });
  const revisions = useQuery({
    queryKey: ["tray-panel", "revisions"],
    queryFn: () => listPendingRevisions(REVISION_LIMIT),
    enabled: up,
  });
  const automatic = useQuery({ queryKey: ["tray-panel", "steep-mode"], queryFn: () => getBackgroundAiEnabled(), enabled: up });
  const material = useQuery({ queryKey: ["tray-panel", "new-material"], queryFn: listNewMaterial, enabled: up });

  // The panel's webview lives on while hidden; reread everything each time
  // it opens, so it never shows what was true an hour ago.
  useEffect(() => {
    const unlisten = listen(TRAY_PANEL_OPENED_EVENT, () => {
      setNow(new Date());
      void queryClient.invalidateQueries({ queryKey: ["tray-panel"] });
    });
    return () => { unlisten.then((f) => f()); };
  }, [queryClient]);

  useEffect(() => {
    if (up) setRestarting(false);
  }, [up]);

  // The window is as tall as its content.
  useEffect(() => {
    const node = rootRef.current;
    if (!node || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(() => {
      void invoke("tray_panel_set_height", { height: Math.ceil(node.getBoundingClientRect().height) }).catch(() => {});
    });
    observer.observe(node);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") void invoke("tray_panel_hide");
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const recent = useMemo(() => recentPages(pages.data ?? []), [pages.data]);
  const pendingEdits = pagesWithPendingEdits(revisions.data ?? []);
  const waiting = Math.max(0, (material.data?.total ?? 0) - (material.data?.held ?? 0));

  let steepLine: string | null = null;
  if (automatic.isSuccess && material.isSuccess) {
    if (automatic.data) {
      steepLine = waiting > 0 ? t("trayPanel.steepAutoQueued", { count: waiting }) : t("trayPanel.steepAutoDone");
    } else {
      steepLine = waiting > 0
        ? t("trayPanel.steepOnDemandReady", { count: waiting, command: STEEP_COMMAND })
        : t("trayPanel.steepOnDemandDone");
    }
  }

  const statusLabel = daemon === "up"
    ? t("trayPanel.running")
    : daemon === "down"
      ? t("trayPanel.stopped")
      : t("trayPanel.starting");

  const restart = () => {
    setRestarting(true);
    restartDaemon().catch(() => setRestarting(false));
  };

  return (
    <div ref={rootRef} className="tray-panel" data-testid="tray-panel">
      <div className="tray-panel-top">
        <span className={`tray-panel-dot tray-panel-dot--${daemon}`} aria-hidden="true" />
        <span className="tray-panel-title">{t("trayPanel.title")}</span>
        <span className="tray-panel-meta" data-testid="tray-panel-status">{statusLabel}</span>
      </div>

      {daemon === "down" ? (
        <div className="tray-panel-need tray-panel-need--danger" data-testid="tray-panel-daemon-down">
          <span className="tray-panel-need-text">{t("trayPanel.daemonDown")}</span>
          <button type="button" className="tray-panel-need-action" onClick={restart} disabled={restarting}>
            {restarting ? t("trayPanel.restarting") : t("trayPanel.restart")}
          </button>
        </div>
      ) : (
        <>
          <button type="button" className="tray-panel-search" onClick={() => void openMain({ kind: "search" })}>
            <SearchIcon />
            <span>{t("trayPanel.search")}</span>
          </button>

          {pendingEdits > 0 && (
            <div className="tray-panel-group">
              <div className="tray-panel-label">{t("trayPanel.needsYou")}</div>
              <div className="tray-panel-need" data-testid="tray-panel-needs-you">
                <span className="tray-panel-need-text">{t("trayPanel.revisions", { count: pendingEdits })}</span>
                <button type="button" className="tray-panel-need-action" onClick={() => void openMain({ kind: "review" })}>
                  {t("trayPanel.review")}
                </button>
              </div>
            </div>
          )}

          {up && pages.isSuccess && (
            <div className="tray-panel-group">
              <div className="tray-panel-label">{t("trayPanel.recent")}</div>
              {recent.length === 0 ? (
                <p className="tray-panel-quiet">{t("trayPanel.noPages")}</p>
              ) : (
                recent.map((page) => (
                  <button
                    key={page.id}
                    type="button"
                    className="tray-panel-row"
                    aria-label={t("trayPanel.openPage", { title: page.title })}
                    onClick={() => void openMain({ kind: "page", pageId: page.id })}
                  >
                    <PageIcon />
                    <span className="tray-panel-row-title">{page.title}</span>
                    <span className="tray-panel-row-meta">
                      {whenLabel(page.last_modified, now, i18n.language, t("trayPanel.yesterday"))}
                    </span>
                  </button>
                ))
              )}
            </div>
          )}

          {steepLine && <p className="tray-panel-quiet" data-testid="tray-panel-steep">{steepLine}</p>}
        </>
      )}

      <div className="tray-panel-foot">
        <button type="button" className="tray-panel-btn tray-panel-btn--main" onClick={() => void openMain({ kind: "home" })}>
          {t("trayPanel.open")}
        </button>
        {daemon !== "down" && (
          <button
            type="button"
            className="tray-panel-btn"
            onClick={() => {
              void invoke("tray_panel_hide")
                .then(() => invoke("open_quick_capture", { placement: "bottom-right" }))
                .catch(() => {});
            }}
          >
            {t("trayPanel.quickCapture")}
          </button>
        )}
        <button type="button" className="tray-panel-btn tray-panel-btn--small" onClick={() => void invoke("tray_panel_quit")}>
          {t("trayPanel.quit")}
        </button>
      </div>
    </div>
  );
}
