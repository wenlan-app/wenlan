// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";

import type { PageMap, PageMapNode, PageOutlineNode } from "../tauri";
import { PAGE_LINE_ROOT, foldableHeadings, moreFor, outlineLine, withPageLines } from "./pageLines";

function line(id: string, parent: string | null, kind: "heading" | "item", label = id): PageOutlineNode {
  return { id, parent_id: parent, kind, label, text: label };
}

function stored(id: string, parent: string | null, extra: Partial<PageMapNode> = {}): PageMapNode {
  return {
    id,
    parent_id: parent,
    rank: 0,
    ref_kind: "memory",
    ref_id: id,
    label: id,
    status: "active",
    pinned: false,
    placed: false,
    collapsed: false,
    x: null,
    y: null,
    width: null,
    height: null,
    ref_state: "live",
    ...extra,
  };
}

function mapWith(nodes: PageMapNode[], outline: PageOutlineNode[]): PageMap {
  return { page_id: "p1", revision: 0, map_schema: 1, nodes, edges: [], outline } as PageMap;
}

const base = { pageId: "p1", pageTitle: "Page", showSuggestions: false, showRestLabel: "Show the rest" };

describe("withPageLines", () => {
  it("UX-VIEW-4 draws the page's own headings and items with nothing to accept", () => {
    const map = mapWith([], [line("h:a", null, "heading"), line("h:a#1", "h:a", "item")]);
    const nodes = withPageLines(map, { ...base, open: new Set() });
    expect(nodes.map((n) => [n.id, n.parent_id, n.status])).toEqual([
      [PAGE_LINE_ROOT, null, "active"],
      ["outline:h:a", PAGE_LINE_ROOT, "active"],
      ["outline:h:a#1", "outline:h:a", "active"],
    ]);
  });

  it("hangs the page's lines under the stored root, drops old section boxes and hidden suggestions", () => {
    const map = mapWith(
      [
        stored("root", null, { ref_kind: "page" }),
        stored("sec", "root", { ref_kind: "section" }),
        stored("guess", "root", { status: "suggested" }),
        stored("kept", "root"),
      ],
      [line("h:a", null, "heading")],
    );
    const ids = withPageLines(map, { ...base, open: new Set() }).map((n) => n.id);
    expect(ids).toEqual(["root", "kept", "outline:h:a"]);
    const shown = withPageLines(map, { ...base, showSuggestions: true, open: new Set() });
    expect(shown.map((n) => n.id)).toContain("guess");
  });

  it("folds a long heading to two items plus a show-the-rest box until opened", () => {
    const outline = [
      line("h:a", null, "heading"),
      ...[1, 2, 3, 4].map((n) => line(`h:a#${n}`, "h:a", "item")),
    ];
    const map = mapWith([], outline);
    const folded = withPageLines(map, { ...base, open: new Set() }).map((n) => n.id);
    expect(folded).toEqual([PAGE_LINE_ROOT, "outline:h:a", "outline:h:a#1", "outline:h:a#2", "outline:more:h:a"]);
    expect(moreFor("outline:more:h:a")).toBe("h:a");
    const open = withPageLines(map, { ...base, open: new Set(["h:a"]) }).map((n) => n.id);
    expect(open).toHaveLength(6);
    expect(open).not.toContain("outline:more:h:a");
    expect([...foldableHeadings(map)]).toEqual(["h:a"]);
  });

  it("keeps the stored map untouched when the daemon sends no outline", () => {
    const map = mapWith([stored("root", null)], []);
    expect(withPageLines(map, { ...base, open: new Set() }).map((n) => n.id)).toEqual(["root"]);
  });

  it("finds the outline line behind a box, but not for the root or a more box", () => {
    const map = mapWith([], [line("h:a", null, "heading")]);
    expect(outlineLine(map, "outline:h:a")?.id).toBe("h:a");
    expect(outlineLine(map, PAGE_LINE_ROOT)).toBeNull();
    expect(outlineLine(map, "outline:more:h:a")).toBeNull();
  });
});

describe("withPageLines on deep pages", () => {
  // page › h1 › h2 › h3 › h4 › item
  const outline = [
    line("h:1", null, "heading"),
    line("h:1/2", "h:1", "heading"),
    line("h:1/2/3", "h:1/2", "heading"),
    line("h:1/2/3/4", "h:1/2/3", "heading"),
    line("h:1/2/3/4#1", "h:1/2/3/4", "item"),
  ];
  const map = mapWith([], outline);

  it("draws three levels and keeps the rest folded under the third", () => {
    const ids = withPageLines(map, { ...base, open: new Set() }).map((n) => n.id);
    expect(ids).toEqual([PAGE_LINE_ROOT, "outline:h:1", "outline:h:1/2", "outline:h:1/2/3"]);
    expect(foldableHeadings(map).has("h:1/2/3")).toBe(true);
  });

  it("opens one level at a time", () => {
    const one = withPageLines(map, { ...base, open: new Set(["h:1/2/3"]) }).map((n) => n.id);
    expect(one).toContain("outline:h:1/2/3/4");
    expect(one).not.toContain("outline:h:1/2/3/4#1");
    expect(one).not.toContain("outline:more:h:1/2/3/4");
    const both = withPageLines(map, { ...base, open: new Set(["h:1/2/3", "h:1/2/3/4"]) }).map((n) => n.id);
    expect(both).toContain("outline:h:1/2/3/4#1");
    expect(both.some((id) => id.startsWith("outline:more:"))).toBe(false);
  });
});

describe("layoutIndented", () => {
  it("keeps a box dragged on the wide canvas in its row, with no hole above the page's lines", async () => {
    const { layoutIndented } = await import("./tree");
    const nodes = [
      stored("root", null, { ref_kind: "page" }),
      stored("kept", "root", { placed: true, x: 900, y: -400 }),
      stored("next", "root"),
    ];
    const views = layoutIndented(nodes, new Map());
    const byId = new Map(views.map((v) => [v.node.id, v]));
    expect(byId.get("kept")!.y).toBeLessThan(byId.get("next")!.y);
    expect(byId.get("kept")!.x).toBeLessThan(200);
  });
});

describe("branch inks and focus", () => {
  it("gives each top-level heading its own ink and lets its lines share it", async () => {
    const { branchSlots } = await import("./pageLines");
    const map = mapWith([], [
      line("lead", null, "item"),
      line("h:a", null, "heading"),
      line("h:a#1", "h:a", "item"),
      line("h:b", null, "heading"),
      line("h:b/c", "h:b", "heading"),
      line("h:b/c#1", "h:b/c", "item"),
    ]);
    const slots = branchSlots(map);
    expect(slots.get("lead")).toBeUndefined();
    expect(slots.get("h:a#1")).toBe(slots.get("h:a"));
    expect(slots.get("h:b/c#1")).toBe(slots.get("h:b"));
    expect(slots.get("h:a")).not.toBe(slots.get("h:b"));
  });

  it("knows which lines sit under a focused heading", async () => {
    const { withinLine, lineIdOf } = await import("./pageLines");
    const map = mapWith([], [line("h:a", null, "heading"), line("h:a/b", "h:a", "heading"), line("h:a/b#1", "h:a/b", "item"), line("h:c", null, "heading")]);
    expect(withinLine(map, "h:a", "h:a/b#1")).toBe(true);
    expect(withinLine(map, "h:a", "h:c")).toBe(false);
    expect(lineIdOf("outline:more:h:a/b")).toBe("h:a/b");
    expect(lineIdOf(PAGE_LINE_ROOT)).toBeNull();
  });
});

describe("shutting a branch", () => {
  it("hides everything under a shut heading, more boxes included, and opens again", async () => {
    const { branchHeadings } = await import("./pageLines");
    const outline = [
      line("h:a", null, "heading"),
      line("h:a/b", "h:a", "heading"),
      ...[1, 2, 3, 4].map((n) => line(`h:a/b#${n}`, "h:a/b", "item")),
      line("h:c", null, "heading"),
    ];
    const map = mapWith([], outline);
    expect([...branchHeadings(map)].sort()).toEqual(["h:a", "h:a/b"]);
    const shut = withPageLines(map, { ...base, open: new Set(), closed: new Set(["h:a"]) }).map((n) => n.id);
    expect(shut).toEqual([PAGE_LINE_ROOT, "outline:h:a", "outline:h:c"]);
    const inner = withPageLines(map, { ...base, open: new Set(), closed: new Set(["h:a/b"]) }).map((n) => n.id);
    expect(inner).toEqual([PAGE_LINE_ROOT, "outline:h:a", "outline:h:a/b", "outline:h:c"]);
  });
});

describe("withPageLines closed headings", () => {
  // # A > ## A1 > ### A1a > #### A1a-i > ##### deepest, plus a long ## A2.
  const outline = [
    line("h:a", null, "heading"),
    line("h:a1", "h:a", "heading"),
    line("h:a1a", "h:a1", "heading"),
    line("h:a1a-i", "h:a1a", "heading"),
    line("h:a1a-i#1", "h:a1a-i", "item"),
    line("h:a2", "h:a", "heading"),
    ...[1, 2, 3, 4, 5].map((n) => line(`h:a2#${n}`, "h:a2", "item")),
  ];
  const open = (ids: string[] = []) => ({ ...base, open: new Set(ids) });
  const moreIds = (nodes: PageMapNode[]) => nodes.filter((n) => moreFor(n.id)).map((n) => moreFor(n.id));

  it("puts no show-the-rest box under a heading that starts shut for depth", () => {
    const nodes = withPageLines(mapWith([], outline), open());
    expect(nodes.some((n) => n.id.endsWith("h:a1a"))).toBe(true);
    expect(nodes.some((n) => n.id.endsWith("h:a1a-i"))).toBe(false);
    expect(moreIds(nodes)).not.toContain("h:a1a");
    // The long open heading still gets its box.
    expect(moreIds(nodes)).toEqual(["h:a2"]);
  });

  it("puts no box under a heading the person closed", () => {
    const nodes = withPageLines(mapWith([], outline), { ...open(), closed: new Set(["h:a2"]) });
    expect(moreIds(nodes)).toEqual([]);
  });
});
