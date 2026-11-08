// SPDX-License-Identifier: AGPL-3.0-only
import Graph from "graphology";
import { describe, expect, it } from "vitest";
import { markKeptLayout } from "./atlas";
import { runAtlasLayoutJob } from "./atlasLayoutJob";
import { REGION_ATTR, type RegionMapReport } from "./regionMap";

function fourGroups(): Graph {
  const g = new Graph({ type: "undirected", multi: true });
  for (const group of ["a", "b", "c", "d"]) {
    for (let i = 0; i < 12; i += 1) g.addNode(`${group}${i}`, { label: `${group}${i}`, entityType: "concept", size: 3, x: 0, y: 0 });
    for (let i = 0; i < 12; i += 1) for (let j = i + 1; j < 12; j += 1) if ((i + j) % 3 !== 0) g.addEdge(`${group}${i}`, `${group}${j}`, { edgeType: "related_to" });
  }
  g.addEdge("a0", "b0", { edgeType: "related_to" });
  g.addEdge("b1", "c1", { edgeType: "related_to" });
  return g;
}

describe("atlas layout worker job", () => {
  it("UX-GRAPH-1: sends back every body's region, so the map does not find the regions again", () => {
    const source = fourGroups();
    const reply = runAtlasLayoutJob({ graph: source.export(), variant: "r2" });
    expect(reply.positions).toHaveLength(48);
    expect(reply.regions).toHaveLength(48);

    // The map puts the spots back with those regions: no community search.
    const graph = fourGroups();
    for (const [id, x, y] of reply.positions) graph.mergeNodeAttributes(id, { x, y });
    const regions = new Map(reply.regions);
    markKeptLayout(graph, "r2", "", regions);
    const report = graph.getAttribute("atlasRegionReport") as RegionMapReport;
    expect(report.timesMs.louvain).toBeUndefined();
    graph.forEachNode((id, attrs) => expect(attrs[REGION_ATTR]).toBe(regions.get(id)));

    // Without them, the map would search for communities again.
    const again = fourGroups();
    markKeptLayout(again, "r2", "", undefined);
    expect((again.getAttribute("atlasRegionReport") as RegionMapReport).timesMs.louvain).toBeDefined();
  });
});
