// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { fillUnsetPinsWithRetry, type PinFill } from "./routingPins";
import { shouldShowWizard } from "./tauri";
import { isWebBuild } from "../web/runtime";

/** True once this launch has started a fill, so a remount does not start another. */
let launchFillStarted = false;

/** Forget that this launch already filled. Tests only. */
export function resetLaunchPinFillForTest(): void {
  launchFillStarted = false;
}

/**
 * Fill the unpinned routing jobs once per app launch.
 *
 * Every other fill happens in the webview at the moment a source is turned on:
 * the wizard's model row, the wizard's Done step, and the Settings provider
 * saves. Each retries in JS, and JS does not survive a quit or a reload. So a
 * user who quits during the multi-minute model download comes back to a loaded
 * model, memories saving, and both jobs still unpinned. Nothing compiles, and
 * no later event tries again. This is the retry that outlives that quit.
 *
 * Two conditions, and both are enforced without a second routing read.
 * Setup complete is checked here, through the same command `App` gates the
 * wizard on, so the two cannot disagree about whose screen this is. "A loaded
 * on-device model or a configured provider" is what `pinsToFill` derives from:
 * with no source in the pool it returns nothing to write, and an on-device
 * model that is selected but not loaded does not count.
 *
 * It never runs while the wizard is showing. `App` renders the wizard instead
 * of `Main`, not beside it, so the hook is not mounted then, and the check
 * below covers the fail-closed path where the wizard is shown because the
 * daemon could not be reached at all.
 *
 * Failures are logged, never surfaced: the user did not ask for this write.
 */
export async function fillUnsetPinsAtLaunch(): Promise<PinFill | null> {
  // This repairs desktop setup state. The browser bundle has no setup wizard
  // and must not probe its native-only completion command during startup.
  if (isWebBuild()) return null;
  if (launchFillStarted) return null;
  // Latched before the first await so two mounts in the same tick (React's
  // development double-effect) cannot both get through.
  launchFillStarted = true;
  try {
    if (await shouldShowWizard()) {
      launchFillStarted = false;
      return null;
    }
  } catch (e) {
    // The daemon could not answer. Unlatch so a later mount can try again.
    launchFillStarted = false;
    console.error("routing: could not read setup status before the launch pin fill", e);
    return null;
  }
  const filled = await fillUnsetPinsWithRetry();
  if (filled === null) {
    // Nothing was filled, because every attempt failed or this daemon has no
    // routing endpoint. Release the latch: a daemon that happens to be down
    // during these few seconds would otherwise disable the fill for the whole
    // session, which is the stranded state this exists to prevent. On a daemon
    // with no routing endpoint the only cost is one more routing read if the
    // shell ever remounts.
    launchFillStarted = false;
  }
  return filled;
}

/** Mount-once wrapper for `fillUnsetPinsAtLaunch`.
 *
 *  Invalidates the routing query when the fill actually wrote a pin. Home reads
 *  routing to decide between "pages are coming" and "choose a model", and its
 *  read happens at mount, before this write lands. Without the invalidation the
 *  user sits looking at "choose a model" until something else refetches, which
 *  is the exact first paint this fill exists to fix. Nothing written means
 *  nothing went stale, so that case skips the refetch. */
export function useLaunchPinFill(): void {
  const queryClient = useQueryClient();
  useEffect(() => {
    void fillUnsetPinsAtLaunch().then((filled) => {
      if (filled && (filled.written.everyday !== null || filled.written.synthesis !== null)) {
        void queryClient.invalidateQueries({ queryKey: ["resolvedRouting"] });
      }
    });
  }, [queryClient]);
}
