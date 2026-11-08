import type Graph from "graphology";
import { islandSolid, type LodState } from "./atlas";
import type { GraphPalette } from "./palette";
import { THING_NODE_TYPE } from "./model";

/**
 * The memory count beside a busy anchor. At the opening view an anchor shows
 * only a pinch of its memories (see atlas.ts's dustVisibleCount); when it
 * has at least DUST_BADGE_MIN the number says how many remain collapsed.
 * Below that the dots themselves say "a few" and a number would be clutter.
 * Drawn on AtlasView's overlay canvas, above sigma, like the place names.
 */
export const DUST_BADGE_MIN = 25;

/** The anchors that can carry a count — fixed for a mounted graph. A
 *  thing's points are all drawn (its frame holds them), so it has none. */
export function dustBadgeAnchors(graph: Graph): string[] {
  return graph.filterNodes(
    (_id, attrs) => attrs.entityType !== THING_NODE_TYPE && ((attrs.dustCount as number | undefined) ?? 0) >= DUST_BADGE_MIN,
  );
}

const BADGE_FONT = "500 10px 'JetBrains Mono', ui-monospace, monospace";
const BADGE_HALO_WIDTH = 3;
/** Extra viewport margin inside which a badge is still drawn. */
const BADGE_CULL_PAD = 24;

export type DustBadge = { id: string; x: number; y: number; width: number; text: string };

/**
 * Where the counts go this paint, before anything is drawn: AtlasView keeps
 * the node names off these spots, and drawDustCounts paints exactly these, so
 * the two never disagree. Text is centred vertically on `y`, 14 px tall.
 */
export function planDustBadges(
  graph: Graph,
  anchors: string[],
  project: (pos: { x: number; y: number }) => { x: number; y: number },
  lod: LodState,
  hovered: string | null,
  viewport: { width: number; height: number },
  pixelScale = 1,
): DustBadge[] {
  if (lod.dustVisible === Infinity || hovered !== null) return [];
  const boxes: DustBadge[] = [];
  for (const id of [...anchors].sort((a, b) => graph.getNodeAttribute(b, "dustCount") - graph.getNodeAttribute(a, "dustCount"))) {
    const attrs = graph.getNodeAttributes(id);
    if (attrs.hidden) continue;
    if (!islandSolid(attrs, lod)) continue;
    const count = attrs.dustCount as number;
    // A crowded ring shows only the dots that fit (markCrowdedDust).
    const shown = Math.min(lod.dustVisible, (attrs.dustRoom as number | undefined) ?? Infinity);
    if (count <= shown) continue;
    const p = project({ x: attrs.x as number, y: attrs.y as number });
    if (
      p.x < -BADGE_CULL_PAD ||
      p.y < -BADGE_CULL_PAD ||
      p.x > viewport.width + BADGE_CULL_PAD ||
      p.y > viewport.height + BADGE_CULL_PAD
    )
      continue;
    const r = Math.min(((attrs.size as number) ?? 4) * pixelScale, 8);
    // Up and to the right of the disc, clear of the radial node label,
    // which sits on the side facing away from the cluster centre.
    const x = p.x + r * 0.8 + 2;
    const y = p.y - r * 0.8 - 3;
    const text = `+${count - shown}`;
    const width = text.length * 6;
    // Keep counts whole at the viewport edge; partial digits misstate totals.
    if (x < 6 || x + width > viewport.width - 6 || y - 7 < 6 || y + 7 > viewport.height - 6) continue;
    if (boxes.length >= 12 || boxes.some((box) => Math.abs(box.y - y) < 18 && x < box.x + box.width + 8 && x + width + 8 > box.x)) continue;
    boxes.push({ id, x, y, width, text });
  }
  return boxes;
}

/**
 * Paint the counts. Nothing is drawn while a hover is live (the hovered
 * inspector exposes its complete connections), when the zoom
 * already shows every memory, for a dim island node, or for an anchor off
 * screen. `project` maps graph coords to viewport CSS px.
 */
export function drawDustCounts(
  ctx: CanvasRenderingContext2D,
  graph: Graph,
  anchors: string[],
  project: (pos: { x: number; y: number }) => { x: number; y: number },
  palette: GraphPalette,
  lod: LodState,
  hovered: string | null,
  viewport: { width: number; height: number },
  pixelScale = 1,
  occupied: readonly { left: number; right: number; top: number; bottom: number }[] = [],
): void {
  const plan = planDustBadges(graph, anchors, project, lod, hovered, viewport, pixelScale);
  if (plan.length === 0) return;
  ctx.save();
  ctx.font = BADGE_FONT;
  ctx.textAlign = "left";
  ctx.textBaseline = "middle";
  ctx.lineJoin = "round";
  ctx.lineWidth = BADGE_HALO_WIDTH;
  ctx.strokeStyle = palette.surface;
  ctx.fillStyle = palette.label ?? palette.labelMuted;
  for (const { x, y, width, text } of plan) {
    if (occupied.some((box) => x < box.right && x + width > box.left && y - 7 < box.bottom && y + 7 > box.top)) continue;
    ctx.strokeText(text, x, y);
    ctx.fillText(text, x, y);
  }
  ctx.restore();
}
