// SPDX-License-Identifier: AGPL-3.0-only
// Quick Capture keeps its unsaved text in this webview's localStorage so a
// restart, a crash or a reload does not lose it. Every access can throw (site
// data blocked, quota), so each call reports whether it worked.

/** The standalone window and the in-app dialog keep separate drafts. */
export function quickCaptureDraftKey(standalone = false): string {
  return `wenlan.quick-capture.draft.v1.${standalone ? "window" : "modal"}`;
}

export function readQuickCaptureDraft(key: string): { content: string; failed: boolean } {
  try {
    return { content: localStorage.getItem(key) ?? "", failed: false };
  } catch {
    return { content: "", failed: true };
  }
}

export function writeQuickCaptureDraft(key: string, content: string): boolean {
  try {
    localStorage.setItem(key, content);
    return true;
  } catch {
    return false;
  }
}

export function clearQuickCaptureDraft(key: string): boolean {
  try {
    localStorage.removeItem(key);
    return true;
  } catch {
    return false;
  }
}
