// SPDX-License-Identifier: AGPL-3.0-only

/**
 * Return a canvas label that fits the measured width. The source string is
 * never changed, so callers can keep using it for hover, selection, and
 * inspector copy.
 */
export function truncateOverviewLabel(
  context: Pick<CanvasRenderingContext2D, "measureText">,
  label: string,
  maxWidth: number,
): string {
  if (!label || maxWidth <= 0) return "";
  if (context.measureText(label).width <= maxWidth) return label;

  const ellipsis = "…";
  const ellipsisWidth = context.measureText(ellipsis).width;
  if (ellipsisWidth > maxWidth) return "";

  const characters = Array.from(label);
  let low = 0;
  let high = characters.length;
  while (low < high) {
    const middle = Math.ceil((low + high) / 2);
    const candidate = `${characters.slice(0, middle).join("")}…`;
    if (context.measureText(candidate).width <= maxWidth) low = middle;
    else high = middle - 1;
  }
  let end = low;
  // Never part way through a word: back to the last whole one when there is
  // one. CJK text can end at any character.
  const word = (char: string | undefined) => char !== undefined && /[\p{L}\p{N}]/u.test(char) && !CJK_CHAR.test(char);
  if (word(characters[end]) && word(characters[end - 1])) {
    let space = end - 1;
    while (space > 0 && !/\s/u.test(characters[space] as string)) space -= 1;
    if (space > 0) end = space;
  }
  const kept = characters.slice(0, end).join("").replace(/[\s,;:–—\-，、；：]+$/u, "");
  return `${kept || characters.slice(0, low).join("")}…`;
}

const CJK_CHAR = /[\u2e80-\u9fff\uac00-\ud7af\uf900-\ufaff\uff00-\uffef]/u;

/** Keep overview names readable on a narrow canvas without letting one name
 * consume the entire map. At desktop this resolves to the requested ~160px. */
export function overviewLabelMaxWidth(viewportWidth: number): number {
  return Math.min(160, Math.max(88, viewportWidth * 0.42));
}

export interface OverviewLabelCandidate {
  id: string;
  x: number;
  y: number;
}

/**
 * Pick a small, deterministic set of fallback names while spreading them
 * across the map. The candidates have already been chosen by the hierarchy
 * pass (one per unrepresented component); this only keeps a young,
 * disconnected graph from turning into a wall of labels.
 */
export function selectOverviewFallbackLabels(
  candidates: readonly OverviewLabelCandidate[],
  maxLabels = 12,
): Set<string> {
  const ordered = [...candidates].sort((a, b) => (a.id < b.id ? -1 : a.id > b.id ? 1 : 0));
  if (maxLabels <= 0 || ordered.length === 0) return new Set();
  if (ordered.length <= maxLabels) return new Set(ordered.map(({ id }) => id));

  const selected = [ordered[0]];
  const remaining = ordered.slice(1);
  while (selected.length < maxLabels && remaining.length > 0) {
    let bestIndex = 0;
    let bestDistance = -Infinity;
    for (let index = 0; index < remaining.length; index += 1) {
      const candidate = remaining[index];
      const distance = Math.min(
        ...selected.map((chosen) => {
          const dx = candidate.x - chosen.x;
          const dy = candidate.y - chosen.y;
          return Number.isFinite(dx) && Number.isFinite(dy) ? dx * dx + dy * dy : 0;
        }),
      );
      if (
        distance > bestDistance ||
        (distance === bestDistance && candidate.id < remaining[bestIndex].id)
      ) {
        bestDistance = distance;
        bestIndex = index;
      }
    }
    selected.push(remaining.splice(bestIndex, 1)[0]);
  }
  return new Set(selected.map(({ id }) => id));
}
