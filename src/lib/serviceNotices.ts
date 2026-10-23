// SPDX-License-Identifier: AGPL-3.0-only
/**
 * What the background service needs the user to know, said in one place.
 *
 * The sidebar's service row (`src/components/memory/ServiceStatus.tsx`) is the
 * only surface that speaks about the service in the workspace: while it
 * starts, when it does not answer, when it runs an older version than the
 * app, and when Rust could not put it under launchd. The sources stay where
 * the events arrive (`src/App.tsx` for the fallback event,
 * `src/components/DaemonVersionWatcher.tsx` for version reports) and write
 * here, so a report that lands before the sidebar mounts is not lost.
 */
import { useSyncExternalStore } from "react";
import type { DaemonVersionEvent, DaemonVersionStatus } from "./tauri";
import type { DaemonStartupPhase } from "./daemonStartup";

export type VersionReport = DaemonVersionStatus | DaemonVersionEvent;

export interface ServiceNotices {
  /** A daemon that answered with a version other than the app's. */
  readonly version: VersionReport | null;
  /** Rust runs without a managed service (`origin-fallback-mode`). */
  readonly fallback: boolean;
}

let notices: ServiceNotices = { version: null, fallback: false };
const listeners = new Set<() => void>();

function update(next: Partial<ServiceNotices>): void {
  notices = { ...notices, ...next };
  for (const listener of listeners) listener();
}

/** Keep a report only when it names a real mismatch. A report sent while
 *  nothing answered (a restart pressed with the daemon down) has no daemon
 *  version and says nothing about which version will answer. */
export function setVersionReport(report: VersionReport | null): void {
  update({ version: report && !report.matched && report.daemon ? report : null });
}

export function setServiceFallback(fallback: boolean): void {
  update({ fallback });
}

export function getServiceNotices(): ServiceNotices {
  return notices;
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}

export function useServiceNotices(): ServiceNotices {
  return useSyncExternalStore(subscribe, getServiceNotices, getServiceNotices);
}

export type ServiceStatusKind = "starting" | "notAnswering" | "outOfDate" | "notRunning";

/** One thing at a time, the one that matters most first. */
export function serviceStatusKind(
  phase: DaemonStartupPhase,
  { version, fallback }: ServiceNotices,
): ServiceStatusKind | null {
  if (phase === "failed") return "notAnswering";
  // While the workspace waits, nothing else is known yet: an old report or a
  // fallback said before the daemon answered may be about to change.
  if (phase === "starting") return "starting";
  if (version) return "outOfDate";
  if (fallback) return "notRunning";
  return null;
}
