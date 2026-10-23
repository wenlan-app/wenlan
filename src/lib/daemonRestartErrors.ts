// SPDX-License-Identifier: AGPL-3.0-only
/**
 * Typed errors `restart_daemon` rejects with (app/src/daemon_start.rs
 * `RESTART_ERROR_*`), mapped to localized copy. Anything else is a CLI
 * failure and renders verbatim. Every surface that offers "Restart service"
 * reads its failure through here, so they say the same thing.
 */
const RESTART_ERROR_KEYS = {
  "daemon-restart:not-owned": "daemonService.errors.notOwned",
  "daemon-restart:isolated": "daemonService.errors.isolated",
  "daemon-restart:still-mismatched": "daemonService.errors.stillMismatched",
} as const;

export type RestartErrorKey = (typeof RESTART_ERROR_KEYS)[keyof typeof RESTART_ERROR_KEYS];

export function restartErrorKey(raw: string): RestartErrorKey | null {
  return Object.prototype.hasOwnProperty.call(RESTART_ERROR_KEYS, raw)
    ? RESTART_ERROR_KEYS[raw as keyof typeof RESTART_ERROR_KEYS]
    : null;
}
