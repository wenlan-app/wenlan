// SPDX-License-Identifier: AGPL-3.0-only
// Lays a large map out off the main thread, so the window stays usable while
// a first open or a re-balance takes a few seconds. It runs the same fresh
// layout and settle as the main thread would and sends back every body's
// spot; the map then places those spots the way it places a kept layout.
import Graph from "graphology";
import { ATLAS_REGION_REPORT_ATTR, createAtlasSimulation, runAtlasLayout, type AtlasLayoutVariant } from "./atlas";
import { MEMORY_NODE_TYPE } from "./model";

export type AtlasLayoutRequest = { graph: ReturnType<Graph["export"]>; variant: AtlasLayoutVariant; scope?: string; regions?: [string, string][] };
export type AtlasLayoutReply = { positions: [string, number, number][]; report?: unknown };

self.onmessage = (event: MessageEvent<AtlasLayoutRequest>) => {
  const graph = Graph.from(event.data.graph);
  runAtlasLayout(graph, event.data.variant, event.data.scope ?? "", event.data.regions ? new Map(event.data.regions) : undefined);
  createAtlasSimulation(graph).stop();
  const positions: [string, number, number][] = [];
  graph.forEachNode((id, attrs) => {
    if (attrs.entityType !== MEMORY_NODE_TYPE) positions.push([id, attrs.x as number, attrs.y as number]);
  });
  (self as unknown as Worker).postMessage({ positions, report: graph.getAttribute(ATLAS_REGION_REPORT_ATTR) } satisfies AtlasLayoutReply);
};
