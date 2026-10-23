// SPDX-License-Identifier: AGPL-3.0-only

/** Past this many line pairs (after the equal ends are set aside) the comparison is skipped: nothing is marked rather than the window stalling. */
const MAX_PAIRS = 250_000;

export interface CodeLineDiff {
  /** For each cited line: true when no line of the current code matches it. */
  removed: boolean[];
  /** For each current line: true when no line of the cited code matches it. */
  added: boolean[];
  /** The lines were too many to compare, so nothing is marked. */
  tooLarge?: boolean;
}

/**
 * A line-level comparison of the cited lines with the current ones: a longest
 * common subsequence over the trimmed lines, so a change of indentation alone
 * is not a change. Lines in the subsequence are equal; every other line is
 * marked on its own side. No word-level detail. The equal lines at both ends
 * are set aside first, so a long span with a few changes is still compared.
 */
export function diffCodeLines(before: readonly string[], after: readonly string[]): CodeLineDiff {
  const removed = before.map(() => false);
  const added = after.map(() => false);
  const a = before.map((line) => line.trim());
  const b = after.map((line) => line.trim());

  let start = 0;
  while (start < a.length && start < b.length && a[start] === b[start]) start += 1;
  let endA = a.length;
  let endB = b.length;
  while (endA > start && endB > start && a[endA - 1] === b[endB - 1]) {
    endA -= 1;
    endB -= 1;
  }
  const rows = endA - start;
  const cols = endB - start;
  if (rows * cols > MAX_PAIRS) return { removed, added, tooLarge: true };

  const width = cols + 1;
  // lcs[i * width + j]: the length of the longest common subsequence of the middle of a from i and of b from j.
  const lcs = new Uint32Array((rows + 1) * width);
  for (let i = rows - 1; i >= 0; i -= 1) {
    for (let j = cols - 1; j >= 0; j -= 1) {
      lcs[i * width + j] = a[start + i] === b[start + j]
        ? lcs[(i + 1) * width + j + 1] + 1
        : Math.max(lcs[(i + 1) * width + j], lcs[i * width + j + 1]);
    }
  }

  let i = 0;
  let j = 0;
  while (i < rows && j < cols) {
    if (a[start + i] === b[start + j]) {
      i += 1;
      j += 1;
    } else if (lcs[(i + 1) * width + j] >= lcs[i * width + j + 1]) {
      removed[start + i] = true;
      i += 1;
    } else {
      added[start + j] = true;
      j += 1;
    }
  }
  for (; i < rows; i += 1) removed[start + i] = true;
  for (; j < cols; j += 1) added[start + j] = true;
  return { removed, added };
}

/** One side of an aligned row: a line (index into its side, changed or not), or a blank filler. */
export type CodeCell = { index: number; changed: boolean } | null;

/** A row of the side-by-side view: a line on each side, or a run of equal lines folded away. */
export type CodeRow =
  | { kind: "pair"; before: CodeCell; after: CodeCell }
  | { kind: "fold"; id: number; count: number; rows: CodeRow[] };

/** Lines of context kept around a change when a run of equal lines is folded. */
const CONTEXT = 3;
/** The shortest run of equal lines worth folding away. */
const MIN_FOLD = 4;

/**
 * The two sides lined up row by row: equal lines face each other, and within a
 * change the removed lines face the added ones, with blank fillers on the
 * shorter side, so both columns scroll together. A run of equal lines longer
 * than the context around it is folded into one row the person can open.
 */
export function alignCodeLines(diff: CodeLineDiff): CodeRow[] {
  const { removed, added } = diff;
  const flat: CodeRow[] = [];
  let i = 0;
  let j = 0;
  while (i < removed.length || j < added.length) {
    if (i < removed.length && j < added.length && !removed[i] && !added[j]) {
      flat.push({ kind: "pair", before: { index: i, changed: false }, after: { index: j, changed: false } });
      i += 1;
      j += 1;
      continue;
    }
    const gone: number[] = [];
    const fresh: number[] = [];
    while (i < removed.length && removed[i]) gone.push(i++);
    while (j < added.length && added[j]) fresh.push(j++);
    // One side ran out while the other still has equal lines: they cannot be
    // paired, so show them as they are.
    if (gone.length === 0 && fresh.length === 0) {
      if (i < removed.length) gone.push(i++);
      else fresh.push(j++);
    }
    for (let k = 0; k < Math.max(gone.length, fresh.length); k += 1) {
      flat.push({
        kind: "pair",
        before: k < gone.length ? { index: gone[k], changed: removed[gone[k]] } : null,
        after: k < fresh.length ? { index: fresh[k], changed: added[fresh[k]] } : null,
      });
    }
  }
  if (diff.tooLarge) return flat;

  const isEqual = (row: CodeRow) => row.kind === "pair" && !row.before?.changed && !row.after?.changed
    && row.before !== null && row.after !== null;
  const out: CodeRow[] = [];
  let fold = 0;
  let k = 0;
  while (k < flat.length) {
    if (!isEqual(flat[k])) {
      out.push(flat[k]);
      k += 1;
      continue;
    }
    let end = k;
    while (end < flat.length && isEqual(flat[end])) end += 1;
    const keepBefore = k === 0 ? 0 : CONTEXT;
    const keepAfter = end === flat.length ? 0 : CONTEXT;
    const hidden = end - k - keepBefore - keepAfter;
    // Folding fewer lines than this saves no room worth the extra click.
    if (hidden >= MIN_FOLD) {
      out.push(...flat.slice(k, k + keepBefore));
      out.push({ kind: "fold", id: fold++, count: hidden, rows: flat.slice(k + keepBefore, end - keepAfter) });
      out.push(...flat.slice(end - keepAfter, end));
    } else {
      out.push(...flat.slice(k, end));
    }
    k = end;
  }
  // Nothing changed at all: show the lines rather than one fold.
  if (out.length === 1 && out[0].kind === "fold") return out[0].rows;
  return out;
}
