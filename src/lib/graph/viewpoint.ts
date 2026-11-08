// SPDX-License-Identifier: AGPL-3.0-only
import type Graph from "graphology";
import type Sigma from "sigma";
import { MEMORY_NODE_TYPE, PAGE_NODE_TYPE } from "./model";

export interface AtlasViewpoint {
  density: number;
  angle: number;
  anchors: { id: string; x: number; y: number }[];
}

export type AtlasFrameMode = "main" | "all" | "core";

export interface AtlasFrameOptions {
  mode?: AtlasFrameMode;
  /** IDs that are currently visible after view filters have been applied. */
  visibleNodeIds?: ReadonlySet<string>;
  /** Extra CSS pixels kept around the framed nodes. */
  padding?: number;
  /** Frame exactly these nodes instead of a mode's pick (a thing's zoom
   *  stops, UX-CHAT-5). */
  nodeIds?: readonly string[];
  /** Per-side room in CSS px, each side at least `padding`: the map's own
   *  controls sit over the canvas, so the top keeps the toolbar's band free
   *  and the bottom the camera controls'. */
  insets?: Partial<AtlasFrameInsets>;
  /** Move there over this many ms instead of at once. */
  duration?: number;
  /** Never closer than this many CSS px per graph unit: a handful of points
   *  framed alone would otherwise fill the screen with empty space. */
  maxDensity?: number;
}

export interface AtlasFrameInsets {
  top: number;
  right: number;
  bottom: number;
  left: number;
}

/** What the controls laid over the map take, CSS px: the search and back
 *  button band at the top (24px down, 36px tall), the count line and camera
 *  controls at the bottom. The opening view and every stop keep it free. */
export const ATLAS_CONTROL_INSETS: AtlasFrameInsets = { top: 84, right: 56, bottom: 60, left: 56 };

const DEFAULT_FRAME_PADDING = 56;

function isVisible(id: string, visibleNodeIds?: ReadonlySet<string>): boolean {
  return visibleNodeIds === undefined || visibleNodeIds.has(id);
}

/**
 * Pick the nodes that define an overview frame. The hierarchy tags are
 * intentionally the source of truth here: a component represented by a
 * labelled landmark is a meaningful part of the network. If no labelled
 * landmark survives (for example a legacy or memory-only fixture), a
 * non-trivial connected component is the narrowest useful fallback.
 */
export function atlasFrameNodeIds(
  graph: Graph,
  options: AtlasFrameOptions = {},
): string[] {
  const visible = graph.nodes().filter((id) => isVisible(id, options.visibleNodeIds));
  if (options.mode === "all") return visible;
  if (options.mode === "core") {
    // The core component alone (round 3): the islands packed round it are
    // left to the edges, so the core fills the opening view.
    const core = visible.filter((id) => {
      const attrs = graph.getNodeAttributes(id);
      return attrs.entityType !== MEMORY_NODE_TYPE && attrs.island === false;
    });
    if (core.length > 0) return core;
  }

  const structural = visible.filter(
    (id) => graph.getNodeAttribute(id, "entityType") !== MEMORY_NODE_TYPE,
  );
  const representedComponents = new Set<string>();
  for (const id of structural) {
    const attrs = graph.getNodeAttributes(id);
    if (attrs.landmarkLabel === true) {
      const componentId = attrs.componentId as string | undefined;
      if (componentId) representedComponents.add(componentId);
    }
  }
  if (representedComponents.size === 0) {
    const componentSizes = new Map<string, number>();
    for (const id of structural) {
      const componentId = graph.getNodeAttribute(id, "componentId") as string | undefined;
      if (componentId) componentSizes.set(componentId, (componentSizes.get(componentId) ?? 0) + 1);
    }
    for (const [componentId, size] of componentSizes) {
      if (size > 1) representedComponents.add(componentId);
    }
  }
  if (representedComponents.size === 0) return visible;

  const main = structural.filter((id) => {
    const componentId = graph.getNodeAttribute(id, "componentId") as string | undefined;
    return componentId !== undefined && representedComponents.has(componentId);
  });
  // What the person is working on opens in view: every thing's points
  // (UX-CHAT-5), whatever component their hidden centre sits in.
  const work = visible.filter((id) => graph.getNodeAttribute(id, "night") === true);
  return main.length > 0 ? [...main, ...work] : visible;
}

function graphCorrectionRatio(
  viewportWidth: number,
  viewportHeight: number,
  graphWidth: number,
  graphHeight: number,
): number {
  const viewportRatio = viewportHeight / Math.max(viewportWidth, 1);
  const graphRatio = graphHeight / Math.max(graphWidth, 1);
  if (
    (viewportRatio < 1 && graphRatio > 1) ||
    (viewportRatio > 1 && graphRatio < 1)
  ) return 1;
  return Math.min(
    Math.max(graphRatio, 1 / graphRatio),
    Math.max(1 / viewportRatio, viewportRatio),
  );
}

/**
 * Fit a Sigma camera around a selected subset while preserving the current
 * camera angle. Sigma's camera coordinates are in its normalized graph
 * space, so `getNodeDisplayData` supplies the right coordinates even when
 * the raw graph bounds change between model rebuilds.
 */
export function frameAtlasView(
  renderer: Sigma,
  graph: Graph,
  options: AtlasFrameOptions = {},
): boolean {
  const ids = options.nodeIds ? options.nodeIds.filter((id) => graph.hasNode(id)) : atlasFrameNodeIds(graph, options);
  if (ids.length === 0) return false;

  const points = ids.flatMap((id) => {
    const point = renderer.getNodeDisplayData(id);
    return point !== undefined && Number.isFinite(point.x) && Number.isFinite(point.y)
      ? [{ x: point.x, y: point.y }]
      : [];
  });
  if (points.length === 0) return false;

  const camera = renderer.getCamera();
  const angle = camera.angle;
  const cos = Math.cos(angle);
  const sin = Math.sin(angle);
  let minX = Infinity;
  let maxX = -Infinity;
  let minY = Infinity;
  let maxY = -Infinity;
  for (const point of points) {
    // Sigma rotates the framed graph by -angle before mapping it to screen.
    const x = cos * point.x + sin * point.y;
    const y = -sin * point.x + cos * point.y;
    minX = Math.min(minX, x);
    maxX = Math.max(maxX, x);
    minY = Math.min(minY, y);
    maxY = Math.max(maxY, y);
  }

  const rotatedCenterX = (minX + maxX) / 2;
  const rotatedCenterY = (minY + maxY) / 2;
  const center = {
    x: cos * rotatedCenterX - sin * rotatedCenterY,
    y: sin * rotatedCenterX + cos * rotatedCenterY,
  };
  const width = Math.max(1, renderer.getDimensions().width);
  const height = Math.max(1, renderer.getDimensions().height);
  const padding = Math.max(0, options.padding ?? DEFAULT_FRAME_PADDING);
  const side = (key: keyof AtlasFrameInsets) => Math.max(padding, options.insets?.[key] ?? 0);
  const insets = { top: side("top"), right: side("right"), bottom: side("bottom"), left: side("left") };
  const availableWidth = Math.max(1, width - insets.left - insets.right);
  const availableHeight = Math.max(1, height - insets.top - insets.bottom);
  const stagePadding = Number(renderer.getSetting?.("stagePadding") ?? 0);
  const smallest = Math.max(1, Math.min(width, height) - 2 * stagePadding);
  const graphDimensions = (renderer.getGraphDimensions?.() ?? renderer.getBBox()) as {
    width?: number;
    height?: number;
    x?: [number, number];
    y?: [number, number];
  };
  const graphWidth = graphDimensions.width
    ?? ((graphDimensions.x?.[1] ?? 1) - (graphDimensions.x?.[0] ?? 0));
  const graphHeight = graphDimensions.height
    ?? ((graphDimensions.y?.[1] ?? 1) - (graphDimensions.y?.[0] ?? 0));
  const correction = graphCorrectionRatio(
    width,
    height,
    Math.max(1e-9, graphWidth),
    Math.max(1e-9, graphHeight),
  );
  const ratio = Math.max(
    0.05,
    (Math.max(maxX - minX, 1e-9) * smallest * correction) / availableWidth,
    (Math.max(maxY - minY, 1e-9) * smallest * correction) / availableHeight,
  );
  const target = { x: center.x, y: center.y, ratio: camera.getBoundedRatio(ratio), angle };
  if (options.maxDensity && typeof renderer.graphToViewport === "function") {
    const origin = renderer.graphToViewport({ x: 0, y: 0 }, { cameraState: target });
    const unit = renderer.graphToViewport({ x: 1, y: 0 }, { cameraState: target });
    const close = Math.hypot(unit.x - origin.x, unit.y - origin.y);
    if (Number.isFinite(close) && close > options.maxDensity) {
      target.ratio = camera.getBoundedRatio(target.ratio * (close / options.maxDensity));
    }
  }
  // Unequal insets: the framed box's middle goes to the middle of the room
  // left between them, not of the canvas. Sigma maps viewport to graph
  // linearly for a given camera, so the shift is read off that map.
  const wanted = { x: (insets.left + width - insets.right) / 2, y: (insets.top + height - insets.bottom) / 2 };
  // Only on a canvas with room for the insets: a canvas not laid out yet
  // (0 x 0) has no meaningful middle, and its projection is not finite.
  const roomy = width > insets.left + insets.right && height > insets.top + insets.bottom;
  if (roomy && (wanted.x !== width / 2 || wanted.y !== height / 2) && typeof renderer.viewportToFramedGraph === "function") {
    const landed = renderer.viewportToFramedGraph(wanted, { cameraState: target });
    if (Number.isFinite(landed.x) && Number.isFinite(landed.y)) {
      target.x = 2 * target.x - landed.x;
      target.y = 2 * target.y - landed.y;
    }
  }
  if (options.duration && options.duration > 0) void camera.animate(target, { duration: options.duration });
  else camera.setState(target);
  return true;
}

/** Physical scale, independent of Sigma's model-dependent normalization. */
function density(renderer: Sigma): number {
  const origin = renderer.graphToViewport({ x: 0, y: 0 });
  const unit = renderer.graphToViewport({ x: 1, y: 0 });
  return Math.hypot(unit.x - origin.x, unit.y - origin.y);
}

/** Keep several nearby anchors so removing a layer can retain another one. */
export function captureAtlasViewpoint(renderer: Sigma, graph: Graph): AtlasViewpoint {
  const { width, height } = renderer.getDimensions();
  const candidates = graph.nodes().map((id) => {
    const point = renderer.graphToViewport(graph.getNodeAttributes(id) as { x: number; y: number });
    return { id, x: point.x / width, y: point.y / height,
      kind: graph.getNodeAttribute(id, "entityType") === MEMORY_NODE_TYPE ? "memory"
        : graph.getNodeAttribute(id, "entityType") === PAGE_NODE_TYPE ? "page" : "entity" };
  });
  candidates.sort((a, b) => Math.hypot(a.x - 0.5, a.y - 0.5) - Math.hypot(b.x - 0.5, b.y - 0.5));
  // Retain candidates from both families, including memory-only views.
  const anchors = [
    ...candidates.filter((node) => node.kind === "entity").slice(0, 8),
    ...candidates.filter((node) => node.kind === "page").slice(0, 8),
    ...candidates.filter((node) => node.kind === "memory").slice(0, 8),
  ].map(({ id, x, y }) => ({ id, x, y }));
  return { density: density(renderer), angle: renderer.getCamera().angle, anchors };
}

/** Retain a surviving node's screen position and graph-unit scale after refit. */
export function restoreAtlasViewpoint(renderer: Sigma, graph: Graph, saved: AtlasViewpoint): boolean {
  const anchor = saved.anchors.find(({ id }) => graph.hasNode(id));
  if (!anchor || !Number.isFinite(saved.density) || saved.density <= 0) return false;
  const currentDensity = density(renderer);
  if (!Number.isFinite(currentDensity) || currentDensity <= 0) return false;
  const camera = renderer.getCamera();
  camera.setState({ ratio: camera.getBoundedRatio(camera.ratio * currentDensity / saved.density), angle: saved.angle });
  const { width, height } = renderer.getDimensions();
  const target = renderer.viewportToFramedGraph({ x: anchor.x * width, y: anchor.y * height });
  const actual = renderer.getNodeDisplayData(anchor.id);
  if (!actual) return false;
  camera.setState({ x: camera.x + actual.x - target.x, y: camera.y + actual.y - target.y });
  return true;
}
