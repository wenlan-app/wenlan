// SPDX-License-Identifier: AGPL-3.0-only

/** Reload the app window, as Cmd/Ctrl+R does in other web-view desktop apps. */
export function reloadWindow(): void {
  window.location.reload();
}
