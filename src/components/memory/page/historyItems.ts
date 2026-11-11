// SPDX-License-Identifier: AGPL-3.0-only
import type { TFunction } from "i18next";
import type { PageChangelogEntry, PageHistoryItem, SharedStatus } from "../../../lib/tauri";
import { sharedWriter } from "../sharing/sharing";
import { revisionWriterLabel } from "./format";

// The writers the daemon records for a person (app editor, vault file edit).
const PERSON_WRITERS = new Set(["manual_edit", "fs_edit", "human", "own_device"]);

/** Who or what made one change, in the person's words. A change taken
 * from a shared folder names the member who made it. */
export function historyItemLabel(
  t: TFunction,
  item: PageHistoryItem,
  shares?: readonly SharedStatus[],
): string {
  if (item.kind === "source_update") {
    return t("pageHistory.sourceUpdate", { title: item.source?.title || t("pageHistory.untitledSource") });
  }
  if (item.restored_from != null) {
    return t("pageHistory.restored", { version: item.restored_from });
  }
  return sharedWriter(item.edited_by, shares, t) ?? revisionWriterLabel(t, item.edited_by ?? "", item.agent);
}

/**
 * The rows of the page's one history (UX-HIST-1), newest first. Uses the
 * merged history when the daemon serves it; an older daemon only has the
 * changelog, which becomes edit rows.
 */
export function historyRows(
  history: readonly PageHistoryItem[] | undefined,
  entries: readonly PageChangelogEntry[],
): PageHistoryItem[] {
  if (history && history.length > 0) {
    return [...history].sort((a, b) => b.at - a.at || (b.version ?? 0) - (a.version ?? 0));
  }
  return [...entries]
    .sort((a, b) => b.version - a.version)
    .map((entry) => ({
      kind: PERSON_WRITERS.has(entry.edited_by) ? "person_edit" as const : "ai_edit" as const,
      at: entry.at,
      version: entry.version,
      edited_by: entry.edited_by,
      agent: entry.agent ?? null,
      delta_summary: entry.delta_summary ?? null,
      person_asked: entry.person_asked ?? null,
      restored_from: entry.restored_from ?? null,
    }));
}

/**
 * The changelog entry a history row summarizes. A run of saves listed as one
 * change (UX-HIST-3) adds up the run's entries, so its summary covers every
 * save in it, not only the last one.
 */
export function entryForItem(
  item: PageHistoryItem,
  entryByVersion: ReadonlyMap<number, PageChangelogEntry>,
): PageChangelogEntry | undefined {
  if (item.kind === "source_update" || item.version == null) return undefined;
  const last = entryByVersion.get(item.version);
  const first = item.first_version;
  if (!last || first == null || first >= item.version) return last;
  const run: PageChangelogEntry[] = [];
  for (let version = first; version <= item.version; version += 1) {
    const entry = entryByVersion.get(version);
    if (entry) run.push(entry);
  }
  const sum = (pick: (entry: PageChangelogEntry) => number | null | undefined) =>
    run.some((entry) => pick(entry) != null)
      ? run.reduce((total, entry) => total + (pick(entry) ?? 0), 0)
      : null;
  const incoming = [...new Set(run.flatMap((entry) => entry.incoming_source_ids ?? []))];
  return {
    ...last,
    delta_summary: null,
    char_delta: sum((entry) => entry.char_delta),
    sources_added: sum((entry) => entry.sources_added),
    sources_removed: sum((entry) => entry.sources_removed),
    incoming_source_ids: incoming.length > 0 ? incoming : null,
  };
}
