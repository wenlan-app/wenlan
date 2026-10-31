// SPDX-License-Identifier: AGPL-3.0-only

/**
 * A page is written for people first and for AI below (docs/ux/spec.md,
 * UX-VIEW-2 and UX-VIEW-3). The part for AI starts at one marker heading;
 * everything above it is the human part. The human part has two lengths: the
 * short one stops before its first section heading, the full one is all of it.
 *
 * Every reader of the layers goes through this one helper, so when the daemon
 * reports the split itself only this file changes.
 */
export interface PageLayers {
  /** Everything above the marker. */
  readonly human: string;
  /** The human part up to its first `##` section; equal to `human` when the
   *  page has no sections, so the two lengths only differ when they can. */
  readonly short: string;
  /** Everything after the marker heading, without it; empty when absent. */
  readonly aiDetails: string;
}

/** Heading lines that start the part for AI, in every interface language. */
const AI_DETAILS_HEADING = /^##[ \t]+(?:給 AI 的細節|给 AI 的细节|Details for AI)[ \t]*$/m;

/** A level-2 heading outside fenced code. */
function firstSectionStart(markdown: string): number {
  let fenced = false;
  let offset = 0;
  for (const line of markdown.split("\n")) {
    if (/^(```|~~~)/.test(line.trim())) fenced = !fenced;
    else if (!fenced && /^##[ \t]+\S/.test(line)) return offset;
    offset += line.length + 1;
  }
  return -1;
}

export function splitPageLayers(markdown: string): PageLayers {
  const marker = AI_DETAILS_HEADING.exec(markdown);
  const human = (marker ? markdown.slice(0, marker.index) : markdown).trim();
  const aiDetails = marker ? markdown.slice(marker.index + marker[0].length).trim() : "";
  const section = firstSectionStart(human);
  const short = section > 0 ? human.slice(0, section).trim() : human;
  return { human, short, aiDetails };
}
