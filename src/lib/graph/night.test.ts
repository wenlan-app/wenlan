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
  nodeSizeCapPx,
  runAtlasLayout,
  satelliteAnchor,
  satellitePlan,
} from "./atlas";
import { attachThingPoints, buildKnowledgeGraphModel, DEFAULT_LAYERS, drawableModel, MEMORY_NODE_TYPE } from "./model";
import {
  applyNightOffsets,
  frameAt,
  growthAt,
  GROWTH_MS,
  layoutNightSentences,
  newNightPoints,
  nightMarks,
  nightOffsetsOf,
  nightPointIds,
  thingFocusState,
  thingStopNodeIds,
  type NightSentence,
} from "./night";
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

function mapWith(points: ThingNightPoints[], links: KnowledgeGraph["memory_links"] = graph().memory_links) {
  const base = drawableModel(buildKnowledgeGraphModel(graph(), { layers: DEFAULT_LAYERS }));
  return buildAtlasGraph(attachThingPoints(base, points, links), PALETTE);
}

/** A second thing, none of whose points links to anything on the map. */
const loose: ThingNightPoints = {
  thing_id: "t2",
  title: "Reading list",
  spaces: [],
  points: [
    { memory_id: "q1", title: "One book a month", decided: true, created_at: 5 },
    { memory_id: "q2", title: "Which shelf", decided: false, created_at: 6 },
  ],
};

const xy = (map: ReturnType<typeof mapWith>, id: string) => ({
  x: map.getNodeAttribute(id, "x") as number,
  y: map.getNodeAttribute(id, "y") as number,
});
const between = (a: { x: number; y: number }, b: { x: number; y: number }) => Math.hypot(a.x - b.x, a.y - b.y);

/** Lay a map out the way AtlasView's mount does: kept night spots first,
 *  then the layout, then the kept body. */
function openMap(map: ReturnType<typeof mapWith>, kept?: { body: Map<string, { x: number; y: number }>; night: Map<string, { x: number; y: number }> }) {
  if (kept) applyNightOffsets(map, kept.night);
  runAtlasLayout(map);
  const sim = createAtlasSimulation(map);
  if (kept) sim.restorePositions(kept.body);
  const body = new Map(map.filterNodes((_id, attrs) => attrs.entityType !== MEMORY_NODE_TYPE).map((id) => [id, xy(map, id)]));
  return { sim, body, night: nightOffsetsOf(map) };
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

  it("settles each thing with its points around its centre", () => {
    const map = mapWith(things());
    runAtlasLayout(map);
    createAtlasSimulation(map);
    const centre = xy(map, "thing:t1");
    expect(Number.isFinite(centre.x) && Number.isFinite(centre.y)).toBe(true);
    for (const id of ["mem:n1", "mem:n2"]) {
      expect(between(xy(map, id), centre)).toBeLessThan(40);
      expect(map.getNodeAttribute(id, "dustOf")).toBe("thing:t1");
    }
  });

  it("puts a thing whose points link to the map beside what they link to, and one with none beside the map", () => {
    const map = mapWith([...things(), loose]);
    const { sim } = openMap(map);
    const hub = xy(map, "hub");
    const linked = xy(map, "thing:t1");
    const unlinked = xy(map, "thing:t2");
    // Close to the hub its point links to: far nearer than the loose thing,
    // which keeps its shelf slot off the side of the map.
    expect(between(linked, hub)).toBeLessThan(between(unlinked, hub));
    expect(map.getNodeAttribute("thing:t2", "island")).toBe(true);
    // ...and clear of every page and entity disc.
    const reach = map.getNodeAttribute("thing:t1", "clusterRadius") as number;
    map.forEachNode((id, attrs) => {
      if (attrs.entityType === MEMORY_NODE_TYPE || attrs.entityType === "thing") return;
      expect(between(xy(map, id), linked)).toBeGreaterThanOrEqual(reach + (attrs.size as number));
    });
    // A frame is pinned: a drag that reheats the map does not move it.
    sim.alpha(0.3);
    sim.tick(30);
    expect(xy(map, "thing:t1")).toEqual(linked);
    expect(xy(map, "thing:t2")).toEqual(unlinked);
  });

  it("puts the same thing in the same spot every time, and keeps a spot once given", () => {
    const first = mapWith([...things(), loose]);
    const kept = openMap(first);
    const again = mapWith([...things(), loose]);
    openMap(again);
    expect(xy(again, "thing:t1")).toEqual(xy(first, "thing:t1"));
    // Reopened with the kept layout, after the entity it links to was
    // dragged away: the thing stays where it was first put.
    const moved = new Map(kept.body);
    moved.set("hub", { x: kept.body.get("hub")!.x + 200, y: kept.body.get("hub")!.y });
    const reopened = mapWith([...things(), loose]);
    openMap(reopened, { ...kept, body: moved });
    expect(xy(reopened, "thing:t1")).toEqual(kept.body.get("thing:t1"));
  });

  it("keeps every point and frame exactly where it was when a new night adds a point", () => {
    const before = mapWith([...things(), loose]);
    const kept = openMap(before);
    // A new night wrote a point that sorts first and links to the hub, and
    // an older point gained a link of its own since.
    const after = mapWith([...things([{ memory_id: "a0", title: "New", decided: false, created_at: 30 }]), loose], [
      ...graph().memory_links,
      { memory_id: "a0", entity_id: "hub" },
      { memory_id: "n1", entity_id: "a" },
    ]);
    const reopened = openMap(after, kept);
    for (const id of before.nodes()) {
      expect(after.hasNode(id)).toBe(true);
      const was = xy(before, id);
      const now = xy(after, id);
      expect(now.x).toBeCloseTo(was.x, 6);
      expect(now.y).toBeCloseTo(was.y, 6);
    }
    // The new point's spot is worked out once and kept from then on.
    expect(reopened.night.has("thing:t1>mem:a0")).toBe(true);
    const third = mapWith([...things([{ memory_id: "a0", title: "New", decided: false, created_at: 30 }]), loose]);
    openMap(third, reopened);
    expect(xy(third, "mem:a0").x).toBeCloseTo(xy(after, "mem:a0").x, 6);
    expect(xy(third, "mem:a0").y).toBeCloseTo(xy(after, "mem:a0").y, 6);
  });

  it("grows a night's points with the zoom like any subject, while plain memories stay specks", () => {
    expect(nodeSizeCapPx({ entityType: "memory", night: true }, false)).toBe(8);
    expect(nodeSizeCapPx({ entityType: "memory" }, false)).toBe(2.8);
    expect(nodeSizeCapPx({ entityType: "concept" }, false)).toBe(8);
    expect(nodeSizeCapPx({ entityType: "memory", night: true }, true)).toBe(12);
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
    expect(frames).toEqual([{ thing: "thing:t1", title: "Site redesign", left: 29, top: 39, right: 71, bottom: 65, dim: false }]);
    expect(moons.map((moon) => [moon.x, moon.y])).toEqual([[40, 50], [60, 54]]);
    // Hovering something else dims it; hovering one of its points does not.
    expect(nightMarks(map, drawnAt, { hovered: "hub", neighbors: new Set(["a"]) }, viewport).frames[0]!.dim).toBe(true);
    expect(nightMarks(map, drawnAt, { hovered: "hub", neighbors: new Set(["mem:n2"]) }, viewport).frames[0]!.dim).toBe(false);
    // Nothing drawn, or all off screen: no frame.
    expect(nightMarks(map, () => null, { hovered: null, neighbors: new Set() }, viewport).frames).toEqual([]);
    expect(nightMarks(map, () => ({ x: 900, y: 50, r: 3 }), { hovered: null, neighbors: new Set() }, viewport).frames).toEqual([]);
  });

  it("hands the shown sentences over only for a thing that shows them, open or settled", () => {
    const map = mapWith(things());
    annotateDust(map, satellitePlan(map));
    const drawnAt = (id: string) => (id === "mem:n1" ? { x: 40, y: 50, r: 3 } : id === "mem:n2" ? { x: 60, y: 90, r: 3 } : null);
    const rest = { hovered: null, neighbors: new Set<string>() };
    const viewport = { width: 400, height: 200 };
    expect(nightMarks(map, drawnAt, rest, viewport).sentences).toEqual([]);
    const shown = nightMarks(map, drawnAt, rest, viewport, (thing) => thing === "thing:t1").sentences;
    expect(shown.map(({ id, text, open }) => [id, text, open])).toEqual([
      ["mem:n1", "Tagline settled", false],
      ["mem:n2", "Pricing page open", true],
    ]);
  });
});

/** 6 px a character: enough to reason about room in the tests below. */
const measure = (text: string) => text.length * 6;
const sentence = (id: string, x: number, y: number, text: string, open = false): NightSentence => ({ id, x, y, r: 4, text, open, dim: false });

describe("a thing's sentences beside its points (UX-CHAT-5)", () => {
  it("writes each sentence to the right of its point, or to the left where the right would cut it short", () => {
    const placed = layoutNightSentences([
      sentence("a", 100, 100, "Things are frames, not nodes"),
      sentence("b", 220, 102, "Night points cluster by id hash"),
      sentence("c", 100, 160, "Moon marks what a night wrote", true),
    ], measure);
    const byId = new Map(placed.map((p) => [p.id, p]));
    // b's disc at 216 would cut a short on the right: the left shows it whole.
    expect(byId.get("a")).toMatchObject({ align: "right", x: 90, y: 100, text: "Things are frames, not nodes" });
    expect(byId.get("b")).toMatchObject({ align: "left", x: 230, text: "Night points cluster by id hash" });
    expect(byId.get("c")).toMatchObject({ align: "left", text: "Moon marks what a night wrote", open: true });
    // Hemmed in on both sides: cut short with an ellipsis where it fits best.
    const [hemmed] = layoutNightSentences([sentence("a", 100, 100, "Things are frames, not nodes")], measure, [
      { left: 0, right: 20, top: 90, bottom: 110 },
      { left: 216, right: 400, top: 90, bottom: 110 },
    ]);
    expect(hemmed!.text.endsWith("…")).toBe(true);
    expect(hemmed!.box.right).toBeLessThanOrEqual(216);
  });

  it("goes to the left when the right is taken, and is left out rather than printed over a word", () => {
    const occupied = [{ left: 205, right: 400, top: 90, bottom: 110 }];
    const left = layoutNightSentences([sentence("a", 200, 100, "Left side")], measure, [...occupied]);
    expect(left).toEqual([expect.objectContaining({ align: "right", x: 190, text: "Left side" })]);
    expect(layoutNightSentences([sentence("a", 200, 100, "Nowhere")], measure, [{ left: 0, right: 400, top: 90, bottom: 110 }])).toEqual([]);
  });

  it("never runs longer than 240 px, nor past the edge of the screen", () => {
    const [only] = layoutNightSentences([sentence("a", 0, 0, "x".repeat(200))], measure);
    expect(measure(only!.text)).toBeLessThanOrEqual(240);
    // A phone: the right edge is near, so the sentence goes left, cut at
    // 240 px; with the left edge near too it is cut at the right edge.
    const [phone] = layoutNightSentences([sentence("a", 250, 0, "Frames never jump when a night adds points")], measure, [], { width: 400 });
    expect(phone).toMatchObject({ align: "right" });
    expect(phone!.box.left).toBeGreaterThanOrEqual(0);
    const [edge] = layoutNightSentences([sentence("a", 60, 0, "Frames never jump when a night adds points")], measure, [], { width: 200 });
    expect(edge).toMatchObject({ align: "left" });
    expect(edge!.box.right).toBeLessThanOrEqual(200);
    expect(edge!.text.endsWith("…")).toBe(true);
  });

  it("picks a thing from a click on its frame or title, the smaller frame where two overlap", () => {
    const hits = [
      { thing: "thing:big", left: 0, top: 0, right: 300, bottom: 300 },
      { thing: "thing:small", left: 100, top: 100, right: 150, bottom: 150 },
      { thing: "thing:small", left: 100, top: 80, right: 200, bottom: 97 },
    ];
    expect(frameAt(hits, 10, 10)).toBe("thing:big");
    expect(frameAt(hits, 120, 120)).toBe("thing:small");
    expect(frameAt(hits, 180, 90)).toBe("thing:small");
    expect(frameAt(hits, 400, 400)).toBeNull();
  });
});

describe("a thing's three zoom stops (UX-CHAT-5)", () => {
  it("frames its points, then what they link to and those bodies' neighbours, then the whole map", () => {
    const map = mapWith(things());
    annotateDust(map, satellitePlan(map));
    expect(thingStopNodeIds(map, "thing:t1", "this")).toEqual(["mem:n1", "mem:n2"]);
    // n2 links to hub; hub's neighbours are a, b and c. d and e are further.
    expect(thingStopNodeIds(map, "thing:t1", "nearby")).toEqual(["a", "b", "c", "hub", "mem:n1", "mem:n2"]);
    expect(thingStopNodeIds(map, "thing:t1", "all")).toEqual(map.nodes());
  });

  it("lights its points and what they link to, draws those lines, and never its tie to its centre", () => {
    const map = mapWith(things());
    annotateDust(map, satellitePlan(map));
    const state = thingFocusState(map, "thing:t1", "this");
    expect([...state.neighbors].sort()).toEqual(["hub", "mem:n1", "mem:n2"]);
    const attrs = { color: "#abcdef", size: 0.5 };
    const point = { entityType: "memory", night: true, dustRank: 1 };
    const hub = { entityType: "concept" };
    const thing = { entityType: "thing" };
    const line = edgeDisplay(state, "e", "mem:n2", "hub", attrs, PALETTE, lodFor(1), { source: point, target: hub });
    expect(line.hidden).toBeUndefined();
    expect(line.color).toBe(PALETTE.edgeStrong);
    expect(edgeDisplay(state, "e", "thing:t1", "mem:n2", attrs, PALETTE, lodFor(1), { source: thing, target: point }).hidden).toBe(true);
    // Hub to a is outside the lit set at 這件事, inside it at 附近.
    expect(edgeDisplay(state, "e", "hub", "a", attrs, PALETTE, lodFor(1), { source: hub, target: hub }).hidden).toBe(true);
    const near = thingFocusState(map, "thing:t1", "nearby");
    expect(edgeDisplay(near, "e", "hub", "a", attrs, PALETTE, lodFor(1), { source: hub, target: hub }).hidden).toBeUndefined();
    // The thing's own node stays hidden, its points lit.
    expect(nodeDisplay(state, "thing:t1", { entityType: "thing", size: 1, label: "Site redesign" }, PALETTE).hidden).toBe(true);
    expect(nodeDisplay(state, "mem:n1", { entityType: "memory", night: true, size: 3, color: "#7a7a7a" }, PALETTE).highlighted).toBe(true);
  });
});

describe("new points grow in once (UX-CHAT-5)", () => {
  it("names the points not seen before, and none on a first look with no record", () => {
    expect(newNightPoints(["mem:a", "mem:b", "mem:c"], new Set(["mem:b"]))).toEqual(["mem:a", "mem:c"]);
    expect(newNightPoints(["mem:a"], null)).toEqual([]);
    expect(newNightPoints(["mem:a"], new Set(["mem:a"]))).toEqual([]);
    const map = mapWith(things());
    expect(nightPointIds(map)).toEqual(["mem:n1", "mem:n2"]);
  });

  it("grows from the centre to the point's spot over 400 ms, easing out", () => {
    expect(GROWTH_MS).toBe(400);
    expect(growthAt(0)).toBe(0);
    expect(growthAt(-5)).toBe(0);
    expect(growthAt(400)).toBe(1);
    expect(growthAt(10_000)).toBe(1);
    const half = growthAt(200);
    expect(half).toBeGreaterThan(0.5);
    expect(half).toBeLessThan(1);
    let previous = 0;
    for (let t = 0; t <= 400; t += 20) {
      expect(growthAt(t)).toBeGreaterThanOrEqual(previous);
      previous = growthAt(t);
    }
  });
});
