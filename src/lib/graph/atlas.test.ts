// SPDX-License-Identifier: AGPL-3.0-only
import { describe, it, expect, vi } from "vitest";
import type Graph from "graphology";
import type { ForceLink, SimulationLinkDatum } from "d3-force";
import type { KnowledgeGraph } from "../tauri";
import type { GraphModel, GraphNode, GraphEdge } from "./model";
import { buildKnowledgeGraphModel, drawableModel } from "./model";
import type { GraphPalette } from "./palette";
import { compositeOver } from "./palette";
import {
  buildAtlasGraph,
  applyAtlasHierarchy,
  runAtlasLayout,
  createAtlasSimulation,
  relaxShelf,
  nonSimulatedIds,
  satellitePlan,
  placeSatellites,
  shelveComponents,
  buildIslandProtection,
  isIslandCenterProtected,
  islandProtectionMetrics,
  ISLAND_GAP,
  ISLAND_PACK_GAP,
  satelliteAnchor,
  annotateDust,
  dustVisibleCount,
  lodFor,
  HOVER_DUST_MAX,
  hoverStateFor,
  NEIGHBOR_LABEL_MAX,
  nodeDisplay,
  labelAnchor,
  layoutFocusLabels,
  edgeDisplay,
  drawRadialNodeLabel,
  NODE_LABEL_FONT,
} from "./atlas";
import type { HoverState, AtlasSimNode, AtlasSimLink } from "./atlas";

const PALETTE: GraphPalette = {
  project: "#111111",
  tool: "#222222",
  org: "#333333",
  person: "#444444",
  concept: "#555555",
  neutral: "#666666",
  edge: "#777777",
  edgeStrong: "#888888",
  label: "#999999",
  labelMuted: "#aaaaaa",
  // Black surface keeps the composite math legible: composited channel is
  // just slotChannel * alpha.
  surface: "#000000",
  graticule: "rgba(4,5,6,0.13)",
  bridge: "#bbbbbb",
  memory: "#cccccc",
  page: "#dddddd",
};

function node(overrides: Partial<GraphNode> = {}): GraphNode {
  return {
    id: overrides.id ?? "n1",
    kind: "entity",
    name: overrides.name ?? "Node",
    entityType: overrides.entityType ?? "concept",
    // NOT `?? true`: null is a real value here (relation-derived neighbors),
    // and ?? would silently promote it to confirmed.
    confirmed: "confirmed" in overrides ? (overrides.confirmed as boolean | null) : true,
    degree: overrides.degree ?? 0,
    space: "space" in overrides ? (overrides.space as string | null) : null,
    createdAt: overrides.createdAt ?? 100,
    updatedAt: overrides.updatedAt ?? 200,
  };
}

function edge(overrides: Partial<GraphEdge> = {}): GraphEdge {
  return {
    id: overrides.id ?? "e1",
    source: overrides.source ?? "n1",
    target: overrides.target ?? "n2",
    type: overrides.type ?? "knows",
    confidence: overrides.confidence ?? null,
    createdAt: overrides.createdAt ?? 100,
  };
}

function makeModel(nodes: GraphNode[], edges: GraphEdge[] = []): GraphModel {
  return { nodes, edges, coverage: { relationsFetchedFor: nodes.length, totalEntities: nodes.length } };
}

describe("buildAtlasGraph", () => {
  it("carries every model node and edge into the graphology graph", () => {
    const model = makeModel(
      [node({ id: "a" }), node({ id: "b" }), node({ id: "c" })],
      [edge({ id: "e1", source: "a", target: "b" }), edge({ id: "e2", source: "b", target: "c" })],
    );
    const graph = buildAtlasGraph(model, PALETTE);
    expect(graph.order).toBe(3);
    expect(graph.size).toBe(2);
  });

  it("is deterministic: the same model produces identical node attributes and positions", () => {
    const model = makeModel(
      [node({ id: "a", degree: 3 }), node({ id: "b", degree: 1 })],
      [edge({ id: "e1", source: "a", target: "b" })],
    );
    const g1 = buildAtlasGraph(model, PALETTE);
    const g2 = buildAtlasGraph(model, PALETTE);
    expect(g1.getNodeAttributes("a")).toEqual(g2.getNodeAttributes("a"));
    expect(g1.getNodeAttributes("b")).toEqual(g2.getNodeAttributes("b"));
  });

  it("scales node size monotonically with degree", () => {
    const model = makeModel([
      node({ id: "a", degree: 0 }),
      node({ id: "b", degree: 1 }),
      node({ id: "c", degree: 4 }),
      node({ id: "d", degree: 9 }),
    ]);
    const graph = buildAtlasGraph(model, PALETTE);
    const sizes = ["a", "b", "c", "d"].map((id) => graph.getNodeAttribute(id, "size") as number);
    expect(sizes[0]).toBeLessThan(sizes[1]);
    expect(sizes[1]).toBeLessThan(sizes[2]);
    expect(sizes[2]).toBeLessThan(sizes[3]);
  });

  it("fills nodes with the stability-tiered composite of their slot color over the surface", () => {
    const model = makeModel([
      node({ id: "p", entityType: "project", confirmed: true }),
      node({ id: "t", entityType: "technology", confirmed: false }),
      node({ id: "x", entityType: "place", confirmed: null }), // unknown type -> neutral
    ]);
    const graph = buildAtlasGraph(model, PALETTE);
    // Confirmed: project #111111 at 0.9 over #000000 → 0x11 * 0.9 = 15 → #0f0f0f.
    expect(graph.getNodeAttribute("p", "color")).toBe("#101010");
    // Unconfirmed: tool #222222 at 0.5 → 0x22 * 0.5 = 17 → #111111.
    expect(graph.getNodeAttribute("t", "color")).toBe("#181818");
    // Unknown status (relation-derived): neutral #666666 at 0.5 → #333333.
    expect(graph.getNodeAttribute("x", "color")).toBe("#474747");
  });

  it("gives a confirmed node a larger size base than an unconfirmed one at equal degree, capped at 14", () => {
    const model = makeModel([
      node({ id: "conf", confirmed: true, degree: 2 }),
      node({ id: "unconf", confirmed: false, degree: 2 }),
      node({ id: "unknown", confirmed: null, degree: 2 }),
      node({ id: "hub", confirmed: true, degree: 300 }),
    ]);
    const graph = buildAtlasGraph(model, PALETTE);
    // base + 1.9 * log2(1 + degree); log2(3) = 1.585.
    const growth = 1.9 * Math.log2(3);
    expect(graph.getNodeAttribute("conf", "size")).toBeCloseTo(4 + growth, 10);
    expect(graph.getNodeAttribute("unconf", "size")).toBeCloseTo(3 + growth, 10);
    expect(graph.getNodeAttribute("unknown", "size")).toBeCloseTo(3 + growth, 10);
    expect(graph.getNodeAttribute("hub", "size")).toBe(14);
  });

  it("keeps a wiki page on the entity scale and a memory below it, capped", () => {
    const model = makeModel([
      node({ id: "page", entityType: "page", confirmed: null, degree: 3 }),
      node({ id: "entity", entityType: "concept", confirmed: false, degree: 3 }),
      node({ id: "memory", entityType: "memory", confirmed: true, degree: 3 }),
      node({ id: "memhub", entityType: "memory", confirmed: true, degree: 300 }),
    ]);
    const graph = buildAtlasGraph(model, PALETTE);
    const growth = 1.9 * Math.log2(4);
    expect(graph.getNodeAttribute("page", "size")).toBeCloseTo(3 + growth, 10);
    expect(graph.getNodeAttribute("entity", "size")).toBeCloseTo(3 + growth, 10);
    // Memories are context: they start lowest and are capped well under the
    // entity/page ceiling, so a much-cited memory can never dominate.
    expect(graph.getNodeAttribute("memory", "size")).toBe(3);
    expect(graph.getNodeAttribute("memhub", "size")).toBe(3);
  });

  it("draws a shared-source edge thinner than a real link and keeps the verb on the edge", () => {
    const model = makeModel(
      [node({ id: "a" }), node({ id: "b" })],
      [
        { id: "w", source: "a", target: "b", type: "wikilink", confidence: null, createdAt: 1 },
        {
          id: "s",
          source: "a",
          target: "b",
          type: "shared_source",
          confidence: null,
          createdAt: 1,
          weight: 2,
        },
      ],
    );
    const graph = buildAtlasGraph(model, PALETTE);
    expect(graph.getEdgeAttribute("w", "size")).toBe(1);
    expect(graph.getEdgeAttribute("s", "size")).toBe(0.6);
    expect(graph.getEdgeAttribute("s", "edgeType")).toBe("shared_source");
  });

  it("stores confirmed on the node so theme recoloring can recompute the tiered fill", () => {
    const graph = buildAtlasGraph(makeModel([node({ id: "a", confirmed: null })]), PALETTE);
    expect(graph.getNodeAttribute("a", "confirmed")).toBeNull();
  });

  it("colors edges with the palette's quiet edge tone, size 1 (CSS px — the hairline default)", () => {
    const model = makeModel(
      [node({ id: "a" }), node({ id: "b" })],
      [edge({ id: "e1", source: "a", target: "b" })],
    );
    const graph = buildAtlasGraph(model, PALETTE);
    expect(graph.getEdgeAttribute("e1", "color")).toBe(PALETTE.edge);
    expect(graph.getEdgeAttribute("e1", "size")).toBe(1);
  });

  it("paints nothing amber at rest — every edge carries palette.edge and no bridge attribute", () => {
    const model = makeModel(
      [node({ id: "a" }), node({ id: "b" })],
      [edge({ id: "e1", source: "a", target: "b" })],
    );
    const graph = buildAtlasGraph(model, PALETTE);
    expect(graph.getEdgeAttribute("e1", "color")).toBe(PALETTE.edge);
    expect(graph.getEdgeAttribute("e1", "bridge")).toBeUndefined();
  });

  it("keeps distinct parallel relations between the same pair as distinct edges", () => {
    // GraphModel's parallel-edge policy (see model.ts) keeps these as two
    // separate edges — a non-multi graph would throw adding the second one.
    const model = makeModel(
      [node({ id: "a" }), node({ id: "b" })],
      [
        edge({ id: "e1", source: "a", target: "b", type: "founded" }),
        edge({ id: "e2", source: "a", target: "b", type: "mentors" }),
      ],
    );
    const graph = buildAtlasGraph(model, PALETTE);
    expect(graph.size).toBe(2);
  });

  it("seeds finite deterministic positions before any layout has run", () => {
    const model = makeModel([node({ id: "a" }), node({ id: "b" }), node({ id: "c" })]);
    const graph = buildAtlasGraph(model, PALETTE);
    for (const id of ["a", "b", "c"]) {
      expect(Number.isFinite(graph.getNodeAttribute(id, "x"))).toBe(true);
      expect(Number.isFinite(graph.getNodeAttribute(id, "y"))).toBe(true);
    }
  });
});

describe("applyAtlasHierarchy", () => {
  it("ranks structural components independently of memory links", () => {
    const model = makeModel(
      [
        node({ id: "hub", degree: 99 }),
        node({ id: "leaf" }),
        node({ id: "other" }),
        node({ id: "memory", entityType: "memory" }),
      ],
      [
        edge({ id: "hub-leaf", source: "hub", target: "leaf" }),
        edge({ id: "hub-memory", source: "hub", target: "memory" }),
        edge({ id: "leaf-memory", source: "leaf", target: "memory" }),
      ],
    );
    const graph = buildAtlasGraph(model, PALETTE);
    applyAtlasHierarchy(graph);

    expect(graph.getNodeAttribute("hub", "structuralDegree")).toBe(1);
    expect(graph.getNodeAttribute("leaf", "structuralDegree")).toBe(1);
    expect(graph.getNodeAttribute("memory", "landmark")).toBeUndefined();
    expect(graph.getNodeAttribute("hub", "componentId")).toBe(graph.getNodeAttribute("leaf", "componentId"));
    expect(graph.getNodeAttribute("hub", "landmark")).toBe(true);
    expect(graph.getNodeAttribute("leaf", "landmark")).toBe(true);
    expect(graph.getNodeAttribute("other", "landmark")).toBe(true);
  });

  it("keeps connected landmarks without promoting tiny islands on larger maps", () => {
    const nodes = Array.from({ length: 61 }, (_, index) => node({ id: `n${index}` }));
    const edges = [
      edge({ id: "hub-1", source: "n0", target: "n1" }),
      edge({ id: "hub-2", source: "n0", target: "n2" }),
      edge({ id: "hub-3", source: "n0", target: "n3" }),
      edge({ id: "other-1", source: "n4", target: "n5" }),
    ];
    const graph = buildAtlasGraph(makeModel(nodes, edges), PALETTE);
    applyAtlasHierarchy(graph);

    expect(graph.getNodeAttribute("n0", "landmark")).toBe(true);
    expect(graph.getNodeAttribute("n1", "landmark")).toBe(false);
    expect(graph.getNodeAttribute("n4", "landmark")).toBe(false);
    expect(graph.getNodeAttribute("n5", "landmark")).toBe(false);
    expect(graph.getNodeAttribute("n0", "landmarkLabel")).toBe(true);
    expect(graph.getNodeAttribute("n12", "landmarkLabel")).toBe(false);
    expect(graph.getNodeAttribute("n20", "landmarkLabel")).toBe(false);
  });

  it("adds secondary high-degree hubs inside one connected core", () => {
    const nodes = Array.from({ length: 70 }, (_, index) => node({ id: `n${index}` }));
    const edges = [
      ...Array.from({ length: 69 }, (_, index) => edge({ id: `chain-${index}`, source: `n${index}`, target: `n${index + 1}` })),
      edge({ id: "hub0-2", source: "n0", target: "n2" }),
      edge({ id: "hub0-3", source: "n0", target: "n3" }),
      edge({ id: "hub0-4", source: "n0", target: "n4" }),
      edge({ id: "hub5-7", source: "n5", target: "n7" }),
      edge({ id: "hub5-8", source: "n5", target: "n8" }),
      edge({ id: "hub10-12", source: "n10", target: "n12" }),
      edge({ id: "hub10-13", source: "n10", target: "n13" }),
    ];
    const graph = buildAtlasGraph(makeModel(nodes, edges), PALETTE);
    applyAtlasHierarchy(graph);

    expect(graph.getNodeAttribute("n0", "componentId")).toBe(graph.getNodeAttribute("n10", "componentId"));
    expect(graph.getNodeAttribute("n0", "landmark")).toBe(true);
    expect(graph.getNodeAttribute("n5", "landmark")).toBe(true);
    expect(graph.getNodeAttribute("n10", "landmark")).toBe(true);
    expect(graph.getNodeAttribute("n11", "landmark")).toBe(false);
    expect(graph.getNodeAttribute("n5", "structuralDegree")).toBe(4);
    expect(graph.getNodeAttribute("n10", "landmarkLabel")).toBe(true);
  });
});

describe("semantic zoom", () => {
  it("keeps a busy hub's memory spokes quiet while preserving individual memory inspection", () => {
    const endpoints = { source: { dustRank: 0 }, target: {} };
    const neighbors = new Set(Array.from({ length: 100 }, (_, i) => `memory-${i}`));
    expect(edgeDisplay({ hovered: "hub", neighbors }, "edge", "memory-0", "hub", {}, PALETTE, lodFor(5), endpoints).hidden).toBe(true);
    expect(edgeDisplay({ hovered: "memory-0", neighbors: new Set(["hub"]) }, "edge", "memory-0", "hub", {}, PALETTE, lodFor(5), endpoints).color).toBe(PALETTE.edgeStrong);
  });

  it("uses overview, neighbourhood, and detail phases at the requested thresholds", () => {
    expect(lodFor(1).phase).toBe("overview");
    expect(lodFor(1.99).phase).toBe("overview");
    expect(lodFor(2).phase).toBe("neighborhood");
    expect(lodFor(3.99).phase).toBe("neighborhood");
    expect(lodFor(4).phase).toBe("detail");
    expect(lodFor(1).dustVisible).toBe(dustVisibleCount(1));
    expect(lodFor(4).dustVisible).toBe(18);
  });

  it("reveals ordinary nodes and edges as the viewer zooms in", () => {
    const model = makeModel(
      [node({ id: "hub" }), node({ id: "leaf" }), node({ id: "far" }), node({ id: "core" }), ...Array.from({ length: 58 }, (_, index) => node({ id: `filler-${index}` }))],
      [
        edge({ id: "hub-leaf", source: "hub", target: "leaf" }),
        edge({ id: "leaf-far", source: "leaf", target: "far" }),
        edge({ id: "hub-core", source: "hub", target: "core" }),
      ],
    );
    const graph = buildAtlasGraph(model, PALETTE);
    applyAtlasHierarchy(graph);
    const rest: HoverState = { hovered: null, neighbors: new Set() };
    const ordinary = graph.getNodeAttributes("leaf");
    const landmark = graph.getNodeAttributes("hub");
    const overview = nodeDisplay(rest, "leaf", ordinary, PALETTE, lodFor(1));
    expect(overview.label).toBe("");
    expect(overview.color).not.toBe(ordinary.color);
    expect(overview.hidden).toBeUndefined();
    expect(nodeDisplay(rest, "hub", landmark, PALETTE, lodFor(1)).forceLabel).toBe(true);
    expect(nodeDisplay(rest, "hub", landmark, PALETTE, lodFor(2.5)).forceLabel).toBe(true);
    expect(nodeDisplay(rest, "hub", landmark, PALETTE, lodFor(5)).forceLabel).toBe(true);
    expect(nodeDisplay(rest, "leaf", ordinary, PALETTE, lodFor(2)).color).toBe(ordinary.color);
    expect(nodeDisplay(rest, "leaf", ordinary, PALETTE, lodFor(4)).color).toBe(ordinary.color);

    const ordinaryEdge = { source: graph.getNodeAttributes("leaf"), target: graph.getNodeAttributes("far") };
    expect(edgeDisplay(rest, "leaf-far", "leaf", "far", {}, PALETTE, lodFor(1), ordinaryEdge).hidden).toBe(true);
    const landmarkEdge = { source: graph.getNodeAttributes("hub"), target: graph.getNodeAttributes("leaf") };
    expect(edgeDisplay(rest, "hub-leaf", "hub", "leaf", {}, PALETTE, lodFor(1), landmarkEdge).hidden).toBeUndefined();
    expect(edgeDisplay(rest, "leaf-far", "leaf", "far", {}, PALETTE, lodFor(2), ordinaryEdge).hidden).toBeUndefined();
    expect(edgeDisplay({ hovered: "leaf", neighbors: new Set(["far"]) }, "leaf-far", "leaf", "far", {}, PALETTE, lodFor(1), ordinaryEdge).hidden).toBeUndefined();
  });
});

describe("runAtlasLayout", () => {
  it("leaves every node with finite coordinates after layout", () => {
    const model = makeModel(
      [node({ id: "a" }), node({ id: "b" }), node({ id: "c" }), node({ id: "d" })],
      [
        edge({ id: "e1", source: "a", target: "b" }),
        edge({ id: "e2", source: "b", target: "c" }),
        edge({ id: "e3", source: "c", target: "d" }),
      ],
    );
    const graph = buildAtlasGraph(model, PALETTE);
    runAtlasLayout(graph);
    graph.forEachNode((_id, attrs) => {
      expect(Number.isFinite(attrs.x)).toBe(true);
      expect(Number.isFinite(attrs.y)).toBe(true);
    });
  });

  it("is deterministic: laying out identically-built graphs lands on the same positions", () => {
    const model = makeModel(
      [node({ id: "a" }), node({ id: "b" }), node({ id: "c" })],
      [edge({ id: "e1", source: "a", target: "b" }), edge({ id: "e2", source: "b", target: "c" })],
    );
    const g1 = buildAtlasGraph(model, PALETTE);
    const g2 = buildAtlasGraph(model, PALETTE);
    runAtlasLayout(g1);
    runAtlasLayout(g2);
    for (const id of ["a", "b", "c"]) {
      expect(g1.getNodeAttribute(id, "x")).toBeCloseTo(g2.getNodeAttribute(id, "x") as number, 10);
      expect(g1.getNodeAttribute(id, "y")).toBeCloseTo(g2.getNodeAttribute(id, "y") as number, 10);
    }
  });
});

describe("large graph lifecycle", () => {
  it("keeps a hub plus many small triples finite when small groups are shown", () => {
    const coreLeafCount = 1_400;
    const smallGroupCount = 180;
    const pageCount = 140;
    const entities: KnowledgeGraph["entities"] = [];
    const relations: KnowledgeGraph["relations"] = [];
    const pages: KnowledgeGraph["pages"] = [];
    const pageLinks: KnowledgeGraph["page_links"] = [];
    const entity = (id: string) => ({
      id,
      name: id,
      entity_type: "concept",
      domain: null,
      space: null,
      source_agent: null,
      confidence: null,
      confirmed: true,
      created_at: 1,
      updated_at: 2,
      memory_count: 0,
      status: "detected" as const,
      established_by: null,
    });
    entities.push(entity("hub"));
    for (let i = 0; i < coreLeafCount; i += 1) {
      const id = `core-${i}`;
      entities.push(entity(id));
      relations.push({
        id: `core-edge-${i}`,
        from_entity: "hub",
        to_entity: id,
        relation_type: "knows",
        source_agent: null,
        created_at: 2,
      });
    }
    for (let group = 0; group < smallGroupCount; group += 1) {
      const ids = [0, 1, 2].map((i) => `small-${group}-${i}`);
      for (const id of ids) entities.push(entity(id));
      for (let i = 1; i < ids.length; i += 1) {
        relations.push({
          id: `small-edge-${group}-${i}`,
          from_entity: ids[i - 1] as string,
          to_entity: ids[i] as string,
          relation_type: "knows",
          source_agent: null,
          created_at: 2,
        });
      }
    }
    for (let i = 0; i < pageCount; i += 1) {
      const id = `page-${i}`;
      pages.push({
        id,
        title: id,
        space: null,
        creation_kind: "distilled",
        entity_id: "hub",
        last_modified: "2026-09-08T00:00:00Z",
      });
      pageLinks.push({
        from: { kind: "page", id },
        to: { kind: "entity", id: "hub" },
        link_type: "about",
      });
    }

    const model = buildKnowledgeGraphModel(
      { entities, relations, memories: [], memory_links: [], pages, page_links: pageLinks },
      { layers: { entity: true, page: true, memory: false } },
    );
    expect(model.nodes.length).toBeGreaterThan(2_000);

    const started = performance.now();
    const graph = buildAtlasGraph(model, PALETTE);
    runAtlasLayout(graph);
    const sim = createAtlasSimulation(graph);
    sim.stop();
    const elapsed = performance.now() - started;

    expect(elapsed).toBeLessThan(30_000);
    graph.forEachNode((_id, attrs) => {
      expect(Number.isFinite(attrs.x)).toBe(true);
      expect(Number.isFinite(attrs.y)).toBe(true);
    });
  }, 120_000);

  it("survives the preview-sized all-nodes to hidden to all-nodes transition", () => {
    const entities: KnowledgeGraph["entities"] = Array.from({ length: 2_381 }, (_, i) => ({
      id: `n${i}`,
      name: i < 12 ? `Hub ${i}` : `Topic ${i}`,
      entity_type: ["project", "technology", "concept", "person", "organization"][i % 5] as string,
      domain: null,
      space: null,
      confirmed: true,
      created_at: 1,
      updated_at: 1,
      memory_count: 1,
      status: "established" as const,
      established_by: "manual",
      source_agent: null,
      confidence: null,
    }));
    const relations: KnowledgeGraph["relations"] = [];
    for (let i = 1; i < 380; i += 1) {
      relations.push({
        id: `r${i}`,
        from_entity: `n${i < 12 ? 0 : i % 12}`,
        to_entity: `n${i}`,
        relation_type: "related",
        source_agent: null,
        created_at: 1,
      });
    }
    for (let i = 380; i < 620; i += 3) {
      relations.push(
        {
          id: `r${i}`,
          from_entity: `n${i}`,
          to_entity: `n${i + 1}`,
          relation_type: "related",
          source_agent: null,
          created_at: 1,
        },
        {
          id: `s${i}`,
          from_entity: `n${i + 1}`,
          to_entity: `n${i + 2}`,
          relation_type: "related",
          source_agent: null,
          created_at: 1,
        },
      );
    }
    const pages: KnowledgeGraph["pages"] = [];
    const pageLinks: KnowledgeGraph["page_links"] = [];
    for (let i = 0; i < 140; i += 1) {
      pages.push({
        id: `p${i}`,
        title: `Research note ${i}`,
        space: null,
        entity_id: null,
        creation_kind: "distilled",
        last_modified: "2026-09-08T00:00:00Z",
      });
      pageLinks.push({
        from: { kind: "page", id: `p${i}` },
        to: { kind: "entity", id: `n${i % 12}` },
        link_type: "about",
      });
      if (i > 0) {
        pageLinks.push({
          from: { kind: "page", id: `p${i}` },
          to: { kind: "page", id: `p${i - 1}` },
          link_type: "wikilink",
        });
      }
    }
    const full = buildKnowledgeGraphModel(
      { entities, relations, pages, page_links: pageLinks, memories: [], memory_links: [] },
      { layers: { entity: true, page: true, memory: false } },
    );
    const hidden = drawableModel(full, false);
    const started = performance.now();
    const preserve = (to: Graph, from: Graph) => {
      from.forEachNode((id, attrs) => {
        if (!to.hasNode(id)) return;
        to.setNodeAttribute(id, "x", attrs.x);
        to.setNodeAttribute(id, "y", attrs.y);
      });
    };

    const allFirst = buildAtlasGraph(full, PALETTE);
    runAtlasLayout(allFirst);
    createAtlasSimulation(allFirst).stop();

    const hiddenGraph = buildAtlasGraph(hidden, PALETTE);
    preserve(hiddenGraph, allFirst);
    runAtlasLayout(hiddenGraph);
    createAtlasSimulation(hiddenGraph).stop();

    const allAgain = buildAtlasGraph(full, PALETTE);
    preserve(allAgain, hiddenGraph);
    runAtlasLayout(allAgain);
    const sim = createAtlasSimulation(allAgain);
    sim.stop();
    expect(performance.now() - started).toBeLessThan(30_000);
    expect(allAgain.order).toBe(full.nodes.length);
    allAgain.forEachNode((_id, attrs) => {
      expect(Number.isFinite(attrs.x)).toBe(true);
      expect(Number.isFinite(attrs.y)).toBe(true);
    });
  }, 120_000);
});

describe("createAtlasSimulation", () => {
  function starGraph(): Graph {
    // Hub "h" with four spokes, laid out once so the spokes start near the hub.
    const model = makeModel(
      [node({ id: "h" }), node({ id: "s1" }), node({ id: "s2" }), node({ id: "s3" }), node({ id: "s4" })],
      [
        edge({ id: "e1", source: "h", target: "s1" }),
        edge({ id: "e2", source: "h", target: "s2" }),
        edge({ id: "e3", source: "h", target: "s3" }),
        edge({ id: "e4", source: "h", target: "s4" }),
      ],
    );
    const graph = buildAtlasGraph(model, PALETTE);
    runAtlasLayout(graph);
    return graph;
  }

  it("pulls a neighbor closer to a hub displaced via fx/fy", () => {
    const graph = starGraph();
    const sim = createAtlasSimulation(graph);
    const simNodes = sim.nodes();
    const hub = simNodes.find((n) => n.id === "h")!;
    const neighbor = simNodes.find((n) => n.id === "s1")!;

    const newHub = { x: hub.x! + 300, y: hub.y! + 300 };
    const distBefore = Math.hypot(neighbor.x! - newHub.x, neighbor.y! - newHub.y);

    hub.fx = newHub.x;
    hub.fy = newHub.y;
    sim.alpha(1);
    sim.tick(30);

    const distAfter = Math.hypot(neighbor.x! - newHub.x, neighbor.y! - newHub.y);
    expect(distAfter).toBeLessThan(distBefore);
  });

  it("restores the saved body, isolates, and satellite anchors without re-shelving", () => {
    const nodes = [
      ...Array.from({ length: 9 }, (_, i) => node({ id: `core${i}` })),
      ...Array.from({ length: 2 }, (_, i) => node({ id: `shelf${i}` })),
      node({ id: "isolate" }),
      node({ id: "memory", entityType: "memory", confirmed: null, degree: 1 }),
    ];
    const edges: GraphEdge[] = [
      ...Array.from({ length: 8 }, (_, i) =>
        edge({ id: `core-edge${i}`, source: `core${i}`, target: `core${i + 1}` }),
      ),
      edge({ id: "shelf-edge", source: "shelf0", target: "shelf1" }),
      edge({ id: "memory-edge", source: "memory", target: "core0", type: "mentions" }),
    ];
    const graph = buildAtlasGraph(makeModel(nodes, edges), PALETTE);
    runAtlasLayout(graph);
    const sim = createAtlasSimulation(graph);
    sim.stop();

    const saved = new Map<string, { x: number; y: number }>();
    graph.forEachNode((id, attrs) => {
      if (attrs.entityType === "memory") return;
      saved.set(id, { x: (attrs.x as number) + 700, y: (attrs.y as number) - 400 });
    });
    sim.restorePositions(saved);

    for (const [id, position] of saved) {
      expect(graph.getNodeAttribute(id, "x")).toBe(position.x);
      expect(graph.getNodeAttribute(id, "y")).toBe(position.y);
      const live = sim.nodes().find((candidate) => candidate.id === id);
      if (live) {
        expect(live.x).toBe(position.x);
        expect(live.y).toBe(position.y);
        expect(live.vx ?? 0).toBe(0);
        expect(live.vy ?? 0).toBe(0);
      }
    }
    expect(sim.alpha()).toBe(0);

    // A stale shelf target would pull this body back to its pre-restoration
    // coordinates. Releasing through the existing reduced-motion path must be
    // a no-op after restoration because its anchors were retargeted in place.
    const restoredShelf = ["shelf0", "shelf1"].map((id) => ({
      x: graph.getNodeAttribute(id, "x") as number,
      y: graph.getNodeAttribute(id, "y") as number,
    }));
    sim.settleShelf();
    restoredShelf.forEach((position, index) => {
      const id = `shelf${index}`;
      expect(graph.getNodeAttribute(id, "x")).toBe(position.x);
      expect(graph.getNodeAttribute(id, "y")).toBe(position.y);
    });

    // The memory was not restored from the body map; it still rides the
    // restored anchor through the normal satellite writeback path.
    const satellite = satellitePlan(graph).find((entry) => entry.id === "memory");
    expect(satellite).toBeDefined();
    const anchorX = graph.getNodeAttribute("core0", "x") as number;
    const anchorY = graph.getNodeAttribute("core0", "y") as number;
    expect(graph.getNodeAttribute("memory", "x")).toBeCloseTo(
      anchorX + (satellite?.radius ?? 0) * Math.cos(satellite?.angle ?? 0),
      8,
    );
    expect(graph.getNodeAttribute("memory", "y")).toBeCloseTo(
      anchorY + (satellite?.radius ?? 0) * Math.sin(satellite?.angle ?? 0),
      8,
    );
  });

  it("repacks only a new island that restoration leaves in the final core void", () => {
    const coreCount = 48;
    const nodes = [
      ...Array.from({ length: coreCount }, (_, i) => node({ id: `ring${i}`, degree: 2 })),
      node({ id: "new-island" }),
    ];
    const edges = Array.from({ length: coreCount }, (_, i) =>
      edge({
        id: `ring-edge${i}`,
        source: `ring${i}`,
        target: `ring${(i + 1) % coreCount}`,
      }),
    );
    const graph = buildAtlasGraph(makeModel(nodes, edges), PALETTE);
    const radius = 260;
    for (let i = 0; i < coreCount; i += 1) {
      const angle = (2 * Math.PI * i) / coreCount;
      graph.setNodeAttribute(`ring${i}`, "x", Math.cos(angle) * radius);
      graph.setNodeAttribute(`ring${i}`, "y", Math.sin(angle) * radius);
    }
    graph.setNodeAttribute("new-island", "x", 0);
    graph.setNodeAttribute("new-island", "y", 0);
    runAtlasLayout(graph);
    const sim = createAtlasSimulation(graph);
    sim.stop();

    const saved = new Map<string, { x: number; y: number }>();
    graph.forEachNode((id, attrs) => {
      if (id === "new-island") return;
      saved.set(id, { x: (attrs.x as number) + 800, y: (attrs.y as number) - 500 });
    });
    graph.setNodeAttribute("new-island", "x", 800);
    graph.setNodeAttribute("new-island", "y", -500);
    sim.restorePositions(saved);

    for (const [id, position] of saved) {
      expect(graph.getNodeAttribute(id, "x")).toBe(position.x);
      expect(graph.getNodeAttribute(id, "y")).toBe(position.y);
    }
    const protection = buildIslandProtection(graph, [...saved.keys()]);
    const islandX = graph.getNodeAttribute("new-island", "x") as number;
    const islandY = graph.getNodeAttribute("new-island", "y") as number;
    expect(isIslandCenterProtected(protection, islandX, islandY, 0)).toBe(false);
    expect(Math.hypot(islandX - 800, islandY + 500)).toBeGreaterThan(0);
  });

  it("excludes isolates from the simulation entirely — the ring-hold is structural, not fx/fy", () => {
    const model = makeModel(
      [node({ id: "a", degree: 1 }), node({ id: "b", degree: 1 }), node({ id: "iso" })],
      [edge({ id: "e1", source: "a", target: "b" })],
    );
    const graph = buildAtlasGraph(model, PALETTE);
    const sim = createAtlasSimulation(graph);
    expect(sim.nodes().some((n) => n.id === "iso")).toBe(false);
  });

  it("EQUILIBRIUM INVARIANT: settles to near-zero alpha at creation, and reheating without a drag barely moves the connected cluster", () => {
    const graph = starGraph();
    const sim = createAtlasSimulation(graph);

    expect(sim.alpha()).toBeLessThanOrEqual(0.01);

    const bboxDiagonal = () => {
      let minX = Infinity;
      let maxX = -Infinity;
      let minY = Infinity;
      let maxY = -Infinity;
      for (const n of sim.nodes()) {
        minX = Math.min(minX, n.x!);
        maxX = Math.max(maxX, n.x!);
        minY = Math.min(minY, n.y!);
        maxY = Math.max(maxY, n.y!);
      }
      return Math.hypot(maxX - minX, maxY - minY);
    };

    const before = bboxDiagonal();
    // Reheat WITHOUT touching fx/fy on anything — no drag in progress, so a
    // sim already at its own equilibrium should barely move.
    sim.alphaTarget(0);
    sim.alpha(0.3);
    sim.tick(60);
    const after = bboxDiagonal();

    expect(Math.abs(after - before) / before).toBeLessThan(0.03);
  });

  it("collapses parallel edges between the same pair to a single sim link", () => {
    const model = makeModel(
      [node({ id: "a" }), node({ id: "b" })],
      [
        edge({ id: "e1", source: "a", target: "b", type: "founded" }),
        edge({ id: "e2", source: "a", target: "b", type: "mentors" }),
      ],
    );
    const graph = buildAtlasGraph(model, PALETTE);
    const sim = createAtlasSimulation(graph);
    const linkForce = sim.force<ForceLink<AtlasSimNode, SimulationLinkDatum<AtlasSimNode>>>("link");
    expect(linkForce?.links()).toHaveLength(1);
  });

  it("syncs ticked positions back onto the graphology graph for connected nodes", () => {
    const graph = starGraph();
    const before = {
      x: graph.getNodeAttribute("s1", "x") as number,
      y: graph.getNodeAttribute("s1", "y") as number,
    };

    const sim = createAtlasSimulation(graph);
    sim.alpha(1);
    sim.tick(30);

    const after = {
      x: graph.getNodeAttribute("s1", "x") as number,
      y: graph.getNodeAttribute("s1", "y") as number,
    };
    expect(after).not.toEqual(before);
  });

  it("invokes onTick after every writeback — settle, the pack pass, then once per manual tick", () => {
    const graph = starGraph();
    const onTick = vi.fn();
    const sim = createAtlasSimulation(graph, onTick);
    // The settle runs as a single wrapped tick(settleTicks) call → one
    // writeback; the final pack-and-anchor pass writes back explicitly so
    // satellites follow their moved anchors → a second. TWO in total, not
    // three: the knot relax in between runs on relaxShelf's scratch
    // simulation, which never touches this graph or this callback.
    expect(onTick).toHaveBeenCalledTimes(2);
    sim.tick(1);
    expect(onTick).toHaveBeenCalledTimes(3);
  });
});

describe("relaxShelf", () => {
  it("owns the shelved nodes and only them, opens them, and leaves the core exactly where it was", () => {
    const nodes: AtlasSimNode[] = [
      { id: "core0", x: 0, y: 0, radius: 5 },
      { id: "core1", x: 40, y: 0, radius: 5 },
      // Two shelved nodes dropped on nearly the same point: the knot this
      // pass exists to open.
      { id: "s0", x: 0, y: -200, radius: 5 },
      { id: "s1", x: 1, y: -200, radius: 5 },
    ];
    const links: AtlasSimLink[] = [
      { source: "core0", target: "core1", type: "knows" },
      { source: "s0", target: "s1", type: "knows" },
    ];
    const placement = [
      ["core0", "core1"],
      ["s0", "s1"],
    ];
    const degree = new Map([
      ["core0", 1],
      ["core1", 1],
      ["s0", 1],
      ["s1", 1],
    ]);

    const sim = relaxShelf(nodes, links, placement, degree);

    // The perf guard, asserted structurally rather than on a stopwatch: the
    // core is already settled, so relaxing it again would only buy back the
    // whole O(core) charge cost this scratch pass was split out to avoid.
    expect(sim.nodes().map((n) => n.id).sort()).toEqual(["s0", "s1"]);
    // Core positions come back byte-identical — it was never simulated.
    expect(nodes[0]).toMatchObject({ x: 0, y: 0 });
    expect(nodes[1]).toMatchObject({ x: 40, y: 0 });
    // The shelved pair opened to its link's rest length...
    const [s0, s1] = [nodes[2] as AtlasSimNode, nodes[3] as AtlasSimNode];
    expect(Math.hypot((s0.x ?? 0) - (s1.x ?? 0), (s0.y ?? 0) - (s1.y ?? 0))).toBeGreaterThan(10);
    // ...around the centroid it started on, held there by its own group anchor.
    expect(((s0.x ?? 0) + (s1.x ?? 0)) / 2).toBeCloseTo(0.5, 1);
    expect(((s0.y ?? 0) + (s1.y ?? 0)) / 2).toBeCloseTo(-200, 1);
  });
});

describe("hoverStateFor", () => {
  it("returns the empty state when nothing is hovered", () => {
    const model = makeModel(
      [node({ id: "a" }), node({ id: "b" })],
      [edge({ id: "e1", source: "a", target: "b" })],
    );
    const graph = buildAtlasGraph(model, PALETTE);
    const state = hoverStateFor(graph, null);
    expect(state.hovered).toBeNull();
    expect(state.neighbors.size).toBe(0);
  });

  it("collects the exact neighbor set for a hovered node with edges", () => {
    const model = makeModel(
      [node({ id: "a" }), node({ id: "b" }), node({ id: "c" }), node({ id: "d" })],
      [edge({ id: "e1", source: "a", target: "b" }), edge({ id: "e2", source: "a", target: "c" })],
    );
    const graph = buildAtlasGraph(model, PALETTE);
    const state = hoverStateFor(graph, "a");
    expect(state.hovered).toBe("a");
    expect(state.neighbors).toEqual(new Set(["b", "c"]));
  });

  it("returns an empty neighbor set for an isolated node", () => {
    const model = makeModel([node({ id: "a" }), node({ id: "iso" })]);
    const graph = buildAtlasGraph(model, PALETTE);
    const state = hoverStateFor(graph, "iso");
    expect(state.neighbors.size).toBe(0);
  });
});

describe("nodeDisplay", () => {
  const attrs = { label: "Alice", color: "#123456", size: 8 };

  it("passes attrs through unchanged when nothing is hovered", () => {
    const state: HoverState = { hovered: null, neighbors: new Set() };
    expect(nodeDisplay(state, "a", attrs, PALETTE)).toEqual(attrs);
  });

  it("keeps the hovered node's own color, forces its label, lifts it, and puts it on top", () => {
    const state: HoverState = { hovered: "a", neighbors: new Set(["b"]) };
    const result = nodeDisplay(state, "a", attrs, PALETTE);
    expect(result.color).toBe(attrs.color);
    expect(result.label).toBe(attrs.label);
    expect(result.forceLabel).toBe(true);
    expect(result.highlighted).toBe(true);
    expect(result.zIndex).toBe(2);
  });

  it("keeps a neighbor's own color, names it, and lifts it, at zIndex 1", () => {
    const state: HoverState = { hovered: "a", neighbors: new Set(["b"]) };
    const result = nodeDisplay(state, "b", attrs, PALETTE);
    expect(result.color).toBe(attrs.color);
    expect(result.label).toBe(attrs.label);
    expect(result.forceLabel).toBe(true);
    expect(result.highlighted).toBe(true);
    expect(result.zIndex).toBe(1);
  });

  it("stops naming and lifting neighbors past NEIGHBOR_LABEL_MAX, leaving them lit", () => {
    const many = new Set(Array.from({ length: NEIGHBOR_LABEL_MAX + 1 }, (_, i) => `n${i}`));
    const result = nodeDisplay({ hovered: "a", neighbors: many }, "n0", attrs, PALETTE);
    expect(result.color).toBe(attrs.color);
    expect(result.label).toBe(attrs.label);
    expect(result.forceLabel).toBeUndefined();
    expect(result.highlighted).toBeUndefined();
    expect(result.zIndex).toBe(1);
    const atCap = new Set(Array.from({ length: NEIGHBOR_LABEL_MAX }, (_, i) => `n${i}`));
    expect(nodeDisplay({ hovered: "a", neighbors: atCap }, "n0", attrs, PALETTE).forceLabel).toBe(true);
  });

  it("mutes and blanks everyone else, at zIndex 0", () => {
    const state: HoverState = { hovered: "a", neighbors: new Set(["b"]) };
    const result = nodeDisplay(state, "c", attrs, PALETTE);
    expect(result.color).toBe(compositeOver(PALETTE.neutral, PALETTE.surface, 0.17));
    expect(result.label).toBe("");
    expect(result.zIndex).toBe(0);
  });

  const rest: HoverState = { hovered: null, neighbors: new Set() };

  it("hides dust past the zoom's visible count, and reveals a bounded group when its anchor is hovered", () => {
    const dust = { ...attrs, dustRank: 7, dustOf: "hub" };
    const opening = lodFor(1);
    expect(nodeDisplay(rest, "m", dust, PALETTE, opening).hidden).toBe(true);
    expect(nodeDisplay(rest, "m", { ...dust, dustRank: 5 }, PALETTE, opening).hidden).toBeUndefined();
    const hoverHub: HoverState = { hovered: "hub", neighbors: new Set(["m"]) };
    expect(nodeDisplay(hoverHub, "m", dust, PALETTE, opening).hidden).toBeUndefined();
    // ...up to HOVER_DUST_MAX of them; the rest wait for the zoom.
    expect(nodeDisplay(hoverHub, "m", { ...dust, dustRank: HOVER_DUST_MAX }, PALETTE, opening).hidden).toBe(true);
    expect(nodeDisplay(hoverHub, "m", { ...dust, dustRank: HOVER_DUST_MAX }, PALETTE, lodFor(4)).hidden).toBe(true);
    // Zoom never expands a dense group without a bound.
    expect(nodeDisplay(rest, "m", dust, PALETTE, lodFor(2)).hidden).toBeUndefined();
    expect(nodeDisplay(rest, "m", { ...dust, dustRank: 40 }, PALETTE, lodFor(2)).hidden).toBe(true);
    expect(nodeDisplay(rest, "m", { ...dust, dustRank: 400 }, PALETTE, lodFor(4)).hidden).toBe(true);
  });

  it("draws an island node dim and nameless at the opening view, solid once zoomed in", () => {
    const island = { ...attrs, island: true };
    const dim = nodeDisplay(rest, "i", island, PALETTE, lodFor(1));
    expect(dim.label).toBe("");
    expect(dim.color).not.toBe(attrs.color);
    expect(dim.color).toBe(compositeOver(attrs.color, PALETTE.surface, 0.72));
    const solid = nodeDisplay(rest, "i", island, PALETTE, lodFor(2));
    expect(solid).toEqual(island);
  });

  it("bounds visible dust at 6 / 12 / 18 with zoom", () => {
    expect(dustVisibleCount(1)).toBe(6);
    expect(dustVisibleCount(1.9)).toBe(6);
    expect(dustVisibleCount(2)).toBe(12);
    expect(dustVisibleCount(4)).toBe(18);
  });
});

describe("edgeDisplay", () => {
  const attrs = { color: "#abcdef", size: 1 };

  it("passes attrs through unchanged when nothing is hovered", () => {
    const state: HoverState = { hovered: null, neighbors: new Set() };
    expect(edgeDisplay(state, "e1", "a", "b", attrs, PALETTE)).toEqual(attrs);
  });

  it("emphasizes an edge incident to the hovered node as source", () => {
    const state: HoverState = { hovered: "a", neighbors: new Set(["b"]) };
    const result = edgeDisplay(state, "e1", "a", "b", attrs, PALETTE);
    expect(result.color).toBe(PALETTE.edgeStrong);
    expect(result.zIndex).toBe(1);
    expect(result.hidden).toBeUndefined();
  });

  it("emphasizes an edge incident to the hovered node as target", () => {
    const state: HoverState = { hovered: "b", neighbors: new Set(["a"]) };
    const result = edgeDisplay(state, "e1", "a", "b", attrs, PALETTE);
    expect(result.color).toBe(PALETTE.edgeStrong);
    expect(result.zIndex).toBe(1);
  });

  it("hides a non-incident edge while hovering", () => {
    const state: HoverState = { hovered: "a", neighbors: new Set(["b"]) };
    const result = edgeDisplay(state, "e2", "b", "c", attrs, PALETTE);
    expect(result.hidden).toBe(true);
  });

  it("emphasizes both parallel edges between the hovered node and a neighbor", () => {
    const state: HoverState = { hovered: "a", neighbors: new Set(["b"]) };
    const r1 = edgeDisplay(state, "e1", "a", "b", attrs, PALETTE);
    const r2 = edgeDisplay(state, "e2", "a", "b", attrs, PALETTE);
    expect(r1.color).toBe(PALETTE.edgeStrong);
    expect(r2.color).toBe(PALETTE.edgeStrong);
  });

  it("hides memory spokes at rest and reveals them when a memory or endpoint is hovered", () => {
    const rest: HoverState = { hovered: null, neighbors: new Set() };
    // The dot is shown at this zoom (rank 0 < 6), but its thread remains
    // quiet so a busy halo does not become a web of spokes.
    const anchored = { source: { dustRank: 0, dustOf: "b" }, target: {} };
    expect(edgeDisplay(rest, "e1", "m", "b", attrs, PALETTE, lodFor(1), anchored).hidden).toBe(true);
    // ...either way round, including the anchor thread.
    expect(edgeDisplay(rest, "e1", "b", "m", attrs, PALETTE, lodFor(1), { source: {}, target: { dustRank: 0, dustOf: "b" } }).hidden).toBe(true);
    // A multi-link memory's other edge is quiet too.
    expect(edgeDisplay(rest, "e2", "m", "c", attrs, PALETTE, lodFor(1), anchored).hidden).toBe(true);
    // A dot the zoom hides remains quiet as well.
    const deep = { source: { dustRank: 10, dustOf: "b" }, target: {} };
    expect(edgeDisplay(rest, "e1", "m", "b", attrs, PALETTE, lodFor(1), deep).hidden).toBe(true);
    expect(edgeDisplay(rest, "e1", "m", "b", attrs, PALETTE, lodFor(2), deep).hidden).toBe(true);
    // Hovering either endpoint shows the incident edge as before.
    const hover: HoverState = { hovered: "b", neighbors: new Set(["m"]) };
    expect(edgeDisplay(hover, "e1", "m", "b", attrs, PALETTE, lodFor(1), deep).hidden).toBeUndefined();
    const hoverMemory: HoverState = { hovered: "m", neighbors: new Set(["b"]) };
    expect(edgeDisplay(hoverMemory, "e1", "m", "b", attrs, PALETTE, lodFor(1), deep).hidden).toBeUndefined();
  });

  it("hides an island's edges while the island is dim", () => {
    const rest: HoverState = { hovered: null, neighbors: new Set() };
    const ends = { source: { island: true }, target: { island: true } };
    expect(edgeDisplay(rest, "e1", "a", "b", attrs, PALETTE, lodFor(1), ends).hidden).toBe(true);
    expect(edgeDisplay(rest, "e1", "a", "b", attrs, PALETTE, lodFor(2), ends)).toEqual(attrs);
  });
});

describe("drawRadialNodeLabel", () => {
  function mockCtx() {
    return {
      font: "",
      fillStyle: "",
      globalAlpha: 1,
      textAlign: "",
      textBaseline: "",
      lineJoin: "",
      lineWidth: 0,
      strokeStyle: "",
      fillText: vi.fn(),
      strokeText: vi.fn(),
    };
  }

  // One node whose GRAPH position we place per case; the drawer's viewport
  // data stays fixed at (100, 50) size 4 → pad 12.
  function graphWithNodeAt(gx: number, gy: number) {
    const graph = buildAtlasGraph(makeModel([node({ id: "n1", name: "Alice" })]), PALETTE);
    graph.setNodeAttribute("n1", "x", gx);
    graph.setNodeAttribute("n1", "y", gy);
    return graph;
  }
  const data = { key: "n1", label: "Alice", size: 4, x: 100, y: 50 };
  const settings = { labelColor: { color: "#abcdef" } };

  it("places the label INWARD (left of the node) for a node right of the graph center", () => {
    const ctx = mockCtx();
    drawRadialNodeLabel(ctx as any, data, settings, graphWithNodeAt(10, 0));
    expect(ctx.textAlign).toBe("right");
    expect(ctx.textBaseline).toBe("middle");
    expect(ctx.fillText).toHaveBeenCalledWith("Alice", 88, 50);
  });

  it("places the label right of the node for a node left of the graph center", () => {
    const ctx = mockCtx();
    drawRadialNodeLabel(ctx as any, data, settings, graphWithNodeAt(-10, 0));
    expect(ctx.textAlign).toBe("left");
    expect(ctx.fillText).toHaveBeenCalledWith("Alice", 112, 50);
  });

  it("places the label below the node for a node above center — graph +y is SCREEN-UP in sigma", () => {
    const ctx = mockCtx();
    drawRadialNodeLabel(ctx as any, data, settings, graphWithNodeAt(0, 10));
    expect(ctx.textAlign).toBe("center");
    expect(ctx.textBaseline).toBe("top");
    // Viewport y grows downward: +pad = below the node on screen = inward.
    expect(ctx.fillText).toHaveBeenCalledWith("Alice", 100, 62);
  });

  it("places the label above the node for a node below center", () => {
    const ctx = mockCtx();
    drawRadialNodeLabel(ctx as any, data, settings, graphWithNodeAt(0, -10));
    expect(ctx.textBaseline).toBe("bottom");
    expect(ctx.fillText).toHaveBeenCalledWith("Alice", 100, 38);
  });

  it("draws the shared node-label font from settings.labelColor at 85% alpha, restored after", () => {
    const ctx = mockCtx();
    let alphaAtDraw = 0;
    ctx.fillText.mockImplementation(() => {
      alphaAtDraw = ctx.globalAlpha;
    });
    drawRadialNodeLabel(ctx as any, data, settings, graphWithNodeAt(10, 0));
    expect(ctx.font).toBe(NODE_LABEL_FONT);
    expect(ctx.fillStyle).toBe("#abcdef");
    expect(alphaAtDraw).toBe(0.85);
    expect(ctx.globalAlpha).toBe(1); // restored — the labels canvas is shared
  });

  it("draws nothing for an empty label (hover reducer blanks dimmed nodes)", () => {
    const ctx = mockCtx();
    drawRadialNodeLabel(ctx as any, { ...data, label: "" }, settings, graphWithNodeAt(10, 0));
    expect(ctx.fillText).not.toHaveBeenCalled();
  });

  it("strokes a ground-coloured halo behind the label, before the fill, when one is given", () => {
    const ctx = mockCtx();
    const order: string[] = [];
    ctx.strokeText.mockImplementation(() => order.push("stroke"));
    ctx.fillText.mockImplementation(() => order.push("fill"));
    drawRadialNodeLabel(ctx as any, data, settings, graphWithNodeAt(10, 0), "#0a0a0a");
    expect(ctx.lineJoin).toBe("round");
    expect(ctx.lineWidth).toBe(3);
    expect(ctx.strokeStyle).toBe("#0a0a0a");
    expect(ctx.strokeText).toHaveBeenCalledWith("Alice", 88, 50);
    expect(order).toEqual(["stroke", "fill"]);
  });

  it("strokes nothing when no halo colour is given", () => {
    const ctx = mockCtx();
    drawRadialNodeLabel(ctx as any, data, settings, graphWithNodeAt(10, 0));
    expect(ctx.strokeText).not.toHaveBeenCalled();
  });
});

describe("layoutFocusLabels", () => {
  // 6 px per character, the way a fixed-width test face would measure.
  const measure = (text: string) => text.length * 6;
  const focus = { key: "tally", x: 400, y: 300, size: 4, label: "tally" };
  const LINE = 14;

  function box(at: { x: number; y: number; align: string; baseline: string }, width: number) {
    const x = at.align === "left" ? at.x : at.align === "right" ? at.x - width : at.x - width / 2;
    const y = at.baseline === "top" ? at.y : at.baseline === "bottom" ? at.y - LINE : at.y - LINE / 2;
    return { x, y, w: width, h: LINE };
  }
  function disjoint(a: ReturnType<typeof box>, b: ReturnType<typeof box>) {
    return a.x + a.w <= b.x || b.x + b.w <= a.x || a.y + a.h <= b.y || b.y + b.h <= a.y;
  }

  it("hangs each neighbor's name on the side facing away from the focus", () => {
    const neighbors = [
      { key: "east", x: 460, y: 300, size: 3, label: "east" },
      { key: "west", x: 340, y: 300, size: 3, label: "west" },
      { key: "north", x: 400, y: 240, size: 3, label: "north" },
      { key: "south", x: 400, y: 360, size: 3, label: "south" },
    ];
    const out = layoutFocusLabels(focus, neighbors, measure);
    expect(out.get("east")).toEqual(labelAnchor(460, 300, 3, "right"));
    expect(out.get("west")).toEqual(labelAnchor(340, 300, 3, "left"));
    expect(out.get("north")).toEqual(labelAnchor(400, 240, 3, "above"));
    expect(out.get("south")).toEqual(labelAnchor(400, 360, 3, "below"));
  });

  it("faces the focus's own name away from where its neighbors sit", () => {
    const neighbors = [
      { key: "a", x: 460, y: 290, size: 3, label: "a" },
      { key: "b", x: 470, y: 310, size: 3, label: "b" },
    ];
    expect(layoutFocusLabels(focus, neighbors, measure).get("tally")).toEqual(labelAnchor(400, 300, 4, "left"));
    expect(layoutFocusLabels(focus, [], measure).get("tally")).toEqual(labelAnchor(400, 300, 4, "right"));
  });

  it("never lets two names overlap, nor a name cover a lit dot — the Tally cluster", () => {
    // Positions from the launch-video take: sqlite and Backup & Review on one
    // row left of tally, Gap-Free just right of it, two memories far below.
    const neighbors = [
      { key: "sqlite", x: 756, y: 254, size: 4, label: "sqlite" },
      { key: "backup", x: 801, y: 254, size: 3, label: "Tally Backup & Review" },
      { key: "gapfree", x: 887, y: 277, size: 3, label: "Tally's Gap-Free Invoice System" },
      { key: "arch", x: 803, y: 678, size: 3, label: "architecture-notes" },
      { key: "uses", x: 839, y: 742, size: 3, label: "Tally Uses SQLite Only" },
    ];
    const tally = { key: "tally", x: 845, y: 274, size: 5, label: "tally" };
    const out = layoutFocusLabels(tally, neighbors, measure);
    const boxes = [...out.entries()].map(([key, at]) => ({
      key,
      box: box(at, measure([tally, ...neighbors].find((n) => n.key === key)!.label)),
    }));
    for (let i = 0; i < boxes.length; i++) {
      for (let j = i + 1; j < boxes.length; j++) {
        expect(disjoint(boxes[i].box, boxes[j].box), `${boxes[i].key} vs ${boxes[j].key}`).toBe(true);
      }
      for (const dot of [tally, ...neighbors]) {
        const disc = { x: dot.x - dot.size, y: dot.y - dot.size, w: 2 * dot.size, h: 2 * dot.size };
        expect(disjoint(boxes[i].box, disc), `${boxes[i].key} over ${dot.key}`).toBe(true);
      }
    }
    // Everyone in this cluster has room; the focus always goes first.
    expect(out.size).toBe(6);
    expect([...out.keys()][0]).toBe("tally");
  });

  it("leaves a name off rather than paint it over another when nothing fits", () => {
    // Four neighbors sitting on top of each other, all wanting the same spot,
    // with long names that leave no free side or push.
    const long = "a name long enough to block every side of the focus at once";
    const neighbors = ["p", "q", "r", "s"].map((key) => ({ key, x: 406, y: 300, size: 3, label: long }));
    const out = layoutFocusLabels({ ...focus, label: long }, neighbors, measure);
    expect(out.size).toBeGreaterThanOrEqual(1);
    expect(out.size).toBeLessThan(5);
    const boxes = [...out.values()].map((at) => box(at, measure(long)));
    for (let i = 0; i < boxes.length; i++)
      for (let j = i + 1; j < boxes.length; j++) expect(disjoint(boxes[i], boxes[j])).toBe(true);
  });

  it("skips a blank name and keeps the rest", () => {
    const out = layoutFocusLabels(focus, [{ key: "blank", x: 460, y: 300, size: 3, label: "" }], measure);
    expect(out.has("blank")).toBe(false);
    expect(out.has("tally")).toBe(true);
  });
});

describe("shared-source edges and node size", () => {
  it("sizes a page from its asserted links only, ignoring shared-source overlap", () => {
    // Same drawn degree (3), but two of "overlap"'s edges are inferred
    // overlap rather than asserted links, so it draws at asserted-degree 1.
    const model = makeModel(
      [
        node({ id: "asserted", entityType: "page", confirmed: null, degree: 3 }),
        node({ id: "overlap", entityType: "page", confirmed: null, degree: 3 }),
        node({ id: "x" }),
        node({ id: "y" }),
        node({ id: "z" }),
      ],
      [
        edge({ id: "a1", source: "asserted", target: "x", type: "wikilink" }),
        edge({ id: "a2", source: "asserted", target: "y", type: "wikilink" }),
        edge({ id: "a3", source: "asserted", target: "z", type: "cites" }),
        edge({ id: "o1", source: "overlap", target: "x", type: "wikilink" }),
        edge({ id: "o2", source: "overlap", target: "y", type: "shared_source" }),
        edge({ id: "o3", source: "overlap", target: "z", type: "shared_source" }),
      ],
    );
    const graph = buildAtlasGraph(model, PALETTE);
    expect(graph.getNodeAttribute("asserted", "size")).toBeCloseTo(3 + 1.9 * Math.log2(4), 10);
    expect(graph.getNodeAttribute("overlap", "size")).toBeCloseTo(3 + 1.9 * Math.log2(2), 10);
  });

  it("never sizes a node below its base, however many shared-source edges touch it", () => {
    const model = makeModel(
      [node({ id: "p", entityType: "page", confirmed: null, degree: 1 }), node({ id: "q", entityType: "page", confirmed: null, degree: 1 })],
      [edge({ id: "s", source: "p", target: "q", type: "shared_source" })],
    );
    const graph = buildAtlasGraph(model, PALETTE);
    expect(graph.getNodeAttribute("p", "size")).toBe(3);
  });
});

describe("nonSimulatedIds and satellites", () => {
  function leafModel(): GraphModel {
    return makeModel(
      [
        node({ id: "hub", degree: 3 }),
        node({ id: "peer", degree: 1 }),
        node({ id: "leaf1", entityType: "memory", confirmed: null, degree: 1 }),
        node({ id: "leaf2", entityType: "memory", confirmed: null, degree: 1 }),
        node({ id: "busy", entityType: "memory", confirmed: null, degree: 2 }),
      ],
      [
        edge({ id: "e1", source: "hub", target: "peer" }),
        edge({ id: "e2", source: "leaf1", target: "hub", type: "mentions" }),
        edge({ id: "e3", source: "leaf2", target: "hub", type: "mentions" }),
        edge({ id: "e4", source: "busy", target: "hub", type: "mentions" }),
        edge({ id: "e5", source: "busy", target: "peer", type: "mentions" }),
      ],
    );
  }

  it("excludes every memory and every isolate, but keeps every other node", () => {
    const graph = buildAtlasGraph(
      makeModel(
        [...leafModel().nodes, node({ id: "iso" })],
        leafModel().edges,
      ),
      PALETTE,
    );
    // `busy` links two entities and is STILL a satellite: a memory never
    // gets springs of its own, whatever its degree.
    expect(nonSimulatedIds(graph).sort()).toEqual(["busy", "iso", "leaf1", "leaf2"]);
  });

  it("anchors a multi-link memory on its most-connected neighbour, ties to the smaller id", () => {
    const graph = buildAtlasGraph(leafModel(), PALETTE);
    // hub has degree 4 (peer, leaf1, leaf2, busy); peer has 2.
    expect(satelliteAnchor(graph, "busy")).toBe("hub");
    const tie = buildAtlasGraph(
      makeModel(
        [node({ id: "b" }), node({ id: "a" }), node({ id: "m", entityType: "memory", confirmed: null })],
        [
          edge({ id: "e1", source: "m", target: "b", type: "mentions" }),
          edge({ id: "e2", source: "m", target: "a", type: "mentions" }),
        ],
      ),
      PALETTE,
    );
    expect(satelliteAnchor(tie, "m")).toBe("a");
    // A memory whose only neighbours are memories has no anchor.
    const lonely = buildAtlasGraph(
      makeModel(
        [
          node({ id: "m1", entityType: "memory", confirmed: null }),
          node({ id: "m2", entityType: "memory", confirmed: null }),
        ],
        [edge({ id: "e1", source: "m1", target: "m2", type: "mentions" })],
      ),
      PALETTE,
    );
    expect(satelliteAnchor(lonely, "m1")).toBeUndefined();
  });

  it("numbers each anchor's satellites by rank and writes the dust bookkeeping onto the graph", () => {
    const graph = buildAtlasGraph(leafModel(), PALETTE);
    const plan = satellitePlan(graph);
    const hubPlan = plan.filter((sat) => sat.anchor === "hub").sort((a, b) => a.rank - b.rank);
    expect(hubPlan.map((sat) => sat.id)).toEqual(["busy", "leaf1", "leaf2"]);
    expect(hubPlan.map((sat) => sat.rank)).toEqual([0, 1, 2]);
    annotateDust(graph, plan);
    expect(graph.getNodeAttribute("hub", "dustCount")).toBe(3);
    expect(graph.getNodeAttribute("leaf2", "dustRank")).toBe(2);
    expect(graph.getNodeAttribute("leaf2", "dustOf")).toBe("hub");
    expect(graph.getNodeAttribute("peer", "dustCount")).toBeUndefined();
  });

  it("does not treat a degree-1 ENTITY as a satellite — only memories orbit", () => {
    const graph = buildAtlasGraph(leafModel(), PALETTE);
    expect(nonSimulatedIds(graph)).not.toContain("peer");
  });

  it("orbits each memory around its anchor at the anchor's radius plus a gap", () => {
    const graph = buildAtlasGraph(leafModel(), PALETTE);
    graph.setNodeAttribute("hub", "x", 40);
    graph.setNodeAttribute("hub", "y", -15);
    const plan = satellitePlan(graph);
    expect(plan.map((s) => s.id).sort()).toEqual(["busy", "leaf1", "leaf2"]);
    placeSatellites(graph, plan);
    const hubSize = graph.getNodeAttribute("hub", "size") as number;
    for (const sat of plan) {
      const dx = (graph.getNodeAttribute(sat.id, "x") as number) - 40;
      const dy = (graph.getNodeAttribute(sat.id, "y") as number) + 15;
      expect(Math.hypot(dx, dy)).toBeCloseTo(sat.radius, 10);
      expect(sat.radius).toBeGreaterThanOrEqual(hubSize + 10);
    }
    // Three on one anchor sit apart, not on top of each other.
    expect(new Set(plan.map((s) => s.angle.toFixed(6))).size).toBe(3);
  });

  it("keeps every memory out of the simulation and its links", () => {
    const graph = buildAtlasGraph(leafModel(), PALETTE);
    const sim = createAtlasSimulation(graph);
    expect(sim.nodes().map((n) => n.id).sort()).toEqual(["hub", "peer"]);
    const linkForce = sim.force<ForceLink<AtlasSimNode, SimulationLinkDatum<AtlasSimNode>>>("link");
    // Only hub-peer: every memory edge is drawn (on hover) but never
    // simulated, so d3 is never asked to resolve an endpoint it does not own.
    expect(linkForce?.links()).toHaveLength(1);
  });

  it("lays the map out the same with the memories on and off", () => {
    // Two components of entities, as the base model draws them; the memory
    // layer then hangs memories on both (attachMemories keeps every base
    // node's degree, so the fixtures agree on size).
    const entities = (): { nodes: GraphNode[]; edges: GraphEdge[] } => {
      const nodes: GraphNode[] = [];
      const edges: GraphEdge[] = [];
      for (let i = 0; i < 8; i += 1) {
        nodes.push(node({ id: `a${i}`, degree: i === 0 ? 7 : 1 }));
        if (i > 0) edges.push(edge({ id: `ae${i}`, source: "a0", target: `a${i}` }));
      }
      for (let i = 0; i < 5; i += 1) {
        nodes.push(node({ id: `b${i}`, degree: i === 0 || i === 4 ? 1 : 2 }));
        if (i > 0) edges.push(edge({ id: `be${i}`, source: `b${i - 1}`, target: `b${i}` }));
      }
      return { nodes, edges };
    };
    const bare = entities();
    const dressed = entities();
    for (let i = 0; i < 30; i += 1) {
      const id = `m${String(i).padStart(2, "0")}`;
      dressed.nodes.push(node({ id, entityType: "memory", confirmed: null, degree: i % 3 === 0 ? 2 : 1 }));
      dressed.edges.push(edge({ id: `${id}-a`, source: id, target: i % 2 ? "a0" : "b2", type: "mentions" }));
      if (i % 3 === 0) dressed.edges.push(edge({ id: `${id}-b`, source: id, target: "a3", type: "mentions" }));
    }
    const withoutMemories = buildAtlasGraph(makeModel(bare.nodes, bare.edges), PALETTE);
    const withMemories = buildAtlasGraph(makeModel(dressed.nodes, dressed.edges), PALETTE);
    // R1: a fresh R2 layout keeps room for the memory dots it shows (a
    // kept map is restored as it was, whichever layers are on).
    runAtlasLayout(withoutMemories, "r1");
    runAtlasLayout(withMemories, "r1");
    createAtlasSimulation(withoutMemories);
    createAtlasSimulation(withMemories);
    for (const { id } of bare.nodes) {
      expect(withMemories.getNodeAttribute(id, "x")).toBeCloseTo(withoutMemories.getNodeAttribute(id, "x") as number, 6);
      expect(withMemories.getNodeAttribute(id, "y")).toBeCloseTo(withoutMemories.getNodeAttribute(id, "y") as number, 6);
    }
  });

  it("carries a dragged anchor's leaves along on every tick writeback", () => {
    const graph = buildAtlasGraph(leafModel(), PALETTE);
    runAtlasLayout(graph);
    const sim = createAtlasSimulation(graph);
    const before = graph.getNodeAttribute("leaf1", "x") as number;
    // Exactly what AtlasView's mousemovebody does: write the pointer position
    // straight onto the graph AND pin the sim node there.
    const hubX = graph.getNodeAttribute("hub", "x") as number;
    graph.setNodeAttribute("hub", "x", hubX + 500);
    const hub = sim.nodes().find((n) => n.id === "hub")!;
    hub.fx = hubX + 500;
    hub.fy = hub.y;
    sim.alpha(0.3);
    sim.tick(5);
    const after = graph.getNodeAttribute("leaf1", "x") as number;
    expect(after - before).toBeCloseTo(500, 6);
  });

  it("is deterministic: the same graph plans the same orbits twice", () => {
    const g1 = buildAtlasGraph(leafModel(), PALETTE);
    const g2 = buildAtlasGraph(leafModel(), PALETTE);
    expect(satellitePlan(g1)).toEqual(satellitePlan(g2));
  });

  /** One entity with `count` leaf memories hanging off it — the real capture's
   *  worst anchor carries 374. */
  function haloModel(count: number): GraphModel {
    const nodes: GraphNode[] = [node({ id: "hub", degree: count })];
    const edges: GraphEdge[] = [];
    for (let i = 0; i < count; i += 1) {
      const id = `leaf${String(i).padStart(3, "0")}`;
      nodes.push(node({ id, entityType: "memory", confirmed: null, degree: 1 }));
      edges.push(edge({ id: `m${i}`, source: id, target: "hub", type: "mentions" }));
    }
    return makeModel(nodes, edges);
  }

  function haloGraph(count: number): Graph {
    const graph = buildAtlasGraph(haloModel(count), PALETTE);
    graph.setNodeAttribute("hub", "x", 0);
    graph.setNodeAttribute("hub", "y", 0);
    return graph;
  }

  it("spreads leaves across increasing radii instead of crowding a circle", () => {
    const rings = (count: number) =>
      new Set(satellitePlan(haloGraph(count)).map((s) => s.radius.toFixed(6))).size;
    // Each point receives its own radius, avoiding concentric bead rings.
    expect(rings(2)).toBe(2);
    expect(rings(40)).toBeGreaterThan(1);
    expect(rings(120)).toBeGreaterThan(rings(40));
  });

  it("never lets two satellite discs of one halo touch, at 40 leaves", () => {
    const graph = haloGraph(40);
    const plan = satellitePlan(graph);
    placeSatellites(graph, plan);
    const leafSize = graph.getNodeAttribute("leaf000", "size") as number;
    // Two discs plus a unit of sky. On one circle at the anchor's radius the
    // forty leaves sit ~1.6 units apart — a solid donut.
    const floor = 2 * leafSize + 1;
    let closest = Infinity;
    for (let i = 0; i < plan.length; i += 1) {
      for (let j = i + 1; j < plan.length; j += 1) {
        const a = plan[i] as { id: string };
        const b = plan[j] as { id: string };
        closest = Math.min(
          closest,
          Math.hypot(
            (graph.getNodeAttribute(a.id, "x") as number) - (graph.getNodeAttribute(b.id, "x") as number),
            (graph.getNodeAttribute(a.id, "y") as number) - (graph.getNodeAttribute(b.id, "y") as number),
          ),
        );
      }
    }
    expect(closest).toBeGreaterThanOrEqual(floor);
  });

  it("leaves the leaf under the pointer where the drag put it", () => {
    const graph = buildAtlasGraph(leafModel(), PALETTE);
    runAtlasLayout(graph);
    const sim = createAtlasSimulation(graph);
    // What AtlasView's mousemovebody does for a satellite: write the pointer
    // position straight onto the graph. It is not a sim node, so there is no
    // pin to set — the sim is told by id instead.
    sim.setDraggingId("leaf1");
    graph.setNodeAttribute("leaf1", "x", 900);
    graph.setNodeAttribute("leaf1", "y", -900);
    sim.alpha(0.3);
    sim.tick(5);
    expect(graph.getNodeAttribute("leaf1", "x")).toBe(900);
    expect(graph.getNodeAttribute("leaf1", "y")).toBe(-900);
    // Its sibling is still riding its orbit, so the writeback did run.
    expect(graph.getNodeAttribute("leaf2", "x")).not.toBe(900);
  });

  it("puts a released leaf back on its orbit — the exemption is for the drag only", () => {
    const graph = buildAtlasGraph(leafModel(), PALETTE);
    runAtlasLayout(graph);
    const sim = createAtlasSimulation(graph);
    graph.setNodeAttribute("leaf1", "x", 900);
    sim.alpha(0.3);
    sim.tick(1);
    expect(graph.getNodeAttribute("leaf1", "x")).not.toBe(900);
  });
});

describe("simulation forces (round 4)", () => {
  function pagePair(): GraphModel {
    return makeModel(
      [
        node({ id: "p1", entityType: "page", degree: 2 }),
        node({ id: "p2", entityType: "page", degree: 2 }),
        node({ id: "p3", entityType: "page", degree: 2 }),
      ],
      [
        edge({ id: "s1", source: "p1", target: "p2", type: "shared_source" }),
        edge({ id: "w1", source: "p2", target: "p3", type: "wikilink" }),
      ],
    );
  }

  it("gives every sim node the same collision radius rule — disc plus COLLIDE_PAD, pages included", () => {
    const model = makeModel(
      [node({ id: "e", degree: 1 }), node({ id: "p", entityType: "page", degree: 1 })],
      [edge({ id: "e1", source: "e", target: "p", type: "about" })],
    );
    const graph = buildAtlasGraph(model, PALETTE);
    const sim = createAtlasSimulation(graph);
    sim.stop();
    const byId = new Map(sim.nodes().map((n) => [n.id, n]));
    const entity = byId.get("e")!;
    const page = byId.get("p")!;
    // Pages no longer carry a separate ring term — same pad as any entity.
    expect(entity.radius).toBe((graph.getNodeAttribute("e", "size") as number) + 2);
    expect(page.radius).toBe((graph.getNodeAttribute("p", "size") as number) + 2);
  });

  it("registers a collide force, so two discs dropped on the same spot separate", () => {
    const graph = buildAtlasGraph(pagePair(), PALETTE);
    // Stack p1 and p2 exactly, which is what the page blob looked like.
    for (const id of ["p1", "p2"]) {
      graph.setNodeAttribute(id, "x", 0);
      graph.setNodeAttribute(id, "y", 0);
    }
    const sim = createAtlasSimulation(graph);
    sim.stop();
    const byId = new Map(sim.nodes().map((n) => [n.id, n]));
    const gap = Math.hypot(byId.get("p1")!.x! - byId.get("p2")!.x!, byId.get("p1")!.y! - byId.get("p2")!.y!);
    expect(sim.force("collide")).toBeDefined();
    expect(gap).toBeGreaterThan(byId.get("p1")!.radius);
  });

  it("gives each link the rest length its verb calls for", () => {
    const graph = buildAtlasGraph(pagePair(), PALETTE);
    const sim = createAtlasSimulation(graph);
    sim.stop();
    const linkForce = sim.force<ForceLink<AtlasSimNode, SimulationLinkDatum<AtlasSimNode>>>("link");
    const links = linkForce!.links() as unknown as { source: AtlasSimNode; target: AtlasSimNode }[];
    const distance = linkForce!.distance() as unknown as (link: unknown) => number;
    const strength = linkForce!.strength() as unknown as (link: unknown) => number;
    const shared = links.find((l) => l.source.id === "p1" || l.target.id === "p1")!;
    const wiki = links.find((l) => l.source.id === "p3" || l.target.id === "p3")!;
    expect(distance(shared)).toBe(70);
    expect(strength(shared)).toBeCloseTo(0.15, 6);
    expect(distance(wiki)).toBe(50);
    expect(strength(wiki)).toBeCloseTo(0.5, 6);
  });

  it("leaves an unlisted verb on d3's own distance and 1/min-degree strength", () => {
    const model = makeModel(
      [node({ id: "a", degree: 2 }), node({ id: "b", degree: 1 }), node({ id: "c", degree: 1 })],
      [
        edge({ id: "e1", source: "a", target: "b", type: "knows" }),
        edge({ id: "e2", source: "a", target: "c", type: "knows" }),
      ],
    );
    const graph = buildAtlasGraph(model, PALETTE);
    const sim = createAtlasSimulation(graph);
    sim.stop();
    const linkForce = sim.force<ForceLink<AtlasSimNode, SimulationLinkDatum<AtlasSimNode>>>("link");
    const links = linkForce!.links() as unknown as { source: AtlasSimNode; target: AtlasSimNode }[];
    const distance = linkForce!.distance() as unknown as (link: unknown) => number;
    const strength = linkForce!.strength() as unknown as (link: unknown) => number;
    // a has two links, b and c one each → 1/min(2,1) = 1, d3's own answer.
    expect(distance(links[0])).toBe(30);
    expect(strength(links[0])).toBeCloseTo(1, 6);
  });

  it("keeps the strongest verb when parallel edges collapse to one spring", () => {
    const model = makeModel(
      [node({ id: "p1", entityType: "page", degree: 2 }), node({ id: "p2", entityType: "page", degree: 2 })],
      [
        edge({ id: "s1", source: "p1", target: "p2", type: "shared_source" }),
        edge({ id: "w1", source: "p1", target: "p2", type: "wikilink" }),
      ],
    );
    const graph = buildAtlasGraph(model, PALETTE);
    const sim = createAtlasSimulation(graph);
    sim.stop();
    const linkForce = sim.force<ForceLink<AtlasSimNode, SimulationLinkDatum<AtlasSimNode>>>("link");
    const links = linkForce!.links();
    const distance = linkForce!.distance() as unknown as (link: unknown) => number;
    expect(links).toHaveLength(1);
    // The wikilink is the asserted link, so it sets the spring — not the
    // shared-source edge that happens to run beside it.
    expect(distance(links[0])).toBe(50);
  });
});

describe("shelveComponents", () => {
  /** One chain per requested size, so component sizes are exactly as asked
   *  and every node is an entity (a leaf MEMORY would be a satellite, which
   *  the shelf measures through its anchor instead). */
  function componentsModel(sizes: number[]): GraphModel {
    const nodes: GraphNode[] = [];
    const edges: GraphEdge[] = [];
    sizes.forEach((size, c) => {
      for (let i = 0; i < size; i += 1) {
        nodes.push(node({ id: `c${c}n${i}` }));
        if (i > 0) {
          edges.push(edge({ id: `c${c}e${i}`, source: `c${c}n${i - 1}`, target: `c${c}n${i}` }));
        }
      }
    });
    return makeModel(nodes, edges);
  }

  function shelved(sizes: number[]) {
    const graph = buildAtlasGraph(componentsModel(sizes), PALETTE);
    // The island shelf is the R1 path's; R2 places its own islands.
    runAtlasLayout(graph, "r1");
    const placement = shelveComponents(graph);
    return { graph, placement };
  }

  /** The drawn extent of one component: every node's disc. */
  function box(graph: Graph, ids: string[]) {
    const pad = (id: string) => graph.getNodeAttribute(id, "size") as number;
    const xs = ids.map((id) => graph.getNodeAttribute(id, "x") as number);
    const ys = ids.map((id) => graph.getNodeAttribute(id, "y") as number);
    return {
      minX: Math.min(...ids.map((id, i) => (xs[i] as number) - pad(id))),
      maxX: Math.max(...ids.map((id, i) => (xs[i] as number) + pad(id))),
      minY: Math.min(...ids.map((id, i) => (ys[i] as number) - pad(id))),
      maxY: Math.max(...ids.map((id, i) => (ys[i] as number) + pad(id))),
    };
  }

  it("centres the largest component on the origin and returns it first", () => {
    const { graph, placement } = shelved([9, 6, 5]);
    expect(placement[0]).toHaveLength(9);
    const core = box(graph, placement[0] as string[]);
    expect((core.minX + core.maxX) / 2).toBeCloseTo(0, 6);
    expect((core.minY + core.maxY) / 2).toBeCloseTo(0, 6);
  });

  /** Gap between the actual drawn discs. A component's bounding box may
   * contain intentional empty space, so box-to-box distance is no longer a
   * valid core contract after islands are allowed to use that space. */
  function discGap(graph: Graph, left: string[], right: string[]): number {
    let closest = Infinity;
    for (const a of left) {
      const ax = graph.getNodeAttribute(a, "x") as number;
      const ay = graph.getNodeAttribute(a, "y") as number;
      const ar = graph.getNodeAttribute(a, "size") as number;
      for (const b of right) {
        const bx = graph.getNodeAttribute(b, "x") as number;
        const by = graph.getNodeAttribute(b, "y") as number;
        const br = graph.getNodeAttribute(b, "size") as number;
        closest = Math.min(closest, Math.hypot(ax - bx, ay - by) - ar - br);
      }
    }
    return closest;
  }

  it("packs every other component as an island ISLAND_GAP clear of the core and ISLAND_PACK_GAP clear of each other", () => {
    const { graph, placement } = shelved([9, 6, 5, 5, 5, 5, 5, 3, 1]);
    const boxes = placement.map((ids) => box(graph, ids));
    for (let i = 1; i < boxes.length; i += 1) {
      expect(discGap(graph, placement[0] as string[], placement[i] as string[])).toBeGreaterThanOrEqual(
        ISLAND_GAP - 1e-6,
      );
      for (let j = i + 1; j < boxes.length; j += 1) {
        expect(discGap(graph, placement[i] as string[], placement[j] as string[])).toBeGreaterThanOrEqual(
          ISLAND_PACK_GAP - 1e-6,
        );
      }
    }
  });

  it("spreads the islands all round the core rather than piling them on one side", () => {
    const { graph, placement } = shelved([9, 5, 5, 5, 5, 5, 5, 5, 5]);
    const angles = placement.slice(1).map((ids) => {
      const b = box(graph, ids);
      return Math.atan2((b.minY + b.maxY) / 2, (b.minX + b.maxX) / 2);
    });
    const quadrants = new Set(angles.map((a) => Math.floor(((a + Math.PI) / (2 * Math.PI)) * 4) % 4));
    expect(quadrants.size).toBe(4);
  });

  it("uses irregular ID-seeded candidates rather than a fixed ring", () => {
    const { graph, placement } = shelved([9, 5, 5, 5, 5, 5, 5, 5, 5]);
    const centres = placement.slice(1).map((ids) => {
      const b = box(graph, ids);
      return { x: (b.minX + b.maxX) / 2, y: (b.minY + b.maxY) / 2 };
    });
    const radii = centres.map(({ x, y }) => Math.hypot(x, y));
    expect(Math.max(...radii) - Math.min(...radii)).toBeGreaterThan(20);
    expect(new Set(centres.map(({ x, y }) => `${Math.round(x)},${Math.round(y)}`)).size).toBe(centres.length);
  });

  it("never overlaps two components", () => {
    // Islands are packed as circles that hold their discs, so the boxes of
    // two islands may cross at a corner; their discs never touch.
    const { graph, placement } = shelved([9, 6, 5, 5, 5, 5, 5]);
    for (let i = 0; i < placement.length; i += 1) {
      for (let j = i + 1; j < placement.length; j += 1) {
        expect(discGap(graph, placement[i] as string[], placement[j] as string[])).toBeGreaterThan(0);
      }
    }
  });

  it("moves a component rigidly — internal distances survive", () => {
    const { graph, placement } = shelved([9, 5]);
    const ids = placement[1] as string[];
    const graph2 = buildAtlasGraph(componentsModel([9, 5]), PALETTE);
    runAtlasLayout(graph2, "r1");
    const at = (g: Graph, id: string) => ({
      x: g.getNodeAttribute(id, "x") as number,
      y: g.getNodeAttribute(id, "y") as number,
    });
    for (let i = 1; i < ids.length; i += 1) {
      const before = Math.hypot(
        at(graph2, ids[i] as string).x - at(graph2, ids[0] as string).x,
        at(graph2, ids[i] as string).y - at(graph2, ids[0] as string).y,
      );
      const after = Math.hypot(
        at(graph, ids[i] as string).x - at(graph, ids[0] as string).x,
        at(graph, ids[i] as string).y - at(graph, ids[0] as string).y,
      );
      expect(after).toBeCloseTo(before, 6);
    }
  });

  it("places islands largest first, lone nodes last", () => {
    const { placement } = shelved([9, 5, 1, 1, 3]);
    expect(placement.slice(1).map((ids) => ids.length)).toEqual([5, 3, 1, 1]);
  });

  it("protects a meaningful hollow core instead of filling its enclosed negative space", () => {
    const coreCount = 1_800;
    const islandCount = 201;
    const coreRadius = 420;
    const nodes: GraphNode[] = [];
    const edges: GraphEdge[] = [];
    for (let i = 0; i < coreCount; i += 1) {
      nodes.push(node({ id: `c0n${i}`, degree: 2 }));
      edges.push(
        edge({
          id: `c0e${i}`,
          source: `c0n${i}`,
          target: `c0n${(i + 1) % coreCount}`,
        }),
      );
    }
    for (let i = 0; i < islandCount; i += 1) nodes.push(node({ id: `c1n${i}` }));
    const graph = buildAtlasGraph(makeModel(nodes, edges), PALETTE);
    for (let i = 0; i < coreCount; i += 1) {
      const angle = (2 * Math.PI * i) / coreCount;
      graph.setNodeAttribute(`c0n${i}`, "x", Math.cos(angle) * coreRadius);
      graph.setNodeAttribute(`c0n${i}`, "y", Math.sin(angle) * coreRadius);
    }
    for (let i = 0; i < islandCount; i += 1) {
      graph.setNodeAttribute(`c1n${i}`, "x", 1_000 + i);
      graph.setNodeAttribute(`c1n${i}`, "y", 1_000);
    }

    const started = performance.now();
    const placement = shelveComponents(graph);
    const elapsed = performance.now() - started;

    expect(placement).toHaveLength(1 + islandCount);
    expect(elapsed).toBeLessThan(2_000);
    // The ring's interior is empty ink, but it is meaningful negative space:
    // the closed structural outline protects it from unrelated components.
    const first = placement[1]![0] as string;
    expect(Math.hypot(graph.getNodeAttribute(first, "x") as number, graph.getNodeAttribute(first, "y") as number)).toBeGreaterThan(
      coreRadius,
    );
    const protection = buildIslandProtection(graph, placement[0] as string[]);
    const metrics = islandProtectionMetrics(protection);
    expect(metrics.columns).toBeLessThanOrEqual(192);
    expect(metrics.rows).toBeLessThanOrEqual(192);
    expect(metrics.protectedCells).toBeGreaterThan(0);
    for (let i = 1; i < placement.length; i += 1) {
      const island = placement[i]![0] as string;
      expect(
        isIslandCenterProtected(
          protection,
          graph.getNodeAttribute(island, "x") as number,
          graph.getNodeAttribute(island, "y") as number,
          graph.getNodeAttribute(island, "size") as number,
        ),
      ).toBe(false);
    }
    for (let i = 1; i < placement.length; i += 1) {
      expect(discGap(graph, placement[0] as string[], placement[i] as string[])).toBeGreaterThanOrEqual(
        ISLAND_GAP - 1e-6,
      );
      for (let j = i + 1; j < placement.length; j += 1) {
        expect(discGap(graph, placement[i] as string[], placement[j] as string[])).toBeGreaterThanOrEqual(
          ISLAND_PACK_GAP - 1e-6,
        );
      }
    }
  });

  it("keeps an oversized component out of the cell walk", () => {
    const nodes: GraphNode[] = [];
    const edges: GraphEdge[] = [];
    for (let i = 0; i < 200; i += 1) {
      nodes.push(node({ id: `core${i}` }));
      if (i > 0) edges.push(edge({ id: `core-edge${i}`, source: `core${i - 1}`, target: `core${i}` }));
    }
    for (let i = 0; i < 10; i += 1) {
      nodes.push(node({ id: `wide${i}` }));
      if (i > 0) edges.push(edge({ id: `wide-edge${i}`, source: `wide${i - 1}`, target: `wide${i}` }));
    }
    nodes.push(node({ id: "small" }));
    const graph = buildAtlasGraph(makeModel(nodes, edges), PALETTE);
    for (let i = 0; i < 200; i += 1) {
      graph.setNodeAttribute(`core${i}`, "x", i * 2);
      graph.setNodeAttribute(`core${i}`, "y", 0);
    }
    for (let i = 0; i < 10; i += 1) {
      graph.setNodeAttribute(`wide${i}`, "x", -1_000_000 + i * 200_000);
      graph.setNodeAttribute(`wide${i}`, "y", 500_000);
    }
    graph.setNodeAttribute("small", "x", 0);
    graph.setNodeAttribute("small", "y", 0);

    const started = performance.now();
    const placement = shelveComponents(graph);
    expect(performance.now() - started).toBeLessThan(2_000);
    expect(placement.map((ids) => ids.length)).toEqual([200, 10, 1]);
    expect(discGap(graph, placement[0] as string[], placement[1] as string[])).toBeGreaterThanOrEqual(
      ISLAND_GAP - 1e-6,
    );
    expect(discGap(graph, placement[1] as string[], placement[2] as string[])).toBeGreaterThanOrEqual(
      ISLAND_GAP - 1e-6,
    );
  });

  it("measures a component by its discs alone — a memory halo never moves an island", () => {
    const leaves = 40;
    const build = (withLeaves: boolean) => {
      const nodes: GraphNode[] = [];
      const edges: GraphEdge[] = [];
      for (let i = 0; i < 6; i += 1) {
        nodes.push(node({ id: `k${i}` }));
        if (i > 0) edges.push(edge({ id: `ke${i}`, source: `k${i - 1}`, target: `k${i}` }));
      }
      if (withLeaves) {
        for (let i = 0; i < leaves; i += 1) {
          const id = `leaf${String(i).padStart(3, "0")}`;
          nodes.push(node({ id, entityType: "memory", confirmed: null, degree: 1 }));
          edges.push(edge({ id: `m${i}`, source: id, target: "k0", type: "mentions" }));
        }
      }
      for (let i = 0; i < 5; i += 1) {
        nodes.push(node({ id: `s${i}` }));
        if (i > 0) edges.push(edge({ id: `se${i}`, source: `s${i - 1}`, target: `s${i}` }));
      }
      const graph = buildAtlasGraph(makeModel(nodes, edges), PALETTE);
      // Placed by hand instead of laid out, so the two graphs start equal.
      for (let i = 0; i < 6; i += 1) {
        graph.setNodeAttribute(`k${i}`, "x", i * 40);
        graph.setNodeAttribute(`k${i}`, "y", 0);
      }
      for (let i = 0; i < 5; i += 1) {
        graph.setNodeAttribute(`s${i}`, "x", i * 40);
        graph.setNodeAttribute(`s${i}`, "y", 500);
      }
      return graph;
    };
    const bare = build(false);
    const haloed = build(true);
    expect(shelveComponents(bare)[0]).toContain("k0");
    expect(shelveComponents(haloed)[0]).toContain("k0");
    for (const id of ["k0", "k3", "s0", "s4"]) {
      expect(haloed.getNodeAttribute(id, "x")).toBeCloseTo(bare.getNodeAttribute(id, "x") as number, 6);
      expect(haloed.getNodeAttribute(id, "y")).toBeCloseTo(bare.getNodeAttribute(id, "y") as number, 6);
    }
  });

  it("is deterministic: the same graph shelves the same way twice", () => {
    const first = shelved([9, 6, 5, 1]);
    const second = shelved([9, 6, 5, 1]);
    expect(second.placement).toEqual(first.placement);
    for (const ids of first.placement) {
      expect(box(second.graph, ids)).toEqual(box(first.graph, ids));
    }
  });

  it("does nothing to an empty graph", () => {
    const graph = buildAtlasGraph(makeModel([]), PALETTE);
    expect(shelveComponents(graph)).toEqual([]);
  });

  it("holds each shelved component's centroid still through a drag reheat", () => {
    const graph = buildAtlasGraph(componentsModel([9, 6, 5]), PALETTE);
    runAtlasLayout(graph);
    const sim = createAtlasSimulation(graph);
    const groups = [
      ["c1n0", "c1n1", "c1n2", "c1n3", "c1n4", "c1n5"],
      ["c2n0", "c2n1", "c2n2", "c2n3", "c2n4"],
    ];
    const centroidOf = (ids: string[]) => {
      const xs = ids.map((id) => graph.getNodeAttribute(id, "x") as number);
      const ys = ids.map((id) => graph.getNodeAttribute(id, "y") as number);
      return {
        x: xs.reduce((a, b) => a + b, 0) / xs.length,
        y: ys.reduce((a, b) => a + b, 0) / ys.length,
      };
    };
    const before = groups.map(centroidOf);
    // Exactly AtlasView's downNode: pin the pressed core node, jump alpha to
    // 0.3 and reheat.
    const pressed = sim.nodes().find((n) => n.id === "c0n0") as {
      fx?: number | null;
      fy?: number | null;
      x?: number;
      y?: number;
    };
    pressed.fx = pressed.x;
    pressed.fy = pressed.y;
    sim.alpha(0.3).alphaTarget(0.3);
    sim.tick(60);

    // Every component is live now (see shelfAnchorForce) — collide and link
    // can still shuffle nodes WITHIN a shelved component on a core reheat,
    // that is the fix — but the component as a whole has to stay on its
    // slot; an exact-box freeze no longer applies.
    //
    // The bound is tight on purpose. A hold that only corrects POSITION
    // leaves each tick's velocity to carry the component a little further,
    // every tick, forever: ~2.2 units here, and 27 units on the real capture
    // where the core's mass sits 3,000 units away and pushes that much
    // harder. Cancelling the component's bulk velocity is what makes the
    // residual vanish rather than merely look small at fixture scale.
    groups.forEach((ids, i) => {
      const after = centroidOf(ids);
      const b = before[i] as { x: number; y: number };
      expect(Math.hypot(after.x - b.x, after.y - b.y)).toBeLessThan(0.5);
    });
    // The two shelved components still keep clear of each other.
    const [b1, b2] = groups.map((ids) => box(graph, ids)) as [
      { minX: number; maxX: number; minY: number; maxY: number },
      { minX: number; maxX: number; minY: number; maxY: number },
    ];
    const overlaps = b1.minX < b2.maxX && b2.minX < b1.maxX && b1.minY < b2.maxY && b2.minY < b1.maxY;
    expect(overlaps).toBe(false);
  });

  it("holds the core in place through a reheat instead of chasing the shelf's mass", () => {
    const graph = buildAtlasGraph(componentsModel([24, 6, 6, 6, 6]), PALETTE);
    runAtlasLayout(graph);
    const sim = createAtlasSimulation(graph);
    // Every component is live now — the core under groupCenterForce, the
    // shelved ones under shelfAnchorForce — so fx == null no longer picks out
    // the core; every node's fx is null right after creation. c0 is the core
    // by construction (24 of 48 nodes, by far the largest), so select it by
    // id instead.
    const core = sim.nodes().filter((n) => n.id.startsWith("c0n"));
    const pressed = core[0] as {
      fx?: number | null;
      fy?: number | null;
      x?: number;
      y?: number;
    };
    const rest = core.filter((n) => n !== pressed);
    const centroid = () => ({
      x: rest.reduce((sum, n) => sum + (n.x ?? 0), 0) / rest.length,
      y: rest.reduce((sum, n) => sum + (n.y ?? 0), 0) / rest.length,
    });
    const span = Math.max(...rest.map((n) => Math.hypot(n.x ?? 0, n.y ?? 0)));
    const before = centroid();

    pressed.fx = pressed.x;
    pressed.fy = pressed.y;
    sim.alpha(0.3).alphaTarget(0.3);
    sim.tick(60);

    const after = centroid();
    // The shelf sits off-centre from the core (in the wings, or below it on
    // overflow rows), so a centre force that averaged every node would see a
    // lopsided centroid every tick and walk the core to compensate — the
    // zones would drift apart. Each component holding its OWN centroid (not
    // a shared one across the whole graph) is what keeps the core from
    // chasing the shelf's mass.
    expect(Math.hypot(after.x - before.x, after.y - before.y)).toBeLessThan(span * 0.08);
  });

  /** Runs AtlasView's gesture — pin `dragged` 50 units away, jump alpha to
   *  0.3, reheat — once per compass direction, and returns the MEAN distance
   *  its chain neighbor travelled.
   *
   *  Two measurement traps this avoids. Distance from the neighbor to the
   *  drag target is not the quantity: the link holds the pair at its rest
   *  length, so a neighbor that follows perfectly still stops ~30 units short
   *  and that gap can grow while it is doing exactly the right thing. And a
   *  single direction is not enough: how far a neighbor gets depends on what
   *  is in the way (a shelved component dragged sideways runs into the one
   *  beside it on its row and stops at 7 units, while the same drag downward
   *  covers 44), so one axis measures collide, not the anchor. */
  function meanNeighborTravel(sizes: number[], dragged: string) {
    const neighbor = dragged.replace("n0", "n1");
    const travels = ([
      [50, 0],
      [-50, 0],
      [0, 50],
      [0, -50],
    ] as const).map(([dx, dy]) => {
      const graph = buildAtlasGraph(componentsModel(sizes), PALETTE);
      runAtlasLayout(graph);
      const sim = createAtlasSimulation(graph);
      const byId = new Map(sim.nodes().map((n) => [n.id, n]));
      const pressed = byId.get(dragged) as { fx?: number | null; fy?: number | null; x?: number; y?: number };
      const follower = byId.get(neighbor) as { x?: number; y?: number };
      const from = { x: follower.x ?? 0, y: follower.y ?? 0 };
      pressed.fx = (pressed.x ?? 0) + dx;
      pressed.fy = (pressed.y ?? 0) + dy;
      sim.alpha(0.3).alphaTarget(0.3);
      sim.tick(120);
      sim.stop();
      return Math.hypot((follower.x ?? 0) - from.x, (follower.y ?? 0) - from.y);
    });
    return travels.reduce((a, b) => a + b, 0) / travels.length;
  }

  it("pulls a shelved component's neighbor along about as readily as a core one", () => {
    const core = meanNeighborTravel([9, 5], "c0n0");
    const shelf = meanNeighborTravel([9, 5], "c1n0");
    // The bug this round exists to fix: under the old fx/fy freeze a shelved
    // neighbor travelled exactly 0, in every direction.
    expect(shelf).toBeGreaterThan(10);
    // "Dragging should feel the same everywhere" — the shelf anchor is a
    // tenth of a link's strength precisely so a hand still wins.
    expect(shelf).toBeGreaterThan(0.5 * core);
  });

  it("holds an untouched component's centroid while a NEIGHBORING shelf component is dragged", () => {
    const graph = buildAtlasGraph(componentsModel([9, 5, 5]), PALETTE);
    runAtlasLayout(graph);
    const sim = createAtlasSimulation(graph);
    const byId = new Map(sim.nodes().map((n) => [n.id, n]));
    const untouched = ["c2n0", "c2n1", "c2n2", "c2n3", "c2n4"].map(
      (id) => byId.get(id) as { x?: number; y?: number },
    );
    const centroid = () => ({
      x: untouched.reduce((sum, n) => sum + (n.x ?? 0), 0) / untouched.length,
      y: untouched.reduce((sum, n) => sum + (n.y ?? 0), 0) / untouched.length,
    });
    const before = centroid();

    const dragged = byId.get("c1n0") as { fx?: number | null; fy?: number | null; x?: number; y?: number };
    dragged.fx = (dragged.x ?? 0) + 50;
    dragged.fy = dragged.y ?? 0;
    sim.alpha(0.3).alphaTarget(0.3);
    sim.tick(60);

    const after = centroid();
    expect(Math.hypot(after.x - before.x, after.y - before.y)).toBeLessThan(0.5);
  });

  it("lets a TWO-node shelf component follow a drag instead of clamping its one free node to the anchor", () => {
    const graph = buildAtlasGraph(componentsModel([9, 2]), PALETTE);
    runAtlasLayout(graph);
    const sim = createAtlasSimulation(graph);
    const byId = new Map(sim.nodes().map((n) => [n.id, n]));
    const dragged = byId.get("c1n0") as { fx?: number | null; fy?: number | null; x?: number; y?: number };
    const neighbor = byId.get("c1n1") as { x?: number; y?: number };
    const from = { x: neighbor.x ?? 0, y: neighbor.y ?? 0 };

    // A pair is the worst case for any group-centroid hold: pin one node and
    // the other is the group's ENTIRE free set, so "hold the free centroid at
    // the group target" degenerates into "teleport the neighbor to the middle
    // of the slot", away from the hand. Drag far enough that following and
    // clamping cannot be confused — a clamped neighbor stays put whatever the
    // drag distance, a following one tracks it.
    dragged.fx = (dragged.x ?? 0) + 200;
    dragged.fy = dragged.y ?? 0;
    sim.alpha(0.3).alphaTarget(0.3);
    sim.tick(60);

    const travel = Math.hypot((neighbor.x ?? 0) - from.x, (neighbor.y ?? 0) - from.y);
    expect(travel).toBeGreaterThan(100);
    // And it went WITH the drag, not to the group's own centre.
    expect((neighbor.x ?? 0) - from.x).toBeGreaterThan(100);
  });

  /** The real mouseup path: pin a shelved node, haul the component `distance`
   *  units toward the core (the shelf is below it, so +y), then let go the way
   *  AtlasView does — clear fx/fy, alphaTarget(0), and let the simulation cool
   *  to alphaMin. Returns where the component ended up relative to its slot. */
  function dragAndRelease(distance: number, reducedMotion = false) {
    const graph = buildAtlasGraph(componentsModel([9, 2]), PALETTE);
    runAtlasLayout(graph);
    const sim = createAtlasSimulation(graph);
    const core = Array.from({ length: 9 }, (_, i) => `c0n${i}`);
    const shelf = ["c1n0", "c1n1"];
    const centroidOf = (group: string[]) => ({
      x: group.reduce((sum, id) => sum + (graph.getNodeAttribute(id, "x") as number), 0) / group.length,
      y: group.reduce((sum, id) => sum + (graph.getNodeAttribute(id, "y") as number), 0) / group.length,
    });
    const slot = centroidOf(shelf);

    const dragged = sim.nodes().find((n) => n.id === "c1n0") as {
      fx?: number | null;
      fy?: number | null;
      x?: number;
      y?: number;
    };
    dragged.fx = dragged.x;
    dragged.fy = (dragged.y ?? 0) + distance;
    sim.alpha(0.3).alphaTarget(0.3);
    sim.tick(60);

    dragged.fx = null;
    dragged.fy = null;
    if (reducedMotion) {
      sim.settleShelf();
      sim.stop();
    } else {
      sim.alphaTarget(0);
      // alphaDecay 0.03 from 0.3 reaches alphaMin in ~190; the cap only keeps
      // a regression here from hanging the suite.
      for (let i = 0; i < 400 && sim.alpha() >= sim.alphaMin(); i += 1) sim.tick(1);
      sim.stop();
    }

    const after = centroidOf(shelf);
    const overlapsDiscs = core.some((a) =>
      shelf.some((b) => {
        const ax = graph.getNodeAttribute(a, "x") as number;
        const ay = graph.getNodeAttribute(a, "y") as number;
        const bx = graph.getNodeAttribute(b, "x") as number;
        const by = graph.getNodeAttribute(b, "y") as number;
        const ar = graph.getNodeAttribute(a, "size") as number;
        const br = graph.getNodeAttribute(b, "size") as number;
        return Math.hypot(ax - bx, ay - by) < ar + br;
      }),
    );
    return {
      offset: Math.hypot(after.x - slot.x, after.y - slot.y),
      overlapsCore: overlapsDiscs,
    };
  }

  it("walks a RELEASED component all the way back to its slot, not part of the way", () => {
    // The failure this catches: hold a component's bulk velocity at zero and
    // the spring's own return velocity is cancelled again every tick, leaving
    // one alpha-scaled impulse per tick to do the whole journey — and alpha is
    // a finite budget. Measured that way, this 200-unit drag settled 77.5
    // units from its slot with its box 13 units inside the core's; the
    // 100-unit drag settled 33.4 out, past the gap that is supposed to
    // separate the zones. Steering the bulk velocity home instead
    // (SHELF_RETURN_RATE) is not alpha-scaled, so the return completes.
    const far = dragAndRelease(200);
    expect(far.offset).toBeLessThan(2);
    expect(far.overlapsCore).toBe(false);
    const near = dragAndRelease(100);
    expect(near.offset).toBeLessThan(2);
    expect(near.overlapsCore).toBe(false);
  });

  it("puts a released component back on its slot in ONE step when reduced motion skips the cooling tail", () => {
    // AtlasView's mouseup stops the simulation outright under reduced motion,
    // so there is no tail to walk the component home — without settleShelf it
    // simply stays where the drag left it, 61 units off slot and overlapping
    // the core.
    const settled = dragAndRelease(200, true);
    expect(settled.offset).toBeLessThan(0.5);
    expect(settled.overlapsCore).toBe(false);
  });

  it("keeps a long shelf component from swinging its far end up into the core over a long reheat", () => {
    // Chains this long are the case a centroid hold cannot catch: holding the
    // CENTRE says nothing about which way a body points, and an elongated
    // component in the core's repulsion field pivots to point at the core —
    // its near edge closes in even though its centroid has barely moved
    // (measured on this fixture: the five-node left-wing chain's centroid
    // drifted under 1e-13 over 120 ticks while its clearance from the core
    // fell by ~21 of the 60-unit gap the packing left. Nowhere near an
    // overlap, and the per-node springs are what stop it going further —
    // a centroid-only hold has nothing to say about rotation at all).
    const graph = buildAtlasGraph(componentsModel([24, 6, 5]), PALETTE);
    runAtlasLayout(graph);
    const sim = createAtlasSimulation(graph);
    const ids = [24, 6, 5].map((size, c) => Array.from({ length: size }, (_, i) => `c${c}n${i}`));
    const centroidOf = (group: string[]) => {
      const xs = group.map((id) => graph.getNodeAttribute(id, "x") as number);
      const ys = group.map((id) => graph.getNodeAttribute(id, "y") as number);
      return { x: xs.reduce((a, b) => a + b, 0) / xs.length, y: ys.reduce((a, b) => a + b, 0) / ys.length };
    };
    const before = ids.map(centroidOf);
    const boxBefore = ids.map((group) => box(graph, group));

    const pressed = sim.nodes().find((n) => n.id === "c0n0") as {
      fx?: number | null;
      fy?: number | null;
      x?: number;
      y?: number;
    };
    pressed.fx = pressed.x;
    pressed.fy = pressed.y;
    sim.alpha(0.3).alphaTarget(0.3);
    sim.tick(120);

    for (let i = 1; i < ids.length; i += 1) {
      const shelf = box(graph, ids[i] as string[]);
      const overlaps = ids[0].some((coreId) =>
        ids[i].some((shelfId) => {
          const coreX = graph.getNodeAttribute(coreId, "x") as number;
          const coreY = graph.getNodeAttribute(coreId, "y") as number;
          const shelfX = graph.getNodeAttribute(shelfId, "x") as number;
          const shelfY = graph.getNodeAttribute(shelfId, "y") as number;
          const coreSize = graph.getNodeAttribute(coreId, "size") as number;
          const shelfSize = graph.getNodeAttribute(shelfId, "size") as number;
          return Math.hypot(coreX - shelfX, coreY - shelfY) < coreSize + shelfSize;
        }),
      );
      expect(overlaps).toBe(false);
      // Measured on the island packing: the island's own box does not move
      // much (every edge within 8 units over 120 ticks) — it is the CORE
      // that breathes out a little under the hold, which is its own
      // business and is what the disc overlap check above covers.
      const shelfBefore = boxBefore[i] as ReturnType<typeof box>;
      for (const side of ["minX", "maxX", "minY", "maxY"] as const) {
        expect(Math.abs(shelf[side] - shelfBefore[side])).toBeLessThan(8);
      }
      const after = centroidOf(ids[i] as string[]);
      const b = before[i] as { x: number; y: number };
      expect(Math.hypot(after.x - b.x, after.y - b.y)).toBeLessThan(0.5);
    }
  });

  it("opens a knot: an overlapping dense cluster clears every pair of discs once settled", () => {
    const spokeCount = 20;
    const nodes: GraphNode[] = [node({ id: "hub", entityType: "page", degree: spokeCount })];
    const edges: GraphEdge[] = [];
    for (let i = 0; i < spokeCount; i += 1) {
      const id = `p${i}`;
      // Ring neighbor plus the hub spoke gives every page node degree 2+, so
      // none of them is a satellite (see nonSimulatedIds) — the whole
      // cluster stays in the sim, discs and all.
      nodes.push(node({ id, entityType: "page", degree: 2 }));
      edges.push(edge({ id: `h${i}`, source: "hub", target: id, type: "about" }));
    }
    for (let i = 0; i < spokeCount; i += 1) {
      edges.push(
        edge({ id: `ring${i}`, source: `p${i}`, target: `p${(i + 1) % spokeCount}`, type: "wikilink" }),
      );
    }
    const graph = buildAtlasGraph(makeModel(nodes, edges), PALETTE);
    // Seeded overlapping — every node on the same point, standing in for the
    // real capture's "Lucian Threads" cluster, which froze mid-settle as a
    // collapsed knot under the old fx/fy-pin design (see the round 5 spec).
    graph.forEachNode((id) => {
      graph.setNodeAttribute(id, "x", 0);
      graph.setNodeAttribute(id, "y", 0);
    });

    const sim = createAtlasSimulation(graph);
    const simNodes = sim.nodes();
    let minClearance = Infinity;
    for (let i = 0; i < simNodes.length; i += 1) {
      for (let j = i + 1; j < simNodes.length; j += 1) {
        const a = simNodes[i] as AtlasSimNode;
        const b = simNodes[j] as AtlasSimNode;
        const dist = Math.hypot((a.x ?? 0) - (b.x ?? 0), (a.y ?? 0) - (b.y ?? 0));
        minClearance = Math.min(minClearance, dist - (a.radius + b.radius));
      }
    }
    // Every pair of discs cleared, not just ring neighbors — the knot opened
    // rather than merely stretching along one axis. Collide runs at strength
    // 0.7 (not 1), so a linked pair settles right at its two radii's shared
    // boundary rather than genuinely apart — measured clearance here is
    // -0.00007, float noise around exactly touching, not real overlap; the
    // knot started at clearance ~-16 (every node stacked on (0,0)).
    expect(minClearance).toBeGreaterThan(-0.01);
  });
});
