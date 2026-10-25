// SPDX-License-Identifier: AGPL-3.0-only
import { describe, it, expect } from "vitest";
import type { PageCitation } from "./tauri";
import {
  CITATION_ANCHOR_PREFIX,
  processCitations,
  citationSourceNumbers,
  stripCitationLinks,
  citationDisplayLabel,
} from "./pageCitations";

const cite = (
  occurrence: number,
  marker: number,
  over: Partial<PageCitation> = {},
): PageCitation => ({
  occurrence,
  marker,
  source_kind: "memory",
  locator: `mem-${marker}`,
  score: 0.9,
  status: "verified",
  scope: "sentence",
  ...over,
});

const kindLabels = {
  memory: "Memory",
  external_url: "Link",
  external_file: "File",
  authored: "Written here",
} as const;

describe("processCitations", () => {
  it("returns state none and unchanged content when no markers and no citations", () => {
    const r = processCitations("Plain prose. No markers here.", undefined);
    expect(r.state).toBe("none");
    expect(r.content).toBe("Plain prose. No markers here.");
    expect(r.byOccurrence.size).toBe(0);
  });

  it("rewrites markers to occurrence-indexed citation links in body order", () => {
    const r = processCitations("A claim.[1] Another.[2]", [cite(1, 1), cite(2, 2)]);
    expect(r.state).toBe("cited");
    expect(r.content).toBe("A claim.[1](#citation:1) Another.[2](#citation:2)");
    expect(r.byOccurrence.get(1)?.locator).toBe("mem-1");
    expect(r.byOccurrence.get(2)?.locator).toBe("mem-2");
  });

  it("drops a marker on the reader's own sentence and keeps the visible numbers contiguous", () => {
    const r = processCitations("Mine [1]. Sourced [2].", [cite(1, 1, { source_kind: "authored" }), cite(2, 2)]);
    expect(r.content).toBe(`Mine. Sourced [2](${CITATION_ANCHOR_PREFIX}2).`);
    expect(r.sourceNumberByOccurrence.get(2)).toBe(1);
  });

  it("numbers multi-source runs like [1][3] by occurrence, not marker", () => {
    const r = processCitations("Claim.[1][3]", [cite(1, 1), cite(2, 3)]);
    expect(r.state).toBe("cited");
    expect(r.content).toBe("Claim.[1](#citation:1)[2](#citation:2)");
  });

  it("counts matches inside fenced code but does not rewrite them", () => {
    const content = "```\narr[1] access\n```\nA claim.[2]";
    const r = processCitations(content, [cite(1, 1), cite(2, 2)]);
    expect(r.state).toBe("cited");
    expect(r.content).toBe("```\narr[1] access\n```\nA claim.[2](#citation:2)");
  });

  it("counts matches inside inline code but does not rewrite them", () => {
    const content = "Use `x[1]` here.[2]";
    const r = processCitations(content, [cite(1, 1), cite(2, 2)]);
    expect(r.state).toBe("cited");
    expect(r.content).toBe("Use `x[1]` here.[2](#citation:2)");
  });

  it("falls back to strip-all when match count disagrees with citations length", () => {
    const r = processCitations("A.[1] B.[2]", [cite(1, 1)]);
    expect(r.state).toBe("stripped-mismatch");
    expect(r.content).toBe("A. B.");
    expect(r.byOccurrence.size).toBe(0);
  });

  it("falls back to strip-all when a marker value disagrees", () => {
    const r = processCitations("A.[5]", [cite(1, 1)]);
    expect(r.state).toBe("stripped-mismatch");
    expect(r.content).toBe("A.");
    expect(r.content).not.toContain("#citation");
  });

  it("display-strips markers when citations are empty (user-edited page)", () => {
    const r = processCitations("A. [1] B.[2]", []);
    expect(r.state).toBe("stripped-empty");
    expect(r.content).toBe("A. B.");
  });

  it("display-strips markers when citations are absent (old daemon shape)", () => {
    const r = processCitations("A.[1] B.", undefined);
    expect(r.state).toBe("stripped-empty");
    expect(r.content).toBe("A. B.");
  });

  it("collapses doubled spaces left by stripping, mirroring backend strip_markers", () => {
    const r = processCitations("A. [1] middle [2] B.", []);
    expect(r.content).toBe("A. middle B.");
  });

  it("drops the space a marker before the period leaves behind, but not one inside code", () => {
    const r = processCitations("Stores all data in one file [5][13]. Run `rm -rf .` first [2].", []);
    expect(r.state).toBe("stripped-empty");
    expect(r.content).toBe("Stores all data in one file. Run `rm -rf .` first.");
  });
});

describe("citation source numbers", () => {
  it("reuses a source's first number when it is cited again", () => {
    const content = "A.[1] B.[2] C.[3] D.[4] E.[5] F.[1]";
    const r = processCitations(content, [cite(1, 1), cite(2, 2), cite(3, 3), cite(4, 4), cite(5, 5), cite(6, 1)]);
    expect(r.state).toBe("cited");
    expect([...r.sourceNumberByOccurrence]).toEqual([[1, 1], [2, 2], [3, 3], [4, 4], [5, 5], [6, 1]]);
    // The link still points at its own occurrence, so each chip opens its own claim.
    expect(r.content).toContain("F.[6](#citation:6)");
  });

  it("gives the Nth distinct source number N whatever marker the writer used", () => {
    const r = processCitations("A.[3] B.[1] C.[3] D.[2]", [cite(1, 3), cite(2, 1), cite(3, 3), cite(4, 2)]);
    expect([...r.sourceNumberByOccurrence]).toEqual([[1, 1], [2, 2], [3, 1], [4, 3]]);
  });

  it("keys a source by kind and locator, not by marker", () => {
    const url = { source_kind: "external_url" as const, locator: "https://example.com/a" };
    const numbers = citationSourceNumbers([
      cite(1, 1, url),
      cite(2, 2),
      cite(3, 3, url),
      cite(4, 4, { source_kind: "external_file", locator: "https://example.com/a" }),
    ]);
    expect([...numbers]).toEqual([[1, 1], [2, 2], [3, 1], [4, 3]]);
  });

  it("numbers in reading order even when the wire order differs", () => {
    const numbers = citationSourceNumbers([cite(3, 1), cite(1, 2), cite(2, 1)]);
    expect(numbers.get(1)).toBe(1);
    expect(numbers.get(2)).toBe(2);
    expect(numbers.get(3)).toBe(2);
  });

  it("does not spend a number on a marker shown raw inside code", () => {
    const r = processCitations("Use `x[1]` here.[2] Then.[1]", [cite(1, 1), cite(2, 2), cite(3, 1)]);
    expect(r.sourceNumberByOccurrence.has(1)).toBe(false);
    expect(r.sourceNumberByOccurrence.get(2)).toBe(1);
    expect(r.sourceNumberByOccurrence.get(3)).toBe(2);
  });

  it("is empty when the citations are not shown", () => {
    expect(processCitations("A.[1]", []).sourceNumberByOccurrence.size).toBe(0);
    expect(processCitations("A.[1] B.[2]", [cite(1, 1)]).sourceNumberByOccurrence.size).toBe(0);
  });
});

describe("stripCitationLinks", () => {
  it("removes rewritten citation links and collapses spacing", () => {
    expect(stripCitationLinks("First claim [1](#citation:1) done.")).toBe(
      "First claim done.",
    );
    expect(stripCitationLinks("Tail.[2](#citation:2)")).toBe("Tail.");
    expect(stripCitationLinks("One file [5](#citation:5)[13](#citation:13).")).toBe("One file.");
  });
});

describe("citationDisplayLabel", () => {
  it("uses a localized source kind for imported memory locators", () => {
    expect(
      citationDisplayLabel(
        cite(1, 1, { locator: "import_20260908T150227Z_0_0" }),
        kindLabels,
      ),
    ).toBe("Memory");
  });

  it("keeps a short URL hostname beside its localized kind", () => {
    const c = cite(1, 1, {
      source_kind: "external_url",
      locator: "https://docs.rs/serde/latest/serde/",
    });
    expect(citationDisplayLabel(c, kindLabels)).toBe("Link · docs.rs");
  });

  it("keeps a short file basename beside its localized kind", () => {
    const c = cite(1, 1, {
      source_kind: "external_file",
      locator: "/Users/l/notes/design.md",
    });
    expect(citationDisplayLabel(c, kindLabels)).toBe("File · design.md");
  });

  it("keeps a relative file locator intact", () => {
    const c = cite(1, 1, {
      source_kind: "external_file",
      locator: "design.md",
    });
    expect(citationDisplayLabel(c, kindLabels)).toBe("File · design.md");
  });

  it("does not put a long external basename in prose", () => {
    const c = cite(1, 1, {
      source_kind: "external_file",
      locator: "/Users/l/notes/a-file-name-that-is-too-long-to-inline.md",
    });
    expect(citationDisplayLabel(c, kindLabels)).toBe("File");
  });

  it("labels authored citations with the localized source kind", () => {
    expect(
      citationDisplayLabel(cite(1, 1, { source_kind: "authored" }), kindLabels),
    ).toBe("Written here");
  });
});
