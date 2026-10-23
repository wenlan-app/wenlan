// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect } from "react";
import { emit, listen } from "@tauri-apps/api/event";
import {
  onUpdateAvailable,
  onUpdateProgress,
  onUpdateStatus,
  type UpdateProgressPayload,
  type UpdateStatusPayload,
} from "../lib/appUpdate";

/**
 * Listens to the app updater and keeps the sidebar's service row informed
 * (`src/lib/appUpdate.ts`). Renders nothing: a new version is a lasting
 * notice, said in the row, not a card over the workspace. Mounted on every
 * branch of App so an offer made during setup is waiting once the
 * workspace opens.
 */
export default function UpdateWatcher() {
  useEffect(() => {
    const unlistenAvailable = listen<{ version: string }>("updater://available", (e) => {
      onUpdateAvailable(e.payload.version);
    });
    const unlistenProgress = listen<UpdateProgressPayload>("updater://progress", (e) => {
      onUpdateProgress(e.payload);
    });
    const unlistenStatus = listen<UpdateStatusPayload>("updater://status", (e) => {
      onUpdateStatus(e.payload);
    });
    void Promise.all([unlistenAvailable, unlistenProgress, unlistenStatus])
      .then(() => emit("updater://ui-ready"))
      .catch(() => {});
    return () => {
      unlistenAvailable.then((fn) => fn());
      unlistenProgress.then((fn) => fn());
      unlistenStatus.then((fn) => fn());
    };
  }, []);

  return null;
}
