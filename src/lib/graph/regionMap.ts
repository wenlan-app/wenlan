// SPDX-License-Identifier: AGPL-3.0-only
// Round 4, layout R2: a community base map. The structural backbone is cut
// into regions (Louvain), each region is laid out on its own as a block, and
// the blocks are arranged like countries on a map. Leaves ring their parent,
// memory-only entities sit on their region's outer ring, and what has no tie
// to any region is left for the island shelf (createAtlasSimulation).
import Graph from "graphology";
import louvain from "graphology-communities-louvain";
import forceAtlas2 from "graphology-layout-forceatlas2";
import { forceCollide, forceLink, forceManyBody, forceSimulation, forceX, forceY, type SimulationNodeDatum } from "d3-force";
import { MEMORY_NODE_TYPE } from "./model";

const THING = "thing";
const GOLDEN = Math.PI * (3 - Math.sqrt(5));

/** Node attribute: the region a body belongs to on an R2 map. */
export const REGION_ATTR = "region";
/** Node attribute: how the body sits in its region ("hub" | "backbone" | "leaf" | "ring"). */
export const REGION_ROLE_ATTR = "regionRole";
/** Edge attribute on an R2 map: which line class the edge is. */
export const REGION_EDGE_ATTR = "regionEdge";
/** Graph attribute: the regions, for the names, tints and the kept map. */
export const REGIONS_GRAPH_ATTR = "atlasRegions";

export type RegionEdgeClass =
  /** A body's one line to its best-linked neighbour in its own region (drawn at fit). */
  | "forest"
  /** Any other line inside one region (drawn from 2x). */
  | "intra"
  /** The one hub-to-hub line of a region pair with weight >= BRIDGE_MIN (drawn from 2x). */
  | "bridge"
  /** A line between two regions (4x, with the fan cap). */
  | "cross"
  /** A line to a super-hub (focus only). */
  | "super";

export interface RegionInfo {
  id: string;
  name: string;
  leaderId: string;
  centre: { x: number; y: number };
  radius: number;
  members: number;
  backbone: number;
  leaves: number;
  ring: number;
  superHub: boolean;
}

export interface RegionMapReport {
  counts: Record<string, number>;
  resolution: number;
  modularity: number;
  regions: RegionInfo[];
  timesMs: Record<string, number>;
  /** Bodies placed into a region, and those left for the island shelf. */
  placed: number;
  islands: number;
}

export interface RegionMapOptions {
  scope?: string;
  /** Visible dust ring per anchor at the 4x zoom, graph units (for block sizes). */
  dustRing?: ReadonlyMap<string, { count: number; ring: number }>;
  /** Memory-co-occurrence placement of memory-only entities (Fable's rule). */
  coOccurrence?: CoOccurrenceRule | false;
  /** Only decide membership and line classes; leave every position as it is
   *  (a kept map put back, or a probe). */
  assignOnly?: boolean;
  /** Region count bounds; outside them Louvain re-runs once at another resolution. */
  regionCount?: { min: number; max: number };
  /** A community with fewer backbone bodies merges into its best neighbour. */
  minRegion?: number;
  /** Region membership kept from an earlier layout (body id -> region key).
   *  When given, communities are not found again: every kept body keeps its
   *  region, and only bodies new since then join one, by their links' vote. */
  stored?: ReadonlyMap<string, string>;
}

export interface CoOccurrenceRule {
  /** Each memory shared by k bodies gives each pair 1/(k-1). Memories naming more than this many bodies are digests, not co-occurrence, and are skipped. */
  maxBodiesPerMemory: number;
  /** Minimum summed weight toward the chosen region (1.0 = one focused memory's worth). */
  minWeight: number;
  /** The chosen region must carry this many times the runner-up's weight; anything closer stays on the rim. */
  margin: number;
  passes: number;
}

/** Round 4 rule: k <= 8, W >= 1.0, W >= 2x the runner-up, 3 passes. */
export const DEFAULT_CO_OCCURRENCE: CoOccurrenceRule = { maxBodiesPerMemory: 8, minWeight: 1, margin: 2, passes: 3 };

const SUPER_HUB_DEGREE = 400;
const SUPER_HUB_SHARE = 0.2;
const MIN_COMPONENT = 5;
const DEFAULT_MIN_REGION = 8;
const BRIDGE_MIN = 3;
/** Graph units per sqrt(weighted member) of a block's radius. */
const BLOCK_UNIT = 15;
const MIN_BLOCK_RADIUS = 28;
const LEAF_MARGIN = 14;
const BLOCK_GAP = 24;
const COLLIDE_GAP = 12;
const ASPECT = 1.6;
const RING_SPACING = 9;
const RING_BAND = 10;
const RING_BANDS = 3;
/** Ground kept round a super-hub for its dust, graph units. */
const SUPER_HUB_DUST_RING = 10;

const typeWeight = (type: string): number => (type === "shared_source" ? 0.5 : 1);
const byId = (a: string, b: string) => (a < b ? -1 : a > b ? 1 : 0);

function hash32(text: string): number {
  let h = 2166136261;
  for (let i = 0; i < text.length; i += 1) {
    h ^= text.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  return h >>> 0;
}

function mulberry32(seed: number): () => number {
  let a = seed;
  return () => {
    a |= 0;
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function stableAngle(id: string): number {
  return (hash32(id) / 4294967296) * Math.PI * 2;
}

const now = () => (typeof performance !== "undefined" ? performance.now() : Date.now());

interface BlockNode extends SimulationNodeDatum {
  id: string;
  r: number;
}

/**
 * Lay an R2 map out in place: writes x/y for every body in a region, and the
 * region attributes the display reads. Islands (no tie to any region) keep
 * their coordinates and carry no region attribute, so the shelf packs them.
 */
export function layoutRegionMap(graph: Graph, options: RegionMapOptions = {}): RegionMapReport {
  const times: Record<string, number> = {};
  let t = now();
  const lap = (name: string) => {
    const n = now();
    times[name] = Math.round((n - t) * 10) / 10;
    t = n;
  };
  const minRegion = options.minRegion ?? DEFAULT_MIN_REGION;
  // Main-component regions (round 4: 6-10 for a backbone of ~200).
  const bounds = options.regionCount ?? { min: 6, max: 10 };

  // ---- 1. Kinds ---------------------------------------------------------
  const isBody = (attrs: Record<string, any>) =>
    attrs.entityType !== MEMORY_NODE_TYPE && attrs.entityType !== THING && attrs.night !== true;
  const bodies: string[] = [];
  graph.forEachNode((id, attrs) => {
    if (isBody(attrs)) bodies.push(id);
    if (attrs[REGION_ATTR] !== undefined) graph.removeNodeAttribute(id, REGION_ATTR);
    if (attrs[REGION_ROLE_ATTR] !== undefined) graph.removeNodeAttribute(id, REGION_ROLE_ATTR);
  });
  bodies.sort(byId);
  const bodySet = new Set(bodies);
  const adj = new Map<string, Map<string, number>>(bodies.map((id) => [id, new Map()]));
  graph.forEachEdge((_key, attrs, source, target) => {
    if (source === target || !bodySet.has(source) || !bodySet.has(target)) return;
    const w = typeWeight((attrs.edgeType as string) ?? "");
    const a = adj.get(source) as Map<string, number>;
    const b = adj.get(target) as Map<string, number>;
    a.set(target, (a.get(target) ?? 0) + w);
    b.set(source, (b.get(source) ?? 0) + w);
  });
  const degree = (id: string) => adj.get(id)?.size ?? 0;
  const label = (id: string) => (graph.getNodeAttribute(id, "label") as string) ?? id;
  const byLeader = (a: string, b: string) => degree(b) - degree(a) || (label(a) < label(b) ? -1 : label(a) > label(b) ? 1 : byId(a, b));

  const componentOf = new Map<string, number>();
  const componentSize: number[] = [];
  for (const id of bodies) {
    if (componentOf.has(id)) continue;
    const index = componentSize.length;
    const stack = [id];
    componentOf.set(id, index);
    let size = 0;
    while (stack.length > 0) {
      const x = stack.pop() as string;
      size += 1;
      for (const y of (adj.get(x) as Map<string, number>).keys()) {
        if (componentOf.has(y)) continue;
        componentOf.set(y, index);
        stack.push(y);
      }
    }
    componentSize.push(size);
  }
  const inMap = (id: string) => (componentSize[componentOf.get(id) as number] ?? 0) >= MIN_COMPONENT;
  const backbone = bodies.filter((id) => inMap(id) && degree(id) >= 2);
  const leafParent = new Map<string, string>();
  for (const id of bodies) {
    if (!inMap(id) || degree(id) !== 1) continue;
    leafParent.set(id, [...(adj.get(id) as Map<string, number>).keys()][0] as string);
  }
  const superHubs = backbone.filter((id) => degree(id) > SUPER_HUB_DEGREE || degree(id) > SUPER_HUB_SHARE * backbone.length);
  const superSet = new Set(superHubs);

  // ---- 2. Communities ---------------------------------------------------
  const pairWeight = (a: string, b: string, raw: number) =>
    Math.min(1, 50 / Math.max(1, degree(a))) * Math.min(1, 50 / Math.max(1, degree(b))) * raw;
  // Louvain runs on the main component; every other component of five or
  // more is one region of its own (it has nothing to merge into).
  let mainComponent = -1;
  for (let i = 0; i < componentSize.length; i += 1) if (mainComponent < 0 || (componentSize[i] as number) > (componentSize[mainComponent] as number)) mainComponent = i;
  const leafCount = new Map<string, number>();
  for (const parent of leafParent.values()) leafCount.set(parent, (leafCount.get(parent) ?? 0) + 1);
  const lg = new Graph({ type: "undirected" });
  for (const id of backbone) if (!superSet.has(id) && componentOf.get(id) === mainComponent) lg.addNode(id);
  for (const id of lg.nodes()) {
    for (const [other, raw] of adj.get(id) as Map<string, number>) {
      if (other <= id || !lg.hasNode(other)) continue;
      lg.addEdge(id, other, { weight: pairWeight(id, other, raw) });
    }
  }
  const scope = options.scope ?? "";
  const runLouvain = (resolution: number) => {
    const result = louvain.detailed(lg, {
      resolution,
      randomWalk: false,
      rng: mulberry32(hash32(`${scope}atlas-regions-v4`)),
      getEdgeWeight: "weight",
    });
    return { resolution, modularity: result.modularity, communities: result.communities as Record<string, number> };
  };

  // A super-hub is a region of its own; its members are the bodies that have
  // nothing else in the backbone.
  const mergeSmall = (communities: Record<string, number>): Map<string, string> => {
    const groups = new Map<string, string[]>();
    for (const id of lg.nodes()) {
      const key = `c${communities[id]}`;
      const list = groups.get(key);
      if (list) list.push(id);
      else groups.set(key, [id]);
    }
    const regionOf = new Map<string, string>();
    for (const [key, ids] of groups) for (const id of ids) regionOf.set(id, key);
    // The floor counts structural members: the backbone body and its leaves.
    const structural = (ids: string[]) => ids.reduce((n, id) => n + 1 + (leafCount.get(id) ?? 0), 0);
    const weightBetween = (key: string) => {
      const toward = new Map<string, number>();
      for (const id of groups.get(key) as string[]) {
        for (const [other, w] of lg.edges(id).map((e) => [lg.opposite(id, e), lg.getEdgeAttribute(e, "weight") as number] as const)) {
          const otherKey = regionOf.get(other) as string;
          if (otherKey !== key) toward.set(otherKey, (toward.get(otherKey) ?? 0) + w);
        }
      }
      return toward;
    };
    for (;;) {
      const small = [...groups.entries()]
        .filter(([, ids]) => structural(ids) < minRegion)
        .sort((a, b) => structural(a[1]) - structural(b[1]) || byId(a[0], b[0]));
      let merged = false;
      for (const [key] of small) {
        const toward = weightBetween(key);
        if (toward.size === 0) continue;
        const target = [...toward.entries()].sort((a, b) => b[1] - a[1] || (groups.get(a[0])!.length - groups.get(b[0])!.length) || byId(a[0], b[0]))[0]![0];
        const ids = groups.get(key) as string[];
        (groups.get(target) as string[]).push(...ids);
        for (const id of ids) regionOf.set(id, target);
        groups.delete(key);
        merged = true;
        break;
      }
      if (!merged) break;
    }
    return regionOf;
  };

  let run: { resolution: number; modularity: number } = { resolution: 0, modularity: 0 };
  let regionOf = new Map<string, string>();
  const role = new Map<string, string>();
  const stored = options.stored && options.stored.size > 0 ? options.stored : undefined;
  const keptRing = new Set<string>();
  if (stored) {
    const backboneSet = new Set(backbone);
    // Kept membership: every body seen before keeps its region; its role
    // follows the shape it has now.
    for (const id of bodies) {
      const key = stored.get(id);
      if (key === undefined) continue;
      regionOf.set(id, key);
      if (leafParent.has(id)) role.set(id, "leaf");
      else if (backboneSet.has(id)) role.set(id, "backbone");
      else {
        role.set(id, "ring");
        keptRing.add(id);
      }
    }
    // New backbone bodies join the region their links weigh most toward;
    // passes let a chain of new bodies follow the first one in.
    const fresh = backbone.filter((id) => !regionOf.has(id));
    for (let pass = 0; pass < 4 && fresh.some((id) => !regionOf.has(id)); pass += 1) {
      const decided: [string, string][] = [];
      for (const id of fresh) {
        if (regionOf.has(id)) continue;
        const toward = new Map<string, number>();
        for (const [other, w] of adj.get(id) as Map<string, number>) {
          if (superSet.has(other) && !superSet.has(id)) continue;
          const key = regionOf.get(other);
          if (key !== undefined) toward.set(key, (toward.get(key) ?? 0) + w);
        }
        const best = [...toward.entries()].sort((a, b) => b[1] - a[1] || byId(a[0], b[0]))[0];
        if (best) decided.push([id, best[0]]);
      }
      for (const [id, key] of decided) {
        regionOf.set(id, key);
        role.set(id, "backbone");
      }
      if (decided.length === 0) break;
    }
    for (const id of fresh) {
      if (regionOf.has(id)) continue;
      // Only a super-hub link, or a new component: the hub's region, or a
      // region of its own.
      const hub = superHubs.filter((h) => adj.get(id)?.has(h)).sort(byLeader)[0];
      const key = superSet.has(id) ? `s:${id}` : hub !== undefined ? (stored.get(hub) ?? `s:${hub}`) : `k:${componentOf.get(id)}`;
      regionOf.set(id, key);
      role.set(id, "backbone");
    }
    for (const [leaf, parent] of leafParent) {
      if (regionOf.has(leaf)) continue;
      const key = regionOf.get(parent);
      if (key === undefined) continue;
      regionOf.set(leaf, key);
      role.set(leaf, "leaf");
    }
    lap("kept");
  } else {
    const first = runLouvain(1.0);
    run = first;
    regionOf = mergeSmall(first.communities);
    const countRegions = (map: Map<string, string>) => {
      const sizes = new Map<string, number>();
      for (const [id, key] of map) sizes.set(key, (sizes.get(key) ?? 0) + 1 + (leafCount.get(id) ?? 0));
      return [...sizes.values()].filter((n) => n >= minRegion).length + superHubs.length;
    };
    const firstCount = countRegions(regionOf);
    if (firstCount < bounds.min || firstCount > bounds.max) {
      const retry = runLouvain(firstCount < bounds.min ? 1.4 : 0.7);
      const retryRegions = mergeSmall(retry.communities);
      const retryCount = countRegions(retryRegions);
      const off = (n: number) => (n < bounds.min ? bounds.min - n : n > bounds.max ? n - bounds.max : 0);
      if (off(retryCount) < off(firstCount)) {
        run = retry;
        regionOf = retryRegions;
      }
    }
    lap("louvain");

    // Super-hub regions; a backbone body left as a lone community with no
    // tie but the super-hub joins it.
    for (const hub of superHubs) regionOf.set(hub, `s:${hub}`);
    for (const id of backbone) {
      if (regionOf.has(id)) continue;
      regionOf.set(id, `k:${componentOf.get(id)}`);
    }
    const sizeOf = new Map<string, number>();
    for (const [id, key] of regionOf) sizeOf.set(key, (sizeOf.get(key) ?? 0) + 1 + (leafCount.get(id) ?? 0));
    // A main-component community still under the floor hangs off a super-hub
    // (that is why it had nothing to merge into): it joins that hub whole.
    const fragments = new Map<string, string[]>();
    for (const id of lg.nodes()) {
      const key = regionOf.get(id) as string;
      if ((sizeOf.get(key) ?? 0) >= minRegion) continue;
      const list = fragments.get(key);
      if (list) list.push(id);
      else fragments.set(key, [id]);
    }
    for (const ids of fragments.values()) {
      const ties = new Map<string, number>();
      for (const id of ids) for (const hub of superHubs) if (adj.get(id)?.has(hub)) ties.set(hub, (ties.get(hub) ?? 0) + 1);
      const hub = [...ties.entries()].sort((a, b) => b[1] - a[1] || byLeader(a[0], b[0]))[0]?.[0];
      if (hub === undefined) continue;
      for (const id of ids) regionOf.set(id, `s:${hub}`);
    }
    for (const id of regionOf.keys()) role.set(id, "backbone");
    // Leaves inherit their parent's region.
    for (const [leaf, parent] of leafParent) {
      const key = regionOf.get(parent);
      if (key === undefined) continue;
      regionOf.set(leaf, key);
      role.set(leaf, "leaf");
    }
  }
  // A component of two to four bodies, or a lone body, is not in the map yet.

  // ---- 2b. Memory-only entities by co-occurrence -------------------------
  const ringOf = new Map<string, string>();
  const evidence = new Map<string, { weight: number; pass: number }>();
  const passAdded: number[] = [];
  const rule = options.coOccurrence === undefined ? DEFAULT_CO_OCCURRENCE : options.coOccurrence;
  if (rule) {
    const co = new Map<string, Map<string, number>>();
    graph.forEachNode((memory, attrs) => {
      if (attrs.entityType !== MEMORY_NODE_TYPE) return;
      const named = new Set<string>();
      for (const n of graph.neighbors(memory)) if (bodySet.has(n)) named.add(n);
      if (named.size < 2 || named.size > rule.maxBodiesPerMemory) return;
      const w = 1 / (named.size - 1);
      const list = [...named];
      for (const a of list) {
        let row = co.get(a);
        if (!row) co.set(a, (row = new Map()));
        for (const b of list) if (a !== b) row.set(b, (row.get(b) ?? 0) + w);
      }
    });
    const candidates = bodies.filter((id) => !regionOf.has(id) && degree(id) === 0);
    for (let pass = 0; pass < rule.passes; pass += 1) {
      const decided: [string, string][] = [];
      for (const id of candidates) {
        if (ringOf.has(id)) continue;
        const row = co.get(id);
        if (!row) continue;
        const toward = new Map<string, number>();
        for (const [other, w] of row) {
          const key = regionOf.get(other) ?? ringOf.get(other);
          if (key !== undefined) toward.set(key, (toward.get(key) ?? 0) + w);
        }
        const ranked = [...toward.entries()].sort((a, b) => b[1] - a[1] || byId(a[0], b[0]));
        const best = ranked[0];
        const second = ranked[1]?.[1] ?? 0;
        if (!best || best[1] < rule.minWeight || best[1] < rule.margin * second) continue;
        evidence.set(id, { weight: best[1], pass });
        decided.push([id, best[0]]);
      }
      // A pass decides on the map as the previous pass left it, so the order
      // bodies are visited in never changes the answer.
      for (const [id, key] of decided) ringOf.set(id, key);
      passAdded.push(decided.length);
      if (decided.length === 0) break;
    }
    for (const [id, key] of ringOf) {
      regionOf.set(id, key);
      role.set(id, "ring");
    }
  }
  // Kept memory-only members stay on their region's band, innermost.
  for (const id of keptRing) {
    ringOf.set(id, regionOf.get(id) as string);
    evidence.set(id, { weight: 0, pass: -1 });
  }
  lap("cooccurrence");

  // ---- 3. Regions -------------------------------------------------------
  const members = new Map<string, string[]>();
  for (const [id, key] of regionOf) {
    const list = members.get(key);
    if (list) list.push(id);
    else members.set(key, [id]);
  }
  for (const list of members.values()) list.sort(byId);
  const dustRing = options.dustRing ?? new Map();
  // A super-hub's dust is far too much to show as a ring; it shows as its
  // count badge, so its region does not keep empty ground for it (round 4:
  // the wenlan block was a wide empty ring round its hub).
  const ringAround = (id: string) => (superSet.has(id) ? Math.min(SUPER_HUB_DUST_RING, dustRing.get(id)?.ring ?? 0) : dustRing.get(id)?.ring ?? 0);
  const dustCount = (id: string) => (superSet.has(id) ? 0 : dustRing.get(id)?.count ?? 0);
  const blockRadius = new Map<string, number>();
  const regionKeys = [...members.keys()].sort(byId);
  const counts = (key: string) => {
    let bb = 0, lv = 0, rg = 0, dust = 0;
    for (const id of members.get(key) as string[]) {
      const r = role.get(id);
      if (r === "leaf") lv += 1;
      else if (r === "ring") rg += 1;
      else bb += 1;
      dust += dustCount(id);
    }
    return { bb, lv, rg, dust };
  };
  // The core's area is honest to structure; ring members widen the block by
  // bands round it, at most RING_BANDS deep, then packed closer.
  const coreRadius = new Map<string, number>();
  const bandsOf = new Map<string, number>();
  for (const key of regionKeys) {
    const c = counts(key);
    const core = Math.max(MIN_BLOCK_RADIUS, BLOCK_UNIT * Math.sqrt(c.bb + c.lv / 3 + c.dust / 8));
    const perBand = Math.max(1, Math.floor((2 * Math.PI * core) / RING_SPACING));
    const bands = c.rg === 0 ? 0 : Math.min(RING_BANDS, Math.ceil(c.rg / perBand));
    coreRadius.set(key, core);
    bandsOf.set(key, bands);
    blockRadius.set(key, core + bands * RING_BAND);
  }

  let centreOf = new Map<string, { x: number; y: number }>();
  if (options.assignOnly) {
    for (const key of regionKeys) {
      const ids = members.get(key) as string[];
      let sx = 0, sy = 0;
      for (const id of ids) {
        sx += (graph.getNodeAttribute(id, "x") as number) ?? 0;
        sy += (graph.getNodeAttribute(id, "y") as number) ?? 0;
      }
      centreOf.set(key, { x: sx / ids.length, y: sy / ids.length });
    }
  } else {
  // ---- 3.1 Inside a block -----------------------------------------------
  const local = new Map<string, { x: number; y: number }>();
  const bodySize = (id: string) => (graph.getNodeAttribute(id, "size") as number) ?? 3;
  for (const key of regionKeys) {
    const ids = (members.get(key) as string[]).filter((id) => role.get(id) === "backbone");
    const leader = [...ids].sort(byLeader)[0] as string;
    if (ids.length <= 3) {
      ids.sort(byLeader).forEach((id, i) => {
        const a = (i * 2 * Math.PI) / 3 - Math.PI / 2;
        local.set(id, i === 0 && ids.length < 3 ? { x: 0, y: 0 } : { x: 18 * Math.cos(a), y: 18 * Math.sin(a) });
      });
      continue;
    }
    const block = new Graph({ type: "undirected" });
    const leavesOf = new Map<string, number>();
    for (const [leaf, parent] of leafParent) if (regionOf.get(leaf) === key) leavesOf.set(parent, (leavesOf.get(parent) ?? 0) + 1);
    ids.forEach((id, i) => {
      const a = (i / ids.length) * Math.PI * 2;
      const ring = ringAround(id);
      const leafRing = leavesOf.has(id) ? 10 + 5 * Math.sqrt(leavesOf.get(id) as number) : 0;
      block.addNode(id, {
        x: id === leader ? 0 : 100 * Math.cos(a),
        y: id === leader ? 0 : 100 * Math.sin(a),
        size: bodySize(id) + Math.max(ring, leafRing),
        fixed: id === leader,
      });
    });
    for (const id of ids) {
      for (const [other, raw] of adj.get(id) as Map<string, number>) {
        if (other <= id || !block.hasNode(other)) continue;
        block.addEdge(id, other, { weight: pairWeight(id, other, raw) });
      }
    }
    forceAtlas2.assign(block, {
      iterations: 200,
      getEdgeWeight: "weight",
      settings: {
        linLogMode: true,
        strongGravityMode: true,
        gravity: 1.5,
        scalingRatio: 10,
        adjustSizes: true,
        outboundAttractionDistribution: true,
      },
    });
    const radii = ids.map((id) => Math.hypot(block.getNodeAttribute(id, "x"), block.getNodeAttribute(id, "y"))).sort((a, b) => a - b);
    const p90 = radii[Math.floor(0.9 * (radii.length - 1))] || 1;
    const target = (coreRadius.get(key) as number) - LEAF_MARGIN;
    const scale = Math.max(target, 10) / p90;
    for (const id of ids) local.set(id, { x: block.getNodeAttribute(id, "x") * scale, y: block.getNodeAttribute(id, "y") * scale });
  }
  lap("blocks");

  // ---- 3.3 Between blocks -----------------------------------------------
  const weightBetween = new Map<string, number>();
  for (const id of regionOf.keys()) {
    if (role.get(id) !== "backbone") continue;
    for (const [other, raw] of adj.get(id) as Map<string, number>) {
      if (other <= id) continue;
      const a = regionOf.get(id) as string;
      const b = regionOf.get(other);
      if (b === undefined || a === b) continue;
      const pair = a < b ? `${a}|${b}` : `${b}|${a}`;
      weightBetween.set(pair, (weightBetween.get(pair) ?? 0) + raw);
    }
  }
  const bySize = [...regionKeys].sort((a, b) => (blockRadius.get(b) as number) - (blockRadius.get(a) as number) || byId(a, b));
  const pinned = superHubs.length > 0 ? `s:${[...superHubs].sort(byLeader)[0]}` : bySize[0];
  const blockNodes: BlockNode[] = bySize.map((id, i) => {
    const a = i * GOLDEN;
    const r = id === pinned ? 0 : 60 * Math.sqrt(i + 1);
    const node: BlockNode = { id, r: blockRadius.get(id) as number, x: r * Math.cos(a), y: r * Math.sin(a) };
    if (id === pinned) {
      node.fx = 0;
      node.fy = 0;
    }
    return node;
  });
  const blockLinks = [...weightBetween.entries()].map(([pair, w]) => {
    const [source, target] = pair.split("|") as [string, string];
    return { source, target, w };
  });
  const sim = forceSimulation(blockNodes)
    .force(
      "link",
      forceLink<BlockNode, { source: string | BlockNode; target: string | BlockNode; w: number }>(blockLinks)
        .id((d) => d.id)
        .distance((l) => (l.source as BlockNode).r + (l.target as BlockNode).r + BLOCK_GAP)
        .strength((l) => 0.3 * Math.min(1, l.w / 5)),
    )
    .force("charge", forceManyBody<BlockNode>().strength(-30))
    .force("collide", forceCollide<BlockNode>().radius((d) => d.r + COLLIDE_GAP).strength(1).iterations(3))
    .force("x", forceX<BlockNode>(0).strength(0.04))
    .force("y", forceY<BlockNode>(0).strength(0.064))
    .stop();
  sim.tick(300);
  // Stretch to the map's aspect, then let collide clear what that closed.
  let minX = Infinity, maxX = -Infinity, minY = Infinity, maxY = -Infinity;
  for (const n of blockNodes) {
    minX = Math.min(minX, (n.x ?? 0) - n.r);
    maxX = Math.max(maxX, (n.x ?? 0) + n.r);
    minY = Math.min(minY, (n.y ?? 0) - n.r);
    maxY = Math.max(maxY, (n.y ?? 0) + n.r);
  }
  const w = maxX - minX, h = maxY - minY;
  if (w > 0 && h > 0) {
    const k = Math.sqrt((ASPECT * h) / w);
    for (const n of blockNodes) {
      n.x = (n.x ?? 0) * k;
      n.y = (n.y ?? 0) / k;
      n.vx = 0;
      n.vy = 0;
    }
  }
  sim.force("link", null).force("charge", null).force("x", null).force("y", null);
  sim.tick(60);
  lap("arrange");

  // ---- Members onto the map ----------------------------------------------
  centreOf = new Map(blockNodes.map((n) => [n.id, { x: n.x ?? 0, y: n.y ?? 0 }]));
  for (const [id, spot] of local) {
    const c = centreOf.get(regionOf.get(id) as string) as { x: number; y: number };
    graph.setNodeAttribute(id, "x", c.x + spot.x);
    graph.setNodeAttribute(id, "y", c.y + spot.y);
  }
  // Leaves on a spiral round their parent, outside its dust ring.
  const leavesByParent = new Map<string, string[]>();
  for (const [leaf, parent] of leafParent) {
    if (!regionOf.has(leaf)) continue;
    const list = leavesByParent.get(parent);
    if (list) list.push(leaf);
    else leavesByParent.set(parent, [leaf]);
  }
  for (const [parent, leaves] of leavesByParent) {
    leaves.sort(byId);
    const px = graph.getNodeAttribute(parent, "x") as number;
    const py = graph.getNodeAttribute(parent, "y") as number;
    const r0 = bodySize(parent) + ringAround(parent) + 8;
    const base = stableAngle(parent);
    leaves.forEach((leaf, i) => {
      const a = base + i * GOLDEN;
      const r = r0 + 5 * Math.sqrt(i);
      graph.setNodeAttribute(leaf, "x", px + r * Math.cos(a));
      graph.setNodeAttribute(leaf, "y", py + r * Math.sin(a));
    });
  }
  // Memory-only entities on their region's outer band, spread by the
  // golden angle in stable id order so the band fills evenly.
  const ringByRegion = new Map<string, string[]>();
  for (const [id, key] of ringOf) {
    const list = ringByRegion.get(key);
    if (list) list.push(id);
    else ringByRegion.set(key, [id]);
  }
  for (const [key, ids] of ringByRegion) {
    // Strongest evidence innermost; later passes sit outermost.
    ids.sort((a, b) => {
      const ea = evidence.get(a) as { weight: number; pass: number };
      const eb = evidence.get(b) as { weight: number; pass: number };
      return ea.pass - eb.pass || eb.weight - ea.weight || byId(a, b);
    });
    const c = centreOf.get(key) as { x: number; y: number };
    const core = coreRadius.get(key) as number;
    const bands = bandsOf.get(key) as number;
    const perBand = Math.ceil(ids.length / Math.max(1, bands));
    const start = stableAngle(key);
    ids.forEach((id, i) => {
      const band = Math.floor(i / perBand);
      const inBand = Math.min(perBand, ids.length - band * perBand);
      const a = start + ((i % perBand) / inBand) * Math.PI * 2 + band * 0.5 * ((Math.PI * 2) / inBand);
      const r = core + (band + 0.5) * RING_BAND;
      graph.setNodeAttribute(id, "x", c.x + r * Math.cos(a));
      graph.setNodeAttribute(id, "y", c.y + r * Math.sin(a));
    });
  }
  lap("leaves");
  }

  // ---- Attributes for the display ----------------------------------------
  const regions: RegionInfo[] = regionKeys.map((key) => {
    const ids = members.get(key) as string[];
    const core = ids.filter((id) => role.get(id) === "backbone");
    const leaderId = [...(core.length > 0 ? core : ids)].sort(byLeader)[0] as string;
    const c = counts(key);
    return {
      id: key,
      name: label(leaderId),
      leaderId,
      centre: centreOf.get(key) as { x: number; y: number },
      radius: blockRadius.get(key) as number,
      members: ids.length,
      backbone: c.bb,
      leaves: c.lv,
      ring: c.rg,
      superHub: key.startsWith("s:"),
    };
  }).sort((a, b) => b.members - a.members || byId(a.id, b.id));
  for (const [id, key] of regionOf) {
    graph.setNodeAttribute(id, REGION_ATTR, key);
    graph.setNodeAttribute(id, REGION_ROLE_ATTR, superSet.has(id) || regions.some((r) => r.leaderId === id) ? "hub" : role.get(id));
  }
  graph.setAttribute(REGIONS_GRAPH_ATTR, regions);
  classifyRegionEdges(graph, superSet);

  const placed = regionOf.size;
  return {
    counts: {
      bodies: bodies.length,
      backbone: backbone.length,
      leaves: leafParent.size,
      superHubs: superHubs.length,
      ring: ringOf.size,
      inRegions: placed,
      outside: bodies.length - placed,
      memoryOnly: bodies.filter((id) => degree(id) === 0).length,
      ...Object.fromEntries(passAdded.map((n, i) => [`ringPass${i + 1}`, n])),
    },
    resolution: run.resolution,
    modularity: Math.round(run.modularity * 1000) / 1000,
    regions,
    timesMs: times,
    placed,
    islands: bodies.length - placed,
  };
}

/**
 * Give every body-to-body edge its R2 line class. A body's forest line runs
 * to its highest-degree neighbour in the same region (ties by id); a bridge
 * is the one line between two regions' leaders, or failing that the
 * heaviest pair, for a region pair whose summed weight reaches BRIDGE_MIN.
 */
export function classifyRegionEdges(graph: Graph, superHubs: ReadonlySet<string> = new Set()): void {
  const region = (id: string) => graph.getNodeAttribute(id, REGION_ATTR) as string | undefined;
  const isBody = (id: string) => {
    const t = graph.getNodeAttribute(id, "entityType");
    return t !== MEMORY_NODE_TYPE && t !== THING && graph.getNodeAttribute(id, "night") !== true;
  };
  const deg = new Map<string, number>();
  graph.forEachEdge((_k, _a, s, t) => {
    if (s === t || !isBody(s) || !isBody(t)) return;
    deg.set(s, (deg.get(s) ?? 0) + 1);
    deg.set(t, (deg.get(t) ?? 0) + 1);
  });
  const best = new Map<string, string>();
  graph.forEachEdge((_k, _a, s, t) => {
    if (s === t || !isBody(s) || !isBody(t)) return;
    const rs = region(s);
    if (rs === undefined || rs !== region(t) || superHubs.has(s) || superHubs.has(t)) return;
    for (const [a, b] of [[s, t], [t, s]] as const) {
      const cur = best.get(a);
      const db = deg.get(b) ?? 0, dc = cur === undefined ? -1 : deg.get(cur) ?? 0;
      if (cur === undefined || db > dc || (db === dc && b < cur)) best.set(a, b);
    }
  });
  const pairEdges = new Map<string, { key: string; score: number }[]>();
  const pairWeight = new Map<string, number>();
  graph.forEachEdge((key, attrs, s, t) => {
    if (graph.getEdgeAttribute(key, REGION_EDGE_ATTR) !== undefined) graph.removeEdgeAttribute(key, REGION_EDGE_ATTR);
    if (s === t || !isBody(s) || !isBody(t)) return;
    const rs = region(s), rt = region(t);
    let cls: RegionEdgeClass;
    if (superHubs.has(s) || superHubs.has(t)) cls = "super";
    else if (rs !== undefined && rs === rt) cls = best.get(s) === t || best.get(t) === s ? "forest" : "intra";
    else {
      cls = "cross";
      if (rs !== undefined && rt !== undefined) {
        const pair = rs < rt ? `${rs}|${rt}` : `${rt}|${rs}`;
        pairWeight.set(pair, (pairWeight.get(pair) ?? 0) + typeWeight((attrs.edgeType as string) ?? ""));
        const list = pairEdges.get(pair) ?? [];
        list.push({ key, score: (deg.get(s) ?? 0) + (deg.get(t) ?? 0) });
        pairEdges.set(pair, list);
      }
    }
    graph.setEdgeAttribute(key, REGION_EDGE_ATTR, cls);
  });
  for (const [pair, list] of pairEdges) {
    if ((pairWeight.get(pair) ?? 0) < BRIDGE_MIN) continue;
    const pick = list.sort((a, b) => b.score - a.score || byId(a.key, b.key))[0];
    if (pick) graph.setEdgeAttribute(pick.key, REGION_EDGE_ATTR, "bridge");
  }
}

/**
 * Spots for bodies that joined a region since the kept layout: each sits by
 * a kept neighbour in its own region (its busiest one), or failing that by
 * the region's kept middle, a little out and clear of the points already
 * there. Returns only the new bodies; kept ones are not touched.
 */
/**
 * Moves each of `movable` the least it takes for its disc to sit clear of
 * every other drawn body's disc at the fit zoom, where one CSS px is
 * `unitsPerPx` graph units. Everything not in `movable` stays exactly where
 * it is; a point that is already clear is not touched.
 */
export function nudgeClearOfOthers(
  graph: Graph,
  movable: Iterable<string>,
  unitsPerPx: number,
  skip: (id: string) => boolean = () => false,
): string[] {
  if (!(unitsPerPx > 0)) return [];
  const AIR_PX = 2;
  const CELL = 16 * unitsPerPx;
  const radius = (id: string) => ((graph.getNodeAttribute(id, "size") as number | undefined) ?? 0) * unitsPerPx;
  const pos = (id: string) => ({ x: graph.getNodeAttribute(id, "x") as number, y: graph.getNodeAttribute(id, "y") as number });
  const cells = new Map<string, Set<string>>();
  const keyOf = (p: { x: number; y: number }) => `${Math.floor(p.x / CELL)},${Math.floor(p.y / CELL)}`;
  let widest = 0;
  graph.forEachNode((id) => {
    if (skip(id)) return;
    const p = pos(id);
    if (!Number.isFinite(p.x) || !Number.isFinite(p.y)) return;
    widest = Math.max(widest, radius(id));
    const k = keyOf(p);
    const set = cells.get(k) ?? new Set<string>();
    set.add(id);
    cells.set(k, set);
  });
  const clashes = (id: string, p: { x: number; y: number }) => {
    const r = radius(id);
    const span = Math.ceil((r + widest + AIR_PX * unitsPerPx) / CELL);
    const cx = Math.floor(p.x / CELL), cy = Math.floor(p.y / CELL);
    for (let dx = -span; dx <= span; dx += 1) for (let dy = -span; dy <= span; dy += 1) {
      for (const other of cells.get(`${cx + dx},${cy + dy}`) ?? []) {
        if (other === id) continue;
        const q = pos(other);
        if (Math.hypot(q.x - p.x, q.y - p.y) < r + radius(other) + AIR_PX * unitsPerPx) return true;
      }
    }
    return false;
  };
  const moved: string[] = [];
  for (const id of [...movable].filter((id) => graph.hasNode(id) && !skip(id)).sort(byId)) {
    const from = pos(id);
    if (!Number.isFinite(from.x) || !Number.isFinite(from.y) || !clashes(id, from)) continue;
    const start = stableAngle(id);
    let spot = from;
    for (let i = 1; i <= 800; i += 1) {
      const r = 3 * unitsPerPx * Math.sqrt(i);
      const a = start + i * GOLDEN;
      const next = { x: from.x + r * Math.cos(a), y: from.y + r * Math.sin(a) };
      if (!clashes(id, next)) {
        spot = next;
        break;
      }
    }
    if (spot === from) continue;
    cells.get(keyOf(from))?.delete(id);
    graph.setNodeAttribute(id, "x", spot.x);
    graph.setNodeAttribute(id, "y", spot.y);
    const k = keyOf(spot);
    const set = cells.get(k) ?? new Set<string>();
    set.add(id);
    cells.set(k, set);
    moved.push(id);
  }
  return moved;
}

export function placeNewRegionMembers(
  graph: Graph,
  kept: ReadonlyMap<string, { x: number; y: number }>,
  options: { unitsPerPx?: number } = {},
): Map<string, { x: number; y: number }> {
  const out = new Map<string, { x: number; y: number }>();
  const region = (id: string) => graph.getNodeAttribute(id, REGION_ATTR) as string | undefined;
  const sums = new Map<string, { x: number; y: number; n: number }>();
  const CELL = 8;
  const MIN_GAP = 6;
  // Node sizes are CSS px on screen; at the fit zoom one px is `unitsPerPx`
  // graph units, so a new disc keeps its drawn radius, its neighbour's and
  // a px or two of air clear of every point already there.
  const unitsPerPx = options.unitsPerPx ?? 0;
  const radiusOf = (id: string) => ((graph.getNodeAttribute(id, "size") as number | undefined) ?? 0) * unitsPerPx;
  const AIR_PX = 2;
  let widest = 0;
  const cells = new Map<string, { x: number; y: number; r: number }[]>();
  const addSpot = (p: { x: number; y: number }, r: number) => {
    widest = Math.max(widest, r);
    const k = `${Math.floor(p.x / CELL)},${Math.floor(p.y / CELL)}`;
    const list = cells.get(k);
    if (list) list.push({ x: p.x, y: p.y, r });
    else cells.set(k, [{ x: p.x, y: p.y, r }]);
  };
  const clear = (p: { x: number; y: number }, r: number) => {
    const span = Math.max(1, Math.ceil((r + widest + AIR_PX * unitsPerPx + MIN_GAP) / CELL));
    const cx = Math.floor(p.x / CELL), cy = Math.floor(p.y / CELL);
    for (let dx = -span; dx <= span; dx += 1) for (let dy = -span; dy <= span; dy += 1) {
      for (const q of cells.get(`${cx + dx},${cy + dy}`) ?? []) {
        if (Math.hypot(q.x - p.x, q.y - p.y) < Math.max(MIN_GAP, r + q.r + AIR_PX * unitsPerPx)) return false;
      }
    }
    return true;
  };
  for (const [id, p] of kept) {
    if (!graph.hasNode(id)) continue;
    addSpot(p, radiusOf(id));
    const key = region(id);
    if (key === undefined) continue;
    const s = sums.get(key) ?? { x: 0, y: 0, n: 0 };
    s.x += p.x;
    s.y += p.y;
    s.n += 1;
    sums.set(key, s);
  }
  const fresh = graph.nodes().filter((id) => region(id) !== undefined && !kept.has(id)).sort(byId);
  const perAnchor = new Map<string, number>();
  // Bodies tied to a kept neighbour first; a chain of new bodies then
  // follows the ones already placed; the rest go by the region's middle.
  for (let pass = 0; pass < 3; pass += 1) {
    for (const id of fresh) {
      if (out.has(id)) continue;
      const key = region(id) as string;
      let anchor: string | undefined;
      let best = -1;
      for (const n of graph.neighbors(id)) {
        if (region(n) !== key) continue;
        if (!kept.has(n) && !out.has(n)) continue;
        const d = graph.degree(n);
        if (d > best || (d === best && anchor !== undefined && n < anchor)) {
          best = d;
          anchor = n;
        }
      }
      let base: { x: number; y: number } | undefined;
      let anchorKey = `r:${key}`;
      if (anchor !== undefined) {
        base = kept.get(anchor) ?? out.get(anchor);
        anchorKey = anchor;
      } else if (pass === 2) {
        const s = sums.get(key);
        if (s && s.n > 0) base = { x: s.x / s.n, y: s.y / s.n };
      }
      if (!base) continue;
      let i = perAnchor.get(anchorKey) ?? 0;
      const start = stableAngle(id);
      let spot = { x: base.x, y: base.y };
      const own = radiusOf(id);
      for (let tries = 0; tries < 400; tries += 1, i += 1) {
        const r = 12 + 5 * Math.sqrt(i);
        const a = start + i * GOLDEN;
        spot = { x: base.x + r * Math.cos(a), y: base.y + r * Math.sin(a) };
        if (clear(spot, own)) break;
      }
      perAnchor.set(anchorKey, i + 1);
      out.set(id, spot);
      addSpot(spot, own);
    }
  }
  return out;
}
