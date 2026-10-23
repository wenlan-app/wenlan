// SPDX-License-Identifier: AGPL-3.0-only
/**
 * The app updater, as the account entry sees it.
 *
 * `app/src/updater.rs` offers a new version and then waits for an answer.
 * `src/components/UpdateWatcher.tsx` turns its events into the
 * `update` notice in `src/lib/serviceNotices.ts`. The account entry
 * (`src/components/memory/IdentityCard.tsx`) offers the install, shows its
 * progress, and offers to try again, with the actions here.
 *
 * Events in:
 *   - `updater://available`  payload `{ version }`: a version is on offer
 *   - `updater://progress`   payload `{ chunk, total, error }`
 *   - `updater://status`     payload `{ state, version?, error? }`
 * Events out:
 *   - `updater://ui-ready`   ask the backend to replay a pending offer
 *   - `updater://action`     payload `"install"` (the offer stays open
 *                            until then; "later" is never sent)
 *   - `updater://check-now`  after a failed install, to try again
 */
import { emit } from "@tauri-apps/api/event";
import { getServiceNotices, setUpdateNotice, type UpdateNotice } from "./serviceNotices";

export interface UpdateProgressPayload {
  chunk?: number;
  total?: number | null;
  done?: boolean;
  error?: string;
}

export interface UpdateStatusPayload {
  state: "checking" | "current" | "available" | "error";
  version?: string;
  error?: string;
}

// Bytes so far for the install in progress; the notice keeps only the share.
let downloaded = 0;
let total: number | null = null;
// Set by "Try again": the check it starts offers the version once more, and
// that offer is accepted without asking a second time.
let retrying = false;

function current(): UpdateNotice | null {
  return getServiceNotices().update;
}

function installing(version: string): void {
  downloaded = 0;
  total = null;
  setUpdateNotice({ phase: "installing", version, pct: null });
}

export function onUpdateAvailable(version: string): void {
  if (retrying && current()?.phase === "installing") {
    retrying = false;
    installing(version);
    void emit("updater://action", "install").catch(() => {
      setUpdateNotice({ phase: "failed", version, pct: null });
    });
    return;
  }
  retrying = false;
  setUpdateNotice({ phase: "available", version, pct: null });
}

export function onUpdateProgress(progress: UpdateProgressPayload): void {
  const update = current();
  if (update?.phase !== "installing") return;
  if (progress.error) {
    setUpdateNotice({ ...update, phase: "failed", pct: null });
    return;
  }
  if (progress.chunk) downloaded += progress.chunk;
  if (progress.total !== undefined && progress.total !== null) total = progress.total;
  const pct = total && total > 0 ? Math.min(100, Math.round((downloaded / total) * 100)) : null;
  if (pct !== update.pct) setUpdateNotice({ ...update, pct });
}

/** Status reports matter only once a version was offered: a scheduled check
 *  that fails in the background is not something to show. */
export function onUpdateStatus(status: UpdateStatusPayload): void {
  const update = current();
  if (!update) return;
  if (status.state === "current") {
    retrying = false;
    setUpdateNotice(null);
  } else if (status.state === "error" && update.phase === "installing") {
    retrying = false;
    setUpdateNotice({ ...update, phase: "failed", pct: null });
  }
}

/** Download, install, and reopen. The app restarts when it is done. */
export async function installUpdate(): Promise<void> {
  const update = current();
  if (update?.phase !== "available") return;
  installing(update.version);
  try {
    await emit("updater://action", "install");
  } catch (error) {
    setUpdateNotice(update);
    throw error;
  }
}

/** After a failed install: check again and install what it offers. */
export async function retryUpdate(): Promise<void> {
  const update = current();
  if (update?.phase !== "failed") return;
  installing(update.version);
  retrying = true;
  try {
    await emit("updater://check-now");
  } catch (error) {
    retrying = false;
    setUpdateNotice(update);
    throw error;
  }
}
