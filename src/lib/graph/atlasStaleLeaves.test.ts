// SPDX-License-Identifier: AGPL-3.0-only
import Graph from "graphology";
import { describe, expect, it } from "vitest";
import { islandSolid, lodFor, STALE_HUB_DEGREE, STALE_LEAF_DISTANCE, staleKeptGroups } from "./atlas";

function hubWithLeaves() {
  const graph = new Graph({ type: "undirected" });
  graph.addNode("hub", { entityType: "concept", x: 0, y: 0, size: 6 });
  const leaves = ["a", "b", "c", "d", "e"];
  for (const id of leaves) graph.addNode(id, { entityType: "concept", x: 0, y: 0, size: 1 });
  for (const id of leaves) graph.addEdge("hub", id);
  for (const id of ["far", "pair1", "pair2"]) graph.addNode(id, { entityType: "concept", x: 0, y: 0, size: 1 });
  graph.addEdge("hub", "far");
  graph.addEdge("hub", "pair1");
  graph.addEdge("pair1", "pair2");
  graph.addNode("note", { entityType: "memory", x: 0, y: 0, size: 1 });
  graph.addEdge("far", "note");
  return graph;
}

const kept = new Map([
  ["hub", { x: 0, y: 0 }],
  ["a", { x: 30, y: 0 }],
  ["b", { x: -30, y: 0 }],
  ["c", { x: 0, y: 30 }],
  ["d", { x: 0, y: -30 }],
  ["e", { x: 20, y: 20 }],
  ["far", { x: 900, y: 600 }],
  ["pair1", { x: -1500, y: 200 }],
  ["pair2", { x: -1540, y: 230 }],
]);

describe("UX-GRAPH-1 staleKeptGroups: small far groups move to the edge", () => {
  it("moves only the small far groups, as whole groups", () => {
    expect([...staleKeptGroups(hubWithLeaves(), kept).keys()].sort()).toEqual(["far", "pair1", "pair2"]);
  });

  it("never moves a hub, however far its links are", () => {
    const far = new Map([...kept].map(([id, p]) => [id, id === "hub" ? { x: 5000, y: 5000 } : p]));
    expect(staleKeptGroups(hubWithLeaves(), far).has("hub")).toBe(false);
  });

  it("does not move a point whose links are not kept", () => {
    expect(staleKeptGroups(hubWithLeaves(), new Map([["far", { x: 900, y: 600 }]])).size).toBe(0);
  });

  it("keeps a group's shape, lands it beside its link, and leaves the rest", () => {
    const moves = staleKeptGroups(hubWithLeaves(), kept);
    const p1 = moves.get("pair1")!;
    const p2 = moves.get("pair2")!;
    expect(p2.x - p1.x).toBeCloseTo(-40);
    expect(p2.y - p1.y).toBeCloseTo(30);
    for (const id of ["far", "pair1"]) {
      expect(Math.hypot(moves.get(id)!.x, moves.get(id)!.y)).toBeLessThan(STALE_LEAF_DISTANCE);
    }
    expect(Math.hypot(moves.get("far")!.x - p1.x, moves.get("far")!.y - p1.y)).toBeGreaterThan(10);
    for (const id of ["hub", "a", "b", "c", "d", "e"]) expect(moves.has(id)).toBe(false);
  });

  it("is one-time: a second restore moves nothing", () => {
    const graph = hubWithLeaves();
    const next = new Map(kept);
    for (const [id, p] of staleKeptGroups(graph, kept)) next.set(id, p);
    expect(staleKeptGroups(graph, next).size).toBe(0);
  });

  it("moves a chain of far groups in one restore, so the next moves nothing", () => {
    const graph = hubWithLeaves();
    graph.addNode("tail", { entityType: "concept", x: 0, y: 0, size: 1 });
    graph.addEdge("far", "tail");
    const positions = new Map(kept);
    positions.set("tail", { x: 3000, y: -2000 });
    const moves = staleKeptGroups(graph, positions);
    expect(moves.has("tail")).toBe(true);
    const next = new Map(positions);
    for (const [id, p] of moves) next.set(id, p);
    expect(staleKeptGroups(graph, next).size).toBe(0);
  });

  it("leaves two small groups joined by a long link where they are", () => {
    const graph = new Graph({ type: "undirected" });
    for (const id of ["x", "y"]) graph.addNode(id, { entityType: "concept", x: 0, y: 0, size: 1 });
    graph.addEdge("x", "y");
    const positions = new Map([["x", { x: 0, y: 0 }], ["y", { x: 2000, y: 0 }]]);
    expect(staleKeptGroups(graph, positions).size).toBe(0);
  });

  it("does not move a group holding a hub", () => {
    const graph = hubWithLeaves();
    const positions = new Map(kept);
    for (let i = 0; i < STALE_HUB_DEGREE; i += 1) {
      graph.addNode("s" + i, { entityType: "concept", x: 0, y: 0, size: 1 });
      graph.addEdge("far", "s" + i);
      positions.set("s" + i, { x: 900 + i * 5, y: 640 });
    }
    expect(staleKeptGroups(graph, positions).has("far")).toBe(false);
  });
});

describe("UX-GRAPH-1 islandSolid: islands at the edge come up by zoom", () => {
  it("brings a small island up at the detail zoom, a larger one at 2x", () => {
    const small = { island: true, islandBodies: 2 };
    const large = { island: true, islandBodies: 6 };
    expect(islandSolid(small, lodFor(2))).toBe(false);
    expect(islandSolid(large, lodFor(2))).toBe(true);
    expect(islandSolid(small, lodFor(4))).toBe(true);
    expect(islandSolid({ island: false }, lodFor(1))).toBe(true);
  });
});
