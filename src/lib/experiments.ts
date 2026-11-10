// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useSyncExternalStore } from "react";

/**
 * Experiments: screens still being tried, off until the person turns them on
 * in Settings. Only the screen is switched; what Wenlan does in the
 * background keeps running, so turning one on shows everything at once.
 * The choice is this app's, kept in this window's storage.
 */
export type Experiment = "things";

const KEY_PREFIX = "wenlan-experiment-";
const CHANGED = "wenlan-experiment-changed";

function read(name: Experiment): boolean {
  try {
    return window.localStorage.getItem(KEY_PREFIX + name) === "on";
  } catch {
    return false;
  }
}

export function setExperiment(name: Experiment, on: boolean): void {
  try {
    if (on) window.localStorage.setItem(KEY_PREFIX + name, "on");
    else window.localStorage.removeItem(KEY_PREFIX + name);
  } catch {
    // Storage refused: the switch keeps its old value.
  }
  window.dispatchEvent(new Event(CHANGED));
}

function subscribe(onChange: () => void): () => void {
  window.addEventListener(CHANGED, onChange);
  window.addEventListener("storage", onChange);
  return () => {
    window.removeEventListener(CHANGED, onChange);
    window.removeEventListener("storage", onChange);
  };
}

/** Whether an experiment is on, and a way to switch it. Off by default. */
export function useExperiment(name: Experiment): [boolean, (on: boolean) => void] {
  const on = useSyncExternalStore(subscribe, () => read(name), () => false);
  const set = useCallback((next: boolean) => setExperiment(name, next), [name]);
  return [on, set];
}
