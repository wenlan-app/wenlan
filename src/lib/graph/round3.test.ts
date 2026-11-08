// SPDX-License-Identifier: AGPL-3.0-only
// Round 3 (balanced map): what can be checked without a browser. The B1-B9
// screen measures need a render and are checked by the headless render run.
import { beforeEach, describe, expect, it } from "vitest";
import Graph from "graphology";
import { townsGraph } from "./__fixtures__/towns";
import { MEMORY_NODE_TYPE } from "./model";
import { planDustBadges } from "./dust";
import {
  ATLAS_DUST_CROWDED_ATTR,
  ATLAS_DUST_ROOM_ATTR,
  annotateDust,
  markCrowdedDust,
  nodeDisplay,
  placeSatellites,
  satellitePlan,
  ATLAS_EDGE_FAN_ATTR,
  ATLAS_EDGE_TIER_ATTR,
  FAN_NEAR_MAX,
  edgeDisplay,
  lodFor,
  ATLAS_SKELETON_HUB_ATTR,
  markKeptLayout,
  LABEL_RING2_PUSH,
  createAtlasSimulation,
  labelCandidates,
  runAtlasLayout,
} from "./atlas";
import {
  ATLAS_LAYOUT_VERSION,
  alignToKept,
  readLayoutVersion,
  readStoredPositions,
  startRearrange,
  storeLayoutVersion,
  storePositions,
  undoRearrange,
} from "./positions";

const laidOut = (scale: number) => {
  const graph = townsGraph({ scale });
  runAtlasLayout(graph, "r1");
  createAtlasSimulation(graph).stop();
  return graph;
};

const positionsOf = (graph: Graph) =>
  new Map(graph.nodes().map((id) => [id, { x: graph.getNodeAttribute(id, "x") as number, y: graph.getNodeAttribute(id, "y") as number }]));

describe("round 3 balanced layout", () => {
  it("lays the same map out the same way every time", () => {
    const a = positionsOf(laidOut(0.2));
    const b = positionsOf(laidOut(0.2));
    expect(b).toEqual(a);
  });

  it("gives the towns map a landscape core between 1.3 and 1.8 wide", () => {
    const graph = laidOut(0.3);
    const hubs = graph.filterNodes((_id, attrs) => attrs[ATLAS_SKELETON_HUB_ATTR] === true);
    expect(hubs.length).toBeGreaterThan(4);
    const core = graph.filterNodes((_id, attrs) => attrs.island === false);
    const xs = core.map((id) => graph.getNodeAttribute(id, "x") as number);
    const ys = core.map((id) => graph.getNodeAttribute(id, "y") as number);
    const w = Math.max(...xs) - Math.min(...xs);
    const h = Math.max(...ys) - Math.min(...ys);
    const aspect = Math.max(w, h) / Math.min(w, h);
    expect(aspect).toBeGreaterThanOrEqual(1.3);
    expect(aspect).toBeLessThanOrEqual(1.8);
  }, 60_000);
});

describe("round 3 re-balance and put back", () => {
  beforeEach(() => window.localStorage.clear());

  it("re-balances a kept layout once per scope", () => {
    expect(readLayoutVersion("s")).toBe(0);
    storeLayoutVersion("s");
    expect(readLayoutVersion("s")).toBe(ATLAS_LAYOUT_VERSION);
    expect(readLayoutVersion("other")).toBe(0);
  });

  it("puts the old layout back exactly", () => {
    const old = new Map([["a", { x: 1.25, y: -3.5 }], ["b", { x: 40, y: 12.75 }]]);
    storePositions("s", old);
    startRearrange("s");
    storePositions("s", new Map([["a", { x: 900, y: 900 }], ["b", { x: -900, y: 5 }]]));
    undoRearrange("s");
    expect(readStoredPositions("s")).toEqual(old);
  });

  it("turns a fresh layout back onto the old one (Procrustes)", () => {
    const old = new Map<string, { x: number; y: number }>();
    for (let i = 0; i < 20; i += 1) old.set(`n${i}`, { x: Math.cos(i) * (50 + i * 7), y: Math.sin(i * 1.7) * 80 });
    const angle = 1.1;
    const fresh = new Map([...old].map(([id, { x, y }]) => [id, {
      x: 300 + x * Math.cos(angle) - y * Math.sin(angle),
      y: -120 + x * Math.sin(angle) + y * Math.cos(angle),
    }]));
    const aligned = alignToKept(fresh, old);
    for (const [id, p] of old) {
      expect(aligned.get(id)!.x).toBeCloseTo(p.x, 6);
      expect(aligned.get(id)!.y).toBeCloseTo(p.y, 6);
    }
  });

  it("keeps a re-balanced map's shape (a rigid move, no scaling)", () => {
    const fresh = new Map([["a", { x: 0, y: 0 }], ["b", { x: 10, y: 0 }], ["c", { x: 0, y: 30 }]]);
    const aligned = alignToKept(fresh, new Map([["a", { x: 5, y: 5 }], ["b", { x: 5, y: 500 }]]));
    const d = (m: Map<string, { x: number; y: number }>, p: string, q: string) => Math.hypot(m.get(p)!.x - m.get(q)!.x, m.get(p)!.y - m.get(q)!.y);
    expect(d(aligned, "a", "b")).toBeCloseTo(10, 6);
    expect(d(aligned, "a", "c")).toBeCloseTo(30, 6);
  });
});

describe("round 3 label spots (section 8)", () => {
  it("tries the four sides, then the same sides further out, then four corners", () => {
    const spots = labelCandidates(100, 100, 6, "right");
    expect(spots).toHaveLength(12);
    const near = spots.slice(0, 4).map((s) => Math.hypot(s.x - 100, s.y - 100));
    const far = spots.slice(4, 8).map((s) => Math.hypot(s.x - 100, s.y - 100));
    for (let i = 0; i < 4; i += 1) expect(far[i]).toBeCloseTo(near[i] + LABEL_RING2_PUSH, 6);
    const corners = spots.slice(8);
    expect(new Set(corners.map((s) => `${Math.sign(s.x - 100)},${Math.sign(s.y - 100)}`)).size).toBe(4);
  });
});

describe("round 1 polish: line tiers and reopening", () => {
  it("keeps every hub tie and one line up per member, and hides the rest at overview", () => {
    const graph = laidOut(0.2);
    const tiers = graph.mapEdges((_e, attrs) => attrs[ATLAS_EDGE_TIER_ATTR] as number | undefined);
    // Count node pairs, not edges: a pair can carry several parallel links.
    const tierOne = new Set(graph.filterEdges((_e, attrs) => attrs[ATLAS_EDGE_TIER_ATTR] === 1)
      .map((e) => [graph.source(e), graph.target(e)].sort().join("|"))).size;
    const tierZero = tiers.filter((t) => t === 0).length;
    expect(tierOne).toBeGreaterThan(0);
    expect(tierZero).toBeGreaterThan(0);
    // Tier 1 is a tree over the main component: never more lines than nodes.
    const main = graph.filterNodes((_id, attrs) => attrs.island === false).length;
    expect(tierOne).toBeLessThan(main);
  }, 60_000);

  it("reopens a kept map without moving it", () => {
    const graph = laidOut(0.2);
    const before = positionsOf(graph);
    const copy = graph.copy();
    markKeptLayout(copy, "r1");
    createAtlasSimulation(copy, undefined, { settled: true }).stop();
    const after = positionsOf(copy);
    for (const [id, p] of before) {
      expect(after.get(id)!.x).toBeCloseTo(p.x, 6);
      expect(after.get(id)!.y).toBeCloseTo(p.y, 6);
    }
    expect(copy.someEdge((_e, attrs) => attrs[ATLAS_EDGE_TIER_ATTR] === 1)).toBe(true);
  }, 60_000);
});

describe("round 1 polish: a huge hub's lines up close", () => {
  const palette = {
    project: "#111111", tool: "#222222", org: "#333333", person: "#444444", concept: "#555555",
    neutral: "#666666", edge: "#777777", edgeStrong: "#888888", label: "#999999", labelMuted: "#aaaaaa",
    surface: "#000000", graticule: "rgba(4,5,6,0.13)", bridge: "#bbbbbb", memory: "#cccccc", page: "#dddddd",
  };
  const rest = { hovered: null, neighbors: new Set<string>() };
  const show = (attrs: Record<string, unknown>, zoom: number) =>
    edgeDisplay(rest, "e", "a", "b", attrs, palette, lodFor(zoom)).hidden !== true;

  it("draws only the hub's nearest lines at the detail zoom", () => {
    const near = { [ATLAS_EDGE_TIER_ATTR]: 1, [ATLAS_EDGE_FAN_ATTR]: FAN_NEAR_MAX - 1 };
    const far = { [ATLAS_EDGE_TIER_ATTR]: 1, [ATLAS_EDGE_FAN_ATTR]: FAN_NEAR_MAX };
    expect(show(near, 5)).toBe(true);
    expect(show(far, 5)).toBe(false);
    expect(show(far, 3)).toBe(true);
    expect(show(far, 1)).toBe(true);
  });

  it("keeps quiet lines for hover only, at every zoom", () => {
    for (const zoom of [1, 3, 5]) expect(show({ [ATLAS_EDGE_TIER_ATTR]: 0 }, zoom)).toBe(false);
  });
});

describe("round 3 B4: a hub's ring of dots never runs through a neighbour", () => {
  const ringGraph = (neighbourAt: number) => {
    const graph = new Graph({ multi: true });
    graph.addNode("hub", { x: 0, y: 0, size: 6, entityType: "concept", label: "hub" });
    graph.addNode("near", { x: neighbourAt, y: 0, size: 4, entityType: "concept", label: "near" });
    graph.addEdge("hub", "near");
    for (let i = 0; i < 30; i += 1) {
      const id = `m${String(i).padStart(2, "0")}`;
      graph.addNode(id, { x: 0, y: 0, size: 2, entityType: MEMORY_NODE_TYPE, label: id });
      graph.addEdge(id, "hub");
    }
    const plan = satellitePlan(graph);
    annotateDust(graph, plan);
    placeSatellites(graph, plan);
    markCrowdedDust(graph, plan);
    return graph;
  };
  const rest = { hovered: null, neighbors: new Set<string>() };
  const palette = {
    project: "#111111", tool: "#222222", org: "#333333", person: "#444444", concept: "#555555",
    neutral: "#666666", edge: "#777777", edgeStrong: "#888888", label: "#999999", labelMuted: "#aaaaaa",
    surface: "#000000", graticule: "rgba(4,5,6,0.13)", bridge: "#bbbbbb", memory: "#cccccc", page: "#dddddd",
  };

  it("folds the dots that would land on a close neighbour, and counts them in the badge", () => {
    const graph = ringGraph(30);
    const room = graph.getNodeAttribute("hub", ATLAS_DUST_ROOM_ATTR) as number;
    expect(room).toBeGreaterThan(0);
    expect(room).toBeLessThan(18);
    const near = { x: 30, y: 0, r: 4 };
    graph.forEachNode((id, attrs) => {
      if (attrs.entityType !== MEMORY_NODE_TYPE) return;
      const shown = nodeDisplay(rest, id, attrs, palette, lodFor(5)).hidden !== true;
      if (shown) expect(Math.hypot((attrs.x as number) - near.x, (attrs.y as number) - near.y)).toBeGreaterThan(near.r + (attrs.size as number));
      expect(attrs[ATLAS_DUST_CROWDED_ATTR] === true).toBe((attrs.dustRank as number) >= room && (attrs.dustRank as number) < 18);
    });
    const badges = planDustBadges(graph, ["hub"], (p) => ({ x: p.x + 400, y: p.y + 300 }), lodFor(5), null, { width: 800, height: 600 });
    expect(badges[0]?.text).toBe(`+${30 - room}`);
    // Hovering the hub still shows its whole ring.
    const crowded = graph.filterNodes((_id, attrs) => attrs[ATLAS_DUST_CROWDED_ATTR] === true)[0] as string;
    expect(nodeDisplay({ hovered: "hub", neighbors: new Set(["hub"]) }, crowded, graph.getNodeAttributes(crowded), palette, lodFor(5)).hidden).not.toBe(true);
  });

  it("leaves a ring with room alone", () => {
    const graph = ringGraph(400);
    expect(graph.getNodeAttribute("hub", ATLAS_DUST_ROOM_ATTR)).toBeUndefined();
    expect(graph.someNode((_id, attrs) => attrs[ATLAS_DUST_CROWDED_ATTR] === true)).toBe(false);
  });
});
