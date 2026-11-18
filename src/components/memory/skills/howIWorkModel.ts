// SPDX-License-Identifier: AGPL-3.0-only
import type { TFunction } from "i18next";
import { SAVED_FROM_TERMINAL, type Page, type PageChangelogEntry, type SkillLine, type SkillLineKind, type SpaceRulesResponse } from "../../../lib/tauri";

export const NOT_FOR_AI_TOOLS_MARKER = "<!-- wenlan:not-for-ai-tools -->";
export const HOW_I_WORK_QUERY_KEYS = {
  enabled: ["skills-from-library-enabled"] as const,
  spaceSuggestions: ["space-rules-suggestions"] as const,
  skillSuggestions: ["skill-suggestions"] as const,
  records: (space: string) => ["skill-borrow-records", space] as const,
  /** The rules the daemon lists for a Space (or for every Space, with ""),
   *  asked again whenever the page in view changes. */
  rules: (space: string, pageId: string, version: number) => ["space-rules", space, pageId, version] as const,
};

const RULE_MARKER = "<!-- wenlan-rule:";
const GENERATED_INTRO = "These are this Space's approved working rules.";
const GENERATED_TITLE_PREFIX = "How I work — ";

function parseQuotedText(raw: string): string {
  try {
    const parsed: unknown = JSON.parse(raw);
    return typeof parsed === "string" ? parsed : raw;
  } catch {
    return raw.replace(/^"|"$/g, "");
  }
}

const MANAGED_LINE = /^- \[(rule|decision|pitfall)\] (.*?) — Source: (.*) \(([^()]*)\) r(-?\d+) at (-?\d+) — Quote: (.+) <!-- wenlan-rule:([^>]+) -->$/u;
const BULLET = /^( {0,3})[-*+] +(.*)$/u;
const TAGGED = /^\[(rule|decision|pitfall)\]\s*/iu;

/** Read the page projection without exposing its revision bindings in the UI.
 *  A managed line names the page it came from. A plain top-level bullet is
 *  read as the daemon reads it (`skill_grow::person_rule_lines`): outside
 *  code blocks, not a to-do box, a rule unless it opens with `[decision]` or
 *  `[pitfall]`. Who wrote a bullet is not in the page text: only the daemon
 *  knows, from the page's history, so a bullet carries no author here until
 *  `withDaemonAuthors` adds the daemon's answer. */
export function parseHowIWorkLines(content: string, page?: Pick<Page, "id" | "title">): SkillLine[] {
  const lines: SkillLine[] = [];
  const seen = new Set<string>();
  let fence: string | null = null;
  let listIndent: number | null = null;
  for (const raw of content.split(/\r?\n/u)) {
    const trimmed = raw.trimStart();
    const opens = ["```", "~~~"].find((mark) => trimmed.startsWith(mark));
    if (opens) { fence = fence === null ? opens : fence === opens ? null : fence; continue; }
    if (fence !== null) continue;
    if (raw.includes(RULE_MARKER)) {
      const match = MANAGED_LINE.exec(raw);
      if (!match) continue;
      const [, kind, text, sourceTitle, sourcePageId, sourceRevision, sourceRevisionAt, quoted, id] = match;
      const sourceQuote = parseQuotedText(quoted);
      lines.push({
        id,
        kind: kind as SkillLineKind,
        text,
        source_page_id: sourcePageId,
        source_page_title: sourceTitle,
        source_revision: Number(sourceRevision),
        source_revision_at: Number(sourceRevisionAt),
        source_quote: sourceQuote,
        person_edited: text !== sourceQuote,
      });
      continue;
    }
    if (trimmed.startsWith("#")) { listIndent = null; continue; }
    const bullet = BULLET.exec(raw);
    if (!bullet) continue;
    const indent = bullet[1].length;
    if (listIndent !== null && indent >= listIndent + 2) continue;
    listIndent = indent;
    const item = bullet[2].trim();
    if (/^\[[ xX]\]/u.test(item)) continue;
    const tag = TAGGED.exec(item);
    const kind = (tag?.[1].toLowerCase() ?? "rule") as SkillLineKind;
    const text = (tag ? item.slice(tag[0].length) : item).split(/\s+/u).filter(Boolean).join(" ");
    const key = text.toLowerCase();
    if (!text || seen.has(key)) continue;
    seen.add(key);
    lines.push({
      id: `own:${lines.length}:${key}`,
      kind,
      text,
      source_page_id: page?.id ?? "",
      source_page_title: page?.title ?? "",
      source_revision: 0,
      source_revision_at: 0,
      source_quote: text,
    });
  }
  return lines;
}

/** One rule's words, compared as the daemon compares them. */
export function ruleTextKey(text: string): string {
  return text.split(/\s+/u).filter(Boolean).join(" ").toLowerCase();
}

/** The daemon's lines that came from `pageId` itself. */
export function rulesFromPage(rules: SpaceRulesResponse | undefined, pageId: string): SkillLine[] {
  return (rules?.lines ?? []).filter((line) => line.source_page_id === pageId);
}

/** Marks a plain bullet as written by the person only where the daemon says
 *  so: a line it returned as hand-written, from this page, with the same
 *  words. Every other bullet keeps no author. */
export function withDaemonAuthors(lines: readonly SkillLine[], rules: SpaceRulesResponse | undefined, pageId: string): SkillLine[] {
  const own = new Map(rulesFromPage(rules, pageId).filter((line) => line.hand_written).map((line) => [ruleTextKey(line.text), line] as const));
  const all = new Map((rules?.lines ?? []).map((line) => [line.id, line] as const));
  return lines.map((line) => {
    if (!line.id.startsWith("own:")) return all.get(line.id)?.general ? { ...line, general: true } : line;
    const told = own.get(ruleTextKey(line.text));
    return told ? { ...line, hand_written: true, person_edited: true, general: told.general } : line;
  });
}

/** Whether `page` gives any rule that is used in every Space: those are the
 *  only rules Wenlan writes into the AI tools' files. `everySpace` says the
 *  daemon was asked with no Space, where every line it lists is one. */
export function pageGivesGeneralRules(
  rules: SpaceRulesResponse | undefined,
  page: Pick<Page, "id">,
  pageLines: readonly SkillLine[],
  everySpace: boolean,
): boolean {
  if (!rules) return false;
  const onPage = new Set(pageLines.map((line) => line.id));
  return rules.lines.some((line) => (everySpace || line.general === true)
    && (line.source_page_id === page.id || (rules.rules_page_id === page.id && onPage.has(line.id))));
}

export function isHowIWorkPage(page: Pick<Page, "content" | "title">): boolean {
  // List rows can arrive without a body or a title; never let one break a list.
  const content = page.content ?? "";
  return content.includes(RULE_MARKER)
    || content.includes(GENERATED_INTRO)
    || (page.title ?? "").startsWith(GENERATED_TITLE_PREFIX);
}

/** The daemon's answer that carries which pages keep saved rules. Its key
 *  opens like every rules query, so whatever refreshes the rules refreshes
 *  it too. */
export const SAVED_RULES_PAGES_QUERY_KEY = ["space-rules", "saved-pages"] as const;
export const NO_SAVED_RULES_PAGES: ReadonlySet<string> = new Set<string>();

/** Whether `page` keeps the rules the person asked to be saved. Only the
 *  daemon knows: its ledger holds each such page's id, and `savedPages` is
 *  that answer. The title says nothing, so a renamed page still is one and
 *  a page somebody titled "Rules I saved" is an ordinary page (UX-SKILL-14). */
export function isSavedRulesPage(page: Pick<Page, "id">, savedPages: ReadonlySet<string>): boolean {
  return savedPages.has(page.id);
}

/** "saved from <tool>" for a rule the person asked to be saved, else null,
 *  in the app's language. The daemon says which kind of place it was saved
 *  from and the tool's name; a tool that calls itself the terminal, the
 *  person or Wenlan is said to be an AI tool, with its name in quotes. It
 *  is never the person (UX-SKILL-14). An older daemon sends only its
 *  English label, shown as it is. */
export function savedFromLabel(line: Pick<SkillLine, "saved_from" | "saved_by">, t: TFunction): string | null {
  const from = line.saved_from?.trim();
  if (!from) return null;
  const name = line.saved_by?.name?.trim() ?? "";
  switch (line.saved_by?.kind) {
    case "terminal": return t("howIWork.saved.fromTerminal");
    case "unnamed": return t("howIWork.saved.fromUnnamed");
    case "self_named": return name ? t("howIWork.saved.fromSelfNamed", { name }) : t("howIWork.saved.fromUnnamed");
    case "tool": return name ? t("howIWork.saved.from", { tool: name }) : t("howIWork.saved.fromUnnamed");
    default: return from === SAVED_FROM_TERMINAL ? t("howIWork.saved.fromTerminal") : t("howIWork.saved.from", { tool: from });
  }
}

/** The daemon's answer to a Remove for a rule that is no longer there: it
 *  was already removed somewhere else (the terminal, another window). */
export function savedRuleAlreadyRemoved(error: unknown): boolean {
  const text = error instanceof Error ? error.message : String(error);
  return text.includes("no saved rule has that id");
}

/** Why a Remove did not go through, in the person's words. The daemon's own
 *  sentence carries a route or an id, so it is never shown as it is. */
export function savedRuleRemoveFailure(error: unknown, t: TFunction): string {
  const text = error instanceof Error ? error.message : String(error);
  if (text.includes("team member")) return t("howIWork.saved.refusedTeam");
  if (text.includes("listens on this computer alone")) return t("howIWork.saved.refusedBeyond");
  return t("howIWork.saved.failed");
}

export function howIWorkSpaceName(page: Pick<Page, "content" | "space" | "title">): string {
  const explicit = page.space?.trim();
  if (explicit) return explicit;
  if (page.title.startsWith(GENERATED_TITLE_PREFIX)) return page.title.slice(GENERATED_TITLE_PREFIX.length).trim();
  const heading = /^# How I work — (.+)$/mu.exec(page.content ?? "")?.[1]?.trim();
  return heading || "";
}

/** The name a "How I work" page is shown under: the same for every Space,
 *  wherever it is shown (the page, its tab, Recent, the Rules list). Its
 *  Space is not part of it (owner's 2026-11-18 approval of sketch
 *  rules-page-v2). */
export function howIWorkPageTitle(
  page: Pick<Page, "content" | "space" | "title">,
  t: TFunction,
): string {
  if (!isHowIWorkPage(page)) return page.title;
  return t("howIWork.page.title");
}

/** How many distinct pages the lines Wenlan maintains on a "How I work" page
 *  come from. A bullet the page itself holds (`own:`) is not one of them. */
export function maintainedSourceCount(lines: readonly SkillLine[]): number {
  return new Set(lines.filter((line) => !line.id.startsWith("own:") && line.source_page_id).map((line) => line.source_page_id)).size;
}

/** When Wenlan last wrote the rules into the page, from the page's history:
 *  the latest `agent_refresh` version, unix seconds. Null when the history
 *  holds none. */
export function lastRulesWrite(entries: readonly Pick<PageChangelogEntry, "at" | "edited_by">[]): number | null {
  const times = entries.filter((entry) => entry.edited_by === "agent_refresh").map((entry) => entry.at);
  return times.length > 0 ? Math.max(...times) : null;
}

/** A short badge for each tool name, for a tool that has no logo: the first
 *  letter of each word (two at most), or the first letter alone for one
 *  word; when two would read the same, the first word's first two letters,
 *  then a number. No two tools in `names` get the same badge. */
export function toolBadges(names: readonly string[]): string[] {
  const taken = new Set<string>();
  return names.map((name) => {
    const words = name.split(/[\s/._-]+/u).filter(Boolean);
    const first = [...(words[0] ?? "?")];
    const candidates = [
      words.length > 1 ? words.slice(0, 2).map((word) => [...word][0].toUpperCase()).join("") : first[0].toUpperCase(),
      first[0].toUpperCase() + (first[1] ?? "").toLowerCase(),
    ];
    let badge = candidates.find((candidate) => !taken.has(candidate));
    for (let n = 2; badge === undefined; n += 1) if (!taken.has(`${candidates[0]}${n}`)) badge = `${candidates[0]}${n}`;
    taken.add(badge);
    return badge;
  });
}

export function visibleSourceTitle(title: string, t: TFunction): string {
  const visible = title.trim();
  if (!visible || /^untitled(?:\s+(?:page|note))?$/iu.test(visible)) return t("howIWork.sourceFallback");
  return visible;
}

export function rulesPageStopped(content: string): boolean {
  return content.split(/\r?\n/u).some((line) => line.trim() === NOT_FOR_AI_TOOLS_MARKER);
}

export function setRulesPageStopped(content: string, stopped: boolean): string {
  const newline = content.includes("\r\n") ? "\r\n" : "\n";
  const withoutMarker = content
    .split(/\r?\n/u)
    .filter((line) => line.trim() !== NOT_FOR_AI_TOOLS_MARKER)
    .join(newline);
  if (!stopped) return withoutMarker;
  const firstBreak = withoutMarker.indexOf(newline);
  return firstBreak < 0
    ? `${withoutMarker}${newline}${newline}${NOT_FOR_AI_TOOLS_MARKER}${newline}`
    : `${withoutMarker.slice(0, firstBreak)}${newline}${newline}${NOT_FOR_AI_TOOLS_MARKER}${withoutMarker.slice(firstBreak)}`;
}

export function replaceHowIWorkLine(content: string, line: SkillLine, nextText: string): string | null {
  const text = nextText.split(/\s+/u).filter(Boolean).join(" ").trim();
  if (!text) return null;
  const marker = `${RULE_MARKER}${line.id} -->`;
  const prefix = `- [${line.kind}] ${line.text}`;
  const newline = content.includes("\r\n") ? "\r\n" : "\n";
  let replaced = false;
  const next = content.split(/\r?\n/u).map((raw) => {
    if (replaced || !raw.includes(marker) || !raw.startsWith(prefix)) return raw;
    replaced = true;
    return `- [${line.kind}] ${text}${raw.slice(prefix.length)}`;
  });
  return replaced ? next.join(newline) : null;
}

export function startOfLocalWeek(now = new Date()): number {
  const start = new Date(now);
  const day = (start.getDay() + 6) % 7;
  start.setDate(start.getDate() - day);
  start.setHours(0, 0, 0, 0);
  return Math.floor(start.getTime() / 1000);
}
