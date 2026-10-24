// SPDX-License-Identifier: AGPL-3.0-only
// Display-side mirror of the backend's per-claim citation contract
// (wenlan-app/wenlan crates/wenlan-core/src/citations.rs). Occurrence k is the
// k-th plain \[(\d+)\] regex match over the raw stored body — deliberately no
// code-fence/wikilink awareness, because the backend counts the same way and
// parity is the contract. Matches inside code consume occurrence indices but
// keep their raw [N] display (a rewritten link inside a code span would render
// as literal garbage). All transforms are display-only.
import type { PageCitation } from "./tauri";

export const CITATION_ANCHOR_PREFIX = "#citation:";

export type CitationState = "cited" | "stripped-empty" | "stripped-mismatch" | "none";

export interface ProcessedCitations {
  /** Body with markers rewritten to [k](#citation:k), or display-stripped. */
  content: string;
  state: CitationState;
  /** 1-based occurrence -> citation; empty unless state === "cited". */
  byOccurrence: Map<number, PageCitation>;
  /** 1-based occurrence -> the number its chip shows: the source's place
   *  among the page's distinct cited sources, so a source cited again
   *  reuses the number of its first citation. Empty unless cited. */
  sourceNumberByOccurrence: Map<number, number>;
}

/** One cited source, whatever marker or occurrence cites it. */
export function citationSourceKey(c: Pick<PageCitation, "source_kind" | "locator">): string {
  return `${c.source_kind}\u0000${c.locator}`;
}

/** Footnote numbering: walk the citations in reading order and give each
 *  distinct source the next number the first time it is cited; a later
 *  citation of the same source reuses that number. The Nth distinct source
 *  is number N, whatever marker the writer used. */
export function citationSourceNumbers(
  citations: Iterable<PageCitation>,
): Map<number, number> {
  const bySource = new Map<string, number>();
  const byOccurrence = new Map<number, number>();
  for (const c of [...citations].sort((a, b) => a.occurrence - b.occurrence)) {
    const key = citationSourceKey(c);
    let n = bySource.get(key);
    if (n === undefined) {
      n = bySource.size + 1;
      bySource.set(key, n);
    }
    byOccurrence.set(c.occurrence, n);
  }
  return byOccurrence;
}

const MARKER_RE = /\[(\d+)\]/g;

/** Character ranges covered by fenced blocks or inline code spans. */
function codeRanges(content: string): Array<[number, number]> {
  const ranges: Array<[number, number]> = [];
  const fence = /^```.*$/gm;
  let openAt: number | null = null;
  let m: RegExpExecArray | null;
  while ((m = fence.exec(content)) !== null) {
    if (openAt === null) {
      openAt = m.index;
    } else {
      ranges.push([openAt, m.index + m[0].length]);
      openAt = null;
    }
  }
  if (openAt !== null) ranges.push([openAt, content.length]);
  const inline = /`[^`\n]*`/g;
  while ((m = inline.exec(content)) !== null) {
    const start = m.index;
    if (!ranges.some(([s, e]) => start >= s && start < e)) {
      ranges.push([start, start + m[0].length]);
    }
  }
  return ranges;
}

/** Mirror the backend's strip_markers: remove [N], collapse doubled spaces.
 *  Only eat the space in front of a marker that sits before closing
 *  punctuation: the distiller often writes "one file [5][13]." and a display
 *  strip must not show "one file .". Code such as "rm -rf ." keeps its space */
function stripMarkers(text: string): string {
  return text
    .replace(/ *(?:\[\d+\])+(?=[.,;:!?])/g, "")
    .replace(/\[\d+\]/g, "")
    .replace(/ {2,}/g, " ");
}

/** Remove rewritten citation links from plain-text contexts (TLDR pull-quote). */
export function stripCitationLinks(text: string): string {
  return text
    .replace(/ *(?:\[\d+\]\(#citation:\d+\))+(?=[.,;:!?])/g, "")
    .replace(/\[\d+\]\(#citation:\d+\)/g, "")
    .replace(/ {2,}/g, " ")
    .trim();
}

export function processCitations(
  content: string,
  citations: PageCitation[] | undefined,
): ProcessedCitations {
  const matches = [...content.matchAll(MARKER_RE)];
  if (matches.length === 0 && (!citations || citations.length === 0)) {
    return { content, state: "none", byOccurrence: new Map(), sourceNumberByOccurrence: new Map() };
  }
  if (!citations || citations.length === 0) {
    // Verified backend behavior: a user content edit resets citations to []
    // but stores the markers verbatim — without this strip every edited page
    // renders permanent [N] noise.
    return { content: stripMarkers(content), state: "stripped-empty", byOccurrence: new Map(), sourceNumberByOccurrence: new Map() };
  }
  const byOccurrence = new Map<number, PageCitation>();
  for (const c of citations) byOccurrence.set(c.occurrence, c);
  // Conservative fallback: any count or marker disagreement means the mapping
  // is untrustworthy — misattributed citations are worse than none.
  const mismatch =
    matches.length !== citations.length ||
    matches.some((m, i) => byOccurrence.get(i + 1)?.marker !== Number(m[1]));
  if (mismatch) {
    return { content: stripMarkers(content), state: "stripped-mismatch", byOccurrence: new Map(), sourceNumberByOccurrence: new Map() };
  }
  const ranges = codeRanges(content);
  const inCode = (idx: number) => ranges.some(([s, e]) => idx >= s && idx < e);
  let out = "";
  let last = 0;
  const shown: PageCitation[] = [];
  matches.forEach((m, i) => {
    const at = m.index ?? 0;
    if (inCode(at)) return; // counted, but displayed raw
    const k = i + 1;
    const c = byOccurrence.get(k);
    if (c) shown.push(c);
    out += content.slice(last, at) + `[${k}](${CITATION_ANCHOR_PREFIX}${k})`;
    last = at + m[0].length;
  });
  out += content.slice(last);
  // Numbered over the chips a reader sees: a marker left raw in code takes no
  // number, so the visible chips run 1, 2, 3 without a gap.
  return { content: out, state: "cited", byOccurrence, sourceNumberByOccurrence: citationSourceNumbers(shown) };
}

export type CitationKindLabels = Readonly<
  Record<PageCitation["source_kind"], string>
>;

/** Keep only short, human-readable external context beside the source kind. */
export function citationExternalDetail(c: PageCitation): string | null {
  if (c.source_kind === "external_url") {
    try {
      const hostname = new URL(c.locator).hostname;
      return hostname || null;
    } catch {
      // A malformed URL still gets its localized kind label. The exact
      // locator remains available in the popover for an honest refusal.
      return null;
    }
  }
  if (c.source_kind === "external_file") {
    // Document locators may carry a registered source id before the path.
    const sep = c.locator.lastIndexOf("::");
    const path = sep === -1 ? c.locator : c.locator.slice(sep + 2);
    return path.split(/[\\/]/).filter(Boolean).pop() ?? null;
  }
  return null;
}

/** Short chip label: localized kind, with a brief external basename/domain. */
export function citationDisplayLabel(
  c: PageCitation,
  kindLabels: CitationKindLabels,
): string {
  const kind = kindLabels[c.source_kind];
  const detail = citationExternalDetail(c);
  // Long paths and domains turn a prose citation into a layout interruption;
  // the popover still carries the exact locator when the reader needs it.
  return detail && detail.length <= 32 ? `${kind} · ${detail}` : kind;
}
