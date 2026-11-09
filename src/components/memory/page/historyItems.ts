// SPDX-License-Identifier: AGPL-3.0-only
import type { TFunction } from "i18next";
import type { PageChangelogEntry, PageHistoryItem, SharedStatus } from "../../../lib/tauri";
import { sharedWriter } from "../sharing/sharing";
import { revisionWriterLabel } from "./format";

// The writers the daemon records for a person (app editor, vault file edit).
const PERSON_WRITERS = new Set(["manual_edit", "fs_edit", "human"]);

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
    }));
}
