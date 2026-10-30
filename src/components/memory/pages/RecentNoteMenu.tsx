// SPDX-License-Identifier: AGPL-3.0-only
import { useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { confirmAction } from "../../../lib/confirmAction";
import { getPage, type Page } from "../../../lib/tauri";
import { deleteWikiPage } from "./deleteWikiPage";
import { archivePageFromList, canArchivePage } from "./archiveWikiPage";
import { useExampleWikiActive } from "../../../lib/exampleWiki/mode";
import { EXPLICIT_BROWSE_QUERY_POLICY } from "./listAllPages";
import { inventoryPageFilename } from "./pageInventory";
import "./pageInventory.css";

export interface RecentNoteMenuTarget {
  readonly id: string;
  readonly x: number;
  readonly y: number;
  readonly invoker: HTMLElement;
}

/** Same rule as the page's own Rename action. */
export function canRenameFromList(page: Page): boolean {
  return page.status === "active" && page.creation_kind !== "entity" &&
    page.creation_kind !== "source" && page.creation_kind !== "imported";
}

// One delete at a time: the menu closes before the confirmation, so the guard
// cannot live in its state.
let deleting = false;

export function RecentNoteMenu({ target, onDismiss, onOpen, onRename, onMove, onRemove, onDeleted, onArchiveOpenPage, onArchived }: {
  readonly target: RecentNoteMenuTarget;
  readonly onDismiss: (restoreFocus: boolean) => void;
  readonly onOpen: () => void;
  readonly onRename: (page: Page) => void;
  readonly onMove: (page: Page) => void;
  readonly onRemove: () => void;
  readonly onDeleted: (id: string) => void;
  /** Hand an archive request to the editor that has this page open; see WikiWorkspace. */
  readonly onArchiveOpenPage?: (id: string) => Promise<boolean>;
  readonly onArchived?: (id: string) => void;
}) {
  const exampleWiki = useExampleWikiActive();
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const menuRef = useRef<HTMLDivElement>(null);
  const [placement, setPlacement] = useState({ x: target.x, y: target.y });
  const page = useQuery({
    queryKey: ["page", target.id],
    queryFn: () => getPage(target.id, "explicit"),
    ...EXPLICIT_BROWSE_QUERY_POLICY,
  });
  const loaded = page.data ?? null;
  const renameReady = loaded !== null && canRenameFromList(loaded);
  const moveReady = loaded !== null && loaded.status === "active" && inventoryPageFilename(loaded) !== null;

  useLayoutEffect(() => {
    const menu = menuRef.current;
    if (!menu) return;
    const bounds = menu.getBoundingClientRect();
    setPlacement({
      x: Math.max(8, Math.min(target.x, window.innerWidth - bounds.width - 8)),
      y: Math.max(8, Math.min(target.y, window.innerHeight - bounds.height - 8)),
    });
    menu.querySelector<HTMLElement>("[role='menuitem']")?.focus();
    const onPointerDown = (event: PointerEvent) => {
      if (!menuRef.current?.contains(event.target as Node)) onDismiss(false);
    };
    const onViewportChange = () => onDismiss(false);
    document.addEventListener("pointerdown", onPointerDown);
    window.addEventListener("resize", onViewportChange);
    window.addEventListener("scroll", onViewportChange, true);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown);
      window.removeEventListener("resize", onViewportChange);
      window.removeEventListener("scroll", onViewportChange, true);
    };
  }, [target, onDismiss]);

  const deleteFromList = async (page: Page) => {
    onDismiss(true);
    if (deleting) return;
    deleting = true;
    try {
      if (!await confirmAction(t("pageDetail.deleteConfirm"), {
        confirm: t("pageDetail.deletePage"),
        cancel: t("common.cancel"),
      })) return;
      await deleteWikiPage(queryClient, page);
      onDeleted(page.id);
    } catch {
      toast(t("pageDetail.deleteError"));
    } finally { deleting = false; }
  };

  const archiveFromList = async (page: Page) => {
    onDismiss(true);
    if (deleting) return;
    await archivePageFromList(queryClient, page, { t, onArchiveOpenPage, onArchived });
  };

  const item = (label: string, enabled: boolean, run: () => void, danger = false) => (
    <button
      type="button"
      role="menuitem"
      className={danger ? "is-danger" : undefined}
      aria-disabled={!enabled}
      onClick={() => { if (enabled) run(); }}
    >
      {label}
    </button>
  );

  return createPortal(
    <div
      className="mem-popover-surface notes-folder-context-menu"
      role="menu"
      aria-label={t("pageDetail.actions")}
      ref={menuRef}
      style={{ left: placement.x, top: placement.y }}
      onContextMenu={event => event.preventDefault()}
      onKeyDown={event => {
        if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); onDismiss(true); }
        else if (event.key === "Tab") onDismiss(false);
        else if (event.key === "ArrowDown" || event.key === "ArrowUp") {
          const items = Array.from(event.currentTarget.querySelectorAll<HTMLElement>("[role='menuitem']"));
          const index = items.indexOf(document.activeElement as HTMLElement);
          event.preventDefault();
          items[(index + (event.key === "ArrowDown" ? 1 : -1) + items.length) % items.length]?.focus();
        }
      }}
    >
      {item(t("pages.folders.openNote"), true, onOpen)}
      {item(t("pageDetail.renamePage"), renameReady, () => { if (loaded) onRename(loaded); })}
      {item(t("pages.folders.move"), moveReady, () => { if (loaded) onMove(loaded); })}
      {item(t("pages.recent.remove"), true, onRemove)}
      {!exampleWiki && loaded !== null && canArchivePage(loaded) &&
        item(t("pageDetail.archivePage"), true, () => void archiveFromList(loaded))}
      {item(t("pages.folders.deleteNote"), loaded !== null, () => { if (loaded) void deleteFromList(loaded); }, true)}
    </div>,
    document.body,
  );
}
