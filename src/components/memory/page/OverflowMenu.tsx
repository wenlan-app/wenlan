// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useRef, useState } from "react";
import "./OverflowMenu.css";

export interface OverflowMenuItem {
  key: string;
  label: string;
  /** A quieter second line under the label. */
  hint?: string;
  disabled?: boolean;
  onSelect: () => void;
}

interface OverflowMenuProps {
  /** The accessible name of the "⋯" trigger. */
  label: string;
  items: readonly OverflowMenuItem[];
  disabled?: boolean;
  className?: string;
}

/**
 * A "⋯" button that opens a short list of actions. Rarely used actions live
 * behind it so the common ones stay visible next to their row.
 */
export function OverflowMenu({ label, items, disabled = false, className }: OverflowMenuProps) {
  const [open, setOpen] = useState(false);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const isInside = (target: EventTarget | null) => target instanceof Node
      && (triggerRef.current?.contains(target) || menuRef.current?.contains(target));
    const closeOutside = (event: Event) => {
      if (!isInside(event.target)) setOpen(false);
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      setOpen(false);
      triggerRef.current?.focus();
    };
    document.addEventListener("pointerdown", closeOutside, true);
    document.addEventListener("focusin", closeOutside);
    document.addEventListener("keydown", closeOnEscape, true);
    return () => {
      document.removeEventListener("pointerdown", closeOutside, true);
      document.removeEventListener("focusin", closeOutside);
      document.removeEventListener("keydown", closeOnEscape, true);
    };
  }, [open]);

  useEffect(() => {
    if (open) menuRef.current?.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus();
  }, [open]);

  const moveFocus = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
    event.preventDefault();
    const buttons = Array.from(menuRef.current?.querySelectorAll<HTMLButtonElement>("button:not(:disabled)") ?? []);
    if (buttons.length === 0) return;
    const current = buttons.indexOf(document.activeElement as HTMLButtonElement);
    const next = event.key === "ArrowDown" ? current + 1 : current - 1;
    buttons[(next + buttons.length) % buttons.length].focus();
  };

  if (items.length === 0) return null;

  return <span className={`overflow-menu${className ? ` ${className}` : ""}`}>
    <button
      ref={triggerRef}
      type="button"
      className="mem-icon-action overflow-menu__trigger"
      aria-label={label}
      aria-haspopup="menu"
      aria-expanded={open}
      disabled={disabled}
      onClick={(event) => {
        event.stopPropagation();
        setOpen((value) => !value);
      }}
    >
      <svg aria-hidden="true" width="16" height="4" viewBox="0 0 16 4" fill="currentColor">
        <circle cx="2" cy="2" r="1.5" /><circle cx="8" cy="2" r="1.5" /><circle cx="14" cy="2" r="1.5" />
      </svg>
    </button>
    {open && <div
      ref={menuRef}
      className="mem-popover-surface overflow-menu__list"
      role="menu"
      aria-label={label}
      onKeyDown={moveFocus}
    >
      {items.map((item) => <button
        key={item.key}
        type="button"
        role="menuitem"
        className="overflow-menu__item"
        disabled={item.disabled}
        onClick={(event) => {
          event.stopPropagation();
          setOpen(false);
          // Hand focus back before the action, which may open a dialog or remove this row.
          triggerRef.current?.focus();
          item.onSelect();
        }}
      >
        <span>{item.label}</span>
        {item.hint && <span className="overflow-menu__hint">{item.hint}</span>}
      </button>)}
    </div>}
  </span>;
}

export default OverflowMenu;
