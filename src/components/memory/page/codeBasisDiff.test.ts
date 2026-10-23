// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import { alignCodeLines, diffCodeLines, fitCurrentLines, type CodeRow } from "./codeBasisDiff";

describe("diffCodeLines", () => {
  it("marks nothing when both sides read the same", () => {
    expect(diffCodeLines(["a", "b"], ["a", "b"])).toEqual({ removed: [false, false], added: [false, false] });
  });

  it("marks a changed line on both sides and leaves the equal lines plain", () => {
    const { removed, added } = diffCodeLines(
      ["fn lex() {", "  lex(input)", "}"],
      ["fn lex() {", "  lex_v2(input)", "}"],
    );
    expect(removed).toEqual([false, true, false]);
    expect(added).toEqual([false, true, false]);
  });

  it("marks only the inserted and the deleted lines", () => {
    const { removed, added } = diffCodeLines(["a", "b", "c"], ["a", "x", "b", "c", "y"]);
    expect(removed).toEqual([false, false, false]);
    expect(added).toEqual([false, true, false, false, true]);
    const gone = diffCodeLines(["a", "b", "c"], ["a", "c"]);
    expect(gone.removed).toEqual([false, true, false]);
    expect(gone.added).toEqual([false, false]);
  });

  it("ignores indentation and trailing space", () => {
    const { removed, added } = diffCodeLines(["  a ", "b"], ["a", "\tb"]);
    expect(removed).toEqual([false, false]);
    expect(added).toEqual([false, false]);
  });

  it("marks everything when nothing is shared, and handles an empty side", () => {
    expect(diffCodeLines(["a"], ["b"])).toEqual({ removed: [true], added: [true] });
    expect(diffCodeLines([], ["b"])).toEqual({ removed: [], added: [true] });
    expect(diffCodeLines(["a"], [])).toEqual({ removed: [true], added: [] });
  });

  it("marks nothing for ranges too large to compare", () => {
    const big = Array.from({ length: 600 }, (_, index) => `line ${index}`);
    const other = Array.from({ length: 600 }, (_, index) => `other ${index}`);
    const { removed, added } = diffCodeLines(big, other);
    expect(removed.some(Boolean) || added.some(Boolean)).toBe(false);
  });

  it("still compares a long span whose changes sit in the middle", () => {
    const before = Array.from({ length: 2000 }, (_, index) => `line ${index}`);
    const after = [...before];
    after[1000] = "changed";
    const diff = diffCodeLines(before, after);
    expect(diff.tooLarge).toBeUndefined();
    expect(diff.removed.filter(Boolean)).toHaveLength(1);
    expect(diff.removed[1000]).toBe(true);
    expect(diff.added[1000]).toBe(true);
  });

  it("says when the lines are too many to compare", () => {
    const big = Array.from({ length: 600 }, (_, index) => `line ${index}`);
    const other = Array.from({ length: 600 }, (_, index) => `other ${index}`);
    expect(diffCodeLines(big, other).tooLarge).toBe(true);
  });
});

const describeRows = (rows: readonly CodeRow[]) => rows.map((row) => row.kind === "fold"
  ? `fold ${row.count}`
  : `${row.before ? `${row.before.index}${row.before.changed ? "-" : ""}` : "_"}|${row.after ? `${row.after.index}${row.after.changed ? "+" : ""}` : "_"}`);

describe("alignCodeLines", () => {
  it("faces a removed line with an added one and pads the shorter side", () => {
    const rows = alignCodeLines(diffCodeLines(["a", "b", "c"], ["a", "x", "y", "c"]));
    expect(describeRows(rows)).toEqual(["0|0", "1-|1+", "_|2+", "2|3"]);
  });

  it("folds a long run of equal lines, keeping three lines next to each change", () => {
    const before = Array.from({ length: 30 }, (_, index) => `line ${index}`);
    const after = [...before];
    after[20] = "changed";
    const rows = alignCodeLines(diffCodeLines(before, after));
    expect(describeRows(rows)).toEqual([
      "fold 17", "17|17", "18|18", "19|19", "20-|20+", "21|21", "22|22", "23|23", "fold 6",
    ]);
    const fold = rows[0];
    expect(fold.kind === "fold" && fold.rows).toHaveLength(17);
  });

  it("does not fold a short run, nor code that did not change at all", () => {
    expect(describeRows(alignCodeLines(diffCodeLines(["a", "b", "c", "d"], ["a", "b", "x", "d"]))))
      .toEqual(["0|0", "1|1", "2-|2+", "3|3"]);
    const same = Array.from({ length: 20 }, (_, index) => `line ${index}`);
    expect(alignCodeLines(diffCodeLines(same, same))).toHaveLength(20);
  });

  it("lines the sides up without folding when they were too many to compare", () => {
    const big = Array.from({ length: 600 }, (_, index) => `line ${index}`);
    const other = Array.from({ length: 601 }, (_, index) => `other ${index}`);
    const rows = alignCodeLines(diffCodeLines(big, other));
    expect(rows).toHaveLength(601);
    expect(rows.every((row) => row.kind === "pair")).toBe(true);
  });
});

describe("fitCurrentLines", () => {
  const cited = ["a", "b", "c", "d"];

  it("keeps the last cited line that an insertion pushed past the span", () => {
    expect(fitCurrentLines(cited, ["a", "new", "b", "c", "d", "e", "f"], 4)).toEqual(["a", "new", "b", "c", "d"]);
  });

  it("stops at the span's end when a deletion pulled later lines up", () => {
    expect(fitCurrentLines(cited, ["a", "c", "d", "e", "f"], 4)).toEqual(["a", "c", "d"]);
  });

  it("keeps a changed last line after the last equal one", () => {
    expect(fitCurrentLines(cited, ["a", "b", "c", "D", "e"], 4)).toEqual(["a", "b", "c", "D"]);
  });

  it("is as long as the span with nothing to compare or nothing in common", () => {
    expect(fitCurrentLines(null, ["a", "b", "c", "d", "e"], 4)).toEqual(["a", "b", "c", "d"]);
    expect(fitCurrentLines(cited, ["w", "x", "y", "z", "q"], 4)).toEqual(["w", "x", "y", "z"]);
  });
});
