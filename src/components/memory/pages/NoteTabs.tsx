// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import { House, Plus, X } from "@phosphor-icons/react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import type { NoteTab } from "./noteTabState";
import "./noteTabs.css";
type NoteTabsProps = {
  readonly tabs: readonly NoteTab[];
  readonly activeKey?: string;
  readonly onSelect: (tab: NoteTab) => void;
  readonly onClose: (tab: NoteTab) => void;
  readonly onCreate: () => void;
  readonly contentId?: string;
  readonly onMoveToOtherGroup?: (tab: NoteTab) => void;
  readonly moveLabel?: string;
  readonly groupId?: "primary" | "secondary";
  readonly tabListLabel?: string;
  /** Primary strip only: a button back to the Wiki home, which has no tab of its own. It hides while the home is showing. */
  readonly onHome?: () => void;
  readonly homeActive?: boolean;
};

type TabContextMenu = { readonly tab: NoteTab; readonly invoker: HTMLButtonElement; readonly x: number; readonly y: number };

export function NoteTabs({ tabs, activeKey, onSelect, onClose, onCreate, contentId = "wiki-note-content", onMoveToOtherGroup, moveLabel, groupId = "primary", tabListLabel, onHome, homeActive = false }: NoteTabsProps) {
  const { t } = useTranslation();
  const id = useId();
  const list = useRef<HTMLDivElement>(null);
  const createButton = useRef<HTMLButtonElement>(null);
  const closing = useRef<{ key: string; index: number } | null>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const menuItemRef = useRef<HTMLButtonElement>(null);
  const [contextMenu, setContextMenu] = useState<TabContextMenu | null>(null);
  const canMove = Boolean(onMoveToOtherGroup && moveLabel);
  useEffect(() => {
    const pending = closing.current;
    if (!pending || tabs.some(tab => tab.key === pending.key)) return;
    closing.current = null;
    if (tabs.length === 0) {
      createButton.current?.focus();
      return;
    }
    const buttons = list.current?.querySelectorAll<HTMLButtonElement>('[role="tab"]');
    const active = tabs.findIndex(tab => tab.key === activeKey);
    buttons?.[active >= 0 ? active : Math.min(pending.index, tabs.length - 1)]?.focus();
  }, [tabs, activeKey]);
  useEffect(() => {
    const strip = list.current;
    if (!strip) return;
    const revealActive = () => {
      const active = strip.querySelector<HTMLElement>('.note-tab[data-active="true"]');
      if (!active) return;
      const bounds = strip.getBoundingClientRect();
      const tabBounds = active.getBoundingClientRect();
      if (tabBounds.left < bounds.left) strip.scrollLeft += tabBounds.left - bounds.left;
      else if (tabBounds.right > bounds.right) strip.scrollLeft += tabBounds.right - bounds.right;
    };
    revealActive();
    // A sidebar or inspector can resize the strip without changing the tab.
    if (typeof ResizeObserver === "undefined") {
      window.addEventListener("resize", revealActive);
      return () => window.removeEventListener("resize", revealActive);
    }
    const observer = new ResizeObserver(revealActive);
    observer.observe(strip);
    return () => observer.disconnect();
  }, [activeKey, tabs]);
  useLayoutEffect(() => {
    if (!contextMenu) return;
    const menu = menuRef.current;
    if (menu) {
      const bounds = menu.getBoundingClientRect();
      const x = Math.max(8, Math.min(contextMenu.x, window.innerWidth - bounds.width - 8));
      const y = Math.max(8, Math.min(contextMenu.y, window.innerHeight - bounds.height - 8));
      if (x !== contextMenu.x || y !== contextMenu.y) {
        setContextMenu(current => current ? { ...current, x, y } : current);
      }
    }
    menuItemRef.current?.focus();
    const onPointerDown = (event: PointerEvent) => {
      if (!menuRef.current?.contains(event.target as Node)) setContextMenu(null);
    };
    const onViewportChange = () => setContextMenu(null);
    document.addEventListener("pointerdown", onPointerDown);
    window.addEventListener("resize", onViewportChange);
    window.addEventListener("scroll", onViewportChange, true);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown);
      window.removeEventListener("resize", onViewportChange);
      window.removeEventListener("scroll", onViewportChange, true);
    };
  }, [contextMenu]);
  const openContextMenu = (tab: NoteTab, invoker: HTMLButtonElement, x: number, y: number) => {
    if (!canMove) return;
    setContextMenu({ tab, invoker, x, y });
  };
  const dismissContextMenu = (restoreFocus: boolean) => {
    const invoker = contextMenu?.invoker;
    setContextMenu(null);
    if (restoreFocus) invoker?.focus();
  };
  // Tauri only drags the window when the pressed element itself is marked, so
  // the strip and its list mark themselves; the tabs and buttons stay clickable.
  return <div className="note-tabs" data-note-tab-group={groupId} data-tauri-drag-region>
    {onHome && <button type="button" className="note-tab-home" aria-label={t("pages.tabs.home")} title={t("pages.tabs.home")}
      data-active={homeActive} aria-current={homeActive ? "page" : undefined} onClick={onHome}>
      <House size={14} aria-hidden="true" />
    </button>}
    {tabs.length > 0 && <div className="note-tabs-list" role="tablist" aria-label={tabListLabel ?? t("pages.tabs.label")} ref={list} data-tauri-drag-region>
      {tabs.map((tab, index) => {
        const title = tab.title.trim() || t("pages.overview.untitledDraft");
        return <div className="note-tab" data-note-tab-key={tab.key} data-active={tab.key === activeKey} key={tab.key}>
          <button type="button" role="tab" id={`${id}-${index}`} aria-selected={tab.key === activeKey}
            aria-controls={contentId} tabIndex={tab.key === activeKey || (!activeKey && index === 0) ? 0 : -1}
            title={title}
            onClick={event => {
              if (event.currentTarget.dataset.noteDragSwallowClick === "true") {
                delete event.currentTarget.dataset.noteDragSwallowClick;
                event.preventDefault();
                event.stopPropagation();
                return;
              }
              onSelect(tab);
            }} onKeyDown={event => {
              if (canMove && (event.key === "ContextMenu" || (event.key === "F10" && event.shiftKey))) {
                event.preventDefault();
                event.stopPropagation();
                const bounds = event.currentTarget.getBoundingClientRect();
                openContextMenu(tab, event.currentTarget, bounds.left, bounds.bottom);
                return;
              }
              const offset = event.key === "ArrowRight" ? 1 : event.key === "ArrowLeft" ? -1 : 0;
              if (offset || event.key === "Home" || event.key === "End") {
                event.preventDefault();
                const target = event.key === "Home" ? 0 : event.key === "End" ? tabs.length - 1 : (index + offset + tabs.length) % tabs.length;
                list.current?.querySelectorAll<HTMLButtonElement>('[role="tab"]')[target]?.focus();
              }
            }} onContextMenu={event => {
              if (!canMove) return;
              event.preventDefault();
              openContextMenu(tab, event.currentTarget, event.clientX, event.clientY);
            }}><span>{title}</span></button>
          <button type="button" className="note-tab-close" aria-label={t("pages.tabs.close", { title })}
            onClick={() => { closing.current = { key: tab.key, index }; onClose(tab); }}><X size={14} aria-hidden="true" /></button>
        </div>;
      })}
    </div>}
    {contextMenu && moveLabel && onMoveToOtherGroup && createPortal(<div className="mem-popover-surface note-tab-context-menu" role="menu" aria-label={moveLabel} ref={menuRef} style={{ left: contextMenu.x, top: contextMenu.y }} onKeyDown={event => {
      if (event.key === "Escape") {
        event.preventDefault(); event.stopPropagation(); dismissContextMenu(true);
      } else if (event.key === "Tab") dismissContextMenu(false);
    }}>
      <button type="button" role="menuitem" ref={menuItemRef} onClick={() => {
        const { tab: target, invoker } = contextMenu;
        setContextMenu(null);
        invoker.focus();
        onMoveToOtherGroup(target);
      }}>{moveLabel}</button>
    </div>, document.body)}
    <button type="button" className="note-tab-create" aria-label={t("pages.overview.newPage")} title={t("pages.overview.newPage")} onClick={onCreate} ref={createButton}>
      <Plus size={14} aria-hidden="true" />
    </button>
  </div>;
}
