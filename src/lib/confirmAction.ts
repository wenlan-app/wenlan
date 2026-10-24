// SPDX-License-Identifier: AGPL-3.0-only
import { ask } from "@tauri-apps/plugin-dialog";
import { hostPlatform } from "./windowChrome";

export interface ConfirmActionLabels {
  /** The button that goes ahead, named for the action ("Delete page"). */
  confirm: string;
  cancel: string;
}

/**
 * Ask before an action that throws work away. Resolves true only when the
 * person chose the confirm button.
 *
 * Never call `window.confirm` in the app: the macOS webview has no handler for
 * it, so it returns false at once without showing anything and the action
 * silently does nothing. Inside the app this asks through the native dialog
 * instead. A plain browser (preview, unit tests) keeps `window.confirm`.
 */
export async function confirmAction(message: string, labels: ConfirmActionLabels): Promise<boolean> {
  if (!("__TAURI_INTERNALS__" in window)) return window.confirm(message);
  // A macOS alert leads with its bold line, so the question goes there. Other
  // platforms show the title as the window caption and the message as the body.
  const options = { okLabel: labels.confirm, cancelLabel: labels.cancel };
  try {
    return hostPlatform() === "macos"
      ? await ask("", { ...options, title: message })
      : await ask(message, options);
  } catch (cause) {
    console.warn("[confirm] dialog failed", cause);
    return false;
  }
}
