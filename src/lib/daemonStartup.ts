// SPDX-License-Identifier: AGPL-3.0-only
/**
 * Whether the daemon has answered yet, for a window that opened before it did.
 *
 * A configured user's workspace opens at launch instead of waiting behind a
 * full-screen "starting" panel (`src/App.tsx`). Until the daemon answers the
 * first-run gate, every IPC call that goes to the daemon waits here rather
 * than failing on a connection the daemon has not opened yet. A waiting call
 * keeps its query in the loading state, so lists show their placeholders, not
 * an error and not an empty library, and a write is sent once instead of being
 * retried into a daemon that may already have received it.
 *
 * "failed" lets every waiting call through so the usual error states appear,
 * and the sidebar's service row (src/components/memory/ServiceStatus.tsx)
 * says why, instead of every surface.
 */
import { useSyncExternalStore } from "react";

export type DaemonStartupPhase = "ready" | "starting" | "failed";

/**
 * Commands that must reach Rust while the daemon is still starting: the gate
 * itself, the reads and actions the service row uses, and the window and
 * quit plumbing that never touches the daemon.
 */
const ANSWERS_WITHOUT_DAEMON = new Set<string>([
  "should_show_wizard",
  "local_setup_completed",
  "wire_state",
  "get_startup_preflight_status",
  "start_daemon_sidecar",
  "daemon_version_status",
  "restart_daemon",
  "set_traffic_lights_visible",
  "acknowledge_guarded_quit_request",
  "cancel_guarded_quit_request",
  "quit_wenlan_full",
]);

let phase: DaemonStartupPhase = "ready";
let release: (() => void) | null = null;
let held: Promise<void> | null = null;
const listeners = new Set<() => void>();

export function setDaemonStartupPhase(next: DaemonStartupPhase): void {
  if (next === phase) return;
  phase = next;
  if (next === "starting") {
    if (!held) held = new Promise<void>((resolve) => { release = resolve; });
  } else {
    release?.();
    release = null;
    held = null;
  }
  for (const listener of listeners) listener();
}

export function getDaemonStartupPhase(): DaemonStartupPhase {
  return phase;
}

/** What a call to `command` must wait for first, or null to send it now. */
export function daemonStartupWait(command: string): Promise<void> | null {
  if (!held || ANSWERS_WITHOUT_DAEMON.has(command)) return null;
  return held;
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}

export function useDaemonStartupPhase(): DaemonStartupPhase {
  return useSyncExternalStore(subscribe, getDaemonStartupPhase, getDaemonStartupPhase);
}
