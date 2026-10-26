// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useRef, useState, type CSSProperties } from "react";
import { useTranslation } from "react-i18next";
import {
  STABILITY_TIERS,
  acceptPendingRevision,
  dismissPendingRevision,
  getPendingRevision,
  type MemoryItem,
  type PendingRevision,
} from "../../lib/tauri";
import ContentRenderer from "./ContentRenderer";
import { ARCHIVED_MEMORY_OPACITY } from "./archivedMemoryOpacity";
import { resolveAgentDisplayName } from "../../lib/agents";
import "./memoryListReading.css";

interface MemoryListRowProps {
  memory: MemoryItem;
  onConfirm: (sourceId: string, confirmed: boolean) => void;
  onDelete: (sourceId: string) => void;
  onPin?: (sourceId: string) => void;
  onUnpin?: (sourceId: string) => void;
  onClick?: (sourceId: string) => void;
  style?: React.CSSProperties;
}

export default function MemoryListRow({
  memory,
  onConfirm,
  onDelete,
  onPin,
  onUnpin,
  onClick,
  style,
}: MemoryListRowProps) {
  const { t } = useTranslation();
  const [deleting, setDeleting] = useState(false);
  const [menuOpen, setMenuOpen] = useState(false);
  const actionsRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const menuFocus = useRef<"first" | "last">("first");
  const [pendingRevision, setPendingRevision] = useState<PendingRevision | null>(null);

  const facetType = memory.memory_type ?? "fact";
  const tier = STABILITY_TIERS[facetType] ?? "ephemeral";
  const isConfirmed = memory.stability === "confirmed" || (!memory.stability && memory.confirmed);
  const rowTitle = memory.title || memory.content || t("memoryList.untitledMemory");
  const handleOpen = () => onClick?.(memory.source_id);
  const handleKeyDown = (event: React.KeyboardEvent<HTMLElement>) => {
    if (event.target !== event.currentTarget) return;
    if (event.key !== "Enter" && event.key !== " ") return;
    event.preventDefault();
    handleOpen();
  };

  const closeMenu = () => {
    setMenuOpen(false);
    triggerRef.current?.focus();
  };

  useEffect(() => {
    if (!menuOpen) return;
    const items = menuRef.current?.querySelectorAll<HTMLButtonElement>("[role='menuitem']");
    items?.[menuFocus.current === "last" ? items.length - 1 : 0]?.focus();
    const dismissOutside = (event: PointerEvent) => {
      if (!actionsRef.current?.contains(event.target as Node)) setMenuOpen(false);
    };
    document.addEventListener("pointerdown", dismissOutside);
    return () => document.removeEventListener("pointerdown", dismissOutside);
  }, [menuOpen]);

  const handleMenuKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      closeMenu();
      return;
    }
    if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) return;
    event.preventDefault();
    event.stopPropagation();
    const items = Array.from(menuRef.current?.querySelectorAll<HTMLButtonElement>("[role='menuitem']") ?? []);
    const current = items.indexOf(document.activeElement as HTMLButtonElement);
    const next = event.key === "Home" ? 0 : event.key === "End" ? items.length - 1
      : (current + (event.key === "ArrowDown" ? 1 : -1) + items.length) % items.length;
    items[next]?.focus();
  };

  useEffect(() => {
    if (tier === "protected" && isConfirmed) {
      getPendingRevision(memory.source_id).then(setPendingRevision).catch(() => {});
    }
  }, [tier, isConfirmed, memory.source_id]);

  const handleAcceptRevision = async () => {
    if (!pendingRevision) return;
    await acceptPendingRevision(memory.source_id);
    setPendingRevision(null);
    window.dispatchEvent(new CustomEvent("memory-updated"));
  };

  const handleDismissRevision = async () => {
    if (!pendingRevision) return;
    await dismissPendingRevision(memory.source_id);
    setPendingRevision(null);
  };

  if (deleting) return null;

  // Caller's style spreads first so the archived values always win.
  // `--mem-enter-opacity` carries the muted value into the `mem-fade-up` end
  // keyframe; without it that animation's retained fill resets the row to full
  // opacity and the fade never shows, however the inline style is written.
  return (
    <article
      aria-label={rowTitle}
      className="memory-list-row memory-list-reading-row"
      onKeyDown={handleKeyDown}
      tabIndex={0}
      style={
        memory.is_archived
          ? ({
              ...style,
              opacity: ARCHIVED_MEMORY_OPACITY,
              "--mem-enter-opacity": ARCHIVED_MEMORY_OPACITY,
            } as CSSProperties)
          : style
      }
    >
      <div className="memory-list-row-body">
        <div className="memory-list-row-copy">
          <button
            type="button"
            aria-label={t("memoryList.openMemory")}
            className="memory-list-row-content"
            onClick={handleOpen}
          >
            <ContentRenderer
              content={memory.content}
              structuredFields={memory.structured_fields}
              variant="card"
            />
          </button>
          {/* Archive-superseded: kept visible but muted, so say why. Opacity
              alone is overloaded here, so the row needs words too. */}
          {(memory.domain || memory.pinned || memory.is_archived) && (
            <div className="memory-list-row-context">
              {memory.domain && <span>{memory.domain}</span>}
              {memory.pinned && <span>{t("memoryList.pinned")}</span>}
              {memory.is_archived && (
                <span className="memory-list-row-archived">{t("entityDetail.archived")}</span>
              )}
            </div>
          )}
        </div>

        {pendingRevision && (
          <div className="memory-list-row-update">
            <div>
              <span className="memory-list-row-update-label">
                {pendingRevision.source_agent
                  ? t("memoryList.proposedUpdateFrom", { agent: resolveAgentDisplayName(pendingRevision.source_agent) })
                  : t("memoryList.proposedUpdate")}
              </span>
              <p>{pendingRevision.content}</p>
            </div>
            <div className="memory-list-row-update-actions">
              <button type="button" aria-label={t("memoryList.acceptUpdate")} onClick={handleAcceptRevision}>
                {t("memoryList.acceptUpdate")}
              </button>
              <button type="button" aria-label={t("memoryList.dismissUpdate")} onClick={handleDismissRevision}>
                {t("memoryList.dismissUpdate")}
              </button>
            </div>
          </div>
        )}
      </div>

      <div
        ref={actionsRef}
        className="memory-list-row-actions memory-list-reading-actions"
        onBlur={(event) => {
          if (!event.currentTarget.contains(event.relatedTarget)) setMenuOpen(false);
        }}
      >
        <button
          ref={triggerRef}
          type="button"
          className="memory-list-row-menu-trigger"
          aria-label={t("memoryList.actions")}
          aria-haspopup="menu"
          aria-expanded={menuOpen}
          onClick={() => { menuFocus.current = "first"; setMenuOpen((open) => !open); }}
          onKeyDown={(event) => {
            if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
            event.preventDefault();
            event.stopPropagation();
            menuFocus.current = event.key === "ArrowUp" ? "last" : "first";
            setMenuOpen(true);
          }}
        >
          <svg aria-hidden="true" width="18" height="4" viewBox="0 0 18 4" fill="currentColor">
            <circle cx="2" cy="2" r="1.5" /><circle cx="9" cy="2" r="1.5" /><circle cx="16" cy="2" r="1.5" />
          </svg>
        </button>
        {menuOpen && (
          <div ref={menuRef} className="mem-popover-surface memory-list-row-menu" role="menu" aria-label={t("memoryList.actions")} onKeyDown={handleMenuKeyDown}>
            <button type="button" role="menuitem" aria-label={isConfirmed ? t("memoryList.unconfirmMemory") : t("memoryList.confirmMemory")} onClick={() => { onConfirm(memory.source_id, !isConfirmed); closeMenu(); }}>
              {isConfirmed ? t("memoryList.unconfirmMemory") : t("memoryList.confirmMemory")}
            </button>
            {(onPin || onUnpin) && (
              <button type="button" role="menuitem" aria-label={memory.pinned ? t("memoryList.unpinMemory") : t("memoryList.pinMemory")} onClick={() => {
                if (memory.pinned) onUnpin?.(memory.source_id);
                else onPin?.(memory.source_id);
                closeMenu();
              }}>
                {memory.pinned ? t("memoryList.unpinMemory") : t("memoryList.pinMemory")}
              </button>
            )}
            <button type="button" role="menuitem" className="memory-list-row-menu-delete" aria-label={t("memoryList.deleteMemory")} onClick={() => { setDeleting(true); onDelete(memory.source_id); }}>
              {t("memoryList.deleteMemory")}
            </button>
          </div>
        )}
      </div>
    </article>
  );
}
