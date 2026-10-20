// SPDX-License-Identifier: AGPL-3.0-only
// Shared display helpers for the page-detail subcomponents.
import type { TFunction } from "i18next";
import { agentDisplayName, type MemoryItem, type PageChangelogEntry } from "../../../lib/tauri";

export function prettyAgent(name: string | null | undefined): string {
  return agentDisplayName(name) ?? "unknown agent";
}

const WRITER_LABEL_KEYS = {
  manual_edit: "revisionLabel.writer.manualEdit",
  fs_edit: "revisionLabel.writer.fsEdit",
  distill: "revisionLabel.writer.distill",
  re_distill: "revisionLabel.writer.reDistill",
  page_growth: "revisionLabel.writer.pageGrowth",
  refinery_merge: "revisionLabel.writer.refineryMerge",
  revision_accept: "revisionLabel.writer.revisionAccept",
  citation_backfill: "revisionLabel.writer.citationBackfill",
} as const;

/** Who made a page revision, in words the reader can use. Never the raw
 * `edited_by` slug: an unrecognized writer reads as a plain "Updated". */
export function revisionWriterLabel(
  t: TFunction,
  editedBy: string,
  agent?: string | null,
): string {
  if (editedBy === "agent_refresh") {
    const name = agentDisplayName(agent?.trim());
    return name
      ? t("revisionLabel.writer.agentRefreshNamed", { agent: name })
      : t("revisionLabel.writer.agentRefresh");
  }
  if (Object.prototype.hasOwnProperty.call(WRITER_LABEL_KEYS, editedBy)) {
    return t(WRITER_LABEL_KEYS[editedBy as keyof typeof WRITER_LABEL_KEYS]);
  }
  return t("revisionLabel.writer.other");
}

/** What a note is built on, e.g. "3 source memories · 2 notes". A zero
 * memory count is left out rather than shown as "0 source memories". */
export function basisCountLabel(t: TFunction, sourceCount: number, noteCount: number): string {
  const notes = t("noteBasis.sourceNotesCount", { count: noteCount });
  return sourceCount > 0 ? `${t("noteBasis.sourceMemoriesCount", { count: sourceCount })} · ${notes}` : notes;
}

/** One-line summary of what a revision changed. Entries with numeric deltas
 * are localized; older entries only have the daemon's English `delta_summary`,
 * which is shown for an English UI and left out otherwise. */
export function revisionSummary(
  t: TFunction,
  entry: PageChangelogEntry,
  language: string | undefined,
): string | null {
  const { char_delta: chars, sources_added: added, sources_removed: removed } = entry;
  if (chars == null && added == null && removed == null) {
    return language?.startsWith("en") ? (entry.delta_summary ?? null) : null;
  }
  const parts: string[] = [];
  if (chars) {
    parts.push(t("revisionLabel.summary.chars", { count: Math.abs(chars), sign: chars < 0 ? "-" : "+" }));
  }
  if (added) parts.push(t("revisionLabel.summary.sourcesAdded", { count: added }));
  if (removed) parts.push(t("revisionLabel.summary.sourcesRemoved", { count: removed }));
  return parts.length > 0 ? parts.join(" · ") : null;
}

const SOURCE_KIND_LABEL: Record<string, string> = {
  memory: "memory",
  chat: "chat",
  file: "file",
  obsidian: "obsidian",
  web: "web",
};

export function sourceKindLabel(mem: MemoryItem): string {
  const mt = mem.memory_type?.toLowerCase() ?? "";
  return SOURCE_KIND_LABEL[mt] ?? (mt || "memory");
}

export function relativeMs(ms: number, locale?: string): string {
  const delta = Date.now() - ms;
  if (locale) {
    const relative = new Intl.RelativeTimeFormat(locale, { numeric: "auto", style: "short" });
    if (delta < 60_000) return relative.format(0, "second");
    if (delta < 3_600_000) return relative.format(-Math.floor(delta / 60_000), "minute");
    if (delta < 86_400_000) return relative.format(-Math.floor(delta / 3_600_000), "hour");
    return relative.format(-Math.floor(delta / 86_400_000), "day");
  }
  if (delta < 60_000) return "just now";
  if (delta < 3_600_000) return `${Math.floor(delta / 60_000)}m ago`;
  if (delta < 86_400_000) return `${Math.floor(delta / 3_600_000)}h ago`;
  return `${Math.floor(delta / 86_400_000)}d ago`;
}
