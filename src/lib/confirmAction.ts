// SPDX-License-Identifier: AGPL-3.0-only
import { createElement } from "react";
import { createRoot } from "react-dom/client";
import { ConfirmDialog } from "../components/ConfirmDialog";

export interface ConfirmActionLabels {
  /** The button that goes ahead, named for the action ("Delete page"). */
  confirm: string;
  cancel: string;
}

type ConfirmPresenter = (message: string, labels: ConfirmActionLabels) => Promise<boolean>;

let closeOpenDialog: (() => void) | null = null;

/** Shows the app's own confirm dialog in a root of its own, then removes it. */
const showConfirmDialog: ConfirmPresenter = (message, labels) => new Promise((resolve) => {
  // One question at a time: a newer one answers the older with no.
  closeOpenDialog?.();
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  let settled = false;
  const answer = (confirmed: boolean) => {
    if (settled) return;
    settled = true;
    if (closeOpenDialog === close) closeOpenDialog = null;
    root.unmount();
    host.remove();
    resolve(confirmed);
  };
  const close = () => answer(false);
  closeOpenDialog = close;
  root.render(createElement(ConfirmDialog, {
    message,
    confirmLabel: labels.confirm,
    cancelLabel: labels.cancel,
    onAnswer: answer,
  }));
});

let presenter: ConfirmPresenter = showConfirmDialog;

/**
 * Ask before an action that throws work away. Resolves true only when the
 * person chose the confirm button.
 *
 * Never call `window.confirm` in the app: the macOS webview has no handler for
 * it, so it returns false at once without showing anything and the action
 * silently does nothing.
 */
export function confirmAction(message: string, labels: ConfirmActionLabels): Promise<boolean> {
  return presenter(message, labels);
}

/** Unit tests answer through a stand-in; pass nothing to restore the dialog. */
export function setConfirmPresenter(next?: ConfirmPresenter) {
  presenter = next ?? showConfirmDialog;
}
