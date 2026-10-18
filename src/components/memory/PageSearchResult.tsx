// SPDX-License-Identifier: AGPL-3.0-only
import type { Page } from "../../lib/tauri";
import { highlightTerms } from "../../lib/highlight";

function previewText(markdown: string): string {
  // Code is literal text: Markdown/citation cleanup must not erase array indices,
  // links, or emphasis that happen to be part of an example.
  const code: string[] = [];
  let marker = "\u0000code";
  while (markdown.includes(marker)) marker += "_";
  const preserve = (text: string) => `${marker}${code.push(text) - 1}\u0000`;
  let fence: { character: string; length: number } | undefined;
  const protectedText = markdown.split("\n").map((line) => {
    if (fence) {
      if (new RegExp(`^ {0,3}${fence.character}{${fence.length},}[ \\t]*$`).test(line)) {
        fence = undefined;
        return "";
      }
      return preserve(line);
    }
    const opening = /^ {0,3}(`{3,}|~{3,})(.*)$/.exec(line);
    if (opening && (opening[1][0] !== "`" || !opening[2].includes("`"))) {
      fence = { character: opening[1][0], length: opening[1].length };
      return "";
    }
    return /^(?: {4}|\t)/.test(line) ? preserve(line.trimStart()) : line;
  }).join("\n").replace(/(?<!`)(`+)(?!`)([\s\S]*?)(?<!`)\1(?!`)/g, (_match, _ticks, text: string) => preserve(text));
  return protectedText
    .replace(/!\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/\[\[([^\]|]+)\|([^\]]+)\]\]/g, "$2")
    .replace(/\[\[([^\]]+)\]\]/g, "$1")
    .replace(/\[\d+\]\(#citation:\d+\)|(?<!\S)\[\d+\](?=\s|[.,!?;:，。！？；：]|$)/g, "")
    .replace(/\[([^\]]+)\]\([^)]*\)/g, "$1")
    .replace(/^\s*(?:#{1,6}\s+|>\s*|[-*+]\s+|\d+[.)]\s+)/gm, "")
    .replace(/(\*\*|\*)(\S(?:[^\n]*?\S)?)\1/g, "$2")
    .replace(/\s+/g, " ")
    .replace(/\s+([.,!?;:，。！？；：])/g, "$1")
    .replace(new RegExp(`${marker}(\\d+)\u0000`, "g"), (_match, index: string) => code[Number(index)])
    .replace(/\s+/g, " ")
    .trim();
}

/** A literal excerpt of returned page text; never infer a semantic match reason. */
export function pageSearchSnippet(page: Pick<Page, "summary" | "content">, query: string): string {
  const summary = previewText(page.summary ?? "");
  const content = previewText(page.content);
  const terms = query.trim().split(/\s+/).filter(Boolean);
  const findMatch = (text: string) => {
    // Match against the original text so Unicode case folding cannot shift offsets.
    const literalIndex = (term: string) => text.search(new RegExp(term.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"), "i"));
    const phrase = query.trim();
    const exact = phrase ? literalIndex(phrase) : -1;
    if (exact >= 0) return { index: exact, rank: 2 };
    const index = terms.reduce((first, term) => {
      const termIndex = literalIndex(term);
      return termIndex >= 0 && (first < 0 || termIndex < first) ? termIndex : first;
    }, -1);
    return { index, rank: index >= 0 ? 1 : 0 };
  };
  const summaryMatch = findMatch(summary);
  const contentMatch = findMatch(content);
  const useSummary = summary.length > 0 && summaryMatch.rank >= contentMatch.rank;
  const text = useSummary ? summary : content;
  const matchIndex = (useSummary ? summaryMatch : contentMatch).index;
  const characters = Array.from(text);
  const match = matchIndex >= 0 ? Array.from(text.slice(0, matchIndex)).length : -1;
  let start = Math.max(0, match - 60);
  const nextSpace = characters.indexOf(" ", start);
  if (start > 0 && nextSpace >= 0 && nextSpace < match) start = nextSpace + 1;
  const excerpt = characters.slice(start, start + 240).join("");
  return `${start > 0 ? "…" : ""}${excerpt}${start + 240 < characters.length ? "…" : ""}`;
}

export default function PageSearchResult({ page, query, onClick }: {
  page: Page;
  query: string;
  onClick: () => void;
}) {
  const snippet = pageSearchSnippet(page, query);
  return (
    <button type="button" className="search-result-button" onClick={onClick}>
      <span className="search-result-line">
        <span className="search-result-dot" />
        <span className="search-result-title">{highlightTerms(page.title, query, "search-result-match")}</span>
        {page.domain && <span className="search-result-domain">{page.domain}</span>}
      </span>
      {snippet && <span className="search-result-summary">{highlightTerms(snippet, query, "search-result-match")}</span>}
    </button>
  );
}
