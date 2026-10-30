// SPDX-License-Identifier: AGPL-3.0-only
/** Separate windows must not overwrite each other's unsaved capture. */
export function quickCaptureDraftKey(standalone = false): string {
  return `wenlan.quick-capture.draft.v1.${standalone ? "window" : "modal"}`;
}
export function readQuickCaptureDraft(key: string): { content: string; failed: boolean } {
  try { return { content: localStorage.getItem(key) ?? "", failed: false }; }
  catch { return { content: "", failed: true }; }
}
export function writeQuickCaptureDraft(key: string, content: string): boolean {
  try { localStorage.setItem(key, content); return true; }
  catch { return false; }
}
export function clearQuickCaptureDraft(key: string): boolean {
  try { localStorage.removeItem(key); return true; }
  catch { return false; }
}
