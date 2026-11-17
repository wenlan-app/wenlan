// SPDX-License-Identifier: AGPL-3.0-only
import type { TFunction } from "i18next";
import type { Page, SkillLine, SkillLineKind } from "../../../lib/tauri";

export const NOT_FOR_AI_TOOLS_MARKER = "<!-- wenlan:not-for-ai-tools -->";
export const HOW_I_WORK_QUERY_KEYS = {
  enabled: ["skills-from-library-enabled"] as const,
  spaceSuggestions: ["space-rules-suggestions"] as const,
  skillSuggestions: ["skill-suggestions"] as const,
  records: (space: string) => ["skill-borrow-records", space] as const,
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
 *  A managed line names the page it came from. A plain top-level bullet is a
 *  rule the person wrote themselves (UX-SKILL-10), read as the daemon reads
 *  it (`skill_grow::person_rule_lines`): outside code blocks, not a to-do
 *  box, a rule unless it opens with `[decision]` or `[pitfall]`. The daemon
 *  also checks who wrote each bullet; the page text alone cannot, so a
 *  bullet someone else added shows the same here. */
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
      person_edited: true,
      hand_written: true,
    });
  }
  return lines;
}

export function isHowIWorkPage(page: Pick<Page, "content" | "title">): boolean {
  // List rows can arrive without a body or a title; never let one break a list.
  const content = page.content ?? "";
  return content.includes(RULE_MARKER)
    || content.includes(GENERATED_INTRO)
    || (page.title ?? "").startsWith(GENERATED_TITLE_PREFIX);
}

export function howIWorkSpaceName(page: Pick<Page, "content" | "space" | "title">): string {
  const explicit = page.space?.trim();
  if (explicit) return explicit;
  if (page.title.startsWith(GENERATED_TITLE_PREFIX)) return page.title.slice(GENERATED_TITLE_PREFIX.length).trim();
  const heading = /^# How I work — (.+)$/mu.exec(page.content ?? "")?.[1]?.trim();
  return heading || "";
}

export function howIWorkPageTitle(
  page: Pick<Page, "content" | "space" | "title">,
  t: TFunction,
): string {
  if (!isHowIWorkPage(page)) return page.title;
  return t("howIWork.page.title", { space: howIWorkSpaceName(page) });
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
