import { useEffect, useRef, useState } from "react";
import type { Space } from "../../../lib/tauri";
import type { SpacesOverviewLabels } from "./spacesTypes";

type SpaceActionsMenuProps = {
  readonly space: Space;
  readonly labels: SpacesOverviewLabels;
  readonly pending: boolean;
  readonly onStar: (space: Space) => void;
  /** Starts the inline rename editor. */
  readonly onRename: (space: Space) => void;
  /** Asks whether to delete the Space. */
  readonly onDelete: (space: Space) => void;
};

/**
 * The inventory actions trigger + popover shared by the rows and cards
 * lenses. The parent keeps its `spaces-menu-anchor` wrapper so the popover
 * positions the same way in both lenses.
 */
export function SpaceActionsMenu(props: SpaceActionsMenuProps) {
  const [menuOpen, setMenuOpen] = useState(false);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!menuOpen) return;
    let pointerInside = false;
    const isInside = (target: EventTarget | null) => target instanceof Node
      && (triggerRef.current?.contains(target) || menuRef.current?.contains(target));
    const closeOutside = (event: Event) => {
      if (event.type === "pointerdown") pointerInside = Boolean(isInside(event.target));
      if (!isInside(event.target)) setMenuOpen(false);
    };
    const closeOnFocusOut = (event: FocusEvent) => {
      // WebKit can send focus to browser chrome, with no subsequent focusin.
      // Its pointer clicks can also blur a button before the menu action fires.
      if (!pointerInside && isInside(event.target) && !isInside(event.relatedTarget)) setMenuOpen(false);
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      pointerInside = false;
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      setMenuOpen(false);
      triggerRef.current?.focus();
    };
    // Capture outside presses even when another control stops propagation.
    document.addEventListener("pointerdown", closeOutside, true);
    document.addEventListener("focusin", closeOutside);
    document.addEventListener("focusout", closeOnFocusOut);
    document.addEventListener("keydown", closeOnEscape, true);
    return () => {
      document.removeEventListener("pointerdown", closeOutside, true);
      document.removeEventListener("focusin", closeOutside);
      document.removeEventListener("focusout", closeOnFocusOut);
      document.removeEventListener("keydown", closeOnEscape, true);
    };
  }, [menuOpen]);

  return (
    <>
      <button
        ref={triggerRef}
        type="button"
        className="mem-icon-action spaces-menu-trigger"
        aria-label={props.labels.actionsFor(props.space.name)}
        aria-haspopup="menu"
        aria-expanded={menuOpen}
        disabled={props.pending}
        onClick={() => setMenuOpen((open) => !open)}
      >
        <svg aria-hidden="true" width="16" height="4" viewBox="0 0 16 4" fill="currentColor">
          <circle cx="2" cy="2" r="1.5" /><circle cx="8" cy="2" r="1.5" /><circle cx="14" cy="2" r="1.5" />
        </svg>
      </button>
      {menuOpen ? (
        <div ref={menuRef} className="mem-popover-surface spaces-menu" role="menu">
          <button type="button" role="menuitem" onClick={() => { props.onStar(props.space); setMenuOpen(false); }}>
            {props.space.starred ? props.labels.unstar : props.labels.star}
          </button>
          <button type="button" role="menuitem" onClick={() => { props.onRename(props.space); setMenuOpen(false); }}>{props.labels.rename}</button>
          <button type="button" role="menuitem" className="spaces-danger" onClick={() => { props.onDelete(props.space); setMenuOpen(false); }}>
            {props.labels.delete}
          </button>
        </div>
      ) : null}
    </>
  );
}
