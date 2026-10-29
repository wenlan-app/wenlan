// SPDX-License-Identifier: AGPL-3.0-only
/**
 * What a new user picked on the welcome screen, carried into the app they
 * land in: open the folder picker, the past-chats choice, a new page, or
 * (leaving the example wiki to start your own) "Add your material".
 * Taken once, so a later visit to the Wiki opens nothing on its own.
 */
export type StartIntent = "folder" | "chats" | "empty" | "material";

const KEY = "onboarding:startIntent";
let pending: StartIntent | null = null;

export function setStartIntent(intent: StartIntent) {
  pending = intent;
  try {
    sessionStorage.setItem(KEY, intent);
  } catch {
    // Storage refused: the in-memory copy still carries it within this window.
  }
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
