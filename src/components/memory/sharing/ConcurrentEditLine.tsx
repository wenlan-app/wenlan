// SPDX-License-Identifier: AGPL-3.0-only
// The one line a page shows when the person and someone they share it with
// edited it at the same time, and the other edit was kept. It shows once, on
// the computer whose edit lost; that edit stays in the page's history.
import { useTranslation } from "react-i18next";
import type { PageChangelogEntry } from "../../../lib/tauri";
import "./sharing.css";

const SEEN_KEY = "wenlan-shared-conflict-seen";

function seenVersions(): Record<string, number> {
  try {
    const raw = globalThis.localStorage?.getItem(SEEN_KEY);
    const parsed: unknown = raw ? JSON.parse(raw) : {};
    return parsed && typeof parsed === "object" ? parsed as Record<string, number> : {};
  } catch {
    return {};
  }
}

/** The changelog entry of the page's current version when it replaced an
 * edit made here at the same time, unless the person already closed it. */
export function unseenConcurrentEdit(
  pageId: string,
  pageVersion: number,
  entries: readonly PageChangelogEntry[] | undefined,
): PageChangelogEntry | undefined {
  const entry = entries?.find((candidate) => candidate.version === pageVersion);
  if (entry?.concurrent_with == null) return undefined;
  return seenVersions()[pageId] === pageVersion ? undefined : entry;
}

export function markConcurrentEditSeen(pageId: string, version: number) {
  try {
    globalThis.localStorage?.setItem(SEEN_KEY, JSON.stringify({ ...seenVersions(), [pageId]: version }));
  } catch {
    // Without storage the line simply shows again next time.
  }
}

export function ConcurrentEditLine({ name, onSeeMine, onDismiss }: {
  readonly name: string;
  readonly onSeeMine: () => void;
  readonly onDismiss: () => void;
}) {
  const { t } = useTranslation();
  return (
    <p className="shared-with-line shared-conflict-line" role="status">
      <span>{t("sharing.conflictLine", { name })}</span>
      <button type="button" className="share-link" onClick={onSeeMine}>{t("sharing.conflictSeeMine")}</button>
      <button type="button" className="share-link" onClick={onDismiss}>{t("sharing.conflictOk")}</button>
    </p>
  );
}
