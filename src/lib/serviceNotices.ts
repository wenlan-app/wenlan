// SPDX-License-Identifier: AGPL-3.0-only
/**
 * What the background service needs the user to know, said in one place.
 *
 * The sidebar's Activity slot (`src/components/memory/ServiceStatus.tsx`) is
 * the only surface that speaks about the service in the workspace: while it
 * starts, when it does not answer, when it runs an older version than the
 * app, and when Rust could not put it under launchd. An app update, from the
 * offer through installing to a failure, is on the account entry
 * (`src/components/memory/IdentityCard.tsx`), which reads it from here too.
 * The sources stay where the events arrive
 * (`src/App.tsx` for the fallback event,
 * `src/components/DaemonVersionWatcher.tsx` for version reports,
 * `src/components/UpdateWatcher.tsx` for the updater) and write here, so a
 * report that lands before the sidebar mounts is not lost.
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
  /** A newer app version the updater offered, from the offer until it is
   *  installed, put off, or found to be current after all. */
  readonly update: UpdateNotice | null;
}

export interface UpdateNotice {
  readonly phase: "available" | "installing" | "failed";
  readonly version: string;
  /** Download progress while installing, when the size is known. */
  readonly pct: number | null;
}

let notices: ServiceNotices = { version: null, fallback: false, update: null };
const listeners = new Set<() => void>();

function publish(next: Partial<ServiceNotices>): void {
  notices = { ...notices, ...next };
  for (const listener of listeners) listener();
}

/** Keep a report only when it names a real mismatch. A report sent while
 *  nothing answered (a restart pressed with the daemon down) has no daemon
 *  version and says nothing about which version will answer. */
export function setVersionReport(report: VersionReport | null): void {
  publish({ version: report && !report.matched && report.daemon ? report : null });
}

export function setServiceFallback(fallback: boolean): void {
  publish({ fallback });
}

export function setUpdateNotice(update: UpdateNotice | null): void {
  publish({ update });
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

export type ServiceStatusKind =
  | "starting"
  | "notAnswering"
  | "outOfDate"
  | "notRunning";

/** One thing at a time, the one that matters most first. Null leaves the
 *  slot to Activity. */
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
