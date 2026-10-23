// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useEffect, useId, useMemo, useState } from "react";
import { DotsSixVertical } from "@phosphor-icons/react";
import { PageIcon } from "../assets/PageIcon";
import { useTranslation } from "react-i18next";
import { readRecentPageHistory } from "../../../lib/recentPages";
import { orderWikiRecentEntries, readWikiCustomOrder, writeWikiCustomOrder } from "../../../lib/wikiNotesPreferences";
import { useNoteReorderDrag } from "./useNoteReorderDrag";
import { useDaemonStartupPhase } from "../../../lib/daemonStartup";
import "./recentNotesPanel.css";

export interface RecentNotesPanelProps {
  readonly currentPageId?: string | null;
  readonly revision?: number;
  readonly mode?: "recent" | "custom";
  readonly onOpenPage?: (id: string) => void;
  /** Offered from the empty list, so a note that was written but never opened is one click away. */
  readonly onBrowseFolders?: () => void;
}

export function RecentNotesPanel({ currentPageId = null, revision = 0, mode = "recent", onOpenPage, onBrowseFolders }: RecentNotesPanelProps) {
  const { t } = useTranslation();
  const isManualOrder = mode === "custom";
  const reorderHintId = useId();
  const entries = useMemo(() => readRecentPageHistory().entries, [revision, currentPageId]);
  const [customOrder, setCustomOrder] = useState<readonly string[]>(() => readWikiCustomOrder());
  const orderedEntries = useMemo(() => mode === "custom" ? orderWikiRecentEntries(entries, customOrder) : entries, [entries, mode, customOrder]);
  const orderedIds = useMemo(() => orderedEntries.map(({ id }) => id), [orderedEntries]);
  useEffect(() => {
    if (mode !== "custom") return;
    setCustomOrder(current => current.length === orderedIds.length && current.every((id, index) => id === orderedIds[index]) ? current : orderedIds);
    writeWikiCustomOrder(orderedIds);
  }, [mode, orderedIds]);
  const reorder = useCallback((id: string, toIndex: number) => {
    const fromIndex = orderedIds.indexOf(id);
    if (fromIndex < 0 || toIndex < 0 || toIndex >= orderedIds.length || fromIndex === toIndex) return;
    const next = [...orderedIds];
    next.splice(fromIndex, 1);
    next.splice(toIndex, 0, id);
    setCustomOrder(next);
    writeWikiCustomOrder(next);
  }, [orderedIds]);
  const drag = useNoteReorderDrag(orderedIds, reorder, mode === "custom");
  // The list itself is this machine's own history and shows at once. Its empty
  // hint ("Pages you open appear here") would read as a lost library while the
  // daemon is still starting, so it waits for the daemon too.
  const daemonReady = useDaemonStartupPhase() === "ready";

  return (
    <section className="wiki-recent-notes" aria-label={isManualOrder ? t("pages.recent.custom") : t("pages.recent.title")}>
      {mode === "custom" && <span id={reorderHintId} className="wiki-recent-sr-only">{t("pages.recent.reorderHint")}</span>}
      {orderedEntries.length === 0 ? (!daemonReady ? null : (
        <div className="wiki-recent-notes-empty">
          <p>{t("pages.recent.empty")}</p>
          {onBrowseFolders && <button type="button" onClick={onBrowseFolders}>{t("pages.recent.browseFolders")}</button>}
        </div>
      )) : (
        <ul className="wiki-recent-notes-list">
          {orderedEntries.map(entry => (
            <li key={entry.id} {...drag.rowProps(entry.id)}>
              {mode === "custom" && (
                <button
                  type="button"
                  className="wiki-recent-note-grip"
                  aria-label={t("pages.recent.dragNote", { title: entry.title, defaultValue: `Drag to reorder ${entry.title}` })}
                  aria-describedby={reorderHintId}
                  title={t("pages.recent.dragNote", { title: entry.title, defaultValue: `Drag to reorder ${entry.title}` })}
                  onPointerDown={event => drag.begin(event, entry.id)}
                  onKeyDown={event => {
                    if (event.key !== "ArrowUp" && event.key !== "ArrowDown") return;
                    event.preventDefault();
                    const index = orderedIds.indexOf(entry.id);
                    reorder(entry.id, index + (event.key === "ArrowUp" ? -1 : 1));
                  }}
                >
                  <DotsSixVertical aria-hidden="true" size={16} />
                </button>
              )}
              <button
                type="button"
                className="wiki-recent-note"
                aria-label={entry.title}
                title={entry.title}
                aria-current={entry.id === currentPageId ? "page" : undefined}
                onClick={() => onOpenPage?.(entry.id)}
                disabled={!onOpenPage}
              >
                <PageIcon size={16} />
                <span>{entry.title}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
