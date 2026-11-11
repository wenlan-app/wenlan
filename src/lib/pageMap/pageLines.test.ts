// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";

import type { PageMap, PageMapNode, PageOutlineNode } from "../tauri";
import { PAGE_LINE_ROOT, foldableHeadings, moreFor, outlineLine, withPageLines, withPageLinesDetailed } from "./pageLines";

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

describe("withPageLinesDetailed doors", () => {
  const door = (id: string, page: string): PageOutlineNode => ({
    ...line(id, "h", "item"),
    links: [{ target: page, title: page, page_id: page, whole_line: true }],
  });
  const centre: PageMap = {
    page_id: "p1",
    revision: 0,
    map_schema: 2,
    nodes: [],
    edges: [],
    outline: [line("h", null, "heading"), door("h#1", "p2"), door("h#2", "p2"), door("h#3", "p1")],
  };
  const options = {
    pageId: "p1",
    pageTitle: "One",
    open: new Set<string>(),
    showSuggestions: false,
    showRestLabel: "rest",
  };

  it("leaves links as plain items when doors are not asked for", () => {
    const { meta } = withPageLinesDetailed(centre, options);
    expect(meta.size).toBe(0);
  });

  it("makes the first link to a page its door, a later one a copy, and a link to the center a way back", () => {
    const { meta } = withPageLinesDetailed(centre, {
      ...options,
      doors: { centreId: "p1", open: new Set(), foreign: new Map(), foreignOpen: new Set(), backLabel: "back" },
    });
    expect(meta.get("outline:h#1")?.kind).toBe("door");
    expect(meta.get("outline:h#2")).toMatchObject({ kind: "dup", doorId: "outline:h#1" });
    expect(meta.get("outline:h#3")?.kind).toBe("back");
  });

  it("grows an opened door's headings only, and folds a long heading to two items", () => {
    const other: PageMap = {
      page_id: "p2",
      revision: 0,
      map_schema: 2,
      nodes: [],
      edges: [],
      outline: [
        line("a", null, "heading"),
        line("a#1", "a", "item"),
        line("a#2", "a", "item"),
        line("a#3", "a", "item"),
        line("a#4", "a", "item"),
      ],
    };
    const ids = (foreignOpen: string[]) =>
      withPageLinesDetailed(centre, {
        ...options,
        doors: {
          centreId: "p1",
          open: new Set(["outline:h#1"]),
          foreign: new Map([["outline:h#1", other]]),
          foreignOpen: new Set(foreignOpen),
          backLabel: "back",
        },
      })
        .nodes.map((n) => n.id)
        .filter((id) => id.startsWith("outline:in:"));
    const heading = "outline:in:h#1|a";
    expect(ids([])).toEqual([heading]);
    expect(ids([heading])).toEqual([heading, `${heading}#1`, `${heading}#2`, "outline:in:h#1|more:a"]);
    expect(ids([heading, `${heading}#all`])).toHaveLength(5);
  });
});

describe("withPageLinesDetailed links inside longer lines", () => {
  const link = (page_id: string, title: string, whole_line = false) => ({ target: title, title, page_id, whole_line });
  const item = (id: string, parent: string, label: string, links: ReturnType<typeof link>[]): PageOutlineNode => ({
    ...line(id, parent, "item", label),
    links,
  });
  const centre = mapWith([], [
    { ...line("h", null, "heading"), links: [link("p5", "Para")] },
    item("h#1", "h", "參考 工作流程", [link("p2", "工作流程")]),
    item("h#2", "h", "see A and B", [link("p3", "A"), link("p4", "B")]),
  ]);
  const doors = { centreId: "p1", open: new Set<string>(), foreign: new Map(), foreignOpen: new Set<string>(), backLabel: "back" };
  const options = { ...base, open: new Set<string>(), doors };

  it("UX-VIEW-9 makes a line with one link the door itself, with its full text and its own id", () => {
    const { nodes, meta } = withPageLinesDetailed(centre, options);
    expect(meta.get("outline:h#1")?.kind).toBe("door");
    expect(nodes.find((n) => n.id === "outline:h#1")?.label).toBe("參考 工作流程");
    // No second door row for the same link.
    expect(nodes.some((n) => n.id.startsWith("outline:door:h#1"))).toBe(false);
  });

  it("UX-VIEW-9 keeps a line with several links a line and hangs each link under it", () => {
    const { nodes, meta } = withPageLinesDetailed(centre, options);
    expect(meta.get("outline:h#2")).toBeUndefined();
    const under = nodes.filter((n) => n.parent_id === "outline:h#2");
    expect(under.map((n) => n.label)).toEqual(["A", "B"]);
    expect(under.every((n) => meta.get(n.id)?.kind === "door")).toBe(true);
  });

  it("UX-VIEW-9 puts a paragraph link under its heading after the items", () => {
    const { nodes } = withPageLinesDetailed(centre, options);
    const kids = nodes.filter((n) => n.parent_id === "outline:h").sort((a, b) => a.rank - b.rank);
    expect(kids.map((n) => n.id)).toEqual(["outline:h#1", "outline:h#2", "outline:door:h:0"]);
  });

  it("UX-VIEW-9 reads a line that links back to the centre with its own words", () => {
    const map = mapWith([], [line("h", null, "heading"), item("h#1", "h", "回到 中心", [link("p1", "中心")])]);
    const { nodes, meta } = withPageLinesDetailed(map, options);
    expect(meta.get("outline:h#1")?.kind).toBe("back");
    expect(nodes.find((n) => n.id === "outline:h#1")?.label).toBe("回到 中心 ↩");
  });

  it("UX-VIEW-9 reads links inside an opened door as grey lines, in reading order", () => {
    const other: PageMap = {
      page_id: "p2",
      revision: 0,
      map_schema: 2,
      nodes: [],
      edges: [],
      outline: [
        { ...line("a", null, "heading"), links: [link("p7", "段落")] },
        line("a#1", "a", "item", "設 25 分鐘"),
        item("a#2", "a", "練 專注力", [link("p6", "專注力")]),
        item("a#3", "a", "回到 中心", [link("p1", "中心")]),
        item("a#4", "a", "專注力", [link("p6", "專注力", true)]),
      ],
    };
    const { nodes } = withPageLinesDetailed(centre, {
      ...base,
      open: new Set(),
      doors: { ...doors, open: new Set(["outline:h#1"]), foreign: new Map([["outline:h#1", other]]), foreignOpen: new Set(["outline:in:h#1|a", "outline:in:h#1|a#all"]) },
    });
    const kids = nodes.filter((n) => n.parent_id === "outline:in:h#1|a").sort((a, b) => a.rank - b.rank);
    expect(kids.map((n) => n.label)).toEqual(["設 25 分鐘", "練 專注力 →", "回到 中心 ↩", "專注力 →", "段落 →"]);
  });
});
