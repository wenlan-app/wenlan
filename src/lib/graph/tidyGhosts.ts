// SPDX-License-Identifier: AGPL-3.0-only
/**
 * Replay's tidy beat (docs/ux/spec.md, UX-GRAPH-7; Fable's design
 * 2026-11-08, with each kind given its own motion at Qi-Xuan's ask the same
 * day): a memory the night hid comes in as a ghost, a hollow ring in the
 * colour of the one that stands now.
 *
 * - merge: the ghosts slide into the dot that stands now, and it takes them
 *   in with one soft swell (taken in);
 * - replace: the ghost comes at the dot, is knocked back off it and
 *   shrinks as it fades; the dot does not swell (pushed out);
 * - update: the old version sits round the dot itself and spreads out from
 *   it as it fades, then the dot swells once (renewed in place).
 *
 * The ghosts are drawn only on the overlay and only while Replay plays, so
 * nothing stays on the map. Pure, so the motion and its tests read one rule.
 */
import { REPLAY_TIMING, type ReplayTidy } from "./lastNight";

/** How strongly a ghost shows: hollow, so it reads "kept out of sight"; near
 *  full strength so it is seen on a dark map (brightened 11-08 at Qi-Xuan's ask). */
export const GHOST_ALPHA = 0.9;
/** A ghost's ring, px. */
export const GHOST_STROKE = 2;
/** How far a merge's ghosts splay from the line out of the thing's centre. */
export const GHOST_SPLAY_DEG = 35;
/** How far out the ghosts rest: two dot-widths. */
export const GHOST_REST_RADII = 4;
/** How much the dot that stands now swells taking a ghost in. */
export const SWELL = 0.25;
/** How far an update's old version spreads out, in dot radii. */
export const RING_RADII = 2.6;
/** How far a replace's ghost comes at the dot before it is knocked back,
 *  and how far back it goes, as shares of its way in. */
export const REACH = 0.6;
export const PUSH = 1.8;
/** The share of a replace's motion spent coming at the dot. */
const COMING = 0.4;
/** An update's old version starts just round the dot, so it is seen. */
const UPDATE_START = 1.35;
/** How small a replace's ghost gets as it is pushed away. */
export const PUSHED_SCALE = 0.4;
/** An update's old version, still with reduced motion: part way out. */
const STILL_SPREAD = 1.7;

export interface Point {
  readonly x: number;
  readonly y: number;
}

/** One tidy playing: from `startAt` (performance.now() time), in the thing
 *  `thing` (its graph node). `still` is the reduced-motion form: the ghosts
 *  rest beside what stands now until Replay is done. */
export interface ReplayGhost {
  readonly tidy: ReplayTidy;
  readonly thing: string;
  readonly startAt: number;
  readonly still: boolean;
}

/** Where a tidy's ghosts rest before they slide, on screen. With what stands
 *  now drawn at `survivor`, they sit on its far side from the thing's centre,
 *  splayed when there are several; with it off the map, beside the centre. */
export function ghostRests(survivor: Point | null, centre: Point | null, radius: number, count: number): Point[] {
  const target = survivor ?? centre;
  if (!target || count <= 0) return [];
  let dx = 0.8;
  let dy = -0.6;
  if (survivor && centre) {
    const length = Math.hypot(survivor.x - centre.x, survivor.y - centre.y);
    if (length > 0.5) {
      dx = (survivor.x - centre.x) / length;
      dy = (survivor.y - centre.y) / length;
    }
  }
  const out = GHOST_REST_RADII * radius;
  const splay = (GHOST_SPLAY_DEG * Math.PI) / 180;
  return Array.from({ length: count }, (_, index) => {
    const angle = count === 1 ? 0 : -splay + (2 * splay * index) / (count - 1);
    const cos = Math.cos(angle);
    const sin = Math.sin(angle);
    return { x: target.x + out * (dx * cos - dy * sin), y: target.y + out * (dx * sin + dy * cos) };
  });
}

/** One moment of a tidy, `elapsed` ms after it starts. `alpha` is the
 *  ghosts' (0 when not shown); `slide` how far along from rest they are
 *  towards what stands now (1 on it, below 0 pushed away from it); `scale`
 *  their size against the dot's; `swell` the scale of the dot that stands
 *  now; and `done` once nothing moves any more. */
export interface GhostPhase {
  readonly alpha: number;
  readonly slide: number;
  readonly scale: number;
  readonly swell: number;
  readonly done: boolean;
}

const easeInOut = (t: number) => (t < 0.5 ? 4 * t ** 3 : 1 - (2 - 2 * t) ** 3 / 2);
const easeOut = (t: number) => 1 - (1 - t) ** 3;

export function ghostPhase(ghost: Pick<ReplayGhost, "tidy" | "still">, elapsed: number): GhostPhase {
  const kind = ghost.tidy.kind;
  // An update's old version is on the dot from the start; the others rest beside it.
  const home = kind === "update" ? 1 : 0;
  if (ghost.still) return { alpha: GHOST_ALPHA, slide: home, scale: kind === "update" ? STILL_SPREAD : 1, swell: 1, done: false };
  const t = REPLAY_TIMING;
  const rest = { slide: home, scale: kind === "update" ? UPDATE_START : 1, swell: 1, done: false };
  if (elapsed < 0) return { ...rest, alpha: 0 };
  if (elapsed < t.ghostFade) return { ...rest, alpha: (GHOST_ALPHA * elapsed) / t.ghostFade };
  const moving = elapsed - t.ghostFade;
  if (moving < t.ghostSlide) {
    const p = moving / t.ghostSlide;
    if (kind === "replace") {
      if (p < COMING) return { alpha: GHOST_ALPHA, slide: REACH * easeInOut(p / COMING), scale: 1, swell: 1, done: false };
      const q = (p - COMING) / (1 - COMING);
      const out = easeOut(q);
      return { alpha: GHOST_ALPHA * (1 - q), slide: REACH - (REACH + PUSH) * out, scale: 1 - (1 - PUSHED_SCALE) * out, swell: 1, done: false };
    }
    if (kind === "update") {
      const out = easeOut(p);
      return { alpha: GHOST_ALPHA * (1 - out), slide: 1, scale: UPDATE_START + (RING_RADII - UPDATE_START) * out, swell: 1, done: false };
    }
    return { alpha: GHOST_ALPHA, slide: easeInOut(p), scale: 1, swell: 1, done: false };
  }
  // A merge's ghosts are in, an update's old version has spread out: what
  // stands now swells once. A replace pushes its ghost away and stays as it
  // is, and into the thing's centre (nothing standing on the map) nothing swells.
  const after = moving - t.ghostSlide;
  const swellMs = kind !== "replace" && ghost.tidy.survivor !== null ? t.swell : 0;
  const end = { alpha: 0, slide: kind === "replace" ? -PUSH : 1, scale: kind === "update" ? RING_RADII : kind === "replace" ? PUSHED_SCALE : 1 };
  if (after < swellMs) return { ...end, swell: 1 + SWELL * Math.sin((Math.PI * after) / swellMs), done: false };
  return { ...end, swell: 1, done: true };
}

/** The scale of `node` at `now`: each tidy taken into it swells it once. */
export function swellOf(ghosts: readonly ReplayGhost[], node: string, now: number): number {
  let scale = 1;
  for (const ghost of ghosts) {
    if (ghost.tidy.survivor === node) scale *= ghostPhase(ghost, now - ghost.startAt).swell;
  }
  return scale;
}

/** Whether any of these tidies still moves at `now`, so the map keeps painting. */
export function ghostsMoving(ghosts: readonly ReplayGhost[], now: number): boolean {
  return ghosts.some((ghost) => !ghost.still && !ghostPhase(ghost, now - ghost.startAt).done);
}

/** A ghost's place on screen, part way from its rest to what it slides into
 *  (`slide` below 0: pushed the other way, away from it). */
export function ghostAt(rest: Point, target: Point, slide: number): Point {
  return { x: rest.x + (target.x - rest.x) * slide, y: rest.y + (target.y - rest.y) * slide };
}
