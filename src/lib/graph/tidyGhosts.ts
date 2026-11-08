// SPDX-License-Identifier: AGPL-3.0-only
/**
 * Replay's tidy beat (docs/ux/spec.md, UX-GRAPH-7; Fable's design
 * 2026-11-08): a memory the night hid comes in as a ghost, a hollow ring in
 * the colour of the one that stands now, slides into it, and that dot takes
 * it in with one soft swell; an update ends with one ring widening out. The
 * ghosts are drawn only on the overlay and only while Replay plays, so
 * nothing stays on the map. Pure, so the motion and its tests read one rule.
 */
import { REPLAY_TIMING, type ReplayTidy } from "./lastNight";

/** How strongly a ghost shows: hollow and faint reads "kept out of sight". */
export const GHOST_ALPHA = 0.55;
/** A ghost's ring, px. */
export const GHOST_STROKE = 1.5;
/** How far a merge's ghosts splay from the line out of the thing's centre. */
export const GHOST_SPLAY_DEG = 35;
/** How far out the ghosts rest: two dot-widths. */
export const GHOST_REST_RADII = 4;
/** How much the dot that stands now swells taking a ghost in. */
export const SWELL = 0.25;
/** How far an update's ring widens, in dot radii. */
export const RING_RADII = 2.6;

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
 *  ghosts' (0 when not shown), `slide` how far along they are (0 at rest, 1
 *  in), `swell` the scale of the dot that stands now, `ring` an update's ring
 *  (0 to 1) or null, and `done` once nothing moves any more. */
export interface GhostPhase {
  readonly alpha: number;
  readonly slide: number;
  readonly swell: number;
  readonly ring: number | null;
  readonly done: boolean;
}

const easeInOut = (t: number) => (t < 0.5 ? 4 * t ** 3 : 1 - (2 - 2 * t) ** 3 / 2);

export function ghostPhase(ghost: Pick<ReplayGhost, "tidy" | "still">, elapsed: number): GhostPhase {
  if (ghost.still) return { alpha: GHOST_ALPHA, slide: 0, swell: 1, ring: null, done: false };
  const t = REPLAY_TIMING;
  if (elapsed < 0) return { alpha: 0, slide: 0, swell: 1, ring: null, done: false };
  if (elapsed < t.ghostFade) return { alpha: (GHOST_ALPHA * elapsed) / t.ghostFade, slide: 0, swell: 1, ring: null, done: false };
  const sliding = elapsed - t.ghostFade;
  if (sliding < t.ghostSlide) return { alpha: GHOST_ALPHA, slide: easeInOut(sliding / t.ghostSlide), swell: 1, ring: null, done: false };
  // In: the ghost is gone, under the dot it slid into. Into the thing's
  // centre (nothing standing on the map) there is no swell.
  const landed = sliding - t.ghostSlide;
  const swells = ghost.tidy.survivor !== null;
  const swellMs = swells ? t.swell : 0;
  if (landed < swellMs) return { alpha: 0, slide: 1, swell: 1 + SWELL * Math.sin((Math.PI * landed) / swellMs), ring: null, done: false };
  const ringing = landed - swellMs;
  if (ghost.tidy.kind === "update" && swells && ringing < t.ring) return { alpha: 0, slide: 1, swell: 1, ring: ringing / t.ring, done: false };
  return { alpha: 0, slide: 1, swell: 1, ring: null, done: true };
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

/** A ghost's place on screen, part way from its rest to what it slides into. */
export function ghostAt(rest: Point, target: Point, slide: number): Point {
  return { x: rest.x + (target.x - rest.x) * slide, y: rest.y + (target.y - rest.y) * slide };
}
