// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import { diffCodeLines } from "./codeBasisDiff";

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
});
