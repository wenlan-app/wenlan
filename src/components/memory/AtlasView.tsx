// SPDX-License-Identifier: AGPL-3.0-only
import { TopicIcon } from "./entities/TopicIcon";
import { WorkspaceBackButton } from "./navigation/WorkspaceNavigation";
import { TabEmptyMark } from "../onboarding/TabIntro";
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { KeyboardEvent as ReactKeyboardEvent } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import Graph from "graphology";
import Sigma from "sigma";
import { EdgeRectangleProgram } from "sigma/rendering";
import {
  ArrowCounterClockwise,
  ArrowRight,
  CaretDown,
  CornersOut,
  Minus,
  Funnel,
  Plus,
  SlidersHorizontal,
} from "@phosphor-icons/react";
import {
  ATLAS_CONTROL_INSETS,
  captureAtlasViewpoint,
  frameAtlasView,
  restoreAtlasViewpoint,
  atlasFrameState,
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
import { entityTypeHidden, filterGraphEntityTypes } from "../../lib/graph/typeFilter";
import { getKnowledgeGraph, getOldMemoryPass, listThingNightPoints, undoNights, undoOldMemoryPass } from "../../lib/tauri";
import { SteepMoon } from "./things/ThingSteeped";
import {
  growMs,
  growStagger,
  nightCounts,
  nightsToday,
  ranOvernight,
  replayStops,
  REPLAY_TIMING,
  tidyMs,
  todayStartSeconds,
  type ReplayStop,
} from "../../lib/graph/lastNight";
import { GHOST_STROKE, ghostAt, ghostPhase, ghostRests, ghostsMoving, saidLine, swellOf, type ReplayGhost } from "../../lib/graph/tidyGhosts";
import type { Entity, KnowledgeGraph, ThingNightPoints } from "../../lib/tauri";
import {
  buildKnowledgeGraphModel,
  entitySpace,
  filterKnowledgeGraph,
  memorySourceId,
  pageIdOf,
  pageNodeId,
  attachMemories,
  attachThingPoints,
  thingIdOf,
  thingNodeId,
  smallGroupIds,
  tuckedNodeIds,
  DEFAULT_LAYERS,
  MEMORY_NODE_TYPE,
  PAGE_NODE_TYPE,
  THING_NODE_TYPE,
} from "../../lib/graph/model";
import type { GraphLayers, GraphModel, GraphNode } from "../../lib/graph/model";
import type { AtlasLayoutReply, AtlasLayoutRequest } from "../../lib/graph/atlasLayoutJob";
import {
  buildAtlasGraph,
  applyAtlasHierarchy,
  markKeptLayout,
  runAtlasLayout,
  DEFAULT_ATLAS_LAYOUT,
  type AtlasLayoutVariant,
  createAtlasSimulation,
  hoverStateFor,
  nodeDisplay,
  edgeDisplay,
  labelCandidates,
  lodFor,
  OPENING_LOD,
  drawNodeLabelAt,
  layoutFocusLabels,
  NEIGHBOR_LABEL_MAX,
  NODE_LABEL_FONT,
  nodeSizeCapPx,
  setThingFlight,
  thingFlightTargets,
  thingLinkedBodies,
  labelSideFor,
} from "../../lib/graph/atlas";
import type { HoverState, AtlasSimulation, FlyTo, LodState, LabelPlacement, LabelSide } from "../../lib/graph/atlas";
import {
  applyNightOffsets,
  drawNightMarks,
  edgeSignAt,
  frameAt,
  growthAt,
  GROWTH_MS,
  newNightPoints,
  nightMarks,
  nightOffsetsOf,
  nightPointIds,
  SENTENCE_MAX_PX,
  THING_STOPS,
  thingFocusState,
  thingPointIds,
  thingStopsFor,
  thingStopNodeIds,
  type FrameHit,
  type ThingStop,
  labelYieldsToFrames,
} from "../../lib/graph/night";
import {
  readNightOffsets,
  readSeenNightPoints,
  readStoredPositions,
  readStoredRegions,
  readPreviousPositions,
  readPreviousRegions,
  storeRegions,
  readThingsApart,
  startRearrange,
  alignToKept,
  ATLAS_LAYOUT_VERSION,
  readLayoutVersion,
  storeLayoutVersion,
  storeNightOffsets,
  storePositions,
  storeSeenNightPoints,
  storeThingsApart,
  undoRearrange,
} from "../../lib/graph/positions";
import { placeNewRegionMembers } from "../../lib/graph/regionMap";
import type { CartographyScene } from "../../lib/graph/cartography";
import {
  communitiesFor,
  cartographyScene,
  drawRegionNames,
  drawPlannedRegionNames,
  planRegionNamesFirst,
  regionNameMeasure,
  type PlannedRegionName,
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
/** How far past the opening view the map may zoom out (camera ratio). */
const MAX_ZOOM_OUT = 2;
/** How many bodies a kept layout may be missing and still be put straight
 *  back (the new ones placed beside it) instead of laid out again. */
export function keptMissingAllowance(bodies: number): number {
  return Math.max(5, Math.ceil(bodies * 0.02));
}
/** Label thresholds per zoom phase: the smallest rendered disc (px) that
 *  may carry a name, and the label grid's cell (px). The overview keeps the
 *  landmarks' names only. */
type Box = { left: number; right: number; top: number; bottom: number };
type GuardDisc = { key: string; x: number; y: number; r: number; owner?: string };
type LabelGuard = {
  cells: Map<number, GuardDisc[]>;
  /** The map's own controls and count line, in canvas pixels, plus the canvas size. */
  chrome: Box[];
  width: number;
  height: number;
  plan: Map<string, { label: string; at: LabelPlacement } | null> | null;
  rings: Map<string, number>;
  /** R2 at overview zoom: region names placed before any point name. */
  regionNames?: PlannedRegionName[];
  /** Hubs whose region name is on screen: their own name is left off. */
  namedHubs?: Set<string>;
};
/** Screen px. A drawn disc at least this big is something a name may not cover. */
const LABEL_GUARD_MIN_RADIUS = 1.5;
/** Screen px per cell of the disc lookup grid. */
const LABEL_GUARD_CELL = 32;
// Names keep this far inside the canvas edge (round 3).
const LABEL_EDGE_INSET = 4;
/** Every drawn disc on screen, the smallest too, on the guard's cell grid:
 *  what an R2 region name may not cover. */
function allDiscCells(sigma: Sigma, width: number, height: number): Map<number, GuardDisc[]> {
  const cells = new Map<number, GuardDisc[]>();
  sigma.getGraph().forEachNode((id) => {
    const display = sigma.getNodeDisplayData(id);
    if (!display || display.hidden) return;
    const r = sigma.scaleSize(display.size);
    const { x, y } = sigma.framedGraphToViewport(display);
    if (x < -r || y < -r || x > width + r || y > height + r) return;
    const disc: GuardDisc = { key: id, x, y, r };
    for (let cx = Math.floor((x - r) / LABEL_GUARD_CELL); cx <= Math.floor((x + r) / LABEL_GUARD_CELL); cx += 1) {
      for (let cy = Math.floor((y - r) / LABEL_GUARD_CELL); cy <= Math.floor((y + r) / LABEL_GUARD_CELL); cy += 1) {
        const list = cells.get(cx * 65536 + cy);
        if (list) list.push(disc);
        else cells.set(cx * 65536 + cy, [disc]);
      }
    }
  });
  return cells;
}

/** Whether `box` comes within `margin` px of any disc in `cells`. */
function discsHit(cells: Map<number, GuardDisc[]>, box: Box, margin: number): boolean {
  for (let cx = Math.floor((box.left - margin) / LABEL_GUARD_CELL); cx <= Math.floor((box.right + margin) / LABEL_GUARD_CELL); cx += 1) {
    for (let cy = Math.floor((box.top - margin) / LABEL_GUARD_CELL); cy <= Math.floor((box.bottom + margin) / LABEL_GUARD_CELL); cy += 1) {
      for (const disc of cells.get(cx * 65536 + cy) ?? []) {
        const nx = Math.max(box.left, Math.min(disc.x, box.right));
        const ny = Math.max(box.top, Math.min(disc.y, box.bottom));
        if (Math.hypot(disc.x - nx, disc.y - ny) < disc.r + margin) return true;
      }
    }
  }
  return false;
}

const LABEL_PASSES_SMALL = (() => { try { return window.localStorage.getItem("atlas.exp.labelPass") === "1"; } catch { return false; } })();
/** The controls drawn over the map; a name keeps out from under them. */
const ATLAS_CHROME_SELECTOR = ".atlas-top-controls > *, .atlas-display-trigger, .atlas-display-panel, .atlas-camera-controls, .atlas-count-line";
/** The most landmark names the zoomed-out map carries at once. */
const OVERVIEW_LABEL_MAX = 30;
// Sigma lays its label grid out once per graph refresh, so zoomed in most of
// the screen falls in a few of its cells; the detail zoom offers more names
// per cell, and the label guard keeps only those with room.
const LABEL_LOD: Record<"overview" | "neighborhood" | "detail", { threshold: number; cell: number; density: number }> = {
  overview: { threshold: 8, cell: 200, density: 0.04 },
  neighborhood: { threshold: 1.8, cell: 140, density: 0.04 },
  detail: { threshold: 1.8, cell: 110, density: 0.2 },
};

/** The closest a thing's own stop (這件事) zooms, CSS px per graph unit:
 *  its points a sentence's line apart or more, not a screen apart. */
const THING_STOP_DENSITY = 12;
/** How long a picked thing's points take to fly out to their topics, ms:
 *  long enough to follow where each one goes (Fable's design, 800-1200). */
const FLIGHT_MS = 900;
/** How long they take to fly home when it is let go, ms, while the camera
 *  goes back to the map in the same time. */
const HOME_MS = 450;
/** How long the camera takes to reach a picked thing's stop, ms. */
const THING_CAMERA_MS = 450;
/** A point's short title when zoomed in, CSS px: a few words, about ten
 *  Chinese characters. Its whole sentence comes up on hover. */
const POINT_TITLE_PX = 120;

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

const EMPTY_THING_POINTS: ThingNightPoints[] = [];

/** Where the layer choice survives a reload. */
const LAYERS_STORAGE_KEY = "atlas.layers";

/** Where the "Working on" choice is remembered across reloads. */
/** How far out a name may sit for its dust (screen px past the disc): about
 *  one ring. Close in, a busy hub's dust spreads 150-300 px, and a name past
 *  all of it no longer reads as the dot's (round 3). Its own dust never
 *  blocks its name; the halo keeps the name legible over it. */
const LABEL_RING_MAX = 22;
/** From this many nodes a fresh layout runs in a worker (it takes seconds). */
const WORKER_LAYOUT_MIN_NODES = 1500;
const labelRing = (radius: number, ring: number | undefined): number =>
  Math.max(radius, Math.min(ring ?? 0, radius + LABEL_RING_MAX));

export const WORKING_STORAGE_KEY = "atlas.showWorking";

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
  | { kind: "page"; id: string }
  | { kind: "thing"; id: string };

/** The node id a click landed on, resolved to a navigable target. */
function targetForNode(nodeId: string): AtlasNodeTarget {
  const sourceId = memorySourceId(nodeId);
  if (sourceId !== null) return { kind: "memory", id: sourceId };
  const thingId = thingIdOf(nodeId);
  if (thingId !== null) return { kind: "thing", id: thingId };
  const pageId = pageIdOf(nodeId);
  if (pageId !== null) return { kind: "page", id: pageId };
  return { kind: "entity", id: nodeId };
}

interface AtlasViewProps {
  onNodeClick?: (target: AtlasNodeTarget) => void;
  /** Open a page with its mind map showing. */
  onOpenMindMap?: (pageId: string) => void;
  // Initial framing: center this entity with its neighborhood emphasized
  // (EntityDetail's overlay "Atlas" mode). Applied instantly on mount — a
  // starting frame, not a transition — so no camera animation.
  focusEntityId?: string;
  /** Initial camera target in the whole graph; never restricts its membership. */
  focusPageId?: string;
  /** Open at this thing's 這件事 stop (from its page's 「在大图上看」). */
  focusThingId?: string;
  /** Home's "Replay" (UX-GRAPH-6): the press it answers. The map plays last
   *  night once for it, 300 ms after its first paint. */
  replay?: number;
  // Main.tsx's Graph view passes navigateBack; renders a back button as the
  // first toolbar item (a floating one would sit on the search box).
  onBack?: () => void;
  /** Opens the list of every topic. The left side has no Topics entry
   *  (UX-FIND-4): the map is where topics live, so their list starts here. */
  onOpenTopics?: () => void;
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
/** The home presses already played, so going back to such a map is quiet. */
const playedReplays = new Set<number>();

/** What Replay is doing: the stops it plays, its timers, the things whose
 *  points it shows, and the view to go back to (UX-GRAPH-6); the tidies it
 *  is showing and their paint loop (UX-GRAPH-7). */
interface ReplayRun {
  readonly stops: readonly ReplayStop[];
  readonly timers: number[];
  readonly things: ReadonlySet<string>;
  readonly overview: AtlasViewpoint;
  ghosts: readonly ReplayGhost[];
  frame: number;
}

/** How long the map's line tells of the pass over old memories. */
const OLD_PASS_SHOWN_SECS = 2 * 86_400;

/** A ghost's size when what it slides into is not drawn, px. */
/** How wide a map label runs, in the map's label font; 0 without a canvas (jsdom). */
let labelMeasurer: CanvasRenderingContext2D | null | undefined;
const measureLabel = (text: string): number => {
  if (labelMeasurer === undefined) {
    labelMeasurer = typeof document === "undefined" ? null : document.createElement("canvas").getContext("2d");
    if (labelMeasurer) labelMeasurer.font = NODE_LABEL_FONT;
  }
  return labelMeasurer ? labelMeasurer.measureText(text).width : text.length * 7;
};
const GHOST_FALLBACK_RADIUS = 5;

export default function AtlasView({ onNodeClick, onOpenMindMap, focusEntityId, focusPageId, focusThingId, replay: replayPress, onBack, onOpenTopics }: AtlasViewProps) {
  const { t } = useTranslation();
  const effectiveFocusId = focusPageId ? pageNodeId(focusPageId) : focusEntityId;
  const palette = useGraphPalette();
  const containerRef = useRef<HTMLDivElement>(null);
  const sigmaRef = useRef<Sigma | null>(null);
  const rendererDisposeRef = useRef<(() => void) | null>(null);
  // UX-GRAPH-1: points keep their places; moving all of them is the
  // person's choice, asked first, and can be taken back once.
  const [layoutVersion, setLayoutVersion] = useState(0);
  const [rebalanced, setRebalanced] = useState(false);
  /** Put back was pressed: the next build brings the layout from before the rebalance back. */
  const rearrangeRef = useRef<"undo" | null>(null);
  // A large fresh layout runs in a worker; its spots come back here and the
  // map is built again from them (round 3 polish).
  const workerLayoutRef = useRef<{ model: GraphModel; version: number; variant: AtlasLayoutVariant; positions: Map<string, { x: number; y: number }>; regions?: Map<string, string>; blockedMs: number; totalMs: number; report?: unknown } | null>(null);
  const [layingOut, setLayingOut] = useState(false);
  const [workerLayouts, setWorkerLayouts] = useState(0);
  const workerFailedRef = useRef(false);
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
  /** The point whose sentence a click pinned open. */
  const pinnedPointRef = useRef<string | null>(null);
  const thingArrivalDoneRef = useRef(false);
  const overviewRef = useRef<AtlasViewpoint | null>(null);
  const openingRatioRef = useRef(1);
  const focusNodeRef = useRef<(id: string) => void>(() => {});
  const returnToMapRef = useRef<() => void>(() => {});
  // A picked thing's zoom stop (UX-CHAT-5), and where the last paint put each
  // frame and title, so a click there picks its thing.
  const [thingStop, setThingStop] = useState<ThingStop>("this");
  const [selectedThing, setSelectedThing] = useState<string | null>(null);
  const [thingStops, setThingStops] = useState<readonly ThingStop[]>(THING_STOPS);
  const thingStopRef = useRef<ThingStop>("this");
  const selectThingRef = useRef<(thing: string, stop: ThingStop, options?: { readonly fly?: boolean }) => void>(() => {});
  const goToSignRef = useRef<(id: string) => void>(() => {});
  const frameHitsRef = useRef<FrameHit[]>([]);
  // Night points that are new to this viewer and have not finished growing
  // in yet. They are marked seen as soon as they are found, so this is what
  // carries them over a rebuild that cuts their growth short.
  const growQueueRef = useRef<Set<string>>(new Set());
  // Sends the picked thing's points out to their topics (see the build).
  const flyRef = useRef<(thing: string | null, startAt: number | null, only?: ReadonlySet<string>) => void>(() => {});
  // When the last thing was picked (performance.now()), so its points' flight
  // keeps its time across a rebuild.
  const pickedAtRef = useRef(0);
  const sceneRef = useRef<CartographyScene | null>(null);
  const sceneDirtyRef = useRef(true);
  // Replay (UX-GRAPH-6): what it is doing, while it plays; "marked" is the
  // reduced-motion form, where everything is lit at once.
  const [replaying, setReplaying] = useState<"idle" | "playing" | "marked">("idle");
  const replayRef = useRef<ReplayRun | null>(null);
  // Grows the given points out of their thing's centre (see the build): from
  // `startAt` (performance.now() time), each `stagger` ms after the last.
  const growRef = useRef<(ids: readonly string[], startAt: number, stagger: number) => void>(() => {});
  const startReplayRef = useRef<() => void>(() => {});
  const endReplayRef = useRef<(camera: "return" | "stay") => void>(() => {});
  // Home's press waits for the map's first paint.
  const replayPendingRef = useRef(replayPress !== undefined && !playedReplays.has(replayPress));

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
  // Each thing with the points its nights wrote (UX-CHAT-5). Those points have
  // no entity or page link of their own, so this is the only way they reach
  // the map. An older daemon without the route leaves the map as it was.
  const { data: thingPoints = EMPTY_THING_POINTS, isPending: thingPointsPending } = useQuery({
    queryKey: ["thing-night-points"],
    queryFn: async (): Promise<ThingNightPoints[]> => {
      try {
        return (await listThingNightPoints()) ?? EMPTY_THING_POINTS;
      } catch {
        return EMPTY_THING_POINTS;
      }
    },
    refetchInterval: 120_000,
  });

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

  // What sigma draws: every node. Small groups (components of fewer than
  // MIN_COMPONENT_SIZE, model.ts) are laid out with the rest but kept out of
  // sight until the reader zooms in (see `tucked` and the node reducer): on
  // real data ~960 of 1,600 entities have no relation at all, and another ~196
  // sit in small groups of four or fewer, which buried the opening map.
  // What active things' steeped points link to is never tucked away, or a
  // linked thing would have no 附近 to go to.
  const thingAnchors = useMemo(() => {
    const points = new Set(thingPoints.filter((thing) => !thing.quiet).flatMap((thing) => thing.points.map((point) => point.memory_id)));
    return new Set(scopedGraph.memory_links.filter((link) => points.has(link.memory_id)).map((link) => link.entity_id));
  }, [thingPoints, scopedGraph]);
  const visibleModel = useMemo<GraphModel>(() => {
    let withMemories = baseModel;
    if (layers.memory) {
      // Nothing to hang the memories on (entity and page layers both off):
      // fall back to drawing the memory model as it is.
      withMemories = baseModel.nodes.length === 0 ? model : attachMemories(baseModel, model);
    }
    // Things and their night's points are drawn whatever the memory layer
    // says: they are the work, not a layer of evidence.
    return attachThingPoints(withMemories, thingPoints, scopedGraph.memory_links, spaceFilter);
  }, [baseModel, model, layers.memory, thingPoints, scopedGraph, spaceFilter]);
  // The small groups, and the memories hung only on them: hidden until the
  // view is zoomed in far enough for the islands to come up solid.
  // The group the map was opened on (a page or topic handed off to it) is
  // never tucked either: the reader came to see it.
  const tucked = useMemo(() => {
    const anchors = effectiveFocusId ? new Set([...thingAnchors, effectiveFocusId]) : thingAnchors;
    return tuckedNodeIds(visibleModel, smallGroupIds(baseModel, anchors));
  }, [visibleModel, baseModel, thingAnchors, effectiveFocusId]);
  const tuckedRef = useRef(tucked);
  tuckedRef.current = tucked;
  const hasThings = visibleModel.nodes.some((node) => node.kind === "thing");
  const hasActiveThings = visibleModel.nodes.some((node) => node.kind === "thing" && !node.quiet);
  const hasThingsRef = useRef(hasThings);
  hasThingsRef.current = hasThings;
  // What last night did on this map (UX-GRAPH-5): the things whose newest
  // night ran today, counted by what is on the map, so another Space's
  // points never count here.
  const tonight = useMemo(() => nightsToday(thingPoints, spaceFilter), [thingPoints, spaceFilter]);
  const tonightRef = useRef(tonight);
  tonightRef.current = tonight;
  const nightCount = useMemo(() => {
    const ids = new Set(visibleModel.nodes.map((node) => node.id));
    return nightCounts(tonight, (id) => ids.has(id));
  }, [tonight, visibleModel]);
  const queryClient = useQueryClient();
  // Undo on the map takes back the whole night for every thing; one thing's
  // own page still takes back only that thing. The line goes at once.
  const undoNight = useMutation({
    mutationFn: () => undoNights(todayStartSeconds()),
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: ["thing-night-points"] });
      void queryClient.invalidateQueries({ queryKey: ["things"] });
      void queryClient.invalidateQueries({ queryKey: ["knowledge-graph"] });
    },
  });
  const nightShown = Object.values(nightCount).some((count) => count > 0) && !undoNight.isPending && !undoNight.isSuccess;
  // The one-time pass over old memories (UX-GRAPH-7): for two days after it
  // ran, the line says what it merged and updated and offers to take it all
  // back, when no night has a line of its own. An older daemon has none.
  const { data: oldPass = null } = useQuery({
    queryKey: ["old-memory-pass"],
    queryFn: async () => {
      try {
        return (await getOldMemoryPass()) ?? null;
      } catch {
        return null;
      }
    },
    refetchInterval: 120_000,
  });
  const undoOldPass = useMutation({
    mutationFn: undoOldMemoryPass,
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: ["old-memory-pass"] });
      void queryClient.invalidateQueries({ queryKey: ["thing-night-points"] });
      void queryClient.invalidateQueries({ queryKey: ["knowledge-graph"] });
    },
  });
  const oldPassShown = oldPass !== null
    && oldPass.merged + oldPass.updated > 0
    && Date.now() / 1000 - oldPass.at < OLD_PASS_SHOWN_SECS
    && !undoOldPass.isPending && !undoOldPass.isSuccess;
  // A fresh read after Undo says what is left; the line follows it again.
  const resetUndo = undoNight.reset;
  const undoDone = undoNight.isSuccess;
  useEffect(() => {
    if (undoDone) resetUndo();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [thingPoints]);

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
  const regionLayoutRef = useRef<Map<string, string> | null>(null);
  const [laidRegionCount, setLaidRegionCount] = useState<number | null>(null);

  const filteredModel = useMemo(() => filterGraphEntityTypes(visibleModel, excludedTypes), [visibleModel, excludedTypes]);

  // Region count for the toolbar count line — membership only, so it agrees
  // with the regions the cartography scene actually names without needing
  // node positions. Counted over the nodes on the map at rest for the same
  // reason: communities come from the full model, but a small group's
  // community is out of sight until zoomed in.
  const regionCount = useMemo(() => {
    const groups = new Map<string, GraphNode[]>();
    for (const node of filteredModel.nodes) {
      if (tucked.has(node.id)) continue;
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
  }, [filteredModel, communities, tucked]);

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
  // "Working on": every active thing with its steeped points, not only the
  // picked one. Off by default so the map opens clean; the choice is kept.
  const [showWorking, setShowWorking] = useState<boolean>(() => {
    if (typeof window === "undefined") return false;
    try {
      return window.localStorage.getItem(WORKING_STORAGE_KEY) === "true";
    } catch {
      return false;
    }
  });
  const showWorkingRef = useRef(showWorking);
  showWorkingRef.current = showWorking;
  const toggleWorking = () => {
    const next = !showWorking;
    showWorkingRef.current = next;
    setShowWorking(next);
    try {
      window.localStorage.setItem(WORKING_STORAGE_KEY, JSON.stringify(next));
    } catch {
      // Private mode / quota: the choice still applies for this session.
    }
  };
  const overlayRef = useRef<HTMLCanvasElement | null>(null);
  const areasRef = useRef<HTMLCanvasElement | null>(null);

  const matches = useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (!needle) return [];
    // A thing being worked on is found by its title too (UX-CHAT-5).
    const things = visibleModel.nodes.filter((node) => node.kind === "thing");
    return [...things, ...model.nodes].filter((node) => !entityTypeHidden(node.entityType, excludedTypes) && node.name.toLowerCase().includes(needle)).slice(0, 8);
  }, [model, visibleModel, query, excludedTypes]);


  const focusEntity = (nodeId: string) => {
    setQuery("");
    setActiveIndex(0);
    searchInputRef.current?.blur();
    const renderer = sigmaRef.current;
    const drawn = graphRef.current;
    if (!drawn || !drawn.hasNode(nodeId)) return;
    const graph = drawn;
    if (!renderer) return;
    // A thing (found by search) is picked like a click on its frame.
    if (graph.getNodeAttribute(nodeId, "entityType") === THING_NODE_TYPE) {
      selectThing(nodeId, "this");
      return;
    }
    if (!selectedRef.current) overviewRef.current = captureAtlasViewpoint(renderer, graph);
    selectedRef.current = nodeId;
    setSelectedId(nodeId);
    setSelectedThing(null);
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
  /**
   * Pick a thing and move to one of its three zoom stops (UX-CHAT-5): 這件事
   * fits its frame with room for the sentences, 附近 adds what its points
   * link to and their neighbours, 全部 fits the whole map. The thing stays
   * picked (its lit set lit, the stops offered) at every stop. The map itself
   * is its picture, so no inspector opens over it.
   */
  const selectThing = (thing: string, stop: ThingStop, { fly = true }: { readonly fly?: boolean } = {}) => {
    const renderer = sigmaRef.current;
    const drawn = graphRef.current;
    if (!renderer || !drawn || !drawn.hasNode(thing)) return;
    if (!selectedRef.current) overviewRef.current = captureAtlasViewpoint(renderer, drawn);
    else if (selectedRef.current !== thing) setSelectedId(null);
    selectedRef.current = thing;
    setSelectedThing(thing);
    setThingStops(thingStopsFor(drawn, thing));
    thingStopRef.current = stop;
    setThingStop(stop);
    hoverStateRef.current = thingFocusState(drawn, thing, stop);
    focusLayoutRef.current = null;
    // Its points set off as the camera arrives, so the flight is seen.
    // With reduced motion the camera is there at once, and so is the rest.
    pickedAtRef.current = performance.now() - (prefersReducedMotion() ? THING_CAMERA_MS : 0);
    // Replay sends out only the points last night connected, later.
    if (fly) flyRef.current(thing, pickedAtRef.current + THING_CAMERA_MS);
    // Its sentences come up once the camera is there (sentencesFor).
    window.setTimeout(() => sigmaRef.current?.refresh(), THING_CAMERA_MS + 20);
    const { width } = renderer.getDimensions();
    // The sentences run beside their points, either side; the title sits
    // above. Room on both sides keeps the thing centred.
    const sentenceRoom = Math.min(SENTENCE_MAX_PX * 0.75, width * 0.2);
    const insets = stop === "this"
      ? { ...ATLAS_CONTROL_INSETS, top: ATLAS_CONTROL_INSETS.top + 24, left: ATLAS_CONTROL_INSETS.left + sentenceRoom, right: ATLAS_CONTROL_INSETS.right + sentenceRoom }
      : ATLAS_CONTROL_INSETS;
    const duration = prefersReducedMotion() ? 0 : THING_CAMERA_MS;
    if (stop === "all") {
      const visibleNodeIds = new Set(
        drawn.nodes().filter((id) => !entityTypeHidden(drawn.getNodeAttribute(id, "entityType"), excludedTypesRef.current)),
      );
      frameAtlasView(renderer, drawn, { mode: "all", visibleNodeIds, insets, duration });
    } else {
      frameAtlasView(renderer, drawn, {
        nodeIds: thingStopNodeIds(drawn, thing, stop),
        insets,
        duration,
        padding: 40,
        maxDensity: stop === "this" ? THING_STOP_DENSITY : undefined,
      });
    }
    renderer.refresh();
  };
  selectThingRef.current = selectThing;
  /** A tap on an edge sign (nightMarks' edges): the camera goes to the far
   *  topic at the same zoom, where the thing's points sit with their
   *  sentences; the back sign returns to the thing's own stop. */
  const goToSign = (id: string) => {
    const renderer = sigmaRef.current;
    const drawn = graphRef.current;
    if (!renderer || !drawn || !drawn.hasNode(id)) return;
    if (id === selectedRef.current) {
      selectThing(id, "this");
      return;
    }
    const display = renderer.getNodeDisplayData(id);
    if (!display) return;
    const camera = renderer.getCamera();
    const state = { x: display.x, y: display.y, ratio: camera.ratio, angle: camera.angle };
    if (prefersReducedMotion()) camera.setState(state);
    else void camera.animate(state, { duration: THING_CAMERA_MS, easing: "quadraticInOut" });
  };
  goToSignRef.current = goToSign;
  const returnToMap = () => {
    const wasThing = selectedRef.current !== null && graphRef.current?.getNodeAttribute(selectedRef.current, "entityType") === THING_NODE_TYPE;
    selectedRef.current = null;
    pinnedPointRef.current = null;
    setSelectedId(null);
    setSelectedThing(null);
    restoreSearchFocusOnCloseRef.current = true;
    const renderer = sigmaRef.current;
    const drawn = graphRef.current;
    if (renderer && drawn) {
      hoverStateRef.current = hoverStateFor(drawn, null);
      const camera = renderer.getCamera();
      if (wasThing && overviewRef.current && !prefersReducedMotion()) {
        // Letting a thing go: the camera goes back to the map while its
        // points fly home, in the same time, so both are seen going back.
        const from = { x: camera.x, y: camera.y, ratio: camera.ratio, angle: camera.angle };
        if (restoreAtlasViewpoint(renderer, drawn, overviewRef.current)) {
          const to = { x: camera.x, y: camera.y, ratio: camera.ratio, angle: camera.angle };
          camera.setState(from);
          void camera.animate(to, { duration: HOME_MS, easing: "quadraticInOut" });
        }
        renderer.refresh();
        overviewRef.current = null;
        return;
      }
      if (overviewRef.current) restoreAtlasViewpoint(renderer, drawn, overviewRef.current);
      // setState does not cancel Sigma's pending fly-to frame. Replace that
      // animation with the restored state so an immediate Escape stays put.
      if (camera.isAnimated()) void camera.animate({ x: camera.x, y: camera.y, ratio: camera.ratio, angle: camera.angle }, { duration: 1 });
      renderer.refresh();
    }
    overviewRef.current = null;
  };
  returnToMapRef.current = returnToMap;
  /**
   * Replay (UX-GRAPH-6, Fable's design 2026-11-06): last night played on the
   * map, only ever on a press. Per thing, the camera goes to it and its new
   * points grow out of its centre one after another; then the camera takes
   * in the topics it connected, and those points fly to sit beside them,
   * with no lines. Between things the points go home; at the end they all
   * do, with the camera going back to the view the press left. With reduced
   * motion everything last night changed is lit at once instead.
   */
  const startReplay = () => {
    const renderer = sigmaRef.current;
    const drawn = graphRef.current;
    if (!renderer || !drawn || replayRef.current) return;
    const stops = replayStops(tonightRef.current, (id) => drawn.hasNode(id));
    if (stops.length === 0) return;
    // A picked thing is let go at once; the view the press left is kept.
    if (selectedRef.current) {
      flyRef.current(null, null);
      selectedRef.current = null;
      pinnedPointRef.current = null;
      setSelectedId(null);
      setSelectedThing(null);
    }
    const run: ReplayRun = {
      stops,
      timers: [],
      things: new Set(stops.map((stop) => stop.thing)),
      overview: captureAtlasViewpoint(renderer, drawn),
      ghosts: [],
      frame: 0,
    };
    replayRef.current = run;
    overviewRef.current = run.overview;
    focusLayoutRef.current = null;
    if (prefersReducedMotion()) {
      const points = stops.flatMap((stop) => thingPointIds(drawn, stop.thing));
      const topics = [...new Set(stops.flatMap((stop) => stop.topics))];
      hoverStateRef.current = { hovered: stops[0]!.thing, neighbors: new Set([...points, ...topics]), linked: new Set(topics) };
      frameAtlasView(renderer, drawn, {
        nodeIds: [...run.things, ...points, ...topics],
        insets: ATLAS_CONTROL_INSETS,
        duration: 0,
        padding: 40,
      });
      // What the night tidied rests beside what stands now, still, until Done.
      run.ghosts = stops.flatMap((stop) => stop.tidies.map((tidy) => ({ tidy, thing: stop.thing, startAt: 0, still: true })));
      setReplaying("marked");
      renderer.refresh();
      return;
    }
    setReplaying("playing");
    const beat = REPLAY_TIMING;
    let at = 0;
    const after = (ms: number, step: () => void) => {
      run.timers.push(window.setTimeout(() => {
        if (replayRef.current === run) step();
      }, ms));
    };
    stops.forEach((stop, index) => {
      if (index > 0) {
        after(at, () => flyRef.current(null, performance.now()));
        at += beat.home;
      }
      after(at, () => {
        selectThingRef.current(stop.thing, "this", { fly: false });
        growRef.current(stop.grow, performance.now() + beat.camera, growStagger(stop.grow.length));
      });
      at += beat.camera;
      if (stop.grow.length > 0) at += growMs(stop.grow.length) + beat.holdGrown;
      // UX-GRAPH-7: what survivors grew is there; what the night hid comes
      // in beside it and is taken in, one tidy after another.
      if (stop.tidies.length > 0) {
        after(at, () => {
          const now = performance.now();
          const gap = growStagger(stop.tidies.length);
          playGhosts(run, stop.tidies.map((tidy, index) => ({ tidy, thing: stop.thing, startAt: now + index * gap, still: false })));
        });
        at += tidyMs(stop.tidies.length) + beat.holdTidied;
      }
      if (stop.fly.length > 0) {
        after(at, () => {
          const view = sigmaRef.current;
          const graph = graphRef.current;
          if (!view || !graph) return;
          const nodeIds = [stop.thing, ...thingPointIds(graph, stop.thing), ...stop.topics].filter((id) => graph.hasNode(id));
          frameAtlasView(view, graph, { nodeIds, insets: ATLAS_CONTROL_INSETS, duration: beat.camera, padding: 40 });
          flyRef.current(stop.thing, performance.now() + beat.camera, new Set(stop.fly));
        });
        at += beat.camera + beat.fly + beat.holdFlown;
      }
    });
    after(at, () => endReplayRef.current("return"));
  };
  startReplayRef.current = startReplay;
  /** Shows these tidies, painting every frame while one moves; when they
   *  are all in, nothing of them is left. */
  const playGhosts = (run: ReplayRun, ghosts: readonly ReplayGhost[]) => {
    cancelAnimationFrame(run.frame);
    run.ghosts = ghosts;
    const tick = () => {
      if (replayRef.current !== run) return;
      const moving = ghostsMoving(run.ghosts, performance.now());
      if (!moving) run.ghosts = [];
      sigmaRef.current?.refresh();
      if (moving) run.frame = requestAnimationFrame(tick);
    };
    run.frame = requestAnimationFrame(tick);
  };
  /** Replay ends: Skip, Esc and its own end take the camera back; touching
   *  the map leaves it where the person put it. Everything lands, and what
   *  the map shows goes back to its usual rules. */
  const endReplay = (camera: "return" | "stay") => {
    const run = replayRef.current;
    if (!run) return;
    for (const timer of run.timers) window.clearTimeout(timer);
    // Ghosts go and survivors land at full size, wherever a tidy had got to.
    cancelAnimationFrame(run.frame);
    run.ghosts = [];
    growRef.current([], 0, 0);
    const renderer = sigmaRef.current;
    const drawn = graphRef.current;
    const animate = camera === "return" && !prefersReducedMotion() && replaying !== "marked";
    // Points fly home with the camera at the end; otherwise they are home at once.
    flyRef.current(null, animate ? performance.now() : null);
    replayRef.current = null;
    selectedRef.current = null;
    pinnedPointRef.current = null;
    overviewRef.current = null;
    focusLayoutRef.current = null;
    setSelectedId(null);
    setSelectedThing(null);
    setReplaying("idle");
    if (!renderer || !drawn) return;
    hoverStateRef.current = hoverStateFor(drawn, null);
    const view = renderer.getCamera();
    if (camera === "return") {
      const from = { x: view.x, y: view.y, ratio: view.ratio, angle: view.angle };
      if (restoreAtlasViewpoint(renderer, drawn, run.overview) && animate) {
        const to = { x: view.x, y: view.y, ratio: view.ratio, angle: view.angle };
        view.setState(from);
        void view.animate(to, { duration: REPLAY_TIMING.home, easing: "quadraticInOut" });
      } else if (view.isAnimated()) {
        void view.animate({ x: view.x, y: view.y, ratio: view.ratio, angle: view.angle }, { duration: 1 });
      }
    }
    sceneDirtyRef.current = true;
    renderer.refresh();
  };
  endReplayRef.current = endReplay;
  const frameMap = (mode: AtlasFrameMode) => {
    const renderer = sigmaRef.current;
    const drawn = graphRef.current;
    selectedRef.current = null;
    setSelectedId(null);
    setSelectedThing(null);
    overviewRef.current = null;
    focusLayoutRef.current = null;
    if (!renderer || !drawn) return;

    const camera = renderer.getCamera();
    const wasAnimated = camera.isAnimated();
    const visibleNodeIds = new Set(
      drawn.nodes().filter((id) => !entityTypeHidden(drawn.getNodeAttribute(id, "entityType"), excludedTypesRef.current)),
    );
    frameAtlasView(renderer, drawn, { mode, visibleNodeIds, padding: 56, insets: ATLAS_CONTROL_INSETS });
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
      if (replayRef.current) {
        endReplayRef.current("return");
        return;
      }
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
      setSelectedThing(null);
      overviewRef.current = null;
    }
  }, [filteredModel]);
  useEffect(() => {
    selectedRef.current = null;
    setSelectedId(null);
    setSelectedThing(null);
    overviewRef.current = null;
  }, [spaceFilter]);
  // Picking a thing flies its linked points out to their topics; letting it
  // go flies them home.
  useEffect(() => {
    // Replay moves the points itself.
    if (replayRef.current) return;
    flyRef.current(selectedThing, performance.now());
  }, [selectedThing]);
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

  // The map's host only exists once both reads are in (see the loading state
  // below). The things' read can land after the graph's and leave the model
  // as it was (no thing has points), so readiness itself must re-run the
  // mount: keyed on the model alone, the effect already ran with no host and
  // never ran again, and the map stayed blank.
  const mapReady = !graphLoading && !thingPointsPending;
  // Mount/rebuild sigma whenever the model changes. `palette` is read here
  // (fresh at build time) but deliberately not a dependency — a theme flip
  // recolors the existing graph in place (below) instead of tearing down and
  // remounting the whole renderer. Dispose before the canvas leaves the DOM;
  // a focus animation can otherwise render into a detached, zero-width host.
  useLayoutEffect(() => {
    // Dispose the current imperative renderer before React mutates or detaches
    // its host. Mount/rebuild stays passive so large layouts do not block paint.
    return () => rendererDisposeRef.current?.();
  }, [visibleModel, layoutVersion, mapReady]);

  useEffect(() => {
    const container = containerRef.current;
    // Guarded on the FULL model, not the drawn one: a graph whose components
    // are all smaller than MIN_COMPONENT_SIZE draws nothing but must still
    // mount, so the map frame and the "N unconnected or paired, hidden" chip
    // explain the emptiness instead of the view silently showing a blank slot.
    if (!container || (model.nodes.length === 0 && visibleModel.nodes.length === 0)) return;

    const graph = buildAtlasGraph(visibleModel, palette);
    // Each night point goes back to the spot it was given the first time it
    // was on the map; only new ones are placed (UX-CHAT-5).
    applyNightOffsets(graph, readNightOffsets());
    // Prototype switch (round 3): which core layout to use, for side-by-side
    // renders; the default is DEFAULT_ATLAS_LAYOUT.
    const layoutStartedAt = performance.now();
    let layoutVariant: AtlasLayoutVariant = DEFAULT_ATLAS_LAYOUT;
    try {
      const pick = window.localStorage.getItem("atlas.exp.layout");
      if (pick === "r0" || pick === "r1" || pick === "r1b" || pick === "r2") layoutVariant = pick;
    } catch {
      // Storage unavailable: the default layout.
    }
    // R2 (round 4): each body keeps the region it was in; only the one-time
    // rebalance works the regions out again (Put back brings the old ones back).
    const undoing = rearrangeRef.current === "undo";
    const regionsAtStart: ReadonlyMap<string, string> | undefined = (() => {
      if (layoutVariant !== "r2") return undefined;
      if (undoing) return readPreviousRegions(spaceFilter);
      if (readLayoutVersion(spaceFilter) < ATLAS_LAYOUT_VERSION) return undefined;
      const stored = readStoredRegions(spaceFilter);
      return stored.size > 0 ? stored : regionLayoutRef.current ?? stored;
    })();
    // A map opened again with its bodies on their kept spots is not laid out
    // again: the kept spots go straight back, and the few bodies new since
    // are placed beside what they link to (most of the cost of opening a
    // large map was a fresh layout that the kept one then replaced). An undo
    // or Put back always goes straight to the layout it brings back.
    const keptAtStart = (() => {
      if (!undoing && layoutVariant !== "r0" && readLayoutVersion(spaceFilter) < ATLAS_LAYOUT_VERSION) return null;
      const prior = viewpointRef.current;
      const stored = undoing ? readPreviousPositions(spaceFilter) : prior?.scope === spaceFilter ? prior.body : readStoredPositions(spaceFilter);
      if (stored.size === 0) return null;
      const apartNow = readThingsApart();
      let bodies = 0;
      let missing = 0;
      for (const id of graph.nodes()) {
        if (graph.getNodeAttribute(id, "entityType") === MEMORY_NODE_TYPE) continue;
        bodies += 1;
        if (!stored.has(id)) {
          missing += 1;
          continue;
        }
        const isApart = apartNow ? apartNow.has(id) : thingIdOf(id) !== null;
        if (isApart && thingLinkedBodies(graph, id).length > 0) return null;
      }
      if (bodies === 0 || missing === bodies) return null;
      return undoing || missing <= keptMissingAllowance(bodies) ? stored : null;
    })();
    // Put back to a layout from before regions: it comes back as it was,
    // without regions, until the layout version changes again.
    const regionless = layoutVariant === "r2" && keptAtStart !== null && (regionsAtStart?.size ?? 0) === 0;
    const job = workerLayoutRef.current;
    const fromWorker = !keptAtStart && job && job.model === visibleModel && job.version === layoutVersion && job.variant === layoutVariant
      ? job.positions
      : null;
    const placed = keptAtStart ?? fromWorker;
    // A layout run that will not end in a worker reply clears the note.
    setLayingOut(false);
    if (placed) {
      for (const [id, spot] of placed) {
        if (graph.hasNode(id)) graph.mergeNodeAttributes(id, { x: spot.x, y: spot.y });
      }
      // The worker worked the regions out already; they are not found again.
      markKeptLayout(graph, regionless ? "r1" : layoutVariant, spaceFilter ?? "", fromWorker ? job?.regions : regionsAtStart);
    } else if (graph.order >= WORKER_LAYOUT_MIN_NODES && typeof Worker !== "undefined" && !workerFailedRef.current) {
      // A large fresh layout takes seconds: run it off the main thread and
      // say so quietly, then build the map again from its spots.
      let blockedMs = 0;
      const worker = new Worker(new URL("../../lib/graph/atlasLayout.worker.ts", import.meta.url), { type: "module" });
      let live = true;
      worker.onmessage = (event: MessageEvent<AtlasLayoutReply>) => {
        worker.terminate();
        if (!live) return;
        workerLayoutRef.current = {
          model: visibleModel,
          version: layoutVersion,
          variant: layoutVariant,
          positions: new Map(event.data.positions.map(([id, x, y]) => [id, { x, y }])),
          ...(event.data.regions ? { regions: new Map(event.data.regions) } : {}),
          report: event.data.report,
          blockedMs,
          totalMs: performance.now() - layoutStartedAt,
        };
        setLayingOut(false);
        setWorkerLayouts((count) => count + 1);
      };
      worker.onerror = () => {
        worker.terminate();
        if (!live) return;
        // No worker after all: lay it out here, as before.
        workerLayoutRef.current = null;
        setLayingOut(false);
        workerFailedRef.current = true;
        setWorkerLayouts((count) => count + 1);
      };
      worker.postMessage({ graph: graph.export(), variant: layoutVariant, scope: spaceFilter ?? "", ...(regionsAtStart?.size ? { regions: [...regionsAtStart] } : {}) } satisfies AtlasLayoutRequest);
      blockedMs = performance.now() - layoutStartedAt;
      setLayingOut(true);
      return () => {
        live = false;
        worker.terminate();
        setLayingOut(false);
      };
    } else runAtlasLayout(graph, layoutVariant, spaceFilter ?? "", regionsAtStart);
    graphRef.current = graph;
    // A rebuild while Replay plays (a fresh read landing) ends it where it is.
    if (replayRef.current) {
      for (const timer of replayRef.current.timers) window.clearTimeout(timer);
      cancelAnimationFrame(replayRef.current.frame);
      replayRef.current = null;
      selectedRef.current = null;
      setSelectedThing(null);
      setReplaying("idle");
    }
    // An R2 map names its own regions (round 4): the paint reads them from
    // the layout instead of the cartography communities.
    if (layoutVariant === "r2" && !regionless) {
      const regionMembers = new Map<string, string>();
      graph.forEachNode((id, attrs) => {
        if (typeof attrs.region === "string") regionMembers.set(id, attrs.region);
      });
      regionLayoutRef.current = regionMembers;
      setLaidRegionCount(new Set(regionMembers.values()).size);
      communitiesRef.current = regionMembers;
      if (import.meta.env.DEV) (window as unknown as Record<string, unknown>).__ATLAS_REGIONS = {
        report: fromWorker ? job?.report : graph.getAttribute("atlasRegionReport"),
        regions: graph.getAttribute("atlasRegions"),
      };
    } else {
      regionLayoutRef.current = null;
      setLaidRegionCount(null);
    }
    hoverStateRef.current = hoverStateFor(graph, null);
    sceneRef.current = null;
    sceneDirtyRef.current = true;

    // Same-frame paint per physics step (see createAtlasSimulation's onTick
    // note). sigmaRef is still null during the synchronous settle ticks, so
    // the 220 pre-paint steps don't render.
    const sim = createAtlasSimulation(graph, () => {
      sceneDirtyRef.current = true;
      sigmaRef.current?.refresh();
    }, { settled: placed !== null });
    if (import.meta.env.DEV) (window as unknown as Record<string, unknown>).__ATLAS_LAYOUT = {
      variant: layoutVariant,
      kept: keptAtStart !== null,
      regionless,
      worker: fromWorker !== null,
      layoutMs: Math.round(performance.now() - layoutStartedAt + (fromWorker ? job!.blockedMs : 0)),
      ...(fromWorker ? { workerTotalMs: Math.round(job!.totalMs) } : {}),
    };
    const previous = viewpointRef.current;
    const bodyIds = graph.nodes().filter((id) => graph.getNodeAttribute(id, "entityType") !== MEMORY_NODE_TYPE);
    // Within a visit the last layout is in memory; on a new visit it comes
    // from the kept layout, so the same point is where it was (UX-GRAPH-1).
    let layoutChange: "fresh" | "undo" | null = rearrangeRef.current;
    rearrangeRef.current = null;
    // A layout kept from before the balanced map is laid out again, once, and
    // turned to sit as close to the old one as it can; the old one can be put
    // back (round 3, section 6).
    const balanced = layoutVariant !== "r0";
    const stale = balanced && !layoutChange && readLayoutVersion(spaceFilter) < ATLAS_LAYOUT_VERSION;
    const oldLayout = stale ? readStoredPositions(spaceFilter) : null;
    const rebalancing = oldLayout !== null && oldLayout.size > 0;
    if (rebalancing) layoutChange = "fresh";
    if (balanced) storeLayoutVersion(spaceFilter);
    if (layoutChange === "fresh") startRearrange(spaceFilter);
    else if (layoutChange === "undo") undoRearrange(spaceFilter);
    const kept = previous?.scope === spaceFilter && !layoutChange ? previous.body : readStoredPositions(spaceFilter);
    const apart = readThingsApart();
    if (bodyIds.length > 0 && kept.size > 0) {
      // Layer/small-group changes and new pages may add or remove nodes.
      // Restore every survivor rather than requiring an identical set, so the
      // learned map stays put while what is new settles around it.
      // A thing kept apart from the map (placed before its points linked to
      // anything) is placed afresh, next to its links, once they appear.
      const survivorPositions = new Map(
        bodyIds
          .filter((id) => !((apart ? apart.has(id) : thingIdOf(id) !== null) && thingLinkedBodies(graph, id).length > 0))
          .map((id) => [id, kept.get(id)] as const)
          .filter((entry): entry is readonly [string, { x: number; y: number }] => entry[1] !== undefined),
      );
      // A body new to an R2 region sits by its kept neighbours there, not
      // where the fresh layout happened to put it.
      // How many graph units one screen px spans once the kept map is fit
      // to the view: under R2 what is new keeps its drawn size apart.
      let unitsPerPx = 0;
      if (layoutVariant === "r2" && survivorPositions.size > 0) {
        let minX = Infinity, maxX = -Infinity, minY = Infinity, maxY = -Infinity;
        for (const p of survivorPositions.values()) {
          minX = Math.min(minX, p.x); maxX = Math.max(maxX, p.x);
          minY = Math.min(minY, p.y); maxY = Math.max(maxY, p.y);
        }
        unitsPerPx = container.clientWidth > 0 && container.clientHeight > 0
          ? 1.3 * Math.max((maxX - minX) / container.clientWidth, (maxY - minY) / container.clientHeight)
          : 0;
        for (const [id, spot] of placeNewRegionMembers(graph, survivorPositions, { unitsPerPx })) survivorPositions.set(id, spot);
      }
      if (survivorPositions.size > 0) sim.restorePositions(survivorPositions, { unitsPerPx });
    }
    if (rebalancing && oldLayout) {
      const fresh = new Map(bodyIds.map((id) => [id, { x: graph.getNodeAttribute(id, "x") as number, y: graph.getNodeAttribute(id, "y") as number }]));
      sim.restorePositions(alignToKept(fresh, oldLayout));
      // The saved camera framed the old layout.
      viewpointRef.current = null;
      setRebalanced(true);
    }
    const bodyPositions = () => {
      const positions = new Map(graph.nodes()
        .filter((id) => graph.getNodeAttribute(id, "entityType") !== MEMORY_NODE_TYPE)
        .map((id) => [id, { x: graph.getNodeAttribute(id, "x") as number, y: graph.getNodeAttribute(id, "y") as number }]));
      // A thing no longer on this map keeps its spot, so its frame comes back
      // where it was if it returns.
      for (const [id, position] of kept) {
        if (thingIdOf(id) !== null && !positions.has(id)) positions.set(id, position);
      }
      return positions;
    };
    // Keep this layout now, so it holds even if the app quits without
    // unmounting the map, and again when the window goes away.
    storePositions(spaceFilter, bodyPositions());
    if (layoutVariant === "r2" && regionLayoutRef.current) storeRegions(spaceFilter, regionLayoutRef.current);
    storeNightOffsets(nightOffsetsOf(graph));
    // Which things on the map now sit apart from it: kept for the next open.
    const thingsHere = bodyIds.filter((id) => thingIdOf(id) !== null);
    const here = new Set(thingsHere);
    storeThingsApart([
      ...[...(apart ?? [])].filter((id) => !here.has(id)),
      ...thingsHere.filter((id) => thingLinkedBodies(graph, id).length === 0),
    ]);
    // A point new since the map was last opened grows out of its thing's
    // centre, once (UX-CHAT-5); it is seen from now on, so it never does again.
    // Opened by home's Replay: they grow once, in the replay.
    const nightNow = nightPointIds(graph);
    if (!replayPendingRef.current) for (const id of newNightPoints(nightNow, readSeenNightPoints())) growQueueRef.current.add(id);
    storeSeenNightPoints(nightNow);
    const growing = prefersReducedMotion() ? [] : nightNow.filter((id) => growQueueRef.current.has(id));
    if (prefersReducedMotion()) growQueueRef.current.clear();
    // Sigma (3.0.3) takes a node's place from the graph on every full paint,
    // not from the reducer, so a growing point is moved on the graph itself:
    // out from its thing's centre to the spot just kept, which it ends on
    // exactly. Nothing kept is touched (its spot was stored above). Each
    // point may start a little after the last (Replay), so each has its own
    // delay and its own progress.
    type Growth = { targets: Map<string, { x: number; y: number; delay: number }>; start: number; progress: Map<string, number>; end: number };
    const growthOf = (ids: readonly string[], start: number, stagger: number): Growth | null => {
      const targets = new Map(ids.filter((id) => graph.hasNode(id)).map((id, index) => [id, {
        x: graph.getNodeAttribute(id, "x") as number,
        y: graph.getNodeAttribute(id, "y") as number,
        delay: index * stagger,
      }] as const));
      return targets.size > 0 ? { targets, start, progress: new Map(), end: GROWTH_MS + (targets.size - 1) * stagger } : null;
    };
    let growth = growthOf(growing, 0, 0);
    const placeGrowth = (elapsed: number) => {
      if (!growth) return;
      for (const [id, target] of growth.targets) {
        const at = growthAt(elapsed - target.delay);
        growth.progress.set(id, at);
        const thing = graph.getNodeAttribute(id, "dustOf") as string | undefined;
        if (at >= 1 || !thing || !graph.hasNode(thing)) {
          graph.mergeNodeAttributes(id, { x: target.x, y: target.y });
          continue;
        }
        const cx = graph.getNodeAttribute(thing, "x") as number;
        const cy = graph.getNodeAttribute(thing, "y") as number;
        graph.mergeNodeAttributes(id, { x: cx + (target.x - cx) * at, y: cy + (target.y - cy) * at });
      }
    };
    placeGrowth(0);
    let growthFrame = 0;
    const growStep = () => {
      if (!growth) return;
      if (growth.start === 0) growth.start = performance.now();
      const elapsed = performance.now() - growth.start;
      placeGrowth(elapsed);
      if (elapsed >= growth.end) {
        for (const id of growth.targets.keys()) growQueueRef.current.delete(id);
        growth = null;
      }
      sceneDirtyRef.current = true;
      sigmaRef.current?.refresh();
      if (growth) growthFrame = requestAnimationFrame(growStep);
    };
    // Replay's growing: whatever was still growing lands first.
    growRef.current = (ids, startAt, stagger) => {
      cancelAnimationFrame(growthFrame);
      placeGrowth(Infinity);
      growth = growthOf(ids, startAt, stagger);
      placeGrowth(performance.now() - startAt);
      sceneDirtyRef.current = true;
      sigmaRef.current?.refresh();
      if (growth) growthFrame = requestAnimationFrame(growStep);
    };
    // A picked thing's points that link to a topic fly out of its frame to
    // sit beside that topic, and home again when it is let go (UX-CHAT-5,
    // Qi-Xuan 11-05): no line runs across the map, and the flight shows the
    // thing reaching into it. A quiet thing's are by their topics already.
    let flightThing: string | null = null;
    let flightTargets: ReadonlyMap<string, FlyTo> = new Map();
    let flightAt = 0;
    let flightTo = 0;
    let flightFrame = 0;
    const showFlight = () => {
      sim.placeSatellitesNow();
      sceneDirtyRef.current = true;
      sigmaRef.current?.refresh();
    };
    // `startAt` (performance.now() time) is when the flight sets off, so a
    // pick's points wait for the camera to arrive and are then seen to fly;
    // null puts them there at once.
    const fly = (wanted: string | null, startAt: number | null, only?: ReadonlySet<string>) => {
      if (wanted !== null && !graph.hasNode(wanted)) wanted = null;
      // Already there, or already on its way there.
      if (wanted === flightThing && (wanted === null || flightTo === 1)) return;
      cancelAnimationFrame(flightFrame);
      // Another thing picked: the last one's points are home at once.
      if (flightThing !== null && wanted !== null && wanted !== flightThing) {
        setThingFlight(graph, null, 0);
        flightAt = 0;
      }
      const thing = wanted ?? flightThing;
      if (thing === null) return;
      if (thing !== flightThing || only) {
        flightTargets = thingFlightTargets(graph, thing);
        // Replay flies only what last night connected.
        if (only) flightTargets = new Map([...flightTargets].filter(([id]) => only.has(id)));
      }
      flightThing = thing;
      const from = flightAt;
      const to = wanted === null ? 0 : 1;
      flightTo = to;
      const land = () => {
        flightAt = to;
        if (to === 0) {
          setThingFlight(graph, null, 0);
          flightThing = null;
          flightTo = 0;
        } else {
          setThingFlight(graph, thing, 1, flightTargets);
        }
        showFlight();
      };
      // Out: eased in and out over FLIGHT_MS, so the whole way is seen, not
      // a jump and a settle. Home: quicker, easing out, beside the camera.
      const duration = to === 1 ? FLIGHT_MS : HOME_MS;
      if (startAt === null || prefersReducedMotion() || performance.now() >= startAt + duration) {
        land();
        return;
      }
      const step = () => {
        const elapsed = performance.now() - startAt;
        if (elapsed >= duration) {
          land();
          return;
        }
        if (elapsed > 0) {
          const t = elapsed / duration;
          const eased = to === 1 ? (t < 0.5 ? 4 * t ** 3 : 1 - (2 - 2 * t) ** 3 / 2) : growthAt(elapsed, duration);
          flightAt = from + (to - from) * eased;
          setThingFlight(graph, thing, flightAt, flightTargets);
          showFlight();
        }
        flightFrame = requestAnimationFrame(step);
      };
      flightFrame = requestAnimationFrame(step);
    };
    flyRef.current = fly;
    const keepOnHide = () => storePositions(spaceFilter, bodyPositions());
    window.addEventListener("pagehide", keepOnHide);
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
    const showOverviewFallbackLabels = !hasMeaningfulComponent;
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
    overlay.style.display = showRegionsRef.current || layers.memory || hasThingsRef.current ? "" : "none";
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
    // The things' frames and titles as last painted: no other name is
    // printed across them (UX-CHAT-5). Frames are painted after the labels,
    // so the labels go by the previous paint's (see drawOverlay).
    let frameGuards: readonly { left: number; right: number; top: number; bottom: number }[] = [];
    let guardRender = false;
    let guardPasses = 0;
    const labelBox = (ctx: CanvasRenderingContext2D, label: string, at: LabelPlacement): Box => {
      ctx.font = NODE_LABEL_FONT;
      const width = ctx.measureText(label).width;
      const left = at.align === "center" ? at.x - width / 2 : at.align === "right" ? at.x - width : at.x;
      const top = at.baseline === "middle" ? at.y - 6 : at.baseline === "top" ? at.y : at.y - 12;
      return { left: left - 4, right: left + width + 4, top: top - 3, bottom: top + 15 };
    };
    // What a name may not cover besides another name: every drawn disc but
    // its own, and the map's controls.
    // Rebuilt once per paint, on the first label sigma asks for, when the
    // display data for that paint are final.
    // The place-name scene, rebuilt only when a paint marked it dirty. The
    // label pass may need it before the overlay does (R2 names go first).
    const currentScene = () => {
      if (sceneDirtyRef.current || sceneRef.current === null) {
        const sceneGraph = excludedTypesRef.current.size ? graph.copy() : graph;
        if (sceneGraph !== graph) for (const id of sceneGraph.nodes()) {
          if (entityTypeHidden(sceneGraph.getNodeAttribute(id, "entityType"), excludedTypesRef.current)) sceneGraph.dropNode(id);
        }
        sceneRef.current = cartographyScene(sceneGraph, communitiesRef.current);
        sceneDirtyRef.current = false;
      }
      return sceneRef.current;
    };
    let guard: LabelGuard | null = null;
    const ensureGuard = (ctx: CanvasRenderingContext2D): LabelGuard | null => {
      if (guard) return guard;
      const sigma = labelSigma;
      if (sigma === null) return null;
      const { width, height } = sigma.getDimensions();
      const cells = new Map<number, GuardDisc[]>();
      const discs: GuardDisc[] = [];
      graph.forEachNode((id) => {
        const display = sigma.getNodeDisplayData(id);
        if (!display || display.hidden) return;
        const r = sigma.scaleSize(display.size);
        if (!(r >= LABEL_GUARD_MIN_RADIUS)) return;
        const { x, y } = sigma.framedGraphToViewport(display);
        // A name near the edge may still show part of itself over a disc just off screen.
        const margin = r + 24;
        if (x < -margin || y < -margin || x > width + margin || y > height + margin) return;
        const dustOf = graph.getNodeAttribute(id, "dustOf");
        const disc: GuardDisc = { key: id, x, y, r, owner: typeof dustOf === "string" ? dustOf : undefined };
        discs.push(disc);
        for (let cx = Math.floor((x - r) / LABEL_GUARD_CELL); cx <= Math.floor((x + r) / LABEL_GUARD_CELL); cx += 1) {
          for (let cy = Math.floor((y - r) / LABEL_GUARD_CELL); cy <= Math.floor((y + r) / LABEL_GUARD_CELL); cy += 1) {
            const cell = cx * 65536 + cy;
            const list = cells.get(cell);
            if (list) list.push(disc);
            else cells.set(cell, [disc]);
          }
        }
      });
      const chrome: Box[] = [];
      const shell = container.parentElement;
      if (shell) {
        const origin = container.getBoundingClientRect();
        shell.querySelectorAll(ATLAS_CHROME_SELECTOR).forEach((element) => {
          const rect = element.getBoundingClientRect();
          if (rect.width <= 0 || rect.height <= 0) return;
          chrome.push({ left: rect.left - origin.left - 4, right: rect.right - origin.left + 4, top: rect.top - origin.top - 4, bottom: rect.bottom - origin.top + 4 });
        });
      }
      // How far each hub's own dust reaches on screen: its name sits outside it.
      const rings = new Map<string, number>();
      for (const disc of discs) {
        if (disc.owner === undefined) continue;
        const display = sigma.getNodeDisplayData(disc.owner);
        if (!display || display.hidden) continue;
        const at = sigma.framedGraphToViewport(display);
        rings.set(disc.owner, Math.max(rings.get(disc.owner) ?? 0, Math.hypot(disc.x - at.x, disc.y - at.y) + disc.r));
      }
      guard = { cells, chrome, width, height, plan: null, rings };
      // An R2 map at overview zoom is a map of named regions: the region
      // names go down first and the point names give way to them (round 5).
      // A name keeps off every drawn point, the smallest too, and the
      // controls.
      if (regionLayoutRef.current && lodRef.current.phase === "overview" && hoverStateRef.current.hovered === null) {
        const scene = currentScene();
        const all = allDiscCells(sigma, width, height);
        const blocked = (box: Box) => {
          if (chrome.some((o) => o.left < box.right && box.left < o.right && o.top < box.bottom && box.top < o.bottom)) return true;
          return discsHit(all, box, 1);
        };
        const names = planRegionNamesFirst(
          lodRef.current.islandsSolid ? scene : { regions: scene.regions.filter((r) => !r.island) },
          (pos) => sigma.graphToViewport(pos),
          regionNameMeasure(ctx),
          { width, height },
          blocked,
        );
        guard.regionNames = names;
        guard.namedHubs = new Set(names.map((n) => n.hubId));
        for (const name of names) labelBoxes.push(name.box);
      }
      // Zoomed out and at the middle zoom, the names are the map's landmarks:
      // the best-linked first,
      // each on the first side that is free, up to OVERVIEW_LABEL_MAX, so they
      // spread over the map instead of piling into its densest cluster.
      const planPhase = lodRef.current.phase;
      if (planPhase !== "detail" && hoverStateRef.current.hovered === null) {
        const plan = new Map<string, { label: string; at: LabelPlacement } | null>();
        const maxWidth = overviewLabelMaxWidth(width);
        let placedCount = 0;
        const candidates = discs
          .filter((d) => graph.getNodeAttribute(d.key, "landmark") === true && graph.getNodeAttribute(d.key, "night") !== true)
          .sort((a, b) =>
            ((graph.getNodeAttribute(b.key, "structuralDegree") as number | undefined) ?? 0)
            - ((graph.getNodeAttribute(a.key, "structuralDegree") as number | undefined) ?? 0)
            || (a.key < b.key ? -1 : 1));
        for (const disc of candidates) {
          if (placedCount >= OVERVIEW_LABEL_MAX || guard.namedHubs?.has(disc.key)) {
            plan.set(disc.key, null);
            continue;
          }
          plan.set(disc.key, null);
          const display = sigma.getNodeDisplayData(disc.key);
          const raw = String(display?.label ?? "");
          if (!raw) continue;
          ctx.font = NODE_LABEL_FONT;
          const label = planPhase === "overview" ? truncateOverviewLabel(ctx, raw, maxWidth) : raw;
          if (!label) continue;
          const preferred = labelSideFor(graph.getNodeAttribute(disc.key, "x") as number, graph.getNodeAttribute(disc.key, "y") as number);
          for (const at of labelCandidates(disc.x, disc.y, labelRing(disc.r, guard.rings.get(disc.key)), preferred)) {
            const box = labelBox(ctx, label, at);
            if (!boxFree(box, disc.key, true, disc)) continue;
            labelBoxes.push(box);
            plan.set(disc.key, { label, at });
            placedCount += 1;
            break;
          }
        }
        guard.plan = plan;
      }
      return guard;
    };
    const boxFree = (box: Box, key: string | undefined, guarded: boolean, own?: { x: number; y: number; r: number }) => {
      const taken = (other: Box) =>
        other.left < box.right && box.left < other.right && other.top < box.bottom && box.top < other.bottom;
      if (labelBoxes.some(taken) || (guarded && frameGuards.some(taken))) return false;
      if (key === undefined || guard === null) return true;
      if (box.left < LABEL_EDGE_INSET || box.top < LABEL_EDGE_INSET || box.right > guard.width - LABEL_EDGE_INSET || box.bottom > guard.height - LABEL_EDGE_INSET) return false;
      if (guard.chrome.some(taken)) return false;
      for (let cx = Math.floor(box.left / LABEL_GUARD_CELL); cx <= Math.floor(box.right / LABEL_GUARD_CELL); cx += 1) {
        for (let cy = Math.floor(box.top / LABEL_GUARD_CELL); cy <= Math.floor(box.bottom / LABEL_GUARD_CELL); cy += 1) {
          for (const disc of guard.cells.get(cx * 65536 + cy) ?? []) {
            if (disc.key === key) continue;
            if (LABEL_PASSES_SMALL && disc.r < 2.5 && lodRef.current.phase === "overview" && graph.getNodeAttribute(disc.key, "landmark") !== true) continue;
            const nx = Math.max(box.left, Math.min(disc.x, box.right));
            const ny = Math.max(box.top, Math.min(disc.y, box.bottom));
            if (Math.hypot(disc.x - nx, disc.y - ny) < disc.r) return false;
          }
        }
      }
      // Whose name is it: the dot nearest the box's inner edge must be the
      // label's own, or a reader would take it for a neighbour's.
      if (own) {
        const px = Math.max(box.left, Math.min(own.x, box.right));
        const py = Math.max(box.top, Math.min(own.y, box.bottom));
        const ownGap = Math.hypot(own.x - px, own.y - py) - own.r;
        for (let cx = Math.floor((px - ownGap) / LABEL_GUARD_CELL); cx <= Math.floor((px + ownGap) / LABEL_GUARD_CELL); cx += 1) {
          for (let cy = Math.floor((py - ownGap) / LABEL_GUARD_CELL); cy <= Math.floor((py + ownGap) / LABEL_GUARD_CELL); cy += 1) {
            for (const disc of guard.cells.get(cx * 65536 + cy) ?? []) {
              if (disc.key === key || disc.owner === key) continue;
              if (Math.hypot(disc.x - px, disc.y - py) - disc.r < ownGap) return false;
            }
          }
        }
      }
      return true;
    };
    const reserveLabel = (ctx: CanvasRenderingContext2D, label: string, at: LabelPlacement, guarded = false, key?: string, own?: { x: number; y: number; r: number }) => {
      const box = labelBox(ctx, label, at);
      if (!boxFree(box, key, guarded, own)) return false;
      labelBoxes.push(box);
      return true;
    };
    // The first free side for a name, starting from its usual one.
    const placeLabel = (ctx: CanvasRenderingContext2D, label: string, data: Record<string, any>, preferred: LabelSide, guarded: boolean): LabelPlacement | null => {
      const own = { x: data.x as number, y: data.y as number, r: data.size as number };
      const ring = labelRing(own.r, guard?.rings.get(data.key));
      for (const at of labelCandidates(own.x, own.y, ring, preferred)) {
        if (reserveLabel(ctx, label, at, guarded, data.key, own)) return at;
      }
      return null;
    };
    // The node under the pointer, whatever the hover state shows.
    let pointerNode: string | null = null;
    // A point shows its whole sentence only when asked: under the pointer,
    // pinned by a click, or at its picked thing's own stop (one thing, and
    // the reader went there to read it). The overlay paints it (night.ts).
    // Zoomed in, every other point carries a short title; zoomed out, none.
    // At the thing's stop they wait for the camera to arrive, so they are not
    // seen in a pile while it is still zoomed out.
    const sentencesFor = (thing: string, point: string) =>
      point === pointerNode || point === pinnedPointRef.current
      || (selectedRef.current === thing && thingStopRef.current === "this" && performance.now() >= pickedAtRef.current + THING_CAMERA_MS);
    const drawLabel = (ctx: CanvasRenderingContext2D, data: Record<string, any>, s: Record<string, any>) => {
      const nodeAttrs = graph.hasNode(data.key) ? graph.getNodeAttributes(data.key) : null;
      const labelGuard = ensureGuard(ctx);
      const nightPoint = nodeAttrs?.night === true && typeof nodeAttrs.dustOf === "string";
      // A picked quiet thing's point beside its topic: a short title too, not
      // its whole sentence.
      const litPoint = nodeAttrs?.entityType === MEMORY_NODE_TYPE && hoverStateRef.current.thing !== undefined && hoverStateRef.current.neighbors.has(data.key);
      if (nightPoint || litPoint) {
        // Its sentence is painted by the overlay, wrapped beside its point;
        // zoomed out, a point carries no words at all.
        if (nightPoint && sentencesFor(nodeAttrs.dustOf, data.key)) return;
        // So is a picked quiet thing's at its own stop (night.ts).
        const picked = selectedRef.current;
        if (litPoint && picked && graph.hasNode(picked) && ((graph.getNodeAttribute(picked, "elsewhere") as string[] | undefined) ?? []).includes(data.key) && sentencesFor(picked, data.key)) return;
        if (lodRef.current.phase === "overview") return;
        ctx.font = NODE_LABEL_FONT;
        const title = truncateOverviewLabel(ctx, String(data.label ?? ""), POINT_TITLE_PX);
        if (!title) return;
        const angle = Math.atan2(-(graph.getNodeAttribute(data.key, "y") as number), graph.getNodeAttribute(data.key, "x") as number);
        const sector = Math.round((angle + Math.PI) / (Math.PI / 2)) % 4;
        // Where a title has no free side, it is left off.
        const at = placeLabel(ctx, title, data, sector === 0 ? "right" : sector === 2 ? "left" : sector === 1 ? "below" : "above", false);
        if (at) drawNodeLabelAt(ctx, { ...data, label: title }, s, at, paletteRef.current.surface);
        return;
      }
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
      // A planned landmark is already reserved; one the plan left off stays
      // blank. A landmark the plan never saw (no drawn disc) goes the usual way.
      if (labelGuard?.namedHubs?.has(data.key)) return;
      if (labelGuard?.plan?.has(data.key)) {
        const planned = labelGuard.plan.get(data.key);
        if (planned) drawNodeLabelAt(ctx, { ...data, label: planned.label }, s, planned.at, paletteRef.current.surface);
        return;
      }
      if (overview) ctx.font = NODE_LABEL_FONT;
      const maxWidth = overviewLabelMaxWidth(labelSigma?.getDimensions().width ?? 160);
      const label = overview ? truncateOverviewLabel(ctx, rawLabel, maxWidth) : rawLabel;
      if (!label) return;
      const preferred = labelSideFor(graph.getNodeAttribute(data.key, "x") as number, graph.getNodeAttribute(data.key, "y") as number);
      const at = placeLabel(ctx, label, data, preferred, labelYieldsToFrames(nodeAttrs?.night === true, hoverStateRef.current.linked?.has(data.key) === true));
      if (at) drawNodeLabelAt(ctx, { ...data, label }, s, at, paletteRef.current.surface);
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
        // A thing's night points show only while that thing is picked (from
        // its page, search, or a stop): the open map stays clear of them
        // (UX-CHAT-5; Fable's design: the thing is a camera, not a mark).
        // The "Working on" switch shows every active thing's points at once;
        // a quiet thing's show only while it is picked.
        // Replay shows every point of last night's things, whatever the switch says.
        if (attrs.night === true && typeof attrs.dustOf === "string" && (!showWorkingRef.current || attrs.quiet === true) && selectedRef.current !== attrs.dustOf && !replayRef.current?.things.has(attrs.dustOf)) return { ...attrs, hidden: true };
        // A small group stays out of sight until the islands come up solid,
        // unless it is what the reader is looking at.
        const hover = hoverStateRef.current;
        if (tuckedRef.current.has(node) && !lodRef.current.islandsSolid && hover.hovered !== node && !hover.neighbors.has(node) && node !== effectiveFocusId) return { ...attrs, hidden: true };
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
        // Zoomed in, each shown point is offered its short title; drawLabel
        // leaves off the ones that would overlap.
        if (attrs.night === true && lodRef.current.phase !== "overview" && !display.hidden) display.forceLabel = true;
        const floor = lodRef.current.phase === "overview" && attrs.landmark ? minimumGraphRadius * 2 : minimumGraphRadius;
        const cap = nodeSizeCapPx(attrs, node === hoverStateRef.current.hovered);
        const ghosts = replayRef.current?.ghosts;
        // While Replay shows what a tidy took away, what stands for it reads
        // as short as the old words beside it, one line each (Qi-Xuan
        // 11-08: easy to compare at a glance); its full words come back
        // when Replay ends.
        if (ghosts && attrs.label && ghosts.some((ghost) => ghost.tidy.survivor === node && ghost.tidy.said.length > 0)) {
          display.label = saidLine(String(attrs.label), measureLabel);
          display.forceLabel = true;
        }
        const size = Math.min(Math.max(display.size, floor), cap * graphUnitsPerPixel) * (ghosts && ghosts.length > 0 ? swellOf(ghosts, node, performance.now()) : 1);
        if (growth && growth.targets.has(node)) return { ...display, size: size * Math.max(0.15, growth.progress.get(node) ?? 0) };
        return { ...display, size };
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

    // Replay's tidies (UX-GRAPH-7): each ghost a hollow ring in the colour
    // of what stands now, placed from where that is on screen now, so a
    // camera move carries them along. Drawn over everything, then gone.
    const drawGhosts = () => {
      const run = replayRef.current;
      if (!run || run.ghosts.length === 0) return;
      const ctx = overlay.getContext("2d");
      if (!ctx) return; // jsdom
      const now = performance.now();
      ctx.save();
      ctx.lineWidth = GHOST_STROKE;
      for (const ghost of run.ghosts) {
        const phase = ghostPhase(ghost, now - ghost.startAt);
        if (phase.alpha <= 0) continue;
        const thing = renderer.getNodeDisplayData(ghost.thing);
        const centre = thing ? renderer.framedGraphToViewport(thing) : null;
        const shown = ghost.tidy.survivor ? renderer.getNodeDisplayData(ghost.tidy.survivor) : undefined;
        const standing = shown && !shown.hidden ? shown : null;
        // Filtered off the map: no motion, as the line does not count it.
        // A replace whose kept memory is not drawn is pushed off the thing.
        if (!standing && (ghost.tidy.kind !== "replace" || !centre)) continue;
        const at = standing ? renderer.framedGraphToViewport(standing) : centre!;
        const radius = standing ? renderer.scaleSize(standing.size) / swellOf([ghost], ghost.tidy.survivor!, now) : GHOST_FALLBACK_RADIUS;
        // An update's old version spreads out in the bridge colour, so it
        // never reads as a replace's ghost.
        ctx.strokeStyle = ghost.tidy.kind === "update" ? paletteRef.current.bridge : standing?.color ?? paletteRef.current.memory;
        ctx.globalAlpha = phase.alpha;
        ghostRests(standing ? at : null, centre, radius, ghost.tidy.ghosts).forEach((rest, index) => {
          const spot = ghostAt(rest, at, phase.slide);
          const ring = radius * phase.scale;
          ctx.beginPath();
          ctx.arc(spot.x, spot.y, ring, 0, Math.PI * 2);
          ctx.stroke();
          // The words that went, faint and struck through, moving with
          // their ghost: on its outer side, or above the dot for an update
          // so they never sit on the words that stand now.
          const said = ghost.tidy.said[index];
          if (!said || phase.words <= 0) return;
          ctx.font = NODE_LABEL_FONT;
          const text = saidLine(said, (t) => ctx.measureText(t).width);
          const width = ctx.measureText(text).width;
          const update = ghost.tidy.kind === "update";
          const left = !update && spot.x < at.x;
          const x = update ? spot.x + radius + 4 : left ? spot.x - ring - 6 - width : spot.x + ring + 6;
          const y = update ? spot.y - radius - 12 : spot.y;
          ctx.save();
          ctx.globalAlpha = phase.words;
          ctx.textBaseline = "middle";
          ctx.textAlign = "left";
          // A halo in the map's ground keeps them readable over anything
          // drawn underneath; the strike stays light so the words lead.
          ctx.strokeStyle = paletteRef.current.surface;
          ctx.lineWidth = 4;
          ctx.lineJoin = "round";
          ctx.strokeText(text, x, y);
          // A touch quieter than the words that stand now beside them.
          ctx.globalAlpha = phase.words * 0.7;
          ctx.fillStyle = paletteRef.current.label;
          ctx.fillText(text, x, y);
          ctx.globalAlpha = phase.words * 0.45;
          ctx.strokeStyle = paletteRef.current.label;
          ctx.lineWidth = 1;
          ctx.beginPath();
          ctx.moveTo(x, y + 0.5);
          ctx.lineTo(x + width, y + 0.5);
          ctx.stroke();
          ctx.restore();
        });
      }
      ctx.restore();
    };
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
      // Region names keep off the frames too; the frames' own
      // marks are placed against the fresh labels only.
      const guardStart = labelBoxes.length;
      labelBoxes.push(...frameGuards);
      // An R2 map is a map of named regions: its names show without the toggle.
      // An R2 name keeps off every drawn point, small ones too, and off the
      // map's controls (round 4).
      let regionNameBlocked: ((box: Box) => boolean) | undefined;
      const overlayGuard = regionLayoutRef.current && lodRef.current.phase === "overview" ? (guard ?? ensureGuard(ctx)) : guard;
      if (regionLayoutRef.current && !overlayGuard?.regionNames) {
        const cells = allDiscCells(renderer, width, height);
        const extra = overlayGuard ? overlayGuard.chrome : [];
        regionNameBlocked = (box) =>
          extra.some((o) => o.left < box.right && box.left < o.right && o.top < box.bottom && box.top < o.bottom) || discsHit(cells, box, 1);
      }
      if (overlayGuard?.regionNames) drawPlannedRegionNames(ctx, overlayGuard.regionNames, paletteRef.current);
      else if (showRegionsRef.current || regionLayoutRef.current) drawRegionNames(ctx, named, project, paletteRef.current, { width, height }, labelledNodes, labelBoxes, regionNameBlocked);
      labelBoxes.splice(guardStart, frameGuards.length);
      const drawnAt = (id: string) => {
        const display = renderer.getNodeDisplayData(id);
        if (!display || display.hidden) return null;
        const at = renderer.framedGraphToViewport(display);
        return { x: at.x, y: at.y, r: renderer.scaleSize(display.size) };
      };
      // The thing's node is hidden but still laid out: its display spot is in
      // the same framed units as every other node's.
      const centreAt = (thing: string) => {
        const display = renderer.getNodeDisplayData(thing);
        return display ? renderer.framedGraphToViewport(display) : null;
      };
      const nightMarksNow = nightMarks(graph, drawnAt, hoverStateRef.current, { width, height }, sentencesFor, selectedRef.current, undefined, showWorkingRef.current, centreAt);
      // While Replay shows what a tidy took away, what stands for it reads
      // as short as the old words beside it, one line each (Qi-Xuan 11-08:
      // easy to compare at a glance); its whole sentence comes back after.
      const tidyGhosts = replayRef.current?.ghosts ?? [];
      const shortened = new Set(tidyGhosts.filter((ghost) => ghost.tidy.said.length > 0 && ghost.tidy.survivor).map((ghost) => ghost.tidy.survivor!));
      const marks = shortened.size === 0 ? nightMarksNow : {
        ...nightMarksNow,
        sentences: nightMarksNow.sentences.map((sentence) => (shortened.has(sentence.id) ? { ...sentence, text: saidLine(sentence.text, measureLabel), dim: false } : sentence)),
      };
      // Sentences keep off the other bodies on screen, not only the labels;
      // with a thing picked, only off what is lit: the dimmed rest is
      // background a sentence may cross.
      const bodies: { left: number; right: number; top: number; bottom: number }[] = [];
      const focus = hoverStateRef.current;
      if (marks.sentences.length > 0) {
        graph.forEachNode((id, attrs) => {
          if (attrs.night === true || attrs.entityType === THING_NODE_TYPE) return;
          if (focus.thing !== undefined && !focus.neighbors.has(id)) return;
          const at = drawnAt(id);
          if (!at || at.x < -at.r || at.y < -at.r || at.x > width + at.r || at.y > height + at.r) return;
          bodies.push({ left: at.x - at.r, right: at.x + at.r, top: at.y - at.r, bottom: at.y + at.r });
        });
      }
      const hits = drawNightMarks(ctx, marks, paletteRef.current, labelBoxes, bodies, selectedRef.current);
      frameHitsRef.current = hits;
      // Where the frames moved, paint the labels once more against them;
      // twice at most in a row, so the two can never chase each other.
      const moved = hits.length !== frameGuards.length || hits.some((hit, index) => {
        const was = frameGuards[index]!;
        return Math.abs(hit.left - was.left) > 0.5 || Math.abs(hit.top - was.top) > 0.5 || Math.abs(hit.right - was.right) > 0.5 || Math.abs(hit.bottom - was.bottom) > 0.5;
      });
      frameGuards = hits;
      guardPasses = guardRender ? guardPasses + 1 : 0;
      guardRender = false;
      if (moved && guardPasses < 2) {
        guardRender = true;
        renderer.scheduleRender();
      }
    };
    // One handler paints the overlay from one scene — rebuilt only when a
    // paint marked it dirty — and sigma sees a single afterRender listener.
    renderer.on("beforeRender", () => {
      labelBoxes.length = 0;
      guard = null;
    });
    renderer.on("afterRender", () => {
      drawOverlay(currentScene());
      drawGhosts();
    });
    // The opening frame follows the hierarchy's meaningful components. A
    // later rebuild restores its saved viewpoint below; only a fresh graph
    // receives this initial fit.
    const visibleNodeIds = new Set(
      graph.nodes().filter((id) => !entityTypeHidden(graph.getNodeAttribute(id, "entityType"), excludedTypesRef.current)),
    );
    const saved = viewpointRef.current;
    const restored = saved?.scope === spaceFilter && restoreAtlasViewpoint(renderer, graph, saved.view);
    // The opening view keeps the toolbar's band clear: no frame starts
    // under the search field (UX-CHAT-5).
    const openingFrame = { mode: "core" as const, visibleNodeIds, padding: 56, insets: ATLAS_CONTROL_INSETS };
    if (!restored) frameAtlasView(renderer, graph, openingFrame);
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
    // Zooming out past twice the opening view only shrinks the map to a
    // blob with its counts stacked on it; stop there.
    // Measured from the fit, not the live camera: a rebuild that restored a
    // zoomed-in view must still let you zoom out past the whole map.
    const fitRatio = restored ? atlasFrameState(renderer, graph, openingFrame)?.ratio ?? mountRatio : mountRatio;
    renderer.setSetting("maxCameraRatio", Math.max(fitRatio, mountRatio) * MAX_ZOOM_OUT);
    const applyLabelLod = (phase: LodState["phase"]) => {
      const lod = LABEL_LOD[phase];
      renderer.setSetting("labelRenderedSizeThreshold", lod.threshold);
      renderer.setSetting("labelGridCellSize", lod.cell);
      renderer.setSetting("labelDensity", lod.density);
    };
    applyLabelLod(OPENING_LOD.phase);
    const camera = renderer.getCamera();
    const onCameraUpdated = ({ ratio }: { ratio: number }) => {
      const next = lodFor(openingRatioRef.current / ratio);
      const prev = lodRef.current;
      const zoomChanged = ratio !== previousRatio;
      previousRatio = ratio;
      updateRadiusScale();
      if (!zoomChanged && next.dustVisible === prev.dustVisible && next.islandsSolid === prev.islandsSolid && next.smallIslandsSolid === prev.smallIslandsSolid && next.phase === prev.phase) return;
      lodRef.current = next;
      // Closer in, more names: a smaller disc may carry one and the label
      // grid is finer, so zooming names more things, not fewer.
      if (next.phase !== prev.phase) applyLabelLod(next.phase);
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
    // Arriving from a thing's page: go straight to its 這件事 stop, once.
    let arriving = false;
    if (focusThingId && !thingArrivalDoneRef.current && graph.hasNode(thingNodeId(focusThingId))) {
      thingArrivalDoneRef.current = true;
      arriving = true;
      selectThingRef.current(thingNodeId(focusThingId), "this");
    }
    if (selectedRef.current && graph.hasNode(selectedRef.current)) {
      // A picked thing keeps its lit set across a rebuild (a new night's
      // points arriving while it is open, UX-CHAT-5).
      const isThing = graph.getNodeAttribute(selectedRef.current, "entityType") === THING_NODE_TYPE;
      hoverStateRef.current = isThing
        ? thingFocusState(graph, selectedRef.current, thingStopRef.current)
        : hoverStateFor(graph, selectedRef.current);
      // A new link can bring 附近 in.
      if (isThing) setThingStops(thingStopsFor(graph, selectedRef.current));
      // Its points carry on out to their topics on a rebuild: still on the
      // way when it was just picked (an arrival's map is often built twice),
      // else there already.
      if (isThing && !arriving) fly(selectedRef.current, pickedAtRef.current + THING_CAMERA_MS);
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
    // New points grow in from the first painted frame on.
    if (growth) growthFrame = requestAnimationFrame(growStep);
    // Home's Replay starts 300 ms after the map's first paint.
    let replayTimer = 0;
    if (replayPendingRef.current) {
      replayTimer = window.setTimeout(() => {
        replayPendingRef.current = false;
        if (replayPress !== undefined) playedReplays.add(replayPress);
        startReplayRef.current();
      }, 300);
    }
    renderer.on("clickNode", ({ node }) => {
      // A moved drag must not also navigate on release.
      if (movedDuringPressRef.current) return;
      // A click on a point pins its sentence open (again unpins it), to read
      // it through or copy it.
      if (graph.getNodeAttribute(node, "night") === true) {
        pinnedPointRef.current = pinnedPointRef.current === node ? null : node;
        renderer.refresh();
        return;
      }
      focusNodeRef.current(node);
    });
    renderer.on("clickStage", ({ event }) => {
      if (pinnedPointRef.current) {
        pinnedPointRef.current = null;
        renderer.refresh();
        return;
      }
      // A sign at the screen's edge takes the camera to the far topic it
      // names, or back to the thing.
      const goTo = edgeSignAt(frameHitsRef.current, event.x, event.y);
      if (goTo) {
        goToSignRef.current(goTo);
        return;
      }
      // A click on a frame or its title picks its thing, at its own stop.
      const thing = frameAt(frameHitsRef.current, event.x, event.y);
      if (thing) {
        selectThingRef.current(thing, "this");
        return;
      }
      if (selectedRef.current) returnToMapRef.current();
    });
    // Hover is LOCKED while a drag is live: our drag doesn't capture the
    // pointer (sigma's captor keeps picking), so sweeping the grabbed node
    // across other hit areas would fire enter/leave mid-drag — the graph
    // dims/undims, labels pop, and the cursor flickers grabbing→pointer→
    // default. force-graph never shows this (d3-drag captures the pointer,
    // hover is inert mid-drag), and the flashing reads as jank.
    let overNode = false;
    renderer.on("enterNode", ({ node }) => {
      overNode = true;
      pointerNode = node;
      if (draggedNodeRef.current || selectedRef.current) {
        // A point's sentence still comes up under the pointer.
        if (graph.getNodeAttribute(node, "night") === true) renderer.refresh();
        return;
      }
      hoverStateRef.current = hoverStateFor(graph, node);
      container.style.cursor = "pointer";
      renderer.refresh();
    });
    renderer.on("leaveNode", () => {
      overNode = false;
      const left = pointerNode;
      pointerNode = null;
      if (draggedNodeRef.current || selectedRef.current) {
        if (left !== null && graph.hasNode(left) && graph.getNodeAttribute(left, "night") === true) renderer.refresh();
        return;
      }
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
      // Touching the map ends Replay at once, the camera left where it is.
      if (replayRef.current) endReplayRef.current("stay");
      // Freeze the camera frame so dragging a boundary node doesn't re-fit it.
      if (!renderer.getCustomBBox()) renderer.setCustomBBox(renderer.getBBox());
    });
    mouseCaptor.on("mousemovebody", (e) => {
      const draggedNode = draggedNodeRef.current;
      if (!draggedNode) {
        // A frame and its title answer a click, so they show the hand.
        if (!overNode) container.style.cursor = frameAt(frameHitsRef.current, e.x, e.y) || edgeSignAt(frameHitsRef.current, e.x, e.y) ? "pointer" : "default";
        return;
      }
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
      if (replayRef.current) endReplayRef.current("stay");
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
      cancelAnimationFrame(growthFrame);
      window.clearTimeout(replayTimer);
      placeGrowth(Infinity);
      growth = null;
      growRef.current = () => {};
      cancelAnimationFrame(flightFrame);
      flyRef.current = () => {};
      // Sigma only detaches its own camera listener; release our listener too.
      camera.off("updated", onCameraUpdated);
      window.removeEventListener("pagehide", keepOnHide);
      if (rearrangeRef.current) {
        // The next build lays out fresh (or brings the old layout back);
        // keeping this one would undo the person's choice.
        viewpointRef.current = null;
      } else {
        const body = bodyPositions();
        viewpointRef.current = {
          scope: spaceFilter,
          view: captureAtlasViewpoint(renderer, graph),
          body,
        };
        storePositions(spaceFilter, body);
      }
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
  }, [visibleModel, layoutVersion, mapReady, workerLayouts]);

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
    // An R2 map keeps the regions its layout drew.
    if (regionLayoutRef.current) return;
    communitiesRef.current = communities;
    // Region membership is derived from the communities map, so a status flip
    // invalidates the cached scene even though nothing moved.
    sceneDirtyRef.current = true;
    sigmaRef.current?.refresh();
  }, [communities]);

  useEffect(() => {
    const overlay = overlayRef.current;
    if (!overlay) return;
    overlay.style.display = showRegions || layers.memory || hasThings ? "" : "none";
    if (areasRef.current) areasRef.current.style.display = showRegions ? "" : "none";
    // The canvas keeps its last frame while hidden; repaint on re-show so it
    // matches wherever the camera and drags went in the meantime.
    if (showRegions) sigmaRef.current?.refresh();
  }, [showRegions, layers.memory, hasThings]);

  useEffect(() => {
    // Showing or hiding every active thing's points changes what is drawn,
    // so the cached scene and the labels laid round it are stale.
    focusLayoutRef.current = null;
    sceneDirtyRef.current = true;
    sigmaRef.current?.refresh();
  }, [showWorking]);

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

  // The things' points are waited for too (their read never fails, see
  // above), so the first layout and the opening frame already hold them.
  if (graphLoading || thingPointsPending) {
    return (
      <div data-testid="atlas-view" style={statusStyle}>
        <span className="entity-empty">{t("constellationMap.loading")}</span>
      </div>
    );
  }

  // Nothing to draw only when the daemon has neither entities NOR wiki pages:
  // a knowledge base made purely of pages is a real map, not an empty one.
  if (entities.length === 0 && graph.pages.length === 0 && !hasThings) {
    return (
      <div data-testid="atlas-view" style={statusStyle}>
        <TabEmptyMark tab="graph" />
        <span className="entity-empty">{t("constellationMap.empty")}</span>
        {/* The topic list still opens from here, even before the map has anything to draw. */}
        {onOpenTopics && (
          <button type="button" className="collection-control atlas-topics-link" onClick={onOpenTopics}>
            <TopicIcon size={16} aria-hidden="true" />
            <span>{t("atlas.allTopics")}</span>
          </button>
        )}
      </div>
    );
  }

  // Counts describe what is ON THE MAP at rest: layer on, and not tucked
  // away in a small group until zoomed in. A layer that is off contributes
  // nothing rather than a zero.
  const drawn = filteredModel.nodes.filter((node) => !tucked.has(node.id));
  const pageCount = drawn.filter((node) => node.kind === "page").length;
  const memoryCount = drawn.filter((node) => node.kind === "memory").length;
  const entityCount = drawn.filter((node) => node.kind === "entity").length;
  // A kind appears when its layer is on and it actually contributed nodes.
  // Entities are the exception and always report, zero included — the round-1
  // line did, and "0 entities" is the honest answer to an empty entity layer.
  const countLine = [
    ...(layers.page && pageCount > 0 ? [t("atlas.countPages", { count: pageCount })] : []),
    ...(layers.entity ? [t("atlas.countEntities", { count: entityCount })] : []),
    ...(layers.memory && memoryCount > 0
      ? [t("atlas.countMemories", { count: memoryCount })]
      : []),
    ...((laidRegionCount ?? regionCount) > 0 ? [t("atlas.countRegions", { count: laidRegionCount ?? regionCount })] : []),
  ].join(" · ");
  const dropdownOpen = searchFocused && query.trim().length > 0;
  // "Last night: 8 points written, 5 connected, 2 merged"; a part that is zero is left out.
  const nightOvernight = ranOvernight(tonight.map((thing) => thing.last_night?.at ?? 0));
  const nightSentence = t("atlas.night.sentence", {
    when: t(nightOvernight ? "atlas.night.lastNight" : "atlas.night.today"),
    what: [
      ...(nightCount.written > 0 ? [t("atlas.night.written", { count: nightCount.written })] : []),
      ...(nightCount.connected > 0 ? [t("atlas.night.connected", { count: nightCount.connected })] : []),
      ...(nightCount.merged > 0 ? [t("atlas.night.merged", { count: nightCount.merged })] : []),
      ...(nightCount.replaced > 0 ? [t("atlas.night.replaced", { count: nightCount.replaced })] : []),
      ...(nightCount.updated > 0 ? [t("atlas.night.updated", { count: nightCount.updated })] : []),
    ].join(t("atlas.night.joiner")),
  });
  const oldPassSentence = oldPass ? t("atlas.night.sentence", {
    when: t("atlas.night.oldPass"),
    what: [
      ...(oldPass.merged > 0 ? [t("atlas.night.merged", { count: oldPass.merged })] : []),
      ...(oldPass.updated > 0 ? [t("atlas.night.updated", { count: oldPass.updated })] : []),
    ].join(t("atlas.night.joiner")),
  }) : "";

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
              onFocus={() => {
                if (replayRef.current) endReplayRef.current("stay");
                setSearchFocused(true);
              }}
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
                          : node.entityType === THING_NODE_TYPE
                            ? palette.bridge
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
        {onOpenTopics && (
          <button type="button" className="collection-control atlas-display-trigger atlas-topics-link" aria-label={t("atlas.allTopics")} title={t("atlas.allTopics")} onClick={onOpenTopics}>
            <TopicIcon size={16} aria-hidden="true" />
            <span>{t("atlas.allTopics")}</span>
          </button>
        )}
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
          {/* What is laid over the map, in the same rows as what is drawn on it. */}
          <div className="atlas-display-group">
            <AtlasTooltip content={t("atlas.regionsDescription")}>
            <button
              type="button"
              className="atlas-content-toggle"
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
            {hasActiveThings && (
              <AtlasTooltip content={t("atlas.workingDescription")}>
              <button
                type="button"
                className="atlas-content-toggle"
                aria-pressed={showWorking}
                aria-label={t("atlas.workingToggle")}
                onClick={toggleWorking}
              >
                <span className="atlas-content-dot" aria-hidden="true" />
                <span>{t("atlas.workingToggle")}</span>
              </button>
              </AtlasTooltip>
            )}
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

      {replaying !== "idle" ? (
        // Replay says what it is doing and how to stop it (UX-GRAPH-6).
        <p className="atlas-night-line" role="status" data-testid="atlas-night-line">
          <SteepMoon />
          <span>{t(replaying === "marked" ? "atlas.night.marked" : "atlas.night.replaying")}</span>
          <span aria-hidden="true">·</span>
          <button type="button" onClick={() => endReplay("return")}>{t(replaying === "marked" ? "atlas.night.done" : "atlas.night.skip")}</button>
        </p>
      ) : nightShown && !selectedThing ? (
        // What last night did, one line above the count line (UX-GRAPH-5).
        <p className="atlas-night-line" data-testid="atlas-night-line" title={nightSentence}>
          {/* The sentence itself plays Replay; the line gains no extra word for it. */}
          <button type="button" className="atlas-night-replay" aria-label={t("atlas.night.replay")} onClick={startReplay}>
            <SteepMoon />
            <span className="atlas-night-line-full">{nightSentence}</span>
            <span className="atlas-night-line-short">{t(nightOvernight ? "atlas.night.lastNightShort" : "atlas.night.todayShort")}</span>
          </button>
          <span aria-hidden="true">·</span>
          <button type="button" disabled={undoNight.isPending} onClick={() => undoNight.mutate()}>{t("atlas.night.undo")}</button>
        </p>
      ) : undoNight.isError ? (
        <p className="atlas-night-line" role="alert">{t("atlas.night.undoFailed")}</p>
      ) : oldPassShown && !selectedThing ? (
        // What the one-time pass over old memories did, like a night's line
        // and in its place (UX-GRAPH-7); it has no Replay.
        <p className="atlas-night-line" data-testid="atlas-old-pass-line" title={oldPassSentence}>
          <SteepMoon />
          <span className="atlas-night-line-full">{oldPassSentence}</span>
          <span className="atlas-night-line-short">{t("atlas.night.oldPassShort")}</span>
          <span aria-hidden="true">·</span>
          <button type="button" disabled={undoOldPass.isPending} onClick={() => undoOldPass.mutate()}>{t("atlas.night.undo")}</button>
        </p>
      ) : undoOldPass.isError ? (
        <p className="atlas-night-line" role="alert">{t("atlas.night.oldPassUndoFailed")}</p>
      ) : null}

      {selectedThing && replaying === "idle" ? (
        // A picked thing's three zoom stops, in the camera controls' own
        // style, where the count line sits (UX-CHAT-5).
        <div className="atlas-camera-controls atlas-thing-stops" role="group" aria-label={t("atlas.thingStops")}>
          {thingStops.map((stop) => (
            <button
              key={stop}
              type="button"
              className="atlas-icon-button atlas-thing-stop"
              aria-pressed={thingStop === stop}
              onClick={() => selectThing(selectedThing, stop)}
            >
              {t(`atlas.thingStop.${stop}`)}
            </button>
          ))}
          {onNodeClick && (
            <button type="button" className="atlas-icon-button atlas-thing-stop" onClick={() => onNodeClick(targetForNode(selectedThing))}>
              {t("atlas.openDetails")}
              <ArrowRight size={14} aria-hidden="true" />
            </button>
          )}
        </div>
      ) : (
        <div className="atlas-count-line" aria-live="polite">{countLine}</div>
      )}

      {filteredModel.nodes.length === 0 && excludedTypes.size > 0 && <div className="atlas-filter-empty">
        <p>{t("atlas.noTypeMatches")}</p><button type="button" className="atlas-action" onClick={() => setExcludedTypes(new Set())}>{t("atlas.allEntityTypes")}</button>
      </div>}
      {selectedNode && <AtlasInspector key={selectedNode.id} node={selectedNode} neighbors={selectedNeighbors}
        edges={filteredModel.edges.filter((edge) => edge.source === selectedNode.id || edge.target === selectedNode.id)}
        onSelect={focusEntity} onClose={returnToMap}
        onOpen={onNodeClick ? () => onNodeClick(targetForNode(selectedNode.id)) : undefined}
        onOpenMindMap={onOpenMindMap && pageIdOf(selectedNode.id) !== null ? () => onOpenMindMap(pageIdOf(selectedNode.id)!) : undefined} />}

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
      {layingOut && (
        <div className="atlas-laying-out" role="status">{t("atlas.layingOut")}</div>
      )}
      {rebalanced && (
        <div className="atlas-rearrange-note" role="status" aria-label={t("atlas.rebalancedShort")}>
          <span className="atlas-note-long">{t("atlas.rebalanced")}</span>
          <span className="atlas-note-short">{t("atlas.rebalancedShort")}</span>
          <span aria-hidden="true">·</span>
          <button type="button" onClick={() => {
            rearrangeRef.current = "undo";
            setRebalanced(false);
            setLayoutVersion((version) => version + 1);
          }}>{t("atlas.putBack")}</button>
        </div>
      )}

    </div>
  );
}
