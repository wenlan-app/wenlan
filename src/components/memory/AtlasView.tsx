// SPDX-License-Identifier: AGPL-3.0-only
import { WorkspaceBackButton } from "./navigation/WorkspaceNavigation";
import { TabEmptyMark } from "../onboarding/TabIntro";
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { KeyboardEvent as ReactKeyboardEvent } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import Graph from "graphology";
import Sigma from "sigma";
import { EdgeRectangleProgram } from "sigma/rendering";
import {
  ArrowCounterClockwise,
  CaretDown,
  CornersOut,
  Minus,
  Funnel,
  Plus,
  SlidersHorizontal,
} from "@phosphor-icons/react";
import {
  captureAtlasViewpoint,
  frameAtlasView,
  restoreAtlasViewpoint,
  type AtlasFrameMode,
  type AtlasViewpoint,
} from "../../lib/graph/viewpoint";
import {
  overviewLabelMaxWidth,
  selectOverviewFallbackLabels,
  truncateOverviewLabel,
} from "../../lib/graph/overviewLabels";
import "./atlasControls.css";
import "./assets/collectionToolbar.css";
import AtlasInspector from "./AtlasInspector";
import AtlasSelect from "./AtlasSelect";
import AtlasTooltip from "./AtlasTooltip";
import AtlasTypeFilters from "./AtlasTypeFilters";
import { entityTypeHidden, filterGraphEntityTypes } from "../../lib/graph/typeFilter";
import { getKnowledgeGraph } from "../../lib/tauri";
import type { Entity, KnowledgeGraph } from "../../lib/tauri";
import {
  buildKnowledgeGraphModel,
  entitySpace,
  filterKnowledgeGraph,
  memorySourceId,
  pageIdOf,
  pageNodeId,
  drawableModel,
  attachMemories,
  smallGroupNodeCount,
  DEFAULT_LAYERS,
  MEMORY_NODE_TYPE,
  PAGE_NODE_TYPE,
} from "../../lib/graph/model";
import type { GraphLayers, GraphModel, GraphNode } from "../../lib/graph/model";
import {
  buildAtlasGraph,
  applyAtlasHierarchy,
  runAtlasLayout,
  createAtlasSimulation,
  hoverStateFor,
  nodeDisplay,
  edgeDisplay,
  labelAnchor,
  lodFor,
  OPENING_LOD,
  drawNodeLabelAt,
  layoutFocusLabels,
  NEIGHBOR_LABEL_MAX,
  NODE_LABEL_FONT,
} from "../../lib/graph/atlas";
import type { HoverState, AtlasSimulation, LodState, LabelPlacement } from "../../lib/graph/atlas";
import { dustBadgeAnchors, drawDustCounts } from "../../lib/graph/dust";
import type { CartographyScene } from "../../lib/graph/cartography";
import {
  communitiesFor,
  cartographyScene,
  drawRegionNames,
  drawRegionAreas,
  isUnscopedSpace,
  MIN_REGION_SIZE,
} from "../../lib/graph/cartography";
import { useGraphPalette, colorForEntityType, nodeFillFor } from "../../lib/graph/palette";
import type { GraphPalette } from "../../lib/graph/palette";
import { fetchCartographyForSpaces, aggregateCartographyStatus } from "../../lib/graph/community";
import type { SpaceCartography } from "../../lib/graph/community";

/** Keep connection strokes in screen pixels while nodes follow graph spacing. */
class ScreenSpaceEdgeProgram extends EdgeRectangleProgram {
  override setUniforms(
    params: Parameters<EdgeRectangleProgram["setUniforms"]>[0],
    programInfo: Parameters<EdgeRectangleProgram["setUniforms"]>[1],
  ) {
    super.setUniforms({ ...params, sizeRatio: 1 }, programInfo);
  }
}

// One shared empty map for the unresolved query. An inline `new Map()` default
// mints a fresh identity on every render, and this map feeds the memoized
// community climb and the place-name overlay — a new identity re-runs the climb
// and repaints every edge each render until the fetch lands.
const EMPTY_CARTOGRAPHY: Map<string, SpaceCartography> = new Map();

// Same reason as EMPTY_CARTOGRAPHY: a stable identity for the unresolved
// graph query, so the model memo below doesn't rebuild on every render.
const EMPTY_GRAPH: KnowledgeGraph = {
  entities: [],
  relations: [],
  memories: [],
  memory_links: [],
  pages: [],
  page_links: [],
};

/** Where the layer choice survives a reload. */
const LAYERS_STORAGE_KEY = "atlas.layers";

/** Where the small-groups choice is remembered across reloads. */
export const SMALL_GROUPS_STORAGE_KEY = "atlas.smallGroups";

/** Read the persisted small-groups choice. Only a stored `true` turns them
 *  on; anything malformed — bad JSON, a number, a string — leaves them
 *  hidden, which is the default the map is designed around. */
export function readStoredSmallGroups(raw: string | null): boolean {
  if (raw === null) return false;
  try {
    return JSON.parse(raw) === true;
  } catch {
    return false;
  }
}

/** Read the persisted layer choice. Anything malformed — bad JSON, a
 *  non-object, a non-boolean field, or all three off — falls back to the
 *  default rather than half-applying a broken value. */
export function readStoredLayers(raw: string | null): GraphLayers {
  if (raw === null) return DEFAULT_LAYERS;
  try {
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed !== "object" || parsed === null) return DEFAULT_LAYERS;
    const record = parsed as Record<string, unknown>;
    const layers: GraphLayers = {
      entity: record.entity,
      page: record.page,
      memory: record.memory,
    } as GraphLayers;
    if (
      typeof layers.entity !== "boolean" ||
      typeof layers.page !== "boolean" ||
      typeof layers.memory !== "boolean"
    ) {
      return DEFAULT_LAYERS;
    }
    if (!layers.entity && !layers.page && !layers.memory) return DEFAULT_LAYERS;
    return layers;
  } catch {
    return DEFAULT_LAYERS;
  }
}

/** What a click on a drawn node resolves to. Memories carry their own
 *  `source_id`, not the prefixed graph-node id. */
export type AtlasNodeTarget =
  | { kind: "entity"; id: string }
  | { kind: "memory"; id: string }
  | { kind: "page"; id: string };

/** The node id a click landed on, resolved to a navigable target. */
function targetForNode(nodeId: string): AtlasNodeTarget {
  const sourceId = memorySourceId(nodeId);
  if (sourceId !== null) return { kind: "memory", id: sourceId };
  const pageId = pageIdOf(nodeId);
  if (pageId !== null) return { kind: "page", id: pageId };
  return { kind: "entity", id: nodeId };
}

interface AtlasViewProps {
  onNodeClick?: (target: AtlasNodeTarget) => void;
  // Initial framing: center this entity with its neighborhood emphasized
  // (EntityDetail's overlay "Atlas" mode). Applied instantly on mount — a
  // starting frame, not a transition — so no camera animation.
  focusEntityId?: string;
  /** Initial camera target in the whole graph; never restricts its membership. */
  focusPageId?: string;
  // Main.tsx's Graph view passes navigateBack; renders a back button as the
  // first toolbar item (a floating one would sit on the search box).
  onBack?: () => void;
}

// jsdom has no matchMedia; treat its absence as "no preference" rather than
// throwing (see the mouseup wiring below, which is exercised by tests that
// don't stub it).
function prefersReducedMotion(): boolean {
  if (typeof window === "undefined" || typeof window.matchMedia !== "function") return false;
  return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

/**
 * sigma-rendered whole-graph view — the shipped Graph tab (Main.tsx) and the
 * entity overlay's "Atlas" mode. Replaced the canvas ConstellationMap; the
 * query keys keep the "constellation-" prefix so nothing else invalidates.
 */
export default function AtlasView({ onNodeClick, focusEntityId, focusPageId, onBack }: AtlasViewProps) {
  const { t } = useTranslation();
  const effectiveFocusId = focusPageId ? pageNodeId(focusPageId) : focusEntityId;
  const palette = useGraphPalette();
  const containerRef = useRef<HTMLDivElement>(null);
  const sigmaRef = useRef<Sigma | null>(null);
  const rendererDisposeRef = useRef<(() => void) | null>(null);
  const viewpointRef = useRef<{ scope: string | null; view: AtlasViewpoint; body: Map<string, { x: number; y: number }> } | null>(null);
  const graphRef = useRef<Graph | null>(null);
  const simRef = useRef<AtlasSimulation | null>(null);
  // Reducer inputs, read from refs so hover/theme changes repaint without a
  // React re-render or a renderer rebuild (see the mount effect below).
  const hoverStateRef = useRef<HoverState>({ hovered: null, neighbors: new Set() });
  // Names of the focused neighborhood, laid out once per (focus, camera) and
  // reused for every label sigma paints in that frame — see layoutFocusLabels.
  const focusLayoutRef = useRef<{ key: string; placements: Map<string, LabelPlacement> } | null>(null);
  const paletteRef = useRef<GraphPalette>(palette);
  // Zoom level of detail the reducers and the overlay read at paint time:
  // how much of each anchor's memory dust is drawn, and whether the islands
  // are solid yet. Set from the camera on every move (see the mount effect).
  const lodRef = useRef<LodState>(OPENING_LOD);
  // Node-drag state: which node (if any) is being dragged, and whether the
  // pointer actually moved during the current press — the latter gates
  // clickNode so a drag-release doesn't also fire entity navigation.
  const draggedNodeRef = useRef<string | null>(null);
  const movedDuringPressRef = useRef(false);
  // Cached cartography scene (the named regions). Rebuilding it
  // on every afterRender meant a plain camera pan or a hover re-measured all
  // 66 regions; the scene only actually changes when node positions or
  // communities do, so paints mark it dirty and the afterRender handler
  // rebuilds only then.
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [displayOpen, setDisplayOpen] = useState(false);
  const displayTriggerRef = useRef<HTMLButtonElement>(null);
  const displayPanelRef = useRef<HTMLDivElement>(null);
  const displayOpenRef = useRef(false);
  displayOpenRef.current = displayOpen;
  const selectedRef = useRef<string | null>(null);
  const overviewRef = useRef<AtlasViewpoint | null>(null);
  const pendingFocusRef = useRef<string | null>(null);
  const openingRatioRef = useRef(1);
  const focusNodeRef = useRef<(id: string) => void>(() => {});
  const returnToMapRef = useRef<() => void>(() => {});
  const sceneRef = useRef<CartographyScene | null>(null);
  const sceneDirtyRef = useRef(true);

  // ONE read for the whole graph. This used to be two queries — every entity,
  // then a detail fetch per entity capped at the first 20 — which drew every
  // connected entity outside that top 20 as an isolate.
  const {
    data: graph = EMPTY_GRAPH,
    isLoading: graphLoading,
    isError: graphError,
    refetch: refetchGraph,
  } = useQuery({
    queryKey: ["knowledge-graph"],
    queryFn: () => getKnowledgeGraph(),
    refetchInterval: 120_000,
  });
  const entities = graph.entities;

  // Same field precedence as the entity page (EntityDetail's space-then-domain
  // rule), through model.ts's entitySpace so the list, the filter, and the
  // graph nodes cannot disagree about which space an entity belongs to.
  const spaces = useMemo(
    () =>
      Array.from(
        new Set(entities.map((e: Entity) => entitySpace(e)).filter((s): s is string => !!s)),
      ).sort(),
    [entities],
  );
  const [excludedTypes, setExcludedTypes] = useState<Set<string>>(() => new Set());
  const excludedTypesRef = useRef<ReadonlySet<string>>(excludedTypes);
  excludedTypesRef.current = excludedTypes;
  const [spaceFilter, setSpaceFilter] = useState<string | null>(null);

  // Which node kinds are drawn. Wiki pages and entities on, memories off by
  // default: memories outnumber everything else and bury the map. Persisted
  // across reloads; a malformed stored value falls back to the default.
  const storedLayersRef = useRef<GraphLayers | null>(null);
  if (storedLayersRef.current === null) {
    let stored = DEFAULT_LAYERS;
    if (typeof window !== "undefined") {
      try {
        stored = readStoredLayers(window.localStorage.getItem(LAYERS_STORAGE_KEY));
      } catch {
        stored = DEFAULT_LAYERS;
      }
    }
    storedLayersRef.current = stored;
  }
  const [layers, setLayers] = useState<GraphLayers>(() =>
    focusPageId ? { ...storedLayersRef.current!, page: true } : storedLayersRef.current!,
  );
  const toggleLayer = (key: keyof GraphLayers) => {
    const next = { ...layers, [key]: !layers[key] };
    // The last lit chip can't be turned off — an empty map is not a view.
    if (!next.entity && !next.page && !next.memory) return;
    setLayers(next);
    const preferences = { ...storedLayersRef.current!, [key]: next[key] };
    // focusPageId temporarily turns on the page layer for this view. If an
    // explicit layer change would serialize every stored layer as off, retain
    // the previous valid preference rather than persist an unusable state.
    if (!preferences.page && !preferences.entity && !preferences.memory) return;
    storedLayersRef.current = preferences;
    try {
      window.localStorage.setItem(LAYERS_STORAGE_KEY, JSON.stringify(preferences));
    } catch {
      // Private mode / quota: the choice still applies for this session.
    }
  };
  // Components of fewer than MIN_COMPONENT_SIZE nodes are off the map until
  // the reader asks for them; the chip below is the ask. Persisted like the
  // layer choice, and a malformed stored value falls back to hidden.
  const [showSmallGroups, setShowSmallGroups] = useState<boolean>(() => {
    if (focusPageId) return true;
    if (typeof window === "undefined") return false;
    try {
      return readStoredSmallGroups(window.localStorage.getItem(SMALL_GROUPS_STORAGE_KEY));
    } catch {
      return false;
    }
  });
  const toggleSmallGroups = () => {
    const next = !showSmallGroups;
    setShowSmallGroups(next);
    try {
      window.localStorage.setItem(SMALL_GROUPS_STORAGE_KEY, JSON.stringify(next));
    } catch {
      // Private mode / quota: the choice still applies for this session.
    }
  };
  const revealSmallGroupsForFocus = () => {
    if (showSmallGroups) return;
    setShowSmallGroups(true);
    try {
      window.localStorage.setItem(SMALL_GROUPS_STORAGE_KEY, JSON.stringify(true));
    } catch {
      // Private mode / quota: the choice still applies for this session.
    }
  };

  const onlyLayerOn = (key: keyof GraphLayers) =>
    layers[key] && Object.values(layers).filter(Boolean).length === 1;

  // D13/App-PR readiness, one fetch-and-classify per known space (community.ts):
  // cursor-paginated to exhaustion, generation-checked, never declared ready off
  // a partial read. Keyed on ALL known spaces regardless of spaceFilter, so
  // one space's trouble stays visible while another is being viewed; the
  // unscoped half of the badge is read off the filtered model instead (below).
  const { data: cartographyBySpace = EMPTY_CARTOGRAPHY } = useQuery({
    queryKey: ["constellation-cartography", spaces],
    queryFn: () => fetchCartographyForSpaces(spaces),
    enabled: spaces.length > 0,
    refetchInterval: 120_000,
  });
  // Scoping filters the model INPUTS: the space's own entities and memories,
  // and only the relations/links whose endpoints both survive. Regions,
  // counts, and insights all re-derive from the scoped model.
  const scopedGraph = useMemo(
    () => filterKnowledgeGraph(graph, spaceFilter),
    [graph, spaceFilter],
  );
  const model = useMemo(
    () => buildKnowledgeGraphModel(scopedGraph, { layers }),
    [scopedGraph, layers],
  );
  // The map is SHAPED by the entity and page layers alone. Memories never
  // move anything: with the chip on they are hung on this base as satellites
  // (attachMemories below, placed by atlas.ts), so the map the reader learned
  // with the chip off is the map they get with it on. On real data the
  // memory layer is 2,000 nodes and 4,910 edges — simulated, it re-laid the
  // whole map into a pile on every press.
  const baseModel = useMemo(
    () =>
      layers.memory
        ? buildKnowledgeGraphModel(scopedGraph, { layers: { ...layers, memory: false } })
        : model,
    [scopedGraph, layers, model],
  );

  // What sigma actually draws. Nodes in a connected component smaller than
  // MIN_COMPONENT_SIZE are left out (model.ts) unless the reader turns them
  // on: on real data ~960 of 1,600 entities have no relation at all, and
  // another ~196 sit in small groups of four or fewer. Both buried the map.
  // The count is shown on the toolbar chip that turns them back on.
  // Overlay entry point: a focused entity that sits in a small group would
  // otherwise vanish with it and the mount focus below would silently not
  // fire, so the small groups are drawn for that mount whatever the chip says.
  const visibleModel = useMemo<GraphModel>(() => {
    let drawable = drawableModel(baseModel, showSmallGroups);
    if (
      effectiveFocusId &&
      !drawable.nodes.some((n) => n.id === effectiveFocusId) &&
      baseModel.nodes.some((n) => n.id === effectiveFocusId)
    ) {
      drawable = drawableModel(baseModel, true);
    }
    if (!layers.memory) return drawable;
    // Nothing to hang the memories on (entity and page layers both off):
    // fall back to drawing the memory model as it is.
    if (baseModel.nodes.length === 0) return drawableModel(model, showSmallGroups);
    return attachMemories(drawable, model);
  }, [baseModel, model, layers.memory, showSmallGroups, effectiveFocusId]);
  // Counted off the FULL base model, so the chip keeps its number when the
  // groups are showing and can offer to hide them again — and keeps it when
  // the memory chip flips, since memories never make or break a group.
  const smallGroupCount = useMemo(() => smallGroupNodeCount(baseModel), [baseModel]);

  // Anything in cartography.ts's unscoped bucket is drawn on the fallback
  // climb, so the badge must never read all-durable while such a node is on
  // the map. Asked of the RENDERED model — the very nodes communitiesFor
  // partitions — so the badge and the drawn cartography cannot disagree.
  // Reading the raw entity list instead misses the case the model CREATES
  // rather than carries: under a space filter a relation to another space's
  // entity keeps its endpoint while that entity is filtered away, so
  // buildGraphModel synthesizes it with no space at all. Going through the
  // model also covers the two unfiltered ways in — a relation-only neighbor,
  // and an entity whose own space is null or empty.
  // Memory and wiki-page nodes are exempt: both inherit their community from
  // an entity (cartography.ts), so a spaceless one is not a node drawn on the
  // fallback climb and must not hold the badge back.
  const hasUnscopedFallback = useMemo(
    () =>
      model.nodes.some(
        (n: GraphNode) => n.kind === "entity" && isUnscopedSpace(n.space),
      ),
    [model],
  );
  const cartographyStatus = useMemo(
    () => aggregateCartographyStatus(cartographyBySpace, hasUnscopedFallback),
    [cartographyBySpace, hasUnscopedFallback],
  );
  // Partitioned off the BASE model: memories are dust on their anchors and
  // neither count toward a region nor widen one (cartography.ts), so the
  // places named on the map are the same with the memory layer on and off.
  const communities = useMemo(
    () => communitiesFor(baseModel, cartographyBySpace),
    [baseModel, cartographyBySpace],
  );
  // Mirrors `communities` for the mount effect's afterRender closure (see
  // the cartography-refresh effect below, by the theme-flip effect) — a
  // space's durable status arriving or regressing must repaint the place
  // names without tearing down the sim/camera, so the paint reads this
  // ref at PAINT time instead of closing over the `communities` value that
  // was current when the sigma renderer was built.
  const communitiesRef = useRef<Map<string, string>>(communities);

  const filteredModel = useMemo(() => filterGraphEntityTypes(visibleModel, excludedTypes), [visibleModel, excludedTypes]);
  const entityTypes = useMemo(() => {
    const counts = new Map<string, number>();
    for (const node of visibleModel.nodes) if (node.kind === "entity") counts.set(node.entityType, (counts.get(node.entityType) ?? 0) + 1);
    return [...counts].sort(([a, countA], [b, countB]) => countB - countA || a.localeCompare(b));
  }, [visibleModel]);
  const toggleEntityType = (type: string) => setExcludedTypes((previous) => {
    const next = new Set(previous);
    if (next.has(type)) next.delete(type); else next.add(type);
    return next;
  });

  // Region count for the toolbar count line — membership only, so it agrees
  // with the regions the cartography scene actually names without needing
  // node positions. Counted over the DRAWN nodes for the same reason:
  // communities come from the full model, but a hidden small group's
  // community is not on the map.
  const regionCount = useMemo(() => {
    const groups = new Map<string, GraphNode[]>();
    for (const node of filteredModel.nodes) {
      const community = communities.get(node.id);
      if (community === undefined) continue;
      const list = groups.get(community);
      if (list) list.push(node);
      else groups.set(community, [node]);
    }
    let count = 0;
    for (const members of groups.values()) {
      if (members.length >= MIN_REGION_SIZE) count += 1;
    }
    return count;
  }, [filteredModel, communities]);

  // Toolbar search (artifact screen 01): type → listbox of entity names,
  // Enter/click → camera fly + the same emphasis hover applies.
  const searchInputRef = useRef<HTMLInputElement>(null);
  const [query, setQuery] = useState("");
  const [activeIndex, setActiveIndex] = useState(0);
  const [searchFocused, setSearchFocused] = useState(false);
  const restoreSearchFocusOnCloseRef = useRef(false);
  useEffect(() => {
    if (selectedId !== null || !restoreSearchFocusOnCloseRef.current) return;
    restoreSearchFocusOnCloseRef.current = false;
    searchInputRef.current?.focus();
  }, [selectedId]);
  // Start with a clean map; Regions reveals community contours and names.
  // The count line keeps reporting regions either way.
  // Ref mirror so the sigma mount effect (which recreates the overlay per
  // model) can apply the current choice without re-running on toggle.
  const [showRegions, setShowRegions] = useState(false);
  const showRegionsRef = useRef(false);
  const overlayRef = useRef<HTMLCanvasElement | null>(null);
  const areasRef = useRef<HTMLCanvasElement | null>(null);

  const matches = useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (!needle) return [];
    return model.nodes.filter((node) => !entityTypeHidden(node.entityType, excludedTypes) && node.name.toLowerCase().includes(needle)).slice(0, 8);
  }, [model, query, excludedTypes]);


  const focusEntity = (nodeId: string) => {
    setQuery("");
    setActiveIndex(0);
    searchInputRef.current?.blur();
    const renderer = sigmaRef.current;
    const drawn = graphRef.current;
    // A degree-0 node is not on the map at all (see visibleModel), but search
    // can still name it. Reveal hidden small groups first so the same search
    // action can select the node in the map, including in the browser preview
    // where no details callback is present.
    if (!drawn || !drawn.hasNode(nodeId)) {
      if (model.nodes.some((node) => node.id === nodeId)) {
        pendingFocusRef.current = nodeId;
        revealSmallGroupsForFocus();
      }
      return;
    }
    const graph = drawn;
    if (!renderer) return;
    if (!selectedRef.current) overviewRef.current = captureAtlasViewpoint(renderer, graph);
    selectedRef.current = nodeId;
    setSelectedId(nodeId);
    // Selection persists while reading the inspector or traversing neighbors.
    hoverStateRef.current = hoverStateFor(graph, nodeId);
    const display = renderer.getNodeDisplayData(nodeId);
    if (display) {
      const camera = renderer.getCamera();
      // Ratio only ever shrinks (zooms in) — landing further out than the
      // current view would read as the map running away from the match.
      const state = { x: display.x, y: display.y, ratio: Math.min(camera.ratio, 1, openingRatioRef.current / 2.5) };
      if (prefersReducedMotion()) camera.setState(state);
      else camera.animate(state, { duration: 450 });
    }
    renderer.refresh();
  };

  focusNodeRef.current = focusEntity;
  const returnToMap = () => {
    selectedRef.current = null;
    setSelectedId(null);
    restoreSearchFocusOnCloseRef.current = true;
    const renderer = sigmaRef.current;
    const drawn = graphRef.current;
    if (renderer && drawn) {
      hoverStateRef.current = hoverStateFor(drawn, null);
      if (overviewRef.current) restoreAtlasViewpoint(renderer, drawn, overviewRef.current);
      const camera = renderer.getCamera();
      // setState does not cancel Sigma's pending fly-to frame. Replace that
      // animation with the restored state so an immediate Escape stays put.
      if (camera.isAnimated()) void camera.animate({ x: camera.x, y: camera.y, ratio: camera.ratio, angle: camera.angle }, { duration: 1 });
      renderer.refresh();
    }
    overviewRef.current = null;
  };
  returnToMapRef.current = returnToMap;
  const frameMap = (mode: AtlasFrameMode) => {
    const renderer = sigmaRef.current;
    const drawn = graphRef.current;
    selectedRef.current = null;
    setSelectedId(null);
    overviewRef.current = null;
    focusLayoutRef.current = null;
    if (!renderer || !drawn) return;

    const camera = renderer.getCamera();
    const wasAnimated = camera.isAnimated();
    const visibleNodeIds = new Set(
      drawn.nodes().filter((id) => !entityTypeHidden(drawn.getNodeAttribute(id, "entityType"), excludedTypesRef.current)),
    );
    frameAtlasView(renderer, drawn, { mode, visibleNodeIds, padding: 56 });
    // Replace a pending search animation with the NEW frame. Animating the
    // old state before fitting would overwrite the fit on its next frame.
    if (wasAnimated) void camera.animate({ x: camera.x, y: camera.y, ratio: camera.ratio, angle: camera.angle }, { duration: 1 });
    hoverStateRef.current = hoverStateFor(drawn, null);
    lodRef.current = OPENING_LOD;
    openingRatioRef.current = camera.ratio;
    renderer.refresh();
  };
  const zoomMap = (factor: number) => {
    const renderer = sigmaRef.current;
    if (!renderer) return;
    const camera = renderer.getCamera();
    const ratio = camera.getBoundedRatio(camera.ratio * factor);
    if (ratio === camera.ratio) return;
    const state = { x: camera.x, y: camera.y, ratio, angle: camera.angle };
    if (prefersReducedMotion()) camera.setState(state);
    else void camera.animate(state, { duration: 180 });
  };
  useEffect(() => {
    const escape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      const target = event.target as Element | null;
      const inDisplayControls = displayOpenRef.current && (
        displayPanelRef.current?.contains(target) ||
        displayTriggerRef.current?.contains(target) ||
        target?.closest?.(".atlas-type-filter-panel")
      );
      if (inDisplayControls) return;
      if (selectedRef.current) returnToMapRef.current();
    };
    window.addEventListener("keydown", escape);
    return () => window.removeEventListener("keydown", escape);
  }, []);
  useEffect(() => {
    if (!displayOpen) return;
    const isInside = (target: EventTarget | null) => {
      const node = target as Node | null;
      const element = target as Element | null;
      return !!node && (
        displayPanelRef.current?.contains(node) ||
        displayTriggerRef.current?.contains(node) ||
        !!element?.closest?.(".atlas-type-filter-panel")
      );
    };
    const dismissPointer = (event: PointerEvent) => {
      if (!isInside(event.target)) setDisplayOpen(false);
    };
    const dismissFocus = (event: FocusEvent) => {
      if (!isInside(event.target)) setDisplayOpen(false);
    };
    const dismissEscape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      // Nested selectors handle Escape first and stop propagation. This outer
      // handler only closes the display disclosure and restores its trigger.
      event.preventDefault();
      event.stopPropagation();
      setDisplayOpen(false);
      displayTriggerRef.current?.focus();
    };
    document.addEventListener("pointerdown", dismissPointer);
    document.addEventListener("focusin", dismissFocus);
    document.addEventListener("keydown", dismissEscape);
    return () => {
      document.removeEventListener("pointerdown", dismissPointer);
      document.removeEventListener("focusin", dismissFocus);
      document.removeEventListener("keydown", dismissEscape);
    };
  }, [displayOpen]);
  useEffect(() => {
    if (selectedRef.current && !filteredModel.nodes.some((node) => node.id === selectedRef.current)) {
      selectedRef.current = null;
      setSelectedId(null);
      overviewRef.current = null;
    }
  }, [filteredModel]);
  useEffect(() => {
    selectedRef.current = null;
    setSelectedId(null);
    overviewRef.current = null;
  }, [spaceFilter]);
  const selectedNode = filteredModel.nodes.find((node) => node.id === selectedId);
  const selectedNeighbors = useMemo(() => {
    if (!selectedId) return [];
    const ids = new Set<string>();
    for (const edge of filteredModel.edges) {
      if (edge.source === selectedId && edge.target !== selectedId) ids.add(edge.target);
      if (edge.target === selectedId && edge.source !== selectedId) ids.add(edge.source);
    }
    return filteredModel.nodes.filter((node) => ids.has(node.id)).sort((a, b) =>
      Number(a.kind === "memory") - Number(b.kind === "memory") || b.degree - a.degree || a.name.localeCompare(b.name));
  }, [selectedId, filteredModel]);

  useEffect(() => {
    const renderer = sigmaRef.current;
    const drawn = graphRef.current;
    if (drawn && hoverStateRef.current.hovered && entityTypeHidden(drawn.getNodeAttribute(hoverStateRef.current.hovered, "entityType"), excludedTypes)) {
      hoverStateRef.current = hoverStateFor(drawn, null);
    }
    focusLayoutRef.current = null;
    sceneDirtyRef.current = true;
    renderer?.refresh();
  }, [excludedTypes]);

  const onSearchKeyDown = (e: ReactKeyboardEvent<HTMLInputElement>) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setActiveIndex((i) => Math.min(i + 1, Math.max(matches.length - 1, 0)));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActiveIndex((i) => Math.max(i - 1, 0));
    } else if (e.key === "Enter") {
      // Opening the inspector focuses its close button. Consume Enter so the
      // browser cannot activate that newly focused button with the same key.
      e.preventDefault();
      const match = matches[activeIndex] ?? matches[0];
      if (match) focusEntity(match.id);
    } else if (e.key === "Escape") {
      setQuery("");
      searchInputRef.current?.blur();
    }
  };

  // Mount/rebuild sigma whenever the model changes. `palette` is read here
  // (fresh at build time) but deliberately not a dependency — a theme flip
  // recolors the existing graph in place (below) instead of tearing down and
  // remounting the whole renderer. Dispose before the canvas leaves the DOM;
  // a focus animation can otherwise render into a detached, zero-width host.
  useLayoutEffect(() => {
    // Dispose the current imperative renderer before React mutates or detaches
    // its host. Mount/rebuild stays passive so large layouts do not block paint.
    return () => rendererDisposeRef.current?.();
  }, [visibleModel]);

  useEffect(() => {
    const container = containerRef.current;
    // Guarded on the FULL model, not the drawn one: a graph whose components
    // are all smaller than MIN_COMPONENT_SIZE draws nothing but must still
    // mount, so the map frame and the "N unconnected or paired, hidden" chip
    // explain the emptiness instead of the view silently showing a blank slot.
    if (!container || model.nodes.length === 0) return;

    const graph = buildAtlasGraph(visibleModel, palette);
    runAtlasLayout(graph);
    graphRef.current = graph;
    hoverStateRef.current = hoverStateFor(graph, null);
    sceneRef.current = null;
    sceneDirtyRef.current = true;

    // Same-frame paint per physics step (see createAtlasSimulation's onTick
    // note). sigmaRef is still null during the synchronous settle ticks, so
    // the 220 pre-paint steps don't render.
    const sim = createAtlasSimulation(graph, () => {
      sceneDirtyRef.current = true;
      sigmaRef.current?.refresh();
    });
    const previous = viewpointRef.current;
    const bodyIds = graph.nodes().filter((id) => graph.getNodeAttribute(id, "entityType") !== MEMORY_NODE_TYPE);
    if (previous?.scope === spaceFilter && bodyIds.length > 0) {
      // Layer/small-group changes may add or remove nodes. Restore every
      // survivor rather than requiring an identical set, so the learned map
      // stays put while the changed layer settles around it.
      const survivorPositions = new Map(
        bodyIds
          .map((id) => [id, previous.body.get(id)] as const)
          .filter((entry): entry is readonly [string, { x: number; y: number }] => entry[1] !== undefined),
      );
      if (survivorPositions.size > 0) sim.restorePositions(survivorPositions);
    }
    applyAtlasHierarchy(graph);
    // Large maps intentionally label only hierarchy landmarks at overview
    // zoom. Keep one stable name for a represented component that has no
    // landmark (a revealed pair or an all-disconnected young graph), so the
    // view does not erase names solely because their degrees are small.
    const fallbackCandidates: { id: string; x: number; y: number }[] = [];
    const components = new Map<string, string[]>();
    graph.forEachNode((id, attrs) => {
      if (attrs.entityType === MEMORY_NODE_TYPE) return;
      const componentId = attrs.componentId as string | undefined;
      if (!componentId) return;
      const members = components.get(componentId);
      if (members) members.push(id);
      else components.set(componentId, [id]);
    });
    for (const members of components.values()) {
      if (members.some((id) => graph.getNodeAttribute(id, "landmark") === true)) continue;
      const candidate = [...members].sort((a, b) => {
        const degreeDelta = Number(graph.getNodeAttribute(b, "structuralDegree") ?? 0)
          - Number(graph.getNodeAttribute(a, "structuralDegree") ?? 0);
        return degreeDelta || (a < b ? -1 : a > b ? 1 : 0);
      })[0];
      if (candidate) {
        const attrs = graph.getNodeAttributes(candidate);
        fallbackCandidates.push({
          id: candidate,
          x: Number(attrs.x),
          y: Number(attrs.y),
        });
      }
    }
    const overviewFallbackLabels = selectOverviewFallbackLabels(fallbackCandidates);
    const hasMeaningfulComponent = [...components.values()].some((members) =>
      members.some((id) => graph.getNodeAttribute(id, "landmark") === true),
    );
    const showOverviewFallbackLabels = showSmallGroups || !hasMeaningfulComponent;
    simRef.current = sim;
    if (import.meta.env.DEV) {
      // Preview/debug handle only — stripped from prod builds.
      (window as unknown as Record<string, unknown>).__ATLAS_SIM = sim;
    }
    // Community contours sit below the graph; their names sit above it.
    // Both use the same current scene and follow camera moves and drags.
    const overlay = document.createElement("canvas");
    overlay.dataset.testid = "atlas-region-names";
    overlay.style.position = "absolute";
    overlay.style.inset = "0";
    overlay.style.width = "100%";
    overlay.style.height = "100%";
    overlay.style.pointerEvents = "none";
    overlay.style.display = showRegionsRef.current || layers.memory ? "" : "none";
    overlayRef.current = overlay;
    const areas = overlay.cloneNode() as HTMLCanvasElement;
    areas.dataset.testid = "atlas-community-areas";
    areas.style.display = showRegionsRef.current ? "" : "none";
    areasRef.current = areas;

    // The label painter for both the labels layer and the hover layer. With a
    // node focused (hover or search jump) the lit neighborhood's names come
    // from layoutFocusLabels so they never cover each other or a lit dot; a
    // neighborhood name with no free spot is skipped rather than painted over
    // something. Every other label keeps the radial rule. `labelSigma` is
    // bound right after construction; sigma's constructor render runs with
    // nothing focused, so the radial path is all it can reach.
    let labelSigma: Sigma | null = null;
    const focusLayout = (ctx: CanvasRenderingContext2D): Map<string, LabelPlacement> | null => {
      const state = hoverStateRef.current;
      const sigma = labelSigma;
      if (state.hovered === null || sigma === null) return null;
      const camera = sigma.getCamera().getState();
      const { width, height } = sigma.getDimensions();
      const key = `${state.hovered}|${camera.x}|${camera.y}|${camera.ratio}|${camera.angle}|${width}x${height}`;
      if (focusLayoutRef.current?.key === key) return focusLayoutRef.current.placements;
      const nodeAt = (id: string) => {
        const display = sigma.getNodeDisplayData(id);
        if (!display || display.hidden) return null;
        const { x, y } = sigma.framedGraphToViewport(display);
        return { key: id, x, y, size: sigma.scaleSize(display.size), label: display.label ?? "" };
      };
      const focus = nodeAt(state.hovered);
      if (!focus) return null;
      const neighbors =
        state.neighbors.size <= NEIGHBOR_LABEL_MAX
          ? [...state.neighbors].map(nodeAt).filter((n): n is NonNullable<typeof n> => n !== null)
          : [];
      ctx.font = NODE_LABEL_FONT;
      const placements = layoutFocusLabels(focus, neighbors, (text) => ctx.measureText(text).width);
      focusLayoutRef.current = { key, placements };
      return placements;
    };
    // Sigma's grid budgets label count, but long labels can cross cell boundaries.
    // Reserve actual painted rectangles and share them with the region overlay.
    const labelBoxes: { left: number; right: number; top: number; bottom: number }[] = [];
    const reserveLabel = (ctx: CanvasRenderingContext2D, label: string, at: LabelPlacement) => {
      ctx.font = NODE_LABEL_FONT;
      const width = ctx.measureText(label).width;
      const left = at.align === "center" ? at.x - width / 2 : at.align === "right" ? at.x - width : at.x;
      const top = at.baseline === "middle" ? at.y - 6 : at.baseline === "top" ? at.y : at.y - 12;
      const box = { left: left - 4, right: left + width + 4, top: top - 3, bottom: top + 15 };
      if (labelBoxes.some((other) => other.left < box.right && box.left < other.right && other.top < box.bottom && box.top < other.bottom)) return false;
      labelBoxes.push(box);
      return true;
    };
    const drawLabel = (ctx: CanvasRenderingContext2D, data: Record<string, any>, s: Record<string, any>) => {
      // Truncation, collision reservation, and drawNodeLabelAt must all use
      // the same body face or a measured width is not a screen-space bound.
      ctx.font = NODE_LABEL_FONT;
      const placements = focusLayout(ctx);
      if (placements) {
        const at = placements.get(data.key);
        if (at) {
          // Focus labels retain the full source string for inspection and
          // selection; overview truncation is deliberately scoped below.
          const focusedData = { ...data, label: String(data.label || graph.getNodeAttribute(data.key, "label") || "") };
          const placement = at;
          const rawLabel = focusedData.label;
          reserveLabel(ctx, rawLabel, placement);
          drawNodeLabelAt(ctx, focusedData, s, placement, paletteRef.current.surface);
          return;
        }
        const state = hoverStateRef.current;
        if (state.hovered === data.key || (state.neighbors.has(data.key) && state.neighbors.size <= NEIGHBOR_LABEL_MAX)) return;
        // Unrelated nodes are intentionally blank while a neighborhood is in
        // focus; do not recover their raw label from graphology here.
        return;
      }
      const rawLabel = String(
        data.label || (showOverviewFallbackLabels && overviewFallbackLabels.has(data.key)
          ? graph.getNodeAttribute(data.key, "label")
          : "") || "",
      );
      if (!rawLabel) return;
      const overview = lodRef.current.phase === "overview" && hoverStateRef.current.hovered === null;
      if (overview) ctx.font = NODE_LABEL_FONT;
      const maxWidth = overviewLabelMaxWidth(labelSigma?.getDimensions().width ?? 160);
      const label = overview ? truncateOverviewLabel(ctx, rawLabel, maxWidth) : rawLabel;
      if (!label) return;
      const angle = Math.atan2(-(graph.getNodeAttribute(data.key, "y") as number), graph.getNodeAttribute(data.key, "x") as number);
      const sector = Math.round((angle + Math.PI) / (Math.PI / 2)) % 4;
      const at = labelAnchor(data.x, data.y, data.size, sector === 0 ? "right" : sector === 2 ? "left" : sector === 1 ? "below" : "above");
      if (reserveLabel(ctx, label, at)) drawNodeLabelAt(ctx, { ...data, label }, s, at, paletteRef.current.surface);
    };
    let minimumGraphRadius = 0;
    let graphUnitsPerPixel = Infinity;
    const renderer = new Sigma(graph, container, {
      // Collision radii are graph units. Pixel-sized discs used to outgrow
      // their spacing at overview zoom, turning otherwise separated hubs into
      // overlapping blobs. Keep discs and positions on the same zoom scale.
      itemSizesReference: "positions",
      zoomToSizeRatioFunction: (ratio: number) => ratio,
      // Only nodes at least this big carry a label. With the log2 size scale
      // (atlas.ts) that is roughly degree >= 5 for an entity and >= 6 for a
      // page, so the zoomed-out map shows hub names only; sigma's own label
      // grid reveals the rest as you zoom in.
      labelRenderedSizeThreshold: 8,
      // Round 4: with memories on, the size threshold alone still let dozens
      // of labels pile on top of each other. Sigma buckets the viewport into
      // a grid of labelGridCellSize screen px and keeps
      // ceil(labelDensity / cameraRatio^2) labels per cell, biggest node
      // first. At 0.04 that is exactly ONE name per cell for any camera ratio
      // above 0.2 — i.e. one per 120 px of screen at every zoom the map
      // normally sits at — and the cell size is what decides how coarse that
      // thinning is. Zooming in reveals more names because the same graph
      // area then spans more cells, not because the per-cell count rises.
      // 200 px cells: a handful of names at fit zoom, read as landmarks,
      // with room for a radial label to extend without touching the next.
      labelDensity: 0.04,
      labelGridCellSize: 200,
      // Default camera fit maps the graph bbox edge-to-edge on the tighter
      // axis, half-clipping the extreme nodes; give the map a margin.
      stagePadding: 40,
      // Sigma's default label ink is black regardless of theme; pass the
      // resolved text token instead (updated on theme flip below).
      labelColor: { color: palette.label },
      // Off by default in sigma — without it, the zIndex values nodeDisplay/
      // edgeDisplay return are computed but never affect paint order.
      zIndex: true,
      // Sigma's default hover renderer (drawDiscNodeHover) paints a hardcoded
      // #FFF label box — unreadable under the dark theme's light label ink.
      // The focused neighborhood is `highlighted` (nodeDisplay), which sigma
      // redraws on the hover layer above every other label: its discs via the
      // hoverNodes program, its names through this painter, laid out by
      // layoutFocusLabels. The labels-layer copy underneath is fully covered
      // by this one's halo, so a lifted name never reads darker.
      defaultDrawNodeHover: drawLabel,
      // Edges are a 1 px hairline (0.6 for shared-source); sigma's default
      // floor of 1.7 would silently bump them back up.
      minEdgeThickness: 0.5,
      edgeProgramClasses: { line: ScreenSpaceEdgeProgram },
      // 12px body-font labels placed radially around the node, facing the
      // cluster center, over a ground-coloured halo — sigma's default is
      // 14px Arial pinned to the right.
      defaultDrawNodeLabel: drawLabel,
      nodeReducer: (node, attrs) => {
        if (entityTypeHidden(attrs.entityType, excludedTypesRef.current)) return { ...attrs, hidden: true };
        const display = nodeDisplay(hoverStateRef.current, node, attrs, paletteRef.current, lodRef.current);
        if (
          showOverviewFallbackLabels &&
          hoverStateRef.current.hovered === null &&
          !display.hidden &&
          overviewFallbackLabels.has(node) &&
          attrs.label
        ) {
          // `nodeDisplay` blanks non-landmarks for the semantic overview. A
          // revealed small group (or a young graph with no core) still gets
          // one stable, source-backed name at the view layer.
          display.label = attrs.label;
          display.forceLabel = true;
        }
        // A note handoff marks the destination without the hover reducer's
        // one-hop dimming, so panning still reveals the surrounding network.
        if (focusPageId && node === effectiveFocusId && hoverStateRef.current.hovered === null) {
          display.label = attrs.label;
          display.forceLabel = true;
          display.highlighted = true;
        }
        const floor = lodRef.current.phase === "overview" && attrs.landmark ? minimumGraphRadius * 2 : minimumGraphRadius;
        const cap = node === hoverStateRef.current.hovered ? 12 : attrs.entityType === MEMORY_NODE_TYPE ? 2.8 : 8;
        return { ...display, size: Math.min(Math.max(display.size, floor), cap * graphUnitsPerPixel) };
      },
      edgeReducer: (edge, attrs) => {
        const [source, target] = graph.extremities(edge);
        if ([source, target].some((id) => entityTypeHidden(graph.getNodeAttribute(id, "entityType"), excludedTypesRef.current))) return { ...attrs, hidden: true };
        const display = edgeDisplay(
          hoverStateRef.current,
          edge,
          source,
          target,
          attrs,
          paletteRef.current,
          lodRef.current,
          { source: graph.getNodeAttributes(source), target: graph.getNodeAttributes(target) },
        );
        return display;
      },
    });
    labelSigma = renderer;
    sigmaRef.current = renderer;
    container.prepend(areas);
    container.appendChild(overlay);
    if (import.meta.env.DEV) {
      // Preview/debug handle only — stripped from prod builds.
      (window as unknown as Record<string, unknown>).__ATLAS_SIGMA = renderer;
    }

    const dustAnchors = dustBadgeAnchors(graph);
    const drawOverlay = (scene: CartographyScene) => {
      const ctx = overlay.getContext("2d");
      if (!ctx) return; // jsdom
      const { width, height } = renderer.getDimensions();
      const dpr = window.devicePixelRatio || 1;
      if (overlay.width !== width * dpr || overlay.height !== height * dpr) {
        overlay.width = width * dpr;
        overlay.height = height * dpr;
      }
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.clearRect(0, 0, width, height);
      const project = (pos: { x: number; y: number }) => renderer.graphToViewport(pos);
      const lod = lodRef.current;
      // A dim island carries no name yet; its name comes up with its colour.
      const named = lod.islandsSolid ? scene : { regions: scene.regions.filter((r) => !r.island) };
      const areaContext = areas.getContext("2d");
      if (areaContext) {
        if (areas.width !== width * dpr || areas.height !== height * dpr) {
          areas.width = width * dpr;
          areas.height = height * dpr;
        }
        areaContext.setTransform(dpr, 0, 0, dpr, 0, 0);
        areaContext.clearRect(0, 0, width, height);
        if (showRegionsRef.current) drawRegionAreas(areaContext, named, project, paletteRef.current, { width, height });
      }
      // Sigma (3.0.3) keeps the nodes it drew a label for this paint on a
      // private field with no public getter. A region is named after its
      // hub, so while the hub's own label is on screen the region name would
      // print the same word twice; the placer skips it. If a later sigma
      // drops the field the names simply stay on, as before.
      const labelledNodes = (renderer as unknown as { displayedNodeLabels?: ReadonlySet<string> })
        .displayedNodeLabels;
      if (showRegionsRef.current) drawRegionNames(ctx, named, project, paletteRef.current, { width, height }, labelledNodes, labelBoxes);
      drawDustCounts(
        ctx,
        graph,
        dustAnchors.filter((id) => !entityTypeHidden(graph.getNodeAttribute(id, "entityType"), excludedTypesRef.current)),
        project,
        paletteRef.current,
        lod,
        hoverStateRef.current.hovered,
        { width, height },
        renderer.scaleSize(1),
        labelBoxes,
      );
    };
    // One handler paints the overlay from one scene — rebuilt only when a
    // paint marked it dirty — and sigma sees a single afterRender listener.
    renderer.on("beforeRender", () => { labelBoxes.length = 0; });
    renderer.on("afterRender", () => {
      if (sceneDirtyRef.current || sceneRef.current === null) {
        const sceneGraph = excludedTypesRef.current.size ? graph.copy() : graph;
        if (sceneGraph !== graph) for (const id of sceneGraph.nodes()) {
          if (entityTypeHidden(sceneGraph.getNodeAttribute(id, "entityType"), excludedTypesRef.current)) sceneGraph.dropNode(id);
        }
        sceneRef.current = cartographyScene(sceneGraph, communitiesRef.current);
        sceneDirtyRef.current = false;
      }
      drawOverlay(sceneRef.current);
    });
    // The opening frame follows the hierarchy's meaningful components. A
    // later rebuild restores its saved viewpoint below; only a fresh graph
    // receives this initial fit.
    const visibleNodeIds = new Set(
      graph.nodes().filter((id) => !entityTypeHidden(graph.getNodeAttribute(id, "entityType"), excludedTypesRef.current)),
    );
    const saved = viewpointRef.current;
    const restored = saved?.scope === spaceFilter && restoreAtlasViewpoint(renderer, graph, saved.view);
    if (!restored) frameAtlasView(renderer, graph, { mode: "main", visibleNodeIds, padding: 56 });
    // Zoom level of detail, relative to THIS opening view: how much memory
    // dust each anchor shows, and whether the islands are solid yet (see
    // atlas.ts's lodFor). The reducers only re-run on refresh(), not on a
    // camera move, so zoom refreshes both LOD and the visible-radius floor;
    // panning only repaints, which is all the overlay needs.
    const mountRatio = renderer.getCamera().ratio;
    openingRatioRef.current = mountRatio;
    // A subpixel disc disappears in a large production graph. Keep a tiny
    // visible point at overview; zoomed discs still follow their spacing.
    const updateRadiusScale = () => {
      const camera = renderer.getCamera();
      const override = { cameraState: { x: camera.x, y: camera.y, angle: camera.angle, ratio: camera.ratio } };
      // Sigma's scaleSize uses the previous paint's matrix during a camera
      // event. Project with the current state so caps hold on the first frame.
      const a = renderer.graphToViewport({ x: 0, y: 0 }, override);
      const b = renderer.graphToViewport({ x: 1, y: 0 }, override);
      graphUnitsPerPixel = 1 / Math.max(0.000001, Math.hypot(b.x - a.x, b.y - a.y));
      minimumGraphRadius = 1.3 * graphUnitsPerPixel;
    };
    updateRadiusScale();
    renderer.on("resize", () => {
      updateRadiusScale(); renderer.refresh();
    });
    // Sigma listens to window resizes; a docked inspector changes only the
    // container. Refresh dimensions without replacing the current camera view.
    const containerSizeObserver = typeof ResizeObserver !== "undefined"
      ? new ResizeObserver(() => {
        if (container.clientWidth > 0 && container.clientHeight > 0) {
          renderer.resize();
          renderer.refresh();
        }
      }) : null;
    containerSizeObserver?.observe(container);
    let previousRatio = mountRatio;
    lodRef.current = OPENING_LOD;
    const camera = renderer.getCamera();
    const onCameraUpdated = ({ ratio }: { ratio: number }) => {
      const next = lodFor(openingRatioRef.current / ratio);
      const prev = lodRef.current;
      const zoomChanged = ratio !== previousRatio;
      previousRatio = ratio;
      updateRadiusScale();
      if (!zoomChanged && next.dustVisible === prev.dustVisible && next.islandsSolid === prev.islandsSolid && next.phase === prev.phase) return;
      lodRef.current = next;
      renderer.refresh();
    };
    camera.on("updated", onCameraUpdated);
    // Entry framing is instantaneous. Entity overlays retain neighborhood
    // emphasis; page handoffs only move the camera in the whole graph.
    if (!restored && effectiveFocusId && graph.hasNode(effectiveFocusId)) {
      if (!focusPageId) hoverStateRef.current = hoverStateFor(graph, effectiveFocusId);
      const display = renderer.getNodeDisplayData(effectiveFocusId);
      // Page handoffs use the same camera zoom as a filter result. Other
      // nodes and connections stay in the graph, including distant groups.
      if (display) {
        const camera = renderer.getCamera();
        camera.setState({ x: display.x, y: display.y, ratio: Math.min(camera.ratio, 1, focusPageId ? openingRatioRef.current / 2.5 : 1) });
      }
    }
    const pendingFocus = pendingFocusRef.current;
    if (pendingFocus && graph.hasNode(pendingFocus)) {
      pendingFocusRef.current = null;
      focusNodeRef.current(pendingFocus);
    }
    if (selectedRef.current && graph.hasNode(selectedRef.current)) {
      hoverStateRef.current = hoverStateFor(graph, selectedRef.current);
    }
    // First paint of THIS renderer, and the only thing that draws the
    // overlay canvas above at all. Sigma's constructor render already happened
    // before the afterRender listeners existed, and the simulation rests at
    // alpha 0 straight out of createAtlasSimulation (atlas.ts settles it
    // synchronously, then stops), so no tick will schedule one either. This
    // used to be left to the cartography effect below — which only runs when
    // `communities` changes, and `communities` is derived from the FULL
    // model. Toggling the small-groups chip rebuilds the renderer off
    // `visibleModel` WITHOUT changing `communities`, so nothing ever painted:
    // the fresh canvas stayed at its untouched 300x150 default and every
    // region name vanished from the map.
    renderer.refresh();
    renderer.on("clickNode", ({ node }) => {
      // A moved drag must not also navigate on release.
      if (movedDuringPressRef.current) return;
      focusNodeRef.current(node);
    });
    renderer.on("clickStage", () => { if (selectedRef.current) returnToMapRef.current(); });
    // Hover is LOCKED while a drag is live: our drag doesn't capture the
    // pointer (sigma's captor keeps picking), so sweeping the grabbed node
    // across other hit areas would fire enter/leave mid-drag — the graph
    // dims/undims, labels pop, and the cursor flickers grabbing→pointer→
    // default. force-graph never shows this (d3-drag captures the pointer,
    // hover is inert mid-drag), and the flashing reads as jank.
    renderer.on("enterNode", ({ node }) => {
      if (draggedNodeRef.current || selectedRef.current) return;
      hoverStateRef.current = hoverStateFor(graph, node);
      container.style.cursor = "pointer";
      renderer.refresh();
    });
    renderer.on("leaveNode", () => {
      if (draggedNodeRef.current || selectedRef.current) return;
      hoverStateRef.current = hoverStateFor(graph, null);
      container.style.cursor = "default";
      renderer.refresh();
    });

    // Node drag — sigma v3's mouse-manipulation pattern (see mouse.d.ts /
    // sigma.esm.js MouseCaptor): downNode starts it, the captor's own
    // mousemovebody/mouseup/mousedown carry the rest. Physics now come from
    // the d3-force simulation (see atlas.ts's createAtlasSimulation) instead
    // of a stepped FA2 loop — downNode pins the pressed node and reheats the
    // sim; mousemovebody drags that pin along with the pointer; mouseup
    // releases it and lets alpha decay naturally (or stops outright under
    // reduced motion).
    renderer.on("downNode", ({ node }) => {
      draggedNodeRef.current = node;
      movedDuringPressRef.current = false;
      graph.setNodeAttribute(node, "highlighted", true);
      container.style.cursor = "grabbing";
      // A leaf memory is not a sim node — the writeback would put it back on
      // its orbit on the next tick unless the sim knows a hand is on it.
      sim.setDraggingId(node);
      const simNode = sim.nodes().find((n) => n.id === node);
      // Leaf memories aren't sim members (see nonSimulatedIds) — dragging one
      // is pure direct manipulation via mousemovebody's graphology writes, so
      // there's nothing here to pin or reheat; setDraggingId above is what
      // stops the writeback snapping it back onto its orbit.
      if (simNode) {
        simNode.fx = simNode.x;
        simNode.fy = simNode.y;
        // alpha JUMPS to the target instead of ramping: the sim rests at
        // alpha 0, and alphaTarget alone climbs at only 3%/tick — neighbor
        // forces stay near-zero for the first ~1/3s of a drag, which reads
        // as lag (measured 3x early neighbor response with the jump). Safe
        // on a settled sim: the equilibrium-invariant test reheats to 0.3
        // and pins bbox drift < 3%.
        sim.alpha(0.3).alphaTarget(0.3).restart();
      }
    });
    const mouseCaptor = renderer.getMouseCaptor();
    mouseCaptor.on("mousedown", () => {
      // Freeze the camera frame so dragging a boundary node doesn't re-fit it.
      if (!renderer.getCustomBBox()) renderer.setCustomBBox(renderer.getBBox());
    });
    mouseCaptor.on("mousemovebody", (e) => {
      const draggedNode = draggedNodeRef.current;
      if (!draggedNode) return;
      movedDuringPressRef.current = true;
      const pos = renderer.viewportToGraph(e);
      graph.setNodeAttribute(draggedNode, "x", pos.x);
      graph.setNodeAttribute(draggedNode, "y", pos.y);
      // Written straight onto the graph, outside the sim's writeback, so the
      // cartography scene has to be told the positions moved.
      sceneDirtyRef.current = true;
      // Instant response between ticks — the dragged node's own position
      // isn't waiting on the next sim tick; its neighbors flow toward this
      // pin as the sim (reheated on downNode) keeps ticking.
      const simNode = sim.nodes().find((n) => n.id === draggedNode);
      if (simNode) {
        simNode.fx = pos.x;
        simNode.fy = pos.y;
      }
      // Sigma's own click suppression (draggedEvents vs. draggedEventsTolerance)
      // never sees this drag — preventSigmaDefault short-circuits handleMove
      // before that counter increments — so movedDuringPressRef above is what
      // actually guards clickNode.
      e.preventSigmaDefault();
      e.original.preventDefault();
      e.original.stopPropagation();
    });
    mouseCaptor.on("mouseup", () => {
      const draggedNode = draggedNodeRef.current;
      if (draggedNode) {
        graph.setNodeAttribute(draggedNode, "highlighted", false);
        const simNode = sim.nodes().find((n) => n.id === draggedNode);
        // Every component is live now (see atlas.ts's groupCenterForce) — its
        // own anchor holds it at its shelf slot, so releasing fx/fy here
        // never hands a shelved node back to unconstrained charge/forceCenter.
        if (simNode) {
          simNode.fx = null;
          simNode.fy = null;
        }
        draggedNodeRef.current = null;
      }
      sim.setDraggingId(null);
      container.style.cursor = hoverStateRef.current.hovered ? "pointer" : "default";
      // Natural decay is the inertia tail; reduced motion skips it outright.
      // That tail is also what walks a released shelf component back onto its
      // slot, so skipping it needs the same correction applied in one step —
      // otherwise a component dragged toward the core just stays there.
      if (prefersReducedMotion()) {
        sim.settleShelf();
        sim.stop();
      } else sim.alphaTarget(0);
    });

    // Direct wheel zoom. Sigma's default quantizes the gesture into 1.7x
    // steps eased over 250ms and DROPS any wheel event landing within
    // zoomDuration/5 = 50ms of the last accepted one — a trackpad's
    // 60-120 events/s collapse to ~20 animated lurches, which reads as a
    // low refresh rate (measured: paint cadence stays 120fps; only the
    // camera moves in steps). The old graph's d3-zoom applies every delta
    // 1:1 in the same frame; do the same, with d3-zoom's own delta scale,
    // zooming toward the cursor.
    mouseCaptor.on("wheel", (e) => {
      e.preventSigmaDefault();
      const we = e.original as WheelEvent;
      // d3-zoom wheelDelta: pixel-mode deltas x0.002, line-mode x0.05,
      // page-mode x1, and pinch (ctrlKey wheel on mac) x10. Camera ratio
      // is inverse scale, so positive deltaY (scroll down) grows it.
      const scale = we.deltaMode === 1 ? 0.05 : we.deltaMode ? 1 : 0.002;
      const factor = Math.pow(2, we.deltaY * scale * (we.ctrlKey ? 10 : 1));
      const camera = renderer.getCamera();
      const newRatio = camera.getBoundedRatio(camera.ratio * factor);
      if (newRatio === camera.ratio) return;
      camera.setState(renderer.getViewportZoomedState({ x: e.x, y: e.y }, newRatio));
    });

    let disposed = false;
    const dispose = () => {
      if (disposed) return;
      disposed = true;
      containerSizeObserver?.disconnect();
      // Sigma only detaches its own camera listener; release our listener too.
      camera.off("updated", onCameraUpdated);
      viewpointRef.current = {
        scope: spaceFilter,
        view: captureAtlasViewpoint(renderer, graph),
        body: new Map(graph.nodes()
          .filter((id) => graph.getNodeAttribute(id, "entityType") !== MEMORY_NODE_TYPE)
          .map((id) => [id, { x: graph.getNodeAttribute(id, "x"), y: graph.getNodeAttribute(id, "y") }])),
      };
      sim.stop();
      simRef.current = null;
      sigmaRef.current = null;
      graphRef.current = null;
      renderer.kill();
      // Sigma removes its own canvases; the overlay is ours to remove.
      overlay.remove();
      areas.remove();
      overlayRef.current = null;
      areasRef.current = null;
      if (rendererDisposeRef.current === dispose) rendererDisposeRef.current = null;
    };
    rendererDisposeRef.current = dispose;
    return dispose;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [visibleModel]);

  // Theme flip: recolor the live graph and repaint — no remount. Also keeps
  // paletteRef current so nodeReducer/edgeReducer (read at paint time) see
  // the new theme without the renderer being rebuilt.
  useEffect(() => {
    paletteRef.current = palette;
    const graph = graphRef.current;
    const renderer = sigmaRef.current;
    if (!graph || !renderer) return;
    graph.updateEachNodeAttributes((_id, attrs) => ({
      ...attrs,
      color: nodeFillFor(attrs.entityType, attrs.confirmed, palette),
    }));
    graph.updateEachEdgeAttributes((_id, attrs) => ({
      ...attrs,
      color: palette.edge,
    }));
    renderer.setSetting("labelColor", { color: palette.label });
    // refresh() re-fires afterRender, which repaints the place names with
    // the palette paletteRef now carries.
    renderer.refresh();
  }, [palette]);

  // Cartography arriving or regressing (fallback -> ready, or ready -> a
  // partial-error) must repaint the place names WITHOUT tearing down the
  // sim/camera — the mount effect above only rebuilds on `visibleModel`
  // changing, so a `cartographyBySpace` refetch that flips a space's status
  // never reaches the renderer otherwise. Same "repaint in place" shape as
  // the theme-flip effect: point communitiesRef at the fresh map (the paint
  // callbacks read it), then refresh.
  useEffect(() => {
    // On the mount pass communitiesRef already holds this very map (it is
    // seeded with it), and the mount effect has just painted with it — so
    // there is nothing to repaint and the scene would be rebuilt twice.
    // Only an actual change to the map is work.
    if (communitiesRef.current === communities) return;
    communitiesRef.current = communities;
    // Region membership is derived from the communities map, so a status flip
    // invalidates the cached scene even though nothing moved.
    sceneDirtyRef.current = true;
    sigmaRef.current?.refresh();
  }, [communities]);

  useEffect(() => {
    const overlay = overlayRef.current;
    if (!overlay) return;
    overlay.style.display = showRegions || layers.memory ? "" : "none";
    if (areasRef.current) areasRef.current.style.display = showRegions ? "" : "none";
    // The canvas keeps its last frame while hidden; repaint on re-show so it
    // matches wherever the camera and drags went in the meantime.
    if (showRegions) sigmaRef.current?.refresh();
  }, [showRegions, layers.memory]);

  const statusStyle = {
    height: "100%",
    width: "100%",
    display: "flex",
    flexDirection: "column" as const,
    alignItems: "center",
    justifyContent: "center",
    gap: 10,
    background: "var(--mem-surface)",
    fontFamily: "var(--mem-font-body)",
  };

  // Honest states: a dead daemon must never look like an empty graph.
  if (graphError) {
    return (
      <div data-testid="atlas-view" style={statusStyle}>
        <p className="entity-empty" style={{ color: "var(--mem-status-danger-text)" }}>
          {t("constellationMap.loadError")}
        </p>
        <button
          type="button"
          className="memory-detail-text-button"
          onClick={() => {
            refetchGraph();
          }}
        >
          {t("constellationMap.retry")}
        </button>
      </div>
    );
  }

  if (graphLoading) {
    return (
      <div data-testid="atlas-view" style={statusStyle}>
        <span className="entity-empty">{t("constellationMap.loading")}</span>
      </div>
    );
  }

  // Nothing to draw only when the daemon has neither entities NOR wiki pages:
  // a knowledge base made purely of pages is a real map, not an empty one.
  if (entities.length === 0 && graph.pages.length === 0) {
    return (
      <div data-testid="atlas-view" style={statusStyle}>
        <TabEmptyMark tab="graph" />
        <span className="entity-empty">{t("constellationMap.empty")}</span>
      </div>
    );
  }

  // Counts describe what is actually ON THE MAP: layer on, and connected to
  // something. What the degree-0 filter took out is reported by its own chip,
  // and a layer that is off contributes nothing rather than a zero.
  const drawn = filteredModel.nodes;
  const pageCount = drawn.filter((node) => node.kind === "page").length;
  const memoryCount = drawn.filter((node) => node.kind === "memory").length;
  const entityCount = drawn.length - pageCount - memoryCount;
  // A kind appears when its layer is on and it actually contributed nodes.
  // Entities are the exception and always report, zero included — the round-1
  // line did, and "0 entities" is the honest answer to an empty entity layer.
  const countLine = [
    ...(layers.page && pageCount > 0 ? [t("atlas.countPages", { count: pageCount })] : []),
    ...(layers.entity ? [t("atlas.countEntities", { count: entityCount })] : []),
    ...(layers.memory && memoryCount > 0
      ? [t("atlas.countMemories", { count: memoryCount })]
      : []),
    ...(regionCount > 0 ? [t("atlas.countRegions", { count: regionCount })] : []),
  ].join(" · ");
  const dropdownOpen = searchFocused && query.trim().length > 0;

  return (
    <div className="atlas-canvas-shell">
      <div ref={containerRef} data-testid="atlas-view" className="atlas-canvas" />

      <div className="atlas-top-controls">
        {onBack && (
          <WorkspaceBackButton
            type="button"
            onClick={onBack}
            className="atlas-back-button"
          >
            <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" aria-hidden="true"><path d="M19 12H5M12 19l-7-7 7-7"/></svg>
            {t("main.back")}
          </WorkspaceBackButton>
        )}
        <div className="atlas-search-control">
          <div className={`collection-search atlas-search-field${searchFocused ? " is-focused" : ""}`}>
            <Funnel size={18} aria-hidden="true" />
            <input
              className="collection-search-input"
              ref={searchInputRef}
              type="text"
              role="combobox"
              aria-expanded={dropdownOpen}
              aria-controls="atlas-search-listbox"
              aria-activedescendant={
                dropdownOpen && matches.length > 0 ? `atlas-search-option-${activeIndex}` : undefined
              }
              aria-label={t("atlas.searchLabel")}
              placeholder={t("atlas.searchPlaceholder")}
              value={query}
              onChange={(e) => {
                setQuery(e.target.value);
                setActiveIndex(0);
              }}
              onFocus={() => setSearchFocused(true)}
              onBlur={() => setSearchFocused(false)}
              onKeyDown={onSearchKeyDown}
            />
          </div>
          {dropdownOpen && (
            <ul id="atlas-search-listbox" role="listbox" className="atlas-search-results">
              {matches.map((node, index) => (
                <li
                  key={node.id}
                  id={`atlas-search-option-${index}`}
                  role="option"
                  aria-selected={index === activeIndex}
                  // Keep the input focused until the selected row is handled.
                  onMouseDown={(e) => e.preventDefault()}
                  onClick={() => focusEntity(node.id)}
                  onMouseEnter={() => setActiveIndex(index)}
                >
                  <span
                    className="atlas-search-result-dot"
                    style={{
                      backgroundColor:
                        node.entityType === MEMORY_NODE_TYPE
                          ? palette.memory
                          : node.entityType === PAGE_NODE_TYPE
                            ? palette.page
                            : colorForEntityType(node.entityType, palette),
                    }}
                  />
                  <span>{node.name}</span>
                </li>
              ))}
              {matches.length === 0 && <li className="atlas-search-no-matches">{t("atlas.noMatches")}</li>}
            </ul>
          )}
        </div>
      </div>

      <div className="atlas-display-anchor">
        <button
          ref={displayTriggerRef}
          type="button"
          className="collection-control atlas-display-trigger"
          aria-label={t("atlas.display")}
          aria-expanded={displayOpen}
          aria-controls={displayOpen ? "atlas-display-panel" : undefined}
          onClick={() => setDisplayOpen((open) => !open)}
        >
          <SlidersHorizontal size={16} weight="regular" aria-hidden="true" />
          <span>{t("atlas.display")}</span>
          <CaretDown size={13} weight="regular" aria-hidden="true" />
        </button>
        {displayOpen && (
          <div
            ref={displayPanelRef}
            id="atlas-display-panel"
            className="atlas-display-panel"
            role="group"
            aria-label={t("atlas.graphContent")}
          >
            {spaces.length > 0 && (
              <AtlasSelect
                label={t("atlas.spaceLabel")}
                value={spaceFilter ?? ""}
                onChange={(value) => setSpaceFilter(value || null)}
                options={[{ value: "", label: t("atlas.spaceAll") }, ...spaces.map((space) => ({ value: space, label: space }))]}
                searchLabel={t("atlas.searchSpaces")}
                noMatchesLabel={t("atlas.noMatches")}
              />
            )}
            <div className="atlas-display-options">
          {(
            [
              { key: "page" as const },
              { key: "entity" as const },
              { key: "memory" as const },
            ]
          ).map(({ key }) => {
            const label = t(`atlas.layer.${key}`);
            const on = layers[key];
            const locked = onlyLayerOn(key);
            return (
              <AtlasTooltip key={key} content={t(`atlas.layerDescription.${key}`)}>
                <button
                  type="button"
                  aria-label={label}
                  aria-pressed={on}
                  disabled={locked}
                  onClick={() => toggleLayer(key)}
                  className="atlas-content-toggle"
                >
                  <span
                    className="atlas-content-dot"
                    aria-hidden="true"
                    style={{ color: key === "page" ? palette.page : key === "memory" ? palette.memory : palette.neutral }}
                  />
                  <span>{label}</span>
                </button>
              </AtlasTooltip>
            );
          })}
          <div className="atlas-content-actions">
            <div className="atlas-content-toolgroup">
            {layers.entity && entityTypes.length > 0 && <AtlasTypeFilters
              types={entityTypes} excluded={excludedTypes} palette={palette} onToggle={toggleEntityType}
              onReset={() => setExcludedTypes(new Set())}
            />}

            <AtlasTooltip content={t("atlas.regionsDescription")}>
            <button
              type="button"
              className="atlas-content-secondary-toggle"
              aria-pressed={showRegions}
              aria-label={t("atlas.regionsToggle")}
              onClick={() => {
                showRegionsRef.current = !showRegions;
                setShowRegions(!showRegions);
              }}
            >
              <span className="atlas-content-dot" aria-hidden="true" />
              <span>{t("atlas.regionsToggle")}</span>
            </button>
            </AtlasTooltip>
            {smallGroupCount > 0 && (
              <AtlasTooltip content={t(showSmallGroups ? "atlas.hideSmallGroups" : "atlas.smallGroupsHidden", { count: smallGroupCount })}>
              <button
                type="button"
                onClick={toggleSmallGroups}
                aria-pressed={showSmallGroups}
                aria-label={t(showSmallGroups ? "atlas.hideSmallGroups" : "atlas.showSmallGroups")}
                className="atlas-content-secondary-toggle"
              >
                <span className="atlas-content-dot" aria-hidden="true" />
                <span>{t("atlas.smallGroupsLabel")}</span>
              </button>
              </AtlasTooltip>
            )}
            </div>
            {cartographyStatus === "partial-error" && (
              <span role="alert" className="atlas-cartography-alert">
                {t("atlas.cartographyPartialError")}
              </span>
            )}
          </div>
            </div>
          </div>
        )}
      </div>

      <div className="atlas-count-line" aria-live="polite">{countLine}</div>

      {filteredModel.nodes.length === 0 && excludedTypes.size > 0 && <div className="atlas-filter-empty">
        <p>{t("atlas.noTypeMatches")}</p><button type="button" className="atlas-action" onClick={() => setExcludedTypes(new Set())}>{t("atlas.allEntityTypes")}</button>
      </div>}
      {selectedNode && <AtlasInspector key={selectedNode.id} node={selectedNode} neighbors={selectedNeighbors}
        edges={filteredModel.edges.filter((edge) => edge.source === selectedNode.id || edge.target === selectedNode.id)}
        onSelect={focusEntity} onClose={returnToMap}
        onOpen={onNodeClick ? () => onNodeClick(targetForNode(selectedNode.id)) : undefined} />}

      <div className="atlas-camera-controls" role="group" aria-label={t("atlas.cameraControls")}>
        <AtlasTooltip content={t("atlas.zoomOut")}>
          <button type="button" className="atlas-icon-button" aria-label={t("atlas.zoomOut")} onClick={() => zoomMap(1.25)}>
            <Minus size={16} weight="regular" aria-hidden="true" />
          </button>
        </AtlasTooltip>
        <AtlasTooltip content={t("atlas.zoomIn")}>
          <button type="button" className="atlas-icon-button" aria-label={t("atlas.zoomIn")} onClick={() => zoomMap(0.8)}>
            <Plus size={16} weight="regular" aria-hidden="true" />
          </button>
        </AtlasTooltip>
        <AtlasTooltip content={t("atlas.mainNetwork")}>
          <button type="button" className="atlas-icon-button" aria-label={t("atlas.mainNetwork")} onClick={() => frameMap("main")}>
            <ArrowCounterClockwise size={16} weight="regular" aria-hidden="true" />
          </button>
        </AtlasTooltip>
        <AtlasTooltip content={t("atlas.fitAll")}>
          <button type="button" className="atlas-icon-button" aria-label={t("atlas.fitAll")} onClick={() => frameMap("all")}>
            <CornersOut size={16} weight="regular" aria-hidden="true" />
          </button>
        </AtlasTooltip>
      </div>

    </div>
  );
}
