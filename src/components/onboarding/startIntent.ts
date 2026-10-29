// SPDX-License-Identifier: AGPL-3.0-only
/**
 * What a new user picked on the welcome screen, carried into the app they
 * land in: open the folder picker, the past-chats choice, a new page, or
 * (leaving the example wiki to start your own) "Add your material".
 * Taken once, so a later visit to the Wiki opens nothing on its own. An intent
 * that opens a dialog is only looked at until that dialog closes, so a Wiki
 * home that is replaced while the dialog is up opens it again.
 */
export type StartIntent = "folder" | "chats" | "empty" | "material";

const KEY = "onboarding:startIntent";
let pending: StartIntent | null = null;
const listeners = new Set<() => void>();

export function setStartIntent(intent: StartIntent) {
  pending = intent;
  try {
    sessionStorage.setItem(KEY, intent);
  } catch {
    // Storage refused: the in-memory copy still carries it within this window.
  }
  for (const listener of listeners) listener();
}

/** Calls `listener` whenever an intent is set, for a Wiki home that is
 *  already on screen (leaving the example wiki from its home). */
export function onStartIntent(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function peekStartIntent(): StartIntent | null {
  try {
    const stored = sessionStorage.getItem(KEY);
    if (stored === "folder" || stored === "chats" || stored === "empty" || stored === "material") return stored;
  } catch {
    // Storage refused: use the in-memory copy.
  }
  return pending;
}

export function takeStartIntent(): StartIntent | null {
  let intent = pending;
  try {
    const stored = sessionStorage.getItem(KEY);
    if (stored === "folder" || stored === "chats" || stored === "empty" || stored === "material") intent = stored;
    sessionStorage.removeItem(KEY);
  } catch {
    // Storage refused: use the in-memory copy.
  }
  pending = null;
  return intent;
}
