// SPDX-License-Identifier: AGPL-3.0-only
import type { TFunction } from "i18next";
import type { PageChangelogEntry, PageHistoryItem } from "../../../lib/tauri";
import { revisionWriterLabel } from "./format";

// The writers the daemon records for a person (app editor, vault file edit).
const PERSON_WRITERS = new Set(["manual_edit", "fs_edit", "human"]);

/** Who or what made one change, in the person's words. */
export function historyItemLabel(t: TFunction, item: PageHistoryItem): string {
  if (item.kind === "source_update") {
    return t("pageHistory.sourceUpdate", { title: item.source?.title || t("pageHistory.untitledSource") });
  }
  return revisionWriterLabel(t, item.edited_by ?? "", item.agent);
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
