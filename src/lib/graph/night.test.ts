// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import type { KnowledgeGraph, ThingNightPoints } from "../tauri";
import {
  annotateDust,
  buildAtlasGraph,
  createAtlasSimulation,
  edgeDisplay,
  lodFor,
  nodeDisplay,
  runAtlasLayout,
  satelliteAnchor,
  satellitePlan,
} from "./atlas";
import { attachThingPoints, buildKnowledgeGraphModel, DEFAULT_LAYERS, drawableModel } from "./model";
import { nightMarks } from "./night";
import type { GraphPalette } from "./palette";

const PALETTE: GraphPalette = {
  project: "#111111",
  tool: "#222222",
  org: "#333333",
  person: "#444444",
  concept: "#555555",
  neutral: "#666666",
  edge: "#777777",
  edgeStrong: "#888888",
  label: "#999999",
  labelMuted: "#aaaaaa",
  surface: "#000000",
  graticule: "rgba(4,5,6,0.13)",
  bridge: "#bbbbbb",
  memory: "#cccccc",
  page: "#dddddd",
};

/** Six linked entities; `hub` is the busiest. */
function graph(): KnowledgeGraph {
  const entity = (id: string) => ({
    id,
    name: id,
    entity_type: "concept",
    domain: null,
    space: null,
    source_agent: null,
    confidence: null,
    confirmed: true,
    created_at: 1,
    updated_at: 1,
    memory_count: 0,
    status: "detected",
    established_by: null,
  });
  const relation = (id: string, from: string, to: string) => ({
    id,
    from_entity: from,
    to_entity: to,
    relation_type: "knows",
    source_agent: null,
    created_at: 1,
  });
  return {
    entities: ["hub", "a", "b", "c", "d", "e"].map(entity),
    relations: [
      relation("r1", "hub", "a"),
      relation("r2", "hub", "b"),
      relation("r3", "hub", "c"),
      relation("r4", "c", "d"),
      relation("r5", "d", "e"),
    ],
    memories: [],
    // n2 later gained a link to the hub.
    memory_links: [{ memory_id: "n2", entity_id: "hub" }],
    pages: [],
    page_links: [],
  } as unknown as KnowledgeGraph;
}

function things(extra: ThingNightPoints["points"] = []): ThingNightPoints[] {
  return [{
    thing_id: "t1",
    title: "Site redesign",
    spaces: [],
    points: [
      { memory_id: "n1", title: "Tagline settled", decided: true, created_at: 10 },
      { memory_id: "n2", title: "Pricing page open", decided: false, created_at: 20 },
      ...extra,
    ],
  }];
}

function mapWith(points: ThingNightPoints[]) {
  const base = drawableModel(buildKnowledgeGraphModel(graph(), { layers: DEFAULT_LAYERS }));
  return buildAtlasGraph(attachThingPoints(base, points, graph().memory_links), PALETTE);
}

describe("a thing and its night's points on the big map (UX-CHAT-5)", () => {
  it("keeps a point in its thing's cluster when it also links to a busier entity", () => {
    const map = mapWith(things());
    expect(map.degree("hub")).toBeGreaterThan(map.degree("thing:t1"));
    expect(satelliteAnchor(map, "mem:n2")).toBe("thing:t1");
    expect(satelliteAnchor(map, "mem:n1")).toBe("thing:t1");
  });

  it("places a new night's point round the older ones, so they stay where they were", () => {
    const before = new Map(satellitePlan(mapWith(things())).map((s) => [s.id, s]));
    // Sorts first by id, but was written last.
    const after = new Map(
      satellitePlan(mapWith(things([{ memory_id: "a0", title: "New", decided: false, created_at: 30 }]))).map((s) => [s.id, s]),
    );
    for (const id of ["mem:n1", "mem:n2"]) {
      expect(after.get(id)!.angle).toBe(before.get(id)!.angle);
      expect(after.get(id)!.radius).toBe(before.get(id)!.radius);
      expect(after.get(id)!.rank).toBe(before.get(id)!.rank);
    }
    expect(after.get("mem:a0")!.rank).toBe(2);
    // Ids that differ only at the end still spread round the centre, kept
    // apart, rather than lining up.
    const plan = [...after.values()];
    const angles = plan.map((s) => s.angle).sort((a, b) => a - b);
    expect(angles[angles.length - 1]! - angles[0]!).toBeGreaterThan(1);
    for (const a of plan) for (const b of plan) {
      if (a === b) continue;
      const d = Math.hypot(a.radius * Math.cos(a.angle) - b.radius * Math.cos(b.angle), a.radius * Math.sin(a.angle) - b.radius * Math.sin(b.angle));
      expect(d).toBeGreaterThanOrEqual(9 - 1e-9);
    }
  });

  it("does not move the pages and entities a fresh layout settles on", () => {
    const plain = mapWith([]);
    const withThing = mapWith(things());
    runAtlasLayout(plain);
    runAtlasLayout(withThing);
    for (const id of ["hub", "a", "b", "c", "d", "e"]) {
      expect(withThing.getNodeAttribute(id, "x")).toBe(plain.getNodeAttribute(id, "x"));
      expect(withThing.getNodeAttribute(id, "y")).toBe(plain.getNodeAttribute(id, "y"));
    }
  });

  it("settles the thing as its own island with its points around it", () => {
    const map = mapWith(things());
    runAtlasLayout(map);
    createAtlasSimulation(map);
    const tx = map.getNodeAttribute("thing:t1", "x") as number;
    const ty = map.getNodeAttribute("thing:t1", "y") as number;
    expect(Number.isFinite(tx) && Number.isFinite(ty)).toBe(true);
    expect(map.getNodeAttribute("thing:t1", "island")).toBe(true);
    for (const id of ["mem:n1", "mem:n2"]) {
      const d = Math.hypot((map.getNodeAttribute(id, "x") as number) - tx, (map.getNodeAttribute(id, "y") as number) - ty);
      expect(d).toBeLessThan(40);
      expect(map.getNodeAttribute(id, "dustOf")).toBe("thing:t1");
    }
  });

  it("never draws the thing as a node, and draws its points whole at the opening view", () => {
    const rest = { hovered: null, neighbors: new Set<string>() };
    const thing = { label: "Site redesign", color: "#7a7a7a", size: 1, entityType: "thing", island: true };
    expect(nodeDisplay(rest, "thing:t1", thing, PALETTE, lodFor(1)).hidden).toBe(true);
    // Neither dimmed as an island nor folded into a count.
    const point = { label: "Tagline", color: "#7a7a7a", size: 3, entityType: "memory", island: true, night: true, dustRank: 40, dustOf: "thing:t1" };
    const shown = nodeDisplay(rest, "mem:n1", point, PALETTE, lodFor(1));
    expect(shown.hidden).toBeUndefined();
    expect(shown.color).toBe("#7a7a7a");
    const dust = { ...point, night: undefined };
    expect(nodeDisplay(rest, "mem:m1", dust, PALETTE, lodFor(1)).hidden).toBe(true);
  });

  it("gives a thing's cluster room on the map, so islands keep off its frame", () => {
    const map = mapWith(things());
    runAtlasLayout(map);
    createAtlasSimulation(map);
    expect(map.getNodeAttribute("thing:t1", "clusterRadius")).toBeGreaterThan(3);
  });

  it("shows a point's line to an entity at rest, while its tie to the thing centre stays quiet", () => {
    const rest = { hovered: null, neighbors: new Set<string>() };
    const attrs = { color: "#abcdef", size: 1 };
    const point = { entityType: "memory", night: true, dustRank: 40, island: true };
    const entity = { entityType: "concept", landmark: true };
    const thing = { entityType: "thing", island: true };
    expect(edgeDisplay(rest, "e", "mem:n2", "hub", attrs, PALETTE, lodFor(1), { source: point, target: entity }).hidden).toBeUndefined();
    expect(edgeDisplay(rest, "e", "thing:t1", "mem:n2", attrs, PALETTE, lodFor(1), { source: thing, target: point }).hidden).toBe(true);
  });

  it("frames each thing's drawn points with its title above, and puts a moon by each point", () => {
    const map = mapWith(things());
    annotateDust(map, satellitePlan(map));
    const spots: Record<string, { x: number; y: number; r: number }> = {
      "mem:n1": { x: 40, y: 50, r: 3 },
      "mem:n2": { x: 60, y: 54, r: 3 },
    };
    const drawnAt = (id: string) => spots[id] ?? null;
    const viewport = { width: 200, height: 200 };
    const { frames, moons } = nightMarks(map, drawnAt, { hovered: null, neighbors: new Set() }, viewport);
    expect(frames).toEqual([{ title: "Site redesign", left: 29, top: 39, right: 71, bottom: 65, dim: false }]);
    expect(moons.map((moon) => [moon.x, moon.y])).toEqual([[40, 50], [60, 54]]);
    // Hovering something else dims it; hovering one of its points does not.
    expect(nightMarks(map, drawnAt, { hovered: "hub", neighbors: new Set(["a"]) }, viewport).frames[0]!.dim).toBe(true);
    expect(nightMarks(map, drawnAt, { hovered: "hub", neighbors: new Set(["mem:n2"]) }, viewport).frames[0]!.dim).toBe(false);
    // Nothing drawn, or all off screen: no frame.
    expect(nightMarks(map, () => null, { hovered: null, neighbors: new Set() }, viewport).frames).toEqual([]);
    expect(nightMarks(map, () => ({ x: 900, y: 50, r: 3 }), { hovered: null, neighbors: new Set() }, viewport).frames).toEqual([]);
  });
});
