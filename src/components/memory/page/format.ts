// SPDX-License-Identifier: AGPL-3.0-only
// Shared display helpers for the page-detail subcomponents.
import type { TFunction } from "i18next";
import { agentDisplayName, RULE_REMOVED_IN_APP, SAVED_FROM_TERMINAL, type MemoryItem, type PageChangelogEntry } from "../../../lib/tauri";
import { writerAgentName } from "../sharing/teamPeople";

export function prettyAgent(name: string | null | undefined): string {
  return agentDisplayName(name) ?? "unknown agent";
}

/** How the daemon opens the id of a tool whose own name reads as the
 *  terminal, the person or Wenlan (`saved_rules::SELF_NAMED_TOOL_ID_PREFIX`). */
const SELF_NAMED_TOOL_PREFIX = "tool:";

const WRITER_LABEL_KEYS = {
  manual_edit: "revisionLabel.writer.manualEdit",
  // The person, or their AI tool, on another of their own computers.
  own_device: "revisionLabel.writer.ownDevice",
  own_device_ai: "revisionLabel.writer.ownDeviceAi",
  fs_edit: "revisionLabel.writer.fsEdit",
  distill: "revisionLabel.writer.distill",
  re_distill: "revisionLabel.writer.reDistill",
  page_growth: "revisionLabel.writer.pageGrowth",
  refinery_merge: "revisionLabel.writer.refineryMerge",
  revision_accept: "revisionLabel.writer.revisionAccept",
  citation_backfill: "revisionLabel.writer.citationBackfill",
  link_rename: "revisionLabel.writer.linkRename",
  // A page's first version, listed in its history as its birth.
  person_create: "revisionLabel.writer.personCreate",
  agent_create: "revisionLabel.writer.agentCreate",
  create: "revisionLabel.writer.create",
} as const;

/** A rule the person asked to be saved, or to be taken out again
 * (`edited_by` is `rule_save`, UX-SKILL-14): where the ask came from. It
 * names the AI tool or the terminal, and "you" only for the Remove the
 * person pressed in the app. `removed` says the change took a rule out. */
function ruleSaveLabel(t: TFunction, agent: string | null | undefined, removed: boolean): string {
  const from = agent?.trim() ?? "";
  if (from === RULE_REMOVED_IN_APP) {
    return t(removed ? "revisionLabel.writer.ruleRemovedByYou" : "revisionLabel.writer.ruleSaved");
  }
  if (from === SAVED_FROM_TERMINAL) {
    return t(removed ? "revisionLabel.writer.ruleRemovedTerminal" : "revisionLabel.writer.ruleSavedTerminal");
  }
  // A tool that calls itself the terminal, the person or Wenlan is recorded
  // as `tool:<name>`. It is said to be an AI tool, with the name it gave in
  // quotes and as it gave it: never the terminal, the person or the app.
  if (from.startsWith(SELF_NAMED_TOOL_PREFIX)) {
    const called = from.slice(SELF_NAMED_TOOL_PREFIX.length).trim();
    if (!called) return t(removed ? "revisionLabel.writer.ruleRemoved" : "revisionLabel.writer.ruleSaved");
    return t(removed ? "revisionLabel.writer.ruleRemovedSelfNamed" : "revisionLabel.writer.ruleSavedSelfNamed", { name: called });
  }
  const name = writerAgentName(t, from);
  if (!name) return t(removed ? "revisionLabel.writer.ruleRemoved" : "revisionLabel.writer.ruleSaved");
  return t(removed ? "revisionLabel.writer.ruleRemovedNamed" : "revisionLabel.writer.ruleSavedNamed", { agent: name });
}

/** Who made a page revision, in words the reader can use. Never the raw
 * `edited_by` slug: an unrecognized writer reads as a plain "Updated".
 * `charDelta` is the change's size where it is known; it tells a saved rule
 * from a removed one. */
export function revisionWriterLabel(
  t: TFunction,
  editedBy: string,
  agent?: string | null,
  charDelta?: number | null,
): string {
  if (editedBy === "rule_save") return ruleSaveLabel(t, agent, (charDelta ?? 0) < 0);
  if (editedBy === "agent_refresh") {
    const name = writerAgentName(t, agent);
    return name
      ? t("revisionLabel.writer.agentRefreshNamed", { agent: name })
      : t("revisionLabel.writer.agentRefresh");
  }
  if (editedBy === "agent_create") {
    // The AI tool that made the page, by its own name where it was kept.
    // A tool that calls itself the terminal, the person or Wenlan is not
    // named as itself.
    const from = agent?.trim() ?? "";
    const selfNamed = from === SAVED_FROM_TERMINAL || from === RULE_REMOVED_IN_APP
      || from.startsWith(SELF_NAMED_TOOL_PREFIX);
    const name = selfNamed ? null : writerAgentName(t, from);
    return name
      ? t("revisionLabel.writer.agentCreateNamed", { agent: name })
      : t("revisionLabel.writer.agentCreate");
  }
  if (editedBy === "person_asked") {
    const name = writerAgentName(t, agent);
    return name
      ? t("revisionLabel.writer.personAskedNamed", { agent: name })
      : t("revisionLabel.writer.personAsked");
  }
  if (Object.prototype.hasOwnProperty.call(WRITER_LABEL_KEYS, editedBy)) {
    return t(WRITER_LABEL_KEYS[editedBy as keyof typeof WRITER_LABEL_KEYS]);
  }
  return t("revisionLabel.writer.other");
}

/** What a note is built on, e.g. "3 source memories · 2 notes · 5 code
 * references". A part that is zero is left out rather than shown as "0 …". */
export function basisCountLabel(
  t: TFunction,
  sourceCount: number,
  noteCount: number,
  codeCount = 0,
): string {
  const parts: string[] = [];
  if (sourceCount > 0) parts.push(t("noteBasis.sourceMemoriesCount", { count: sourceCount }));
  if (noteCount > 0) parts.push(t("noteBasis.sourceNotesCount", { count: noteCount }));
  if (codeCount > 0) parts.push(t("codeBasis.codeCount", { count: codeCount }));
  return parts.join(" \u00b7 ");
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
