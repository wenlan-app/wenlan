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

/** Read the page projection without exposing its revision bindings in the UI. */
export function parseHowIWorkLines(content: string): SkillLine[] {
  const lines: SkillLine[] = [];
  for (const raw of content.split(/\r?\n/u)) {
    if (!raw.includes(RULE_MARKER)) continue;
    const match = /^- \[(rule|decision|pitfall)\] (.*?) — Source: (.*) \(([^()]*)\) r(-?\d+) at (-?\d+) — Quote: (.+) <!-- wenlan-rule:([^>]+) -->$/u.exec(raw);
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
  }
  return lines;
}

export function isHowIWorkPage(page: Pick<Page, "content" | "title">): boolean {
  return page.content.includes(RULE_MARKER)
    || page.content.includes(GENERATED_INTRO)
    || page.title.startsWith(GENERATED_TITLE_PREFIX);
}

export function howIWorkSpaceName(page: Pick<Page, "content" | "space" | "title">): string {
  const explicit = page.space?.trim();
  if (explicit) return explicit;
  if (page.title.startsWith(GENERATED_TITLE_PREFIX)) return page.title.slice(GENERATED_TITLE_PREFIX.length).trim();
  const heading = /^# How I work — (.+)$/mu.exec(page.content)?.[1]?.trim();
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
