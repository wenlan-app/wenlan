// SPDX-License-Identifier: AGPL-3.0-only
//
// Pure glue between the daemon's page-map payload and the radial layout in
// `./radial.ts`. Everything here is deliberately free of React and of
// @xyflow/react so the tricky parts (spine reconstruction, label resolution,
// placed-vs-computed positions) are testable without a renderer.

import { radialPolar, type MapNodeInput, type PlacedNode } from "./radial";
import type { PageMapNode } from "../tauri";

/** Box metrics. Kept in one place so layout and render can never disagree. */
export const NODE_HEIGHT = 44;
const NODE_MIN_WIDTH = 120;
const NODE_MAX_WIDTH = 260;
// Rough advance width for the 12px UI face. Measuring in the DOM would be
// exact, but it costs a synchronous reflow per node per relayout and the
// layout only needs boxes that *bound* the text. ponytail: estimate; swap for
// a canvas measureText pass if labels start visibly overflowing.
const CHAR_PX = 7.2;
const NODE_PADDING_X = 28;

// A suggestion carries an accept and a dismiss button inside the box. They took
// their room out of the label, so "Vector search" rendered as "Vector …" on
// every guess the daemon made — the one kind of box whose whole job is to be
// read and judged.
export const CONTROLS_PX = 48;

/**
 * Deterministic box size for a node, derived from its display label.
 *
 * `extra` is room for anything sharing the box with the label.
 */
export function nodeBoxSize(
  label: string,
  extra = 0,
): { width: number; height: number } {
  const raw = label.length * CHAR_PX + NODE_PADDING_X + extra;
  return {
    width: Math.min(NODE_MAX_WIDTH, Math.max(NODE_MIN_WIDTH, Math.round(raw))),
    height: NODE_HEIGHT,
  };
}

/**
 * Display text for a node. Precedence: the map-local `label` override, then a
 * caller-supplied resolution of the backing object (page title, memory
 * excerpt, entity name), then a last-resort placeholder.
 *
 * The daemon stores `label` only for `section` and wikilink-`page` nodes;
 * `memory`, `entity`, and the root arrive as null and MUST be resolved here
 * (see the spec's "Content ownership" — `NULL` = render from the backing
 * object). `overrides` is keyed `"{ref_kind}:{ref_id}"`.
 */
export function displayLabel(
  node: PageMapNode,
  overrides: ReadonlyMap<string, string>,
  fallback = "Untitled",
): string {
  if (node.label && node.label.trim()) return node.label.trim();
  const resolved = overrides.get(`${node.ref_kind}:${node.ref_id}`);
  if (resolved && resolved.trim()) return resolved.trim();
  return fallback;
}

export interface CanvasNodeView {
  node: PageMapNode;
  label: string;
  x: number;
  y: number;
  width: number;
  height: number;
  depth: number;
}

export type MapOrientation = "landscape" | "portrait";

/**
 * Reconstruct the parent_id spine into the nested shape flextree wants.
 *
 * Defensive on purpose — the daemon guarantees exactly one root and no
 * cycles, but a client that renders `undefined` on a contract slip is worse
 * than one that drops an unreachable branch. Nodes whose parent is missing
 * from the payload (or that sit on a cycle) are simply not reachable from the
 * root and get skipped; `dismissed` nodes are dropped before the walk since
 * the default GET hides them and a stray one must not create a gap in the
 * tree. Siblings are ordered by `rank`, ties broken by id for stability.
 */
export function buildSpine(
  nodes: readonly PageMapNode[],
  label: (n: PageMapNode) => string,
): { root: MapNodeInput; byId: Map<string, PageMapNode> } | null {
  const live = nodes.filter((n) => n.status !== "dismissed");
  const root = live.find((n) => n.parent_id === null);
  if (!root) return null;

  const byId = new Map(live.map((n) => [n.id, n]));
  const childrenOf = new Map<string, PageMapNode[]>();
  for (const n of live) {
    if (n.parent_id === null) continue;
    if (!byId.has(n.parent_id)) continue; // orphan: parent absent from payload
    const bucket = childrenOf.get(n.parent_id);
    if (bucket) bucket.push(n);
    else childrenOf.set(n.parent_id, [n]);
  }
  for (const bucket of childrenOf.values()) {
    bucket.sort((a, b) => a.rank - b.rank || a.id.localeCompare(b.id));
  }

  const seen = new Set<string>();
  const walk = (n: PageMapNode): MapNodeInput => {
    seen.add(n.id);
    const size = nodeBoxSize(
      label(n),
      n.status === "suggested" ? CONTROLS_PX : 0,
    );
    const kids = (childrenOf.get(n.id) ?? []).filter((c) => !seen.has(c.id));
    return {
      id: n.id,
      width: size.width,
      height: size.height,
      children: kids.map(walk),
    };
  };
  return { root: walk(root), byId };
}

/**
 * Final on-screen geometry for every reachable node.
 *
 * The radial layout always runs over the whole tree, then any node the user
 * has actually dragged (`placed`, with real coordinates) overrides its
 * computed slot. Running the layout for placed nodes too — rather than
 * laying out only the unplaced ones — is what keeps a half-arranged map
 * stable: unplaced siblings keep the ring they would have had, so accepting
 * one suggestion doesn't reshuffle everything the user already positioned.
 */
export function layoutMap(
  nodes: readonly PageMapNode[],
  overrides: ReadonlyMap<string, string>,
  fallback?: string,
  orientation: MapOrientation = "landscape",
): CanvasNodeView[] {
  const label = (n: PageMapNode) => displayLabel(n, overrides, fallback);
  const spine = buildSpine(nodes, label);
  if (!spine) return [];

  const hasPlacedCoordinates = nodes.some(
    (node) => node.status !== "dismissed" && node.placed && Number.isFinite(node.x) && Number.isFinite(node.y),
  );
  const usePortraitLayout = orientation === "portrait" && !hasPlacedCoordinates;
  const solverRoot = usePortraitLayout ? transpose(spine.root) : spine.root;

  let views: CanvasNodeView[];
  try {
    let placed: PlacedNode[] = radialPolar(solverRoot);
    if (usePortraitLayout) {
      placed = placed.map((node) => ({
        ...node,
        x: -node.y,
        y: node.x,
        width: node.height,
        height: node.width,
      }));
    }

    views = [];
    for (const p of placed) {
      const node = spine.byId.get(p.id);
      if (!node) continue;
      const pinnedPosition = node.placed && Number.isFinite(node.x) && Number.isFinite(node.y);
      views.push({
        node,
        label: label(node),
        x: pinnedPosition ? (node.x as number) : p.x,
        y: pinnedPosition ? (node.y as number) : p.y,
        width: node.width ?? p.width,
        height: node.height ?? p.height,
        depth: p.depth,
      });
    }
    if (usePortraitLayout) views = spreadPortraitCenters(views);
  } catch (error) {
    if (usePortraitLayout && error instanceof Error && error.message.startsWith("Portrait layout has coincident centers:")) {
      console.warn(error.message);
    }
    // Keep the existing recoverable empty layout if the solver or portrait
    // spacing fails instead of manufacturing overlaps or breaking navigation.
    return [];
  }
  return views;
}

/**
 * Radial's collision solver was tuned for a wide canvas. After rotating that
 * solution for a narrow portrait canvas, one uniform center scale restores a
 * 12px clearance on at least one axis for every pair without changing ranks,
 * box sizes, or the tree's shape.
 */
function spreadPortraitCenters(views: CanvasNodeView[]): CanvasNodeView[] {
  let scale = 1;
  for (let i = 0; i < views.length; i++) {
    const a = views[i];
    for (let j = i + 1; j < views.length; j++) {
      const b = views[j];
      const dx = Math.abs(a.x - b.x);
      const dy = Math.abs(a.y - b.y);
      if (dx === 0 && dy === 0) {
        throw new Error(`Portrait layout has coincident centers: ${a.node.id}, ${b.node.id}`);
      }
      const neededX = dx === 0 ? Number.POSITIVE_INFINITY : ((a.width + b.width) / 2 + 12) / dx;
      const neededY = dy === 0 ? Number.POSITIVE_INFINITY : ((a.height + b.height) / 2 + 12) / dy;
      scale = Math.max(scale, Math.min(neededX, neededY));
    }
  }
  if (scale === 1) return views;
  return views.map((view) => ({ ...view, x: view.x * scale, y: view.y * scale }));
}

function transpose(node: MapNodeInput): MapNodeInput {
  return {
    id: node.id,
    width: node.height,
    height: node.width,
    children: node.children?.map(transpose),
  };
}

/** Horizontal room between a box and its children, px. */
const TREE_GAP_X = 44;
/** Vertical room between sibling boxes, px. */
const TREE_GAP_Y = 10;
/**
 * Box width for a line of the page. Wider than `nodeBoxSize` allows, because
 * these labels were already shortened to one readable clause, and aware that a
 * CJK character takes about twice the room of a Latin one.
 */
export function lineBoxWidth(label: string, extra = 0): number {
  let px = 0;
  for (const ch of label) px += /[\u1100-\u115f\u2e80-\ua4cf\uac00-\ud7a3\uf900-\ufaff\ufe30-\ufe4f\uff00-\uff60]/u.test(ch) ? 12.5 : 6.9;
  return Math.min(340, Math.max(NODE_MIN_WIDTH, Math.round(px + NODE_PADDING_X + extra)));
}

/** Outline boxes are shorter than the radial map's: they are lines of a page. */
export const OUTLINE_NODE_HEIGHT = 32;

/**
 * Left-to-right tidy tree: the page on the left, its headings to the right,
 * each heading's items to the right of it, siblings stacked top to bottom in
 * reading order. This is the shape an outline already has, so the map reads
 * like the page instead of a ring that has to be decoded.
 *
 * Boxes the person dragged (`placed` with coordinates) keep their spot; every
 * other box takes its computed slot.
 */
export function layoutTree(
  nodes: readonly PageMapNode[],
  overrides: ReadonlyMap<string, string>,
  fallback?: string,
  heightOf: (n: PageMapNode) => number = () => NODE_HEIGHT,
  widthOf: (n: PageMapNode, label: string, base: number) => number = (_n, _l, base) => base,
): CanvasNodeView[] {
  const label = (n: PageMapNode) => displayLabel(n, overrides, fallback);
  const spine = buildSpine(nodes, label);
  if (!spine) return [];

  interface Box { id: string; width: number; height: number; children: Box[]; span: number }
  const toBox = (n: MapNodeInput): Box => {
    const node = spine.byId.get(n.id);
    const height = node ? heightOf(node) : n.height;
    const width = node ? widthOf(node, label(node), n.width) : n.width;
    return { id: n.id, width, height, children: (n.children ?? []).map(toBox), span: 0 };
  };
  const root = toBox(spine.root);
  const measure = (b: Box): number => {
    const kids = b.children.reduce((sum, k) => sum + measure(k), 0);
    const gaps = Math.max(0, b.children.length - 1) * TREE_GAP_Y;
    b.span = Math.max(b.height, kids + gaps);
    return b.span;
  };
  measure(root);

  const views: CanvasNodeView[] = [];
  const place = (b: Box, left: number, top: number, depth: number) => {
    const node = spine.byId.get(b.id);
    if (!node) return;
    const cx = left + b.width / 2;
    const cy = top + b.span / 2;
    const pinned = node.placed && Number.isFinite(node.x) && Number.isFinite(node.y);
    views.push({
      node,
      label: label(node),
      x: pinned ? (node.x as number) : cx,
      y: pinned ? (node.y as number) : cy,
      width: b.width,
      height: b.height,
      depth,
    });
    const kidsSpan =
      b.children.reduce((sum, k) => sum + k.span, 0) +
      Math.max(0, b.children.length - 1) * TREE_GAP_Y;
    let cursor = top + (b.span - kidsSpan) / 2;
    for (const kid of b.children) {
      place(kid, left + b.width + TREE_GAP_X, cursor, depth + 1);
      cursor += kid.span + TREE_GAP_Y;
    }
  };
  // The page box sits at the origin, as it does on the radial layout.
  place(root, -root.width / 2, -root.span / 2, 0);
  return views;
}

/** How far each level steps in on the indented outline, px. */
export const INDENT_STEP = 26;
const INDENT_GAP_Y = 8;

/**
 * Top-to-bottom indented outline, for a narrow panel: the page on the first
 * row, each heading one step in below it, each item one step in below its
 * heading. A side panel is a column, and a left-to-right tree in a column
 * either shrinks to unreadable or runs off both edges.
 */
export function layoutIndented(
  nodes: readonly PageMapNode[],
  overrides: ReadonlyMap<string, string>,
  fallback?: string,
  heightOf: (n: PageMapNode) => number = () => NODE_HEIGHT,
  widthOf: (n: PageMapNode, label: string, base: number) => number = (_n, _l, base) => base,
): CanvasNodeView[] {
  const label = (n: PageMapNode) => displayLabel(n, overrides, fallback);
  const spine = buildSpine(nodes, label);
  if (!spine) return [];
  const views: CanvasNodeView[] = [];
  let top = 0;
  const walk = (n: MapNodeInput, depth: number) => {
    const node = spine.byId.get(n.id);
    if (!node) return;
    const height = heightOf(node);
    const width = widthOf(node, label(node), n.width);
    const pinned = node.placed && Number.isFinite(node.x) && Number.isFinite(node.y);
    views.push({
      node,
      label: label(node),
      x: pinned ? (node.x as number) : depth * INDENT_STEP + width / 2,
      y: pinned ? (node.y as number) : top + height / 2,
      width,
      height,
      depth,
    });
    top += height + INDENT_GAP_Y;
    for (const kid of n.children ?? []) walk(kid, depth + 1);
  };
  walk(spine.root, 0);
  return views;
}
