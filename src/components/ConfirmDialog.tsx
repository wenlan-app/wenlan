// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useId, useRef, type ReactNode } from "react";
import { Button } from "./memory/settings/primitives";
import "./confirmDialog.css";

/**
 * How the confirm button looks. `danger` (the default) is for an action that
 * cannot be undone; `neutral` is for one the person can take back later.
 */
export type ConfirmTone = "danger" | "neutral";

export interface ConfirmDialogProps {
  message: string;
  details?: ReactNode;
  confirmLabel: string;
  cancelLabel?: string;
  tone?: ConfirmTone;
  onAnswer: (confirmed: boolean) => void;
}

// Registered when this module loads, before any screen mounts its own
// capture-phase Escape handler, so an open question sees keys first.
let openDialogKeydown: ((event: KeyboardEvent) => void) | null = null;
if (typeof window !== "undefined") window.addEventListener("keydown", (event) => openDialogKeydown?.(event), true);

/**
 * The app's own "are you sure?" question. The confirm button names the action
 * and starts focused, as a native alert's default button does; Escape, the
 * backdrop and Cancel all answer no.
 */
export function ConfirmDialog({ message, details, confirmLabel, cancelLabel, tone = "danger", onAnswer }: ConfirmDialogProps) {
  const messageId = useId();
  const detailsId = useId();
  const panel = useRef<HTMLDivElement>(null);
  const answer = useRef(onAnswer);
  answer.current = onAnswer;

  useEffect(() => {
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const focusConfirm = () => panel.current?.querySelector<HTMLElement>("[data-confirm-action]")?.focus();
    focusConfirm();
    const keydown = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !event.isComposing) {
        event.preventDefault();
        event.stopImmediatePropagation();
        answer.current(false);
        return;
      }
      if (event.key !== "Tab") return;
      const controls = Array.from(panel.current?.querySelectorAll<HTMLElement>("button, summary, [tabindex='0']") ?? [])
        .filter((control) => control.tagName === "SUMMARY" || !control.closest("details:not([open])"));
      const first = controls[0];
      const last = controls[controls.length - 1];
      if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
    };
    // The question is modal: focus that wanders out comes back to it.
    const focusin = (event: FocusEvent) => {
      if (event.target instanceof Node && !panel.current?.contains(event.target)) focusConfirm();
    };
    openDialogKeydown = keydown;
    document.addEventListener("focusin", focusin);
    return () => {
      if (openDialogKeydown === keydown) openDialogKeydown = null;
      document.removeEventListener("focusin", focusin);
      if (previous?.isConnected) previous.focus();
    };
  }, []);

  return (
    <div className="confirm-dialog-overlay" onClick={(event) => {
      if (event.target === event.currentTarget) onAnswer(false);
    }}>
      <div ref={panel} role="alertdialog" aria-modal="true" aria-labelledby={messageId} aria-describedby={details ? detailsId : undefined}
        className="confirm-dialog">
        <p id={messageId} className="confirm-dialog-message">{message}</p>
        {details && <div id={detailsId} className="confirm-dialog-details">{details}</div>}
        <div className="confirm-dialog-actions">
          {cancelLabel && <Button variant="ghost" onClick={() => onAnswer(false)}>{cancelLabel}</Button>}
          <Button data-confirm-action data-tone={tone} variant={tone === "neutral" ? "primary" : "danger"}
            onClick={() => onAnswer(true)}>{confirmLabel}</Button>
        </div>
      </div>
    </div>
  );
}
