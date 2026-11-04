// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import Graph from "graphology";
import type Sigma from "sigma";
import { atlasFrameNodeIds, captureAtlasViewpoint, frameAtlasView, restoreAtlasViewpoint } from "./viewpoint";
import { PAGE_NODE_TYPE, MEMORY_NODE_TYPE } from "./model";

function projection(graph: Graph, normalization: number, offset: number) {
  const camera = {
    x: 0.4, y: 0.6, angle: 0, ratio: 0.3,
    getBoundedRatio: (ratio: number) => ratio,
    setState(state: object) { Object.assign(camera, state); },
  };
  const display = (id: string) => {
    const { x, y } = graph.getNodeAttributes(id);
    return { x: x * normalization + offset, y: y * normalization + offset };
  };
  const renderer = {
    getCamera: () => camera,
    getDimensions: () => ({ width: 1000, height: 600 }),
    getNodeDisplayData: display,
    graphToViewport: ({ x, y }: { x: number; y: number }) => ({
      x: 500 + (x * normalization + offset - camera.x) * 1000 / camera.ratio,
      y: 300 - (y * normalization + offset - camera.y) * 1000 / camera.ratio,
    }),
    viewportToFramedGraph: ({ x, y }: { x: number; y: number }) => ({
      x: camera.x + (x - 500) * camera.ratio / 1000,
      y: camera.y - (y - 300) * camera.ratio / 1000,
    }),
  };
  return renderer as unknown as Sigma;
}

describe("Atlas viewpoint across model rebuilds", () => {
  it("frames all nodes or only hierarchy-labelled components", () => {
    const graph = new Graph();
    graph.addNode("core", { x: 0, y: 0, entityType: "project", componentId: "core", landmark: true, landmarkLabel: true });
    graph.addNode("core-leaf", { x: 1, y: 0, entityType: "project", componentId: "core", landmark: false, landmarkLabel: false });
    graph.addNode("wing", { x: 80, y: 0, entityType: "project", componentId: "wing", landmark: true, landmarkLabel: false });
    graph.addNode("memory", { x: 80, y: 1, entityType: MEMORY_NODE_TYPE });
    const visible = new Set(graph.nodes());

    expect(atlasFrameNodeIds(graph, { mode: "main", visibleNodeIds: visible })).toEqual(["core", "core-leaf"]);
    expect(atlasFrameNodeIds(graph, { mode: "all", visibleNodeIds: visible })).toEqual(graph.nodes());
  });

  it("fits a selected hierarchy subset with padding and keeps the camera angle", () => {
    const graph = new Graph();
    graph.addNode("a", { x: 0, y: 0, entityType: "project", componentId: "main", landmarkLabel: true });
    graph.addNode("b", { x: 10, y: 0, entityType: "project", componentId: "main", landmarkLabel: false });
    const camera = {
      x: 0.5, y: 0.5, angle: Math.PI / 4, ratio: 1,
      getBoundedRatio: (ratio: number) => ratio,
      setState(state: object) { Object.assign(camera, state); },
    };
    const renderer = {
      getCamera: () => camera,
      getDimensions: () => ({ width: 1000, height: 600 }),
      getNodeDisplayData: (id: string) => ({ x: id === "a" ? 0.25 : 0.75, y: 0.5 }),
      getGraphDimensions: () => ({ width: 10, height: 10 }),
      getSetting: (key: string) => key === "stagePadding" ? 40 : undefined,
    };
    expect(frameAtlasView(renderer as unknown as Sigma, graph, { mode: "main", padding: 56 })).toBe(true);
    expect(camera.angle).toBeCloseTo(Math.PI / 4);
    expect(camera.x).toBeCloseTo(0.5);
    expect(camera.y).toBeCloseTo(0.5);
    expect(camera.ratio).toBeGreaterThan(0);
  });

  it("preserves screen position and physical scale when bounds and graph origin change", () => {
    const before = new Graph();
    before.addNode("a", { x: 3, y: 5, entityType: "project" });
    before.addNode("b", { x: 5, y: 8, entityType: "technology" });
    const oldRenderer = projection(before, 0.01, 0.4);
    const saved = captureAtlasViewpoint(oldRenderer, before);
    const after = new Graph();
    after.addNode("a", { x: 103, y: 30, entityType: "project" });
    after.addNode("b", { x: 105, y: 33, entityType: "technology" });
    after.addNode("distant-memory", { x: 10000, y: 5000, entityType: MEMORY_NODE_TYPE });
    const newRenderer = projection(after, 0.0001, 0.1);
    expect(restoreAtlasViewpoint(newRenderer, after, saved)).toBe(true);
    for (const id of ["a", "b"]) {
      const oldPoint = oldRenderer.graphToViewport(before.getNodeAttributes(id) as { x: number; y: number });
      const newPoint = newRenderer.graphToViewport(after.getNodeAttributes(id) as { x: number; y: number });
      expect(newPoint.x).toBeCloseTo(oldPoint.x, 5);
      expect(newPoint.y).toBeCloseTo(oldPoint.y, 5);
    }
  });

  it("keeps a page anchor available when the entity layer is removed", () => {
    const graph = new Graph();
    for (let i = 0; i < 50; i++) graph.addNode(`e${i}`, { x: i / 100, y: 0, entityType: "project" });
    graph.addNode("page", { x: 12, y: 8, entityType: PAGE_NODE_TYPE });
    const saved = captureAtlasViewpoint(projection(graph, 0.01, 0.4), graph);
    const pagesOnly = new Graph();
    pagesOnly.addNode("page", { x: 0, y: 0, entityType: PAGE_NODE_TYPE });
    expect(restoreAtlasViewpoint(projection(pagesOnly, 0.2, 0.5), pagesOnly, saved)).toBe(true);
  });

  it("leaves the default fit intact when no prior anchor survives", () => {
    const graph = new Graph();
    graph.addNode("new", { x: 0, y: 0, entityType: "project" });
    const renderer = projection(graph, 0.01, 0.4);
    expect(restoreAtlasViewpoint(renderer, graph, { density: 3, angle: 0, anchors: [{ id: "gone", x: 0.5, y: 0.5 }] })).toBe(false);
    expect(renderer.getCamera().ratio).toBe(0.3);
  });

  it("keeps the toolbar's band free and frames exactly the nodes a thing's stop names (UX-CHAT-5)", () => {
    const graph = new Graph();
    graph.addNode("a", { x: 0, y: 0, entityType: MEMORY_NODE_TYPE, night: true });
    graph.addNode("b", { x: 1, y: 0, entityType: MEMORY_NODE_TYPE, night: true });
    graph.addNode("far", { x: 9, y: 9, entityType: "project" });
    const spot: Record<string, { x: number; y: number }> = { a: { x: 0.4, y: 0.5 }, b: { x: 0.6, y: 0.52 }, far: { x: 0.9, y: 0.9 } };
    type State = { x: number; y: number; ratio: number; angle: number };
    const animated: State[] = [];
    const camera = {
      x: 0.5, y: 0.5, angle: 0, ratio: 1,
      getBoundedRatio: (ratio: number) => ratio,
      setState(state: Partial<State>) { Object.assign(camera, state); },
      animate(state: State) { animated.push(state); Object.assign(camera, state); return Promise.resolve(); },
    };
    // 600 px of screen per framed unit at ratio 1; screen y runs down.
    const toViewport = (point: { x: number; y: number }, state: State = camera) => ({
      x: 500 + (point.x - state.x) * 600 / state.ratio,
      y: 300 - (point.y - state.y) * 600 / state.ratio,
    });
    const renderer = {
      getCamera: () => camera,
      getDimensions: () => ({ width: 1000, height: 600 }),
      getNodeDisplayData: (id: string) => spot[id],
      getGraphDimensions: () => ({ width: 10, height: 10 }),
      getSetting: () => 0,
      viewportToFramedGraph: (v: { x: number; y: number }, override?: { cameraState?: State }) => {
        const state = override?.cameraState ?? camera;
        return { x: state.x + (v.x - 500) * state.ratio / 600, y: state.y - (v.y - 300) * state.ratio / 600 };
      },
    };
    const insets = { top: 120, right: 56, bottom: 60, left: 56 };
    expect(frameAtlasView(renderer as unknown as Sigma, graph, { nodeIds: ["a", "b"], insets, padding: 40 })).toBe(true);
    const a = toViewport(spot.a!);
    const b = toViewport(spot.b!);
    // The pair sits in the middle of the room the insets leave...
    expect((a.x + b.x) / 2).toBeCloseTo((56 + 1000 - 56) / 2, 6);
    expect((a.y + b.y) / 2).toBeCloseTo((120 + 600 - 60) / 2, 6);
    // ...filling its width, and nothing is under the toolbar's band.
    expect(Math.min(a.x, b.x)).toBeCloseTo(56, 6);
    expect(Math.min(a.y, b.y)).toBeGreaterThanOrEqual(120);
    // The distant node is not part of this frame.
    expect(toViewport(spot.far!).x).toBeGreaterThan(1000);
    // A stop moves there over time when asked to.
    frameAtlasView(renderer as unknown as Sigma, graph, { nodeIds: ["a", "b", "far"], insets, duration: 450 });
    expect(animated).toHaveLength(1);
  });
});
