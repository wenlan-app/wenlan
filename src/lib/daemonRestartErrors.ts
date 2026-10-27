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
  "daemon-restart:sidecar-still-running": "daemonService.errors.sidecarStopUnconfirmed",
  "daemon-restart:sidecar-stop-unverified": "daemonService.errors.sidecarStopUnconfirmed",
} as const;

export type RestartErrorKey = (typeof RESTART_ERROR_KEYS)[keyof typeof RESTART_ERROR_KEYS];

export function restartErrorKey(raw: string): RestartErrorKey | null {
  const key = Object.keys(RESTART_ERROR_KEYS).find((prefix) => raw === prefix || raw.startsWith(`${prefix}:`));
  return key ? RESTART_ERROR_KEYS[key as keyof typeof RESTART_ERROR_KEYS] : null;
}
