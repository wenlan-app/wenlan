// SPDX-License-Identifier: AGPL-3.0-only
// The work the layout worker does, kept apart from the worker so it can be
// tested: the same fresh layout and settle as the main thread would run,
// returning every body's spot and, for R2, every body's region.
import Graph from "graphology";
import { ATLAS_REGION_REPORT_ATTR, createAtlasSimulation, runAtlasLayout, type AtlasLayoutVariant } from "./atlas";
import { REGION_ATTR } from "./regionMap";
import { MEMORY_NODE_TYPE } from "./model";

export type AtlasLayoutRequest = { graph: ReturnType<Graph["export"]>; variant: AtlasLayoutVariant; scope?: string; regions?: [string, string][] };
/** `regions`: each body's R2 region, so the map does not work them out again. */
export type AtlasLayoutReply = { positions: [string, number, number][]; regions?: [string, string][]; report?: unknown };

export function runAtlasLayoutJob(request: AtlasLayoutRequest): AtlasLayoutReply {
  const graph = Graph.from(request.graph);
  runAtlasLayout(graph, request.variant, request.scope ?? "", request.regions ? new Map(request.regions) : undefined);
  createAtlasSimulation(graph).stop();
  const positions: [string, number, number][] = [];
  const regions: [string, string][] = [];
  graph.forEachNode((id, attrs) => {
    if (attrs.entityType !== MEMORY_NODE_TYPE) positions.push([id, attrs.x as number, attrs.y as number]);
    if (typeof attrs[REGION_ATTR] === "string") regions.push([id, attrs[REGION_ATTR] as string]);
  });
  return { positions, regions, report: graph.getAttribute(ATLAS_REGION_REPORT_ATTR) };
}
