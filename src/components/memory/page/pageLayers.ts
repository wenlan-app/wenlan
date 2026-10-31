// SPDX-License-Identifier: AGPL-3.0-only

/**
 * A page is written for people first and for AI below (docs/ux/spec.md,
 * UX-VIEW-2 and UX-VIEW-3). The rules match the daemon's
 * `wenlan_core::page_body` (storage PR1 contract, section 1): the part for AI
 * starts at the `<!-- wenlan:details -->` anchor line, or without it at a
 * level-2 heading named for it in any interface language. Everything above is
 * the human part. Its short length is the first paragraph after any leading
 * headings; the full length is all of it. Fenced code never counts.
 *
 * Every reader of the layers goes through this one helper, so when the daemon
 * reports the split itself only this file changes.
 */
export interface PageLayers {
  /** Everything above the marker. */
  readonly human: string;
  /** The first paragraph of the human part; equal to `human` when that is
   *  all there is, so the two lengths only differ when they can. */
  readonly short: string;
  /** Everything after the marker heading, without it; empty when absent. */
  readonly aiDetails: string;
}

/** Where the layers start in the raw text, for views that keep the text
 *  whole (the editor folds these ranges instead of cutting them out). */
export interface PageLayerOffsets {
  /** Where the rest of the human part starts after the short length; `null`
   *  when the short and full lengths are the same. */
  readonly sectionStart: number | null;
  /** Start of the marker (anchor or heading) line; `null` without a part for AI. */
  readonly aiStart: number | null;
}

const ANCHOR = "<!-- wenlan:details -->";
const HEADINGS = ["給 AI 的細節", "给 AI 的细节", "Details for AI", "Detalles para la IA"].map(normalize);

function normalize(text: string): string {
  return text.replace(/\s+/g, "").toLowerCase();
}

interface Line {
  readonly text: string;
  readonly start: number;
  readonly fenced: boolean;
}

function lines(markdown: string): Line[] {
  const out: Line[] = [];
  let fenced = false;
  let start = 0;
  for (const text of markdown.split("\n")) {
    const fence = /^(```|~~~)/.test(text.trim());
    out.push({ text, start, fenced: fenced || fence });
    if (fence) fenced = !fenced;
    start += text.length + 1;
  }
  return out;
}

interface Marker {
  /** Start of the anchor or heading line. */
  readonly start: number;
  /** Start of the AI text after the marker heading (or the bare anchor). */
  readonly bodyStart: number;
}

function findMarker(all: readonly Line[]): Marker | null {
  for (let i = 0; i < all.length; i += 1) {
    const line = all[i];
    if (line.fenced) continue;
    if (line.text.trim() === ANCHOR) {
      const next = all[i + 1];
      const headed = next !== undefined && !next.fenced && /^#{1,6}[ \t]/.test(next.text);
      const after = headed ? all[i + 2] : next;
      return { start: line.start, bodyStart: after ? after.start : line.start + line.text.length };
    }
    const heading = /^##[ \t]+(.+?)[ \t]*$/.exec(line.text);
    if (heading && HEADINGS.includes(normalize(heading[1]))) {
      const after = all[i + 1];
      return { start: line.start, bodyStart: after ? after.start : line.start + line.text.length };
    }
  }
  return null;
}

/** Offset just past the first paragraph of `human` (leading headings
 *  skipped), or `null` when nothing of substance follows it. */
function shortEnd(human: string): number | null {
  const all = lines(human);
  const heading = (line: Line) => !line.fenced && /^#{1,6}[ \t]/.test(line.text);
  let i = 0;
  while (i < all.length && (all[i].text.trim() === "" || heading(all[i]))) i += 1;
  if (i >= all.length) return null;
  while (i < all.length && (all[i].fenced || all[i].text.trim() !== "")) i += 1;
  const end = i < all.length ? all[i].start : human.length;
  return human.slice(end).trim() === "" ? null : end;
}

export function pageLayerOffsets(markdown: string): PageLayerOffsets {
  const marker = findMarker(lines(markdown));
  const aiStart = marker ? marker.start : null;
  const end = shortEnd(aiStart === null ? markdown : markdown.slice(0, aiStart));
  return { sectionStart: end, aiStart };
}

export function splitPageLayers(markdown: string): PageLayers {
  const marker = findMarker(lines(markdown));
  const humanRaw = marker ? markdown.slice(0, marker.start) : markdown;
  const human = humanRaw.trim();
  const aiDetails = marker ? markdown.slice(marker.bodyStart).trim() : "";
  const end = shortEnd(humanRaw);
  const short = end === null ? human : humanRaw.slice(0, end).trim();
  return { human, short, aiDetails };
}
