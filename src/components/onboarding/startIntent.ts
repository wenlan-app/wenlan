// SPDX-License-Identifier: AGPL-3.0-only
/**
 * What a new user picked on the welcome screen, carried into the app they
 * land in: open the folder picker, the past-chats choice, or a new page.
 * Taken once, so a later visit to the Wiki opens nothing on its own.
 */
export type StartIntent = "folder" | "chats" | "empty";

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
    if (stored === "folder" || stored === "chats" || stored === "empty") intent = stored;
    sessionStorage.removeItem(KEY);
  } catch {
    // Storage refused: use the in-memory copy.
  }
  pending = null;
  return intent;
}
