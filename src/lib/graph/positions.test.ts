// SPDX-License-Identifier: AGPL-3.0-only
import { beforeEach, describe, expect, it } from "vitest";
import Graph from "graphology";
import {
  canUndoRearrange,
  decodePositions,
  encodePositions,
  readStoredPositions,
  startRearrange,
  storePositions,
  undoRearrange,
} from "./positions";
import { placeNewBesideKept } from "./atlas";

describe("kept map layout (UX-GRAPH-1)", () => {
  beforeEach(() => window.localStorage.clear());

  it("round-trips positions and treats damaged data as none", () => {
    const positions = new Map([["p1", { x: 1.234567, y: -2 }], ["p2", { x: Number.NaN, y: 0 }]]);
    expect(decodePositions(encodePositions(positions))).toEqual(new Map([["p1", { x: 1.23, y: -2 }]]));
    expect(decodePositions("{not json")).toEqual(new Map());
    expect(decodePositions(JSON.stringify({ p1: [1], p2: ["a", 2], p3: [3, 4] }))).toEqual(new Map([["p3", { x: 3, y: 4 }]]));
  });

  it("keeps a layout per scope, and a rearrangement can be taken back once", () => {
    storePositions(null, new Map([["p1", { x: 1, y: 2 }]]));
    storePositions("work", new Map([["p9", { x: 9, y: 9 }]]));
    expect(readStoredPositions(null).get("p1")).toEqual({ x: 1, y: 2 });
    expect(readStoredPositions("work").has("p1")).toBe(false);

    startRearrange(null);
    expect(readStoredPositions(null).size).toBe(0);
    expect(canUndoRearrange(null)).toBe(true);
    undoRearrange(null);
    expect(readStoredPositions(null).get("p1")).toEqual({ x: 1, y: 2 });
    expect(canUndoRearrange(null)).toBe(false);
    expect(readStoredPositions("work").get("p9")).toEqual({ x: 9, y: 9 });
  });

  it("puts a new point beside the kept points it links to, the same way every time", () => {
    const build = () => {
      const graph = new Graph({ multi: true });
      graph.addNode("hub", { x: 100, y: 100, size: 4, entityType: "page" });
      graph.addNode("old", { x: 140, y: 100, size: 2, entityType: "page" });
      graph.addNode("new", { x: -900, y: 900, size: 2, entityType: "page" });
      graph.addNode("newer", { x: -900, y: -900, size: 2, entityType: "page" });
      graph.addEdge("hub", "old");
      graph.addEdge("hub", "new");
      graph.addEdge("new", "newer");
      placeNewBesideKept(graph, new Set(["hub", "old"]));
      return graph;
    };
    const graph = build();
    expect(graph.getNodeAttributes("hub")).toMatchObject({ x: 100, y: 100 });
    expect(graph.getNodeAttributes("old")).toMatchObject({ x: 140, y: 100 });
    const near = (a: string, b: string) => Math.hypot(
      graph.getNodeAttribute(a, "x") - graph.getNodeAttribute(b, "x"),
      graph.getNodeAttribute(a, "y") - graph.getNodeAttribute(b, "y"),
    );
    expect(near("new", "hub")).toBeLessThan(20);
    expect(near("newer", "new")).toBeLessThan(20);
    const again = build();
    expect(again.getNodeAttributes("new")).toEqual(graph.getNodeAttributes("new"));
  });
});
