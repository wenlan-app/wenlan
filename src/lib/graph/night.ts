// SPDX-License-Identifier: AGPL-3.0-only
import type Graph from "graphology";
import { satelliteAnchor, type HoverState } from "./atlas";
import { convexHull, traceRoundedArea } from "./cartography";
import { MEMORY_NODE_TYPE, THING_NODE_TYPE } from "./model";
import type { GraphPalette } from "./palette";

/**
 * What sigma's round discs cannot draw, painted on AtlasView's overlay canvas
 * above the map like the dust counts (UX-CHAT-5): the picked thing as a
 * faint halo round the cluster of points its nights wrote, with the thing's
 * title beside it, one small moon after the title while a night has steeped
 * it in the last day (the same crescent the thing's page uses; one per
 * thing, never one per point), and, close in, each point's sentence beside
 * it. The thing itself is no node on the map; its hidden centre only holds
 * the cluster in place.
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
  /** A night wrote one of its points in the last day: a moon after the title. */
  steeping?: boolean;
  /** Its points' centres and radii, viewport CSS px: the halo is grown round
   *  these (and its sentences), so it takes the shape of what it holds. */
  spots?: readonly { x: number; y: number; r: number }[];
}

/** Where a point is drawn this frame. */
export interface NightSpot {
  /** The point's centre and radius, viewport CSS px. */
  x: number;
  y: number;
  r: number;
  dim: boolean;
}

/** A point whose sentence is shown beside it. */
export interface NightSentence extends NightSpot {
  id: string;
  text: string;
  /** Still open (not decided): written in the muted ink. */
  open: boolean;
  /** Its thing's node id: the frame it belongs inside. */
  thing: string;
  /** Out beside its topic while its thing is picked (atlas.ts's
   *  setThingFlight): its sentence is not wrapped into the thing's frame. */
  away?: boolean;
}

/**
 * A sign held at the screen's edge for what the picked thing reaches that is
 * off screen (Fable's design, Qi-Xuan 11-05): its points that flew out to a
 * far topic, one sign per topic ("↖ memory UX · 2" over the first point's
 * sentence), or, once the camera has gone to such a topic, the thing itself
 * ("back"). A tap moves the camera to `goTo`.
 */
export interface EdgeSign {
  kind: "topic" | "back";
  /** The picked thing's node id. */
  thing: string;
  /** Where a tap takes the camera: the topic, or the thing. */
  goTo: string;
  /** The topic's name, or the thing's title. */
  title: string;
  /** Points of the thing out by that topic (0 for "back"). */
  count: number;
  /** The first of those points' sentences ("" for "back"). */
  text: string;
  open: boolean;
  /** Where `goTo` is drawn, viewport CSS px: the sign points that way. */
  toward: { x: number; y: number };
}

/** Where a node is drawn this frame, in viewport CSS px, or null when sigma
 *  does not draw it (hidden by a reducer, such as a filtered type). */
export type DrawnAt = (id: string) => { x: number; y: number; r: number } | null;

/** Room between a point's disc (or a sentence) and the frame, CSS px. */
const FRAME_PAD = 8;
/** The smallest a frame is drawn, CSS px: a cluster seen from far off is
 *  still plainly a frame, titled or not. */
const MIN_FRAME_WIDTH = 30;
const MIN_FRAME_HEIGHT = 22;
/** Extra viewport margin inside which a mark is still drawn. */
const CULL_PAD = 24;

/** How recent a night's point keeps its thing's moon up, seconds. */
export const STEEPING_SECS = 24 * 60 * 60;

/** The frames and sentences to paint this frame. */
export function nightMarks(
  graph: Graph,
  drawnAt: DrawnAt,
  hover: HoverState,
  viewport: { width: number; height: number },
  /** Whether a thing's point shows its whole sentence (see AtlasView: under
   *  the pointer, pinned by a click, or at its thing's own zoom stop). */
  sentencesFor: (thing: string, point: string) => boolean = () => false,
  /** The picked thing. Only its frame and points are drawn; with nothing
   *  picked the map shows none (its points are hidden too, AtlasView). */
  picked: string | null = null,
  /** Now, Unix seconds (points' createdAt is too). */
  now: number = Date.now() / 1000,
  /** The map's "Working on" switch: every active thing's frame is drawn,
   *  not only the picked one's. */
  showAll = false,
  /** Where a thing's centre is drawn, viewport CSS px: the frame's spot
   *  when every one of its points is out beside its topic. */
  centreAt: (thing: string) => { x: number; y: number } | null = () => null,
): { frames: ThingFrame[]; sentences: NightSentence[]; edges: EdgeSign[] } {
  const frames: ThingFrame[] = [];
  const sentences: NightSentence[] = [];
  const edges: EdgeSign[] = [];
  const onScreen = (at: { x: number; y: number }) => at.x >= 0 && at.y >= 0 && at.x <= viewport.width && at.y <= viewport.height;
  const lit = (id: string) => hover.hovered === null || hover.hovered === id || hover.neighbors.has(id);
  graph.forEachNode((thing, attrs) => {
    if (attrs.entityType !== THING_NODE_TYPE) return;
    if (thing !== picked && (!showAll || attrs.quiet === true)) return;
    let left = Infinity;
    let top = Infinity;
    let right = -Infinity;
    let bottom = -Infinity;
    let anyLit = lit(thing);
    let steeping = false;
    const points: NightSentence[] = [];
    const awayPoints: NightSentence[] = [];
    graph.forEachNeighbor(thing, (point, pointAttrs) => {
      if (pointAttrs.night !== true || pointAttrs.dustOf !== thing) return;
      const at = drawnAt(point);
      if (!at) return;
      const made = pointAttrs.createdAt;
      if (typeof made === "number" && now - made < STEEPING_SECS) steeping = true;
      if (lit(point)) anyLit = true;
      const sentence = { ...at, dim: !lit(point), id: point, text: String(pointAttrs.label ?? ""), open: pointAttrs.open === true, thing };
      // Out beside its topic: on the map there, not in the frame.
      if (typeof pointAttrs.fly === "number" && pointAttrs.fly > 0) {
        // Flown off screen: its sentence waits at the edge, under its topic.
        const anchor = (pointAttrs.flyTo as { anchor?: string } | undefined)?.anchor;
        if (thing === picked && anchor && graph.hasNode(anchor) && !onScreen(at)) {
          const sign = edges.find((edge) => edge.kind === "topic" && edge.goTo === anchor);
          if (sign) sign.count += 1;
          else edges.push({ kind: "topic", thing, goTo: anchor, title: String(graph.getNodeAttribute(anchor, "label") ?? ""), count: 1, text: sentence.text, open: sentence.open, toward: drawnAt(anchor) ?? at });
          return;
        }
        awayPoints.push({ ...sentence, away: true });
        return;
      }
      left = Math.min(left, at.x - at.r);
      right = Math.max(right, at.x + at.r);
      top = Math.min(top, at.y - at.r);
      bottom = Math.max(bottom, at.y + at.r);
      points.push(sentence);
    });
    if (points.length === 0) {
      // Every point is out by its topic: the frame stays, small, on the
      // thing's centre, so its name still says where the thing is.
      const centre = awayPoints.length > 0 || edges.some((edge) => edge.thing === thing) ? centreAt(thing) : null;
      if (!centre) return;
      left = right = centre.x;
      top = bottom = centre.y;
    }
    const grow = (low: number, high: number, least: number) => {
      const short = Math.max(0, least - (high - low)) / 2;
      return [low - short, high + short] as const;
    };
    const [frameLeft, frameRight] = grow(left - FRAME_PAD, right + FRAME_PAD, MIN_FRAME_WIDTH);
    const [frameTop, frameBottom] = grow(top - FRAME_PAD, bottom + FRAME_PAD, MIN_FRAME_HEIGHT);
    const frame = {
      thing,
      title: String(attrs.label ?? ""),
      left: frameLeft,
      top: frameTop,
      right: frameRight,
      bottom: frameBottom,
      dim: !anyLit,
      steeping,
      spots: points.map(({ x, y, r }) => ({ x, y, r })),
    };
    if (frame.right < -CULL_PAD || frame.bottom < -CULL_PAD || frame.left > viewport.width + CULL_PAD || frame.top > viewport.height + CULL_PAD) {
      // The camera went to a far topic: a sign leads back to the thing.
      if (thing === picked) {
        edges.push({ kind: "back", thing, goTo: thing, title: frame.title, count: 0, text: "", open: false, toward: { x: (frame.left + frame.right) / 2, y: (frame.top + frame.bottom) / 2 } });
        sentences.push(...awayPoints.filter((point) => point.text && sentencesFor(thing, point.id)));
      }
      return;
    }
    frames.push(frame);
    sentences.push(...[...points, ...awayPoints].filter((point) => point.text && sentencesFor(thing, point.id)));
  });
  return { frames, sentences, edges };
}

/** The crescent of ThingSteeped's SteepMoon, on its 16-unit box. */
const MOON_PATH = "M10.5 1.5a6.5 6.5 0 1 0 4 11.6A6 6 0 0 1 10.5 1.5z";
/** The moon's drawn size, CSS px. */
const MOON_PX = 11;
/** Gap between a title and its moon, CSS px. */
const MOON_GAP = 4;
const FRAME_RADIUS = 10;
/** Room for a point's own dot inside the halo's padding, CSS px. */
const HALO_DOT_ROOM = 2;

/** The hull a thing's halo is grown round: its points only, so the halo keeps
 *  the round, region-like shape of what it holds at every zoom. Sentences sit
 *  beside the halo; wrapping them in made a wide, flat band once zoomed in.
 *  Null when there is nothing to grow round. */
export function haloOutline(frame: ThingFrame): { x: number; y: number }[] | null {
  const around = (frame.spots ?? []).map(({ x, y }) => ({ x, y }));
  if (around.length === 0) return null;
  return convexHull(around);
}

/** How far a halo reaches past its hull: the padding plus its largest dot. */
export function haloPad(frame: ThingFrame): number {
  const dot = Math.max(0, ...(frame.spots ?? []).map(({ r }) => r));
  return FRAME_PAD + dot + HALO_DOT_ROOM;
}
const TITLE_FONT = '500 12px "Instrument Sans", -apple-system, sans-serif';
const TITLE_MAX_PX = 240;
/** Shorter cuts a crowded frame's title is tried at, CSS px. */
const TITLE_SHORTER_PX = [140, 80] as const;
/** A title's line, CSS px: the 12px face's ascent and descent with room. */
const TITLE_LINE = 17;
/** How many lines further out a picked thing's title may go in a crowd. */
const TITLE_FURTHER_LINES = [1, 2, 3] as const;
const SENTENCE_FONT = '12px "Instrument Sans", -apple-system, sans-serif';
/** A sentence wraps at this width, CSS px. */
export const SENTENCE_MAX_PX = 320;
/** ...onto at most this many lines; past them, an ellipsis. */
export const SENTENCE_MAX_LINES = 3;
/** Narrower and narrower blocks are tried where the widest would cover
 *  another point or word; narrower than the last, the side is given up. */
const SENTENCE_WIDTHS = [SENTENCE_MAX_PX, 260, 200, 150, 110];
/** Gap between a point's disc and its sentence, CSS px. */
const SENTENCE_GAP = 6;
/** One line of sentence, CSS px, with its halo. */
export const SENTENCE_LINE = 15;

type Measure = (text: string) => number;

/** One wrappable piece: a word with the space after it, or a single CJK
 *  character (those wrap anywhere). */
const PIECE = /[\u2e80-\u9fff\uac00-\ud7af\uf900-\ufaff\uff00-\uffef]|[^\s\u2e80-\u9fff\uac00-\ud7af\uf900-\ufaff\uff00-\uffef]+\s*|\s+/gu;
const WORD_CHAR = /[\p{L}\p{N}]/u;
const CJK = /[\u2e80-\u9fff\uac00-\ud7af\uf900-\ufaff\uff00-\uffef]/u;

/**
 * `text` cut with an ellipsis to fit `max` px, at a word's end where the
 * text has words, never part way through one; CJK text can end at any
 * character. Unchanged when it fits.
 */
export function cutToFit(measure: Measure, text: string, max: number): string {
  if (measure(text) <= max) return text;
  let cut = text;
  while (cut.length > 1 && measure(`${cut.trimEnd()}…`) > max) cut = cut.slice(0, -1);
  const next = text.charAt(cut.length);
  const last = cut.charAt(cut.length - 1);
  if (WORD_CHAR.test(next) && WORD_CHAR.test(last) && !CJK.test(next) && !CJK.test(last)) {
    // Mid-word: back to the end of the last whole word, if there is one.
    const space = cut.search(/\s\S*$/u);
    if (space > 0) cut = cut.slice(0, space);
  }
  return `${cut.trimEnd().replace(/[\s,;:–—\-，、；：]+$/u, "")}…`;
}

/**
 * `text` wrapped into lines no wider than `width`, breaking between words
 * (or between CJK characters), at most `maxLines` of them; the last is cut
 * with an ellipsis when the text runs on. A word wider than a line is
 * broken where it must be.
 */
export function wrapText(measure: Measure, text: string, width: number, maxLines = SENTENCE_MAX_LINES): string[] {
  const source = text.trim().replace(/\s+/gu, " ");
  // Each piece with where it starts; a word wider than a line becomes its
  // characters, so it breaks where it must.
  const units: { text: string; at: number }[] = [];
  let at = 0;
  for (const piece of source.match(PIECE) ?? []) {
    if (measure(piece.trimEnd()) <= width) units.push({ text: piece, at });
    else [...piece].reduce((offset, char) => (units.push({ text: char, at: offset }), offset + char.length), at);
    at += piece.length;
  }
  const lines: string[] = [];
  const starts: number[] = [];
  let line = "";
  for (const unit of units) {
    if (line && measure((line + unit.text).trimEnd()) > width) {
      lines.push(line.trimEnd());
      line = "";
    }
    if (!line) {
      if (!unit.text.trim()) continue;
      starts.push(unit.at);
      line = unit.text.trimStart();
    } else line += unit.text;
  }
  if (line.trim()) lines.push(line.trimEnd());
  if (lines.length <= maxLines) return lines;
  const kept = lines.slice(0, maxLines);
  // The last kept line runs on with everything after it, cut to fit.
  kept[maxLines - 1] = cutToFit(measure, source.slice(starts[maxLines - 1]), width);
  return kept;
}

export type Box = { left: number; right: number; top: number; bottom: number };

const overlapping = (a: Box, b: Box) => a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom;

/** A sentence as it will be painted: its lines, the x they start (or end)
 *  at, and the middle of its first line, level with its point. */
export interface PlacedSentence {
  id: string;
  thing: string;
  lines: string[];
  x: number;
  y: number;
  align: "left" | "right";
  open: boolean;
  dim: boolean;
  box: Box;
  away?: boolean;
}

const shown = (lines: readonly string[]) => lines.join(" ").replace(/…$/u, "").length;

/**
 * Where each shown sentence goes (UX-CHAT-5): beside its point, its first
 * line level with it, wrapped at SENTENCE_MAX_PX onto at most
 * SENTENCE_MAX_LINES lines. It goes to the right of the point, or to the
 * left when that shows more of it; where the full width would cover another
 * point or a word already placed, narrower blocks are tried, so it is cut
 * only when no room is left; it is left out rather than printed over
 * something, a point or another body on the map (`bodies`, which are only
 * read). Never past the viewport's edge when one is given. Points are
 * taken top to bottom, so the result does not depend on graph order.
 * `occupied` (names already painted) is read and extended.
 */
export function layoutNightSentences(
  sentences: readonly NightSentence[],
  measure: Measure,
  occupied: Box[] = [],
  viewport?: { width: number },
  bodies: readonly Box[] = [],
): PlacedSentence[] {
  const ordered = [...sentences].sort((a, b) => a.y - b.y || a.x - b.x || (a.id < b.id ? -1 : 1));
  const discs = sentences.map((point) => ({ id: point.id, left: point.x - point.r, right: point.x + point.r, top: point.y - point.r, bottom: point.y + point.r }));
  const placed: PlacedSentence[] = [];
  for (const point of ordered) {
    const blockers: Box[] = [...discs.filter((disc) => disc.id !== point.id), ...bodies, ...occupied];
    // `align` is the text's anchor, so "left" writes to the right of the point.
    const trySide = (align: "left" | "right"): PlacedSentence | null => {
      const start = align === "left" ? point.x + point.r + SENTENCE_GAP : point.x - point.r - SENTENCE_GAP;
      const edge = viewport ? (align === "left" ? viewport.width - SENTENCE_GAP - start : start - SENTENCE_GAP) : Infinity;
      for (const widest of SENTENCE_WIDTHS) {
        const width = Math.min(widest, edge);
        if (width < SENTENCE_WIDTHS[SENTENCE_WIDTHS.length - 1]!) return null;
        const lines = wrapText(measure, point.text, width);
        if (lines.length === 0) return null;
        const inked = Math.max(...lines.map(measure));
        const top = point.y - SENTENCE_LINE / 2;
        const box = align === "left"
          ? { left: start - 2, right: start + inked + 2, top, bottom: top + lines.length * SENTENCE_LINE }
          : { left: start - inked - 2, right: start + 2, top, bottom: top + lines.length * SENTENCE_LINE };
        if (blockers.some((other) => overlapping(other, box))) continue;
        return { id: point.id, thing: point.thing, lines, x: start, y: point.y, align, open: point.open, dim: point.dim, box, ...(point.away ? { away: true } : {}) };
      }
      return null;
    };
    const after = trySide("left");
    const whole = (spot: PlacedSentence | null) => spot !== null && !spot.lines[spot.lines.length - 1]!.endsWith("…");
    const before = whole(after) ? null : trySide("right");
    const spot = before && (!after || shown(before.lines) > shown(after.lines)) ? before : after;
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
  /** An edge sign: where a tap on it takes the camera (EdgeSign.goTo). */
  goTo?: string;
}

/** Where a tap on an edge sign takes the camera, or null off every sign. */
export function edgeSignAt(hits: readonly FrameHit[], x: number, y: number): string | null {
  for (const hit of hits) {
    if (hit.goTo && x >= hit.left && x <= hit.right && y >= hit.top && y <= hit.bottom) return hit.goTo;
  }
  return null;
}

/** Room kept clear at the screen's edges for the map's own controls, CSS px. */
export const EDGE_SIGN_INSETS = { top: 84, right: 16, bottom: 60, left: 16 } as const;
const EDGE_SIGN_MAX_PX = 220;
const EDGE_SIGN_PAD = 8;
const EDGE_SIGN_GAP = 6;
const EDGE_HEADER_LINE = 16;
const ARROWS = ["→", "↘", "↓", "↙", "←", "↖", "↑", "↗"] as const;

/** The arrow pointing from (x, y) toward `to`, screen y running down. */
export function arrowToward(from: { x: number; y: number }, to: { x: number; y: number }): string {
  const sector = Math.round(Math.atan2(to.y - from.y, to.x - from.x) / (Math.PI / 4));
  return ARROWS[((sector % 8) + 8) % 8]!;
}

/** An edge sign as it will be painted. */
export interface PlacedEdgeSign {
  sign: EdgeSign;
  header: string;
  /** The sentence line, cut to fit ("" for a back sign). */
  line: string;
  box: Box;
}

/**
 * Where each edge sign goes: on the line from the screen's centre toward
 * what it points at, pushed out to the edge (inside EDGE_SIGN_INSETS), so its
 * arrow tells the true way. Signs that would overlap slide along the edge;
 * one with no room left is left out.
 */
export function placeEdgeSigns(
  signs: readonly EdgeSign[],
  measure: Measure,
  viewport: { width: number; height: number },
  insets: { top: number; right: number; bottom: number; left: number } = EDGE_SIGN_INSETS,
): PlacedEdgeSign[] {
  const centre = { x: viewport.width / 2, y: viewport.height / 2 };
  const placed: PlacedEdgeSign[] = [];
  for (const sign of signs) {
    const dx = sign.toward.x - centre.x;
    const dy = sign.toward.y - centre.y;
    if (Math.hypot(dx, dy) < 1) continue;
    const arrow = arrowToward(centre, sign.toward);
    const room = EDGE_SIGN_MAX_PX - measure(`${arrow} · ${sign.count}  `);
    const name = cutToFit(measure, sign.title, Math.max(40, room));
    const count = sign.count > 1 ? ` · ${sign.count}` : "";
    const header = dx < 0 ? `${arrow} ${name}${count}` : `${name}${count} ${arrow}`;
    const line = sign.text ? cutToFit(measure, sign.text, EDGE_SIGN_MAX_PX) : "";
    const width = Math.min(EDGE_SIGN_MAX_PX, Math.max(measure(header), line ? measure(line) : 0)) + 2 * EDGE_SIGN_PAD;
    const height = EDGE_HEADER_LINE + (line ? SENTENCE_LINE : 0) + EDGE_SIGN_PAD;
    const lowX = insets.left + width / 2;
    const highX = viewport.width - insets.right - width / 2;
    const lowY = insets.top + height / 2;
    const highY = viewport.height - insets.bottom - height / 2;
    if (lowX > highX || lowY > highY) continue;
    // Out from the centre to the first edge the line meets.
    const reach = Math.min(
      dx > 0 ? (highX - centre.x) / dx : dx < 0 ? (lowX - centre.x) / dx : Infinity,
      dy > 0 ? (highY - centre.y) / dy : dy < 0 ? (lowY - centre.y) / dy : Infinity,
    );
    const at = { x: centre.x + dx * reach, y: centre.y + dy * reach };
    const onSide = Math.abs(at.x - lowX) < 0.5 || Math.abs(at.x - highX) < 0.5;
    const boxAt = (x: number, y: number) => ({ left: x - width / 2, right: x + width / 2, top: y - height / 2, bottom: y + height / 2 });
    let box: Box | null = null;
    for (const step of [0, 1, -1, 2, -2, 3, -3]) {
      const x = onSide ? at.x : Math.min(highX, Math.max(lowX, at.x + step * (width + EDGE_SIGN_GAP)));
      const y = onSide ? Math.min(highY, Math.max(lowY, at.y + step * (height + EDGE_SIGN_GAP))) : at.y;
      const tried = boxAt(x, y);
      if (placed.some((other) => overlapping(other.box, tried))) continue;
      box = tried;
      break;
    }
    if (!box) continue;
    placed.push({ sign, header, line, box });
  }
  return placed;
}

/** The thing whose frame or title is under (x, y), the smallest when frames
 *  overlap, else null. */
export function frameAt(hits: readonly FrameHit[], x: number, y: number): string | null {
  let best: FrameHit | null = null;
  for (const hit of hits) {
    if (hit.goTo) continue;
    if (x < hit.left || x > hit.right || y < hit.top || y > hit.bottom) continue;
    const area = (hit.right - hit.left) * (hit.bottom - hit.top);
    if (!best || area < (best.right - best.left) * (best.bottom - best.top)) best = hit;
  }
  return best?.thing ?? null;
}

/** A frame grown round the sentences laid out for its points, so its
 *  border never runs through their text. */
export function encloseSentences<F extends ThingFrame>(frames: readonly F[], placed: readonly PlacedSentence[]): F[] {
  return frames.map((frame) => {
    let { left, top, right, bottom } = frame;
    for (const sentence of placed) {
      if (sentence.thing !== frame.thing || sentence.away) continue;
      left = Math.min(left, sentence.box.left - FRAME_PAD);
      right = Math.max(right, sentence.box.right + FRAME_PAD);
      top = Math.min(top, sentence.box.top - FRAME_PAD);
      bottom = Math.max(bottom, sentence.box.bottom + FRAME_PAD);
    }
    return { ...frame, left, top, right, bottom };
  });
}

/** The sentences to keep: where a thing's sentences would grow its frame
 *  onto another thing's, it shows only its points (the picked thing always
 *  keeps its sentences). With "Working on" every active thing is drawn, and
 *  neighbours' sentences would otherwise run their halos into each other. */
export function uncrowdedSentences<F extends ThingFrame>(frames: readonly F[], placed: readonly PlacedSentence[], picked: string | null = null): PlacedSentence[] {
  if (frames.length < 2) return [...placed];
  const grown = encloseSentences(frames, placed);
  const meets = (a: ThingFrame, b: ThingFrame) => a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom;
  const crowded = new Set<string>();
  // Sentences that reach another thing's points go first...
  for (const frame of grown) {
    if (frame.thing === picked) continue;
    if (frames.some((other) => other.thing !== frame.thing && meets(frame, other))) crowded.add(frame.thing);
  }
  // ...then, of two grown halos that still meet, the later one gives way
  // (the picked thing is taken first and never gives way).
  const accepted: ThingFrame[] = [];
  const ordered = [...grown.filter((frame) => frame.thing === picked), ...grown.filter((frame) => frame.thing !== picked)];
  for (const frame of ordered) {
    if (crowded.has(frame.thing)) continue;
    if (frame.thing !== picked && accepted.some((other) => meets(frame, other))) crowded.add(frame.thing);
    else accepted.push(frame);
  }
  return crowded.size === 0 ? [...placed] : placed.filter((sentence) => !crowded.has(sentence.thing));
}

/** Where each frame's title goes: above the frame, else below it, each
 *  flush with the frame's left edge, else its right; at most TITLE_MAX_PX
 *  (or the frame's width) long and cut at a word. Where none of those spots
 *  is free the title is tried again cut shorter (TITLE_SHORTER_PX), so a
 *  crowded small frame still says what it is. A title is left out where
 *  every spot would cover a name, a sentence, another title or another
 *  frame's border, except the picked thing's: it is placed first and always
 *  gets a title, further out where it must be, so what is picked can be read. `occupied` is read
 *  and extended. A steeping thing's title keeps room after it for its moon. */
export function placeFrameTitles(
  frames: readonly ThingFrame[],
  measure: Measure,
  occupied: Box[] = [],
  picked: string | null = null,
): { thing: string; text: string; x: number; baseline: number; box: Box }[] {
  const out: { thing: string; text: string; x: number; baseline: number; box: Box }[] = [];
  const ordered = picked === null ? frames : [...frames.filter((frame) => frame.thing === picked), ...frames.filter((frame) => frame.thing !== picked)];
  for (const frame of ordered) {
    if (!frame.title) continue;
    // Another frame's border counts as ink; its own does not, nor the
    // inside of a frame this one sits in.
    const borders = frames.filter((other) => other !== frame);
    const crosses = (box: Box, other: Box) =>
      overlapping(box, other) && !(box.left > other.left && box.right < other.right && box.top > other.top && box.bottom < other.bottom);
    const spotsFor = (text: string) => {
      // A steeping thing's moon sits after its title, so it takes room too.
      const width = measure(text) + (frame.steeping ? MOON_GAP + MOON_PX : 0);
      const xs = [frame.left + 2, frame.right - 2 - width];
      return [frame.top - 4, frame.bottom + TITLE_LINE].flatMap((baseline) =>
        (xs[1]! > xs[0]! + 0.5 ? xs : [xs[0]!]).map((x) => ({
          text,
          x,
          baseline,
          box: { left: x - 3, right: x + width + 3, top: baseline - TITLE_LINE + 2, bottom: baseline + 3 },
        })),
      );
    };
    const full = cutToFit(measure, frame.title, Math.max(TITLE_MAX_PX, frame.right - frame.left));
    const texts = [full];
    for (const limit of TITLE_SHORTER_PX) {
      if (measure(texts[texts.length - 1]!) <= limit) break;
      const shorter = cutToFit(measure, frame.title, limit);
      if (shorter && shorter !== texts[texts.length - 1] && shorter !== "…") texts.push(shorter);
    }
    const free = ({ box }: { box: Box }) => !occupied.some((other) => overlapping(other, box)) && !borders.some((other) => crosses(box, other));
    let spot: ReturnType<typeof spotsFor>[number] | undefined;
    for (const text of texts) {
      spot = spotsFor(text).find(free);
      if (spot) break;
    }
    if (!spot && frame.thing === picked) {
      // Crowded all round: a line or more further out, before covering a name.
      const near = spotsFor(full);
      const further = TITLE_FURTHER_LINES.flatMap((lines) =>
        near.map((spot) => {
          const shift = (spot.baseline < frame.top ? -lines : lines) * TITLE_LINE;
          return { ...spot, baseline: spot.baseline + shift, box: { ...spot.box, top: spot.box.top + shift, bottom: spot.box.bottom + shift } };
        }),
      );
      spot = further.find(free) ?? near[0];
    }
    if (!spot) continue;
    occupied.push(spot.box);
    out.push({ thing: frame.thing, text: spot.text, x: spot.x, baseline: spot.baseline, box: spot.box });
  }
  return out;
}

/** Whether a name is left off where a frame or its title already sits. Points
 *  have their own rule, and a name the picked thing's points link to is the
 *  reason for the picture: it stays, and the frame's marks keep off it. */
export function labelYieldsToFrames(isNight: boolean, isLinked: boolean): boolean {
  return !isNight && !isLinked;
}

/**
 * Paint the halos, their titles (a moon after a steeping thing's) and the
 * shown sentences.
 * Sentences are laid out first (layoutNightSentences) and each frame grows
 * round its own, then titles take a free spot (placeFrameTitles); a settled
 * point's sentence is in the label ink, an open one's in the muted ink.
 * Everything placed is added to `occupied`, shared with the node labels.
 * Returns where a click picks each thing (frameAt).
 */
export function drawNightMarks(
  ctx: CanvasRenderingContext2D,
  marks: { frames: readonly ThingFrame[]; sentences?: readonly NightSentence[]; edges?: readonly EdgeSign[] },
  palette: GraphPalette,
  occupied: Box[] = [],
  bodies: readonly Box[] = [],
  /** The picked thing, whose title always shows (placeFrameTitles). */
  picked: string | null = null,
): FrameHit[] {
  const hits: FrameHit[] = [];
  if (marks.frames.length === 0 && !marks.edges?.length && !marks.sentences?.length) return hits;
  ctx.save();
  ctx.font = SENTENCE_FONT;
  const width = ctx.canvas.width / (typeof window === "undefined" ? 1 : window.devicePixelRatio || 1);
  const placed = marks.sentences && marks.sentences.length > 0
    ? layoutNightSentences(marks.sentences, (text) => ctx.measureText(text).width, occupied, { width }, bodies)
    : [];
  const kept = uncrowdedSentences(marks.frames, placed, picked);
  const frames = encloseSentences(marks.frames, kept);
  // A soft ground under the thing's points, in the region names' manner
  // (Fable's design: a faint halo and its name, no drawn border).
  ctx.fillStyle = palette.bridge;
  for (const frame of frames) {
    ctx.globalAlpha = frame.dim ? 0.04 : 0.1;
    const frameWidth = frame.right - frame.left;
    const frameHeight = frame.bottom - frame.top;
    ctx.beginPath();
    const outline = haloOutline(frame);
    if (outline) traceRoundedArea(ctx, outline, haloPad(frame));
    else if (typeof ctx.roundRect === "function") ctx.roundRect(frame.left, frame.top, frameWidth, frameHeight, Math.min(FRAME_RADIUS, frameHeight / 2));
    else ctx.rect(frame.left, frame.top, frameWidth, frameHeight);
    ctx.closePath();
    ctx.fill();
    hits.push({ thing: frame.thing, left: frame.left, top: frame.top, right: frame.right, bottom: frame.bottom });
  }
  ctx.font = TITLE_FONT;
  ctx.textAlign = "left";
  ctx.textBaseline = "alphabetic";
  ctx.lineJoin = "round";
  ctx.lineWidth = 3;
  ctx.strokeStyle = palette.surface;
  ctx.fillStyle = palette.label;
  const dimOf = new Map(frames.map((frame) => [frame.thing, frame.dim]));
  const steepingThings = new Set(frames.filter((frame) => frame.steeping).map((frame) => frame.thing));
  const moon = typeof Path2D === "function" ? new Path2D(MOON_PATH) : null;
  for (const title of placeFrameTitles(frames, (text) => ctx.measureText(text).width, occupied, picked)) {
    ctx.globalAlpha = dimOf.get(title.thing) ? 0.3 : 1;
    hits.push({ thing: title.thing, ...title.box });
    ctx.strokeText(title.text, title.x, title.baseline);
    ctx.fillText(title.text, title.x, title.baseline);
    if (moon && steepingThings.has(title.thing)) {
      ctx.save();
      ctx.fillStyle = palette.moon ?? palette.bridge;
      ctx.translate(title.x + ctx.measureText(title.text).width + MOON_GAP, title.baseline - MOON_PX + 1);
      ctx.scale(MOON_PX / 16, MOON_PX / 16);
      ctx.globalAlpha = dimOf.get(title.thing) ? 0.3 : 1;
      ctx.fill(moon);
      ctx.restore();
    }
  }
  if (kept.length > 0) {
    ctx.font = SENTENCE_FONT;
    ctx.textBaseline = "middle";
    for (const sentence of kept) {
      ctx.globalAlpha = sentence.dim ? 0.3 : 1;
      ctx.textAlign = sentence.align;
      ctx.fillStyle = sentence.open ? palette.labelMuted : palette.label;
      sentence.lines.forEach((line, index) => {
        const y = sentence.y + index * SENTENCE_LINE;
        ctx.strokeText(line, sentence.x, y);
        ctx.fillText(line, sentence.x, y);
      });
    }
    ctx.textAlign = "left";
  }
  if (marks.edges && marks.edges.length > 0) {
    const height = ctx.canvas.height / (typeof window === "undefined" ? 1 : window.devicePixelRatio || 1);
    ctx.textAlign = "left";
    ctx.textBaseline = "middle";
    for (const { sign, header, line, box } of placeEdgeSigns(marks.edges, (text) => {
      ctx.font = TITLE_FONT;
      return ctx.measureText(text).width;
    }, { width, height })) {
      ctx.globalAlpha = 0.92;
      ctx.fillStyle = palette.surface;
      ctx.beginPath();
      if (typeof ctx.roundRect === "function") ctx.roundRect(box.left, box.top, box.right - box.left, box.bottom - box.top, 8);
      else ctx.rect(box.left, box.top, box.right - box.left, box.bottom - box.top);
      ctx.fill();
      ctx.globalAlpha = 0.5;
      ctx.lineWidth = 1;
      ctx.strokeStyle = palette.bridge;
      ctx.stroke();
      ctx.globalAlpha = 1;
      ctx.font = TITLE_FONT;
      ctx.fillStyle = palette.label;
      ctx.fillText(header, box.left + EDGE_SIGN_PAD, box.top + EDGE_SIGN_PAD / 2 + EDGE_HEADER_LINE / 2);
      if (line) {
        ctx.font = SENTENCE_FONT;
        ctx.fillStyle = sign.open ? palette.labelMuted : palette.label;
        ctx.fillText(line, box.left + EDGE_SIGN_PAD, box.top + EDGE_SIGN_PAD / 2 + EDGE_HEADER_LINE + SENTENCE_LINE / 2);
      }
      hits.push({ thing: sign.thing, goTo: sign.goTo, ...box });
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
  const under = graph.filterNeighbors(thing, (_id, attrs) => attrs.night === true && (attrs.dustOf === undefined || attrs.dustOf === thing));
  const elsewhere = (graph.getNodeAttribute(thing, "elsewhere") as string[] | undefined) ?? [];
  return [...under, ...elsewhere.filter((id) => graph.hasNode(id))].sort();
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
 * The nodes a stop frames. 這件事 (this): the thing's points and what they link
 * to, so a link is never a line running off the screen. 附近 (nearby): those,
 * and each of those bodies' own neighbours (pages and entities, not their memory
 * dust). 全部 (all): every node.
 */
export function thingStopNodeIds(graph: Graph, thing: string, stop: ThingStop): string[] {
  if (stop === "all") return graph.nodes();
  const points = thingPointIds(graph, thing);
  const linked = linkedBodies(graph, points);
  if (stop === "this") {
    // An active thing frames itself and the topics near it only: a far one
    // would pull the camera back until the thing is a speck (Fable's design,
    // Qi-Xuan 11-05). Its points that fly off to a far topic are shown at the
    // screen's edge instead (nightMarks' `edges`).
    if (graph.getNodeAttribute(thing, "quiet") === true) return [...points, ...linked].sort();
    const near = new Set(nearLinkedBodies(graph, thing));
    const stays = points.filter((point) => {
      const flown = graph.getNodeAttribute(point, "flyTo") as { anchor: string } | undefined;
      return !flown || near.has(flown.anchor);
    });
    return [...new Set([thing, ...stays, ...near])].sort();
  }
  const near = new Set([...points, ...linked]);
  for (const body of linked) {
    graph.forEachNeighbor(body, (other, attrs) => {
      if (attrs.entityType !== MEMORY_NODE_TYPE && attrs.entityType !== THING_NODE_TYPE) near.add(other);
    });
  }
  return [...near].sort();
}

/** Room round a thing's cluster for its points' sentences, graph units: about
 *  three quarters of a sentence's width at the 這件事 zoom. */
export const NEAR_ROOM = 20;

/**
 * The topics a thing's points link to that are near enough to frame with it at
 * 這件事: within twice the reach of its cluster and the room its sentences take,
 * from its centre. The rest are far; a point flying to one leaves the screen.
 */
export function nearLinkedBodies(graph: Graph, thing: string): string[] {
  if (!graph.hasNode(thing)) return [];
  const cx = graph.getNodeAttribute(thing, "x") as number;
  const cy = graph.getNodeAttribute(thing, "y") as number;
  const reach = 2 * (((graph.getNodeAttribute(thing, "clusterRadius") as number | undefined) ?? 0) + NEAR_ROOM);
  return linkedBodies(graph, thingPointIds(graph, thing)).filter((id) => {
    const x = graph.getNodeAttribute(id, "x") as number;
    const y = graph.getNodeAttribute(id, "y") as number;
    return Math.hypot(x - cx, y - cy) <= reach;
  });
}

/** The stops a thing offers: 附近 only when its points link to something
 *  on the map, since without links it would frame just what 這件事 does. */
export function thingStopsFor(graph: Graph, thing: string): ThingStop[] {
  return linkedBodies(graph, thingPointIds(graph, thing)).length > 0 ? [...THING_STOPS] : ["this", "all"];
}

/** The lit set while a thing is selected: its points and what they link to,
 *  plus the neighbourhood at 附近. Like a hover on the thing, but its lines
 *  to its own centre stay hidden and the lines inside the lit set show. */
export function thingFocusState(graph: Graph, thing: string, stop: ThingStop): HoverState {
  const points = thingPointIds(graph, thing);
  const linked = linkedBodies(graph, points);
  const lit = stop === "nearby" ? thingStopNodeIds(graph, thing, stop) : [...points, ...linked];
  return { hovered: thing, neighbors: new Set(lit), thing, linked: new Set(linked) };
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
