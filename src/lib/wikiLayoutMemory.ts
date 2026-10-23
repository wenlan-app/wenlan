// SPDX-License-Identifier: AGPL-3.0-only
/**
 * Whether the Wiki last opened as a single column (directory hidden).
 *
 * Main decides it from the library and the getting-started checklist, which
 * need the daemon. A workspace that opens before the daemon answers would
 * otherwise show the two-column layout for seconds and then collapse it; this
 * keeps the layout the user saw last time until the real answer arrives.
 */
const KEY = "wiki:directoryHiddenLastTime";

export function readRememberedDirectoryHidden(): boolean {
  try {
    return localStorage.getItem(KEY) === "1";
  } catch {
    return false;
  }
}

export function rememberDirectoryHidden(hidden: boolean): void {
  try {
    if (hidden) localStorage.setItem(KEY, "1");
    else localStorage.removeItem(KEY);
  } catch {
    // Storage can be unavailable; the next launch just starts two-column.
  }
}
