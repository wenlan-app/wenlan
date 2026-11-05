// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import type { KnowledgeGraph, ThingNightPoints } from "../tauri";
import {
  annotateDust,
  buildAtlasGraph,
  createAtlasSimulation,
  edgeDisplay,
  NEIGHBOR_LABEL_MAX,
  THING_LINK_LINE_SIZE,
  lodFor,
  nodeDisplay,
  OPENING_LOD,
  nodeSizeCapPx,
  runAtlasLayout,
  satelliteAnchor,
  satellitePlan,
  setThingFlight,
} from "./atlas";
import { attachThingPoints, buildKnowledgeGraphModel, DEFAULT_LAYERS, drawableModel, MEMORY_NODE_TYPE, thingTitle } from "./model";
import {
  applyNightOffsets,
  cutToFit,
  encloseSentences,
  frameAt,
  growthAt,
  haloOutline,
  haloPad,
  GROWTH_MS,
  layoutNightSentences,
  newNightPoints,
  nightMarks,
  nightOffsetsOf,
  nightPointIds,
  labelYieldsToFrames,
  placeFrameTitles,
  SENTENCE_LINE,
  STEEPING_SECS,
  SENTENCE_MAX_PX,
  thingFocusState,
  thingPointIds,
  thingStopNodeIds,
  thingStopsFor,
  uncrowdedSentences,
  wrapText,
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

  it("draws a point's line to an entity only on hover or pick, never at rest, and never its tie to the thing centre", () => {
    const rest = { hovered: null, neighbors: new Set<string>() };
    const attrs = { color: "#abcdef", size: 1 };
    const point = { entityType: "memory", night: true, dustRank: 40, island: true };
    const entity = { entityType: "concept", landmark: true };
    const thing = { entityType: "thing", island: true };
    const ends = { source: point, target: entity };
    // At the overview the line would cross the map from the thing's cluster.
    expect(edgeDisplay(rest, "e", "mem:n2", "hub", attrs, PALETTE, lodFor(1), ends).hidden).toBe(true);
    expect(edgeDisplay(rest, "e", "mem:n2", "hub", attrs, PALETTE, lodFor(8), ends).hidden).toBe(true);
    const hovered = { hovered: "mem:n2", neighbors: new Set(["hub"]) };
    expect(edgeDisplay(hovered, "e", "mem:n2", "hub", attrs, PALETTE, lodFor(1), ends).hidden).toBeUndefined();
    const picked = { hovered: "thing:t1", neighbors: new Set(["mem:n2", "hub"]), thing: "thing:t1" };
    expect(edgeDisplay(picked, "e", "mem:n2", "hub", attrs, PALETTE, lodFor(1), ends).hidden).toBeUndefined();
    expect(edgeDisplay(rest, "e", "thing:t1", "mem:n2", attrs, PALETTE, lodFor(1), { source: thing, target: point }).hidden).toBe(true);
  });

  it("flies a picked thing's linked point out beside its topic and home again, and frames only the points left (Qi-Xuan 11-05)", () => {
    const map = mapWith(things());
    runAtlasLayout(map);
    const sim = createAtlasSimulation(map);
    const home = { n1: xy(map, "mem:n1"), n2: xy(map, "mem:n2") };
    const fromHub = (id: string) => Math.hypot(xy(map, id).x - xy(map, "hub").x, xy(map, id).y - xy(map, "hub").y);
    const before = fromHub("mem:n2");
    // Only the point that links to a topic flies.
    expect(setThingFlight(map, "thing:t1", 0.5)).toEqual(["mem:n2"]);
    sim.placeSatellitesNow();
    const half = fromHub("mem:n2");
    expect(half).toBeLessThan(before);
    setThingFlight(map, "thing:t1", 1);
    sim.placeSatellitesNow();
    expect(fromHub("mem:n2")).toBeLessThan(half);
    // Right beside the hub, on the side facing its thing.
    const hubR = map.getNodeAttribute("hub", "size") as number;
    const dot = map.getNodeAttribute("mem:n2", "size") as number;
    expect(fromHub("mem:n2")).toBeGreaterThan(hubR + dot);
    expect(fromHub("mem:n2")).toBeLessThan(hubR + 3 * dot + 5);
    const towardThing = (xy(map, "thing:t1").x - xy(map, "hub").x) * (xy(map, "mem:n2").x - xy(map, "hub").x)
      + (xy(map, "thing:t1").y - xy(map, "hub").y) * (xy(map, "mem:n2").y - xy(map, "hub").y);
    expect(towardThing).toBeGreaterThan(0);
    expect(xy(map, "mem:n1")).toEqual(home.n1);
    // The frame is round the point left with the thing; the one out by its
    // topic keeps its sentence there, outside the frame.
    annotateDust(map, satellitePlan(map));
    const drawnAt = (id: string) => (id === "mem:n1" ? { x: 100, y: 100, r: 3 } : id === "mem:n2" ? { x: 300, y: 300, r: 3 } : null);
    const marks = nightMarks(map, drawnAt, { hovered: null, neighbors: new Set() }, { width: 400, height: 400 }, () => true, "thing:t1");
    expect(marks.frames[0]!.right).toBeLessThan(200);
    expect(marks.sentences.find((sentence) => sentence.id === "mem:n2")).toMatchObject({ away: true });
    // With every point away, a small frame stays on the thing's centre.
    map.mergeNodeAttributes("mem:n1", { fly: 1, flyTo: { anchor: "hub", dx: 1, dy: 0 } });
    const centred = nightMarks(map, drawnAt, { hovered: null, neighbors: new Set() }, { width: 400, height: 400 }, undefined, "thing:t1", undefined, false, () => ({ x: 50, y: 60 }));
    expect((centred.frames[0]!.left + centred.frames[0]!.right) / 2).toBe(50);
    expect((centred.frames[0]!.top + centred.frames[0]!.bottom) / 2).toBe(60);
    // Let go: home exactly where it was.
    expect(setThingFlight(map, null, 0)).toEqual([]);
    expect(map.getNodeAttribute("mem:n1", "fly")).toBeUndefined();
    sim.placeSatellitesNow();
    expect(map.getNodeAttribute("mem:n2", "fly")).toBeUndefined();
    expect(xy(map, "mem:n2").x).toBeCloseTo(home.n2.x, 9);
    expect(xy(map, "mem:n2").y).toBeCloseTo(home.n2.y, 9);
  });

  it("keeps a frame visibly a frame when its points are drawn tiny", () => {
    const map = mapWith(things());
    annotateDust(map, satellitePlan(map));
    const drawnAt = (id: string) => (id === "mem:n1" ? { x: 100, y: 100, r: 1 } : null);
    const [frame] = nightMarks(map, drawnAt, { hovered: null, neighbors: new Set() }, { width: 400, height: 400 }, undefined, "thing:t1").frames;
    expect(frame!.right - frame!.left).toBeGreaterThanOrEqual(30);
    expect(frame!.bottom - frame!.top).toBeGreaterThanOrEqual(22);
    // Grown evenly: still centred on the point.
    expect((frame!.left + frame!.right) / 2).toBe(100);
    expect((frame!.top + frame!.bottom) / 2).toBe(100);
  });

  it("frames only the picked thing's drawn points with its title, and gives the thing one moon while a night steeped it in the last day", () => {
    const map = mapWith(things());
    annotateDust(map, satellitePlan(map));
    const spots: Record<string, { x: number; y: number; r: number }> = {
      "mem:n1": { x: 40, y: 50, r: 3 },
      "mem:n2": { x: 60, y: 54, r: 3 },
    };
    const drawnAt = (id: string) => spots[id] ?? null;
    const viewport = { width: 200, height: 200 };
    const { frames } = nightMarks(map, drawnAt, { hovered: null, neighbors: new Set() }, viewport, undefined, "thing:t1", 20 + STEEPING_SECS + 1);
    expect(frames).toEqual([{ thing: "thing:t1", title: "Site redesign", left: 29, top: 39, right: 71, bottom: 65, dim: false, steeping: false, spots: [spots["mem:n1"], spots["mem:n2"]] }]);
    // The halo grows round its points, the way a region does, not a box.
    expect(haloOutline(frames[0]!)).toEqual([{ x: 40, y: 50 }, { x: 60, y: 54 }]);
    // Its sentences sit beside it: a wide frame grown by them leaves the halo round its points.
    expect(haloOutline({ ...frames[0]!, left: -300, right: 400 })).toEqual([{ x: 40, y: 50 }, { x: 60, y: 54 }]);
    // The halo reaches past its largest dot.
    expect(haloPad(frames[0]!)).toBe(13);
    // Its newest point (written at 20) is under a day old: one moon, by the title.
    expect(nightMarks(map, drawnAt, { hovered: null, neighbors: new Set() }, viewport, undefined, "thing:t1", 20 + STEEPING_SECS - 1).frames[0]!.steeping).toBe(true);
    // Hovering something else dims it; hovering one of its points does not.
    expect(nightMarks(map, drawnAt, { hovered: "hub", neighbors: new Set(["a"]) }, viewport, undefined, "thing:t1").frames[0]!.dim).toBe(true);
    expect(nightMarks(map, drawnAt, { hovered: "hub", neighbors: new Set(["mem:n2"]) }, viewport, undefined, "thing:t1").frames[0]!.dim).toBe(false);
    // Nothing picked, or another thing picked: the map stays clean.
    expect(nightMarks(map, drawnAt, { hovered: null, neighbors: new Set() }, viewport).frames).toEqual([]);
    expect(nightMarks(map, drawnAt, { hovered: null, neighbors: new Set() }, viewport, undefined, "thing:t2").frames).toEqual([]);
    // The "Working on" switch draws every active thing, picked or not.
    expect(nightMarks(map, drawnAt, { hovered: null, neighbors: new Set() }, viewport, undefined, null, undefined, true).frames.map((f) => f.thing)).toEqual(["thing:t1"]);
    // A quiet thing's points with no topic are framed under it when it is
    // picked; "Working on" is for active things.
    const quietMap = mapWith([{ ...loose, quiet: true }]);
    annotateDust(quietMap, satellitePlan(quietMap));
    const quietAt = (id: string) => (id.startsWith("mem:q") ? { x: 40, y: 50, r: 3 } : null);
    expect(nightMarks(quietMap, quietAt, { hovered: null, neighbors: new Set() }, viewport).frames).toEqual([]);
    expect(nightMarks(quietMap, quietAt, { hovered: null, neighbors: new Set() }, viewport, undefined, "thing:t2").frames.map((f) => f.thing)).toEqual(["thing:t2"]);
    expect(nightMarks(quietMap, quietAt, { hovered: null, neighbors: new Set() }, viewport, undefined, null, undefined, true).frames).toEqual([]);
    // Nothing drawn, or all off screen: no frame.
    expect(nightMarks(map, () => null, { hovered: null, neighbors: new Set() }, viewport, undefined, "thing:t1").frames).toEqual([]);
    expect(nightMarks(map, () => ({ x: 900, y: 50, r: 3 }), { hovered: null, neighbors: new Set() }, viewport, undefined, "thing:t1").frames).toEqual([]);
  });

  it("lights a picked quiet thing's points where they sit, beside their topics, and draws no frame for them", () => {
    const quiet = things()
      .map((thing) => ({ ...thing, quiet: true, points: thing.points.filter((point) => point.memory_id === "n2") }))
      .filter((thing) => thing.points.length > 0);
    const map = mapWith(quiet);
    const plan = satellitePlan(map);
    annotateDust(map, plan);
    // It rides its topic, not the thing, so picking it moves nothing.
    const anchor = plan.find((s) => s.id === "mem:n2")!.anchor;
    expect(anchor).not.toBe("thing:t1");
    expect(thingPointIds(map, "thing:t1")).toEqual(["mem:n2"]);
    expect(thingStopNodeIds(map, "thing:t1", "this")).toEqual([anchor, "mem:n2"].sort());
    expect(thingStopsFor(map, "thing:t1")).toContain("nearby");
    const lit = thingFocusState(map, "thing:t1", "this");
    expect(lit.neighbors).toContain("mem:n2");
    expect(lit.neighbors).toContain(anchor);
    // Folded away as dust at this zoom, but not while its thing is picked.
    const attrs = { ...map.getNodeAttributes("mem:n2"), dustRank: 99, dustOf: anchor };
    const lod = { ...OPENING_LOD, dustVisible: 0 };
    expect(nodeDisplay({ hovered: null, neighbors: new Set() }, "mem:n2", attrs, PALETTE, lod).hidden).toBe(true);
    expect(nodeDisplay(lit, "mem:n2", attrs, PALETTE, lod).hidden).not.toBe(true);
    const at = () => ({ x: 40, y: 50, r: 3 });
    expect(nightMarks(map, at, lit, { width: 400, height: 300 }, undefined, "thing:t1").frames).toEqual([]);
  });

  it("ends a thing's name cut short when it was stored at the length cap, at a word, with an ellipsis", () => {
    expect(thingTitle("Site redesign")).toBe("Site redesign");
    // Stored names are cut at 40 characters, often mid-word.
    expect(thingTitle("Wenlan PR review and merge queue managem")).toBe("Wenlan PR review and merge queue…");
    expect(thingTitle("Wenlan PR review, merge queue, and fixes")).toBe("Wenlan PR review, merge queue, and…");
    expect(thingTitle("整理文澜大图上每一件事的位置和夜里沉淀下来的那些点让它们不再挤在一起而且看得清楚")).toMatch(/…$/u);
  });

  it("hands the shown sentences over only for a thing that shows them, open or settled", () => {
    const map = mapWith(things());
    annotateDust(map, satellitePlan(map));
    const drawnAt = (id: string) => (id === "mem:n1" ? { x: 40, y: 50, r: 3 } : id === "mem:n2" ? { x: 60, y: 90, r: 3 } : null);
    const rest = { hovered: null, neighbors: new Set<string>() };
    const viewport = { width: 400, height: 200 };
    expect(nightMarks(map, drawnAt, rest, viewport, undefined, "thing:t1").sentences).toEqual([]);
    const shown = nightMarks(map, drawnAt, rest, viewport, (thing) => thing === "thing:t1", "thing:t1").sentences;
    expect(shown.map(({ id, text, open }) => [id, text, open])).toEqual([
      ["mem:n1", "Tagline settled", false],
      ["mem:n2", "Pricing page open", true],
    ]);
    // Hovered or pinned: only that one point's sentence.
    const one = nightMarks(map, drawnAt, rest, viewport, (_thing, point) => point === "mem:n2", "thing:t1").sentences;
    expect(one.map(({ id }) => id)).toEqual(["mem:n2"]);
  });
});

/** 6 px a character: enough to reason about room in the tests below. */
const measure = (text: string) => text.length * 6;
const sentence = (id: string, x: number, y: number, text: string, open = false, thing = "thing:t1"): NightSentence => ({ id, thing, x, y, r: 4, text, open, dim: false });
/** CJK characters are about twice as wide as Latin ones. */
const measureWide = (text: string) => [...text].reduce((sum, char) => sum + (/[\u2e80-\u9fff\uff00-\uffef]/u.test(char) ? 12 : 6), 0);
const LONG = "Pricing page stays open until the owner confirms the annual discount with the finance team";
const LONG_ZH = "週末重寫了定價頁的文案，年繳方案的折扣還要等財務確認後才能定下來，先不放到首頁上";

describe("a thing's sentences beside its points (UX-CHAT-5)", () => {
  it("writes each sentence to the right of its point, or to the left where the right would cut it short", () => {
    const placed = layoutNightSentences([
      sentence("a", 100, 100, "Things are frames, not nodes"),
      sentence("b", 220, 102, "Night points cluster by id hash"),
      sentence("c", 100, 160, "Moon marks what a night wrote", true),
    ], measure);
    const byId = new Map(placed.map((p) => [p.id, p]));
    // b's disc at 216 would cut a short on the right: the left shows it whole.
    expect(byId.get("a")).toMatchObject({ align: "right", x: 90, y: 100, lines: ["Things are frames, not nodes"] });
    expect(byId.get("b")).toMatchObject({ align: "left", x: 230, lines: ["Night points cluster by id hash"] });
    expect(byId.get("c")).toMatchObject({ align: "left", lines: ["Moon marks what a night wrote"], open: true, thing: "thing:t1" });
  });

  it("shows a long sentence whole, wrapped at 320 px onto at most three lines", () => {
    const [latin] = layoutNightSentences([sentence("a", 100, 100, LONG)], measure);
    expect(latin!.lines.length).toBe(2);
    expect(latin!.lines.join(" ")).toBe(LONG);
    for (const line of latin!.lines) expect(measure(line)).toBeLessThanOrEqual(SENTENCE_MAX_PX);
    expect(latin!.box.bottom - latin!.box.top).toBe(2 * SENTENCE_LINE);
    // Chinese wraps between characters, never with a space added.
    const [zh] = layoutNightSentences([sentence("b", 100, 100, LONG_ZH)], measureWide);
    expect(zh!.lines.length).toBe(2);
    expect(zh!.lines.join("")).toBe(LONG_ZH);
    for (const line of zh!.lines) expect(measureWide(line)).toBeLessThanOrEqual(SENTENCE_MAX_PX);
  });

  it("narrows the block where the full width would cover something, and cuts only past three lines", () => {
    // Room on the right up to 240 only, the left taken.
    const [hemmed] = layoutNightSentences([sentence("a", 100, 100, LONG)], measure, [
      { left: 0, right: 88, top: 90, bottom: 150 },
      { left: 240, right: 400, top: 90, bottom: 150 },
    ]);
    expect(hemmed).toMatchObject({ align: "left" });
    expect(hemmed!.lines.length).toBe(3);
    expect(hemmed!.lines[2]!.endsWith("…")).toBe(true);
    expect(hemmed!.box.right).toBeLessThanOrEqual(240);
  });

  it("goes to the left when the right is taken, and is left out rather than printed over a word", () => {
    const occupied = [{ left: 205, right: 400, top: 90, bottom: 110 }];
    const left = layoutNightSentences([sentence("a", 200, 100, "Left side")], measure, [...occupied]);
    expect(left).toEqual([expect.objectContaining({ align: "right", x: 190, lines: ["Left side"] })]);
    expect(layoutNightSentences([sentence("a", 200, 100, "Nowhere")], measure, [{ left: 0, right: 400, top: 90, bottom: 110 }])).toEqual([]);
    // Another body on the map (an entity's disc) is kept clear of too, and
    // is only read, never taken as placed text.
    const bodies = [{ left: 230, right: 240, top: 95, bottom: 105 }];
    const occupiedNow: { left: number; right: number; top: number; bottom: number }[] = [];
    const [clear] = layoutNightSentences([sentence("a", 200, 100, "Left side")], measure, occupiedNow, undefined, bodies);
    expect(clear).toMatchObject({ align: "right", x: 190 });
    expect(occupiedNow).toEqual([clear!.box]);
  });

  it("never runs wider than 320 px, nor past the edge of the screen", () => {
    const [only] = layoutNightSentences([sentence("a", 0, 0, "x".repeat(200))], measure);
    expect(only!.lines.length).toBe(3);
    for (const line of only!.lines) expect(measure(line)).toBeLessThanOrEqual(SENTENCE_MAX_PX);
    expect(only!.lines[2]!.endsWith("…")).toBe(true);
    // A phone: the right edge is near, so the sentence wraps narrower there,
    // still whole.
    const text = "Frames never jump when a night adds points";
    const [phone] = layoutNightSentences([sentence("a", 250, 0, text)], measure, [], { width: 400 });
    expect(phone).toMatchObject({ align: "left" });
    expect(phone!.lines.join(" ")).toBe(text);
    expect(phone!.box.right).toBeLessThanOrEqual(400);
    // Both edges near: cut on the third line, inside the screen.
    const [edge] = layoutNightSentences([sentence("a", 60, 0, LONG)], measure, [], { width: 200 });
    expect(edge).toMatchObject({ align: "left" });
    expect(edge!.box.right).toBeLessThanOrEqual(200);
    expect(edge!.lines[edge!.lines.length - 1]!.endsWith("…")).toBe(true);
  });

  it("wraps between words, or between Chinese characters, ending the third line with an ellipsis", () => {
    const lines = wrapText(measure, LONG, 120);
    expect(lines.length).toBe(3);
    for (const line of lines) expect(measure(line)).toBeLessThanOrEqual(120);
    expect(lines[2]!.endsWith("…")).toBe(true);
    // Every line, the cut one too, ends on a whole word.
    const words = new Set(LONG.split(" "));
    for (const line of lines) expect(words.has(line.replace(/…$/u, "").split(" ").pop()!)).toBe(true);
    expect(LONG.startsWith(`${lines[0]} ${lines[1]}`)).toBe(true);
    const zh = wrapText(measureWide, LONG_ZH, 60);
    expect(zh.slice(0, 2)).toEqual([LONG_ZH.slice(0, 5), LONG_ZH.slice(5, 10)]);
    expect(zh[2]).toBe(`${LONG_ZH.slice(10, 14)}…`);
    // A word wider than a line breaks where it must.
    expect(wrapText(measure, "a".repeat(50), 60)).toEqual(["a".repeat(10), "a".repeat(10), `${"a".repeat(9)}…`]);
    expect(wrapText(measure, "Short one", 120)).toEqual(["Short one"]);
    expect(wrapText(measure, "   ", 120)).toEqual([]);
  });

  it("cuts a name at a word's end, or at any Chinese character, with an ellipsis", () => {
    expect(cutToFit(measure, "Short", 100)).toBe("Short");
    expect(cutToFit(measure, "Pricing page stays open", 80)).toBe("Pricing page…");
    // "Pricing pa…" would cut the word: back to "Pricing".
    expect(cutToFit(measure, "Pricing page stays open", 70)).toBe("Pricing…");
    expect(cutToFit(measure, "Tagline, pricing", 60)).toBe("Tagline…");
    expect(cutToFit(measureWide, "週末重寫了定價頁的文案", 60)).toBe("週末重寫…");
  });

  it("grows a frame round its own points' sentences, so its border never runs through text", () => {
    const placed = layoutNightSentences([sentence("a", 100, 100, LONG), sentence("b", 400, 300, "Other thing", false, "thing:t2")], measure);
    const frames = [
      { thing: "thing:t1", title: "Site", left: 85, top: 85, right: 115, bottom: 115, dim: false },
      { thing: "thing:t2", title: "Other", left: 385, top: 285, right: 415, bottom: 315, dim: false },
    ];
    const [grown, other] = encloseSentences(frames, placed);
    const box = placed.find((p) => p.id === "a")!.box;
    expect(grown!.left).toBeLessThanOrEqual(box.left - 8);
    expect(grown!.right).toBeGreaterThanOrEqual(box.right + 8);
    expect(grown!.top).toBeLessThanOrEqual(box.top - 8);
    expect(grown!.bottom).toBeGreaterThanOrEqual(box.bottom + 8);
    // Its points stay inside; the other thing grows only round its own.
    expect(grown!.left).toBe(85);
    expect(other!.right).toBeGreaterThan(415);
    expect(other!.left).toBe(385);
  });

  it("drops a thing's sentences where they would run its halo into another thing's, but never the picked thing's", () => {
    const sentence = (id: string, thing: string, left: number, right: number) => ({
      id, thing, lines: [id], x: left, y: 100, align: "left" as const, open: false, dim: false,
      box: { left, right, top: 95, bottom: 105 },
    });
    const frames = [
      { thing: "thing:a", title: "A", left: 0, top: 90, right: 20, bottom: 110, dim: false },
      { thing: "thing:b", title: "B", left: 200, top: 90, right: 220, bottom: 110, dim: false },
    ];
    // a's sentence runs right up to b's halo: a shows points only.
    const crowded = [sentence("s1", "thing:a", 30, 210), sentence("s2", "thing:b", 230, 300)];
    expect(uncrowdedSentences(frames, crowded).map((s) => s.id)).toEqual(["s2"]);
    expect(uncrowdedSentences(frames, crowded, "thing:a").map((s) => s.id)).toEqual(["s1"]);
    // Room for both: both keep their sentences.
    const roomy = [sentence("s1", "thing:a", 30, 120), sentence("s2", "thing:b", 230, 300)];
    expect(uncrowdedSentences(frames, roomy).map((s) => s.id)).toEqual(["s1", "s2"]);
  });

  it("keeps room after a steeping thing's title for its one moon", () => {
    const base = { thing: "thing:t1", title: "Site", left: 0, top: 100, right: 200, bottom: 140, dim: false };
    const [plain] = placeFrameTitles([base], measure);
    const [steeping] = placeFrameTitles([{ ...base, steeping: true }], measure);
    expect(steeping!.text).toBe("Site");
    expect(steeping!.box.right - steeping!.box.left).toBeGreaterThan(plain!.box.right - plain!.box.left);
  });

  it("puts each frame's title clear of other titles, names and frames' borders, cut at a word", () => {
    const frame = (thing: string, title: string, left: number, top: number, right: number, bottom: number) => ({ thing, title, left, top, right, bottom, dim: false });
    const row = [0, 70, 140, 210].map((left, index) => frame(`thing:${index}`, "Site redesign", left, 100, left + 60, 140));
    const titles = placeFrameTitles(row, measure);
    expect(titles.length).toBe(4);
    for (const [index, a] of titles.entries()) {
      for (const b of titles.slice(index + 1)) {
        expect(a.box.left < b.box.right && b.box.left < a.box.right && a.box.top < b.box.bottom && b.box.top < a.box.bottom).toBe(false);
      }
    }
    // A frame just under another: its title goes below rather than over the
    // upper frame's border.
    const stacked = placeFrameTitles([frame("thing:a", "Upper", 0, 100, 200, 160), frame("thing:b", "Lower", 10, 170, 200, 240)], measure);
    expect(stacked.find((t) => t.thing === "thing:b")!.baseline).toBeGreaterThan(240);
    // A frame inside another still gets its title.
    const nested = placeFrameTitles([frame("thing:a", "Outer", 0, 0, 400, 400), frame("thing:b", "Inner", 100, 100, 200, 200)], measure);
    expect(nested.map((t) => t.thing).sort()).toEqual(["thing:a", "thing:b"]);
    // Cut at a word.
    const [long] = placeFrameTitles([frame("thing:c", "Rewrite the pricing page before the launch", 0, 100, 100, 140)], measure);
    expect(long!.text).toBe("Rewrite the pricing page before the…");
    // No room anywhere: left out.
    expect(placeFrameTitles(row, measure, [{ left: -1000, right: 1000, top: -1000, bottom: 1000 }])).toEqual([]);
  });

  it("always shows the picked thing's title, and tries the frame's right edge before giving up", () => {
    const frame = (thing: string, title: string, left: number, top: number, right: number, bottom: number) => ({ thing, title, left, top, right, bottom, dim: false });
    const wide = frame("thing:w", "Merge queue", 0, 100, 300, 160);
    // A name over the frame's left corners: the title moves to the right edge.
    const leftNames = [{ left: -10, right: 60, top: 70, bottom: 100 }, { left: -10, right: 60, top: 160, bottom: 190 }];
    const [right] = placeFrameTitles([wide], measure, [...leftNames]);
    expect(right!.box.right).toBeLessThanOrEqual(301 + 3);
    expect(right!.box.left).toBeGreaterThan(60);
    // Picked, with no room anywhere: placed anyway, and first.
    const full = [{ left: -1000, right: 1000, top: -1000, bottom: 1000 }];
    const picked = placeFrameTitles([frame("thing:a", "Other", 0, 300, 100, 340), wide], measure, [...full], "thing:w");
    expect(picked.map((t) => t.thing)).toEqual(["thing:w"]);
    expect(picked[0]!.baseline).toBe(96);
  });

  it("moves a picked thing's title further out rather than cover a linked topic's name", () => {
    const wide = { thing: "thing:w", title: "Merge queue", left: 0, top: 100, right: 300, bottom: 160, dim: false };
    // A name across the whole line above the frame and the whole line below.
    const names = [
      { left: -10, right: 310, top: 78, bottom: 99 },
      { left: -10, right: 310, top: 161, bottom: 182 },
    ];
    const [title] = placeFrameTitles([wide], measure, [...names], "thing:w");
    expect(title).toBeDefined();
    for (const name of names) {
      const covers = title!.box.left < name.right && name.left < title!.box.right && title!.box.top < name.bottom && name.top < title!.box.bottom;
      expect(covers).toBe(false);
    }
    // Only the picked thing moves away; another thing's title is left out.
    expect(placeFrameTitles([wide], measure, [...names])).toEqual([]);
  });

  it("keeps a linked topic's name even where the picked frame already sits", () => {
    expect(labelYieldsToFrames(false, true)).toBe(false);
    expect(labelYieldsToFrames(false, false)).toBe(true);
    expect(labelYieldsToFrames(true, false)).toBe(false);
  });

  it("cuts a crowded frame's title shorter rather than leaving it out", () => {
    const small = { thing: "thing:s", title: "Wenlan PR review and merge queue", left: 100, top: 100, right: 130, bottom: 122, dim: false };
    // Names to the left and right of the frame leave about 100 px above and below it.
    const names = [
      { left: 0, right: 98, top: 60, bottom: 160 },
      { left: 200, right: 400, top: 60, bottom: 160 },
    ];
    const [title] = placeFrameTitles([small], measure, [...names]);
    expect(title!.text).toBe("Wenlan PR…");
    expect(title!.box.right).toBeLessThan(200);
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
  it("offers 附近 only when the thing's points link to something", () => {
    const linked = mapWith(things());
    annotateDust(linked, satellitePlan(linked));
    expect(thingStopsFor(linked, "thing:t1")).toEqual(["this", "nearby", "all"]);
    const alone = mapWith(things(), []);
    annotateDust(alone, satellitePlan(alone));
    expect(thingStopsFor(alone, "thing:t1")).toEqual(["this", "all"]);
  });

  it("frames its points and what they link to, then those bodies' neighbours, then the whole map", () => {
    const map = mapWith(things());
    annotateDust(map, satellitePlan(map));
    expect(thingStopNodeIds(map, "thing:t1", "this")).toEqual(["hub", "mem:n1", "mem:n2"]);
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
    expect(line.color).toBe(PALETTE.bridge);
    expect(line.size).toBe(THING_LINK_LINE_SIZE);
    // The same line whichever way round the edge runs, and still at 附近.
    expect(edgeDisplay(state, "e", "hub", "mem:n2", attrs, PALETTE, lodFor(1), { source: hub, target: point }).size).toBe(THING_LINK_LINE_SIZE);
    expect(edgeDisplay(thingFocusState(map, "thing:t1", "nearby"), "e", "mem:n2", "hub", attrs, PALETTE, lodFor(1), { source: point, target: hub }).size).toBe(THING_LINK_LINE_SIZE);
    // Nothing picked: the line stays hidden, the map at rest is unchanged.
    expect(edgeDisplay({ hovered: null, neighbors: new Set() }, "e", "mem:n2", "hub", attrs, PALETTE, lodFor(1), { source: point, target: hub }).hidden).toBe(true);
    // Lines among topics are not point links, even at 附近.
    expect(edgeDisplay(thingFocusState(map, "thing:t1", "nearby"), "e", "hub", "a", attrs, PALETTE, lodFor(1), { source: hub, target: hub }).size).toBe(0.5);
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

describe("a picked thing names what its points link to (UX-CHAT-5)", () => {
  it("names a linked topic however many points are lit, and not its neighbours at 附近", () => {
    const state = {
      hovered: "thing:t1",
      thing: "thing:t1",
      neighbors: new Set(["hub", "near", ...Array.from({ length: NEIGHBOR_LABEL_MAX + 1 }, (_, i) => `mem:${i}`)]),
      linked: new Set(["hub"]),
    };
    const topic = { entityType: "concept", size: 3, label: "Topic" };
    expect(nodeDisplay(state, "hub", topic, PALETTE).forceLabel).toBe(true);
    expect(nodeDisplay(state, "near", topic, PALETTE).forceLabel).toBeUndefined();
    expect(nodeDisplay(state, "mem:0", { entityType: "memory", night: true, size: 3 }, PALETTE).forceLabel).toBeUndefined();
    // Nothing picked: no name is forced.
    expect(nodeDisplay({ hovered: null, neighbors: new Set() }, "hub", topic, PALETTE).forceLabel).toBeUndefined();
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
