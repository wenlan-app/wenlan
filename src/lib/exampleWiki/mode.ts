// SPDX-License-Identifier: AGPL-3.0-only
/**
 * The example wiki: a separate, built-in wiki a new user can look around in.
 * While it is open, every daemon read is answered from the example's own
 * data and every write is refused, so it never touches the user's library
 * and never reaches what their AI tools can recall.
 */
import { useSyncExternalStore } from "react";

let active = false;
const listeners = new Set<() => void>();

export function exampleWikiActive(): boolean {
  return active;
}

export function setExampleWikiActive(on: boolean) {
  if (active === on) return;
  active = on;
  listeners.forEach((listener) => listener());
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function useExampleWikiActive(): boolean {
  return useSyncExternalStore(subscribe, exampleWikiActive, () => false);
}
