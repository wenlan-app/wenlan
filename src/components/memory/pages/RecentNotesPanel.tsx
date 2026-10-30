// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useEffect, useId, useMemo, useRef, useState } from "react";
import { DotsSixVertical } from "@phosphor-icons/react";
import { PageIcon } from "../assets/PageIcon";
import { useTranslation } from "react-i18next";
import { forgetRecentPage, readRecentPageHistory, renameRecentPage } from "../../../lib/recentPages";
import type { Page } from "../../../lib/tauri";
import { MovePageDialog } from "./MovePageDialog";
import { RecentNoteMenu, type RecentNoteMenuTarget } from "./RecentNoteMenu";
import { RecentNoteRename } from "./RecentNoteRename";
import { orderWikiRecentEntries, readWikiCustomOrder, writeWikiCustomOrder } from "../../../lib/wikiNotesPreferences";
import { useNoteReorderDrag } from "./useNoteReorderDrag";
import { useDaemonStartupPhase } from "../../../lib/daemonStartup";
import "./recentNotesPanel.css";

export interface RecentNotesPanelProps {
  readonly currentPageId?: string | null;
  readonly revision?: number;
  readonly mode?: "recent" | "custom";
  readonly onOpenPage?: (id: string) => void;
  /** Called after a page is deleted from this list, so open tabs can let it go. */
  readonly onDeletePage?: (id: string) => void;
  /** Offered from the empty list, so a note that was written but never opened is one click away. */
  readonly onBrowseFolders?: () => void;
}

export function RecentNotesPanel({ currentPageId = null, revision = 0, mode = "recent", onOpenPage, onDeletePage, onBrowseFolders }: RecentNotesPanelProps) {
  const { t } = useTranslation();
  const isManualOrder = mode === "custom";
  const reorderHintId = useId();
  // Bumped when this list changes the history itself (remove, rename, delete).
  const [localRevision, setLocalRevision] = useState(0);
  const refresh = useCallback(() => setLocalRevision(value => value + 1), []);
  const entries = useMemo(() => readRecentPageHistory().entries, [revision, currentPageId, localRevision]);
  const [menu, setMenu] = useState<RecentNoteMenuTarget | null>(null);
  const [renaming, setRenaming] = useState<Page | null>(null);
  const [movePage, setMovePage] = useState<Page | null>(null);
  const rowButtons = useRef(new Map<string, HTMLButtonElement>());
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

  const menuStateRef = useRef(menu);
  menuStateRef.current = menu;
  const dismissMenu = useCallback((restoreFocus: boolean) => {
    if (restoreFocus) menuStateRef.current?.invoker.focus();
    setMenu(null);
  }, []);
  const openMenu = (id: string, invoker: HTMLElement, x: number, y: number) => {
    if (renaming) return;
    setMenu({ id, invoker, x, y });
  };
  const focusRow = (id: string) => requestAnimationFrame(() => rowButtons.current.get(id)?.focus());
  const removeFromList = (id: string) => {
    setMenu(null);
    forgetRecentPage(id);
    refresh();
  };
  const deleted = (id: string) => {
    forgetRecentPage(id);
    refresh();
    onDeletePage?.(id);
  };
  const renamed = (page: Page, title: string | null) => {
    setRenaming(null);
    if (title !== null) {
      renameRecentPage(page.id, title);
      refresh();
    }
    focusRow(page.id);
  };

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
              {renaming?.id === entry.id ? (
                <RecentNoteRename page={renaming} onDone={title => renamed(renaming, title)} />
              ) : (
              <button
                type="button"
                className="wiki-recent-note"
                ref={node => { if (node) rowButtons.current.set(entry.id, node); else rowButtons.current.delete(entry.id); }}
                aria-label={entry.title}
                title={entry.title}
                aria-current={entry.id === currentPageId ? "page" : undefined}
                onClick={() => onOpenPage?.(entry.id)}
                onContextMenu={event => {
                  event.preventDefault();
                  openMenu(entry.id, event.currentTarget, event.clientX, event.clientY);
                }}
                onKeyDown={event => {
                  if (event.key === "ContextMenu" || (event.key === "F10" && event.shiftKey)) {
                    event.preventDefault();
                    const rect = event.currentTarget.getBoundingClientRect();
                    openMenu(entry.id, event.currentTarget, rect.left, rect.bottom);
                  }
                }}
                disabled={!onOpenPage}
              >
                <PageIcon size={16} />
                <span>{entry.title}</span>
              </button>
              )}
            </li>
          ))}
        </ul>
      )}
      {menu && (
        <RecentNoteMenu
          target={menu}
          onDismiss={dismissMenu}
          onOpen={() => { setMenu(null); onOpenPage?.(menu.id); }}
          onRename={page => { setMenu(null); setRenaming(page); }}
          onMove={page => { setMenu(null); setMovePage(page); }}
          onRemove={() => removeFromList(menu.id)}
          onDeleted={deleted}
        />
      )}
      {movePage && <MovePageDialog page={movePage} onClose={() => { const { id } = movePage; setMovePage(null); focusRow(id); }} />}
    </section>
  );
}
