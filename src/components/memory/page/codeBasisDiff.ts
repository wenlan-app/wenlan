// SPDX-License-Identifier: AGPL-3.0-only

/** Past this many line pairs the comparison is skipped: nothing is marked rather than the window stalling. */
const MAX_PAIRS = 250_000;

export interface CodeLineDiff {
  /** For each cited line: true when no line of the current code matches it. */
  removed: boolean[];
  /** For each current line: true when no line of the cited code matches it. */
  added: boolean[];
}

/**
 * A line-level comparison of the cited lines with the current ones: a longest
 * common subsequence over the trimmed lines, so a change of indentation alone
 * is not a change. Lines in the subsequence are equal; every other line is
 * marked on its own side. No word-level detail.
 */
export function diffCodeLines(before: readonly string[], after: readonly string[]): CodeLineDiff {
  const removed = before.map(() => false);
  const added = after.map(() => false);
  if (before.length * after.length > MAX_PAIRS) return { removed, added };

  const a = before.map((line) => line.trim());
  const b = after.map((line) => line.trim());
  const width = b.length + 1;
  // lcs[i * width + j]: the length of the longest common subsequence of a[i..] and b[j..].
  const lcs = new Uint32Array((a.length + 1) * width);
  for (let i = a.length - 1; i >= 0; i -= 1) {
    for (let j = b.length - 1; j >= 0; j -= 1) {
      lcs[i * width + j] = a[i] === b[j]
        ? lcs[(i + 1) * width + j + 1] + 1
        : Math.max(lcs[(i + 1) * width + j], lcs[i * width + j + 1]);
    }
  }

  let i = 0;
  let j = 0;
  while (i < a.length && j < b.length) {
    if (a[i] === b[j]) {
      i += 1;
      j += 1;
    } else if (lcs[(i + 1) * width + j] >= lcs[i * width + j + 1]) {
      removed[i] = true;
      i += 1;
    } else {
      added[j] = true;
      j += 1;
    }
  }
  for (; i < a.length; i += 1) removed[i] = true;
  for (; j < b.length; j += 1) added[j] = true;
  return { removed, added };
}
