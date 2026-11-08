// SPDX-License-Identifier: AGPL-3.0-only
/**
 * Last night on the big map (docs/ux/spec.md, UX-GRAPH-5, UX-GRAPH-6): which
 * things a night changed today, what the morning line counts, and the stops
 * Replay makes. Pure, so the line, the replay and their tests read one rule.
 */
import type { NightTidied, ThingNightPoints } from "../tauri";
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

/** What the morning line counts, in the owner's order: points written and
 *  connected, then memories merged, replaced and updated (UX-GRAPH-7), each
 *  only while what stands now is on the map (`has` takes a graph node id),
 *  so another Space's never count. */
export interface NightCounts {
  written: number;
  connected: number;
  merged: number;
  replaced: number;
  updated: number;
}

export function nightCounts(things: readonly ThingNightPoints[], has: (nodeId: string) => boolean): NightCounts {
  const counts: NightCounts = { written: 0, connected: 0, merged: 0, replaced: 0, updated: 0 };
  for (const thing of things) {
    const night = thing.last_night;
    if (!night) continue;
    counts.written += night.written.filter((id) => has(memoryNodeId(id))).length;
    counts.connected += new Set(night.linked.filter((link) => has(memoryNodeId(link.memory_id)) && has(link.entity_id)).map((link) => link.memory_id)).size;
    for (const tidy of night.tidied ?? []) {
      // A replace may keep a memory that is not one of the thing's points:
      // it counts while the thing is on the map.
      const seen = has(memoryNodeId(tidy.memory_id)) || (tidy.kind === "replace" && has(thingNodeId(thing.thing_id)));
      if (!seen) continue;
      if (tidy.kind === "merge") counts.merged += 1;
      else if (tidy.kind === "replace") counts.replaced += 1;
      else counts.updated += 1;
    }
  }
  return counts;
}

/** Most things Replay stops at; past that, the rest are left out of the
 *  motion and only counted on the line. */
export const REPLAY_MAX_THINGS = 5;

/** One tidy Replay shows (UX-GRAPH-7, Fable's design 2026-11-08): the
 *  memories it hid come in as hollow ghosts beside the one that stands now.
 *  `survivor` is that memory's graph node, or null when it is not on the map
 *  (a replace that kept another thing's memory): then the ghost moves by
 *  the thing's centre. `said` is what each ghost's memory said, one per
 *  ghost where known, so the words that went are seen going. */
export interface ReplayTidy {
  readonly kind: NightTidied["kind"];
  readonly survivor: string | null;
  readonly ghosts: number;
  readonly said: readonly string[];
}

/** Most ghosts one tidy shows, however many memories it took in. */
export const REPLAY_MAX_GHOSTS = 3;

/** One thing's part of Replay: its new points grow out of it, what the night
 *  tidied is taken in, then the points it connected fly to sit beside their
 *  topics. Graph node ids. */
export interface ReplayStop {
  readonly thing: string;
  readonly grow: readonly string[];
  readonly tidies: readonly ReplayTidy[];
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
    // What a merge or update wrote is new on the map and grows like a point.
    const tidiedNew = (night.tidied ?? []).filter((tidy) => tidy.kind !== "replace").map((tidy) => tidy.memory_id);
    const grow = [...new Set([...night.written, ...tidiedNew])].map(memoryNodeId).filter(has);
    const links = night.linked.filter((link) => has(memoryNodeId(link.memory_id)) && has(link.entity_id));
    const fly = [...new Set(links.map((link) => memoryNodeId(link.memory_id)))];
    const topics = [...new Set(links.map((link) => link.entity_id))];
    // The same rule as the line: a tidy moves while what stands now is on
    // the map; a replace's kept memory may be off it, the thing is not.
    const tidies: ReplayTidy[] = [];
    for (const tidy of night.tidied ?? []) {
      const survivor = memoryNodeId(tidy.memory_id);
      const drawn = has(survivor);
      if (!drawn && tidy.kind !== "replace") continue;
      const ghosts = Math.max(1, Math.min(tidy.kind === "merge" ? tidy.replaced.length : 1, REPLAY_MAX_GHOSTS));
      tidies.push({ kind: tidy.kind, survivor: drawn ? survivor : null, ghosts, said: (tidy.said ?? []).slice(0, ghosts) });
    }
    if (grow.length === 0 && fly.length === 0 && tidies.length === 0) continue;
    stops.push({ thing: node, grow, tidies, fly, topics });
    if (stops.length === REPLAY_MAX_THINGS) break;
  }
  return stops;
}

/** Replay's beats, ms: Fable's design (2026-11-06), about a third slower after
 *  Qi-Xuan tried it on real data (2026-11-08: a little fast). */
export const REPLAY_TIMING = {
  /** The camera reaching a stop. */
  camera: 600,
  /** One point growing out of its thing's centre. */
  grow: 550,
  /** Between one point starting to grow and the next. */
  stagger: 110,
  /** Most a thing's points take to all start growing, so a long night stays short. */
  staggerMax: 800,
  /** Looking at what grew. */
  holdGrown: 600,
  /** A tidy's ghosts fading in beside what stands now (UX-GRAPH-7). */
  ghostFade: 200,
  /** The ghosts resting with the words that went, long enough to read. */
  ghostRead: 900,
  /** The ghosts moving: a merge's sliding in, a replace's pushed away, an
   *  update's spreading out from the dot. */
  ghostSlide: 600,
  /** What stands now taking them in: one soft swell, during the look after. */
  swell: 150,
  /** Looking at what was tidied. */
  holdTidied: 450,
  /** Points flying out to their topics. */
  fly: 1200,
  /** Looking at where they went. */
  holdFlown: 700,
  /** Points flying home, with the camera going back. */
  home: 600,
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

/** How long one stop's tidying takes until its look, for `count` tidies:
 *  they start one after another like growing points. */
export function tidyMs(count: number): number {
  if (count === 0) return 0;
  return REPLAY_TIMING.ghostFade + REPLAY_TIMING.ghostRead + REPLAY_TIMING.ghostSlide + growStagger(count) * (count - 1);
}

/** The whole replay's length, ms: per stop the camera, the growing and a
 *  look, the tidying and a look, then (with links) the camera again, the
 *  flight and a look; between
 *  stops the points go home; at the end everything does, with the camera. */
export function replayMs(stops: readonly ReplayStop[]): number {
  const t = REPLAY_TIMING;
  let total = 0;
  stops.forEach((stop, index) => {
    if (index > 0) total += t.home;
    total += t.camera;
    if (stop.grow.length > 0) total += growMs(stop.grow.length) + t.holdGrown;
    if (stop.tidies.length > 0) total += tidyMs(stop.tidies.length) + t.holdTidied;
    if (stop.fly.length > 0) total += t.camera + t.fly + t.holdFlown;
  });
  return stops.length > 0 ? total + t.home : 0;
}
