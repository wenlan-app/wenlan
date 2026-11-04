// SPDX-License-Identifier: AGPL-3.0-only
import type Graph from "graphology";
import { satelliteAnchor, type HoverState } from "./atlas";
import { MEMORY_NODE_TYPE, THING_NODE_TYPE } from "./model";
import type { GraphPalette } from "./palette";

/**
 * What sigma's round discs cannot draw, painted on AtlasView's overlay canvas
 * above the map like the dust counts (UX-CHAT-5): each thing as a frame, a
 * rounded dashed outline round the cluster of points its nights wrote with
 * the thing's title above it, a small moon beside each of those points, the
 * same crescent the thing's page marks them with, and, close in, each
 * point's sentence beside it. The thing itself is no node on the map; its
 * hidden centre only holds the cluster in place.
 */
export interface ThingFrame {
  /** The thing's node id (`thing:<id>`). */
  thing: string;
  title: string;
  /** Viewport CSS px. */
  left: number;
  top: number;
  right: number;
  bottom: number;
  /** Another node is hovered and nothing in this thing is in its reach. */
  dim: boolean;
}

export interface NightMoon {
  /** The point's centre and radius, viewport CSS px. */
  x: number;
  y: number;
  r: number;
  dim: boolean;
}

/** A point whose sentence is shown beside it. */
export interface NightSentence extends NightMoon {
  id: string;
  text: string;
  /** Still open (not decided): written in the muted ink. */
  open: boolean;
}

/** Where a node is drawn this frame, in viewport CSS px, or null when sigma
 *  does not draw it (hidden by a reducer, such as a filtered type). */
export type DrawnAt = (id: string) => { x: number; y: number; r: number } | null;

/** Room between a point's disc and the frame, CSS px. */
const FRAME_PAD = 8;
/** Extra viewport margin inside which a mark is still drawn. */
const CULL_PAD = 24;

/** The frames and moons to paint this frame. */
export function nightMarks(
  graph: Graph,
  drawnAt: DrawnAt,
  hover: HoverState,
  viewport: { width: number; height: number },
  /** Whether this thing's points show their sentences (see AtlasView: the
   *  thing's own zoom stop, or zoomed in past the overview). */
  sentencesFor: (thing: string) => boolean = () => false,
): { frames: ThingFrame[]; moons: NightMoon[]; sentences: NightSentence[] } {
  const frames: ThingFrame[] = [];
  const moons: NightMoon[] = [];
  const sentences: NightSentence[] = [];
  const lit = (id: string) => hover.hovered === null || hover.hovered === id || hover.neighbors.has(id);
  graph.forEachNode((thing, attrs) => {
    if (attrs.entityType !== THING_NODE_TYPE) return;
    let left = Infinity;
    let top = Infinity;
    let right = -Infinity;
    let bottom = -Infinity;
    let anyLit = lit(thing);
    const points: NightSentence[] = [];
    graph.forEachNeighbor(thing, (point, pointAttrs) => {
      if (pointAttrs.night !== true || pointAttrs.dustOf !== thing) return;
      const at = drawnAt(point);
      if (!at) return;
      left = Math.min(left, at.x - at.r);
      right = Math.max(right, at.x + at.r);
      top = Math.min(top, at.y - at.r);
      bottom = Math.max(bottom, at.y + at.r);
      if (lit(point)) anyLit = true;
      points.push({ ...at, dim: !lit(point), id: point, text: String(pointAttrs.label ?? ""), open: pointAttrs.open === true });
    });
    if (points.length === 0) return;
    const frame = {
      thing,
      title: String(attrs.label ?? ""),
      left: left - FRAME_PAD,
      top: top - FRAME_PAD,
      right: right + FRAME_PAD,
      bottom: bottom + FRAME_PAD,
      dim: !anyLit,
    };
    if (frame.right < -CULL_PAD || frame.bottom < -CULL_PAD || frame.left > viewport.width + CULL_PAD || frame.top > viewport.height + CULL_PAD) return;
    frames.push(frame);
    moons.push(...points.map(({ x, y, r, dim }) => ({ x, y, r, dim })));
    if (sentencesFor(thing)) sentences.push(...points.filter((point) => point.text));
  });
  return { frames, moons, sentences };
}

/** The crescent of ThingSteeped's SteepMoon, on its 16-unit box. */
const MOON_PATH = "M10.5 1.5a6.5 6.5 0 1 0 4 11.6A6 6 0 0 1 10.5 1.5z";
/** The moon's drawn size, CSS px. */
const MOON_PX = 7;
const FRAME_RADIUS = 10;
const TITLE_FONT = '500 12px "Instrument Sans", -apple-system, sans-serif';
const TITLE_MAX_PX = 240;

const SENTENCE_FONT = '12px "Instrument Sans", -apple-system, sans-serif';
/** A sentence is never longer than this on screen; past it, an ellipsis. */
export const SENTENCE_MAX_PX = 240;
/** A sentence with less room than this beside its point is left out. */
const SENTENCE_MIN_PX = 56;
/** Gap between a point's disc and its sentence, CSS px. */
const SENTENCE_GAP = 6;
/** One line of sentence, CSS px, with its halo. */
const SENTENCE_LINE = 16;

type Measure = (text: string) => number;

/** `text` cut with an ellipsis to fit `max` px. */
function fitWith(measure: Measure, text: string, max: number): string {
  if (measure(text) <= max) return text;
  let cut = text;
  while (cut.length > 1 && measure(`${cut}…`) > max) cut = cut.slice(0, -1);
  return `${cut.trimEnd()}…`;
}

const fit = (ctx: CanvasRenderingContext2D, text: string, max: number) => fitWith((t) => ctx.measureText(t).width, text, max);

export type Box = { left: number; right: number; top: number; bottom: number };

const overlapping = (a: Box, b: Box) => a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom;

/** A sentence as it will be painted: its text (cut to the room it has), the
 *  x its text starts or ends at, and its vertical middle. */
export interface PlacedSentence {
  id: string;
  text: string;
  x: number;
  y: number;
  align: "left" | "right";
  open: boolean;
  dim: boolean;
  box: Box;
}

/**
 * Where each shown sentence goes (UX-CHAT-5): on the line of its point, to
 * the right of it, cut short with an ellipsis where the next point or an
 * already placed word would be; to the left instead when that shows more of
 * it; left out when neither side has room, rather than printed over
 * something. At most SENTENCE_MAX_PX long, and
 * never past the viewport's edge when one is given. Points are taken top to
 * bottom, so the result does not depend on graph order. `occupied` (names
 * already painted) is read and extended.
 */
export function layoutNightSentences(
  sentences: readonly NightSentence[],
  measure: Measure,
  occupied: Box[] = [],
  viewport?: { width: number },
): PlacedSentence[] {
  const ordered = [...sentences].sort((a, b) => a.y - b.y || a.x - b.x || (a.id < b.id ? -1 : 1));
  const discs = sentences.map((point) => ({ id: point.id, left: point.x - point.r, right: point.x + point.r, top: point.y - point.r, bottom: point.y + point.r }));
  const placed: PlacedSentence[] = [];
  for (const point of ordered) {
    const top = point.y - SENTENCE_LINE / 2;
    const bottom = point.y + SENTENCE_LINE / 2;
    const inLine = (box: Box) => box.top < bottom && top < box.bottom;
    const blockers: Box[] = [...discs.filter((disc) => disc.id !== point.id), ...occupied].filter(inLine);
    const trySide = (align: "left" | "right"): PlacedSentence | null => {
      const start = align === "left" ? point.x + point.r + SENTENCE_GAP : point.x - point.r - SENTENCE_GAP;
      let room = SENTENCE_MAX_PX;
      if (viewport) room = Math.min(room, align === "left" ? viewport.width - SENTENCE_GAP - start : start - SENTENCE_GAP);
      for (const box of blockers) {
        const distance = align === "left" ? box.left - start : start - box.right;
        // Something already across the start of the line: no room at all.
        if (distance < 0 && (align === "left" ? box.right > start : box.left < start)) return null;
        if (distance >= 0) room = Math.min(room, distance - SENTENCE_GAP);
      }
      if (room < SENTENCE_MIN_PX) return null;
      const text = fitWith(measure, point.text, room);
      const width = measure(text);
      const box = align === "left"
        ? { left: start - 2, right: start + width + 2, top, bottom }
        : { left: start - width - 2, right: start + 2, top, bottom };
      if (occupied.some((other) => overlapping(other, box))) return null;
      return { id: point.id, text, x: start, y: point.y, align, open: point.open, dim: point.dim, box };
    };
    // `align` is the text's anchor, so "left" writes to the right of the
    // point. That side first; the other only when it shows more of the
    // sentence (the right is cut short or taken).
    const after = trySide("left");
    const before = after && after.text === point.text ? null : trySide("right");
    const spot = before && (!after || before.text.length > after.text.length) ? before : after;
    if (!spot) continue;
    occupied.push(spot.box);
    placed.push(spot);
  }
  return placed;
}

/** The part of the overlay a click on a frame answers to: the frame itself
 *  and its title. */
export interface FrameHit extends Box {
  thing: string;
}

/** The thing whose frame or title is under (x, y), the smallest when frames
 *  overlap, else null. */
export function frameAt(hits: readonly FrameHit[], x: number, y: number): string | null {
  let best: FrameHit | null = null;
  for (const hit of hits) {
    if (x < hit.left || x > hit.right || y < hit.top || y > hit.bottom) continue;
    const area = (hit.right - hit.left) * (hit.bottom - hit.top);
    if (!best || area < (best.right - best.left) * (best.bottom - best.top)) best = hit;
  }
  return best?.thing ?? null;
}

/**
 * Paint the frames, their titles, the moons and the shown sentences. A title
 * goes above its frame, else below it when that spot is taken by a name
 * already painted (`occupied`, shared with the node labels) or another
 * thing's title, else it is left out for that frame rather than printed over
 * another word. Sentences follow layoutNightSentences; a settled point's
 * sentence is in the label ink, an open one's in the muted ink. Returns where
 * a click picks each thing (frameAt).
 */
export function drawNightMarks(
  ctx: CanvasRenderingContext2D,
  marks: { frames: readonly ThingFrame[]; moons: readonly NightMoon[]; sentences?: readonly NightSentence[] },
  palette: GraphPalette,
  occupied: Box[] = [],
): FrameHit[] {
  const hits: FrameHit[] = [];
  if (marks.frames.length === 0) return hits;
  ctx.save();
  ctx.lineWidth = 1.25;
  ctx.setLineDash([5, 4]);
  ctx.strokeStyle = palette.bridge;
  for (const frame of marks.frames) {
    ctx.globalAlpha = frame.dim ? 0.2 : 0.8;
    const width = frame.right - frame.left;
    const height = frame.bottom - frame.top;
    ctx.beginPath();
    if (typeof ctx.roundRect === "function") ctx.roundRect(frame.left, frame.top, width, height, FRAME_RADIUS);
    else ctx.rect(frame.left, frame.top, width, height);
    ctx.stroke();
    hits.push({ thing: frame.thing, left: frame.left, top: frame.top, right: frame.right, bottom: frame.bottom });
  }
  ctx.setLineDash([]);
  ctx.font = TITLE_FONT;
  ctx.textAlign = "left";
  ctx.textBaseline = "bottom";
  ctx.lineJoin = "round";
  ctx.lineWidth = 3;
  ctx.strokeStyle = palette.surface;
  ctx.fillStyle = palette.label;
  for (const frame of marks.frames) {
    if (!frame.title) continue;
    ctx.globalAlpha = frame.dim ? 0.3 : 1;
    const title = fit(ctx, frame.title, Math.max(TITLE_MAX_PX, frame.right - frame.left));
    const width = ctx.measureText(title).width;
    const x = frame.left + 2;
    const spot = [frame.top - 3, frame.bottom + 17]
      .map((baseline) => ({ baseline, box: { left: x - 3, right: x + width + 3, top: baseline - 15, bottom: baseline + 2 } }))
      .find(({ box }) => !occupied.some((other) => other.left < box.right && box.left < other.right && other.top < box.bottom && box.top < other.bottom));
    if (!spot) continue;
    occupied.push(spot.box);
    hits.push({ thing: frame.thing, ...spot.box });
    ctx.strokeText(title, x, spot.baseline);
    ctx.fillText(title, x, spot.baseline);
  }
  if (marks.sentences && marks.sentences.length > 0) {
    ctx.font = SENTENCE_FONT;
    ctx.textBaseline = "middle";
    const width = ctx.canvas.width / (typeof window === "undefined" ? 1 : window.devicePixelRatio || 1);
    const placed = layoutNightSentences(marks.sentences, (text) => ctx.measureText(text).width, occupied, { width });
    for (const sentence of placed) {
      ctx.globalAlpha = sentence.dim ? 0.3 : 1;
      ctx.textAlign = sentence.align;
      ctx.fillStyle = sentence.open ? palette.labelMuted : palette.label;
      ctx.strokeText(sentence.text, sentence.x, sentence.y);
      ctx.fillText(sentence.text, sentence.x, sentence.y);
    }
    ctx.textAlign = "left";
  }
  const moon = typeof Path2D === "function" ? new Path2D(MOON_PATH) : null;
  if (moon) {
    ctx.fillStyle = palette.bridge;
    for (const point of marks.moons) {
      ctx.save();
      // Up and to the right of the dot, clear of it.
      ctx.translate(point.x + point.r * 0.7 + 1, point.y - point.r * 0.7 - MOON_PX - 1);
      ctx.scale(MOON_PX / 16, MOON_PX / 16);
      ctx.globalAlpha = point.dim ? 0.2 : 0.85;
      ctx.fill(moon);
      ctx.restore();
    }
  }
  ctx.restore();
  return hits;
}

/** The three stops of a thing's zoom (UX-CHAT-5): its own points, its
 *  neighbourhood on the map, the whole map. */
export type ThingStop = "this" | "nearby" | "all";
export const THING_STOPS: readonly ThingStop[] = ["this", "nearby", "all"];

/** A thing's points on the map, in id order. */
export function thingPointIds(graph: Graph, thing: string): string[] {
  if (!graph.hasNode(thing)) return [];
  return graph.filterNeighbors(thing, (_id, attrs) => attrs.night === true && (attrs.dustOf === undefined || attrs.dustOf === thing)).sort();
}

/** What a thing's points link to on the map: pages and entities. */
function linkedBodies(graph: Graph, points: readonly string[]): string[] {
  const linked = new Set<string>();
  for (const point of points) {
    graph.forEachNeighbor(point, (other, attrs) => {
      if (attrs.entityType !== MEMORY_NODE_TYPE && attrs.entityType !== THING_NODE_TYPE) linked.add(other);
    });
  }
  return [...linked].sort();
}

/**
 * The nodes a stop frames. 這件事 (this): the thing's points. 附近 (nearby):
 * those, what they link to, and each of those bodies' own neighbours (pages
 * and entities, not their memory dust). 全部 (all): every node.
 */
export function thingStopNodeIds(graph: Graph, thing: string, stop: ThingStop): string[] {
  if (stop === "all") return graph.nodes();
  const points = thingPointIds(graph, thing);
  if (stop === "this") return points;
  const linked = linkedBodies(graph, points);
  const near = new Set([...points, ...linked]);
  for (const body of linked) {
    graph.forEachNeighbor(body, (other, attrs) => {
      if (attrs.entityType !== MEMORY_NODE_TYPE && attrs.entityType !== THING_NODE_TYPE) near.add(other);
    });
  }
  return [...near].sort();
}

/** The lit set while a thing is selected: its points and what they link to,
 *  plus the neighbourhood at 附近. Like a hover on the thing, but its lines
 *  to its own centre stay hidden and the lines inside the lit set show. */
export function thingFocusState(graph: Graph, thing: string, stop: ThingStop): HoverState {
  const points = thingPointIds(graph, thing);
  const lit = stop === "nearby" ? thingStopNodeIds(graph, thing, stop) : [...points, ...linkedBodies(graph, points)];
  return { hovered: thing, neighbors: new Set(lit), thing };
}

/** How long a new point takes to grow out of its thing's centre, ms. */
export const GROWTH_MS = 400;

/** How far grown a new point is `elapsed` ms after it started: 0 at the
 *  centre, 1 at its own spot, easing out so it settles rather than stops. */
export function growthAt(elapsed: number, duration = GROWTH_MS): number {
  if (!(elapsed > 0)) return 0;
  if (elapsed >= duration) return 1;
  const t = elapsed / duration;
  return 1 - (1 - t) ** 3;
}

/** The night points that are new since the map was last opened: on the map
 *  now and not seen before. With no record at all (the very first open)
 *  nothing is new, so a first look never starts with everything growing. */
export function newNightPoints(current: Iterable<string>, seen: ReadonlySet<string> | null): string[] {
  if (seen === null) return [];
  return [...current].filter((id) => !seen.has(id)).sort();
}

/** The night points on the map now. */
export function nightPointIds(graph: Graph): string[] {
  return graph.filterNodes((_id, attrs) => attrs.night === true).sort();
}

/** The key a point's kept spot is filed under: its thing and itself, so a
 *  point that moves to another thing (a merge) is placed afresh there. */
export const nightOffsetKey = (thing: string, point: string) => `${thing}>${point}`;

/**
 * Hand each night point its kept spot round its thing's centre (Slice B,
 * fixed positions), as `nightOffset`, before the layout runs; atlas.ts's
 * thingCluster puts it exactly there and places only points with none.
 */
export function applyNightOffsets(graph: Graph, kept: ReadonlyMap<string, { x: number; y: number }>): void {
  if (kept.size === 0) return;
  graph.forEachNode((id, attrs) => {
    if (attrs.night !== true) return;
    const thing = satelliteAnchor(graph, id);
    if (thing === undefined || graph.getNodeAttribute(thing, "entityType") !== THING_NODE_TYPE) return;
    const offset = kept.get(nightOffsetKey(thing, id));
    if (offset) graph.setNodeAttribute(id, "nightOffset", [offset.x, offset.y]);
  });
}

/** Every night point's spot round its thing's centre as laid out now, to be
 *  kept for the next open. */
export function nightOffsetsOf(graph: Graph): Map<string, { x: number; y: number }> {
  const offsets = new Map<string, { x: number; y: number }>();
  graph.forEachNode((id, attrs) => {
    if (attrs.night !== true) return;
    const thing = attrs.dustOf as string | undefined;
    if (!thing || !graph.hasNode(thing) || graph.getNodeAttribute(thing, "entityType") !== THING_NODE_TYPE) return;
    offsets.set(nightOffsetKey(thing, id), {
      x: (attrs.x as number) - (graph.getNodeAttribute(thing, "x") as number),
      y: (attrs.y as number) - (graph.getNodeAttribute(thing, "y") as number),
    });
  });
  return offsets;
}
