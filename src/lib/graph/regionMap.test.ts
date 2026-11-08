import { describe, expect, it } from "vitest";
import Graph from "graphology";
import { layoutRegionMap, nudgeClearOfOthers, placeNewRegionMembers, REGION_ATTR } from "./regionMap";
import { cartographyScene, fitRegionName, placeRegionLabels, planRegionNamesFirst, REGION_NAME_EDGE_PX, regionNameKeepsEnough } from "./cartography";
import { readPreviousRegions, readStoredRegions, startRearrange, storeRegions, undoRearrange } from "./positions";

/** Four dense groups of twelve, tied by a line or two each. */
function fourGroups(): Graph {
  const g = new Graph({ type: "undirected", multi: true });
  for (const group of ["a", "b", "c", "d"]) {
    for (let i = 0; i < 12; i += 1) g.addNode(`${group}${i}`, { label: `${group}${i}`, entityType: "concept", size: 3, x: 0, y: 0 });
    for (let i = 0; i < 12; i += 1) for (let j = i + 1; j < 12; j += 1) if ((i + j) % 3 !== 0) g.addEdge(`${group}${i}`, `${group}${j}`, { edgeType: "related_to" });
  }
  g.addEdge("a0", "b0", { edgeType: "related_to" });
  g.addEdge("b1", "c1", { edgeType: "related_to" });
  g.addEdge("c2", "d2", { edgeType: "related_to" });
  return g;
}

const regionsOf = (g: Graph) => {
  const m = new Map<string, string>();
  g.forEachNode((id, a) => {
    if (typeof a[REGION_ATTR] === "string") m.set(id, a[REGION_ATTR] as string);
  });
  return m;
};

describe("R2 kept regions (round 4)", () => {
  it("keeps every kept body's region when the map grows, and new bodies join by their links", () => {
    const before = fourGroups();
    layoutRegionMap(before, { scope: "t", regionCount: { min: 3, max: 5 } });
    const kept = regionsOf(before);
    expect(new Set(kept.values()).size).toBe(4);

    const grown = fourGroups();
    before.forEachNode((id, a) => grown.mergeNodeAttributes(id, { x: a.x, y: a.y }));
    // A new body tied into group a, a new leaf off c, and a line that would
    // pull a3 toward b if regions were found again.
    grown.addNode("n1", { label: "n1", entityType: "concept", size: 3, x: 0, y: 0 });
    grown.addEdge("n1", "a2", { edgeType: "related_to" });
    grown.addEdge("n1", "a5", { edgeType: "related_to" });
    grown.addNode("n2", { label: "n2", entityType: "concept", size: 3, x: 0, y: 0 });
    grown.addEdge("n2", "c4", { edgeType: "related_to" });
    for (let i = 2; i < 6; i += 1) grown.addEdge("a3", `b${i}`, { edgeType: "related_to" });
    layoutRegionMap(grown, { scope: "t", assignOnly: true, stored: kept });
    const after = regionsOf(grown);
    for (const [id, key] of kept) expect(after.get(id)).toBe(key);
    expect(after.get("n1")).toBe(kept.get("a2"));
    expect(after.get("n2")).toBe(kept.get("c4"));
    expect(new Set(after.values()).size).toBe(4);

    const positions = new Map<string, { x: number; y: number }>();
    before.forEachNode((id, a) => positions.set(id, { x: a.x as number, y: a.y as number }));
    const spots = placeNewRegionMembers(grown, positions);
    expect([...spots.keys()].sort()).toEqual(["n1", "n2"]);
    const c4 = positions.get("c4")!;
    const n2 = spots.get("n2")!;
    expect(Math.hypot(n2.x - c4.x, n2.y - c4.y)).toBeLessThan(40);
  });

  it("stores regions per scope and moves them with Rearrange and its undo", () => {
    window.localStorage.clear();
    storeRegions("s", new Map([["x", "c1"]]));
    expect(readStoredRegions("s").get("x")).toBe("c1");
    expect(readStoredRegions("other").size).toBe(0);
    startRearrange("s");
    expect(readStoredRegions("s").size).toBe(0);
    expect(readPreviousRegions("s").get("x")).toBe("c1");
    storeRegions("s", new Map([["x", "c2"]]));
    undoRearrange("s");
    expect(readStoredRegions("s").get("x")).toBe("c1");
    expect(readPreviousRegions("s").size).toBe(0);
  });
});

describe("R2 region names keep off the points (round 4)", () => {
  const scene = () => {
    const g = new Graph({ type: "undirected" });
    // A tight cluster round the middle, where the name would first go.
    for (let i = 0; i < 16; i += 1) g.addNode(`p${i}`, { label: `p${i}`, entityType: "concept", size: 5, x: 200 + 30 * (i % 4), y: 200 + 30 * Math.floor(i / 4) });
    for (let i = 1; i < 16; i += 1) g.addEdge("p0", `p${i}`);
    return { g, scene: cartographyScene(g, new Map(g.nodes().map((id) => [id, "c"]))) };
  };
  const project = (p: { x: number; y: number }) => ({ x: p.x, y: p.y });
  const measure = (text: string, size: number) => text.length * size * 0.6;

  it("moves a name to free ground instead of over a point", () => {
    const { g, scene: s } = scene();
    const discs = g.nodes().map((id) => ({ x: g.getNodeAttribute(id, "x") as number, y: g.getNodeAttribute(id, "y") as number, r: 5 }));
    const blocked = (b: { left: number; right: number; top: number; bottom: number }) =>
      discs.some((d) => Math.hypot(d.x - Math.max(b.left, Math.min(d.x, b.right)), d.y - Math.max(b.top, Math.min(d.y, b.bottom))) < d.r + 1);
    const placed = placeRegionLabels(s, project, measure, { width: 800, height: 800 }, undefined, [], blocked);
    expect(placed).toHaveLength(1);
    const [name] = placed;
    const half = measure(name!.name, name!.size) / 2;
    expect(blocked({ left: name!.x - half, right: name!.x + half, top: name!.y - name!.size / 2, bottom: name!.y + name!.size / 2 })).toBe(false);
  });

  it("leaves a region unnamed rather than cover something", () => {
    const { scene: s } = scene();
    expect(placeRegionLabels(s, project, measure, { width: 800, height: 800 }, undefined, [], () => true)).toHaveLength(0);
  });
});

describe("R2 region names go first at overview (round 5)", () => {
  const measure = (text: string, size: number) => text.length * size * 0.6;
  const project = (p: { x: number; y: number }) => ({ x: p.x, y: p.y });

  it("names every region, no cap, without covering points or each other", () => {
    const g = new Graph({ type: "undirected" });
    const communities = new Map<string, string>();
    // Twenty small regions on a grid: more than the old cap of twelve.
    for (let r = 0; r < 20; r += 1) {
      const ox = 80 + 180 * (r % 5);
      const oy = 80 + 160 * Math.floor(r / 5);
      for (let i = 0; i < 10; i += 1) {
        const id = `r${r}p${i}`;
        g.addNode(id, { label: `region${r}`, entityType: "concept", size: 4, x: ox + 20 * (i % 5), y: oy + 20 * Math.floor(i / 5) });
        communities.set(id, `c${r}`);
        if (i > 0) g.addEdge(`r${r}p0`, id);
      }
    }
    const discs = g.nodes().map((id) => ({ x: g.getNodeAttribute(id, "x") as number, y: g.getNodeAttribute(id, "y") as number, r: 4 }));
    const blocked = (b: { left: number; right: number; top: number; bottom: number }) =>
      discs.some((d) => Math.hypot(d.x - Math.max(b.left, Math.min(d.x, b.right)), d.y - Math.max(b.top, Math.min(d.y, b.bottom))) < d.r + 1);
    const names = planRegionNamesFirst(cartographyScene(g, communities), project, measure, { width: 1000, height: 800 }, blocked);
    expect(names).toHaveLength(20);
    for (const [i, a] of names.entries()) {
      expect(blocked(a.box)).toBe(false);
      for (const b of names.slice(i + 1)) {
        const hit = a.box.left < b.box.right && b.box.left < a.box.right && a.box.top < b.box.bottom && b.box.top < a.box.bottom;
        expect(hit).toBe(false);
      }
    }
  });

  it("cuts a long name with an ellipsis to fit", () => {
    expect(fitRegionName("short", 12, 220, measure)).toBe("short");
    const cut = fitRegionName("a very long region name that runs on and on", 12, 120, measure);
    expect(cut.endsWith("…")).toBe(true);
    expect(measure(cut, 12)).toBeLessThanOrEqual(120);
  });
});

describe("R2 names and new points (round 6)", () => {
  const measure = (text: string, size: number) => text.length * size * 0.6;
  const project = (p: { x: number; y: number }) => ({ x: p.x, y: p.y });

  it("a shortened name keeps its first words, or it does not count", () => {
    expect(regionNameKeepsEnough("Lucian Threads Writing Workflow Index", "Lucian Threads Writ…")).toBe(true);
    expect(regionNameKeepsEnough("Lucian Threads Writing Workflow Index", "Lucian Thr…")).toBe(false);
    expect(regionNameKeepsEnough("ultrapowers", "ultr…")).toBe(true);
    expect(regionNameKeepsEnough("全渠道品牌與內容共用", "全渠…")).toBe(false);
  });

  it("names a crowded region after its best-linked point, on its own region", () => {
    const g = new Graph({ type: "undirected" });
    const communities = new Map<string, string>();
    // One region whose name is far too long for the room around it.
    for (let i = 0; i < 6; i += 1) {
      const id = `p${i}`;
      g.addNode(id, { label: i === 0 ? "An extraordinarily long hub name nobody could fit anywhere" : `pt${i}`, entityType: "concept", size: 4, x: 100 + 12 * i, y: 100 });
      communities.set(id, "c0");
      if (i > 0) g.addEdge("p0", id);
    }
    g.addEdge("p1", "p2");
    const names = planRegionNamesFirst(cartographyScene(g, communities), project, (t, s) => measure(t, s) * 3, { width: 400, height: 300 }, () => false);
    expect(names).toHaveLength(1);
    expect(names[0]!.name).toBe("pt1");
    expect(names[0]!.hubId).toBe("p1");
  });

  it("keeps every name at its own region, never in free ground elsewhere", () => {
    const g = new Graph({ type: "undirected" });
    const communities = new Map<string, string>();
    for (let r = 0; r < 6; r += 1) {
      const ox = 100 + 250 * (r % 3);
      const oy = 100 + 250 * Math.floor(r / 3);
      for (let i = 0; i < 8; i += 1) {
        const id = `r${r}p${i}`;
        g.addNode(id, { label: `region ${r}`, entityType: "concept", size: 4, x: ox + 14 * (i % 4), y: oy + 14 * Math.floor(i / 4) });
        communities.set(id, `c${r}`);
        if (i > 0) g.addEdge(`r${r}p0`, id);
      }
    }
    const names = planRegionNamesFirst(cartographyScene(g, communities), project, measure, { width: 900, height: 700 }, () => false);
    expect(names).toHaveLength(6);
    for (const n of names) {
      const owner = communities.get(n.hubId)!;
      const own = g.nodes().filter((id) => communities.get(id) === owner);
      const gap = Math.min(...own.map((id) => {
        const x = g.getNodeAttribute(id, "x") as number, y = g.getNodeAttribute(id, "y") as number;
        return Math.hypot(Math.max(n.box.left - x, 0, x - n.box.right), Math.max(n.box.top - y, 0, y - n.box.bottom)) - 4;
      }));
      expect(gap).toBeLessThanOrEqual(REGION_NAME_EDGE_PX);
    }
  });

  it("new points keep their drawn size apart from every point", () => {
    const g = new Graph({ type: "undirected" });
    const kept = new Map<string, { x: number; y: number }>();
    for (let i = 0; i < 30; i += 1) {
      const id = `k${i}`;
      const p = { x: 10 * (i % 6), y: 10 * Math.floor(i / 6) };
      g.addNode(id, { size: 6, x: p.x, y: p.y, [REGION_ATTR]: "c0" });
      kept.set(id, p);
    }
    for (let i = 0; i < 25; i += 1) {
      g.addNode(`n${i}`, { size: 8, [REGION_ATTR]: "c0" });
      g.addEdge("k0", `n${i}`);
    }
    const unitsPerPx = 2;
    const placed = placeNewRegionMembers(g, kept, { unitsPerPx });
    expect(placed.size).toBe(25);
    const all = [...[...kept].map(([id, p]) => ({ id, ...p })), ...[...placed].map(([id, p]) => ({ id, ...p }))];
    for (const [id, p] of placed) {
      for (const q of all) {
        if (q.id === id) continue;
        const rr = ((g.getNodeAttribute(id, "size") as number) + (g.getNodeAttribute(q.id, "size") as number)) * unitsPerPx;
        expect(Math.hypot(p.x - q.x, p.y - q.y)).toBeGreaterThanOrEqual(rr);
      }
    }
    for (const [id, p] of kept) expect(placed.has(id) ? null : p).toEqual(kept.get(id));
  });
});

describe("R2 nudge after grow (round 6)", () => {
  it("moves only the placed point, until its disc is clear", () => {
    const g = new Graph({ type: "undirected" });
    for (let i = 0; i < 9; i += 1) g.addNode(`k${i}`, { size: 5, x: 12 * (i % 3), y: 12 * Math.floor(i / 3) });
    g.addNode("new", { size: 6, x: 12, y: 12 });
    const before = new Map(g.nodes().map((id) => [id, [g.getNodeAttribute(id, "x"), g.getNodeAttribute(id, "y")]]));
    expect(nudgeClearOfOthers(g, ["new"], 1)).toEqual(["new"]);
    for (const id of g.nodes()) if (id !== "new") expect([g.getNodeAttribute(id, "x"), g.getNodeAttribute(id, "y")]).toEqual(before.get(id));
    const p = { x: g.getNodeAttribute("new", "x") as number, y: g.getNodeAttribute("new", "y") as number };
    for (let i = 0; i < 9; i += 1) {
      expect(Math.hypot(p.x - (g.getNodeAttribute(`k${i}`, "x") as number), p.y - (g.getNodeAttribute(`k${i}`, "y") as number))).toBeGreaterThanOrEqual(11);
    }
    expect(nudgeClearOfOthers(g, ["new"], 1)).toEqual([]);
  });
});
