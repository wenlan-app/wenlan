// SPDX-License-Identifier: AGPL-3.0-only

declare const __WENLAN_WEB__: boolean;

/** True only for the daemon-served browser bundle, never for Tauri or preview. */
export function isWebBuild(): boolean {
  return typeof __WENLAN_WEB__ !== "undefined" && __WENLAN_WEB__;
}
