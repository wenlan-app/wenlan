// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import {
  DEL_CLOSE,
  DEL_OPEN,
  INS_CLOSE,
  INS_OPEN,
  markdownDiff,
  splitBlocks,
} from "./markdownDiff";

const readable = (text: string) =>
  text
    .replaceAll(INS_OPEN, "{+")
    .replaceAll(INS_CLOSE, "+}")
    .replaceAll(DEL_OPEN, "[-")
    .replaceAll(DEL_CLOSE, "-]");

describe("markdownDiff", () => {
  it("keeps a heading's # outside the marks when its words change", () => {
    const out = readable(
      markdownDiff("# Reading position\n\nText.", "# Reading place\n\nText.", "unified"),
    );
    expect(out).toBe("# Reading [-position-] {+place+}\n\nText.");
  });

  it("marks a whole added heading after its # so it still renders as one", () => {
    const out = readable(markdownDiff("Intro.", "Intro.\n\n## Reading position", "unified"));
    expect(out).toBe("Intro.\n\n## {+Reading position+}");
  });

  it("changes only the word inside bold, leaving the ** pair whole", () => {
    const out = readable(markdownDiff("a **b c** d", "a **b e** d", "unified"));
    expect(out).toBe("a **b [-c-] {+e+}** d");
  });

  it("keeps list items in one list and marks a new item after its bullet", () => {
    const out = readable(markdownDiff("- one\n- two", "- one\n- two\n- three", "unified"));
    expect(out).toBe("- one\n- two\n- {+three+}");
  });

  it("shows each side alone for side by side", () => {
    expect(readable(markdownDiff("# A b\n\nx", "# A c\n\nx", "before"))).toBe("# A [-b-]\n\nx");
    expect(readable(markdownDiff("# A b\n\nx", "# A c\n\nx", "after"))).toBe("# A {+c+}\n\nx");
  });

  it("diffs code fences by line and turns a changed diagram into its source", () => {
    const before = "```mermaid\ngraph TD\nA-->B\n```";
    const after = "```mermaid\ngraph TD\nA-->C\n```";
    expect(readable(markdownDiff(before, after, "unified"))).toBe(
      "```\ngraph TD\n[-A-->B-]\n{+A-->C+}\n```",
    );
    expect(markdownDiff(before, before, "unified")).toBe(before);
  });

  it("marks table cells, not the pipes", () => {
    const before = "| a | b |\n| - | - |\n| 1 | 2 |";
    const after = "| a | b |\n| - | - |\n| 1 | 2 |\n| 3 | 4 |";
    expect(readable(markdownDiff(before, after, "unified"))).toBe(
      "| a | b |\n| - | - |\n| 1 | 2 |\n| {+3+} | {+4+} |",
    );
  });

  it("marks only the changed cell of an edited row", () => {
    const before = "| Setting | Default |\n| --- | --- |\n| Save interval | 5 seconds |";
    const after = "| Setting | Default |\n| --- | --- |\n| Save interval | 2 seconds |";
    expect(readable(markdownDiff(before, after, "unified"))).toBe(
      "| Setting | Default |\n| --- | --- |\n| Save interval | [-5-] {+2+} seconds |",
    );
    expect(readable(markdownDiff(before, after, "before"))).toBe(
      "| Setting | Default |\n| --- | --- |\n| Save interval | [-5-] seconds |",
    );
  });

  it("reads a setext heading as a heading", () => {
    const [block] = splitBlocks("Title\n=====\n\ntext");
    expect(block.kind).toBe("heading");
    expect(block.prefix).toBe("# ");
  });

  it("returns the page itself when nothing changed", () => {
    const page = "## T\n\nA [link](https://x.y) and `code`.\n\n> quote\n> more\n\n1. one\n2. two";
    expect(markdownDiff(page, page, "unified")).toBe(page);
  });

  it("leaves out an unchanged leading # title, which the dialog already shows", () => {
    expect(markdownDiff("# Title\n\nold", "# Title\n\nnew", "after")).toBe(`${INS_OPEN}new${INS_CLOSE}`);
    expect(readable(markdownDiff("# Title a\n\nx", "# Title b\n\nx", "unified"))).toBe("# Title [-a-] {+b+}\n\nx");
  });
});
