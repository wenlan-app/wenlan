// SPDX-License-Identifier: AGPL-3.0-only
import { isInputCompositionActive } from "./inputComposition";
export const EDITOR_LINK_REQUEST = "wenlan-editor-link-request";

/** Route a native shortcut to the focused shared editor before global search. */
export function requestFocusedMarkdownLink(): boolean {
  if (isInputCompositionActive()) return true;
  const active = document.activeElement;
  if (!(active instanceof HTMLElement) || !active.closest('[data-markdown-editor-engine="codemirror"]')) return false;
  return !active.dispatchEvent(new Event(EDITOR_LINK_REQUEST, { bubbles: true, cancelable: true }));
}
