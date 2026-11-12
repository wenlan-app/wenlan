// SPDX-License-Identifier: AGPL-3.0-only

/**
 * The one gate for page looks (UX-LOOK-1..6). Everything that shows a look,
 * its switch or its menu items asks here, so the feature can be put behind
 * Settings › Experiments (`useExperiment`, localStorage `wenlan-experiment-*`,
 * see `src/lib/experiments.ts`) by changing this function alone.
 */
export const PAGE_LOOKS_ENABLED = true;

export function pageLooksEnabled(): boolean {
  return PAGE_LOOKS_ENABLED;
}
