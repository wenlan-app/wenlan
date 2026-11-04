// SPDX-License-Identifier: AGPL-3.0-only
import type Graph from "graphology";
import type { HoverState } from "./atlas";
import { THING_NODE_TYPE } from "./model";
import type { GraphPalette } from "./palette";

/**
 * What sigma's round discs cannot draw, painted on AtlasView's overlay canvas
 * above the map like the dust counts (UX-CHAT-5): each thing as a frame, a
 * rounded dashed outline round the cluster of points its nights wrote with
 * the thing's title above it, and a small moon beside each of those points,
 * the same crescent the thing's page marks them with. The thing itself is no
 * node on the map; its hidden centre only holds the cluster in place.
 */
export interface ThingFrame {
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
): { frames: ThingFrame[]; moons: NightMoon[] } {
  const frames: ThingFrame[] = [];
  const moons: NightMoon[] = [];
  const lit = (id: string) => hover.hovered === null || hover.hovered === id || hover.neighbors.has(id);
  graph.forEachNode((thing, attrs) => {
    if (attrs.entityType !== THING_NODE_TYPE) return;
    let left = Infinity;
    let top = Infinity;
    let right = -Infinity;
    let bottom = -Infinity;
    let anyLit = lit(thing);
    const points: NightMoon[] = [];
    graph.forEachNeighbor(thing, (point, pointAttrs) => {
      if (pointAttrs.night !== true || pointAttrs.dustOf !== thing) return;
      const at = drawnAt(point);
      if (!at) return;
      left = Math.min(left, at.x - at.r);
      right = Math.max(right, at.x + at.r);
      top = Math.min(top, at.y - at.r);
      bottom = Math.max(bottom, at.y + at.r);
      if (lit(point)) anyLit = true;
      points.push({ ...at, dim: !lit(point) });
    });
    if (points.length === 0) return;
    const frame = {
      title: String(attrs.label ?? ""),
      left: left - FRAME_PAD,
      top: top - FRAME_PAD,
      right: right + FRAME_PAD,
      bottom: bottom + FRAME_PAD,
      dim: !anyLit,
    };
    if (frame.right < -CULL_PAD || frame.bottom < -CULL_PAD || frame.left > viewport.width + CULL_PAD || frame.top > viewport.height + CULL_PAD) return;
    frames.push(frame);
    moons.push(...points);
  });
  return { frames, moons };
}

/** The crescent of ThingSteeped's SteepMoon, on its 16-unit box. */
const MOON_PATH = "M10.5 1.5a6.5 6.5 0 1 0 4 11.6A6 6 0 0 1 10.5 1.5z";
/** The moon's drawn size, CSS px. */
const MOON_PX = 7;
const FRAME_RADIUS = 10;
const TITLE_FONT = '500 12px "Instrument Sans", -apple-system, sans-serif';
const TITLE_MAX_PX = 240;

/** `text` cut with an ellipsis to fit `max` px. */
function fit(ctx: CanvasRenderingContext2D, text: string, max: number): string {
  if (ctx.measureText(text).width <= max) return text;
  let cut = text;
  while (cut.length > 1 && ctx.measureText(`${cut}…`).width > max) cut = cut.slice(0, -1);
  return `${cut.trimEnd()}…`;
}

type Box = { left: number; right: number; top: number; bottom: number };

/**
 * Paint the frames, their titles and the moons. A title goes above its frame,
 * else below it when that spot is taken by a name already painted
 * (`occupied`, shared with the node labels) or another thing's title, else it
 * is left out for that frame rather than printed over another word.
 */
export function drawNightMarks(
  ctx: CanvasRenderingContext2D,
  marks: { frames: readonly ThingFrame[]; moons: readonly NightMoon[] },
  palette: GraphPalette,
  occupied: Box[] = [],
): void {
  if (marks.frames.length === 0) return;
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
    ctx.strokeText(title, x, spot.baseline);
    ctx.fillText(title, x, spot.baseline);
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
}
