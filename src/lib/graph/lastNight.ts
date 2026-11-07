// SPDX-License-Identifier: AGPL-3.0-only
/**
 * Last night on the big map (docs/ux/spec.md, UX-GRAPH-5, UX-GRAPH-6): which
 * things a night changed today, what the morning line counts, and the stops
 * Replay makes. Pure, so the line, the replay and their tests read one rule.
 */
import type { ThingNightPoints } from "../tauri";
import { memoryNodeId, thingNodeId } from "./model";

/** Steeping before this local hour happened overnight; later, it was today. */
export const NIGHT_ENDS_HOUR = 7;

/** Whether a night's steeping (unix seconds) happened today, local time:
 *  then the app says "last night" and marks what it steeped as new. */
export function steepedToday(atSeconds: number, now = new Date()): boolean {
  const at = new Date(atSeconds * 1000);
  return at.getFullYear() === now.getFullYear() && at.getMonth() === now.getMonth() && at.getDate() === now.getDate();
}

/** Local midnight today, unix seconds: what "the whole night" is taken back from. */
export function todayStartSeconds(now = new Date()): number {
  return Math.floor(new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime() / 1000);
}

/** The things whose newest night ran today, in `space` (all when null). */
export function nightsToday(things: readonly ThingNightPoints[], space: string | null, now = new Date()): ThingNightPoints[] {
  return things.filter((thing) => thing.last_night && steepedToday(thing.last_night.at, now) && (!space || thing.spaces.includes(space)));
}

/** Whether every one of these nights (unix seconds) ran before morning, so
 *  the line says "last night" rather than "today". */
export function ranOvernight(ats: readonly number[]): boolean {
  return ats.every((at) => new Date(at * 1000).getHours() < NIGHT_ENDS_HOUR);
}

/** What the morning line counts: points written and points connected that
 *  are on the map now (`has` takes a graph node id), so another Space's never
 *  count. */
export function nightCounts(things: readonly ThingNightPoints[], has: (nodeId: string) => boolean): { written: number; connected: number } {
  let written = 0;
  let connected = 0;
  for (const thing of things) {
    const night = thing.last_night;
    if (!night) continue;
    written += night.written.filter((id) => has(memoryNodeId(id))).length;
    connected += new Set(night.linked.filter((link) => has(memoryNodeId(link.memory_id)) && has(link.entity_id)).map((link) => link.memory_id)).size;
  }
  return { written, connected };
}

/** Most things Replay stops at; past that, the rest are left out of the
 *  motion and only counted on the line. */
export const REPLAY_MAX_THINGS = 5;

/** One thing's part of Replay: its new points grow out of it, then the
 *  points it connected fly to sit beside their topics. Graph node ids. */
export interface ReplayStop {
  readonly thing: string;
  readonly grow: readonly string[];
  readonly fly: readonly string[];
  readonly topics: readonly string[];
}

/** The stops Replay makes, in the order the things are listed (newest
 *  activity first), each holding only what is on the map now. A thing with
 *  nothing left on the map is no stop. */
export function replayStops(things: readonly ThingNightPoints[], has: (nodeId: string) => boolean): ReplayStop[] {
  const stops: ReplayStop[] = [];
  for (const thing of things) {
    const night = thing.last_night;
    const node = thingNodeId(thing.thing_id);
    if (!night || !has(node)) continue;
    const grow = night.written.map(memoryNodeId).filter(has);
    const links = night.linked.filter((link) => has(memoryNodeId(link.memory_id)) && has(link.entity_id));
    const fly = [...new Set(links.map((link) => memoryNodeId(link.memory_id)))];
    const topics = [...new Set(links.map((link) => link.entity_id))];
    if (grow.length === 0 && fly.length === 0) continue;
    stops.push({ thing: node, grow, fly, topics });
    if (stops.length === REPLAY_MAX_THINGS) break;
  }
  return stops;
}

/** Replay's beats, ms (Fable's design, 2026-11-06). */
export const REPLAY_TIMING = {
  /** The camera reaching a stop. */
  camera: 450,
  /** One point growing out of its thing's centre. */
  grow: 400,
  /** Between one point starting to grow and the next. */
  stagger: 80,
  /** Most a thing's points take to all start growing, so a long night stays short. */
  staggerMax: 600,
  /** Looking at what grew. */
  holdGrown: 400,
  /** Points flying out to their topics. */
  fly: 900,
  /** Looking at where they went. */
  holdFlown: 500,
  /** Points flying home, with the camera going back. */
  home: 450,
} as const;

/** The gap between points starting to grow, for `count` of them. */
export function growStagger(count: number): number {
  if (count <= 1) return 0;
  return Math.min(REPLAY_TIMING.stagger, REPLAY_TIMING.staggerMax / (count - 1));
}

/** How long one stop's growing takes, from the camera arriving. */
export function growMs(count: number): number {
  if (count === 0) return 0;
  return REPLAY_TIMING.grow + growStagger(count) * (count - 1);
}

/** The whole replay's length, ms: per stop the camera, the growing and a
 *  look, then (with links) the camera again, the flight and a look; between
 *  stops the points go home; at the end everything does, with the camera. */
export function replayMs(stops: readonly ReplayStop[]): number {
  const t = REPLAY_TIMING;
  let total = 0;
  stops.forEach((stop, index) => {
    if (index > 0) total += t.home;
    total += t.camera;
    if (stop.grow.length > 0) total += growMs(stop.grow.length) + t.holdGrown;
    if (stop.fly.length > 0) total += t.camera + t.fly + t.holdFlown;
  });
  return stops.length > 0 ? total + t.home : 0;
}
