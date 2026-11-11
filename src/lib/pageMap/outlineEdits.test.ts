// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";

import { applyOutlineEdit, lineShape } from "./outlineEdits";

const page = [
  "---", "title: T", "---", // 0-2
  "# Plan", // 3
  "", // 4
  "- first", // 5
  "  more about first", // 6
  "- [x] second", // 7
  "", // 8
  "## Notes", // 9
  "", // 10
  "**Lead.** the rest", // 11
  "", // 12
  "| a | b |", // 13
  "", // 14
  "# Later", // 15
  "text", // 16
].join("\n");

const lines = (body: string | null) => body!.split("\n");

describe("applyOutlineEdit", () => {
  it("renames a heading, a list item and a bold lead, keeping their marks", () => {
    expect(lines(applyOutlineEdit(page, { op: "rename", line: 3, text: "Roadmap" }))[3]).toBe("# Roadmap");
    expect(lines(applyOutlineEdit(page, { op: "rename", line: 7, text: "next" }))[7]).toBe("- [x] next");
    expect(lines(applyOutlineEdit(page, { op: "rename", line: 11, text: "Point" }))[11]).toBe("**Point** the rest");
    expect(lines(applyOutlineEdit(page, { op: "rename", line: 5, text: "a\n# b" }))[5]).toBe("- a # b");
  });

  it("leaves table rows, plain text and blank names alone", () => {
    expect(applyOutlineEdit(page, { op: "rename", line: 13, text: "x" })).toBeNull();
    expect(applyOutlineEdit(page, { op: "rename", line: 16, text: "x" })).toBeNull();
    expect(applyOutlineEdit(page, { op: "rename", line: 3, text: "  " })).toBeNull();
    expect(lineShape("| a |")).toBeNull();
  });

  it("adds an item after an item, below its nested lines, with the same marker", () => {
    const out = lines(applyOutlineEdit(page, { op: "addAfter", line: 5, text: "between" }));
    expect(out.slice(5, 9)).toEqual(["- first", "  more about first", "- between", "- [x] second"]);
    const numbered = lines(applyOutlineEdit("1. one\n2. two", { op: "addAfter", line: 0, text: "x" }));
    expect(numbered).toEqual(["1. one", "2. x", "2. two"]);
  });

  it("adds under a heading by joining its list, or starting one", () => {
    const joined = lines(applyOutlineEdit(page, { op: "addUnder", line: 3, text: "third" }));
    expect(joined.slice(7, 10)).toEqual(["- [x] second", "- [ ] third", ""]);
    const fresh = lines(applyOutlineEdit(page, { op: "addUnder", line: 9, text: "idea" }));
    expect(fresh.slice(9, 17)).toEqual(["## Notes", "", "**Lead.** the rest", "", "| a | b |", "", "- idea", ""]);
    const empty = lines(applyOutlineEdit("# A\n# B", { op: "addUnder", line: 0, text: "x" }));
    expect(empty).toEqual(["# A", "", "- x", "", "# B"]);
  });

  it("adds a heading after a heading's whole section", () => {
    const out = lines(applyOutlineEdit(page, { op: "addAfter", line: 3, text: "Middle" }));
    expect(out.slice(13, 19)).toEqual(["| a | b |", "", "# Middle", "", "# Later", "text"]);
  });

  it("deletes an item with its nested lines, and a heading with what it holds", () => {
    const item = lines(applyOutlineEdit(page, { op: "delete", line: 5 }));
    expect(item.slice(3, 7)).toEqual(["# Plan", "", "- [x] second", ""]);
    const heading = lines(applyOutlineEdit(page, { op: "delete", line: 9 }));
    expect(heading.slice(7, 11)).toEqual(["- [x] second", "", "# Later", "text"]);
  });

  it("ignores heading marks inside a code block", () => {
    const body = "# A\n```\n# not\n```\n- x\n# B";
    const out = lines(applyOutlineEdit(body, { op: "delete", line: 0 }));
    expect(out).toEqual(["# B"]);
  });
});

describe("applyOutlineEdit move", () => {
  const doc = [
    "# A", // 0
    "", // 1
    "- one", // 2
    "  - one.1", // 3
    "- two", // 4
    "", // 5
    "## A1", // 6
    "", // 7
    "- deep", // 8
    "", // 9
    "# B", // 10
    "", // 11
    "1. bee", // 12
    "", // 13
    "# C", // 14
    "c text", // 15
  ].join("\n");

  it("moves an item after its sibling, nested lines and all", () => {
    expect(lines(applyOutlineEdit(doc, { op: "move", line: 2, target: 4, place: "after" })).slice(0, 5))
      .toEqual(["# A", "", "- two", "- one", "  - one.1"]);
  });

  it("moves an item before an item of another list, taking that list's marker", () => {
    const out = lines(applyOutlineEdit(doc, { op: "move", line: 4, target: 12, place: "before" }));
    expect(out).toContain("1. two");
    expect(out.indexOf("1. two")).toBe(out.indexOf("1. bee") - 1);
    expect(out).not.toContain("- two");
  });

  it("moves an item into a heading, joining the list there", () => {
    const out = lines(applyOutlineEdit(doc, { op: "move", line: 8, target: 10, place: "under" }));
    expect(out.slice(out.indexOf("# B"), out.indexOf("# B") + 4)).toEqual(["# B", "", "1. bee", "2. deep"]);
    expect(out.join("\n")).toContain("## A1\n\n# B");
  });

  it("moves a heading section after another, keeping its level", () => {
    const out = lines(applyOutlineEdit(doc, { op: "move", line: 0, target: 10, place: "after" }));
    expect(out.slice(0, 4)).toEqual(["# B", "", "1. bee", ""]);
    expect(out.indexOf("# A")).toBeLessThan(out.indexOf("# C"));
    expect(out).toContain("## A1");
  });

  it("moves a heading under another, one level deeper with its subheadings", () => {
    const out = lines(applyOutlineEdit(doc, { op: "move", line: 0, target: 14, place: "under" }));
    expect(out).toContain("## A");
    expect(out).toContain("### A1");
    expect(out.indexOf("## A")).toBeGreaterThan(out.indexOf("c text"));
  });

  it("refuses moves that make no sense", () => {
    expect(applyOutlineEdit(doc, { op: "move", line: 0, target: 6, place: "after" })).toBeNull(); // into itself
    expect(applyOutlineEdit(doc, { op: "move", line: 0, target: 2, place: "after" })).toBeNull(); // heading next to an item
    expect(applyOutlineEdit(doc, { op: "move", line: 2, target: 10, place: "after" })).toBeNull(); // item beside a heading
    expect(applyOutlineEdit(doc, { op: "move", line: 2, target: 2, place: "after" })).toBeNull();
  });
});
