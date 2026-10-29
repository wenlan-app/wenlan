// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useSyncExternalStore } from "react";

// The getting-started card and the everyday overlay are a new user's one
// teacher. While either is up, the other first-use voices (milestone toasts,
// the first-page dialog) stay quiet, so only one place talks at a time.
const holders = new Set<symbol>();
const listeners = new Set<() => void>();

function notify() {
  listeners.forEach((listener) => listener());
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** Called by the teacher: holds the stage while `active`. */
export function useHoldOnboardingStage(active: boolean) {
  useEffect(() => {
    if (!active) return;
    const holder = Symbol("onboarding-stage");
    holders.add(holder);
    notify();
    return () => {
      holders.delete(holder);
      notify();
    };
  }, [active]);
}

/** Whether the new-user teacher is on screen right now. */
export function useOnboardingOnStage(): boolean {
  return useSyncExternalStore(subscribe, () => holders.size > 0, () => false);
}
