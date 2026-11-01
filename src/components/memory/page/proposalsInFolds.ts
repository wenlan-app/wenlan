// SPDX-License-Identifier: AGPL-3.0-only
import { locateSentence, type SentenceProposalView } from "../editor/sentenceProposals";
import type { PageLength } from "./PageReadingLayers";
import { pageLayerOffsets } from "./pageLayers";

/**
 * How many proposals sit in each folded part of the page (UX-VIEW-2/3 fold
 * the text, UX-WHO-2 puts a proposal under its sentence). The fold line
 * carries the count, so a proposal inside a fold is never out of sight. The
 * part for AI is counted whether or not it is open: once opened, its
 * proposals draw in place and the closed line is gone.
 */
export function proposalsInFolds(
  content: string,
  proposals: readonly SentenceProposalView[],
  length: PageLength,
): { short: number; ai: number } {
  const { sectionStart, aiStart } = pageLayerOffsets(content);
  let short = 0;
  let ai = 0;
  for (const proposal of proposals) {
    const at = locateSentence(content, proposal.before);
    if (!at) continue;
    if (aiStart !== null && at.from >= aiStart) ai += 1;
    else if (length === "short" && sectionStart !== null && at.from >= sectionStart) short += 1;
  }
  return { short, ai };
}
