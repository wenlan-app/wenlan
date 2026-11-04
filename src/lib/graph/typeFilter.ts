// SPDX-License-Identifier: AGPL-3.0-only
import type { GraphModel } from "./model";
import { MEMORY_NODE_TYPE, PAGE_NODE_TYPE, THING_NODE_TYPE } from "./model";

/** Readable fallback without changing stored types or their filter identity. */
export function entityTypeLabel(type: string): string {
  const label = type.replace(/[_-]+/g, " ").trim();
  return label.charAt(0).toUpperCase() + label.slice(1);
}

export function entityTypeHidden(entityType: string, excluded: ReadonlySet<string>): boolean {
  return entityType !== MEMORY_NODE_TYPE && entityType !== PAGE_NODE_TYPE && entityType !== THING_NODE_TYPE && excluded.has(entityType);
}

/** Filter presentation only: retain the model's original positions and degrees. */
export function filterGraphEntityTypes(model: GraphModel, excluded: ReadonlySet<string>): GraphModel {
  if (!excluded.size) return model;
  const nodes = model.nodes.filter((node) => node.kind !== "entity" || !excluded.has(node.entityType));
  const ids = new Set(nodes.map((node) => node.id));
  return { ...model, nodes, edges: model.edges.filter((edge) => ids.has(edge.source) && ids.has(edge.target)) };
}
