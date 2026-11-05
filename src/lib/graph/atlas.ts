// SPDX-License-Identifier: AGPL-3.0-only
import Graph from "graphology";
import forceAtlas2 from "graphology-layout-forceatlas2";
import {
  forceSimulation,
  forceLink,
  forceManyBody,
  forceCollide,
  type Simulation,
  type SimulationNodeDatum,
} from "d3-force";
import type { GraphModel } from "./model";
import {
  MEMORY_NODE_TYPE,
  PAGE_NODE_TYPE,
  SHARED_SOURCE_EDGE_TYPE,
  WIKILINK_EDGE_TYPE,
  CITES_EDGE_TYPE,
  MEMORY_EDGE_TYPE,
  THING_NODE_TYPE,
} from "./model";
import { compositeOver, nodeFillFor, type GraphPalette } from "./palette";

const MIN_NODE_SIZE = 3;
const MAX_NODE_SIZE = 14;
/** Wiki pages sit on the same scale as entities. Round 3 pulled the base from
 *  3.5 down to the unconfirmed-entity 3: with shared-source overlap no longer
 *  counting toward size (see buildAtlasGraph), a page reads as a subject
 *  without the whole page layer drawing a size step above the entities. */
const PAGE_NODE_BASE = 3;
/** Memory nodes start below the smallest entity and stay there: they are
 *  context around the subjects, and a memory linked to six entities must not
 *  outgrow the entities themselves. */
const MEMORY_NODE_BASE = 1.5;
const MEMORY_MAX_SIZE = 3;
/** A thing is never drawn as a node: AtlasView frames its points instead
 *  (night.ts). Its node is the hidden centre its points cluster round, kept
 *  tiny so the cluster starts right at it. */
const THING_NODE_SIZE = 1;
/** Graph-space distance from a thing's centre to its nearest points. */
const THING_CLUSTER_CORE = 3;
/** Px added per doubling of degree. Linear growth (round 1's degree * 0.5)
 *  saturated the cap on real data — 1,600 entities where the hubs run to
 *  hundreds of links drew as one uniform field of 8s. log2 keeps the hubs
 *  visibly bigger while leaving the long tail distinguishable; the gain and
 *  cap went up together (1.6/12 -> 1.9/14) so a hub reads as a landmark on
 *  the relief map, not just a slightly larger dot. */
const DEGREE_GAIN = 1.9;

// Base by stability/kind (confirmed 4, unconfirmed 3, page 3, memory 1.5)
// plus DEGREE_GAIN per doubling of degree, capped. Size still encodes
// confirmation at degree 0, as it did before.
function nodeSizeFor(confirmed: boolean | null, degree: number, entityType?: string): number {
  const growth = DEGREE_GAIN * Math.log2(1 + degree);
  if (entityType === MEMORY_NODE_TYPE) {
    return Math.min(MEMORY_MAX_SIZE, MEMORY_NODE_BASE + growth);
  }
  if (entityType === THING_NODE_TYPE) return THING_NODE_SIZE;
  const base =
    entityType === PAGE_NODE_TYPE ? PAGE_NODE_BASE : confirmed === true ? 4 : MIN_NODE_SIZE;
  return Math.min(MAX_NODE_SIZE, base + growth);
}

/** Edge stroke by verb, in CSS px. Edges are the quietest ink on the map — a
 *  hairline at rest, so the points carry the picture; a
 *  shared-source edge (an inferred overlap, not an asserted link) is thinner
 *  still. Hover emphasis recolours, it does not thicken. */
export function edgeSizeFor(type: string): number {
  return type === SHARED_SOURCE_EDGE_TYPE ? 0.6 : 1;
}

/**
 * GraphModel -> a graphology instance sigma can render directly. Positions
 * seed on a deterministic circle (node array order), so the result is
 * reproducible without running a layout; runAtlasLayout refines it from
 * there. `multi: true` because GraphModel's parallel-edge policy keeps
 * distinct relations between the same pair as distinct edges (see model.ts)
 * — a simple graph would throw adding the second one.
 */
export function buildAtlasGraph(model: GraphModel, palette: GraphPalette): Graph {
  const graph = new Graph({ multi: true });
  // ponytail: the old graph's confirmed-glow halo (r+3 disc at 0.1 alpha) is
  // skipped — it needs a custom WebGL node program in sigma; the tiered fills
  // and size base carry the confirmed/unconfirmed distinction instead.

  // Shared-source edges stand for an inferred overlap, not an asserted link,
  // so they must not inflate a page's disc: size is computed from the degree
  // MINUS however many shared-source edges touch the node. Done here rather
  // than as a second field on GraphNode — the model type stays untouched, and
  // the one place that draws discs is the one place that has to know.
  const sharedSourceIncident = new Map<string, number>();
  for (const edge of model.edges) {
    if (edge.type !== SHARED_SOURCE_EDGE_TYPE) continue;
    sharedSourceIncident.set(edge.source, (sharedSourceIncident.get(edge.source) ?? 0) + 1);
    if (edge.target !== edge.source) {
      sharedSourceIncident.set(edge.target, (sharedSourceIncident.get(edge.target) ?? 0) + 1);
    }
  }

  // Seed circle over the nodes the layout will move — memories are not among
  // them (see nonSimulatedIds: a memory rides its anchor), so they are left
  // off the circle and out of its count. That is what keeps the seed, and so
  // the whole layout, identical with the memory layer on and off. Things
  // stay off it too (FA2 skips them, see runAtlasLayout): a thing's first
  // night must not reshuffle a fresh map of pages and entities. They start
  // on the same circle at an angle of their own and are shelved as islands,
  // which puts each cluster in free space beside the map.
  const seeded = model.nodes.filter((node) => node.entityType !== MEMORY_NODE_TYPE && node.entityType !== THING_NODE_TYPE);
  const n = seeded.length;
  const seedIndex = new Map(seeded.map((node, i) => [node.id, i]));
  model.nodes.forEach((node) => {
    const i = seedIndex.get(node.id);
    const thing = node.entityType === THING_NODE_TYPE;
    const angle = thing ? stableAngle(node.id) : i === undefined ? 0 : (2 * Math.PI * i) / Math.max(n, 1);
    const sizingDegree = Math.max(0, node.degree - (sharedSourceIncident.get(node.id) ?? 0));
    graph.addNode(node.id, {
      label: node.name,
      size: nodeSizeFor(node.confirmed, sizingDegree, node.entityType),
      color: nodeFillFor(node.entityType, node.confirmed, palette),
      entityType: node.entityType,
      // Kept on the node so the theme-flip recolor (AtlasView) can recompute
      // the stability-tiered fill without re-reading the model.
      confirmed: node.confirmed,
      // A night's point keeps its moon and its age: satellitePlan puts a
      // thing's points in the order they were written.
      ...(node.night ? { night: true, createdAt: node.createdAt, open: node.open === true } : {}),
      ...(node.quiet ? { quiet: true } : {}),
      ...(node.elsewhere ? { elsewhere: node.elsewhere } : {}),
      x: i === undefined && !thing ? 0 : Math.cos(angle),
      y: i === undefined && !thing ? 0 : Math.sin(angle),
    });
  });

  // ponytail: parallel edges between the same pair draw fully overlapped —
  // a view decision (see model.ts's parallel-edge note), fine at round-1 scale.
  for (const edge of model.edges) {
    graph.addEdgeWithKey(edge.id, edge.source, edge.target, {
      // Kept on the edge so the hover reducer can read the verb without
      // re-reading the model.
      edgeType: edge.type,
      // AtlasView's edge program keeps this width in CSS pixels at every zoom.
      // Needs minEdgeThickness lowered in AtlasView — sigma's default floor
      // (1.7) silently bumps a hairline back up.
      size: edgeSizeFor(edge.type),
      color: palette.edge,
    });
  }
  return graph;
}

/**
 * Precompute the semantic hierarchy used by the overview zoom.
 *
 * Components and degree are intentionally built from non-memory edges only:
 * memories are evidence attached to a subject, not structural weight. Every
 * component contributes its highest-degree representative as a landmark;
 * larger maps then add up to twenty-four of the strongest structural hubs and
 * keep the twelve strongest landmarks labelled.
 * Small maps stay readable as a whole, while still capping labels.
 *
 * This is a union/find pass over nodes and edges followed by bounded ranking
 * (O((V+E) log V)), so callers can safely run it after each graph rebuild.
 * The helper mutates node attributes and returns no derived graph state; the
 * reducers can read the tags without traversing the graph per frame.
 */
export function applyAtlasHierarchy(graph: Graph): void {
  const parent = new Map<string, string>();
  const structuralDegree = new Map<string, number>();
  const structuralNeighbors = new Map<string, Set<string>>();

  graph.forEachNode((id, attrs) => {
    if (attrs.entityType === MEMORY_NODE_TYPE) return;
    parent.set(id, id);
    structuralDegree.set(id, 0);
    structuralNeighbors.set(id, new Set());
  });

  const find = (start: string): string => {
    let root = start;
    while (parent.get(root) !== root) root = parent.get(root) as string;
    let current = start;
    while (parent.get(current) !== root) {
      const next = parent.get(current) as string;
      parent.set(current, root);
      current = next;
    }
    return root;
  };

  const union = (a: string, b: string): void => {
    const rootA = find(a);
    const rootB = find(b);
    if (rootA !== rootB) parent.set(rootA, rootB);
  };

  graph.forEachEdge((_key, _attrs, source, target) => {
    if (!parent.has(source) || !parent.has(target)) return;
    if (source === target) return;
    structuralNeighbors.get(source)?.add(target);
    structuralNeighbors.get(target)?.add(source);
    union(source, target);
  });

  for (const [id, neighbors] of structuralNeighbors) {
    structuralDegree.set(id, neighbors.size);
  }

  const components = new Map<string, string[]>();
  for (const id of parent.keys()) {
    const root = find(id);
    const ids = components.get(root);
    if (ids) ids.push(id);
    else components.set(root, [id]);
  }
  const orderedComponents = [...components.values()]
    .map((ids) => ids.sort())
    .sort((a, b) => {
      const aFirst = a[0] as string;
      const bFirst = b[0] as string;
      return aFirst < bFirst ? -1 : aFirst > bFirst ? 1 : 0;
    });

  const representatives: string[] = [];
  for (const [componentIndex, ids] of orderedComponents.entries()) {
    let representative = ids[0] as string;
    for (const id of ids.slice(1)) {
      const degreeDelta = (structuralDegree.get(id) ?? 0) - (structuralDegree.get(representative) ?? 0);
      if (degreeDelta > 0 || (degreeDelta === 0 && id < representative)) representative = id;
    }
    representatives.push(representative);
    for (const id of ids) {
      graph.mergeNodeAttributes(id, {
        componentId: `component-${componentIndex}`,
        structuralDegree: structuralDegree.get(id) ?? 0,
        landmark: false,
        landmarkLabel: false,
      });
    }
  }

  const smallGraph = parent.size <= 60;
  const landmarkIds = new Set<string>(smallGraph ? [...parent.keys()] : representatives.filter((id) => (structuralDegree.get(id) ?? 0) >= 2));
  if (!smallGraph) {
    const additionalHubs = [...parent.keys()]
      .filter((id) => !landmarkIds.has(id) && (structuralDegree.get(id) ?? 0) >= 3)
      .sort((a, b) => {
        const degreeDelta = (structuralDegree.get(b) ?? 0) - (structuralDegree.get(a) ?? 0);
        return degreeDelta || (a < b ? -1 : a > b ? 1 : 0);
      })
      .slice(0, 24);
    for (const id of additionalHubs) landmarkIds.add(id);
  }
  const labelledLandmarks = [...landmarkIds]
    .sort((a, b) => {
      const degreeDelta = (structuralDegree.get(b) ?? 0) - (structuralDegree.get(a) ?? 0);
      return degreeDelta || (a < b ? -1 : a > b ? 1 : 0);
    })
    .slice(0, 12);
  const landmarkLabelIds = new Set(labelledLandmarks);
  for (const id of parent.keys()) {
    graph.mergeNodeAttributes(id, {
      landmark: landmarkIds.has(id),
      landmarkLabel: landmarkLabelIds.has(id),
    });
  }
}

/** Iteration budget for FA2: 600 on anything up to 200 nodes (unchanged from
 *  round 2 at demo scale), decaying to the 60 floor by ~2,000 nodes. The whole
 *  layout is synchronous on the main thread, so the budget has to shrink as
 *  the per-iteration cost grows or the memory layer freezes the UI. */
function layoutIterations(order: number): number {
  return Math.min(600, Math.max(60, Math.floor(120_000 / Math.max(order, 1))));
}

/** Same shape for the d3 settle: the full 220 pre-paint ticks up to ~720
 *  nodes, then down to the 60 floor. */
function settleTicks(order: number): number {
  return Math.min(220, Math.max(60, Math.floor(160_000 / Math.max(order, 1))));
}

/**
 * Force-directed refinement of the seeded circle, synchronous.
 *
 * Non-simulated nodes (leaf memories, isolates — see nonSimulatedIds) are laid
 * out on a scratch copy that leaves them out entirely, then the positions are
 * copied back. On real data that is ~1,300 of ~3,300 nodes and their edges
 * removed from the O(n log n) repulsion, and they get their real positions
 * from placeSatellites afterwards anyway.
 *
 * ponytail: still sync FA2. If this ever blocks past ~3s again, the next step
 * is graphology-layout-forceatlas2's worker entry plus a "laying out" state.
 */
export function runAtlasLayout(graph: Graph): void {
  const excluded = new Set(nonSimulatedIds(graph));
  // A thing has no structural link (only its points), so FA2 has nothing to
  // say about it beyond pushing the map around; it is shelved as an island.
  graph.forEachNode((id, attrs) => {
    if (attrs.entityType === THING_NODE_TYPE) excluded.add(id);
  });
  if (excluded.size === 0) {
    forceAtlas2.assign(graph, {
      iterations: layoutIterations(graph.order),
      settings: forceAtlas2.inferSettings(graph),
    });
    return;
  }

  const core = new Graph({ multi: true });
  graph.forEachNode((id, attrs) => {
    if (excluded.has(id)) return;
    core.addNode(id, { x: attrs.x, y: attrs.y, size: attrs.size });
  });
  graph.forEachEdge((key, _attrs, source, target) => {
    if (excluded.has(source) || excluded.has(target)) return;
    core.addEdgeWithKey(key, source, target, {});
  });
  if (core.order === 0) return;
  forceAtlas2.assign(core, {
    iterations: layoutIterations(core.order),
    settings: forceAtlas2.inferSettings(core),
  });
  core.forEachNode((id, attrs) => {
    graph.setNodeAttribute(id, "x", attrs.x);
    graph.setNodeAttribute(id, "y", attrs.y);
  });
}

/** Graph-space distance from the anchor disc's EDGE to a first-ring leaf
 *  memory's CENTRE (the leaf's own radius, 1.5-3 px, comes out of it, so the
 *  edge-to-edge clearance is 7-8.5). Went 6 -> 10 with the memory discs
 *  shrinking: at 6 the first ring of discs touched the page disc and read
 *  as a grey ring round a teal one, a donut, on the real map with memories
 *  on. */
const SATELLITE_GAP = 10;

/**
 * Nodes no force layout runs on: degree-0 isolates (round 1's ring) plus EVERY
 * memory, whatever its degree. A memory is evidence about a subject, not a
 * place of its own: it rides the subject it says most about (see
 * satelliteAnchor) instead of pulling subjects together with its own springs.
 * That is what keeps the map identical with the memory layer on and off — on
 * real data the layer adds 2,000 memories and 4,910 edges, and simulating
 * them re-laid the whole map into a pile every time the chip was pressed.
 */
export function nonSimulatedIds(graph: Graph): string[] {
  const ids: string[] = [];
  graph.forEachNode((id, attrs) => {
    if (graph.degree(id) === 0 || attrs.entityType === MEMORY_NODE_TYPE) ids.push(id);
  });
  return ids;
}

/** The node a memory rides: its thing's centre when a night wrote it (UX-CHAT-5),
 *  else its most-connected non-memory neighbour, ties to the smaller id so
 *  the answer never depends on edge order. A memory linked to three entities
 *  sits by the hub among them, where the eye looks for it; its other links
 *  are drawn on hover only (see edgeDisplay). A night's point stays by its
 *  thing when it later gains an entity link, so the link never moves it. */
export function satelliteAnchor(graph: Graph, id: string): string | undefined {
  let best: string | undefined;
  let bestDegree = -1;
  let thing: string | undefined;
  for (const neighbour of graph.neighbors(id)) {
    const type = graph.getNodeAttribute(neighbour, "entityType");
    if (type === THING_NODE_TYPE) {
      if (thing === undefined || neighbour < thing) thing = neighbour;
      continue;
    }
    if (type === MEMORY_NODE_TYPE) continue;
    const degree = graph.degree(neighbour);
    if (degree > bestDegree || (degree === bestDegree && best !== undefined && neighbour < best)) {
      best = neighbour;
      bestDegree = degree;
    }
  }
  return thing ?? best;
}

/** Where one memory sits relative to the node it rides. */
export interface Satellite {
  id: string;
  anchor: string;
  angle: number;
  radius: number;
  /** 0-based place along the anchor's spiral, assigned in stable ID order.
   *  The zoom tiers (see dustVisibleCount) show the first N. */
  rank: number;
}

/** Stable, spaced memory points on a golden-angle spiral, independent of
 * graph iteration order. */
export function satellitePlan(graph: Graph): Satellite[] {
  const leavesByAnchor = new Map<string, string[]>();
  for (const id of nonSimulatedIds(graph)) {
    if (graph.degree(id) === 0) continue;
    const anchor = satelliteAnchor(graph, id);
    if (anchor === undefined) continue;
    const list = leavesByAnchor.get(anchor);
    if (list) list.push(id);
    else leavesByAnchor.set(anchor, [id]);
  }
  const plan: Satellite[] = [];
  for (const [anchor, leaves] of leavesByAnchor) {
    // A thing's points are placed in the order its nights wrote them, so a
    // new night's points settle round the older ones and never move them.
    // Everything else: stable id order.
    const byAge = graph.getNodeAttribute(anchor, "entityType") === THING_NODE_TYPE;
    const at = (id: string) => (graph.getNodeAttribute(id, "createdAt") as number | undefined) ?? 0;
    const sorted = byAge
      ? [...leaves].sort((a, b) => at(a) - at(b) || (a < b ? -1 : a > b ? 1 : 0))
      : [...leaves].sort();
    // Uniform clearance based on the widest leaf keeps every pair separated.
    let leafSize = 0;
    for (const id of sorted) {
      leafSize = Math.max(leafSize, (graph.getNodeAttribute(id, "size") as number) ?? 0);
    }
    const clearance = 2 * leafSize + 3;
    if (byAge) {
      plan.push(...thingCluster(graph, anchor, sorted, clearance));
      continue;
    }
    const startRadius = (graph.getNodeAttribute(anchor, "size") as number) + SATELLITE_GAP;
    let seed = 0;
    for (const char of anchor) seed = (Math.imul(seed, 31) + char.charCodeAt(0)) >>> 0;
    const rotation = (seed / 0xffffffff) * 2 * Math.PI;
    const accepted: { x: number; y: number }[] = [];
    let candidate = 0;
    for (let rank = 0; rank < sorted.length; rank++) {
      let radius: number, angle: number, x: number, y: number;
      do {
        angle = rotation + candidate * Math.PI * (3 - Math.sqrt(5));
        radius = Math.sqrt(startRadius * startRadius + clearance * clearance * candidate);
        x = radius * Math.cos(angle); y = radius * Math.sin(angle);
        candidate++;
      } while (accepted.some((point) => Math.hypot(point.x - x, point.y - y) < clearance));
      accepted.push({ x, y });
      plan.push({ id: sorted[rank]!, anchor, angle, radius, rank });
    }
  }
  return plan;
}

/**
 * A thing's points as one tight cluster round its centre (UX-CHAT-5): each
 * point's direction and first distance come from a hash of its id, and it
 * steps outward only until it clears the points already placed. `ordered` is
 * oldest first, so a new night's point fits round the older ones and none of
 * them moves; the same points always land in the same places.
 *
 * A point whose spot was worked out on an earlier open carries it as
 * `nightOffset` (AtlasView keeps them, see positions.ts): it goes exactly
 * there, whatever has changed since (a new link that grew its disc, a point
 * that left), and only points with no kept spot are placed, round those.
 */
function thingCluster(graph: Graph, anchor: string, ordered: readonly string[], clearance: number): Satellite[] {
  const placed: { x: number; y: number }[] = [];
  const plan: Satellite[] = new Array(ordered.length);
  const fresh: [string, number][] = [];
  ordered.forEach((id, rank) => {
    const kept = graph.getNodeAttribute(id, "nightOffset") as unknown;
    if (Array.isArray(kept) && kept.length === 2 && kept.every((value) => typeof value === "number" && Number.isFinite(value))) {
      const [x, y] = kept as [number, number];
      placed.push({ x, y });
      plan[rank] = { id, anchor, angle: Math.atan2(y, x), radius: Math.hypot(x, y), rank };
    } else fresh.push([id, rank]);
  });
  for (const [id, rank] of fresh) {
    const angle = mixedUnit(id) * 2 * Math.PI;
    let radius = THING_CLUSTER_CORE + mixedUnit(`${id}#r`) * clearance;
    let x = radius * Math.cos(angle);
    let y = radius * Math.sin(angle);
    while (placed.some((point) => Math.hypot(point.x - x, point.y - y) < clearance)) {
      radius += clearance / 3;
      x = radius * Math.cos(angle);
      y = radius * Math.sin(angle);
    }
    placed.push({ x, y });
    plan[rank] = { id, anchor, angle, radius, rank };
  }
  return plan;
}

/** A well-mixed hash of `id` in [0, 1). FNV-1a alone barely moves when only
 *  the last character differs (point ids often do), so its result is run
 *  through murmur3's finaliser to spread nearby ids round the whole circle. */
function mixedUnit(id: string): number {
  let hash = 2166136261;
  for (let index = 0; index < id.length; index += 1) {
    hash ^= id.charCodeAt(index);
    hash = Math.imul(hash, 16777619);
  }
  hash ^= hash >>> 16;
  hash = Math.imul(hash, 0x85ebca6b);
  hash ^= hash >>> 13;
  hash = Math.imul(hash, 0xc2b2ae35);
  hash ^= hash >>> 16;
  return (hash >>> 0) / 4294967296;
}

/** Write a satellite plan onto the graph. Cheap enough (two trig calls per
 *  leaf) to re-run on every tick writeback, which is what makes a dragged
 *  entity carry its memories along.
 *
 *  `skipId` is the node the pointer is currently holding. A satellite is not
 *  a sim node, so a drag moves it by writing the graph directly; without this
 *  exemption the next writeback would put it straight back on its orbit and
 *  the leaf would look unmovable while the sim is warm. */
export function placeSatellites(graph: Graph, plan: Satellite[], skipId?: string | null): void {
  for (const satellite of plan) {
    if (satellite.id === skipId) continue;
    const ax = graph.getNodeAttribute(satellite.anchor, "x") as number;
    const ay = graph.getNodeAttribute(satellite.anchor, "y") as number;
    graph.setNodeAttribute(satellite.id, "x", ax + satellite.radius * Math.cos(satellite.angle));
    graph.setNodeAttribute(satellite.id, "y", ay + satellite.radius * Math.sin(satellite.angle));
  }
}

/** Write the halo bookkeeping the reducers read onto the graph: each memory's
 *  `dustRank` and `dustOf` (its anchor), and each anchor's `dustCount`. The
 *  plan is the authority; this just makes it reachable from a node's attrs
 *  at paint time without a lookup. */
export function annotateDust(graph: Graph, plan: Satellite[]): void {
  const counts = new Map<string, number>();
  const reach = new Map<string, number>();
  for (const satellite of plan) {
    graph.mergeNodeAttributes(satellite.id, { dustRank: satellite.rank, dustOf: satellite.anchor });
    counts.set(satellite.anchor, (counts.get(satellite.anchor) ?? 0) + 1);
    if (graph.getNodeAttribute(satellite.anchor, "entityType") === THING_NODE_TYPE) {
      const size = (graph.getNodeAttribute(satellite.id, "size") as number | undefined) ?? 0;
      reach.set(satellite.anchor, Math.max(reach.get(satellite.anchor) ?? 0, satellite.radius + size));
    }
  }
  // A thing's cluster takes room on the map like a disc of that radius: the
  // shelf and the collide force keep other ink off its frame.
  for (const [anchor, radius] of reach) graph.setNodeAttribute(anchor, "clusterRadius", radius);
  for (const [anchor, count] of counts) graph.setNodeAttribute(anchor, "dustCount", count);
}

/**
 * How many of an anchor's memories are drawn at a given zoom, as a multiple
 * of the zoom the map opened at (`zoomIn` = mount ratio / current ratio, so 1
 * at the opening view and 2 when the viewer has zoomed in twice). At the
 * opening view an anchor shows a pinch of dust — six dots — and a count of
 * the rest (AtlasView's overlay); closer in, twelve representative points;
 * closer still, eighteen. Focusing an anchor reveals up to 36; its inspector
 * retains every memory, including those represented by the count.
 */
export function dustVisibleCount(zoomIn: number): number {
  if (zoomIn < 2) return 6;
  if (zoomIn < 4) return 12;
  return 18;
}

/** How many of an anchor's memories a hover reveals at most. Everything
 *  would be the honest answer, but the busiest anchor on real data carries
 *  hundreds and at the opening zoom they pack into a solid disc. Bounded
 *  representative points preserve the subject; the inspector exposes the
 *  complete group. */
export const HOVER_DUST_MAX = 36;

/** How many neighbors a hover names outright (`forceLabel`). Beyond this the
 *  label grid decides, so a hub with dozens of memories stays readable. */
export const NEIGHBOR_LABEL_MAX = 12;

/** Zoomed in this much past the opening view, the islands (every component
 *  but the core) are drawn solid with their names and edges; before that they
 *  sit dim at the rim so the core is the one thing the eye lands on. */
export const ISLANDS_SOLID_ZOOM = 2;

/** The semantic zoom phase. The opening map presents a few landmarks first;
 * zooming in reveals the neighbourhood, then the full detail layer. */
export type AtlasZoomPhase = "overview" | "neighborhood" | "detail";

/** What the reducers need to know about the camera, refreshed by AtlasView
 *  before every render. */
export interface LodState {
  dustVisible: number;
  islandsSolid: boolean;
  phase: AtlasZoomPhase;
}

/** The opening view's state: a pinch of dust, islands dim. */
export const OPENING_LOD: LodState = {
  dustVisible: dustVisibleCount(1),
  islandsSolid: false,
  phase: "overview",
};

export function lodFor(zoomIn: number): LodState {
  const phase: AtlasZoomPhase = zoomIn < 2 ? "overview" : zoomIn < 4 ? "neighborhood" : "detail";
  return { dustVisible: dustVisibleCount(zoomIn), islandsSolid: zoomIn >= ISLANDS_SOLID_ZOOM, phase };
}

/** Clearance between the core's occupied discs and the nearest island, and between any
 *  two islands, in graph units — a moat, so an island never reads as a
 *  peninsula of the core or of its neighbour. */
export const ISLAND_GAP = 34;
/** Keep pathological geometry from turning placement into an unbounded loop. */
const MAX_ISLAND_PROBES = 768;
/** The protection mask is adaptive, but never grows beyond this many cells on
 * one side. Sparse occupied cells keep an elongated core from becoming a
 * filled rectangle just because its bounds are large. */
const MAX_PROTECTION_GRID_SIDE = 192;
const MIN_PROTECTION_CELL_SIZE = 8;
const MAX_PROTECTION_QUERY_CELLS = 16_384;
const EDGE_INK_RADIUS = 2;
const EDGE_SAMPLE_STEP = 16;
const MAX_EDGE_INDEX_SAMPLES = 512;

/** One component's drawn extent, in graph units. */
interface ComponentBox {
  ids: string[];
  minX: number;
  maxX: number;
  minY: number;
  maxY: number;
}

const boxWidth = (box: ComponentBox): number => box.maxX - box.minX;
const boxHeight = (box: ComponentBox): number => box.maxY - box.minY;

/** Connected components over the DRAWN graph, in graph node order, with one
 *  rule for memories: a memory belongs to its ANCHOR's component and joins
 *  nothing else. Its other edges are not adjacency here — a memory that
 *  mentions two entities from two components is evidence about both, not a
 *  bridge between them, and treating it as one would merge two islands the
 *  moment the memory layer is turned on. `anchorOf` is the satellite plan's
 *  memory -> anchor map. */
function graphComponents(graph: Graph, anchorOf: Map<string, string>): string[][] {
  const parent = new Map<string, string>();
  graph.forEachNode((id) => parent.set(id, id));
  const find = (start: string): string => {
    let root = start;
    while (parent.get(root) !== root) root = parent.get(root) as string;
    let walk = start;
    while (parent.get(walk) !== root) {
      const next = parent.get(walk) as string;
      parent.set(walk, root);
      walk = next;
    }
    return root;
  };
  graph.forEachEdge((_key, _attrs, source, target) => {
    if (anchorOf.has(source) || anchorOf.has(target)) return;
    const a = find(source);
    const b = find(target);
    if (a !== b) parent.set(a, b);
  });
  for (const [memory, anchor] of anchorOf) {
    const a = find(memory);
    const b = find(anchor);
    if (a !== b) parent.set(a, b);
  }
  const groups = new Map<string, string[]>();
  graph.forEachNode((id) => {
    const root = find(id);
    const list = groups.get(root);
    if (list) list.push(id);
    else groups.set(root, [id]);
  });
  return [...groups.values()].map((ids) => ids.sort());
}

/** Bounding box of a component's ink: every node's disc. Satellites are
 *  skipped — a memory's dust is drawn at most a few dots deep at the opening
 *  view and never counts toward the room a component takes, which is what
 *  keeps the memory layer from reshaping the map. */
function measureComponent(graph: Graph, ids: string[], satellites: Set<string>): ComponentBox {
  let minX = Infinity;
  let maxX = -Infinity;
  let minY = Infinity;
  let maxY = -Infinity;
  for (const id of ids) {
    if (satellites.has(id)) continue;
    const x = graph.getNodeAttribute(id, "x") as number;
    const y = graph.getNodeAttribute(id, "y") as number;
    const pad = bodyRadius(graph.getNodeAttributes(id));
    minX = Math.min(minX, x - pad);
    maxX = Math.max(maxX, x + pad);
    minY = Math.min(minY, y - pad);
    maxY = Math.max(maxY, y + pad);
  }
  if (minX === Infinity) {
    // A component of nothing but satellites cannot happen (a satellite's
    // anchor is in its own component), but a zero box beats a NaN one.
    return { ids, minX: 0, maxX: 0, minY: 0, maxY: 0 };
  }
  return { ids, minX, maxX, minY, maxY };
}

/** The room a node's ink takes: its disc, or a thing's whole cluster. */
function bodyRadius(attrs: Record<string, unknown>): number {
  return Math.max((attrs.size as number | undefined) ?? 0, (attrs.clusterRadius as number | undefined) ?? 0);
}

/** A placed island: centre and the radius of the circle it claims. */
interface Island {
  x: number;
  y: number;
  r: number;
}

/** A compact spatial index for occupied component discs. Keeping this index
 * by centre cell makes the 2,000+ node case cheap without changing the exact
 * circle-to-circle clearance check. */
interface CircleIndex {
  cellSize: number;
  maxRadius: number;
  cells: Map<string, Island[]>;
  /** Circles too large to index safely by the normal candidate grid. */
  oversized: Island[];
}

const circleCell = (value: number, cellSize: number): number => Math.floor(value / cellSize);
const circleCellKey = (x: number, y: number): string => `${x},${y}`;
const MAX_INDEXED_RADIUS = 384;
const MAX_QUERY_CELLS = 16_384;

function newCircleIndex(cellSize = 48): CircleIndex {
  return { cellSize, maxRadius: 0, cells: new Map(), oversized: [] };
}

function addCircle(index: CircleIndex, circle: Island): void {
  if (circle.r > MAX_INDEXED_RADIUS) {
    index.oversized.push(circle);
    return;
  }
  const cx = circleCell(circle.x, index.cellSize);
  const cy = circleCell(circle.y, index.cellSize);
  const key = circleCellKey(cx, cy);
  const bucket = index.cells.get(key);
  if (bucket) bucket.push(circle);
  else index.cells.set(key, [circle]);
  index.maxRadius = Math.max(index.maxRadius, circle.r);
}

/** True when a candidate circle has the requested moat from every indexed
 * circle. The candidate radius is included in the query range, so no nearby
 * obstacle can be missed at a cell boundary. */
function clearOfCircles(index: CircleIndex, x: number, y: number, radius: number, gap = ISLAND_GAP): boolean {
  for (const circle of index.oversized) {
    if (Math.hypot(circle.x - x, circle.y - y) < circle.r + radius + gap) return false;
  }
  const range = radius + index.maxRadius + gap;
  const minX = circleCell(x - range, index.cellSize);
  const maxX = circleCell(x + range, index.cellSize);
  const minY = circleCell(y - range, index.cellSize);
  const maxY = circleCell(y + range, index.cellSize);
  // A very large candidate (or a very large indexed neighbourhood) must not
  // turn into an enormous nested cell walk. Directly checking the normal
  // circles is linear in occupied ink and remains bounded in memory.
  if ((maxX - minX + 1) * (maxY - minY + 1) > MAX_QUERY_CELLS) {
    for (const bucket of index.cells.values()) {
      for (const circle of bucket) {
        if (Math.hypot(circle.x - x, circle.y - y) < circle.r + radius + gap) return false;
      }
    }
    return true;
  }
  for (let cx = minX; cx <= maxX; cx += 1) {
    for (let cy = minY; cy <= maxY; cy += 1) {
      const bucket = index.cells.get(circleCellKey(cx, cy));
      if (!bucket) continue;
      for (const circle of bucket) {
        if (Math.hypot(circle.x - x, circle.y - y) < circle.r + radius + gap) return false;
      }
    }
  }
  return true;
}

interface CoreSegment {
  sourceX: number;
  sourceY: number;
  targetX: number;
  targetY: number;
}

/** A bounded, sparse mask of the core's meaningful ink and enclosed voids.
 * `occupied` contains cells touched by node discs or structural edges;
 * `protectedCells` contains empty cells enclosed by that ink after a small
 * dilation. A candidate queries both with its own radius plus ISLAND_GAP, so
 * the mask is conservative while the circle index remains the final exact
 * clearance check for drawn discs. */
export interface IslandProtection {
  minX: number;
  minY: number;
  cellSize: number;
  columns: number;
  rows: number;
  occupied: ReadonlySet<number>;
  protectedCells: ReadonlySet<number>;
}

export interface IslandProtectionMetrics {
  cellSize: number;
  columns: number;
  rows: number;
  occupiedCells: number;
  protectedCells: number;
}

const protectionKey = (column: number, row: number): number =>
  column * MAX_PROTECTION_GRID_SIDE + row;

function markProtectionPoint(
  occupied: Set<number>,
  pointX: number,
  pointY: number,
  radius: number,
  minX: number,
  minY: number,
  cellSize: number,
  columns: number,
  rows: number,
): void {
  const minColumn = Math.max(0, Math.floor((pointX - radius - minX) / cellSize) - 1);
  const maxColumn = Math.min(columns - 1, Math.floor((pointX + radius - minX) / cellSize) + 1);
  const minRow = Math.max(0, Math.floor((pointY - radius - minY) / cellSize) - 1);
  const maxRow = Math.min(rows - 1, Math.floor((pointY + radius - minY) / cellSize) + 1);
  for (let column = minColumn; column <= maxColumn; column += 1) {
    const cellMinX = minX + column * cellSize;
    const cellMaxX = cellMinX + cellSize;
    for (let row = minRow; row <= maxRow; row += 1) {
      const cellMinY = minY + row * cellSize;
      const cellMaxY = cellMinY + cellSize;
      const dx = Math.max(cellMinX - pointX, 0, pointX - cellMaxX);
      const dy = Math.max(cellMinY - pointY, 0, pointY - cellMaxY);
      if (Math.hypot(dx, dy) <= radius) occupied.add(protectionKey(column, row));
    }
  }
}

function markProtectionSegment(
  occupied: Set<number>,
  segment: CoreSegment,
  minX: number,
  minY: number,
  cellSize: number,
  columns: number,
  rows: number,
): void {
  const length = Math.hypot(segment.targetX - segment.sourceX, segment.targetY - segment.sourceY);
  const samples = Math.max(1, Math.ceil(length / (cellSize * 0.5)));
  // Mark a little wider than the actual edge ink. This makes the grid useful
  // for hole detection even when an edge passes through a cell corner, while
  // the exact edge samples in the circle index retain the final guarantee.
  const maskRadius = EDGE_INK_RADIUS + cellSize * 0.75;
  for (let sample = 0; sample <= samples; sample += 1) {
    const t = sample / samples;
    markProtectionPoint(
      occupied,
      segment.sourceX + (segment.targetX - segment.sourceX) * t,
      segment.sourceY + (segment.targetY - segment.sourceY) * t,
      maskRadius,
      minX,
      minY,
      cellSize,
      columns,
      rows,
    );
  }
}

function dilateProtection(
  occupied: ReadonlySet<number>,
  columns: number,
  rows: number,
): Set<number> {
  const dilated = new Set<number>();
  for (const key of occupied) {
    const column = Math.floor(key / MAX_PROTECTION_GRID_SIDE);
    const row = key % MAX_PROTECTION_GRID_SIDE;
    for (let dc = -1; dc <= 1; dc += 1) {
      const nextColumn = column + dc;
      if (nextColumn < 0 || nextColumn >= columns) continue;
      for (let dr = -1; dr <= 1; dr += 1) {
        const nextRow = row + dr;
        if (nextRow >= 0 && nextRow < rows) dilated.add(protectionKey(nextColumn, nextRow));
      }
    }
  }
  return dilated;
}

/** Build the core mask from node discs and non-memory edges. Bounds are
 * deliberately only the drawn core extent; candidates outside those bounds
 * remain eligible, so a long thin component cannot turn its bounding box into
 * a giant protected rectangle. */
export function buildIslandProtection(
  graph: Graph,
  coreIds: readonly string[],
  satellites: ReadonlySet<string> = new Set(),
): IslandProtection {
  let minX = Infinity;
  let maxX = -Infinity;
  let minY = Infinity;
  let maxY = -Infinity;
  const coreSet = new Set(coreIds);
  const discs: Island[] = [];
  for (const id of coreIds) {
    if (satellites.has(id) || !graph.hasNode(id)) continue;
    const x = graph.getNodeAttribute(id, "x") as number;
    const y = graph.getNodeAttribute(id, "y") as number;
    const radius = (graph.getNodeAttribute(id, "size") as number) ?? 0;
    if (!Number.isFinite(x) || !Number.isFinite(y)) continue;
    discs.push({ x, y, r: radius });
    minX = Math.min(minX, x - radius);
    maxX = Math.max(maxX, x + radius);
    minY = Math.min(minY, y - radius);
    maxY = Math.max(maxY, y + radius);
  }

  const segments: CoreSegment[] = [];
  graph.forEachEdge((_key, _attrs, source, target) => {
    if (!coreSet.has(source) || !coreSet.has(target)) return;
    if (satellites.has(source) || satellites.has(target)) return;
    const sourceX = graph.getNodeAttribute(source, "x") as number;
    const sourceY = graph.getNodeAttribute(source, "y") as number;
    const targetX = graph.getNodeAttribute(target, "x") as number;
    const targetY = graph.getNodeAttribute(target, "y") as number;
    if (![sourceX, sourceY, targetX, targetY].every(Number.isFinite)) return;
    segments.push({ sourceX, sourceY, targetX, targetY });
    minX = Math.min(minX, sourceX, targetX);
    maxX = Math.max(maxX, sourceX, targetX);
    minY = Math.min(minY, sourceY, targetY);
    maxY = Math.max(maxY, sourceY, targetY);
  });

  if (minX === Infinity) {
    minX = -MIN_PROTECTION_CELL_SIZE;
    maxX = MIN_PROTECTION_CELL_SIZE;
    minY = -MIN_PROTECTION_CELL_SIZE;
    maxY = MIN_PROTECTION_CELL_SIZE;
  }
  const span = Math.max(maxX - minX, maxY - minY, MIN_PROTECTION_CELL_SIZE);
  const cellSize = Math.max(MIN_PROTECTION_CELL_SIZE, span / (MAX_PROTECTION_GRID_SIDE - 1));
  const columns = Math.min(MAX_PROTECTION_GRID_SIDE, Math.max(1, Math.ceil((maxX - minX) / cellSize) + 1));
  const rows = Math.min(MAX_PROTECTION_GRID_SIDE, Math.max(1, Math.ceil((maxY - minY) / cellSize) + 1));
  const occupied = new Set<number>();
  for (const disc of discs) {
    markProtectionPoint(occupied, disc.x, disc.y, disc.r, minX, minY, cellSize, columns, rows);
  }
  for (const segment of segments) {
    markProtectionSegment(occupied, segment, minX, minY, cellSize, columns, rows);
  }

  // One-cell dilation closes only gaps on the scale of the actual grid. A
  // flood fill from the grid boundary then identifies enclosed negative space;
  // open space around an irregular core remains available to islands.
  const dilated = dilateProtection(occupied, columns, rows);
  const outside = new Set<number>();
  const queue: number[] = [];
  const enqueue = (column: number, row: number) => {
    const key = protectionKey(column, row);
    if (dilated.has(key) || outside.has(key)) return;
    outside.add(key);
    queue.push(key);
  };
  for (let column = 0; column < columns; column += 1) {
    enqueue(column, 0);
    if (rows > 1) enqueue(column, rows - 1);
  }
  for (let row = 1; row < rows - 1; row += 1) {
    enqueue(0, row);
    if (columns > 1) enqueue(columns - 1, row);
  }
  for (let index = 0; index < queue.length; index += 1) {
    const key = queue[index] as number;
    const column = Math.floor(key / MAX_PROTECTION_GRID_SIDE);
    const row = key % MAX_PROTECTION_GRID_SIDE;
    if (column > 0) enqueue(column - 1, row);
    if (column + 1 < columns) enqueue(column + 1, row);
    if (row > 0) enqueue(column, row - 1);
    if (row + 1 < rows) enqueue(column, row + 1);
  }
  const protectedCells = new Set<number>();
  for (let column = 0; column < columns; column += 1) {
    for (let row = 0; row < rows; row += 1) {
      const key = protectionKey(column, row);
      if (!dilated.has(key) && !outside.has(key)) protectedCells.add(key);
    }
  }
  return { minX, minY, cellSize, columns, rows, occupied, protectedCells };
}

export function islandProtectionMetrics(protection: IslandProtection): IslandProtectionMetrics {
  return {
    cellSize: protection.cellSize,
    columns: protection.columns,
    rows: protection.rows,
    occupiedCells: protection.occupied.size,
    protectedCells: protection.protectedCells.size,
  };
}

/** Conservative grid predicate for a candidate island centre. Its square
 * footprint is queried because that is cheaper and safer than accepting a
 * circle that clips a protected cell corner. */
export function isIslandCenterProtected(
  protection: IslandProtection,
  x: number,
  y: number,
  radius: number,
): boolean {
  if (![x, y, radius].every(Number.isFinite) || radius < 0) return true;
  const reach = radius + ISLAND_GAP;
  const minColumn = Math.max(0, Math.floor((x - reach - protection.minX) / protection.cellSize));
  const maxColumn = Math.min(
    protection.columns - 1,
    Math.floor((x + reach - protection.minX) / protection.cellSize),
  );
  const minRow = Math.max(0, Math.floor((y - reach - protection.minY) / protection.cellSize));
  const maxRow = Math.min(
    protection.rows - 1,
    Math.floor((y + reach - protection.minY) / protection.cellSize),
  );
  if (minColumn > maxColumn || minRow > maxRow) return false;

  const span = (maxColumn - minColumn + 1) * (maxRow - minRow + 1);
  if (span > MAX_PROTECTION_QUERY_CELLS) {
    for (const key of [...protection.occupied, ...protection.protectedCells]) {
      const column = Math.floor(key / MAX_PROTECTION_GRID_SIDE);
      const row = key % MAX_PROTECTION_GRID_SIDE;
      if (column >= minColumn && column <= maxColumn && row >= minRow && row <= maxRow) return true;
    }
    return false;
  }
  for (let column = minColumn; column <= maxColumn; column += 1) {
    for (let row = minRow; row <= maxRow; row += 1) {
      const key = protectionKey(column, row);
      if (protection.occupied.has(key) || protection.protectedCells.has(key)) return true;
    }
  }
  return false;
}

function hashIslandIds(ids: readonly string[]): number {
  let hash = 2_166_136_261;
  for (const id of ids) {
    for (let index = 0; index < id.length; index += 1) {
      hash ^= id.charCodeAt(index);
      hash = Math.imul(hash, 16_777_619);
    }
    hash ^= 0;
    hash = Math.imul(hash, 16_777_619);
  }
  return hash >>> 0;
}

function islandRandom(seed: number, probe: number, stream: number): number {
  let value = (seed + Math.imul(probe + 1, 1_664_525) + Math.imul(stream + 1, 1_013_904_223)) >>> 0;
  value ^= value >>> 16;
  value = Math.imul(value, 2_246_822_507);
  value ^= value >>> 13;
  value = Math.imul(value, 3_266_489_909);
  return (value ^ (value >>> 16)) >>> 0;
}

function islandGaussianCandidate(seed: number, probe: number, sigma: number): { x: number; y: number } {
  // Start with a Box-Muller field, then smoothly compress rare outliers below.
  // Clamping u1 avoids log(0); no samples are snapped onto an outer ring.
  const u1 = Math.max(1e-7, islandRandom(seed, probe, 0) / 4_294_967_296);
  const u2 = islandRandom(seed, probe, 1) / 4_294_967_296;
  const gaussianMagnitude = Math.sqrt(-2 * Math.log(u1));
  // Keep the Gaussian's soft, non-geometric support while compressing rare
  // six-sigma samples. Without this gentle tail bend, a few late singletons
  // can dominate fit-to-screen even though no placement fallback occurred.
  const magnitude = gaussianMagnitude / Math.sqrt(1 + 0.06 * gaussianMagnitude * gaussianMagnitude);
  const angle = 2 * Math.PI * u2;
  const skew = (islandRandom(seed, probe, 2) / 4_294_967_296 - 0.5) * 0.18;
  const x = magnitude * Math.cos(angle) * sigma;
  return { x, y: (magnitude * Math.sin(angle) + skew * Math.cos(angle)) * sigma };
}

interface IslandPlan {
  box: ComponentBox;
  r: number;
  spacing: number;
  seed: number;
}

function coreCollisionIndex(
  graph: Graph,
  core: ComponentBox,
  satellites: ReadonlySet<string>,
): { index: CircleIndex; discs: Island[] } {
  const coreDiscs: Island[] = core.ids
    .filter((id) => !satellites.has(id))
    .map((id) => ({
      x: graph.getNodeAttribute(id, "x") as number,
      y: graph.getNodeAttribute(id, "y") as number,
      r: (graph.getNodeAttribute(id, "size") as number) ?? 0,
    }));
  const index = newCircleIndex();
  for (const disc of coreDiscs) addCircle(index, disc);
  // Edge circles use the same spatial index as node discs for the final
  // clearance check. The occupancy mask is intentionally conservative too,
  // but samples here prevent a candidate from slipping through a long edge's
  // cells at a corner.
  const coreSet = new Set(core.ids);
  graph.forEachEdge((_key, _attrs, source, target) => {
    if (!coreSet.has(source) || !coreSet.has(target)) return;
    if (satellites.has(source) || satellites.has(target)) return;
    const sourceX = graph.getNodeAttribute(source, "x") as number;
    const sourceY = graph.getNodeAttribute(source, "y") as number;
    const targetX = graph.getNodeAttribute(target, "x") as number;
    const targetY = graph.getNodeAttribute(target, "y") as number;
    const length = Math.hypot(targetX - sourceX, targetY - sourceY);
    const samples = Math.min(MAX_EDGE_INDEX_SAMPLES, Math.max(1, Math.ceil(length / EDGE_SAMPLE_STEP)));
    for (let sample = 0; sample <= samples; sample += 1) {
      const t = sample / samples;
      addCircle(index, {
        x: sourceX + (targetX - sourceX) * t,
        y: sourceY + (targetY - sourceY) * t,
        r: EDGE_INK_RADIUS,
      });
    }
  });
  return { index, discs: coreDiscs };
}

function islandPlan(box: ComponentBox, satellites: ReadonlySet<string>): IslandPlan {
  const r = Math.max(1, Math.hypot(boxWidth(box), boxHeight(box)) / 2);
  return {
    box,
    r,
    spacing: Math.max(48, r + ISLAND_GAP),
    seed: hashIslandIds(box.ids.filter((id) => !satellites.has(id)).sort()),
  };
}

function translateComponent(graph: Graph, box: ComponentBox, dx: number, dy: number): void {
  for (const id of box.ids) {
    graph.setNodeAttribute(id, "x", (graph.getNodeAttribute(id, "x") as number) + dx);
    graph.setNodeAttribute(id, "y", (graph.getNodeAttribute(id, "y") as number) + dy);
  }
  box.minX += dx;
  box.maxX += dx;
  box.minY += dy;
  box.maxY += dy;
}

function placeIslandBoxes(
  graph: Graph,
  core: ComponentBox,
  boxesToPlace: ComponentBox[],
  existingBoxes: ComponentBox[],
  satellites: ReadonlySet<string>,
): IslandPlacementMetrics {
  const protection = buildIslandProtection(graph, core.ids, satellites);
  const { index: coreIndex, discs: coreDiscs } = coreCollisionIndex(graph, core, satellites);
  const placedIndex = newCircleIndex();
  const existingIslands = existingBoxes.map((box) => ({
    x: (box.minX + box.maxX) / 2,
    y: (box.minY + box.maxY) / 2,
    r: Math.max(1, Math.hypot(boxWidth(box), boxHeight(box)) / 2),
  }));
  for (const island of existingIslands) addCircle(placedIndex, island);
  const placedIslands = [...existingIslands];
  const plans = boxesToPlace.map((box) => islandPlan(box, satellites));
  const totalSpacingSquared = [...existingIslands, ...plans].reduce(
    (sum, island) => sum + ("spacing" in island ? island.spacing * island.spacing : (island.r + ISLAND_GAP) ** 2),
    0,
  );
  const coreExtent = Math.hypot(boxWidth(core), boxHeight(core)) / 2;
  const globalSigma = Math.max(96, Math.sqrt(totalSpacingSquared / 2) * 1.08 + coreExtent * 0.22);
  let totalProbes = 0;
  let maxProbes = 0;
  let fallbackCount = 0;
  let placedSpacingSquared = existingIslands.reduce(
    (sum, island) => sum + (island.r + ISLAND_GAP) ** 2,
    0,
  );

  for (const { box, r, spacing, seed } of plans) {
    // Occupied footprint grows as components are accepted, so each later
    // group gets a gentle widening even when its first deterministic sample is
    // rejected. The batch cap keeps every per-island walk bounded.
    const occupiedSigma = Math.sqrt((placedSpacingSquared + spacing * spacing) / 2) * 1.04;
    const baseSigma = Math.max(globalSigma, occupiedSigma + coreExtent * 0.12);
    let candidate: Island | undefined;
    let probes = 0;
    for (; probes < MAX_ISLAND_PROBES; probes += 1) {
      const batch = Math.min(12, Math.floor(probes / 32));
      const point = islandGaussianCandidate(seed, probes, baseSigma * (1 + batch * 0.12));
      if (
        !isIslandCenterProtected(protection, point.x, point.y, r) &&
        clearOfCircles(coreIndex, point.x, point.y, r) &&
        clearOfCircles(placedIndex, point.x, point.y, r)
      ) {
        candidate = { ...point, r };
        break;
      }
    }
    if (candidate) {
      probes += 1;
      totalProbes += probes;
      maxProbes = Math.max(maxProbes, probes);
    }
    // The bounded probe loop normally finds a slot long before this. A
    // deterministic outer fallback keeps adversarial geometry finite while
    // retaining the same protection and circle-clearance guarantees.
    if (!candidate) {
      totalProbes += MAX_ISLAND_PROBES;
      maxProbes = Math.max(maxProbes, MAX_ISLAND_PROBES);
      fallbackCount += 1;
      let reach = r + ISLAND_GAP + 1;
      for (const disc of coreDiscs) {
        reach = Math.max(reach, Math.hypot(disc.x, disc.y) + disc.r + r + ISLAND_GAP);
      }
      for (const island of placedIslands) {
        reach = Math.max(reach, Math.hypot(island.x, island.y) + island.r + r + ISLAND_GAP);
      }
      // Include the farthest corner of the bounded mask so a fallback cannot
      // land on a structural edge even when the core has few node discs.
      const farthestCorner = Math.max(
        Math.hypot(protection.minX, protection.minY),
        Math.hypot(protection.minX + protection.cellSize * protection.columns, protection.minY),
        Math.hypot(protection.minX, protection.minY + protection.cellSize * protection.rows),
        Math.hypot(
          protection.minX + protection.cellSize * protection.columns,
          protection.minY + protection.cellSize * protection.rows,
        ),
      );
      reach = Math.max(reach, farthestCorner + r + ISLAND_GAP + 1);
      const angle = (islandRandom(seed, MAX_ISLAND_PROBES, 0) / 4_294_967_296) * 2 * Math.PI;
      for (let attempt = 0; attempt < 64; attempt += 1) {
        const radius = reach + attempt * Math.max(16, r + ISLAND_GAP);
        const point = { x: radius * Math.cos(angle), y: radius * Math.sin(angle) };
        if (
          !isIslandCenterProtected(protection, point.x, point.y, r) &&
          clearOfCircles(coreIndex, point.x, point.y, r) &&
          clearOfCircles(placedIndex, point.x, point.y, r)
        ) {
          candidate = { ...point, r };
          break;
        }
      }
      // The fallback radius is outside every indexed obstacle by construction;
      // retain a finite defensive value for an otherwise malformed graph.
      candidate ??= { x: reach, y: 0, r };
    }
    const placedPlan = { x: candidate.x, y: candidate.y, r: candidate.r };
    addCircle(placedIndex, placedPlan);
    placedIslands.push(placedPlan);
    placedSpacingSquared += (candidate.r + ISLAND_GAP) ** 2;
    translateComponent(graph, box, candidate.x - (box.minX + box.maxX) / 2, candidate.y - (box.minY + box.maxY) / 2);
  }
  return {
    islandCount: plans.length,
    totalProbes,
    maxProbes,
    fallbackCount,
    protection: islandProtectionMetrics(protection),
  };
}

/**
 * Pack disconnected components as a natural field around the core.
 *
 * The biggest component is the CORE: it keeps the layout the sim gave it and
 * is recentred so its bbox centre is the origin. Every other component is an
 * ISLAND, placed largest first. Island centres come from an ID-seeded,
 * irregular field with smoothly compressed tails. A bounded
 * protection mask keeps centres out of meaningful enclosed core voids, and
 * the circle index supplies the exact moat around the drawn core discs and
 * earlier islands. Each component is translated rigidly by its own centre, so
 * its internal geometry survives packing.
 *
 * At the opening view the islands are drawn dim, with no names or edges (see
 * LodState), so the eye lands on the core while the islands remain available
 * as quiet context. Zoom in past ISLANDS_SOLID_ZOOM and they come up solid.
 *
 * Returns the node ids of each component in placement order — the core first,
 * so the caller can tell it from the islands and hold each kind the way it
 * needs (rigid centroid for the core, springs for the islands).
 *
 * Pure in the sense that matters here: it reads x/y/size off the graph and
 * writes x/y back, touching nothing else and consulting no clock or random.
 */
export interface IslandPlacementMetrics {
  islandCount: number;
  totalProbes: number;
  maxProbes: number;
  fallbackCount: number;
  protection: IslandProtectionMetrics;
}

export interface IslandPlacementReport {
  placement: string[][];
  metrics: IslandPlacementMetrics;
}

function shelveComponentsDetailed(graph: Graph): IslandPlacementReport {
  if (graph.order === 0) {
    return {
      placement: [],
      metrics: {
        islandCount: 0,
        totalProbes: 0,
        maxProbes: 0,
        fallbackCount: 0,
        protection: { cellSize: 0, columns: 0, rows: 0, occupiedCells: 0, protectedCells: 0 },
      },
    };
  }
  const anchorOf = new Map(satellitePlan(graph).map((satellite) => [satellite.id, satellite.anchor]));
  const satellites = new Set(anchorOf.keys());
  // Ranked by the nodes that take room — satellites count for nothing here
  // either, or the memory layer could promote an island to core.
  const weight = (box: ComponentBox) => box.ids.filter((id) => !satellites.has(id)).length;
  const boxes = graphComponents(graph, anchorOf)
    .map((ids) => measureComponent(graph, ids, satellites))
    .sort((a, b) => {
      const aKey = a.ids[0] as string;
      const bKey = b.ids[0] as string;
      return weight(b) - weight(a) || (aKey < bKey ? -1 : aKey > bKey ? 1 : 0);
    });

  const core = boxes[0] as ComponentBox;
  translateComponent(graph, core, -(core.minX + core.maxX) / 2, -(core.minY + core.maxY) / 2);
  const metrics = placeIslandBoxes(graph, core, boxes.slice(1), [], satellites);
  return {
    placement: [core.ids, ...boxes.slice(1).map((box) => box.ids)],
    metrics,
  };
}

export function shelveComponents(graph: Graph): string[][] {
  return shelveComponentsDetailed(graph).placement;
}

export function shelveComponentsWithMetrics(graph: Graph): IslandPlacementReport {
  return shelveComponentsDetailed(graph);
}

/** Restore-aware shelf repair used after a layer or small-group rebuild.
 * Existing body coordinates are user state and stay where the caller put
 * them; only components with no saved body member are placed again. Those new
 * components are tested against the restored core and every surviving island,
 * which keeps a partial restore from leaving a fresh island in a protected
 * core void. */
/** Graph-space distance from a kept point to a new point placed beside it. */
const NEW_POINT_GAP = 14;

/** Deterministic angle for a point, so a new point lands in the same spot
 *  every time the same map is opened (UX-GRAPH-1). */
function stableAngle(id: string): number {
  let hash = 2166136261;
  for (let index = 0; index < id.length; index += 1) {
    hash ^= id.charCodeAt(index);
    hash = Math.imul(hash, 16777619);
  }
  return ((hash >>> 0) / 4294967296) * Math.PI * 2;
}

/**
 * A new page or entity that joined a group whose other points were restored
 * would otherwise keep its coordinate from the fresh layout, which belongs to
 * a different arrangement. Put it beside the kept points it links to instead,
 * then let chains of new points follow, so the old points never move and the
 * new one appears next to what it is about.
 */
export function placeNewBesideKept(graph: Graph, preservedIds: ReadonlySet<string>): void {
  const placed = new Set(preservedIds);
  const waiting = graph.nodes().filter((id) =>
    !placed.has(id) && graph.getNodeAttribute(id, "entityType") !== MEMORY_NODE_TYPE);
  for (let pass = 0; pass < 8 && waiting.length > 0; pass += 1) {
    const next: string[] = [];
    const settled: [string, number, number][] = [];
    for (const id of waiting) {
      let sumX = 0;
      let sumY = 0;
      let count = 0;
      graph.forEachNeighbor(id, (neighbor, attrs) => {
        if (!placed.has(neighbor) || attrs.entityType === MEMORY_NODE_TYPE) return;
        sumX += attrs.x as number;
        sumY += attrs.y as number;
        count += 1;
      });
      if (count === 0) {
        next.push(id);
        continue;
      }
      const angle = stableAngle(id);
      const size = (graph.getNodeAttribute(id, "size") as number | undefined) ?? 1;
      settled.push([id, sumX / count + Math.cos(angle) * (NEW_POINT_GAP + size), sumY / count + Math.sin(angle) * (NEW_POINT_GAP + size)]);
    }
    if (settled.length === 0) break;
    for (const [id, x, y] of settled) {
      graph.setNodeAttribute(id, "x", x);
      graph.setNodeAttribute(id, "y", y);
      placed.add(id);
    }
    waiting.splice(0, waiting.length, ...next);
  }
}

/** Clearance between a thing's cluster and the ink it is placed among, graph
 *  units: close enough that its link lines stay short, clear enough that
 *  its frame does not sit on another dot or line. */
const THING_GAP = 8;
/** How many rings outward the search for a thing's spot walks before it
 *  leaves the thing on its shelf slot. */
const THING_PLACE_RINGS = 240;

/** The bodies a thing's points link to (UX-CHAT-5): every page or entity a
 *  night's point has a line to, in id order. */
export function thingLinkedBodies(graph: Graph, thing: string): string[] {
  const linked = new Set<string>();
  graph.forEachNeighbor(thing, (point, attrs) => {
    if (attrs.night !== true) return;
    graph.forEachNeighbor(point, (other, otherAttrs) => {
      if (otherAttrs.entityType === MEMORY_NODE_TYPE || otherAttrs.entityType === THING_NODE_TYPE) return;
      linked.add(other);
    });
  });
  return [...linked].sort();
}

/**
 * Put each thing whose points link to the map NEAR what they link to
 * (UX-CHAT-5): its centre goes to the first free spot on rings round the
 * middle of those bodies, clear of every disc, every satellite a zoom can
 * show and every structural line, so its frame sits beside its subjects and
 * the link lines stay short. A thing with no link keeps the slot the shelf
 * gave it, beside the map. Things in `preservedIds` were placed on an earlier
 * open and never move (AtlasView keeps their spot with the rest of the body,
 * see positions.ts), and they count as ink for the ones placed now. Returns
 * the things it moved. Deterministic: same graph, same spots.
 */
export function placeThingCentres(graph: Graph, plan: readonly Satellite[], preservedIds: ReadonlySet<string> = new Set()): string[] {
  const linkedOf = new Map<string, string[]>();
  graph.forEachNode((id, attrs) => {
    if (attrs.entityType !== THING_NODE_TYPE || preservedIds.has(id)) return;
    const linked = thingLinkedBodies(graph, id);
    if (linked.length > 0) linkedOf.set(id, linked);
  });
  if (linkedOf.size === 0) return [];
  const satelliteOf = new Map(plan.map((satellite) => [satellite.id, satellite]));
  const at = (id: string) => ({ x: graph.getNodeAttribute(id, "x") as number, y: graph.getNodeAttribute(id, "y") as number });
  const index = newCircleIndex();
  graph.forEachNode((id, attrs) => {
    if (linkedOf.has(id)) return;
    const satellite = satelliteOf.get(id);
    if (satellite) {
      // A thing's own points are its cluster disc; dust past the deepest
      // zoom tier only shows on hover and takes no room here.
      if (linkedOf.has(satellite.anchor)) return;
      if (graph.getNodeAttribute(satellite.anchor, "entityType") === THING_NODE_TYPE) return;
      if (satellite.rank >= dustVisibleCount(Infinity)) return;
      const anchor = at(satellite.anchor);
      addCircle(index, {
        x: anchor.x + satellite.radius * Math.cos(satellite.angle),
        y: anchor.y + satellite.radius * Math.sin(satellite.angle),
        r: (attrs.size as number | undefined) ?? 0,
      });
      return;
    }
    if (attrs.entityType === MEMORY_NODE_TYPE) return;
    const { x, y } = at(id);
    if (Number.isFinite(x) && Number.isFinite(y)) addCircle(index, { x, y, r: bodyRadius(attrs) });
  });
  graph.forEachEdge((_key, _attrs, source, target) => {
    if (satelliteOf.has(source) || satelliteOf.has(target) || linkedOf.has(source) || linkedOf.has(target)) return;
    const a = at(source);
    const b = at(target);
    const samples = Math.min(MAX_EDGE_INDEX_SAMPLES, Math.max(1, Math.ceil(Math.hypot(b.x - a.x, b.y - a.y) / EDGE_SAMPLE_STEP)));
    for (let sample = 0; sample <= samples; sample += 1) {
      const t = sample / samples;
      addCircle(index, { x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t, r: EDGE_INK_RADIUS });
    }
  });
  const moved: string[] = [];
  for (const thing of [...linkedOf.keys()].sort()) {
    const linked = linkedOf.get(thing) as string[];
    let cx = 0;
    let cy = 0;
    for (const id of linked) {
      const p = at(id);
      cx += p.x / linked.length;
      cy += p.y / linked.length;
    }
    const radius = Math.max(bodyRadius(graph.getNodeAttributes(thing)), THING_CLUSTER_CORE);
    const step = Math.max(4, radius / 2);
    const start = mixedUnit(thing) * 2 * Math.PI;
    let spot: { x: number; y: number } | null = null;
    for (let ring = 0; ring <= THING_PLACE_RINGS && !spot; ring += 1) {
      const distance = ring * step;
      const count = ring === 0 ? 1 : Math.max(8, Math.ceil((2 * Math.PI * distance) / step));
      for (let k = 0; k < count; k += 1) {
        const angle = start + (2 * Math.PI * k) / count;
        const x = cx + distance * Math.cos(angle);
        const y = cy + distance * Math.sin(angle);
        if (clearOfCircles(index, x, y, radius, THING_GAP)) {
          spot = { x, y };
          break;
        }
      }
    }
    if (!spot) continue;
    graph.setNodeAttribute(thing, "x", spot.x);
    graph.setNodeAttribute(thing, "y", spot.y);
    addCircle(index, { x: spot.x, y: spot.y, r: radius });
    moved.push(thing);
  }
  return moved;
}

function placeUnrestoredComponents(
  graph: Graph,
  placement: readonly string[][],
  preservedIds: ReadonlySet<string>,
): void {
  if (placement.length < 2) return;
  const anchorOf = new Map(satellitePlan(graph).map((satellite) => [satellite.id, satellite.anchor]));
  const satellites = new Set(anchorOf.keys());
  const coreIds = placement[0] as string[];
  const core = measureComponent(graph, coreIds, satellites);
  const boxes = placement.slice(1).map((ids) => measureComponent(graph, ids, satellites));
  const bodyIds = (box: ComponentBox) => box.ids.filter((id) => !satellites.has(id));
  const newBoxes = boxes.filter((box) => {
    const ids = bodyIds(box);
    return ids.length > 0 && ids.every((id) => !preservedIds.has(id));
  });
  if (newBoxes.length === 0) return;
  const existingBoxes = boxes.filter((box) => !newBoxes.includes(box));
  const existingIndex = newCircleIndex();
  for (const box of existingBoxes) {
    addCircle(existingIndex, {
      x: (box.minX + box.maxX) / 2,
      y: (box.minY + box.maxY) / 2,
      r: Math.max(1, Math.hypot(boxWidth(box), boxHeight(box)) / 2),
    });
  }
  const protection = buildIslandProtection(graph, core.ids, satellites);
  const coreIndex = coreCollisionIndex(graph, core, satellites).index;
  const keptNewBoxes: ComponentBox[] = [];
  const boxesToPlace: ComponentBox[] = [];
  for (const box of newBoxes) {
    const r = Math.max(1, Math.hypot(boxWidth(box), boxHeight(box)) / 2);
    const x = (box.minX + box.maxX) / 2;
    const y = (box.minY + box.maxY) / 2;
    const valid =
      !isIslandCenterProtected(protection, x, y, r) &&
      clearOfCircles(coreIndex, x, y, r) &&
      clearOfCircles(existingIndex, x, y, r);
    if (valid) {
      keptNewBoxes.push(box);
      addCircle(existingIndex, { x, y, r });
    } else {
      boxesToPlace.push(box);
    }
  }
  if (boxesToPlace.length > 0) {
    placeIslandBoxes(graph, core, boxesToPlace, [...existingBoxes, ...keptNewBoxes], satellites);
  }
}

/** The sim plus the one thing the view has to tell it: which node the pointer
 *  is holding right now. Satellites are not sim nodes, so the writeback needs
 *  to be told to leave the dragged one alone (see placeSatellites). */
export interface AtlasSimulation extends Simulation<AtlasSimNode, undefined> {
  setDraggingId(id: string | null): void;
  /** Restore a previously settled non-memory body without running the shelf
   *  pack again. Used when only the memory layer changed; the saved body map
   *  is authoritative for both graphology and the live force nodes. */
  restorePositions(positions: ReadonlyMap<string, { x: number; y: number }>): void;
  /** Put every unheld shelf component back on its slot right now, instead of
   *  over the cooling tail. The release path uses it when there will be no
   *  cooling tail (see shelfAnchorForce's `settle`). */
  settleShelf(): void;
}

export interface AtlasSimNode extends SimulationNodeDatum {
  id: string;
  /** Collision radius: the drawn disc plus COLLIDE_PAD. */
  radius: number;
}

export interface AtlasSimLink {
  source: string | AtlasSimNode;
  target: string | AtlasSimNode;
  /** The graph edge's verb, which sets this link's rest length and pull. */
  type: string;
}

/** Node-to-node repulsion, matching the retired ConstellationMap feel. Named
 *  because the shelf relax pass (relaxShelf) has to run the same charge over
 *  its scratch copy or a component would relax to a different shape there
 *  than it holds in the live simulation. */
const CHARGE_STRENGTH = -40;
/** Collide runs at 0.7 rather than 1 so a collision resolves over a few ticks
 *  instead of snapping, which reads calmer. */
const COLLIDE_STRENGTH = 0.7;

/** Breathing room between two discs that are not otherwise pushed apart. Two
 *  px is enough to stop the overlap that made page clusters read as one blob
 *  without visibly loosening the rest of the map. */
const COLLIDE_PAD = 2;
/** Rest length and pull per edge verb; anything not listed keeps d3's own
 *  defaults (distance 30, strength 1/min(degree)). A shared-source edge is an
 *  inferred overlap, so it sits long and slack — it should suggest that two
 *  pages are near each other, not staple them together. A wikilink is an
 *  asserted link between pages, so it is shorter and firmer, but still longer
 *  than a relation so page clusters read as a stratum, not a clump. */
const LINK_LAYOUT: Record<string, { distance: number; strength: number }> = {
  [SHARED_SOURCE_EDGE_TYPE]: { distance: 70, strength: 0.15 },
  [WIKILINK_EDGE_TYPE]: { distance: 50, strength: 0.5 },
};

/** Which verb survives when parallel edges between one pair collapse to a
 *  single spring: the most strongly asserted one wins, so a wikilink is never
 *  loosened by a shared-source edge that happens to run beside it. */
function linkPriority(type: string): number {
  if (type === WIKILINK_EDGE_TYPE) return 3;
  if (type === CITES_EDGE_TYPE || type === MEMORY_EDGE_TYPE) return 1;
  if (type === SHARED_SOURCE_EDGE_TYPE) return 0;
  // Entity relations and page->entity `about` links: asserted, unremarkable.
  return 2;
}

function linkEndId(end: string | AtlasSimNode): string {
  return typeof end === "string" ? end : end.id;
}

/** Rest length for one link's verb — LINK_LAYOUT's, or d3's own 30. */
function linkDistanceFor(link: AtlasSimLink): number {
  return LINK_LAYOUT[link.type]?.distance ?? 30;
}

/** Pull for one link's verb. d3's default strength is 1/min(degree) over the
 *  LINK graph, computed privately inside forceLink — overriding .strength()
 *  would throw that away for every verb, so the same formula is rebuilt here
 *  from a precomputed degree map and used for anything LINK_LAYOUT does not
 *  name. */
function linkStrengthFor(link: AtlasSimLink, degree: Map<string, number>): number {
  const named = LINK_LAYOUT[link.type];
  if (named) return named.strength;
  const source = degree.get(linkEndId(link.source)) ?? 1;
  const target = degree.get(linkEndId(link.target)) ?? 1;
  return 1 / Math.min(source, target);
}

/** One shelf group's rigid centroid anchor: every free (non-fx/fy) member is
 *  shifted uniformly so the group's own centroid sits at `targetX/targetY`. */
interface CenterGroup {
  targetX: number;
  targetY: number;
}

/**
 * `forceCenter(0, 0)`, but blind to pinned nodes, PER GROUP, and able to hold
 * a target other than the origin.
 *
 * d3's own centre force averages EVERY node and shifts them all by the
 * offset. Pinned nodes snap straight back to fx/fy afterwards, so with a
 * shelf hanging below the core the average sits below the origin every tick
 * and the whole correction lands on the core: on the real capture a
 * drag-reheat slid the core 776 units up a 3,939-unit span, pulling the two
 * zones apart. Averaging the FREE nodes only cuts that to 371.
 *
 * `retarget()` removes the rest. shelveComponents centres each component by
 * its BOUNDING BOX, which is not where its centroid is; asking the force for
 * a centroid of exactly (0,0) therefore slides a component by the difference
 * on the first reheat. Retargeting after the shelf is laid out tells the
 * force to hold each component exactly where the composition put it, which
 * leaves only the small amount a component genuinely relaxes by.
 *
 * Round 5's live shelf generalizes this from one group to many (see
 * `setGroups`). On the live simulation only the CORE is held this way: a
 * rigid hold corrects position but not velocity, so a group carries whatever
 * momentum the external field gave it into the next tick — negligible across
 * the core's thousands of free nodes, but the dominant error on a handful of
 * nodes sitting thousands of units from the core's mass (measured at capture
 * scale: 28 units of centroid drift over a 60-tick drag reheat). Shelved
 * components are held by `shelfAnchorForce` instead. The many-group form is
 * still what `relaxShelf`'s scratch simulation uses, where every group is a
 * shelf component and there is no distant core to be pushed by.
 *
 * Before `setGroups` is ever called (the pre-shelf settle), every node falls
 * into one implicit group targeting the origin — exactly the old single-group
 * behavior. After it is called, a node no group names is left alone.
 */
function groupCenterForce(): {
  (alpha: number): void;
  initialize(nodes: AtlasSimNode[]): void;
  setGroups(placement: string[][]): void;
  retarget(): void;
} {
  let nodes: AtlasSimNode[] = [];
  // Null until setGroups runs: every node is then in one implicit group at the
  // origin. Once groups are installed, an id no group names is skipped — on
  // the live simulation that is every shelved node, held by shelfAnchorForce.
  let groupOf: Map<string, CenterGroup> | null = null;
  const defaultGroup: CenterGroup = { targetX: 0, targetY: 0 };
  const groupFor = (id: string): CenterGroup | undefined =>
    groupOf === null ? defaultGroup : groupOf.get(id);

  // One pass over all nodes, bucketed by group — O(nodes) regardless of how
  // many components are on the shelf, rather than O(nodes * groups).
  const centroids = (): Map<CenterGroup, { x: number; y: number; free: number }> => {
    const sums = new Map<CenterGroup, { x: number; y: number; free: number }>();
    for (const node of nodes) {
      if (node.fx != null || node.fy != null) continue;
      const group = groupFor(node.id);
      if (!group) continue;
      const entry = sums.get(group) ?? { x: 0, y: 0, free: 0 };
      entry.x += node.x ?? 0;
      entry.y += node.y ?? 0;
      entry.free += 1;
      sums.set(group, entry);
    }
    for (const entry of sums.values()) {
      entry.x /= entry.free;
      entry.y /= entry.free;
    }
    return sums;
  };

  const force = () => {
    const sums = centroids();
    for (const node of nodes) {
      if (node.fx != null || node.fy != null) continue;
      const group = groupFor(node.id);
      if (!group) continue;
      const centroid = sums.get(group);
      if (!centroid) continue;
      node.x = (node.x ?? 0) - (centroid.x - group.targetX);
      node.y = (node.y ?? 0) - (centroid.y - group.targetY);
    }
  };
  force.initialize = (ns: AtlasSimNode[]) => {
    nodes = ns;
  };
  // Installs one group per placement entry. An id no entry names is left to
  // whatever else holds it — on the live simulation, the shelf springs.
  force.setGroups = (placement: string[][]) => {
    const installed = new Map<string, CenterGroup>();
    groupOf = installed;
    for (const ids of placement) {
      const group: CenterGroup = { targetX: 0, targetY: 0 };
      for (const id of ids) installed.set(id, group);
    }
  };
  force.retarget = () => {
    for (const [group, centroid] of centroids()) {
      group.targetX = centroid.x;
      group.targetY = centroid.y;
    }
  };
  return force;
}

/** How hard a shelved node is pulled back to the slot the packing gave it.
 *  0.1 is d3's own forceX/forceY default: an order of magnitude weaker than a
 *  link (strength 1), so a hand on one node still drags its neighbors along
 *  instead of fighting the anchor, and a released node eases back to its slot
 *  instead of snapping. */
const SHELF_ANCHOR_STRENGTH = 0.1;

/** What fraction of a component's remaining centroid offset is closed per
 *  tick while no hand is on it. Deliberately NOT scaled by alpha, unlike every
 *  other force here: alpha is a finite budget (it decays 3%/tick toward
 *  alphaMin, so the whole cooling tail after a release sums to about 10), and
 *  an alpha-scaled return of this shape completes only ~53% of the journey
 *  before the simulation goes to sleep — measured as a 77.5-unit residual
 *  offset, enough for a released pair to sit inside the core's box. Unscaled,
 *  0.1 closes the offset geometrically (0.925 per tick after velocity decay)
 *  and is done long before alphaMin. It is the same rate as the springs, so a
 *  release reads as one motion rather than two. */
const SHELF_RETURN_RATE = 0.1;

/** One shelved node's slot, and which component it belongs to. */
interface ShelfAnchor {
  x: number;
  y: number;
  group: number;
}

/**
 * What holds the shelf together once no component is frozen with fx/fy.
 *
 * Two parts, and both are needed:
 *
 * - A per-node SPRING toward the exact position the packing gave that node.
 *   Unlike a centroid hold this resists rotation and shear as well as
 *   translation, which is what the packed rows actually need: an elongated
 *   component in the core's repulsion field is a body in a radial field, and
 *   it swings to point at the core unless something pulls each end back. Its
 *   centroid barely moves while it does that, so a centroid hold does not
 *   even see it — on the 24/6/5 fixture a five-node chain raised its top edge
 *   9.1 units toward the core over 120 ticks under a centroid-only hold, and
 *   0.75 with these springs. Soft, so a drag still wins locally.
 * - Control of each component's BULK velocity, for components with no hand on
 *   them: whatever link and charge just handed the component as a whole is
 *   replaced by exactly the velocity that carries its centroid back to the
 *   slot, `offset * SHELF_RETURN_RATE`. Only the uniform part is touched — the
 *   part that translates a component without changing its shape — so every
 *   relative motion (a drag pulling a neighbor, collide opening a knot) is
 *   left alone. This is what a spring this soft cannot do on its own: at
 *   capture scale the core's mass accelerates a distant component by roughly
 *   11 units per tick, which a 0.1 spring only cancels hundreds of units away
 *   from the slot. Measured together at capture scale: 28.5 units of drift
 *   under the centroid hold, 0.65 under these two.
 *
 *   Steering home rather than simply zeroing the bulk velocity is what makes a
 *   RELEASE land. Zeroing removes the spring's own return velocity again on
 *   the following tick, leaving only one alpha-scaled impulse per tick to do
 *   the whole journey — and alpha runs out first: a 200-unit drag on a shelved
 *   pair used to settle 77.5 units from its slot, overlapping the core's box
 *   by 13. With the bulk velocity steered home it lands within a unit.
 *
 * A component the pointer is holding is left entirely alone: with one node
 * pinned, "the group's bulk velocity" is mostly the drag response itself, and
 * overriding it would clamp the very neighbors that are supposed to follow.
 * The hand is the constraint while it is down; this force takes over on
 * release. When the release skips the cooling tail altogether (reduced
 * motion), `settle` does the same correction in one step.
 */
function shelfAnchorForce(): {
  (alpha: number): void;
  initialize(nodes: AtlasSimNode[]): void;
  setPlacement(shelf: string[][]): void;
  settle(): void;
} {
  let nodes: AtlasSimNode[] = [];
  let anchorOf = new Map<string, ShelfAnchor>();
  let groupCount = 0;

  interface Bulk {
    /** Summed velocity and summed offset-to-anchor over the group's free members. */
    vx: number;
    vy: number;
    dx: number;
    dy: number;
    free: number;
  }

  /** Per group: how its free members are moving, how far they are from their
   *  slots, and whether the pointer is holding one of them. */
  const measure = (): { bulk: Bulk[]; handOn: boolean[] } => {
    const bulk = Array.from({ length: groupCount }, () => ({ vx: 0, vy: 0, dx: 0, dy: 0, free: 0 }));
    const handOn = new Array<boolean>(groupCount).fill(false);
    for (const node of nodes) {
      const anchor = anchorOf.get(node.id);
      if (!anchor) continue;
      if (node.fx != null || node.fy != null) {
        handOn[anchor.group] = true;
        continue;
      }
      const entry = bulk[anchor.group] as Bulk;
      entry.vx += node.vx ?? 0;
      entry.vy += node.vy ?? 0;
      entry.dx += anchor.x - (node.x ?? 0);
      entry.dy += anchor.y - (node.y ?? 0);
      entry.free += 1;
    }
    return { bulk, handOn };
  };

  const force = (alpha: number) => {
    if (anchorOf.size === 0) return;
    const { bulk, handOn } = measure();
    for (const node of nodes) {
      const anchor = anchorOf.get(node.id);
      if (!anchor) continue;
      if (node.fx != null || node.fy != null) continue;
      const entry = bulk[anchor.group] as Bulk;
      if (!handOn[anchor.group] && entry.free > 0) {
        // Shift every member by (wanted bulk velocity - current bulk velocity),
        // both means over the group's free set.
        node.vx = (node.vx ?? 0) + (entry.dx * SHELF_RETURN_RATE - entry.vx) / entry.free;
        node.vy = (node.vy ?? 0) + (entry.dy * SHELF_RETURN_RATE - entry.vy) / entry.free;
      }
      node.vx = (node.vx ?? 0) + (anchor.x - (node.x ?? 0)) * SHELF_ANCHOR_STRENGTH * alpha;
      node.vy = (node.vy ?? 0) + (anchor.y - (node.y ?? 0)) * SHELF_ANCHOR_STRENGTH * alpha;
    }
  };
  force.initialize = (ns: AtlasSimNode[]) => {
    nodes = ns;
  };
  /** Anchor every named node where it stands right now — call it straight
   *  after the pack, whose output IS the arrangement to hold. */
  force.setPlacement = (shelf: string[][]) => {
    const byId = new Map(nodes.map((node) => [node.id, node]));
    anchorOf = new Map();
    groupCount = shelf.length;
    shelf.forEach((ids, group) => {
      for (const id of ids) {
        const node = byId.get(id);
        if (!node) continue;
        anchorOf.set(id, { x: node.x ?? 0, y: node.y ?? 0, group });
      }
    });
  };
  /** The cooling tail's whole job, done in one step: rigidly translate every
   *  unheld component back onto its slot and stop it dead. For the release
   *  path under reduced motion, where there IS no cooling tail — the
   *  simulation is stopped on mouseup, so a component would otherwise keep
   *  whatever displacement the drag gave it, up to and including sitting on
   *  top of the core. Rigid, so the shape the drag produced survives; only the
   *  offset that breaks the two zones apart is undone. */
  force.settle = () => {
    if (anchorOf.size === 0) return;
    const { bulk, handOn } = measure();
    for (const node of nodes) {
      const anchor = anchorOf.get(node.id);
      if (!anchor) continue;
      if (node.fx != null || node.fy != null) continue;
      const entry = bulk[anchor.group] as Bulk;
      if (handOn[anchor.group] || entry.free === 0) continue;
      node.x = (node.x ?? 0) + entry.dx / entry.free;
      node.y = (node.y ?? 0) + entry.dy / entry.free;
      node.vx = 0;
      node.vy = 0;
    }
  };
  return force;
}

/**
 * The knot-opening relax pass, run on a scratch simulation that owns the
 * SHELVED nodes only.
 *
 * Every component keeps its own rigid centroid hold here (groupCenterForce
 * over the shelf placement), so charge, collide and links can spread a
 * component that settled into a tangle without any of them wandering off its
 * slot. Relaxed positions are copied back onto `nodes` — the caller re-packs
 * afterwards, because a component that opened up no longer fits its old row.
 *
 * Scratch rather than the live simulation for two reasons: the core is
 * already settled, so re-running it buys nothing and costs the whole O(core)
 * charge (the live-sim version of this pass doubled Atlas's synchronous load
 * time, 543 ms to ~1,000 ms on the real capture); and with the core absent
 * there is no distant mass to push the shelf around while it relaxes.
 *
 * Returns the scratch simulation, which is stopped and whose only remaining
 * use is to show a caller (or a test) exactly which nodes it owned.
 */
export function relaxShelf(
  nodes: AtlasSimNode[],
  links: AtlasSimLink[],
  placement: string[][],
  linkDegree: Map<string, number>,
): Simulation<AtlasSimNode, undefined> {
  const shelfPlacement = placement.slice(1);
  const shelfIds = new Set(shelfPlacement.flat());
  // Fresh node and link objects: d3 stamps `index` onto every node it
  // simulates and rewrites a link's endpoints from id to node reference, so
  // handing it the live simulation's own objects would corrupt the live
  // collide and link forces.
  const scratch: AtlasSimNode[] = [];
  const relaxed = new Map<string, AtlasSimNode>();
  for (const node of nodes) {
    if (!shelfIds.has(node.id)) continue;
    const copy: AtlasSimNode = { id: node.id, x: node.x, y: node.y, radius: node.radius };
    scratch.push(copy);
    relaxed.set(node.id, copy);
  }
  const scratchLinks: AtlasSimLink[] = [];
  for (const link of links) {
    const source = linkEndId(link.source);
    const target = linkEndId(link.target);
    if (!shelfIds.has(source) || !shelfIds.has(target)) continue;
    scratchLinks.push({ source, target, type: link.type });
  }

  const center = groupCenterForce();
  const sim = forceSimulation(scratch)
    .force(
      "link",
      forceLink<AtlasSimNode, AtlasSimLink>(scratchLinks)
        .id((d) => d.id)
        .distance(linkDistanceFor)
        .strength((link) => linkStrengthFor(link, linkDegree)),
    )
    .force("charge", forceManyBody<AtlasSimNode>().strength(CHARGE_STRENGTH))
    .force("center", center)
    .force(
      "collide",
      forceCollide<AtlasSimNode>().radius((d) => d.radius).strength(COLLIDE_STRENGTH).iterations(1),
    )
    .alphaDecay(0.03)
    .velocityDecay(0.25)
    // forceSimulation starts its own animation frame loop on construction;
    // this pass is synchronous and ticks by hand.
    .stop();
  center.setGroups(shelfPlacement);
  center.retarget();
  sim.alpha(0.3);
  sim.tick(settleTicks(scratch.length));

  for (const node of nodes) {
    const copy = relaxed.get(node.id);
    if (!copy) continue;
    node.x = copy.x;
    node.y = copy.y;
  }
  return sim;
}

/** d3-force simulation over the live graphology graph — the interaction engine.
 *  Sim nodes are the drawable graph MINUS the nodes nonSimulatedIds names:
 *  degree-0 isolates and every memory. Neither is simulated —
 *  isolates are not drawn at all once drawableModel drops components under
 *  MIN_COMPONENT_SIZE, and a memory rides its anchor as a satellite.
 *  Matches the retired ConstellationMap feel: charge -40,
 *  centre-on-origin (groupCenterForce), alphaDecay 0.03, velocityDecay 0.25, d3-default link
 *  force. Parallel edges collapse to one link per undirected pair (d3 sums
 *  pull per link; sigma still RENDERS every parallel edge). Settles
 *  synchronously to its own equilibrium before returning — a FA2 seed handed
 *  straight to a fresh sim explodes toward the sim's roomier rest state on
 *  first drag; settling here means the graph the caller paints is already at
 *  rest, and a drag only flexes it (see round 5 spec). Every tick writes sim
 *  x/y back into the graph (sigma auto-repaints on attr change). */
export function createAtlasSimulation(
  graph: Graph,
  onTick?: () => void,
): AtlasSimulation {
  const excluded = new Set(nonSimulatedIds(graph));
  const satellites = satellitePlan(graph);
  annotateDust(graph, satellites);
  const nodes: AtlasSimNode[] = [];
  graph.forEachNode((id, attrs) => {
    if (excluded.has(id)) return;
    nodes.push({
      id,
      x: attrs.x as number,
      y: attrs.y as number,
      radius: bodyRadius(attrs) + COLLIDE_PAD,
    });
  });
  const simNodeById = new Map(nodes.map((node) => [node.id, node]));

  const linkByPair = new Map<string, AtlasSimLink>();
  graph.forEachEdge((_edge, attrs, source, target) => {
    // A link to a node the sim doesn't own would make d3 throw looking the
    // endpoint up; a memory's edges are drawn (on hover) but never simulated.
    if (excluded.has(source) || excluded.has(target)) return;
    const pairKey = [source, target].sort().join("|");
    const type = (attrs.edgeType as string) ?? "";
    const existing = linkByPair.get(pairKey);
    if (!existing) {
      linkByPair.set(pairKey, { source, target, type });
      return;
    }
    if (linkPriority(type) > linkPriority(existing.type)) existing.type = type;
  });
  const links = [...linkByPair.values()];

  // Degrees over the LINK graph, which is what d3's own 1/min(degree) default
  // strength is computed from (see linkStrengthFor).
  const linkDegree = new Map<string, number>();
  for (const link of links) {
    const source = linkEndId(link.source);
    const target = linkEndId(link.target);
    linkDegree.set(source, (linkDegree.get(source) ?? 0) + 1);
    linkDegree.set(target, (linkDegree.get(target) ?? 0) + 1);
  }

  const center = groupCenterForce();
  const shelfAnchor = shelfAnchorForce();
  const sim = forceSimulation(nodes)
    .force(
      "link",
      forceLink<AtlasSimNode, AtlasSimLink>(links)
        .id((d) => d.id)
        .distance(linkDistanceFor)
        .strength((link) => linkStrengthFor(link, linkDegree)),
    )
    .force("charge", forceManyBody<AtlasSimNode>().strength(CHARGE_STRENGTH))
    .force("center", center)
    // After link and charge, before collide: the shelf springs cancel the
    // bulk velocity those two just handed a shelved component, and collide
    // then gets the last word on discs that would otherwise overlap.
    .force("shelf", shelfAnchor)
    // Keeps discs off each other.
    .force(
      "collide",
      forceCollide<AtlasSimNode>().radius((d) => d.radius).strength(COLLIDE_STRENGTH).iterations(1),
    )
    .alphaDecay(0.03)
    .velocityDecay(0.25);

  // onTick runs after every position writeback so the caller can PAINT in
  // the same frame the physics stepped. Relying on sigma's graph-event
  // scheduled render instead paints every tick one frame late: d3's timer
  // and sigma's scheduler are separate rAF queues, and a render requested
  // mid-frame only runs on the next one — a constant extra frame of drag
  // latency (the old force-graph loop ticked and painted together).
  let draggingId: string | null = null;
  const writeBack = (notify = true) => {
    for (const node of nodes) {
      if (node.fx != null && node.fy != null) continue;
      graph.setNodeAttribute(node.id, "x", node.x);
      graph.setNodeAttribute(node.id, "y", node.y);
    }
    // Memories ride their anchor, so they are re-placed after every
    // position writeback — including mid-drag, which is what carries a
    // dragged entity's memories along with it. The one exception is a memory
    // the pointer is holding: that one is being positioned by hand.
    placeSatellites(graph, satellites, draggingId);
    if (notify) onTick?.();
  };
  sim.on("tick", writeBack);

  // d3's own per-frame loop (driven by restart()) calls its internal tick
  // step directly, bypassing this method — wrapping it only affects manual
  // callers, which keeps sim.tick() synchronous AND graph-synced for tests
  // without double-writing during live, restart()-driven dragging.
  const rawTick = sim.tick.bind(sim);
  sim.tick = (iterations?: number) => {
    rawTick(iterations);
    writeBack();
    return sim;
  };

  // Settle to equilibrium synchronously before first paint (≈ full alpha
  // decay at 0.03 when the budget allows it) — see the doc comment above.
  // Budgeted by the nodes the sim owns, not the graph: memories are not
  // simulated, and counting them would change the settle with the layer.
  sim.tick(settleTicks(nodes.length));

  // Round 5: the settle leaves every small component on roughly one circle
  // around the core (charge pushes out, forceCenter pulls in, and they
  // balance at about the same radius for all of them). Pack them as islands
  // round the core instead (see shelveComponents), then copy those positions
  // back INTO the sim so a later drag flexes the real layout rather than
  // snapping to the ring.
  let placement = shelveComponents(graph);
  for (const node of nodes) {
    node.x = graph.getNodeAttribute(node.id, "x") as number;
    node.y = graph.getNodeAttribute(node.id, "y") as number;
  }

  // Live shelf: instead of freezing every shelved component with fx/fy, open
  // the ones that settled into a tangled knot. Charge and collide inside a
  // component are what spread it; the core is already settled and would only
  // make the pass cost more, so this runs on a shelf-only scratch simulation
  // (see relaxShelf) whose results are copied back onto the sim nodes.
  relaxShelf(nodes, links, placement, linkDegree);
  for (const node of nodes) {
    graph.setNodeAttribute(node.id, "x", node.x);
    graph.setNodeAttribute(node.id, "y", node.y);
  }

  // Relaxing changes a component's bbox (that is the point — a knot spreading
  // out grows its own box), so the row packing above is stale. Re-shelve:
  // rigid translation preserves the shape the relax pass just found, it only
  // restores which row and how much space each component gets.
  placement = shelveComponents(graph);
  // A thing whose points link to the map leaves its shelf slot for a spot
  // beside what they link to (UX-CHAT-5); one with no link stays shelved.
  placeThingCentres(graph, satellites);
  for (const node of nodes) {
    node.x = graph.getNodeAttribute(node.id, "x") as number;
    node.y = graph.getNodeAttribute(node.id, "y") as number;
    // The settle and the relax both left velocity behind, and the pack just
    // teleported everything anyway — a later reheat must start from rest
    // rather than resume momentum aimed at positions that no longer exist.
    node.vx = 0;
    node.vy = 0;
  }
  // The core keeps the rigid centroid hold it has always had; every island
  // is held at the slot this pack just gave it by its own springs.
  center.setGroups(placement.slice(0, 1));
  center.retarget();
  shelfAnchor.setPlacement(placement.slice(1));
  // Which nodes are islands, for the reducers (dim at the opening view).
  const islandIds = new Set(placement.slice(1).flat());
  graph.forEachNode((id) => {
    graph.setNodeAttribute(id, "island", islandIds.has(id));
  });

  // A thing's centre is pinned where it was placed: no force moves a frame,
  // and the discs a drag pushes about are pushed off it (collide), never it
  // off them. Its spot is kept with the rest of the body (UX-GRAPH-1).
  const pinThings = () => {
    for (const node of nodes) {
      if (graph.getNodeAttribute(node.id, "entityType") !== THING_NODE_TYPE) continue;
      node.fx = node.x;
      node.fy = node.y;
      node.vx = 0;
      node.vy = 0;
    }
  };
  pinThings();

  // Same writeback path as a tick, so satellites re-place around their moved
  // anchors and the caller's onTick marks the cartography scene dirty.
  writeBack();

  sim.alpha(0);
  sim.stop();

  const atlasSim = sim as AtlasSimulation;
  atlasSim.setDraggingId = (id: string | null) => {
    draggingId = id;
  };
  atlasSim.restorePositions = (positions) => {
    for (const [id, position] of positions) {
      if (!graph.hasNode(id)) continue;
      // The caller normally supplies only the non-memory body, but keep this
      // boundary defensive so stale memory coordinates can never fight the
      // satellite placement below.
      if (graph.getNodeAttribute(id, "entityType") === MEMORY_NODE_TYPE) continue;
      if (!Number.isFinite(position.x) || !Number.isFinite(position.y)) continue;
      graph.setNodeAttribute(id, "x", position.x);
      graph.setNodeAttribute(id, "y", position.y);
      const simNode = simNodeById.get(id);
      if (!simNode) continue;
      simNode.x = position.x;
      simNode.y = position.y;
      simNode.vx = 0;
      simNode.vy = 0;
    }
    // A rebuild can add whole components after this simulation's initial
    // shelf pass. Keep restored survivors fixed, then place only those new
    // components against the restored core and surviving islands.
    const preservedIds = new Set(positions.keys());
    placeUnrestoredComponents(graph, placement, preservedIds);
    placeNewBesideKept(graph, preservedIds);
    // A thing seen before keeps its kept spot; a new one goes beside what
    // its points link to on the map as it stands now.
    placeThingCentres(graph, satellites, preservedIds);
    for (const node of nodes) {
      if (preservedIds.has(node.id)) continue;
      const x = graph.getNodeAttribute(node.id, "x") as number;
      const y = graph.getNodeAttribute(node.id, "y") as number;
      if (!Number.isFinite(x) || !Number.isFinite(y)) continue;
      node.x = x;
      node.y = y;
      node.vx = 0;
      node.vy = 0;
    }
    pinThings();
    // The component membership and placement remain those from the settled
    // graph. Only the actual coordinates and their force targets move.
    center.retarget();
    shelfAnchor.setPlacement(placement.slice(1));
    sim.alpha(0).alphaTarget(0).stop();
    // The renderer has not been installed yet. Re-place satellites through
    // the normal writeback path, but do not notify a previous renderer via
    // onTick while the new graph is still mounting.
    writeBack(false);
    sim.alpha(0).alphaTarget(0).stop();
  };
  atlasSim.settleShelf = () => {
    shelfAnchor.settle();
    writeBack();
  };
  return atlasSim;
}

export interface HoverState {
  hovered: string | null;
  neighbors: Set<string>;
  /** Set when the focus is a thing picked on the map (night.ts's
   *  thingFocusState): `neighbors` is then its lit set, and the lines inside
   *  that set are drawn. */
  thing?: string;
}

/** The most a node's disc grows to on screen, CSS px radius: memory dust
 *  stays a speck at any zoom, a subject grows to 8, the hovered node to 12.
 *  A night's point is its thing's content, not dust (UX-CHAT-5): it grows
 *  with the zoom like a subject, so a frame seen close is filled. */
export function nodeSizeCapPx(attrs: Record<string, unknown>, hovered: boolean): number {
  if (hovered) return 12;
  return attrs.entityType === MEMORY_NODE_TYPE && attrs.night !== true ? 2.8 : 8;
}

/** Hovered node id plus its neighbor set, or the empty state when nothing is hovered. */
export function hoverStateFor(graph: Graph, hovered: string | null): HoverState {
  if (hovered === null) return { hovered: null, neighbors: new Set() };
  return { hovered, neighbors: new Set(graph.neighbors(hovered)) };
}

// sigma's own nodeReducer/edgeReducer types resolve `data` to graphology's
// permissive `Attributes` (`{[name: string]: any}`) for a default-generic
// Graph, and expect a return assignable to `Partial<NodeDisplayData>` /
// `Partial<EdgeDisplayData>` — neither of which this package exposes without
// pulling in graphology-types as a new direct dependency. `any` matches what
// sigma already passes through, so it typechecks both ways without one.

/** Dim fill for an island at the opening view: the node's own ink at 72%
 *  over the ground. Cached per (ink, ground) pair — the reducer runs per node
 *  per frame. */
const dimFillCache = new Map<string, string>();
function dimFill(color: string, surface: string): string {
  return dimFillAt(color, surface, 0.72);
}

function dimFillAt(color: string, surface: string, alpha: number): string {
  const key = `${color}|${surface}|${alpha}`;
  let dim = dimFillCache.get(key);
  if (dim === undefined) {
    dim = compositeOver(color, surface, alpha);
    dimFillCache.set(key, dim);
  }
  return dim;
}

/** Overview ink for ordinary nodes: visible enough to preserve map texture,
 * quiet enough that the selected landmarks establish the first hierarchy. */
const SEMANTIC_SUBDUED_ALPHA = 0.78;
const LANDMARK_MIN_SIZE = 4;

/**
 * Node display override for sigma's nodeReducer — pure so it's unit-testable
 * without a sigma renderer. Two concerns, zoom first, then hover:
 *
 * - ZOOM (`lod`). A memory is dust on its anchor: only the first
 *   `lod.dustVisible` of an anchor's memories are drawn (by `dustRank`),
 *   unless the anchor is hovered, which shows the first HOVER_DUST_MAX. An island node (`island`) is drawn dim and nameless until
 *   `lod.islandsSolid`.
 * - HOVER, the round-2 spec: no-hover passthrough, the hovered node itself,
 *   its neighbors, everyone else muted and blanked. The hovered node and, when
 *   there are at most NEIGHBOR_LABEL_MAX of them, its neighbors are LIFTED:
 *   `highlighted` makes sigma redraw their discs and labels on the hover
 *   layer, above every label the grid paints, so a lit dot is never buried
 *   under a neighbouring name. Neighbors are named (`forceLabel`) because the
 *   whole point of lighting them is to show what the node connects to, and
 *   small nodes never earn a label from sigma's size threshold on their own;
 *   where those names land is layoutFocusLabels' job. Past the cap the label
 *   grid decides, so a hub with dozens of memories does not turn into a wall
 *   of text.
 */
export function nodeDisplay(
  state: HoverState,
  nodeId: string,
  attrs: Record<string, any>,
  palette: GraphPalette,
  lod: LodState = OPENING_LOD,
): Record<string, any> {
  // A thing is its frame (AtlasView paints it round the points); its node is
  // only the centre they cluster on.
  if (attrs.entityType === THING_NODE_TYPE) return { ...attrs, hidden: true };
  const dustRank = attrs.dustRank as number | undefined;
  // A night's points are the thing's whole picture: never folded into a count.
  if (dustRank !== undefined && dustRank >= lod.dustVisible && attrs.night !== true) {
    const anchorHovered = state.hovered === attrs.dustOf && dustRank < Math.max(lod.dustVisible, HOVER_DUST_MAX);
    // A picked thing's point is never folded away, wherever it sits.
    const pickedPoint = state.thing !== undefined && state.neighbors.has(nodeId);
    if (!anchorHovered && !pickedPoint && state.hovered !== nodeId) return { ...attrs, hidden: true };
  }
  let base = attrs;
  // A night's point is the work itself: drawn whole at every zoom, never as
  // part of a dim island.
  if (attrs.island && !lod.islandsSolid && attrs.night !== true) {
    base = { ...attrs, color: dimFill(attrs.color as string, palette.surface), label: "" };
  }
  // Keep orientation names through the middle zoom range, where ordinary
  // discs have not yet reached Sigma's label-size threshold.
  if (attrs.landmarkLabel === true && attrs.entityType !== MEMORY_NODE_TYPE) {
    base = { ...base, forceLabel: true };
  }
  // `landmark` is deliberately checked by key presence as well as value: an
  // untagged graph is a legacy fixture/caller and must retain its old display
  // behavior until applyAtlasHierarchy has been run.
  if (lod.phase === "overview" && Object.prototype.hasOwnProperty.call(attrs, "landmark") && attrs.entityType !== MEMORY_NODE_TYPE) {
    if (attrs.landmark === true) {
      base = {
        ...base,
        size: Math.max((base.size as number) ?? 0, LANDMARK_MIN_SIZE),
        ...(attrs.landmarkLabel === true ? { forceLabel: true } : {}),
      };
    } else {
      base = {
        ...base,
        color: dimFillAt((base.color as string) ?? palette.edge, palette.surface, SEMANTIC_SUBDUED_ALPHA),
        label: "",
      };
    }
  }
  if (state.hovered === null) return base;
  if (nodeId === state.hovered) return { ...attrs, color: attrs.entityType ? nodeFillFor(attrs.entityType, true, palette) : attrs.color, size: attrs.size * 1.15, forceLabel: true, highlighted: true, zIndex: 2 };
  if (state.neighbors.has(nodeId)) {
    return state.neighbors.size <= NEIGHBOR_LABEL_MAX
      ? { ...attrs, color: attrs.entityType ? nodeFillFor(attrs.entityType, true, palette) : attrs.color, forceLabel: true, highlighted: true, zIndex: 1 }
      : { ...attrs, color: attrs.entityType ? nodeFillFor(attrs.entityType, true, palette) : attrs.color, zIndex: 1 };
  }
  return { ...base, color: dimFillAt(palette.neutral, palette.surface, 0.17), label: "", forceLabel: false, zIndex: 0 };
}

/**
 * Edge display override for sigma's edgeReducer. Memory edges are quiet at
 * rest, including the thread to the anchor: the satellites already show the
 * relationship spatially, and persistent spokes turn a busy halo into a
 * tangle. Hovering/focusing either endpoint reveals its incident edges. An
 * island's edges are hidden while the island is dim. Then the hover rule:
 * edges incident to the hovered node get emphasized, everything else hides.
 */
export function edgeDisplay(
  state: HoverState,
  _edgeId: string,
  source: string,
  target: string,
  attrs: Record<string, any>,
  palette: GraphPalette,
  lod: LodState = OPENING_LOD,
  endpointAttrs?: { source: Record<string, any>; target: Record<string, any> },
): Record<string, any> {
  // A point's tie to its thing's hidden centre is never a line on the map,
  // hovered or not (UX-CHAT-5): the frame is what says they belong together.
  if (endpointAttrs && (endpointAttrs.source.entityType === THING_NODE_TYPE || endpointAttrs.target.entityType === THING_NODE_TYPE)) {
    return { ...attrs, hidden: true };
  }
  // A picked thing: the lines among its lit set show, its points' lines to
  // what they are about drawn strong.
  if (state.thing !== undefined && state.neighbors.has(source) && state.neighbors.has(target)) {
    const night = endpointAttrs !== undefined && (endpointAttrs.source.night === true || endpointAttrs.target.night === true);
    return night ? { ...attrs, color: palette.edgeStrong, size: Math.max(attrs.size ?? 0, 0.9), zIndex: 1 } : { ...attrs, zIndex: 1 };
  }
  const incident = state.hovered !== null && (source === state.hovered || target === state.hovered);
  if (incident && endpointAttrs && state.neighbors.size > HOVER_DUST_MAX) {
    const memorySource = endpointAttrs.source.dustRank !== undefined;
    const memoryTarget = endpointAttrs.target.dustRank !== undefined;
    const memoryFocused = (memorySource && state.hovered === source) || (memoryTarget && state.hovered === target);
    // A busy hub's satellites already express membership. Hundreds of lit
    // spokes obscure both them and the structural links; reveal the threads
    // when one particular memory is inspected instead.
    if ((memorySource || memoryTarget) && !memoryFocused) return { ...attrs, hidden: true };
  }
  if (incident) return { ...attrs, color: palette.edgeStrong, size: Math.max(attrs.size ?? 0, 0.9), zIndex: 1 };
  if (state.hovered !== null) return { ...attrs, hidden: true };
  if (endpointAttrs) {
    const { source: s, target: t } = endpointAttrs;
    // A night's point stays in its thing's cluster, so its line to an entity
    // can cross the whole map; at rest it is not drawn (UX-CHAT-5), only
    // when the point is hovered (above) or its thing is picked.
    if (s.night === true || t.night === true) return { ...attrs, hidden: true };
    // Satellite position is enough context at rest. Every memory edge waits
    // for the memory or one of its endpoints to be hovered/focused, including
    // the anchor thread; this removes the baseline spoke clutter while
    // keeping the existing incident-edge emphasis above.
    if (s.dustRank !== undefined || t.dustRank !== undefined) {
      return { ...attrs, hidden: true };
    }
    if ((s.island || t.island) && !lod.islandsSolid) return { ...attrs, hidden: true };
    const hierarchyApplied = Object.prototype.hasOwnProperty.call(s, "landmark") || Object.prototype.hasOwnProperty.call(t, "landmark");
    if (hierarchyApplied && lod.phase === "overview" && s.landmark !== true && t.landmark !== true) {
      return { ...attrs, hidden: true };
    }
  }
  return attrs;
}

/** The node-label typeface: the app's body face, with the system font behind
 *  it for the rare machine that has not loaded it. */
export const NODE_LABEL_FONT = '12px "Instrument Sans", -apple-system, sans-serif';
/** Ground-coloured halo stroke behind a node label, in CSS px — the label
 *  stays legible where it crosses an edge or a neighbouring disc. */
const NODE_LABEL_HALO_WIDTH = 3;

/**
 * Sector-radial label drawer, ported from the old canvas graph: 12px body
 * font at 85% ink, placed left/right/above/below the node by its angle from
 * the graph center (0,0 — forceCenter pins the cluster there) so labels face
 * INWARD toward the cluster instead of expanding the bbox. Graph y is negated
 * for the angle: sigma renders graph +y screen-up while the old canvas
 * rendered it screen-down, and inward placement must track the on-screen
 * quadrant, not the raw coordinate. `halo`, when given, is the ground colour
 * stroked behind the text first. Wired into sigma via
 * settings.defaultDrawNodeLabel (data carries the node key plus viewport
 * x/y/size — see sigma's renderLabels call site).
 */
export function drawRadialNodeLabel(
  context: CanvasRenderingContext2D,
  data: Record<string, any>,
  settings: Record<string, any>,
  graph: Graph,
  halo?: string,
): void {
  if (!data.label) return;
  const gx = graph.getNodeAttribute(data.key, "x") as number;
  const gy = graph.getNodeAttribute(data.key, "y") as number;
  const angle = Math.atan2(-gy, gx);
  const sector = Math.round((angle + Math.PI) / (Math.PI / 2)) % 4;
  const side: LabelSide = sector === 0 ? "right" : sector === 2 ? "left" : sector === 1 ? "below" : "above";
  drawNodeLabelAt(context, data, settings, labelAnchor(data.x as number, data.y as number, data.size as number, side), halo);
}

/** Which side of its dot a label sits on. */
export type LabelSide = "left" | "right" | "above" | "below";

/** A resolved label position: the canvas anchor plus how the text hangs off it. */
export interface LabelPlacement {
  x: number;
  y: number;
  align: CanvasTextAlign;
  baseline: CanvasTextBaseline;
}

/** Anchor for a label on `side` of a dot at (x, y) of radius `size`, `push`
 *  extra px further out. The same geometry drawRadialNodeLabel always used. */
export function labelAnchor(x: number, y: number, size: number, side: LabelSide, push = 0): LabelPlacement {
  const pad = size + 8 + push;
  switch (side) {
    case "right":
      return { x: x + pad, y, align: "left", baseline: "middle" };
    case "left":
      return { x: x - pad, y, align: "right", baseline: "middle" };
    case "below":
      return { x, y: y + pad, align: "center", baseline: "top" };
    case "above":
      return { x, y: y - pad, align: "center", baseline: "bottom" };
  }
}

/** Paint one label at a resolved placement: the shared 12px face at 85% ink
 *  over an optional ground-coloured halo. */
export function drawNodeLabelAt(
  context: CanvasRenderingContext2D,
  data: Record<string, any>,
  settings: Record<string, any>,
  at: LabelPlacement,
  halo?: string,
): void {
  if (!data.label) return;
  context.textAlign = at.align;
  context.textBaseline = at.baseline;
  context.font = NODE_LABEL_FONT;
  context.fillStyle = settings.labelColor?.color ?? "#000000";
  context.globalAlpha = 0.85;
  if (halo) {
    context.lineJoin = "round";
    context.lineWidth = NODE_LABEL_HALO_WIDTH;
    context.strokeStyle = halo;
    context.strokeText(data.label, at.x, at.y);
  }
  context.fillText(data.label, at.x, at.y);
  context.globalAlpha = 1;
}

/** A node as the focus layout sees it: viewport px, CSS-px radius, its name. */
export interface FocusLabelNode {
  key: string;
  x: number;
  y: number;
  size: number;
  label: string;
}

/** Height of one 12px label line, in CSS px, for collision boxes. */
const LABEL_LINE_HEIGHT = 14;
/** Clearance kept around a lit dot and between two names, in CSS px. */
const LABEL_CLEARANCE = 2;

interface Box {
  x: number;
  y: number;
  w: number;
  h: number;
}

function overlaps(a: Box, b: Box): boolean {
  return (
    a.x < b.x + b.w + LABEL_CLEARANCE &&
    a.x + a.w + LABEL_CLEARANCE > b.x &&
    a.y < b.y + b.h + LABEL_CLEARANCE &&
    a.y + a.h + LABEL_CLEARANCE > b.y
  );
}

function boxFor(at: LabelPlacement, width: number): Box {
  const x = at.align === "left" ? at.x : at.align === "right" ? at.x - width : at.x - width / 2;
  const y =
    at.baseline === "top" ? at.y : at.baseline === "bottom" ? at.y - LABEL_LINE_HEIGHT : at.y - LABEL_LINE_HEIGHT / 2;
  return { x, y, w: width, h: LABEL_LINE_HEIGHT };
}

function discBox(node: FocusLabelNode): Box {
  const r = node.size + LABEL_CLEARANCE;
  return { x: node.x - r, y: node.y - r, w: 2 * r, h: 2 * r };
}

/** The side of `from` that points away from (dx, dy): the dominant axis wins. */
function sideAway(dx: number, dy: number): LabelSide {
  if (Math.abs(dx) >= Math.abs(dy)) return dx >= 0 ? "right" : "left";
  return dy >= 0 ? "below" : "above";
}

const SIDES: LabelSide[] = ["right", "left", "above", "below"];

/**
 * Where the names of a focused neighborhood go. A hover or a search jump
 * lights one node and its neighbors and names them all; in a tight cluster
 * the plain radial rule stacks those names on top of each other and over the
 * lit dots themselves, and the last one painted wins. This pass lays them
 * out instead:
 *
 * - each name radiates AWAY from the focus (a neighbor right of the focus
 *   hangs its name to the right), the focus's own name away from where its
 *   neighbors sit, so the cluster reads as a hub with spokes;
 * - names are placed nearest-first and never overlap a placed name or any
 *   lit dot; a name whose preferred side is taken tries the other sides,
 *   then its preferred side pushed further out;
 * - a name that fits nowhere is left off the map rather than painted over
 *   something. Its dot stays lit and hoverable.
 *
 * Coordinates are viewport CSS px; `measure` is the canvas text width for
 * the label font. Returns a placement per node key that got one.
 */
export function layoutFocusLabels(
  focus: FocusLabelNode,
  neighbors: FocusLabelNode[],
  measure: (text: string) => number,
): Map<string, LabelPlacement> {
  const placed: Box[] = [focus, ...neighbors].map(discBox);
  const out = new Map<string, LabelPlacement>();
  const tryPlace = (node: FocusLabelNode, preferred: LabelSide) => {
    if (!node.label) return;
    const width = measure(node.label);
    const candidates: Array<[LabelSide, number]> = [
      [preferred, 0],
      ...SIDES.filter((side) => side !== preferred).map((side): [LabelSide, number] => [side, 0]),
      [preferred, LABEL_LINE_HEIGHT],
      [preferred, 2 * LABEL_LINE_HEIGHT],
    ];
    for (const [side, push] of candidates) {
      const at = labelAnchor(node.x, node.y, node.size, side, push);
      const box = boxFor(at, width);
      if (placed.some((other) => overlaps(box, other))) continue;
      placed.push(box);
      out.set(node.key, at);
      return;
    }
  };
  // The focus faces away from the centroid of its neighbors.
  let cx = 0;
  let cy = 0;
  for (const n of neighbors) {
    cx += n.x - focus.x;
    cy += n.y - focus.y;
  }
  tryPlace(focus, neighbors.length === 0 ? "right" : sideAway(-cx, -cy));
  const byDistance = [...neighbors].sort(
    (a, b) => Math.hypot(a.x - focus.x, a.y - focus.y) - Math.hypot(b.x - focus.x, b.y - focus.y),
  );
  for (const n of byDistance) tryPlace(n, sideAway(n.x - focus.x, n.y - focus.y));
  return out;
}
